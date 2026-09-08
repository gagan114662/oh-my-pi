//! Every settings group the overlay advertises binds at least one convar,
//! and every convar that carries UI metadata names an advertised group
//! (#55). This lives in the app crate so every crate's convars are linked.

use omp_chat::overlays::settings::{advertised_groups, group_is_bound, unadvertised_bindings};

#[test]
fn every_advertised_settings_group_binds_a_convar() {
	let con = omp_con::Ctx::new();
	let missing = advertised_groups()
		.into_iter()
		.filter(|(tab, group)| !group_is_bound(&con, tab, group))
		.collect::<Vec<_>>();
	assert!(missing.is_empty(), "advertised settings groups with no bound convar row: {missing:?}");
}

#[test]
fn every_ui_bound_convar_names_an_advertised_group() {
	let con = omp_con::Ctx::new();
	let stray = unadvertised_bindings(&con);
	assert!(stray.is_empty(), "convars whose ui.tab/ui.group the overlay never shows: {stray:?}");
}
