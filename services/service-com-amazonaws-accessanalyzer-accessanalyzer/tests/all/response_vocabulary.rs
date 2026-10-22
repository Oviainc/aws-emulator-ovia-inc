// SPDX-License-Identifier: CC0-1.0

//! The VALUES this service puts in machine- and human-read response members, driven over
//! the wire.
//!
//! Two rules are under test, each over a table that spans what it generalises over rather
//! than over one recorded request:
//!
//! 1. A `ValidationException` spells its `reason` the way the model names the
//!    discriminator — the SCREAMING_SNAKE `name` the model's `ValidationExceptionReason`
//!    enum pairs with each lowerCamel `value` (see `harness::VALIDATION_REASONS`) — in
//!    every refusal this service raises, whichever operation raises it; and the
//!    discriminator it picks classifies the refusal: a request this service cannot act on
//!    is a FIELD-validation failure, a document it cannot parse is `cannotParse`.//! 2. `CheckNoNewAccess$message` is the service's fixed sentence for the `result` it
//!    reports and for nothing else: every request reaching a verdict gets that verdict's
//!    sentence, with none of the request's own content in it.
//!
//! Every case goes through the real router (`tests/all/harness.rs`), so restJson1 routing,
//! the boundary constraint validation, the handler and the response serialization all run
//! exactly as they do in the emulator binary.

use serde_json::{json, Value};
use service_com_amazonaws_accessanalyzer_accessanalyzer::ops::types::VALIDATION_EXCEPTION_REASON_VALUES;
use service_com_amazonaws_accessanalyzer_accessanalyzer::state;

use crate::harness::{wire_reason, Emulator, Reply, VALIDATION_REASONS};

/// The route `CheckNoNewAccess` is bound to, from the crate's `OPERATIONS.md`:
/// `POST /policy/check-no-new-access -> 200`.
const CHECK_NO_NEW_ACCESS: &str = "/policy/check-no-new-access";

/// The route `CheckAccessNotGranted` is bound to, from the same file:
/// `POST /policy/check-access-not-granted -> 200`.
const CHECK_ACCESS_NOT_GRANTED: &str = "/policy/check-access-not-granted";

/// The sentence a `PASS` reports, as the handler's `MESSAGE_PASS` holds it.
const MESSAGE_PASS: &str = "The modified permissions do not grant new access.";

/// The sentence a `FAIL` reports, as the handler's `MESSAGE_FAIL` holds it — AWS's own
/// text for that verdict, naming the baseline the comparison was made against.
const MESSAGE_FAIL: &str = "The modified permissions grant new access compared to your existing policy.";

/// The modelled discriminator for a member that failed validation.
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The modelled discriminator for a document that could not be parsed at all.
const CANNOT_PARSE: &str = "cannotParse";

/// A single-statement identity policy allowing `actions` on `resources`.
fn identity_allow(actions: Value, resources: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Action": actions, "Resource": resources}],
    })
}

/// A single-statement resource policy granting `principal` `sqs:SendMessage` on a queue.
fn resource_allow(principal: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": principal,
            "Action": "sqs:SendMessage",
            "Resource": "arn:aws:sqs:us-east-1:111122223333:jobs",
        }],
    })
}

/// Run one comparison over the wire. Both documents travel as JSON *strings*, as
/// `AccessCheckPolicyDocument` is modelled.
async fn check_no_new_access(
    emulator: &Emulator,
    policy_type: &str,
    existing: &Value,
    updated: &Value,
) -> Reply {
    emulator
        .send(
            "POST",
            CHECK_NO_NEW_ACCESS,
            Some(json!({
                "existingPolicyDocument": existing.to_string(),
                "newPolicyDocument": updated.to_string(),
                "policyType": policy_type,
            })),
        )
        .await
}

/// The names a refusal's `fieldList` carries, in reply order.
fn named_fields(reply: &Reply) -> Vec<String> {
    reply
        .member("fieldList")
        .and_then(Value::as_array)
        .map(|fields| {
            fields
                .iter()
                .map(|field| {
                    field
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The service's reason vocabulary IS the model's: the same discriminator/name pairs, and
/// the value a refusal names is always the `name` half.
///
/// The expectation here is the model-cited table in `harness`, which is written out
/// independently of the crate's own copy in `state`.
#[test]
fn the_reason_vocabulary_is_the_models_own() {
    assert_eq!(
        state::VALIDATION_REASONS, VALIDATION_REASONS,
        "the service's reason table must be the model's enum pairs, got {:?}",
        state::VALIDATION_REASONS
    );

    // The discriminator half is exactly the generated value list the model carries.
    let mut discriminators: Vec<&str> = state::VALIDATION_REASONS
        .iter()
        .map(|(discriminator, _)| *discriminator)
        .collect();
    discriminators.sort_unstable();
    let mut modelled = VALIDATION_EXCEPTION_REASON_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        discriminators, modelled,
        "the table must pair every modelled discriminator and no others, got {discriminators:?}"
    );

    // Every value a refusal can name is the model's name for its discriminator, and never
    // the lowerCamel discriminator itself.
    for (discriminator, named) in [
        (
            FIELD_VALIDATION_FAILED,
            state::VALIDATION_REASON_FIELD_VALIDATION_FAILED,
        ),
        (CANNOT_PARSE, state::VALIDATION_REASON_CANNOT_PARSE),
    ] {
        assert_eq!(
            named,
            wire_reason(discriminator),
            "case {discriminator:?}: a refusal must name {:?}, got {named:?}",
            wire_reason(discriminator)
        );
        assert_ne!(
            named, discriminator,
            "case {discriminator:?}: the lowerCamel discriminator is not what a response \
             carries, got {named:?}"
        );
    }

    // The lookup answers for every modelled discriminator, and for nothing else — a wire
    // name is not a discriminator, so it cannot be transformed twice.
    for (discriminator, name) in VALIDATION_REASONS {
        assert_eq!(
            state::validation_reason_wire(discriminator),
            Some(*name),
            "case {discriminator:?}: the lookup must answer {name:?}"
        );
        assert_eq!(
            state::validation_reason_wire(name),
            None,
            "case {name:?}: a wire name is not a modelled discriminator"
        );
    }
}

/// One refused request, and the reason the rule says it carries.
struct RefusalCase {
    /// What this row varies, named in every assertion.
    case: String,
    reply: Reply,
    /// The modelled discriminator the refusal must classify itself with.
    discriminator: &'static str,
    /// The members `fieldList` must name; empty when the refusal is about the request as
    /// a whole and so names no member.
    fields: Vec<&'static str>,
}

/// RULE 1: every refusal this service raises carries its `reason` in the model's name
/// spelling, and the discriminator classifies what was refused.
#[tokio::test]
async fn every_refusal_carries_its_reason_in_the_wire_spelling() {
    let emulator = Emulator::new();

    // The analyzers the rows refuse against: both external-access types (no
    // configuration at all) and one configured type (the wrong union member).
    for (name, analyzer_type) in [
        ("external_account", "ACCOUNT"),
        ("external_org", "ORGANIZATION"),
        ("configured_unused", "ACCOUNT_UNUSED_ACCESS"),
    ] {
        let created = emulator
            .create_analyzer(json!({"analyzerName": name, "type": analyzer_type}))
            .await;
        assert_eq!(
            created.status, 200,
            "CreateAnalyzer must succeed for {name}, got {created:?}"
        );
    }

    let good_policy = identity_allow(json!("s3:GetObject"), json!("arn:aws:s3:::reports/*"));

    // The rows span the refusal CLASSES this service raises — an action the analyzer's
    // type does not support, a union member the type does not take, a documented bound,
    // a modelled constraint, a paging argument, an unparseable document, a bounded list
    // member — across six operations, so the rule is about every refusal rather than
    // about one handler.
    let mut cases = vec![
        RefusalCase {
            case: "UpdateAnalyzer against an ACCOUNT analyzer: the action is not supported"
                .to_string(),
            reply: emulator
                .update_analyzer(
                    "external_account",
                    json!({"configuration": {"unusedAccess": {"unusedAccessAge": 90}}}),
                )
                .await,
            discriminator: FIELD_VALIDATION_FAILED,
            fields: vec![],
        },
        RefusalCase {
            case: "UpdateAnalyzer against an ORGANIZATION analyzer carrying no configuration"
                .to_string(),
            reply: emulator.update_analyzer("external_org", json!({})).await,
            discriminator: FIELD_VALIDATION_FAILED,
            fields: vec![],
        },
        RefusalCase {
            case: "UpdateAnalyzer with the other family's union member".to_string(),
            reply: emulator
                .update_analyzer(
                    "configured_unused",
                    json!({"configuration": {"internalAccess": {"analysisRule": {"inclusions": [
                        {"resourceTypes": ["AWS::S3::Bucket"]}
                    ]}}}}),
                )
                .await,
            discriminator: FIELD_VALIDATION_FAILED,
            fields: vec!["configuration"],
        },
        RefusalCase {
            case: "UpdateAnalyzer with an unusedAccessAge past its documented maximum".to_string(),
            reply: emulator
                .update_analyzer(
                    "configured_unused",
                    json!({"configuration": {"unusedAccess": {"unusedAccessAge": 366}}}),
                )
                .await,
            discriminator: FIELD_VALIDATION_FAILED,
            fields: vec!["configuration.unusedAccess.unusedAccessAge"],
        },
        RefusalCase {
            case: "CreateAnalyzer with an unusedAccessAge below its documented minimum"
                .to_string(),
            reply: emulator
                .create_analyzer(json!({
                    "analyzerName": "age_zero",
                    "type": "ACCOUNT_UNUSED_ACCESS",
                    "configuration": {"unusedAccess": {"unusedAccessAge": 0}},
                }))
                .await,
            discriminator: FIELD_VALIDATION_FAILED,
            fields: vec!["configuration.unusedAccess.unusedAccessAge"],
        },
        RefusalCase {
            case: "ListAnalyzers with a maxResults of 0".to_string(),
            reply: emulator.list_analyzers("?maxResults=0").await,
            discriminator: FIELD_VALIDATION_FAILED,
            fields: vec!["maxResults"],
        },
        RefusalCase {
            case: "ListArchiveRules with a nextToken this service never issued".to_string(),
            reply: emulator
                .list_archive_rules("external_account", "?nextToken=not-a-token")
                .await,
            discriminator: FIELD_VALIDATION_FAILED,
            fields: vec!["nextToken"],
        },
        RefusalCase {
            case: "ListPolicyGenerations with a principalArn outside its modelled pattern"
                .to_string(),
            reply: emulator.list_policy_generations("?principalArn=not-an-arn").await,
            discriminator: FIELD_VALIDATION_FAILED,
            fields: vec!["principalArn"],
        },
        RefusalCase {
            case: "CheckAccessNotGranted with an empty access list".to_string(),
            reply: emulator
                .send(
                    "POST",
                    CHECK_ACCESS_NOT_GRANTED,
                    Some(json!({
                        "policyDocument": good_policy.to_string(),
                        "policyType": "IDENTITY_POLICY",
                        "access": [],
                    })),
                )
                .await,
            discriminator: FIELD_VALIDATION_FAILED,
            fields: vec!["access"],
        },
        RefusalCase {
            case: "CheckAccessNotGranted with an access entry naming neither actions nor resources"
                .to_string(),
            reply: emulator
                .send(
                    "POST",
                    CHECK_ACCESS_NOT_GRANTED,
                    Some(json!({
                        "policyDocument": good_policy.to_string(),
                        "policyType": "IDENTITY_POLICY",
                        "access": [{"actions": [], "resources": []}],
                    })),
                )
                .await,
            discriminator: FIELD_VALIDATION_FAILED,
            fields: vec!["access"],
        },
    ];

    // The documents that cannot be parsed at all: each member in turn, and two ways of
    // not being a JSON policy.
    for (member, existing, updated) in [
        ("existingPolicyDocument", "{".to_string(), good_policy.to_string()),
        (
            "newPolicyDocument",
            good_policy.to_string(),
            "not json at all".to_string(),
        ),
    ] {
        cases.push(RefusalCase {
            case: format!("CheckNoNewAccess with an unparseable {member}"),
            reply: emulator
                .send(
                    "POST",
                    CHECK_NO_NEW_ACCESS,
                    Some(json!({
                        "existingPolicyDocument": existing,
                        "newPolicyDocument": updated,
                        "policyType": "IDENTITY_POLICY",
                    })),
                )
                .await,
            discriminator: CANNOT_PARSE,
            fields: vec![member],
        });
    }

    for RefusalCase {
        case,
        reply,
        discriminator,
        fields,
    } in &cases
    {
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a refused request is HTTP 400, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            "ValidationException",
            "case {case:?}: the refusal must name the modelled error, got {reply:?}"
        );
        let reason = reply
            .member("reason")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            reason,
            wire_reason(discriminator),
            "case {case:?}: expected reason {:?}, got {reason:?} in {reply:?}",
            wire_reason(discriminator)
        );
        assert!(
            VALIDATION_REASONS.iter().any(|(_, name)| *name == reason),
            "case {case:?}: {reason:?} is not a name the model's \
             ValidationExceptionReason declares"
        );
        assert!(
            reply
                .member("message")
                .and_then(Value::as_str)
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: the modelled `message` is @required, got {reply:?}"
        );
        let named = named_fields(reply);
        assert_eq!(
            named,
            fields.iter().map(|name| name.to_string()).collect::<Vec<_>>(),
            "case {case:?}: expected fieldList to name {fields:?}, got {named:?} in {reply:?}"
        );
    }
}

/// One comparison, and the verdict the rule says it reaches.
struct MessageCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    policy_type: &'static str,
    existing: Value,
    updated: Value,
    /// The modelled `CheckNoNewAccessResult` the comparison reaches.
    result: &'static str,
}

/// RULE 2: the `message` is the service's sentence for the `result` and for nothing else.
#[tokio::test]
async fn check_no_new_access_message_is_the_sentence_for_its_result() {
    let role = "arn:aws:iam::111122223333:role/app";
    let other_role = "arn:aws:iam::111122223333:role/batch";

    // The rows differ along which element widens (action, resource, principal), the form
    // the element takes (a single string, a list, a wildcard), how many statements report
    // new access, the policy type, and the direction of the change — so both sentences
    // are asserted over a CLASS of requests, not over one document.
    let cases = vec![
        MessageCase {
            case: "FAIL: the action widened to a wildcard",
            policy_type: "IDENTITY_POLICY",
            existing: identity_allow(json!("s3:GetObject"), json!("arn:aws:s3:::reports/*")),
            updated: identity_allow(json!("s3:*"), json!("arn:aws:s3:::reports/*")),
            result: "FAIL",
        },
        MessageCase {
            case: "FAIL: the resource widened to a wildcard",
            policy_type: "IDENTITY_POLICY",
            existing: identity_allow(json!("s3:GetObject"), json!("arn:aws:s3:::reports/q1")),
            updated: identity_allow(json!("s3:GetObject"), json!("arn:aws:s3:::reports/*")),
            result: "FAIL",
        },
        MessageCase {
            case: "FAIL: a second action added as a list",
            policy_type: "IDENTITY_POLICY",
            existing: identity_allow(json!("s3:GetObject"), json!("arn:aws:s3:::reports/*")),
            updated: identity_allow(
                json!(["s3:GetObject", "s3:PutObject"]),
                json!("arn:aws:s3:::reports/*"),
            ),
            result: "FAIL",
        },
        MessageCase {
            case: "FAIL: two statements each grant new access",
            policy_type: "IDENTITY_POLICY",
            existing: identity_allow(json!("s3:GetObject"), json!("arn:aws:s3:::reports/*")),
            updated: json!({"Version": "2012-10-17", "Statement": [
                {"Effect": "Allow", "Action": "s3:PutObject", "Resource": "arn:aws:s3:::reports/*"},
                {"Effect": "Allow", "Action": "sqs:SendMessage", "Resource": "arn:aws:sqs:us-east-1:111122223333:jobs"},
            ]}),
            result: "FAIL",
        },
        MessageCase {
            case: "FAIL: a resource policy gained a principal",
            policy_type: "RESOURCE_POLICY",
            existing: resource_allow(json!({"AWS": role})),
            updated: resource_allow(json!({"AWS": [role, other_role]})),
            result: "FAIL",
        },
        MessageCase {
            case: "PASS: the two documents are the same",
            policy_type: "IDENTITY_POLICY",
            existing: identity_allow(json!("s3:GetObject"), json!("arn:aws:s3:::reports/*")),
            updated: identity_allow(json!("s3:GetObject"), json!("arn:aws:s3:::reports/*")),
            result: "PASS",
        },
        MessageCase {
            case: "PASS: the action narrowed",
            policy_type: "IDENTITY_POLICY",
            existing: identity_allow(json!("s3:*"), json!("arn:aws:s3:::reports/*")),
            updated: identity_allow(json!("s3:GetObject"), json!("arn:aws:s3:::reports/*")),
            result: "PASS",
        },
        MessageCase {
            case: "PASS: a resource policy lost a principal",
            policy_type: "RESOURCE_POLICY",
            existing: resource_allow(json!({"AWS": [role, other_role]})),
            updated: resource_allow(json!({"AWS": role})),
            result: "PASS",
        },
    ];

    let emulator = Emulator::new();

    for MessageCase {
        case,
        policy_type,
        existing,
        updated,
        result,
    } in &cases
    {
        let reply = check_no_new_access(&emulator, policy_type, existing, updated).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a decided comparison is HTTP 200, got {reply:?}"
        );
        let reported = reply
            .member("result")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            reported, *result,
            "case {case:?}: expected result {result:?}, got {reported:?} in {reply:?}"
        );
        let expected = if *result == "FAIL" {
            MESSAGE_FAIL
        } else {
            MESSAGE_PASS
        };
        let message = reply
            .member("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            message, expected,
            "case {case:?}: expected the {result} sentence {expected:?}, got {message:?}"
        );
        // The sentence is a function of the result alone: nothing the request carried —
        // an action, a resource, a principal, a policy type — appears in it.
        for leaked in [
            "s3:",
            "sqs:",
            "arn:",
            role,
            other_role,
            policy_type,
            "Statement",
        ] {
            assert!(
                !message.contains(leaked),
                "case {case:?}: the message must carry no request content, but {message:?} \
                 contains {leaked:?}"
            );
        }
    }
}
