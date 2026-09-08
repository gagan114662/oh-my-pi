//! Per-account transient-failure breaker (#119).
//!
//! Without it, every caller in the process runs its own retry ladder against
//! an endpoint that is down; with subagents that is a retry storm. The
//! breaker counts consecutive transient failures per account; once `failures`
//! are seen it opens for a window, doubling up to `max` while probes keep
//! failing. While open, every attempt on the account is told to wait for the
//! window; when it expires exactly one caller is admitted as the probe and
//! the others wait a short grace, so an outage costs one request per window.
//! A success closes it.

use std::time::{Duration, SystemTime};

/// Thresholds; see `RetrySettings::breaker_policy`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BreakerPolicy {
	/// Consecutive transient failures that open the breaker; zero disables it.
	pub failures:    u32,
	/// First open window.
	pub base:        Duration,
	/// Largest open window; windows double up to this while probes fail.
	pub max:         Duration,
	/// How long non-probe callers wait while a probe is in flight.
	pub probe_grace: Duration,
}

impl BreakerPolicy {
	/// A breaker that never opens.
	pub const DISABLED: Self = Self {
		failures:    0,
		base:        Duration::ZERO,
		max:         Duration::ZERO,
		probe_grace: Duration::ZERO,
	};
}

/// What an attempt on the account may do right now.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Admission {
	/// The breaker is closed; proceed normally.
	Closed,
	/// The window expired and this caller is the one probe; proceed.
	Probe,
	/// Wait until `until`, then ask again.
	Open { until: SystemTime },
}

/// Breaker state for one account.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Breaker {
	consecutive_failures: u32,
	window:               Option<Duration>,
	open_until:           Option<SystemTime>,
	probe_in_flight:      bool,
}

impl Breaker {
	/// Decides whether an attempt may start at `now`.
	pub fn admission(&mut self, now: SystemTime, policy: &BreakerPolicy) -> Admission {
		let Some(until) = self.open_until else {
			return Admission::Closed;
		};
		if until > now {
			return Admission::Open { until };
		}
		if self.probe_in_flight {
			return Admission::Open { until: now + policy.probe_grace };
		}
		self.probe_in_flight = true;
		Admission::Probe
	}

	/// Records a transient failure; returns the window end when the breaker
	/// opened or re-opened.
	pub fn record_failure(&mut self, now: SystemTime, policy: &BreakerPolicy) -> Option<SystemTime> {
		if policy.failures == 0 {
			return None;
		}
		self.consecutive_failures = self.consecutive_failures.saturating_add(1);
		let window = match self.window {
			// A failed probe doubles the window, up to the cap.
			Some(window) if self.probe_in_flight => {
				window.saturating_mul(2).min(policy.max.max(policy.base))
			},
			Some(window) => window,
			None if self.consecutive_failures >= policy.failures => policy.base,
			None => return None,
		};
		self.probe_in_flight = false;
		self.window = Some(window);
		let until = now + window;
		self.open_until = Some(until);
		Some(until)
	}

	/// Records a success: the breaker closes and forgets its window.
	pub fn record_success(&mut self) {
		*self = Self::default();
	}

	/// Whether the breaker is currently open or probing.
	#[must_use]
	pub const fn is_open(&self) -> bool {
		self.open_until.is_some()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const POLICY: BreakerPolicy = BreakerPolicy {
		failures:    3,
		base:        Duration::from_secs(30),
		max:         Duration::from_secs(120),
		probe_grace: Duration::from_secs(5),
	};

	fn at(seconds: u64) -> SystemTime {
		SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
	}

	#[test]
	fn opens_after_the_failure_threshold_and_waits_for_the_window() {
		let mut breaker = Breaker::default();
		assert_eq!(breaker.record_failure(at(0), &POLICY), None);
		assert_eq!(breaker.record_failure(at(1), &POLICY), None);
		assert_eq!(breaker.admission(at(1), &POLICY), Admission::Closed);
		assert_eq!(breaker.record_failure(at(2), &POLICY), Some(at(32)));
		assert_eq!(breaker.admission(at(10), &POLICY), Admission::Open { until: at(32) });
	}

	#[test]
	fn one_probe_per_window_and_a_failed_probe_doubles_it_up_to_the_cap() {
		let mut breaker = Breaker::default();
		for second in 0..3 {
			breaker.record_failure(at(second), &POLICY);
		}
		assert_eq!(breaker.admission(at(40), &POLICY), Admission::Probe);
		assert_eq!(
			breaker.admission(at(41), &POLICY),
			Admission::Open { until: at(46) },
			"a second caller waits the probe grace"
		);
		assert_eq!(breaker.record_failure(at(42), &POLICY), Some(at(102)), "30 s doubles to 60 s");
		assert_eq!(breaker.admission(at(110), &POLICY), Admission::Probe);
		assert_eq!(breaker.record_failure(at(111), &POLICY), Some(at(231)), "60 s doubles to 120 s");
		assert_eq!(breaker.admission(at(240), &POLICY), Admission::Probe);
		assert_eq!(breaker.record_failure(at(241), &POLICY), Some(at(361)), "capped at 120 s");
	}

	#[test]
	fn a_success_closes_the_breaker_and_resets_the_window() {
		let mut breaker = Breaker::default();
		for second in 0..3 {
			breaker.record_failure(at(second), &POLICY);
		}
		assert!(breaker.is_open());
		assert_eq!(breaker.admission(at(40), &POLICY), Admission::Probe);
		breaker.record_success();
		assert!(!breaker.is_open());
		assert_eq!(breaker.admission(at(41), &POLICY), Admission::Closed);
		// The next outage starts from the base window again.
		for second in 50..53 {
			breaker.record_failure(at(second), &POLICY);
		}
		assert_eq!(breaker.admission(at(60), &POLICY), Admission::Open { until: at(82) });
	}

	#[test]
	fn a_zero_threshold_never_opens() {
		let mut breaker = Breaker::default();
		for second in 0..50 {
			assert_eq!(breaker.record_failure(at(second), &BreakerPolicy::DISABLED), None);
		}
		assert_eq!(breaker.admission(at(60), &BreakerPolicy::DISABLED), Admission::Closed);
	}
}
