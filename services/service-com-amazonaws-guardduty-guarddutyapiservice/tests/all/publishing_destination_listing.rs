// SPDX-License-Identifier: CC0-1.0

//! `ListPublishingDestinations`, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. `ListPublishingDestinations` names exactly the publishing destinations the
//!    request's detector holds — every id `CreatePublishingDestination` minted and
//!    nothing else — in `DestinationId` order, and each entry carries the
//!    `destinationType` and `status` the stored record carries, as
//!    `DescribePublishingDestination` reports them.
//! 2. It is PAGINATED (`smithy_query(operation="ListPublishingDestinations")` reports
//!    `inputToken`/`outputToken` `NextToken` and `pageSize` `MaxResults`): walking the
//!    pages with any page size yields each destination exactly once in the unpaged
//!    order, the last page carries no `nextToken`, and no page is longer than
//!    `maxResults` or than the member's own `range 1..50` ceiling. Each published bound
//!    of that range is accepted and a value past it is refused.
//! 3. It REPORTS: listing changes nothing a later caller sees. Only the CLOCK moves a
//!    destination out of `PENDING_VERIFICATION`, so a list inside the verification span
//!    reports the pending status and leaves it pending for the next reader, and a list
//!    past the span reports the settled one; two lists in a row are identical.
//! 4. A destination belongs to one detector, which is regional, so a list names only
//!    that detector's destinations through the Region's endpoint that created them; a
//!    `detectorId` the store does not resolve and a `nextToken` this service never
//!    issued are refused with HTTP 400 under the `InvalidInputException` class naming
//!    the member at fault — never a hollow empty page, and never echoing the caller's
//!    value.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::client_rejections::page_token_refusal;
use crate::detector_lifecycle::{
    call_in, enabled_detector, error_code, error_shape, new_router, Router, ABSENT_DETECTOR_ID,
};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    DETECTOR_ID_MEMBER, NEXT_TOKEN_MEMBER, PENDING_VERIFICATION, PUBLISHING,
    PUBLISHING_DESTINATIONS_PER_DETECTOR,
};

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator (see `client_rejections.rs`).
const INVALID_INPUT: &str = "InvalidInputException";

/// The modelled error SHAPE that class is enveloped in — the only client error
/// `ListPublishingDestinations` declares
/// (`smithy_query(operation="ListPublishingDestinations")`).
const ENVELOPE: &str = "BadRequestException";

/// The only member of the `DestinationType` enum
/// (`smithy_query(shape="DestinationType")`).
const S3: &str = "S3";

/// `MaxResults` is `range 1..50`
/// (`smithy_query(operation="ListPublishingDestinations")`), so one page can never
/// carry more than 50 destinations however many the detector holds.
const MAX_RESULTS_CEILING: usize = 50;

/// The Region these tests sign for unless a row names another: the one
/// `detector_lifecycle::call` uses, so `enabled_detector`'s detector is reachable.
const HOME_REGION: &str = "us-east-1";

/// Longer than `state::VERIFICATION_DELAY_MILLIS`, so a read after this wait is past
/// the span the clock owes the `PENDING_VERIFICATION` -> `PUBLISHING` move at.
const PAST_THE_VERIFICATION_SPAN: std::time::Duration = std::time::Duration::from_millis(900);

/// A create body differing from its neighbours in bucket, folder depth, KMS resource
/// form, partition, whether it names an `expectedBucketOwner` and whether it carries
/// tags, so the rows the listing tests build span those classes rather than repeating
/// one recorded request. Every ARN form here is one
/// `smithy_query(query="doc",shape="DestinationProperties",member="DestinationArn")`
/// names ("To specify an S3 bucket folder use the following format:
/// `arn:aws:s3:::DOC-EXAMPLE-BUCKET/myFolder/`").
fn create_body(index: usize) -> Value {
    let partition = if index.is_multiple_of(5) {
        "aws-us-gov"
    } else {
        "aws"
    };
    let destination_arn = match index % 3 {
        0 => format!("arn:{partition}:s3:::findings-{index:03}"),
        1 => format!("arn:{partition}:s3:::findings-{index:03}/exports/"),
        _ => format!("arn:{partition}:s3:::findings-{index:03}/exports/2026/{index:03}/"),
    };
    let kms_key_arn = if index.is_multiple_of(2) {
        format!("arn:{partition}:kms:us-east-1:111122223333:key/0000abcd-12ab-34cd-56ef-{index:012}")
    } else {
        format!("arn:{partition}:kms:eu-west-1:444455556666:alias/guardduty-{index:03}")
    };
    let mut properties = json!({
        "destinationArn": destination_arn,
        "kmsKeyArn": kms_key_arn,
    });
    if index.is_multiple_of(3) {
        properties["expectedBucketOwner"] = json!("111122223333");
    }
    let mut body = json!({
        "destinationType": S3,
        "destinationProperties": properties,
    });
    if index.is_multiple_of(4) {
        body["tags"] = json!({"index": format!("{index:03}")});
    }
    body
}

/// Create one destination and return its minted id, failing the test if the create did
/// not succeed.
async fn created(
    router: &Router,
    region: &str,
    detector_id: &str,
    case: &str,
    body: Value,
) -> String {
    let (status, response) = call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/publishingDestination"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: create-publishing-destination {body} expected 200, got {status} {response:?}"
    );
    response["destinationId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a destinationId, got {response:?}")
        })
        .to_string()
}

/// One `ListPublishingDestinations` call. `query` is the raw query string (`""` for
/// none).
async fn list_destinations(
    router: &Router,
    region: &str,
    detector_id: &str,
    query: &str,
) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("/detector/{detector_id}/publishingDestination{query}"),
        None,
    )
    .await
}

/// The `destinations` array of one response, as `(destinationId, destinationType,
/// status)` triples.
fn entries_in(body: &Value, case: &str) -> Vec<(String, String, String)> {
    body["destinations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a destinations array, got {body:?}"))
        .iter()
        .map(|entry| {
            let member = |name: &str| {
                entry
                    .get(name)
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected a {name} on every entry, got {entry:?}")
                    })
                    .to_string()
            };
            (
                member("destinationId"),
                member("destinationType"),
                member("status"),
            )
        })
        .collect()
}

/// The ids of one page, failing the test if the list did not succeed.
async fn page_entries(
    router: &Router,
    detector_id: &str,
    query: &str,
    case: &str,
) -> Vec<(String, String, String)> {
    let (status, body) = list_destinations(router, HOME_REGION, detector_id, query).await;
    assert_eq!(
        status, 200,
        "case {case:?}: list-publishing-destinations{query} expected 200, got {status} {body:?}"
    );
    entries_in(&body, case)
}

/// Walk every page with `page_size` (`None` asks for the service's own page size) and
/// return the entries in the order the pages reported them, plus each page's length.
async fn walk_pages(
    router: &Router,
    detector_id: &str,
    page_size: Option<i64>,
    case: &str,
) -> (Vec<(String, String, String)>, Vec<usize>) {
    let mut entries = Vec::new();
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
        let (status, body) = list_destinations(router, HOME_REGION, detector_id, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: list-publishing-destinations{query} expected 200, got {status} {body:?}"
        );
        let page = entries_in(&body, case);
        assert!(
            page.len() <= MAX_RESULTS_CEILING,
            "case {case:?}: a page must carry at most {MAX_RESULTS_CEILING} destinations, got {}",
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
        entries.extend(page);
        match body["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return (entries, page_lengths),
        }
    }
    panic!("case {case:?}: list-publishing-destinations never ran out of pages");
}

/// What `DescribePublishingDestination` reports for one destination, as the same
/// `(id, type, status)` triple a list entry carries.
async fn described(
    router: &Router,
    detector_id: &str,
    destination_id: &str,
    case: &str,
) -> (String, String, String) {
    let (status, body) = call_in(
        router,
        HOME_REGION,
        "GET",
        &format!("/detector/{detector_id}/publishingDestination/{destination_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: describe-publishing-destination {destination_id:?} expected 200, got {status} {body:?}"
    );
    let member = |name: &str| {
        body.get(name)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected a {name}, got {body:?}"))
            .to_string()
    };
    (
        member("destinationId"),
        member("destinationType"),
        member("status"),
    )
}

// ---------------------------------------------------------------------------
// Rule 1: the list names exactly the stored destinations, in DestinationId order.
// ---------------------------------------------------------------------------

/// One row: the case it stands for and how many destinations the detector holds.
///
/// The rows span what the rule generalises over — COUNT (none, one, several, the
/// detector's whole collection at its bound) — with each create differing in bucket,
/// folder depth, key form, partition, owner and tags ([`create_body`]); ids are minted,
/// so creation order and `DestinationId` order are unrelated.
fn count_rows() -> Vec<(&'static str, usize)> {
    vec![
        ("a detector holding no destinations", 0),
        ("a single destination", 1),
        ("several destinations", 6),
        (
            "the detector's whole collection at its maximum",
            PUBLISHING_DESTINATIONS_PER_DETECTOR,
        ),
    ]
}

#[tokio::test]
async fn a_list_names_every_stored_destination_in_order() {
    for (case, count) in count_rows() {
        let router = new_router();
        let detector = enabled_detector(&router).await;

        // Before any create the list is empty — not absent, not canned.
        let empty = page_entries(&router, &detector, "", case).await;
        assert!(
            empty.is_empty(),
            "case {case:?}: a detector with no destinations must list nothing, got {empty:?}"
        );

        let mut minted = BTreeSet::new();
        for index in 0..count {
            let id = created(&router, HOME_REGION, &detector, case, create_body(index)).await;
            assert!(
                minted.insert(id.clone()),
                "case {case:?}: every create mints its own destinationId, but got {id:?} twice"
            );
        }

        // `DestinationId` order over every id the creates minted: the ids a caller gets
        // back are the stored keys, sorted, whatever order the creates arrived in.
        let expected: Vec<String> = minted.iter().cloned().collect();
        let (listed, _) = walk_pages(&router, &detector, None, case).await;
        let listed_ids: Vec<String> = listed.iter().map(|(id, _, _)| id.clone()).collect();
        assert_eq!(
            listed_ids, expected,
            "case {case:?}: expected the minted ids in DestinationId order {expected:?}, got {listed_ids:?}"
        );

        // Every entry carries the type and status the stored record carries, which is
        // what the implemented reader reports for the same destination: the list is the
        // stored destinations and not a synthesised answer.
        for entry in &listed {
            let read = described(&router, &detector, &entry.0, case).await;
            assert_eq!(
                *entry, read,
                "case {case:?}: the list entry must match the stored destination {read:?}, got {entry:?}"
            );
            assert_eq!(
                entry.1, S3,
                "case {case:?}: expected destinationType {S3}, got {:?}",
                entry.1
            );
        }
    }
}

/// A create the service refused leaves the list exactly as it was: the destinations
/// named are the ones that were really persisted.
#[tokio::test]
async fn a_refused_create_adds_nothing_to_the_list() {
    let router = new_router();
    let detector = enabled_detector(&router).await;
    let kept = created(
        &router,
        HOME_REGION,
        &detector,
        "the stored destination",
        create_body(0),
    )
    .await;

    // Each row: the case, the detector the create names, and the create body — a type
    // the enum does not carry, a destination ARN that is not an S3 ARN, a missing KMS
    // key, a non-numeric bucket owner, and a create under a detector the store does not
    // resolve.
    let rows: Vec<(&str, String, Value)> = vec![
        (
            "a destinationType the enum does not carry",
            detector.clone(),
            json!({
                "destinationType": "KINESIS",
                "destinationProperties": {
                    "destinationArn": "arn:aws:s3:::refused-bucket",
                    "kmsKeyArn": "arn:aws:kms:us-east-1:111122223333:key/abcd",
                },
            }),
        ),
        (
            "a destinationArn that is not an ARN of the destination's service",
            detector.clone(),
            json!({
                "destinationType": S3,
                "destinationProperties": {
                    "destinationArn": "arn:aws:sqs:us-east-1:111122223333:queue",
                    "kmsKeyArn": "arn:aws:kms:us-east-1:111122223333:key/abcd",
                },
            }),
        ),
        (
            "no kmsKeyArn at all",
            detector.clone(),
            json!({
                "destinationType": S3,
                "destinationProperties": {"destinationArn": "arn:aws:s3:::refused-bucket"},
            }),
        ),
        (
            "an expectedBucketOwner that is not an account id",
            detector.clone(),
            json!({
                "destinationType": S3,
                "destinationProperties": {
                    "destinationArn": "arn:aws:s3:::refused-bucket",
                    "kmsKeyArn": "arn:aws:kms:us-east-1:111122223333:key/abcd",
                    "expectedBucketOwner": "not-an-account",
                },
            }),
        ),
        (
            "a create under a detector the store does not resolve",
            ABSENT_DETECTOR_ID.to_string(),
            create_body(1),
        ),
    ];

    for (case, target, body) in rows {
        let (status, response) = call_in(
            &router,
            HOME_REGION,
            "POST",
            &format!("/detector/{target}/publishingDestination"),
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create to be refused with 400, got {status} {response:?}"
        );
        let listed = page_entries(&router, &detector, "", case).await;
        let ids: Vec<String> = listed.iter().map(|(id, _, _)| id.clone()).collect();
        assert_eq!(
            ids,
            vec![kept.clone()],
            "case {case:?}: a refused create must leave the list unchanged, got {ids:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: pagination over any page size, and the published bounds of MaxResults.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn any_page_size_walks_every_destination_exactly_once() {
    let router = new_router();
    let detector = enabled_detector(&router).await;
    let mut minted = BTreeSet::new();
    for index in 0..11 {
        minted.insert(created(&router, HOME_REGION, &detector, "paged", create_body(index)).await);
    }
    let expected: Vec<String> = minted.iter().cloned().collect();

    // The page sizes span both published bounds of `MaxResults` (`range 1..50`), a size
    // that divides the collection, one that does not, and one larger than it, plus the
    // service's own page size (no `maxResults` at all).
    let sizes: Vec<Option<i64>> = vec![None, Some(1), Some(2), Some(4), Some(11), Some(12), Some(50)];
    for size in sizes {
        let case = format!("maxResults={size:?} over {} destinations", expected.len());
        let (walked, page_lengths) = walk_pages(&router, &detector, size, &case).await;
        let ids: Vec<String> = walked.iter().map(|(id, _, _)| id.clone()).collect();
        assert_eq!(
            ids, expected,
            "case {case:?}: expected every destination exactly once in DestinationId order {expected:?}, got {ids:?}"
        );
        // Every page but the last is full, so the walk is pages of `maxResults` and not
        // one id at a time.
        if let Some(size) = size {
            let full = usize::try_from(size).expect("a positive page size");
            for (index, length) in page_lengths.iter().enumerate() {
                if index + 1 < page_lengths.len() {
                    assert_eq!(
                        *length, full,
                        "case {case:?}: page {index} must be full at {full}, got {length}"
                    );
                }
            }
        }
    }
}

/// Each published bound of `MaxResults` (`range 1..50`) is accepted and a value past
/// either bound is refused by the modelled constraint — under the error shape this
/// operation declares, not a code the model declares for nothing.
#[tokio::test]
async fn max_results_is_accepted_on_its_bounds_and_refused_past_them() {
    let router = new_router();
    let detector = enabled_detector(&router).await;
    for index in 0..3 {
        created(&router, HOME_REGION, &detector, "bounds", create_body(index)).await;
    }

    for bound in [1, 50] {
        let case = format!("maxResults={bound}, a published bound");
        let (status, body) =
            list_destinations(&router, HOME_REGION, &detector, &format!("?maxResults={bound}"))
                .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body:?}"
        );
        let page = entries_in(&body, &case);
        assert!(
            !page.is_empty() && page.len() <= bound,
            "case {case:?}: expected at most {bound} entries and at least one, got {}",
            page.len()
        );
    }

    for past in [0, 51, 1_000] {
        let case = format!("maxResults={past}, past a published bound");
        let (status, body) =
            list_destinations(&router, HOME_REGION, &detector, &format!("?maxResults={past}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled range to refuse it, got {status} {body:?}"
        );
        assert_eq!(
            error_shape(&body),
            ENVELOPE,
            "case {case:?}: expected the modelled error shape {ENVELOPE}, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a list reports — only the clock moves a destination's status.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let router = new_router();
    let detector = enabled_detector(&router).await;
    let mut ids = Vec::new();
    for index in 0..4 {
        ids.push(created(&router, HOME_REGION, &detector, "reporting", create_body(index)).await);
    }

    // Inside the verification span: every list reports the pending status, and no
    // number of lists settles it early for the next reader.
    for attempt in 1..=3 {
        let case = format!("list {attempt} inside the verification span");
        let listed = page_entries(&router, &detector, "", &case).await;
        for (id, _, status) in &listed {
            assert_eq!(
                status, PENDING_VERIFICATION,
                "case {case:?}: destination {id:?} must still be {PENDING_VERIFICATION}, got {status:?}"
            );
        }
        let read = described(&router, &detector, &ids[0], &case).await;
        assert_eq!(
            read.2, PENDING_VERIFICATION,
            "case {case:?}: a read after the list must still be {PENDING_VERIFICATION}, got {read:?}"
        );
    }

    // Past the span the CLOCK — nothing the caller did — has moved every destination to
    // the settled status, and a second list is identical to the first.
    tokio::time::sleep(PAST_THE_VERIFICATION_SPAN).await;
    let case = "list past the verification span";
    let first = page_entries(&router, &detector, "", case).await;
    for (id, _, status) in &first {
        assert_eq!(
            status, PUBLISHING,
            "case {case:?}: destination {id:?} must have settled to {PUBLISHING}, got {status:?}"
        );
    }
    let second = page_entries(&router, &detector, "", case).await;
    assert_eq!(
        first, second,
        "case {case:?}: two lists in a row must be identical, got {first:?} then {second:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: scoped to one detector and Region; bad input is a classified 400.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_list_is_scoped_to_its_detector_and_region() {
    let router = new_router();
    let home = enabled_detector(&router).await;
    let mut home_ids = BTreeSet::new();
    for index in 0..2 {
        home_ids.insert(
            created(
                &router,
                HOME_REGION,
                &home,
                "the us-east-1 detector",
                create_body(index),
            )
            .await,
        );
    }

    // A detector of its own in another Region, with destinations of its own: one
    // Region's list must name neither the other Region's destinations nor its detector.
    let away_region = "eu-west-1";
    let (status, body) = call_in(
        &router,
        away_region,
        "POST",
        "/detector",
        Some(json!({"enable": true})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"create-detector in {away_region}\": expected 200, got {status} {body:?}"
    );
    let away = body["detectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a detectorId in {away_region}, got {body:?}"))
        .to_string();
    let mut away_ids = BTreeSet::new();
    for index in 2..5 {
        away_ids.insert(
            created(
                &router,
                away_region,
                &away,
                "the eu-west-1 detector",
                create_body(index),
            )
            .await,
        );
    }

    let listed = page_entries(&router, &home, "", "the us-east-1 list").await;
    let ids: Vec<String> = listed.iter().map(|(id, _, _)| id.clone()).collect();
    assert_eq!(
        ids,
        home_ids.iter().cloned().collect::<Vec<String>>(),
        "case \"the us-east-1 list\": expected only this detector's destinations {home_ids:?}, got {ids:?}"
    );

    let (status, body) = list_destinations(&router, away_region, &away, "").await;
    assert_eq!(
        status, 200,
        "case \"the eu-west-1 list\": expected 200, got {status} {body:?}"
    );
    let ids: Vec<String> = entries_in(&body, "the eu-west-1 list")
        .iter()
        .map(|(id, _, _)| id.clone())
        .collect();
    assert_eq!(
        ids,
        away_ids.iter().cloned().collect::<Vec<String>>(),
        "case \"the eu-west-1 list\": expected only that detector's destinations {away_ids:?}, got {ids:?}"
    );

    // Each detector is served only through the Region that created it.
    for (region, detector_id) in [(away_region, home.as_str()), ("us-west-2", away.as_str())] {
        let case = format!("{detector_id} through {region}");
        let (status, body) = list_destinations(&router, region, detector_id, "").await;
        assert_eq!(
            status, 400,
            "case {case:?}: another Region's endpoint must not list the destinations, got {status} {body:?}"
        );
    }
}

#[tokio::test]
async fn an_unresolvable_detector_or_token_is_refused_without_echoing_it() {
    let router = new_router();
    let detector = enabled_detector(&router).await;
    for index in 0..3 {
        created(&router, HOME_REGION, &detector, "refusals", create_body(index)).await;
    }

    // Each row: the case, the detector the list names, the query string, the value that
    // must not be echoed back, and the member the message must name.
    let rows: Vec<(&str, String, String, String, &str)> = vec![
        (
            "a well-formed detectorId the store does not hold",
            ABSENT_DETECTOR_ID.to_string(),
            String::new(),
            ABSENT_DETECTOR_ID.to_string(),
            DETECTOR_ID_MEMBER,
        ),
        (
            "a detectorId that cannot be an id at all",
            "not-a-detector".to_string(),
            String::new(),
            "not-a-detector".to_string(),
            DETECTOR_ID_MEMBER,
        ),
        (
            "a nextToken that is not an offset this service issues",
            detector.clone(),
            "?nextToken=not-a-token".to_string(),
            "not-a-token".to_string(),
            NEXT_TOKEN_MEMBER,
        ),
        (
            "a nextToken past the end of the collection",
            detector.clone(),
            "?nextToken=9999".to_string(),
            "9999".to_string(),
            NEXT_TOKEN_MEMBER,
        ),
        (
            "a negative nextToken",
            detector.clone(),
            "?nextToken=-1".to_string(),
            "-1".to_string(),
            NEXT_TOKEN_MEMBER,
        ),
    ];

    for (case, target, query, secret, member) in rows {
        let (status, body) = list_destinations(&router, HOME_REGION, &target, &query).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_INPUT,
            "case {case:?}: expected the {INVALID_INPUT} class, got {body:?}"
        );
        assert_eq!(
            error_shape(&body),
            ENVELOPE,
            "case {case:?}: expected the modelled error shape {ENVELOPE}, got {body:?}"
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
                "case {case:?}: expected the message to name {member:?}, got {message:?}"
            );
        }
        assert!(
            !message.contains(&secret),
            "case {case:?}: the refusal must not echo {secret:?}, got {message:?}"
        );
        assert!(
            body.get("destinations").is_none_or(Value::is_null),
            "case {case:?}: a refusal must carry no destinations page, got {body:?}"
        );
    }
}
