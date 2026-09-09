//! Read-only liveness projection of successful durable journal appends.

use std::time::Instant;

use omp_journal::{EntryId, KindName};
use tokio::sync::watch;

/// Latest durable non-stream entry observed by this session owner.
///
/// The timestamp is monotonic host observation time, not a persisted clock.
/// Reopening establishes a fresh observation epoch; it never invents progress
/// entries or counts time while no process owns an active turn.
#[derive(Clone, Copy, Debug)]
pub struct JournalProgress {
	/// Durable entry responsible for progress, absent before genesis.
	pub entry:       Option<EntryId>,
	/// Monotonic time at which the durable append was observed.
	pub observed_at: Instant,
}

pub(crate) fn channel() -> watch::Sender<JournalProgress> {
	watch::channel(JournalProgress { entry: None, observed_at: Instant::now() }).0
}

pub(crate) fn committed(
	progress: &watch::Sender<JournalProgress>,
	kind: KindName,
	entry: EntryId,
	observed_at: Instant,
) {
	if kind != KindName::Stream {
		progress.send_replace(JournalProgress { entry: Some(entry), observed_at });
	}
}

#[cfg(test)]
mod tests {
	use std::time::Duration;

	use super::*;

	#[test]
	fn session_stream_appends_and_reopen_keep_the_last_non_stream_entry_identity() {
		let directory = tempfile::tempdir().unwrap();
		let path = directory.path().join("progress.oms");
		let mut session =
			crate::Session::create(&path, crate::ComponentRegistry::standard()).unwrap();
		session.begin_turn().unwrap();
		session.user("start", Vec::new()).unwrap();
		session
			.assistant_start("model", "provider", "route")
			.unwrap();
		let expected = *session.journal_progress().borrow();
		let assistant = session
			.dom()
			.select("body turn assistant")
			.unwrap()
			.last()
			.unwrap();
		let stream = session
			.stream_open(assistant, omp_dom::PropId::Text.into())
			.unwrap();
		session.stream_append(stream, "   ").unwrap();
		assert_eq!(session.journal_progress().borrow().entry, expected.entry);
		assert_eq!(session.journal_progress().borrow().observed_at, expected.observed_at);
		drop(session);
		let reopened = crate::Session::open(&path, crate::ComponentRegistry::standard()).unwrap();
		assert_eq!(reopened.journal_progress().borrow().entry, expected.entry);
		assert!(reopened.journal_progress().borrow().observed_at >= expected.observed_at);
	}

	#[test]
	fn injected_clock_stream_deltas_never_extend_the_progress_deadline() {
		let progress = channel();
		let mut observer = progress.subscribe();
		let start = Instant::now();
		let entry = EntryId::default();
		committed(&progress, KindName::TurnStart, entry, start);
		assert!(observer.has_changed().unwrap());
		observer.borrow_and_update();
		for minutes in 1..=31 {
			committed(&progress, KindName::Stream, entry, start + Duration::from_secs(minutes * 60));
		}
		assert!(!observer.has_changed().unwrap());
		assert_eq!(observer.borrow().observed_at, start);
		assert_eq!(observer.borrow().entry, Some(entry));
		committed(&progress, KindName::Patch, entry, start + Duration::from_secs(31 * 60));
		assert!(observer.has_changed().unwrap());
		assert_eq!(observer.borrow().observed_at, start + Duration::from_secs(31 * 60));
	}
}
