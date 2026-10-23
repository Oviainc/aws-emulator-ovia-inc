// SPDX-License-Identifier: CC0-1.0

//! `ListHosts` over the wire: the paginated report of the hosts the account holds.
//!
//! The rule these suites assert: a paginated report of this service reports exactly the
//! stored rows its input selects — here every host of the account, since `ListHosts`
//! models no filter — in the store's own total `HostArn` order, each entry the SAME row
//! the single-resource reader (`GetHost`) reports, at most `MaxResults` of them per
//! response (clamped to the member's published maximum, and that maximum when the caller
//! asks for nothing), with the remainder reachable through `NextToken` and the token
//! absent exactly when nothing is left; and a `NextToken` this service never issued
//! names no position, so it is refused rather than answered from the top of the list.
//! A report writes nothing: repeating it answers the same thing.
//!
//! Every request goes through the real `MultiServiceRouter` dispatch, reusing
//! `connection_lifecycle`'s and `host_association`'s helpers, so these tests exercise the
//! same path a signed SDK request takes. The recorded unit the issue cited — a list of the
//! hosts a create had just persisted — is ONE row of the table below.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use super::connection_lifecycle::{REGION, call, create, router};
use super::git_sync::error_code;
use super::host_association::{HostCase, create_host, host_cases, plain_host};

/// The published bounds of the modeled `MaxResults` member (`range 0..100`, from
/// `smithy_query(query="operation", operation="ListHosts")`): zero is its modeled
/// default, and 100 the largest page the service renders.
const MAX_RESULTS_MIN: i64 = 0;
const MAX_RESULTS_MAX: i64 = 100;

/// Call `ListHosts` with the page size and token a case names.
async fn list(
    router: &MultiServiceRouter,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (u16, serde_json::Value) {
    let mut body = json!({});
    if let Some(max_results) = max_results {
        body["MaxResults"] = json!(max_results);
    }
    if let Some(next_token) = next_token {
        body["NextToken"] = json!(next_token);
    }
    call(router, "ListHosts", body).await
}

/// The `Hosts` entries a response reports, in the order it reported them.
fn reported_hosts(case: &str, body: &serde_json::Value) -> Vec<serde_json::Value> {
    body["Hosts"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: Hosts must be a list, got {body}"))
        .clone()
}

/// The `HostArn`s a response reports, in the order it reported them.
fn reported_arns(case: &str, body: &serde_json::Value) -> Vec<String> {
    reported_hosts(case, body)
        .iter()
        .map(|host| {
            host["HostArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every entry carries a HostArn, got {body}")
                })
                .to_string()
        })
        .collect()
}

/// The report holds exactly the hosts the store holds, in `HostArn` order, each entry
/// the row `GetHost` reports; it reports nothing for an account that holds none, and it
/// changes nothing a later caller sees.
#[tokio::test]
async fn listing_reports_every_stored_host_as_get_host_reports_it() {
    let router = router();

    // An account that holds no host at all: the report is an empty list, never a
    // refusal and never a token.
    let (status, empty) = list(&router, None, None).await;
    assert_eq!(
        status, 200,
        "case \"an account with no host\": expected 200 from ListHosts, got {status} {empty}"
    );
    assert!(
        reported_hosts("an account with no host", &empty).is_empty(),
        "case \"an account with no host\": expected no host, got {empty}"
    );
    assert!(
        empty["NextToken"].is_null(),
        "case \"an account with no host\": nothing is left, so no NextToken may come back, got {empty}"
    );

    // A connection, so a report that over-reads the store (reporting every resource
    // rather than every host) fails here.
    create(&router, "not-a-host", "GitHub").await;

    let cases = host_cases();
    let mut arns: Vec<String> = Vec::new();
    for case in &cases {
        let arn = create_host(&router, case).await;

        // Every host created so far is reported, in `HostArn` order, as soon as it is
        // persisted: the lifecycle the report reflects.
        arns.push(arn);
        let mut expected = arns.clone();
        expected.sort();
        let (status, body) = list(&router, None, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from ListHosts, got {status} {body}",
            case.label
        );
        assert_eq!(
            reported_arns(case.label, &body),
            expected,
            "case {:?}: expected every stored host in HostArn order, got {body}",
            case.label
        );
    }

    let (_, body) = list(&router, None, None).await;
    let mut expected = arns.clone();
    expected.sort();

    // Every reported entry is the stored row, not a recomputed one: its members are
    // exactly what the single-resource reader reports for the same ARN.
    for (position, arn) in expected.iter().enumerate() {
        let (status, got) = call(&router, "GetHost", json!({ "HostArn": arn })).await;
        assert_eq!(
            status, 200,
            "case {arn:?}: expected 200 from GetHost, got {status} {got}"
        );
        let entry = &body["Hosts"][position];
        // `GetHostOutput` models every `Host` member except `HostArn`
        // (`smithy_query(query="operation", operation="GetHost")`), which the list
        // entry carries because the list is addressed by nothing.
        for member in ["Name", "Status", "ProviderType", "ProviderEndpoint", "VpcConfiguration"] {
            assert_eq!(
                entry[member], got[member],
                "case {arn:?}: entry {position} member {member} must be the stored row, got {body}"
            );
        }
        assert_eq!(
            entry["HostArn"],
            json!(arn),
            "case {arn:?}: entry {position} must report its own ARN, got {body}"
        );
    }

    // A report changes nothing: the same call answers the same thing.
    let (status, again) = list(&router, None, None).await;
    assert_eq!(
        status, 200,
        "case \"a repeated report\": expected 200 from ListHosts, got {status} {again}"
    );
    assert_eq!(
        again["Hosts"], body["Hosts"],
        "case \"a repeated report\": listing must not change what a later caller sees, got {again}"
    );
}

/// Page sizes: each published bound of the modeled `MaxResults` (its `0` default and its
/// maximum of 100), the member omitted entirely, and sizes below, equal to and above the
/// number of hosts the account holds.
const PAGE_SIZES: &[Option<i64>] = &[
    None,
    Some(MAX_RESULTS_MIN),
    Some(1),
    Some(2),
    Some(3),
    Some(7),
    Some(8),
    Some(MAX_RESULTS_MAX),
];

/// Paging covers every host exactly once, in `HostArn` order, for every page size: the
/// invariant `@paginated(inputToken=NextToken, outputToken=NextToken,
/// pageSize=MaxResults)` states. No page exceeds the size the caller asked for, nor the
/// member's published maximum when it asked for nothing, and the token comes back exactly
/// while items are left.
#[tokio::test]
async fn paging_covers_every_host_exactly_once() {
    let router = router();
    let mut expected: Vec<String> = Vec::new();
    for index in 0..7 {
        expected.push(
            create_host(
                &router,
                &plain_host(&format!("paged-{index}"), "GitHubEnterpriseServer"),
            )
            .await,
        );
    }
    expected.sort();

    for page_size in PAGE_SIZES {
        let bound = match page_size {
            Some(asked) if *asked > 0 => (*asked).min(MAX_RESULTS_MAX),
            _ => MAX_RESULTS_MAX,
        };
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;

        loop {
            let (status, body) = list(&router, *page_size, token.as_deref()).await;
            assert_eq!(
                status, 200,
                "case {page_size:?}: expected 200 from ListHosts, got {status} {body}"
            );
            let page = reported_arns("paged", &body);
            assert!(
                page.len() as i64 <= bound,
                "case {page_size:?}: a page holds at most {bound} entries, got {} ({body})",
                page.len()
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {page_size:?}: paging must terminate, saw {pages} pages and {seen:?}"
            );
            match body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        assert_eq!(
            seen, expected,
            "case {page_size:?}: paging must report every host exactly once in HostArn order, got {seen:?}"
        );
    }
}

/// `NextToken` values this service never issued: text that is not hex at all, hex of
/// text no `HostArn` this service mints could be (another resource type's ARN, another
/// service's namespace, a malformed account field), and hex that decodes to nothing.
fn foreign_tokens(connection_arn: &str) -> Vec<(&'static str, String)> {
    let account = engine::emulator_account_id();
    vec![
        ("not hex at all", "not-a-page-token".to_string()),
        ("hex with an odd length", "abc".to_string()),
        (
            "hex of a connection ARN rather than a host ARN",
            hex::encode(connection_arn),
        ),
        (
            "hex of a host ARN in a partition no ARN of this service names",
            hex::encode(format!(
                "arn:not-a-partition:codeconnections:{REGION}:{account}:host/abc"
            )),
        ),
        (
            "hex of another service's ARN",
            hex::encode(format!("arn:aws:codebuild:{REGION}:{account}:host/abc")),
        ),
        (
            "hex of a host ARN whose account field is not twelve digits",
            hex::encode(format!("arn:aws:codeconnections:{REGION}:12345:host/abc")),
        ),
        (
            "hex of a host ARN with no resource id",
            hex::encode(format!(
                "arn:aws:codeconnections:{REGION}:{account}:host/"
            )),
        ),
        ("hex of the empty string", String::new()),
    ]
}

/// A `NextToken` this service never issued is refused with a 400 `ValidationException`
/// rather than answered from the top of the list, and a `MaxResults` past the member's
/// published maximum is refused by the modeled range. Neither is answered with a page,
/// a success or the unmodeled 501, and neither disturbs the stored hosts.
#[tokio::test]
async fn listing_refuses_a_foreign_token_or_an_out_of_range_page() {
    let router = router();
    // A host that IS there, so a refusal can never mean "the store is empty".
    let live = create_host(&router, &plain_host("live", "GitLabSelfManaged")).await;
    let connection = create(&router, "live-connection", "GitHub").await;

    for (label, token) in foreign_tokens(&connection) {
        let (status, body) = list(&router, None, Some(&token)).await;
        // The empty token is refused by the modeled `NextToken` length (`1..1024`)
        // before this handler runs; every other row reaches the handler. Both are
        // client errors, and neither reports a host.
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400 for a token this service did not issue, got {status} {body}"
        );
        assert!(
            body["Hosts"].is_null(),
            "case {label:?}: a refusal reports no host, got {body}"
        );
        if !token.is_empty() {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {label:?}: expected ValidationException, got {body}"
            );
        }
    }

    // One past the published `MaxResults` maximum: the modeled range refuses it before
    // the handler runs, so the answer is a 400 and never a page.
    let (status, body) = list(&router, Some(MAX_RESULTS_MAX + 1), None).await;
    assert_eq!(
        status, 400,
        "case \"MaxResults past the published maximum\": expected 400, got {status} {body}"
    );
    assert!(
        body["Hosts"].is_null(),
        "case \"MaxResults past the published maximum\": a refusal reports no host, got {body}"
    );

    // And the live host is still the one host reported: a refusal wrote nothing.
    let (status, body) = list(&router, None, None).await;
    assert_eq!(
        status, 200,
        "case \"live host\": expected 200 from ListHosts, got {status} {body}"
    );
    assert_eq!(
        reported_arns("live host", &body),
        vec![live],
        "case \"live host\": the live host must still be reported, got {body}"
    );
}

/// A token one page issued resumes exactly after the host that page ended on, for a
/// store whose hosts differ in every member: the position is a key in the store's own
/// order, so the pages a token walks partition the hosts.
#[tokio::test]
async fn a_token_resumes_after_the_host_its_page_ended_on() {
    let router = router();
    let cases: Vec<HostCase> = host_cases();
    let mut expected: Vec<String> = Vec::new();
    for case in &cases {
        expected.push(create_host(&router, case).await);
    }
    expected.sort();

    let (status, first) = list(&router, Some(2), None).await;
    assert_eq!(
        status, 200,
        "case \"first page of two\": expected 200 from ListHosts, got {status} {first}"
    );
    let head = reported_arns("first page of two", &first);
    assert_eq!(
        head,
        expected[..2].to_vec(),
        "case \"first page of two\": expected the first two hosts in HostArn order, got {first}"
    );
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"first page of two\": hosts are left, so a NextToken must come back, got {first}"));

    let (status, second) = list(&router, Some(MAX_RESULTS_MAX), Some(token)).await;
    assert_eq!(
        status, 200,
        "case \"resumed page\": expected 200 from ListHosts, got {status} {second}"
    );
    assert_eq!(
        reported_arns("resumed page", &second),
        expected[2..].to_vec(),
        "case \"resumed page\": a token must resume after the host its page ended on, got {second}"
    );
    assert!(
        second["NextToken"].is_null(),
        "case \"resumed page\": nothing is left, so no NextToken may come back, got {second}"
    );
}
