// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the pagination token both of this service's listings hand
//! out, driven through the real wire path (the production `MultiServiceRouter`,
//! restJson1 serialization and the handlers), so what is asserted is what a signed AWS
//! SDK request observes.
//!
//! The rule under test: a paginated listing reports its output token member on EVERY
//! page — an opaque resume key while records remain, and the EMPTY STRING once the
//! listing is exhausted, never a member the body leaves out. Both listings document it
//! that way: `ListConfigurationManagers$NextToken` is "The token to use when requesting
//! the next set of configuration managers. If there are no additional operations to
//! return, the string is empty."
//! (`smithy_query(query="doc",shape="ListConfigurationManagersOutput",member="NextToken")`)
//! and `ListConfigurationsOutput$NextToken` says the same of its items
//! (`smithy_query(query="doc",shape="ListConfigurationsOutput",member="NextToken")`).
//! So a client decides whether to ask for another page by whether the token it was handed
//! is empty — and the empty token the service itself hands out names no record, so a
//! request that sends it back as `StartingToken` means exactly what one carrying no token
//! means.
//!
//! The two operations covered here are every paginated operation this service has: only
//! `ListConfigurationManagers` and `ListConfigurations` carry the `@paginated`
//! `outputToken` (`smithy_query(query="digest")`).

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::MAX_ITEMS_LIMIT;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ssmquicksetup_quicksetup::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `ssm-quicksetup` in [`REGION`], the signal the router routes on
/// and the handlers read the region from.
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("ssm-quicksetup.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDQUICKSETUP/20260101/{REGION}/ssm-quicksetup/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    (status, parsed)
}

/// The minimum a create needs: one definition naming a published Quick Setup type,
/// somewhere to deploy to, the Regions to deploy into and every parameter that type's
/// documented table marks `(Required)`. One organizational unit and one Region, so the
/// create deploys exactly ONE configuration per manager and both listings hold the same
/// number of records.
fn create_body(index: usize) -> Value {
    json!({
        "Name": format!("manager-{index}"),
        "ConfigurationDefinitions": [{
            "Type": "AWSQuickSetupType-SSMOpsCenter",
            "Parameters": {
                "DelegatedAccountId": "111122223333",
                "TargetOrganizationalUnits": "ou-1",
                "TargetRegions": REGION,
            },
        }],
    })
}

/// Fill the store with `count` managers, each deploying one configuration.
async fn fill(router: &MultiServiceRouter, case: &str, count: usize) {
    for index in 0..count {
        let (status, answer) = call(
            router,
            "POST",
            "/configurationManager",
            Some(&create_body(index)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: CreateConfigurationManager must succeed, got {status} / {answer:?}"
        );
    }
}

/// One listing of this service: where to POST it, the member carrying its rows, and the
/// member identifying one row.
struct Listing {
    uri: &'static str,
    rows: &'static str,
    id: &'static str,
}

/// Every paginated listing this service serves.
const LISTINGS: [Listing; 2] = [
    Listing {
        uri: "/listConfigurationManagers",
        rows: "ConfigurationManagersList",
        id: "ManagerArn",
    },
    Listing {
        uri: "/listConfigurations",
        rows: "ConfigurationsList",
        id: "Id",
    },
];

/// The token one page reports, asserting the member is THERE and a string: a page that
/// leaves it out is the defect this suite is about.
fn reported_token(case: &str, answer: &Value) -> String {
    let member = answer
        .get("NextToken")
        .unwrap_or_else(|| panic!("case {case}: every page reports NextToken, got {answer:?}"));
    member
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: NextToken is a string, got {member:?}"))
        .to_string()
}

/// One page of `listing`, asserting it succeeded.
async fn page_of(
    router: &MultiServiceRouter,
    case: &str,
    listing: &Listing,
    max_items: Option<i64>,
    token: Option<&str>,
) -> Value {
    let mut body = Map::new();
    if let Some(max_items) = max_items {
        body.insert("MaxItems".to_string(), json!(max_items));
    }
    if let Some(token) = token {
        body.insert("StartingToken".to_string(), json!(token));
    }
    let (status, answer) = call(router, "POST", listing.uri, Some(&Value::Object(body))).await;
    assert_eq!(
        status, 200,
        "case {case}: {} must succeed, got {status} / {answer:?}",
        listing.uri
    );
    answer
}

/// The ids one page reports.
fn ids_of(case: &str, listing: &Listing, answer: &Value) -> Vec<String> {
    answer
        .get(listing.rows)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: expected a {}, got {answer:?}", listing.rows))
        .iter()
        .map(|row| {
            row.get(listing.id)
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: expected a {}, got {row:?}", listing.id))
                .to_string()
        })
        .collect()
}

/// The token is the empty string exactly on the page that exhausts the listing, and an
/// opaque resume key on every page before it — whatever the size of the store and the
/// page bound, and for either listing. Paging on those tokens still visits every record
/// exactly once.
#[tokio::test]
async fn an_exhausted_listing_reports_the_empty_token_and_every_earlier_page_a_resume_key() {
    // (records in the store, MaxItems asked for) — the rows differ along what the rule
    // generalises over: an empty store, a store the page exactly fills, a store needing
    // several pages, an unbounded page, and both published ends of the modelled
    // `MaxItems` `@range(1, 100)`.
    let cases: [(usize, Option<i64>); 7] = [
        (0, None),
        (0, Some(1)),
        (1, Some(1)),
        (1, None),
        (3, Some(1)),
        (5, Some(2)),
        (4, Some(i64::from(MAX_ITEMS_LIMIT))),
    ];

    for (count, max_items) in cases {
        for listing in &LISTINGS {
            let case = format!("{} / {count} records, MaxItems={max_items:?}", listing.uri);
            let router = emulator();
            fill(&router, &case, count).await;

            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                let answer = page_of(&router, &case, listing, max_items, token.as_deref()).await;
                let rows = ids_of(&case, listing, &answer);
                if let Some(max_items) = max_items {
                    assert!(
                        rows.len() <= max_items as usize,
                        "case {case}: a page may not exceed MaxItems, got {} rows",
                        rows.len()
                    );
                }
                seen.extend(rows);
                pages += 1;
                assert!(
                    pages <= count + 1,
                    "case {case}: paging did not terminate after {pages} pages"
                );

                let reported = reported_token(&case, &answer);
                // Everything the store holds is already on the pages read so far exactly
                // when this page exhausted the listing, which is exactly when the token
                // must be empty.
                let exhausted = seen.len() == count;
                assert_eq!(
                    reported.is_empty(),
                    exhausted,
                    "case {case}: after {} of {count} records the token must be {}, got {reported:?}",
                    seen.len(),
                    if exhausted { "empty" } else { "a resume key" }
                );
                if reported.is_empty() {
                    break;
                }
                token = Some(reported);
            }

            assert_eq!(
                seen.len(),
                count,
                "case {case}: paging must visit every record exactly once, got {seen:?}"
            );
            assert_eq!(
                seen.iter().cloned().collect::<BTreeSet<String>>().len(),
                count,
                "case {case}: no record may be served twice, got {seen:?}"
            );
        }
    }
}

/// The empty token a listing hands out names no record, so sending it back as
/// `StartingToken` is accepted and answers the first page — the same rows and the same
/// token a request carrying no token at all answers. (A token this service never issued
/// is still refused; that is pinned by `tests/all/configuration_managers.rs`.)
#[tokio::test]
async fn the_empty_token_a_listing_hands_out_reads_as_no_token_at_all() {
    // Store sizes either side of the page bound below, so the resumed listing is an
    // exhausted one in some rows and one with a next page in others.
    for count in [0usize, 1, 3] {
        for max_items in [None, Some(2)] {
            for listing in &LISTINGS {
                let case = format!("{} / {count} records, MaxItems={max_items:?}", listing.uri);
                let router = emulator();
                fill(&router, &case, count).await;

                let without = page_of(&router, &case, listing, max_items, None).await;
                let with_empty = page_of(&router, &case, listing, max_items, Some("")).await;
                assert_eq!(
                    ids_of(&case, listing, &with_empty),
                    ids_of(&case, listing, &without),
                    "case {case}: the empty token must answer the first page, got {with_empty:?}"
                );
                assert_eq!(
                    reported_token(&case, &with_empty),
                    reported_token(&case, &without),
                    "case {case}: the empty token must answer the first page's own token, got \
                     {with_empty:?}"
                );
            }
        }
    }
}
