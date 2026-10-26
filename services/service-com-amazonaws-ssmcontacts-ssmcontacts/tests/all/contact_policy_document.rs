// SPDX-License-Identifier: CC0-1.0

//! The grammar `PutContactPolicy` holds a contact's resource policy to, end to end over
//! the wire.
//!
//! One rule drives every suite below:
//!
//! > A member that carries a RESOURCE-BASED policy document is satisfied only by a document
//! > in the IAM JSON policy language — an object whose `Statement` holds at least one
//! > statement, each stating an `Effect` of `Allow` or `Deny`, at least one action and the
//! > principal the permission is granted to, under a `Version` the policy language has —
//! > and a body outside that language is refused with the operation's modelled
//! > `ValidationException` (HTTP 400) without being written.
//!
//! Each suite is a TABLE rather than one call: the accepting rows differ along the forms
//! the language admits (`Statement` as one object or as a list, one statement or several,
//! an element written as a string, as a list of them or as the `{"AWS": …}` object a
//! principal is written as, with and without a `Version`), and the refusing rows along the
//! clause of the language the document breaks (not JSON at all, not an object, a version no
//! policy language has, no statement, a statement that is not an object, and a statement
//! missing each of its three required elements in turn).
//!
//! The document the issue this suite was filed for cited — a JSON object carrying none of a
//! policy's elements — is ONE refusing row among them.

use serde_json::{json, Value};

use crate::contact_lifecycle::{error_code, Emulator, ACCOUNT, REGION};
use crate::data_plane_precondition::onboarded_contact;

// ---------------------------------------------------------------------------
// The documents
// ---------------------------------------------------------------------------

/// A statement that grants one action to one principal: the smallest statement a
/// resource-based policy can carry. A resource-based policy may omit `Resource` — the
/// resource the document is attached to is the implicit one — which is why the accepting
/// rows below do so.
const STATEMENT: &str = r#"{"Effect": "Allow", "Principal": {"AWS": "000000000000"},
     "Action": "ssm-contacts:GetContact"}"#;

/// A document the policy language admits.
struct Accepted {
    case: &'static str,
    document: String,
}

/// Every form of the language this suite spans, built around [`STATEMENT`].
fn accepted_documents() -> Vec<Accepted> {
    let row = |case: &'static str, document: String| Accepted { case, document };
    vec![
        row(
            "one statement in a list",
            format!(r#"{{"Version": "2012-10-17", "Statement": [{STATEMENT}]}}"#),
        ),
        row(
            "two statements in a list",
            format!(r#"{{"Version": "2012-10-17", "Statement": [{STATEMENT}, {STATEMENT}]}}"#),
        ),
        row(
            "one statement written as the object itself",
            format!(r#"{{"Version": "2012-10-17", "Statement": {STATEMENT}}}"#),
        ),
        row(
            "a document with no Version, which the language lets a policy omit",
            format!(r#"{{"Statement": [{STATEMENT}]}}"#),
        ),
        // SOURCE: the IAM JSON policy reference documents exactly two `Version` values —
        // `2012-10-17`, the current one, and `2008-10-17`, the legacy one. The row above
        // covers the current one; this row is the other published value.
        row(
            "the legacy policy language version",
            format!(r#"{{"Version": "2008-10-17", "Statement": [{STATEMENT}]}}"#),
        ),
        row(
            "a principal written as a string and an action written as a list",
            r#"{"Version": "2012-10-17", "Statement": [
                {"Sid": "DenyEveryone", "Effect": "Deny", "Principal": "*",
                 "Action": ["ssm-contacts:GetContact", "ssm-contacts:DescribeEngagement"],
                 "Resource": "*",
                 "Condition": {"StringNotEquals": {"aws:PrincipalOrgID": "o-1234567890"}}}
            ]}"#
                .to_string(),
        ),
        row(
            "the complements of the action and principal elements",
            r#"{"Version": "2012-10-17", "Statement": [
                {"Effect": "Deny", "NotPrincipal": {"AWS": ["000000000000"]},
                 "NotAction": "ssm-contacts:GetContact"}
            ]}"#
                .to_string(),
        ),
    ]
}

/// A body outside the policy language, and the clause of it the body breaks — a phrase the
/// refusal has to carry, so a caller is told WHAT the document failed to be rather than only
/// that it failed.
struct Refused {
    case: &'static str,
    document: String,
    clause: &'static str,
}

/// One row per clause of the language, each written in the smallest body that breaks it.
fn refused_documents() -> Vec<Refused> {
    let row = |case: &'static str, document: String, clause: &'static str| Refused {
        case,
        document,
        clause,
    };
    vec![
        row("text that is not JSON at all", "not a policy".to_string(), "not JSON"),
        row("a truncated object", r#"{"Version":"#.to_string(), "not JSON"),
        row("a JSON number", "17".to_string(), "not a JSON object"),
        row("a JSON string", r#""policy""#.to_string(), "not a JSON object"),
        row("a JSON null", "null".to_string(), "not a JSON object"),
        row("a JSON boolean", "true".to_string(), "not a JSON object"),
        row(
            "a bare list of statements with no document around them",
            format!("[{STATEMENT}]"),
            "not a JSON object",
        ),
        // The document the issue cited: a JSON object, and nothing else about it is a
        // policy.
        row("an object carrying none of a policy's elements", r#"{"foo": 1}"#.to_string(), "no statement"),
        row("an empty object", "{}".to_string(), "no statement"),
        row(
            "a Statement that is an empty list",
            r#"{"Version": "2012-10-17", "Statement": []}"#.to_string(),
            "no statement",
        ),
        row(
            "a Statement that is a string",
            r#"{"Version": "2012-10-17", "Statement": "let me in"}"#.to_string(),
            "no statement",
        ),
        row(
            "a statement entry that is not an object",
            r#"{"Version": "2012-10-17", "Statement": ["let me in"]}"#.to_string(),
            "statement is not a JSON object",
        ),
        row(
            "a version no policy language has",
            format!(r#"{{"Version": "2024-01-01", "Statement": [{STATEMENT}]}}"#),
            "version",
        ),
        row(
            "a statement with no Effect",
            r#"{"Statement": [{"Principal": "*", "Action": "ssm-contacts:GetContact"}]}"#
                .to_string(),
            "Effect",
        ),
        row(
            "a statement whose Effect is neither Allow nor Deny",
            r#"{"Statement": [{"Effect": "Maybe", "Principal": "*",
                 "Action": "ssm-contacts:GetContact"}]}"#
                .to_string(),
            "Effect",
        ),
        row(
            "a statement with no action",
            r#"{"Statement": [{"Effect": "Allow", "Principal": "*"}]}"#.to_string(),
            "Action",
        ),
        row(
            "a statement whose Action list is empty",
            r#"{"Statement": [{"Effect": "Allow", "Principal": "*", "Action": []}]}"#
                .to_string(),
            "Action",
        ),
        row(
            "a statement with no principal, which an IDENTITY policy omits but a resource \
             policy states",
            r#"{"Statement": [{"Effect": "Allow", "Action": "ssm-contacts:GetContact",
                 "Resource": "*"}]}"#
                .to_string(),
            "Principal",
        ),
        row(
            "a statement whose Principal object names nothing",
            r#"{"Statement": [{"Effect": "Allow", "Action": "ssm-contacts:GetContact",
                 "Principal": {}}]}"#
                .to_string(),
            "Principal",
        ),
        // The grammar holds of EVERY statement, not only the first: a list whose last
        // statement is broken is not a policy either.
        row(
            "a list whose LAST statement is broken",
            format!(
                r#"{{"Statement": [{STATEMENT},
                     {{"Effect": "Allow", "Action": "ssm-contacts:GetContact"}}]}}"#
            ),
            "Principal",
        ),
    ]
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// What `GetContactPolicy` reports as `contact`'s policy right now, or `None` when it
/// carries none.
async fn stored_policy(emulator: &Emulator, contact: &str, case: &str) -> Option<String> {
    let (status, body) = emulator
        .call("GetContactPolicy", json!({"ContactArn": contact}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetContactPolicy to answer 200, got {status} {body:?}"
    );
    assert_eq!(
        body["ContactArn"].as_str(),
        Some(contact),
        "case {case:?}: expected the contact's own ARN, got {body:?}"
    );
    body.get("Policy")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Assert that `body` is the operation's modelled `ValidationException` for a `Policy` that
/// is not a resource-based policy document: it names the member at fault and says which
/// clause of the policy language the document broke.
fn assert_policy_validation(case: &str, status: u16, body: &Value, clause: &str) {
    assert_eq!(status, 400, "case {case:?}: expected HTTP 400, got {status} {body:?}");
    assert!(
        error_code(body).contains("ValidationException"),
        "case {case:?}: expected a ValidationException, got {body:?}"
    );
    let message = body["Message"].as_str().unwrap_or_default();
    assert!(
        message.contains("policy"),
        "case {case:?}: expected the message to name the member at fault, got {message:?}"
    );
    assert!(
        message.contains(clause),
        "case {case:?}: expected the message to say the document {clause:?}, got {message:?}"
    );
    // `FIELD_VALIDATION_FAILED` is the modelled `ValidationExceptionReason` for a fault in
    // one field of the request, which this is: the document the request carried.
    let reason = body["Reason"].as_str().unwrap_or_default();
    assert_eq!(
        reason, "FIELD_VALIDATION_FAILED",
        "case {case:?}: expected the field-validation reason, got {body:?}"
    );
    let fields = body["Fields"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        fields.len(),
        1,
        "case {case:?}: expected exactly the member at fault, got {body:?}"
    );
    assert_eq!(
        fields[0]["Name"].as_str(),
        Some("policy"),
        "case {case:?}: expected the fault to name the policy member, got {body:?}"
    );
    assert!(
        fields[0]["Message"]
            .as_str()
            .is_some_and(|text| text.contains(clause)),
        "case {case:?}: expected the field's message to say the document {clause:?}, got \
         {body:?}"
    );
}

/// An ARN shaped like a contact's that nothing ever created, for the ordering suite.
fn absent_contact_arn() -> String {
    format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/never-created")
}

// ---------------------------------------------------------------------------
// Suites
// ---------------------------------------------------------------------------

/// Every form the policy language admits is attached and reads back VERBATIM: the grammar
/// refuses what is not a policy without narrowing what is one.
#[tokio::test]
async fn every_form_of_the_policy_language_is_attached_and_read_back() {
    let emulator = Emulator::new();
    let contact = onboarded_contact(&emulator, "oncall-primary").await;

    // A contact that has had no put carries no policy, so a reported document below is one
    // this suite's put wrote.
    let before = stored_policy(&emulator, &contact, "before any put").await;
    assert!(
        before.is_none(),
        "expected a contact with no put to carry no policy, got {before:?}"
    );

    for Accepted { case, document } in accepted_documents() {
        let (status, body) = emulator
            .call(
                "PutContactPolicy",
                json!({"ContactArn": contact, "Policy": document}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutContactPolicy to answer 200, got {status} {body:?}"
        );
        let stored = stored_policy(&emulator, &contact, case).await;
        assert_eq!(
            stored.as_deref(),
            Some(document.as_str()),
            "case {case:?}: expected the document the put carried, got {stored:?}"
        );
    }
}

/// A body outside the policy language is refused, and the contact keeps the policy it had:
/// a refused put writes nothing.
#[tokio::test]
async fn a_body_outside_the_policy_language_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let contact = onboarded_contact(&emulator, "oncall-primary").await;

    // The policy the contact carries before each refusal, so "writes nothing" is asserted
    // against a document that really is there rather than against the absence of one.
    let kept = accepted_documents()
        .into_iter()
        .next()
        .expect("the accepting table is not empty")
        .document;
    let (status, body) = emulator
        .call(
            "PutContactPolicy",
            json!({"ContactArn": contact, "Policy": kept}),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected the policy the refusals must not disturb to be attached, got {status} \
         {body:?}"
    );

    for Refused {
        case,
        document,
        clause,
    } in refused_documents()
    {
        let (status, body) = emulator
            .call(
                "PutContactPolicy",
                json!({"ContactArn": contact, "Policy": document}),
            )
            .await;
        assert_policy_validation(case, status, &body, clause);

        let stored = stored_policy(&emulator, &contact, case).await;
        assert_eq!(
            stored.as_deref(),
            Some(kept.as_str()),
            "case {case:?}: a refused put must leave the stored policy alone, got {stored:?}"
        );
    }
}

/// The rule is about the REQUEST, so it is applied before the ARN the request carries is
/// resolved: a document outside the policy language naming a contact that does not exist is
/// refused for the document, not reported as absent.
#[tokio::test]
async fn the_document_is_judged_before_the_contact_arn_is_resolved() {
    let emulator = Emulator::new();
    let absent = absent_contact_arn();

    for Refused {
        case,
        document,
        clause,
    } in refused_documents()
    {
        let (status, body) = emulator
            .call(
                "PutContactPolicy",
                json!({"ContactArn": absent, "Policy": document}),
            )
            .await;
        assert_policy_validation(case, status, &body, clause);
    }

    // The control: the SAME absent ARN with a document the language admits is reported
    // absent, which is what makes the refusals above statements about the document.
    let document = accepted_documents()
        .into_iter()
        .next()
        .expect("the accepting table is not empty")
        .document;
    let (status, body) = emulator
        .call(
            "PutContactPolicy",
            json!({"ContactArn": absent, "Policy": document}),
        )
        .await;
    assert_eq!(
        status, 404,
        "case \"a policy on an absent contact\": expected HTTP 404, got {status} {body:?}"
    );
    assert!(
        error_code(&body).contains("ResourceNotFoundException"),
        "case \"a policy on an absent contact\": expected a ResourceNotFoundException, got \
         {body:?}"
    );
}

/// `PutContactPolicy` attaches a policy to "a contact OR escalation plan", so the rule holds
/// of every contact type the model admits, not only of the personal contact the issue cited.
#[tokio::test]
async fn the_rule_holds_for_every_contact_type_a_policy_attaches_to() {
    // SOURCE: the model's `ContactType` value set, rendered into
    // `src/ops/types/shared_types.rs` as `CONTACT_TYPE_VALUES`.
    for contact_type in ["PERSONAL", "ESCALATION", "ONCALL_SCHEDULE"] {
        let emulator = Emulator::new();
        let request = json!({
            "Alias": format!("subject-{}", contact_type.to_lowercase()),
            "Type": contact_type,
            "Plan": {"Stages": [{"DurationInMinutes": 5, "Targets": []}]},
        });
        let (status, body) = emulator.call("CreateContact", request).await;
        assert_eq!(
            status, 200,
            "case {contact_type:?}: expected CreateContact to answer 200, got {status} {body:?}"
        );
        let contact = body["ContactArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {contact_type:?}: CreateContact must answer the ARN"))
            .to_string();

        let document = accepted_documents()
            .into_iter()
            .next()
            .expect("the accepting table is not empty")
            .document;
        let (status, body) = emulator
            .call(
                "PutContactPolicy",
                json!({"ContactArn": contact, "Policy": document}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {contact_type:?}: expected a policy to attach, got {status} {body:?}"
        );

        let (status, body) = emulator
            .call(
                "PutContactPolicy",
                json!({"ContactArn": contact, "Policy": r#"{"foo": 1}"#}),
            )
            .await;
        assert_policy_validation(contact_type, status, &body, "no statement");

        let stored = stored_policy(&emulator, &contact, contact_type).await;
        assert_eq!(
            stored.as_deref(),
            Some(document.as_str()),
            "case {contact_type:?}: a refused put must leave the stored policy alone, got \
             {stored:?}"
        );
    }
}
