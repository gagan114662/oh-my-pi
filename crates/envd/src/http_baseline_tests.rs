//! Enhancement baseline: an older host has broad env.net but no native scope
//! setting. This fixture is copied unchanged into both revisions. Only the
//! expected observation differs; no destination admission implementation is
//! copied into the older source.

use std::sync::atomic::{AtomicUsize, Ordering};

use omp_core::Principal;
use tokio::{
	io::{AsyncReadExt, AsyncWriteExt},
	net::TcpListener,
};

use super::*;

#[tokio::test]
async fn native_http_parent_and_head_effectful_get_observation() {
	let expectation = std::env::var("OMP_HTTP_BASELINE_EXPECTATION")
		.unwrap_or_else(|_| "restricted-head".to_owned());
	assert!(matches!(expectation.as_str(), "broad-parent" | "restricted-head"));
	// The destination binds an ephemeral port and the observation reports the
	// actual URL, so no fixture destination is hardcoded. A bind failure is an
	// infrastructure failure, never evidence of denied or permitted egress.
	let listener = TcpListener::bind("127.0.0.1:0")
		.await
		.expect("independent destination");
	let port = listener.local_addr().expect("destination address").port();
	let hits = Arc::new(AtomicUsize::new(0));
	let destination = tokio::spawn({
		let hits = hits.clone();
		async move {
			loop {
				let (mut stream, _) = listener.accept().await.expect("destination accept");
				let mut headers = Vec::new();
				while !headers.ends_with(b"\r\n\r\n") {
					let mut byte = [0];
					stream
						.read_exact(&mut byte)
						.await
						.expect("destination header");
					headers.push(byte[0]);
					assert!(headers.len() < 16384);
				}
				assert!(headers.starts_with(b"GET /mutate?value=1 HTTP/1.1\r\n"));
				hits.fetch_add(1, Ordering::SeqCst);
				stream
					.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
					.await
					.expect("destination response");
			}
		}
	});
	let root = tempfile::tempdir().expect("workspace");
	let state = tempfile::tempdir().expect("state");
	let con = Arc::new(Ctx::new());
	let configured = match con.set_typed("sv_native_http_policy", Str::from(r#"{"mode":"deny"}"#)) {
		Ok(_) => true,
		Err(omp_con::ConError::Unknown { name }) if name.as_str() == "sv_native_http_policy" => false,
		Err(error) => panic!("unexpected owner configuration failure: {error}"),
	};
	let server = Arc::new(
		EnvServer::open_local(
			root.path(),
			state.path(),
			Registry::new(),
			ExtHostConfig::new(
				PathBuf::from("unused"),
				Principal::new(sf!("test"), sf!("Test")),
				sf!("baseline-http"),
				1,
			),
			&con,
			Arc::new(crate::exthost::ConvarControlFactory::new(con.clone())),
			RegistryBridges::default(),
		)
		.await
		.expect("real native environment host"),
	);
	let host = HostKey::new("workspace", "baseline", "native-http");
	server
		.authority
		.register_host(host.clone(), Grants::supported(["env.net"]));
	server
		.authority
		.open(host.clone(), sf!("baseline-invocation"));
	server
		.authority
		.authorize(
			&host,
			"baseline-invocation",
			Bytes::from_static(b"baseline-token"),
			Grants::supported(["env.net"]),
			1,
			1,
			1,
		)
		.expect("real invocation authorization");
	let (requests, request_rx) = flume::bounded(8);
	let (response_tx, responses) = flume::bounded(8);
	let serving = tokio::spawn(async move {
		server
			.serve_frames(request_rx, response_tx, ConnectionPolicy::extension(host, ["env.net"]))
			.await;
	});
	requests
		.send_async(pb::ClientFrame {
			request_id: 0,
			body: Some(client_frame::Body::Hello(pb::ClientHello {
				client: "baseline-http".to_owned(),
				schema_rev: omp_proto::SCHEMA_REV,
				capabilities: vec!["env.net".to_owned()],
				..Default::default()
			})),
			..Default::default()
		})
		.await
		.expect("client hello");
	assert!(matches!(
		time::timeout(Duration::from_secs(3), responses.recv_async())
			.await
			.expect("hello timeout")
			.expect("hello response")
			.body,
		Some(server_frame::Body::Hello(_))
	));
	let url = format!("http://127.0.0.1:{port}/mutate?value=1");
	requests
		.send_async(pb::ClientFrame {
			request_id: 1,
			scope: Some(pb::InvocationScope {
				invocation_id: "baseline-invocation".to_owned(),
				effect_token: Bytes::from_static(b"baseline-token"),
				host_generation: 1,
				session_generation: 1,
				..Default::default()
			}),
			body: Some(client_frame::Body::HttpRequest(pb::HttpRequest {
				method: "GET".to_owned(),
				url: url.clone(),
				redirects: 0,
				timeout_ms: 2000,
				..Default::default()
			})),
			..Default::default()
		})
		.await
		.expect("native HTTP frame");
	let response = time::timeout(Duration::from_secs(3), responses.recv_async())
		.await
		.expect("HTTP timeout")
		.expect("HTTP response");
	assert_eq!(response.request_id, 1);
	let observation = match response.body {
		Some(server_frame::Body::HttpResponse(pb::HttpResponse { status: 200, .. })) => "http-200",
		Some(server_frame::Body::Error(pb::ProtocolError { code, .. }))
			if code == pb::ProtocolErrorCode::PermissionDenied as i32 =>
		{
			"permission-denied"
		},
		body => panic!("unexpected native HTTP response: {body:?}"),
	};
	let counter = hits.load(Ordering::SeqCst);
	eprintln!(
		"HTTP_BASELINE_OBSERVATION={}",
		serde_json::json!({"expectation": expectation, "method": "GET", "url": url, "capability": "env.net", "invocation": "baseline-invocation", "configuration_supported": configured, "response": observation, "destination_counter": counter})
	);
	destination.abort();
	serving.abort();
	if expectation == "broad-parent" {
		assert!(!configured, "parent must lack the new scope configuration");
		assert_eq!(observation, "http-200");
		assert_eq!(counter, 1, "broad parent must reach independent effectful destination");
	} else {
		assert!(configured, "head must accept the trusted owner restriction");
		assert_eq!(observation, "permission-denied");
		assert_eq!(counter, 0, "restricted head must not reach effectful destination");
	}
}
