//! The product's linked convar registry must agree with its settings roster.

use omp_chat::overlays::settings::settings_group_coverage;

#[test]
fn every_product_settings_group_has_renderable_bindings_in_both_directions() {
	// The same link-time registry consumed by the application, without loading
	// ambient user cfg files or resolving dynamic choices through services.
	let con = omp_con::Ctx::new();
	let _ = omp_app::settings::CL_UPDATE_CHANNEL.get(&con);
	let coverage = settings_group_coverage(&con);
	assert!(!coverage.is_empty());
	let missing: Vec<_> = coverage
		.iter()
		.filter(|group| !group.is_complete())
		.collect();

	// Write all observations before the assertion, including on a parent or
	// deliberately mutated source revision. Export names only, never values.
	if let Some(path) = std::env::var_os("OMP_SETTINGS_ROSTER_EVIDENCE_DIR") {
		let path = std::path::PathBuf::from(path);
		std::fs::create_dir_all(&path).expect("create settings evidence directory");
		std::fs::write(
			path.join("settings-roster.json"),
			serde_json::to_vec_pretty(&coverage).unwrap(),
		)
		.unwrap();
		let mut summary = String::from("# Linked product settings coverage\n");
		let advertised: Vec<_> = coverage.iter().filter(|group| group.advertised).collect();
		let unique: std::collections::BTreeSet<_> = advertised
			.iter()
			.map(|group| group.group.as_str())
			.collect();
		{
			use std::fmt::Write as _;
			writeln!(
				summary,
				"\nObserved advertised roster: {} tab/group pairs, {} unique group names.\n",
				advertised.len(),
				unique.len()
			)
			.unwrap();
		}
		summary.push_str(
			"| Tab | Group | Advertised | Renderable convars | Rejected convars | Complete \
			 |\n|---|---|---|---|---|---|\n",
		);
		for group in &coverage {
			use std::fmt::Write as _;
			let convars = group
				.convars
				.iter()
				.map(|name| name.as_str())
				.collect::<Vec<_>>()
				.join(", ");
			let rejected = group
				.unrenderable
				.iter()
				.map(|name| name.as_str())
				.collect::<Vec<_>>()
				.join(", ");
			writeln!(
				summary,
				"| {} | {} | {} | {} | {} | {} |",
				group.tab,
				group.group,
				group.advertised,
				convars,
				rejected,
				group.is_complete()
			)
			.unwrap();
		}
		std::fs::write(path.join("settings-roster.md"), summary).unwrap();
	}
	assert!(
		missing.is_empty(),
		"settings declarations diverge from the actual UI roster: {missing:#?}"
	);
}
