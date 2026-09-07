//! Journal-backed, owner-scoped context pins.

use std::collections::BTreeMap;

use omp_core::Str;
use omp_dom::{Dom, Op, PropKey, Txn, Value};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Session, SessionError};

const PINS_PROP: &str = "omp/context-pins";
const RECEIPTS_PROP: &str = "omp/context-receipts";

/// Durable request identity and canonical argument digest supplied by CONTROL.
pub struct ContextRequestKey {
	/// Authenticated extension owner.
	pub owner:       Str,
	/// Caller idempotency key.
	pub key:         Str,
	/// Digest including the operation and normalized arguments.
	pub fingerprint: Str,
}

#[derive(Serialize, Deserialize)]
struct ContextReceipt {
	fingerprint: Str,
	count:       usize,
}

/// Durable provenance for one model-facing projection item.
#[derive(Clone, Debug)]
pub struct ContextOrigin {
	/// Stable item id, independent of the current projection position.
	pub id:            Str,
	/// Physical journal event index of the owning element.
	pub event:         u64,
	/// Owning turn entry identity, when present.
	pub turn_id:       Option<Str>,
	/// Creation time encoded in the authoritative entry ULID.
	pub created_at_ms: u64,
	/// Whether an extension has durably protected the item.
	pub pinned:        bool,
}

pub(crate) fn validate_compaction_pins(
	dom: &Dom,
	boundary: omp_journal::EntryId,
) -> Result<(), SessionError> {
	let pins = context_pins(dom).map_err(|_| SessionError::InvalidContextPins)?;
	for id in pins.keys() {
		let entry = id
			.rsplit_once(':')
			.and_then(|(entry, _)| entry.parse::<omp_journal::EntryId>().ok())
			.ok_or(SessionError::InvalidContextPins)?;
		let replaces_summary = dom.children(dom.meta()).iter().any(|handle| {
			dom.get(*handle).is_some_and(|node| {
				node.tag.as_str() == "compaction"
					&& node
						.prop(&omp_dom::PropId::Id.into())
						.and_then(Value::as_str)
						== Some(entry.to_string().as_str())
			})
		});
		if entry <= boundary || replaces_summary {
			return Err(SessionError::PinnedContext { id: id.clone() });
		}
	}
	Ok(())
}

/// One durable extension pin. Its budget charge is supplied by the agent's
/// model-facing context projection, never by the extension.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContextPin {
	/// Authenticated extension principal that owns this pin.
	pub owner:  Str,
	/// Human-readable reason for keeping this item.
	pub reason: Str,
	/// Agent-estimated token charge when the pin was accepted.
	pub tokens: u64,
}

/// Context pin admission failure; every failure leaves the journal unchanged.
#[derive(Debug, Error)]
pub enum ContextPinError {
	/// A durable request key was reused for a different operation or arguments.
	#[error("context idempotency key was reused with different arguments")]
	IdempotencyConflict,
	/// The item is no longer in the selected live projection.
	#[error("context item is no longer live: {id}")]
	ContextGone {
		/// Stable item identity.
		id: Str,
	},
	/// The requested removal includes another principal's pin.
	#[error("context pin belongs to another principal: {id}")]
	PermissionDenied {
		/// Stable item identity.
		id: Str,
	},
	/// The union of pins would exceed the configured token allowance.
	#[error("context pins exceed the token budget")]
	BudgetExceeded,
	/// Pin state in the authoritative DOM is malformed.
	#[error("invalid durable context pin state")]
	InvalidState,
	/// Journal append or DOM transaction failed.
	#[error(transparent)]
	Session(#[from] SessionError),
}

/// Reads durable pins from the selected branch. No process-local pin cache is
/// involved, so rewind and replay select exactly the journaled ownership.
pub fn context_pins(dom: &Dom) -> Result<BTreeMap<Str, ContextPin>, ContextPinError> {
	let value = dom
		.get(dom.meta())
		.and_then(|node| node.prop(&PropKey::Custom(Str::new_static(PINS_PROP))));
	match value {
		None => Ok(BTreeMap::new()),
		Some(Value::Json(raw)) => {
			serde_json::from_str(raw.get()).map_err(|_| ContextPinError::InvalidState)
		},
		Some(_) => Err(ContextPinError::InvalidState),
	}
}

impl Session {
	/// Resolves a body reference against the current selected projection,
	/// excluding its leading system items exactly as the agent ContextView does.
	/// A stale sequence/event cannot accidentally address another journal item.
	pub fn context_item(
		&self,
		id: &str,
		event: u64,
		seq: usize,
	) -> Result<omp_proto::thread::v1::Item, ContextPinError> {
		let gone = || ContextPinError::ContextGone { id: Str::new(id) };
		let item = crate::project_thread(self.dom())
			.into_iter()
			.skip_while(|item| matches!(item.kind.as_ref(), Some(omp_proto::thread::v1::item::Kind::Message(message)) if message.role == omp_proto::thread::v1::Role::System as i32))
			.nth(seq)
			.ok_or_else(gone)?;
		let origin = self.context_origin(&item)?.ok_or_else(gone)?;
		if origin.id.as_str() != id || origin.event != event {
			return Err(gone());
		}
		Ok(item)
	}

	/// Reads the original streamed argument emission, before `call_ready`
	/// replaced the DOM input with canonical/repaired JSON. Non-streamed calls
	/// have no separately captured raw emission and return `None`.
	pub fn context_raw_args(
		&self,
		id: &str,
		event: u64,
		seq: usize,
	) -> Result<Option<Vec<u8>>, ContextPinError> {
		use omp_journal::{
			data::{Stream, StreamOp, ToolCall},
			kind,
		};
		let item = self.context_item(id, event, seq)?;
		if !matches!(item.kind, Some(omp_proto::thread::v1::item::Kind::ToolCall(_))) {
			return Ok(None);
		}
		let index = usize::try_from(event).map_err(|_| ContextPinError::InvalidState)?;
		let entry = self
			.entries
			.get(index)
			.ok_or(ContextPinError::InvalidState)?;
		let call: ToolCall =
			serde_json::from_str(entry.data.as_str()).map_err(|_| ContextPinError::InvalidState)?;
		let Some(sid) = call.sid else {
			return Ok(None);
		};
		let mut raw = Vec::new();
		let head = self.head().ok_or(ContextPinError::InvalidState)?;
		let chain = self.chain_indices(head)?;
		for index in chain.into_iter().filter(|candidate| *candidate > index) {
			let entry = &self.entries[index];
			if entry.kind.name.as_str() != kind::STREAM {
				continue;
			}
			let stream: Stream =
				serde_json::from_str(entry.data.as_str()).map_err(|_| ContextPinError::InvalidState)?;
			if stream.sid != sid {
				continue;
			}
			match stream.op {
				StreamOp::Append => {
					if let Some(text) = stream.text {
						raw.extend_from_slice(text.as_bytes());
					}
				},
				StreamOp::Close => break,
				StreamOp::Open => {},
			}
		}
		Ok(Some(raw))
	}

	/// Resolves projection provenance against this session's real journal.
	pub fn context_origin(
		&self,
		item: &omp_proto::thread::v1::Item,
	) -> Result<Option<ContextOrigin>, ContextPinError> {
		self.context_origin_with_pins(item, &context_pins(self.dom())?)
	}

	/// Resolves a complete projection while decoding the durable pin set once.
	pub fn context_origins(
		&self,
		items: &[omp_proto::thread::v1::Item],
	) -> Result<Vec<Option<ContextOrigin>>, ContextPinError> {
		let pins = context_pins(self.dom())?;
		items
			.iter()
			.map(|item| self.context_origin_with_pins(item, &pins))
			.collect()
	}

	fn context_origin_with_pins(
		&self,
		item: &omp_proto::thread::v1::Item,
		pins: &BTreeMap<Str, ContextPin>,
	) -> Result<Option<ContextOrigin>, ContextPinError> {
		use omp_proto::inference::v1::value::Kind;
		let Some(fields) = item.props.as_ref().map(|props| &props.fields) else {
			return Ok(None);
		};
		let Some(Kind::String(id)) = fields
			.get(crate::projection::CONTEXT_ITEM_ID_PROP)
			.and_then(|value| value.kind.as_ref())
		else {
			return Ok(None);
		};
		let Some(Kind::String(entry)) = fields
			.get(crate::projection::CONTEXT_ENTRY_PROP)
			.and_then(|value| value.kind.as_ref())
		else {
			return Ok(None);
		};
		let entry = entry
			.parse::<omp_journal::EntryId>()
			.map_err(|_| ContextPinError::InvalidState)?;
		let index = self
			.entry_index
			.get(&entry)
			.ok_or(ContextPinError::InvalidState)?;
		let record = &self.entries[*index];
		let mut by = record.by;
		let mut turn_id = None;
		while let Some(parent) = by.and_then(|id| self.entry(id)) {
			if parent.kind.name.as_str() == omp_journal::kind::TURN_START {
				turn_id = Some(Str::new(parent.id.to_string()));
				break;
			}
			by = parent.by;
		}
		Ok(Some(ContextOrigin {
			id: Str::new(id),
			event: u64::try_from(*index).map_err(|_| ContextPinError::InvalidState)?,
			turn_id,
			created_at_ms: entry.as_ulid().timestamp_ms(),
			pinned: pins.contains_key(id.as_str()),
		}))
	}

	/// Records pins after the owning agent has resolved live item identities and
	/// computed their token costs. The caller must never use extension-provided
	/// token costs or accept identities absent from its current projection.
	pub fn pin_context(
		&mut self,
		owner: &str,
		items: &[(Str, u64)],
		reason: &str,
		budget_tokens: u64,
	) -> Result<usize, ContextPinError> {
		self.pin_context_request(owner, items, reason, budget_tokens, None)
	}

	/// Pins and records the original acknowledgement in one journal transaction.
	pub fn pin_context_request(
		&mut self,
		owner: &str,
		items: &[(Str, u64)],
		reason: &str,
		budget_tokens: u64,
		request: Option<&ContextRequestKey>,
	) -> Result<usize, ContextPinError> {
		if let Some(count) = request
			.map(|request| self.context_request_count(request))
			.transpose()?
			.flatten()
		{
			return Ok(count);
		}
		let mut pins = context_pins(self.dom())?;
		let live = crate::project_thread(self.dom());
		for (id, _) in items {
			let present = live.iter().any(|item| {
				item.props.as_ref()
					.and_then(|props| props.fields.get(crate::projection::CONTEXT_ITEM_ID_PROP))
					.and_then(|value| value.kind.as_ref())
					.is_some_and(|value| matches!(value, omp_proto::inference::v1::value::Kind::String(value) if value == id.as_str()))
			});
			if !present {
				return Err(ContextPinError::ContextGone { id: id.clone() });
			}
		}
		let mut added = 0;
		for (id, tokens) in items {
			if pins.contains_key(id) {
				continue;
			}
			pins.insert(id.clone(), ContextPin {
				owner:  Str::new(owner),
				reason: Str::new(reason),
				tokens: *tokens,
			});
			added += 1;
		}
		let total = pins
			.values()
			.try_fold(0_u64, |total, pin| total.checked_add(pin.tokens));
		if total.is_none_or(|total| total > budget_tokens) {
			return Err(ContextPinError::BudgetExceeded);
		}
		if added != 0 || request.is_some() {
			self.write_context_pins(&pins, request.map(|request| (request, added)))?;
		}
		Ok(added)
	}

	/// Atomically releases only the authenticated owner's pins. Mixed-owner
	/// requests fail before deleting any pin.
	pub fn unpin_context(&mut self, owner: &str, ids: &[Str]) -> Result<usize, ContextPinError> {
		self.unpin_context_request(owner, ids, None)
	}

	/// Unpins and records the original acknowledgement in one journal
	/// transaction.
	pub fn unpin_context_request(
		&mut self,
		owner: &str,
		ids: &[Str],
		request: Option<&ContextRequestKey>,
	) -> Result<usize, ContextPinError> {
		if let Some(count) = request
			.map(|request| self.context_request_count(request))
			.transpose()?
			.flatten()
		{
			return Ok(count);
		}
		let mut pins = context_pins(self.dom())?;
		for id in ids {
			if pins.get(id).is_some_and(|pin| pin.owner.as_str() != owner) {
				return Err(ContextPinError::PermissionDenied { id: id.clone() });
			}
		}
		let removed = ids.iter().filter(|id| pins.remove(*id).is_some()).count();
		if removed != 0 || request.is_some() {
			self.write_context_pins(&pins, request.map(|request| (request, removed)))?;
		}
		Ok(removed)
	}

	fn write_context_pins(
		&mut self,
		pins: &BTreeMap<Str, ContextPin>,
		request: Option<(&ContextRequestKey, usize)>,
	) -> Result<(), ContextPinError> {
		let data =
			serde_json::value::to_raw_value(pins).map_err(|_| ContextPinError::InvalidState)?;
		let cause = self.head().ok_or(ContextPinError::InvalidState)?;
		let mut ops = vec![Op::Set {
			h:     self.dom().meta(),
			prop:  PropKey::Custom(Str::new_static(PINS_PROP)),
			value: Value::Json(data),
		}];
		if let Some((request, count)) = request {
			let mut receipts = self.context_receipts()?;
			receipts.insert(receipt_key(request), ContextReceipt {
				fingerprint: request.fingerprint.clone(),
				count,
			});
			ops.push(Op::Set {
				h:     self.dom().meta(),
				prop:  PropKey::Custom(Str::new_static(RECEIPTS_PROP)),
				value: Value::Json(
					serde_json::value::to_raw_value(&receipts)
						.map_err(|_| ContextPinError::InvalidState)?,
				),
			});
		}
		self.patch(Txn { cause, label: Some(Str::new_static("context.pins")), ops })?;
		Ok(())
	}

	fn context_receipts(&self) -> Result<BTreeMap<String, ContextReceipt>, ContextPinError> {
		match self
			.dom()
			.get(self.dom().meta())
			.and_then(|node| node.prop(&PropKey::Custom(Str::new_static(RECEIPTS_PROP))))
		{
			None => Ok(BTreeMap::new()),
			Some(Value::Json(raw)) => {
				serde_json::from_str(raw.get()).map_err(|_| ContextPinError::InvalidState)
			},
			Some(_) => Err(ContextPinError::InvalidState),
		}
	}

	/// Returns the original acknowledgement for an exact durable request retry.
	pub fn context_request_count(
		&self,
		request: &ContextRequestKey,
	) -> Result<Option<usize>, ContextPinError> {
		match self.context_receipts()?.get(&receipt_key(request)) {
			Some(receipt) if receipt.fingerprint != request.fingerprint => {
				Err(ContextPinError::IdempotencyConflict)
			},
			Some(receipt) => Ok(Some(receipt.count)),
			None => Ok(None),
		}
	}
}

fn receipt_key(request: &ContextRequestKey) -> String {
	serde_json::json!([request.owner, request.key]).to_string()
}
