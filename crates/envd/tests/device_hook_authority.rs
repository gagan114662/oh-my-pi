//! Proves dynamic device generation fencing and exact cancellation of queued
//! hook callbacks.
use std::{
	sync::Arc,
	time::{Duration, Instant},
};

use omp_core::{CowBytes, Str, sf};
use omp_envd::{
	exthost::{CallbackConcurrency, DispatchError, DispatchRequest, DispatchRouter, EventDeadline},
	worker::HostKey,
};
use omp_tool::{
	AvailabilityDelta, LeafOwner, LeafReplacementError, LeafReplacementRegistry, LeafVersion,
	RegistryLeaf, Rev,
};

fn leaf(name: &str) -> RegistryLeaf<Str> {
	RegistryLeaf {
		name:  Str::new(name),
		rev:   Rev { family: sf!("integration"), n: 1 },
		code:  omp_core::Hash32::new([7; 32]),
		value: Arc::new(Str::new(name)),
	}
}

#[test]
fn dynamic_device_catalog_transitions_are_atomic_and_generation_fenced() {
	let catalog = LeafReplacementRegistry::new();
	let owner = LeafOwner { root: sf!("dynamic"), claimant: sf!("acme/devices") };
	catalog
		.replace(owner.clone(), LeafVersion { manager_generation: 7, definition_epoch: 1 }, vec![
			leaf("dynamic/echo"),
			leaf("dynamic/search"),
		])
		.expect("sealed mount");
	let epoch = catalog
		.set_availability_many(7, &[
			(owner.clone(), AvailabilityDelta {
				name:    sf!("dynamic/echo"),
				mounted: false,
				reason:  Some(sf!("maintenance")),
			}),
			(owner.clone(), AvailabilityDelta {
				name:    sf!("dynamic/search"),
				mounted: false,
				reason:  None,
			}),
		])
		.expect("one availability transition");
	let snapshot = catalog.snapshot();
	assert_eq!(snapshot.epoch, epoch);
	assert!(snapshot.leaves.iter().all(|leaf| !leaf.mounted));
	assert_eq!(snapshot.leaves[0].reason.as_deref(), Some("maintenance"));
	assert_eq!(
		catalog
			.set_availability_many(6, &[(owner, AvailabilityDelta {
				name:    sf!("dynamic/echo"),
				mounted: true,
				reason:  None,
			},)])
			.expect_err("stale host is fenced"),
		LeafReplacementError::Generation { expected: 7, actual: 6 },
	);
}

#[tokio::test]
async fn queued_hook_callback_cancellation_is_exact() {
	let host = HostKey::new(sf!("project"), sf!("trusted"), sf!("acme/hooks"));
	let mut router = DispatchRouter::new(host, 7);
	let request = |id| DispatchRequest {
		id,
		policy: CallbackConcurrency::Serialized,
		deadline: EventDeadline { at: Instant::now() + Duration::from_secs(5) },
		payload: CowBytes::from(Vec::new()),
	};
	let (ready, _running) = router
		.dispatch(sf!("acme/hooks"), request(1))
		.expect("running");
	assert!(ready.is_some());
	let (ready, queued) = router
		.dispatch(sf!("acme/hooks"), request(2))
		.expect("queued");
	assert!(ready.is_none());
	assert!(
		router
			.cancel_queued("acme/hooks", 2)
			.expect("cancel queued")
	);
	assert_eq!(queued.response().await, Err(DispatchError::Cancelled));
}

#[tokio::test]
async fn completing_one_concurrent_callback_does_not_admit_a_queued_serialized_callback() {
	let mut router = DispatchRouter::new(HostKey::new("project", "trusted", "hooks"), 7);
	let request = |id, policy| DispatchRequest {
		id,
		policy,
		deadline: EventDeadline { at: Instant::now() + Duration::from_secs(5) },
		payload: CowBytes::from(Vec::new()),
	};
	for id in [1, 2] {
		let (ready, _) = router
			.dispatch("hooks", request(id, CallbackConcurrency::Concurrent { limit: 2 }))
			.expect("concurrent");
		assert!(ready.is_some());
	}
	let (ready, queued) = router
		.dispatch("hooks", request(3, CallbackConcurrency::Serialized))
		.expect("queued");
	assert!(ready.is_none());
	assert!(
		router
			.complete("hooks", 1, 7, Ok(CowBytes::from(Vec::new())))
			.expect("first completes")
			.is_none(),
		"second callback still occupies the serialized actor"
	);
	let ready = router
		.complete("hooks", 2, 7, Ok(CowBytes::from(Vec::new())))
		.expect("second completes")
		.expect("now admitted");
	assert_eq!(ready.id, 3);
	router
		.complete("hooks", 3, 7, Ok(CowBytes::from(Vec::new())))
		.expect("third completes");
	assert!(queued.response().await.is_ok());
}

#[tokio::test]
async fn nested_callback_reentry_excludes_unrelated_and_sibling_callbacks() {
	let mut router = DispatchRouter::new(HostKey::new("project", "trusted", "hooks"), 7);
	let request = |id| DispatchRequest {
		id,
		policy: CallbackConcurrency::Serialized,
		deadline: EventDeadline { at: Instant::now() + Duration::from_secs(5) },
		payload: CowBytes::from(Vec::new()),
	};
	assert!(
		router
			.dispatch("hooks", request(1))
			.expect("parent")
			.0
			.is_some()
	);
	assert!(
		router
			.dispatch("hooks", request(2))
			.expect("unrelated")
			.0
			.is_none()
	);
	assert!(
		matches!(
			router.dispatch_nested("hooks", request(3), 1),
			Err(DispatchError::InvalidParent(1))
		),
		"running alone is not a reentry permit"
	);
	router
		.begin_control_wait("hooks", 1)
		.expect("authenticated wait");
	assert!(
		router
			.dispatch_nested("hooks", request(3), 1)
			.expect("descendant")
			.0
			.is_some()
	);
	assert!(
		router
			.dispatch_nested("hooks", request(4), 1)
			.expect("sibling")
			.0
			.is_none(),
		"sibling cannot overlap serialized child"
	);
	let next = router
		.complete("hooks", 3, 7, Ok(CowBytes::from(Vec::new())))
		.expect("child complete")
		.expect("sibling admitted");
	assert_eq!(next.id, 4, "unrelated callback remains blocked by parent");
	assert!(
		router
			.complete("hooks", 4, 7, Ok(CowBytes::from(Vec::new())))
			.expect("sibling complete")
			.is_none()
	);
	router.end_control_wait("hooks", 1);
	assert!(
		matches!(
			router.dispatch_nested("hooks", request(5), 1),
			Err(DispatchError::InvalidParent(1))
		),
		"completed CONTROL wait grants no reentry"
	);
	assert_eq!(
		router
			.complete("hooks", 1, 7, Ok(CowBytes::from(Vec::new())))
			.expect("parent complete")
			.expect("unrelated admitted")
			.id,
		2
	);
}
