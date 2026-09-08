//! Installed-product entry point for the external edit benchmark evaluator.
//!
//! The adapter is embedded so releases do not depend on a source checkout.
//! Bun is an explicit host prerequisite, never downloaded implicitly. Measured
//! turns run the configured production `omp --mode json` binaries.

use std::{ffi::OsString, path::PathBuf};

use miette::{IntoDiagnostic as _, WrapErr as _, miette};

use crate::cli::BenchCommand;

const ADAPTER: &str = include_str!("../../../scripts/metaharness/adapter.ts");
const RULER_ADAPTER: &str = include_str!("../../../scripts/metaharness/ruler.ts");
const ENTRY_POINT: &str = "\nif (import.meta.main) await main(Bun.argv.slice(2));\n";

fn invocation(command: BenchCommand) -> (PathBuf, Vec<OsString>) {
	match command {
		BenchCommand::Arm(args) => {
			let mut argv = vec![OsString::from("run"), args.manifest.into_os_string()];
			if args.same_commit {
				argv.push("--same-commit".into());
			}
			if let Some(base) = args.base {
				argv.extend([OsString::from("--base"), OsString::from(base.as_str())]);
			}
			(args.bun, argv)
		},
		BenchCommand::Build(args) => {
			let mut argv = vec![
				OsString::from("build"),
				args.source.into_os_string(),
				args.binary.into_os_string(),
				args.provenance.into_os_string(),
				OsString::from("--"),
			];
			argv.extend(args.command.iter().map(|arg| OsString::from(arg.as_str())));
			(args.bun, argv)
		},
	}
}

pub async fn run(command: BenchCommand) -> miette::Result<()> {
	let (bun, argv) = invocation(command);
	let directory = tempfile::Builder::new()
		.prefix("omp-benchmark-")
		.tempdir()
		.into_diagnostic()?;
	tokio::fs::write(directory.path().join("ruler.ts"), RULER_ADAPTER)
		.await
		.into_diagnostic()?;
	let script = directory.path().join("adapter.ts");
	tokio::fs::write(&script, format!("{ADAPTER}{ENTRY_POINT}"))
		.await
		.into_diagnostic()?;
	let mut child = tokio::process::Command::new(&bun)
		.arg(&script)
		.args(argv)
		.kill_on_drop(true)
		.spawn()
		.into_diagnostic()
		.wrap_err_with(|| {
			format!(
				"Could not run benchmark adapter with {}. Install Bun or pass --bun \
				 /absolute/path/to/bun; no runtime is downloaded automatically.",
				bun.display()
			)
		})?;
	#[cfg(unix)]
	let status = {
		let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
			.into_diagnostic()?;
		tokio::select! {
			status = child.wait() => status.into_diagnostic()?,
			_ = tokio::signal::ctrl_c() => {
				stop_adapter(&mut child).await?;
				return Err(miette!("Benchmark interrupted"));
			},
			_ = terminate.recv() => {
				stop_adapter(&mut child).await?;
				return Err(miette!("Benchmark terminated"));
			},
		}
	};
	#[cfg(not(unix))]
	let status = child.wait().await.into_diagnostic()?;
	if !status.success() {
		return Err(miette!("Benchmark adapter failed ({status}); see its diagnostic above"));
	}
	Ok(())
}

/// Let the adapter reap its detached trial groups before removing its script.
#[cfg(unix)]
async fn stop_adapter(child: &mut tokio::process::Child) -> miette::Result<()> {
	if let Some(id) = child.id() {
		let pid = i32::try_from(id).into_diagnostic()?;
		match nix::sys::signal::kill(
			nix::unistd::Pid::from_raw(pid),
			nix::sys::signal::Signal::SIGTERM,
		) {
			Ok(()) | Err(nix::errno::Errno::ESRCH) => {},
			Err(error) => return Err(error).into_diagnostic(),
		}
	}
	match tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await {
		Ok(status) => {
			status.into_diagnostic()?;
		},
		Err(_) => child.kill().await.into_diagnostic()?,
	}

	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::cli::BenchArmArgs;

	#[test]
	fn manifest_and_reference_are_passed_without_shell_interpolation() {
		let (bun, argv) = invocation(BenchCommand::Arm(BenchArmArgs {
			manifest:    PathBuf::from("manifest with spaces.json"),
			same_commit: false,
			base:        Some("HEAD~1".into()),
			bun:         PathBuf::from("/tools/bun"),
		}));
		assert_eq!(bun, PathBuf::from("/tools/bun"));
		assert_eq!(argv, ["run", "manifest with spaces.json", "--base", "HEAD~1"]);
	}
}
