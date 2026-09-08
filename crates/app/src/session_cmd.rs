//! Read-only physical journal inspection at the application boundary.

use std::io::{self, Write};

use miette::IntoDiagnostic as _;
use omp_journal::{EntryId, Journal, JournalError, integrity::Verification};
use serde::Serialize;
use thiserror::Error;

use crate::cli::{SessionArgs, SessionCommand, SessionVerifyArgs};

/// A verification result never treats legacy or incomplete history as clean.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, strum::Display)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum VerifyStatus {
	/// Every committed frame is sealed and there is no incomplete suffix.
	Verified,
	/// Historical bytes lack seals and cannot establish integrity.
	LegacyUnsealed,
	/// A verified prefix exists but incomplete final bytes remain.
	TornTail,
	/// There are no committed entries to verify.
	Empty,
	/// Integrity, framing, semantic validation or reading failed.
	Invalid,
}

/// Typed command failures after a readable report has been emitted.
#[derive(Debug, Error)]
pub enum VerifyError {
	/// Filesystem or journal verification failed.
	#[error(transparent)]
	Journal(#[from] JournalError),
	/// The file is inspectable but cannot be called fully verified.
	#[error("session verification did not pass: {status}")]
	Unverified {
		/// Explicit reason the operation did not succeed.
		status: VerifyStatus,
	},
	/// Writing the result failed.
	#[error("could not write session verification output")]
	Output(#[from] io::Error),
	/// Serializing a structured report failed.
	#[error("could not serialize session verification report")]
	Json(#[from] serde_json::Error),
}

#[derive(Serialize)]
struct Report<'a> {
	status:                VerifyStatus,
	path:                  &'a std::path::Path,
	expected_tip:          Option<omp_core::Hash32>,
	verification:          Option<&'a Verification>,
	first_bad_entry_id:    Option<EntryId>,
	first_bad_byte_offset: Option<usize>,
	diagnostic:            Option<&'a str>,
}

/// Dispatches session inspection without composing a model or opening a writer.
///
/// # Errors
/// Returns the typed verification or output failure as an application
/// diagnostic.
pub fn run(args: SessionArgs) -> miette::Result<()> {
	match args.command {
		SessionCommand::Verify(args) => verify_to(&args, io::stdout().lock()).into_diagnostic(),
	}
}

/// Writes the same report used by `omp session verify` to a selected sink.
/// The journal path is only read; even a rejected or torn file is left intact.
///
/// # Errors
/// Returns a typed failure for invalid, legacy, empty or torn history, and for
/// output errors. Only a nonempty, wholly sealed file yields success.
pub fn verify_to(args: &SessionVerifyArgs, mut output: impl Write) -> Result<(), VerifyError> {
	let checked = Journal::verify_path(&args.path, args.expected_tip);
	let status = match &checked {
		Err(_) => VerifyStatus::Invalid,
		Ok(report) if report.legacy_bytes != 0 => VerifyStatus::LegacyUnsealed,
		Ok(report) if report.torn_tail_bytes != 0 => VerifyStatus::TornTail,
		Ok(report) if report.sealed_entries == 0 => VerifyStatus::Empty,
		Ok(_) => VerifyStatus::Verified,
	};
	// Error text is rendered only here at the application/output boundary.
	let diagnostic = checked.as_ref().err().map(ToString::to_string);
	let (first_bad_entry_id, first_bad_byte_offset) = match &checked {
		Err(JournalError::Integrity(error)) => (error.id, Some(error.offset)),
		_ => (None, None),
	};
	let report = Report {
		status,
		path: &args.path,
		expected_tip: args.expected_tip,
		verification: checked.as_ref().ok(),
		first_bad_entry_id,
		first_bad_byte_offset,
		diagnostic: diagnostic.as_deref(),
	};
	if args.json {
		serde_json::to_writer_pretty(&mut output, &report)?;
		writeln!(output)?;
	} else {
		writeln!(output, "{}: {}", args.path.display(), status)?;
		if let Some(verified) = report.verification {
			writeln!(
				output,
				"sealed entries: {}; legacy entries: {}; unsealed bytes: {}; committed bytes: {}; \
				 torn-tail bytes: {}",
				verified.sealed_entries,
				verified.legacy_entries,
				verified.legacy_bytes,
				verified.committed_bytes,
				verified.torn_tail_bytes
			)?;
			if let Some(tip) = verified.tip {
				writeln!(output, "physical tip: {tip}")?;
			}
		}
		if let Some(offset) = first_bad_byte_offset {
			if let Some(id) = first_bad_entry_id {
				writeln!(output, "first divergence: entry {id}, byte {offset}")?;
			} else {
				writeln!(output, "first divergence: unparseable entry id, byte {offset}")?;
			}
		}
		if let Some(diagnostic) = diagnostic {
			writeln!(output, "{diagnostic}")?;
		}
		writeln!(
			output,
			"Read-only inspection; no bytes repaired. Unkeyed seals prove consistency, not \
			 authorship."
		)?;
	}
	checked?;
	if status != VerifyStatus::Verified {
		return Err(VerifyError::Unverified { status });
	}
	Ok(())
}
