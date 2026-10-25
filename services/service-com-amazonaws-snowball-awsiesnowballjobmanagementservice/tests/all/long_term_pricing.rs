// SPDX-License-Identifier: CC0-1.0

//! `CreateLongTermPricing`, driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE of requests rather than over a single call: every
//! accepted create mints a fresh `LongTermPricingId` in the modelled form and persists a
//! contract under it carrying the term and device type the request named, the auto-renew
//! choice it made (or the "do not renew" default it left), the instant of the call as the
//! contract's start and that instant plus the whole calendar months the
//! `LongTermPricingType` names as its end; a request naming a value outside one of the two
//! modelled enum catalogs, or past this emulator's store ceiling, is refused with HTTP 400
//! and persists nothing.
//!
//! The rows differ along what the rule generalises over: the `LongTermPricingType` (every
//! value of the modelled catalog, and so every contract term), the `SnowballType` (every
//! value of the modelled catalog), and whether `IsLongTermPricingAutoRenew` is absent,
//! `false` or `true`.
//!
//! Every call goes through the same front door a client uses — the `awsJson1_1` router
//! `job_lifecycle` drives the service with. A bought contract is read back the way a client
//! reads one: by quoting its id on `CreateJob`, which the service accepts only for a
//! contract it holds and reports back through `DescribeJob`. Its own stored fields — the
//! term window, the status and the auto-renew choice — are also checked off the engine
//! directly, which pins what the create persisted independently of any reader; the operation
//! that reports those members to a client is `ListLongTermPricing`, whose own suite
//! (`long_term_pricing_listing`) asserts them over the wire.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
// The term table this operation reads a contract's length from, and the catalogs the
// modelled enums admit, taken from the crate rather than restated, so a row cannot drift
// from the values the model allows.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::create_long_term_pricing::LONG_TERM_PRICING_TERM_MONTHS;
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::{
    LONG_TERM_PRICING_TYPE_VALUES, SNOWBALL_TYPE_VALUES,
};
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::{
    service_state, StoredLongTermPricing, LONG_TERM_PRICING_STATUS_ACTIVE,
    MAX_STORED_LONG_TERM_PRICING,
};
use smithy_server_runtime::MultiServiceRouter;

use super::cluster_creation::UNKNOWN_LONG_TERM_PRICING_ID;
use super::cluster_updates::job_metadata;
use super::job_lifecycle::{
    assert_prefixed_uuid, call_in_region, complete_order, create_address, deliverable_address, ok,
    refused, string,
};

/// The region every request in this suite signs for. A long-term pricing contract is
/// bought for a term and a device type, neither of which the Region bears on, so the
/// Region is fixed here rather than varied.
const SIGNED_REGION: &str = "us-west-2";

/// The prefix the modelled `LongTermPricingId` pattern requires
/// (`^LTPID[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`, length 41..41,
/// `smithy_query(query="operation", operation="CreateLongTermPricing")`).
const PRICING_ID_PREFIX: &str = "LTPID";

/// The exact length the modelled `LongTermPricingId` admits.
const PRICING_ID_LEN: usize = 41;

/// The wire error code a refusal this operation models no error for carries.
const VALIDATION_ERROR: &str = "ValidationException";

/// Each `LongTermPricingType`, the term in whole calendar months it names, and the number
/// of days that term can span.
///
/// The months are what the handler's `LONG_TERM_PRICING_TERM_MONTHS` says the type names
/// (asserted against it below, so the two cannot drift). The day spans are the calendar's
/// own: one calendar month is 28 to 31 days; twelve are 365 or 366, depending on whether a
/// leap day falls inside the term; thirty-six are 1095 or 1096, for the same reason.
const TERM_DAY_SPANS: &[(&str, u32, u64, u64)] = &[
    ("OneMonth", 1, 28, 31),
    ("OneYear", 12, 365, 366),
    ("ThreeYear", 36, 1095, 1096),
];

/// A router with just this service mounted, plus the engine it was built on.
///
/// The engine is handed back so a case can read the contract a create persisted: see the
/// module docs for why the store, rather than a reader operation, is what observes it.
fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let shared: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not what these tests are about; the router still routes by the SigV4
    // credential scope the calls below carry.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::EmulatedService::new(
            Arc::clone(&shared),
        ),
    );
    (router, shared)
}

/// One `CreateLongTermPricing` call that must succeed, returning the id it minted.
async fn create_pricing(router: &MultiServiceRouter, body: Value, case: &str) -> String {
    let created = ok(router, "CreateLongTermPricing", body, case).await;
    let pricing_id = string(&created, "LongTermPricingId", case);
    assert_prefixed_uuid(&pricing_id, PRICING_ID_PREFIX, case);
    assert_eq!(
        pricing_id.len(),
        PRICING_ID_LEN,
        "case {case:?}: expected a {PRICING_ID_LEN}-character LongTermPricingId, got \
         {pricing_id:?}"
    );
    pricing_id
}

/// The contract stored under `pricing_id`, panicking when the create persisted none.
fn stored_contract(
    shared: &engine::InMemoryEngine,
    pricing_id: &str,
    case: &str,
) -> StoredLongTermPricing {
    let state = service_state(shared)
        .unwrap_or_else(|| panic!("case {case:?}: this service's store is on the engine"));
    let store = state.lock();
    store
        .long_term_pricing
        .get(pricing_id)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a contract stored under {pricing_id:?}, got the ids \
                 {:?}",
                store.long_term_pricing_order
            )
        })
}

/// How many contracts the store holds, and the ids it holds them under in creation order.
fn stored_ids(shared: &engine::InMemoryEngine, case: &str) -> Vec<String> {
    let state = service_state(shared)
        .unwrap_or_else(|| panic!("case {case:?}: this service's store is on the engine"));
    let store = state.lock();
    store.long_term_pricing_order.clone()
}

/// Every contract row: each `LongTermPricingType` the modelled catalog admits against each
/// `SnowballType` it admits, with `IsLongTermPricingAutoRenew` cycling through absent,
/// `false` and `true` so every form of the member meets several terms and device types.
///
/// Each row carries the auto-renew the request asked for, and the value the stored
/// contract must answer with — absent means "do not renew".
fn contract_rows() -> Vec<(String, Value, bool)> {
    let renewals = [None, Some(false), Some(true)];
    let mut rows = Vec::new();
    for (device_index, snowball_type) in SNOWBALL_TYPE_VALUES.iter().enumerate() {
        for (term_index, pricing_type) in LONG_TERM_PRICING_TYPE_VALUES.iter().enumerate() {
            let asked = renewals[(device_index + term_index) % renewals.len()];
            let mut body = json!({
                "LongTermPricingType": pricing_type,
                "SnowballType": snowball_type,
            });
            if let Some(auto_renew) = asked {
                body["IsLongTermPricingAutoRenew"] = json!(auto_renew);
            }
            rows.push((
                format!(
                    "{pricing_type} on {snowball_type} with auto-renew {}",
                    asked.map_or_else(|| "absent".to_string(), |flag| flag.to_string())
                ),
                body,
                asked.unwrap_or(false),
            ));
        }
    }
    rows
}

/// The term `pricing_type` names, and the days it may span, from [`TERM_DAY_SPANS`].
fn term_of(pricing_type: &str) -> (u32, u64, u64) {
    TERM_DAY_SPANS
        .iter()
        .find(|(name, ..)| *name == pricing_type)
        .map(|(_, months, min_days, max_days)| (*months, *min_days, *max_days))
        .unwrap_or_else(|| panic!("the term table covers {pricing_type:?}"))
}

#[test]
fn the_term_table_spells_exactly_the_modelled_pricing_types() {
    let mut termed: Vec<&str> = LONG_TERM_PRICING_TERM_MONTHS
        .iter()
        .map(|(pricing_type, _)| *pricing_type)
        .collect();
    termed.sort_unstable();
    let mut modelled: Vec<&str> = LONG_TERM_PRICING_TYPE_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        termed, modelled,
        "case \"the term table\": expected a term for exactly the modelled \
         LongTermPricingType values {modelled:?}, got {termed:?}"
    );

    for (pricing_type, months) in LONG_TERM_PRICING_TERM_MONTHS {
        let (expected_months, ..) = term_of(pricing_type);
        assert_eq!(
            *months, expected_months,
            "case {pricing_type:?}: expected a term of {expected_months} calendar months, got \
             {months}"
        );
    }
}

#[tokio::test]
async fn create_long_term_pricing_persists_every_contract_it_mints() {
    let (router, shared) = router_with_engine();
    let mut minted: BTreeSet<String> = BTreeSet::new();
    let mut created_in_order: Vec<String> = Vec::new();

    for (case, body, expected_auto_renew) in contract_rows() {
        let pricing_type = string(&body, "LongTermPricingType", &case);
        let snowball_type = string(&body, "SnowballType", &case);

        // The contract starts when it is bought, so the call is bracketed and the stored
        // start date must fall inside that window rather than on any fixed instant.
        let before = SystemTime::now();
        let pricing_id = create_pricing(&router, body, &case).await;
        let after = SystemTime::now();

        assert!(
            minted.insert(pricing_id.clone()),
            "case {case:?}: expected a LongTermPricingId no earlier create minted, got \
             {pricing_id:?} again"
        );
        created_in_order.push(pricing_id.clone());

        let contract = stored_contract(&shared, &pricing_id, &case);
        assert_eq!(
            contract.long_term_pricing_id, pricing_id,
            "case {case:?}: expected the contract keyed by the minted id, got {contract:?}"
        );
        assert_eq!(
            contract.long_term_pricing_type, pricing_type,
            "case {case:?}: expected the term the request asked for, got {contract:?}"
        );
        assert_eq!(
            contract.snowball_type, snowball_type,
            "case {case:?}: expected the device type the request asked for, got {contract:?}"
        );
        assert_eq!(
            contract.is_long_term_pricing_auto_renew, expected_auto_renew,
            "case {case:?}: expected auto-renew {expected_auto_renew}, got {contract:?}"
        );
        assert_eq!(
            contract.long_term_pricing_status, LONG_TERM_PRICING_STATUS_ACTIVE,
            "case {case:?}: expected a contract the service is holding to, got {contract:?}"
        );
        assert!(
            contract.long_term_pricing_start_date >= before
                && contract.long_term_pricing_start_date <= after,
            "case {case:?}: expected a start date at the instant of the call, got {contract:?}"
        );

        let (months, min_days, max_days) = term_of(&pricing_type);
        let term = contract
            .long_term_pricing_end_date
            .duration_since(contract.long_term_pricing_start_date)
            .unwrap_or_else(|_| {
                panic!("case {case:?}: expected an end date after the start, got {contract:?}")
            });
        let days = term.as_secs() / 86_400;
        assert!(
            (min_days..=max_days).contains(&days),
            "case {case:?}: expected a term of {months} calendar months ({min_days} to \
             {max_days} days), got {days} days from {contract:?}"
        );
    }

    assert_eq!(
        stored_ids(&shared, "every row"),
        created_in_order,
        "case \"every row\": expected the store to hold exactly the contracts created, in \
         creation order"
    );
}

#[tokio::test]
async fn every_create_mints_a_new_contract_for_the_same_request() {
    let (router, shared) = router_with_engine();
    // One request submitted repeatedly: a contract is the service's own record, so each
    // call buys a new one rather than keying on, or reusing, the values submitted.
    let body = json!({
        "LongTermPricingType": "OneYear",
        "SnowballType": "EDGE_S",
        "IsLongTermPricingAutoRenew": true,
    });
    let repeats = 4usize;
    let mut minted: BTreeSet<String> = BTreeSet::new();

    for index in 0..repeats {
        let case = format!("repeat {index} of {repeats}");
        let pricing_id = create_pricing(&router, body.clone(), &case).await;
        assert!(
            minted.insert(pricing_id.clone()),
            "case {case:?}: expected a fresh LongTermPricingId, got {pricing_id:?} again"
        );
        let contract = stored_contract(&shared, &pricing_id, &case);
        assert_eq!(
            contract.long_term_pricing_type, "OneYear",
            "case {case:?}: expected the submitted term, got {contract:?}"
        );
    }

    assert_eq!(
        stored_ids(&shared, "repeated creates").len(),
        repeats,
        "case \"repeated creates\": expected one stored contract per call"
    );
}

#[tokio::test]
async fn a_bought_contract_is_one_an_order_can_be_billed_under() {
    // The contract a create persists is the record the rest of the service reads: an order
    // may be billed under a `LongTermPricingId` exactly when this store holds a contract
    // under it, so a bought one is accepted and reported back while an id of the same form
    // that no create minted is refused. The rows span every term the modelled catalog
    // admits, so the rule holds of a contract of any term and not just of one.
    let (router, _shared) = router_with_engine();
    let address_case = "the address every order below ships to";
    let address_id = create_address(
        &router,
        deliverable_address("Contract Receiver", "7 Contract Way", json!({})),
        address_case,
    )
    .await;

    for pricing_type in LONG_TERM_PRICING_TYPE_VALUES {
        let case = format!("an order billed under a {pricing_type} contract");
        let pricing_id = create_pricing(
            &router,
            json!({ "LongTermPricingType": pricing_type, "SnowballType": "EDGE" }),
            &case,
        )
        .await;
        let order = complete_order(&json!({
            "AddressId": address_id,
            "LongTermPricingId": pricing_id,
        }));
        let created = ok(&router, "CreateJob", order, &case).await;
        let job_id = string(&created, "JobId", &case);
        let metadata = job_metadata(&router, &job_id, &case).await;
        assert_eq!(
            metadata.get("LongTermPricingId").and_then(Value::as_str),
            Some(pricing_id.as_str()),
            "case {case:?}: expected the order to report the contract it is billed under, got \
             {metadata}"
        );
    }

    let case = "an order billed under an id no create minted";
    let order = complete_order(&json!({
        "AddressId": address_id,
        "LongTermPricingId": UNKNOWN_LONG_TERM_PRICING_ID,
    }));
    refused(
        &router,
        "CreateJob",
        order,
        "InvalidResourceException",
        case,
    )
    .await;
}

#[tokio::test]
async fn create_long_term_pricing_refuses_a_value_outside_a_modelled_catalog() {
    let (router, shared) = router_with_engine();

    // Each row names a value no modelled catalog value spells — a term that does not
    // exist, a device type that does not exist, a catalog value in the wrong case, and one
    // with stray whitespace — so there is no contract to buy and nothing to store.
    for (case, body) in [
        (
            "a term no LongTermPricingType value spells",
            json!({ "LongTermPricingType": "OneDecade", "SnowballType": "EDGE" }),
        ),
        (
            "a LongTermPricingType value in the wrong case",
            json!({ "LongTermPricingType": "oneyear", "SnowballType": "EDGE" }),
        ),
        (
            "a LongTermPricingType value with stray whitespace",
            json!({ "LongTermPricingType": "OneYear ", "SnowballType": "EDGE" }),
        ),
        (
            "a device no SnowballType value spells",
            json!({ "LongTermPricingType": "OneYear", "SnowballType": "EDGE_X" }),
        ),
        (
            "a SnowballType value in the wrong case",
            json!({ "LongTermPricingType": "OneYear", "SnowballType": "edge_s" }),
        ),
        (
            "an empty SnowballType",
            json!({ "LongTermPricingType": "OneYear", "SnowballType": "" }),
        ),
    ] {
        refused(
            &router,
            "CreateLongTermPricing",
            body,
            VALIDATION_ERROR,
            case,
        )
        .await;
    }

    // A request missing a member the model marks required is refused at the boundary,
    // before the handler runs, so only the status is asserted for these rows.
    for (case, body) in [
        ("neither required member", json!({})),
        ("no LongTermPricingType", json!({ "SnowballType": "EDGE" })),
        ("no SnowballType", json!({ "LongTermPricingType": "OneYear" })),
    ] {
        let (status, parsed) =
            call_in_region(&router, SIGNED_REGION, "CreateLongTermPricing", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} with {parsed}"
        );
    }

    assert!(
        stored_ids(&shared, "every refused row").is_empty(),
        "case \"every refused row\": expected a refused create to persist nothing, got {:?}",
        stored_ids(&shared, "every refused row")
    );
}

#[tokio::test]
async fn a_create_past_the_store_ceiling_is_refused() {
    let (router, shared) = router_with_engine();
    let body = json!({
        "LongTermPricingType": "OneMonth",
        "SnowballType": "STANDARD",
    });

    // Fill the store to its ceiling, read from the crate rather than restated, then assert
    // the request one past it is refused rather than accepted.
    for index in 0..MAX_STORED_LONG_TERM_PRICING {
        let case = format!("contract {index} of {MAX_STORED_LONG_TERM_PRICING}");
        create_pricing(&router, body.clone(), &case).await;
    }
    refused(
        &router,
        "CreateLongTermPricing",
        body,
        VALIDATION_ERROR,
        "one contract past the ceiling",
    )
    .await;

    assert_eq!(
        stored_ids(&shared, "a full store").len(),
        MAX_STORED_LONG_TERM_PRICING,
        "case \"a full store\": expected the store to hold exactly its ceiling"
    );
}
