//! Environment-owned containment and publication authority for generated
//! skills.

use std::{
	collections::{BTreeMap, BTreeSet},
	fs::{self, Metadata, OpenOptions},
	io::{self, Read as _, Write as _},
	path::{Path, PathBuf},
	sync::{
		Arc,
		atomic::{AtomicU64, Ordering},
	},
};

use bytes::BytesMut;
use omp_agent::{GateError, HookEvent, HookGate, HookPatch};
use omp_core::{Hash32, Str};
use omp_proto::toolhost::v1::HookEventId;
use omp_tools::manage_skill::{
	Action, AuthorityError, ManagedSkillAuthority, MutationOutcome, MutationRequest,
};
use parking_lot::Mutex;
use serde::Serialize;

use crate::managed_skills_domain::{
	CandidateError, MAX_SKILL_BYTES, ManagedSkillCandidate, is_valid_name,
};

#[derive(Serialize)]
struct ManagedResourceRef<'a> {
	uri:    &'a str,
	kind:   &'static str,
	origin: &'static str,
}

#[derive(Serialize)]
struct ManagedResourcesChangedEvent<'a> {
	added:   Box<[ManagedResourceRef<'a>]>,
	removed: Box<[ManagedResourceRef<'a>]>,
	reason:  &'static str,
}

impl HookEvent for ManagedResourcesChangedEvent<'_> {
	type Return = ();

	const ID: HookEventId = HookEventId::HookEventResourcesChanged;
	const REV: u32 = 1;

	fn encode_into(&self, out: &mut BytesMut) {
		out.extend_from_slice(b"\n");
		out.extend_from_slice(
			&serde_json::to_vec(self).expect("managed resource payload must serialize to JSON"),
		);
	}

	fn apply(&mut self, _: &HookPatch) -> Result<(), GateError> {
		Ok(())
	}
}

/// Environment authority for one profile-scoped managed-skill root.
pub struct ManagedSkills {
	root:           PathBuf,
	authored_names: BTreeSet<Str>,
	hooks:          Arc<HookGate>,
	mutation_locks: Mutex<BTreeMap<Str, Arc<Mutex<()>>>>,
	revision:       AtomicU64,
	temp_sequence:  AtomicU64,
}

impl ManagedSkills {
	/// Creates an authority with an immutable set of authored names which
	/// generated skills may never claim.
	pub fn new(root: PathBuf, authored_names: BTreeSet<Str>, hooks: Arc<HookGate>) -> Self {
		Self {
			root,
			authored_names,
			hooks,
			mutation_locks: Mutex::new(BTreeMap::new()),
			revision: AtomicU64::new(0),
			temp_sequence: AtomicU64::new(0),
		}
	}

	fn notify_changed(&self, action: Action, name: &str) {
		if !self
			.hooks
			.subscribed(HookEventId::HookEventResourcesChanged)
		{
			return;
		}
		let uri = self.root.join(name).join("SKILL.md");
		let uri = uri.to_string_lossy();
		let resource = || ManagedResourceRef {
			uri:    uri.as_ref(),
			kind:   "skill",
			origin: crate::managed_skills_domain::PROVIDER_ID,
		};
		let (added, removed) = match action {
			Action::Create => (vec![resource()].into_boxed_slice(), Vec::new().into_boxed_slice()),
			Action::Update => {
				(vec![resource()].into_boxed_slice(), vec![resource()].into_boxed_slice())
			},
			Action::Delete => (Vec::new().into_boxed_slice(), vec![resource()].into_boxed_slice()),
		};
		self
			.hooks
			.notify(&ManagedResourcesChangedEvent { added, removed, reason: "reload" });
	}

	fn serialize_name<'a>(&self, raw: &'a str) -> Result<(Str, NameLock<'_>), AuthorityError> {
		let normalized = raw.trim().to_ascii_lowercase();
		if !is_valid_name(&normalized) {
			return Err(AuthorityError::InvalidName);
		}
		let name = Str::from(normalized);
		let lock = {
			let mut locks = self.mutation_locks.lock();
			Arc::clone(
				locks
					.entry(name.clone())
					.or_insert_with(|| Arc::new(Mutex::new(()))),
			)
		};
		Ok((name.clone(), NameLock { owner: self, name, lock }))
	}

	fn mutate_locked(
		&self,
		name: &Str,
		request: MutationRequest<'_>,
	) -> Result<MutationOutcome, AuthorityError> {
		match (request.action, request.expected_version) {
			(Action::Create, Some(_)) => return Err(AuthorityError::UnexpectedVersion),
			(Action::Update | Action::Delete, None) => {
				return Err(AuthorityError::MissingExpectedVersion);
			},
			_ => {},
		}
		self.ensure_root()?;
		let directory = self.root.join(name.as_str());
		let mut previous_version = None;
		let mut version = None;
		match request.action {
			Action::Create => {
				if self.authored_names.contains(name) {
					return Err(AuthorityError::AuthoredShadow);
				}
				let candidate = self.candidate(request)?;
				self.ensure_skill_directory(&directory, true)?;
				let file = directory.join("SKILL.md");
				let bytes = candidate.serialize();
				write_exclusive(&file, bytes.as_bytes())?;
				version = Some(Hash32::sum(bytes.as_bytes()));
			},
			Action::Update => {
				let candidate = self.candidate(request)?;
				self.ensure_skill_directory(&directory, false)?;
				let file = directory.join("SKILL.md");
				previous_version = Some(check_version(&file, request.expected_version)?);
				let bytes = candidate.serialize();
				self.atomic_replace(&directory, &file, bytes.as_bytes())?;
				version = Some(Hash32::sum(bytes.as_bytes()));
			},
			Action::Delete => {
				self.ensure_skill_directory(&directory, false)?;
				let file = directory.join("SKILL.md");
				previous_version = Some(check_version(&file, request.expected_version)?);
				fs::remove_dir_all(&directory).map_err(map_io)?;
			},
		}
		let revision = self
			.revision
			.fetch_add(1, Ordering::AcqRel)
			.saturating_add(1);
		Ok(MutationOutcome {
			action: request.action,
			name: name.clone(),
			path: Str::from(format!("{}/SKILL.md", name.as_str())),
			revision,
			previous_version,
			version,
		})
	}

	fn candidate(
		&self,
		request: MutationRequest<'_>,
	) -> Result<ManagedSkillCandidate, AuthorityError> {
		let description = request
			.description
			.ok_or(AuthorityError::InvalidDescription)?;
		let body = request.body.ok_or(AuthorityError::EmptyBody)?;
		ManagedSkillCandidate::new(request.name, description, body).map_err(|error| match error {
			CandidateError::InvalidName => AuthorityError::InvalidName,
			CandidateError::InvalidDescription => AuthorityError::InvalidDescription,
			CandidateError::EmptyBody => AuthorityError::EmptyBody,
			CandidateError::TooLarge => AuthorityError::TooLarge,
		})
	}

	fn ensure_root(&self) -> Result<(), AuthorityError> {
		match fs::symlink_metadata(&self.root) {
			Ok(metadata) => ensure_directory(&metadata),
			Err(error) if error.kind() == io::ErrorKind::NotFound => {
				let parent = self.root.parent().ok_or(AuthorityError::UnsafePath)?;
				fs::create_dir_all(parent).map_err(map_io)?;
				match fs::create_dir(&self.root) {
					Ok(()) => Ok(()),
					Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
						let metadata = fs::symlink_metadata(&self.root).map_err(map_io)?;
						ensure_directory(&metadata)
					},
					Err(error) => Err(map_io(error)),
				}
			},
			Err(error) => Err(map_io(error)),
		}
	}

	fn ensure_skill_directory(&self, directory: &Path, create: bool) -> Result<(), AuthorityError> {
		match fs::symlink_metadata(directory) {
			Ok(metadata) => ensure_directory(&metadata),
			Err(error) if error.kind() == io::ErrorKind::NotFound && create => {
				match fs::create_dir(directory) {
					Ok(()) => Ok(()),
					Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
						let metadata = fs::symlink_metadata(directory).map_err(map_io)?;
						ensure_directory(&metadata)
					},
					Err(error) => Err(map_io(error)),
				}
			},
			Err(error) if error.kind() == io::ErrorKind::NotFound => Err(AuthorityError::NotFound),
			Err(error) => Err(map_io(error)),
		}
	}

	#[cfg(unix)]
	fn atomic_replace(
		&self,
		directory: &Path,
		file: &Path,
		bytes: &[u8],
	) -> Result<(), AuthorityError> {
		let metadata = fs::symlink_metadata(file).map_err(map_update_io)?;
		ensure_regular_unlinked(&metadata)?;
		let sequence = self.temp_sequence.fetch_add(1, Ordering::Relaxed);
		let temporary = directory.join(format!(".SKILL.md.{}.{}.tmp", std::process::id(), sequence));
		let result = (|| {
			write_exclusive(&temporary, bytes)?;
			ensure_directory(&fs::symlink_metadata(directory).map_err(map_io)?)?;
			ensure_regular_unlinked(&fs::symlink_metadata(file).map_err(map_update_io)?)?;
			fs::rename(&temporary, file).map_err(map_io)
		})();
		if result.is_err() {
			let _ = fs::remove_file(&temporary);
		}
		result
	}

	#[cfg(windows)]
	fn atomic_replace(
		&self,
		_directory: &Path,
		file: &Path,
		bytes: &[u8],
	) -> Result<(), AuthorityError> {
		use std::os::windows::fs::OpenOptionsExt as _;
		const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
		let metadata = fs::symlink_metadata(file).map_err(map_update_io)?;
		ensure_regular_unlinked(&metadata)?;
		let mut handle = OpenOptions::new()
			.write(true)
			.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
			.open(file)
			.map_err(map_update_io)?;
		ensure_regular_unlinked(&handle.metadata().map_err(map_io)?)?;
		handle.set_len(0).map_err(map_io)?;
		handle.write_all(bytes).map_err(map_io)?;
		handle.sync_all().map_err(map_io)
	}
}

impl ManagedSkillAuthority for ManagedSkills {
	fn mutate(&self, request: MutationRequest<'_>) -> Result<MutationOutcome, AuthorityError> {
		let action = request.action;
		let (name, lock) = self.serialize_name(request.name)?;
		let guard = lock.lock.lock();
		let outcome = self.mutate_locked(&name, request);
		drop(guard);
		drop(lock);
		if outcome.is_ok() {
			self.notify_changed(action, name.as_str());
		}
		outcome
	}
}

#[must_use]
struct NameLock<'a> {
	owner: &'a ManagedSkills,
	name:  Str,
	lock:  Arc<Mutex<()>>,
}

impl Drop for NameLock<'_> {
	fn drop(&mut self) {
		let mut locks = self.owner.mutation_locks.lock();
		if Arc::strong_count(&self.lock) == 2
			&& locks
				.get(&self.name)
				.is_some_and(|current| Arc::ptr_eq(current, &self.lock))
		{
			locks.remove(&self.name);
		}
	}
}

/// Compare exact SKILL.md bytes while the authority's per-name lock is held.
/// The lock serializes this authority, not independent filesystem writers.
fn check_version(path: &Path, expected: Option<Hash32>) -> Result<Hash32, AuthorityError> {
	let expected = expected.ok_or(AuthorityError::MissingExpectedVersion)?;
	ensure_regular_unlinked(&fs::symlink_metadata(path).map_err(map_update_io)?)?;
	let mut options = OpenOptions::new();
	options.read(true);
	#[cfg(unix)]
	{
		use std::os::unix::fs::OpenOptionsExt as _;
		options.custom_flags(libc::O_NOFOLLOW);
	}
	#[cfg(windows)]
	{
		use std::os::windows::fs::OpenOptionsExt as _;
		options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
	}
	let file = options.open(path).map_err(map_update_io)?;
	ensure_regular_unlinked(&file.metadata().map_err(map_io)?)?;
	let mut bytes = Vec::new();
	file
		.take(MAX_SKILL_BYTES as u64 + 1)
		.read_to_end(&mut bytes)
		.map_err(map_io)?;
	if bytes.len() > MAX_SKILL_BYTES {
		return Err(AuthorityError::TooLarge);
	}
	let actual = Hash32::sum(&bytes);
	if actual != expected {
		return Err(AuthorityError::VersionConflict { expected, actual });
	}
	Ok(actual)
}

fn write_exclusive(path: &Path, bytes: &[u8]) -> Result<(), AuthorityError> {
	let mut file = match OpenOptions::new().write(true).create_new(true).open(path) {
		Ok(file) => file,
		Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
			return Err(AuthorityError::AlreadyExists);
		},
		Err(error) => return Err(map_io(error)),
	};
	file.write_all(bytes).map_err(map_io)?;
	file.sync_all().map_err(map_io)
}

fn ensure_directory(metadata: &Metadata) -> Result<(), AuthorityError> {
	if metadata.file_type().is_symlink() || !metadata.is_dir() {
		Err(AuthorityError::UnsafePath)
	} else {
		Ok(())
	}
}

fn ensure_regular_unlinked(metadata: &Metadata) -> Result<(), AuthorityError> {
	if metadata.file_type().is_symlink() || !metadata.is_file() || link_count(metadata) > 1 {
		Err(AuthorityError::UnsafePath)
	} else {
		Ok(())
	}
}

#[cfg(unix)]
fn link_count(metadata: &Metadata) -> u64 {
	use std::os::unix::fs::MetadataExt as _;
	metadata.nlink()
}

#[cfg(windows)]
fn link_count(metadata: &Metadata) -> u64 {
	use std::os::windows::fs::MetadataExt as _;
	u64::from(metadata.number_of_links())
}

fn map_update_io(error: io::Error) -> AuthorityError {
	if error.kind() == io::ErrorKind::NotFound {
		AuthorityError::NotFound
	} else {
		map_io(error)
	}
}

fn map_io(_: io::Error) -> AuthorityError {
	AuthorityError::Io
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn serializes_mutations_and_refuses_links_and_authored_shadows() {
		let tree = tempfile::tempdir().unwrap();
		let root = tree.path().join("managed-skills");
		let authority = ManagedSkills::new(
			root.clone(),
			BTreeSet::from([Str::from("authored")]),
			Arc::new(HookGate::channel().0),
		);
		let shadowed = authority.mutate(MutationRequest {
			action:           Action::Create,
			name:             "authored",
			description:      Some("when useful"),
			body:             Some("body"),
			expected_version: None,
		});
		assert_eq!(shadowed, Err(AuthorityError::AuthoredShadow));
		let created = authority
			.mutate(MutationRequest {
				action:           Action::Create,
				name:             "new-skill",
				description:      Some("when useful"),
				body:             Some("body"),
				expected_version: None,
			})
			.unwrap();
		assert_eq!(created.revision, 1);
		assert!(root.join("new-skill/SKILL.md").is_file());
	}
	fn request(
		action: Action,
		body: Option<&str>,
		expected_version: Option<Hash32>,
	) -> MutationRequest<'_> {
		MutationRequest {
			action,
			name: "shared-skill",
			description: Some("when useful"),
			body,
			expected_version,
		}
	}

	#[test]
	fn stale_edit_and_delete_leave_bytes_unchanged_and_digest_survives_restart() {
		let tree = tempfile::tempdir().unwrap();
		let root = tree.path().join("managed-skills");
		let make =
			|| ManagedSkills::new(root.clone(), BTreeSet::new(), Arc::new(HookGate::channel().0));
		let authority = make();
		for action in [Action::Update, Action::Delete] {
			assert_eq!(
				authority.mutate(request(action, Some("old client"), None)),
				Err(AuthorityError::MissingExpectedVersion)
			);
			assert!(!root.exists(), "missing version is refused before filesystem effects");
		}
		assert_eq!(
			authority.mutate(request(Action::Create, Some("body"), Some(Hash32::default()))),
			Err(AuthorityError::UnexpectedVersion)
		);
		assert!(!root.exists());
		let created = authority
			.mutate(request(Action::Create, Some("original"), None))
			.unwrap();
		let file = root.join("shared-skill/SKILL.md");
		let original = fs::read(&file).unwrap();
		assert_eq!(created.version, Some(Hash32::sum(&original)));
		assert_eq!(created.previous_version, None);
		let first = authority
			.mutate(request(Action::Update, Some("editor one"), created.version))
			.unwrap();
		let current = fs::read(&file).unwrap();
		assert_ne!(current, original);
		assert_eq!(first.previous_version, created.version);
		assert_eq!(first.version, Some(Hash32::sum(&current)));
		for action in [Action::Update, Action::Delete] {
			assert_eq!(
				authority.mutate(request(action, Some("stale editor"), created.version)),
				Err(AuthorityError::VersionConflict {
					expected: created.version.unwrap(),
					actual:   first.version.unwrap(),
				})
			);
			assert_eq!(fs::read(&file).unwrap(), current);
			assert_eq!(
				authority.revision.load(Ordering::Acquire),
				2,
				"refusals emit no inventory revision"
			);
			assert_eq!(
				authority.mutate(request(action, Some("old client"), None)),
				Err(AuthorityError::MissingExpectedVersion)
			);
			assert_eq!(fs::read(&file).unwrap(), current);
		}
		assert_eq!(
			fs::read_dir(file.parent().unwrap()).unwrap().count(),
			1,
			"refused changes left no staged files"
		);
		drop(authority);
		let restarted = make();
		let same = restarted
			.mutate(request(Action::Update, Some("editor one"), first.version))
			.unwrap();
		assert_eq!(same.version, first.version, "exact content version survives authority restart");
		assert_eq!(same.previous_version, first.version);
		assert_eq!(same.revision, 1, "inventory sequence is explicitly process local");
		let deleted = restarted
			.mutate(request(Action::Delete, None, same.version))
			.unwrap();
		assert_eq!(deleted.previous_version, same.version);
		assert_eq!(deleted.version, None);
		assert!(!file.parent().unwrap().exists());
	}

	#[test]
	fn simultaneous_editors_cannot_both_replace_the_same_version() {
		let tree = tempfile::tempdir().unwrap();
		let authority = ManagedSkills::new(
			tree.path().join("managed-skills"),
			BTreeSet::new(),
			Arc::new(HookGate::channel().0),
		);
		let version = authority
			.mutate(request(Action::Create, Some("original"), None))
			.unwrap()
			.version;
		let barrier = std::sync::Barrier::new(2);
		let outcomes = std::thread::scope(|scope| {
			let one = scope.spawn(|| {
				barrier.wait();
				authority.mutate(request(Action::Update, Some("first editor"), version))
			});
			let two = scope.spawn(|| {
				barrier.wait();
				authority.mutate(request(Action::Update, Some("second editor"), version))
			});
			[one.join().unwrap(), two.join().unwrap()]
		});
		assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
		let winner = outcomes
			.iter()
			.find_map(|outcome| outcome.as_ref().ok())
			.unwrap();
		let loser = outcomes
			.iter()
			.find_map(|outcome| outcome.as_ref().err())
			.unwrap();
		assert_eq!(loser, &AuthorityError::VersionConflict {
			expected: version.unwrap(),
			actual:   winner.version.unwrap(),
		});
		assert_eq!(
			winner.version,
			Some(Hash32::sum(&fs::read(authority.root.join("shared-skill/SKILL.md")).unwrap()))
		);
		assert_eq!(authority.revision.load(Ordering::Acquire), 2);
	}
	#[cfg(unix)]
	#[test]
	fn version_check_does_not_follow_a_managed_skill_link() {
		let tree = tempfile::tempdir().unwrap();
		let root = tree.path().join("managed-skills");
		let authority =
			ManagedSkills::new(root.clone(), BTreeSet::new(), Arc::new(HookGate::channel().0));
		let created = authority
			.mutate(request(Action::Create, Some("original"), None))
			.unwrap();
		let file = root.join("shared-skill/SKILL.md");
		let outside = tree.path().join("authored.md");
		let bytes = fs::read(&file).unwrap();
		fs::write(&outside, &bytes).unwrap();
		fs::remove_file(&file).unwrap();
		std::os::unix::fs::symlink(&outside, &file).unwrap();
		for action in [Action::Update, Action::Delete] {
			assert_eq!(
				authority.mutate(request(action, Some("replacement"), created.version)),
				Err(AuthorityError::UnsafePath)
			);
			assert_eq!(fs::read(&outside).unwrap(), bytes);
			assert!(
				fs::symlink_metadata(&file)
					.unwrap()
					.file_type()
					.is_symlink()
			);
		}
	}
}
