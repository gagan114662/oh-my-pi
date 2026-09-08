//! Trusted native HTTP scope, independent of command sandbox enablement.

use std::sync::Arc;

use omp_con::Ctx;
use omp_core::Str;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::policy::SandboxNetworkPolicy;

omp_con::var! {
	/// Native HTTP destination policy using the SandboxNetworkPolicy JSON schema.
	/// Captured when the Environment is opened; command sandbox mode is independent.
	pub static SV_NATIVE_HTTP_POLICY = sv_native_http_policy: Str {
		default: Str::new_static("{\"mode\":\"open\"}"),
		validate: |_ctx, value| parse_policy(value).map(|_| ()).map_err(|_| Str::new_static("invalid native HTTP network policy")),
		flags: archive,
	};
	/// Host-owned native HTTP parent restrictions inherited by delegated sessions.
	pub static SV_NATIVE_HTTP_INHERITED_POLICIES = sv_native_http_inherited_policies: Vec<Str> {
		default: Vec::new(),
		validate: |_ctx, values| values.iter().try_for_each(|value| parse_policy(value).map(|_| ()).map_err(|_| Str::new_static("invalid inherited native HTTP policy"))),
		flags: readonly | archive,
	};

}

/// Failure to decode or capture trusted native HTTP configuration.
#[derive(Debug, Error)]
pub enum NativeHttpPolicyError {
	/// The console refused host-owned scope capture.
	#[error("native HTTP inherited authority could not be captured")]
	Console(#[from] omp_con::ConError),
	/// The configured value does not match the shared schema.
	#[error("native HTTP network policy is invalid JSON")]
	Json(#[from] serde_json::Error),
	/// Unknown modes fail closed.
	#[error("native HTTP network policy mode or DNS mode is unsupported")]
	Unsupported,
}

pub(crate) fn from_con(ctx: &Ctx) -> Result<Vec<SandboxNetworkPolicy>, NativeHttpPolicyError> {
	let mut sources = SV_NATIVE_HTTP_INHERITED_POLICIES.get(ctx);
	let current = SV_NATIVE_HTTP_POLICY.get(ctx);
	if !sources.contains(&current) {
		sources.push(current);
	}
	let policies = sources
		.iter()
		.map(|value| parse_policy(value))
		.collect::<Result<Vec<_>, _>>()?;
	// Capture the startup scope into the host-only floor before model execution.
	// Children still inherit it if the public setting changes during this session.
	SV_NATIVE_HTTP_INHERITED_POLICIES.set(ctx, sources)?;
	Ok(policies)
}

/// Captures the composition's startup authority before tools or child spawning.
/// Subsequent model-visible console writes cannot replace the inherited floor.
pub fn capture_native_http_policy(ctx: &Ctx) -> Result<(), NativeHttpPolicyError> {
	from_con(ctx).map(|_| ())
}

pub(crate) fn with_owner_baseline(
	ctx: &Ctx,
	owner: &[String],
) -> Result<Vec<SandboxNetworkPolicy>, NativeHttpPolicyError> {
	let mut policies = from_con(ctx)?;
	if owner.is_empty() && policies.iter().any(|policy| policy.mode != "open") {
		return Err(NativeHttpPolicyError::Unsupported);
	}
	for source in owner {
		policies.push(parse_policy(source)?);
	}
	// Preserve the project floor for further delegation from this session.
	let mut inherited = SV_NATIVE_HTTP_INHERITED_POLICIES.get(ctx);
	for source in owner {
		let source = Str::from(source.as_str());
		if !inherited.contains(&source) {
			inherited.push(source);
		}
	}
	SV_NATIVE_HTTP_INHERITED_POLICIES.set(ctx, inherited)?;
	Ok(policies)
}

pub(crate) fn parse_policy(value: &str) -> Result<SandboxNetworkPolicy, NativeHttpPolicyError> {
	let policy: SandboxNetworkPolicy = serde_json::from_str(value)?;
	if !matches!(policy.mode.as_str(), "open" | "proxy" | "deny")
		|| !matches!(policy.dns.as_str(), "proxy_only" | "allow" | "deny")
	{
		return Err(NativeHttpPolicyError::Unsupported);
	}
	Ok(policy)
}

/// An intersection of host-owned policies. Appending a policy can only narrow
/// the scope, including when the appended policy itself is explicitly open.
#[derive(Clone, Default)]
pub(crate) struct NativeHttpScope {
	pub(crate) policies: Arc<[SandboxNetworkPolicy]>,
	pub(crate) revoked:  CancellationToken,
}

impl NativeHttpScope {
	pub(crate) fn narrow(&self, policy: SandboxNetworkPolicy) -> Self {
		let mut policies = self.policies.to_vec();
		policies.push(policy);
		Self { policies: policies.into(), revoked: self.revoked.child_token() }
	}

	pub(crate) fn restricted(&self) -> bool {
		self.policies.iter().any(|policy| policy.mode != "open")
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn inherited_scope_rejects_script_widening_and_unknown_owner_baseline() {
		let ctx = Ctx::new();
		SV_NATIVE_HTTP_POLICY
			.set(&ctx, Str::from("{\"mode\":\"deny\"}"))
			.expect("owner config");
		capture_native_http_policy(&ctx).expect("capture");
		ctx.set_value(
			SV_NATIVE_HTTP_POLICY.name(),
			omp_con::Value::Str(Str::from("{\"mode\":\"open\"}")),
			omp_con::SetSource::Script,
		)
		.expect("public console write");
		assert!(
			ctx.set_value(
				SV_NATIVE_HTTP_INHERITED_POLICIES.name(),
				omp_con::Value::List(Vec::new()),
				omp_con::SetSource::Script
			)
			.is_err()
		);
		assert!(
			from_con(&ctx)
				.expect("scope")
				.iter()
				.any(|policy| policy.mode == "deny")
		);
		assert!(with_owner_baseline(&ctx, &[]).is_err());
		let broad = Ctx::new();
		assert!(with_owner_baseline(&broad, &[]).is_ok(), "legacy broad owner remains compatible");
		assert!(with_owner_baseline(&broad, &["{\"mode\":\"invalid\"}".to_owned()]).is_err());
	}
}
