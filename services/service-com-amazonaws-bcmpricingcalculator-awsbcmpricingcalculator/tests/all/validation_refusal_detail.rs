// SPDX-License-Identifier: CC0-1.0

//! The STRUCTURED half of a refusal: `ValidationException`'s `reason` and `fieldList`,
//! driven end-to-end over this service's awsJson1_0 wire.
//!
//! RULE under test: a request that breaks a constraint this service's model declares is
//! refused with the modelled `ValidationException` (HTTP 400) carrying
//!
//! 1. `reason` = `fieldValidationFailed` — the `ValidationExceptionReason` value for a
//!    named member whose value the service does not accept, in the MODEL's own spelling
//!    (smithy_query: `shape="ValidationExceptionReason"` → `[unknownOperation,
//!    cannotParse, fieldValidationFailed, invalidRequestFromMember, disallowedRate,
//!    other]`); and
//! 2. `fieldList` = one `ValidationExceptionField` per violation, whose `name` is the
//!    member path the violation sits at and whose `message` is that violation's own
//!    clause, naming the constraint the member must satisfy
//!    (`ValidationExceptionField` declares `name` and `message`, both `@required` —
//!    smithy_query: `shape="ValidationExceptionField"`).
//!
//! The `message` envelope itself is pinned by `validation_refusal_framing.rs`, whose
//! clause builders these suites reuse, so the two halves of one body cannot be asserted
//! two ways.
//!
//! The rows span what the rule generalises over: the constraint class (pattern, length
//! maximum, length minimum on a list, range, absence), the member and its shape, the
//! depth of the member (top level and inside a map), the operation — including the three
//! the recorded instances name, one on the account-class-gated surface, and ones they do
//! not — and the NUMBER of violations in one request.
//!
//! The account-class gate's own refusal carries a different `reason`, in a different
//! spelling, and no `fieldList` at all; that half is pinned in `account_class_gate.rs`.

use serde_json::{json, Value};
use service_com_amazonaws_bcmpricingcalculator_awsbcmpricingcalculator::state;

use crate::bill_scenario_lifecycle::{error_code, Emulator};
use crate::validation_refusal_framing::{
    clause, envelope, error_message, pattern_requirement, ARN_PATTERN, NAME_PATTERN,
    PATTERN_ONLY_ARN, PATTERN_ONLY_RESOURCE_ID, RESOURCE_ID_PATTERN, TAG_VALUE_PATTERN,
};

/// The `ValidationExceptionReason` value a refusal of a named member reports: the model's
/// own spelling of the enum value (see the module docs).
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// One `fieldList` entry a row expects: the member path, the subject its clause states
/// (`Value`, or `Value null` for a member that was absent), and the requirement.
struct Field {
    name: &'static str,
    subject: &'static str,
    requirement: String,
}

/// One row: a request the model refuses, and every entry its `fieldList` must carry, in
/// the order the validating walk finds them (schema member order).
struct Case {
    case: &'static str,
    operation: &'static str,
    request: Value,
    fields: Vec<Field>,
}

/// A `BatchCreateWorkloadEstimateUsageEntry` every member of which satisfies the model,
/// so nothing but the member a row is about can be refused. The required members are
/// `key`, `usageAccountId`, `amount`, `serviceCode`, `usageType` and `operation`
/// (smithy_query: `shape="BatchCreateWorkloadEstimateUsageEntry"`).
fn usage_entry() -> Value {
    json!({
        "key": "k0",
        "serviceCode": "AmazonEC2",
        "usageType": "BoxUsage:m5.xlarge",
        "operation": "RunInstances",
        // A 12-digit `AccountId`, the form `^\d{12}$` requires.
        "usageAccountId": "000000000000",
        "amount": 1.0,
    })
}

/// A well-formed `Arn` for this emulator's account, so an ARN row is about the member it
/// names rather than about the ARN's own shape.
fn valid_arn() -> String {
    "arn:aws:bcm-pricing-calculator:us-east-1:000000000000:workload-estimate/\
     0f9e8d7c-6b5a-4938-2716-05f4e3d2c1b0"
        .to_string()
}

fn cases() -> Vec<Case> {
    let over_long_name = "a".repeat(65);
    let over_long_name_with_space = format!("{} x", "a".repeat(64));
    vec![
        Case {
            case: "a @pattern on a required name",
            operation: "CreateWorkloadEstimate",
            request: json!({ "name": "bad name!" }),
            fields: vec![Field {
                name: "name",
                subject: "Value",
                requirement: pattern_requirement(NAME_PATTERN),
            }],
        },
        Case {
            case: "a @pattern on the parent id of a batch create",
            operation: "BatchCreateWorkloadEstimateUsage",
            request: json!({
                "workloadEstimateId": PATTERN_ONLY_RESOURCE_ID,
                "usage": [usage_entry()],
            }),
            fields: vec![Field {
                name: "workloadEstimateId",
                subject: "Value",
                requirement: pattern_requirement(RESOURCE_ID_PATTERN),
            }],
        },
        Case {
            case: "a @pattern on the identifier of an update",
            operation: "UpdateWorkloadEstimate",
            request: json!({ "identifier": PATTERN_ONLY_RESOURCE_ID }),
            fields: vec![Field {
                name: "identifier",
                subject: "Value",
                requirement: pattern_requirement(RESOURCE_ID_PATTERN),
            }],
        },
        Case {
            case: "a @pattern on an ARN, on the tagging surface",
            operation: "ListTagsForResource",
            request: json!({ "arn": PATTERN_ONLY_ARN }),
            fields: vec![Field {
                name: "arn",
                subject: "Value",
                requirement: pattern_requirement(ARN_PATTERN),
            }],
        },
        Case {
            case: "a @pattern on an operation of the account-class-gated surface",
            operation: "CreateBillScenario",
            request: json!({ "name": "bad name!" }),
            fields: vec![Field {
                name: "name",
                subject: "Value",
                requirement: pattern_requirement(NAME_PATTERN),
            }],
        },
        Case {
            case: "a @length maximum",
            operation: "CreateWorkloadEstimate",
            request: json!({ "name": over_long_name }),
            fields: vec![Field {
                name: "name",
                subject: "Value",
                requirement: "Member must have length less than or equal to 64".to_string(),
            }],
        },
        Case {
            case: "a @range maximum",
            operation: "ListBillScenarios",
            request: json!({ "maxResults": 26 }),
            fields: vec![Field {
                name: "maxResults",
                subject: "Value",
                requirement: "Member must have value less than or equal to 25".to_string(),
            }],
        },
        Case {
            case: "an absent @required member",
            operation: "CreateWorkloadEstimate",
            request: json!({}),
            fields: vec![Field {
                name: "name",
                subject: "Value null",
                requirement: "Member must not be null".to_string(),
            }],
        },
        Case {
            case: "two violations of one member, one entry each",
            operation: "CreateWorkloadEstimate",
            request: json!({ "name": over_long_name_with_space }),
            fields: vec![
                Field {
                    name: "name",
                    subject: "Value",
                    requirement: "Member must have length less than or equal to 64".to_string(),
                },
                Field {
                    name: "name",
                    subject: "Value",
                    requirement: pattern_requirement(NAME_PATTERN),
                },
            ],
        },
        Case {
            case: "two members at fault, one entry each",
            operation: "BatchCreateWorkloadEstimateUsage",
            request: json!({ "workloadEstimateId": PATTERN_ONLY_RESOURCE_ID, "usage": [] }),
            fields: vec![
                Field {
                    name: "workloadEstimateId",
                    subject: "Value",
                    requirement: pattern_requirement(RESOURCE_ID_PATTERN),
                },
                Field {
                    name: "usage",
                    subject: "Value",
                    requirement: "Member must have length greater than or equal to 1".to_string(),
                },
            ],
        },
        Case {
            case: "a member nested in a map, named by its whole path",
            operation: "TagResource",
            request: json!({ "arn": valid_arn(), "tags": { "team": "finops!" } }),
            fields: vec![Field {
                name: "tags.team",
                subject: "Value",
                requirement: pattern_requirement(TAG_VALUE_PATTERN),
            }],
        },
    ]
}

/// The `fieldList` of `body` as `(name, message)` in the order the body carries them.
fn field_list(body: &Value) -> Vec<(String, String)> {
    body["fieldList"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    (
                        entry["name"].as_str().unwrap_or_default().to_string(),
                        entry["message"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// RULE (1) and (2): every refused member is named in `fieldList` with the constraint it
/// failed, under the modelled `reason`.
#[tokio::test]
async fn every_refused_member_is_named_with_the_constraint_it_failed() {
    let emulator = Emulator::new();
    for case in cases() {
        let (status, body) = emulator.call(case.operation, case.request.clone()).await;
        assert_eq!(
            status, 400,
            "case {:?} ({}): expected 400, got {status} with {body}",
            case.case, case.operation
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {:?} ({}): expected ValidationException, got {body}",
            case.case,
            case.operation
        );
        assert_eq!(
            body["reason"].as_str(),
            Some(FIELD_VALIDATION_FAILED),
            "case {:?} ({}): expected reason {FIELD_VALIDATION_FAILED:?}, got {:?}",
            case.case,
            case.operation,
            body["reason"]
        );

        let expected: Vec<(String, String)> = case
            .fields
            .iter()
            .map(|field| {
                (
                    field.name.to_string(),
                    clause(field.subject, field.name, &field.requirement),
                )
            })
            .collect();
        let actual = field_list(&body);
        assert_eq!(
            actual, expected,
            "case {:?} ({}): expected fieldList {expected:?}, got {actual:?} in {body}",
            case.case, case.operation
        );
    }
}

/// The INVARIANT that ties the two halves of a refusal body together, over the same
/// table: the `message` is the envelope over exactly the clauses the `fieldList` entries
/// carry, in the same order — so a client reading either half is told about the same
/// members, and the structured half can never state a member the prose does not.
#[tokio::test]
async fn the_field_list_and_the_message_state_the_same_violations() {
    let emulator = Emulator::new();
    for case in cases() {
        let (_, body) = emulator.call(case.operation, case.request.clone()).await;
        let entries = field_list(&body);
        let clauses: Vec<String> = entries.iter().map(|(_, message)| message.clone()).collect();
        let message = error_message(&body);
        assert!(
            !clauses.is_empty(),
            "case {:?} ({}): expected a fieldList, got {body}",
            case.case,
            case.operation
        );
        assert_eq!(
            message,
            envelope(&clauses),
            "case {:?} ({}): expected the message to be the envelope over the fieldList \
             clauses {clauses:?}, got {message:?}",
            case.case,
            case.operation
        );
        for (name, entry_message) in &entries {
            assert!(
                entry_message.contains(&format!("at '{name}'")),
                "case {:?} ({}): entry {name:?} does not name its own member: \
                 {entry_message:?}",
                case.case,
                case.operation
            );
        }
    }
}

/// A request the model accepts carries NEITHER half: `reason` and `fieldList` are the
/// refusal's own members, so an admitted request never reports them. The rows are the
/// same operations the table above refuses, with each member inside its constraint.
#[tokio::test]
async fn an_accepted_request_reports_no_reason_and_no_field_list() {
    let emulator = Emulator::new();
    let rows = [
        ("CreateWorkloadEstimate", json!({ "name": "fine-name" })),
        ("CreateBillScenario", json!({ "name": "fine-name" })),
        ("ListBillScenarios", json!({ "maxResults": 25 })),
        (
            "UpdateWorkloadEstimate",
            json!({ "identifier": "0f9e8d7c-6b5a-4938-2716-05f4e3d2c1b0" }),
        ),
        (
            "BatchCreateWorkloadEstimateUsage",
            json!({
                "workloadEstimateId": "0f9e8d7c-6b5a-4938-2716-05f4e3d2c1b0",
                "usage": [usage_entry()],
            }),
        ),
    ];
    for (operation, request) in rows {
        let (status, body) = emulator.call(operation, request.clone()).await;
        assert_ne!(
            error_code(&body),
            "ValidationException",
            "case {operation} {request}: an accepted request was refused with {status} \
             and {body}"
        );
        assert!(
            body.get("fieldList").is_none(),
            "case {operation} {request}: expected no fieldList, got {body}"
        );
        assert_ne!(
            body["reason"].as_str(),
            Some(FIELD_VALIDATION_FAILED),
            "case {operation} {request}: expected no field-validation reason, got {body}"
        );
    }
}

/// The account-class gate's spelling of a `ValidationExceptionReason` value, over EVERY
/// value the model's enum admits (smithy_query: `shape="ValidationExceptionReason"`) —
/// including the values no refusal of this emulator names today, so the rendering is
/// pinned for the next one that does.
///
/// Each lowerCamel boundary becomes an `_`, which is the inverse of the spelling Smithy
/// enum values use; the gate puts that form on the wire (recorded behaviour) while the
/// modelled input validator above puts the model's own spelling there.
#[test]
fn the_gate_spells_every_modelled_reason_in_screaming_snake_case() {
    let rows = [
        ("unknownOperation", "UNKNOWN_OPERATION"),
        ("cannotParse", "CANNOT_PARSE"),
        ("fieldValidationFailed", "FIELD_VALIDATION_FAILED"),
        ("invalidRequestFromMember", "INVALID_REQUEST_FROM_MEMBER"),
        ("disallowedRate", "DISALLOWED_RATE"),
        ("other", "OTHER"),
    ];
    for (modelled, expected) in rows {
        let actual = state::gate_reason(modelled);
        assert_eq!(
            actual, expected,
            "case {modelled:?}: expected {expected:?}, got {actual:?}"
        );
    }
}
