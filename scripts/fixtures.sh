#!/usr/bin/env bash
# Refreshes the test fixtures of one cluster from its live state: Circle's CCTP V2 programs, the
# accounts deposit_for_burn reads, the cluster's active and inactive feature gates and Circle's IDL.
#
# Usage: scripts/fixtures.sh devnet|mainnet
set -euo pipefail

case "${1:-}" in
  devnet)
    url=devnet
    usdc=4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU
    ;;
  mainnet)
    url=mainnet-beta
    usdc=EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v
    ;;
  *)
    echo "usage: $0 devnet|mainnet" >&2
    exit 1
    ;;
esac

tmm=CCTPV2vPZJS2u2BBsUoscuikbYjnpFmbFsvVuJdgUMQe
mt=CCTPV2Sm4AdWt5296sk4P66VBZ7bEhcARwFaaS9YPbeC
program="$(cd "$(dirname "$0")/.." && pwd)/programs/mozaik-cctp-forwarder"
dir="$program/tests/fixtures/$1"
mkdir -p "$dir"

pda() {
  solana find-program-derived-address "$@" | awk '{print $1}'
}

# A dump is the whole on-chain program buffer, so it can end in zero padding after the ELF. The
# runtime ignores the padding, and so do the tests.
solana program dump -u "$url" "$tmm" "$dir/token_messenger_minter_v2.so"
solana program dump -u "$url" "$mt" "$dir/message_transmitter_v2.so"

accounts=(
  "token_messenger:$(pda "$tmm" string:token_messenger)"
  "token_minter:$(pda "$tmm" string:token_minter)"
  "remote_token_messenger_base:$(pda "$tmm" string:remote_token_messenger string:6)"
  "local_token_usdc:$(pda "$tmm" string:local_token "pubkey:$usdc")"
  "message_transmitter:$(pda "$mt" string:message_transmitter)"
  "usdc_mint:$usdc"
)

for entry in "${accounts[@]}"; do
  solana account -u "$url" "${entry#*:}" --output json -o "$dir/${entry%%:*}.json" >/dev/null
done

status="$(solana feature status -u "$url" --display-all --output json)"
jq '[.features[] | select(.status == "active") | .id] | sort' <<<"$status" > "$dir/features.json"
jq '[.features[] | select(.status != "active") | .id] | sort' <<<"$status" > "$dir/inactive_features.json"

# The CPI types come from Circle's IDL, which is the same on both clusters
if [ "$1" = mainnet ]; then
  NO_DNA=1 anchor legacy-idl fetch "$tmm" --provider.cluster mainnet \
    -o "$program/idls/token_messenger_minter_v2.json"
fi

echo "Wrote the $1 fixtures to $dir"
