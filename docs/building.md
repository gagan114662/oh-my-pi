# Build prerequisites

Run `just doctor` before a build. It checks prerequisites offline without
installing anything or compiling Rust. It exits nonzero and lists repairs for
missing tools. With only Rust installed, bootstrap `just` and Python 3.11+ first;
`python3 scripts/build-doctor.py` also works without just. The version probes
share a two-second deadline. A successful doctor means prerequisites were
detected, not that a clean workspace build or test sweep passed.

On Apple Silicon macOS, install Xcode Command Line Tools (`xcode-select
--install`) and `brew install just python cmake ninja lld@22 uv`. The checked-in
Rust flags require **`/opt/homebrew/opt/lld@22/bin/ld64.lld` for development links too**.
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

## Package feature isolation

Feature unification is scoped to the selected packages. `just check-pkg` and
`just test-pkg` therefore use the selected package's dependency features instead
of inheriting every application feature. Whole-workspace commands still build
the full selected workspace. This can produce separate cached dependency
artifacts for different package selections; it avoids compiling audio C code
for unrelated package builds.

Use Cargo's `selected` mode: dependencies shared by packages in the same
invocation still share their feature set. The `package` mode can build distinct
copies of shared types and fails the workspace's transport interfaces on the
pinned toolchain. See [Cargo feature unification](https://doc.rust-lang.org/nightly/cargo/reference/unstable.html#feature-unification).

`omp-app` and `omp-chat` retain their realtime APIs and dependencies. Root serde
explicitly enables `derive` and `rc`; hmac and sha2 retain their default features.
The previously reported `omp-secrets` and `omp-catalog` package-isolation
typecheck errors did not reproduce with the current lockfile.

Dependency-tree and isolated typecheck results do not establish a clean-host
build or a passing test sweep. Completing #39 still requires the recorded
clean-host demonstration, full affected tests and doctests, and hosted evidence
that unrelated package builds exclude audio C dependencies.

### Minimal-container prerequisite evidence

The `pristine-prerequisites` job in `build-isolation.yml` runs the actual
`just doctor` from an archived source revision in a newly created Debian
container. It installs the pinned Rust toolchain and its declared components,
plus `just` and Python to launch the doctor. Native build prerequisites
(including CMake and Ninja) are genuinely absent, verified against normal
PATH discovery and the full package inventory; they are not hidden by a
restricted PATH or replaced with stubs. The measured invocation has no
network and no host toolchain, vendor, or Cargo-cache mounts.

The job requires a nonzero diagnostic exit, actionable missing-CMake and
missing-Ninja lines, recognition of the installed pinned Rust components,
and elapsed wall time below three seconds. It retains source/checker hashes,
container image identity/history, package/tool inventories, raw diagnostics,
measured time, and pass/fail observations even when the check fails.

This proves the minimal **Linux container** missing-prerequisite path in
roadmap Appendix E. It is not a pristine macOS VM, not literally a Rust-only
installation (the diagnostic launcher needs just/Python), and not a full
workspace test sweep. The Apple Silicon lld prerequisite still needs separate
macOS evidence; configured macOS builds and simulated missing-linker unit
tests must retain their separate labels.
