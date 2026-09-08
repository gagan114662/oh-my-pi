#!/bin/sh
# The bootstrap path needs only the host shell. The full doctor remains Python.
if command -v python3 >/dev/null 2>&1; then
	case "$0" in
		*/*) doctor_directory=${0%/*} ;;
		*) doctor_directory=. ;;
	esac
	exec python3 "$doctor_directory/build-doctor.py" "$@"
fi

printf '%s\n' 'FAIL python3: Install Python 3.11+ to run the complete build doctor.'
for tool in just cc c++ cmake ninja uv curl zstd tar cargo-nextest; do
	if ! command -v "$tool" >/dev/null 2>&1; then
		case "$tool" in
			cmake) remedy='Install CMake 3.15 or newer (cmake package).' ;;
			ninja) remedy='Install Ninja (ninja-build package; brew install ninja).' ;;
			just) remedy='Install just (cargo install just --locked; brew install just).' ;;
			cc|c++) remedy='Install a C/C++ compiler or Xcode Command Line Tools.' ;;
			cargo-nextest) remedy='Install cargo-nextest (cargo install cargo-nextest --locked).' ;;
			*) remedy="Install $tool; see docs/building.md." ;;
		esac
		printf 'FAIL %s: %s\n' "$tool" "$remedy"
	fi
done
printf '%s\n' \
	'Apple Silicon macOS also requires lld@22 at /opt/homebrew/opt/lld@22/bin/ld64.lld.' \
	'The repository sets the vendored Opus CMake policy in .cargo/config.toml.' \
	'After installing Python, rerun sh scripts/build-doctor.sh for version and runtime checks.'
exit 1
