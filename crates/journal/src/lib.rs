//! Crash-tolerant session journal and content-addressed blob storage.
//!
//! Each session is one flat raw-SSE `.oms` file. A blank line commits an
//! entry; opening a journal truncates only bytes after the last commit point.
//! Branches are `prior` links, so rewinding never destroys abandoned history.

pub mod blob;
mod chain;
pub mod data;
mod entry;
pub mod gc;
pub mod integrity;
pub mod kind;
pub mod sse;
pub mod ulid;

use std::{
	fs::{self, File, OpenOptions},
	io::{self, Read as _, Write as _},
	path::{Path, PathBuf},
};

pub use chain::{abandoned, live_chain};
pub use entry::{Entry, EntryDraft, EntryId};
pub use kind::{Kind, KindError, KindName};
use omp_core::{FastHashSet, Hash32, Str, Ulid};
use thiserror::Error;

use crate::{
	kind::JOURNAL,
	sse::{Scanner, SseError},
	ulid::{MonotonicUlid, UlidGenerationError},
};

/// Conventional journal file extension.
pub const FILE_EXTENSION: &str = "oms";

/// The single writer for one session journal.
///
/// A live `Journal` holds an exclusive advisory lock on a stable sidecar for
/// its whole lifetime, so two processes (or two owners in one process) can
/// never append to divergent materializations of one `.oms`, and
/// [`gc::prune_abandoned`] cannot replace a file another writer is
/// appending to. It also holds a shared directory-namespace lease; collection
/// takes that lease exclusively from journal inventory through CAS sweep, so
/// a new writer cannot cross the mark boundary. Read-only consumers use
/// [`Journal::scan`], which takes no lock and never truncates.
#[derive(Debug)]
pub struct Journal {
	path:                 PathBuf,
	file:                 File,
	_lock:                WriterLock,
	_namespace:           JournalNamespaceLock,
	generator:            MonotonicUlid,
	ids:                  FastHashSet<EntryId>,
	entry_count:          usize,
	recovered_tail_bytes: u64,
	integrity:            integrity::Verification,
	write_failed:         bool,
}

impl Journal {
	/// Creates a new empty journal at `path`.
	///
	/// The first appended entry must be the `journal@1` genesis.
	///
	/// # Errors
	///
	/// Returns [`JournalError::Io`] if the parent cannot be created or the file
	/// already exists.
	pub fn create(path: impl AsRef<Path>) -> Result<Self, JournalError> {
		let path = path.as_ref().to_path_buf();
		if let Some(parent) = path
			.parent()
			.filter(|parent| !parent.as_os_str().is_empty())
		{
			fs::create_dir_all(parent)?;
		}
		let namespace = JournalNamespaceLock::acquire_shared(&path)?;
		let lock = WriterLock::acquire(&path)?;
		let file = OpenOptions::new()
			.create_new(true)
			.append(true)
			.read(true)
			.open(&path)?;
		Ok(Self {
			path,
			file,
			_lock: lock,
			_namespace: namespace,
			generator: MonotonicUlid::default(),
			ids: FastHashSet::default(),
			entry_count: 0,
			recovered_tail_bytes: 0,
			integrity: integrity::Verification::default(),
			write_failed: false,
		})
	}

	/// Opens an existing journal, returning its committed entries.
	///
	/// Bytes after the last committing blank line are physically truncated.
	/// The number removed is available through [`Self::recovered_tail_bytes`].
	///
	/// # Errors
	///
	/// Returns a typed error for I/O, malformed complete frames, invalid journal
	/// structure, or invalid branch links.
	pub fn open(path: impl AsRef<Path>) -> Result<(Self, Vec<Entry>), JournalError> {
		Self::open_checked(path.as_ref(), None, false)
	}

	/// Opens only sealed history, optionally bound to an independently saved
	/// tip. Verification and legacy refusal precede torn-tail truncation.
	/// Session replay uses this boundary so rejected history is never modified.
	pub fn open_verified(
		path: impl AsRef<Path>,
		expected_tip: Option<Hash32>,
	) -> Result<(Self, Vec<Entry>), JournalError> {
		Self::open_checked(path.as_ref(), expected_tip, true)
	}

	fn open_checked(
		path: &Path,
		expected_tip: Option<Hash32>,
		require_sealed: bool,
	) -> Result<(Self, Vec<Entry>), JournalError> {
		let path = path.to_path_buf();
		// Lock the stable sidecar before opening the journal. Locking the
		// journal inode itself is insufficient: GC replaces that inode, and
		// an opener that raced the rename could otherwise lock and append to
		// the unlinked predecessor.
		let namespace = JournalNamespaceLock::acquire_shared(&path)?;
		let lock = WriterLock::acquire(&path)?;
		let file = OpenOptions::new().append(true).read(true).open(&path)?;
		let mut bytes = Vec::new();
		(&file).read_to_end(&mut bytes)?;
		let (entries, verification) = decode_verified(&bytes)?;
		if require_sealed && verification.legacy_bytes != 0 {
			return Err(JournalError::LegacyUnsealed {
				entries: verification.legacy_entries,
				bytes:   verification.legacy_bytes,
			});
		}
		if expected_tip.is_some() {
			integrity::verify_bytes(&bytes, expected_tip)?;
		}
		let clean_len = verification.committed_bytes;
		let truncated = bytes.len().saturating_sub(clean_len);
		if truncated != 0 {
			file.set_len(u64::try_from(clean_len).map_err(|_| JournalError::FileTooLarge)?)?;
			file.sync_data()?;
		}
		let mut ids = FastHashSet::default();
		let mut floor = None;
		for entry in &entries {
			ids.insert(entry.id);
			let id = entry.id.as_ulid();
			floor = Some(floor.map_or(id, |prior: Ulid| prior.max(id)));
		}
		Ok((
			Self {
				path,
				file,
				_lock: lock,
				_namespace: namespace,
				generator: MonotonicUlid::seeded(floor),
				ids,
				entry_count: entries.len(),
				recovered_tail_bytes: truncated as u64,
				integrity: verification,
				write_failed: false,
			},
			entries,
		))
	}

	/// Reads the committed entries of a journal without taking the writer
	/// lock or truncating a torn tail.
	///
	/// This is the read-only path for session indexes, pickers, and
	/// renderers of a journal that may be live in another process: it sees
	/// the committed prefix exactly as a later [`Self::open`] would. Legacy
	/// entries remain readable for inspection; use [`Self::verify_path`] to
	/// distinguish them, or [`Self::open_verified`] before authoritative replay.
	///
	/// # Errors
	///
	/// Returns a typed error for I/O, malformed complete frames, invalid
	/// journal structure, or invalid branch links.
	pub fn scan(path: impl AsRef<Path>) -> Result<Vec<Entry>, JournalError> {
		let bytes = fs::read(path)?;
		decode_committed(&bytes).map(|(entries, _)| entries)
	}

	/// Appends and durably commits one entry.
	///
	/// # Errors
	///
	/// Returns a typed error when the draft violates genesis/cause/branch rules,
	/// its payload is not single-line JSON or exceeds one mebibyte, identity
	/// generation is exhausted, or the durable write fails.
	pub fn append(&mut self, draft: EntryDraft) -> Result<Entry, JournalError> {
		self.append_with_persistence(draft, |file, encoded| {
			file.write_all(encoded)?;
			file.sync_data()
		})
	}

	// A narrow persistence boundary permits deterministic failure after actual
	// partial/full file writes; production always uses write_all + sync_data.
	fn append_with_persistence(
		&mut self,
		draft: EntryDraft,
		persist: impl FnOnce(&mut File, &[u8]) -> io::Result<()>,
	) -> Result<Entry, JournalError> {
		if self.write_failed {
			return Err(JournalError::WriterPoisoned);
		}
		self.require_sealed()?;
		validate_draft(&draft, self.entry_count, &self.ids)?;
		let id = EntryId::from(self.generator.generate()?);
		let entry = Entry {
			id,
			kind: draft.kind,
			by: draft.by,
			prior: draft.prior,
			label: draft.label,
			data: draft.data,
		};
		let mut encoded = Vec::with_capacity(entry.data.len() + 160);
		let tip = integrity::encode(&entry, self.integrity.tip.unwrap_or_default(), &mut encoded)
			.map_err(map_sse_write_error)?;
		// A failed write or sync leaves durability and the physical EOF uncertain.
		// Never append using the old cached predecessor after that boundary.
		self.write_failed = true;
		persist(&mut self.file, &encoded)?;
		self.write_failed = false;
		self.ids.insert(id);
		self.entry_count += 1;
		self.integrity.tip = Some(tip);
		self.integrity.sealed_entries += 1;
		self.integrity.committed_bytes += encoded.len();
		self.integrity.torn_tail_bytes = 0;
		Ok(entry)
	}

	/// Rechecks the on-disk physical chain without mutating it.
	///
	/// # Errors
	/// Returns I/O, integrity, framing or semantic history errors.
	pub fn verify(&self) -> Result<integrity::Verification, JournalError> {
		Self::verify_path(&self.path, None)
	}

	/// Inspects a path without acquiring a writer lock or truncating a torn
	/// tail. An optional externally retained tip detects complete suffix
	/// removal too.
	///
	/// # Errors
	/// Returns I/O, integrity (including expected-tip mismatch), framing or
	/// semantic history errors.
	pub fn verify_path(
		path: impl AsRef<Path>,
		expected_tip: Option<Hash32>,
	) -> Result<integrity::Verification, JournalError> {
		let bytes = fs::read(path)?;
		let (_, report) = decode_verified(&bytes)?;
		if expected_tip.is_some() {
			integrity::verify_bytes(&bytes, expected_tip)?;
		}
		Ok(report)
	}

	/// Refuses treating historical unsealed entries as verified history.
	/// Read-only scanning and verification inventory remain available.
	///
	/// # Errors
	/// Returns [`JournalError::LegacyUnsealed`] for unsealed entries.
	pub fn require_sealed(&self) -> Result<(), JournalError> {
		if self.integrity.legacy_bytes != 0 {
			return Err(JournalError::LegacyUnsealed {
				entries: self.integrity.legacy_entries,
				bytes:   self.integrity.legacy_bytes,
			});
		}
		Ok(())
	}

	/// Returns the journal file path.
	#[must_use]
	pub fn path(&self) -> &Path {
		&self.path
	}

	/// Returns the number of torn-tail bytes removed by [`Self::open`].
	#[must_use]
	pub const fn recovered_tail_bytes(&self) -> u64 {
		self.recovered_tail_bytes
	}

	/// Closes the replaceable data inode while retaining the stable sidecar
	/// lock. GC uses this immediately before its atomic rename.
	pub(crate) fn close_for_replace(self) -> ReplaceLock {
		let Self { _lock, _namespace, .. } = self;
		ReplaceLock { _writer: _lock, _namespace }
	}
}

/// Journal creation, recovery, validation, and append failure.
#[derive(Debug, Error)]
pub enum JournalError {
	/// A prior persistence failure left this writer's physical tip uncertain.
	#[error("journal writer must be closed and reopened after a write or sync failure")]
	WriterPoisoned,
	/// A complete physical frame fails its seal or predecessor check.
	#[error(transparent)]
	Integrity(#[from] integrity::IntegrityError),
	/// Legacy bytes have no evidence of integrity and need explicit migration.
	#[error(
		"journal contains {entries} legacy entries in {bytes} unsealed bytes; explicit migration is \
		 required before replay or append"
	)]
	LegacyUnsealed {
		/// Count of entries without physical seals.
		entries: usize,
		/// Complete unsealed bytes, including spacer blocks.
		bytes:   usize,
	},
	/// A filesystem operation failed.
	#[error("journal I/O failed")]
	Io(#[from] io::Error),
	/// A complete SSE frame is malformed.
	#[error("journal contains a malformed complete frame")]
	Frame {
		/// SSE codec failure.
		#[source]
		source: SseError,
	},
	/// A kind is outside the closed revision-1 vocabulary.
	#[error("journal kind {kind} is not in the closed revision-1 vocabulary")]
	UnknownKind {
		/// Unsupported versioned kind.
		kind: Kind,
	},
	/// A non-genesis entry has no causal `by` link.
	#[error("non-genesis entry {kind} requires a `by` cause")]
	MissingCause {
		/// Offending versioned kind.
		kind: Kind,
	},
	/// A non-genesis entry appeared before the genesis.
	#[error("the first journal entry must be journal@1 genesis")]
	GenesisMustBeFirst,
	/// A genesis entry appeared after the first position.
	#[error("journal@1 genesis may appear only as the first entry")]
	GenesisOnlyFirst,
	/// An explicit `prior` link does not name an earlier entry.
	#[error("journal prior entry {id} does not exist earlier in the file")]
	UnknownPrior {
		/// Missing branch target.
		id: EntryId,
	},
	/// An opened file repeats an entry identity.
	#[error("journal entry id {id} is duplicated")]
	DuplicateId {
		/// Repeated identity.
		id: EntryId,
	},
	/// The JSON payload exceeds one mebibyte.
	#[error("journal data payload is {len} bytes; maximum is 1048576")]
	DataTooLarge {
		/// Payload byte length.
		len: usize,
	},
	/// The JSON payload contains a physical line break.
	#[error("journal data payload must occupy one physical line")]
	MultilineData,
	/// The optional operation label contains a physical line break.
	#[error("journal operation label must occupy one physical line")]
	MultilineLabel,
	/// The JSON payload is malformed.
	#[error("journal data payload is invalid JSON")]
	InvalidData {
		/// JSON decoder failure.
		#[source]
		source: serde_json::Error,
	},
	/// The platform cannot represent the journal file length.
	#[error("journal file length cannot be represented")]
	FileTooLarge,
	/// Another writer holds the journal's exclusive lock.
	#[error("journal {} is locked by another writer", path.display())]
	Locked {
		/// Journal file path.
		path: PathBuf,
	},
	/// Namespace collection excludes session writers while establishing roots.
	#[error("journal namespace {} is locked for garbage collection", path.display())]
	NamespaceLocked {
		/// Namespace lock path.
		path: PathBuf,
	},
	/// No larger ULID can be generated.
	#[error(transparent)]
	Ulid(#[from] UlidGenerationError),
}

/// Decodes every complete frame, returning the entries and the byte offset of
/// the last commit point.
fn decode_committed(bytes: &[u8]) -> Result<(Vec<Entry>, usize), JournalError> {
	let (entries, report) = decode_verified(bytes)?;
	Ok((entries, report.committed_bytes))
}

fn decode_verified(bytes: &[u8]) -> Result<(Vec<Entry>, integrity::Verification), JournalError> {
	let report = integrity::verify_bytes(bytes, None)?;
	let mut scanner = Scanner::new(bytes);
	let mut entries = Vec::new();
	while let Some(frame) = scanner.next() {
		entries.push(
			frame
				.map_err(|source| JournalError::Frame { source })?
				.entry,
		);
	}
	validate_history(&entries)?;
	Ok((entries, report))
}

/// Stable sidecar lock shared by journal writers and atomic replacement.
///
/// The sidecar is deliberately never deleted: unlinking a lock file allows a
/// contender to create and lock a new inode while an existing owner still
/// holds the old one.
#[derive(Debug)]
pub(crate) struct WriterLock {
	_file: File,
}

/// Shared session-writer or exclusive collector lease for one journal
/// directory.
#[derive(Debug)]
pub(crate) struct JournalNamespaceLock {
	_file: File,
}

impl JournalNamespaceLock {
	fn path(journal: &Path) -> PathBuf {
		journal
			.parent()
			.unwrap_or_else(|| Path::new("."))
			.join(".journal-gc.lock")
	}

	fn acquire_shared(journal: &Path) -> Result<Self, JournalError> {
		let path = Self::path(journal);
		let file = OpenOptions::new()
			.create(true)
			.read(true)
			.write(true)
			.open(&path)?;
		match File::try_lock_shared(&file) {
			Ok(()) => Ok(Self { _file: file }),
			Err(fs::TryLockError::WouldBlock) => Err(JournalError::NamespaceLocked { path }),
			Err(fs::TryLockError::Error(source)) => Err(JournalError::Io(source)),
		}
	}

	pub(crate) fn acquire_exclusive(root: &Path) -> Result<Self, JournalError> {
		let path = root.join(".journal-gc.lock");
		let file = OpenOptions::new()
			.create(true)
			.read(true)
			.write(true)
			.open(&path)?;
		match file.try_lock() {
			Ok(()) => Ok(Self { _file: file }),
			Err(fs::TryLockError::WouldBlock) => Err(JournalError::NamespaceLocked { path }),
			Err(fs::TryLockError::Error(source)) => Err(JournalError::Io(source)),
		}
	}
}

/// Locks retained across an atomic journal inode replacement.
pub(crate) struct ReplaceLock {
	_writer:    WriterLock,
	_namespace: JournalNamespaceLock,
}

impl WriterLock {
	/// Takes the journal's exclusive advisory lock without blocking.
	fn acquire(path: &Path) -> Result<Self, JournalError> {
		let mut name = path.file_name().unwrap_or_default().to_os_string();
		name.push(".lock");
		let file = OpenOptions::new()
			.create(true)
			.read(true)
			.write(true)
			.open(path.with_file_name(name))?;
		match file.try_lock() {
			Ok(()) => Ok(Self { _file: file }),
			Err(fs::TryLockError::WouldBlock) => {
				Err(JournalError::Locked { path: path.to_path_buf() })
			},
			Err(fs::TryLockError::Error(source)) => Err(JournalError::Io(source)),
		}
	}
}

fn validate_history(entries: &[Entry]) -> Result<(), JournalError> {
	let mut ids = FastHashSet::default();
	for (index, entry) in entries.iter().enumerate() {
		validate_entry(entry, index, &ids)?;
		if !ids.insert(entry.id) {
			return Err(JournalError::DuplicateId { id: entry.id });
		}
	}
	Ok(())
}

fn validate_draft(
	draft: &EntryDraft,
	index: usize,
	ids: &FastHashSet<EntryId>,
) -> Result<(), JournalError> {
	validate_fields(&draft.kind, draft.by, draft.prior, &draft.label, &draft.data, index, ids)
}

fn validate_entry(
	entry: &Entry,
	index: usize,
	ids: &FastHashSet<EntryId>,
) -> Result<(), JournalError> {
	validate_fields(&entry.kind, entry.by, entry.prior, &entry.label, &entry.data, index, ids)
}

fn validate_fields(
	kind: &Kind,
	by: Option<EntryId>,
	prior: Option<EntryId>,
	label: &Option<Str>,
	data: &Str,
	index: usize,
	ids: &FastHashSet<EntryId>,
) -> Result<(), JournalError> {
	if !kind.is_known() {
		return Err(JournalError::UnknownKind { kind: kind.clone() });
	}
	let genesis = kind.name.as_str() == JOURNAL && kind.rev == 1;
	if !genesis && by.is_none() {
		return Err(JournalError::MissingCause { kind: kind.clone() });
	}
	if index == 0 && !genesis {
		return Err(JournalError::GenesisMustBeFirst);
	}
	if index != 0 && genesis {
		return Err(JournalError::GenesisOnlyFirst);
	}
	if let Some(prior) = prior
		&& !ids.contains(&prior)
	{
		return Err(JournalError::UnknownPrior { id: prior });
	}
	if data.len() > sse::DATA_HARD_CAP {
		return Err(JournalError::DataTooLarge { len: data.len() });
	}
	if data.bytes().any(|byte| matches!(byte, b'\r' | b'\n')) {
		return Err(JournalError::MultilineData);
	}
	if label
		.as_ref()
		.is_some_and(|value| value.bytes().any(|byte| matches!(byte, b'\r' | b'\n')))
	{
		return Err(JournalError::MultilineLabel);
	}
	serde_json::from_str::<serde::de::IgnoredAny>(data)
		.map(|_| ())
		.map_err(|source| JournalError::InvalidData { source })
}

fn map_sse_write_error(source: SseError) -> JournalError {
	match source {
		SseError::DataTooLarge { len } => JournalError::DataTooLarge { len },
		SseError::MultilineData => JournalError::MultilineData,
		SseError::MultilineLabel => JournalError::MultilineLabel,
		SseError::InvalidData { source } => JournalError::InvalidData { source },
		other => JournalError::Frame { source: other },
	}
}

#[cfg(test)]
mod persistence_tests {
	use super::*;

	fn draft(kind: KindName, by: Option<EntryId>) -> EntryDraft {
		EntryDraft {
			kind: Kind::known(kind),
			by,
			prior: None,
			label: None,
			data: Str::new_static("{}"),
		}
	}

	fn failed_persistence_requires_reopen(full_write: bool) {
		let directory = tempfile::tempdir().expect("tempdir");
		let path = directory.path().join("failure.oms");
		let mut journal = Journal::create(&path).expect("create");
		let genesis = journal
			.append(draft(KindName::Journal, None))
			.expect("genesis");
		let original_tip = journal.integrity.tip;
		let error = journal.append_with_persistence(
			draft(KindName::TurnStart, Some(genesis.id)),
			|file, encoded| {
				// Actual filesystem bytes, followed by the same error boundary as
				// write_all (partial) or sync_data (complete write, failed sync).
				let prefix = if full_write {
					encoded
				} else {
					&encoded[..encoded.len() / 2]
				};
				file.write_all(prefix)?;
				Err(io::Error::other("injected persistence failure"))
			},
		);
		assert!(matches!(error, Err(JournalError::Io(_))));
		assert_eq!(journal.integrity.tip, original_tip, "failed durability is not acknowledged");
		assert_eq!(journal.entry_count, 1);
		let after_failure = fs::read(&path).expect("actual bytes after failure");
		let report = journal.verify().expect("complete prefix verifies");
		assert_eq!(report.sealed_entries, if full_write { 2 } else { 1 });
		assert_eq!(report.torn_tail_bytes == 0, full_write);
		assert!(matches!(
			journal.append(draft(KindName::TurnStart, Some(genesis.id))),
			Err(JournalError::WriterPoisoned)
		));
		assert_eq!(fs::read(&path).expect("read blocked append"), after_failure);
		drop(journal);
		let (mut reopened, entries) =
			Journal::open_verified(&path, None).expect("recover actual bytes");
		assert_eq!(entries.len(), if full_write { 2 } else { 1 });
		reopened
			.append(draft(KindName::TurnStart, Some(genesis.id)))
			.expect("append after recovery");
		let final_report = reopened
			.verify()
			.expect("new append commits to recovered physical tip");
		assert_eq!(final_report.sealed_entries, entries.len() + 1);
		assert_eq!(final_report.torn_tail_bytes, 0);
	}

	#[test]
	fn partial_write_failure_poison_prevents_appending_to_torn_frame() {
		failed_persistence_requires_reopen(false);
	}

	#[test]
	fn sync_failure_after_complete_write_poison_prevents_stale_predecessor() {
		failed_persistence_requires_reopen(true);
	}
}
