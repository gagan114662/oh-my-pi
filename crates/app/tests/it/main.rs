//! Consolidated omp-app integration harness; one binary links the embedded
//! `CPython` image once instead of once per test file.

#[cfg(unix)]
mod envd_contract;
#[cfg(unix)]
mod envd_documents;
mod envd_policy;
#[cfg(windows)]
mod envd_windows;
#[cfg(unix)]
mod envd_workspace;
mod process_smoke;
mod stock_sdk_clients;
#[cfg(unix)]
mod tool_worker;
#[cfg(windows)]
mod windows_named_pipe;
mod zz_sizes;

/// Keep contained extension failures visible in the failing test's output.
#[cfg(unix)]
fn init_extension_test_tracing() {
	static INIT: std::sync::Once = std::sync::Once::new();
	INIT.call_once(|| {
		tracing_subscriber::fmt()
			.with_max_level(tracing::Level::WARN)
			.with_test_writer()
			.with_ansi(false)
			.try_init()
			.expect("install integration-test diagnostics");
	});
}
