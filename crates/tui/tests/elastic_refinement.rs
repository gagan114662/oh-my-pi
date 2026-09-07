//! Replay original TLC state traces through the production Slots API and
//! renderer.
//!
//! This is a bounded correspondence check, not full refinement: Wide means
//! unwrapped rows; model Create/Admit maps to one Slots::open at admission;
//! BeginFlush is a scheduler stutter because Slots::plan already offers final
//! rows. The abstract emitted counter is zero once a block commits, whereas
//! Rust retains its delivered-row count for bookkeeping. Resize, queued layout,
//! failed writes and repaired finalized snapshots are outside this profile.

use omp_tui::{
	Size, frame_text,
	slots::{BlockId, BlockState, Delivered, Mode, ResizePolicy, Slots},
};
use serde::Deserialize;

#[derive(Debug, Deserialize, PartialEq)]
struct TaggedRow {
	owner: u32,
	row:   String,
}
#[derive(Deserialize)]
struct Step {
	action:   String,
	phase:    Vec<String>,
	mode:     Vec<String>,
	want:     Vec<Vec<String>>,
	emitted:  Vec<usize>,
	frontier: usize,
	history:  Vec<TaggedRow>,
}
#[derive(Deserialize)]
struct Case {
	name:   String,
	height: u16,
	steps:  Vec<Step>,
}
#[derive(Deserialize)]
struct Traces {
	cases: Vec<Case>,
}

fn rows(slots: &Slots) -> Vec<TaggedRow> {
	slots
		.logical_history()
		.map(|row| TaggedRow { owner: row.block().get(), row: row.text().to_owned() })
		.collect()
}
fn argument(action: &str) -> usize {
	action
		.split_once('(')
		.expect("parameterized action")
		.1
		.split([',', ')'])
		.next()
		.expect("block")
		.parse::<usize>()
		.expect("block number")
		- 1
}

#[test]
fn tlc_traces_refine_public_slots_history_lifecycle_and_rendered_delivery() {
	let traces: Traces = serde_json::from_str(include_str!("fixtures/elastic-model-traces.json"))
		.expect("TLC-generated traces");
	assert_eq!(traces.cases.len(), 2, "bounded profile inventory");
	for case in traces.cases {
		let mut slots = Slots::new(32, case.height, ResizePolicy::Preserve);
		let mut ids: Vec<BlockId> = Vec::new();
		for step in case.steps {
			let before = rows(&slots);
			let action = step.action.as_str();
			if action.starts_with("Admit(") {
				let index = argument(action);
				assert_eq!(index, ids.len(), "FIFO admission mapping");
				ids.push(slots.open(if step.mode[index] == "Mutable" {
					Mode::Mutable
				} else {
					Mode::AppendOnly
				}));
			} else if action.starts_with("Update(") {
				let index = argument(action);
				let text = step.want[index].join("\n");
				if step.mode[index] == "Mutable" {
					slots.set(ids[index], text.as_str());
				} else {
					slots.append(ids[index], &format!("{text}\n"));
				}
			} else if action.starts_with("FinalizeActive(") {
				slots.finalize(ids[argument(action)]);
			} else if action == "AppendStable" || action.starts_with("RetireSuccess(") {
				let plan = slots.plan();
				assert_eq!(rows(&slots), before, "{} {action}: planning must not commit", case.name);
				let delivered = plan
					.rows()
					.iter()
					.map(|row| {
						assert_eq!(
							frame_text(row.frame()).trim_end(),
							row.logical().text(),
							"real rendered row agrees with semantic row"
						);
						TaggedRow {
							owner: row.logical().block().get(),
							row:   row.logical().text().to_owned(),
						}
					})
					.collect::<Vec<_>>();
				assert_eq!(
					delivered,
					step.history[before.len()..],
					"{} {action}: model delivery batch",
					case.name
				);
				slots.commit(plan, Delivered::All);
			} else {
				assert!(
					action == "Init" || action == "BeginFlush" || action.starts_with("Create("),
					"unmapped model action {action}"
				);
			}
			assert_eq!(
				rows(&slots),
				step.history,
				"{} after {action}: exact model history",
				case.name
			);
			let mut frontier = 0;
			for (index, id) in ids.iter().enumerate() {
				let actual = slots.state(*id);
				let expected = match step.phase[index].as_str() {
					"Active" => BlockState::Active,
					"Finalized" => BlockState::Finalized,
					"Committed" => BlockState::Committed,
					other => panic!("unmapped admitted model phase {other}"),
				};
				assert_eq!(actual, expected, "{} {action}: block {} lifecycle", case.name, index + 1);
				if actual == BlockState::Committed {
					frontier += 1;
				}
				let emitted = if actual == BlockState::Committed {
					0
				} else {
					slots.emitted(*id)
				};
				assert_eq!(emitted, step.emitted[index], "{} {action}: abstract emitted", case.name);
			}
			assert_eq!(frontier, step.frontier, "{} {action}: model frontier", case.name);
			let plan = slots.plan();
			assert_eq!(plan.viewport().size(), Size::new(32, case.height));
			assert_eq!(rows(&slots), step.history, "viewport render must not alter history");
			println!("| {} | {action} | {} | {frontier} | matched |", case.name, step.history.len());
		}
	}
}
