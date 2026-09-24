#!/usr/bin/env bash
# Turns the contract admin account into an M-of-N multisig account.
# Usage: scripts/setup-multisig-admin.sh <admin-identity> <network> <threshold> <signer-address>...
#
# Every listed signer gets weight 1, the admin account's own master key is
# reduced to weight 0, and all three thresholds are set to <threshold>. Run
# this before calling `initialize` so the multisig account is the admin.
set -euo pipefail

ADMIN="${1:?admin identity required}"
NETWORK="${2:?network required (testnet|futurenet|mainnet)}"
THRESHOLD="${3:?threshold required}"
shift 3

if [ "$#" -lt 2 ]; then
  echo "at least two signer addresses are required for a multisig admin" >&2
  exit 1
fi
if [ "$THRESHOLD" -lt 2 ] || [ "$THRESHOLD" -gt "$#" ]; then
  echo "threshold must be between 2 and the number of signers ($#)" >&2
  exit 1
fi

SIGNER_ARGS=()
for signer in "$@"; do
  SIGNER_ARGS+=(--signer "$signer" --signer-weight 1)
done

# Signers are added first while the master key still has weight, then the
# master key is dropped and the thresholds are raised in a second transaction.
stellar tx new set-options \
  --source-account "$ADMIN" \
  --network "$NETWORK" \
  "${SIGNER_ARGS[@]}"

stellar tx new set-options \
  --source-account "$ADMIN" \
  --network "$NETWORK" \
  --master-weight 0 \
  --low-threshold "$THRESHOLD" \
  --med-threshold "$THRESHOLD" \
  --high-threshold "$THRESHOLD"
