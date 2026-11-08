// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListLogGroupsForQuery`: the log groups one started CloudWatch Logs
//! Insights query was run against, and the two refusals the operation owes.
//!
//! The rules under test:
//!
//! * the report is computed from the query the `queryId` names — "An array of the names and
//!   ARNs of the log groups that were processed in the query" — so it names exactly the log
//!   groups that query reads, however the start selected them, and reading it changes nothing
//!   a later caller sees;
//! * a `queryId` that names no query of this account is an AUTHORIZATION refusal
//!   (`AccessDeniedException`) carrying no sentence at all, rather than a missing resource;
//! * a `maxResults` outside the range the model declares for it is refused
//!   `InvalidParameterException` with the member and the bound the model carries, and every
//!   value inside that range — both published bounds included — is served.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response a handler does not actually compute cannot pass.
//!
//! The tables span what the rules generalise over: how many log groups the query reads and
//! which member the start named them by, which form an unresolvable `queryId` takes (both
//! published length bounds of `QueryId` included), and which end of the `maxResults` range a
//! value sits outside.

use serde_json::{json, Value};

use crate::harness::{now_ms, Logs};

/// The operations under test.
const LIST: &str = "ListLogGroupsForQuery";
const START: &str = "StartQuery";

/// The modelled error codes the rules name
/// (`smithy_query(query="operation",operation="ListLogGroupsForQuery")` ->
/// `AccessDeniedException` | `InvalidParameterException` | `ResourceNotFoundException` |
/// `ServiceUnavailableException`).
const ACCESS_DENIED: &str = "AccessDeniedException";
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The range the model declares for `maxResults` (`ListLogGroupsForQueryMaxResults`,
/// `range {min: 50, max: 500}`).
const MAX_RESULTS_MIN: i64 = 50;
const MAX_RESULTS_MAX: i64 = 500;

/// A query string every case starts, which has nothing to do with the log groups it reads.
const QUERY: &str = "fields @timestamp, @message | limit 20";

/// The window a query is started with, in the epoch SECONDS `StartQuery`'s range is specified
/// in.
fn window() -> (i64, i64) {
    let now_secs = now_ms() / 1_000 + 1;
    (now_secs - 60 * 60, now_secs)
}

/// Start a query selecting log groups with `selector`, answering the `queryId` it mints.
async fn start(logs: &Logs, selector: Value, query_string: &str) -> String {
    let (start_time, end_time) = window();
    let mut request = selector;
    request["startTime"] = json!(start_time);
    request["endTime"] = json!(end_time);
    request["queryString"] = json!(query_string);
    let answer = logs.ok(START, &request).await;
    answer["queryId"]
        .as_str()
        .unwrap_or_else(|| panic!("StartQuery answers a queryId, got {answer}"))
        .to_string()
}

/// The `logGroupIdentifiers` of one answered page.
fn identifiers(case: &str, page: &Value) -> Vec<String> {
    page["logGroupIdentifiers"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: the answer carries logGroupIdentifiers, got {page}"))
        .iter()
        .map(|value| value.as_str().unwrap_or_default().to_string())
        .collect()
}

/// The report names the log groups the query reads, whichever member the start selected them
/// by — and reading it twice reports the same thing.
#[tokio::test]
async fn the_report_names_the_log_groups_the_query_reads() {
    let logs = Logs::new();
    const ONE: &str = "/queried/one";
    const TWO: &str = "/queried/two";
    const THREE: &str = "/queried/three";
    for group in [ONE, TWO, THREE] {
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
    }
    // The ARN form a log group is reported under: `logGroupArn`, the ARN without the trailing
    // `:*`, read back from the group itself so the identifier is not restated here.
    let one = logs.group_arns(ONE).await.1;
    let two = logs.group_arns(TWO).await.1;
    let three = logs.group_arns(THREE).await.1;

    // (case, the member that selects the log groups, the identifiers the report must name)
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "one group named by logGroupName",
            json!({"logGroupName": ONE}),
            vec![one.clone()],
        ),
        (
            "one group named by logGroupNames",
            json!({"logGroupNames": [TWO]}),
            vec![two.clone()],
        ),
        (
            "three groups named by logGroupNames",
            json!({"logGroupNames": [TWO, THREE, ONE]}),
            vec![one.clone(), three.clone(), two.clone()],
        ),
        (
            "one group named by its ARN",
            json!({"logGroupIdentifiers": [three.clone()]}),
            vec![three.clone()],
        ),
        (
            "the same group named twice",
            json!({"logGroupNames": [ONE, ONE]}),
            vec![one.clone()],
        ),
        (
            // A query whose own SOURCE command selects the log groups names none of the three
            // members, so the start resolved no group: the report names none either.
            "a SOURCE command instead of a member",
            json!({"queryString": r#"SOURCE logGroups(namePrefix: ["/queried/"]) | fields @message"#}),
            vec![],
        ),
    ];
    for (case, selector, expected) in cases {
        let query_string = selector
            .get("queryString")
            .and_then(Value::as_str)
            .unwrap_or(QUERY)
            .to_string();
        let query_id = start(&logs, selector, &query_string).await;
        let page = logs.ok(LIST, &json!({"queryId": query_id})).await;
        let mut reported = identifiers(case, &page);
        let mut wanted = expected.clone();
        reported.sort();
        wanted.sort();
        assert_eq!(
            reported, wanted,
            "case {case}: expected the query's own log groups, got {page}"
        );
        assert!(
            page["nextToken"].is_null(),
            "case {case}: one page holds every group of the query, got {page}"
        );
        // Reporting is a read: calling it twice reports the same thing.
        let again = logs.ok(LIST, &json!({"queryId": query_id})).await;
        assert_eq!(
            again, page,
            "case {case}: the report changes nothing a later caller sees"
        );
    }
}

/// A `queryId` that names no query of this account is refused as unauthorized, with no
/// sentence at all.
#[tokio::test]
async fn a_query_id_that_names_no_query_is_refused_as_unauthorized() {
    let logs = Logs::new();
    const GROUP: &str = "/queried/denied";
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    // One query that DOES exist, so the refusals below are about the id rather than about an
    // empty store.
    let held = start(&logs, json!({"logGroupName": GROUP}), QUERY).await;

    // (case, the queryId) — each well formed against `QueryId` (`@length(1..256)`) and held by
    // no query of this account, at both published bounds of that length.
    let shortest = "q".to_string();
    let longest = "q".repeat(256);
    let cases: Vec<(&str, String)> = vec![
        (
            "a well-formed id in the shape StartQuery mints",
            "00000000-0000-4000-8000-00000000dead".to_string(),
        ),
        ("an id of one character, the modelled minimum", shortest),
        ("an id of 256 characters, the modelled maximum", longest),
        (
            "the held id with one character appended",
            format!("{held}0"),
        ),
        ("an id that is not a uuid at all", "not-a-query".to_string()),
    ];
    for (case, query_id) in cases {
        let (status, body) = logs.call(LIST, &json!({"queryId": query_id})).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        let code = crate::harness::error_code(&body);
        assert_eq!(
            code, ACCESS_DENIED,
            "case {case}: expected {ACCESS_DENIED}, got {code} with {body}"
        );
        let said = body
            .get("message")
            .or_else(|| body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            said.is_empty(),
            "case {case}: the refusal carries no sentence, got {said:?}"
        );
    }

    // The id the account DOES hold is served, so the refusal is about the id and not about
    // the operation.
    let page = logs.ok(LIST, &json!({"queryId": held})).await;
    assert_eq!(
        identifiers("the held id", &page).len(),
        1,
        "case the held id: the query's one log group is reported, got {page}"
    );
}

/// `maxResults` is held to the range the model declares for it: outside it the request is
/// refused naming the member and the bound, inside it — both published bounds included — it is
/// served.
#[tokio::test]
async fn max_results_is_held_to_the_modelled_range() {
    let logs = Logs::new();
    const GROUP: &str = "/queried/page-size";
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    let query_id = start(&logs, json!({"logGroupName": GROUP}), QUERY).await;

    // (case, the value, the requirement the clause must state) — one row per side of the
    // range, at the first value outside it and further out.
    let refused: Vec<(&str, i64, String)> = vec![
        (
            "one above the modelled maximum",
            MAX_RESULTS_MAX + 1,
            format!("Member must have value less than or equal to {MAX_RESULTS_MAX}"),
        ),
        (
            "well above the modelled maximum",
            MAX_RESULTS_MAX * 2,
            format!("Member must have value less than or equal to {MAX_RESULTS_MAX}"),
        ),
        (
            "one below the modelled minimum",
            MAX_RESULTS_MIN - 1,
            format!("Member must have value greater than or equal to {MAX_RESULTS_MIN}"),
        ),
        (
            "zero",
            0,
            format!("Member must have value greater than or equal to {MAX_RESULTS_MIN}"),
        ),
    ];
    for (case, value, requirement) in refused {
        let (status, body) = logs
            .call(
                LIST,
                &json!({"queryId": query_id.clone(), "maxResults": value}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        let code = crate::harness::error_code(&body);
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case}: expected {INVALID_PARAMETER}, got {code} with {body}"
        );
        let said = body
            .get("message")
            .or_else(|| body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            said,
            format!("1 validation error detected: Value at 'maxResults' failed to satisfy constraint: {requirement}"),
            "case {case}: expected the constraint clause for maxResults, got {body}"
        );
    }

    for value in [MAX_RESULTS_MIN, MAX_RESULTS_MIN + 1, MAX_RESULTS_MAX] {
        let case = format!("maxResults {value}, inside the modelled range");
        let page = logs
            .ok(
                LIST,
                &json!({"queryId": query_id.clone(), "maxResults": value}),
            )
            .await;
        assert_eq!(
            identifiers(&case, &page).len(),
            1,
            "case {case}: the query's one log group is reported, got {page}"
        );
    }

    // A continuation token minted for nothing is not a position in this enumeration.
    let (status, body) = logs
        .call(
            LIST,
            &json!({"queryId": query_id, "nextToken": "not-a-token"}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case a token this enumeration never minted: expected HTTP 400, got {status} with {body}"
    );
    let code = crate::harness::error_code(&body);
    assert_eq!(
        code, INVALID_PARAMETER,
        "case a token this enumeration never minted: expected {INVALID_PARAMETER}, got {code} \
         with {body}"
    );
}
