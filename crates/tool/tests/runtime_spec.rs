//! Runtime operation admission contracts shared by the host and Python surface.

use std::collections::BTreeSet;

use omp_core::InvocationPhase;
use omp_tool::{Authority, Durability, operation_spec, runtime_symbols};

#[test]
fn data_dispatch_keys_resolve_to_environment_effect_authority() {
	for operation in [
		"omp.env.Process.info",
		"omp.env.fs.privileged_mutation",
		"omp.env.workspace.list",
		"omp.env.worktree",
		"omp.env.mcp.status",
		"omp.env.mcp.subscribe",
		"omp.env.mcp.reset",
		"omp.env.mcp.live-header",
		"omp.env.mcp.resource",
		"omp.env.mcp.prompt",
		"omp.env.mcp.invoke",
		"omp.env.mcp.config",
	] {
		let spec = operation_spec(operation).expect("live DATA dispatch key must be registered");
		assert_eq!(spec.authority, Authority::Environment, "{operation}");
		assert_eq!(spec.minimum_phase, InvocationPhase::EffectsAuthorized, "{operation}");
		assert!(!InvocationPhase::Open.allows_operation(spec.minimum_phase));
		assert!(!InvocationPhase::Settled.allows_operation(spec.minimum_phase));
	}
	assert!(operation_spec("omp.env.mcp.invalid").is_none());
}

#[test]
fn declared_context_durability_does_not_grant_data_authority() {
	for operation in ["omp.context.pin", "omp.context.unpin", "omp.context.compact"] {
		let spec = operation_spec(operation).expect("context request has a spec");
		assert_eq!(spec.authority, Authority::Core);
		assert_eq!(spec.durability, Durability::Durable);
		assert_eq!(spec.minimum_phase, InvocationPhase::Open);
		assert!(!InvocationPhase::Settled.allows_operation(spec.minimum_phase));
	}
	let inference = operation_spec("omp.provider.request").expect("paid inference has a spec");
	assert_eq!(inference.minimum_phase, InvocationPhase::EffectsAuthorized);
	assert_eq!(inference.durability, Durability::Durable);
}

#[test]
fn control_metadata_keeps_owner_admission_and_lookup_keys_unambiguous() {
	for operation in ["omp.devices.dynamic_mount", "omp.devices.refresh", "omp.hooks.dispatch"] {
		assert_eq!(
			operation_spec(operation)
				.expect("active callback operation")
				.minimum_phase,
			InvocationPhase::EffectsAuthorized,
		);
	}
	for operation in ["omp.convars.get", "omp.ui.dynamic_mount", "omp.telemetry.span.open"] {
		assert_eq!(
			operation_spec(operation)
				.expect("early CONTROL operation")
				.minimum_phase,
			InvocationPhase::Open,
		);
	}
	let mut keys = BTreeSet::new();
	for row in runtime_symbols() {
		assert!(keys.insert(row.public_name), "duplicate symbol {}", row.public_name);
		if let Some(key) = row.dispatch_key {
			assert!(keys.insert(key), "duplicate dispatch key {key}");
			assert_eq!(operation_spec(key), operation_spec(row.public_name));
		}
	}
}
