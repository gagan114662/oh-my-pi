//! Real loopback HTTP peers prove stream bounds, deadlines and socket teardown.

use std::{
	sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	},
	time::Duration,
};

use omp_http::{BodyError, read_bounded};
use tokio::{
	io::{AsyncReadExt as _, AsyncWriteExt as _},
	net::{TcpListener, TcpStream},
	task::JoinHandle,
	time::{sleep, timeout},
};

const CHUNK: usize = 16 * 1024;
const OFFERED: usize = 1024 * 1024 * 1024;
const WATCHDOG: Duration = Duration::from_secs(5);

struct SlowServer {
	url:    String,
	chunks: Arc<AtomicUsize>,
	task:   JoinHandle<bool>,
}

impl Drop for SlowServer {
	fn drop(&mut self) {
		self.task.abort();
	}
}

async fn request_headers(socket: &mut TcpStream) {
	let mut bytes = Vec::new();
	let mut buffer = [0; 1024];
	loop {
		let size = socket.read(&mut buffer).await.expect("request read");
		assert!(size > 0, "client closed before request");
		bytes.extend_from_slice(&buffer[..size]);
		assert!(bytes.len() <= 16 * 1024, "bounded request headers");
		if bytes.windows(4).any(|part| part == b"\r\n\r\n") {
			return;
		}
	}
}

impl SlowServer {
	async fn start(declared_size: bool) -> Self {
		let listener = TcpListener::bind("127.0.0.1:0")
			.await
			.expect("loopback listener");
		let url = format!("http://{}/slow", listener.local_addr().expect("address"));
		let chunks = Arc::new(AtomicUsize::new(0));
		let sent = Arc::clone(&chunks);
		let task = tokio::spawn(async move {
			let (mut socket, _) = listener.accept().await.expect("client");
			request_headers(&mut socket).await;
			let header = if declared_size {
				format!("HTTP/1.1 200 OK\r\nContent-Length: {OFFERED}\r\n\r\n")
			} else {
				"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_owned()
			};
			socket.write_all(header.as_bytes()).await.expect("headers");
			let (mut reader, mut writer) = socket.into_split();
			let payload = vec![b'x'; CHUNK];
			let mut packet = Vec::with_capacity(CHUNK + 16);
			if !declared_size {
				packet.extend_from_slice(format!("{CHUNK:x}\r\n").as_bytes());
			}
			packet.extend_from_slice(&payload);
			if !declared_size {
				packet.extend_from_slice(b"\r\n");
			}
			let mut probe = [0; 1];
			for _ in 0..OFFERED / CHUNK {
				tokio::select! {
					closed = reader.read(&mut probe) => return matches!(closed, Ok(0) | Err(_)),
					written = writer.write_all(&packet) => if written.is_err() { return true; },
				}
				sent.fetch_add(1, Ordering::Release);
				tokio::select! {
					closed = reader.read(&mut probe) => return matches!(closed, Ok(0) | Err(_)),
					() = sleep(Duration::from_millis(10)) => {},
				}
			}
			false
		});
		Self { url, chunks, task }
	}

	async fn assert_closed(&mut self) {
		assert!(
			timeout(WATCHDOG, &mut self.task)
				.await
				.expect("no orphan socket")
				.expect("server task")
		);
		let sent = self.chunks.load(Ordering::Acquire) * CHUNK;
		assert!(sent < OFFERED, "client must stop before the offered 1 GiB completes");
		println!("offered_bytes={OFFERED} sent_bytes={sent} peer_closed=true");
	}
}

#[tokio::test]
async fn declared_oversize_body_is_rejected_and_socket_closed() {
	let mut server = SlowServer::start(true).await;
	let response = omp_http::default_client()
		.get(&server.url)
		.timeout(WATCHDOG)
		.send()
		.await
		.expect("headers");
	assert!(matches!(
		read_bounded(response, CHUNK).await,
		Err(BodyError::TooLarge { limit: CHUNK })
	));
	server.assert_closed().await;
}

#[tokio::test]
async fn chunked_body_is_bounded_without_a_content_length() {
	let mut server = SlowServer::start(false).await;
	let response = omp_http::default_client()
		.get(&server.url)
		.timeout(WATCHDOG)
		.send()
		.await
		.expect("headers");
	assert_eq!(response.content_length(), None);
	assert!(matches!(
		read_bounded(response, CHUNK).await,
		Err(BodyError::TooLarge { limit: CHUNK })
	));
	server.assert_closed().await;
}

#[tokio::test]
async fn request_deadline_expires_while_body_keeps_arriving() {
	let mut server = SlowServer::start(false).await;
	let response = omp_http::default_client()
		.get(&server.url)
		.timeout(Duration::from_millis(500))
		.send()
		.await
		.expect("headers before deadline");
	let error = timeout(WATCHDOG, read_bounded(response, OFFERED))
		.await
		.expect("deadline bounded")
		.expect_err("slow body expires");
	assert!(matches!(error, BodyError::Transport(ref error) if error.is_timeout()));
	assert!(server.chunks.load(Ordering::Acquire) > 0, "deadline reached mid-body");
	server.assert_closed().await;
}

#[tokio::test]
async fn cancellation_mid_body_closes_the_slow_gib_stream() {
	let mut server = SlowServer::start(false).await;
	let mut response = omp_http::default_client()
		.get(&server.url)
		.timeout(WATCHDOG)
		.send()
		.await
		.expect("headers");
	let first = response
		.chunk()
		.await
		.expect("body read")
		.expect("first body chunk");
	assert!(!first.is_empty(), "cancellation happens after actual body consumption");
	let task = tokio::spawn(read_bounded(response, OFFERED));
	tokio::task::yield_now().await;
	task.abort();
	assert!(task.await.expect_err("request cancelled").is_cancelled());
	server.assert_closed().await;
}

#[tokio::test]
async fn default_follows_redirect_but_disabled_pool_returns_location() {
	let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
	let address = listener.local_addr().expect("address");
	let server = tokio::spawn(async move {
		for reply in [
			"HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
			"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
			"HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
		] {
			let (mut socket, _) = listener.accept().await.expect("request");
			request_headers(&mut socket).await;
			socket.write_all(reply.as_bytes()).await.expect("reply");
		}
	});
	// AbortOnDrop owns cleanup even if an assertion fails before the final join.
	let mut server = AbortOnDrop(server);
	let url = format!("http://{address}/redirect");
	let followed = omp_http::default_client()
		.get(&url)
		.timeout(WATCHDOG)
		.send()
		.await
		.expect("redirect response");
	assert_eq!(followed.status(), 200);
	assert_eq!(followed.url().path(), "/final");
	assert_eq!(
		read_bounded(followed, 2)
			.await
			.expect("exact limit")
			.as_ref(),
		b"ok"
	);
	let disabled = omp_http::no_redirect_client()
		.get(&url)
		.timeout(WATCHDOG)
		.send()
		.await
		.expect("unfollowed redirect");
	assert_eq!(disabled.status(), 302);
	assert_eq!(disabled.headers()["location"], "/final");
	timeout(WATCHDOG, async { (&mut server.0).await.expect("server") })
		.await
		.expect("server completion");
}

struct AbortOnDrop(JoinHandle<()>);
impl Drop for AbortOnDrop {
	fn drop(&mut self) {
		self.0.abort();
	}
}
