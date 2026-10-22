// SPDX-License-Identifier: CC0-1.0

//! What `DescribePendingAggregationRequests` reports, end to end over the wire.
//!
//! A pending aggregation request is an aggregator account that has asked to aggregate
//! this account's data and is not authorized yet
//! (`PendingAggregationRequest`: "an aggregator account that is requesting
//! authorization but is not yet authorized"). The REQUESTER is another account's
//! aggregator, and the `PutConfigurationAggregator` that creates the entry reaches that
//! account's Config rather than this one, so no operation of this API mints one: each
//! case seeds that inventory in the shared engine the router hands the handlers, and
//! then drives the service the way a client does — an `awsJson1_1` `POST /` through the
//! same `MultiServiceRouter` the emulator binary mounts. Every expectation below is
//! therefore about what the handler computes from persisted state.
//!
//! The rule under test: the operation reports exactly the pending requests the store
//! holds, in the store's stable (requester account, requester Region) order, cut into
//! pages of the requested `Limit` — where 0 asks for the operation's maximum — handing
//! back a `NextToken` exactly when requests remain, and refusing a token it did not
//! issue with the modelled `InvalidNextTokenException`.

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    service_state, PendingAggregationRequestEntry,
};

use crate::config_setup_and_rule_loop::Emulator;

/// The most pending aggregation requests one page carries: the maximum of this
/// operation's own `Limit` member, from
/// `smithy_query(query="operation", operation="DescribePendingAggregationRequests")`
/// (`Limit`, range 0..20, "The default is maximum. If you specify 0, Config uses the
/// default").
const MAX_LIMIT: usize = 20;

/// Twelve-digit account ids standing for aggregator accounts, deliberately NOT in
/// ascending order so the reported order can only come from the store's key ordering
/// (`PendingAggregationRequest$RequesterAccountId`, pattern `^\d{12}$`).
const ACCOUNT_A: &str = "210987654321";
/// See [`ACCOUNT_A`].
const ACCOUNT_B: &str = "333344445555";
/// See [`ACCOUNT_A`].
const ACCOUNT_C: &str = "999988887777";

/// Seed `requests` as the pending aggregation requests this account holds, reaching the
/// store the way a handler does.
fn seed(emulator: &Emulator, requests: &[(String, String)]) {
    let state = service_state(&*emulator.engine).expect("the test engine is the shared engine");
    let mut store = state.lock();
    for (account, region) in requests {
        let entry = PendingAggregationRequestEntry::new(account, region);
        store.pending_aggregation_requests.insert(entry.key(), entry);
    }
}

/// `requests` as the wire objects they must be reported as, in the order the store
/// keys them by.
fn expected(requests: &[(String, String)]) -> Vec<Value> {
    let mut sorted = requests.to_vec();
    sorted.sort();
    sorted
        .into_iter()
        .map(|(account, region)| {
            json!({ "RequesterAccountId": account, "RequesterAwsRegion": region })
        })
        .collect()
}

/// `count` seeded requests spread over three Regions, with account ids that do not
/// ascend with the insertion order.
fn generated(count: usize) -> Vec<(String, String)> {
    const REGIONS: [&str; 3] = ["us-east-1", "eu-west-1", "ap-southeast-2"];
    (0..count)
        .map(|index| {
            let account = format!("{:012}", 999_999_999_999_u64 - (index as u64) * 7);
            (account, REGIONS[index % REGIONS.len()].to_string())
        })
        .collect()
}

/// Every page of `DescribePendingAggregationRequests` at `limit`, walked through its
/// `NextToken`: the pages' contents in order, and the size of each page.
async fn walk_pages(case: &str, emulator: &Emulator, limit: usize) -> (Vec<Value>, Vec<usize>) {
    let mut reported: Vec<Value> = Vec::new();
    let mut page_sizes: Vec<usize> = Vec::new();
    let mut next_token: Option<String> = None;
    // A page carries at least one entry whenever one remains, so the walk cannot take
    // more steps than there are entries, plus the final empty-tail page.
    for _ in 0..=(MAX_LIMIT * 4) {
        let mut request = json!({ "Limit": limit });
        if let Some(token) = next_token.as_deref() {
            request["NextToken"] = json!(token);
        }
        let body = emulator
            .ok("DescribePendingAggregationRequests", &request)
            .await;
        let page = body["PendingAggregationRequests"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a PendingAggregationRequests list, got {body}")
            })
            .clone();
        page_sizes.push(page.len());
        reported.extend(page);
        next_token = match body.get("NextToken") {
            Some(Value::String(token)) => {
                assert!(
                    !token.is_empty() && !token.contains('#'),
                    "case {case:?}: expected an opaque NextToken, got {token:?}"
                );
                Some(token.clone())
            }
            _ => None,
        };
        if next_token.is_none() {
            return (reported, page_sizes);
        }
    }
    panic!("case {case:?}: the pages of a finite store must end, got {page_sizes:?}");
}

#[tokio::test]
async fn pending_requests_are_reported_in_key_order_and_paged_at_the_requested_limit() {
    // Rows differ along what the rule generalises over: how many requests the account
    // holds (none, one, several in one account, several accounts inserted out of
    // ascending order, exactly the page maximum, and one past it) crossed with the page
    // size asked for (0 — the maximum — one, two, seven, and the maximum itself).
    let seeds: [(&str, Vec<(String, String)>); 6] = [
        ("no pending requests at all", Vec::new()),
        (
            "one pending request",
            vec![(ACCOUNT_A.to_string(), "us-east-1".to_string())],
        ),
        (
            "one account asking from three Regions",
            vec![
                (ACCOUNT_B.to_string(), "us-west-2".to_string()),
                (ACCOUNT_B.to_string(), "ap-northeast-1".to_string()),
                (ACCOUNT_B.to_string(), "eu-central-1".to_string()),
            ],
        ),
        (
            "three accounts, inserted out of order",
            vec![
                (ACCOUNT_C.to_string(), "us-east-2".to_string()),
                (ACCOUNT_A.to_string(), "sa-east-1".to_string()),
                (ACCOUNT_B.to_string(), "eu-west-3".to_string()),
            ],
        ),
        ("exactly one page of requests", generated(MAX_LIMIT)),
        ("one request past one page", generated(MAX_LIMIT + 1)),
    ];

    for (seed_case, requests) in seeds {
        for limit in [0_usize, 1, 2, 7, MAX_LIMIT] {
            let case = format!("{seed_case}, Limit {limit}");
            let emulator = Emulator::new();
            seed(&emulator, &requests);

            let (reported, page_sizes) = walk_pages(&case, &emulator, limit).await;
            let want = expected(&requests);
            assert_eq!(
                reported, want,
                "case {case:?}: expected the {} seeded requests in (account, Region) order, \
                 got {reported:?}",
                want.len()
            );

            let page_size = if limit == 0 { MAX_LIMIT } else { limit };
            for (index, size) in page_sizes.iter().enumerate() {
                assert!(
                    *size <= page_size,
                    "case {case:?}: page {index} must carry at most {page_size} requests, \
                     got {size}"
                );
            }
            // Every page but the last is full: a NextToken comes back only when
            // requests remain.
            for (index, size) in page_sizes.iter().enumerate().take(page_sizes.len() - 1) {
                assert_eq!(
                    *size, page_size,
                    "case {case:?}: page {index} must be full when a NextToken follows it, \
                     got {size}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_next_token_the_operation_did_not_issue_is_refused() {
    // Rows differ along the form of the bad token: not base64 at all, base64 of text
    // with no key separator, base64 of a key whose account is not twelve digits or
    // whose Region is empty, and a token another paginated operation of this service
    // issued (a Config rule name).
    let cases: [(&str, String); 7] = [
        ("not base64 at all", "not-a-token!!".to_string()),
        ("base64 of empty text", base64("")),
        ("base64 of text with no separator", base64("210987654321")),
        ("base64 of a short account id", base64("21098#us-east-1")),
        (
            "base64 of a non-numeric account id",
            base64("abcdefghijkl#us-east-1"),
        ),
        ("base64 of an empty Region", base64("210987654321#")),
        (
            "a token DescribeConfigRules would issue",
            base64("some-config-rule"),
        ),
    ];

    for (case, token) in cases {
        let emulator = Emulator::new();
        seed(
            &emulator,
            &[
                (ACCOUNT_A.to_string(), "us-east-1".to_string()),
                (ACCOUNT_C.to_string(), "us-east-1".to_string()),
            ],
        );

        let (status, code) = emulator
            .refused(
                "DescribePendingAggregationRequests",
                &json!({ "NextToken": token }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidNextTokenException"),
            "case {case:?}: a NextToken this operation did not issue must be refused \
             400 InvalidNextTokenException, got {status} {code}"
        );

        // The refusal changes nothing: the requests are still all reported.
        let body = emulator
            .ok("DescribePendingAggregationRequests", &json!({}))
            .await;
        assert_eq!(
            body["PendingAggregationRequests"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            2,
            "case {case:?}: the refused page must leave the store alone, got {body}"
        );
    }
}

#[tokio::test]
async fn a_limit_outside_the_modelled_range_is_refused() {
    // The modelled range is 0..20, so each side of it is refused; the rows are one past
    // each published bound.
    for limit in [-1_i64, i64::try_from(MAX_LIMIT).unwrap_or(20) + 1] {
        let case = format!("Limit {limit}");
        let emulator = Emulator::new();
        seed(
            &emulator,
            &[(ACCOUNT_A.to_string(), "us-east-1".to_string())],
        );

        let (status, code) = emulator
            .refused(
                "DescribePendingAggregationRequests",
                &json!({ "Limit": limit }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a Limit outside the modelled range must be refused, got \
             {status} {code}"
        );
        assert_eq!(
            code, "ValidationException",
            "case {case:?}: the modelled range is refused by request validation before \
             the handler, got {code}"
        );
    }
}

#[tokio::test]
async fn describing_pending_requests_reports_state_without_changing_it() {
    // Rows differ along how many requests the account holds and the page size, so the
    // read is checked both when it pages and when it does not.
    for (seed_case, count, limit) in [
        ("an empty store", 0_usize, 0_usize),
        ("a single request", 1, 0),
        ("several requests on one page", 5, 0),
        ("several requests over three pages", 5, 2),
    ] {
        let case = format!("{seed_case}, Limit {limit}");
        let emulator = Emulator::new();
        let requests = generated(count);
        seed(&emulator, &requests);

        let (first, _) = walk_pages(&case, &emulator, limit).await;
        let (again, _) = walk_pages(&case, &emulator, limit).await;
        assert_eq!(
            again, first,
            "case {case:?}: a second read must report the same requests, got {again:?}"
        );
        assert_eq!(
            first,
            expected(&requests),
            "case {case:?}: expected the seeded requests, got {first:?}"
        );
    }
}

/// `text` as the base64 the framework's page tokens are encoded with.
fn base64(text: &str) -> String {
    aws_smithy_types::base64::encode(text)
}
