//! Actual executable verification must be read-only and fail closed.
use std::{
	fs,
	path::Path,
	process::{Child, Command, ExitStatus, Stdio},
	thread,
	time::{Duration, Instant},
};

use clap::Parser as _;
use omp_app::cli::{Command as AppCommand, OmpCli, SessionCommand};
use omp_core::Hash32;
use omp_journal::{Journal, Kind, KindName, sse::Scanner};
use omp_session::{ComponentRegistry, Session};

#[test]
fn verification_cli_requires_path_and_valid_independent_tip() {
	assert!(OmpCli::try_parse_from(["omp", "session", "verify"]).is_err());
	assert!(
		OmpCli::try_parse_from([
			"omp",
			"session",
			"verify",
			"file.oms",
			"--expected-tip",
			"not-a-digest"
		])
		.is_err()
	);
	let tip = Hash32::sum(b"independently recorded tip").to_string();
	let cli = OmpCli::try_parse_from([
		"omp",
		"session",
		"verify",
		"file.oms",
		"--expected-tip",
		&tip,
		"--json",
	])
	.expect("parse");
	let Some(AppCommand::Session(args)) = cli.command else {
		panic!("session dispatch")
	};
	let SessionCommand::Verify(args) = args.command;
	assert_eq!(args.path, Path::new("file.oms"));
	assert_eq!(args.expected_tip, Some(Hash32::sum(b"independently recorded tip")));
	assert!(args.json);
}

struct OwnedVerifier {
	child:  Child,
	reaped: bool,
}

impl OwnedVerifier {
	fn poll_until(&mut self, deadline: Instant) -> std::io::Result<Option<ExitStatus>> {
		loop {
			if let Some(status) = self.child.try_wait()? {
				self.reaped = true;
				return Ok(Some(status));
			}
			if Instant::now() >= deadline {
				return Ok(None);
			}
			thread::sleep(Duration::from_millis(20));
		}
	}

	fn stop(&mut self) -> std::io::Result<()> {
		if self.reaped {
			return Ok(());
		}
		let kill = self.child.kill();
		if self
			.poll_until(Instant::now() + Duration::from_secs(5))?
			.is_some()
		{
			return Ok(());
		}
		kill?;
		Err(std::io::Error::new(
			std::io::ErrorKind::TimedOut,
			"verifier did not reap within five seconds after kill",
		))
	}
}

impl Drop for OwnedVerifier {
	fn drop(&mut self) {
		if let Err(error) = self.stop() {
			eprintln!("verifier cleanup failed: {error}");
		}
	}
}

fn evidence(stage: &str, name: &str, bytes: &[u8]) {
	if let Some(root) = std::env::var_os("OMP_JOURNAL_PROOF_DIR") {
		let directory = std::path::PathBuf::from(root).join(stage);
		fs::create_dir_all(&directory).expect("proof directory");
		fs::write(directory.join(name), bytes).expect("proof artifact");
	}
}

fn execute(path: &Path, extra: &[&str], stage: &str) -> (ExitStatus, String, String) {
	let isolated = tempfile::tempdir().expect("isolated CLI environment");
	let out = isolated.path().join("stdout");
	let err = isolated.path().join("stderr");
	let mut command = Command::new(env!("CARGO_BIN_EXE_omp"));
	command.env_clear().current_dir(isolated.path());
	if let Some(path) = std::env::var_os("PATH") {
		command.env("PATH", path);
	}
	for (variable, directory) in [
		("HOME", "home"),
		("OMP_CONFIG_DIR", "config"),
		("OMP_DATA_DIR", "data"),
		("XDG_CONFIG_HOME", "config"),
		("XDG_DATA_HOME", "data"),
		("XDG_CACHE_HOME", "cache"),
		("XDG_STATE_HOME", "state"),
		("TMPDIR", "tmp"),
	] {
		let root = isolated.path().join(directory);
		fs::create_dir_all(&root).expect("isolated directory");
		command.env(variable, root);
	}
	let child = command
		.args(["session", "verify"])
		.arg(path)
		.args(extra)
		.stdin(Stdio::null())
		.stdout(fs::File::create(&out).expect("stdout"))
		.stderr(fs::File::create(&err).expect("stderr"))
		.spawn();
	let child = match child {
		Ok(child) => child,
		Err(error) => {
			let retained = isolated.keep();
			evidence(
				stage,
				"failure.txt",
				format!("spawn failed: {error}; retained {}", retained.display()).as_bytes(),
			);
			panic!("spawn failed: {error}; logs retained in {}", retained.display());
		},
	};
	let mut owned = OwnedVerifier { child, reaped: false };
	let observed = owned.poll_until(Instant::now() + Duration::from_secs(30));
	let status = match observed {
		Ok(Some(status)) => status,
		other => {
			let cleanup = owned.stop();
			let retained = isolated.keep();
			evidence(stage, "stdout.txt", &fs::read(&out).unwrap_or_default());
			evidence(stage, "stderr.txt", &fs::read(&err).unwrap_or_default());
			evidence(
				stage,
				"failure.txt",
				format!("poll: {other:?}; cleanup: {cleanup:?}; retained: {}", retained.display())
					.as_bytes(),
			);
			panic!(
				"verifier failed: {other:?}; cleanup: {cleanup:?}; logs retained in {}",
				retained.display()
			);
		},
	};
	let stdout = fs::read_to_string(out).expect("stdout text");
	let stderr = fs::read_to_string(err).expect("stderr text");
	evidence(stage, "stdout.txt", stdout.as_bytes());
	evidence(stage, "stderr.txt", stderr.as_bytes());
	evidence(stage, "journal.oms", &fs::read(path).expect("proof journal"));
	evidence(
		stage,
		"exit.json",
		serde_json::to_vec(
			&serde_json::json!({"code": status.code(), "success": status.success(), "reaped": owned.reaped}),
		)
		.expect("exit JSON")
		.as_slice(),
	);
	(status, stdout, stderr)
}

#[test]
fn actual_cli_reports_exact_tamper_location_and_never_repairs_legacy_or_torn_bytes() {
	let scratch = tempfile::tempdir().expect("scratch");
	let path = scratch.path().join("session.oms");
	let mut session = Session::create(&path, ComponentRegistry::standard()).expect("session");
	session.begin_turn().expect("first turn");
	session
		.user("deterministic tool-result integrity fixture", Vec::new())
		.expect("user");
	session
		.assistant_start("fixture-model", "fixture-provider", "fixture-route")
		.expect("assistant");
	let call = session
		.call(
			"fixture",
			1,
			"fixture-call",
			None,
			Some(serde_json::value::to_raw_value(&serde_json::json!({})).expect("args")),
			None,
		)
		.expect("call");
	session.assistant_end("tool_calls").expect("assistant end");
	let result = session
		.settle(
			call,
			serde_json::value::to_raw_value(&serde_json::json!({"text": "alpha payload"}))
				.expect("result payload"),
		)
		.expect("settled result");
	session.begin_turn().expect("successor");
	let entries = Journal::scan(&path).expect("entries");
	let original = fs::read(&path).expect("bytes");
	let tip = Journal::verify_path(&path, None)
		.expect("verified source")
		.tip
		.expect("tip")
		.to_string();
	// The writer remains held: verify must not take a writer lock.
	let (status, stdout, stderr) = execute(&path, &["--json", "--expected-tip", &tip], "valid");
	assert!(status.success(), "{stdout}\n{stderr}");
	let report: serde_json::Value = serde_json::from_str(&stdout).expect("JSON");
	assert_eq!(report["status"], "verified");
	assert_eq!(report["verification"]["sealed_entries"], entries.len());
	assert_eq!(fs::read(&path).expect("unchanged valid file"), original);
	drop(session);

	let mut scanner = Scanner::new(&original);
	let mut target = None;
	while let Some(frame) = scanner.next() {
		let frame = frame.expect("frame");
		if frame.entry.kind == Kind::known(KindName::ToolResult) {
			target = Some(frame);
		}
	}
	let target = target.expect("middle tool result entry");
	assert_eq!(target.entry.id, result);
	assert_eq!(target.entry.by, Some(call));
	assert!(
		target.span.start > 0 && target.span.end < original.len(),
		"edited result must have predecessor and successor frames"
	);
	let mut changed = original.clone();
	let local = changed[target.span.clone()]
		.windows(5)
		.position(|value| value == b"alpha")
		.expect("payload");
	evidence("expected", "oracle.json", serde_json::to_vec_pretty(&serde_json::json!({"id": target.entry.id, "offset": target.span.start, "frame_end": target.span.end, "changed_byte_offset": target.span.start + local, "original_tip": tip, "kind": target.entry.kind.to_string(), "call_id": call, "source": "actual Session fixture before byte edit"})).expect("oracle JSON").as_slice());
	changed[target.span.start + local] = b'o';
	fs::write(&path, &changed).expect("flip valid JSON byte");
	let refused = Session::open(&path, ComponentRegistry::standard())
		.err()
		.expect("forged history must not fold");
	let diagnostic = refused.to_string();
	let exact_refusal = matches!(refused, omp_session::SessionError::Journal(omp_journal::JournalError::Integrity(error)) if error.id == Some(target.entry.id) && error.offset == target.span.start);
	evidence(
		"tampered",
		"session-open.json",
		serde_json::to_vec_pretty(
			&serde_json::json!({"refused": exact_refusal, "diagnostic": diagnostic}),
		)
		.expect("refusal JSON")
		.as_slice(),
	);
	assert!(exact_refusal, "Session::open must identify the same divergent frame before fold");
	let (status, stdout, _) = execute(&path, &["--json"], "tampered");
	assert_eq!(status.code(), Some(1));
	let report: serde_json::Value = serde_json::from_str(&stdout).expect("invalid JSON report");
	assert_eq!(report["status"], "invalid");
	assert_eq!(report["first_bad_entry_id"], target.entry.id.to_string());
	assert_eq!(report["first_bad_byte_offset"], target.span.start);
	let (status, text, _) = execute(&path, &[], "tampered-text");
	assert_eq!(status.code(), Some(1));
	assert!(text.contains(&format!("entry {}, byte {}", target.entry.id, target.span.start)));
	assert_eq!(fs::read(&path).expect("unrepaired tamper"), changed);

	let mut legacy = Vec::new();
	for entry in &entries {
		omp_journal::sse::encode(entry, &mut legacy).expect("legacy codec");
	}
	let torn = original[..original.len() - 1].to_vec();
	for (bytes, expected) in
		[(legacy, "legacy_unsealed"), (torn, "torn_tail"), (Vec::new(), "empty")]
	{
		fs::write(&path, &bytes).expect("write fixture");
		let (status, stdout, _) = execute(&path, &["--json"], expected);
		assert_eq!(status.code(), Some(1), "{expected} must fail normally");
		let report: serde_json::Value = serde_json::from_str(&stdout).expect("report");
		assert_eq!(report["status"], expected);
		assert_eq!(fs::read(&path).expect("no repair"), bytes);
	}
	fs::write(&path, &original[..target.span.end]).expect("remove complete suffix");
	let shortened = fs::read(&path).expect("shortened");
	let (status, stdout, _) =
		execute(&path, &["--json", "--expected-tip", &tip], "expected-tip-mismatch");
	assert_eq!(status.code(), Some(1));
	let report: serde_json::Value = serde_json::from_str(&stdout).expect("tip report");
	assert_eq!(report["status"], "invalid");
	assert_eq!(report["first_bad_byte_offset"], shortened.len());
	assert_eq!(fs::read(&path).expect("no repair"), shortened);
}
