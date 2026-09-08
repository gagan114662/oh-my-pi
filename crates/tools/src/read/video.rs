//! Video selector grammar and the environment-owned extraction boundary.

use std::path::Path;

use bytes::Bytes;
use omp_core::Str;
use serde::{Deserialize, Serialize};

/// A zero-based frame index, timestamp, or evenly sampled preview grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Selection {
	/// A 3 by 3 contact sheet covering the video duration.
	Preview,
	/// A zero-based decoded frame index.
	Frame(u64),
	/// Timestamp in seconds from the start.
	Time(f64),
}

/// Closed video failure facts carried across the read tool's wire boundary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, thiserror::Error)]
pub enum VideoFault {
	/// Invalid frame or timestamp syntax.
	#[error(
		"Invalid video selector: use a zero-based frame (412, f412, frame412) or timestamp \
		 (1h5m42s, 01:05:42, 42.5)."
	)]
	Selector,
	/// This source adapter cannot extract video.
	#[error("Video extraction is unavailable in this environment.")]
	Unavailable,
	/// ffmpeg or ffprobe is absent from the environment's PATH.
	#[error(
		"Video reads require ffmpeg and ffprobe on the environment PATH; install ffmpeg and retry."
	)]
	MissingBinary,
	/// A host I/O operation failed with an optional OS error code.
	#[error("Video extraction I/O failed (OS code {code:?}).")]
	Io {
		/// Host OS error code, when available.
		code: Option<i32>,
	},
	/// A media utility exited unsuccessfully.
	#[error("Video utility failed (exit code {code:?}); the source may be corrupt or unsupported.")]
	Process {
		/// Child exit status, absent when terminated by a signal.
		code: Option<i32>,
	},
	/// The bounded extraction deadline elapsed.
	#[error("Video extraction exceeded its 30 second deadline.")]
	Deadline,
	/// Metadata or PNG output exceeded its byte limit.
	#[error("Video utility output exceeded the bounded output limit.")]
	OutputLimit,
	/// No valid finite-duration video stream was available.
	#[error("Video metadata has no usable video stream, dimensions, or finite positive duration.")]
	Metadata,
	/// Timestamp or frame lies outside the source.
	#[error("The requested video frame or timestamp lies outside the source.")]
	OutOfRange,
	/// The media utility returned no PNG image.
	#[error("Video extraction returned no PNG frame.")]
	NoFrame,
}

/// Materialized PNG with metadata ready for durable blob storage.
#[derive(Debug)]
pub struct Output {
	/// The extracted frame or contact sheet, encoded as PNG.
	pub png:         Bytes,
	/// Human and model readable source metadata and selection.
	pub description: Str,
}

/// Whether a local path names a supported video container.
pub fn is_video(path: &str) -> bool {
	Path::new(path)
		.extension()
		.and_then(|part| part.to_str())
		.is_some_and(|extension| {
			["mp4", "mov", "mkv", "webm", "m4v", "avi", "wmv"]
				.iter()
				.any(|candidate| extension.eq_ignore_ascii_case(candidate))
		})
}

/// Split after a video extension, retaining colons inside timestamps.
/// Callers must check a literal existing filename before using this split.
pub fn split_target(path: &str) -> Option<(&str, &str)> {
	path.char_indices().find_map(|(index, character)| {
		(character == ':' && is_video(&path[..index])).then_some((&path[..index], &path[index + 1..]))
	})
}

/// Parse the v1 video selector spellings without treating an integer as
/// seconds.
pub fn parse(selector: Option<&str>) -> Result<Selection, VideoFault> {
	let Some(value) = selector.map(str::trim).filter(|value| !value.is_empty()) else {
		return Ok(Selection::Preview);
	};
	let lower = value.to_ascii_lowercase();
	let frame = lower
		.strip_prefix("frame")
		.or_else(|| lower.strip_prefix('f'))
		.unwrap_or(&lower);
	if !frame.is_empty() && frame.bytes().all(|byte| byte.is_ascii_digit()) {
		let frame: u64 = frame.parse().map_err(|_| VideoFault::Selector)?;
		if frame > 9_007_199_254_740_991 {
			return Err(VideoFault::Selector);
		}
		return Ok(Selection::Frame(frame));
	}
	let number = |part: &str| -> Result<f64, VideoFault> {
		if part.is_empty()
			|| !part
				.bytes()
				.all(|byte| byte.is_ascii_digit() || byte == b'.')
		{
			return Err(VideoFault::Selector);
		}
		let value: f64 = part.parse().map_err(|_| VideoFault::Selector)?;
		if value.is_finite() && value >= 0.0 {
			Ok(value)
		} else {
			Err(VideoFault::Selector)
		}
	};
	let seconds = if lower.contains(':') {
		let parts: smallvec::SmallVec<&str, 3> = lower.split(':').collect();
		if !(2..=3).contains(&parts.len()) {
			return Err(VideoFault::Selector);
		}
		let mut total = 0.0;
		for (index, part) in parts.iter().enumerate() {
			let component = number(part)?;
			if (index > 0 || parts.len() == 2) && component >= 60.0 {
				return Err(VideoFault::Selector);
			}
			if index + 1 < parts.len() && part.contains('.') {
				return Err(VideoFault::Selector);
			}
			total = total * 60.0 + component;
		}
		total
	} else if lower.ends_with(['h', 'm', 's']) {
		let mut remaining = lower.as_str();
		let mut total = 0.0;
		let mut found = false;
		for (unit, scale) in [('h', 3600.0), ('m', 60.0), ('s', 1.0)] {
			if let Some(index) = remaining.find(unit) {
				total += number(&remaining[..index])? * scale;
				remaining = &remaining[index + 1..];
				found = true;
			}
		}
		if !found || !remaining.is_empty() {
			return Err(VideoFault::Selector);
		}
		total
	} else if lower.contains('.') {
		number(&lower)?
	} else {
		return Err(VideoFault::Selector);
	};
	if !seconds.is_finite() {
		return Err(VideoFault::Selector);
	}
	Ok(Selection::Time(seconds))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn v1_video_selectors_keep_frames_distinct_from_timestamps() {
		for suffix in ["412", "f412", "FRAME412"] {
			assert_eq!(parse(Some(suffix)), Ok(Selection::Frame(412)));
		}
		for suffix in ["1h5m42s", "01:05:42", "3942.0", "65m42s"] {
			assert_eq!(parse(Some(suffix)), Ok(Selection::Time(3942.0)));
		}
		assert_eq!(parse(Some("0")), Ok(Selection::Frame(0)));
		assert_eq!(parse(Some("0s")), Ok(Selection::Time(0.0)));
		assert_eq!(split_target("clip.MP4:01:05:42"), Some(("clip.MP4", "01:05:42")));
		for suffix in ["-1", "NaN", "1:60", "1s2h", "1.2.3", "f18446744073709551616", "1s;touch x"] {
			assert!(parse(Some(suffix)).is_err(), "{suffix}");
		}
	}
}
