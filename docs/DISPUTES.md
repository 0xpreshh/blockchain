# Dispute and Chargeback Hook (Design)

Status: design proposal for issue #238. Not yet implemented.

## Context

The contract has no dispute mechanism. `revoke_ticket` lets an organizer
unilaterally void a ticket, but there is no neutral party to resolve
disagreements between buyer, seller, and organizer (e.g. a resale buyer
claims a ticket was never delivered, or a card payment behind an escrowed
sale is charged back).

## Goals

- Give disputed funds (primarily escrowed primary-sale proceeds, see
  [ESCROW](./ARCHITECTURE.md)) a neutral resolution path instead of an
  all-or-nothing release to the organizer.
- Keep the arbiter's power bounded: it can only rule on a specific,
  opened dispute, never move funds outside of one.
- No change to the happy path — disputes are opt-in per event.

## Design

### New role: Arbiter

- `DataKey::Arbiter` — a single global address, set by the contract admin
  via `set_arbiter(env, admin, arbiter)`. Starts unset; while unset,
  disputes cannot be opened (fails closed).
- Rationale for a single global arbiter over a per-event arbiter: matches
  the platform's existing single-admin model and keeps the design small.
  A per-event arbiter (organizer-nominated) is a natural v2 extension —
  swap `DataKey::Arbiter` for `DataKey::EventArbiter(u64)` and let
  `create_event` accept an optional arbiter address.

### New state

```rust
pub enum DisputeStatus {
    Open,
    ResolvedBuyer,   // funds returned to buyer
    ResolvedSeller,  // funds released to seller/organizer
}

pub struct Dispute {
    pub ticket_id: u64,
    pub opener: Address,
    pub amount: i128,       // funds held pending resolution
    pub status: DisputeStatus,
}
```

`DataKey::Dispute(ticket_id)` — at most one open dispute per ticket at a
time.

### New errors

`DisputeAlreadyOpen`, `DisputeNotFound`, `NotArbiter`, `NoArbiterSet`,
`DisputeAlreadyResolved`.

### New entry points

- `open_dispute(env, opener: Address, ticket_id: u64, amount: i128) -> Result<(), Error>`
  - `opener` must be the ticket's current owner (buyer-initiated) or the
    ticket's event organizer (organizer-initiated chargeback response).
  - Requires `opener.require_auth()`.
  - Moves `amount` out of the relevant escrow/resale balance into the
    `Dispute` record so it can't be released or spent while a dispute is
    open. For an escrowed primary sale this means reducing
    `Event.escrow_balance` by `amount` before recording the dispute — the
    tokens themselves stay in the contract, only the bookkeeping changes.
  - Fails if a dispute is already open for the ticket, or if `amount`
    exceeds the disputable balance.

- `resolve_dispute(env, arbiter: Address, ticket_id: u64, favor_buyer: bool) -> Result<(), Error>`
  - Requires `arbiter.require_auth()` and `arbiter == DataKey::Arbiter`.
  - Transfers the held `amount` to the buyer (refund) or to the
    organizer/seller (release), sets `status` accordingly, and leaves the
    resolved `Dispute` record in storage as an audit trail (not deleted —
    disputes are rare enough that TTL extension cost is negligible and an
    auditable history matters more than storage savings).

- `set_arbiter(env, admin: Address, arbiter: Address) -> Result<(), Error>`
  - Admin-only, mirrors the pattern in `set_purchase_throttle`.

### Interaction with escrow release

`release_escrow` must reject if the event has any ticket with an
`Open` dispute funded from that event's escrow — otherwise an organizer
could race a dispute by releasing the full balance first. Simplest
enforcement: track a per-event `disputed_balance: i128` counter
(incremented on `open_dispute`, decremented on `resolve_dispute`) and
have `release_escrow` transfer only `escrow_balance - disputed_balance`.

### Out of scope for the first implementation

- On-chain evidence/discussion (handled off-chain by the platform; the
  contract only stores the ruling).
- Multi-arbiter voting/quorum — a single trusted arbiter address is the
  simplest structure that satisfies the acceptance criteria and matches
  the existing single-admin trust model; can be swapped for an N-of-M
  scheme later without changing the external interface of
  `resolve_dispute`.
- Auto-resolution timeouts (e.g. resolve in buyer's favor if the arbiter
  doesn't act within N ledgers) — worth adding once real dispute volume
  shows it's needed.

## Acceptance criteria mapping

- "Design an arbiter role" → `Arbiter` global role + `set_arbiter`,
  `resolve_dispute` above.
- "Design doc then implementation" → this document; implementation is a
  follow-up PR once the design is agreed, adding `Dispute`,
  `DisputeStatus`, the four new errors, and the three new entry points
  described here, plus tests mirroring the existing `escrow_*` and
  `purchase_throttle_*` test style in `contracts/ticketing/src/test.rs`.
