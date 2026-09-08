//! Read-only Claude marketplace skill roots, selected by its installed
//! registry.

use std::{
	collections::BTreeMap,
	fs,
	path::{Path, PathBuf},
};

use omp_core::Str;
use serde::Deserialize;

use super::{
	github::contained,
	skills::{SkillLevel, SkillPolicy, SkillSource, agent_plugin_skill_source},
};

#[derive(Deserialize)]
struct Registry {
	version: u64,
	plugins: BTreeMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
	install_path: PathBuf,
	scope:        Option<String>,
	project_path: Option<PathBuf>,
	enabled:      Option<bool>,
}

fn json(path: &Path) -> Option<serde_json::Value> {
	serde_json::from_slice(&fs::read(path).ok()?).ok()
}

pub(super) fn sources(project: &Path, home: &Path, policy: &SkillPolicy) -> Vec<SkillSource> {
	let claude = home.join(".claude");
	let registry_path = claude.join("plugins/installed_plugins.json");
	if !contained(&claude, &registry_path) {
		return Vec::new();
	}
	let Some(registry) = fs::read(&registry_path)
		.ok()
		.and_then(|body| serde_json::from_slice::<Registry>(&body).ok())
	else {
		return Vec::new();
	};
	// The registry version is metadata; v1 accepts any numeric version.
	let _version = registry.version;
	let active = project
		.ancestors()
		.find(|dir| dir.join(".omp").is_dir())
		.or_else(|| project.ancestors().find(|dir| dir.join(".git").exists()))
		.unwrap_or(project);
	let mut enabled = BTreeMap::new();
	let mut settings = vec![claude.join("settings.json")];
	for dir in [active, project] {
		settings.extend([dir.join(".claude/settings.json"), dir.join(".claude/settings.local.json")]);
	}
	for path in settings {
		if let Some(value) = json(&path) {
			if let Some(overrides) = value
				.get("enabledPlugins")
				.and_then(serde_json::Value::as_object)
			{
				for (id, value) in overrides {
					if let Some(value) = value.as_bool() {
						enabled.insert(id.clone(), value);
					}
				}
			}
		}
	}
	let mut sources = Vec::new();
	for (id, entries) in registry.plugins {
		let Some((plugin, _marketplace)) = id.rsplit_once('@') else {
			continue;
		};
		let Some(entries) = entries.as_array() else {
			continue;
		};
		for entry in entries {
			let Ok(entry) = serde_json::from_value::<Entry>(entry.clone()) else {
				continue;
			};
			if entry.enabled == Some(false) || enabled.get(&id) == Some(&false) {
				continue;
			}
			let project_scope = matches!(entry.scope.as_deref(), Some("local" | "project"));
			if project_scope {
				if !policy.claude_project {
					continue;
				}
				if enabled.get(&id) != Some(&true)
					&& !entry.project_path.as_ref().is_some_and(|path| {
						path
							.canonicalize()
							.ok()
							.zip(active.canonicalize().ok())
							.is_some_and(|(left, right)| left == right)
					}) {
					continue;
				}
			} else if !policy.claude_user {
				continue;
			}
			// The registry explicitly authorizes the install path. Components must
			// remain inside it; no filesystem-wide cache/version scan is performed.
			let Ok(root) = entry.install_path.canonicalize() else {
				continue;
			};
			let level = if project_scope {
				SkillLevel::Project
			} else {
				SkillLevel::User
			};
			let standard_manifest = root.join("plugin.json");
			if standard_manifest.exists() {
				if !contained(&root, &standard_manifest) {
					continue;
				}
				if json(&standard_manifest).is_some_and(|value| {
					value
						.get("$schema")
						.and_then(serde_json::Value::as_str)
						.is_some_and(|schema| schema.contains("agent-plugins.org/"))
				}) {
					if let Some(source) = agent_plugin_skill_source(&root, level) {
						sources.push(source);
					}
					continue;
				}
			}
			let manifest_path = root.join(".claude-plugin/plugin.json");
			let manifest = contained(&root, &manifest_path)
				.then(|| json(&manifest_path))
				.flatten();
			let declared = manifest.as_ref().and_then(|value| value.get("skills"));
			let declared = match declared {
				Some(serde_json::Value::String(path)) => vec![path.as_str()],
				Some(serde_json::Value::Array(paths)) => {
					paths.iter().filter_map(serde_json::Value::as_str).collect()
				},
				_ => Vec::new(),
			}
			.into_iter()
			.map(str::trim)
			.filter(|path| !path.is_empty())
			.collect::<Vec<_>>();
			let marketplace = root.join("marketplace.json");
			let replaces = contained(&root, &marketplace)
				&& json(&marketplace)
					.and_then(|value| {
						value
							.get("plugins")
							.and_then(serde_json::Value::as_array)
							.cloned()
					})
					.is_some_and(|entries| {
						entries
							.iter()
							.any(|entry| entry["name"] == plugin && entry["source"] == "./")
					});
			let mut dirs = Vec::new();
			if declared.is_empty() || !replaces {
				dirs.push(root.join("skills"));
			}
			dirs.extend(declared.into_iter().map(|path| root.join(path)));
			for dir in dirs {
				if contained(&root, &dir)
					&& dir.is_dir()
					&& !sources.iter().any(|source| source.root == dir)
				{
					sources.push(SkillSource {
						provider: Str::new_static("claude-plugins"),
						root: dir,
						level,
					});
				}
			}
		}
	}
	sources
}

#[cfg(test)]
mod tests {
	use serde_json::json;

	use super::{
		super::skills::{self, discover},
		*,
	};

	fn write(root: &Path, path: &str, text: &str) {
		let path = root.join(path);
		fs::create_dir_all(path.parent().unwrap()).unwrap();
		fs::write(path, text).unwrap();
	}

	fn skill(root: &Path, path: &str, name: &str) {
		write(
			root,
			path,
			&format!("---\nname: {name}\ndescription: Real fixture\n---\nUse the registered skill"),
		);
	}

	#[test]
	fn installed_registry_scope_and_settings_select_marketplace_skills() {
		let temp = tempfile::tempdir().unwrap();
		let home = temp.path().join("home");
		let project = home.join("repo");
		fs::create_dir_all(project.join(".git")).unwrap();
		let cache = home.join(".claude/plugins/cache");
		let user = cache.join("market/user/v1");
		let local = cache.join("market/local/v2");
		let disabled = cache.join("market/disabled/v1");
		let stale = cache.join("market/unregistered/v9");
		skill(&user, "skills/user-skill/SKILL.md", "user-skill");
		skill(&local, "skills/local-skill/SKILL.md", "local-skill");
		skill(&disabled, "skills/disabled/SKILL.md", "disabled");
		skill(&stale, "skills/stale/SKILL.md", "stale");
		let registry = json!({"version":2,"plugins":{
			"user@market":[{"installPath":user,"scope":"user"}, {"bad":"entry"}],
			"local@market":[{"installPath":local,"scope":"local","projectPath":project}],
			"disabled@market":[{"installPath":disabled,"enabled":false}]
		}});
		write(&home, ".claude/plugins/installed_plugins.json", &registry.to_string());
		let policy = SkillPolicy::default();
		let found = discover(&sources(&project, &home, &policy), &policy);
		assert_eq!(
			found
				.skills
				.iter()
				.map(|skill| skill.name.as_str())
				.collect::<Vec<_>>(),
			["local-skill"]
		);
		let policy = SkillPolicy { claude_user: true, ..policy };
		let found = discover(&sources(&project, &home, &policy), &policy);
		assert_eq!(found.skills.len(), 2);
		assert!(
			!found
				.skills
				.iter()
				.any(|skill| skill.name == "disabled" || skill.name == "stale")
		);
		write(&home, ".claude/settings.json", r#"{"enabledPlugins":{"user@market":false}}"#);
		write(&project, ".claude/settings.json", r#"{"enabledPlugins":{"user@market":true}}"#);
		write(&project, ".claude/settings.local.json", r#"{"enabledPlugins":{"user@market":false}}"#);
		assert_eq!(
			discover(&sources(&project, &home, &policy), &policy)
				.skills
				.len(),
			1
		);
		let other = home.join("other");
		fs::create_dir_all(&other).unwrap();
		assert!(
			discover(&sources(&other, &home, &policy), &policy)
				.skills
				.is_empty()
		);
		write(&other, ".claude/settings.json", r#"{"enabledPlugins":{"local@market":true}}"#);
		assert_eq!(discover(&sources(&other, &home, &policy), &policy).skills[0].name, "local-skill");
		println!(
			"| claude-plugins | installed registry, scopes, overrides | discovered; \
			 disabled/unregistered rejected |"
		);
	}

	#[test]
	fn marketplace_declared_skills_add_to_default_and_opencode_user_wins() {
		let temp = tempfile::tempdir().unwrap();
		let home = temp.path().join("home");
		let project = home.join("repo");
		fs::create_dir_all(&project).unwrap();
		let root = home.join(".claude/plugins/cache/market/plugin/v1");
		skill(&root, "skills/default/SKILL.md", "default");
		skill(&root, "custom/SKILL.md", "custom");
		skill(&home, "outside/SKILL.md", "outside");
		write(
			&root,
			".claude-plugin/plugin.json",
			&json!({"skills":["./custom",home.join("outside")]}).to_string(),
		);
		write(
			&home,
			".claude/plugins/installed_plugins.json",
			&json!({"version":2,"plugins":{"plugin@market":[{"scope":"user","installPath":root}]}})
				.to_string(),
		);
		let policy = SkillPolicy { claude_user: true, ..SkillPolicy::default() };
		let found = discover(&sources(&project, &home, &policy), &policy);
		assert_eq!(
			found
				.skills
				.iter()
				.map(|skill| skill.name.as_str())
				.collect::<Vec<_>>(),
			["custom", "default"]
		);
		write(&root, "marketplace.json", r#"{"plugins":[{"name":"plugin","source":"./"}]}"#);
		assert_eq!(discover(&sources(&project, &home, &policy), &policy).skills[0].name, "custom");
		assert_eq!(
			discover(&sources(&project, &home, &policy), &policy)
				.skills
				.len(),
			1
		);
		skill(&home, ".config/opencode/skills/edit/SKILL.md", "edit");
		skill(&project, ".opencode/skills/edit/SKILL.md", "edit");
		let found = discover(&skills::sources(&project, &home, &home.join(".o2"), &policy), &policy);
		let edit = found
			.skills
			.iter()
			.find(|skill| skill.name == "edit")
			.unwrap();
		assert_eq!(edit.level, SkillLevel::User);
		assert_eq!(edit.provider, "opencode");
		println!("| opencode | user skill root | discovered before project collision |");
	}
	#[cfg(unix)]
	#[test]
	fn registered_plugin_components_cannot_follow_links_outside_the_install() {
		use std::os::unix::fs::symlink;
		let temp = tempfile::tempdir().unwrap();
		let home = temp.path().join("home");
		let project = home.join("repo");
		fs::create_dir_all(&project).unwrap();
		let root = home.join(".claude/plugins/cache/market/plugin/v1");
		skill(&home, "outside/secret/SKILL.md", "secret");
		fs::create_dir_all(&root).unwrap();
		symlink(home.join("outside"), root.join("skills")).unwrap();
		write(
			&home,
			".claude/plugins/installed_plugins.json",
			&json!({"version":2,"plugins":{"plugin@market":[{"scope":"user","installPath":root}]}})
				.to_string(),
		);
		let policy = SkillPolicy { claude_user: true, ..SkillPolicy::default() };
		assert!(
			discover(&sources(&project, &home, &policy), &policy)
				.skills
				.is_empty()
		);
	}
}
