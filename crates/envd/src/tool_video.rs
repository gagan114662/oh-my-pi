//! Bounded environment-owned ffprobe/ffmpeg video extraction.

use std::{
	fs::File,
	io::{self, Seek as _},
	path::Path,
	process::Stdio,
	time::Duration,
};

use bytes::Bytes;
use omp_core::Str;
use omp_tools::read::video::{Output, Selection, VideoFault};
use serde::Deserialize;
use tokio::{
	io::{AsyncRead, AsyncReadExt as _},
	process::Command,
};

const PNG_LIMIT: usize = 8 * 1024 * 1024;
const PROBE_LIMIT: usize = 256 * 1024;
const STDERR_LIMIT: usize = 64 * 1024;

/// Internal source-preserving failures, projected only at the tool boundary.
#[derive(Debug, thiserror::Error)]
enum Error {
	#[error("video utility I/O failed")]
	Io(#[from] io::Error),
	#[error("video utility could not start")]
	Spawn(#[source] io::Error),
	#[error("video metadata JSON failed")]
	Json(#[from] serde_json::Error),
	#[error(transparent)]
	Fault(#[from] VideoFault),
}

impl Error {
	fn into_fault(self) -> VideoFault {
		match self {
			Self::Spawn(error) if error.kind() == io::ErrorKind::NotFound => VideoFault::MissingBinary,
			Self::Io(error) | Self::Spawn(error) => VideoFault::Io { code: error.raw_os_error() },
			Self::Json(_) => VideoFault::Metadata,
			Self::Fault(fault) => fault,
		}
	}
}

async fn read_bounded(reader: impl AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>, Error> {
	let mut bytes = Vec::new();
	reader
		.take((limit + 1) as u64)
		.read_to_end(&mut bytes)
		.await?;
	if bytes.len() > limit {
		return Err(VideoFault::OutputLimit.into());
	}
	Ok(bytes)
}

async fn run(
	program: &str,
	arguments: &[String],
	limit: usize,
	source: &mut File,
	deadline: tokio::time::Instant,
) -> Result<Vec<u8>, Error> {
	if tokio::time::Instant::now() >= deadline {
		return Err(VideoFault::Deadline.into());
	}
	source.rewind()?;
	let input = source.try_clone()?;
	let mut child = Command::new(program)
		.args(arguments)
		.stdin(Stdio::from(input))
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.kill_on_drop(true)
		.spawn()
		.map_err(Error::Spawn)?;
	let stdout = child.stdout.take().expect("piped video stdout");
	let stderr = child.stderr.take().expect("piped video stderr");
	let operation = async {
		let (stdout, _stderr, status) = tokio::try_join!(
			read_bounded(stdout, limit),
			read_bounded(stderr, STDERR_LIMIT),
			async { child.wait().await.map_err(Error::Io) },
		)?;
		if !status.success() {
			return Err(VideoFault::Process { code: status.code() }.into());
		}
		Ok(stdout)
	};
	match tokio::time::timeout_at(deadline, operation).await {
		Ok(Ok(output)) => Ok(output),
		result => {
			// Bounds/deadlines stop and reap the child. Dropping the outer read
			// future also kills it through Tokio's kill-on-drop ownership.
			let cleanup = async {
				child.start_kill()?;
				child.wait().await?;
				Ok::<(), io::Error>(())
			};
			match tokio::time::timeout(Duration::from_secs(2), cleanup).await {
				Ok(result) => result?,
				Err(_) => return Err(VideoFault::CleanupDeadline.into()),
			}
			match result {
				Ok(Err(error)) => Err(error),
				Err(_) => Err(VideoFault::Deadline.into()),
				Ok(Ok(_)) => unreachable!(),
			}
		},
	}
}

#[derive(Deserialize)]
struct Probe {
	#[serde(default)]
	streams: Vec<Stream>,
	format:  Format,
}

#[derive(Deserialize)]
struct Stream {
	codec_type:     Option<String>,
	codec_name:     Option<String>,
	width:          Option<u32>,
	height:         Option<u32>,
	avg_frame_rate: Option<String>,
	nb_frames:      Option<String>,
}

#[derive(Deserialize)]
struct Format {
	duration:    Option<String>,
	format_name: Option<String>,
}

pub(crate) async fn extract(path: &Path, selection: Selection) -> Result<Output, VideoFault> {
	extract_inner(path, selection)
		.await
		.map_err(Error::into_fault)
}

/// Container selection prevents playlist/concat autodetection from reading
/// resources other than the approved source. MOV external references stay off.
#[derive(Clone, Copy, strum::EnumString, strum::IntoStaticStr)]
#[strum(ascii_case_insensitive)]
enum Demuxer {
	#[strum(serialize = "mp4", serialize = "mov", serialize = "m4v", to_string = "mov")]
	Mov,
	#[strum(serialize = "mkv", serialize = "webm", to_string = "matroska")]
	Matroska,
	#[strum(serialize = "avi", to_string = "avi")]
	Avi,
	#[strum(serialize = "wmv", to_string = "asf")]
	Asf,
}

#[cfg(unix)]
fn open_source(path: &Path) -> io::Result<File> {
	use std::path::Component;

	use rustix::fs::{Mode, OFlags, open, openat};
	if !path.is_absolute() {
		return Err(io::Error::from(io::ErrorKind::InvalidInput));
	}
	// Walk from a held root descriptor. No component, including a swapped
	// ancestor, may become a symlink between resolution and opening.
	let mut directory =
		open("/", OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty())?;
	let mut components = path.components().peekable();
	while let Some(component) = components.next() {
		match component {
			Component::RootDir => continue,
			Component::Normal(name) => {
				let mut flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK;
				if components.peek().is_some() {
					flags |= OFlags::DIRECTORY;
				}
				let opened = openat(&directory, name, flags, Mode::empty())?;
				if components.peek().is_none() {
					let file = File::from(opened);
					if !file.metadata()?.is_file() {
						return Err(io::Error::from(io::ErrorKind::InvalidInput));
					}
					return Ok(file);
				}
				directory = opened;
			},
			_ => return Err(io::Error::from(io::ErrorKind::InvalidInput)),
		}
	}
	Err(io::Error::from(io::ErrorKind::InvalidInput))
}

#[cfg(not(unix))]
fn open_source(_path: &Path) -> io::Result<File> {
	Err(io::Error::from(io::ErrorKind::Unsupported))
}

async fn extract_inner(path: &Path, selection: Selection) -> Result<Output, Error> {
	let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
	let demuxer: Demuxer = path
		.extension()
		.and_then(|value| value.to_str())
		.ok_or(VideoFault::Metadata)?
		.parse()
		.map_err(|_| VideoFault::Metadata)?;
	let mut source = open_source(path)?;
	let initial = source.metadata()?;
	let path = path.to_str().ok_or(VideoFault::Metadata)?;
	let demuxer_name: &'static str = demuxer.into();
	let mut input_options = vec![
		"-protocol_whitelist".to_owned(),
		"fd,pipe".to_owned(),
		"-f".to_owned(),
		demuxer_name.to_owned(),
		"-fd".to_owned(),
		"0".to_owned(),
	];
	if matches!(demuxer, Demuxer::Mov) {
		input_options.extend([
			"-enable_drefs".to_owned(),
			"0".to_owned(),
			"-use_absolute_path".to_owned(),
			"0".to_owned(),
		]);
	}
	let mut arguments = [
		"-v",
		"error",
		"-show_entries",
		"format=duration,format_name:stream=codec_type,codec_name,width,height,avg_frame_rate,\
		 nb_frames",
		"-of",
		"json",
	]
	.map(str::to_owned)
	.to_vec();
	arguments.extend(input_options.iter().cloned());
	arguments.push("fd:".to_owned());
	let probe: Probe = serde_json::from_slice(
		&run("ffprobe", &arguments, PROBE_LIMIT, &mut source, deadline).await?,
	)?;
	let video = probe
		.streams
		.iter()
		.find(|stream| stream.codec_type.as_deref() == Some("video"))
		.ok_or(VideoFault::Metadata)?;
	let duration = probe
		.format
		.duration
		.as_deref()
		.and_then(|value| value.parse::<f64>().ok())
		.filter(|value| value.is_finite() && *value > 0.0)
		.ok_or(VideoFault::Metadata)?;
	let (width, height) = video
		.width
		.zip(video.height)
		.filter(|(w, h)| {
			*w > 0
				&& *h > 0
				&& *w <= 16384
				&& *h <= 16384
				&& u64::from(*w) * u64::from(*h) <= 64 * 1024 * 1024
		})
		.ok_or(VideoFault::Metadata)?;
	match selection {
		Selection::Time(seconds) if !seconds.is_finite() || seconds < 0.0 || seconds >= duration => {
			return Err(VideoFault::OutOfRange.into());
		},
		Selection::Frame(frame)
			if video
				.nb_frames
				.as_deref()
				.and_then(|value| value.parse::<u64>().ok())
				.is_some_and(|count| frame >= count) =>
		{
			return Err(VideoFault::OutOfRange.into());
		},
		_ => {},
	}
	let mut arguments: Vec<String> = ["-v", "error", "-nostdin", "-threads", "1"]
		.into_iter()
		.map(str::to_owned)
		.collect();
	arguments.extend(input_options);
	if let Selection::Time(seconds) = selection {
		arguments.extend(["-ss".to_owned(), seconds.to_string()]);
	}
	arguments.extend([
		"-i".to_owned(),
		"fd:".to_owned(),
		"-map".to_owned(),
		"0:v:0".to_owned(),
		"-an".to_owned(),
		"-sn".to_owned(),
	]);
	let (filter, label) = match selection {
		Selection::Preview => (
			format!(
				"fps={},scale=320:180:force_original_aspect_ratio=decrease,pad=320:180:(ow-iw)/2:\
				 (oh-ih)/2,tile=3x3",
				9.0 / duration
			),
			"Preview: 3x3 evenly spaced frames".to_owned(),
		),
		Selection::Frame(frame) => (
			format!("select=eq(n\\,{frame}),scale=1280:720:force_original_aspect_ratio=decrease"),
			format!("Frame: {frame} (zero-based)"),
		),
		Selection::Time(seconds) => (
			"scale=1280:720:force_original_aspect_ratio=decrease".to_owned(),
			format!("Timestamp: {seconds:.3}s"),
		),
	};
	arguments.extend([
		"-vf".to_owned(),
		filter,
		"-frames:v".to_owned(),
		"1".to_owned(),
		"-threads".to_owned(),
		"1".to_owned(),
		"-f".to_owned(),
		"image2pipe".to_owned(),
		"-c:v".to_owned(),
		"png".to_owned(),
		"pipe:1".to_owned(),
	]);
	let png = run("ffmpeg", &arguments, PNG_LIMIT, &mut source, deadline).await?;
	let final_metadata = source.metadata()?;
	if initial.len() != final_metadata.len() || initial.modified()? != final_metadata.modified()? {
		return Err(VideoFault::SourceChanged.into());
	}
	if !png.starts_with(b"\x89PNG\r\n\x1a\n") {
		return Err(VideoFault::NoFrame.into());
	}
	let audio = probe
		.streams
		.iter()
		.find(|stream| stream.codec_type.as_deref() == Some("audio"));
	let description = format!(
		"Video: {path}\n{label}\nDuration: {duration:.3}s\nResolution: {width}x{height}\nFrame \
		 rate: {}\nVideo codec: {}\nAudio codec: {}\nContainer: {}",
		video.avg_frame_rate.as_deref().unwrap_or("unknown"),
		video.codec_name.as_deref().unwrap_or("unknown"),
		audio
			.and_then(|stream| stream.codec_name.as_deref())
			.unwrap_or("none"),
		probe.format.format_name.as_deref().unwrap_or("unknown")
	);
	Ok(Output { png: Bytes::from(png), description: Str::from(description) })
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn media_output_is_bounded_before_collection_finishes() {
		assert!(matches!(
			read_bounded(&b"12345"[..], 4).await,
			Err(Error::Fault(VideoFault::OutputLimit))
		));
		assert_eq!(read_bounded(&b"1234"[..], 4).await.unwrap(), b"1234");
	}
	#[cfg(unix)]
	#[test]
	fn held_video_source_survives_replacement_and_rejects_symlinks() {
		use std::{io::Read as _, os::unix::fs::symlink};
		let directory = tempfile::tempdir().unwrap();
		let root = directory.path().canonicalize().unwrap();
		let source = root.join("source.mp4");
		std::fs::write(&source, b"original").unwrap();
		let mut held = open_source(&source).unwrap();
		std::fs::rename(&source, root.join("old.mp4")).unwrap();
		std::fs::write(&source, b"replacement").unwrap();
		let mut bytes = Vec::new();
		held.read_to_end(&mut bytes).unwrap();
		assert_eq!(bytes, b"original");
		symlink(&source, root.join("link.mp4")).unwrap();
		assert!(open_source(&root.join("link.mp4")).is_err());
		symlink(&root, root.join("alias")).unwrap();
		assert!(open_source(&root.join("alias/source.mp4")).is_err());
	}

	#[cfg(unix)]
	#[tokio::test]
	async fn media_deadline_and_output_limit_kill_and_reap_children() {
		let mut input = tempfile::tempfile().unwrap();
		let deadline = tokio::time::Instant::now() + Duration::from_millis(50);
		let result = tokio::time::timeout(
			Duration::from_secs(3),
			run("/bin/sh", &["-c".to_owned(), "exec sleep 30".to_owned()], 1024, &mut input, deadline),
		)
		.await
		.expect("deadline and cleanup are bounded");
		// Deadline is returned only after successful wait; cleanup timeout
		// produces a distinct error, so this also proves child reaping.
		assert!(matches!(result, Err(Error::Fault(VideoFault::Deadline))));
		let result = run(
			"/usr/bin/printf",
			&["12345".to_owned()],
			4,
			&mut input,
			tokio::time::Instant::now() + Duration::from_secs(2),
		)
		.await;
		assert!(matches!(result, Err(Error::Fault(VideoFault::OutputLimit))));
	}
}
