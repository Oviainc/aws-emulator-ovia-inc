// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Amazon Inspector EC2 deep inspection surface:
//! `UpdateEc2DeepInspectionConfiguration` and `GetEc2DeepInspectionConfiguration`.
//!
//! The rule under test: an account's deep inspection activation is the one its last
//! update asked for — reported as `PENDING` while a CHANGE of activation is still being
//! applied and as `ACTIVATED`/`DEACTIVATED` once that span has fallen due, and following
//! the account's EC2 scanning enablement while the account has never configured it — and
//! its custom paths are exactly the `PathList` the last update wrote, reported beside the
//! organization's own, with every read reporting that state without changing it.
//!
//! Every request goes through the real `MultiServiceRouter` — routing, restJson1
//! deserialization, modelled-constraint validation, the handler, and restJson1
//! serialization — so what is asserted here is the wire behaviour a client sees.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_inspector2_inspector2::state::{
    self, DEEP_INSPECTION_ACTIVATED, DEEP_INSPECTION_DEACTIVATED, DEEP_INSPECTION_PENDING,
    DEEP_INSPECTION_TRANSITION, MAX_ENABLED_ACCOUNTS,
};
use smithy_server_runtime::MultiServiceRouter;

// The signed-request harness this crate's suites share: `router()` builds a router over
// its own engine, `router_over` one over an engine the test keeps a handle to, and
// `post_in` sends one signed restJson1 POST.
use crate::filters_and_enablement::{post_in, router, router_over};

/// Slack added to a span before a test reads the status the clock then owes, so a loaded
/// machine cannot read one scheduling tick early.
const MARGIN: Duration = Duration::from_millis(400);

/// Well-formed `Path` values, one per form the modelled pattern allows: a POSIX path, one
/// with a trailing separator, one with a dot-prefixed segment, one with a space inside a
/// segment, and the two Windows forms (with and without a directory).
/// Source: `smithy_query(query="shape", shape="Path")` — length 1..512, pattern
/// `^(?:(?:/(?:\.[-\w]+|[-\w]+(?:[. ][-\w]+)*))+/?|[A-Za-z]:\\(?:[-\w]+(?:[. ][-\w]+)*\\)*[-\w]+(?:[. ][-\w]+)*)$`.
const PATHS: [&str; 6] = [
    "/usr/local/bin",
    "/opt/inspector/lib/",
    "/home/.config",
    "/var/log/my app",
    "C:\\Program Files\\inspector",
    "D:\\data",
];

/// Every `ResourceScanType` token, in model order.
/// Source: `smithy_query(query="shape", shape="ResourceScanType")`.
const RESOURCE_SCAN_TYPES: [&str; 5] = ["EC2", "ECR", "LAMBDA", "LAMBDA_CODE", "CODE_REPOSITORY"];

/// The `Ec2DeepInspectionStatus` tokens a read may report `span` after an activation
/// change that settles on `settled`: the change is applied over a span, so while that span
/// is open the status is `PENDING`, and only a test slow enough to have passed the due
/// instant may see the settled token instead. Recomputed before each assertion, so a stall
/// between two calls cannot make a test flaky.
fn owed(elapsed: Duration, settled: &'static str) -> Vec<&'static str> {
    if elapsed < DEEP_INSPECTION_TRANSITION {
        vec![DEEP_INSPECTION_PENDING]
    } else {
        vec![DEEP_INSPECTION_PENDING, settled]
    }
}

/// `GetEc2DeepInspectionConfiguration` — `POST /ec2deepinspectionconfiguration/get`, no
/// members at all.
async fn get_config(router: &MultiServiceRouter) -> (u16, Option<String>, Value) {
    post_in(
        router,
        "us-east-1",
        "/ec2deepinspectionconfiguration/get",
        &json!({}),
    )
    .await
}

/// `UpdateEc2DeepInspectionConfiguration` — `POST
/// /ec2deepinspectionconfiguration/update`, every member in the body.
async fn update_config(router: &MultiServiceRouter, body: &Value) -> (u16, Option<String>, Value) {
    post_in(
        router,
        "us-east-1",
        "/ec2deepinspectionconfiguration/update",
        body,
    )
    .await
}

/// `Enable` for the calling account, so a test can put EC2 scanning on.
async fn enable(router: &MultiServiceRouter, resource_types: &[&str]) {
    let (status, error_type, body) = post_in(
        router,
        "us-east-1",
        "/enable",
        &json!({ "resourceTypes": resource_types }),
    )
    .await;
    assert_eq!(
        status, 200,
        "Enable {resource_types:?}: expected 200, got {status} / {body} ({error_type:?})"
    );
}

/// The `status` a successful read reports.
async fn status_of(router: &MultiServiceRouter, case: &str) -> String {
    let (status, error_type, body) = get_config(router).await;
    assert_eq!(
        status, 200,
        "case {case}: GetEc2DeepInspectionConfiguration: expected 200, got {status} / {body} \
         ({error_type:?})"
    );
    body["status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: the read must report a status, got {body}"))
        .to_string()
}

/// An update's activation change is reported as pending, and once the span the change
/// started has fallen due the account reports the activation it asked for — a transition
/// the CLOCK owes, so merely reading does not bring it forward.
#[tokio::test]
async fn an_activation_change_is_pending_until_its_span_falls_due() {
    let router = router();

    // Rows: each asks for the activation the account does NOT have, so each is a change;
    // they differ along the direction of the change and along whether custom paths ride
    // along with it.
    let cases: [(bool, Option<&[&str]>, &str); 3] = [
        (true, None, DEEP_INSPECTION_ACTIVATED),
        (false, Some(&[PATHS[0]]), DEEP_INSPECTION_DEACTIVATED),
        (true, Some(&[PATHS[1], PATHS[4]]), DEEP_INSPECTION_ACTIVATED),
    ];

    for (activate, paths, settled) in cases {
        let case = format!("activate={activate} paths={paths:?}");
        let mut body = json!({ "activateDeepInspection": activate });
        if let Some(paths) = paths {
            body["packagePaths"] = json!(paths);
        }

        let started = Instant::now();
        let (status, error_type, updated) = update_config(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {case}: UpdateEc2DeepInspectionConfiguration: expected 200, got {status} / \
             {updated} ({error_type:?})"
        );
        // A change is not applied instantly: the status is PENDING while the span the
        // change started is open.
        let allowed = owed(started.elapsed(), settled);
        let reported = updated["status"].as_str().unwrap_or_default();
        assert!(
            allowed.contains(&reported),
            "case {case}: the update must report one of {allowed:?} {:?} after the change, got \
             {reported:?} / {updated}",
            started.elapsed()
        );
        let allowed = owed(started.elapsed(), settled);
        let read = status_of(&router, &case).await;
        assert!(
            allowed.contains(&read.as_str()),
            "case {case}: a read inside the span must report one of {allowed:?}, got {read:?}"
        );

        tokio::time::sleep(DEEP_INSPECTION_TRANSITION + MARGIN).await;

        // Twice: the settled status is state, so reading it again reports the same thing.
        for read in 1..=2 {
            let reported = status_of(&router, &case).await;
            assert_eq!(
                reported, settled,
                "case {case}: read #{read} once the span has fallen due must report \
                 {settled:?}, got {reported:?}"
            );
        }
    }
}

/// An update asking for the activation already in force has nothing to apply, so it
/// reports a terminal status rather than starting a span.
#[tokio::test]
async fn an_activation_already_in_force_starts_no_transition() {
    // Rows: the two activations, each asked for on an account that already has it — the
    // second by way of the EC2 scanning enablement its default follows.
    let cases: [(&[&str], bool, &str); 2] = [
        (&[], false, DEEP_INSPECTION_DEACTIVATED),
        (&["EC2"], true, DEEP_INSPECTION_ACTIVATED),
    ];

    for (resource_types, activate, expected) in cases {
        let case = format!("enabled={resource_types:?} activate={activate}");
        let router = router();
        if !resource_types.is_empty() {
            enable(&router, resource_types).await;
        }

        let before = status_of(&router, &case).await;
        assert_eq!(
            before, expected,
            "case {case}: before the update the account must report {expected:?}, got {before:?}"
        );

        let (status, error_type, updated) =
            update_config(&router, &json!({ "activateDeepInspection": activate })).await;
        assert_eq!(
            status, 200,
            "case {case}: UpdateEc2DeepInspectionConfiguration: expected 200, got {status} / \
             {updated} ({error_type:?})"
        );
        assert_eq!(
            updated["status"], expected,
            "case {case}: an unchanged activation must stay terminal, got {updated}"
        );
        let after = status_of(&router, &case).await;
        assert_eq!(
            after, expected,
            "case {case}: the read after the update must report {expected:?}, got {after:?}"
        );
    }
}

/// Until the account configures deep inspection itself, its activation follows its EC2
/// scanning enablement: on for an account scanning EC2, off for one that is not.
#[tokio::test]
async fn the_unconfigured_activation_follows_ec2_scanning() {
    // Rows differ along WHICH and HOW MANY scan types are on, including a type the issue
    // never cited and the modelled list bounds (1 and all 5).
    let cases: [&[&str]; 6] = [
        &[],
        &["EC2"],
        &["ECR"],
        &["CODE_REPOSITORY"],
        &["LAMBDA", "EC2"],
        &RESOURCE_SCAN_TYPES,
    ];

    for resource_types in cases {
        let case = format!("enabled={resource_types:?}");
        let router = router();
        if !resource_types.is_empty() {
            enable(&router, resource_types).await;
        }

        let expected = if resource_types.contains(&"EC2") {
            DEEP_INSPECTION_ACTIVATED
        } else {
            DEEP_INSPECTION_DEACTIVATED
        };
        let reported = status_of(&router, &case).await;
        assert_eq!(
            reported, expected,
            "case {case}: an account that never configured deep inspection must report \
             {expected:?}, got {reported:?}"
        );

        // And nothing about the account's custom paths: it has none.
        let (_, _, body) = get_config(&router).await;
        assert!(
            body.get("packagePaths").is_none(),
            "case {case}: an account with no custom paths must report none, got {body}"
        );
    }
}

/// The custom paths an account has are exactly the `PathList` its last update wrote: a
/// later update replaces them, an empty list clears them, and an update that omits the
/// member leaves them alone.
#[tokio::test]
async fn an_update_replaces_the_accounts_custom_paths() {
    let router = router();

    // Rows differ along the number of paths (0, 1, the modelled maximum of 5), their form
    // (POSIX, dot-prefixed, spaced, Windows) and their order; `None` omits the member.
    let cases: [(Option<&[&str]>, &[&str]); 7] = [
        (Some(&[PATHS[0]]), &[PATHS[0]]),
        (Some(&[PATHS[4], PATHS[5]]), &[PATHS[4], PATHS[5]]),
        (None, &[PATHS[4], PATHS[5]]),
        (
            Some(&[PATHS[0], PATHS[1], PATHS[2], PATHS[3], PATHS[4]]),
            &[PATHS[0], PATHS[1], PATHS[2], PATHS[3], PATHS[4]],
        ),
        (Some(&[PATHS[3]]), &[PATHS[3]]),
        (Some(&[]), &[]),
        (Some(&[PATHS[2]]), &[PATHS[2]]),
    ];

    for (requested, expected) in cases {
        let case = format!("write {requested:?} -> {expected:?}");
        let body = match requested {
            Some(paths) => json!({ "packagePaths": paths }),
            // A request with no members asks nothing of either setting.
            None => json!({}),
        };

        let (status, error_type, updated) = update_config(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {case}: UpdateEc2DeepInspectionConfiguration: expected 200, got {status} / \
             {updated} ({error_type:?})"
        );
        let expected_member = if expected.is_empty() {
            Value::Null
        } else {
            json!(expected)
        };
        assert_eq!(
            updated.get("packagePaths").cloned().unwrap_or(Value::Null),
            expected_member,
            "case {case}: the update must report the paths the account now has, got {updated}"
        );
        // A paths-only update asks for no activation change, so the status stays terminal.
        assert_eq!(
            updated["status"], DEEP_INSPECTION_DEACTIVATED,
            "case {case}: a paths-only update must not move the activation, got {updated}"
        );

        // Twice: the paths are state, so reading them again reports the same list.
        for read in 1..=2 {
            let (status, _, body) = get_config(&router).await;
            assert_eq!(
                status, 200,
                "case {case}: read #{read}: expected 200, got {status} / {body}"
            );
            assert_eq!(
                body.get("packagePaths").cloned().unwrap_or(Value::Null),
                expected_member,
                "case {case}: read #{read} must report the paths last written, got {body}"
            );
        }
    }
}

/// A `packagePaths` list outside the modelled `PathList`/`Path` constraints is refused
/// with 400 `ValidationException`, and the refused request configures nothing.
#[tokio::test]
async fn a_path_outside_the_modelled_constraints_is_refused_and_changes_nothing() {
    // Rows differ along WHICH constraint is violated, including the published length
    // bounds of `Path` (1..512) and the maximum length of `PathList` (5).
    let too_long = format!("/{}", "a".repeat(512));
    let at_the_maximum = format!("/{}", "a".repeat(511));
    let malformed: [(&str, Vec<&str>); 8] = [
        ("empty path", vec![""]),
        ("a relative path", vec!["relative/path"]),
        ("a bare separator", vec!["/"]),
        ("an empty segment", vec!["/bad//segment"]),
        (
            "a Windows drive with POSIX separators",
            vec!["C:/forward/slashes"],
        ),
        ("one path past the length maximum", vec![&too_long]),
        (
            "one well-formed path beside a malformed one",
            vec![PATHS[0], "nope"],
        ),
        (
            "six paths, one past the list maximum",
            vec![PATHS[0], PATHS[1], PATHS[2], PATHS[3], PATHS[4], PATHS[5]],
        ),
    ];

    // The account starts with one path in force, which no refused request may disturb.
    let router = router();
    let in_force = [at_the_maximum.as_str(), PATHS[5]];
    let (status, _, body) = update_config(&router, &json!({ "packagePaths": in_force })).await;
    assert_eq!(
        status, 200,
        "a path at the modelled length maximum must be accepted, got {status} / {body}"
    );
    assert_eq!(
        body["packagePaths"],
        json!(in_force),
        "the accepted paths must be reported back, got {body}"
    );

    for (case, paths) in malformed {
        let (status, error_type, body) =
            update_config(&router, &json!({ "packagePaths": paths })).await;
        assert_eq!(
            status, 400,
            "case {case}: UpdateEc2DeepInspectionConfiguration: expected 400, got {status} / \
             {body}"
        );
        assert_eq!(
            error_type.as_deref(),
            Some("ValidationException"),
            "case {case}: expected ValidationException, got {error_type:?} / {body}"
        );

        let (status, _, body) = get_config(&router).await;
        assert_eq!(
            status, 200,
            "case {case}: GetEc2DeepInspectionConfiguration after a refusal: expected 200, got \
             {status} / {body}"
        );
        assert_eq!(
            body["packagePaths"],
            json!(in_force),
            "case {case}: a refused update must leave the paths in force, got {body}"
        );
    }
}

/// The organization's custom paths are reported beside the account's own, from the
/// organization-wide setting rather than from the account's list.
#[tokio::test]
async fn the_organizations_custom_paths_are_reported_beside_the_accounts() {
    // Rows differ along how many organization paths are set (none, one, the modelled
    // maximum of 5) and along their form.
    let cases: [&[&str]; 4] = [
        &[],
        &[PATHS[2]],
        &[PATHS[5], PATHS[0]],
        &[PATHS[0], PATHS[1], PATHS[2], PATHS[3], PATHS[4]],
    ];

    for org_paths in cases {
        let case = format!("org={org_paths:?}");
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_over(&engine);
        // The organization-wide list is written by `UpdateOrgEc2DeepInspectionConfiguration`,
        // which is still a scaffold stub, so it is seeded in the store the handlers read.
        state::service_state(&*engine)
            .expect("the inspector2 store")
            .lock()
            .org_package_paths = org_paths.iter().map(|path| (*path).to_string()).collect();

        let account_paths = [PATHS[3]];
        let (status, error_type, updated) =
            update_config(&router, &json!({ "packagePaths": account_paths })).await;
        assert_eq!(
            status, 200,
            "case {case}: UpdateEc2DeepInspectionConfiguration: expected 200, got {status} / \
             {updated} ({error_type:?})"
        );

        let expected_org = if org_paths.is_empty() {
            Value::Null
        } else {
            json!(org_paths)
        };
        for (source, body) in [("the update", updated), ("the read", get_config(&router).await.2)] {
            assert_eq!(
                body.get("orgPackagePaths").cloned().unwrap_or(Value::Null),
                expected_org,
                "case {case}: {source} must report the organization's paths, got {body}"
            );
            assert_eq!(
                body["packagePaths"],
                json!(account_paths),
                "case {case}: {source} must report the account's own paths unchanged, got {body}"
            );
        }
    }
}

/// The number of accounts this emulator keeps records for is bounded, and the bound holds
/// on the deep inspection write path too: with the map full, an update for an account that
/// has no record is refused with 400 `ValidationException` and stores nothing, while an
/// account that already has one stays configurable.
#[tokio::test]
async fn an_update_cannot_grow_the_account_map_past_its_bound() {
    // Rows: the two sides of the bound — the calling account already holding a record, and
    // the calling account being the one that would not fit.
    for tracked in [false, true] {
        let case = format!("the calling account tracked={tracked}");
        let router = router();
        if tracked {
            enable(&router, &["EC2"]).await;
        }

        // Fill the map with other accounts, at most 100 ids per call (the modelled
        // `AccountIdSet` maximum).
        let mut recorded = usize::from(tracked);
        let mut next: u64 = 1;
        while recorded < MAX_ENABLED_ACCOUNTS {
            let batch: Vec<String> = (recorded..MAX_ENABLED_ACCOUNTS.min(recorded + 100))
                .map(|_| {
                    let account_id = format!("{next:012}");
                    next += 1;
                    account_id
                })
                .collect();
            recorded += batch.len();
            let (status, error_type, body) = post_in(
                &router,
                "us-east-1",
                "/enable",
                &json!({ "resourceTypes": ["EC2"], "accountIds": batch }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: Enable while filling the account map: expected 200, got {status} / \
                 {body} ({error_type:?})"
            );
        }

        let (status, error_type, body) =
            update_config(&router, &json!({ "packagePaths": [PATHS[0]] })).await;
        if tracked {
            assert_eq!(
                status, 200,
                "case {case}: an account that already has a record stays configurable: expected \
                 200, got {status} / {body} ({error_type:?})"
            );
            assert_eq!(
                body["packagePaths"],
                json!([PATHS[0]]),
                "case {case}: the written path must be reported back, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case}: an update past the account bound: expected 400, got {status} / \
                 {body}"
            );
            assert_eq!(
                error_type.as_deref(),
                Some("ValidationException"),
                "case {case}: expected ValidationException, got {error_type:?} / {body}"
            );
            let (status, _, body) = get_config(&router).await;
            assert_eq!(
                status, 200,
                "case {case}: GetEc2DeepInspectionConfiguration after the refusal: expected 200, \
                 got {status} / {body}"
            );
            assert!(
                body.get("packagePaths").is_none(),
                "case {case}: a refused update must store nothing, got {body}"
            );
        }
    }
}
