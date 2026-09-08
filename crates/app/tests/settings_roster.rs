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

#[test]
fn production_approval_and_fallback_maps_roundtrip_exact_values() {
	let con = omp_con::Ctx::new();
	for (name, source) in [
		("sv_tools_approval", "{bash deny read allow write prompt}"),
		("ai_retry_fallback_chains", "{default [openai/gpt-4o-mini anthropic/claude-sonnet-4]}"),
	] {
		con.run(&format!("{name} {source}"))
			.expect("production map validator accepts fixture");
		let original = con.get(name).expect("registered production setting");
		let statements = omp_con::parse(&omp_core::sf!("setting {original}")).expect("script map");
		let [statement] = statements.as_slice() else {
			panic!("single statement")
		};
		let [_, arg @ omp_con::Arg::Kv(_)] = statement.args.as_slice() else {
			panic!("one map")
		};
		assert_eq!(omp_con::coerce_one(arg, &omp_con::TypeSpec::KV).expect("typed map"), original);
		con.run(&format!("{name} {}", arg.to_script()))
			.expect("production validator retained");
		assert_eq!(con.get(name), Some(original));
	}
}
