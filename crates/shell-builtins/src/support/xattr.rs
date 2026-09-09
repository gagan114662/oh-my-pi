//! Extended-attribute probes and preservation for filesystem builtins.

#[cfg(all(unix, not(target_os = "macos")))]
use std::path::Path;
#[cfg(target_os = "linux")]
use std::{ffi, io, ptr};

#[cfg(all(unix, not(any(target_os = "macos", target_os = "redox"))))]
type Attributes = omp_core::FastHashMap<std::ffi::OsString, Vec<u8>>;

/// Read the attributes before a cross-filesystem move removes its source.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "redox"))))]
pub(crate) fn retrieve_xattrs(path: impl AsRef<Path>) -> std::io::Result<Attributes> {
	#[cfg(target_os = "linux")]
	{
		use std::os::unix::ffi::OsStrExt;
		let path = path.as_ref();
		let mut attributes = Attributes::default();
		for name in list_xattrs(path)?.split_inclusive(|byte| *byte == 0) {
			if name.len() > 1 {
				let value = get_xattr(path, name)?;
				attributes.insert(ffi::OsStr::from_bytes(&name[..name.len() - 1]).to_owned(), value);
			}
		}
		Ok(attributes)
	}
	#[cfg(not(target_os = "linux"))]
	{
		let _ = path;
		Err(std::io::Error::from_raw_os_error(libc::EOPNOTSUPP))
	}
}

/// Apply every saved attribute, propagating permission and I/O failures.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "redox"))))]
pub(crate) fn apply_xattrs(path: impl AsRef<Path>, attributes: Attributes) -> std::io::Result<()> {
	#[cfg(target_os = "linux")]
	{
		for (name, value) in attributes {
			set_xattr(path.as_ref(), &name, &value)?;
		}
		Ok(())
	}
	#[cfg(not(target_os = "linux"))]
	{
		let _ = (path, attributes);
		Err(std::io::Error::from_raw_os_error(libc::EOPNOTSUPP))
	}
}

/// Preserve attributes without deleting or modifying the source.
#[cfg(all(unix, not(any(target_os = "macos", target_os = "redox"))))]
pub(crate) fn copy_xattrs(from: impl AsRef<Path>, to: impl AsRef<Path>) -> std::io::Result<()> {
	apply_xattrs(to, retrieve_xattrs(from)?)
}

#[cfg(target_os = "linux")]
fn set_xattr(path: &Path, name: &ffi::OsStr, value: &[u8]) -> io::Result<()> {
	use std::os::unix::ffi::OsStrExt;
	let path = path_cstring(path)?;
	let name = ffi::CString::new(name.as_bytes())
		.map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid attribute name"))?;
	// SAFETY: both C strings and the value buffer remain live for this call.
	let result = unsafe {
		libc::setxattr(path.as_ptr(), name.as_ptr(), value.as_ptr().cast(), value.len(), 0)
	};
	if result < 0 {
		Err(io::Error::last_os_error())
	} else {
		Ok(())
	}
}

/// Returns whether a path has at least one extended ACL or attribute.
#[cfg(all(unix, not(any(target_os = "android", target_os = "macos"))))]
pub(crate) fn has_acl(path: impl AsRef<Path>) -> bool {
	#[cfg(target_os = "linux")]
	return list_xattrs(path.as_ref()).is_ok_and(|names| !names.is_empty());
	#[cfg(not(target_os = "linux"))]
	{
		let _ = path;
		false
	}
}

/// Returns whether Linux's `security.capability` extended attribute is present.
#[cfg(all(unix, not(any(target_os = "android", target_os = "macos"))))]
pub(crate) fn has_security_cap_acl(path: impl AsRef<Path>) -> bool {
	#[cfg(target_os = "linux")]
	return list_xattrs(path.as_ref()).is_ok_and(|names| {
		names
			.split(|byte| *byte == 0)
			.any(|name| name == b"security.capability")
	});
	#[cfg(not(target_os = "linux"))]
	{
		let _ = path;
		false
	}
}

/// Extracts inherited owner, group, and other permission bits from a Linux
/// default ACL.
#[cfg(target_os = "linux")]
pub(crate) fn get_acl_perm_bits_from_xattr(path: impl AsRef<Path>) -> u32 {
	get_xattr(path.as_ref(), b"system.posix_acl_default\0")
		.ok()
		.and_then(|value| parse_default_acl_permissions(&value))
		.unwrap_or(0)
}

#[cfg(target_os = "linux")]
fn path_cstring(path: &Path) -> io::Result<ffi::CString> {
	use std::os::unix::ffi::OsStrExt;
	ffi::CString::new(path.as_os_str().as_bytes())
		.map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))
}

#[cfg(target_os = "linux")]
fn list_xattrs(path: &Path) -> io::Result<Vec<u8>> {
	let path = path_cstring(path)?;
	// SAFETY: the C path is valid and a null buffer with length zero performs a
	// size query.
	let size = unsafe { libc::listxattr(path.as_ptr(), ptr::null_mut(), 0) };
	if size < 0 {
		return Err(io::Error::last_os_error());
	}
	if size == 0 {
		return Ok(Vec::new());
	}
	let mut names = vec![0_u8; size as usize];
	// SAFETY: `names` is writable for its full reported capacity.
	let read = unsafe { libc::listxattr(path.as_ptr(), names.as_mut_ptr().cast(), names.len()) };
	if read < 0 {
		return Err(io::Error::last_os_error());
	}
	names.truncate(read as usize);
	Ok(names)
}

#[cfg(target_os = "linux")]
fn get_xattr(path: &Path, name: &[u8]) -> io::Result<Vec<u8>> {
	let path = path_cstring(path)?;
	let name = ffi::CStr::from_bytes_with_nul(name)
		.map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid attribute name"))?;
	// SAFETY: both C strings are valid; a null value pointer performs a size query.
	let size = unsafe { libc::getxattr(path.as_ptr(), name.as_ptr(), ptr::null_mut(), 0) };
	if size < 0 {
		return Err(io::Error::last_os_error());
	}
	let mut value = vec![0_u8; size as usize];
	// SAFETY: `value` is writable for the queried size and all pointers remain
	// live.
	let read = unsafe {
		libc::getxattr(path.as_ptr(), name.as_ptr(), value.as_mut_ptr().cast(), value.len())
	};
	if read < 0 {
		return Err(io::Error::last_os_error());
	}
	value.truncate(read as usize);
	Ok(value)
}

#[cfg(target_os = "linux")]
fn parse_default_acl_permissions(value: &[u8]) -> Option<u32> {
	const ACL_USER_OBJ: u16 = 0x01;
	const ACL_GROUP_OBJ: u16 = 0x04;
	const ACL_MASK: u16 = 0x10;
	const ACL_OTHER: u16 = 0x20;
	if value.len() < 4 || u32::from_le_bytes(value[..4].try_into().ok()?) != 2 {
		return None;
	}
	let mut owner = None;
	let mut group = None;
	let mut mask = None;
	let mut other = None;
	for entry in value[4..].chunks_exact(8) {
		let tag = u16::from_le_bytes([entry[0], entry[1]]);
		let permissions = u32::from(u16::from_le_bytes([entry[2], entry[3]]) & 0o7);
		match tag {
			ACL_USER_OBJ => owner = Some(permissions),
			ACL_GROUP_OBJ => group = Some(permissions),
			ACL_MASK => mask = Some(permissions),
			ACL_OTHER => other = Some(permissions),
			_ => {},
		}
	}
	Some((owner? << 6) | (mask.or(group)? << 3) | other?)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
	use super::*;

	#[test]
	fn copies_binary_and_empty_attributes_without_changing_source() {
		let dir = tempfile::tempdir().unwrap();
		let source = dir.path().join("source");
		let destination = dir.path().join("destination");
		std::fs::write(&source, b"source").unwrap();
		std::fs::write(&destination, b"destination").unwrap();
		set_xattr(&source, ffi::OsStr::new("user.omp.binary"), &[0, 255, 1]).unwrap();
		set_xattr(&source, ffi::OsStr::new("user.omp.empty"), &[]).unwrap();
		let before = retrieve_xattrs(&source).unwrap();
		copy_xattrs(&source, &destination).unwrap();
		assert_eq!(retrieve_xattrs(&destination).unwrap(), before);
		assert_eq!(retrieve_xattrs(&source).unwrap(), before);
		assert_eq!(std::fs::read(&destination).unwrap(), b"destination");
	}

	#[test]
	fn missing_destination_propagates_error_and_preserves_source_attributes() {
		let dir = tempfile::tempdir().unwrap();
		let source = dir.path().join("source");
		std::fs::write(&source, b"source").unwrap();
		set_xattr(&source, ffi::OsStr::new("user.omp.keep"), b"evidence").unwrap();
		let error = copy_xattrs(&source, dir.path().join("missing")).unwrap_err();
		assert_eq!(error.kind(), io::ErrorKind::NotFound);
		assert_eq!(get_xattr(&source, b"user.omp.keep\0").unwrap(), b"evidence");
	}

	#[test]
	fn parses_default_acl_mode_bits() {
		let mut value = 2_u32.to_le_bytes().to_vec();
		for (tag, permissions) in [(1_u16, 7_u16), (4, 5), (16, 4), (32, 1)] {
			value.extend_from_slice(&tag.to_le_bytes());
			value.extend_from_slice(&permissions.to_le_bytes());
			value.extend_from_slice(&u32::MAX.to_le_bytes());
		}
		assert_eq!(parse_default_acl_permissions(&value), Some(0o741));
	}
}
