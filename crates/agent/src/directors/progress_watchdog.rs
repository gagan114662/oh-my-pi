//! Journal-progress Director and its disposable cancellation observer.

use std::{
	sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
	},
	time::{Duration, Instant},
};

use omp_core::Str;
use omp_dom::Node;
use omp_session::{JournalProgress, Session};
use tokio::sync::watch;

use crate::director::{BindValue, Director, state_int};

/// Registered family of the mandatory active-turn liveness policy.
pub const FAMILY: &str = "progress_watchdog";

/// Settles an active turn when durable non-stream journal progress stops.
/// The engagement records policy; observation state is rebuilt each turn.
pub struct ProgressWatchdog {
	limit: Duration,
}

impl ProgressWatchdog {
	/// Constructs the policy from the host's effective idle convar.
	pub const fn new(limit: Duration) -> Self {
		Self { limit }
	}

	/// Reconstructs policy from the normal journal-derived Director element.
	pub fn from_node(node: &Node) -> Self {
		let millis = state_int(node, "idle_ms")
			.and_then(|value| u64::try_from(value).ok())
			.unwrap_or(30 * 60 * 1000);
		Self::new(Duration::from_millis(millis))
	}

	pub(crate) fn arm(&self, session: &Session) -> IdleWatchdog {
		IdleWatchdog(Arc::new(Observer {
			progress:   session.journal_progress(),
			phase:      watch::channel(Phase { suspended: false, resumed_at: Instant::now() }).0,
			settlement: watch::channel(false).0,
			limit:      self.limit,
			fired:      AtomicBool::new(false),
		}))
	}
}

impl Director for ProgressWatchdog {
	fn id(&self) -> &str {
		FAMILY
	}

	fn state(&self) -> Vec<(Str, BindValue)> {
		vec![(
			Str::new_static("idle_ms"),
			BindValue::Int(i64::try_from(self.limit.as_millis()).unwrap_or(i64::MAX)),
		)]
	}
}

#[derive(Clone, Copy, Debug)]
struct Phase {
	suspended:  bool,
	resumed_at: Instant,
}

#[derive(Debug)]
struct Observer {
	progress:   watch::Receiver<JournalProgress>,
	phase:      watch::Sender<Phase>,
	settlement: watch::Sender<bool>,
	limit:      Duration,
	fired:      AtomicBool,
}

/// Disposable reader; its only progress input is the session commit projection.
#[derive(Clone, Debug)]
pub(crate) struct IdleWatchdog(Arc<Observer>);

impl IdleWatchdog {
	pub(crate) fn fired(&self) -> bool {
		self.0.fired.load(Ordering::Acquire)
	}

	fn deadline(&self) -> Option<Instant> {
		let phase = *self.0.phase.borrow();
		(!phase.suspended)
			.then(|| self.0.progress.borrow().observed_at.max(phase.resumed_at) + self.0.limit)
	}

	pub(crate) fn expired(&self) -> bool {
		if self
			.deadline()
			.is_some_and(|deadline| Instant::now() >= deadline)
		{
			self.0.fired.store(true, Ordering::Release);
		}
		self.fired()
	}

	pub(crate) async fn wait(&self) {
		let mut progress = self.0.progress.clone();
		let mut phase = self.0.phase.subscribe();
		loop {
			if self.expired() {
				return;
			}
			let deadline = self.deadline();
			tokio::select! {
				_ = async {
					if let Some(deadline) = deadline {
						tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
					} else { std::future::pending::<()>().await; }
				} => {},
				changed = progress.changed() => { if changed.is_err() { return; } },
				changed = phase.changed() => { if changed.is_err() { return; } },
			}
		}
	}

	/// Protects the dispatcher's ownership of call tasks and uncertain-effects
	/// settlement from an outer future drop. This does not suspend the idle
	/// deadline or cancellation; the dispatcher receives both normally.
	pub(crate) fn settling_calls(&self) -> CallSettlement {
		self.0.settlement.send_replace(true);
		CallSettlement(self.clone())
	}

	pub(crate) async fn calls_settled(&self) {
		let mut state = self.0.settlement.subscribe();
		loop {
			let pending = *state.borrow_and_update();
			if !pending || state.changed().await.is_err() {
				return;
			}
		}
	}

	pub(crate) fn suspend(&self) -> IdlePause {
		self.0.phase.send_modify(|phase| phase.suspended = true);
		IdlePause(self.clone())
	}
}

/// Restores observation on every exit from a safe-point pause, including
/// errors.
pub(crate) struct IdlePause(IdleWatchdog);
impl Drop for IdlePause {
	fn drop(&mut self) {
		self
			.0
			.0
			.phase
			.send_replace(Phase { suspended: false, resumed_at: Instant::now() });
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn explicit_pause_suspends_idle_and_resume_gets_a_new_interval() {
		let directory = tempfile::tempdir().unwrap();
		let session = Session::create(
			directory.path().join("pause.oms"),
			omp_session::ComponentRegistry::standard(),
		)
		.unwrap();
		let watchdog = ProgressWatchdog::new(Duration::from_millis(50)).arm(&session);
		let pause = watchdog.suspend();
		assert!(
			tokio::time::timeout(Duration::from_millis(80), watchdog.wait())
				.await
				.is_err()
		);
		assert!(!watchdog.fired());
		drop(pause);
		assert!(!watchdog.expired());
		tokio::time::timeout(Duration::from_secs(1), watchdog.wait())
			.await
			.unwrap();
		assert!(watchdog.fired());
	}
}

/// Releases the dispatcher's exclusive settlement phase on every exit.
pub(crate) struct CallSettlement(IdleWatchdog);
impl Drop for CallSettlement {
	fn drop(&mut self) {
		self.0.0.settlement.send_replace(false);
	}
}
