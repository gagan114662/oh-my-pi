//! Joined native HTTP tests: real EnvServer dispatch and independent servers.

use std::sync::atomic::{AtomicUsize, Ordering};

use omp_core::Principal;
use tokio::{
	io::{AsyncReadExt, AsyncWriteExt},
	net::TcpListener,
	sync::Notify,
};

use super::*;
use crate::{
	SV_NATIVE_HTTP_POLICY,
	policy::{SandboxDomainRule, SandboxNetworkPolicy},
};

struct Destination {
	received: Arc<Mutex<Vec<String>>>,
	port:     u16,
	hits:     Arc<AtomicUsize>,
	started:  Arc<Notify>,
	release:  Arc<Notify>,
	task:     tokio::task::JoinHandle<()>,
}

impl Destination {
	async fn start(location: Option<String>, hold: bool) -> Self {
		let listener = TcpListener::bind("127.0.0.1:0")
			.await
			.expect("destination listener");
		let port = listener.local_addr().expect("destination address").port();
		let hits = Arc::new(AtomicUsize::new(0));
		let received = Arc::new(Mutex::new(Vec::new()));
		let started = Arc::new(Notify::new());
		let release = Arc::new(Notify::new());
		let task = tokio::spawn({
			let hits = hits.clone();
			let received = received.clone();
			let started = started.clone();
			let release = release.clone();
			async move {
				loop {
					let (mut stream, _) = listener.accept().await.expect("accept destination");
					let mut bytes = Vec::new();
					loop {
						let mut byte = [0];
						if stream.read_exact(&mut byte).await.is_err() {
							break;
						}
						bytes.push(byte[0]);
						if bytes.ends_with(b"\r\n\r\n") {
							break;
						}
						assert!(bytes.len() < 16384, "bounded test headers");
					}
					if bytes.is_empty() {
						continue;
					}
					received
						.lock()
						.push(String::from_utf8(bytes.clone()).expect("HTTP headers"));
					hits.fetch_add(1, Ordering::SeqCst);
					started.notify_one();
					if hold {
						release.notified().await;
					}
					let path = std::str::from_utf8(&bytes)
						.expect("HTTP headers")
						.split_whitespace()
						.nth(1)
						.expect("request path");
					let response = if path == "/ok" || location.is_none() {
						"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_owned()
					} else {
						format!(
							"HTTP/1.1 302 Found\r\nLocation: {}\r\nContent-Length: 0\r\nConnection: \
							 close\r\n\r\n",
							location.as_deref().expect("redirect")
						)
					};
					let _ = stream.write_all(response.as_bytes()).await;
				}
			}
		});
		Self { received, port, hits, started, release, task }
	}

	fn url(&self, path: &str) -> String {
		format!("http://127.0.0.1:{}{path}", self.port)
	}

	fn hits(&self) -> usize {
		self.hits.load(Ordering::SeqCst)
	}
}

impl Drop for Destination {
	fn drop(&mut self) {
		self.task.abort();
	}
}

struct Harness {
	server:             Arc<EnvServer>,
	con:                Arc<Ctx>,
	requests:           flume::Sender<pb::ClientFrame>,
	responses:          flume::Receiver<pb::ServerFrame>,
	host:               Option<HostKey>,
	session_generation: u64,
	task:               tokio::task::JoinHandle<()>,
	_root:              tempfile::TempDir,
	_state:             tempfile::TempDir,
}

fn policy(ports: &[u16]) -> SandboxNetworkPolicy {
	SandboxNetworkPolicy {
		allow_domains: vec![SandboxDomainRule {
			domain: Str::new_static("127.0.0.1"),
			ports:  ports.to_vec(),
		}],
		allow_ports: Vec::new(),
		allow_localhost: true,
		..Default::default()
	}
}

impl Harness {
	async fn new(network: SandboxNetworkPolicy, invocation: bool) -> Self {
		Self::with_tls_root(network, invocation, None).await
	}

	async fn with_tls_root(
		network: SandboxNetworkPolicy,
		invocation: bool,
		root_der: Option<Vec<u8>>,
	) -> Self {
		let root = tempfile::tempdir().expect("workspace");
		let state = tempfile::tempdir().expect("state");
		let con = Arc::new(Ctx::new());
		SV_NATIVE_HTTP_POLICY
			.set(&con, Str::from(serde_json::to_string(&network).expect("policy JSON")))
			.expect("configure policy");
		let convars = Arc::new(crate::exthost::ConvarControlFactory::new(con.clone()));
		let mut server = EnvServer::open_local(
			root.path(),
			state.path(),
			Registry::new(),
			ExtHostConfig::new(
				PathBuf::from("unused"),
				Principal::new(sf!("test"), sf!("Test")),
				sf!("http-test"),
				1,
			),
			&con,
			convars,
			RegistryBridges::default(),
		)
		.await
		.expect("real local environment");
		server.http_egress.tls_root_der = root_der;
		let server = Arc::new(server);
		Self::from_server(server, con, root, state, invocation, 1).await
	}

	async fn from_server(
		server: Arc<EnvServer>,
		con: Arc<Ctx>,
		root: tempfile::TempDir,
		state: tempfile::TempDir,
		invocation: bool,
		session_generation: u64,
	) -> Self {
		let host = invocation.then(|| HostKey::new("workspace", "sandboxed", "http-child"));
		let connection_policy = if let Some(host) = &host {
			server
				.authority
				.register_host(host.clone(), Grants::supported(["env.net"]));
			server.authority.open(host.clone(), sf!("http-invocation"));
			// A child asking for open access still intersects the captured parent.
			server
				.authority
				.narrow_native_http_invocation(host, "http-invocation", SandboxNetworkPolicy {
					mode: sf!("open"),
					..Default::default()
				})
				.expect("trusted child scope");
			server
				.authority
				.authorize(
					host,
					"http-invocation",
					Bytes::from_static(b"http-token"),
					Grants::supported(["env.net"]),
					1,
					1,
					session_generation,
				)
				.expect("authorize");
			ConnectionPolicy::extension(host.clone(), ["env.net"])
		} else {
			ConnectionPolicy::external(None)
		};
		let (requests, request_rx) = flume::bounded(16);
		let (response_tx, responses) = flume::bounded(16);
		let task = tokio::spawn({
			let server = server.clone();
			async move {
				server
					.serve_frames(request_rx, response_tx, connection_policy)
					.await;
			}
		});
		requests
			.send_async(pb::ClientFrame {
				request_id: 0,
				body: Some(client_frame::Body::Hello(pb::ClientHello {
					client: "http-test".to_owned(),
					schema_rev: omp_proto::SCHEMA_REV,
					capabilities: vec!["env.net".to_owned()],
					..Default::default()
				})),
				..Default::default()
			})
			.await
			.expect("hello");
		assert!(matches!(
			responses.recv_async().await.expect("hello response").body,
			Some(server_frame::Body::Hello(_))
		));
		Self {
			server,
			con,
			requests,
			responses,
			host,
			session_generation,
			task,
			_root: root,
			_state: state,
		}
	}

	async fn send(&self, id: u64, url: String) {
		self.send_headers(id, url, Vec::new()).await;
	}

	async fn send_headers(&self, id: u64, url: String, headers: Vec<pb::HttpHeader>) {
		let scope = self.host.as_ref().map(|_| pb::InvocationScope {
			invocation_id: "http-invocation".to_owned(),
			effect_token: Bytes::from_static(b"http-token"),
			host_generation: 1,
			session_generation: self.session_generation,
			..Default::default()
		});
		self
			.requests
			.send_async(pb::ClientFrame {
				request_id: id,
				scope,
				body: Some(client_frame::Body::HttpRequest(pb::HttpRequest {
					method: "GET".to_owned(),
					url,
					headers,
					redirects: 10,
					timeout_ms: 2000,
					..Default::default()
				})),
				..Default::default()
			})
			.await
			.expect("HTTP frame");
	}

	async fn response(&self, id: u64) -> server_frame::Body {
		let response = time::timeout(Duration::from_secs(3), self.responses.recv_async())
			.await
			.expect("bounded HTTP completion")
			.expect("HTTP response");
		assert_eq!(response.request_id, id);
		response.body.expect("response body")
	}
}

impl Drop for Harness {
	fn drop(&mut self) {
		self.task.abort();
	}
}

fn denied(response: server_frame::Body) {
	assert!(
		matches!(response,server_frame::Body::Error(pb::ProtocolError{code,..}) if code==pb::ProtocolErrorCode::PermissionDenied as i32)
	);
}

#[tokio::test]
async fn native_http_denies_effectful_get_and_redirect_before_destination_effects() {
	let denied_destination = Destination::start(None, false).await;
	let allowed =
		Destination::start(Some(denied_destination.url("/effectful-get?secret=unpublished")), false)
			.await;
	let harness = Harness::new(policy(&[allowed.port]), true).await;
	harness
		.send(1, denied_destination.url("/effectful-get"))
		.await;
	let response = harness.response(1).await;
	assert_eq!(denied_destination.hits(), 0, "denied GET reached independent destination");
	denied(response);
	harness.send(2, allowed.url("/redirect")).await;
	denied(harness.response(2).await);
	harness.send(3, allowed.url("/ok")).await;
	assert!(matches!(
		harness.response(3).await,
		server_frame::Body::HttpResponse(pb::HttpResponse { status: 200, .. })
	));
	assert_eq!(allowed.hits(), 2);
	assert_eq!(denied_destination.hits(), 0, "denied GET must never reach its destination");
	// Model/control configuration changes cannot widen the captured authority.
	SV_NATIVE_HTTP_POLICY
		.set(&harness.con, sf!("{{\"mode\":\"open\"}}"))
		.expect("change console policy");
	harness
		.send(4, denied_destination.url("/effectful-get"))
		.await;
	denied(harness.response(4).await);
	assert_eq!(denied_destination.hits(), 0);
}

#[tokio::test]
async fn native_http_allows_relative_and_explicit_cross_origin_redirects() {
	let target = Destination::start(None, false).await;
	let cross = Destination::start(Some(target.url("/ok")), false).await;
	let relative = Destination::start(Some("/ok".to_owned()), false).await;
	let harness = Harness::new(policy(&[target.port, cross.port, relative.port]), false).await;
	for (id, destination) in [(1, &cross), (2, &relative)] {
		harness
			.send_headers(id, destination.url("/redirect"), vec![
				pb::HttpHeader {
					name: "Authorization".to_owned(),
					value: "Bearer private-token".to_owned(),
					..Default::default()
				},
				pb::HttpHeader {
					name: "Cookie".to_owned(),
					value: "private-cookie".to_owned(),
					..Default::default()
				},
				pb::HttpHeader {
					name: "Host".to_owned(),
					value: "unadmitted.test".to_owned(),
					..Default::default()
				},
			])
			.await;
		assert!(matches!(
			harness.response(id).await,
			server_frame::Body::HttpResponse(pb::HttpResponse { status: 200, .. })
		));
	}
	assert_eq!(cross.hits(), 1);
	assert_eq!(target.hits(), 1);
	assert_eq!(relative.hits(), 2);
	assert!(cross.received.lock()[0].contains("private-token"));
	assert!(!cross.received.lock()[0].contains("unadmitted.test"));
	assert!(!target.received.lock()[0].contains("private-token"));
	assert!(!target.received.lock()[0].contains("private-cookie"));
	assert!(
		relative.received.lock()[1].contains("private-token"),
		"same-origin redirect retains credentials"
	);
}

#[tokio::test]
async fn native_http_revocation_interrupts_held_redirect_and_preserves_destination_counter() {
	let target = Destination::start(None, false).await;
	let redirect = Destination::start(Some(target.url("/effectful-get")), true).await;
	let harness = Harness::new(policy(&[target.port, redirect.port]), true).await;
	harness.send(1, redirect.url("/redirect")).await;
	time::timeout(Duration::from_secs(2), redirect.started.notified())
		.await
		.expect("first destination");
	harness
		.server
		.authority
		.settle(harness.host.as_ref().expect("invocation host"), "http-invocation");
	denied(harness.response(1).await);
	redirect.release.notify_one();
	assert_eq!(redirect.hits(), 1);
	assert_eq!(target.hits(), 0);
	harness.send(2, target.url("/effectful-get")).await;
	assert!(matches!(harness.response(2).await, server_frame::Body::Error(_)));
	assert_eq!(target.hits(), 0);
}

#[tokio::test]
async fn native_http_cancel_is_processed_while_redirect_response_is_pending() {
	let target = Destination::start(None, false).await;
	let redirect = Destination::start(Some(target.url("/effectful-get")), true).await;
	let harness = Harness::new(policy(&[target.port, redirect.port]), false).await;
	harness.send(1, redirect.url("/redirect")).await;
	time::timeout(Duration::from_secs(2), redirect.started.notified())
		.await
		.expect("first destination");
	harness
		.requests
		.send_async(pb::ClientFrame {
			request_id: 0,
			body: Some(client_frame::Body::Cancel(pb::CancelRequest {
				target: Some(pb::cancel_request::Target::TargetRequestId(1)),
				..Default::default()
			})),
			..Default::default()
		})
		.await
		.expect("cancel");
	// This later request proves the connection consumed cancel before we release
	// the redirect; a sleeping client alone would not prove cancellation.
	harness
		.send(2, "ftp://invalid.test/private".to_owned())
		.await;
	assert!(
		matches!(harness.response(2).await,server_frame::Body::Error(pb::ProtocolError{code,..}) if code==pb::ProtocolErrorCode::InvalidArgument as i32)
	);
	redirect.release.notify_one();
	harness.send(3, target.url("/ok")).await;
	assert!(matches!(
		harness.response(3).await,
		server_frame::Body::HttpResponse(pb::HttpResponse { status: 200, .. })
	));
	assert_eq!(redirect.hits(), 1);
	assert_eq!(target.hits(), 1, "only the later benign request may arrive");
}

#[tokio::test]
async fn native_http_explicit_broad_owner_scope_preserves_existing_local_access() {
	let target = Destination::start(None, false).await;
	let harness =
		Harness::new(SandboxNetworkPolicy { mode: sf!("open"), ..Default::default() }, false).await;
	harness.send(1, target.url("/ok")).await;
	assert!(matches!(
		harness.response(1).await,
		server_frame::Body::HttpResponse(pb::HttpResponse { status: 200, .. })
	));
	assert_eq!(target.hits(), 1);
}

#[tokio::test]
async fn native_http_https_downgrade_never_reaches_plaintext_destination() {
	omp_http::install_tls_provider();
	use std::io::{Read, Write};
	let target = Destination::start(None, false).await;
	let certificate =
		rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()]).expect("test TLS identity");
	let der = certificate.cert.der().to_vec();
	let key = rustls::pki_types::PrivatePkcs8KeyDer::from(certificate.key_pair.serialize_der());
	let tls = rustls::ServerConfig::builder()
		.with_no_client_auth()
		.with_single_cert(vec![certificate.cert.der().clone()], key.into())
		.expect("TLS server config");
	let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("TLS listener");
	let port = listener.local_addr().expect("TLS address").port();
	let tls_hits = Arc::new(AtomicUsize::new(0));
	let hits = tls_hits.clone();
	let location = target.url("/effectful-get");
	let tls_task = tokio::task::spawn_blocking(move || {
		let (socket, _) = listener.accept().expect("TLS connection");
		socket
			.set_read_timeout(Some(Duration::from_secs(3)))
			.expect("TLS timeout");
		let connection = rustls::ServerConnection::new(Arc::new(tls)).expect("TLS state");
		let mut stream = rustls::StreamOwned::new(connection, socket);
		let mut request = Vec::new();
		while !request.ends_with(b"\r\n\r\n") {
			let mut byte = [0];
			stream.read_exact(&mut byte).expect("TLS request");
			request.push(byte[0]);
			assert!(request.len() < 16384);
		}
		hits.fetch_add(1, Ordering::SeqCst);
		write!(
			stream,
			"HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: \
			 close\r\n\r\n"
		)
		.expect("TLS redirect");
		stream.flush().expect("TLS flush");
	});
	let harness = Harness::with_tls_root(policy(&[port, target.port]), true, Some(der)).await;
	harness
		.send(1, format!("https://127.0.0.1:{port}/redirect"))
		.await;
	denied(harness.response(1).await);
	tls_task.await.expect("TLS server completed");
	assert_eq!(tls_hits.load(Ordering::SeqCst), 1, "TLS redirect must actually be received");
	assert_eq!(target.hits(), 0, "downgrade must not dispatch plaintext GET");
}

#[tokio::test]
async fn native_http_session_host_intersects_authenticated_project_baseline() {
	let permitted = Destination::start(None, false).await;
	let forbidden = Destination::start(None, false).await;
	let owner = Harness::new(policy(&[permitted.port]), false).await;
	let (owner_client, transport) = EnvClient::in_process(16);
	let owner_server = owner.server.clone();
	let serving = tokio::spawn(async move { owner_server.serve_in_process(transport).await });
	let hello = owner_client
		.hello(pb::ClientHello {
			client: "peer".to_owned(),
			schema_rev: omp_proto::SCHEMA_REV,
			..Default::default()
		})
		.await
		.expect("authenticated owner hello");
	assert!(!hello.native_http_policies_json.is_empty());
	let con = Arc::new(Ctx::new());
	SV_NATIVE_HTTP_POLICY
		.set(
			&con,
			Str::from(
				serde_json::to_string(&policy(&[permitted.port, forbidden.port])).expect("child scope"),
			),
		)
		.expect("configure child");
	let root = tempfile::tempdir().expect("retained scratch");
	let state = tempfile::tempdir().expect("peer state");
	let server = EnvServer::open_session_host(
		owner._root.path(),
		state.path(),
		Registry::new(),
		ExtHostConfig::new(
			PathBuf::from("unused"),
			Principal::new(sf!("test"), sf!("Test")),
			sf!("resumed-child"),
			2,
		),
		None,
		&con,
		Arc::new(crate::exthost::ConvarControlFactory::new(con.clone())),
		RegistryBridges::default(),
		owner_client,
	)
	.await
	.expect("real session host");
	assert!(server.environment_authorities().is_none(), "exercise slim production host");
	let peer = Harness::from_server(Arc::new(server), con, root, state, true, 2).await;
	peer.send(1, forbidden.url("/effectful-get")).await;
	denied(peer.response(1).await);
	peer.send(2, permitted.url("/ok")).await;
	assert!(matches!(
		peer.response(2).await,
		server_frame::Body::HttpResponse(pb::HttpResponse { status: 200, .. })
	));
	assert_eq!(permitted.hits(), 1);
	assert_eq!(forbidden.hits(), 0);
	// Replaying an old incarnation cannot use the resumed host's authority.
	peer
		.requests
		.send_async(pb::ClientFrame {
			request_id: 3,
			scope: Some(pb::InvocationScope {
				invocation_id: "http-invocation".to_owned(),
				effect_token: Bytes::from_static(b"http-token"),
				host_generation: 1,
				session_generation: 1,
				..Default::default()
			}),
			body: Some(client_frame::Body::HttpRequest(pb::HttpRequest {
				method: "GET".to_owned(),
				url: permitted.url("/effectful-get"),
				..Default::default()
			})),
			..Default::default()
		})
		.await
		.expect("stale request");
	assert!(
		matches!(peer.response(3).await,server_frame::Body::Error(pb::ProtocolError{code,..}) if code==pb::ProtocolErrorCode::PreconditionFailed as i32)
	);
	assert_eq!(permitted.hits(), 1);
	serving.abort();
}

#[tokio::test]
async fn native_http_rejects_malformed_redirect_without_leaking_url_secrets() {
	let malformed =
		Destination::start(Some("http://[?credential=private-value".to_owned()), false).await;
	let harness = Harness::new(policy(&[malformed.port]), true).await;
	harness.send(1, malformed.url("/redirect")).await;
	let server_frame::Body::Error(error) = harness.response(1).await else {
		panic!("expected malformed redirect error")
	};
	assert_eq!(error.code, pb::ProtocolErrorCode::InvalidArgument as i32);
	assert!(!format!("{error:?}").contains("private-value"));
	assert_eq!(malformed.hits(), 1);
}
