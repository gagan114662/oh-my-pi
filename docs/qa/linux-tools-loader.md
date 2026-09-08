# Linux tools loader diagnosis

The parent b59b952172f1f331c2a79a6bc22f6ea25a3e2135 `omp-tools` libtest also
exits with SIGSEGV (-11) on `--list --format terse`. Its recorded SHA-256 is
739b98cb870b37fc115caf0866ddfde00adc6deda4dc8b663ddec34375643ce3.
The original GDB trace points into ld-linux before test execution, without loader
symbols. This establishes that the new read selectors are not necessary to
trigger the fault; it does not establish a codegen or linker root cause.

The workspace selects Cranelift for most workspace crates, LLVM for third-party
packages and certain workspace exceptions, including omp-con. omp-tools itself
still expands omp_con::var! declarations into linkme distributed-slice statics.
This makes its compilation backend worth isolating, without assuming those
statics cause the crash.

The upstream [linkme symbol issue](https://github.com/rust-lang/rustc_codegen_cranelift/issues/1689)
concerns Mach-O symbol mangling and linker resolution, not this Linux runtime
signature. The upstream [Valgrind SIGSEGV report](https://github.com/rust-lang/rustc_codegen_cranelift/issues/1424)
concerns stack probes during application execution, also a different signature.
Neither is evidence of this failure's mechanism.

Manually dispatch `.github/workflows/linux-tools-loader.yml` with an exact
`source_ref`. Its two fresh Ubuntu 24.04 jobs build only the selected source's
omp-tools libtest and dependencies, using identical pinned toolchain and setup.
The current arm applies no override. The tools-llvm arm adds only:

```text
--config profile.dev.package.omp-tools.codegen-backend="llvm"
```

This is a diagnostic CLI override, not a production configuration change. Verbose
Cargo commands, source/config hashes, toolchain/runtime versions, binary hashes,
raw discovery exits, both binaries, ELF and symbolized loader traces are retained.
Each failure remains a failed job; neither arm retries or suppresses failures.
This is discovery evidence, not executed test or doctest coverage.

If only tools-llvm succeeds, the intervention narrows the failure to changing
omp-tools' emitted objects/link result, but still does not prove linkme caused it.
If both crash, backend choices in dependencies or other loader/linker inputs
remain candidates. Compare symbolized fault locations and ELF relocations before
choosing the next experiment. A full-workspace LLVM comparison would be a separate
broader intervention and must not be confused with this package-only comparison.
