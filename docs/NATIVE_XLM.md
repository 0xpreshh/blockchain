# Native XLM payments

The ticketing contract accepts any SEP-41 token as its payment token —
including the **native XLM asset contract** that Stellar deploys for lumens.

## Why the native asset contract

XLM is not an issued asset, but Soroban still exposes it through a built-in
Stellar Asset Contract (SAC) deployed at a network-deterministic address. That
contract implements the same SEP-41 interface as issued-token SACs
(`transfer`, `balance`, `decimals`, …), so the ticketing contract can use it
as its payment token with no special-casing:

- `initialize(admin, payment_token)` accepts the native SAC address directly.
  The token probe (`decimals()`) succeeds — the native SAC reports **7
  decimals**.
- `purchase_primary`, `buy_resale`, and (with escrow enabled)
  `release_escrow` all move real lumens through the native asset contract.
- `token_decimals()` returns `7`, so clients format amounts as
  `amount / 10^7` XLM.

Because the underlying balance lives in the account's XLM balance, no minting
is needed: buyers pay from their wallet's lumens, and the organizer receives
lumens directly.

## Deploying / configuring

When running against a real network, use the SAC address for the native asset
of that network (deterministically derived from the asset preimage — the same
address every Stellar network uses for XLM's SAC). Pass it as `payment_token`
at `initialize`, or move to it later through the two-step
`propose_payment_token` / `apply_payment_token` timelocked change.

## Testing with the native asset contract

`contracts/ticketing/src/test.rs` contains a reproducible test-environment
setup:

- `register_native_asset_contract(&env)` registers the built-in SAC for
  `Asset::Native` by invoking the `CreateContract` host function with the
  asset preimage — the same mechanism `Env::register_stellar_asset_contract_v2`
  uses for issued assets.
- `create_funded_xlm_account(&env, key, balance)` seeds an account ledger
  entry with lumens so native transfers have a balance to draw from.

Covered flows (`contracts/ticketing/src/test.rs`):

1. `native_xlm_sac_is_accepted_as_payment_token` — initialize with the native
   SAC succeeds and reports 7 decimals.
2. `native_xlm_primary_sale_moves_xlm_and_mints_the_ticket` — the buyer pays
   200 XLM, the organizer receives it, the ticket is minted.
3. `native_xlm_resale_settles_atomically` — resale settles royalty +
   seller proceeds atomically in lumens.
