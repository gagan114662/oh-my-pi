# Build prerequisites

Run `just doctor` before a build. It checks prerequisites offline without
installing anything or compiling Rust. It exits nonzero and lists repairs for
missing tools. With only Rust installed, bootstrap `just` and Python 3.11+ first;
`python3 scripts/build-doctor.py` also works without just. The version probes
share a two-second deadline. A successful doctor means prerequisites were
detected, not that a clean workspace build or test sweep passed.

On Apple Silicon macOS, install Xcode Command Line Tools (`xcode-select
--install`) and `brew install just python cmake ninja lld uv`. The checked-in
Rust flags require **`/opt/homebrew/bin/ld64.lld` for development links too**.
Run `rustup show` in the checkout to install the pinned toolchain and components,
then install nextest (`cargo install cargo-nextest --locked`) and run
`just setup-python` to prepare the embedded CPython bundle and frozen packages.

Linux builds need a C/C++ compiler, CMake 3.15+, Ninja, pkg-config, Python 3.11+,
just, uv, nextest, and the pinned Rust toolchain. Native development libraries
also depend on the chosen packages and GUI/audio backends; the doctor does not
yet exhaustively validate their headers and link libraries. Linux does not use
the Apple Silicon linker path. The doctor currently covers Linux and macOS;
Windows native prerequisites still require separate verification.

`audiopus_sys 0.2.2` vendors Opus with an old CMake policy declaration. The
workspace sets `CMAKE_POLICY_VERSION_MINIMUM = "3.5"` in `.cargo/config.toml`
so CMake 4 can configure it without a shell workaround. Do not override this
with a lower value in your environment. Ninja remains the configured generator.

`just doctor-test` exercises missing commands with an isolated PATH, Apple
Silicon linker detection, old CMake, wrong toolchain, and missing/stale Python
inputs. These tests are diagnostics proofs; they do not substitute for issue
#39's recorded clean-host build/failure demonstration.

## Feature graph work still required

Workspace feature unification remains enabled. `omp-app` and `omp-chat` import
realtime APIs, so removing their realtime feature would break real behavior.
Root serde already enables `derive` and `rc`; hmac and sha2 use their default
features. The historical package-unification errors must be reproduced against
the current lockfile before making a targeted feature fix. No claim is made
that package-isolated compilation works, or that Opus is absent from unrelated
package builds. Completing #39 requires that feature-graph proof and a clean
host run with uploaded logs and readable CI summary, in addition to the doctor.
