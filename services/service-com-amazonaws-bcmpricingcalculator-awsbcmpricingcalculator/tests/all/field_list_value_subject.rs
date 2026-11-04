// SPDX-License-Identifier: CC0-1.0

//! The SUBJECT of a `fieldList` entry's own message, driven end-to-end over this
//! service's awsJson1_0 wire.
//!
//! RULE under test: the two halves of one `ValidationException` body are not the same
//! sentence. The `message` ENVELOPE is prose about a whole request, so each of its clauses
//! names the member path the violation sits at and nothing of the value
//! (`Value at '<path>' failed to satisfy constraint: <requirement>`, pinned by
//! `validation_refusal_framing.rs`). A `fieldList` ENTRY is a structure whose own `name`
//! already names that member (`ValidationExceptionField` declares `name` and `message`,
//! both `@required` — smithy_query: `shape="ValidationExceptionField"`), so its `message`
//! identifies no member a second time and is a sentence ABOUT THE VALUE the entry refused:
//!
//! 1. a member the request CARRIED a value for states that value as the fixed
//!    [`WITHHELD`] marker — `Value [redacted] failed to satisfy constraint: <requirement>`
//!    — which is the same text whatever the member, whatever the operation and whatever
//!    the value, and is never derived from the request;
//! 2. a member the request did NOT carry has no value to withhold and keeps the
//!    [`NULL`] its absence is — `Value null failed to satisfy constraint: Member must not
//!    be null`;
//! 3. the REQUIREMENT an entry states is the one the envelope's clause for that violation
//!    states, so the structured half and the prose half cannot word one constraint two
//!    ways;
//! 4. no entry message carries a member path (`at '<path>'`), because its `name` does.
//!
//! The rows span what the rule generalises over: the constraint class (pattern, length
//! maximum, length minimum, range maximum, range minimum, absence), the member and its
//! shape, the depth of the member (top level, inside a map), the operation — the three the
//! recorded instances name and others they do not, on both the dispatcher-fronted and the
//! tagging surface — the submitted value's content and size, and the number of violations
//! in one request.

use serde_json::{json, Value};

use crate::bill_scenario_lifecycle::{error_code, Emulator};
use crate::validation_refusal_framing::{
    clause, envelope, error_message, pattern_requirement, ARN_PATTERN, NAME_PATTERN,
    PATTERN_ONLY_ARN, PATTERN_ONLY_RESOURCE_ID, RESOURCE_ID_PATTERN, TAG_VALUE_PATTERN,
};

/// The marker an entry's message states where the value it refused would go: a value the
/// request CARRIED is withheld behind it (rule 1).
pub(crate) const WITHHELD: &str = "[redacted]";

/// The subject an entry's message keeps for a member the request did not carry (rule 2).
pub(crate) const NULL: &str = "null";

/// The text that separates an entry's subject from the requirement it states.
const CLAUSE_MARKER: &str = " failed to satisfy constraint: ";

/// One `fieldList` entry's whole message: the subject — [`WITHHELD`] or [`NULL`] — and the
/// requirement, with no member path anywhere in it.
pub(crate) fn field_message(subject: &str, requirement: &str) -> String {
    format!("Value {subject}{CLAUSE_MARKER}{requirement}")
}

/// One entry a row expects: the member path it names, whether that member was ABSENT from
/// the request, and the requirement it states.
struct Field {
    name: &'static str,
    absent: bool,
    requirement: String,
}

impl Field {
    /// A member the request carried a value for.
    fn present(name: &'static str, requirement: String) -> Self {
        Self {
            name,
            absent: false,
            requirement,
        }
    }

    /// A member the request did not carry at all.
    fn absent(name: &'static str, requirement: String) -> Self {
        Self {
            name,
            absent: true,
            requirement,
        }
    }

    /// The whole message this entry must carry (rules 1 and 2).
    fn expected_message(&self) -> String {
        let subject = if self.absent { NULL } else { WITHHELD };
        field_message(subject, &self.requirement)
    }

    /// The clause the ENVELOPE must state for the same violation (rule 3): the member path
    /// and the same requirement.
    fn expected_clause(&self) -> String {
        let subject = if self.absent { "Value null" } else { "Value" };
        clause(subject, self.name, &self.requirement)
    }
}

/// One row: a request the model refuses, and every entry its `fieldList` must carry, in
/// the order the validating walk finds them (schema member order).
struct Case {
    case: &'static str,
    operation: &'static str,
    request: Value,
    fields: Vec<Field>,
}

/// A `BatchCreateWorkloadEstimateUsageEntry` every member of which satisfies the model, so
/// nothing but the member a row is about can be refused (smithy_query:
/// `shape="BatchCreateWorkloadEstimateUsageEntry"`).
fn usage_entry() -> Value {
    json!({
        "key": "k0",
        "serviceCode": "AmazonEC2",
        "usageType": "BoxUsage:m5.xlarge",
        "operation": "RunInstances",
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
        // --- the three operations the recorded instances name --------------------
        Case {
            case: "a @pattern on a required name",
            operation: "CreateWorkloadEstimate",
            request: json!({ "name": "bad name!" }),
            fields: vec![Field::present("name", pattern_requirement(NAME_PATTERN))],
        },
        Case {
            case: "a @pattern on the parent id of a batch create",
            operation: "BatchCreateWorkloadEstimateUsage",
            request: json!({
                "workloadEstimateId": PATTERN_ONLY_RESOURCE_ID,
                "usage": [usage_entry()],
            }),
            fields: vec![Field::present(
                "workloadEstimateId",
                pattern_requirement(RESOURCE_ID_PATTERN),
            )],
        },
        Case {
            case: "a @pattern on the identifier of an update",
            operation: "UpdateWorkloadEstimate",
            request: json!({ "identifier": PATTERN_ONLY_RESOURCE_ID }),
            fields: vec![Field::present(
                "identifier",
                pattern_requirement(RESOURCE_ID_PATTERN),
            )],
        },
        // --- operations and surfaces they do not name ----------------------------
        Case {
            case: "a @pattern on an ARN, on the tagging surface",
            operation: "ListTagsForResource",
            request: json!({ "arn": PATTERN_ONLY_ARN }),
            fields: vec![Field::present("arn", pattern_requirement(ARN_PATTERN))],
        },
        Case {
            case: "a @pattern on an operation of the account-class-gated surface",
            operation: "CreateBillScenario",
            request: json!({ "name": "bad name!" }),
            fields: vec![Field::present("name", pattern_requirement(NAME_PATTERN))],
        },
        Case {
            case: "a member nested in a map, whose path the entry's name carries",
            operation: "TagResource",
            request: json!({ "arn": valid_arn(), "tags": { "team": "finops!" } }),
            fields: vec![Field::present(
                "tags.team",
                pattern_requirement(TAG_VALUE_PATTERN),
            )],
        },
        // --- the other constraint classes ----------------------------------------
        Case {
            case: "a @length maximum",
            operation: "CreateWorkloadEstimate",
            request: json!({ "name": over_long_name }),
            fields: vec![Field::present(
                "name",
                "Member must have length less than or equal to 64".to_string(),
            )],
        },
        Case {
            case: "a @length minimum, with the pattern violation beside it",
            operation: "GetWorkloadEstimate",
            request: json!({ "identifier": "0f9e8d7c" }),
            fields: vec![
                Field::present(
                    "identifier",
                    "Member must have length greater than or equal to 36".to_string(),
                ),
                Field::present("identifier", pattern_requirement(RESOURCE_ID_PATTERN)),
            ],
        },
        Case {
            case: "a @range maximum",
            operation: "ListBillScenarios",
            request: json!({ "maxResults": 26 }),
            fields: vec![Field::present(
                "maxResults",
                "Member must have value less than or equal to 25".to_string(),
            )],
        },
        Case {
            case: "a @range minimum",
            operation: "ListWorkloadEstimates",
            request: json!({ "maxResults": 0 }),
            fields: vec![Field::present(
                "maxResults",
                "Member must have value greater than or equal to 1".to_string(),
            )],
        },
        // --- absence: the one subject the two halves share -----------------------
        Case {
            case: "an absent @required member",
            operation: "CreateWorkloadEstimate",
            request: json!({}),
            fields: vec![Field::absent(
                "name",
                "Member must not be null".to_string(),
            )],
        },
        Case {
            case: "an absent @required identifier of another operation",
            operation: "GetWorkloadEstimate",
            request: json!({}),
            fields: vec![Field::absent(
                "identifier",
                "Member must not be null".to_string(),
            )],
        },
        // --- several violations in one request -----------------------------------
        Case {
            case: "two violations of one member, one entry each",
            operation: "CreateWorkloadEstimate",
            request: json!({ "name": over_long_name_with_space }),
            fields: vec![
                Field::present(
                    "name",
                    "Member must have length less than or equal to 64".to_string(),
                ),
                Field::present("name", pattern_requirement(NAME_PATTERN)),
            ],
        },
        Case {
            case: "two members at fault, one entry each",
            operation: "BatchCreateWorkloadEstimateUsage",
            request: json!({ "workloadEstimateId": PATTERN_ONLY_RESOURCE_ID, "usage": [] }),
            fields: vec![
                Field::present(
                    "workloadEstimateId",
                    pattern_requirement(RESOURCE_ID_PATTERN),
                ),
                Field::present(
                    "usage",
                    "Member must have length greater than or equal to 1".to_string(),
                ),
            ],
        },
    ]
}

/// The `fieldList` of `body` as `(name, message)` in the order the body carries them.
///
/// `pub(crate)` so the suite that pins the structured half beside this one
/// (`validation_refusal_detail.rs`) reads a refusal's entries exactly the same way.
pub(crate) fn field_list(body: &Value) -> Vec<(String, String)> {
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

/// RULES 1, 2 and 4: every entry states its requirement about the value it refused — the
/// withheld marker, or the `null` an absent member was — and names no member path.
#[tokio::test]
async fn a_field_entry_states_its_requirement_about_the_value_it_refused() {
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

        let expected: Vec<(String, String)> = case
            .fields
            .iter()
            .map(|field| (field.name.to_string(), field.expected_message()))
            .collect();
        let actual = field_list(&body);
        assert_eq!(
            actual, expected,
            "case {:?} ({}): expected fieldList {expected:?}, got {actual:?} in {body}",
            case.case, case.operation
        );
        for (name, message) in &actual {
            assert!(
                !message.contains("at '"),
                "case {:?} ({}): entry {name:?} names a member path in its message: \
                 {message:?}",
                case.case,
                case.operation
            );
        }
    }
}

/// RULE 3, over the same table: the envelope states the SAME requirement for each
/// violation, under its own member-naming subject — so the prose half and the structured
/// half of one body cannot word a constraint two ways, and neither can state a violation
/// the other does not.
#[tokio::test]
async fn both_halves_of_the_body_state_the_same_requirements() {
    let emulator = Emulator::new();
    for case in cases() {
        let (_, body) = emulator.call(case.operation, case.request.clone()).await;
        let clauses: Vec<String> = case
            .fields
            .iter()
            .map(Field::expected_clause)
            .collect::<Vec<_>>();
        let message = error_message(&body);
        assert_eq!(
            message,
            envelope(&clauses),
            "case {:?} ({}): expected the envelope over {clauses:?}, got {message:?}",
            case.case,
            case.operation
        );
        // The requirement each entry states, read off the entry itself, is the one the
        // envelope's clause for that violation carries.
        for (index, (name, entry_message)) in field_list(&body).iter().enumerate() {
            let requirement = entry_message
                .rsplit(CLAUSE_MARKER)
                .next()
                .unwrap_or_default();
            let stated = clauses.get(index).map(String::as_str).unwrap_or_default();
            assert!(
                stated.ends_with(requirement),
                "case {:?} ({}): entry {name:?} states {requirement:?}, which the \
                 envelope clause {stated:?} does not",
                case.case,
                case.operation
            );
        }
    }
}

/// RULE 1 as an INVARIANT over the submitted value: the entry's message is the same bytes
/// whatever the caller sent, so nothing of the value — not the value, not a prefix of it,
/// not the framework's `... [<n> bytes]` elision marker — reaches the structured half of
/// the body, and the half is never sized by the request.
#[tokio::test]
async fn the_entry_message_does_not_depend_on_the_submitted_value() {
    // Each value is refused by `WorkloadEstimateName`'s `@pattern` or `@length`, and each
    // is a different hazard: punctuation, a quote, a newline, a non-ASCII grapheme, a
    // value far over the framework's echo cap, one that looks like that cap's elision
    // marker, and one that IS the withheld marker — which must still be stated as the
    // marker rather than as something read off the request.
    let pattern = pattern_requirement(NAME_PATTERN);
    let too_long = "Member must have length less than or equal to 64".to_string();
    let long = "z".repeat(4096);
    let long_and_outside = format!("{long} !");
    let rows: Vec<(&str, Vec<&String>)> = vec![
        ("bad name!", vec![&pattern]),
        ("it's bad", vec![&pattern]),
        ("two\nlines", vec![&pattern]),
        ("estimación", vec![&pattern]),
        ("a b", vec![&pattern]),
        ("zzz ... [7 bytes]", vec![&pattern]),
        (WITHHELD, vec![&pattern]),
        (long.as_str(), vec![&too_long]),
        (long_and_outside.as_str(), vec![&too_long, &pattern]),
    ];

    let emulator = Emulator::new();
    for (value, requirements) in rows {
        let (status, body) = emulator
            .call("CreateWorkloadEstimate", json!({ "name": value }))
            .await;
        assert_eq!(
            status, 400,
            "case {value:?}: expected 400, got {status} with {body}"
        );
        let expected: Vec<(String, String)> = requirements
            .iter()
            .map(|requirement| ("name".to_string(), field_message(WITHHELD, requirement)))
            .collect();
        let entries = field_list(&body);
        assert_eq!(
            entries, expected,
            "case {value:?}: expected fieldList {expected:?}, got {entries:?}"
        );
        for (_, stated) in &entries {
            let prefix: String = value.chars().take(8).collect();
            assert!(
                value == WITHHELD || !stated.contains(&prefix),
                "case {value:?}: a prefix of the submitted value reaches {stated:?}"
            );
            assert!(
                !stated.contains("... ["),
                "case {value:?}: the framework's elision marker reaches {stated:?}"
            );
            assert!(
                stated.len() < 512,
                "case {value:?}: the entry message is request-sized ({} bytes): {stated:?}",
                stated.len()
            );
        }
    }
}
