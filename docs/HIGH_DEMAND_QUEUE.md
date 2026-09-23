# High-Demand Sale Queue via Commit-Reveal (Design)

Status: design proposal for issue #240. Not yet implemented.

## Context

`purchase_primary` is first-come-first-served: whichever transaction
lands first in a ledger wins. For high-demand events this favors bots
that can submit transactions faster and with tighter fee bidding than
real buyers. [#239](./ARCHITECTURE.md) (ledger-spacing throttle) slows
down *repeat* purchases by the same address but does nothing for the
initial land-grab across many addresses.

## Approach: commit-reveal over a lottery

A lottery (pick N random winners from all entrants) requires an
on-chain randomness source. Soroban has no native secure RNG, and
ledger-derived pseudo-randomness (hash of ledger sequence/close time) is
predictable enough for a sufficiently motivated bot to game, since the
attacker chooses when to submit within a window. Commit-reveal avoids
needing randomness at all: fairness comes from every entrant being
locked into a commitment before anyone can see who else entered or in
what order, which removes the incentive to race. That is the
recommended design.

## Flow

1. **Commit window** (`event.queue_open_ledger` .. `event.queue_commit_deadline_ledger`)
   Buyers call `commit_entry(env, buyer: Address, event_id: u64, commitment: BytesN<32>)`.
   `commitment = sha256(buyer_address || nonce || tier)` computed off-chain
   by the buyer's wallet/app. The contract stores
   `DataKey::QueueEntry(event_id, buyer)` = `commitment` and nothing else
   — at this point no one, including the organizer, can see what tier a
   given commitment is for. One commitment per buyer per event
   (re-committing overwrites, so a buyer can't flood the queue).

2. **Reveal window** (`queue_commit_deadline_ledger` .. `queue_reveal_deadline_ledger`)
   Buyers call `reveal_entry(env, buyer, event_id, nonce, tier)`. Contract
   recomputes the hash and checks it matches the stored commitment, then
   appends `buyer` to an ordered `DataKey::QueueRevealed(event_id)` list
   (a `Vec<Address>` in persistent storage, capped at the tier's
   remaining supply — see Scaling note below). Order within the reveal
   window is by ledger sequence then transaction order, same as today,
   but since nobody could act on knowledge of other entrants during the
   commit window, being fast during *reveal* buys nothing an attacker
   can plan for in advance.

3. **Purchase window** (after `queue_reveal_deadline_ledger`)
   Only addresses present in `QueueRevealed(event_id)` may call
   `purchase_primary` for that event, for a limited number of ledgers
   (e.g. equal to queue size, so late revealers near the cap get a fair
   shot before it's opened to the general purchase flow). Addresses
   that committed but never revealed forfeit their spot — no refund is
   owed since no funds move until `purchase_primary`.

## New state

```rust
pub struct QueueConfig {
    pub commit_deadline_ledger: u32,
    pub reveal_deadline_ledger: u32,
    pub max_entries: u32,   // caps storage/iteration cost
}
```

`Event` gains `pub queue: Option<QueueConfig>` (organizer opts in at
`create_event` time, defaulting to `None` so existing events are
unaffected).

`DataKey::QueueCommitment(event_id, Address)` → `BytesN<32>`
`DataKey::QueueRevealed(event_id)` → `Vec<Address>`, insertion-ordered

## New errors

`QueueNotOpen`, `QueueCommitWindowClosed`, `QueueRevealWindowClosed`,
`QueueRevealWindowNotOver`, `CommitmentMismatch`, `NoCommitmentFound`,
`AlreadyRevealed`, `NotInQueue`, `QueueFull`.

## purchase_primary interaction

When `event.queue` is `Some`, `purchase_primary` gains one extra check:
before the existing throttle/escrow logic, require
`env.ledger().sequence() > queue.reveal_deadline_ledger` and that
`buyer` is present in `QueueRevealed(event_id)`.

## Scaling note

A `Vec<Address>` scanned linearly is fine for hundreds of entrants; for
very large drops (thousands+), swap the membership check to a
`DataKey::QueueRevealed(event_id, Address) -> ()` presence key instead of
a `Vec`, trading "get the ordered list" for O(1) membership checks —
recommended if `max_entries` exceeds roughly 500, since Soroban persistent
`Vec` reads/writes cost scales with the full entry, not just the delta.

## Out of scope for the first implementation

- Per-tier queues (this design queues the whole event; splitting by tier
  is a straightforward follow-up: key everything by
  `(event_id, tier_hash)` instead of `event_id`).
- On-chain nonce/commitment generation helpers — left to client SDKs.
- Automatic refund/penalty for no-show revealers — there's nothing to
  refund since no funds move before `purchase_primary`.

## Acceptance criteria mapping

"Design commit-reveal or lottery" → commit-reveal chosen over lottery
because it needs no on-chain randomness source and Soroban has none
that resists a motivated attacker; the three-phase flow, state, errors,
and `purchase_primary` gating above are the design. "Design doc" is the
full deliverable for this issue; implementation is a follow-up PR.
