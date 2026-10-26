// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetAcmeExternalAccountBindingCredentials` over the wire
//! (`POST /`, `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! RULE: the credentials of an ACME external account binding are minted once, when the
//! binding is created, so the operation that reports them reads the stored record — the
//! same `KeyId` and `MacKey` on every call, one pair per binding, and nothing in the
//! store changed by the read — and refuses the request once the binding's authority is
//! gone: a revoked binding is still a record, so its credentials are refused as an
//! invalid request against it, while an ARN no stored record answers to is not found.
//!
//! Every assertion goes through [`MultiServiceRouter::dispatch`], the same path the
//! emulator serves production traffic on, and the credentials are compared against the
//! record the CREATE persisted (read straight off the store) rather than against
//! anything the response or the request carries, so a response that is not computed from
//! the persisted store cannot pass. The wire harness and the table of bindings are
//! shared with [`crate::acme_external_account_binding_lifecycle`].
//!
//! The table spans the classes the rule generalises over: bindings created with every
//! `TimeType` an `Expiration` can be counted in and with none at all, role ARNs of
//! several forms, tag sets of several sizes — and, for the refusal halves, repeated
//! reads of one revoked binding beside an untouched sibling under the same endpoint, and
//! every way a well-formed binding ARN can answer to no record.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::acme_domain_validation_lifecycle::endpoint_body;
use crate::acme_endpoint_lifecycle::create_endpoint;
use crate::acme_external_account_binding_lifecycle::{
    assert_mac_key_round_trips, binding_body, binding_cases, create_binding, describe_binding,
    stored_credentials,
};
use crate::acme_external_account_binding_revocation::{held_bindings, revoke_binding};
use crate::certificate_lifecycle::{
    call, error_code, error_message, router_with_engine, ACCOUNT, REGION,
};

/// `GetAcmeExternalAccountBindingCredentials` for `arn`, as `(status, body)`.
async fn get_credentials(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(
        router,
        "GetAcmeExternalAccountBindingCredentials",
        &json!({ "AcmeExternalAccountBindingArn": arn }),
    )
    .await
}

/// The `(KeyId, MacKey)` the operation reports, asserted to succeed.
async fn credentials(router: &MultiServiceRouter, case: &str, arn: &str) -> (String, String) {
    let (status, response) = get_credentials(router, arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetAcmeExternalAccountBindingCredentials({arn}) must succeed, got {status} {response}"
    );
    let key_id = response["KeyId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the answer reported no KeyId in {response}"))
        .to_string();
    let mac_key = response["MacKey"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the answer reported no MacKey in {response}"))
        .to_string();
    (key_id, mac_key)
}

/// The id segment of a binding ARN: everything after the last `/`.
fn binding_id(arn: &str) -> &str {
    arn.rsplit_once('/')
        .unwrap_or_else(|| panic!("{arn} is not an external account binding ARN"))
        .1
}

/// RULE: the operation reports the credentials the create minted and stored for the
/// binding its ARN names — one pair per binding, decodable key material, the same answer
/// on every call — and changes nothing a later caller sees, for every row of the table.
#[tokio::test]
async fn get_reports_the_credentials_the_create_persisted() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "credentials host", &endpoint_body()).await;

    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();

    for case in binding_cases() {
        let (what, _expiration, _role_arn, _tags) = case.clone();
        let body = binding_body(&case, &endpoint_arn);
        let created = create_binding(&router, what, &body).await;
        let arn = created["AcmeExternalAccountBindingArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {what:?}: the create reported no ARN in {created}"))
            .to_string();

        // What the CREATE persisted, read straight off the store, is what the operation
        // must report: neither member is a member of any request.
        let stored = stored_credentials(&engine, &arn);
        let reported = credentials(&router, what, &arn).await;
        assert_eq!(
            reported, stored,
            "case {what:?}: expected the credentials the create stored {stored:?}, got {reported:?}"
        );
        assert_eq!(
            reported.0,
            binding_id(&arn),
            "case {what:?}: the key id must identify the binding it belongs to, got {:?}",
            reported.0
        );
        assert_mac_key_round_trips(what, &reported.1);
        assert!(
            !body.to_string().contains(&reported.1) && !body.to_string().contains(&reported.0),
            "case {what:?}: neither credential may come from the request {body}"
        );
        assert!(
            seen.insert(reported.clone()),
            "case {what:?}: every binding must carry its own credentials, but the pair reported for key id {:?} was reported twice",
            reported.0
        );

        // The read is a read: the record it reported from stands, so a repeat answers the
        // same pair and the describe beside it is unchanged — including `LastUsedAt`,
        // which an ACME client's registration sets and a credential read does not.
        let described = describe_binding(&router, what, &arn).await;
        let again = credentials(&router, what, &arn).await;
        assert_eq!(
            again, reported,
            "case {what:?}: reading the credentials again must answer the same pair, got {again:?}"
        );
        assert_eq!(
            describe_binding(&router, what, &arn).await,
            described,
            "case {what:?}: reading the credentials must leave the binding record as it stood"
        );
        assert!(
            described["LastUsedAt"].is_null(),
            "case {what:?}: no ACME client has registered with the binding, so reading its credentials may not record a use, got {described}"
        );
        assert_eq!(
            stored_credentials(&engine, &arn),
            stored,
            "case {what:?}: a read must not re-mint the binding's credentials"
        );
    }
}

/// RULE: a revocation withdraws the binding's authority, so from that instant the
/// operation refuses the request as invalid — naming the binding it refuses — however
/// often it is repeated, while the record itself, its credentials and every sibling
/// binding stand.
#[tokio::test]
async fn a_revoked_binding_refuses_its_credentials() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "revoked host", &endpoint_body()).await;

    // One, two and three reads after the revocation, so "still refused" is asserted over
    // a range of repeat counts rather than a single call.
    for (row, case) in binding_cases().into_iter().enumerate() {
        let repeats = 1 + row % 3;
        let (what, _expiration, _role_arn, _tags) = case.clone();
        let what = format!("{what}, read {repeats} times after the revocation");
        let body = binding_body(&case, &endpoint_arn);

        let created = create_binding(&router, &what, &body).await;
        let sibling = create_binding(&router, &what, &body).await;
        let arn = created["AcmeExternalAccountBindingArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {what:?}: the create reported no ARN in {created}"))
            .to_string();
        let sibling_arn = sibling["AcmeExternalAccountBindingArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {what:?}: the create reported no ARN in {sibling}"))
            .to_string();

        // Before the revocation the credentials are reported; the sibling's pair is read
        // too, so the refusal below is shown to follow the revocation and not the row.
        let before = credentials(&router, &what, &arn).await;
        let sibling_before = credentials(&router, &what, &sibling_arn).await;

        revoke_binding(&router, &what, &arn).await;

        for repeat in 1..=repeats {
            let (status, response) = get_credentials(&router, &arn).await;
            assert_eq!(
                status, 400,
                "case {what:?}: read {repeat} of a revoked binding's credentials must be refused, got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {what:?}: read {repeat} expected ValidationException, got {response}"
            );
            assert_eq!(
                error_message(&response),
                state::acme_external_account_binding_revoked_message(&arn),
                "case {what:?}: read {repeat} must name the revoked binding the request asked for"
            );
            assert!(
                !response.to_string().contains(before.1.as_str()),
                "case {what:?}: read {repeat} must not carry the MAC key it refused to report, got {response}"
            );
        }

        // The refusal is about the binding's authority, not about its record: the record
        // still reports the revocation, its stored credentials are untouched, and the
        // sibling under the same endpoint still reports its own pair.
        let described = describe_binding(&router, &what, &arn).await;
        assert!(
            !described["RevokedAt"].is_null(),
            "case {what:?}: the revoked binding must still be reported, with its revocation instant, got {described}"
        );
        assert_eq!(
            stored_credentials(&engine, &arn),
            before,
            "case {what:?}: a refused read must leave the stored credentials exactly as they were"
        );
        assert_eq!(
            credentials(&router, &what, &sibling_arn).await,
            sibling_before,
            "case {what:?}: revoking {arn} must leave the sibling binding {sibling_arn} reporting its own credentials"
        );
    }

    // Two bindings per row, and no read or refusal added or dropped one.
    let held = held_bindings(&engine);
    assert_eq!(
        held,
        binding_cases().len() * 2,
        "expected the two bindings each row created to still be stored, got {held}"
    );
}

/// RULE: a binding ARN no stored record answers to is not found — whichever way the
/// record is missing — and the refusal writes nothing.
#[tokio::test]
async fn credentials_of_a_binding_nothing_answers_to_are_not_found() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "absent host", &endpoint_body()).await;
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
    let kept_credentials = credentials(&router, "a kept binding", &kept_arn).await;
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
        let (status, response) = get_credentials(&router, arn).await;
        assert_eq!(
            status, 400,
            "case {what:?}: reading the credentials of {arn} must be refused, got {status} {response}"
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
            "case {what:?}: a refused read must write nothing"
        );
    }

    // The binding that does exist still reports the pair it reported before any of those
    // refusals.
    assert_eq!(
        credentials(&router, "a kept binding", &kept_arn).await,
        kept_credentials,
        "the stored binding must still report the credentials its create minted"
    );
}
