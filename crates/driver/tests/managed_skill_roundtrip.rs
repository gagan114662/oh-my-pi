//! Serialized managed declarations reloaded through the production skill
//! parser.

use std::{fs, path::Path};

use omp_core::Str;
use omp_driver::discovery::skills::{SkillLevel, SkillPolicy, SkillSource, discover};
use omp_envd::managed_skills_domain::ManagedSkillCandidate;

fn write_candidate(root: &Path, name: &str, description: &str, body: &str) {
	let candidate = ManagedSkillCandidate::new(name, description, body).expect("valid candidate");
	let directory = root.join(candidate.name.as_str());
	fs::create_dir_all(&directory).expect("candidate directory");
	fs::write(directory.join("SKILL.md"), candidate.serialize().as_bytes()).expect("candidate file");
}

#[test]
fn managed_serialization_roundtrips_yaml_scalar_names_and_replacements() {
	let tree = tempfile::tempdir().expect("temporary skill roots");
	let root = tree.path().join("managed");
	let sources = [SkillSource {
		provider: Str::from("omp-managed"),
		root:     root.clone(),
		level:    SkillLevel::User,
	}];
	let policy = SkillPolicy::default();
	for name in ["null", "true", "false", "123", "001", "1e3", "0xff", "ordinary-skill"] {
		for (description, body) in [
			("A user's scalar-name skill", "Original instructions."),
			("Updated user's skill", "Updated instructions.\n\nKeep this paragraph."),
		] {
			write_candidate(&root, name, description, body);
			let loaded = discover(&sources, &policy);
			assert!(loaded.warnings.is_empty(), "{name}: {:?}", loaded.warnings);
			let skill = loaded
				.get(name)
				.expect("published skill must reload by exact name");
			assert_eq!(skill.name.as_str(), name);
			assert_eq!(skill.description.as_str(), description);
			assert_eq!(skill.body.as_str(), body);
			assert!(
				loaded
					.prompt_facts()
					.iter()
					.any(|fact| fact["name"] == name)
			);
		}
	}
}

#[test]
fn authored_skill_keeps_precedence_over_published_managed_skill() {
	let tree = tempfile::tempdir().expect("temporary skill roots");
	let managed = tree.path().join("managed");
	let authored = tree.path().join("authored");
	write_candidate(&managed, "same-name", "Managed description", "Managed instructions.");
	fs::create_dir_all(authored.join("same-name")).expect("authored directory");
	fs::write(
		authored.join("same-name/SKILL.md"),
		"---\nname: same-name\ndescription: Authored description\n---\nAuthored instructions.\n",
	)
	.expect("authored declaration");
	let loaded = discover(
		&[
			SkillSource {
				provider: Str::from("native"),
				root:     authored,
				level:    SkillLevel::Project,
			},
			SkillSource {
				provider: Str::from("omp-managed"),
				root:     managed,
				level:    SkillLevel::User,
			},
		],
		&SkillPolicy::default(),
	);
	let skill = loaded.get("same-name").expect("authored skill");
	assert_eq!(skill.provider.as_str(), "native");
	assert_eq!(skill.body.as_str(), "Authored instructions.");
}
