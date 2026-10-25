// SPDX-License-Identifier: CC0-1.0

//! `ListGroundStations` (`GET /groundstation`), end to end over HTTP through the same front
//! door as the rest of this target's suites.
//!
//! The RULE under test: a listing REPORTS the store it reads, so `ListGroundStations`
//! answers exactly the ground stations this account is served by — those its own inventory
//! records and those the satellites it is on-boarded to are on-boarded to, each name once —
//! as one `GroundStationData` per station carrying that one name as both `groundStationId`
//! and `groundStationName` plus the Region the request addressed, in a stable order, a page
//! at a time; a `satelliteId` narrows the answer to the stations that satellite is
//! on-boarded to, and one naming no onboarding of this account is refused as the missing
//! resource it names rather than answered with an empty page.
//!
//! So the suites below assert that rule over SETS of inputs rather than over one call:
//!
//! * a table of accounts differing in where their ground stations are recorded and how
//!   many (none at all, inventory only, onboardings only, both with a station recorded in
//!   each, a name repeated inside one onboarding), including each published bound of
//!   `GroundStationName`'s length;
//! * paging over a table of page sizes — dividing the sequence evenly, unevenly, exactly,
//!   and larger than the inventory — each of which must yield every ground station exactly
//!   once in the listing's order, with `maxResults` asserted on each published bound of
//!   `PaginationMaxResults` and one value past each;
//! * the `satelliteId` filter over a table of onboardings (several stations, one, none),
//!   beside ids no onboarding carries and ids the modelled `Uuid` refuses before the
//!   handler runs;
//! * a `nextToken` this listing did not issue — including one it issued under a DIFFERENT
//!   filter — refused rather than resumed.
//!
//! A ground station is not created through this API (the model declares no operation that
//! builds one), so the inventory is seeded into this service's own state — exactly where
//! `state::GROUND_STATIONS_ENV` and `state::SATELLITES_ENV` land it — and then read back
//! through the front door.

use std::collections::HashMap;

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    parse_ground_stations, parse_satellites, satellite_not_found_message, service_state, Satellite,
    BAD_REQUEST_ERROR_CODE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::scheduling_loop::{harness_with_engine, list_pages, send, ABSENT_UUID, REGION};

/// This operation's route.
const ROUTE: &str = "/groundstation";

/// The shortest name a ground station can carry: `GroundStationName` is 4..97 characters
/// (`smithy_query(query="shape", shape="GroundStationName")`).
const SHORTEST_STATION: &str = "Ohio";

/// The longest name a ground station can carry, per the same bound.
fn longest_station() -> String {
    "S".repeat(97)
}

/// Record the ground stations `setting` describes in this service's store.
fn describe(engine: &engine::InMemoryEngine, setting: &str) {
    let inventory = parse_ground_stations(setting);
    let state = service_state(engine).expect("this service's state store");
    state.lock().ground_stations = inventory;
}

/// Onboard the satellites `setting` describes, so the ground stations they are on-boarded
/// to are ones this account is served by even where the inventory records none.
fn onboard(engine: &engine::InMemoryEngine, setting: &str) -> HashMap<String, Satellite> {
    let catalog = parse_satellites(setting);
    let state = service_state(engine).expect("this service's state store");
    state.lock().satellites = catalog.clone();
    catalog
}

/// The onboarding of the spacecraft whose NORAD catalogue number is `norad`.
fn onboarding(catalog: &HashMap<String, Satellite>, norad: i32) -> &Satellite {
    catalog
        .values()
        .find(|satellite| satellite.norad_satellite_id == norad)
        .unwrap_or_else(|| panic!("the setting should onboard NORAD {norad}, got {catalog:?}"))
}

/// Every ground station `ListGroundStations` reports under `query`, across as many pages as
/// it takes, in the order the listing gave them.
async fn list_all(router: &MultiServiceRouter, query: &str, page_size: Option<i64>) -> Vec<Value> {
    let path = if query.is_empty() {
        ROUTE.to_string()
    } else {
        format!("{ROUTE}?{query}")
    };
    list_pages(router, &path, "groundStationList", page_size).await
}

/// A JSON member of `item`, as a string (empty when absent, so a missing member fails the
/// comparison it is part of rather than panicking here).
fn member(item: &Value, name: &str) -> String {
    item.get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `groundStationId` of each item, in the order the listing gave them.
fn reported(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .map(|item| member(item, "groundStationId"))
        .collect()
}

/// `names` in the order `ListGroundStations` must report them: by name, each name once.
fn in_listing_order(names: &[&str]) -> Vec<String> {
    let mut ordered: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
    ordered.sort();
    ordered.dedup();
    ordered
}

/// Assert every member of one reported ground station.
fn assert_item(case: &str, item: &Value, name: &str) {
    assert_eq!(
        member(item, "groundStationId"),
        name,
        "case {case}: a station is reported under the name it is recorded by, got {item}"
    );
    // The id and the name are ONE name in this API: both members target
    // `GroundStationName` (`smithy_query(query="shape", shape="GroundStationData")`).
    assert_eq!(
        member(item, "groundStationName"),
        name,
        "case {case}: groundStationName must equal groundStationId, got {item}"
    );
    assert!(
        (4..=97).contains(&name.len()),
        "case {case}: a GroundStationName is 4..97 characters, got {} in {name:?}",
        name.len()
    );
    // The Region of a ground station is the Region the request addressed — the one the
    // caller's credential scope names — and is spellable as an `AWSRegion` (`^[\w-]+$`,
    // len 1..128 — `smithy_query(query="shape", shape="AWSRegion")`).
    let region = member(item, "region");
    assert_eq!(
        region, REGION,
        "case {case}: expected the Region the request addressed, got {item}"
    );
    assert!(
        (1..=128).contains(&region.len())
            && region
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
        "case {case}: an AWSRegion is 1..128 characters of [\\w-], got {region:?}"
    );
}

/// One row of the account table: where this account's ground stations are recorded, and the
/// stations the listing must then report.
struct Account {
    case: &'static str,
    inventory: String,
    onboardings: String,
    serves: Vec<String>,
}

impl Account {
    fn new(case: &'static str, inventory: &str, onboardings: &str, serves: &[&str]) -> Self {
        Self {
            case,
            inventory: inventory.to_string(),
            onboardings: onboardings.to_string(),
            serves: in_listing_order(serves),
        }
    }
}

#[tokio::test]
async fn list_ground_stations_reports_exactly_the_stations_this_account_is_served_by() {
    let longest = longest_station();
    let rows = vec![
        Account::new("an account told about none", "", "", &[]),
        Account::new(
            "its own inventory only",
            "Oregon 1=Antenna 1,Ohio 1",
            "",
            &["Ohio 1", "Oregon 1"],
        ),
        Account::new(
            "onboardings only",
            "",
            "25994=Punta Arenas 1+Dubbo 1",
            &["Dubbo 1", "Punta Arenas 1"],
        ),
        Account::new(
            "an onboarding that reaches no ground station",
            "Ohio 1",
            "25994",
            &["Ohio 1"],
        ),
        Account::new(
            "a station recorded in both places, and one named twice in an onboarding",
            "Ohio 1=Antenna 1,Oregon 1",
            "25994=Ohio 1+Punta Arenas 1+Punta Arenas 1,33333=Ohio 1",
            &["Ohio 1", "Oregon 1", "Punta Arenas 1"],
        ),
        // Each published bound of `GroundStationName`'s length, recorded one in each place
        // so neither source is the only one that carries a bound.
        Account::new(
            "names at both published length bounds",
            SHORTEST_STATION,
            &format!("25994={longest}"),
            &[SHORTEST_STATION, &longest],
        ),
    ];

    for row in &rows {
        let (router, engine) = harness_with_engine();
        describe(&engine, &row.inventory);
        onboard(&engine, &row.onboardings);
        let case = row.case;

        let reply = send(&router, "GET", ROUTE, None).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected ListGroundStations 200, got {} {}",
            reply.status, reply.body
        );
        assert!(
            reply.field("nextToken").is_null(),
            "case {case}: a listing with nothing more to come carries no nextToken, got {}",
            reply.body
        );
        if row.serves.is_empty() {
            assert_eq!(
                reply.field("groundStationList"),
                &json!([]),
                "case {case}: an account served by no ground station must list none, got {}",
                reply.body
            );
        }

        let items = list_all(&router, "", None).await;
        assert_eq!(
            reported(&items),
            row.serves,
            "case {case}: expected the recorded stations in name order, got {:?}",
            reported(&items)
        );
        for (item, name) in items.iter().zip(&row.serves) {
            assert_item(case, item, name);
        }

        // A listing is a REPORT: reading it twice reports the same store.
        assert_eq!(
            items,
            list_all(&router, "", None).await,
            "case {case}: a listing must not change what the next caller sees"
        );
    }
}

#[tokio::test]
async fn list_ground_stations_pages_through_every_station_once_whatever_the_page_size() {
    let (router, engine) = harness_with_engine();
    // Six ground stations, so a page size can divide the sequence evenly, unevenly,
    // exactly, or not bound it at all.
    describe(&engine, "Ohio 1,Oregon 1,Dubbo 1,Hawaii 1=Antenna 1");
    onboard(&engine, "25994=Punta Arenas 1+Bahrain 1");
    let expected = in_listing_order(&[
        "Bahrain 1",
        "Dubbo 1",
        "Hawaii 1",
        "Ohio 1",
        "Oregon 1",
        "Punta Arenas 1",
    ]);

    let whole = reported(&list_all(&router, "", None).await);
    assert_eq!(
        whole, expected,
        "case whole-listing: expected every recorded station, got {whole:?}"
    );

    for size in [1_i64, 2, 3, 4, 5, 6, 7, 100] {
        let paged = reported(&list_all(&router, "", Some(size)).await);
        assert_eq!(
            paged, whole,
            "case maxResults={size}: paging must yield every station exactly once, in the \
             listing's order, got {paged:?}"
        );
    }

    // Each published bound of `PaginationMaxResults` and one value past it (source:
    // `smithy_query(query="operation", operation="ListGroundStations")` — range 1..100). A
    // value the model refuses never reaches the handler: this service's validator answers
    // it.
    for (case, size, accepted) in [
        ("the lower bound", 1_i64, true),
        ("the upper bound", 100, true),
        ("one below the lower bound", 0, false),
        ("one above the upper bound", 101, false),
    ] {
        let reply = send(&router, "GET", &format!("{ROUTE}?maxResults={size}"), None).await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case maxResults={size} ({case}): expected 200, got {} {}",
                reply.status, reply.body
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case maxResults={size} ({case}): expected a refusal, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                BAD_REQUEST_ERROR_CODE,
                "case maxResults={size} ({case}): expected {BAD_REQUEST_ERROR_CODE}, got {}",
                reply.body
            );
        }
    }
}

#[tokio::test]
async fn a_satellite_filter_reports_exactly_the_stations_that_satellite_is_on_boarded_to() {
    let (router, engine) = harness_with_engine();
    // An inventory station no onboarding reaches, so a filtered answer is narrower than the
    // whole listing rather than the same answer under another name.
    describe(&engine, "Ohio 1=Antenna 1,Oregon 1");
    let catalog = onboard(
        &engine,
        "25994=Punta Arenas 1+Ohio 1+Punta Arenas 1,33333=Dubbo 1,44444",
    );

    // Rows differ in how many ground stations the onboarding reaches: several (named out of
    // order and with a repeat, so the answer's order and uniqueness are the handler's and
    // not the setting's), one, and none at all.
    let rows = [
        (
            "several stations",
            25994,
            in_listing_order(&["Ohio 1", "Punta Arenas 1"]),
        ),
        ("one station", 33333, in_listing_order(&["Dubbo 1"])),
        ("no station", 44444, Vec::new()),
    ];

    for (case, norad, expected) in rows {
        let satellite_id = onboarding(&catalog, norad).satellite_id.clone();
        let query = format!("satelliteId={satellite_id}");

        let reply = send(&router, "GET", &format!("{ROUTE}?{query}"), None).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected ListGroundStations 200, got {} {}",
            reply.status, reply.body
        );
        if expected.is_empty() {
            assert_eq!(
                reply.field("groundStationList"),
                &json!([]),
                "case {case}: a satellite on-boarded to no ground station lists none, got {}",
                reply.body
            );
        }

        let items = list_all(&router, &query, None).await;
        assert_eq!(
            reported(&items),
            expected,
            "case {case}: expected the stations this onboarding reaches, got {:?}",
            reported(&items)
        );
        for (item, name) in items.iter().zip(&expected) {
            assert_item(case, item, name);
        }

        // Paging the filtered sequence yields the same stations, so the filter and the
        // page boundary compose.
        assert_eq!(
            reported(&list_all(&router, &query, Some(1)).await),
            expected,
            "case {case}: paging the filtered listing must yield the same stations"
        );
    }

    // The unfiltered listing is the union, so the rows above narrowed it rather than
    // replacing it.
    assert_eq!(
        reported(&list_all(&router, "", None).await),
        in_listing_order(&["Dubbo 1", "Ohio 1", "Oregon 1", "Punta Arenas 1"]),
        "case unfiltered: expected every station this account is served by"
    );
}

#[tokio::test]
async fn a_satellite_filter_naming_no_onboarding_of_this_account_is_refused() {
    let (router, engine) = harness_with_engine();
    describe(&engine, "Ohio 1=Antenna 1");
    let catalog = onboard(&engine, "25994=Punta Arenas 1");
    let known = onboarding(&catalog, 25994).satellite_id.clone();

    // Rows differ along WHY no onboarding carries the id: one nothing records, and one that
    // differs from a known id in a single character.
    let absent_tail: String = format!("{}f", &known[..known.len() - 1]);
    let flipped = if absent_tail == known {
        format!("{}0", &known[..known.len() - 1])
    } else {
        absent_tail
    };
    for (case, satellite_id) in [
        ("an id nothing records", ABSENT_UUID.to_string()),
        ("an id one character from a known one", flipped),
    ] {
        let reply = send(
            &router,
            "GET",
            &format!("{ROUTE}?satelliteId={satellite_id}"),
            None,
        )
        .await;
        assert_eq!(
            reply.status, 434,
            "case {case} ({satellite_id}): expected 434, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ResourceNotFoundException",
            "case {case} ({satellite_id}): wrong error shape, got {}",
            reply.body
        );
        assert_eq!(
            member(&reply.body, "message"),
            satellite_not_found_message(&satellite_id),
            "case {case}: the refusal must name the id it was given, got {}",
            reply.body
        );
    }

    // And a `satelliteId` the modelled `Uuid` refuses never reaches the handler: it is
    // 36 characters of `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`
    // (`smithy_query(query="operation", operation="ListGroundStations")`), so a value past
    // that length and one inside it but outside the pattern are refused by the validator.
    for (case, satellite_id) in [
        ("shorter than the modelled length", "0000".to_string()),
        ("longer than the modelled length", format!("{ABSENT_UUID}0")),
        (
            "inside the length but outside the pattern",
            "AAAAAAAA-aaaa-aaaa-aaaa-aaaaaaaaaaaa".to_string(),
        ),
    ] {
        let reply = send(
            &router,
            "GET",
            &format!("{ROUTE}?satelliteId={satellite_id}"),
            None,
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {case} ({satellite_id}): expected the modelled refusal, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            BAD_REQUEST_ERROR_CODE,
            "case {case} ({satellite_id}): expected {BAD_REQUEST_ERROR_CODE}, got {}",
            reply.body
        );
    }

    // The known onboarding still answers, so the refusals above are about the id and not
    // about the filter.
    let reply = send(&router, "GET", &format!("{ROUTE}?satelliteId={known}"), None).await;
    assert_eq!(
        reply.status, 200,
        "case known-satellite: expected 200, got {} {}",
        reply.status, reply.body
    );
}

#[tokio::test]
async fn list_ground_stations_refuses_a_next_token_it_did_not_issue() {
    let (router, engine) = harness_with_engine();
    describe(&engine, "Ohio 1,Oregon 1,Dubbo 1");
    let catalog = onboard(&engine, "25994=Punta Arenas 1+Bahrain 1,33333");
    let satellite_id = onboarding(&catalog, 25994).satellite_id.clone();
    let filter = format!("satelliteId={satellite_id}");

    /// The `nextToken` a capped page of `query` hands back.
    async fn token(router: &MultiServiceRouter, query: &str) -> String {
        let separator = if query.is_empty() { "" } else { "&" };
        send(
            router,
            "GET",
            &format!("{ROUTE}?{query}{separator}maxResults=1"),
            None,
        )
        .await
        .field("nextToken")
        .as_str()
        .map(str::to_string)
        .expect("a capped page hands back a token")
    }

    let unfiltered_token = token(&router, "").await;
    let filtered_token = token(&router, &filter).await;

    let resumed = send(
        &router,
        "GET",
        &format!("{ROUTE}?maxResults=1&nextToken={unfiltered_token}"),
        None,
    )
    .await;
    assert_eq!(
        resumed.status, 200,
        "case own-token: a token this listing issued must resume it, got {} {}",
        resumed.status, resumed.body
    );

    let satellites_token = send(&router, "GET", "/satellite?maxResults=1", None)
        .await
        .field("nextToken")
        .as_str()
        .map(str::to_string)
        .expect("a capped page of satellites hands back a token");

    // A token addresses an offset into the sequence that issued it, so this listing accepts
    // only its own — under THESE filters — and refuses anything else rather than resuming
    // at an offset into an unrelated sequence.
    let refused: Vec<(&str, String, String)> = vec![
        (
            "a token this service never issued",
            "not-a-token".to_string(),
            String::new(),
        ),
        (
            "a token naming no offset",
            "ground-stations.not-a-number".to_string(),
            String::new(),
        ),
        (
            "a token of this listing without its filter scope",
            "ground-stations.1".to_string(),
            String::new(),
        ),
        (
            "a token from another listing",
            satellites_token,
            String::new(),
        ),
        (
            "this listing's unfiltered token under a satellite filter",
            unfiltered_token.clone(),
            filter.clone(),
        ),
        (
            "this listing's filtered token with no filter",
            filtered_token,
            String::new(),
        ),
    ];

    for (case, token, query) in refused {
        let separator = if query.is_empty() { "" } else { "&" };
        let reply = send(
            &router,
            "GET",
            &format!("{ROUTE}?{query}{separator}maxResults=1&nextToken={token}"),
            None,
        )
        .await;
        assert_eq!(
            reply.status, 431,
            "case {case} ({token}): expected 431, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterException",
            "case {case} ({token}): wrong error shape, got {}",
            reply.body
        );
        assert_eq!(
            member(&reply.body, "parameterName"),
            "nextToken",
            "case {case} ({token}): the refusal must name the parameter at fault, got {}",
            reply.body
        );
    }
}
