//! GitHub Copilot's project-local Markdown layouts.
//!
//! Mirrors the v1 provider's suffix filtering and recursion. Canonical paths
//! must stay under the declared source so linked instructions cannot silently
//! acquire a different filesystem authority.

use std::{
	collections::BTreeSet,
	fs,
	path::{Path, PathBuf},
};

use omp_core::Str;

use super::rules::Warning;

pub(super) fn contained(base: &Path, path: &Path) -> bool {
	let Ok(base) = base.canonicalize() else {
		return false;
	};
	path.canonicalize().is_ok_and(|path| path.starts_with(base))
}

pub(super) fn files(
	project: &Path,
	relative: &str,
	suffix: &str,
	recursive: bool,
	warnings: &mut Vec<Warning>,
) -> Vec<PathBuf> {
	let root = project.join(relative);
	if !root.exists() {
		return Vec::new();
	}
	if !contained(project, &root) {
		warnings.push(Warning {
			path:    root,
			message: Str::new_static("GitHub source resolves outside its discovery root"),
		});
		return Vec::new();
	}
	let mut out = Vec::new();
	let mut visited = BTreeSet::new();
	walk(&root, &root, suffix, recursive, &mut visited, &mut out, warnings);
	out
}

fn walk(
	root: &Path,
	dir: &Path,
	suffix: &str,
	recursive: bool,
	visited: &mut BTreeSet<PathBuf>,
	out: &mut Vec<PathBuf>,
	warnings: &mut Vec<Warning>,
) {
	let Ok(canonical) = dir.canonicalize() else {
		return;
	};
	if !visited.insert(canonical) {
		return;
	}
	let entries = match fs::read_dir(dir) {
		Ok(entries) => entries,
		Err(error) => {
			warnings.push(Warning {
				path:    dir.to_owned(),
				message: Str::new(format!("cannot read GitHub source: {error}")),
			});
			return;
		},
	};
	let mut paths = entries
		.filter_map(Result::ok)
		.map(|entry| entry.path())
		.collect::<Vec<_>>();
	paths.sort();
	for path in paths {
		let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
			continue;
		};
		if name.starts_with('.') {
			continue;
		}
		if !contained(root, &path) {
			warnings.push(Warning {
				path,
				message: Str::new_static("GitHub declaration resolves outside its discovery root"),
			});
			continue;
		}
		if path.is_file() && name.ends_with(suffix) {
			out.push(path);
		} else if recursive && path.is_dir() {
			walk(root, &path, suffix, recursive, visited, out, warnings);
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::discovery::{
		prompts::PromptTemplates,
		rules::{ActiveRules, ContextFiles, MAIN_AGENT},
		skills::{self, SkillPolicy},
	};

	fn write(root: &Path, relative: &str, body: &str) {
		let path = root.join(relative);
		fs::create_dir_all(path.parent().unwrap()).unwrap();
		fs::write(path, body).unwrap();
	}

	#[test]
	fn documented_foreign_provider_filesystem_inventory() {
		println!("| Provider | Capability | Evidence |");
		println!("|---|---|---|");
		for (provider, path, body, capability) in [
			("claude", ".claude/CLAUDE.md", "Claude fixture", "context"),
			("gemini", ".gemini/GEMINI.md", "Gemini fixture", "context"),
			("cursor", ".cursor/rules/cursor.mdc", "---\ndescription: Cursor\n---\nFixture", "rules"),
			(
				"windsurf",
				".windsurf/rules/windsurf.md",
				"---\ndescription: Windsurf\n---\nFixture",
				"rules",
			),
			("cline", ".clinerules", "Cline fixture", "rules"),
			(
				"github",
				".github/instructions/github.instructions.md",
				"---\napplyTo: '*.rs'\n---\nFixture",
				"rules",
			),
			(
				"codex",
				".codex/skills/codex/SKILL.md",
				"---\nname: codex\ndescription: Codex fixture\n---\nFixture",
				"skills",
			),
		] {
			let temp = tempfile::tempdir().unwrap();
			let project = temp.path().join("project");
			let home = temp.path().join("home");
			write(&project, path, body);
			let found = match capability {
				"context" => ContextFiles::discover(&project, &home, &home.join(".o2"))
					.files
					.iter()
					.any(|file| file.provider == provider),
				"rules" => ActiveRules::discover(&project, &home, &home.join(".o2"))
					.rules
					.iter()
					.any(|rule| rule.provider == provider),
				"skills" => {
					let policy = SkillPolicy::default();
					skills::discover(
						&skills::sources(&project, &home, &home.join(".o2"), &policy),
						&policy,
					)
					.skills
					.iter()
					.any(|skill| skill.provider == provider)
				},
				_ => unreachable!("fixture capability"),
			};
			assert!(found, "{provider} no longer discovers {path}");
			println!("| {provider} | {capability} | discovered |");
		}
	}

	#[test]
	fn github_instruction_scopes_are_retained_and_not_injected_globally() {
		let temp = tempfile::tempdir().unwrap();
		let project = temp.path().join("project");
		let home = temp.path().join("home");
		write(
			&project,
			".github/instructions/deep/types.instructions.md",
			"---\napplyTo: ['**/*.ts, **/*.tsx', 'src/*.js']\n---\nTypeScript guidance\n",
		);
		write(
			&project,
			".github/instructions/global.instructions.md",
			"---\napplyTo: '**/*'\n---\nGlobal guidance\n",
		);
		write(&project, ".github/instructions/missing.instructions.md", "No scope\n");
		write(&project, ".github/instructions/notes.md", "Not instructions\n");
		write(
			&project,
			".github/instructions/broken.instructions.md",
			"---\napplyTo: [\n---\nBroken\n",
		);
		let rules = ActiveRules::discover(&project, &home, &home.join(".o2"));
		let rule = rules.get("types").unwrap();
		assert_eq!(rule.provider, "github");
		assert_eq!(rule.globs.iter().map(Str::as_str).collect::<Vec<_>>(), [
			"**/*.ts", "**/*.tsx", "src/*.js"
		]);
		assert_eq!(rule.content, "TypeScript guidance\n");
		assert!(!rule.always_apply);
		let facts = rules.prompt_facts(MAIN_AGENT);
		assert!(
			facts
				.rulebook
				.iter()
				.any(|row| row["name"] == "types" && row["globs"][0] == "**/*.ts")
		);
		assert!(
			!facts
				.always_apply
				.iter()
				.any(|row| row["name"] == "types" || row["name"] == "missing")
		);
		assert!(facts.always_apply.iter().any(|row| row["name"] == "global"));
		assert!(rules.get("notes").is_none());
		assert!(rules.get("broken").is_none());
		assert!(
			rules
				.warnings
				.iter()
				.any(|warning| warning.message.contains("Missing applyTo"))
		);
		assert!(
			rules
				.warnings
				.iter()
				.any(|warning| warning.message.contains("frontmatter"))
		);
	}

	#[test]
	fn github_context_and_rule_collisions_respect_existing_precedence() {
		let temp = tempfile::tempdir().unwrap();
		let project = temp.path().join("project");
		let home = temp.path().join("home");
		write(&project, ".github/copilot-instructions.md", "Copilot project");
		write(&project, "AGENTS.md", "Lower precedence");
		write(&home, ".copilot/copilot-instructions.md", "Copilot user");
		let context = ContextFiles::discover(&project, &home, &home.join(".o2"));
		assert_eq!(
			context
				.files
				.iter()
				.map(|file| file.content.as_str())
				.collect::<Vec<_>>(),
			["Copilot user", "Copilot project"]
		);
		write(&project, ".omp/AGENTS.md", "Native project");
		write(&project, ".omp/rules/style.md", "---\ndescription: native\n---\nNative rule");
		write(
			&project,
			".github/instructions/style.instructions.md",
			"---\napplyTo: '**/*.ts'\n---\nForeign rule",
		);
		let context = ContextFiles::discover(&project, &home, &home.join(".o2"));
		assert_eq!(context.files.last().unwrap().content, "Native project");
		let rules = ActiveRules::discover(&project, &home, &home.join(".o2"));
		assert_eq!(rules.get("style").unwrap().provider, "native");
		assert!(
			rules
				.warnings
				.iter()
				.any(|warning| warning.message.contains("collision"))
		);
	}

	#[test]
	fn github_prompts_and_skills_load_native_names_and_policy() {
		let temp = tempfile::tempdir().unwrap();
		let project = temp.path().join("project");
		let home = temp.path().join("home");
		write(
			&project,
			".github/prompts/review.prompt.md",
			"---\nname: inspect\n---\nInspect $ARGUMENTS",
		);
		write(&project, ".github/prompts/notes.md", "Not a command");
		write(&project, ".github/prompts/deep/nested.prompt.md", "Not a direct prompt");
		write(
			&project,
			".github/skills/review/SKILL.md",
			"---\nname: review\ndescription: Review safely\n---\nRead the diff",
		);
		write(
			&project,
			".github/skills/invalid/SKILL.md",
			"---\nname: invalid\n---\nMissing description",
		);
		let prompts = PromptTemplates::discover(&project, &home.join(".o2"), &[], true);
		assert_eq!(prompts.templates.len(), 1);
		assert_eq!(prompts.get("inspect").unwrap().content, "Inspect $ARGUMENTS");
		assert!(prompts.get("review.prompt").is_none());
		assert!(
			PromptTemplates::discover(&project, &home.join(".o2"), &[], false)
				.templates
				.is_empty()
		);
		let policy = SkillPolicy::default();
		let sources = skills::sources(&project, &home, &home.join(".o2"), &policy);
		let found = skills::discover(&sources, &policy);
		assert_eq!(found.skills.len(), 1);
		assert_eq!(found.skills[0].provider, "github");
		assert_eq!(found.skills[0].body, "Read the diff");
		let disabled =
			SkillPolicy { disabled: [Str::new_static("review")].into_iter().collect(), ..policy };
		assert!(skills::discover(&sources, &disabled).skills.is_empty());
		write(&project, ".omp/prompts/inspect.md", "Native command");
		assert_eq!(
			PromptTemplates::discover(&project, &home.join(".o2"), &[], true)
				.get("inspect")
				.unwrap()
				.content,
			"Native command"
		);
	}

	#[cfg(unix)]
	#[test]
	fn github_linked_escapes_and_cycles_do_not_admit_foreign_files() {
		use std::os::unix::fs::symlink;
		let temp = tempfile::tempdir().unwrap();
		let project = temp.path().join("project");
		let outside = temp.path().join("outside");
		write(
			&project,
			".github/instructions/inside.instructions.md",
			"---\napplyTo: '*.rs'\n---\nInside",
		);
		write(&outside, "secret.instructions.md", "---\napplyTo: '**'\n---\nOutside");
		symlink(
			outside.join("secret.instructions.md"),
			project.join(".github/instructions/escape.instructions.md"),
		)
		.unwrap();
		symlink(project.join(".github/instructions"), project.join(".github/instructions/cycle"))
			.unwrap();
		let home = temp.path().join("home");
		let rules = ActiveRules::discover(&project, &home, &home.join(".o2"));
		assert!(rules.get("inside").is_some());
		assert!(rules.get("escape").is_none());
		assert!(
			rules
				.warnings
				.iter()
				.any(|warning| warning.message.contains("outside its discovery root"))
		);
		write(&outside, "stolen.prompt.md", "Do not admit");
		symlink(&outside, project.join(".github/prompts")).unwrap();
		assert!(
			PromptTemplates::discover(&project, &home.join(".o2"), &[], true)
				.templates
				.is_empty()
		);
		write(
			&outside,
			"review/SKILL.md",
			"---\nname: review\ndescription: Outside\n---\nDo not admit",
		);
		symlink(&outside, project.join(".github/skills")).unwrap();
		let policy = SkillPolicy::default();
		assert!(
			skills::discover(&skills::sources(&project, &home, &home.join(".o2"), &policy), &policy)
				.skills
				.is_empty()
		);
	}
}
