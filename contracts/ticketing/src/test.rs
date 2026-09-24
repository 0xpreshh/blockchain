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
    assert!(first_owner == entrant_a || first_owner == entrant_b || first_owner == entrant_c);
    assert!(second_owner == entrant_a || second_owner == entrant_b || second_owner == entrant_c);
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
fn lottery_rejects_duplicate_entrants() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);

    let entrant = Address::generate(&env);
    let mut entrants = Vec::new(&env);
    entrants.push_back(entrant.clone());
    entrants.push_back(entrant);

    let result = client.try_allocate_lottery(
        &organizer,
        &1,
        &entrants,
        &2u32,
        &String::from_str(&env, "Lottery"),
        &0i128,
    );

    assert_eq!(result, Err(Ok(Error::InvalidLottery)));
    assert_eq!(client.get_event(&1).tickets_issued, 0);
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
fn resale_listing_invalidates_an_existing_gift_claim() {
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
    client.list_for_resale(&owner, &ticket_id, &1_100i128);

    assert_eq!(
        client.try_claim_gift(&recipient, &ticket_id, &secret),
        Err(Ok(Error::GiftClaimNotFound))
    );
    assert_eq!(
        client.verify_ticket(&ticket_id).status,
        TicketStatus::Resale
    );
}

#[test]
fn gift_claim_creation_cancels_an_existing_resale_listing() {
    let (env, client, _token, token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);

    let owner = Address::generate(&env);
    let recipient = Address::generate(&env);
    let buyer = Address::generate(&env);
    let ticket_id = client.issue_ticket(
        &organizer,
        &1,
        &owner,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &1_000i128,
    );
    client.list_for_resale(&owner, &ticket_id, &1_100i128);
    token_asset.mint(&buyer, &2_000i128);

    let secret = Bytes::from_slice(&env, b"claim-me");
    let secret_hash = env.crypto().sha256(&secret).to_bytes();
    client.create_gift_claim(&owner, &ticket_id, &secret_hash, &500u64);

    let ticket = client.verify_ticket(&ticket_id);
    assert_eq!(ticket.status, TicketStatus::Valid);
    assert_eq!(ticket.resale_price, 0);
    assert_eq!(
        client.try_buy_resale(&buyer, &ticket_id),
        Err(Ok(Error::NotForResale))
    );

    client.claim_gift(&recipient, &ticket_id, &secret);
    assert_eq!(client.verify_ticket(&ticket_id).owner, recipient);
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

#[test]
fn escrowed_primary_sale_holds_funds_in_the_contract() {
    let (env, client, token, token_asset, _admin, organizer) = setup();
    client.create_event(
        &organizer,
        &1,
        &String::from_str(&env, "Escrowed Show"),
        &String::from_str(&env, "concert"),
        &12_000u32,
        &500u32,
        &10_000u64,
        &100u64,
        &200u64,
    );
    client.enable_escrow(&organizer, &1, &1_000u32);
    let buyer = Address::generate(&env);
    token_asset.mint(&buyer, &5_000i128);

    client.purchase_primary(
        &buyer,
        &1,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &1_000i128,
    );

    assert_eq!(token.balance(&organizer), 0);
    assert_eq!(token.balance(&client.address), 1_000);
    assert_eq!(client.get_event(&1).escrow_balance, 1_000);
}

#[test]
fn release_escrow_rejects_before_the_event_ends() {
    let (env, client, _token, token_asset, _admin, organizer) = setup();
    client.create_event(
        &organizer,
        &1,
        &String::from_str(&env, "Escrowed Show"),
        &String::from_str(&env, "concert"),
        &12_000u32,
        &500u32,
        &10_000u64,
        &100u64,
        &200u64,
    );
    client.enable_escrow(&organizer, &1, &1_000u32);
    let buyer = Address::generate(&env);
    token_asset.mint(&buyer, &5_000i128);
    client.purchase_primary(
        &buyer,
        &1,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &1_000i128,
    );

    let result = client.try_release_escrow(&organizer, &1);
    assert_eq!(result, Err(Ok(Error::EventNotEnded)));
}

#[test]
fn release_escrow_pays_the_organizer_after_the_event_ends() {
    let (env, client, token, token_asset, _admin, organizer) = setup();
    client.create_event(
        &organizer,
        &1,
        &String::from_str(&env, "Escrowed Show"),
        &String::from_str(&env, "concert"),
        &12_000u32,
        &500u32,
        &10_000u64,
        &100u64,
        &200u64,
    );
    client.enable_escrow(&organizer, &1, &1_000u32);
    let buyer = Address::generate(&env);
    token_asset.mint(&buyer, &5_000i128);
    client.purchase_primary(
        &buyer,
        &1,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &1_000i128,
    );

    env.ledger().with_mut(|l| l.sequence_number = 1_000);
    client.release_escrow(&organizer, &1);

    assert_eq!(token.balance(&organizer), 1_000);
    assert_eq!(token.balance(&client.address), 0);
    assert_eq!(client.get_event(&1).escrow_balance, 0);

    client.release_escrow(&organizer, &1);
    assert_eq!(token.balance(&organizer), 1_000);
}

#[test]
fn release_escrow_rejects_a_non_escrow_event() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);

    let result = client.try_release_escrow(&organizer, &1);
    assert_eq!(result, Err(Ok(Error::EscrowNotEnabled)));
}

#[test]
fn enable_escrow_rejects_once_tickets_have_been_sold() {
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

    let result = client.try_enable_escrow(&organizer, &1, &1_000u32);
    assert_eq!(result, Err(Ok(Error::EventAlreadyStarted)));
}

#[test]
fn purchase_throttle_rejects_rapid_repeat_purchases() {
    let (env, client, _token, token_asset, admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    client.set_purchase_throttle(&admin, &10u32);
    let buyer = Address::generate(&env);
    token_asset.mint(&buyer, &5_000i128);

    client.purchase_primary(
        &buyer,
        &1,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &0i128,
    );

    let result = client.try_purchase_primary(
        &buyer,
        &1,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &0i128,
    );
    assert_eq!(result, Err(Ok(Error::PurchaseTooSoon)));
}

#[test]
fn purchase_throttle_allows_purchase_after_spacing_elapses() {
    let (env, client, _token, token_asset, admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    client.set_purchase_throttle(&admin, &10u32);
    let buyer = Address::generate(&env);
    token_asset.mint(&buyer, &5_000i128);

    client.purchase_primary(
        &buyer,
        &1,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &0i128,
    );

    env.ledger().with_mut(|l| l.sequence_number += 10);

    let ticket_id = client.purchase_primary(
        &buyer,
        &1,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &0i128,
    );
    assert_eq!(client.verify_ticket(&ticket_id).owner, buyer);
}

#[test]
fn set_purchase_throttle_rejects_a_non_admin_caller() {
    let (env, client, _token, _token_asset, _admin, _organizer) = setup();
    let not_admin = Address::generate(&env);
    let result = client.try_set_purchase_throttle(&not_admin, &10u32);
    assert_eq!(result, Err(Ok(Error::NotAdmin)));
}

#[test]
fn verify_tickets_returns_all_tickets() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let buyer = Address::generate(&env);
    let t1 = client.issue_ticket(
        &organizer,
        &1,
        &buyer,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "A1"),
        &5_000i128,
    );
    let t2 = client.issue_ticket(
        &organizer,
        &1,
        &buyer,
        &String::from_str(&env, "VIP"),
        &String::from_str(&env, "B1"),
        &10_000i128,
    );

    let mut ids = Vec::new(&env);
    ids.push_back(t1);
    ids.push_back(t2);

    let tickets = client.verify_tickets(&ids);
    assert_eq!(tickets.len(), 2);
    assert_eq!(tickets.get(0).unwrap().owner, buyer);
    assert_eq!(tickets.get(1).unwrap().owner, buyer);
    assert_eq!(tickets.get(0).unwrap().status, TicketStatus::Valid);
    assert_eq!(tickets.get(1).unwrap().status, TicketStatus::Valid);
}

#[test]
fn verify_tickets_rejects_empty_and_oversized_batches() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);

    let empty = Vec::new(&env);
    let res_empty = client.try_verify_tickets(&empty);
    assert_eq!(res_empty, Err(Ok(Error::EmptyBatch)));

    let mut too_large = Vec::new(&env);
    for i in 0..(MAX_BATCH_SIZE + 1) {
        too_large.push_back(i as u64);
    }
    let res_large = client.try_verify_tickets(&too_large);
    assert_eq!(res_large, Err(Ok(Error::BatchTooLarge)));
}

#[test]
fn verify_tickets_rejects_nonexistent_ticket() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let buyer = Address::generate(&env);
    let t1 = client.issue_ticket(
        &organizer,
        &1,
        &buyer,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "A1"),
        &5_000i128,
    );
    let mut ids = Vec::new(&env);
    ids.push_back(t1);
    ids.push_back(999u64);

    let res = client.try_verify_tickets(&ids);
    assert_eq!(res, Err(Ok(Error::TicketNotFound)));
}

#[test]
fn check_in_batch_marks_all_used_and_rejects_reentry() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let buyer = Address::generate(&env);
    let t1 = client.issue_ticket(
        &organizer,
        &1,
        &buyer,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "A1"),
        &5_000i128,
    );
    let t2 = client.issue_ticket(
        &organizer,
        &1,
        &buyer,
        &String::from_str(&env, "VIP"),
        &String::from_str(&env, "B1"),
        &10_000i128,
    );

    let mut ids = Vec::new(&env);
    ids.push_back(t1);
    ids.push_back(t2);

    client.check_in_batch(&organizer, &ids);
    assert_eq!(client.verify_ticket(&t1).status, TicketStatus::Used);
    assert_eq!(client.verify_ticket(&t2).status, TicketStatus::Used);

    let res = client.try_check_in_batch(&organizer, &ids);
    assert_eq!(res, Err(Ok(Error::AlreadyUsed)));
}

#[test]
fn check_in_batch_validates_bounds_and_organizer() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let wrong_organizer = Address::generate(&env);
    let buyer = Address::generate(&env);
    let t1 = client.issue_ticket(
        &organizer,
        &1,
        &buyer,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "A1"),
        &5_000i128,
    );
    let mut ids = Vec::new(&env);
    ids.push_back(t1);

    let empty = Vec::new(&env);
    assert_eq!(
        client.try_check_in_batch(&organizer, &empty),
        Err(Ok(Error::EmptyBatch))
    );

    let mut too_large = Vec::new(&env);
    for i in 0..(MAX_BATCH_SIZE + 1) {
        too_large.push_back(i as u64);
    }
    assert_eq!(
        client.try_check_in_batch(&organizer, &too_large),
        Err(Ok(Error::BatchTooLarge))
    );

    assert_eq!(
        client.try_check_in_batch(&wrong_organizer, &ids),
        Err(Ok(Error::NotOrganizer))
    );
}

#[test]
fn revoke_batch_marks_all_revoked() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let buyer = Address::generate(&env);
    let t1 = client.issue_ticket(
        &organizer,
        &1,
        &buyer,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "A1"),
        &5_000i128,
    );
    let t2 = client.issue_ticket(
        &organizer,
        &1,
        &buyer,
        &String::from_str(&env, "VIP"),
        &String::from_str(&env, "B1"),
        &10_000i128,
    );

    let mut ids = Vec::new(&env);
    ids.push_back(t1);
    ids.push_back(t2);

    client.revoke_batch(&organizer, &ids);
    assert_eq!(client.verify_ticket(&t1).status, TicketStatus::Revoked);
    assert_eq!(client.verify_ticket(&t2).status, TicketStatus::Revoked);

    assert_eq!(
        client.try_check_in(&organizer, &t1),
        Err(Ok(Error::Revoked))
    );
}

#[test]
fn revoke_batch_validates_bounds_and_organizer() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let wrong_organizer = Address::generate(&env);
    let buyer = Address::generate(&env);
    let t1 = client.issue_ticket(
        &organizer,
        &1,
        &buyer,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "A1"),
        &5_000i128,
    );
    let mut ids = Vec::new(&env);
    ids.push_back(t1);

    let empty = Vec::new(&env);
    assert_eq!(
        client.try_revoke_batch(&organizer, &empty),
        Err(Ok(Error::EmptyBatch))
    );

    let mut too_large = Vec::new(&env);
    for i in 0..(MAX_BATCH_SIZE + 1) {
        too_large.push_back(i as u64);
    }
    assert_eq!(
        client.try_revoke_batch(&organizer, &too_large),
        Err(Ok(Error::BatchTooLarge))
    );

    assert_eq!(
        client.try_revoke_batch(&wrong_organizer, &ids),
        Err(Ok(Error::NotOrganizer))
    );
}

#[test]
fn transfer_batch_moves_ownership() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let buyer1 = Address::generate(&env);
    let buyer2 = Address::generate(&env);

    let t1 = client.issue_ticket(
        &organizer,
        &1,
        &buyer1,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "A1"),
        &5_000i128,
    );
    let t2 = client.issue_ticket(
        &organizer,
        &1,
        &buyer1,
        &String::from_str(&env, "VIP"),
        &String::from_str(&env, "B1"),
        &10_000i128,
    );

    let mut ids = Vec::new(&env);
    ids.push_back(t1);
    ids.push_back(t2);

    client.transfer_batch(&buyer1, &ids, &buyer2);
    assert_eq!(client.verify_ticket(&t1).owner, buyer2);
    assert_eq!(client.verify_ticket(&t2).owner, buyer2);
    assert_eq!(client.verify_ticket(&t1).status, TicketStatus::Valid);
    assert_eq!(client.verify_ticket(&t2).status, TicketStatus::Valid);
}

#[test]
fn transfer_batch_validates_bounds_and_ownership() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let buyer1 = Address::generate(&env);
    let buyer2 = Address::generate(&env);
    let non_owner = Address::generate(&env);

    let t1 = client.issue_ticket(
        &organizer,
        &1,
        &buyer1,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "A1"),
        &5_000i128,
    );
    let mut ids = Vec::new(&env);
    ids.push_back(t1);

    let empty = Vec::new(&env);
    assert_eq!(
        client.try_transfer_batch(&buyer1, &empty, &buyer2),
        Err(Ok(Error::EmptyBatch))
    );

    let mut too_large = Vec::new(&env);
    for i in 0..(MAX_BATCH_SIZE + 1) {
        too_large.push_back(i as u64);
    }
    assert_eq!(
        client.try_transfer_batch(&buyer1, &too_large, &buyer2),
        Err(Ok(Error::BatchTooLarge))
    );

    assert_eq!(
        client.try_transfer_batch(&non_owner, &ids, &buyer2),
        Err(Ok(Error::NotOwner))
    );
}

#[test]
fn transfer_batch_rejects_when_transfers_frozen() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let buyer1 = Address::generate(&env);
    let buyer2 = Address::generate(&env);

    let t1 = client.issue_ticket(
        &organizer,
        &1,
        &buyer1,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "A1"),
        &5_000i128,
    );
    let mut ids = Vec::new(&env);
    ids.push_back(t1);

    // event starts_at = 10_000, freeze_seconds = 100 => frozen at timestamp >= 9_900
    env.ledger().with_mut(|l| l.timestamp = 9_950);

    let res = client.try_transfer_batch(&buyer1, &ids, &buyer2);
    assert_eq!(res, Err(Ok(Error::TransfersFrozen)));
}

#[test]
fn initialize_rejects_an_address_that_is_not_a_token_contract() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let not_a_token = env.register(TicketingContract, ());

    let contract_id = env.register(TicketingContract, ());
    let client = TicketingContractClient::new(&env, &contract_id);
    let result = client.try_initialize(&admin, &not_a_token);
    assert_eq!(result, Err(Ok(Error::InvalidPaymentToken)));
}

#[test]
fn admin_can_change_payment_token_after_the_timelock() {
    let (env, client, _token, _token_asset, admin, _organizer) = setup();
    let new_token = env.register_stellar_asset_contract_v2(Address::generate(&env));

    client.propose_payment_token(&admin, &new_token.address());

    let early = client.try_apply_payment_token(&admin);
    assert_eq!(early, Err(Ok(Error::TimelockNotElapsed)));

    env.ledger()
        .with_mut(|l| l.sequence_number += PAYMENT_TOKEN_CHANGE_DELAY_LEDGERS);
    client.apply_payment_token(&admin);

    let again = client.try_apply_payment_token(&admin);
    assert_eq!(again, Err(Ok(Error::NoPendingPaymentToken)));
}

#[test]
fn payment_token_change_requires_the_admin() {
    let (env, client, _token, _token_asset, _admin, _organizer) = setup();
    let stranger = Address::generate(&env);
    let new_token = env.register_stellar_asset_contract_v2(Address::generate(&env));

    let result = client.try_propose_payment_token(&stranger, &new_token.address());
    assert_eq!(result, Err(Ok(Error::NotAdmin)));
}

#[test]
fn proposing_a_non_token_payment_token_is_rejected() {
    let (env, client, _token, _token_asset, admin, _organizer) = setup();
    let not_a_token = Address::generate(&env);

    let result = client.try_propose_payment_token(&admin, &not_a_token);
    assert_eq!(result, Err(Ok(Error::InvalidPaymentToken)));
}


// ── Token decimals for client display (issue #233) ──────────────────────────

#[test]
fn token_decimals_are_cached_on_initialize() {
    let (env, client, _token, _token_asset, _admin, _organizer) = setup();

    // The Stellar Asset Contract wraps 7-decimal assets.
    assert_eq!(client.token_decimals(), 7);
}

#[test]
fn token_decimals_refresh_when_the_payment_token_changes() {
    let (env, client, _token, _token_asset, admin, _organizer) = setup();

    // A second token contract (another Stellar Asset Contract instance).
    let second_admin = Address::generate(&env);
    let second = env.register_stellar_asset_contract_v2(second_admin);

    client.propose_payment_token(&admin, &second.address());

    // Fast-forward past the payment-token timelock and apply the change.
    env.ledger().with_mut(|li| li.sequence_number += PAYMENT_TOKEN_CHANGE_DELAY_LEDGERS + 1);
    client.apply_payment_token(&admin);

    // The cached decimals are refreshed for the now-active token.
    assert_eq!(client.token_decimals(), 7);
}

#[test]
fn token_decimals_reject_an_invalid_token_on_propose() {
    let (env, client, _token, _token_asset, admin, _organizer) = setup();

    // An address that is not a token contract.
    let not_a_token = Address::generate(&env);
    let result = client.try_propose_payment_token(&admin, &not_a_token);
    assert_eq!(result, Err(Ok(Error::InvalidPaymentToken)));
}

// ── Native XLM payments via the Stellar Asset Contract (issue #234) ─────────

/// Registers the built-in Stellar Asset Contract for the NATIVE XLM asset in
/// the test environment, mirroring how `Env::register_stellar_asset_contract_v2`
/// deploys an asset-wrapped SAC, but with `Asset::Native` as the preimage.
fn register_native_asset_contract(env: &Env) -> Address {
    use std::rc::Rc;
    let create = xdr::HostFunction::CreateContract(xdr::CreateContractArgs {
        contract_id_preimage: xdr::ContractIdPreimage::Asset(xdr::Asset::Native),
        executable: xdr::ContractExecutable::StellarAsset,
    });
    let token_id: Address = env
        .host()
        .invoke_function(create)
        .unwrap()
        .try_into_val(env)
        .unwrap();
    token_id
}

/// Creates an account ledger entry funded with `balance` lumens, so the
/// native asset contract's transfers have a balance to draw from. Returns
/// the SDK `Address` for the account.
fn create_funded_xlm_account(env: &Env, key: [u8; 32], balance: i64) -> Address {
    use std::rc::Rc;
    let account_id = xdr::AccountId(xdr::PublicKey::PublicKeyTypeEd25519(xdr::Uint256(key)));
    let ledger_key = Rc::new(xdr::LedgerKey::Account(xdr::LedgerKeyAccount {
        account_id: account_id.clone(),
    }));
    let ledger_entry = Rc::new(xdr::LedgerEntry {
        data: xdr::LedgerEntryData::Account(xdr::AccountEntry {
            account_id: account_id.clone(),
            balance,
            flags: 0,
            home_domain: Default::default(),
            inflation_dest: None,
            num_sub_entries: 0,
            seq_num: xdr::SequenceNumber(0),
            thresholds: xdr::Thresholds([1; 4]),
            signers: xdr::VecM::default(),
            ext: xdr::AccountEntryExt::V0,
        }),
        last_modified_ledger_seq: 0,
        ext: xdr::LedgerEntryExt::V0,
    });
    env.host().add_ledger_entry(&ledger_key, &ledger_entry, None).unwrap();
    xdr::ScAddress::Account(account_id).try_into_val(env).unwrap()
}

#[test]
fn native_xlm_sac_is_accepted_as_payment_token() {
    let env = Env::default();
    env.mock_all_auths();

    // The native XLM Stellar Asset Contract is deterministic per network.
    let native_sac = register_native_asset_contract(&env);
    let admin = Address::generate(&env);
    let organizer = create_funded_xlm_account(&env, [1u8; 32], 1_000_000_000);

    let contract_id = env.register(TicketingContract, ());
    let client = TicketingContractClient::new(&env, &contract_id);
    // Initialize probes the native SAC's decimals() — an address that is not
    // a token contract would be rejected here.
    client.initialize(&admin, &native_sac);

    // Issue #233's getter reports the native asset's 7 decimals.
    assert_eq!(client.token_decimals(), 7);
}

#[test]
fn native_xlm_primary_sale_moves_xlm_and_mints_the_ticket() {
    let env = Env::default();
    env.mock_all_auths();

    let native_sac = register_native_asset_contract(&env);
    let native_token = TokenClient::new(&env, &native_sac);

    let admin = Address::generate(&env);
    // Organizer and buyer as funded on-chain accounts holding real XLM.
    let organizer = create_funded_xlm_account(&env, [1u8; 32], 1_000_000_000);
    let buyer = create_funded_xlm_account(&env, [2u8; 32], 5_000_000_000);

    let contract_id = env.register(TicketingContract, ());
    let client = TicketingContractClient::new(&env, &contract_id);
    client.initialize(&admin, &native_sac);
    make_event(&env, &client, &organizer, 1);

    let ticket_id = client.purchase_primary(
        &buyer,
        &1,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &2_000_000_000i128, // 200 XLM
    );

    // Payment moved through the native asset contract.
    assert_eq!(native_token.balance(&organizer), 3_000_000_000); // 100 XLM initial + 200 XLM ticket
    assert_eq!(native_token.balance(&buyer), 3_000_000_000); // 500 XLM initial - 200 XLM ticket

    let ticket = client.verify_ticket(&ticket_id);
    assert_eq!(ticket.owner, buyer);
    assert_eq!(ticket.status, TicketStatus::Valid);
    assert_eq!(ticket.original_price, 2_000_000_000);
}

#[test]
fn native_xlm_resale_settles_atomically() {
    let env = Env::default();
    env.mock_all_auths();

    let native_sac = register_native_asset_contract(&env);
    let native_token = TokenClient::new(&env, &native_sac);

    let admin = Address::generate(&env);
    let organizer = create_funded_xlm_account(&env, [1u8; 32], 1_000_000_000);
    let seller = create_funded_xlm_account(&env, [2u8; 32], 5_000_000_000);
    let buyer = create_funded_xlm_account(&env, [3u8; 32], 5_000_000_000);

    let contract_id = env.register(TicketingContract, ());
    let client = TicketingContractClient::new(&env, &contract_id);
    client.initialize(&admin, &native_sac);
    make_event(&env, &client, &organizer, 1);

    let ticket_id = client.issue_ticket(
        &organizer,
        &1,
        &seller,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &2_000_000_000i128, // 200 XLM face value
    );

    // Resale at face value (cap is 120%); 5% royalty to the organizer.
    client.list_for_resale(&seller, &ticket_id, &2_000_000_000i128);
    client.buy_resale(&buyer, &ticket_id);

    // Royalty 5% of 2 XLM = 0.1 XLM; seller receives 1.9 XLM.
    assert_eq!(native_token.balance(&organizer), 1_000_100_000); // 100.1 XLM
    assert_eq!(native_token.balance(&seller), 4_900_000_000); // 490 XLM
    assert_eq!(native_token.balance(&buyer), 3_000_000_000); // 300 XLM

    let ticket = client.verify_ticket(&ticket_id);
    assert_eq!(ticket.owner, buyer);
    assert_eq!(ticket.status, TicketStatus::Valid);
}

// ── Per-event accepted payment token (issue #235) ───────────────────────────

#[test]
fn organizer_sets_a_per_event_payment_token_before_sales() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);

    let other_admin = Address::generate(&env);
    let other_token = env.register_stellar_asset_contract_v2(other_admin);

    client.set_event_payment_token(&organizer, &1, &Some(other_token.address()));

    // The event settles in its own token; the contract-wide one is untouched.
    assert_eq!(
        client.event_payment_token(&1),
        other_token.address()
    );
}

#[test]
fn event_without_override_uses_the_contract_wide_token() {
    let (env, client, token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);

    // No per-event override: resolution falls back to the global token.
    assert_eq!(client.event_payment_token(&1), token.address().unwrap());

    // Clearing an unset override is a no-op.
    client.set_event_payment_token(&organizer, &1, &None);
    assert_eq!(client.event_payment_token(&1), token.address().unwrap());
}

#[test]
fn event_with_override_settles_purchases_in_its_own_token() {
    let (env, client, global_token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);

    let other_admin = Address::generate(&env);
    let event_token_contract = env.register_stellar_asset_contract_v2(other_admin);
    let event_token = TokenClient::new(&env, &event_token_contract.address());
    let token_asset = StellarAssetClient::new(&env, &event_token_contract.address());

    client.set_event_payment_token(&organizer, &1, &Some(event_token_contract.address()));

    let buyer = Address::generate(&env);
    token_asset.mint(&buyer, &5_000i128);
    global_token.mint(&buyer, &5_000i128);

    let ticket_id = client.purchase_primary(
        &buyer,
        &1,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &2_000i128,
    );

    // Payment moved in the per-event token, not the contract-wide one.
    assert_eq!(event_token.balance(&buyer), 3_000);
    assert_eq!(global_token.balance(&buyer), 5_000);

    let ticket = client.verify_ticket(&ticket_id);
    assert_eq!(ticket.owner, buyer);
    assert_eq!(ticket.original_price, 2_000);
}

#[test]
fn non_organizer_cannot_set_the_event_payment_token() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);

    let impostor = Address::generate(&env);
    let result = client.try_set_event_payment_token(&impostor, &1, &None);
    assert_eq!(result, Err(Ok(Error::NotOrganizer)));
}

#[test]
fn event_payment_token_cannot_change_after_tickets_are_issued() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);
    let buyer = Address::generate(&env);
    client.issue_ticket(
        &organizer,
        &1,
        &buyer,
        &String::from_str(&env, "GA"),
        &String::from_str(&env, "unassigned"),
        &1_000i128,
    );

    let other_admin = Address::generate(&env);
    let other_token = env.register_stellar_asset_contract_v2(other_admin);

    let result = client.try_set_event_payment_token(&organizer, &1, &Some(other_token.address()));
    assert_eq!(result, Err(Ok(Error::TicketsAlreadyIssued)));
}

#[test]
fn event_payment_token_rejects_a_non_token_address() {
    let (env, client, _token, _token_asset, _admin, organizer) = setup();
    make_event(&env, &client, &organizer, 1);

    let not_a_token = Address::generate(&env);
    let result = client.try_set_event_payment_token(&organizer, &1, &Some(not_a_token));
    assert_eq!(result, Err(Ok(Error::InvalidPaymentToken)));
}
