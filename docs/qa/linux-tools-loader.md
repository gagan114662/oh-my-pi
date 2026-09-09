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

## Observed A/B result and final-link correction

[Run 34255101131](https://github.com/gagan114662/oh-my-pi/actions/runs/34255101131)
built both parent arms successfully. The actual compiler commands confirm the
requested backend difference. Both binaries then returned -11 on discovery with
libc 2.39-0ubuntu8.8, in `dl_main` at `elf/rtld.c:2397`. The tools-only LLVM
binary SHA-256 was 986e6f674744d60b9ac905a19da800b6e0a2d8e2eb94eb8aad1977e1889d2111;
the current binary matched the earlier parent hash above. Both ELF headers had
entry point **0x0**. Both compiler invocations carried
`-C link-arg=-Wl,-export_dynamic` from omp-tools/build.rs.

That argument uses ld64's spelling. The ELF linker interprets it as entry symbol
`xport_dynamic`, leaving a zero entry point when that symbol is absent. A bounded
local cross-target assembly/link test with LLVM 22 confirmed the same behavior:
`ld.lld -export_dynamic` emitted the missing-entry-symbol warning and entry 0;
`ld.lld --export-dynamic` on the identical `_start` object emitted no warning and
entry 0x201120. This tested linking, not executing the Linux binary on macOS.

The fix applies the target-specific flag selection already used by app, e2e,
and py build scripts to the sole remaining unconditional site in tools. It does
not change any compilation backend. Linux diagnostics now assert nonzero ELF
entry in addition to executing the same actual discovery command. The normal
read-tail Linux job also runs all omp-tools test targets and doctests, retaining
their actual exits. The A/B leaf has returned to manual-only after registration;
its parent artifacts remain the unchanged failure evidence. Hosted execution of
the corrected build is still required before claiming runtime success.
