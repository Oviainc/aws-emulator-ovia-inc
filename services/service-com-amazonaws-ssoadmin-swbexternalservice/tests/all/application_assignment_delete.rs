// SPDX-License-Identifier: CC0-1.0

//! `DeleteApplicationAssignment` end to end over the `awsJson1_1` wire.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! example:
//!
//! 1. a delete REVOKES exactly the assignment its (application, principal type, principal
//!    id) tuple names: that tuple stops resolving through
//!    `DescribeApplicationAssignment`, the stored row is gone, and every other stored
//!    assignment — of the same application and of the same principal on another
//!    application — still resolves;
//! 2. a delete is NOT idempotent on an access that is not granted: a second delete of the
//!    same tuple, a tuple no create ever granted on a stored application, and an
//!    `ApplicationArn` this account holds no application for are each refused
//!    `ResourceNotFoundException`, never answered 200, and store nothing;
//! 3. revoking an access changes only the assignment: the application itself still
//!    describes afterwards, and the same tuple can be granted again and reads back.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints,
//! the `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than
//! bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    call, error_code, router_over, seeded_instance_arn, with_store,
};

/// The AWS-owned provider a customer managed application is created under, an entry of
/// the catalog this service publishes (`state::BUILT_IN_APPLICATION_PROVIDERS`).
const CUSTOM_PROVIDER_ARN: &str = "arn:aws:sso::aws:applicationProvider/custom";

/// The principals the rules are asserted over: both values of `PrincipalType`
/// (`USER | GROUP`) and both forms the model's `PrincipalId` pattern admits — a bare GUID
/// in lower and in upper hex, and the `[0-9a-f]{10}-` prefixed form, including one at the
/// 47-character length limit
/// (`smithy_query(query="operation",operation="DeleteApplicationAssignment")`).
const PRINCIPALS: &[(&str, &str, &str)] = &[
    (
        "an uppercase-hex USER GUID",
        "USER",
        "0F1E2D3C-4B5A-4697-8869-7A6B5C4D3E2F",
    ),
    (
        "a lowercase-hex GROUP GUID",
        "GROUP",
        "b7c4d1e0-2a39-4f58-9671-0c1d2e3f4a5b",
    ),
    (
        "a prefixed GROUP id at the 47-character limit",
        "GROUP",
        "13579bdf02-fedc1234-5678-4abc-8def-0123456789ab",
    ),
    (
        "a prefixed USER id",
        "USER",
        "02468ace13-0a1b2c3d-4e5f-4061-8273-8495a6b7c8d9",
    ),
];

/// Create one application and return the `ApplicationArn` it minted.
///
/// A local builder rather than a call into `application_assignments.rs`: the equivalents
/// there are module-private to that suite, and promoting them into the shared harness is
/// a change to suites this fix does not own.
async fn minted_application(
    router: &MultiServiceRouter,
    instance_arn: &str,
    name: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreateApplication",
        &json!({
            "InstanceArn": instance_arn,
            "ApplicationProviderArn": CUSTOM_PROVIDER_ARN,
            "Name": name,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateApplication must answer 200, got {status} {body}"
    );
    body["ApplicationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: CreateApplication minted no ARN: {body}"))
        .to_string()
}

/// The assignment request body every operation of this slice takes.
fn assignment(application_arn: &str, principal_type: &str, principal_id: &str) -> Value {
    json!({
        "ApplicationArn": application_arn,
        "PrincipalType": principal_type,
        "PrincipalId": principal_id,
    })
}

async fn create_assignment(
    router: &MultiServiceRouter,
    application_arn: &str,
    principal_type: &str,
    principal_id: &str,
) -> (u16, Value) {
    call(
        router,
        "CreateApplicationAssignment",
        &assignment(application_arn, principal_type, principal_id),
    )
    .await
}

async fn describe_assignment(
    router: &MultiServiceRouter,
    application_arn: &str,
    principal_type: &str,
    principal_id: &str,
) -> (u16, Value) {
    call(
        router,
        "DescribeApplicationAssignment",
        &assignment(application_arn, principal_type, principal_id),
    )
    .await
}

async fn delete_assignment(
    router: &MultiServiceRouter,
    application_arn: &str,
    principal_type: &str,
    principal_id: &str,
) -> (u16, Value) {
    call(
        router,
        "DeleteApplicationAssignment",
        &assignment(application_arn, principal_type, principal_id),
    )
    .await
}

/// Assert one call was refused the modelled `ResourceNotFoundException`.
///
/// The awsJson1_1 framing this service serves answers an error of this operation with
/// HTTP 400 and the error code in `__type`, so the status asserted here is 400 and the
/// code is what identifies the modelled error — the same assertion the sibling suites of
/// this crate make (`ssoadmin_permission_sets::assert_refused`).
fn assert_not_found(case: &str, what: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: {what} must be refused 400, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "ResourceNotFoundException",
        "case {case:?}: {what} expected ResourceNotFoundException, got {body}"
    );
}

/// Rule 1: every row of the table is revoked, and only that row — the tuple stops
/// resolving while every other granted tuple keeps resolving.
#[tokio::test]
async fn a_delete_revokes_exactly_the_tuple_it_names_and_leaves_every_other_assignment() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(engine.clone());
    let instance_arn = seeded_instance_arn(&router).await;

    // Two applications, so a revocation is pinned to the application it names and not
    // just to the principal.
    let first = minted_application(&router, &instance_arn, "revocable-one").await;
    let second = minted_application(&router, &instance_arn, "revocable-two").await;

    // Grant every (application, principal) pair of the table.
    for application_arn in [&first, &second] {
        for (label, principal_type, principal_id) in PRINCIPALS {
            let (status, body) =
                create_assignment(&router, application_arn, principal_type, principal_id).await;
            assert_eq!(
                status, 200,
                "case {label:?} on {application_arn}: the grant must answer 200, got \
                 {status} {body}"
            );
        }
    }
    let granted_rows = with_store(&engine, |store| store.application_assignments.len());
    assert_eq!(
        granted_rows,
        2 * PRINCIPALS.len(),
        "the grants of the table must store one row each, got {granted_rows}"
    );

    // Revoke them one at a time, on the FIRST application only: after each delete the
    // tuple it named is gone and every tuple not yet deleted — including every assignment
    // of the second application — still resolves.
    let mut revoked: Vec<(&str, &str, &str)> = Vec::new();
    for (index, (label, principal_type, principal_id)) in PRINCIPALS.iter().enumerate() {
        let case = format!("{label} on the first application");

        let (status, body) = delete_assignment(&router, &first, principal_type, principal_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the delete must answer 200, got {status} {body}"
        );
        assert_eq!(
            body,
            json!({}),
            "case {case:?}: the model declares an empty output, so the delete must answer \
             an empty JSON object, got {body}"
        );
        revoked.push((label, principal_type, principal_id));

        let rows = with_store(&engine, |store| store.application_assignments.len());
        assert_eq!(
            rows,
            granted_rows - revoked.len(),
            "case {case:?}: the delete must remove exactly one stored row, got {rows}"
        );

        // The revoked tuples no longer resolve.
        for (revoked_label, revoked_type, revoked_id) in &revoked {
            let (status, body) = describe_assignment(&router, &first, revoked_type, revoked_id)
                .await;
            assert_not_found(
                &format!("{revoked_label} on the first application"),
                "a describe of a revoked assignment",
                status,
                &body,
            );
        }
        // Every tuple of the first application not yet revoked still does.
        for (kept_label, kept_type, kept_id) in PRINCIPALS.iter().skip(index + 1) {
            let (status, body) = describe_assignment(&router, &first, kept_type, kept_id).await;
            assert_eq!(
                status, 200,
                "case {kept_label:?} on the first application: an assignment no delete \
                 named must still describe, got {status} {body}"
            );
        }
        // And so does every assignment of the OTHER application, for the same principals.
        for (other_label, other_type, other_id) in PRINCIPALS {
            let (status, body) = describe_assignment(&router, &second, other_type, other_id).await;
            assert_eq!(
                status, 200,
                "case {other_label:?} on the second application: a delete on another \
                 application must not revoke it, got {status} {body}"
            );
        }
    }

    let rows = with_store(&engine, |store| store.application_assignments.len());
    assert_eq!(
        rows,
        PRINCIPALS.len(),
        "only the first application's assignments may have been revoked, got {rows} rows"
    );
}

/// Rule 2: an access that is not granted has nothing to revoke, for every way of not
/// being granted.
#[tokio::test]
async fn a_delete_of_an_access_that_is_not_granted_is_refused_and_stores_nothing() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(engine.clone());
    let instance_arn = seeded_instance_arn(&router).await;
    let stored = minted_application(&router, &instance_arn, "stored-app").await;

    // The one granted tuple, so the rows below differ from a granted assignment in one
    // part each.
    let (granted_label, granted_type, granted_id) = PRINCIPALS[0];
    let (status, body) = create_assignment(&router, &stored, granted_type, granted_id).await;
    assert_eq!(
        status, 200,
        "case {granted_label:?}: the grant must answer 200, got {status} {body}"
    );

    // The account segment and the instance segment of a minted `ApplicationArn`, so an
    // unknown application can be named in the modelled form
    // (`^arn:aws(-[a-z]{1,5}){0,3}:sso::\d{12}:application/(sso)?ins-[a-zA-Z0-9-.]{16}/apl-[a-zA-Z0-9]{16}$`).
    let account = stored.split(':').nth(4).unwrap_or_default().to_string();
    let instance_id = instance_arn.rsplit('/').next().unwrap_or_default().to_string();
    let unknown_application =
        format!("arn:aws:sso::{account}:application/{instance_id}/apl-0000000000000000");

    let mut cases: Vec<(String, String, String, String)> = vec![
        (
            "a principal of the other PrincipalType on a granted id".to_string(),
            stored.clone(),
            if granted_type == "USER" { "GROUP" } else { "USER" }.to_string(),
            granted_id.to_string(),
        ),
        (
            "a pattern-valid PrincipalId nothing granted".to_string(),
            stored.clone(),
            granted_type.to_string(),
            "ffffffff-ffff-4fff-8fff-ffffffffffff".to_string(),
        ),
        (
            "the granted tuple under an application nothing holds".to_string(),
            unknown_application,
            granted_type.to_string(),
            granted_id.to_string(),
        ),
    ];
    // Every principal of the table except the granted one is ungranted on the stored
    // application too.
    for (label, principal_type, principal_id) in PRINCIPALS.iter().skip(1) {
        cases.push((
            format!("{label}, never granted"),
            stored.clone(),
            principal_type.to_string(),
            principal_id.to_string(),
        ));
    }

    for (case, application_arn, principal_type, principal_id) in &cases {
        let (status, body) =
            delete_assignment(&router, application_arn, principal_type, principal_id).await;
        assert_not_found(case, "the delete", status, &body);
        let rows = with_store(&engine, |store| store.application_assignments.len());
        assert_eq!(
            rows, 1,
            "case {case:?}: a refused delete must change no stored row, got {rows}"
        );
    }

    // The granted access is still granted after every refusal above.
    let (status, body) = describe_assignment(&router, &stored, granted_type, granted_id).await;
    assert_eq!(
        status, 200,
        "case {granted_label:?}: the granted assignment must survive every refused \
         delete, got {status} {body}"
    );

    // And a delete is not idempotent: the tuple's own second delete is the same refusal.
    let (status, body) = delete_assignment(&router, &stored, granted_type, granted_id).await;
    assert_eq!(
        status, 200,
        "case {granted_label:?}: the first delete must answer 200, got {status} {body}"
    );
    let (status, body) = delete_assignment(&router, &stored, granted_type, granted_id).await;
    assert_not_found(granted_label, "a second delete of the same tuple", status, &body);
    let rows = with_store(&engine, |store| store.application_assignments.len());
    assert_eq!(
        rows, 0,
        "case {granted_label:?}: a second delete must store nothing, got {rows} rows"
    );
}

/// Rule 3: a revocation touches the assignment only — the application stays, and the
/// access can be granted again and reads back.
#[tokio::test]
async fn revoking_an_access_leaves_the_application_and_the_access_can_be_granted_again() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(engine.clone());
    let instance_arn = seeded_instance_arn(&router).await;
    let application_arn = minted_application(&router, &instance_arn, "regrantable-app").await;

    let (status, before) = call(
        &router,
        "DescribeApplication",
        &json!({"ApplicationArn": application_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeApplication must answer 200 before the grants, got {status} {before}"
    );

    for (label, principal_type, principal_id) in PRINCIPALS {
        let (status, body) =
            create_assignment(&router, &application_arn, principal_type, principal_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the grant must answer 200, got {status} {body}"
        );
        let (status, granted) =
            describe_assignment(&router, &application_arn, principal_type, principal_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the granted assignment must describe, got {status} {granted}"
        );

        let (status, body) =
            delete_assignment(&router, &application_arn, principal_type, principal_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the delete must answer 200, got {status} {body}"
        );

        // The application is untouched by the revocation of an access to it.
        let (status, after) = call(
            &router,
            "DescribeApplication",
            &json!({"ApplicationArn": application_arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the application must survive the revocation, got {status} \
             {after}"
        );
        assert_eq!(
            after, before,
            "case {label:?}: revoking an access must change nothing the application \
             reports, got {after}"
        );

        // Granted again, the access reads back exactly as it did the first time.
        let (status, body) =
            create_assignment(&router, &application_arn, principal_type, principal_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the re-grant must answer 200, got {status} {body}"
        );
        let (status, regranted) =
            describe_assignment(&router, &application_arn, principal_type, principal_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the re-granted assignment must describe, got {status} \
             {regranted}"
        );
        assert_eq!(
            regranted, granted,
            "case {label:?}: a re-granted access must read back as it did before, got \
             {regranted}"
        );

        // Leave the store as it was found, so the row count below counts only the
        // re-grants of this loop.
        let (status, body) =
            delete_assignment(&router, &application_arn, principal_type, principal_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the final delete must answer 200, got {status} {body}"
        );
    }

    let rows = with_store(&engine, |store| store.application_assignments.len());
    assert_eq!(
        rows, 0,
        "every access granted in this test was revoked, got {rows} rows"
    );
}
