#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{StellarAssetClient, TokenClient},
    Bytes, Env, String, Vec,
};

fn setup<'a>() -> (
    Env,
    TicketingContractClient<'a>,
    TokenClient<'a>,
    StellarAssetClient<'a>,
    Address,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let organizer = Address::generate(&env);

    let token_admin = Address::generate(&env);
    let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
    let token = TokenClient::new(&env, &token_contract.address());
    let token_asset = StellarAssetClient::new(&env, &token_contract.address());

    let contract_id = env.register(TicketingContract, ());
    let client = TicketingContractClient::new(&env, &contract_id);
    client.initialize(&admin, &token_contract.address());

    (env, client, token, token_asset, admin, organizer)
}

fn make_event(env: &Env, client: &TicketingContractClient, organizer: &Address, event_id: u64) {
    client.create_event(
        organizer,
        &event_id,
        &String::from_str(env, "Radiohead Live"),
        &String::from_str(env, "concert"),
        &12_000u32, // max 120% of face value on resale
        &500u32,    // 5% organizer royalty
        &10_000u64,
        &100u64,
        &200u64,
    );
}

#[test]
fn issues_and_verifies_ticket() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);

    let buyer = Address::generate(&env);
    let ticket_id = client.issue_ticket(
        &organizer,
        &1,
        &buyer,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &5_000i128,
    );

    let ticket = client.verify_ticket(&ticket_id);
    assert_eq!(ticket.owner, buyer);
    assert_eq!(ticket.status, TicketStatus::Valid);
    assert_eq!(ticket.original_price, 5_000);

    let event = client.get_event(&1);
    assert_eq!(event.tickets_issued, 1);
}

#[test]
fn check_in_marks_used_and_rejects_reentry() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let buyer = Address::generate(&env);
    let ticket_id = client.issue_ticket(
        &organizer,
        &1,
        &buyer,
        &String::from_str(&env, "VIP"),
        &String::from_str(&env, "A1"),
        &10_000i128,
    );

    client.check_in(&organizer, &ticket_id);
    let ticket = client.verify_ticket(&ticket_id);
    assert_eq!(ticket.status, TicketStatus::Used);

    let result = client.try_check_in(&organizer, &ticket_id);
    assert_eq!(result, Err(Ok(Error::AlreadyUsed)));
}

#[test]
fn revoked_ticket_cannot_be_checked_in_or_transferred() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let buyer = Address::generate(&env);
    let ticket_id = client.issue_ticket(
        &organizer,
        &1,
        &buyer,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &1_000i128,
    );

    client.revoke_ticket(&organizer, &ticket_id);

    let checkin_result = client.try_check_in(&organizer, &ticket_id);
    assert_eq!(checkin_result, Err(Ok(Error::Revoked)));

    let other = Address::generate(&env);
    let transfer_result = client.try_transfer_ticket(&buyer, &ticket_id, &other);
    assert_eq!(transfer_result, Err(Ok(Error::Revoked)));
}

#[test]
fn transfer_moves_ownership() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let buyer = Address::generate(&env);
    let friend = Address::generate(&env);
    let ticket_id = client.issue_ticket(
        &organizer,
        &1,
        &buyer,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &1_000i128,
    );

    client.transfer_ticket(&buyer, &ticket_id, &friend);
    let ticket = client.verify_ticket(&ticket_id);
    assert_eq!(ticket.owner, friend);

    let stale = client.try_transfer_ticket(&buyer, &ticket_id, &organizer);
    assert_eq!(stale, Err(Ok(Error::NotOwner)));
}

#[test]
fn resale_listing_rejects_prices_above_cap() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1); // cap is 120% of face value
    let buyer = Address::generate(&env);
    let ticket_id = client.issue_ticket(
        &organizer,
        &1,
        &buyer,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &1_000i128,
    );

    let too_high = client.try_list_for_resale(&buyer, &ticket_id, &1_201i128);
    assert_eq!(too_high, Err(Ok(Error::ResalePriceExceedsCap)));

    client.list_for_resale(&buyer, &ticket_id, &1_200i128);
    let ticket = client.verify_ticket(&ticket_id);
    assert_eq!(ticket.status, TicketStatus::Resale);
    assert_eq!(ticket.resale_price, 1_200);
}

#[test]
fn buy_resale_splits_royalty_and_transfers_ownership() {
    let (env, client, token, token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1); // 5% royalty
    let seller = Address::generate(&env);
    let buyer = Address::generate(&env);

    let ticket_id = client.issue_ticket(
        &organizer,
        &1,
        &seller,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &1_000i128,
    );
    client.list_for_resale(&seller, &ticket_id, &1_100i128);

    token_asset.mint(&buyer, &10_000i128);
    client.buy_resale(&buyer, &ticket_id);

    // 5% of 1100 = 55 to organizer, 1045 to seller.
    assert_eq!(token.balance(&organizer), 55);
    assert_eq!(token.balance(&seller), 1_045);
    assert_eq!(token.balance(&buyer), 10_000 - 1_100);

    let ticket = client.verify_ticket(&ticket_id);
    assert_eq!(ticket.owner, buyer);
    assert_eq!(ticket.status, TicketStatus::Valid);
    assert_eq!(ticket.resale_price, 0);
}

#[test]
fn purchase_primary_pays_organizer_on_chain() {
    let (env, client, token, token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let buyer = Address::generate(&env);
    token_asset.mint(&buyer, &5_000i128);

    let ticket_id = client.purchase_primary(
        &buyer,
        &1,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &2_000i128,
    );

    assert_eq!(token.balance(&organizer), 2_000);
    assert_eq!(token.balance(&buyer), 3_000);
    let ticket = client.verify_ticket(&ticket_id);
    assert_eq!(ticket.owner, buyer);
    assert_eq!(ticket.original_price, 2_000);
}

#[test]
fn non_organizer_cannot_issue_tickets_for_someone_elses_event() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let impostor = Address::generate(&env);
    let buyer = Address::generate(&env);

    let result = client.try_issue_ticket(
        &impostor,
        &1,
        &buyer,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &500i128,
    );
    assert_eq!(result, Err(Ok(Error::NotOrganizer)));
}

#[test]
fn cancel_resale_rejects_a_ticket_that_is_not_listed() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let owner = Address::generate(&env);
    let ticket_id = client.issue_ticket(
        &organizer,
        &1,
        &owner,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &1_000i128,
    );

    let result = client.try_cancel_resale(&owner, &ticket_id);
    assert_eq!(result, Err(Ok(Error::NotForResale)));
}

#[test]
fn list_for_resale_rejects_a_non_owner() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let owner = Address::generate(&env);
    let impostor = Address::generate(&env);
    let ticket_id = client.issue_ticket(
        &organizer,
        &1,
        &owner,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &1_000i128,
    );

    let result = client.try_list_for_resale(&impostor, &ticket_id, &1_000i128);
    assert_eq!(result, Err(Ok(Error::NotOwner)));
}

#[test]
fn buy_resale_rejects_a_ticket_that_is_not_listed() {
    let (env, client, _token, token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let owner = Address::generate(&env);
    let buyer = Address::generate(&env);
    token_asset.mint(&buyer, &10_000i128);
    let ticket_id = client.issue_ticket(
        &organizer,
        &1,
        &owner,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &1_000i128,
    );

    let result = client.try_buy_resale(&buyer, &ticket_id);
    assert_eq!(result, Err(Ok(Error::NotForResale)));
}

#[test]
fn check_in_rejects_the_wrong_organizer() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let impostor = Address::generate(&env);
    let owner = Address::generate(&env);
    let ticket_id = client.issue_ticket(
        &organizer,
        &1,
        &owner,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &1_000i128,
    );

    let result = client.try_check_in(&impostor, &ticket_id);
    assert_eq!(result, Err(Ok(Error::NotOrganizer)));
}

#[test]
fn revoke_rejects_the_wrong_organizer() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let impostor = Address::generate(&env);
    let owner = Address::generate(&env);
    let ticket_id = client.issue_ticket(
        &organizer,
        &1,
        &owner,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &1_000i128,
    );

    let result = client.try_revoke_ticket(&impostor, &ticket_id);
    assert_eq!(result, Err(Ok(Error::NotOrganizer)));
}

#[test]
fn get_event_reports_not_found_for_an_unknown_id() {
    let (_env, client, _token, _token_asset, _admin, _organizer) = setup();
    match client.try_get_event(&999) {
        Err(Ok(Error::EventNotFound)) => {}
        other => panic!("expected EventNotFound, got {other:?}"),
    }
}

#[test]
fn get_ticket_reports_not_found_for_an_unknown_id() {
    let (_env, client, _token, _token_asset, _admin, _organizer) = setup();
    match client.try_get_ticket(&999) {
        Err(Ok(Error::TicketNotFound)) => {}
        other => panic!("expected TicketNotFound, got {other:?}"),
    }
}

#[test]
fn create_event_rejects_a_duplicate_event_id() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);

    let result = client.try_create_event(
        &organizer,
        &1,
        &String::from_str(&env, "Another Show"),
        &String::from_str(&env, "concert"),
        &12_000u32,
        &500u32,
        &10_000u64,
        &100u64,
        &200u64,
    );
    assert_eq!(result, Err(Ok(Error::EventAlreadyExists)));
}

#[test]
fn initialize_rejects_a_second_call() {
    let (env, client, _token, _token_asset, admin, _organizer) = setup();
    let other_token = Address::generate(&env);
    let result = client.try_initialize(&admin, &other_token);
    assert_eq!(result, Err(Ok(Error::AlreadyInitialized)));
}

#[test]
fn issue_ticket_rejects_a_negative_price() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let buyer = Address::generate(&env);

    let result = client.try_issue_ticket(
        &organizer,
        &1,
        &buyer,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &-1i128,
    );
    assert_eq!(result, Err(Ok(Error::InvalidPrice)));
}

#[test]
fn issue_ticket_allows_a_zero_price_comp_ticket() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let vip_guest = Address::generate(&env);

    let ticket_id = client.issue_ticket(
        &organizer,
        &1,
        &vip_guest,
        &String::from_str(&env, "Comp"),
        &String::from_str(&env, "unassigned"),
        &0i128,
    );

    let ticket = client.verify_ticket(&ticket_id);
    assert_eq!(ticket.original_price, 0);
    assert_eq!(ticket.status, TicketStatus::Valid);
}

#[test]
fn create_event_rejects_a_royalty_above_10_000_bps() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    let result = client.try_create_event(
        &organizer,
        &1,
        &String::from_str(&env, "Radiohead Live"),
        &String::from_str(&env, "concert"),
        &12_000u32,
        &10_001u32,
        &10_000u64,
        &100u64,
        &200u64,
    );
    assert_eq!(result, Err(Ok(Error::InvalidRoyalty)));
}

#[test]
fn list_for_resale_rejects_a_zero_price() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let owner = Address::generate(&env);
    let ticket_id = client.issue_ticket(
        &organizer,
        &1,
        &owner,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &1_000i128,
    );

    let result = client.try_list_for_resale(&owner, &ticket_id, &0i128);
    assert_eq!(result, Err(Ok(Error::InvalidPrice)));
}

#[test]
fn cancel_resale_returns_a_ticket_to_valid() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let owner = Address::generate(&env);
    let ticket_id = client.issue_ticket(
        &organizer,
        &1,
        &owner,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &1_000i128,
    );

    client.list_for_resale(&owner, &ticket_id, &1_100i128);
    assert_eq!(
        client.verify_ticket(&ticket_id).status,
        TicketStatus::Resale
    );

    client.cancel_resale(&owner, &ticket_id);
    let ticket = client.verify_ticket(&ticket_id);
    assert_eq!(ticket.status, TicketStatus::Valid);
    assert_eq!(ticket.resale_price, 0);
}

#[test]
fn organizer_can_run_multiple_independent_events() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    make_event(&env, &client, &organizer, 2);

    let buyer = Address::generate(&env);
    let ticket_a = client.issue_ticket(
        &organizer,
        &1,
        &buyer,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &1_000i128,
    );
    let ticket_b = client.issue_ticket(
        &organizer,
        &2,
        &buyer,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &2_000i128,
    );

    assert_eq!(client.get_event(&1).tickets_issued, 1);
    assert_eq!(client.get_event(&2).tickets_issued, 1);
    assert_eq!(client.verify_ticket(&ticket_a).event_id, 1);
    assert_eq!(client.verify_ticket(&ticket_b).event_id, 2);
}

#[test]
fn transferring_a_resale_listed_ticket_clears_the_listing_state() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let owner = Address::generate(&env);
    let friend = Address::generate(&env);
    let ticket_id = client.issue_ticket(
        &organizer,
        &1,
        &owner,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &1_000i128,
    );

    client.list_for_resale(&owner, &ticket_id, &1_100i128);
    client.transfer_ticket(&owner, &ticket_id, &friend);

    let ticket = client.verify_ticket(&ticket_id);
    assert_eq!(ticket.owner, friend);
    assert_eq!(ticket.status, TicketStatus::Valid);
    assert_eq!(ticket.resale_price, 0);
}

#[test]
fn purchase_primary_increments_tickets_issued() {
    let (env, client, _token, token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let buyer = Address::generate(&env);
    token_asset.mint(&buyer, &5_000i128);

    client.purchase_primary(
        &buyer,
        &1,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &1_000i128,
    );

    assert_eq!(client.get_event(&1).tickets_issued, 1);
}

#[test]
fn revoke_permanently_blocks_resale_actions() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let owner = Address::generate(&env);
    let ticket_id = client.issue_ticket(
        &organizer,
        &1,
        &owner,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &1_000i128,
    );

    client.revoke_ticket(&organizer, &ticket_id);

    let list_result = client.try_list_for_resale(&owner, &ticket_id, &1_000i128);
    assert_eq!(list_result, Err(Ok(Error::Revoked)));
}

#[test]
fn buy_resale_with_zero_royalty_pays_the_seller_in_full() {
    let (env, client, token, token_asset, _admin, organizer) = setup();
    client.create_event(
        &organizer,
        &1,
        &String::from_str(&env, "Community Meetup"),
        &String::from_str(&env, "corporate_events"),
        &15_000u32,
        &0u32, // no royalty
        &10_000u64,
        &100u64,
        &200u64,
    );
    let seller = Address::generate(&env);
    let buyer = Address::generate(&env);

    let ticket_id = client.issue_ticket(
        &organizer,
        &1,
        &seller,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &1_000i128,
    );
    client.list_for_resale(&seller, &ticket_id, &1_200i128);
    token_asset.mint(&buyer, &5_000i128);

    client.buy_resale(&buyer, &ticket_id);

    assert_eq!(token.balance(&organizer), 0);
    assert_eq!(token.balance(&seller), 1_200);
}

#[test]
fn resale_price_exactly_at_the_face_value_cap_is_allowed() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    client.create_event(
        &organizer,
        &1,
        &String::from_str(&env, "University Lecture"),
        &String::from_str(&env, "universities"),
        &10_000u32, // no markup allowed at all
        &0u32,
        &10_000u64,
        &100u64,
        &200u64,
    );
    let owner = Address::generate(&env);
    let ticket_id = client.issue_ticket(
        &organizer,
        &1,
        &owner,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &1_000i128,
    );

    // Exactly face value should be allowed even with a 100% (no markup) cap.
    client.list_for_resale(&owner, &ticket_id, &1_000i128);
    assert_eq!(client.verify_ticket(&ticket_id).resale_price, 1_000);

    // One unit above face value must still be rejected under the same cap.
    let over = client.try_list_for_resale(&owner, &ticket_id, &1_001i128);
    assert_eq!(over, Err(Ok(Error::ResalePriceExceedsCap)));
}

#[test]
fn seat_and_tier_survive_a_transfer() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let owner = Address::generate(&env);
    let friend = Address::generate(&env);
    let ticket_id = client.issue_ticket(
        &organizer,
        &1,
        &owner,
        &String::from_str(&env, "VIP"),
        &String::from_str(&env, "A1"),
        &1_000i128,
    );

    client.transfer_ticket(&owner, &ticket_id, &friend);

    let ticket = client.verify_ticket(&ticket_id);
    assert_eq!(ticket.tier, String::from_str(&env, "VIP"));
    assert_eq!(ticket.seat, String::from_str(&env, "A1"));
}

#[test]
fn purchase_primary_allows_a_free_event() {
    let (env, client, token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let buyer = Address::generate(&env);

    let ticket_id = client.purchase_primary(
        &buyer,
        &1,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &0i128,
    );

    assert_eq!(token.balance(&organizer), 0);
    let ticket = client.verify_ticket(&ticket_id);
    assert_eq!(ticket.owner, buyer);
    assert_eq!(ticket.original_price, 0);
}


#[test]
fn lottery_allocates_requested_number_of_tickets() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);

    let entrant_a = Address::generate(&env);
    let entrant_b = Address::generate(&env);
    let entrant_c = Address::generate(&env);
    let mut entrants = Vec::new(&env);
    entrants.push_back(entrant_a.clone());
    entrants.push_back(entrant_b.clone());
    entrants.push_back(entrant_c.clone());

    let ticket_ids = client.allocate_lottery(
        &organizer,
        &1,
        &entrants,
        &2u32,
        &String::from_str(&env, "Lottery"),
        &0i128,
    );

    assert_eq!(ticket_ids.len(), 2);
    let first_owner = client.verify_ticket(&ticket_ids.get(0).unwrap()).owner;
    let second_owner = client.verify_ticket(&ticket_ids.get(1).unwrap()).owner;
    assert_ne!(first_owner, second_owner);
    assert!(
        first_owner == entrant_a || first_owner == entrant_b || first_owner == entrant_c
    );
    assert!(
        second_owner == entrant_a || second_owner == entrant_b || second_owner == entrant_c
    );
    assert_eq!(client.get_event(&1).tickets_issued, 2);
}

#[test]
fn lottery_rejects_more_winners_than_entrants() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);

    let mut entrants = Vec::new(&env);
    entrants.push_back(Address::generate(&env));

    let result = client.try_allocate_lottery(
        &organizer,
        &1,
        &entrants,
        &2u32,
        &String::from_str(&env, "Lottery"),
        &0i128,
    );
    assert_eq!(result, Err(Ok(Error::InvalidLottery)));
}

#[test]
fn gift_claim_transfers_ticket_with_correct_secret() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);

    let owner = Address::generate(&env);
    let recipient = Address::generate(&env);
    let ticket_id = client.issue_ticket(
        &organizer,
        &1,
        &owner,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &1_000i128,
    );

    let secret = Bytes::from_slice(&env, b"claim-me");
    let secret_hash = env.crypto().sha256(&secret).to_bytes();
    client.create_gift_claim(&owner, &ticket_id, &secret_hash, &500u64);
    client.claim_gift(&recipient, &ticket_id, &secret);

    assert_eq!(client.verify_ticket(&ticket_id).owner, recipient);
    assert_eq!(
        client.try_claim_gift(&owner, &ticket_id, &secret),
        Err(Ok(Error::GiftClaimNotFound))
    );
}

#[test]
fn gift_claim_rejects_wrong_secret_and_expired_claim() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);

    let owner = Address::generate(&env);
    let recipient = Address::generate(&env);
    let ticket_id = client.issue_ticket(
        &organizer,
        &1,
        &owner,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &1_000i128,
    );

    let secret = Bytes::from_slice(&env, b"claim-me");
    let wrong_secret = Bytes::from_slice(&env, b"wrong");
    let secret_hash = env.crypto().sha256(&secret).to_bytes();
    client.create_gift_claim(&owner, &ticket_id, &secret_hash, &500u64);

    assert_eq!(
        client.try_claim_gift(&recipient, &ticket_id, &wrong_secret),
        Err(Ok(Error::InvalidSecret))
    );

    env.ledger().set_timestamp(500);
    assert_eq!(
        client.try_claim_gift(&recipient, &ticket_id, &secret),
        Err(Ok(Error::GiftClaimExpired))
    );
}

#[test]
fn direct_transfer_freezes_at_configured_window() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    client.create_event(
        &organizer,
        &1,
        &String::from_str(&env, "Timed Event"),
        &String::from_str(&env, "concert"),
        &12_000u32,
        &500u32,
        &1_000u64,
        &100u64,
        &200u64,
    );

    let owner = Address::generate(&env);
    let friend = Address::generate(&env);
    let ticket_before = client.issue_ticket(
        &organizer,
        &1,
        &owner,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "A1"),
        &1_000i128,
    );
    let ticket_frozen = client.issue_ticket(
        &organizer,
        &1,
        &owner,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "A2"),
        &1_000i128,
    );

    env.ledger().set_timestamp(899);
    client.transfer_ticket(&owner, &ticket_before, &friend);

    env.ledger().set_timestamp(900);
    assert_eq!(
        client.try_transfer_ticket(&owner, &ticket_frozen, &friend),
        Err(Ok(Error::TransfersFrozen))
    );
}

#[test]
fn resale_listing_and_purchase_close_at_cutoff() {
    let (env, client, _token, token_asset, _admin, organizer) = setup();
    client.create_event(
        &organizer,
        &1,
        &String::from_str(&env, "Timed Event"),
        &String::from_str(&env, "concert"),
        &12_000u32,
        &500u32,
        &1_000u64,
        &100u64,
        &200u64,
    );

    let seller = Address::generate(&env);
    let buyer = Address::generate(&env);
    token_asset.mint(&buyer, &10_000i128);

    let listed_ticket = client.issue_ticket(
        &organizer,
        &1,
        &seller,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "A1"),
        &1_000i128,
    );
    let late_ticket = client.issue_ticket(
        &organizer,
        &1,
        &seller,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "A2"),
        &1_000i128,
    );

    env.ledger().set_timestamp(799);
    client.list_for_resale(&seller, &listed_ticket, &1_100i128);

    env.ledger().set_timestamp(800);
    assert_eq!(
        client.try_list_for_resale(&seller, &late_ticket, &1_100i128),
        Err(Ok(Error::ResaleClosed))
    );
    assert_eq!(
        client.try_buy_resale(&buyer, &listed_ticket),
        Err(Ok(Error::ResaleClosed))
    );
}
