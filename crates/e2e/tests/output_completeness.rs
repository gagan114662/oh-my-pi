//! Real Bash invocation output intent, omission, and complete artifact
//! integrity.

#![cfg(unix)]

use bytes::Bytes;
use omp_core::{Hash32, Str};
use omp_e2e::{
	Result, error,
	support::{DEFAULT_TIMEOUT, EnvHarness, Scratch, within},
};
use omp_env::{EnvClient, InvocationEvent};
use omp_proto::{
	blob::v1::GetRequest,
	env::v1::{Admission, InvokeTool, OutputRequest, Verdict},
	thread::v1::Blob,
};
use omp_tool::{CallOutcome, Registry};
use serde_json::json;

const OUTPUT_BYTES: usize = 2 * 1024 * 1024;
const TRANSFER_LIMIT: u64 = 16 * 1024 * 1024;

async fn verified_blob(client: &EnvClient, blob: &Blob) -> Result<Vec<u8>> {
	let hash = Hash32::new(
		blob
			.hash
			.as_ref()
			.try_into()
			.map_err(|_| error("invalid artifact digest length"))?,
	);
	assert!(blob.inline.is_empty(), "artifact must use the real blob transfer");
	let download = within(
		"open output artifact",
		DEFAULT_TIMEOUT,
		client.blob_get(GetRequest { hash: blob.hash.clone(), offset: 0, length: 0 }),
	)
	.await??;
	let mut bytes = Vec::new();
	let verified = within(
		"verify complete output artifact",
		DEFAULT_TIMEOUT,
		download.write_verified(hash, blob.size, TRANSFER_LIMIT, &mut bytes),
	)
	.await??;
	assert_eq!(verified.hash, hash);
	assert_eq!(verified.size, blob.size);
	assert_eq!(Hash32::sum(&bytes), hash);
	Ok(bytes)
}

async fn invoke_shell(client: &EnvClient, notrunc: bool) -> Result<Verdict> {
	let id = if notrunc { "complete-2m" } else { "bounded-2m" };
	let args = serde_json::value::to_raw_value(
		&json!({"i":"Checking complete output", "command":"cat output-2m.txt", "timeout":30, "notrunc":notrunc}),
	)?;
	// Exercise the actual caller-argument interpretation before crossing the wire.
	let intent = omp_agent::DispatchOptions::from_args(&args).output_request();
	let output_request = match intent {
		omp_tool::OutputRequest::Bounded => OutputRequest::Bounded,
		omp_tool::OutputRequest::Complete => OutputRequest::Complete,
	};
	assert_eq!(output_request == OutputRequest::Complete, notrunc);
	let mut invocation = within(
		"open 2MiB Bash invocation",
		DEFAULT_TIMEOUT,
		client.invoke(InvokeTool {
			invocation_id: id.to_owned(),
			name: "bash".to_owned(),
			rev: "2".to_owned(),
			deadline_ms: 30_000,
			output_request: output_request as i32,
			..Default::default()
		}),
	)
	.await??;
	assert!(matches!(
		within("Bash accepted", DEFAULT_TIMEOUT, invocation.next_event()).await??,
		Some(InvocationEvent::Accepted(_))
	));
	within(
		"commit 2MiB Bash arguments",
		DEFAULT_TIMEOUT,
		invocation.commit_args(
			Bytes::copy_from_slice(args.get().as_bytes()),
			Bytes::from_static(b"output-completeness-proof"),
			1000,
			None,
		),
	)
	.await??;
	loop {
		match within("2MiB Bash terminal", DEFAULT_TIMEOUT, invocation.next_event()).await?? {
			Some(InvocationEvent::Admission(request)) => {
				assert_eq!(request.invocation_id, id);
				within(
					"admit read-only fixture cat",
					DEFAULT_TIMEOUT,
					invocation.admit(Admission {
						invocation_id: id.to_owned(),
						allow: true,
						..Default::default()
					}),
				)
				.await??;
			},
			Some(InvocationEvent::Update(_)) => {},
			Some(InvocationEvent::Verdict(verdict)) => return Ok(verdict),
			event => return Err(error(format!("unexpected 2MiB Bash event: {event:?}"))),
		}
	}
}

#[tokio::test]
async fn two_megabyte_bash_output_preserves_complete_truth_for_both_caller_modes() -> Result<()> {
	let scratch = Scratch::new()?;
	// A finite file avoids a producer blocked on a consumer that never reads.
	let expected = b"0123456789abcde\n"
		.iter()
		.copied()
		.cycle()
		.take(OUTPUT_BYTES)
		.collect::<Vec<_>>();
	scratch.write("output-2m.txt", &expected)?;
	let env = EnvHarness::spawn(&scratch, Registry::new()).await?;
	let mut observations = Vec::new();
	for notrunc in [false, true] {
		let verdict = invoke_shell(env.client(), notrunc).await?;
		assert!(!verdict.is_error, "Bash fault: {verdict:?}");
		let details = verdict
			.details_blob
			.as_ref()
			.expect("durable structured verdict");
		assert_eq!(details.mime, "application/json");
		let structured = verified_blob(env.client(), details).await?;
		let projection = verdict.projection.as_ref().expect("host output projection");
		assert_eq!(
			projection.request,
			if notrunc {
				OutputRequest::Complete
			} else {
				OutputRequest::Bounded
			} as i32
		);
		assert_eq!(projection.source_bytes, structured.len() as u64);
		assert_eq!(projection.inline_bytes, verdict.json.len() as u64);
		assert_eq!(projection.omitted, verdict.json.len() < structured.len());
		assert_eq!(projection.artifact.as_ref(), Some(details));
		if !verdict.json.is_empty() {
			assert_eq!(verdict.json.as_ref(), structured);
		}
		let outcome: CallOutcome<omp_tools::shell::Payload, omp_tools::shell::Fault> =
			serde_json::from_slice(&structured)?;
		let CallOutcome::Ok(payload) = outcome else {
			return Err(error(format!("Bash did not succeed: {outcome:?}")));
		};
		assert_eq!(payload.status.exit_code, Some(0));
		assert_eq!(payload.status.outcome, omp_tools::shell::ExecOutcome::Exited);
		assert!(!payload.status.effects_unknown);
		let inline = payload
			.transcript
			.iter()
			.flat_map(|frame| frame.data.as_ref().iter().copied())
			.collect::<Vec<_>>();
		assert!(!inline.is_empty(), "real invocation must expose output");
		assert!(inline.len() <= expected.len());
		assert_eq!(inline, expected[..inline.len()], "retained bytes are an exact prefix");
		if !notrunc {
			assert!(inline.len() <= 64 * 1024);
			assert!(inline.len() < expected.len());
		}
		// A Complete request still may encounter the host's frame/backpressure
		// bound. Record that limitation explicitly; artifact recovery proves
		// complete source retention, never complete inline delivery.
		let omitted = inline.len() != expected.len();
		assert_eq!(
			payload.status.spilled_output.is_some(),
			omitted,
			"omission must carry full raw-output recovery"
		);
		if let Some(artifact) = &payload.status.spilled_output {
			let reference = omp_journal::blob::BlobRef::parse_hex(&artifact.hash, artifact.byte_len)?;
			assert_eq!(artifact.byte_len, OUTPUT_BYTES as u64);
			let wire = Blob {
				hash: Bytes::copy_from_slice(reference.hash.as_bytes()),
				mime: artifact.media_type.to_string(),
				size: artifact.byte_len,
				..Default::default()
			};
			let full = verified_blob(env.client(), &wire).await?;
			assert_eq!(full, expected, "full raw output is recovered byte-for-byte");
		} else {
			assert_eq!(inline, expected, "complete means all 2MiB, not merely a successful exit");
		}
		observations.push(json!({
			 "notrunc": notrunc,
			 "source_bytes": OUTPUT_BYTES,
			 "inline_raw_bytes": inline.len(),
			 "omitted_raw_bytes": expected.len() - inline.len(),
			 "raw_omitted": omitted,
			 "complete_inline": !omitted,
			 "verdict_inline_bytes": verdict.json.len(),
			 "verdict_omitted": projection.omitted,
			 "raw_artifact": payload.status.spilled_output,
			 "verified_raw_sha256": Str::new(Hash32::sum(&expected).to_hex().as_str()),
		}));
	}
	let artifact_root = std::env::var_os("CARGO_TARGET_DIR")
		.map(std::path::PathBuf::from)
		.unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"))
		.join("e2e-artifacts");
	std::fs::create_dir_all(&artifact_root)?;
	std::fs::write(
		artifact_root.join("output-completeness.json"),
		serde_json::to_vec_pretty(&observations)?,
	)?;
	println!("2MiB output evidence: {}", serde_json::to_string(&observations)?);
	env.shutdown().await?;
	Ok(())
}
