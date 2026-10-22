// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `ListPartnerApps`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.ListPartnerApps`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, via the
//! shared harness in `tests/all/inference_deployment_lifecycle.rs`, so what these tests
//! exercise is what a real SDK client gets.
//!
//! `CreatePartnerApp` is still a scaffold stub (it answers 501), so the apps a case lists
//! are seeded into the ONE shared store the handler reads — `engine.state::<...>()`, the
//! same store a create will write once it is implemented — rather than through a second
//! copy of the state.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the
//! summary each stored app is reported as, for every documented `PartnerAppType` and
//! `PartnerAppStatus`; the clock-owed `Creating` -> `Available` transition; the page walk
//! for every page size over several store sizes; and `MaxResults` on each published bound
//! and one past it.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_sagemaker_sagemaker::state::{
    ComAmazonawsSagemakerSagemakerState, PartnerApp, MAX_PAGE_SIZE,
    PARTNER_APP_DEFAULT_PAGE_SIZE,
};

use crate::inference_deployment_lifecycle::{error_code, str_field, Emulator, ACCOUNT};

/// The page ceiling the service applies whatever `MaxResults` asks for.
const PAGE_CEILING: usize = MAX_PAGE_SIZE as usize;

/// `ListPartnerApps`' documented `MaxResults` default ("The default value for MaxResults
/// is 10"), taken from the service's own constant so the test cannot drift from it.
const DEFAULT_PAGE_SIZE: usize = PARTNER_APP_DEFAULT_PAGE_SIZE as usize;

/// Every member of `PartnerAppType`, from the model
/// (`smithy_query(query="shape",shape="PartnerAppType")`).
const PARTNER_APP_TYPES: [&str; 4] = [
    "lakera-guard",
    "comet",
    "deepchecks-llm-evaluation",
    "fiddler",
];

/// Every member of `PartnerAppStatus`, from the model
/// (`smithy_query(query="shape",shape="PartnerAppStatus")`).
const PARTNER_APP_STATUSES: [&str; 7] = [
    "Creating",
    "Updating",
    "Deleting",
    "Available",
    "Failed",
    "UpdateFailed",
    "Deleted",
];

/// A stored app, as an AWS-minted `app-[A-Z0-9]{12}` id, the ARN carrying it, and the rest
/// of what `PartnerAppSummary` reports.
///
/// `created_ago` places the app's `CreationTime` in the past so an ordering is
/// unambiguous, and `available_in` is the instant the `Creating` -> `Available`
/// transition falls due (`None` keeps it in the future, i.e. not yet owed).
fn stored_app(
    index: usize,
    name: &str,
    app_type: &str,
    status: &str,
    created_ago: Duration,
    available_in: Option<Duration>,
) -> PartnerApp {
    // `PartnerAppArn` is `pattern ^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:
    // partner-app\/app-[A-Z0-9]{12}$` (smithy_query operation=DescribePartnerApp), so the
    // id is `app-` plus exactly 12 characters from `[A-Z0-9]`.
    let app_id = format!("app-AWS{index:09}");
    let now = SystemTime::now();
    PartnerApp {
        arn: format!(
            "arn:aws:sagemaker:us-east-1:{ACCOUNT}:partner-app/{app_id}",
        ),
        app_id,
        name: name.to_string(),
        app_type: app_type.to_string(),
        status: status.to_string(),
        creation_time: now - created_ago,
        last_modified_time: now - created_ago,
        available_at: match available_in {
            Some(span) => now + span,
            None => now + Duration::from_secs(3_600),
        },
    }
}

/// Put `apps` in the ONE store the service's handlers read.
fn seed(emulator: &Emulator, apps: Vec<PartnerApp>) {
    let state = emulator
        .engine
        .state::<ComAmazonawsSagemakerSagemakerState>();
    let mut store = state.lock();
    for app in apps {
        store.partner_apps.insert(app.app_id.clone(), app);
    }
}

/// `count` apps whose creation times are strictly increasing, oldest first, in the order
/// `ListPartnerApps` must report them.
fn seed_many(emulator: &Emulator, count: usize) -> Vec<String> {
    let apps: Vec<PartnerApp> = (0..count)
        .map(|index| {
            stored_app(
                index,
                &format!("app{index}"),
                PARTNER_APP_TYPES[index % PARTNER_APP_TYPES.len()],
                "Available",
                // Oldest first: app 0 was created the longest ago.
                Duration::from_secs((count - index) as u64),
                None,
            )
        })
        .collect();
    let arns = apps.iter().map(|app| app.arn.clone()).collect();
    seed(emulator, apps);
    arns
}

/// The `Summaries` a `ListPartnerApps` response carries (an absent member reads as an
/// empty page).
fn summaries(body: &Value) -> Vec<Value> {
    body.get("Summaries")
        .map(|value| {
            value
                .as_array()
                .unwrap_or_else(|| panic!("Summaries must be an array, got {value}"))
                .clone()
        })
        .unwrap_or_default()
}

/// The `CreationTime` a summary reports, as epoch seconds.
fn creation_time(summary: &Value) -> f64 {
    summary
        .get("CreationTime")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("expected a CreationTime timestamp in {summary}"))
}

/// A stored instant as epoch seconds, to compare against what a summary reports.
fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .expect("a clock after the epoch")
        .as_secs_f64()
}

/// RULE: `ListPartnerApps` reports one `PartnerAppSummary` per stored app, carrying that
/// app's ARN, name, type, status and creation time as stored — for every documented
/// `PartnerAppType` and `PartnerAppStatus`.
#[tokio::test]
async fn every_stored_app_is_summarised_with_the_arn_name_type_and_status_it_holds() {
    // Rows differ along the axes the rule generalises over: the app's type, its status,
    // and the shape of its name.
    let names = [
        "a",
        "MyPartnerApp2",
        // `PartnerAppName` is `len 1..256, pattern ^[a-zA-Z0-9]+$`
        // (smithy_query operation=CreatePartnerApp), so this row sits on the upper bound.
        &"a".repeat(256),
    ];

    for app_type in PARTNER_APP_TYPES {
        for status in PARTNER_APP_STATUSES {
            for (index, name) in names.iter().enumerate() {
                let case = format!("type={app_type} status={status} name_len={}", name.len());
                let emulator = Emulator::new();
                let app = stored_app(
                    index,
                    name,
                    app_type,
                    status,
                    Duration::from_secs(60),
                    // Far in the future, so `Creating` is NOT yet owed its transition and
                    // the status a row seeds is the status the listing must report.
                    None,
                );
                let expected_arn = app.arn.clone();
                let created_at = app.creation_time;
                seed(&emulator, vec![app]);

                let (http_status, listed) = emulator.call("ListPartnerApps", &json!({})).await;
                assert_eq!(
                    http_status, 200,
                    "case {case}: ListPartnerApps, got {http_status} with {listed:?}"
                );
                let page = summaries(&listed);
                assert_eq!(
                    page.len(),
                    1,
                    "case {case}: one stored app must be listed once, got {listed:?}"
                );
                assert_eq!(
                    str_field(&page[0], "Arn"),
                    expected_arn,
                    "case {case}: Arn, got {:?}",
                    page[0]
                );
                assert_eq!(
                    str_field(&page[0], "Name"),
                    *name,
                    "case {case}: Name, got {:?}",
                    page[0]
                );
                assert_eq!(
                    str_field(&page[0], "Type"),
                    app_type,
                    "case {case}: Type, got {:?}",
                    page[0]
                );
                assert_eq!(
                    str_field(&page[0], "Status"),
                    status,
                    "case {case}: Status, got {:?}",
                    page[0]
                );
                let reported = creation_time(&page[0]);
                let stored = epoch_seconds(created_at);
                assert!(
                    (reported - stored).abs() < 0.001,
                    "case {case}: CreationTime must be the stored instant {stored}, got \
                     {reported}"
                );
                assert!(
                    listed.get("NextToken").is_none(),
                    "case {case}: a single-app listing is not truncated, got {listed:?}"
                );
            }
        }
    }
}

/// RULE: an app the service is still provisioning reports `Creating` until the instant its
/// transition to `Available` is due and `Available` from then on, whatever the number of
/// reads — and its `CreationTime` never moves, so listing the apps never changes what a
/// later caller sees.
#[tokio::test]
async fn an_app_moves_from_creating_to_available_only_once_the_clock_owes_it() {
    // Rows differ along where the due instant sits relative to the call: still ahead,
    // ahead by a span the test can wait out, and already past.
    let cases: [(&str, Option<Duration>, &str); 3] = [
        ("due-far-ahead", None, "Creating"),
        (
            // Far enough ahead that the two in-process reads below cannot straddle the
            // due instant (each is microseconds), short enough to wait out.
            "due-shortly",
            Some(Duration::from_millis(1_500)),
            "Creating",
        ),
        // A due instant the clock has already passed: the transition was owed before the
        // first call, so the first read reports it.
        ("already-due", Some(Duration::ZERO), "Available"),
    ];

    for (case, available_in, first_status) in cases {
        let emulator = Emulator::new();
        let app = stored_app(
            0,
            "transition",
            "comet",
            "Creating",
            Duration::from_secs(1),
            available_in,
        );
        let created = epoch_seconds(app.creation_time);
        seed(&emulator, vec![app]);

        // Two reads before the due instant: the status follows the clock, not the reads.
        let mut reported: Vec<f64> = Vec::new();
        for read in ["first-read", "second-read"] {
            let page = summaries(&emulator.ok("ListPartnerApps", &json!({})).await);
            assert_eq!(
                str_field(&page[0], "Status"),
                first_status,
                "case {case}/{read}: expected {first_status}, got {:?}",
                page[0]
            );
            reported.push(creation_time(&page[0]));
        }

        if let Some(span) = available_in {
            tokio::time::sleep(span + Duration::from_millis(100)).await;
            for read in ["first-read-after-due", "second-read-after-due"] {
                let page = summaries(&emulator.ok("ListPartnerApps", &json!({})).await);
                assert_eq!(
                    str_field(&page[0], "Status"),
                    "Available",
                    "case {case}/{read}: expected Available once the transition is owed, \
                     got {:?}",
                    page[0]
                );
                reported.push(creation_time(&page[0]));
            }
        }

        assert!(
            reported
                .iter()
                .all(|value| (value - reported[0]).abs() < f64::EPSILON),
            "case {case}: CreationTime must not move across reads, got {reported:?}"
        );
        assert!(
            (reported[0] - created).abs() < 0.001,
            "case {case}: CreationTime must be the stored instant {created}, got {:?}",
            reported[0]
        );
    }
}

/// RULE: a status other than `Creating` is terminal as far as this emulator is concerned —
/// no reporting call moves it, however long ago its stored due instant fell.
#[tokio::test]
async fn a_status_other_than_creating_is_never_moved_by_a_listing() {
    for status in PARTNER_APP_STATUSES.iter().filter(|s| **s != "Creating") {
        let emulator = Emulator::new();
        let mut app = stored_app(
            0,
            "terminal",
            "fiddler",
            status,
            Duration::from_secs(60),
            Some(Duration::ZERO),
        );
        // A due instant well in the past: only a `Creating` app has a transition owed.
        app.available_at = SystemTime::now() - Duration::from_secs(60);
        seed(&emulator, vec![app]);

        let page = summaries(&emulator.ok("ListPartnerApps", &json!({})).await);
        assert_eq!(
            str_field(&page[0], "Status"),
            *status,
            "case status={status}: a listing must report the stored status unchanged, got \
             {:?}",
            page[0]
        );
    }
}

/// RULE: `ListPartnerApps` pages the stored apps in one stable creation order — at most
/// `MaxResults` (10 by default, never more than the service's own ceiling) per call, and a
/// `NextToken` exactly while apps are left — so following the token walks every app
/// exactly once.
#[tokio::test]
async fn the_page_walk_returns_every_app_exactly_once_in_creation_order() {
    // Rows differ along the number of stored apps (none, one, exactly a page, one past a
    // page, several pages) crossed with the requested page size, including none at all.
    for count in [0_usize, 1, 3, 10, 11, 13] {
        let emulator = Emulator::new();
        let seeded = seed_many(&emulator, count);

        for requested in [None, Some(1_i32), Some(2), Some(7), Some(10), Some(100)] {
            let case = format!("count={count} max_results={requested:?}");
            let page_size = requested.map_or(DEFAULT_PAGE_SIZE, |max| max as usize);
            let expected_page = page_size.min(PAGE_CEILING);

            let mut walked: Vec<String> = Vec::new();
            let mut times: Vec<f64> = Vec::new();
            let mut token: Option<String> = None;
            let mut calls = 0_usize;
            loop {
                let mut body = json!({});
                if let Some(max) = requested {
                    body["MaxResults"] = json!(max);
                }
                if let Some(ref next) = token {
                    body["NextToken"] = json!(next);
                }
                let listed = emulator.ok("ListPartnerApps", &body).await;
                let page = summaries(&listed);
                calls += 1;
                assert!(
                    calls <= count + 2,
                    "case {case}: the walk must terminate, it has made {calls} calls"
                );
                assert!(
                    page.len() <= expected_page,
                    "case {case}: a page holds at most {expected_page} apps, got {}",
                    page.len()
                );
                for summary in &page {
                    walked.push(str_field(summary, "Arn"));
                    times.push(creation_time(summary));
                }
                match listed.get("NextToken").and_then(Value::as_str) {
                    Some(next) => {
                        assert_eq!(
                            page.len(),
                            expected_page,
                            "case {case}: a truncated page must be full, got {}",
                            page.len()
                        );
                        token = Some(next.to_string());
                    }
                    None => break,
                }
            }

            assert_eq!(
                walked, seeded,
                "case {case}: the walk must return every app exactly once, oldest first"
            );
            assert_eq!(
                walked.iter().collect::<BTreeSet<_>>().len(),
                count,
                "case {case}: no app may be returned twice, got {walked:?}"
            );
            assert!(
                times.windows(2).all(|pair| pair[0] <= pair[1]),
                "case {case}: CreationTime must not decrease across the walk, got {times:?}"
            );
        }
    }
}

/// RULE: `MaxResults` is an upper bound, not a target: the service's own page ceiling
/// still applies to a request asking for more, and the rest follows the token.
#[tokio::test]
async fn a_max_results_above_the_page_ceiling_is_capped_and_the_rest_follows_the_token() {
    let count = PAGE_CEILING + 2;
    let emulator = Emulator::new();
    let seeded = seed_many(&emulator, count);

    let first = emulator
        .ok("ListPartnerApps", &json!({ "MaxResults": 100 }))
        .await;
    let page = summaries(&first);
    assert_eq!(
        page.len(),
        PAGE_CEILING,
        "case MaxResults=100 with {count} apps: expected the {PAGE_CEILING}-app ceiling, \
         got {}",
        page.len()
    );
    let token = str_field(&first, "NextToken");

    let second = emulator
        .ok(
            "ListPartnerApps",
            &json!({ "MaxResults": 100, "NextToken": token }),
        )
        .await;
    let rest = summaries(&second);
    assert_eq!(
        rest.len(),
        count - PAGE_CEILING,
        "case the second page: expected {} apps, got {}",
        count - PAGE_CEILING,
        rest.len()
    );
    assert!(
        second.get("NextToken").is_none(),
        "case the second page: the walk is complete, got {second:?}"
    );

    let walked: Vec<String> = page
        .iter()
        .chain(rest.iter())
        .map(|summary| str_field(summary, "Arn"))
        .collect();
    assert_eq!(
        walked, seeded,
        "case the whole walk: every stored app exactly once, oldest first"
    );
}

/// RULE: `MaxResults` is modelled `range 1..100`, so a value on either published bound is
/// accepted and a value past it is refused.
#[tokio::test]
async fn max_results_is_accepted_on_its_bounds_and_refused_past_them() {
    let emulator = Emulator::new();
    seed_many(&emulator, 2);

    for accepted in [1, 2, 99, 100] {
        let (status, body) = emulator
            .call("ListPartnerApps", &json!({ "MaxResults": accepted }))
            .await;
        assert_eq!(
            status, 200,
            "case MaxResults={accepted}: expected HTTP 200 inside range 1..100, got \
             {status} with {body:?}"
        );
        assert!(
            !summaries(&body).is_empty(),
            "case MaxResults={accepted}: expected a page of apps, got {body:?}"
        );
    }
    for refused in [0, -1, 101, 1000] {
        let (status, body) = emulator
            .call("ListPartnerApps", &json!({ "MaxResults": refused }))
            .await;
        assert_eq!(
            status, 400,
            "case MaxResults={refused}: expected HTTP 400 outside range 1..100, got \
             {status} with {body:?}"
        );
    }
}

/// RULE: a `NextToken` that names no boundary of the result set is refused 400
/// `ValidationException` rather than silently restarting the listing.
#[tokio::test]
async fn a_next_token_that_names_no_page_boundary_is_refused() {
    let emulator = Emulator::new();
    seed_many(&emulator, 3);

    // Rows: not hex at all, hex of an app id no stored app holds, hex of a name rather
    // than an id, and an odd-length hex string.
    let tokens = [
        "not-a-token".to_string(),
        hex::encode("app-ZZZZZZZZZZZZ"),
        hex::encode("app0"),
        "abc".to_string(),
    ];
    for token in tokens {
        let (status, body) = emulator
            .call("ListPartnerApps", &json!({ "NextToken": token }))
            .await;
        assert_eq!(
            status, 400,
            "case NextToken={token:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case NextToken={token:?}: error code, got {body:?}"
        );
    }
}

/// RULE: a listing computed from an empty store is an empty page — not a refusal, and not
/// a truncated one.
#[tokio::test]
async fn an_empty_store_lists_no_apps_and_no_token() {
    let emulator = Emulator::new();
    for body in [json!({}), json!({ "MaxResults": 5 }), json!({ "MaxResults": 1 })] {
        let listed = emulator.ok("ListPartnerApps", &body).await;
        assert!(
            summaries(&listed).is_empty(),
            "case {body}: expected an empty page, got {listed:?}"
        );
        assert!(
            listed.get("NextToken").is_none(),
            "case {body}: an empty page is not truncated, got {listed:?}"
        );
    }
}

/// RULE: a page the caller resumed is computed against the store as it stands, so an app
/// added after the first page is reported by the walk when it belongs after the boundary.
#[tokio::test]
async fn a_resumed_page_sees_an_app_added_after_the_boundary() {
    let emulator = Emulator::new();
    let seeded = seed_many(&emulator, 2);

    let first = emulator
        .ok("ListPartnerApps", &json!({ "MaxResults": 1 }))
        .await;
    let token = str_field(&first, "NextToken");
    assert_eq!(
        str_field(&summaries(&first)[0], "Arn"),
        seeded[0],
        "case the first page: the oldest app comes first, got {first:?}"
    );

    // A newer app, created after the boundary the token names.
    let newer = stored_app(
        9,
        "newest",
        "comet",
        "Available",
        Duration::ZERO,
        Some(Duration::ZERO),
    );
    let newer_arn = newer.arn.clone();
    seed(&emulator, vec![newer]);

    let second = emulator
        .ok(
            "ListPartnerApps",
            &json!({ "MaxResults": 10, "NextToken": token }),
        )
        .await;
    let rest: Vec<String> = summaries(&second)
        .iter()
        .map(|summary| str_field(summary, "Arn"))
        .collect();
    assert_eq!(
        rest,
        vec![seeded[1].clone(), newer_arn],
        "case the resumed page: expected the apps after the boundary in creation order, \
         got {second:?}"
    );
}
