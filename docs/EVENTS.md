# On-chain events

The contract publishes two Soroban events, defined with
`#[contractevent]` in `lib.rs`:

## `TicketIssued`

Emitted by `issue_ticket` and `purchase_primary`.

| Field | Type | Notes |
|---|---|---|
| `ticket_id` | `u64` | topic |
| `event_id` | `u64` | |

## `TicketCheckedIn`

Emitted by `check_in`.

| Field | Type | Notes |
|---|---|---|
| `ticket_id` | `u64` | topic |
| `organizer` | `Address` | |

Indexers and the backend's reconciliation job can subscribe to these
instead of polling `get_ticket` for every ticket on every block.

## Admin events

| Event | Emitted by | Fields |
|---|---|---|
| `ContractInitialized` | `initialize` | `admin` (topic), `payment_token` |
| `PurchaseThrottleUpdated` | `set_purchase_throttle` | `admin` (topic), `min_ledger_spacing` |
| `PaymentTokenProposed` | `propose_payment_token` | `admin` (topic), `new_token`, `apply_after_ledger` |
| `PaymentTokenChanged` | `apply_payment_token` | `admin` (topic), `old_token`, `new_token` |
