// SPDX-License-Identifier: CC0-1.0

//! `ListFrameworks` driven as real HTTP through the router.
//!
//! The rules under test:
//!
//! * a listing REPORTS the stored frameworks: every framework
//!   `CreateFramework` inserted appears exactly once, in `FrameworkName` order,
//!   carrying the ARN the create minted, the description it was given, the
//!   number of controls it holds, its creation instant and its deployment
//!   status — and a framework the account does not hold appears not at all;
//! * the listing is cut into pages of at most `MaxResults` entries plus an
//!   opaque cursor that resumes exactly where the page stopped, and the pages
//!   concatenate to the whole result set in that one order;
//! * a `MaxResults` outside the model's `@range(1..1000)` is
//!   `InvalidParameterValueException`, and so is a `NextToken` that names no
//!   page of this result set;
//! * a listing is a REPORT: it materialises only the deployment the clock has
//!   already made due, and a repeat of it answers the same thing.
//!
//! The request harness (`router`, `call`, `error_code`) and the framework
//! helpers (`create_framework`, `describe_framework`, `text`) are the ones the
//! sibling suites expose to this test binary, so there is ONE wire path for the
//! service's suites.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::framework::{
    create_framework, describe_framework, text, DEPLOYMENT_COMPLETED, PAST_FRAMEWORK_SETTLE,
};
use super::vault_and_backup_job::{call, error_code, router, ACCOUNT, REGION};

/// `DeploymentStatus` of a framework whose controls are not deployed yet — one
/// of the statuses the model lists for `Framework$DeploymentStatus`.
const DEPLOYMENT_CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";

/// The ends of the modelled page-size range (`smithy_query` operation
/// `ListFrameworks`: `MaxResults` targets `MaxFrameworkInputs`, `@range`
/// 1..1000).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 1000;

/// `GET /audit/frameworks` — `ListFrameworks`'s HTTP binding, with whatever
/// query string the case under test asks for.
async fn list_frameworks(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("/audit/frameworks{query}"), None).await
}

/// The `Frameworks` member of a successful listing.
fn entries(body: &Value) -> Vec<Value> {
    body.get("Frameworks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `FrameworkName` of every entry of a page, in the order the page lists
/// them.
fn names(body: &Value) -> Vec<String> {
    entries(body)
        .iter()
        .map(|entry| text(entry, "FrameworkName"))
        .collect()
}

/// One create request: a framework of `name` carrying `controls` controls, with
/// `description` when it is `Some`.
///
/// The control names are the published Backup Audit Manager control
/// identifiers, taken in order, so a row asking for three controls carries
/// three DISTINCT ones (a framework may not repeat a control name).
fn framework_request(name: &str, controls: usize, description: Option<&str>) -> Value {
    const CONTROL_NAMES: [&str; 5] = [
        "BACKUP_RECOVERY_POINT_ENCRYPTED",
        "BACKUP_LAST_RECOVERY_POINT_CREATED",
        "BACKUP_RESOURCES_PROTECTED_BY_BACKUP_PLAN",
        "BACKUP_RECOVERY_POINT_MINIMUM_RETENTION_CHECK",
        "BACKUP_RECOVERY_POINT_MANUAL_DELETION_DISABLED",
    ];
    let mut request = json!({
        "FrameworkName": name,
        "FrameworkControls": CONTROL_NAMES[..controls]
            .iter()
            .map(|control| json!({"ControlName": control}))
            .collect::<Vec<Value>>(),
    });
    if let Some(description) = description {
        request["FrameworkDescription"] = json!(description);
    }
    request
}

/// Create every framework a row asks for and return the create answers, keyed
/// by name.
async fn seed(
    router: &MultiServiceRouter,
    requests: &[Value],
) -> Vec<(String, Value)> {
    let mut created = Vec::new();
    for request in requests {
        let (status, answer) = create_framework(router, request).await;
        assert_eq!(
            status, 200,
            "setup: expected CreateFramework 200 for {request:?}, got {status} {answer:?}"
        );
        created.push((text(request, "FrameworkName"), answer));
    }
    created
}

/// The rows the "report what is stored" table walks: they differ in HOW MANY
/// frameworks the account holds, in how many controls each one holds, in
/// whether a description was given, and in the order the creates ran relative
/// to the name order a listing must report.
fn stored_set_cases() -> Vec<(&'static str, Vec<Value>)> {
    vec![
        ("an account holding no framework at all", vec![]),
        (
            "one framework, one control, no description",
            vec![framework_request("Alpha", 1, None)],
        ),
        (
            "one framework carrying every control the catalog names",
            vec![framework_request("Everything", 5, Some("all five controls"))],
        ),
        (
            "three frameworks created in reverse name order",
            vec![
                framework_request("Zulu", 2, Some("created first, listed last")),
                framework_request("Mike", 3, None),
                framework_request("Alfa", 1, Some("created last, listed first")),
            ],
        ),
        (
            "names that differ only in case and in digits",
            vec![
                framework_request("Audit_2", 1, None),
                framework_request("Audit_10", 2, None),
                framework_request("audit_2", 3, Some("lower case is a different name")),
            ],
        ),
        (
            "six frameworks, control counts walking the catalog",
            (0..6)
                .map(|index| {
                    framework_request(
                        &format!("Framework_{index}"),
                        index % 5 + 1,
                        (index % 2 == 0).then_some("every other row has a description"),
                    )
                })
                .collect(),
        ),
    ]
}

/// Every stored framework is listed exactly once, in name order, reporting the
/// members the stored record holds; nothing else is listed.
#[tokio::test]
async fn a_listing_reports_every_stored_framework_once_in_name_order() {
    for (case, requests) in stored_set_cases() {
        // One router per row: the row's frameworks are the only ones the
        // account holds, so a row cannot see what another row stored.
        let router = router();
        let created = seed(&router, &requests).await;

        let (status, listed) = list_frameworks(&router, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListFrameworks 200, got {status} {listed:?}"
        );

        let mut expected: Vec<String> = created.iter().map(|(name, _)| name.clone()).collect();
        expected.sort();
        assert_eq!(
            names(&listed),
            expected,
            "case {case:?}: expected every stored framework once in name order, got {listed:?}"
        );
        assert!(
            listed.get("NextToken").is_none(),
            "case {case:?}: one page holds the whole set, so there is no cursor, got {listed:?}"
        );

        for entry in entries(&listed) {
            let name = text(&entry, "FrameworkName");
            let request = requests
                .iter()
                .find(|request| text(request, "FrameworkName") == name)
                .unwrap_or_else(|| panic!("case {case:?}: {name:?} was never created"));
            let (_, create_answer) = created
                .iter()
                .find(|(created_name, _)| *created_name == name)
                .unwrap_or_else(|| panic!("case {case:?}: {name:?} was never created"));

            assert_eq!(
                entry.get("FrameworkArn"),
                create_answer.get("FrameworkArn"),
                "case {case:?}: {name:?} must be listed under the ARN its create minted, \
                 got {entry:?}"
            );
            let prefix = format!("arn:aws:backup:{REGION}:{ACCOUNT}:framework:{name}-");
            assert!(
                text(&entry, "FrameworkArn").starts_with(&prefix),
                "case {case:?}: {name:?} expected an ARN starting {prefix:?}, got {entry:?}"
            );
            assert_eq!(
                entry.get("FrameworkDescription"),
                request.get("FrameworkDescription"),
                "case {case:?}: {name:?} must report the description its create was given, \
                 got {entry:?}"
            );
            let expected_controls = request
                .get("FrameworkControls")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            assert_eq!(
                entry.get("NumberOfControls").and_then(Value::as_u64),
                Some(expected_controls as u64),
                "case {case:?}: {name:?} expected {expected_controls} controls, got {entry:?}"
            );
            assert!(
                entry["CreationTime"].is_number(),
                "case {case:?}: {name:?} expected a CreationTime, got {entry:?}"
            );

            // The listing and the describe answer from ONE record, so they
            // cannot disagree about the framework's deployment.
            let (status, read) = describe_framework(&router, &name).await;
            assert_eq!(
                (status, read.get("DeploymentStatus"), read.get("CreationTime")),
                (
                    200,
                    entry.get("DeploymentStatus"),
                    entry.get("CreationTime")
                ),
                "case {case:?}: {name:?} listed and described must agree, got {read:?}"
            );
        }
    }
}

/// The listing is paged: a page holds at most `MaxResults` entries plus a
/// cursor, and walking the cursors yields the whole result set exactly once in
/// the order one unpaginated call reports.
#[tokio::test]
async fn a_listing_pages_by_max_results_with_a_resumable_cursor() {
    let router = router();
    let requests: Vec<Value> = (0..7)
        .map(|index| framework_request(&format!("Paged_{index}"), index % 5 + 1, None))
        .collect();
    seed(&router, &requests).await;

    let (status, whole) = list_frameworks(&router, "").await;
    assert_eq!(
        status, 200,
        "expected the unpaginated listing 200, got {status} {whole:?}"
    );
    let expected = names(&whole);
    assert_eq!(
        expected.len(),
        requests.len(),
        "setup: expected every framework in one page, got {whole:?}"
    );

    // Page sizes below, at and above the number of frameworks held, plus the
    // published minimum.
    for page_size in [1_usize, 2, 3, 7, 8, 1000] {
        let mut walked: Vec<String> = Vec::new();
        let mut query = format!("?MaxResults={page_size}");
        let mut pages = 0;
        loop {
            let (status, page) = list_frameworks(&router, &query).await;
            assert_eq!(
                status, 200,
                "case page_size={page_size}: expected 200, got {status} {page:?}"
            );
            let page_names = names(&page);
            assert!(
                page_names.len() <= page_size,
                "case page_size={page_size}: a page must not exceed MaxResults, got {page:?}"
            );
            walked.extend(page_names);
            pages += 1;
            assert!(
                pages <= requests.len() + 1,
                "case page_size={page_size}: the walk must terminate, got {walked:?}"
            );
            match page.get("NextToken").and_then(Value::as_str) {
                Some(token) => query = format!("?MaxResults={page_size}&NextToken={token}"),
                None => break,
            }
        }
        assert_eq!(
            walked, expected,
            "case page_size={page_size}: the pages must concatenate to the whole set in order, \
             got {walked:?}"
        );
    }
}

/// A `MaxResults` the model's `@range` admits is served; one outside it is this
/// service's `InvalidParameterValueException`.
#[tokio::test]
async fn a_max_results_outside_the_published_range_is_refused() {
    let router = router();
    seed(&router, &[framework_request("Bounded", 1, None)]).await;

    let cases: [(&str, i64, u16); 7] = [
        ("at the published minimum", MAX_RESULTS_MIN, 200),
        ("at the published maximum", MAX_RESULTS_MAX, 200),
        ("inside the range", 42, 200),
        ("one below the published minimum", MAX_RESULTS_MIN - 1, 400),
        ("negative", -1, 400),
        ("one above the published maximum", MAX_RESULTS_MAX + 1, 400),
        ("far above", 100_000, 400),
    ];
    for (case, value, expected) in cases {
        let (status, body) = list_frameworks(&router, &format!("?MaxResults={value}")).await;
        assert_eq!(
            status, expected,
            "case {case:?} (MaxResults={value}): expected {expected}, got {status} {body:?}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&body).as_str(),
                "InvalidParameterValueException",
                "case {case:?} (MaxResults={value}): expected the modelled refusal, got {body:?}"
            );
            assert!(
                !text(&body, "Message").is_empty(),
                "case {case:?} (MaxResults={value}): the refusal states the condition, \
                 got {body:?}"
            );
        } else {
            assert_eq!(
                names(&body),
                vec!["Bounded".to_string()],
                "case {case:?} (MaxResults={value}): expected the stored framework, got {body:?}"
            );
        }
    }
}

/// A `NextToken` this result set never handed out names no page of it, and is
/// refused rather than silently answering the first page.
#[tokio::test]
async fn a_next_token_outside_this_result_set_is_refused() {
    let router = router();
    seed(&router, &[framework_request("Cursored", 1, None)]).await;

    let cases: [(&str, &str); 3] = [
        ("a bare word", "not-a-token"),
        // The hex encoding of a name the account does not hold — a cursor of
        // this service's own form pointing at nothing.
        (
            "this service's form naming an unheld framework",
            "4e65766572437265617465",
        ),
        ("odd-length hex", "abc"),
    ];
    for (case, token) in cases {
        let (status, body) = list_frameworks(&router, &format!("?NextToken={token}")).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?} ({token:?}): expected the modelled refusal, got {status} {body:?}"
        );
    }

    // A cursor the listing DID hand out resolves.
    seed(&router, &[framework_request("Cursored_Two", 2, None)]).await;
    let (status, first) = list_frameworks(&router, "?MaxResults=1").await;
    let token = text(&first, "NextToken");
    assert_eq!(
        (status, token.is_empty()),
        (200, false),
        "expected a cursor from the first page, got {status} {first:?}"
    );
    let (status, second) = list_frameworks(&router, &format!("?MaxResults=1&NextToken={token}")).await;
    assert_eq!(
        (status, names(&second)),
        (200, vec!["Cursored_Two".to_string()]),
        "the cursor must resume at the next framework, got {status} {second:?}"
    );
}

/// A listing reports the deployment the clock has already made due, and repeats
/// its answer: a read changes nothing a later reader sees.
#[tokio::test]
async fn a_listing_reports_the_deployment_the_clock_made_due_and_repeats_it() {
    let router = router();
    seed(
        &router,
        &[
            framework_request("Settling_One", 1, None),
            framework_request("Settling_Two", 3, Some("two of a kind")),
        ],
    )
    .await;

    let (status, deploying) = list_frameworks(&router, "").await;
    assert_eq!(
        status, 200,
        "expected ListFrameworks 200, got {status} {deploying:?}"
    );
    for entry in entries(&deploying) {
        assert_eq!(
            text(&entry, "DeploymentStatus").as_str(),
            DEPLOYMENT_CREATE_IN_PROGRESS,
            "a framework listed straight after its create is still deploying, got {entry:?}"
        );
    }

    // Repeating the listing answers the same thing: the first one moved nothing
    // along.
    let (status, again) = list_frameworks(&router, "").await;
    assert_eq!(
        (status, &again),
        (200, &deploying),
        "two listings taken within the settle span must agree, got {status} {again:?}"
    );

    std::thread::sleep(PAST_FRAMEWORK_SETTLE);

    let (status, deployed) = list_frameworks(&router, "").await;
    assert_eq!(
        status, 200,
        "expected ListFrameworks 200, got {status} {deployed:?}"
    );
    for entry in entries(&deployed) {
        assert_eq!(
            text(&entry, "DeploymentStatus").as_str(),
            DEPLOYMENT_COMPLETED,
            "a framework whose settle span has passed reports a settled deployment, got {entry:?}"
        );
    }
    for member in ["FrameworkName", "FrameworkArn", "CreationTime", "NumberOfControls"] {
        let before: Vec<Option<Value>> = entries(&deploying)
            .iter()
            .map(|entry| entry.get(member).cloned())
            .collect();
        let after: Vec<Option<Value>> = entries(&deployed)
            .iter()
            .map(|entry| entry.get(member).cloned())
            .collect();
        assert_eq!(
            before, after,
            "settling must not change {member}, got {deployed:?}"
        );
    }
}
