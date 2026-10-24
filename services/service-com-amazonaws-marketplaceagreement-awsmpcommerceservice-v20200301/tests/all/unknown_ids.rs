// SPDX-License-Identifier: CC0-1.0

//! An id that names no resource of this service, over the whole class of id
//! members and id forms.
//!
//! The rule under test: when an identifier member satisfies its modelled
//! constraints but names no resource of this service, the operation answers the
//! modelled `ResourceNotFoundException` body — naming in `resourceId` and
//! `resourceType` what was looked for — at HTTP 400, the status this service
//! answers a lookup that found nothing with; and the SAME operation serves the
//! id once it does name a stored resource, so the refusal comes from the lookup
//! and not from the shape of the id.
//!
//! HTTP 400 is what the recorded AWS responses for
//! `AWSMPCommerceService_v20200301` carry with that body; the shape itself is
//! modelled `@httpError(404)`, which is the status this emulator used to answer
//! and which no recorded response shows (see `crate::state::NOT_FOUND_STATUS` in
//! the crate).
//!
//! The rows span what the rule generalises over: which operation is called,
//! which id member carries the unknown value, which resource kind it names, and
//! the FORM of the id within its modelled pattern and length. The bounds and the
//! character classes come from `smithy_query(query="operation",
//! operation="DescribeAgreement,CancelAgreement,GetAgreementTerms,
//! AcceptAgreementRequest,CreateAgreementRequest")`: `ResourceId` is
//! `@length(min: 1, max: 64)` `^[A-Za-z0-9_/-]+$` and `AgreementRequestId` is
//! `@length(min: 1, max: 64)` `^ar-[A-Za-z0-9]+$`.

use serde_json::json;

use crate::agreement_lifecycle::{accept, agreement_over, call, create, emulator, term, Answer};

/// The resource kind a lookup names in `resourceType` — `ResourceType`'s values
/// (`smithy_query(query="shape", shape="ResourceType")`).
const AGREEMENT: &str = "Agreement";
const AGREEMENT_REQUEST: &str = "AgreementRequest";

/// `ResourceId` forms that satisfy `^[A-Za-z0-9_/-]+$` and
/// `@length(min: 1, max: 64)`, including both published length bounds, and none
/// of which this emulator's id minter ever produces.
fn unknown_agreement_ids() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest ResourceId, one character", "A".to_string()),
        (
            "a hyphenated, prefix-shaped id",
            "agmt-ffffffffffffffff".to_string(),
        ),
        (
            "a uuid-shaped id, as a client that keeps its own keys uses",
            "7fa92ff8-f1ab-4a21-8241-0845139308ee".to_string(),
        ),
        (
            "the underscore and slash the pattern admits",
            "tenant_7/agreement_1".to_string(),
        ),
        ("mixed case and digits", "AgmT0123456789".to_string()),
        ("the longest ResourceId, 64 characters", "z".repeat(64)),
    ]
}

/// `AgreementRequestId` forms that satisfy `^ar-[A-Za-z0-9]+$` and its length
/// bounds, the longest being 64 characters in all.
fn unknown_agreement_request_ids() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest id the ar- pattern admits", "ar-0".to_string()),
        (
            "a hex id of the width this emulator mints, but never minted",
            "ar-ffffffffffffffff".to_string(),
        ),
        (
            "mixed case and digits after the prefix",
            "ar-Sim2RealProbe71ebf109".to_string(),
        ),
        (
            "the longest AgreementRequestId, 64 characters",
            format!("ar-{}", "A".repeat(61)),
        ),
    ]
}

/// Assert one refusal: the status this service answers a lookup that found
/// nothing with, the wire error code, and the resource the lookup names.
fn assert_not_found(case: &str, resource_type: &str, id: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: an id naming no {resource_type} must answer HTTP 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/resourceType"),
        resource_type,
        "case {case:?}: expected resourceType {resource_type:?}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/resourceId"),
        id,
        "case {case:?}: the refusal must name the id that was looked for, got {:?}",
        answer.body
    );
    assert!(
        !answer.str_at("/message").is_empty(),
        "case {case:?}: a refusal must say what was wrong, got {:?}",
        answer.body
    );
}

/// Every operation that takes an `agreementId` refuses one that names no
/// agreement, for every form the member's pattern admits.
#[tokio::test]
async fn an_agreement_id_naming_no_agreement_is_not_found() {
    let router = emulator();
    // A stored agreement, so the store is not merely empty: the refusal is
    // about the id, not about there being nothing to find.
    let stored = agreement_over(&router, "ap-offer1", vec![term("eula.v1")]).await;

    for (case, id) in unknown_agreement_ids() {
        assert_ne!(
            id, stored,
            "case {case:?}: the unknown id must differ from the stored agreement"
        );
        for operation in [
            "DescribeAgreement",
            "GetAgreementTerms",
            "GetAgreementEntitlements",
            "CancelAgreement",
        ] {
            let answer = call(&router, operation, json!({"agreementId": id})).await;
            assert_not_found(
                &format!("{case} via {operation}"),
                AGREEMENT,
                &id,
                &answer,
            );
        }
    }
}

/// `AcceptAgreementRequest` refuses an `agreementRequestId` that names no
/// agreement request the same way, naming the other resource kind.
#[tokio::test]
async fn an_agreement_request_id_naming_no_request_is_not_found() {
    let router = emulator();
    // One stored, unaccepted request, so the store holds the resource kind the
    // unknown ids are looked up in.
    let created = create(
        &router,
        json!({
            "intent": "NEW",
            "agreementProposalIdentifier": "ap-offer1",
            "requestedTerms": [term("eula.v1")],
        }),
    )
    .await;
    let stored = created.str_at("/agreementRequestId");
    assert!(
        !stored.is_empty(),
        "the stored agreement request needs an id, got {:?}",
        created.body
    );

    for (case, id) in unknown_agreement_request_ids() {
        assert_ne!(
            id, stored,
            "case {case:?}: the unknown id must differ from the stored request"
        );
        let answer = accept(&router, json!({"agreementRequestId": id})).await;
        assert_not_found(case, AGREEMENT_REQUEST, &id, &answer);
    }
}

/// `CreateAgreementRequest` looks up the agreement its `sourceAgreementIdentifier`
/// names, and refuses the same way when that lookup finds nothing — the class is
/// keyed to the lookup, not to the operation or to the member that carried the id.
#[tokio::test]
async fn a_source_agreement_identifier_naming_no_agreement_is_not_found() {
    let router = emulator();
    let _stored = agreement_over(&router, "ap-offer1", vec![term("eula.v1")]).await;

    for (case, id) in unknown_agreement_ids() {
        for intent in ["AMEND", "REPLACE"] {
            let mut request = json!({
                "intent": intent,
                "requestedTerms": [term("eula.v1")],
                "sourceAgreementIdentifier": id,
            });
            if intent == "REPLACE" {
                // "This parameter is required if the intent is not AMEND."
                request["agreementProposalIdentifier"] = json!("ap-offer1");
            }
            let answer = create(&router, request).await;
            assert_not_found(
                &format!("{case} with intent {intent}"),
                AGREEMENT,
                &id,
                &answer,
            );
        }
    }
}

/// The other half of the rule: an id that DOES name a stored resource is served,
/// so the refusal above comes from the lookup rather than from the id's form.
///
/// Each row also asserts something the stored agreement decides, so a handler
/// that answered an empty 200 would fail here.
#[tokio::test]
async fn a_stored_id_is_served_by_the_same_operations() {
    let router = emulator();
    let stored = agreement_over(&router, "ap-offer1", vec![term("eula.v1")]).await;

    // (operation, request, a JSON pointer into the answer, what it must carry)
    for (operation, body, pointer, expected) in [
        (
            "DescribeAgreement",
            json!({"agreementId": stored.clone()}),
            "/agreementId",
            stored.clone(),
        ),
        (
            "GetAgreementTerms",
            json!({"agreementId": stored.clone()}),
            // The one term the stored agreement was accepted over.
            "/acceptedTerms/0/legalTerm/id",
            "eula.v1".to_string(),
        ),
        (
            "CreateAgreementRequest",
            json!({
                "intent": "AMEND",
                "requestedTerms": [term("eula.v1")],
                "sourceAgreementIdentifier": stored.clone(),
            }),
            // `ChargeSummary$newAgreementValue` is computed from the terms the
            // quote was asked for against the source agreement it names: "0 if
            // the accepted terms contain no term committing an amount".
            "/chargeSummary/newAgreementValue",
            "0".to_string(),
        ),
        // Last, because it moves the agreement out of ACTIVE.
        (
            "CancelAgreement",
            json!({"agreementId": stored.clone()}),
            // `CancelAgreementOutput` has no members; the agreement's new status
            // is asserted below, on the describe that follows.
            "",
            String::new(),
        ),
    ] {
        let answer = call(&router, operation, body).await;
        assert_eq!(
            answer.status, 200,
            "case {operation:?}: a stored id must be served, got {} with {:?}",
            answer.status, answer.body
        );
        if !pointer.is_empty() {
            assert_eq!(
                answer.str_at(pointer),
                expected,
                "case {operation:?}: expected {pointer} to carry {expected:?}, got {:?}",
                answer.body
            );
        }
    }

    // The cancel above was served, not merely accepted: the stored agreement now
    // reads CANCELLED.
    let described = call(&router, "DescribeAgreement", json!({"agreementId": stored})).await;
    assert_eq!(
        described.str_at("/status"),
        "CANCELLED",
        "the cancel must be what the later report reads, got {:?}",
        described.body
    );
}

/// A stored agreement request is accepted, and the second accept is refused as a
/// CONFLICT rather than as a lookup that found nothing: the id names a resource,
/// so the failure belongs to that resource's state.
#[tokio::test]
async fn an_accepted_request_is_not_reported_as_an_unknown_id() {
    let router = emulator();
    let created = create(
        &router,
        json!({
            "intent": "NEW",
            "agreementProposalIdentifier": "ap-offer1",
            "requestedTerms": [term("eula.v1")],
        }),
    )
    .await;
    let request_id = created.str_at("/agreementRequestId");

    let first = accept(&router, json!({"agreementRequestId": request_id.clone()})).await;
    assert_eq!(
        first.status, 200,
        "accepting a stored request must succeed, got {} with {:?}",
        first.status, first.body
    );

    let second = accept(&router, json!({"agreementRequestId": request_id.clone()})).await;
    assert_eq!(
        second.status, 409,
        "case {request_id:?}: a second accept is a conflict, got {} with {:?}",
        second.status, second.body
    );
    assert_eq!(
        second.error_code(),
        "ConflictException",
        "case {request_id:?}: expected ConflictException, got {:?}",
        second.body
    );
}
