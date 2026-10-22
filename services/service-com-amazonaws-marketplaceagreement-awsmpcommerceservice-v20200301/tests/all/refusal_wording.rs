// SPDX-License-Identifier: CC0-1.0

//! The two refusal classes this service answers at HTTP 400, and what tells them
//! apart.
//!
//! The rule under test: this service answers BOTH a member whose value its model
//! does not admit and a lookup that found nothing with HTTP 400, so the status
//! alone does not say which happened — the error SHAPE does. A value outside its
//! member's constraints is the modelled `ValidationException` (with the `reason`
//! naming the member); a value the constraints admit that names no resource is
//! the modelled `ResourceNotFoundException` (with `resourceId` and
//! `resourceType` naming what was looked for), whose `@httpError(404)` the
//! recorded responses for this service contradict — see
//! `crate::state::NOT_FOUND_STATUS`.
//!
//! The rows differ along what the rule generalises over: which operation is
//! called, and which side of its member's constraints the id falls on — the
//! pattern, each published length bound, and a well-formed id of several shapes.
//! `tests/all/unknown_ids.rs` spans the not-found half over more id forms and
//! members, and `tests/all/validation_envelope.rs` spans the validation half
//! over the model's other constraints; this suite pins that one status carries
//! two distinguishable classes.

use serde_json::json;

use crate::agreement_lifecycle::{agreement_over, call, emulator, term, Answer};

/// Which shape a refusal must be answered in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// The value is not one the member's modelled constraints admit.
    Validation,
    /// The value is admitted, and names no resource.
    NotFound,
}

/// The operations that take an `agreementId` and look an agreement up by it.
const BY_AGREEMENT_ID: [&str; 3] = ["DescribeAgreement", "GetAgreementTerms", "CancelAgreement"];

/// `ResourceId` is `@length(min: 1, max: 64)` with pattern
/// `^[A-Za-z0-9_/-]+$` (`smithy_query(query="operation",
/// operation="DescribeAgreement")`): a value outside any of those is refused as
/// invalid, and every value inside them that names nothing is refused as not
/// found.
fn rows() -> Vec<(&'static str, String, Class)> {
    vec![
        (
            "a space and punctuation the pattern does not admit",
            "not a valid id!!".to_string(),
            Class::Validation,
        ),
        (
            "empty, so below the minimum length",
            String::new(),
            Class::Validation,
        ),
        (
            "one character past the maximum length",
            "z".repeat(65),
            Class::Validation,
        ),
        (
            "at the maximum length, so admitted",
            "z".repeat(64),
            Class::NotFound,
        ),
        (
            "one character, the minimum length, so admitted",
            "A".to_string(),
            Class::NotFound,
        ),
        (
            "a uuid-shaped id, so admitted",
            "7fa92ff8-f1ab-4a21-8241-0845139308ee".to_string(),
            Class::NotFound,
        ),
        (
            "the underscore and slash the pattern admits",
            "tenant_7/agreement_1".to_string(),
            Class::NotFound,
        ),
    ]
}

/// Assert `answer` is the refusal `class` states, at the one status both carry.
fn assert_class(case: &str, class: Class, id: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: both refusal classes answer HTTP 400, got {} with {:?}",
        answer.status, answer.body
    );
    match class {
        Class::Validation => {
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: a value its constraints do not admit is a ValidationException, \
                 got {:?}",
                answer.body
            );
            assert_eq!(
                answer.reason(),
                "INVALID_AGREEMENT_ID",
                "case {case:?}: the reason must name the member, got {:?}",
                answer.body
            );
        }
        Class::NotFound => {
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: an admitted id that names nothing is a \
                 ResourceNotFoundException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.str_at("/resourceId"),
                id,
                "case {case:?}: the refusal must name the id looked for, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.str_at("/resourceType"),
                "Agreement",
                "case {case:?}: the refusal must name the resource kind, got {:?}",
                answer.body
            );
            assert!(
                answer.body["reason"].is_null(),
                "case {case:?}: a not-found refusal carries no validation reason, got {:?}",
                answer.body
            );
        }
    }
}

/// One status, two classes: every operation that looks an agreement up answers
/// an id outside its member's constraints as a validation refusal and an
/// admitted id that names nothing as a not-found refusal.
#[tokio::test]
async fn one_status_carries_both_refusal_classes() {
    let router = emulator();
    // A stored agreement, so neither refusal depends on the store being empty.
    let stored = agreement_over(&router, "ap-offer1", vec![term("eula.v1")]).await;

    for (case, id, class) in rows() {
        assert_ne!(
            id, stored,
            "case {case:?}: the id under test must not be the stored agreement"
        );
        for operation in BY_AGREEMENT_ID {
            let answer = call(&router, operation, json!({"agreementId": id})).await;
            assert_class(&format!("{case} via {operation}"), class, &id, &answer);
        }
    }
}
