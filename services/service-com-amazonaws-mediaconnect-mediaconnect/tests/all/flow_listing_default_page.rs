// SPDX-License-Identifier: CC0-1.0

//! The page `GET /v1/flows` returns when the request names no `MaxResults`.
//!
//! The rule under test: a ListFlows request that omits `MaxResults` is paginated at the
//! page size the model documents for that member — "If MaxResults is not included in the
//! request, the service defaults to pagination with a maximum of 10 results per page"
//! (`smithy_query(query="doc",shape="ListFlowsRequest",member="MaxResults")`) — so the
//! page carries at most that many flows and a `NextToken` naming the key the next page
//! resumes at whenever more flows remain.
//!
//! The rows vary what the rule generalises over: how many flows the account holds,
//! relative to that default (none, one, one short of it, exactly it, one past it, and
//! several multiples of it), and whether the caller walks the listing with the default
//! page size or names one explicitly. Every call goes over the wire through the same
//! `MultiServiceRouter` the emulator's front door uses, on the harness `flow_lifecycle`
//! owns.

use serde_json::{json, Value};

use crate::flow_lifecycle::Emulator;

/// The documented default page size, from the model sentence quoted in the module docs.
const DOCUMENTED_DEFAULT: usize = 10;

/// The ARNs of a ListFlows response's `Flows` member, in the order the page carries them.
fn listed_arns(body: &Value) -> Vec<String> {
    body["flows"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|flow| flow["flowArn"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Create `count` flows over the wire and return every stored flow ARN in listing order.
async fn stored_flows(emulator: &Emulator, case: &str, count: usize) -> Vec<String> {
    for index in 0..count {
        let (status, code, body) = emulator
            .create_flow(json!({
                "name": format!("default-page-{index:03}"),
                "source": { "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
            }))
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: creating flow {index} expected 201, got {status} {code:?}: {body}"
        );
    }

    // The maximum of the modelled `MaxResults` range (1..1000) is the whole store here, so
    // this is the full listing order every assertion below is measured against.
    let (status, code, all) = emulator.list_flows("?maxResults=1000").await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: listing every flow expected 200, got {status} {code:?}: {all}"
    );
    let arns = listed_arns(&all);
    assert_eq!(
        arns.len(),
        count,
        "case {case:?}: every created flow is listed, got {all}"
    );
    arns
}

/// A ListFlows request that names no `MaxResults` holds at most the documented default of
/// ten flows, and names the key the next page resumes at whenever more are stored.
#[tokio::test]
async fn the_default_page_holds_the_documented_maximum() {
    // The rows span the default: under it, at it, and past it by one and by multiples.
    for count in [0_usize, 1, 9, 10, 11, 12, 25] {
        let case = format!("{count} stored flows");
        let emulator = Emulator::new();
        let expected = stored_flows(&emulator, &case, count).await;

        let (status, code, page) = emulator.list_flows("").await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: the default page expected 200, got {status} {code:?}: {page}"
        );
        let page_arns = listed_arns(&page);
        let expected_len = count.min(DOCUMENTED_DEFAULT);
        assert_eq!(
            page_arns,
            expected[..expected_len].to_vec(),
            "case {case:?}: a request with no MaxResults holds the first {expected_len} \
             stored flows, got {page}"
        );
        let expected_token = expected.get(DOCUMENTED_DEFAULT).map(String::as_str);
        assert_eq!(
            page["nextToken"].as_str(),
            expected_token,
            "case {case:?}: the default page names the key the next page resumes at only \
             when more flows remain, got {page}"
        );
    }
}

/// Walking the listing with no `MaxResults` at all partitions every stored flow exactly
/// once, in one stable order, with every page but the last holding exactly the documented
/// default — the same partition an explicit page size produces.
#[tokio::test]
async fn walking_with_no_max_results_pages_every_flow_exactly_once() {
    let case = "25 stored flows, walked with the default page size";
    let emulator = Emulator::new();
    let total = 25_usize;
    let expected = stored_flows(&emulator, case, total).await;

    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0_usize;
    loop {
        let query = match &token {
            Some(token) => format!("?nextToken={token}"),
            None => String::new(),
        };
        let (status, code, body) = emulator.list_flows(&query).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: page {pages} expected 200, got {status} {code:?}: {body}"
        );
        let page = listed_arns(&body);
        token = body["nextToken"].as_str().map(ToString::to_string);
        if token.is_some() {
            assert_eq!(
                page.len(),
                DOCUMENTED_DEFAULT,
                "case {case:?}: page {pages} carries a token, so it is full at the \
                 documented default, got {body}"
            );
        }
        assert!(
            page.len() <= DOCUMENTED_DEFAULT,
            "case {case:?}: page {pages} holds at most the documented default, got {body}"
        );
        seen.extend(page);
        pages += 1;
        if token.is_none() {
            break;
        }
        assert!(
            pages <= total,
            "case {case:?}: paging must terminate, got {pages} pages"
        );
    }

    assert_eq!(
        seen, expected,
        "case {case:?}: the default-size pages partition every stored flow exactly once, \
         in order"
    );
    assert_eq!(
        pages,
        total.div_ceil(DOCUMENTED_DEFAULT),
        "case {case:?}: {total} flows at {DOCUMENTED_DEFAULT} per page take that many pages"
    );

    // An explicit `MaxResults` still governs the page it names, so the default is a
    // default and not a cap the handler applies regardless.
    for page_size in [1_usize, 7, 10, 25, 1000] {
        let (status, code, body) = emulator
            .list_flows(&format!("?maxResults={page_size}"))
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "page size {page_size}: expected 200, got {status} {code:?}: {body}"
        );
        let page = listed_arns(&body);
        assert_eq!(
            page,
            expected[..page_size.min(total)].to_vec(),
            "page size {page_size}: the named page size governs the page, got {body}"
        );
    }
}
