// SPDX-License-Identifier: CC0-1.0

//! `DeleteDashboard` is idempotent: it reports the post-condition, not the removal.
//!
//! The rule under test: a delete whose operation models no not-found error reports the
//! post-condition the caller asked for, so a request carrying an ARN that satisfies the
//! input member's modelled constraints answers HTTP 200 with the required output member
//! echoing exactly the requested ARN — whether or not a record was stored under it —
//! removing the record when one was there and changing nothing when none was. An ARN that
//! does NOT satisfy those constraints is still refused at the boundary with the modelled
//! `ValidationException` (400): idempotence is about a missing resource, never about a
//! malformed request.
//!
//! `DeleteDashboard` declares only `AccessDeniedException`, `ThrottlingException`,
//! `ValidationException` and `InternalServerException` — no `ResourceNotFoundException`
//! at all — while `DeleteScheduledReport` does declare one (source:
//! `smithy_query(query="digest")`), which is what tells the two deletes apart.
//!
//! The rows below are a TABLE spanning the class rather than the one recorded request:
//! they differ in the resource's HISTORY (created and then deleted, versus never created),
//! in the ARN's FORM (the partition suffix the pattern's `aws[-a-z0-9]*` admits, the
//! resource id — a one-character id, a dashed id, the 32-hex id form this emulator mints,
//! and the `*` the pattern admits — and the account id's digits), in the ARN's LENGTH
//! (including the published 2048-character maximum and one character past it) and in the
//! NUMBER of deletes of the same ARN (one, two, three).
//!
//! Every call goes through the real router, so what these tests assert is what an SDK
//! client sees.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use super::dashboard_lifecycle::{call, error_code, graph_config, ok_call, router};

/// The fixed part of every `DashboardArn`: `arn:aws` + the (here empty) partition suffix,
/// the service, the empty region segment, the 12-digit account id and the resource type.
/// Source: `DashboardArn`'s modelled pattern
/// `^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:dashboard/(\*|[-a-z0-9]+)$`, length
/// 20..2048, as reported by `smithy_query(query="operation",operation="DeleteDashboard")`.
const ARN_PREFIX: &str = "arn:aws:bcm-dashboards::000000000000:dashboard/";

/// An ARN of exactly `len` characters that satisfies `DashboardArn`'s pattern. Padding is
/// `a`, which the pattern's `[-a-z0-9]+` resource segment admits. `len` must leave room
/// for the fixed prefix — the shortest ARN the pattern admits is one character longer.
fn arn_of_length(len: usize) -> String {
    assert!(
        len > ARN_PREFIX.len(),
        "an arn of {len} characters cannot carry the {}-character prefix and a resource id",
        ARN_PREFIX.len()
    );
    format!("{ARN_PREFIX}{}", "a".repeat(len - ARN_PREFIX.len()))
}

/// Create one dashboard and answer its ARN.
async fn create(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok_call(
        router,
        "CreateDashboard",
        &json!({
            "name": name,
            "widgets": [{"title": "Spend", "configs": [graph_config("BlendedCost", "STACK", "MONTHLY")]}],
            "resourceTags": [{"key": "team", "value": "finops"}]
        }),
    )
    .await;
    created["arn"]
        .as_str()
        .expect("CreateDashboard answers an arn")
        .to_string()
}

/// One well-formed ARN no dashboard was ever stored under, and how many times in a row it
/// is deleted.
struct AbsentCase {
    label: &'static str,
    arn: String,
    deletes: usize,
}

fn absent_cases() -> Vec<AbsentCase> {
    vec![
        AbsentCase {
            label: "a one-character resource id, deleted once",
            arn: format!("{ARN_PREFIX}a"),
            deletes: 1,
        },
        AbsentCase {
            label: "a dashed resource id, deleted twice",
            arn: format!("{ARN_PREFIX}does-not-exist"),
            deletes: 2,
        },
        AbsentCase {
            label: "the 32-hex resource id form this emulator mints, deleted three times",
            arn: format!("{ARN_PREFIX}0123456789abcdef0123456789abcdef"),
            deletes: 3,
        },
        AbsentCase {
            label: "the wildcard resource id the pattern admits",
            arn: format!("{ARN_PREFIX}*"),
            deletes: 2,
        },
        AbsentCase {
            label: "a non-default partition suffix",
            arn: "arn:aws-us-gov:bcm-dashboards::123456789012:dashboard/absent".to_string(),
            deletes: 1,
        },
        AbsentCase {
            label: "an account id of twelve non-zero digits",
            arn: "arn:aws:bcm-dashboards::987654321098:dashboard/absent".to_string(),
            deletes: 1,
        },
        AbsentCase {
            // The published maximum of `DashboardArn`. The 20-character minimum is
            // unreachable: the shortest string the pattern admits is 48 characters.
            label: "an arn at the 2048-character maximum",
            arn: arn_of_length(2048),
            deletes: 1,
        },
    ]
}

/// Deleting a well-formed ARN that no stored dashboard answers to is a 200 no-op echoing
/// the requested ARN, however many times it is repeated, and it leaves the live records
/// alone.
#[tokio::test]
async fn deleting_an_absent_dashboard_is_an_idempotent_no_op() {
    for case in absent_cases() {
        let router = router();
        // A live dashboard beside the miss, so a no-op that touched the store would show.
        let survivor = create(&router, "Survivor").await;

        for attempt in 1..=case.deletes {
            let (status, body) = call(&router, "DeleteDashboard", &json!({"arn": case.arn})).await;
            assert_eq!(
                status, 200,
                "case {} (attempt {attempt}): deleting an absent dashboard must be a no-op 200, got {status}: {body}",
                case.label
            );
            assert_eq!(
                body["arn"].as_str(),
                Some(case.arn.as_str()),
                "case {} (attempt {attempt}): the answer must echo the requested arn, got {body}",
                case.label
            );
        }

        // The no-op changed nothing a later caller can see.
        let listed = ok_call(&router, "ListDashboards", &json!({})).await;
        let remaining: Vec<&str> = listed["dashboards"]
            .as_array()
            .expect("dashboards")
            .iter()
            .map(|item| item["arn"].as_str().unwrap_or("<none>"))
            .collect();
        assert_eq!(
            remaining,
            vec![survivor.as_str()],
            "case {}: the no-op must leave the stored dashboards untouched: {listed}",
            case.label
        );
        let read_back = ok_call(&router, "GetDashboard", &json!({"arn": survivor})).await;
        assert_eq!(
            read_back["arn"].as_str(),
            Some(survivor.as_str()),
            "case {}: the live dashboard must still resolve: {read_back}",
            case.label
        );
    }
}

/// A delete of a dashboard that IS stored removes it — and every repeat of that same
/// delete answers 200 with the same ARN, so a client retrying a delete whose first answer
/// it never saw sees success rather than a hard failure.
#[tokio::test]
async fn repeating_a_delete_answers_the_same_200() {
    // The row varies in how many extra deletes follow the one that removed the record.
    for repeats in [1_usize, 2, 5] {
        let router = router();
        let doomed = create(&router, "Delete Me").await;
        let survivor = create(&router, "Keep Me").await;

        for attempt in 1..=repeats {
            let (status, body) = call(&router, "DeleteDashboard", &json!({"arn": doomed})).await;
            assert_eq!(
                status, 200,
                "case {repeats} repeats (attempt {attempt}): expected 200, got {status}: {body}"
            );
            assert_eq!(
                body["arn"].as_str(),
                Some(doomed.as_str()),
                "case {repeats} repeats (attempt {attempt}): the answer must echo the requested arn, got {body}"
            );

            // After every one of them the post-condition holds: the ARN resolves to
            // nothing and is not listed.
            let (status, body) = call(&router, "GetDashboard", &json!({"arn": doomed})).await;
            assert_eq!(
                status, 400,
                "case {repeats} repeats (attempt {attempt}): a deleted dashboard must not resolve, got {status}: {body}"
            );
            assert!(
                error_code(&body).contains("ResourceNotFoundException"),
                "case {repeats} repeats (attempt {attempt}): expected ResourceNotFoundException, got {}",
                error_code(&body)
            );
        }

        let listed = ok_call(&router, "ListDashboards", &json!({})).await;
        let remaining: Vec<&str> = listed["dashboards"]
            .as_array()
            .expect("dashboards")
            .iter()
            .map(|item| item["arn"].as_str().unwrap_or("<none>"))
            .collect();
        assert_eq!(
            remaining,
            vec![survivor.as_str()],
            "case {repeats} repeats: only the surviving dashboard must be listed: {listed}"
        );
    }
}

/// The name a deleted dashboard held is free again, and the no-op delete of an absent ARN
/// neither frees nor reserves anything: a create of that name succeeds exactly once more.
#[tokio::test]
async fn an_idempotent_delete_does_not_disturb_name_uniqueness() {
    let router = router();
    let first = create(&router, "Recycled Name").await;

    ok_call(&router, "DeleteDashboard", &json!({"arn": first})).await;
    // Two more no-ops on the same, now absent, ARN.
    ok_call(&router, "DeleteDashboard", &json!({"arn": first})).await;
    ok_call(&router, "DeleteDashboard", &json!({"arn": first})).await;

    let second = create(&router, "Recycled Name").await;
    assert_ne!(
        second, first,
        "case recycled name: the re-created dashboard must get a fresh arn, got {second}"
    );

    // And the second record is intact after yet another no-op on the first, dead ARN.
    ok_call(&router, "DeleteDashboard", &json!({"arn": first})).await;
    let read_back = ok_call(&router, "GetDashboard", &json!({"arn": second})).await;
    assert_eq!(
        read_back["name"].as_str(),
        Some("Recycled Name"),
        "case recycled name: the live dashboard must still resolve: {read_back}"
    );
}

/// Idempotence does not extend to a malformed request: an `arn` that breaks
/// `DashboardArn`'s pattern or its 2048-character maximum is still refused with the
/// modelled `ValidationException` (400) rather than answered as a no-op.
#[tokio::test]
async fn a_malformed_arn_is_still_refused() {
    let router = router();

    let malformed: Vec<(&str, String)> = vec![
        ("not an arn at all", "not-an-arn".to_string()),
        (
            "another service",
            "arn:aws:bcm-dataexports::000000000000:dashboard/abc".to_string(),
        ),
        (
            "a region where the model allows none",
            "arn:aws:bcm-dashboards:us-east-1:000000000000:dashboard/abc".to_string(),
        ),
        (
            "an account id that is not twelve digits",
            "arn:aws:bcm-dashboards::12345:dashboard/abc".to_string(),
        ),
        (
            "an uppercase resource id",
            "arn:aws:bcm-dashboards::000000000000:dashboard/ABC".to_string(),
        ),
        (
            "the wrong resource type",
            "arn:aws:bcm-dashboards::000000000000:scheduled-report/abc".to_string(),
        ),
        ("an empty arn", String::new()),
        (
            "one character past the 2048-character maximum",
            arn_of_length(2049),
        ),
    ];

    for (label, arn) in malformed {
        let (status, body) = call(&router, "DeleteDashboard", &json!({"arn": arn})).await;
        assert_eq!(
            status, 400,
            "case {label}: a malformed arn must be refused, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {label}: expected ValidationException, got {}",
            error_code(&body)
        );
    }
}
