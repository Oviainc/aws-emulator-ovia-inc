// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListSinks`.
//!
//! The rule under test: `ListSinks` reports exactly the STORED sinks of the Region the
//! request was signed for — each member read off that sink's own stored record — walked
//! in one stable order and cut into pages no longer than the page size the caller asked
//! for, with a `NextToken` on every page but the last and none on it.
//!
//! Every case drives the real wire path (the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers), so what is
//! asserted is what a signed AWS SDK request observes.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_oam_oamservice::state::{sink_arn, Sink};
use service_com_amazonaws_oam_oamservice::ComAmazonawsOamOamserviceState;
use smithy_server_runtime::MultiServiceRouter;

use super::sinks_and_links::{
    call, call_as, create_sink_ok, emulator, emulator_on, error_code, field, ACCOUNT, AKID,
};

/// A second access key id. A sink belongs to the monitoring ACCOUNT, not to the
/// credential that created it, so a second credential in the same Region must see the
/// very same enumeration — which is what this key id is for.
const OTHER_AKID: &str = "AKIDOAMTEST2";

/// The maximum of the modelled `MaxResults` range, from
/// `smithy_query(query="operation",operation="ListSinks")`: `ListSinksMaxResults`
/// carries `range { min: 1, max: 100 }`. It is also the page size the handler applies
/// when the request omits `MaxResults`.
const MAX_RESULTS_MAX: i64 = 100;

/// The constant message `ListSinks` answers an unusable pagination token with, as
/// `src/ops/list_sinks.rs`'s `decode_page_token` states it: a fixed string that names
/// nothing from the request.
const INVALID_PAGINATION_TOKEN: &str =
    "NextToken is not a pagination token this ListSinks call can resume";

/// Every item `ListSinks` reports in `region` for `akid`, read one page at a time with
/// `max_results`, asserting the page contract as it walks: no page is longer than asked
/// for, a page that carries a `NextToken` is not empty, and the last page carries none.
async fn listed_sinks(
    router: &MultiServiceRouter,
    case: &str,
    max_results: Option<i64>,
    akid: &str,
    region: &str,
) -> Vec<Value> {
    let mut collected = Vec::new();
    let mut next_token: Option<String> = None;
    for page in 1..=32 {
        let mut body = Map::new();
        if let Some(max_results) = max_results {
            body.insert("MaxResults".to_string(), Value::from(max_results));
        }
        if let Some(token) = &next_token {
            body.insert("NextToken".to_string(), Value::String(token.clone()));
        }
        let (status, listed) =
            call_as(router, "/ListSinks", &Value::Object(body), akid, region).await;
        assert_eq!(
            status, 200,
            "case {case:?} page {page}: expected 200, got {status} {listed:?}"
        );
        let items = listed
            .get("Items")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("case {case:?} page {page}: Items is @required, got {listed:?}")
            });
        if let Some(max_results) = max_results {
            assert!(
                items.len() as i64 <= max_results,
                "case {case:?} page {page}: expected at most {max_results} items, got {}",
                items.len()
            );
        }
        collected.extend(items.iter().cloned());
        next_token = listed
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        match &next_token {
            None => return collected,
            Some(token) => assert!(
                !items.is_empty(),
                "case {case:?} page {page}: a page carrying NextToken {token:?} must carry items"
            ),
        }
    }
    panic!("case {case:?}: pagination did not terminate after 32 pages");
}

/// Give `region` `count` sinks, and answer their ARNs.
///
/// The first one is created through `CreateSink` over the wire, so the Region recorded
/// on it is exactly what the handlers derive from the request; any further one is a
/// copy of that STORED record under a freshly minted id and a distinct name, written
/// through the state's own `insert_sink`. Seeding is what makes a multi-page
/// enumeration reachable at all: `CreateSink` allows one sink per Region, so the wire
/// path alone can give one Region only a single sink.
async fn sinks_for(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    region: &str,
    count: usize,
) -> BTreeSet<String> {
    let mut arns = BTreeSet::new();
    if count == 0 {
        return arns;
    }
    let first = create_sink_ok(router, "demo-sink", region).await;
    arns.insert(first.clone());

    let state = engine.state::<ComAmazonawsOamOamserviceState>();
    let mut store = state.lock();
    let seed = store
        .sinks
        .values()
        .find(|sink| sink.arn == first)
        .cloned()
        .unwrap_or_else(|| panic!("the created sink {first} must be stored"));
    for extra in 1..count {
        let id = store
            .fresh_sink_id()
            .expect("the store must mint a fresh sink id");
        let arn = sink_arn(region, &id);
        store.insert_sink(Sink {
            id,
            arn: arn.clone(),
            name: format!("seeded-sink-{extra}"),
            ..seed.clone()
        });
        arns.insert(arn);
    }
    arns
}

/// One row of the table below: a sink to create, and the Region to create it in.
struct SinkCase<'a> {
    case: &'a str,
    region: &'a str,
    /// A name inside the modelled `CreateSinkInput` `Name` pattern
    /// `^[a-zA-Z0-9_\.\-]{1,255}$` (from
    /// `smithy_query(query="operation",operation="CreateSink")`).
    name: &'a str,
}

#[tokio::test]
async fn list_sinks_reports_the_created_sink_from_its_stored_record() {
    // Rows differ along what the rule generalises over: the Region the sink lives in
    // (including the emulator's default one, which a request with no Region in its
    // credential scope falls back to) and the form and length of the name — each one
    // inside the modelled `Name` pattern, the last being `{1,255}`'s published upper
    // bound.
    let longest_name = "n".repeat(255);
    let cases: Vec<SinkCase> = vec![
        SinkCase {
            case: "a plain name in us-west-2",
            region: "us-west-2",
            name: "demo-sink",
        },
        SinkCase {
            case: "the default Region",
            region: "us-east-1",
            name: "default-region-sink",
        },
        SinkCase {
            case: "dots and underscores",
            region: "eu-central-1",
            name: "Team.Sink_01-prod",
        },
        SinkCase {
            case: "a single-character name",
            region: "ap-southeast-2",
            name: "s",
        },
        SinkCase {
            case: "digits only",
            region: "sa-east-1",
            name: "0123456789",
        },
        SinkCase {
            case: "the longest name the pattern admits",
            region: "ca-central-1",
            name: &longest_name,
        },
    ];

    for SinkCase { case, region, name } in &cases {
        let router = emulator();
        let (status, created) = call(&router, "/CreateSink", &json!({ "Name": name }), region).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from CreateSink, got {status} {created:?}"
        );

        let items = listed_sinks(&router, case, None, AKID, region).await;
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: expected exactly the one created sink, got {items:?}"
        );
        let item = &items[0];

        // Every reported member is the one the create stored, not one echoed out of
        // the list request (which carried no sink values at all).
        for member in ["Arn", "Id", "Name"] {
            assert_eq!(
                field(case, item, member),
                field(case, &created, member),
                "case {case:?}: expected the stored {member}, got {item:?}"
            );
        }
        assert_eq!(
            field(case, item, "Name"),
            *name,
            "case {case:?}: expected the stored name, got {item:?}"
        );
        // `Id` is the ARN's last segment, as it is everywhere else in this service.
        assert_eq!(
            field(case, item, "Arn"),
            format!(
                "arn:aws:oam:{region}:{ACCOUNT}:sink/{}",
                field(case, item, "Id")
            ),
            "case {case:?}: expected Id to be the ARN's id segment, got {item:?}"
        );

        // The round trip: the implemented reader answers the same record for the ARN
        // the list reported.
        let (status, fetched) = call(
            &router,
            "/GetSink",
            &json!({ "Identifier": field(case, item, "Arn") }),
            region,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetSink, got {status} {fetched:?}"
        );
        for member in ["Arn", "Id", "Name"] {
            assert_eq!(
                field(case, item, member),
                field(case, &fetched, member),
                "case {case:?}: GetSink must report the listed {member}, got {fetched:?}"
            );
        }

        // A sink belongs to the monitoring account, not to one credential: another
        // credential in the same Region sees the same enumeration.
        let other = listed_sinks(&router, case, None, OTHER_AKID, region).await;
        assert_eq!(
            other, items,
            "case {case:?}: another credential must see the same sinks, got {other:?}"
        );

        // And the enumeration is regional: a different Region's endpoint does not
        // report this sink.
        let elsewhere = if *region == "us-west-2" {
            "us-east-1"
        } else {
            "us-west-2"
        };
        let (status, abroad) = call(&router, "/ListSinks", &json!({}), elsewhere).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 in {elsewhere}, got {abroad:?}"
        );
        assert_eq!(
            abroad.get("Items"),
            Some(&json!([])),
            "case {case:?}: a sink in {region} is not reported in {elsewhere}, got {abroad:?}"
        );
    }
}

#[tokio::test]
async fn pagination_reports_every_sink_exactly_once_in_a_stable_order() {
    // Rows differ along the count of stored sinks and the requested page size,
    // including both published bounds of `MaxResults` (1 and 100) and its absence.
    let cases: &[(&str, usize, Option<i64>)] = &[
        ("no sinks, default page size", 0, None),
        ("no sinks, smallest page", 0, Some(1)),
        ("one sink, default page size", 1, None),
        ("one sink, smallest page", 1, Some(1)),
        ("two sinks, one per page", 2, Some(1)),
        ("three sinks, one per page", 3, Some(1)),
        ("three sinks, two per page", 3, Some(2)),
        ("seven sinks, three per page", 7, Some(3)),
        ("seven sinks, largest page", 7, Some(MAX_RESULTS_MAX)),
        ("seven sinks, default page size", 7, None),
    ];

    for (case, count, max_results) in cases {
        let region = "us-west-2";
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_on(Arc::clone(&engine));
        let stored = sinks_for(&router, &engine, region, *count).await;
        assert_eq!(
            stored.len(),
            *count,
            "case {case:?}: expected {count} stored sinks, got {stored:?}"
        );

        let items = listed_sinks(&router, case, *max_results, AKID, region).await;
        let collected: Vec<String> = items
            .iter()
            .map(|item| field(case, item, "Arn"))
            .collect::<Vec<String>>();
        assert_eq!(
            collected.len(),
            *count,
            "case {case:?}: expected {count} reported sinks with no repeats, got {collected:?}"
        );
        assert_eq!(
            collected.iter().cloned().collect::<BTreeSet<String>>(),
            stored,
            "case {case:?}: expected exactly the stored sinks, got {collected:?}"
        );
        // The order is the store's own (ascending sink id), and the ARN's last segment
        // is that id, so the reported ARNs come out sorted.
        let mut sorted = collected.clone();
        sorted.sort();
        assert_eq!(
            collected, sorted,
            "case {case:?}: expected the stable ascending-id order, got {collected:?}"
        );
    }
}

#[tokio::test]
async fn list_sinks_refuses_a_page_size_or_token_it_cannot_honour() {
    let region = "us-west-2";
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    sinks_for(&router, &engine, region, 3).await;

    // Both published bounds of the modelled 1..100 range are accepted.
    for max_results in [1, MAX_RESULTS_MAX] {
        let (status, listed) = call(
            &router,
            "/ListSinks",
            &json!({ "MaxResults": max_results }),
            region,
        )
        .await;
        assert_eq!(
            status, 200,
            "MaxResults {max_results}: expected 200, got {status} {listed:?}"
        );
    }

    // One step outside either bound is a modelled-constraint violation, which this
    // service words as `InvalidParameterException` (`src/service.rs`).
    for max_results in [0, MAX_RESULTS_MAX + 1] {
        let (status, got) = call(
            &router,
            "/ListSinks",
            &json!({ "MaxResults": max_results }),
            region,
        )
        .await;
        assert_eq!(
            status, 400,
            "MaxResults {max_results}: expected 400, got {status} {got:?}"
        );
        assert_eq!(
            error_code(&got),
            "InvalidParameterException",
            "MaxResults {max_results}: got {got:?}"
        );
    }

    // A token the service cannot resume is refused with HTTP 400, the modelled
    // `InvalidParameterException` and the constant message, whatever is wrong with it.
    let token_600 = "a".repeat(600);
    let refusals: &[(&str, Value)] = &[
        (
            "a NextToken that is not hexadecimal",
            json!({ "NextToken": "not-a-token" }),
        ),
        // Hexadecimal, decodes to text, but names no Region to resume.
        (
            "a NextToken with no separator",
            json!({ "NextToken": hex::encode("0123456789abcdef") }),
        ),
        // Hexadecimal with a separator, but a Region this call is not enumerating.
        (
            "a NextToken from another Region",
            json!({ "NextToken": hex::encode("eu-west-1:11111111-2222-4333-8444-555555555555") }),
        ),
        // The right Region, but nothing usable as a position in the enumeration.
        (
            "a NextToken with an empty id",
            json!({ "NextToken": hex::encode("us-west-2:") }),
        ),
        (
            "a NextToken far longer than any issued one",
            json!({ "NextToken": token_600 }),
        ),
    ];
    for (case, body) in refusals {
        let (status, got) = call(&router, "/ListSinks", body, region).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {got:?}"
        );
        assert_eq!(
            error_code(&got),
            "InvalidParameterException",
            "case {case:?}: got {got:?}"
        );
        assert_eq!(
            field(case, &got, "message"),
            INVALID_PAGINATION_TOKEN,
            "case {case:?}: expected the constant token message, got {got:?}"
        );
    }
}

#[tokio::test]
async fn a_page_token_resumes_only_the_enumeration_it_was_issued_for() {
    let region = "us-west-2";
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    let stored = sinks_for(&router, &engine, region, 2).await;

    let (status, page) = call(&router, "/ListSinks", &json!({ "MaxResults": 1 }), region).await;
    assert_eq!(status, 200, "the first page: expected 200, got {page:?}");
    let token = field("first page", &page, "NextToken");
    let first_arn = page
        .get("Items")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .map(|item| field("first page", item, "Arn"))
        .unwrap_or_else(|| panic!("the first page must carry one item, got {page:?}"));

    // The token resumes this Region's enumeration, and reports the rest of it.
    let (status, resumed) = call(
        &router,
        "/ListSinks",
        &json!({ "NextToken": token.clone() }),
        region,
    )
    .await;
    assert_eq!(status, 200, "the resumed page: expected 200, got {resumed:?}");
    let resumed_arns: BTreeSet<String> = resumed
        .get("Items")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("Items is @required, got {resumed:?}"))
        .iter()
        .map(|item| field("resumed page", item, "Arn"))
        .collect();
    assert!(
        !resumed_arns.contains(&first_arn),
        "the resumed page must not repeat {first_arn}, got {resumed_arns:?}"
    );
    assert_eq!(
        resumed_arns
            .union(&BTreeSet::from([first_arn.clone()]))
            .cloned()
            .collect::<BTreeSet<String>>(),
        stored,
        "the two pages together must be exactly the stored sinks, got {resumed_arns:?}"
    );

    // Another Region cannot resume it: the token names the enumeration it was issued
    // for.
    for elsewhere in ["us-east-1", "eu-central-1"] {
        let (status, got) = call(
            &router,
            "/ListSinks",
            &json!({ "NextToken": token.clone() }),
            elsewhere,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {elsewhere:?}: expected 400, got {status} {got:?}"
        );
        assert_eq!(
            error_code(&got),
            "InvalidParameterException",
            "case {elsewhere:?}: got {got:?}"
        );
        assert_eq!(
            field(elsewhere, &got, "message"),
            INVALID_PAGINATION_TOKEN,
            "case {elsewhere:?}: expected the constant token message, got {got:?}"
        );
    }
}

#[tokio::test]
async fn a_deleted_sink_leaves_the_enumeration() {
    let region = "us-west-2";
    let router = emulator();
    let arn = create_sink_ok(&router, "demo-sink", region).await;
    let listed = listed_sinks(&router, "before the delete", None, AKID, region).await;
    assert_eq!(
        listed
            .iter()
            .map(|item| field("before the delete", item, "Arn"))
            .collect::<Vec<String>>(),
        vec![arn.clone()],
        "the created sink must be reported"
    );

    let (status, deleted) = call(
        &router,
        "/DeleteSink",
        &json!({ "Identifier": arn.clone() }),
        region,
    )
    .await;
    assert_eq!(status, 200, "DeleteSink: expected 200, got {deleted:?}");

    let (status, listed) = call(&router, "/ListSinks", &json!({}), region).await;
    assert_eq!(status, 200, "expected 200, got {listed:?}");
    assert_eq!(
        listed.get("Items"),
        Some(&json!([])),
        "Items is @required, so an empty enumeration is [], got {listed:?}"
    );
}
