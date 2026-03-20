#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 4 ]; then
  echo "usage: $0 <binary-path> <tag> <target> <out-dir>" >&2
  exit 1
fi

binary_path="$1"
tag="$2"
target="$3"
out_dir="$4"

archive_base="taida-${tag}-${target}"
stage_root="$(mktemp -d)"
stage_dir="${stage_root}/${archive_base}"

mkdir -p "${stage_dir}" "${out_dir}"
cp "${binary_path}" "${stage_dir}/taida"
cp README.md "${stage_dir}/README.md"
cp PHILOSOPHY.md "${stage_dir}/PHILOSOPHY.md"
cp LICENSE "${stage_dir}/LICENSE"

tar -C "${stage_root}" -czf "${out_dir}/${archive_base}.tar.gz" "${archive_base}"
rm -rf "${stage_root}"
