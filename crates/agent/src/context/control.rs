//! Actor-bound context CONTROL transport. No environment task owns a Session.

use std::sync::Arc;

use omp_core::Str;
use omp_session::Session;
use parking_lot::RwLock;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::{SessionMutation, Up, loop_::ContextProjector};

/// Host-authenticated parent callback for a nested context operation.
/// Never decoded from CONTROL operation arguments.
#[derive(Clone, Debug)]
pub struct ContextControlOrigin {
	/// Exact parent invocation, issued by the host runtime.
	pub invocation:         Str,
	/// Authenticated extension identity.
	pub extension:          Str,
	/// Authenticated deployment layer.
	pub layer:              Str,
	/// Authenticated trust tier.
	pub tier:               Str,
	/// Live worker generation.
	pub host_generation:    u64,
	/// Live session generation.
	pub session_generation: u64,
}

tokio::task_local! {
	static CONTEXT_ORIGIN: Option<ContextControlOrigin>;
}

/// Scopes descendant hook dispatch to its trusted CONTROL caller.
pub async fn with_context_origin<T>(
	origin: Option<ContextControlOrigin>,
	future: impl std::future::Future<Output = T>,
) -> T {
	CONTEXT_ORIGIN.scope(origin, future).await
}

/// Returns only a host-scoped parent, never an argument-supplied identity.
#[must_use]
pub fn current_context_origin() -> Option<ContextControlOrigin> {
	CONTEXT_ORIGIN.try_with(Clone::clone).ok().flatten()
}

/// Typed context rejection returned unchanged through the environment owner.
#[derive(Clone, Debug, thiserror::Error)]
#[error("{code}: {message}")]
pub struct ContextControlError {
	/// Python domain exception or protocol code.
	pub code:    Str,
	/// Human-readable explanation.
	pub message: Str,
}

impl ContextControlError {
	pub(crate) fn new(code: &str, message: impl ToString) -> Self {
		Self { code: Str::new(code), message: Str::new(message.to_string()) }
	}
}

/// Revocable admission lease. The read lock covers the actual actor mutation;
/// revocation waits for any already-committing operation to finish.
#[derive(Clone, Debug, Default)]
pub struct ContextControlLease {
	active:  Arc<RwLock<bool>>,
	revoked: CancellationToken,
}

/// Admission held again around the final journal append after summarization.
#[derive(Clone, Debug)]
pub(crate) struct ContextCommitGuard {
	live:                    ContextControlLease,
	connection:              ContextControlLease,
	pub(crate) cancellation: CancellationToken,
	pub(crate) outcome:      Arc<parking_lot::Mutex<Option<Value>>>,
	request:                 omp_session::context::ContextRequestKey,
}

impl ContextCommitGuard {
	pub(crate) fn replay(&self, session: &Session) -> Result<Option<Value>, ContextControlError> {
		let live = self.live.active.read();
		let connection = self.connection.active.read();
		if !*live || !*connection {
			return Err(ContextControlError::new("StaleGeneration", "context owner was revoked"));
		}
		if self.cancellation.is_cancelled() {
			return Err(ContextControlError::new("Cancelled", "context request was cancelled"));
		}
		session
			.context_compaction_result(&self.request)
			.map_err(session_error)
	}

	pub(crate) fn commit(
		&self,
		session: &mut Session,
		mut compaction: omp_journal::data::Compaction,
		outcome: Value,
	) -> Result<omp_journal::EntryId, omp_session::SessionError> {
		let live = self.live.active.read();
		let connection = self.connection.active.read();
		if !*live || !*connection || self.cancellation.is_cancelled() {
			return Err(omp_session::SessionError::ContextAdmissionRevoked);
		}
		compaction.receipt = Some(omp_journal::data::CompactionReceipt {
			owner: self.request.owner.clone(),
			key: self.request.key.clone(),
			fingerprint: self.request.fingerprint.clone(),
			outcome,
		});
		let entry = session.compaction(compaction)?;
		*self.outcome.lock() = session
			.context_compaction_result(&self.request)
			.map_err(|_| omp_session::SessionError::InvalidContextReceipt)?;
		Ok(entry)
	}
}

pub(crate) struct PendingContextCompaction {
	pub(crate) focus:  Option<Str>,
	pub(crate) origin: Option<ContextControlOrigin>,
	pub(crate) guard:  ContextCommitGuard,
	pub(crate) reply:  flume::Sender<Result<Value, ContextControlError>>,
}

/// A one-shot request for the real kernel compaction director.
#[derive(Clone)]
pub struct ContextCompactRequest(Arc<parking_lot::Mutex<Option<PendingContextCompaction>>>);

impl std::fmt::Debug for ContextCompactRequest {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("ContextCompactRequest")
			.finish_non_exhaustive()
	}
}

impl ContextCompactRequest {
	pub(crate) fn take(&self) -> Option<PendingContextCompaction> {
		self.0.lock().take()
	}

	/// Rejects recursive compaction while the kernel is inside an active effect.
	pub fn reject_busy(&self) {
		if let Some(request) = self.take() {
			let _ = request.reply.send(Err(ContextControlError::new(
				"CompactionBusy",
				"the kernel cannot compact during the current operation",
			)));
		}
	}
}

impl ContextControlLease {
	/// Creates an admitted host-generation lease.
	#[must_use]
	pub fn admitted() -> Self {
		Self { active: Arc::new(RwLock::new(true)), revoked: CancellationToken::new() }
	}

	/// Revokes every queued operation sharing this lease.
	pub fn revoke(&self) {
		*self.active.write() = false;
		self.revoked.cancel();
	}
}

/// The existing kernel mailbox plus its current live projection recipe.
#[derive(Clone)]
pub struct ContextControl {
	origin:    Option<ContextControlOrigin>,
	sender:    flume::Sender<Up>,
	projector: Arc<RwLock<Option<ContextProjector>>>,
	live:      ContextControlLease,
}

impl ContextControl {
	pub(crate) fn new(sender: flume::Sender<Up>) -> Self {
		Self {
			origin: None,
			sender,
			projector: Arc::new(RwLock::new(None)),
			live: ContextControlLease::admitted(),
		}
	}

	/// Revokes queued requests when this kernel binding is replaced.
	pub fn revoke(&self) {
		self.live.revoke();
	}

	/// Derives invocation scope from the authenticated environment owner.
	#[must_use]
	pub fn for_invocation(mut self, origin: Option<ContextControlOrigin>) -> Self {
		self.origin = origin;
		self
	}

	pub(crate) fn refresh(&self, projector: ContextProjector) {
		*self.projector.write() = Some(projector);
	}

	/// Runs an operation on the Session owner. `owner` must come from the
	/// authenticated connection, never from its argument object.
	pub async fn request(
		&self,
		owner: Str,
		operation: Str,
		arguments: Map<String, Value>,
		lease: ContextControlLease,
	) -> Result<Value, ContextControlError> {
		let projector = self.projector.clone();
		let live = self.live.clone();
		let binding_revoked = live.revoked.clone();
		let connection_revoked = lease.revoked.clone();
		let cancellation = CancellationToken::new();
		let _cancel_on_drop = cancellation.clone().drop_guard();
		let (reply, response) = flume::bounded(1);
		if operation == "omp.context.compact" {
			let request = request_key(&owner, &operation, &arguments)?;
			if arguments
				.get("focus")
				.is_some_and(|focus| !focus.is_string())
			{
				return Err(ContextControlError::new("InvalidArgument", "focus must be a string"));
			}
			if arguments
				.get("tier")
				.is_some_and(|tier| !tier.is_null() && tier.as_str() != Some("local"))
			{
				return Err(ContextControlError::new(
					"CompactionRefused",
					"the active compaction director does not implement the requested tier",
				));
			}
			let focus = arguments
				.get("focus")
				.and_then(Value::as_str)
				.filter(|focus| !focus.is_empty())
				.map(Str::new);
			let pending = PendingContextCompaction {
				focus,
				origin: self.origin.clone(),
				guard: ContextCommitGuard {
					live,
					connection: lease,
					cancellation,
					outcome: Arc::default(),
					request,
				},
				reply,
			};
			self
				.sender
				.send_async(Up::ContextCompact(ContextCompactRequest(Arc::new(
					parking_lot::Mutex::new(Some(pending)),
				))))
				.await
				.map_err(|_| {
					ContextControlError::new("ContextUnavailable", "kernel mailbox is closed")
				})?;
			return receive_response(response, binding_revoked, connection_revoked).await;
		}
		let mutation = SessionMutation::new(move |session| {
			let binding = live.active.read();
			let admitted = lease.active.read();
			let result = if !*admitted || !*binding {
				Err(ContextControlError::new("StaleGeneration", "context owner was revoked"))
			} else if cancellation.is_cancelled() {
				Err(ContextControlError::new(
					"Cancelled",
					"context request was cancelled before admission",
				))
			} else {
				let projector = projector.read().clone();
				projector
					.ok_or_else(|| {
						ContextControlError::new(
							"ContextUnavailable",
							"kernel projection has not been bound",
						)
					})
					.and_then(|projector| execute(session, &projector, &owner, &operation, &arguments))
			};
			let _ = reply.send(result);
		});
		self
			.sender
			.send_async(Up::SessionMutation(mutation))
			.await
			.map_err(|_| ContextControlError::new("ContextUnavailable", "kernel mailbox is closed"))?;
		receive_response(response, binding_revoked, connection_revoked).await
	}
}

async fn receive_response(
	response: flume::Receiver<Result<Value, ContextControlError>>,
	binding_revoked: CancellationToken,
	connection_revoked: CancellationToken,
) -> Result<Value, ContextControlError> {
	tokio::select! {
		biased;
		result = response.recv_async() => result.map_err(|_| ContextControlError::new("ContextUnavailable", "kernel dropped the request"))?,
		() = binding_revoked.cancelled() => Err(ContextControlError::new("StaleGeneration", "kernel binding was revoked")),
		() = connection_revoked.cancelled() => Err(ContextControlError::new("StaleGeneration", "context connection was revoked")),
	}
}

fn string<'a>(
	arguments: &'a Map<String, Value>,
	key: &str,
) -> Result<&'a str, ContextControlError> {
	arguments
		.get(key)
		.and_then(Value::as_str)
		.ok_or_else(|| ContextControlError::new("InvalidArgument", format!("{key} must be a string")))
}

fn reference(arguments: &Map<String, Value>) -> Result<(&str, u64, usize), ContextControlError> {
	let id = string(arguments, "id")?;
	let event = arguments
		.get("event")
		.and_then(Value::as_u64)
		.ok_or_else(|| {
			ContextControlError::new("InvalidArgument", "event must be an unsigned integer")
		})?;
	let seq = arguments
		.get("seq")
		.and_then(Value::as_u64)
		.and_then(|seq| usize::try_from(seq).ok())
		.ok_or_else(|| {
			ContextControlError::new("InvalidArgument", "seq must be an unsigned integer")
		})?;
	Ok((id, event, seq))
}

fn session_error(error: omp_session::context::ContextPinError) -> ContextControlError {
	use omp_session::context::ContextPinError;
	let code = match &error {
		ContextPinError::IdempotencyConflict => "IdempotencyConflict",
		ContextPinError::ContextGone { .. } => "ContextGone",
		ContextPinError::PermissionDenied { .. } => "PermissionDenied",
		ContextPinError::BudgetExceeded => "PinBudgetExceeded",
		ContextPinError::InvalidState | ContextPinError::Session(_) => "ContextStateError",
	};
	ContextControlError::new(code, error)
}

fn request_key(
	owner: &str,
	operation: &str,
	arguments: &Map<String, Value>,
) -> Result<omp_session::context::ContextRequestKey, ContextControlError> {
	let key = string(arguments, "idempotency_key")?;
	if key.is_empty() || key.len() > 512 {
		return Err(ContextControlError::new(
			"InvalidArgument",
			"idempotency_key must contain 1..512 bytes",
		));
	}
	let mut normalized = Value::Object(arguments.clone());
	normalized
		.as_object_mut()
		.expect("argument object")
		.remove("idempotency_key");
	normalized.sort_all_objects();
	let fingerprint =
		omp_core::Hash32::sum(json!([operation, normalized]).to_string().as_bytes()).to_hex();
	Ok(omp_session::context::ContextRequestKey {
		owner:       Str::new(owner),
		key:         Str::new(key),
		fingerprint: Str::new(fingerprint),
	})
}

fn execute(
	session: &mut Session,
	projector: &ContextProjector,
	owner: &str,
	operation: &str,
	arguments: &Map<String, Value>,
) -> Result<Value, ContextControlError> {
	let receipt = if matches!(operation, "omp.context.pin" | "omp.context.unpin") {
		let request = request_key(owner, operation, arguments)?;
		if let Some(count) = session
			.context_request_count(&request)
			.map_err(session_error)?
		{
			return Ok(json!(count));
		}
		Some(request)
	} else {
		None
	};
	match operation {
		"omp.context.view" | "omp.context.usage" => {
			let (facts, messages) = projector
				.project(session)
				.map_err(|error| ContextControlError::new("ContextProjectionError", error))?;
			let mut view = super::context_view(&facts, &messages[super::prompt_head_len(&messages)..]);
			Ok(if operation == "omp.context.usage" {
				view["usage"].take()
			} else {
				view
			})
		},
		"omp.context.epoch" => Ok(json!(session.dom().count("compaction").unwrap_or(0))),
		"omp.context.message.raw_args" => {
			let (id, event, seq) = reference(arguments)?;
			Ok(session
				.context_raw_args(id, event, seq)
				.map_err(session_error)?
				.map_or(
					Value::Null,
					|raw| json!({"base64": omp_core::base64::encode(&raw).into_string()}),
				))
		},
		"omp.context.message.parts" | "omp.context.message.verdict" => {
			let (id, event, seq) = reference(arguments)?;
			let item = session
				.context_item(id, event, seq)
				.map_err(session_error)?;
			if operation.ends_with(".verdict") {
				return Err(ContextControlError::new(
					"NoVerdict",
					"this journal item has no declared Python verdict type",
				));
			}
			use omp_proto::thread::v1::{item, part};
			let parts = match item.kind {
				Some(item::Kind::Message(message)) => message.parts,
				Some(item::Kind::ToolResult(result)) => result.parts,
				Some(item::Kind::ToolCall(call)) => {
					return serde_json::from_slice::<Value>(&call.args_json)
						.map(|value| json!([{"kind": "json", "value": value}]))
						.map_err(|error| ContextControlError::new("ContextStateError", error));
				},
				None => {
					return Err(ContextControlError::new(
						"ContextStateError",
						"journal projection has no item kind",
					));
				},
			};
			parts.into_iter().map(|part| match part.kind {
				Some(part::Kind::Text(text)) => Ok(json!({"kind": "text", "text": text})),
				Some(part::Kind::Blob(blob)) => Ok(json!({"kind": "blob", "hash": omp_core::encoding::hex::encode(&blob.hash).into_string(), "size": blob.size})),
				Some(part::Kind::Thinking(thinking)) if !thinking.redacted => Ok(json!({"kind": "text", "text": thinking.text})),
				_ => Err(ContextControlError::new("UnsupportedContextPart", "this typed journal part has no Python Part representation")),
			}).collect::<Result<Vec<_>, _>>().map(Value::Array)
		},
		"omp.context.unpin" => {
			let ids = ids(arguments)?;
			session
				.unpin_context_request(owner, &ids, receipt.as_ref())
				.map(|count| json!(count))
				.map_err(session_error)
		},
		"omp.context.pin" => {
			let ids = ids(arguments)?;
			let reason = string(arguments, "reason")?;
			let (facts, messages) = projector
				.project(session)
				.map_err(|error| ContextControlError::new("ContextProjectionError", error))?;
			let view = super::context_view(&facts, &messages[super::prompt_head_len(&messages)..]);
			let rows = view["messages"]
				.as_array()
				.expect("context view contains messages");
			let items = ids
				.into_iter()
				.map(|id| {
					let row = rows
						.iter()
						.find(|row| row["id"].as_str() == Some(id.as_str()))
						.ok_or_else(|| ContextControlError::new("ContextGone", &id))?;
					Ok((
						id,
						row["tokens"]
							.as_u64()
							.expect("context token estimate is unsigned"),
					))
				})
				.collect::<Result<Vec<_>, ContextControlError>>()?;
			session
				.pin_context_request(
					owner,
					&items,
					reason,
					projector.pin_budget(facts.context_window),
					receipt.as_ref(),
				)
				.map(|count| json!(count))
				.map_err(session_error)
		},
		_ => Err(ContextControlError::new("unhandled_operation", operation)),
	}
}

fn ids(arguments: &Map<String, Value>) -> Result<Vec<Str>, ContextControlError> {
	arguments
		.get("ids")
		.and_then(Value::as_array)
		.ok_or_else(|| ContextControlError::new("InvalidArgument", "ids must be an array"))?
		.iter()
		.map(|id| {
			id.as_str()
				.filter(|id| !id.is_empty())
				.map(Str::new)
				.ok_or_else(|| {
					ContextControlError::new("InvalidArgument", "ids must contain nonempty strings")
				})
		})
		.collect()
}
