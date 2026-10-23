// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListLinks`.
//!
//! The rule under test: `ListLinks` reports exactly the STORED links whose source
//! account is the one the request's credential speaks for and whose Region is the one
//! the request was signed for — each member read off that link's own stored record —
//! walked in one stable order and cut into pages no longer than the page size the
//! caller asked for, with a `NextToken` on every page but the last and none on it.
//!
//! Every case drives the real wire path (the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers), so what is
//! asserted is what a signed AWS SDK request observes.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_oam_oamservice::state::{link_arn, Link};
use service_com_amazonaws_oam_oamservice::ComAmazonawsOamOamserviceState;
use smithy_server_runtime::MultiServiceRouter;

use super::sinks_and_links::{
    call_as, create_sink_ok, emulator, emulator_on, error_code, field, ACCOUNT, AKID,
    RESOURCE_TYPES,
};

/// A second access key id, so a case can ask what another source account sees. A link
/// belongs to the source account the request speaks for, and that is read from this
/// credential.
const OTHER_AKID: &str = "AKIDOAMTEST2";

/// The maximum of the modelled `MaxResults` range, from
/// `smithy_query(query="operation",operation="ListLinks")`: `ListLinksMaxResults`
/// carries `range { min: 1, max: 5 }`. It is also the page size the handler applies
/// when the request omits `MaxResults`.
const MAX_RESULTS_MAX: i64 = 5;

/// The constant message `ListLinks` answers an unusable pagination token with, as
/// `src/ops/list_links.rs`'s `INVALID_PAGINATION_TOKEN` states it: a fixed string that
/// names nothing from the request.
const INVALID_PAGINATION_TOKEN: &str = "Invalid pagination token";

/// Every `Arn` `ListLinks` reports for `akid` in `region`, read one page at a time
/// with `max_results`, asserting the page contract as it walks: no page is longer than
/// asked for, a page that carries a `NextToken` is not empty, and the last page
/// carries no token.
async fn listed_link_arns(
    router: &MultiServiceRouter,
    case: &str,
    max_results: Option<i64>,
    akid: &str,
    region: &str,
) -> Vec<String> {
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
            call_as(router, "/ListLinks", &Value::Object(body), akid, region).await;
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
        for item in items {
            collected.push(field(case, item, "Arn"));
        }
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

/// Give `akid`'s source account `count` links to `sink_arn` in `region`, and answer
/// their ARNs.
///
/// The first one is created through `CreateLink` over the wire, so the source account
/// and Region recorded on it are exactly what the handlers derive from the request;
/// any further one is a copy of that STORED record under a freshly minted link id,
/// written through the state's own `insert_link`. Seeding is what makes a multi-page
/// enumeration reachable at all: `CreateSink` allows one sink per Region and
/// `CreateLink` one link per source account per sink, so the wire path alone can give
/// one source account only a single link in one Region.
async fn links_for(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    sink_arn: &str,
    akid: &str,
    region: &str,
    count: usize,
) -> BTreeSet<String> {
    let mut arns = BTreeSet::new();
    if count == 0 {
        return arns;
    }
    let body = json!({
        "LabelTemplate": "$AccountName",
        "ResourceTypes": ["AWS::Logs::LogGroup"],
        "SinkIdentifier": sink_arn,
    });
    let (status, created) = call_as(router, "/CreateLink", &body, akid, region).await;
    assert_eq!(
        status, 200,
        "CreateLink as {akid}: expected 200, got {status} {created:?}"
    );
    let first = field("seed", &created, "Arn");
    arns.insert(first.clone());

    let state = engine.state::<ComAmazonawsOamOamserviceState>();
    let mut store = state.lock();
    let seed = store
        .links
        .values()
        .find(|link| link.arn == first)
        .cloned()
        .unwrap_or_else(|| panic!("the created link {first} must be stored"));
    for _ in 1..count {
        let id = store
            .fresh_link_id()
            .expect("the store must mint a fresh link id");
        let arn = link_arn(region, &id);
        store.insert_link(Link {
            id,
            arn: arn.clone(),
            ..seed.clone()
        });
        arns.insert(arn);
    }
    arns
}

/// One row of the table below: a link to create, and what `ListLinks` must then
/// report for it.
struct LinkCase {
    case: &'static str,
    /// The Region the sink and the link live in.
    region: &'static str,
    sink_name: &'static str,
    /// The template `CreateLink` sends.
    label_template: &'static str,
    /// The `Label` the service reports for that template, with its variables
    /// resolved.
    expected_label: &'static str,
    resource_types: &'static [&'static str],
}

#[tokio::test]
async fn list_links_reports_the_created_link_from_its_stored_record() {
    // Rows differ along what the rule generalises over: the Region the link lives in,
    // the label template (a bare variable, a variable inside text, and text with no
    // variable at all) and the number and identity of the resource types — including
    // entries the report did not cite, taken from the modelled `ResourceType` catalog
    // in `RESOURCE_TYPES`.
    let cases: &[LinkCase] = &[
        LinkCase {
            case: "one metric resource type, bare variable label",
            region: "us-west-2",
            sink_name: "demo-sink",
            label_template: "$AccountName",
            expected_label: ACCOUNT,
            resource_types: &["AWS::CloudWatch::Metric"],
        },
        LinkCase {
            case: "a resource type the report did not cite",
            region: "us-east-1",
            sink_name: "trace-sink",
            label_template: "$AccountEmail",
            expected_label: ACCOUNT,
            resource_types: &["AWS::XRay::Trace"],
        },
        LinkCase {
            case: "variable inside surrounding text",
            region: "eu-central-1",
            sink_name: "Team.Sink_01-prod",
            label_template: "team-$AccountEmailNoDomain-src",
            expected_label: "team-000000000000-src",
            resource_types: &["AWS::Logs::LogGroup", "AWS::XRay::Trace"],
        },
        LinkCase {
            case: "a label with no variable at all",
            region: "ap-southeast-2",
            sink_name: "plain",
            label_template: "my-own-label",
            expected_label: "my-own-label",
            resource_types: &[
                "AWS::ApplicationInsights::Application",
                "AWS::InternetMonitor::Monitor",
                "AWS::ApplicationSignals::Service",
            ],
        },
        LinkCase {
            case: "every modelled resource type",
            region: "sa-east-1",
            sink_name: "all-types",
            label_template: "$AccountName",
            expected_label: ACCOUNT,
            resource_types: RESOURCE_TYPES,
        },
    ];

    for LinkCase {
        case,
        region,
        sink_name,
        label_template,
        expected_label,
        resource_types,
    } in cases
    {
        let router = emulator();
        let sink_arn = create_sink_ok(&router, sink_name, region).await;
        let (status, created) = call_as(
            &router,
            "/CreateLink",
            &json!({
                "LabelTemplate": label_template,
                "ResourceTypes": resource_types,
                "SinkIdentifier": sink_arn.clone(),
            }),
            AKID,
            region,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from CreateLink, got {status} {created:?}"
        );

        let (status, listed) = call_as(&router, "/ListLinks", &json!({}), AKID, region).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from ListLinks, got {status} {listed:?}"
        );
        let items = listed
            .get("Items")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: Items is @required, got {listed:?}"));
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: expected exactly the one created link, got {listed:?}"
        );
        let item = &items[0];

        // Every reported member is the one the create stored, not one echoed out of
        // the list request (which carried no link values at all).
        for member in ["Arn", "Id", "Label", "SinkArn"] {
            assert_eq!(
                field(case, item, member),
                field(case, &created, member),
                "case {case:?}: expected the stored {member}, got {item:?}"
            );
        }
        assert_eq!(
            field(case, item, "Label"),
            *expected_label,
            "case {case:?}: expected the resolved label, got {item:?}"
        );
        assert_eq!(
            field(case, item, "SinkArn"),
            sink_arn,
            "case {case:?}: expected the sink the link was attached to, got {item:?}"
        );
        assert_eq!(
            item.get("ResourceTypes"),
            Some(&json!(resource_types)),
            "case {case:?}: expected the stored resource types, got {item:?}"
        );
        // `Id` is the ARN's last segment, as it is everywhere else in this service.
        let arn = field(case, item, "Arn");
        assert_eq!(
            arn,
            format!(
                "arn:aws:oam:{region}:{ACCOUNT}:link/{}",
                field(case, item, "Id")
            ),
            "case {case:?}: expected Id to be the ARN's id segment, got {item:?}"
        );

        // One source account's list is not another's: the same Region, another
        // credential, no links.
        let (status, other) = call_as(&router, "/ListLinks", &json!({}), OTHER_AKID, region).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 for the other source account, got {other:?}"
        );
        assert_eq!(
            other.get("Items"),
            Some(&json!([])),
            "case {case:?}: another source account holds no links, got {other:?}"
        );

        // And the enumeration is regional: a different Region's endpoint does not
        // report this link.
        let elsewhere = if *region == "us-west-2" {
            "us-east-1"
        } else {
            "us-west-2"
        };
        let (status, abroad) = call_as(&router, "/ListLinks", &json!({}), AKID, elsewhere).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 in {elsewhere}, got {abroad:?}"
        );
        assert_eq!(
            abroad.get("Items"),
            Some(&json!([])),
            "case {case:?}: a link in {region} is not reported in {elsewhere}, got {abroad:?}"
        );
    }
}

#[tokio::test]
async fn pagination_reports_every_link_exactly_once_in_a_stable_order() {
    // Rows differ along the count of stored links and the requested page size,
    // including both published bounds of `MaxResults` (1 and 5) and its absence.
    let cases: &[(&str, usize, Option<i64>)] = &[
        ("no links, default page size", 0, None),
        ("no links, smallest page", 0, Some(1)),
        ("one link, default page size", 1, None),
        ("one link, smallest page", 1, Some(1)),
        ("two links, one per page", 2, Some(1)),
        ("three links, one per page", 3, Some(1)),
        ("three links, two per page", 3, Some(2)),
        ("five links, two per page", 5, Some(2)),
        ("five links, largest page", 5, Some(MAX_RESULTS_MAX)),
        ("five links, default page size", 5, None),
    ];

    for (case, count, max_results) in cases {
        let region = "us-west-2";
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_on(Arc::clone(&engine));
        let sink_arn = create_sink_ok(&router, "demo-sink", region).await;
        let stored = links_for(&router, &engine, &sink_arn, AKID, region, *count).await;
        assert_eq!(
            stored.len(),
            *count,
            "case {case:?}: expected {count} stored links, got {stored:?}"
        );

        let collected = listed_link_arns(&router, case, *max_results, AKID, region).await;
        assert_eq!(
            collected.len(),
            *count,
            "case {case:?}: expected {count} reported links with no repeats, got {collected:?}"
        );
        assert_eq!(
            collected.iter().cloned().collect::<BTreeSet<String>>(),
            stored,
            "case {case:?}: expected exactly the stored links, got {collected:?}"
        );
        let mut sorted = collected.clone();
        sorted.sort();
        assert_eq!(
            collected, sorted,
            "case {case:?}: expected the stable ascending-id order, got {collected:?}"
        );
    }
}

#[tokio::test]
async fn list_links_refuses_a_page_size_or_token_it_cannot_honour() {
    let region = "us-west-2";
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    let sink_arn = create_sink_ok(&router, "demo-sink", region).await;
    links_for(&router, &engine, &sink_arn, AKID, region, 3).await;

    // Both published bounds of the modelled 1..5 range are accepted.
    for max_results in [1, MAX_RESULTS_MAX] {
        let (status, listed) = call_as(
            &router,
            "/ListLinks",
            &json!({ "MaxResults": max_results }),
            AKID,
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
        let (status, got) = call_as(
            &router,
            "/ListLinks",
            &json!({ "MaxResults": max_results }),
            AKID,
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

    // A token the service cannot resume is refused with HTTP 400
    // `ValidationException` and the constant message, whatever is wrong with it.
    let token_600 = "a".repeat(600);
    let refusals: &[(&str, Value)] = &[
        (
            "a NextToken that is not hexadecimal",
            json!({ "NextToken": "not-a-token" }),
        ),
        // Hexadecimal, decodes to text, but carries no enumeration fingerprint.
        (
            "a NextToken with no separator",
            json!({ "NextToken": hex::encode("0123456789abcdef") }),
        ),
        // Hexadecimal with a separator, but a fingerprint this call never issued.
        (
            "a NextToken from no enumeration",
            json!({ "NextToken": hex::encode("deadbeef:11111111-2222-4333-8444-555555555555") }),
        ),
        (
            "a NextToken far longer than any issued one",
            json!({ "NextToken": token_600 }),
        ),
    ];
    for (case, body) in refusals {
        let (status, got) = call_as(&router, "/ListLinks", body, AKID, region).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {got:?}"
        );
        assert_eq!(
            error_code(&got),
            "ValidationException",
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
    let sink_arn = create_sink_ok(&router, "demo-sink", region).await;
    let stored = links_for(&router, &engine, &sink_arn, AKID, region, 2).await;

    let (status, page) = call_as(
        &router,
        "/ListLinks",
        &json!({ "MaxResults": 1 }),
        AKID,
        region,
    )
    .await;
    assert_eq!(status, 200, "the first page: expected 200, got {page:?}");
    let token = field("first page", &page, "NextToken");
    let first_arn = page
        .get("Items")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .map(|item| field("first page", item, "Arn"))
        .unwrap_or_else(|| panic!("the first page must carry one item, got {page:?}"));

    // The token resumes this caller's own enumeration, and reports the rest of it.
    let (status, resumed) = call_as(
        &router,
        "/ListLinks",
        &json!({ "NextToken": token.clone() }),
        AKID,
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
        "the two pages together must be exactly the stored links, got {resumed_arns:?}"
    );

    // Another source account cannot resume it, and neither can another Region: the
    // token names the enumeration it was issued for.
    for (case, akid, region) in [
        ("another source account", OTHER_AKID, region),
        ("another Region", AKID, "us-east-1"),
    ] {
        let (status, got) = call_as(
            &router,
            "/ListLinks",
            &json!({ "NextToken": token.clone() }),
            akid,
            region,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {got:?}"
        );
        assert_eq!(
            error_code(&got),
            "ValidationException",
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
async fn a_deleted_link_leaves_the_enumeration() {
    let region = "us-west-2";
    let router = emulator();
    let sink_arn = create_sink_ok(&router, "demo-sink", region).await;
    let (status, created) = call_as(
        &router,
        "/CreateLink",
        &json!({
            "LabelTemplate": "$AccountName",
            "ResourceTypes": ["AWS::CloudWatch::Metric"],
            "SinkIdentifier": sink_arn.clone(),
        }),
        AKID,
        region,
    )
    .await;
    assert_eq!(status, 200, "CreateLink: expected 200, got {created:?}");
    let arn = field("created", &created, "Arn");
    assert_eq!(
        listed_link_arns(&router, "before the delete", None, AKID, region).await,
        vec![arn.clone()],
        "the created link must be reported"
    );

    let (status, deleted) = call_as(
        &router,
        "/DeleteLink",
        &json!({ "Identifier": arn.clone() }),
        AKID,
        region,
    )
    .await;
    assert_eq!(status, 200, "DeleteLink: expected 200, got {deleted:?}");

    let (status, listed) = call_as(&router, "/ListLinks", &json!({}), AKID, region).await;
    assert_eq!(status, 200, "expected 200, got {listed:?}");
    assert_eq!(
        listed.get("Items"),
        Some(&json!([])),
        "Items is @required, so an empty enumeration is [], got {listed:?}"
    );
}
