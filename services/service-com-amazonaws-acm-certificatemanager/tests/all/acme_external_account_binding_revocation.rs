// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `RevokeAcmeExternalAccountBinding` over the wire (`POST /`,
//! `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! RULE: revoking an external account binding records the instant the revocation took
//! effect on that one stored binding and changes nothing else about it — the endpoint it
//! belongs to, the IAM role it binds, its expiry, its last use, the instant it was
//! created and the credentials it carries all stand — so a later
//! `DescribeAcmeExternalAccountBinding` reports that instant; the call is `@idempotent`,
//! so a repeat keeps the instant the first one recorded; and a binding ARN no stored
//! record answers to is refused as not found, with nothing written.
//!
//! Every assertion goes through [`MultiServiceRouter::dispatch`], the same path the
//! emulator serves production traffic on, and every read goes through a DIFFERENT
//! operation than the write, so a response that is not computed from the persisted store
//! cannot pass. The wire harness and the table of bindings are shared with
//! [`crate::acme_external_account_binding_lifecycle`].
//!
//! The table spans the classes the rule generalises over: bindings created with every
//! `TimeType` an `Expiration` can be counted in and with none at all, role ARNs of
//! several forms, tag sets of several sizes — and, for the idempotence half, one, two and
//! three revocations of the same binding, so "preserves what the first recorded" is
//! asserted over a range of repeat counts rather than a single pair of calls.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use service_com_amazonaws_acm_certificatemanager::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::acme_domain_validation_lifecycle::endpoint_body;
use crate::acme_endpoint_lifecycle::create_endpoint;
use crate::acme_external_account_binding_lifecycle::{
    binding_body, binding_cases, create_binding, describe_binding, seconds, stored_credentials,
};
use crate::certificate_lifecycle::{
    call, error_code, error_message, router_with_engine, ACCOUNT, REGION,
};

/// `RevokeAcmeExternalAccountBinding`, asserted to succeed and to answer with the empty
/// body its modelled `Unit` output carries.
async fn revoke_binding(router: &MultiServiceRouter, case: &str, arn: &str) {
    let (status, response) = call(
        router,
        "RevokeAcmeExternalAccountBinding",
        &json!({ "AcmeExternalAccountBindingArn": arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: RevokeAcmeExternalAccountBinding({arn}) must succeed, got {status} {response}"
    );
    assert!(
        response.is_null() || response.as_object().is_some_and(serde_json::Map::is_empty),
        "case {case:?}: the operation models no output, so its answer must carry no members, got {response}"
    );
}

/// `SystemTime` as the epoch seconds a wire timestamp is compared against.
fn unix_seconds(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs_f64()
}

/// How many external account bindings the store holds.
fn held_bindings(engine: &Arc<engine::InMemoryEngine>) -> usize {
    let store = engine.state::<state::ComAmazonawsAcmCertificatemanagerState>();
    let store = store.lock();
    store.acme_external_account_bindings.len()
}

/// Every member of `AcmeExternalAccountBinding` the revocation leaves alone
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#AcmeExternalAccountBinding")`
/// lists `RevokedAt` and `UpdatedAt` beside these).
const UNCHANGED_MEMBERS: &[&str] = &[
    "AcmeExternalAccountBindingArn",
    "AcmeEndpointArn",
    "RoleArn",
    "ExpiresAt",
    "LastUsedAt",
    "CreatedAt",
];

/// RULE: the revocation is recorded on the one binding it names — the instant it
/// happened, and nothing else about the record — for every row of the table.
#[tokio::test]
async fn revoking_records_the_instant_on_the_stored_binding() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "revocation host", &endpoint_body()).await;

    for case in binding_cases() {
        let (what, _expiration, _role_arn, _tags) = case.clone();
        let body = binding_body(&case, &endpoint_arn);

        // A sibling under the same endpoint, created from the same row, so the test can
        // tell "this binding was revoked" from "revoking touched the endpoint's bindings".
        let revoked = create_binding(&router, what, &body).await;
        let sibling = create_binding(&router, what, &body).await;
        let arn = revoked["AcmeExternalAccountBindingArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {what:?}: the create reported no ARN in {revoked}"))
            .to_string();
        let sibling_arn = sibling["AcmeExternalAccountBindingArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {what:?}: the create reported no ARN in {sibling}"))
            .to_string();

        let before_revoking = describe_binding(&router, what, &arn).await;
        assert!(
            before_revoking["RevokedAt"].is_null(),
            "case {what:?}: nothing has revoked the fresh binding, so it must report no RevokedAt, got {before_revoking}"
        );
        let credentials = stored_credentials(&engine, &arn);

        let before = unix_seconds(SystemTime::now());
        revoke_binding(&router, what, &arn).await;
        let after = unix_seconds(SystemTime::now());

        // The instant is read back through a DIFFERENT operation, and it is the instant
        // the revocation was served at — not a value from the request, which carries none.
        let described = describe_binding(&router, what, &arn).await;
        let revoked_at = seconds(&described, "RevokedAt");
        assert!(
            (before..=after).contains(&revoked_at),
            "case {what:?}: expected RevokedAt between {before} and {after}, got {revoked_at}"
        );
        assert_eq!(
            described["UpdatedAt"], described["RevokedAt"],
            "case {what:?}: the revocation is the last change to the binding, so UpdatedAt must be RevokedAt, got {described}"
        );
        for member in UNCHANGED_MEMBERS {
            assert_eq!(
                described[member], before_revoking[member],
                "case {what:?}: {member} must stand through the revocation, got {described}"
            );
        }
        assert_eq!(
            stored_credentials(&engine, &arn),
            credentials,
            "case {what:?}: revoking withdraws the authority to register, so it must not re-mint the credentials"
        );

        // The revocation named ONE binding: the sibling under the same endpoint is
        // untouched.
        let sibling_now = describe_binding(&router, what, &sibling_arn).await;
        assert_eq!(
            sibling_now, sibling,
            "case {what:?}: revoking {arn} must leave the sibling binding {sibling_arn} as its create stored it"
        );
    }

    // Two bindings per row, and no revocation added or dropped one.
    let held = held_bindings(&engine);
    assert_eq!(
        held,
        binding_cases().len() * 2,
        "expected the two bindings each row created to still be stored, got {held}"
    );
}

/// RULE: the operation is `@idempotent`, so a caller may repeat it — every repeat
/// succeeds and the binding keeps the instant the first revocation recorded.
#[tokio::test]
async fn revoking_again_preserves_the_recorded_instant() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "repeat host", &endpoint_body()).await;

    // One, two and three further revocations after the first, so the invariant is
    // asserted over a range of repeat counts.
    for (row, case) in binding_cases().into_iter().enumerate() {
        let repeats = 1 + row % 3;
        let (what, _expiration, _role_arn, _tags) = case.clone();
        let what = format!("{what}, revoked {} times", repeats + 1);
        let body = binding_body(&case, &endpoint_arn);

        let created = create_binding(&router, &what, &body).await;
        let arn = created["AcmeExternalAccountBindingArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {what:?}: the create reported no ARN in {created}"))
            .to_string();

        revoke_binding(&router, &what, &arn).await;
        let first = describe_binding(&router, &what, &arn).await;
        assert!(
            !first["RevokedAt"].is_null(),
            "case {what:?}: the first revocation must record an instant, got {first}"
        );

        for repeat in 1..=repeats {
            // Each repeat is served a measurable span after the one before it, so an
            // implementation that re-dated the revocation could not pass by answering the
            // same instant.
            std::thread::sleep(std::time::Duration::from_millis(10));
            revoke_binding(&router, &what, &arn).await;
            let described = describe_binding(&router, &what, &arn).await;
            assert_eq!(
                described, first,
                "case {what:?}: repeat {repeat} must leave the binding exactly as the first revocation left it"
            );
        }
    }

    // Every repeat answered from the record the first call wrote: nothing was created.
    let held = held_bindings(&engine);
    assert_eq!(
        held,
        binding_cases().len(),
        "expected one binding per row, got {held}"
    );
}

/// RULE: a binding ARN no stored record answers to is not found, and the refusal writes
/// nothing — whichever way the record is missing.
#[tokio::test]
async fn revoking_a_binding_nothing_answers_to_is_not_found() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "refusal host", &endpoint_body()).await;
    let doomed_endpoint = create_endpoint(&router, "doomed host", &endpoint_body()).await;

    // A binding under an endpoint that is then deleted: the endpoint's bindings go with
    // it, so its ARN answers to nothing afterwards.
    let case = binding_cases()
        .into_iter()
        .next()
        .expect("the table has at least one row");
    let created = create_binding(
        &router,
        "a binding of an endpoint that is deleted",
        &binding_body(&case, &doomed_endpoint),
    )
    .await;
    let orphan = created["AcmeExternalAccountBindingArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let kept = create_binding(&router, "a kept binding", &binding_body(&case, &endpoint_arn)).await;
    let kept_arn = kept["AcmeExternalAccountBindingArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, response) = call(
        &router,
        "DeleteAcmeEndpoint",
        &json!({ "AcmeEndpointArn": doomed_endpoint }),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteAcmeEndpoint must succeed, got {status} {response}"
    );

    // Each row is a well-formed `AcmeExternalAccountBindingArn` (its modelled `@pattern`
    // is checked before the handler runs) that no stored record answers to.
    let cases: Vec<(&str, String)> = vec![
        (
            "an id under an endpoint this account does hold",
            state::acme_external_account_binding_arn(
                &endpoint_arn,
                "00000000-0000-4000-8000-000000000000",
            ),
        ),
        (
            "an endpoint this account never created",
            state::acme_external_account_binding_arn(
                &format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/absent-endpoint"),
                "absent-binding",
            ),
        ),
        (
            "an endpoint of another account",
            state::acme_external_account_binding_arn(
                &format!("arn:aws:acm:{REGION}:210987654321:acme-endpoint/other-endpoint"),
                "other-binding",
            ),
        ),
        ("a binding whose endpoint was deleted", orphan),
    ];

    for (what, arn) in &cases {
        let held_before = held_bindings(&engine);
        let (status, response) = call(
            &router,
            "RevokeAcmeExternalAccountBinding",
            &json!({ "AcmeExternalAccountBindingArn": arn }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {what:?}: revoking {arn} must be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {what:?}: expected ResourceNotFoundException for {arn}, got {response}"
        );
        assert_eq!(
            error_message(&response),
            state::acme_external_account_binding_not_found_message(arn),
            "case {what:?}: the refusal must name the binding the request asked for"
        );
        assert_eq!(
            held_bindings(&engine),
            held_before,
            "case {what:?}: a refused revocation must write nothing"
        );
    }

    // The binding that does exist was never touched by any of those refusals.
    let described = describe_binding(&router, "a kept binding", &kept_arn).await;
    assert!(
        described["RevokedAt"].is_null(),
        "the stored binding must still report no RevokedAt, got {described}"
    );
}
