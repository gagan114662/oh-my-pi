//! Journal-owned agent-settlement continuations.

use std::collections::BTreeMap;

use omp_core::Str;
use omp_dom::{KnownTag, NodeSpec, Op, PropId, PropKey, Txn, Value};
use serde::{Deserialize, Serialize};

use crate::{Session, SessionError};

/// Projection marker retaining a collapsed continuation only in history.
pub const COLLAPSED_PROP: &str = "omp/continuation-collapsed";
pub(crate) const OWNER_PROP: &str = "omp/continuation-owner";
const LEDGER_PROP: &str = "omp/continuation-ledger";
/// Documented Core ceiling for consecutive settlement continuations.
pub const CONTINUATION_CAP: u64 = 8;

/// Python Continue wire role, validated before journaling.
#[derive(
	Clone,
	Copy,
	Debug,
	Default,
	Eq,
	PartialEq,
	Serialize,
	Deserialize,
	strum::Display,
	strum::EnumString,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ContinuationRole {
	/// Provider-compatible user continuation.
	User,
	/// Mid-thread system continuation.
	#[default]
	System,
}

/// Exact Python Continue payload plus its authenticated host owner.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Continuation {
	/// Authenticated extension owner attached by the host.
	pub owner:          Str,
	/// Next model instruction.
	pub prompt:         Str,
	/// Transcript visibility.
	#[serde(default)]
	pub visible:        bool,
	/// Model-facing role.
	#[serde(default)]
	pub role:           ContinuationRole,
	/// Optional telemetry label.
	#[serde(default)]
	pub label:          Option<Str>,
	/// Keep only this owner's newest unpinned continuation runnable.
	#[serde(default = "default_collapse")]
	pub collapse_prior: bool,
}
const fn default_collapse() -> bool {
	true
}

#[derive(Default, Serialize, Deserialize)]
struct Ledger {
	consecutive: u64,
	total:       u64,
	refusals:    u64,
	last_user:   Option<Str>,
}

impl Session {
	/// Commits a continuation and its cap debit together. A refusal is also
	/// durable, so a restart cannot turn exhaustion into a fresh allowance.
	pub fn continue_from_settlement(
		&mut self,
		continuation: &Continuation,
	) -> Result<bool, SessionError> {
		let dom = self.dom();
		let meta = dom.meta();
		let prop = PropKey::Custom(Str::new_static(LEDGER_PROP));
		let mut ledgers: BTreeMap<Str, Ledger> = match dom.get(meta).and_then(|node| node.prop(&prop))
		{
			None => BTreeMap::new(),
			Some(Value::Json(raw)) => serde_json::from_str(raw.get())?,
			Some(_) => return Err(SessionError::InvalidContinuationState),
		};
		let owner_prop = PropKey::Custom(Str::new_static(OWNER_PROP));
		let last_user = dom
			.select("user")
			.map_err(|_| SessionError::InvalidContinuationState)?
			.filter_map(|handle| dom.get(handle))
			.filter(|node| node.prop(&owner_prop).is_none())
			.filter_map(|node| {
				node
					.prop(&PropId::Id.into())
					.and_then(Value::as_str)
					.map(Str::new)
			})
			.last();
		let ledger = ledgers.entry(continuation.owner.clone()).or_default();
		if ledger.last_user != last_user {
			ledger.consecutive = 0;
			ledger.last_user = last_user;
		}
		let accepted = ledger.consecutive < CONTINUATION_CAP;
		if accepted {
			ledger.consecutive += 1;
			ledger.total = ledger.total.saturating_add(1);
		} else {
			ledger.refusals = ledger.refusals.saturating_add(1);
		}
		let turn = *dom
			.children(dom.body())
			.last()
			.ok_or(SessionError::NoActiveTurn)?;
		let cause = self.head().ok_or(SessionError::NoActiveTurn)?;
		let mut ops = Vec::new();
		if accepted && continuation.collapse_prior {
			let pins =
				crate::context::context_pins(dom).map_err(|_| SessionError::InvalidContextPins)?;
			for tag in ["user", "developer"] {
				for handle in dom
					.select(tag)
					.map_err(|_| SessionError::InvalidContinuationState)?
				{
					let Some(node) = dom.get(handle) else {
						continue;
					};
					if node.prop(&owner_prop).and_then(Value::as_str)
						!= Some(continuation.owner.as_str())
					{
						continue;
					}
					let entry = node.prop(&PropId::Id.into()).and_then(Value::as_str);
					if entry.is_some_and(|entry| pins.contains_key(&Str::new(format!("{entry}:0")))) {
						continue;
					}
					ops.push(Op::Set {
						h:     handle,
						prop:  PropKey::Custom(Str::new_static(COLLAPSED_PROP)),
						value: Value::Bool(true),
					});
				}
			}
		}
		let node = if accepted {
			let tag = match continuation.role {
				ContinuationRole::User => KnownTag::User,
				ContinuationRole::System => KnownTag::Developer,
			};
			let mut node = NodeSpec::new(tag)
				.with_content(continuation.prompt.clone())
				.with_prop(owner_prop, Value::Str(continuation.owner.clone()))
				.with_prop(
					PropKey::Custom(Str::new_static(crate::custom_message::DISPLAY_PROP)),
					Value::Bool(continuation.visible),
				);
			if let Some(label) = &continuation.label {
				node = node.with_prop(PropId::Name, Value::Str(label.clone()));
			}
			node
		} else {
			NodeSpec::new(KnownTag::Notice)
				.with_prop(PropId::Kind, Value::Str(Str::new_static("warn")))
				.with_prop(PropId::Name, Value::Str(Str::new_static("continuation-cap")))
				.with_content(Str::new_static(
					"Settlement continuation refused: consecutive continuation cap reached",
				))
		};
		ops.push(Op::Ins { parent: turn, after: dom.children(turn).last().copied(), node });
		ops.push(Op::Set {
			h: meta,
			prop,
			value: Value::Json(serde_json::value::to_raw_value(&ledgers)?),
		});
		self.patch(Txn { cause, label: Some(Str::new_static("agent.settled-continuation")), ops })?;
		Ok(accepted)
	}
}
