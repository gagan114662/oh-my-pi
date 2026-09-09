#![allow(
	clippy::disallowed_types,
	reason = "omp-http is the workspace-owned reqwest connection-pool boundary"
)]

//! Shared outbound HTTP clients and process-wide TLS policy.
//!
//! Reqwest clients own connection pools. Callers clone one of the process-wide
//! clients instead of constructing a pool per request or host instance.

use std::{collections::VecDeque, net::SocketAddr, ops::Deref, sync::LazyLock};

use parking_lot::Mutex;
use reqwest::{Client as ReqwestClient, ClientBuilder, redirect::Policy};

/// Cloneable handle to the workspace-owned HTTP connection pool.
#[derive(Clone, Debug)]
pub struct Client(ReqwestClient);

impl Deref for Client {
	type Target = ReqwestClient;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

impl From<ReqwestClient> for Client {
	fn from(client: ReqwestClient) -> Self {
		Self(client)
	}
}

static DEFAULT_CLIENT: LazyLock<Client> =
	LazyLock::new(|| build_client(client_builder(), "default", "default"));
static NO_REDIRECT_CLIENT: LazyLock<Client> = LazyLock::new(|| {
	build_client(client_builder().redirect(Policy::none()), "no_redirect", "disabled")
});
static TLS_PROVIDER_READY: LazyLock<()> = LazyLock::new(|| {
	if rustls::crypto::ring::default_provider()
		.install_default()
		.is_ok()
	{
		tracing::debug!("HTTP TLS provider installed");
	} else {
		tracing::debug!("HTTP TLS provider already installed");
	}
});

/// Clones the process-wide client using Reqwest's default redirect policy.
#[inline]
pub fn default_client() -> Client {
	DEFAULT_CLIENT.clone()
}

/// Clones the process-wide redirect-disabled client.
#[inline]
pub fn no_redirect_client() -> Client {
	NO_REDIRECT_CLIENT.clone()
}

struct PinnedPool {
	root_certificate: Option<Vec<u8>>,
	host:             String,
	addresses:        Vec<SocketAddr>,
	client:           Client,
}

static PINNED_POOLS: Mutex<VecDeque<PinnedPool>> = Mutex::new(VecDeque::new());

/// Reuses a bounded pool keyed by the exact addresses already admitted by the
/// host. Redirects and ambient proxies are disabled: neither may resolve or
/// choose a destination outside those addresses. TLS still authenticates the
/// original URL hostname. The caller must validate every address before use.
pub fn pinned_destination_client(
	host: &str,
	addresses: &[SocketAddr],
) -> Result<Client, reqwest::Error> {
	pinned_destination_client_with_root(host, addresses, None)
}

/// Reuses an admitted-destination pool with an optional host-trusted DER root.
/// A distinct root owns a distinct pool; URL names are still verified normally.
pub fn pinned_destination_client_with_root(
	host: &str,
	addresses: &[SocketAddr],
	root_certificate: Option<&[u8]>,
) -> Result<Client, reqwest::Error> {
	let mut pools = PINNED_POOLS.lock();
	if let Some(index) = pools.iter().position(|pool| {
		pool.host == host
			&& pool.addresses == addresses
			&& pool.root_certificate.as_deref() == root_certificate
	}) {
		let pool = pools.remove(index).expect("located pool exists");
		let client = pool.client.clone();
		pools.push_back(pool);
		return Ok(client);
	}
	let mut builder = client_builder()
		.no_proxy()
		.redirect(Policy::none())
		.resolve_to_addrs(host, addresses);
	if let Some(root) = root_certificate {
		builder = builder.tls_certs_merge([reqwest::Certificate::from_der(root)?]);
	}
	let client = Client::from(builder.build()?);
	if pools.len() == 64 {
		pools.pop_front();
	}
	pools.push_back(PinnedPool {
		root_certificate: root_certificate.map(<[u8]>::to_vec),
		host:             host.to_owned(),
		addresses:        addresses.to_vec(),
		client:           client.clone(),
	});
	Ok(client)
}

/// Starts a client builder after installing the workspace Ring provider.
#[inline]
pub fn client_builder() -> ClientBuilder {
	LazyLock::force(&TLS_PROVIDER_READY);
	ReqwestClient::builder()
}
/// Installs the process-wide rustls Ring crypto provider.
///
/// Idempotent. Hosts call it once at process bootstrap so TLS clients built
/// outside this crate (telemetry exporters, vendored SDKs) never construct
/// before a provider exists.
pub fn install_tls_provider() {
	LazyLock::force(&TLS_PROVIDER_READY);
}

#[tracing::instrument(
	level = "debug",
	name = "http_client_pool_init",
	skip_all,
	fields(http.pool = pool, http.redirect_policy = redirect_policy)
)]
fn build_client(
	builder: ClientBuilder,
	pool: &'static str,
	redirect_policy: &'static str,
) -> Client {
	let client = builder.build().unwrap_or_else(|error| {
		panic!("{pool} HTTP client configuration must be valid: {error}");
	});
	tracing::debug!("HTTP client pool initialized");
	Client::from(client)
}

/// Failure while collecting a response within the caller's byte ceiling.
#[derive(Debug, thiserror::Error)]
pub enum BodyError {
	/// The declared or streamed body exceeds the configured ceiling.
	#[error("HTTP response exceeds the {limit}-byte body limit")]
	TooLarge {
		/// Maximum retained response bytes.
		limit: usize,
	},
	/// Reading the transport failed, including an expired request timeout.
	#[error("HTTP response transport failed: {0}")]
	Transport(#[from] reqwest::Error),
}

/// Collects at most `limit` response bytes, rejecting oversized bodies.
///
/// Checks both Content-Length and actual streamed bytes. Dropping this future
/// drops its response stream; request deadlines set on the request builder
/// continue to apply while the body is read. The caller owns byte/deadline
/// policy, so streaming provider clients are not assigned a global limit.
pub async fn read_bounded(
	mut response: reqwest::Response,
	limit: usize,
) -> Result<bytes::Bytes, BodyError> {
	if response
		.content_length()
		.is_some_and(|length| length > limit as u64)
	{
		return Err(BodyError::TooLarge { limit });
	}
	let mut body = bytes::BytesMut::with_capacity(
		response
			.content_length()
			.and_then(|length| usize::try_from(length).ok())
			.unwrap_or_default()
			.min(limit),
	);
	while let Some(chunk) = response.chunk().await? {
		if chunk.len() > limit.saturating_sub(body.len()) {
			return Err(BodyError::TooLarge { limit });
		}
		body.extend_from_slice(&chunk);
	}
	Ok(body.freeze())
}
