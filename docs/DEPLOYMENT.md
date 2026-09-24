# Deployment checklist

1. `cargo test --workspace` — full suite green
2. `cargo clippy --all-targets -- -D warnings` — no lint warnings
3. `stellar contract build` — optimized wasm builds cleanly
4. Deploy to **testnet** first; run through the full ticket lifecycle
   manually (create event, issue, verify, check in, list, buy resale)
   against a real testnet SEP-41 token
5. Only after a testnet soak period, deploy to **mainnet** with a
   production payment token and a hardware-backed admin key
6. Record the deployed contract ID in the backend's
   `TICKETING_CONTRACT_ID` environment variable

## Multisig admin

On mainnet the contract admin should be a multisig account rather than a
single key. Configure it before calling `initialize`:

```
scripts/setup-multisig-admin.sh <admin-identity> <network> <threshold> <signer-address>...
```

Run it on testnet first and confirm an admin call (for example
`set_purchase_throttle`) needs `<threshold>` signatures before doing the same
on mainnet.
