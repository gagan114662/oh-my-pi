//! Dead-loop detection for one turn (#124).
//!
//! A dead loop is the model issuing the same tool calls, receiving the same
//! results, and producing no visible text, round after round. The guard sees
//! two things per round: the last exchange as the model is about to see it
//! (previous calls and their results, from the projected request) and the
//! calls the model just issued. After `limit` identical executions the next
//! identical round is refused; after `limit` refusals the turn settles.
//! Different arguments or different results are progress and reset it.

use std::{
	collections::hash_map::DefaultHasher,
	hash::{Hash, Hasher},
};

use omp_ai::{ContentPart, Message, Role};

use crate::dispatch::{PreparedCall, call_target};

/// What the guard decided about the round the model just issued.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Verdict {
	/// Execute the calls.
	Proceed,
	/// Refuse the calls: `repeat` refusals so far; `settle` ends the turn.
	Refuse { repeat: u32, settle: bool },
}

/// Per-turn loop state. `limit == 0` disables the guard.
#[derive(Debug)]
pub(crate) struct LoopGuard {
	limit:            u32,
	last_exchange:    Option<u64>,
	results_repeat:   bool,
	last_calls:       Option<u64>,
	identical_rounds: u32,
	refused_rounds:   u32,
}

impl LoopGuard {
	pub(crate) const fn new(limit: u32) -> Self {
		Self {
			limit,
			last_exchange: None,
			results_repeat: false,
			last_calls: None,
			identical_rounds: 0,
			refused_rounds: 0,
		}
	}

	pub(crate) const fn limit(&self) -> u32 {
		self.limit
	}

	/// Records the exchange the model is about to see; called once per
	/// request, after the request is final.
	pub(crate) fn observe_request(&mut self, messages: &[Message]) {
		let exchange = exchange_fingerprint(messages);
		self.results_repeat = exchange.is_some() && exchange == self.last_exchange;
		self.last_exchange = exchange;
	}

	/// Judges the calls the model issued for this round. `calls` is
	/// [`calls_fingerprint`] of the round, `None` when it issued none;
	/// `textual_progress` is whether it also produced visible text.
	pub(crate) fn judge(&mut self, calls: Option<u64>, textual_progress: bool) -> Verdict {
		let same_calls = calls.is_some() && calls == self.last_calls;
		self.last_calls = calls;
		if self.limit == 0 || !same_calls || textual_progress {
			self.identical_rounds = 0;
			self.refused_rounds = 0;
			return Verdict::Proceed;
		}
		// `identical_rounds` counts consecutive rounds whose calls AND
		// results matched the round before; two more executions than that
		// count have run, so refusal starts once `limit` have executed.
		if self.refused_rounds > 0 || self.identical_rounds.saturating_add(2) >= self.limit {
			self.refused_rounds = self.refused_rounds.saturating_add(1);
			return Verdict::Refuse {
				repeat: self.refused_rounds,
				settle: self.refused_rounds >= self.limit,
			};
		}
		if self.results_repeat {
			self.identical_rounds = self.identical_rounds.saturating_add(1);
		} else {
			self.identical_rounds = 0;
		}
		Verdict::Proceed
	}
}

/// Stable fingerprint of a round's tool calls: target and arguments, in
/// order, ignoring call ids.
pub(crate) fn calls_fingerprint(calls: &[PreparedCall]) -> u64 {
	let mut hasher = DefaultHasher::new();
	for call in calls {
		call_target(call).hash(&mut hasher);
		call.args().map(|args| args.get()).hash(&mut hasher);
	}
	hasher.finish()
}

/// Fingerprint of the last exchange in a projected request: the newest
/// assistant message and every message after it (its tool results), by
/// semantic content only. `None` when no assistant message exists yet.
fn exchange_fingerprint(messages: &[Message]) -> Option<u64> {
	let start = messages
		.iter()
		.rposition(|message| message.role == Role::Assistant)?;
	let mut hasher = DefaultHasher::new();
	for message in &messages[start..] {
		format!("{:?}", message.role).hash(&mut hasher);
		for part in message.content.iter() {
			match part {
				ContentPart::Text { text, .. } => ("text", text.as_str()).hash(&mut hasher),
				ContentPart::Reasoning { text, .. } => ("reasoning", text.as_str()).hash(&mut hasher),
				ContentPart::ToolCall { name, arguments, .. } => {
					("call", name.as_str(), arguments.0.to_string()).hash(&mut hasher);
				},
				ContentPart::ToolResult { name, content, is_error, .. } => {
					("result", name.as_deref(), *is_error, format!("{content:?}")).hash(&mut hasher);
				},
				other => format!("{other:?}").hash(&mut hasher),
			}
		}
	}
	Some(hasher.finish())
}

#[cfg(test)]
mod tests {
	use std::sync::Arc;

	use omp_core::Str;

	use super::*;

	fn text(role: Role, text: &str) -> Message {
		Message {
			role,
			content: Arc::from([ContentPart::Text { text: Str::new(text), proof: None }]),
			name: None,
		}
	}

	/// A round whose exchange repeats the previous one.
	fn repeated_exchange(guard: &mut LoopGuard) {
		guard.observe_request(&[
			text(Role::User, "go"),
			text(Role::Assistant, "same"),
			text(Role::User, "result"),
		]);
	}

	#[test]
	fn identical_rounds_execute_limit_times_then_refuse_then_settle() {
		let mut guard = LoopGuard::new(3);
		let calls = Some(7);
		let mut verdicts = Vec::new();
		for _ in 0..6 {
			repeated_exchange(&mut guard);
			verdicts.push(guard.judge(calls, false));
		}
		assert_eq!(verdicts, [
			Verdict::Proceed,
			Verdict::Proceed,
			Verdict::Proceed,
			Verdict::Refuse { repeat: 1, settle: false },
			Verdict::Refuse { repeat: 2, settle: false },
			Verdict::Refuse { repeat: 3, settle: true },
		]);
	}

	#[test]
	fn different_calls_or_visible_text_reset_the_streak() {
		let mut guard = LoopGuard::new(2);
		repeated_exchange(&mut guard);
		assert_eq!(guard.judge(Some(1), false), Verdict::Proceed);
		repeated_exchange(&mut guard);
		assert_eq!(guard.judge(Some(1), false), Verdict::Proceed);
		repeated_exchange(&mut guard);
		assert_eq!(guard.judge(Some(1), true), Verdict::Proceed, "text is progress");
		repeated_exchange(&mut guard);
		assert_eq!(guard.judge(Some(2), false), Verdict::Proceed, "new arguments are progress");
	}

	#[test]
	fn changing_results_are_progress_even_with_identical_calls() {
		let mut guard = LoopGuard::new(2);
		for page in 0..6 {
			guard.observe_request(&[
				text(Role::Assistant, "same call"),
				text(Role::User, &format!("page {page}")),
			]);
			assert_eq!(guard.judge(Some(9), false), Verdict::Proceed, "round {page}");
		}
	}

	#[test]
	fn a_zero_limit_disables_the_guard() {
		let mut guard = LoopGuard::new(0);
		for _ in 0..50 {
			repeated_exchange(&mut guard);
			assert_eq!(guard.judge(Some(4), false), Verdict::Proceed);
		}
	}

	#[test]
	fn exchange_fingerprint_needs_an_assistant_message_and_reads_content_only() {
		assert_eq!(exchange_fingerprint(&[text(Role::User, "only a prompt")]), None);
		let a = exchange_fingerprint(&[text(Role::Assistant, "x"), text(Role::User, "r")]);
		let b = exchange_fingerprint(&[
			text(Role::User, "earlier"),
			text(Role::Assistant, "x"),
			text(Role::User, "r"),
		]);
		let c = exchange_fingerprint(&[text(Role::Assistant, "x"), text(Role::User, "different")]);
		assert_eq!(a, b, "history before the last exchange does not matter");
		assert_ne!(a, c, "a different result is a different exchange");
	}
}
