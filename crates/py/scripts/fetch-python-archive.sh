#!/usr/bin/env bash
# Fetch and validate the complete archive before replacing an installed tree.
# Separate stages prevent a tar reader closing early from signalling upstream
# download/decompression processes, and retain the exact failing stage status.
set -euo pipefail

URL=$1
VENDOR=$2
NAME=$3
TMP=$(mktemp -d "$(dirname "$VENDOR")/.fetch-py.XXXXXX")
trap 'rm -rf "$TMP"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

stage() {
	local LABEL=$1
	shift
	local STATUS
	if "$@"; then
		echo "python archive ${LABEL}: exit 0" >&2
	else
		STATUS=$?
		echo "error: python archive ${LABEL}: exit ${STATUS}" >&2
		exit "$STATUS"
	fi
}

stage download curl -fsSL --output "$TMP/archive.tar.zst" "$URL"
stage decompress zstd -d -o "$TMP/archive.tar" "$TMP/archive.tar.zst"
rm "$TMP/archive.tar.zst"
mkdir "$TMP/extracted"
stage extract tar -xf "$TMP/archive.tar" -C "$TMP/extracted"
if [ ! -d "$TMP/extracted/python" ] || [ -L "$TMP/extracted/python" ]; then
	echo 'error: python archive missing regular python directory' >&2
	exit 1
fi
printf '%s\n' "$NAME" > "$TMP/extracted/python/.archive.stamp"
rm -rf "$VENDOR"
mv "$TMP/extracted/python" "$VENDOR"
