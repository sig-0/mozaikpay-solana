#!/usr/bin/env bash
# Checks that the program deployed at a program id is a local binary followed only by zero padding.
#
# Usage: scripts/check-deployed.sh <rpc url> <program id> <binary>
set -euo pipefail
export LC_ALL=C

url="$1"
program_id="$2"
binary="$3"

dump="$(mktemp)"
trap 'rm -f "$dump"' EXIT

solana program dump "$program_id" "$dump" --url "$url" >/dev/null

size="$(wc -c < "$binary" | tr -d ' ')"

if ! head -c "$size" "$dump" | cmp -s - "$binary"; then
  echo "The program at $program_id is not $binary" >&2
  exit 1
fi

if [ -n "$(tail -c +"$((size + 1))" "$dump" | tr -d '\000')" ]; then
  echo "The program at $program_id has data after $binary" >&2
  exit 1
fi

echo "The program at $program_id is $binary"
