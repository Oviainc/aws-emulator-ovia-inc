// SPDX-License-Identifier: CC0-1.0

//! `ListThreatEntitySets`, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. `ListThreatEntitySets` names exactly the threat entity sets the request's detector
//!    holds — every id `CreateThreatEntitySet` minted and nothing else — in
//!    `ThreatEntitySetId` order, and each id it reports resolves through
//!    `GetThreatEntitySet`.
//! 2. It is PAGINATED (`smithy_query(operation="ListThreatEntitySets")` reports
//!    `inputToken`/`outputToken` `NextToken`, `items` `ThreatEntitySetIds`, `pageSize`
//!    `MaxResults`): walking the pages with any page size yields each id exactly once in
//!    the unpaged order, the last page carries no `NextToken`, and no page is longer than
//!    `ThreatEntitySetIds`'s own `len 0..50` whatever the caller asks for.
//! 3. It REPORTS: listing changes nothing a later call sees — no status settles early and
//!    no later list or read differs because something was listed.
//! 4. A threat entity set belongs to one detector, which is regional, so a list names
//!    only that detector's sets through the Region's endpoint that created it; a
//!    `detectorId` the store does not resolve and a `nextToken` this service never issued
//!    are refused with HTTP 400 under the `InvalidInputException` class naming the member
//!    at fault — never a hollow empty page, and never echoing the caller's value.

use std::time::Duration;

use serde_json::{json, Value};

use crate::client_rejections::page_token_refusal;
use crate::detector_lifecycle::{
    call, call_in, enabled_detector, error_code, new_router, Router, ABSENT_DETECTOR_ID,
};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    DETECTOR_ID_MEMBER, ENTITY_SET_ACTIVATING, ENTITY_SET_INACTIVE,
    ENTITY_SET_TRANSITION_MILLIS, NEXT_TOKEN_MEMBER, THREAT_ENTITY_SETS_PER_DETECTOR,
};

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// `ThreatEntitySetIds` is `len 0..50` and `MaxResults` is `range 1..50`
/// (`smithy_query(operation="ListThreatEntitySets")`), so one page carries at most 50 ids
/// however many entity sets the detector holds.
const THREAT_ENTITY_SET_IDS_MAX: i64 = 50;

/// A well-formed entity-set id (32 lowercase hex characters, the form this service mints)
/// that no create ever minted.
const ABSENT_SET_ID: &str = "0123456789abcdef0123456789abcdef";

/// A margin on top of the transition span, for the wall clock the handler reads.
const SETTLE_MARGIN_MILLIS: u64 = 250;

/// Long enough that an activated set's `ACTIVATING` -> `ACTIVE` transition is due.
fn past_the_transition() -> Duration {
    let span = u64::try_from(ENTITY_SET_TRANSITION_MILLIS).unwrap_or_default();
    Duration::from_millis(span + SETTLE_MARGIN_MILLIS)
}

async fn create_set(router: &Router, region: &str, detector_id: &str, body: Value) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/threatentityset"),
        Some(body),
    )
    .await
}

/// Create a threat entity set and return its minted id, failing the test if the create
/// did not succeed.
async fn created(
    router: &Router,
    region: &str,
    detector_id: &str,
    case: &str,
    body: Value,
) -> String {
    let (status, response) = create_set(router, region, detector_id, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: create-threat-entity-set expected 200, got {status} {response:?}"
    );
    response["threatEntitySetId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a threatEntitySetId, got {response:?}"))
        .to_string()
}

/// A create body differing from its neighbours in name, format, location and whether it
/// asks for activation, so the rows the listing tests build span those classes rather
/// than repeating one recorded request.
fn create_body(index: usize) -> Value {
    // Every member of `ThreatEntitySetFormat`
    // (`smithy_query(shape="ThreatEntitySetFormat")`).
    const FORMATS: [&str; 6] = [
        "TXT",
        "STIX",
        "OTX_CSV",
        "ALIEN_VAULT",
        "PROOF_POINT",
        "FIRE_EYE",
    ];
    json!({
        "name": format!("listed-{index:03}"),
        "format": FORMATS[index % FORMATS.len()],
        "location": format!("s3://my-bucket/lists/listed-{index:03}.txt"),
        "activate": index.is_multiple_of(2)
    })
}

/// One `ListThreatEntitySets` call. `query` is the raw query string (`""` for none).
async fn list_sets(router: &Router, region: &str, detector_id: &str, query: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("/detector/{detector_id}/threatentityset{query}"),
        None,
    )
    .await
}

fn ids_in(body: &Value, case: &str) -> Vec<String> {
    body["threatEntitySetIds"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a threatEntitySetIds array, got {body:?}")
        })
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a threatEntitySetId, got {value:?}")
                })
                .to_string()
        })
        .collect()
}

/// The `ThreatEntitySetIds` of one page, failing the test if the list did not succeed.
async fn page_ids(router: &Router, detector_id: &str, query: &str, case: &str) -> Vec<String> {
    let (status, body) = list_sets(router, "us-east-1", detector_id, query).await;
    assert_eq!(
        status, 200,
        "case {case:?}: list-threat-entity-sets{query} expected 200, got {status} {body:?}"
    );
    ids_in(&body, case)
}

/// Walk every page of `ListThreatEntitySets` with `page_size` (`None` asks for the
/// service's own page size) and return the ids in the order the pages reported them, plus
/// the length of each page.
async fn walk_pages(
    router: &Router,
    detector_id: &str,
    page_size: Option<i64>,
    case: &str,
) -> (Vec<String>, Vec<usize>) {
    let mut ids = Vec::new();
    let mut page_lengths = Vec::new();
    let mut token: Option<String> = None;
    // Bounded so a token the service never advances cannot loop forever.
    for _ in 0..512 {
        let mut query = String::new();
        if let Some(size) = page_size {
            query.push_str(&format!("?maxResults={size}"));
        }
        if let Some(token) = token.as_deref() {
            query.push_str(if query.is_empty() { "?" } else { "&" });
            query.push_str(&format!("nextToken={token}"));
        }
        let (status, body) = list_sets(router, "us-east-1", detector_id, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: list-threat-entity-sets{query} expected 200, got {status} {body:?}"
        );
        let page = ids_in(&body, case);
        assert!(
            page.len() as i64 <= THREAT_ENTITY_SET_IDS_MAX,
            "case {case:?}: a page must carry at most {THREAT_ENTITY_SET_IDS_MAX} ids, got {}",
            page.len()
        );
        if let Some(size) = page_size {
            assert!(
                page.len() as i64 <= size,
                "case {case:?}: a page must not exceed maxResults={size}, got {}",
                page.len()
            );
        }
        page_lengths.push(page.len());
        ids.extend(page);
        match body["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return (ids, page_lengths),
        }
    }
    panic!("case {case:?}: list-threat-entity-sets never ran out of pages");
}

/// The `Status` `GetThreatEntitySet` reports, failing the test if the read did not
/// succeed.
async fn status_of(router: &Router, detector_id: &str, set_id: &str, case: &str) -> String {
    let (status, body) = call(
        router,
        "GET",
        &format!("/detector/{detector_id}/threatentityset/{set_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-threat-entity-set expected 200, got {status} {body:?}"
    );
    body["status"].as_str().unwrap_or_default().to_string()
}

// ---------------------------------------------------------------------------
// Rule 1: the list names exactly the stored entity sets, in ThreatEntitySetId order.
// ---------------------------------------------------------------------------

/// One row: the case it stands for and how many threat entity sets the detector holds.
///
/// The rows span what the rule generalises over — COUNT (none, one, several, the
/// detector's whole `THREAT_ENTITY_SETS_PER_DETECTOR` collection) — with each create
/// differing in name, format, location and activation ([`create_body`]).
fn count_rows() -> Vec<(&'static str, usize)> {
    vec![
        ("a detector holding no threat entity sets", 0),
        ("a single threat entity set", 1),
        ("several threat entity sets", 3),
        (
            "the detector's whole collection at its ceiling",
            THREAT_ENTITY_SETS_PER_DETECTOR,
        ),
    ]
}

#[tokio::test]
async fn a_list_names_every_stored_threat_entity_set_in_order() {
    for (case, count) in count_rows() {
        let router = new_router();
        let detector = enabled_detector(&router).await;

        // Before any create the list is empty — not absent, not canned.
        let empty = page_ids(&router, &detector, "", case).await;
        assert!(
            empty.is_empty(),
            "case {case:?}: a detector with no threat entity sets must list nothing, got {empty:?}"
        );

        let mut minted = Vec::new();
        for index in 0..count {
            minted.push(created(&router, "us-east-1", &detector, case, create_body(index)).await);
        }

        // `ThreatEntitySetId` order over every id the creates minted: the ids a caller
        // gets back are the stored keys, sorted, whatever order the creates arrived in.
        let mut expected = minted.clone();
        expected.sort();
        let (listed, _) = walk_pages(&router, &detector, None, case).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the minted ids in ThreatEntitySetId order {expected:?}, got {listed:?}"
        );

        // Every id the list reports resolves through the implemented reader, so the list
        // is the stored entity sets and not a synthesised answer.
        for (index, id) in listed.iter().enumerate() {
            let (status, body) = call(
                &router,
                "GET",
                &format!("/detector/{detector}/threatentityset/{id}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: get-threat-entity-set {id:?} expected 200, got {status} {body:?}"
            );
            let reported = body["name"].as_str().unwrap_or_default();
            assert!(
                reported.starts_with("listed-"),
                "case {case:?}: listed id {index} must read back a stored entity set, got {body:?}"
            );
        }
    }
}

/// A create the service refused leaves the list exactly as it was: the ids named are the
/// ones that were really persisted.
#[tokio::test]
async fn a_refused_create_adds_nothing_to_the_list() {
    let router = new_router();
    let detector = enabled_detector(&router).await;
    let kept = created(
        &router,
        "us-east-1",
        &detector,
        "the stored threat entity set",
        create_body(0),
    )
    .await;

    // Each row: the case, the detector the create names, and the create body — a name
    // outside the documented character class, one outside the modelled `len 1..300`, a
    // format the enum does not carry, a location that is not an S3 URL, and a create
    // under a detector the store does not resolve.
    let rows: Vec<(&str, String, Value)> = vec![
        (
            "a name longer than the modelled maximum",
            detector.clone(),
            json!({"name": "n".repeat(301), "format": "TXT", "location": "s3://b/l.txt", "activate": true}),
        ),
        (
            "an empty name, below the modelled minimum",
            detector.clone(),
            json!({"name": "", "format": "TXT", "location": "s3://b/l.txt", "activate": true}),
        ),
        (
            "a name holding a character the documented class excludes",
            detector.clone(),
            json!({"name": "bad name", "format": "TXT", "location": "s3://b/l.txt", "activate": true}),
        ),
        (
            "a format the enum does not carry",
            detector.clone(),
            json!({"name": "bad-format", "format": "NOT_A_FORMAT", "location": "s3://b/l.txt", "activate": true}),
        ),
        (
            "a location that is not an S3 URL",
            detector.clone(),
            json!({"name": "bad-location", "format": "TXT", "location": "ftp://b/l.txt", "activate": true}),
        ),
        (
            "a create under a detector the store does not resolve",
            ABSENT_DETECTOR_ID.to_string(),
            create_body(1),
        ),
    ];

    for (case, detector_id, body) in rows {
        let (status, response) = create_set(&router, "us-east-1", &detector_id, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create to be refused with 400, got {status} {response:?}"
        );
        let listed = page_ids(&router, &detector, "", case).await;
        assert_eq!(
            listed,
            vec![kept.clone()],
            "case {case:?}: a refused create must not change the list, got {listed:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: paging with any page size yields each id exactly once, in order.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn paging_yields_each_threat_entity_set_exactly_once_in_order() {
    let router = new_router();
    let detector = enabled_detector(&router).await;

    // The detector's whole collection, so every page size below spans as many pages as
    // the ceiling allows.
    let count = THREAT_ENTITY_SETS_PER_DETECTOR;
    let mut minted = Vec::new();
    for index in 0..count {
        minted.push(
            created(
                &router,
                "us-east-1",
                &detector,
                "the paged detector",
                create_body(index),
            )
            .await,
        );
    }
    let mut expected = minted.clone();
    expected.sort();

    // The page sizes span `MaxResults`'s modelled range (`range 1..50`): its published
    // minimum, divisors and non-divisors of the list length in between, the size that
    // exactly covers the list, one past it, its published maximum, and the service's own
    // page size when the caller names none.
    let mut page_sizes: Vec<Option<i64>> = [
        1,
        2,
        3,
        4,
        count as i64,
        count as i64 + 1,
        THREAT_ENTITY_SET_IDS_MAX,
    ]
    .into_iter()
    .map(Some)
    .collect();
    page_sizes.push(None);

    for size in page_sizes {
        let case = match size {
            Some(size) => format!("maxResults={size}"),
            None => "no maxResults".to_string(),
        };
        let (listed, page_lengths) = walk_pages(&router, &detector, size, &case).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: paging must yield every id exactly once in order, got {listed:?}"
        );
        // Every page but the last is full: that is what makes the token walk terminate in
        // the number of calls a client budgets for.
        let per_page = size
            .unwrap_or(THREAT_ENTITY_SET_IDS_MAX)
            .min(THREAT_ENTITY_SET_IDS_MAX) as usize;
        for (index, length) in page_lengths.iter().enumerate() {
            if index + 1 < page_lengths.len() {
                assert_eq!(
                    *length, per_page,
                    "case {case:?}: page {index} must be full at {per_page}, got {length}"
                );
            } else {
                assert!(
                    *length <= per_page && *length > 0,
                    "case {case:?}: the last page must carry 1..={per_page} ids, got {length}"
                );
            }
        }
    }

    // A page that covers the whole list carries no token at all, at every size that
    // covers it — a token this service hands out always names an entry that exists.
    for size in [count as i64, count as i64 + 1, THREAT_ENTITY_SET_IDS_MAX] {
        let case = format!("a complete page at maxResults={size}");
        let (status, body) =
            list_sets(&router, "us-east-1", &detector, &format!("?maxResults={size}")).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body:?}");
        assert!(
            body.get("nextToken").is_none_or(Value::is_null),
            "case {case:?}: a complete page must carry no nextToken, got {:?}",
            body["nextToken"]
        );
    }

    // One past each published bound of `MaxResults` is refused by the modelled
    // constraint, before the handler runs: a list is not asked for a page size the model
    // does not admit.
    for size in [0, THREAT_ENTITY_SET_IDS_MAX + 1] {
        let case = format!("maxResults={size}, outside the modelled range");
        let (status, body) =
            list_sets(&router, "us-east-1", &detector, &format!("?maxResults={size}")).await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status} {body:?}");
    }
}

// ---------------------------------------------------------------------------
// Rule 3: listing reports; it changes nothing a later call sees.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listing_changes_nothing_a_later_call_sees() {
    let router = new_router();
    let detector = enabled_detector(&router).await;

    // One entity set the create asked GuardDuty to start using (transitional) and one it
    // did not (stored unused): neither status may move because something listed.
    let activating = created(
        &router,
        "us-east-1",
        &detector,
        "the activating entity set",
        create_body(0),
    )
    .await;
    let inactive = created(
        &router,
        "us-east-1",
        &detector,
        "the inactive entity set",
        create_body(1),
    )
    .await;

    let first = page_ids(&router, &detector, "", "the first list").await;
    // Several lists, some of them paged, in between.
    for size in [1, 2, 3] {
        let case = format!("an interleaved page at maxResults={size}");
        walk_pages(&router, &detector, Some(size), &case).await;
    }
    let again = page_ids(&router, &detector, "", "the list after the walks").await;
    assert_eq!(
        again, first,
        "case \"a repeated list\": listing must not change what a later list sees, got {again:?} after {first:?}"
    );

    // No status settled early: the transition is owed to the CLOCK, not to a list.
    for (case, id, expected) in [
        ("activate=true", &activating, ENTITY_SET_ACTIVATING),
        ("activate=false", &inactive, ENTITY_SET_INACTIVE),
    ] {
        let status = status_of(&router, &detector, id, case).await;
        assert_eq!(
            status, expected,
            "case {case:?}: a list must not settle the status, expected {expected}, got {status:?}"
        );
    }

    // And once the clock has settled them, further lists still change nothing.
    tokio::time::sleep(past_the_transition()).await;
    let settled: Vec<String> = {
        let mut statuses = Vec::new();
        for id in [&activating, &inactive] {
            statuses.push(status_of(&router, &detector, id, "after the transition").await);
        }
        statuses
    };
    for size in [1, 2] {
        walk_pages(&router, &detector, Some(size), "a list after the transition").await;
    }
    for (index, id) in [&activating, &inactive].into_iter().enumerate() {
        let now = status_of(&router, &detector, id, "after the later lists").await;
        assert_eq!(
            now, settled[index],
            "case \"settled entity set {index}\": a list must not move a settled status, got {now:?} after {:?}",
            settled[index]
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: scoped to one detector and Region; bad input is a classified 400.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_list_is_scoped_to_its_detector_and_region() {
    let router = new_router();
    let home = enabled_detector(&router).await;
    let mut home_ids = Vec::new();
    for index in 0..2 {
        home_ids.push(
            created(
                &router,
                "us-east-1",
                &home,
                "the us-east-1 detector",
                create_body(index),
            )
            .await,
        );
    }
    home_ids.sort();

    // A detector of its own in another Region, with entity sets of its own: one Region's
    // list must name neither the other Region's entity sets nor its detector.
    let (status, body) = call_in(
        &router,
        "eu-west-1",
        "POST",
        "/detector",
        Some(json!({"enable": true})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"create-detector in eu-west-1\": expected 200, got {status} {body:?}"
    );
    let away = body["detectorId"]
        .as_str()
        .expect("a detectorId in eu-west-1")
        .to_string();
    let mut away_ids = Vec::new();
    for index in 2..5 {
        away_ids.push(
            created(
                &router,
                "eu-west-1",
                &away,
                "the eu-west-1 detector",
                create_body(index),
            )
            .await,
        );
    }
    away_ids.sort();

    let listed = page_ids(&router, &home, "", "the us-east-1 list").await;
    assert_eq!(
        listed, home_ids,
        "case \"the us-east-1 list\": expected only this detector's entity sets {home_ids:?}, got {listed:?}"
    );

    let (status, body) = list_sets(&router, "eu-west-1", &away, "").await;
    assert_eq!(
        status, 200,
        "case \"the eu-west-1 list\": expected 200, got {status} {body:?}"
    );
    let listed = ids_in(&body, "the eu-west-1 list");
    assert_eq!(
        listed, away_ids,
        "case \"the eu-west-1 list\": expected only that detector's entity sets {away_ids:?}, got {listed:?}"
    );

    // Each detector is served only through the Region that created it.
    for (region, detector_id) in [("eu-west-1", home.as_str()), ("us-west-2", away.as_str())] {
        let case = format!("{detector_id} through {region}");
        let (status, body) = list_sets(&router, region, detector_id, "").await;
        assert_eq!(
            status, 400,
            "case {case:?}: another Region's endpoint must not list the entity sets, got {status} {body:?}"
        );
    }
}

#[tokio::test]
async fn an_unresolvable_detector_or_token_is_refused_without_echoing_it() {
    let router = new_router();
    let detector = enabled_detector(&router).await;
    let mut stored = Vec::new();
    for index in 0..2 {
        stored.push(
            created(
                &router,
                "us-east-1",
                &detector,
                "the stored entity sets",
                create_body(index),
            )
            .await,
        );
    }
    stored.sort();

    // Each row: the case, the detectorId the list uses, the query it carries, and the
    // member the refusal must name.
    let rows: Vec<(&str, String, String, &str)> = vec![
        (
            "a well-formed detectorId no create minted",
            ABSENT_DETECTOR_ID.to_string(),
            String::new(),
            DETECTOR_ID_MEMBER,
        ),
        (
            "a value that cannot be a detectorId at all",
            "not-a-detector".to_string(),
            String::new(),
            DETECTOR_ID_MEMBER,
        ),
        (
            "a detectorId at the modelled minimum length",
            "x".to_string(),
            String::new(),
            DETECTOR_ID_MEMBER,
        ),
        (
            "a detectorId at the modelled maximum length",
            "d".repeat(300),
            String::new(),
            DETECTOR_ID_MEMBER,
        ),
        (
            "an entity-set id used where a detectorId belongs",
            ABSENT_SET_ID.to_string(),
            String::new(),
            DETECTOR_ID_MEMBER,
        ),
        (
            "a nextToken that is not a token this service issues",
            detector.clone(),
            "?nextToken=not-a-token".to_string(),
            NEXT_TOKEN_MEMBER,
        ),
        (
            "a nextToken pointing past the end of the list",
            detector.clone(),
            "?nextToken=9999".to_string(),
            NEXT_TOKEN_MEMBER,
        ),
        (
            "a nextToken pointing at the entry just past the last one",
            detector.clone(),
            "?nextToken=2".to_string(),
            NEXT_TOKEN_MEMBER,
        ),
    ];

    for (case, detector_id, query, member) in rows {
        let (status, body) = list_sets(&router, "us-east-1", &detector_id, &query).await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status} {body:?}");
        let class = error_code(&body);
        assert_eq!(
            class, INVALID_INPUT,
            "case {case:?}: expected the {INVALID_INPUT} class, got {class:?} {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        if member == NEXT_TOKEN_MEMBER {
            // A page token this service did not issue is refused with the ONE
            // sentence the whole pagination surface answers, which names no member
            // at all.
            assert_eq!(
                message,
                page_token_refusal(),
                "case {case:?}: expected the page-token refusal, got {message:?}"
            );
        } else {
            assert!(
                message.contains(member),
                "case {case:?}: the refusal must name {member}, got {message:?}"
            );
        }
        for echoed in [
            "not-a-detector",
            "not-a-token",
            "9999",
            ABSENT_DETECTOR_ID,
            ABSENT_SET_ID,
        ] {
            assert!(
                !message.contains(echoed),
                "case {case:?}: the refusal must not echo {echoed:?}, got {message:?}"
            );
        }
        // The refusal leaves the store alone: the stored entity sets still list.
        let listed = page_ids(&router, &detector, "", case).await;
        assert_eq!(
            listed, stored,
            "case {case:?}: a refusal must not change the list, got {listed:?}"
        );
    }

    // The token the service DOES issue is accepted on the same list: the refusals above
    // are about tokens it never handed out, not about paging itself.
    let (status, body) = list_sets(&router, "us-east-1", &detector, "?maxResults=1").await;
    assert_eq!(
        status, 200,
        "case \"the first page\": expected 200, got {status} {body:?}"
    );
    let token = body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"the first page\": expected a nextToken, got {body:?}"));
    let second = page_ids(
        &router,
        &detector,
        &format!("?maxResults=1&nextToken={token}"),
        "the issued token",
    )
    .await;
    assert_eq!(
        second,
        vec![stored[1].clone()],
        "case \"the issued token\": expected the second page, got {second:?}"
    );
}
