# Ticket Merge/Split Design for Group Tickets

## Issue #250: Add ticket merge/split for group tickets

### Overview
This document outlines the design for supporting ticket merge and split operations for group tickets in the StellarTickets blockchain contract.

### Problem Statement
Currently, the ticketing system treats each ticket as an atomic unit. Group tickets (e.g., 4-pack venue seating, team event bundles) cannot be efficiently represented or managed without creating individual tickets for each seat, which leads to:
- Inefficient storage and state management
- Difficulty tracking group cohesion
- Limited resale scenarios for partial group transfers

### Design Goals
1. Enable efficient representation of group tickets
2. Support splitting a group ticket into individual tickets
3. Support merging individual tickets back into group tickets
4. Maintain backward compatibility with existing single-ticket flows
5. Preserve pricing logic and anti-scalping constraints

### Proposed Solution

#### Data Structures

**Option 1: Explicit Group Ticket Type (Recommended)**
- Extend the `Ticket` struct to include an optional `group_id` field
- Create a `GroupTicket` contract type to track group composition
- Group tickets would consist of multiple "child" tickets

**Option 2: Implicit Grouping via Seat Ranges**
- Use seat numbering conventions to identify groups (e.g., "A1-A4" = 4-pack)
- Simpler implementation but less flexible and harder to validate

#### Core Operations

**1. Split Group Ticket**
```
split_group_ticket(
    owner: Address,
    group_ticket_id: u64
) -> Result<Vec<u64>, Error>
```
- Converts a single group ticket into individual child tickets
- Each child ticket gets proportional pricing (original_price / group_size)
- Requires explicit group ticket type
- Preconditions:
  - Caller is the group ticket owner
  - Ticket is not Used or Revoked
  - Ticket is not for Resale (to prevent split-then-resell arbitrage)

**2. Merge Individual Tickets**
```
merge_tickets(
    owner: Address,
    ticket_ids: Vec<u64>,
    new_tier: String,
    group_name: String
) -> Result<u64, Error>
```
- Creates a new group ticket from individual tickets
- All tickets must be from the same event
- Pricing: sum of all individual original_prices
- Preconditions:
  - Caller is owner of all tickets
  - All tickets are from same event
  - All tickets are Valid status
  - At least 2 tickets to merge
  - Batch size limited by MAX_BATCH_SIZE

#### Validation Rules

1. **Merge Constraints**
   - All tickets must belong to the same event
   - All tickets must have same status (Valid)
   - Cannot merge tickets already in a group
   - Minimum 2 tickets per merge

2. **Split Constraints**
   - Group ticket must have group_size >= 2
   - Cannot split non-group tickets
   - Split tickets inherit original event_id

3. **Resale Integrity**
   - Cannot split/merge tickets listed for resale
   - Max resale multiplier applies to merged group total
   - Resale price tracking maintained across operations

#### Storage Impact
- New `GroupTicket` data key type for group ticket metadata
- New `TicketGroup(u64)` data key for group membership tracking
- TTL management aligned with existing ticket entries

### Implementation Recommendation

Given complexity constraints, recommend **phased approach**:

1. **Phase 1 (Current)**: Document design without implementation
   - This establishes the contract and prevents Ad-Hoc solutions
   - Allows off-chain grouping via seat conventions
   
2. **Phase 2** (Future release if demand exists):
   - Implement split_group_ticket for known group formats
   - Merge deferred until clear use cases emerge

3. **Phase 3** (Optional):
   - Full bidirectional merge/split support
   - Advanced group management APIs

### Not Planned Rationale

This can remain not-planned (design-only) for now because:
1. **Alternative Exists**: Organizers can issue logically-linked tickets with related seat numbers
2. **Complexity vs. Value**: Full implementation requires significant storage and validation logic
3. **Migration Risk**: Adding group support later requires data migration strategy
4. **Resale Complexity**: Coordinating resale pricing across group splits is non-trivial
5. **Market Viability**: Single-ticket model is sufficient for MVP; demand may shift requirements

### Future Considerations

If merge/split is implemented, should also support:
- Partial group splits (e.g., merge 2 of 4 tickets)
- Group gift transfers (send entire group as one claim)
- Group refund semantics
- Analytics on group ticket flow

### Approval Status
- [ ] Requires team discussion for Phase 2+ commitment
- [x] Design documented for reference
