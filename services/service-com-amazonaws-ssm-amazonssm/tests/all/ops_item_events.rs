// SPDX-License-Identifier: CC0-1.0

//! OpsCenter's OpsItem event trail end to end: `ListOpsItemEvents` reports the events the
//! trail holds, narrowed by the filters the request names.
//!
//! Every listing goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`router`], [`router_with_engine`], [`call_in_region`],
//! [`error_code`]) dispatches `POST /` with `X-Amz-Target: AmazonSSM.<Operation>` through
//! the multi-service router, so a response that is not computed from persisted state
//! cannot pass.
//!
//! Two rules are under test. The first is that opening an OpsItem records no event, so the
//! trail of a just-created OpsItem is empty. The second is that a filter is a CRITERION
//! and not a resource to resolve: a filter value no event of the Region is about — an
//! OpsItem id nothing was created under among them — selects no event, which is an empty
//! page rather than a refusal. To tell an empty page apart from a store that can only ever
//! be empty, the suites that exercise narrowing and paging SEED events the way
//! `all/resource_tags.rs` seeds a managed node: no operation of this service records an
//! OpsItem event yet (`UpdateOpsItem` is still scaffolded), so the events come from the
//! store's one write path and every listing of them still goes through the router.
//!
//! The suites are tables. Each row is one input along the axis the rule under test
//! generalises over — how many OpsItems were written, how many events each carries, which
//! of them a filter names, how the filter values are spread over the filters, the page
//! size, the Region — rather than the single listing a recorded unit happened to make.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssm_amazonssm::state::ops_item_events::{
    written_detail, DETAIL_TYPE_UPDATE,
};
use service_com_amazonaws_ssm_amazonssm::state::ComAmazonawsSsmAmazonssmState;
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call_in_region, error_code, router, router_with_engine, REGION};

/// The modelled bounds of `OpsItemEventMaxResults` (`smithy_query(query="operation",
/// operation="ListOpsItemEvents")`: range 1..50).
const MIN_MAX_RESULTS: i64 = 1;

/// See [`MIN_MAX_RESULTS`].
const MAX_MAX_RESULTS: i64 = 50;

/// How many OpsItem ids one listing may be narrowed to before it is refused
/// (`crate::state::ops_item_events::MAX_FILTER_VALUES`, the emulator's own bound: nothing
/// in the model bounds the `OpsItemEventFilterValues` list).
const MAX_FILTER_VALUES: usize = 100;

/// The fixed text of the refusal a listing past that bound carries.
const FILTER_VALUES_EXCEEDED: &str = "Maximum number of OpsItemEvent filter values exceeded";

/// The `Source` every event of this service carries.
const EVENT_SOURCE: &str = "aws.ssm";

/// An OpsItem id of the modelled `OpsItemId` form (`^(oi)-[0-9a-f]{12}$`) that no create
/// here mints, for the cases about an id the Region holds no OpsItem under.
fn unknown_ops_item_id(seed: usize) -> String {
    format!("oi-{seed:012x}")
}

/// `CreateOpsItem` in `region`, asserting it succeeded, and answering the OpsItem id.
async fn create_in(router: &MultiServiceRouter, case: &str, region: &str, title: &str) -> String {
    let (status, response) = call_in_region(
        router,
        region,
        "CreateOpsItem",
        &json!({
            "Title": title,
            "Description": format!("what {title} is about"),
            "Source": "emulator-tests",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: CreateOpsItem {title} must succeed, got {status} {response}"
    );
    response["OpsItemId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: CreateOpsItem reports an OpsItemId, got {response}"))
        .to_string()
}

/// [`create_in`] at [`REGION`]'s endpoint.
async fn create(router: &MultiServiceRouter, case: &str, title: &str) -> String {
    create_in(router, case, REGION, title).await
}

/// Record one event about `ops_item_id` in `region`, `age` after the epoch of this suite's
/// seeding, and answer its event id.
///
/// The store's own write path is what records it ([`DETAIL_TYPE_UPDATE`] is the detail type
/// of an edit of an OpsItem), so a seeded event is indistinguishable from one a write of
/// this service recorded; only the listing is what the assertions read.
fn seed_event(
    engine: &Arc<engine::InMemoryEngine>,
    region: &str,
    ops_item_id: &str,
    age: u64,
) -> String {
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + age);
    engine
        .state::<ComAmazonawsSsmAmazonssmState>()
        .lock()
        .record_ops_item_event(
            region,
            ops_item_id,
            DETAIL_TYPE_UPDATE,
            written_detail(age + 1, "Open"),
            "arn:aws:sts::123456789012:assumed-role/emulator-tests/emulator-tests",
            at,
        )
}

/// One `ListOpsItemEvents` call in `region`, answering `(status, body)`.
async fn list_in(router: &MultiServiceRouter, region: &str, request: &Value) -> (u16, Value) {
    call_in_region(router, region, "ListOpsItemEvents", request).await
}

/// `ListOpsItemEvents` at [`REGION`], asserting it succeeded, and answering its body.
async fn list_ok(router: &MultiServiceRouter, case: &str, request: &Value) -> Value {
    let (status, body) = list_in(router, REGION, request).await;
    assert_eq!(
        status, 200,
        "case {case}: ListOpsItemEvents {request} must succeed, got {status} {body}"
    );
    body
}

/// The `Summaries` of a successful listing, which a listing always reports — empty when
/// nothing it selected.
async fn summaries(router: &MultiServiceRouter, case: &str, request: &Value) -> Vec<Value> {
    let body = list_ok(router, case, request).await;
    body["Summaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: ListOpsItemEvents reports Summaries, got {body}"))
        .clone()
}

/// A filter naming `ids` by the only key and operator the model supports
/// (`smithy_query(query="shape", shape="OpsItemEventFilter")`: `OpsItemId`, `Equal`).
fn ops_item_filter(ids: &[String]) -> Value {
    json!({ "Key": "OpsItemId", "Values": ids, "Operator": "Equal" })
}

/// The `OpsItemId` of every summary, in the order the listing reported them.
fn listed_ops_item_ids(summaries: &[Value]) -> Vec<String> {
    summaries
        .iter()
        .map(|summary| summary["OpsItemId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The `EventId` of every summary, in the order the listing reported them.
fn listed_event_ids(summaries: &[Value]) -> Vec<String> {
    summaries
        .iter()
        .map(|summary| summary["EventId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Whether `id` has the 8-4-4-4-12 lowercase-hex form an event id carries — the same form
/// a request id carries, so the clause-wording suite reads its id with this too.
fn is_uuid(id: &str) -> bool {
    let mut groups = id.split('-');
    for width in [8usize, 4, 4, 4, 12] {
        let Some(group) = groups.next() else {
            return false;
        };
        let hex = group.len() == width
            && group
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
        if !hex {
            return false;
        }
    }
    groups.next().is_none()
}

/// Assert `body` is a served listing that selected nothing: status 200, a `Summaries`
/// member that is present and empty, and no page to resume.
fn assert_empty_page(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 200,
        "case {case}: a listing that selects nothing is served, got {status} {body}"
    );
    assert_eq!(
        body["Summaries"].as_array().map(Vec::len),
        Some(0),
        "case {case}: expected Summaries present and empty, got {body}"
    );
    assert!(
        body["NextToken"].is_null(),
        "case {case}: an empty page has nothing to resume, got {body}"
    );
}

/// One row of the trail table: how many OpsItems were created before the listing.
struct TrailCase<'a> {
    case: &'a str,
    titles: &'a [&'a str],
}

/// Opening an OpsItem records no event: however many OpsItems a Region holds, a listing of
/// its trail — unfiltered, or narrowed to any of them — is an empty page.
#[tokio::test]
async fn opening_an_ops_item_records_no_event() {
    let cases = [
        TrailCase {
            case: "no OpsItem at all",
            titles: &[],
        },
        TrailCase {
            case: "one OpsItem",
            titles: &["A disk is filling up"],
        },
        TrailCase {
            case: "two OpsItems",
            titles: &["A node stopped reporting", "A build is failing"],
        },
        TrailCase {
            case: "five OpsItems",
            titles: &[
                "Latency is up",
                "A queue is backing up",
                "A certificate expires soon",
                "A deployment rolled back",
                "A budget was exceeded",
            ],
        },
    ];
    for TrailCase { case, titles } in cases {
        // A fresh router per row, so what a listing reports is exactly this row's writes.
        let router = router();
        let mut created = Vec::new();
        for title in titles {
            created.push(create(&router, case, title).await);
        }

        let (status, body) = list_in(&router, REGION, &json!({})).await;
        assert_empty_page(case, status, &body);

        // The same holds of the trail of each OpsItem the row created, asked for by name.
        for ops_item_id in &created {
            let request = json!({ "Filters": [ops_item_filter(std::slice::from_ref(ops_item_id))] });
            let (status, body) = list_in(&router, REGION, &request).await;
            assert_empty_page(&format!("{case}, the trail of {ops_item_id}"), status, &body);
        }

        // A report writes nothing: a second listing reports what the first one did.
        let (status, again) = list_in(&router, REGION, &json!({})).await;
        assert_empty_page(&format!("{case}, listed twice"), status, &again);
    }
}

/// One row of the selection table: which ids the filters name, and whether any event of
/// the Region is about them.
struct SelectionCase<'a> {
    case: &'a str,
    /// The filters' values, one `Vec` per filter.
    filters: Vec<Vec<String>>,
    /// The OpsItem ids whose events the listing must report, newest event first.
    expected: Vec<String>,
}

/// A filter value is a criterion and not a lookup: an id no event is about — including an
/// OpsItem id nothing was ever created under — selects nothing, while the Region's other
/// events stay selectable, so the empty page is a filter result rather than an empty store.
#[tokio::test]
async fn a_filter_value_no_event_is_about_selects_an_empty_page() {
    let (router, engine) = router_with_engine();
    let recorded = create(&router, "the OpsItem with a trail", "A disk is filling up").await;
    let quiet = create(&router, "the OpsItem without a trail", "A build is failing").await;
    seed_event(&engine, REGION, &recorded, 0);

    let unknown = unknown_ops_item_id(0xabc);
    let cases = vec![
        SelectionCase {
            case: "an id nothing was created under",
            filters: vec![vec![unknown.clone()]],
            expected: vec![],
        },
        SelectionCase {
            case: "a held OpsItem no event is about",
            filters: vec![vec![quiet.clone()]],
            expected: vec![],
        },
        SelectionCase {
            case: "an id nothing was created under beside the one with a trail",
            filters: vec![vec![unknown.clone(), recorded.clone()]],
            expected: vec![recorded.clone()],
        },
        SelectionCase {
            case: "two filters, one of which names an id nothing was created under",
            filters: vec![vec![recorded.clone()], vec![unknown.clone()]],
            expected: vec![],
        },
        SelectionCase {
            case: "a filter naming no id at all",
            filters: vec![vec![]],
            expected: vec![],
        },
        SelectionCase {
            case: "as many ids as a listing accepts, none of them created",
            filters: vec![(1..=MAX_FILTER_VALUES).map(unknown_ops_item_id).collect()],
            expected: vec![],
        },
        SelectionCase {
            case: "the id with a trail",
            filters: vec![vec![recorded.clone()]],
            expected: vec![recorded.clone()],
        },
    ];

    for SelectionCase {
        case,
        filters,
        expected,
    } in cases
    {
        let request = json!({
            "Filters": filters
                .iter()
                .map(|values| ops_item_filter(values))
                .collect::<Vec<Value>>(),
        });
        let (status, body) = list_in(&router, REGION, &request).await;
        if expected.is_empty() {
            assert_empty_page(case, status, &body);
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case}: {request} must be served, got {status} {body}"
        );
        let listed = body["Summaries"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            listed_ops_item_ids(&listed),
            expected,
            "case {case}: {request} must select {expected:?}, got {listed:?}"
        );
    }
}

/// One row of the filter table: which of the OpsItems the filters name, and how those
/// names are spread over the filters.
struct FilterCase<'a> {
    case: &'a str,
    /// The indexes into the OpsItems each filter names.
    filters: &'a [&'a [usize]],
    /// The indexes whose events the listing must report, newest first.
    expected: &'a [usize],
}

/// A filter narrows the listing to the OpsItems its values name; the values of one filter
/// are alternatives and two filters narrow together.
#[tokio::test]
async fn filters_narrow_the_trail_to_the_ops_items_they_name() {
    let (router, engine) = router_with_engine();
    let titles = [
        "A disk is filling up",
        "A node stopped reporting",
        "A build is failing",
    ];
    let mut created = Vec::new();
    for (age, title) in titles.iter().enumerate() {
        let ops_item_id = create(&router, "seed", title).await;
        seed_event(&engine, REGION, &ops_item_id, age as u64);
        created.push(ops_item_id);
    }

    let cases = [
        FilterCase {
            case: "no filter reports the whole trail",
            filters: &[],
            expected: &[2, 1, 0],
        },
        FilterCase {
            case: "one filter naming one OpsItem",
            filters: &[&[0]],
            expected: &[0],
        },
        FilterCase {
            case: "one filter naming another OpsItem",
            filters: &[&[2]],
            expected: &[2],
        },
        FilterCase {
            case: "one filter naming two OpsItems",
            filters: &[&[0, 2]],
            expected: &[2, 0],
        },
        FilterCase {
            case: "two filters that overlap in one OpsItem",
            filters: &[&[0, 1], &[1, 2]],
            expected: &[1],
        },
        FilterCase {
            case: "two filters that overlap in none",
            filters: &[&[0], &[2]],
            expected: &[],
        },
        FilterCase {
            case: "a filter naming no OpsItem at all",
            filters: &[&[]],
            expected: &[],
        },
    ];
    for FilterCase {
        case,
        filters,
        expected,
    } in cases
    {
        let request = if filters.is_empty() {
            json!({})
        } else {
            let filters: Vec<Value> = filters
                .iter()
                .map(|names| {
                    let ids: Vec<String> =
                        names.iter().map(|index| created[*index].clone()).collect();
                    ops_item_filter(&ids)
                })
                .collect();
            json!({ "Filters": filters })
        };
        let listed = summaries(&router, case, &request).await;
        let wanted: Vec<String> = expected
            .iter()
            .map(|index| created[*index].clone())
            .collect();
        assert_eq!(
            listed_ops_item_ids(&listed),
            wanted,
            "case {case}: {request} must select {wanted:?}, got {listed:?}"
        );

        // Every summary is projected from the event that was recorded, and an event id
        // identifies one event.
        for summary in &listed {
            assert_eq!(
                summary["Source"].as_str(),
                Some(EVENT_SOURCE),
                "case {case}: expected Source {EVENT_SOURCE}, got {summary}"
            );
            assert_eq!(
                summary["DetailType"].as_str(),
                Some(DETAIL_TYPE_UPDATE),
                "case {case}: expected DetailType {DETAIL_TYPE_UPDATE}, got {summary}"
            );
            let event_id = summary["EventId"].as_str().unwrap_or_default();
            assert!(
                is_uuid(event_id),
                "case {case}: expected an event id of the UUID form, got {event_id:?} in \
                 {summary}"
            );
            assert!(
                summary["CreatedTime"].as_f64().is_some_and(|at| at > 0.0),
                "case {case}: an event is dated by the write it records, got {summary}"
            );
        }
        let mut ids = listed_event_ids(&listed);
        ids.sort();
        let total = ids.len();
        ids.dedup();
        assert_eq!(
            ids.len(),
            total,
            "case {case}: every event carries its own event id, got {listed:?}"
        );
    }
}

/// An event is reported through the endpoint of the Region it belongs to and through no
/// other — and another Region's OpsItem id is a criterion this Region's events do not
/// meet, not a refusal.
#[tokio::test]
async fn a_trail_is_reported_only_through_its_own_region() {
    let (router, engine) = router_with_engine();
    // Three Regions, the first of which is the harness default while the others are not,
    // so what a listing reports came from the request's own credential scope rather than
    // from one Region standing in for every other.
    let regions = ["us-west-2", "eu-central-1", "ap-southeast-1"];
    let mut recorded = Vec::new();
    for (age, region) in regions.iter().enumerate() {
        let ops_item_id =
            create_in(&router, region, region, &format!("An OpsItem of {region}")).await;
        seed_event(&engine, region, &ops_item_id, age as u64);
        recorded.push((*region, ops_item_id));
    }

    for (region, ops_item_id) in &recorded {
        let (status, body) = list_in(&router, region, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {region}: ListOpsItemEvents must succeed, got {status} {body}"
        );
        let listed = body["Summaries"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            listed_ops_item_ids(&listed),
            vec![ops_item_id.clone()],
            "case {region}: only the events of {region} are reported, got {listed:?}"
        );
        // Another Region's OpsItem is not this Region's to report on, so a filter naming
        // it selects nothing here.
        for (other, other_id) in &recorded {
            if other == region {
                continue;
            }
            let (status, body) = list_in(
                &router,
                region,
                &json!({ "Filters": [ops_item_filter(std::slice::from_ref(other_id))] }),
            )
            .await;
            assert_empty_page(&format!("{region}, filtering on {other}'s OpsItem"), status, &body);
        }
    }
}

/// One row of the paging table: the page size the listing asks for.
struct PageCase<'a> {
    case: &'a str,
    max_results: i64,
}

/// A listing is paged by `MaxResults` and resumed by the `NextToken` it hands back: the
/// pages partition the trail, in the trail's own order, and the last page hands back none.
#[tokio::test]
async fn pages_partition_the_trail_and_resume_from_the_token() {
    let (router, engine) = router_with_engine();
    let events = 7;
    let mut recorded = Vec::new();
    for step in 0..events {
        let ops_item_id = create(&router, "seed", &format!("OpsItem number {step}")).await;
        seed_event(&engine, REGION, &ops_item_id, step as u64);
        recorded.push(ops_item_id);
    }
    let mut whole_trail = recorded.clone();
    whole_trail.reverse();

    let cases = [
        PageCase {
            case: "the smallest page the model allows",
            max_results: MIN_MAX_RESULTS,
        },
        PageCase {
            case: "a page smaller than the trail",
            max_results: 3,
        },
        PageCase {
            case: "a page one short of the trail",
            max_results: events as i64 - 1,
        },
        PageCase {
            case: "a page the size of the trail",
            max_results: events as i64,
        },
        PageCase {
            case: "the largest page the model allows",
            max_results: MAX_MAX_RESULTS,
        },
    ];
    for PageCase { case, max_results } in cases {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let request = match &token {
                None => json!({ "MaxResults": max_results }),
                Some(token) => json!({ "MaxResults": max_results, "NextToken": token }),
            };
            let body = list_ok(&router, case, &request).await;
            let listed = body["Summaries"].as_array().cloned().unwrap_or_default();
            pages += 1;
            assert!(
                listed.len() as i64 <= max_results,
                "case {case}: a page holds at most {max_results} events, got {listed:?}"
            );
            walked.extend(listed_ops_item_ids(&listed));
            match body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
            assert!(
                pages <= events + 1,
                "case {case}: paging must terminate, walked {walked:?}"
            );
        }
        assert_eq!(
            walked, whole_trail,
            "case {case}: the pages partition the trail in its own order, got {walked:?}"
        );
    }
}

/// One row of the refusal table: the request, the status and the error code it answers.
struct RefusalCase<'a> {
    case: &'a str,
    request: Value,
    code: &'a str,
}

/// Every way a listing cannot be served, and what it is refused with.
#[tokio::test]
async fn unservable_listings_are_refused() {
    let (router, engine) = router_with_engine();
    let held = create(&router, "seed", "A disk is filling up").await;
    seed_event(&engine, REGION, &held, 0);

    let past_bound: Vec<String> = (1..=MAX_FILTER_VALUES + 1).map(unknown_ops_item_id).collect();
    let (first_half, second_half) = past_bound.split_at(past_bound.len() / 2);
    let cases = [
        RefusalCase {
            case: "one value more than the listing accepts",
            request: json!({ "Filters": [ops_item_filter(&past_bound)] }),
            code: "OpsItemInvalidParameterException",
        },
        RefusalCase {
            case: "the same values spread over two filters",
            request: json!({
                "Filters": [
                    ops_item_filter(first_half),
                    ops_item_filter(second_half),
                ]
            }),
            code: "OpsItemInvalidParameterException",
        },
        RefusalCase {
            case: "more criteria than the listing accepts, none of them naming a value",
            request: json!({
                "Filters": (0..=MAX_FILTER_VALUES)
                    .map(|_| ops_item_filter(&[]))
                    .collect::<Vec<Value>>()
            }),
            code: "OpsItemInvalidParameterException",
        },
        RefusalCase {
            case: "a NextToken this service never minted",
            request: json!({ "NextToken": "not-a-token-of-ours" }),
            code: "OpsItemInvalidParameterException",
        },
        RefusalCase {
            case: "a NextToken of the right form naming no event of the result",
            request: json!({ "NextToken": "00000000-0000-4000-8000-000000000000" }),
            code: "OpsItemInvalidParameterException",
        },
        RefusalCase {
            case: "a page size under the modelled minimum",
            request: json!({ "MaxResults": MIN_MAX_RESULTS - 1 }),
            code: "ValidationException",
        },
        RefusalCase {
            case: "a page size over the modelled maximum",
            request: json!({ "MaxResults": MAX_MAX_RESULTS + 1 }),
            code: "ValidationException",
        },
    ];
    for RefusalCase {
        case,
        request,
        code,
    } in cases
    {
        let (status, body) = list_in(&router, REGION, &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case}: expected {code}, got {body}"
        );
    }
}

/// The refusal of a listing naming more OpsItem ids than it accepts states the limit class
/// and carries the diagnostic clause the service's own validation appends, whose request id
/// is this response's own.
#[tokio::test]
async fn the_filter_value_limit_refusal_is_worded_as_the_service_words_it() {
    let router = router();
    let past_bound: Vec<String> = (1..=MAX_FILTER_VALUES + 1).map(unknown_ops_item_id).collect();
    let request = json!({ "Filters": [ops_item_filter(&past_bound)] });

    let mut request_ids = Vec::new();
    // Twice, because the clause's request id is the one per-run field: two refusals of the
    // one request carry the same fixed text and two different ids.
    for case in ["the first refusal", "the second refusal"] {
        let (status, body) = list_in(&router, REGION, &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        let message = body["Message"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: the refusal carries a Message, got {body}"))
            .to_string();
        let opening = " (Service: AmazonSSM; Status Code: 400; \
                        Error Code: OpsItemInvalidParameterException; Request ID: ";
        let closing = "; Proxy: null)";
        let at = message.find(opening).unwrap_or_else(|| {
            panic!("case {case}: expected the clause {opening:?} in {message:?}")
        });
        assert_eq!(
            &message[..at], FILTER_VALUES_EXCEEDED,
            "case {case}: expected the sentence {FILTER_VALUES_EXCEEDED:?}, got {message:?}"
        );
        assert!(
            message.ends_with(closing),
            "case {case}: expected the Message to end with {closing:?}, got {message:?}"
        );
        let request_id = &message[at + opening.len()..message.len() - closing.len()];
        assert!(
            is_uuid(request_id),
            "case {case}: the clause carries a request id, got {request_id:?} in {message:?}"
        );
        request_ids.push(request_id.to_string());
    }
    assert_ne!(
        request_ids[0], request_ids[1],
        "two refusals report two request ids, got {request_ids:?}"
    );
}
