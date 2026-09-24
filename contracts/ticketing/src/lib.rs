#![no_std]
#![allow(clippy::too_many_arguments)]

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, token, Address, Bytes,
    BytesN, Env, String, Vec,
};

#[contractevent]
#[derive(Clone, Debug)]
pub struct TicketIssued {
    #[topic]
    pub ticket_id: u64,
    pub event_id: u64,
}

#[contractevent]
#[derive(Clone, Debug)]
pub struct ContractInitialized {
    #[topic]
    pub admin: Address,
    pub payment_token: Address,
}

#[contractevent]
#[derive(Clone, Debug)]
pub struct PurchaseThrottleUpdated {
    #[topic]
    pub admin: Address,
    pub min_ledger_spacing: u32,
}

#[contractevent]
#[derive(Clone, Debug)]
pub struct PaymentTokenProposed {
    #[topic]
    pub admin: Address,
    pub new_token: Address,
    pub apply_after_ledger: u32,
}

#[contractevent]
#[derive(Clone, Debug)]
pub struct PaymentTokenChanged {
    #[topic]
    pub admin: Address,
    pub old_token: Address,
    pub new_token: Address,
}

#[contractevent]
#[derive(Clone, Debug)]
pub struct TicketCheckedIn {
    #[topic]
    pub ticket_id: u64,
    pub organizer: Address,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TicketStatus {
    Valid,
    Used,
    Revoked,
    Resale,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Event {
    pub organizer: Address,
    pub name: String,
    /// Category such as "concert", "flight", "sports", "conference", etc.
    /// Kept as free text metadata rather than a fixed enum so new industries
    /// don't require a contract migration.
    pub category: String,
    /// Basis points cap on resale price relative to original sale price
    /// (e.g. 12000 = 120%). Anti-scalping enforcement.
    pub max_resale_multiplier_bps: u32,
    /// Basis points of every resale price paid to the organizer as royalty.
    pub royalty_bps: u32,
    pub tickets_issued: u64,
    pub starts_at: u64,
    pub transfer_freeze_seconds: u64,
    pub resale_cutoff_seconds: u64,
    /// When true, primary sale proceeds are held by the contract instead of
    /// paid to the organizer immediately, and can only be released once the
    /// ledger sequence reaches `escrow_release_ledger`.
    pub escrow_enabled: bool,
    /// Ledger sequence after which escrowed proceeds may be released.
    /// Ignored when `escrow_enabled` is false.
    pub escrow_release_ledger: u32,
    /// Primary sale proceeds currently held in escrow for this event.
    pub escrow_balance: i128,
    /// Per-event accepted payment token (issue #235). `None` means the
    /// event settles in the contract-wide payment token set at
    /// initialization. Lockable only while no tickets have been issued so
    /// existing sales stay denominated in the token they were paid in.
    pub payment_token: Option<Address>,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Ticket {
    pub event_id: u64,
    pub owner: Address,
    pub tier: String,
    pub seat: String,
    pub status: TicketStatus,
    pub original_price: i128,
    pub resale_price: i128,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct GiftClaim {
    pub from: Address,
    pub secret_hash: BytesN<32>,
    pub expires_at: u64,
}

/// A payment token change that has been proposed but not yet applied.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingPaymentToken {
    pub token: Address,
    pub apply_after_ledger: u32,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    PaymentToken,
    TokenDecimals,
    PendingPaymentToken,
    Event(u64),
    Ticket(u64),
    GiftClaim(u64),
    NextTicketId,
    LastPurchaseLedger(Address),
    MinPurchaseSpacing,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    EventNotFound = 3,
    EventAlreadyExists = 4,
    TicketNotFound = 5,
    NotOrganizer = 6,
    NotOwner = 7,
    AlreadyUsed = 8,
    Revoked = 9,
    NotForResale = 10,
    ResalePriceExceedsCap = 11,
    InvalidPrice = 12,
    InvalidRoyalty = 13,
    EscrowNotEnabled = 14,
    EventNotEnded = 15,
    PurchaseTooSoon = 16,
    NotAdmin = 17,
    EventAlreadyStarted = 18,
    InvalidEventTime = 19,
    TransfersFrozen = 20,
    ResaleClosed = 21,
    InvalidLottery = 22,
    GiftClaimNotFound = 23,
    GiftClaimExpired = 24,
    InvalidSecret = 25,
    InvalidExpiry = 26,
    EmptyBatch = 27,
    BatchTooLarge = 28,
    InvalidPaymentToken = 29,
    NoPendingPaymentToken = 30,
    TimelockNotElapsed = 31,
    TicketsAlreadyIssued = 32,
}

pub const MAX_BATCH_SIZE: u32 = 50;
/// Ledgers that must pass between proposing and applying a payment token
/// change (~1 day at 5s/ledger).
pub const PAYMENT_TOKEN_CHANGE_DELAY_LEDGERS: u32 = 17_280;
const LEDGER_BUMP: u32 = 535_679; // ~31 days at 5s/ledger, matches other Soroban tooling defaults
const LEDGER_THRESHOLD: u32 = 500_000;

#[contract]
pub struct TicketingContract;

#[contractimpl]
impl TicketingContract {
    /// One-time setup. `payment_token` is the Stellar Asset Contract (or any
    /// SEP-41 token) used for on-chain primary sales and resale settlement.
    pub fn initialize(env: Env, admin: Address, payment_token: Address) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();
        // The probe validates that `payment_token` is a real token contract
        // and captures its decimals for client display (issue #233).
        let decimals = Self::ensure_token_contract(&env, &payment_token)?;
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::PaymentToken, &payment_token);
        env.storage().instance().set(&DataKey::TokenDecimals, &decimals);
        env.storage().instance().set(&DataKey::NextTicketId, &0u64);
        env.storage()
            .instance()
            .extend_ttl(LEDGER_THRESHOLD, LEDGER_BUMP);
        ContractInitialized {
            admin,
            payment_token,
        }
        .publish(&env);
        Ok(())
    }

    /// Step one of a payment token change: the admin proposes a new token,
    /// which can only be applied after `PAYMENT_TOKEN_CHANGE_DELAY_LEDGERS`.
    /// A new proposal replaces any pending one. Proceeds already held in
    /// escrow stay denominated in the old token, so release them first.
    pub fn propose_payment_token(
        env: Env,
        admin: Address,
        new_token: Address,
    ) -> Result<(), Error> {
        Self::require_admin(&env, &admin)?;
        // The probe validates that the proposed token is a real token
        // contract. Its decimals are captured when the change is APPLIED so
        // clients always see decimals matching the active payment token
        // (issue #233).
        Self::ensure_token_contract(&env, &new_token)?;
        let apply_after_ledger = env
            .ledger()
            .sequence()
            .saturating_add(PAYMENT_TOKEN_CHANGE_DELAY_LEDGERS);
        env.storage().instance().set(
            &DataKey::PendingPaymentToken,
            &PendingPaymentToken {
                token: new_token.clone(),
                apply_after_ledger,
            },
        );
        PaymentTokenProposed {
            admin,
            new_token,
            apply_after_ledger,
        }
        .publish(&env);
        Ok(())
    }

    /// Step two: once the delay has elapsed, the admin applies the pending
    /// payment token.
    pub fn apply_payment_token(env: Env, admin: Address) -> Result<(), Error> {
        Self::require_admin(&env, &admin)?;
        let pending: PendingPaymentToken = env
            .storage()
            .instance()
            .get(&DataKey::PendingPaymentToken)
            .ok_or(Error::NoPendingPaymentToken)?;
        if env.ledger().sequence() < pending.apply_after_ledger {
            return Err(Error::TimelockNotElapsed);
        }
        let old_token = Self::payment_token(&env)?;
        // Capture the decimals of the token becoming active so clients always
        // see decimals matching the active payment token (issue #233).
        let decimals = Self::ensure_token_contract(&env, &pending.token)?;
        env.storage()
            .instance()
            .set(&DataKey::PaymentToken, &pending.token);
        env.storage().instance().set(&DataKey::TokenDecimals, &decimals);
        env.storage().instance().remove(&DataKey::PendingPaymentToken);
        PaymentTokenChanged {
            admin,
            old_token,
            new_token: pending.token,
        }
        .publish(&env);
        Ok(())
    }

    /// Registers a new event/route/showing under an organizer. `event_id` is
    /// chosen by the caller's backend (e.g. a ULID cast to u64) so it can be
    /// correlated with the off-chain event record.
    pub fn create_event(
        env: Env,
        organizer: Address,
        event_id: u64,
        name: String,
        category: String,
        max_resale_multiplier_bps: u32,
        royalty_bps: u32,
        starts_at: u64,
        transfer_freeze_seconds: u64,
        resale_cutoff_seconds: u64,
    ) -> Result<(), Error> {
        organizer.require_auth();
        if royalty_bps > 10_000 {
            return Err(Error::InvalidRoyalty);
        }
        if starts_at <= env.ledger().timestamp() {
            return Err(Error::InvalidEventTime);
        }
        let key = DataKey::Event(event_id);
        if env.storage().persistent().has(&key) {
            return Err(Error::EventAlreadyExists);
        }
        let event = Event {
            organizer,
            name,
            category,
            max_resale_multiplier_bps,
            royalty_bps,
            tickets_issued: 0,
            starts_at,
            transfer_freeze_seconds,
            resale_cutoff_seconds,
            escrow_enabled: false,
            escrow_release_ledger: 0,
            escrow_balance: 0,
            payment_token: None,
        };
        env.storage().persistent().set(&key, &event);
        env.storage()
            .persistent()
            .extend_ttl(&key, LEDGER_THRESHOLD, LEDGER_BUMP);
        Ok(())
    }

    /// Randomly allocates complimentary/reserved tickets across a supplied
    /// entrant set. The organizer controls the entrant list; winner selection
    /// is performed on-chain using Soroban's PRNG.
    pub fn allocate_lottery(
        env: Env,
        organizer: Address,
        event_id: u64,
        mut entrants: Vec<Address>,
        winner_count: u32,
        tier: String,
        price: i128,
    ) -> Result<Vec<u64>, Error> {
        organizer.require_auth();
        if price < 0 || winner_count == 0 || winner_count > entrants.len() {
            return Err(Error::InvalidLottery);
        }
        for i in 0..entrants.len() {
            for j in (i + 1)..entrants.len() {
                if entrants.get(i) == entrants.get(j) {
                    return Err(Error::InvalidLottery);
                }
            }
        }
        let mut event = Self::get_event(&env, event_id)?;
        if event.organizer != organizer {
            return Err(Error::NotOrganizer);
        }

        env.prng().shuffle(&mut entrants);
        let mut ticket_ids = Vec::new(&env);
        for i in 0..winner_count {
            let winner = entrants.get(i).unwrap();
            let ticket_id = Self::mint(
                &env,
                event_id,
                winner,
                tier.clone(),
                String::from_str(&env, "unassigned"),
                price,
            );
            ticket_ids.push_back(ticket_id);
        }

        event.tickets_issued += winner_count as u64;
        env.storage()
            .persistent()
            .set(&DataKey::Event(event_id), &event);
        Ok(ticket_ids)
    }

    /// Opts an event into escrow: primary sale proceeds are held by the
    /// contract instead of paid to the organizer immediately, and can only
    /// be released via `release_escrow` once the ledger sequence reaches
    /// `escrow_release_ledger` (e.g. the event's end). Must be called
    /// before any tickets are sold, since it would otherwise change the
    /// settlement terms for purchases already made.
    pub fn enable_escrow(
        env: Env,
        organizer: Address,
        event_id: u64,
        escrow_release_ledger: u32,
    ) -> Result<(), Error> {
        organizer.require_auth();
        let mut event = Self::get_event(&env, event_id)?;
        if event.organizer != organizer {
            return Err(Error::NotOrganizer);
        }
        if event.tickets_issued > 0 {
            return Err(Error::EventAlreadyStarted);
        }
        event.escrow_enabled = true;
        event.escrow_release_ledger = escrow_release_ledger;
        env.storage()
            .persistent()
            .set(&DataKey::Event(event_id), &event);
        Ok(())
    }

    /// Sets the event's accepted payment token (issue #235). When set, the
    /// event's primary sales, resale settlement, escrow and refunds are all
    /// denominated in this token instead of the contract-wide payment token.
    /// Pass `None` to fall back to the contract-wide token. Only callable by
    /// the organizer while no tickets have been issued — existing sales must
    /// stay denominated in the token they were paid in.
    pub fn set_event_payment_token(
        env: Env,
        organizer: Address,
        event_id: u64,
        token: Option<Address>,
    ) -> Result<(), Error> {
        organizer.require_auth();
        let mut event = Self::get_event(&env, event_id)?;
        if event.organizer != organizer {
            return Err(Error::NotOrganizer);
        }
        if event.tickets_issued > 0 {
            return Err(Error::TicketsAlreadyIssued);
        }
        if let Some(token) = &token {
            Self::ensure_token_contract(&env, token)?;
        }
        event.payment_token = token;
        env.storage()
            .persistent()
            .set(&DataKey::Event(event_id), &event);
        Ok(())
    }

    /// Returns the event's accepted payment token: the per-event override
    /// when one is set, otherwise the contract-wide payment token
    /// (issue #235).
    pub fn event_payment_token(env: Env, event_id: u64) -> Result<Address, Error> {
        let event = Self::get_event(&env, event_id)?;
        Self::payment_token_for_event(&env, &event)
    }

    /// Organizer-authorized issuance for tickets already paid for off-chain
    /// (card payment, comp, or fiat-to-crypto settled by the platform).
    pub fn issue_ticket(
        env: Env,
        organizer: Address,
        event_id: u64,
        to: Address,
        tier: String,
        seat: String,
        price: i128,
    ) -> Result<u64, Error> {
        organizer.require_auth();
        if price < 0 {
            return Err(Error::InvalidPrice);
        }
        let mut event = Self::get_event(&env, event_id)?;
        if event.organizer != organizer {
            return Err(Error::NotOrganizer);
        }
        let ticket_id = Self::mint(&env, event_id, to, tier, seat, price);
        event.tickets_issued += 1;
        env.storage()
            .persistent()
            .set(&DataKey::Event(event_id), &event);
        Ok(ticket_id)
    }

    /// Fully on-chain primary sale: buyer pays the organizer directly in
    /// `payment_token`, then the ticket is minted to the buyer atomically.
    pub fn purchase_primary(
        env: Env,
        buyer: Address,
        event_id: u64,
        tier: String,
        seat: String,
        price: i128,
    ) -> Result<u64, Error> {
        buyer.require_auth();
        if price < 0 {
            return Err(Error::InvalidPrice);
        }
        Self::enforce_purchase_throttle(&env, &buyer)?;
        let mut event = Self::get_event(&env, event_id)?;
        let token_client = token::Client::new(&env, &Self::payment_token_for_event(&env, &event)?);
        if price > 0 {
            if event.escrow_enabled {
                token_client.transfer(&buyer, env.current_contract_address(), &price);
                event.escrow_balance += price;
            } else {
                token_client.transfer(&buyer, &event.organizer, &price);
            }
        }
        let ticket_id = Self::mint(&env, event_id, buyer.clone(), tier, seat, price);
        event.tickets_issued += 1;
        env.storage()
            .persistent()
            .set(&DataKey::Event(event_id), &event);
        Self::record_purchase(&env, &buyer);
        Ok(ticket_id)
    }

    /// Releases an event's escrowed primary sale proceeds to the organizer.
    /// Only callable by the organizer, and only once the current ledger
    /// sequence has reached `escrow_release_ledger` (i.e. the event has
    /// ended).
    pub fn release_escrow(env: Env, organizer: Address, event_id: u64) -> Result<(), Error> {
        organizer.require_auth();
        let mut event = Self::get_event(&env, event_id)?;
        if event.organizer != organizer {
            return Err(Error::NotOrganizer);
        }
        if !event.escrow_enabled {
            return Err(Error::EscrowNotEnabled);
        }
        if env.ledger().sequence() < event.escrow_release_ledger {
            return Err(Error::EventNotEnded);
        }
        let amount = event.escrow_balance;
        event.escrow_balance = 0;
        env.storage()
            .persistent()
            .set(&DataKey::Event(event_id), &event);
        if amount > 0 {
            let token_client =
                token::Client::new(&env, &Self::payment_token_for_event(&env, &event)?);
            token_client.transfer(&env.current_contract_address(), &organizer, &amount);
        }
        Ok(())
    }

    /// Admin-configurable minimum number of ledgers a buyer must wait
    /// between primary purchases. Set to 0 to disable the throttle.
    pub fn set_purchase_throttle(
        env: Env,
        admin: Address,
        min_ledger_spacing: u32,
    ) -> Result<(), Error> {
        Self::require_admin(&env, &admin)?;
        env.storage()
            .instance()
            .set(&DataKey::MinPurchaseSpacing, &min_ledger_spacing);
        PurchaseThrottleUpdated {
            admin,
            min_ledger_spacing,
        }
        .publish(&env);
        Ok(())
    }

    /// Direct, non-marketplace transfer (gift, family member, etc).
    pub fn transfer_ticket(
        env: Env,
        from: Address,
        ticket_id: u64,
        to: Address,
    ) -> Result<(), Error> {
        from.require_auth();
        let mut ticket = Self::get_ticket(&env, ticket_id)?;
        if ticket.owner != from {
            return Err(Error::NotOwner);
        }
        match ticket.status {
            TicketStatus::Used => return Err(Error::AlreadyUsed),
            TicketStatus::Revoked => return Err(Error::Revoked),
            _ => {}
        }
        let event = Self::get_event(&env, ticket.event_id)?;
        if Self::transfer_frozen(&env, &event) {
            return Err(Error::TransfersFrozen);
        }
        ticket.owner = to;
        ticket.status = TicketStatus::Valid;
        ticket.resale_price = 0;
        Self::remove_gift_claim(&env, ticket_id);
        Self::save_ticket(&env, ticket_id, &ticket);
        Ok(())
    }

    /// Direct, non-marketplace batch transfer of tickets to a single recipient.
    /// Bounded by `MAX_BATCH_SIZE`.
    pub fn transfer_batch(
        env: Env,
        from: Address,
        ticket_ids: Vec<u64>,
        to: Address,
    ) -> Result<(), Error> {
        from.require_auth();
        if ticket_ids.is_empty() {
            return Err(Error::EmptyBatch);
        }
        if ticket_ids.len() > MAX_BATCH_SIZE {
            return Err(Error::BatchTooLarge);
        }
        for ticket_id in ticket_ids.iter() {
            let mut ticket = Self::get_ticket(&env, ticket_id)?;
            if ticket.owner != from {
                return Err(Error::NotOwner);
            }
            match ticket.status {
                TicketStatus::Used => return Err(Error::AlreadyUsed),
                TicketStatus::Revoked => return Err(Error::Revoked),
                _ => {}
            }
            let event = Self::get_event(&env, ticket.event_id)?;
            if Self::transfer_frozen(&env, &event) {
                return Err(Error::TransfersFrozen);
            }
            ticket.owner = to.clone();
            ticket.status = TicketStatus::Valid;
            ticket.resale_price = 0;
            Self::remove_gift_claim(&env, ticket_id);
            Self::save_ticket(&env, ticket_id, &ticket);
        }
        Ok(())
    }

    /// Upgrades a ticket to a higher tier by charging the difference in price.
    /// The owner must pay the difference between new_price and the ticket's original_price.
    /// The ticket status is reset to Valid after upgrade.
    pub fn upgrade_ticket(
        env: Env,
        owner: Address,
        ticket_id: u64,
        new_tier: String,
        new_price: i128,
    ) -> Result<(), Error> {
        owner.require_auth();
        let mut ticket = Self::get_ticket(&env, ticket_id)?;
        if ticket.owner != owner {
            return Err(Error::NotOwner);
        }
        if new_price <= 0 {
            return Err(Error::InvalidPrice);
        }
        if new_price < ticket.original_price {
            return Err(Error::InvalidPrice);
        }
        match ticket.status {
            TicketStatus::Used => return Err(Error::AlreadyUsed),
            TicketStatus::Revoked => return Err(Error::Revoked),
            _ => {}
        }

        let event = Self::get_event(&env, ticket.event_id)?;
        let organizer = event.organizer.clone();

        // Calculate the upgrade fee (difference between new and original price)
        let upgrade_fee = new_price.saturating_sub(ticket.original_price);

        if upgrade_fee > 0 {
            // Transfer the upgrade fee from owner to organizer
            let payment_token = Self::payment_token_for_event(&env, &event)?;
            let token_client = token::Client::new(&env, &payment_token);
            token_client.transfer(&owner, &organizer, &upgrade_fee);
        }

        // Update ticket with new tier and price
        ticket.tier = new_tier;
        ticket.original_price = new_price;
        ticket.status = TicketStatus::Valid;
        ticket.resale_price = 0;
        Self::remove_gift_claim(&env, ticket_id);
        Self::save_ticket(&env, ticket_id, &ticket);
        Ok(())
    }

    /// Creates a claim link without requiring the recipient address up front.
    /// The owner shares the preimage off-chain; only its SHA-256 digest is stored.
    pub fn create_gift_claim(
        env: Env,
        owner: Address,
        ticket_id: u64,
        secret_hash: BytesN<32>,
        expires_at: u64,
    ) -> Result<(), Error> {
        owner.require_auth();
        if expires_at <= env.ledger().timestamp() {
            return Err(Error::InvalidExpiry);
        }

        let mut ticket = Self::get_ticket(&env, ticket_id)?;
        if ticket.owner != owner {
            return Err(Error::NotOwner);
        }
        match ticket.status {
            TicketStatus::Used => return Err(Error::AlreadyUsed),
            TicketStatus::Revoked => return Err(Error::Revoked),
            _ => {}
        }
        if ticket.status == TicketStatus::Resale {
            ticket.status = TicketStatus::Valid;
            ticket.resale_price = 0;
            Self::save_ticket(&env, ticket_id, &ticket);
        }

        let claim = GiftClaim {
            from: owner,
            secret_hash,
            expires_at,
        };
        let key = DataKey::GiftClaim(ticket_id);
        env.storage().persistent().set(&key, &claim);
        env.storage()
            .persistent()
            .extend_ttl(&key, LEDGER_THRESHOLD, LEDGER_BUMP);
        Ok(())
    }

    /// Claims a gifted ticket by presenting the secret preimage before expiry.
    pub fn claim_gift(
        env: Env,
        recipient: Address,
        ticket_id: u64,
        secret: Bytes,
    ) -> Result<(), Error> {
        recipient.require_auth();
        let key = DataKey::GiftClaim(ticket_id);
        let claim: GiftClaim = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(Error::GiftClaimNotFound)?;

        if env.ledger().timestamp() >= claim.expires_at {
            return Err(Error::GiftClaimExpired);
        }
        if env.crypto().sha256(&secret).to_bytes() != claim.secret_hash {
            return Err(Error::InvalidSecret);
        }

        let mut ticket = Self::get_ticket(&env, ticket_id)?;
        if ticket.owner != claim.from {
            return Err(Error::NotOwner);
        }
        match ticket.status {
            TicketStatus::Used => return Err(Error::AlreadyUsed),
            TicketStatus::Revoked => return Err(Error::Revoked),
            _ => {}
        }

        let event = Self::get_event(&env, ticket.event_id)?;
        if Self::transfer_frozen(&env, &event) {
            return Err(Error::TransfersFrozen);
        }

        ticket.owner = recipient;
        ticket.status = TicketStatus::Valid;
        ticket.resale_price = 0;
        env.storage().persistent().remove(&key);
        Self::save_ticket(&env, ticket_id, &ticket);
        Ok(())
    }

    /// Read-only on-chain verification — the core fraud-prevention primitive.
    /// Any scanner/app can call this without authentication to confirm a
    /// ticket's current owner and status before admitting entry.
    /// Lightweight ownership check: returns the owner of a ticket without
    /// full ticket details. Optimized for scanners and ownership verification.
    pub fn owner_of(env: Env, ticket_id: u64) -> Result<Address, Error> {
        let ticket = Self::get_ticket(&env, ticket_id)?;
        Ok(ticket.owner)
    }

    pub fn verify_ticket(env: Env, ticket_id: u64) -> Result<Ticket, Error> {
        Self::get_ticket(&env, ticket_id)
    }

    /// Read-only on-chain batch verification of tickets.
    /// Allows scanners to inspect multiple tickets in one call.
    /// Bounded by `MAX_BATCH_SIZE`.
    pub fn verify_tickets(env: Env, ticket_ids: Vec<u64>) -> Result<Vec<Ticket>, Error> {
        if ticket_ids.is_empty() {
            return Err(Error::EmptyBatch);
        }
        if ticket_ids.len() > MAX_BATCH_SIZE {
            return Err(Error::BatchTooLarge);
        }
        let mut tickets = Vec::new(&env);
        for ticket_id in ticket_ids.iter() {
            tickets.push_back(Self::get_ticket(&env, ticket_id)?);
        }
        Ok(tickets)
    }

    /// Paginated ticket enumeration for an event. Returns up to `limit` tickets
    /// starting from `start_index`. Enables bounded enumeration across all tickets.
    /// `start_index` is 0-based; returns empty if start exceeds total tickets.
    pub fn list_tickets(
        env: Env,
        event_id: u64,
        start_index: u64,
        limit: u32,
    ) -> Result<Vec<Ticket>, Error> {
        // Verify event exists
        let _ = Self::get_event(&env, event_id)?;

        if limit == 0 || limit > MAX_BATCH_SIZE {
            return Err(Error::BatchTooLarge);
        }

        let mut tickets = Vec::new(&env);
        let next_ticket_id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextTicketId)
            .unwrap_or(0);

        let mut count = 0u32;
        let mut current_index = 0u64;

        for ticket_id in 0..next_ticket_id {
            if let Ok(ticket) = Self::get_ticket(&env, ticket_id) {
                if ticket.event_id == event_id {
                    if current_index >= start_index && count < limit {
                        tickets.push_back(ticket);
                        count += 1;
                    }
                    current_index += 1;
                    if count >= limit {
                        break;
                    }
                }
            }
        }

        Ok(tickets)
    }

    /// Marks a ticket as used at the point of entry. Only the event's
    /// organizer (or their delegated gate device, via a shared Soroban
    /// signer) may check a ticket in, and only once.
    pub fn check_in(env: Env, organizer: Address, ticket_id: u64) -> Result<(), Error> {
        organizer.require_auth();
        let mut ticket = Self::get_ticket(&env, ticket_id)?;
        let event = Self::get_event(&env, ticket.event_id)?;
        if event.organizer != organizer {
            return Err(Error::NotOrganizer);
        }
        match ticket.status {
            TicketStatus::Used => return Err(Error::AlreadyUsed),
            TicketStatus::Revoked => return Err(Error::Revoked),
            _ => {}
        }
        ticket.status = TicketStatus::Used;
        Self::remove_gift_claim(&env, ticket_id);
        Self::save_ticket(&env, ticket_id, &ticket);
        TicketCheckedIn {
            ticket_id,
            organizer,
        }
        .publish(&env);
        Ok(())
    }

    /// Marks a batch of tickets as used at the point of entry for group admission.
    /// Only the event's organizer may check tickets in, bounded by `MAX_BATCH_SIZE`.
    pub fn check_in_batch(env: Env, organizer: Address, ticket_ids: Vec<u64>) -> Result<(), Error> {
        organizer.require_auth();
        if ticket_ids.is_empty() {
            return Err(Error::EmptyBatch);
        }
        if ticket_ids.len() > MAX_BATCH_SIZE {
            return Err(Error::BatchTooLarge);
        }
        for ticket_id in ticket_ids.iter() {
            let mut ticket = Self::get_ticket(&env, ticket_id)?;
            let event = Self::get_event(&env, ticket.event_id)?;
            if event.organizer != organizer {
                return Err(Error::NotOrganizer);
            }
            match ticket.status {
                TicketStatus::Used => return Err(Error::AlreadyUsed),
                TicketStatus::Revoked => return Err(Error::Revoked),
                _ => {}
            }
            ticket.status = TicketStatus::Used;
            Self::remove_gift_claim(&env, ticket_id);
            Self::save_ticket(&env, ticket_id, &ticket);
            TicketCheckedIn {
                ticket_id,
                organizer: organizer.clone(),
            }
            .publish(&env);
        }
        Ok(())
    }

    /// Fraud prevention: organizer voids a ticket (chargeback, counterfeit
    /// report, policy violation). Revoked tickets can never be transferred,
    /// resold, or checked in again.
    pub fn revoke_ticket(env: Env, organizer: Address, ticket_id: u64) -> Result<(), Error> {
        organizer.require_auth();
        let mut ticket = Self::get_ticket(&env, ticket_id)?;
        let event = Self::get_event(&env, ticket.event_id)?;
        if event.organizer != organizer {
            return Err(Error::NotOrganizer);
        }
        ticket.status = TicketStatus::Revoked;
        Self::remove_gift_claim(&env, ticket_id);
        Self::save_ticket(&env, ticket_id, &ticket);
        Ok(())
    }

    /// Revokes a ticket with an OPTIONAL refund to the current owner
    /// (issue #236). When `refund` is true, the organizer pays the ticket's
    /// `original_price` back to the owner in the event's accepted payment
    /// token before the ticket is voided — the revocation remains
    /// permanent afterwards. Without a refund, behavior matches
    /// `revoke_ticket`. Revoking a used ticket is rejected either way.
    pub fn revoke_with_refund(
        env: Env,
        organizer: Address,
        ticket_id: u64,
        refund: bool,
    ) -> Result<(), Error> {
        organizer.require_auth();
        let mut ticket = Self::get_ticket(&env, ticket_id)?;
        let event = Self::get_event(&env, ticket.event_id)?;
        if event.organizer != organizer {
            return Err(Error::NotOrganizer);
        }
        if ticket.status == TicketStatus::Used {
            return Err(Error::AlreadyUsed);
        }
        if refund && ticket.original_price > 0 {
            let token_client =
                token::Client::new(&env, &Self::payment_token_for_event(&env, &event)?);
            token_client.transfer(&organizer, &ticket.owner, &ticket.original_price);
        }
        ticket.status = TicketStatus::Revoked;
        Self::remove_gift_claim(&env, ticket_id);
        Self::save_ticket(&env, ticket_id, &ticket);
        Ok(())
    }

    /// Mass revocation of tickets by the event organizer (chargeback, policy violation).
    /// Bounded by `MAX_BATCH_SIZE`.
    pub fn revoke_batch(env: Env, organizer: Address, ticket_ids: Vec<u64>) -> Result<(), Error> {
        organizer.require_auth();
        if ticket_ids.is_empty() {
            return Err(Error::EmptyBatch);
        }
        if ticket_ids.len() > MAX_BATCH_SIZE {
            return Err(Error::BatchTooLarge);
        }
        for ticket_id in ticket_ids.iter() {
            let mut ticket = Self::get_ticket(&env, ticket_id)?;
            let event = Self::get_event(&env, ticket.event_id)?;
            if event.organizer != organizer {
                return Err(Error::NotOrganizer);
            }
            ticket.status = TicketStatus::Revoked;
            Self::remove_gift_claim(&env, ticket_id);
            Self::save_ticket(&env, ticket_id, &ticket);
        }
        Ok(())
    }

    /// Lists an owned, valid ticket on the resale marketplace. The price is
    /// capped at the event's `max_resale_multiplier_bps` of the original
    /// sale price to curb scalping.
    pub fn list_for_resale(
        env: Env,
        owner: Address,
        ticket_id: u64,
        price: i128,
    ) -> Result<(), Error> {
        owner.require_auth();
        if price <= 0 {
            return Err(Error::InvalidPrice);
        }
        let mut ticket = Self::get_ticket(&env, ticket_id)?;
        if ticket.owner != owner {
            return Err(Error::NotOwner);
        }
        match ticket.status {
            TicketStatus::Used => return Err(Error::AlreadyUsed),
            TicketStatus::Revoked => return Err(Error::Revoked),
            _ => {}
        }
        let event = Self::get_event(&env, ticket.event_id)?;
        if Self::resale_closed(&env, &event) {
            return Err(Error::ResaleClosed);
        }
        let cap = ticket.original_price * event.max_resale_multiplier_bps as i128 / 10_000;
        if price > cap {
            return Err(Error::ResalePriceExceedsCap);
        }
        ticket.status = TicketStatus::Resale;
        ticket.resale_price = price;
        Self::remove_gift_claim(&env, ticket_id);
        Self::save_ticket(&env, ticket_id, &ticket);
        Ok(())
    }

    pub fn cancel_resale(env: Env, owner: Address, ticket_id: u64) -> Result<(), Error> {
        owner.require_auth();
        let mut ticket = Self::get_ticket(&env, ticket_id)?;
        if ticket.owner != owner {
            return Err(Error::NotOwner);
        }
        if ticket.status != TicketStatus::Resale {
            return Err(Error::NotForResale);
        }
        ticket.status = TicketStatus::Valid;
        ticket.resale_price = 0;
        Self::save_ticket(&env, ticket_id, &ticket);
        Ok(())
    }

    /// Buys a resale-listed ticket. Payment is settled atomically on-chain:
    /// the organizer's royalty cut is paid first, the remainder to the
    /// seller, then ownership transfers to the buyer.
    pub fn buy_resale(env: Env, buyer: Address, ticket_id: u64) -> Result<(), Error> {
        buyer.require_auth();
        let mut ticket = Self::get_ticket(&env, ticket_id)?;
        if ticket.status != TicketStatus::Resale {
            return Err(Error::NotForResale);
        }
        let event = Self::get_event(&env, ticket.event_id)?;
        if Self::resale_closed(&env, &event) {
            return Err(Error::ResaleClosed);
        }
        let token_client = token::Client::new(&env, &Self::payment_token_for_event(&env, &event)?);
        let royalty = ticket.resale_price * event.royalty_bps as i128 / 10_000;
        let seller_amount = ticket.resale_price - royalty;
        if royalty > 0 {
            token_client.transfer(&buyer, &event.organizer, &royalty);
        }
        if seller_amount > 0 {
            token_client.transfer(&buyer, &ticket.owner, &seller_amount);
        }
        ticket.owner = buyer;
        ticket.status = TicketStatus::Valid;
        ticket.resale_price = 0;
        Self::remove_gift_claim(&env, ticket_id);
        Self::save_ticket(&env, ticket_id, &ticket);
        Ok(())
    }

    pub fn get_event(env: &Env, event_id: u64) -> Result<Event, Error> {
        env.storage()
            .persistent()
            .get(&DataKey::Event(event_id))
            .ok_or(Error::EventNotFound)
    }

    pub fn get_ticket(env: &Env, ticket_id: u64) -> Result<Ticket, Error> {
        env.storage()
            .persistent()
            .get(&DataKey::Ticket(ticket_id))
            .ok_or(Error::TicketNotFound)
    }

    fn save_ticket(env: &Env, ticket_id: u64, ticket: &Ticket) {
        let key = DataKey::Ticket(ticket_id);
        env.storage().persistent().set(&key, ticket);
        env.storage()
            .persistent()
            .extend_ttl(&key, LEDGER_THRESHOLD, LEDGER_BUMP);
    }

    fn remove_gift_claim(env: &Env, ticket_id: u64) {
        env.storage()
            .persistent()
            .remove(&DataKey::GiftClaim(ticket_id));
    }

    fn transfer_frozen(env: &Env, event: &Event) -> bool {
        env.ledger().timestamp()
            >= event
                .starts_at
                .saturating_sub(event.transfer_freeze_seconds)
    }

    fn resale_closed(env: &Env, event: &Event) -> bool {
        env.ledger().timestamp() >= event.starts_at.saturating_sub(event.resale_cutoff_seconds)
    }

    fn enforce_purchase_throttle(env: &Env, buyer: &Address) -> Result<(), Error> {
        let spacing: u32 = env
            .storage()
            .instance()
            .get(&DataKey::MinPurchaseSpacing)
            .unwrap_or(0);
        if spacing == 0 {
            return Ok(());
        }
        let key = DataKey::LastPurchaseLedger(buyer.clone());
        if let Some(last_ledger) = env.storage().persistent().get::<_, u32>(&key) {
            let current = env.ledger().sequence();
            if current.saturating_sub(last_ledger) < spacing {
                return Err(Error::PurchaseTooSoon);
            }
        }
        Ok(())
    }

    fn record_purchase(env: &Env, buyer: &Address) {
        let key = DataKey::LastPurchaseLedger(buyer.clone());
        let current = env.ledger().sequence();
        env.storage().persistent().set(&key, &current);
        env.storage()
            .persistent()
            .extend_ttl(&key, LEDGER_THRESHOLD, LEDGER_BUMP);
    }

    fn require_admin(env: &Env, admin: &Address) -> Result<(), Error> {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)?;
        if stored_admin != *admin {
            return Err(Error::NotAdmin);
        }
        admin.require_auth();
        Ok(())
    }

    /// Probes `token` with a `decimals()` call so an address that is not a
    /// token contract is rejected up front. Returns the token's decimals so
    /// callers can cache them (issue #233).
    fn ensure_token_contract(env: &Env, token: &Address) -> Result<u32, Error> {
        match token::Client::new(env, token).try_decimals() {
            Ok(Ok(decimals)) => Ok(decimals),
            _ => Err(Error::InvalidPaymentToken),
        }
    }

    /// Decimals of the active payment token, cached at initialization and
    /// refreshed whenever the payment token changes. Frontends use this to
    /// convert token amounts between raw units and display units.
    pub fn token_decimals(env: Env) -> Result<u32, Error> {
        env.storage()
            .instance()
            .get(&DataKey::TokenDecimals)
            .ok_or(Error::NotInitialized)
    }

    fn payment_token(env: &Env) -> Result<Address, Error> {
        env.storage()
            .instance()
            .get(&DataKey::PaymentToken)
            .ok_or(Error::NotInitialized)
    }

    /// Resolves the payment token for an event: the per-event accepted token
    /// when set, otherwise the contract-wide payment token (issue #235).
    fn payment_token_for_event(env: &Env, event: &Event) -> Result<Address, Error> {
        match &event.payment_token {
            Some(token) => Ok(token.clone()),
            None => Self::payment_token(env),
        }
    }

    fn mint(env: &Env, event_id: u64, to: Address, tier: String, seat: String, price: i128) -> u64 {
        let ticket_id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::NextTicketId)
            .unwrap_or(0);
        let ticket = Ticket {
            event_id,
            owner: to,
            tier,
            seat,
            status: TicketStatus::Valid,
            original_price: price,
            resale_price: 0,
        };
        Self::save_ticket(env, ticket_id, &ticket);
        env.storage()
            .instance()
            .set(&DataKey::NextTicketId, &(ticket_id + 1));
        TicketIssued {
            ticket_id,
            event_id,
        }
        .publish(env);
        ticket_id
    }
}

#[cfg(test)]
mod test;
