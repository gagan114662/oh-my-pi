//! P6: a real OMP process killed during a streamed turn can lose only its
//! unfinished suffix, and the CLI resumes the durable prefix.

#![cfg(unix)]

use std::{
	fs,
	future::Future,
	io::{BufRead as _, BufReader, Write as _},
	os::{fd, unix::net::UnixStream},
	path::Path,
	pin::Pin,
	sync::{
		Arc, Mutex,
		atomic::{AtomicBool, Ordering},
	},
	task::{Context, Poll},
	thread,
	time::{Duration, Instant},
};

use flume::{Receiver, Sender};
use futures::StreamExt as _;
use nix::{
	errno::Errno,
	fcntl::{FcntlArg, OFlag, fcntl},
	pty::{Winsize, openpty},
	sys::signal,
	unistd::{Pid, ttyname},
};
use omp_ai::{
	Answer, Error as InferenceError, Registry,
	answer::{AnswerBody, ChatStream},
	call::Call,
	event::{BlockKind, ChatEvent, Completion, FinishReason, WorkflowResponse},
	layer::{LayerCall, stack::RouteProviderService},
	provider::fake::{FakeProvider, FakeScript},
	receipt::{ExecutionReceipt, ReasonId, Usage},
	registry::RouteUnavailable,
	session::ConversationSessionPlanner,
};
use omp_app::{
	daemon::{DaemonConfig, DaemonHandle},
	endpoint::LocalEndpoint,
};
use omp_catalog::{
	ManagementCapabilities, OperationBits, OperationKind,
	snapshot::{Catalog, SnapshotProvenance},
};
use omp_core::{Str, sf};
use omp_e2e::support::{
	OwnedProcess, create_session, install_omp_binary_env, omp_binary, reopen_session, within,
};
use omp_tool::Registry as ToolRegistry;
use serde_json::{Value, json};
use tokio::{process::Command, time};
use tower::Service;

const READY_TIMEOUT: Duration = Duration::from_secs(30);
const JOURNAL_TIMEOUT: Duration = Duration::from_secs(3);
const IO_TIMEOUT: Duration = Duration::from_secs(2);
const PREFIX: &str = "durable streamed prefix";
const LOST_SUFFIX: &str = " suffix that must not appear";

// Written even during a failed proof: missing/unfinished phases must never
// become zero-duration successes in the published evidence.
struct P6Timings {
	data:   Value,
	active: Option<(&'static str, Instant)>,
}

impl P6Timings {
	fn new() -> Self {
		Self { data: json!({"completed": false, "journal": null, "resume": null}), active: None }
	}

	fn begin(&mut self, phase: &'static str, bound: Duration) {
		assert!(self.active.is_none(), "timing phases cannot overlap");
		self.data[phase] = json!({"bound_ms": bound.as_secs_f64() * 1000.0, "completed": false});
		self.active = Some((phase, Instant::now()));
		self.publish();
	}

	fn end(&mut self, completed: bool) {
		let (phase, started) = self.active.take().expect("active timing phase");
		self.data[phase]["elapsed_ms"] = json!(started.elapsed().as_secs_f64() * 1000.0);
		self.data[phase]["completed"] = json!(completed);
		self.publish();
	}

	fn publish(&self) {
		println!("P6 timings: {}", self.data);
		if let Some(path) = std::env::var_os("OMP_P6_LATENCY_PATH") {
			let result = fs::write(path, serde_json::to_vec_pretty(&self.data).expect("timing JSON"));
			if thread::panicking() {
				if let Err(error) = result {
					eprintln!("could not retain failed P6 timings: {error}");
				}
			} else {
				result.expect("publish P6 timings");
			}
		}
	}
}

impl Drop for P6Timings {
	fn drop(&mut self) {
		if thread::panicking() {
			self.data["completed"] = json!(false);
		}
		if self.active.is_some() {
			self.end(false);
		}
		self.publish();
	}
}

#[derive(Clone)]
struct CrashRoute {
	fake:     FakeProvider,
	streamed: Sender<()>,
	release:  Receiver<()>,
}

impl Service<LayerCall<Call>> for CrashRoute {
	type Error = InferenceError;
	type Future = Pin<Box<dyn Future<Output = Result<Answer, InferenceError>> + Send>>;
	type Response = Answer;

	fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
		<FakeProvider as Service<Call>>::poll_ready(&mut self.fake, context)
	}

	fn call(&mut self, request: LayerCall<Call>) -> Self::Future {
		let response = <FakeProvider as Service<Call>>::call(&mut self.fake, request.payload);
		let streamed = self.streamed.clone();
		let release = self.release.clone();
		Box::pin(async move {
			let Answer { meta, receipt, body } = response.await?;
			let body = match body {
				AnswerBody::Chat(mut chat) => {
					let events = async_stream::stream! {
						let mut pause_after_prefix = true;
						while let Some(event) = chat.next().await {
							let pause = pause_after_prefix
								&& matches!(&event, Ok(ChatEvent::TextDelta { text, .. }) if text.as_str() == PREFIX);
							yield event;
							if pause {
								pause_after_prefix = false;
								streamed.send_async(()).await.expect("crash observer remains open");
								release.recv_async().await.expect("crash gate remains open");
							}
						}
					};
					AnswerBody::Chat(ChatStream::ordinary(Box::pin(events)))
				},
				body => body,
			};
			Ok(Answer { meta, receipt, body })
		})
	}
}

struct CrashGateway {
	_handle:    DaemonHandle,
	model:      String,
	streamed:   Receiver<()>,
	release:    Sender<()>,
	_responses: Receiver<WorkflowResponse>,
}

impl CrashGateway {
	async fn start(scratch: &Path, socket: &Path) -> Self {
		let mut compiled = Catalog::embedded().compiled().clone();
		for provider in &mut compiled.providers {
			provider.management = ManagementCapabilities {
				operations:        OperationBits::empty(),
				multiple_accounts: false,
				refresh:           false,
				principal_quota:   false,
			};
		}
		let artifacts = Catalog::encode(compiled, SnapshotProvenance { source_digest: [0; 32] })
			.expect("catalog snapshot");
		let catalog = Arc::new(Catalog::decode(&artifacts.postcard).expect("catalog decode"));
		let model = catalog
			.models()
			.iter()
			.find(|candidate| {
				candidate
					.capabilities
					.operations
					.contains_kind(OperationKind::Chat)
			})
			.expect("chat model");
		let model_key = model.key.as_str().to_owned();
		let route_id = model.routes.first().expect("chat route").clone();
		let route = catalog.route(&route_id).expect("selected route");
		let fake = FakeProvider::new(route.provider.clone(), route_id.clone());
		fake.extend([FakeScript::chat(vec![
			Ok(ChatEvent::BlockStarted { index: 0, kind: BlockKind::Text }),
			Ok(ChatEvent::TextDelta { index: 0, text: Str::new_static(PREFIX) }),
			Ok(ChatEvent::TextDelta { index: 0, text: Str::new_static(LOST_SUFFIX) }),
			Ok(ChatEvent::Completed(Completion {
				reason:  FinishReason::Stop,
				blocks:  1,
				usage:   Usage::default(),
				receipt: ExecutionReceipt::default().into(),
			})),
		])]);
		let (streamed_tx, streamed) = flume::bounded(1);
		let (release, release_rx) = flume::bounded(1);
		let route_service =
			RouteProviderService::new(CrashRoute { fake, streamed: streamed_tx, release: release_rx });
		let mut builder = Registry::builder(Arc::clone(&catalog));
		for candidate in catalog.routes() {
			builder = if candidate.id == route_id {
				builder
					.register_route(candidate.id.clone(), route_service.clone())
					.expect("crash route registers")
			} else {
				builder
					.register_unavailable(RouteUnavailable::new(
						candidate.id.clone(),
						ReasonId(sf!("p6-scripted-route-only")),
						None,
					))
					.expect("unavailable route registers")
			};
		}
		let sessions = ConversationSessionPlanner::open(scratch.join("gateway-sessions.db"), catalog)
			.expect("conversation store opens");
		let (responses, incoming) = flume::bounded(8);
		let handle = time::timeout(
			READY_TIMEOUT,
			DaemonHandle::start_for_test(
				DaemonConfig::local(LocalEndpoint::from(socket.to_path_buf()))
					.with_data_dir(scratch.join("gateway-state")),
				builder.build().expect("inference registry"),
				sessions,
				Arc::new(ToolRegistry::new()),
				responses,
			),
		)
		.await
		.expect("gateway startup timed out")
		.expect("scripted gateway starts");
		Self { _handle: handle, model: model_key, streamed, release, _responses: incoming }
	}
}

const PTY_TAIL_LIMIT: usize = 64 * 1024;

struct PtyDrain {
	tail:   Arc<Mutex<Vec<u8>>>,
	stop:   Arc<AtomicBool>,
	reader: Option<thread::JoinHandle<Result<usize, Errno>>>,
}

impl PtyDrain {
	fn start(master: fd::OwnedFd) -> Self {
		fcntl(&master, FcntlArg::F_SETFL(OFlag::O_NONBLOCK)).expect("nonblocking crash PTY");
		let stop = Arc::new(AtomicBool::new(false));
		let reader_stop = Arc::clone(&stop);
		let tail = Arc::new(Mutex::new(Vec::new()));
		let reader_tail = Arc::clone(&tail);
		let reader = thread::spawn(move || {
			let mut buffer = [0_u8; 16 * 1024];
			let mut bytes = 0;
			while !reader_stop.load(Ordering::Acquire) {
				match nix::unistd::read(&master, &mut buffer) {
					Ok(0) | Err(Errno::EAGAIN) => thread::sleep(Duration::from_millis(5)),
					Ok(count) => {
						bytes += count;
						let mut tail = reader_tail.lock().expect("PTY tail lock");
						tail.extend_from_slice(&buffer[..count]);
						let overflow = tail.len().saturating_sub(PTY_TAIL_LIMIT);
						tail.drain(..overflow);
					},
					Err(Errno::EINTR) => {},
					Err(Errno::EIO) => break,
					Err(error) => return Err(error),
				}
			}
			Ok(bytes)
		});
		Self { stop, tail, reader: Some(reader) }
	}

	fn finish(&mut self) -> usize {
		self.stop.store(true, Ordering::Release);
		self
			.reader
			.take()
			.expect("PTY reader owned")
			.join()
			.expect("PTY reader joins")
			.expect("PTY output read succeeds")
	}
}

impl Drop for PtyDrain {
	fn drop(&mut self) {
		self.stop.store(true, Ordering::Release);
		if let Some(reader) = self.reader.take() {
			let _ = reader.join();
		}
	}
}

struct ChatProcess {
	process: OwnedProcess,
	drain:   PtyDrain,
	_slave:  fd::OwnedFd,
}

fn spawn_chat(
	binary: &Path,
	project: &Path,
	home: &Path,
	gateway: &Path,
	model: &str,
	sessions: &Path,
	session: &Path,
	debug: &Path,
	resume: bool,
) -> ChatProcess {
	let window = Winsize { ws_row: 40, ws_col: 100, ws_xpixel: 0, ws_ypixel: 0 };
	let pty = openpty(Some(&window), None).expect("open chat PTY");
	let device = ttyname(&pty.slave).expect("PTY slave path");
	// A real terminal consumes rendered bytes. Leaving this unread can fill
	// the PTY and block repaint, including the debug frame response.
	let drain = PtyDrain::start(pty.master);
	let mut command = Command::new(binary);
	command
		.arg("chat")
		.arg("--no-ext")
		.arg("--no-tools")
		.arg("--model")
		.arg(model)
		.arg("--project")
		.arg(project)
		.arg("--gateway")
		.arg(gateway)
		.arg("--session-dir")
		.arg(sessions)
		.arg("--envd-idle-timeout")
		.arg("2");
	if resume {
		command.arg("-c");
	} else {
		command
			.arg("--session")
			.arg(session)
			.arg("stream until killed");
	}
	command
		.current_dir(project)
		.env("TERM", "xterm-256color")
		.env("NO_COLOR", "1")
		.env("HOME", home)
		.env("OMP_CONFIG_DIR", home.join("config"))
		.env("OMP_DATA_DIR", home.join("data"))
		.env("OMP_STATE_DIR", home.join("state"))
		.env("OMP_CACHE_DIR", home.join("cache"))
		.env("OMP_TTY", &device)
		.env("OMP_TUI_DEBUG", debug);
	let process = OwnedProcess::spawn(command).expect("spawn real OMP chat");
	ChatProcess { process, drain, _slave: pty.slave }
}

#[derive(Debug, thiserror::Error)]
enum DebugRequestError {
	#[error("debug socket connection failed: {0}")]
	Connect(#[source] std::io::Error),
	#[error("debug socket configuration failed: {0}")]
	Configure(#[source] std::io::Error),
	#[error("debug request encoding failed: {0}")]
	Encode(#[source] serde_json::Error),
	#[error("debug request write failed: {0}")]
	Write(#[source] std::io::Error),
	#[error("debug socket connected, but response read failed: {0}")]
	Read(#[source] std::io::Error),
	#[error("debug response decoding failed: {0}")]
	Decode(#[source] serde_json::Error),
	#[error("debug server rejected request: {0}")]
	Rejected(Value),
}

fn debug_request(path: &Path, request: &Value) -> Result<Value, DebugRequestError> {
	let stream = UnixStream::connect(path).map_err(DebugRequestError::Connect)?;
	stream
		.set_read_timeout(Some(IO_TIMEOUT))
		.map_err(DebugRequestError::Configure)?;
	stream
		.set_write_timeout(Some(IO_TIMEOUT))
		.map_err(DebugRequestError::Configure)?;
	let mut writer = stream.try_clone().map_err(DebugRequestError::Configure)?;
	serde_json::to_writer(&mut writer, request).map_err(DebugRequestError::Encode)?;
	writer.write_all(b"\n").map_err(DebugRequestError::Write)?;
	writer.flush().map_err(DebugRequestError::Write)?;
	let mut line = String::new();
	BufReader::new(stream)
		.read_line(&mut line)
		.map_err(DebugRequestError::Read)?;
	let response: Value = serde_json::from_str(&line).map_err(DebugRequestError::Decode)?;
	if response.get("ok").and_then(Value::as_bool) == Some(true) {
		Ok(response)
	} else {
		Err(DebugRequestError::Rejected(response))
	}
}

fn wait_for_resumed_frame(path: &Path, process: &mut OwnedProcess) -> String {
	let started = Instant::now();
	let deadline = started + READY_TIMEOUT;
	let mut problem;
	loop {
		if let Some(status) = process.try_wait().expect("inspect resumed process") {
			panic!(
				"resumed OMP exited before its frame became ready after {:?}: {status}",
				started.elapsed()
			);
		}

		match debug_request(path, &json!({ "op": "frame" })) {
			Ok(response) => {
				let frame = response
					.get("lines")
					.and_then(Value::as_array)
					.into_iter()
					.flatten()
					.filter_map(Value::as_str)
					.collect::<Vec<_>>()
					.join("\n");
				if frame.contains("stream until killed") && frame.contains(PREFIX) {
					return frame;
				}
				problem = format!("resumed frame did not contain durable blocks:\n{frame}");
			},
			Err(error) => problem = error.to_string(),
		}
		assert!(
			Instant::now() < deadline,
			"resumed process was running at last check; elapsed {:?}; frame readiness bound {:?}; \
			 last observation: {problem}",
			started.elapsed(),
			READY_TIMEOUT
		);
		std::thread::sleep(Duration::from_millis(20));
	}
}

// Capture before process teardown destroys the useful state. This is performed
// only after the original wait failed; it does not extend or retry acceptance.
async fn retain_shutdown_failure(resumed: &ChatProcess, debug: &Path, journal: &Path) {
	let pid = resumed.process.id();
	let mut observation = json!({"pid": pid, "process_group": resumed.process.process_group()});
	if let Some(pid) = pid {
		let mut command = Command::new("ps");
		command.args(["-p", &pid.to_string(), "-o", "pid=,ppid=,pgid=,state=,wchan=,comm="]);
		command.kill_on_drop(true);
		observation["process_state"] = match time::timeout(IO_TIMEOUT, command.output()).await {
			Ok(Ok(output)) => json!({
				 "status": output.status.code(),
				 "stdout": String::from_utf8_lossy(&output.stdout),
				 "stderr": String::from_utf8_lossy(&output.stderr),
			}),
			Ok(Err(error)) => json!({"error": error.to_string()}),
			Err(error) => json!({"error": error.to_string()}),
		};
	}
	observation["frame_after_timeout"] = match debug_request(debug, &json!({"op": "frame"})) {
		Ok(frame) => frame,
		Err(error) => json!({"error": error.to_string()}),
	};
	let tail = resumed.drain.tail.lock().expect("PTY tail lock").clone();
	observation["pty_tail"] = json!(String::from_utf8_lossy(&tail));
	observation["pty_tail_limit"] = json!(PTY_TAIL_LIMIT);
	// This fixture contains only synthetic input and provider output.
	observation["journal"] = match fs::read_to_string(journal) {
		Ok(journal) => json!(journal),
		Err(error) => json!({"error": error.to_string()}),
	};
	eprintln!("P6 shutdown failure: {observation}");
	if let Some(path) = std::env::var_os("OMP_P6_LATENCY_PATH") {
		let path = std::path::PathBuf::from(path).with_extension("shutdown.json");
		if let Err(error) =
			fs::write(path, serde_json::to_vec_pretty(&observation).expect("diagnostic JSON"))
		{
			eprintln!("could not retain P6 shutdown failure: {error}");
		}
	}
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p6_killed_real_streaming_omp_resumes_durable_prefix_through_cli() {
	let mut timings = P6Timings::new();
	install_omp_binary_env().expect("install real OMP binary");
	let scratch = tempfile::tempdir().expect("P6 scratch");
	let project = scratch.path().join("project");
	let home = scratch.path().join("home");
	let sessions = scratch.path().join("sessions");
	fs::create_dir_all(&project).expect("project directory");
	fs::create_dir_all(&home).expect("isolated home");
	fs::create_dir_all(&sessions).expect("session directory");
	let project = fs::canonicalize(project).expect("canonical project");
	let session = sessions.join("crash.oms");
	let gateway_socket = scratch.path().join("gateway.sock");
	let first_debug = scratch.path().join("first-debug.sock");
	let resume_debug = scratch.path().join("resume-debug.sock");
	let gateway = CrashGateway::start(scratch.path(), &gateway_socket).await;
	let binary = omp_binary().expect("locate real OMP binary");

	let mut crashed = spawn_chat(
		&binary,
		&project,
		&home,
		&gateway_socket,
		&gateway.model,
		&sessions,
		&session,
		&first_debug,
		false,
	);
	within("provider prefix reaches OMP", READY_TIMEOUT, gateway.streamed.recv_async())
		.await
		.expect("prefix timeout")
		.expect("prefix gate remains open");
	timings.begin("journal", JOURNAL_TIMEOUT);
	let journal_result = within("stream prefix reaches journal", JOURNAL_TIMEOUT, async {
		loop {
			if fs::read_to_string(&session).is_ok_and(|journal| journal.contains(PREFIX)) {
				break;
			}
			time::sleep(Duration::from_millis(5)).await;
		}
	})
	.await;
	timings.end(journal_result.is_ok());
	journal_result.expect("journal prefix timeout");
	let group = crashed.process.process_group().expect("OMP process group");
	signal::killpg(Pid::from_raw(group), Some(signal::Signal::SIGKILL))
		.expect("crash OMP process group");
	let status = crashed
		.process
		.wait(Duration::from_secs(3))
		.await
		.expect("reap crashed OMP");
	use std::os::unix::process::ExitStatusExt as _;
	assert_eq!(status.signal(), Some(libc::SIGKILL), "OMP was not killed by SIGKILL");
	println!("crashed terminal drained {} bytes", crashed.drain.finish());
	gateway
		.release
		.send(())
		.expect("release crashed provider call");

	let journal = fs::read_to_string(&session).expect("crashed journal remains readable");
	assert!(journal.contains("stream until killed"), "committed user block was lost");
	assert!(journal.contains(PREFIX), "committed streamed prefix was lost");
	assert!(!journal.contains(LOST_SUFFIX), "unseen provider suffix was invented");
	assert!(!journal.contains("event: msg.assistant.end@1"));
	assert!(!journal.contains("event: turn.receipt@1"));

	timings.begin("resume", READY_TIMEOUT);
	let mut resumed = spawn_chat(
		&binary,
		&project,
		&home,
		&gateway_socket,
		&gateway.model,
		&sessions,
		&session,
		&resume_debug,
		true,
	);
	let frame = wait_for_resumed_frame(&resume_debug, &mut resumed.process);
	timings.end(true);
	assert!(!frame.contains(LOST_SUFFIX), "resumed host displayed an uncommitted suffix\n{frame}");
	debug_request(&resume_debug, &json!({ "op": "keys", "keys": "ctrl+c ctrl+c" }))
		.expect("quit resumed chat through its real input path");
	let quit_started = Instant::now();
	let quit_result = resumed.process.wait(READY_TIMEOUT).await;
	timings.data["quit"] = json!({
		 "bound_ms": READY_TIMEOUT.as_secs_f64() * 1000.0,
		 "elapsed_ms": quit_started.elapsed().as_secs_f64() * 1000.0,
		 "completed": quit_result.as_ref().is_ok_and(|status| status.success()),
	});
	timings.publish();
	if !quit_result.as_ref().is_ok_and(|status| status.success()) {
		retain_shutdown_failure(&resumed, &resume_debug, &session).await;
	}
	let status = quit_result.expect("resumed OMP exits");
	assert!(status.success(), "resumed OMP did not exit cleanly: {status}");
	println!("resumed terminal drained {} bytes", resumed.drain.finish());
	timings.data["completed"] = json!(true);
}

#[test]
fn p6_resume_preserves_open_stream_prefix_without_inventing_completion() {
	let temp = tempfile::tempdir().expect("P6 scratch");
	let path = temp.path().join("open-stream.oms");
	let mut live = create_session(&path).expect("session");
	live.begin_turn().expect("turn");
	live.user("resume", Vec::new()).expect("user");
	live
		.assistant_start("model", "provider", "route")
		.expect("assistant");
	let assistant = live
		.dom()
		.select("body turn assistant")
		.expect("selector")
		.next()
		.expect("assistant handle");
	let sid = live
		.stream_open(assistant, omp_dom::PropKey::from(omp_dom::PropId::Text))
		.expect("stream");
	live.stream_append(sid, "visible").expect("append");
	let expected = live.dom().snapshot();
	drop(live);
	let replay = reopen_session(&path).expect("resume");
	assert_eq!(replay.dom().snapshot(), expected);
	let journal = fs::read_to_string(path).expect("journal");
	assert!(!journal.contains("event: msg.assistant.end@1"));
	assert!(!journal.contains("event: turn.receipt@1"));
}
