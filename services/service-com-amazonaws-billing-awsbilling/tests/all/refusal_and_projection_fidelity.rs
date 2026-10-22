// SPDX-License-Identifier: CC0-1.0

//! What this service PROJECTS and what it SAYS when it refuses, end to end over
//! the awsJson1_0 wire. Four rules, each over a table:
//!
//! 1. The account's default `PRIMARY` billing view is a seeded resource, so every
//!    documented member of it is populated — `description` included — and a read
//!    of it answers the same thing before and after a refused write.
//! 2. An `AccessDeniedException` for an ARN outside the caller's account names the
//!    CALLER's own identity: the ARN `sts:GetCallerIdentity` reports for the same
//!    credentials, never a second principal minted by this service.
//! 3. `GetCreditAllocationHistory`'s window is half-open — `startDate` must be
//!    strictly before `endDate` — and the refusal names the member and the
//!    constraint it broke.
//! 4. A not-found refusal carries the sentence its own operation produces: the
//!    reads that resolve a `BillingViewArn` report the ARN they could not
//!    resolve, and the tagging APIs answer a fixed sentence that does not echo it
//!    back. A managed view is not taggable at all.
//!
//! The harness (`call`, `error_code`, …) is the one `billing_views.rs` owns.

use serde_json::{json, Value};
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state::ambient_identity;

use crate::billing_views::{
    call, emulator_with_engine, error_code, ABSENT_ARN, ACCOUNT_ID, PRIMARY_ARN,
};
// The civil-date to epoch-seconds conversion and the `message` reader already
// exist in this test crate, so this module imports them rather than writing a
// second copy of either.
use crate::credit_allocation_history::at;
use crate::rejection_fidelity::message;

/// The `@pattern` of `BillingViewDescription`, minus its anchors. Source:
/// `smithy_query(query="operation", operation="CreateBillingView")`.
const DESCRIPTION_CHARACTERS: &str = " abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_+=.-@";

/// The fixed sentence the tagging APIs answer for a resource that does not
/// exist (`state::RESOURCE_NOT_FOUND_MESSAGE`).
const RESOURCE_NOT_FOUND: &str = "Resource not found";

/// The sentence a tagging request against a view AWS manages is refused with
/// (`state::MANAGED_VIEW_NOT_TAGGABLE`).
const MANAGED_NOT_TAGGABLE: &str = "Managed billing views cannot be tagged";

/// The `billingView` a `GetBillingView` answer carries.
async fn get_view(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    arn: &str,
) -> Value {
    let (status, body) = call(router, "GetBillingView", &json!({ "arn": arn })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetBillingView to answer 200, got {status} {body:?}"
    );
    body["billingView"].clone()
}

#[tokio::test]
async fn the_default_primary_view_carries_every_documented_member() {
    let (router, _engine) = emulator_with_engine();

    let view = get_view(&router, "seeded PRIMARY", PRIMARY_ARN).await;
    // The members the seeded view must carry: a value for each, not an absent
    // key, so a client reading the default view gets what the model documents.
    for member in [
        "arn",
        "name",
        "description",
        "billingViewType",
        "ownerAccountId",
        "createdAt",
    ] {
        assert!(
            !view[member].is_null(),
            "case \"seeded PRIMARY\": expected member {member:?} to be populated, got {view:?}"
        );
    }
    let description = view["description"].as_str().unwrap_or_default();
    assert!(
        !description.is_empty()
            && description
                .chars()
                .all(|c| DESCRIPTION_CHARACTERS.contains(c)),
        "case \"seeded PRIMARY\": the description must satisfy BillingViewDescription's \
         pattern, got {description:?}"
    );

    // A read reports the same description every time, and a REFUSED write leaves
    // it where it was: the rows are the ways a caller can try to move it.
    let refused: Vec<(&str, Value)> = vec![
        (
            "a rename of the PRIMARY view",
            json!({ "arn": PRIMARY_ARN, "name": "renamed" }),
        ),
        (
            "a new description for the PRIMARY view",
            json!({ "arn": PRIMARY_ARN, "description": "mine now" }),
        ),
        (
            "an update of an ARN no view is stored under",
            json!({ "arn": ABSENT_ARN, "description": "mine now" }),
        ),
    ];
    for (case, request) in refused {
        let before = get_view(&router, case, PRIMARY_ARN).await;
        let (status, body) = call(&router, "UpdateBillingView", &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the write to be refused, got {status} {body:?}"
        );
        let after = get_view(&router, case, PRIMARY_ARN).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused write leaves the projection untouched"
        );
        assert_eq!(
            after["description"], before["description"],
            "case {case:?}: the description is readable before and after"
        );
    }

    // `ListBillingViews` projects the same record, so the two reads agree.
    let (status, listed) = call(
        &router,
        "ListBillingViews",
        &json!({ "billingViewTypes": ["PRIMARY"] }),
    )
    .await;
    assert_eq!(status, 200, "case \"list PRIMARY\": got {status} {listed:?}");
    assert_eq!(
        listed["billingViews"][0]["description"], view["description"],
        "case \"list PRIMARY\": the list and the get project one record, got {listed:?}"
    );
}

#[tokio::test]
async fn a_cross_account_refusal_names_the_identity_sts_reports() {
    let (router, _engine) = emulator_with_engine();
    // The identity STS answers `GetCallerIdentity` with for the credentials this
    // harness signs with: a key that names no session, so it is STS's ambient
    // identity. The refusal must name THAT principal.
    let caller = ambient_identity().arn;

    // (case, operation, how the row builds its request from the ARN)
    type BodyOf = fn(&str) -> Value;
    let operations: [(&str, BodyOf); 4] = [
        ("GetBillingView", |arn| json!({ "arn": arn })),
        ("UpdateBillingView", |arn| {
            json!({ "arn": arn, "name": "renamed" })
        }),
        ("DeleteBillingView", |arn| json!({ "arn": arn })),
        ("DisassociateSourceViews", |arn| {
            json!({ "arn": arn, "sourceViews": [PRIMARY_ARN] })
        }),
    ];
    // Foreign ARNs differing in account, partition and resource id — including
    // `primary`, which exists in the caller's OWN account.
    let foreign_arns = [
        "arn:aws:billing::111122223333:billingview/primary",
        "arn:aws:billing::123456789012:billingview/does-not-exist",
        "arn:aws-us-gov:billing::111122223333:billingview/custom-7",
    ];

    for arn in foreign_arns {
        for (operation, body_of) in operations {
            let case = format!("{operation} on {arn}");
            let (status, body) = call(&router, operation, &body_of(arn)).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (403, "AccessDeniedException"),
                "case {case:?}: expected 403 AccessDeniedException, got {status} {body:?}"
            );
            assert_eq!(
                message(&body),
                format!(
                    "User: {caller} is not authorized to perform: billing:{operation} on \
                     resource: {arn} because no resource-based policy allows the \
                     billing:{operation} action"
                ),
                "case {case:?}: expected the refusal to name the caller STS reports, got {:?}",
                message(&body)
            );
            assert!(
                !message(&body).contains(":root"),
                "case {case:?}: the principal is the caller's identity, not an account root, \
                 got {:?}",
                message(&body)
            );
        }
    }
}

#[tokio::test]
async fn the_credit_allocation_window_is_half_open() {
    let (router, _engine) = emulator_with_engine();

    // (case, startDate, endDate, accepted?) — the rows walk the boundary of the
    // one rule from both sides, at instants in different months and years and at
    // different distances, so neither bound is special-cased.
    let rows: Vec<(&str, i64, i64, bool)> = vec![
        ("one second wide", at(2025, 2, 10, 12), at(2025, 2, 10, 12) + 1, true),
        ("one hour wide", at(2025, 2, 10, 12), at(2025, 2, 10, 13), true),
        ("a whole month wide", at(2026, 1, 1, 0), at(2026, 2, 1, 0), true),
        (
            "24 billing months wide, the published maximum",
            at(2024, 6, 15, 0),
            at(2026, 6, 15, 0),
            true,
        ),
        ("zero width, at a month's first instant", at(2025, 3, 1, 0), at(2025, 3, 1, 0), false),
        ("zero width, mid-month", at(2025, 2, 10, 12), at(2025, 2, 10, 12), false),
        ("zero width, in another year", at(2023, 11, 30, 23), at(2023, 11, 30, 23), false),
        ("one second inverted", at(2025, 2, 10, 12) + 1, at(2025, 2, 10, 12), false),
        ("a month inverted", at(2026, 2, 1, 0), at(2026, 1, 1, 0), false),
    ];

    for (case, start, end, accepted) in rows {
        let (status, body) = call(
            &router,
            "GetCreditAllocationHistory",
            &json!({ "accountId": ACCOUNT_ID, "startDate": start, "endDate": end }),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the window to be accepted, got {status} {body:?}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException, got {status} {body:?}"
        );
        let stated = message(&body);
        assert!(
            stated.contains("startDate") && stated.contains("endDate"),
            "case {case:?}: the refusal names the member and the constraint, got {stated:?}"
        );
        assert_eq!(
            body["fieldList"][0]["name"].as_str(),
            Some("startDate"),
            "case {case:?}: expected fieldList to name startDate, got {:?}",
            body["fieldList"]
        );
        for value in [start.to_string(), end.to_string()] {
            assert!(
                !stated.contains(&value),
                "case {case:?}: the rejected value is not echoed back, got {stated:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_not_found_refusal_carries_its_own_operations_sentence() {
    let (router, _engine) = emulator_with_engine();
    let absent_arns = [
        ABSENT_ARN.to_string(),
        format!("arn:aws:billing::{ACCOUNT_ID}:billingview/also-absent"),
        format!("arn:aws:billing::{ACCOUNT_ID}:billingview/custom-4242"),
    ];

    for arn in &absent_arns {
        // The read that resolves one `BillingViewArn` reports the ARN it could
        // not resolve, in this service's ARN-validation form.
        let case = format!("ListSourceViewsForBillingView on {arn}");
        let (status, body) = call(
            &router,
            "ListSourceViewsForBillingView",
            &json!({ "arn": arn }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {status} {body:?}"
        );
        assert_eq!(
            message(&body),
            format!("[{arn}] is an invalid billing view arn"),
            "case {case:?}: expected the ARN-validation sentence, got {:?}",
            message(&body)
        );
        assert_eq!(
            (
                body["resourceId"].as_str(),
                body["resourceType"].as_str()
            ),
            (Some(arn.as_str()), Some("billingview")),
            "case {case:?}: the modeled members still identify the resource, got {body:?}"
        );

        // The tagging APIs answer a FIXED sentence instead, and carry the ARN
        // only in the member the model has for it.
        let tagging: Vec<(&str, Value)> = vec![
            (
                "TagResource",
                json!({ "resourceArn": arn, "resourceTags": [{ "key": "team", "value": "finance" }] }),
            ),
            ("ListTagsForResource", json!({ "resourceArn": arn })),
        ];
        for (operation, request) in tagging {
            let case = format!("{operation} on {arn}");
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: expected 404 ResourceNotFoundException, got {status} {body:?}"
            );
            assert_eq!(
                message(&body),
                RESOURCE_NOT_FOUND,
                "case {case:?}: expected the fixed sentence, got {:?}",
                message(&body)
            );
            assert!(
                !message(&body).contains(arn.as_str()),
                "case {case:?}: the tagging APIs do not echo the ARN into the message, got {:?}",
                message(&body)
            );
            assert_eq!(
                body["resourceId"].as_str(),
                Some(arn.as_str()),
                "case {case:?}: the ARN is reported in resourceId, got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_managed_view_is_not_taggable_through_any_tagging_operation() {
    let (router, _engine) = emulator_with_engine();

    // The account's PRIMARY view is managed, so neither tagging operation
    // touches it — a different refusal from "no such resource", and one that
    // does not depend on what the request's tags are.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "TagResource with one tag",
            "TagResource",
            json!({ "resourceArn": PRIMARY_ARN, "resourceTags": [{ "key": "team", "value": "finance" }] }),
        ),
        (
            "TagResource with no tags",
            "TagResource",
            json!({ "resourceArn": PRIMARY_ARN, "resourceTags": [] }),
        ),
        (
            "ListTagsForResource",
            "ListTagsForResource",
            json!({ "resourceArn": PRIMARY_ARN }),
        ),
    ];

    for (case, operation, request) in rows {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {body:?}"
        );
        assert_eq!(
            message(&body),
            MANAGED_NOT_TAGGABLE,
            "case {case:?}: expected the managed-view sentence, got {:?}",
            message(&body)
        );
    }

    // The control: a CUSTOM view of the same account takes the same tag and
    // reports it back, so the refusal is about the view and not the request.
    let (status, created) = call(
        &router,
        "CreateBillingView",
        &json!({ "name": "taggable-view", "sourceViews": [PRIMARY_ARN] }),
    )
    .await;
    assert_eq!(status, 200, "case \"control create\": got {status} {created:?}");
    let arn = created["arn"].as_str().unwrap_or_default().to_string();
    let (status, tagged) = call(
        &router,
        "TagResource",
        &json!({ "resourceArn": arn, "resourceTags": [{ "key": "team", "value": "finance" }] }),
    )
    .await;
    assert_eq!(status, 200, "case \"control tag\": got {status} {tagged:?}");
    let (status, listed) = call(
        &router,
        "ListTagsForResource",
        &json!({ "resourceArn": arn }),
    )
    .await;
    assert_eq!(
        (status, listed["resourceTags"][0]["key"].as_str()),
        (200, Some("team")),
        "case \"control read back\": expected the stored tag, got {status} {listed:?}"
    );
}
