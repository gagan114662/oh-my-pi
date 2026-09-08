//! Default palette resolution and retained-frame evidence for issue #24.

use std::{fs, sync::Arc};

use omp_chat::settings::{CL_THEME, CL_THEME_DARK, CL_THEME_LIGHT};
use omp_tui::{Appearance, Prop, ThemeCatalog, Ui, UiContext, components::TextLeaf};

#[test]
fn default_convars_resolve_and_paint_named_palettes() {
	let temporary = tempfile::tempdir().unwrap();
	let output = std::env::var_os("OMP_THEME_PROOF_DIR")
		.map(std::path::PathBuf::from)
		.unwrap_or_else(|| temporary.path().to_path_buf());
	fs::create_dir_all(&output).unwrap();
	let catalog = ThemeCatalog::load(&[], &[]).unwrap();
	let ctx = omp_con::Ctx::new();
	let mut rows = Vec::new();
	let mut failures = Vec::new();
	for (convar, name) in [
		("cl_theme", CL_THEME.get(&ctx)),
		("cl_theme_dark", CL_THEME_DARK.get(&ctx)),
		("cl_theme_light", CL_THEME_LIGHT.get(&ctx)),
	] {
		let palette = catalog.get(&name);
		for (appearance_name, appearance) in
			[("dark", Appearance::Dark), ("light", Appearance::Light)]
		{
			let Some(palette) = &palette else {
				let reason = format!("default {convar}={name} does not resolve; rendering skipped");
				failures.push(reason.clone());
				rows.push(serde_json::json!({
					"convar": convar, "name": name.as_str(), "source": "missing",
					"appearance": appearance_name, "resolved": false, "passed": false,
					"accent": null, "painted_accent": null,
					"frame_ansi": null, "frame_png": null, "render_status": reason,
				}));
				continue;
			};
			let context = UiContext { appearance, ..UiContext::default() }
				.with_palette(Some(Arc::clone(palette)));
			let expected = context.theme.accent;
			let ui = Ui::from_root(
				TextLeaf::new()
					.with(Prop::Fg, "accent")
					.text("Default theme sample"),
				40,
				context,
			);
			let painted = ui.frame().cell(0, 0).style().fg;
			let color_matches = painted == expected;
			if !color_matches {
				failures.push(format!(
					"{convar}/{appearance_name}: expected accent {expected:?}, painted {painted:?}"
				));
			}
			let stem = format!("{convar}-{appearance_name}");
			fs::write(output.join(format!("{stem}.ansi")), omp_tui::frame_ansi(ui.frame())).unwrap();
			let (png_path, render_status) = match omp_tui::frame_png(ui.frame()) {
				Ok(png) => {
					let path = format!("{stem}.png");
					fs::write(output.join(&path), png).unwrap();
					(Some(path), "rendered".to_owned())
				},
				Err(error) => {
					let reason = format!("PNG rendering failed: {error}");
					failures.push(format!("{convar}/{appearance_name}: {reason}"));
					(None, reason)
				},
			};
			let passed = color_matches && png_path.is_some();
			rows.push(serde_json::json!({
				"convar": convar, "name": name.as_str(), "source": "built-in",
				"appearance": appearance_name, "resolved": true, "passed": passed,
				"accent": format!("{expected:?}"), "painted_accent": format!("{painted:?}"),
				"frame_ansi": format!("{stem}.ansi"), "frame_png": png_path, "render_status": render_status,
			}));
		}
	}
	fs::write(output.join("themes.json"), serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
	let mut table = String::from(
		"| Convar | Default | Source | Appearance | Resolved | Expected accent | Painted accent | \
		 Render | Result |\n| --- | --- | --- | --- | --- | --- | --- | --- | --- |\n",
	);
	for row in &rows {
		use std::fmt::Write as _;
		writeln!(
			table,
			"| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
			row["convar"].as_str().unwrap(),
			row["name"].as_str().unwrap(),
			row["source"].as_str().unwrap(),
			row["appearance"].as_str().unwrap(),
			row["resolved"].as_bool().unwrap(),
			row["accent"].as_str().unwrap_or("not available"),
			row["painted_accent"].as_str().unwrap_or("not rendered"),
			row["render_status"].as_str().unwrap(),
			if row["passed"] == true {
				"PASS"
			} else {
				"FAIL"
			},
		)
		.unwrap();
	}
	table.push_str(
		"\nPNG uses the existing monochrome debug frame encoder: it proves content, not color. ANSI \
		 frames and asserted cell styles retain color evidence. This is a retained-component \
		 render, not a live Settings/PTY interaction proof.\n",
	);
	fs::write(output.join("summary.md"), table).unwrap();
	assert!(failures.is_empty(), "theme acceptance failures: {failures:#?}");
}

#[test]
fn theme_symbols_cannot_be_accepted_without_effect() {
	for source in
		[r#"{"symbols":{}}"#, r#"{"symbols":null}"#, r#"{"symbols":{"spinnerFrames":["."]}}"#]
	{
		assert!(
			omp_tui::JsonTheme::parse(source).is_err(),
			"unsupported symbols must reject: {source}"
		);
	}
}
