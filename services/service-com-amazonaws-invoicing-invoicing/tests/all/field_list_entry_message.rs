// SPDX-License-Identifier: CC0-1.0

//! What a `ValidationExceptionField` entry's own `message` says, driven end to end over
//! this service's wire protocol.
//!
//! Every call here goes through the same `MultiServiceRouter` the production router uses:
//! `POST /` with `X-Amz-Target: Invoicing.<OperationName>` and a JSON body, so what these
//! tests see is exactly the error document an SDK client reads.
//!
//! The rule pinned here: an entry of a `ValidationException`'s `fieldList` identifies the
//! member it refused in its own `name` member
//! (`smithy_query(query="shape", shape="ValidationExceptionField")`:
//! `{name required, message required}`), so its `message` identifies no member and states
//! only the VALUE it refused and the requirement that value failed — `Value [redacted]
//! failed to satisfy constraint: <requirement>`, with the withheld value's marker where
//! the value would stand, or the `null` an absent member was. The envelope `message`, the
//! other half of the same body, is prose about the whole request with no `name` beside
//! it, so it keeps naming the member path; both halves state the SAME requirement for the
//! same violation.
//!
//! The rows differ along what the rule generalises over rather than along the one request
//! a recording happened to carry:
//!
//! * the CONSTRAINT class the model declares: a `@pattern`, both `@length` bounds on a
//!   string, a `@length` minimum on a list, both `@range` bounds, an enum value set, and
//!   a `@required` member absent;
//! * the member's NESTING: a member at the top level of the input, a member of a
//!   structure member, a list element, and a member inside a list element;
//! * the FORM and size of the rejected value, including values that differ only in what
//!   the rule generalises over — two values outside one pattern earn the same entry;
//! * how MANY members one request spoils at once, and one member failing TWO constraints;
//! * which OPERATION and which plane refuses it: the invoice-unit, invoice-profile,
//!   tag and procurement-portal planes alike, since the modelled refusal body belongs to
//!   the refusal rather than to the plane.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use service_com_amazonaws_invoicing_invoicing::ops::types::{
    LIST_INVOICE_SUMMARIES_RESOURCE_TYPE_VALUES, PROCUREMENT_PORTAL_PREFERENCE_STATUS_VALUES,
};

/// The marker a `fieldList` entry's message states where the value it refused would
/// stand: the value is withheld, and this says so.
const REDACTED: &str = "[redacted]";

/// A router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signing is the router's concern, not this slice's: drive the operations unsigned.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_invoicing_invoicing::EmulatedService::new(
        engine,
    ));
    router
}

/// What one call answered.
#[derive(Debug)]
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type` on the awsJson protocols), shape name only.
    fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// A string member of the body.
    fn member(&self, name: &str) -> String {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The `fieldList` entries, as `(name, message)` pairs in the order they are stated.
    fn entries(&self) -> Vec<(String, String)> {
        self.body
            .get("fieldList")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| {
                        (
                            entry
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            entry
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// One request, returning everything a client can see of the refusal.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("x-amz-target", format!("Invoicing.{operation}"))
        .header("content-type", "application/x-amz-json-1.0")
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    Answer { status, body }
}

/// One member a row spoils: the path the entry must name it by, the requirement its
/// message must state, and whether the request carried a value for it at all.
#[derive(Debug, Clone)]
struct Refused {
    path: &'static str,
    requirement: String,
    /// `false` for a `@required` member the request left out, which had no value to
    /// withhold.
    carried_a_value: bool,
}

impl Refused {
    fn present(path: &'static str, requirement: impl Into<String>) -> Self {
        Self {
            path,
            requirement: requirement.into(),
            carried_a_value: true,
        }
    }

    fn absent(path: &'static str) -> Self {
        Self {
            path,
            // The requirement an absent `@required` member failed.
            requirement: "Member must not be null".to_string(),
            carried_a_value: false,
        }
    }

    /// The `message` the entry for this member must state: the value it refused and the
    /// requirement, and no member path.
    fn entry_message(&self) -> String {
        let value = if self.carried_a_value { REDACTED } else { "null" };
        format!(
            "Value {value} failed to satisfy constraint: {}",
            self.requirement
        )
    }

    /// The clause the ENVELOPE states for this member: the same requirement, under the
    /// member path, which is how prose with no `name` beside it identifies a member.
    fn clause(&self) -> String {
        let subject = if self.carried_a_value {
            "Value"
        } else {
            "Value null"
        };
        format!(
            "{subject} at '{}' failed to satisfy constraint: {}",
            self.path, self.requirement
        )
    }
}

/// `Member must satisfy regular expression pattern: <expression>`, the expression as the
/// service advertises it.
fn pattern(expression: &str) -> String {
    format!("Member must satisfy regular expression pattern: {expression}")
}

/// `Member must satisfy enum value set: [<values>]`, over the value set an enum shape
/// declares. The ORDER the set is enumerated in belongs to the model's own member order
/// rather than to this rule, so a message carrying one is compared through
/// [`with_sorted_sets`].
fn enum_set(declared: &[&str]) -> String {
    format!("Member must satisfy enum value set: [{}]", declared.join(", "))
}

/// `message` with any enumerated value set it carries sorted, so two messages are
/// compared on the SET a requirement enumerates rather than on the order it lists it in.
fn with_sorted_sets(message: &str) -> String {
    let Some((head, set)) = message.split_once(": [") else {
        return message.to_string();
    };
    let Some(set) = set.strip_suffix(']') else {
        return message.to_string();
    };
    let mut values: Vec<&str> = set.split(", ").collect();
    values.sort_unstable();
    format!("{head}: [{}]", values.join(", "))
}

/// The expression the service advertises for `AccountIdString`'s `@pattern`
/// (`smithy_query(query="shape", shape="AccountIdString")`: `^\d{12}$`, without the
/// anchors a model spells it with).
const ACCOUNT_ID_EXPRESSION: &str = r"\d{12}";

/// The expression the service advertises for `TagrisArn`'s `@pattern`
/// (`smithy_query(query="operation", operation="ListTagsForResource")`).
const TAGRIS_ARN_EXPRESSION: &str = r"arn:aws[-a-z0-9]*:(invoicing)::[0-9]{12}:[-a-zA-Z0-9/:_]+";

/// A well-formed `TagrisArn`, so a tag request is refused for its tags rather than for
/// the resource it names.
const RESOURCE_ARN: &str = "arn:aws:invoicing::000000000000:invoice-unit/ffffffffffff";

/// A well-formed `ProcurementPortalPreferenceArnString`, so a status request is refused
/// for its status rather than for the ARN.
const PREFERENCE_ARN: &str =
    "arn:aws:invoicing::000000000000:procurement-portal-preference/NoSuchPreference";

/// A `CreateInvoiceUnit` body with every `@required` member, which a row then spoils.
fn create_request() -> Value {
    json!({
        "Name": "EntryMessageUnit",
        "InvoiceReceiver": "123456789012",
        "TaxInheritanceDisabled": false,
        "Rule": {"LinkedAccounts": ["210987654321"]},
    })
}

/// One refused request and the members its `fieldList` must state.
struct Case {
    operation: &'static str,
    /// What makes this row its own case, for the assertion messages.
    label: String,
    request: Value,
    refused: Vec<Refused>,
    /// Caller-sent values that must appear NOWHERE in the refusal document.
    rejected: Vec<String>,
}

impl std::fmt::Debug for Case {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.operation, self.label)
    }
}

impl Case {
    fn new(operation: &'static str, label: &str, request: Value, refused: Vec<Refused>) -> Self {
        Self {
            operation,
            label: label.to_string(),
            request,
            refused,
            rejected: Vec::new(),
        }
    }

    fn rejecting(mut self, value: impl Into<String>) -> Self {
        self.rejected.push(value.into());
        self
    }
}

/// Every row of the table.
fn cases() -> Vec<Case> {
    let long_name = "n".repeat(51);
    let long_value = "v".repeat(300);
    vec![
        // `@pattern` on a top-level string member — the class the recorded refusal
        // carries — and a second value outside the same pattern: the entry does not vary
        // with the value it refused.
        Case::new(
            "CreateInvoiceUnit",
            "an InvoiceReceiver outside the account-id pattern",
            {
                let mut request = create_request();
                request["InvoiceReceiver"] = json!("not-an-account-id");
                request
            },
            vec![Refused::present(
                "invoiceReceiver",
                pattern(ACCOUNT_ID_EXPRESSION),
            )],
        )
        .rejecting("not-an-account-id"),
        Case::new(
            "CreateInvoiceUnit",
            "an InvoiceReceiver of the wrong digit count",
            {
                let mut request = create_request();
                request["InvoiceReceiver"] = json!("12345");
                request
            },
            vec![Refused::present(
                "invoiceReceiver",
                pattern(ACCOUNT_ID_EXPRESSION),
            )],
        )
        .rejecting("12345"),
        // `@length`'s maximum on a string member …
        Case::new(
            "CreateInvoiceUnit",
            "a Name past the length maximum",
            {
                let mut request = create_request();
                request["Name"] = json!(long_name.clone());
                request
            },
            vec![Refused::present(
                "name",
                "Member must have length less than or equal to 50",
            )],
        )
        .rejecting(long_name.clone()),
        // … and its minimum.
        Case::new(
            "CreateInvoiceUnit",
            "a Name below the length minimum",
            {
                let mut request = create_request();
                request["Name"] = json!("");
                request
            },
            vec![Refused::present(
                "name",
                "Member must have length greater than or equal to 1",
            )],
        ),
        // A `@required` member the request left out: the one entry that states a `null`,
        // because there was no value to withhold.
        Case::new(
            "CreateInvoiceUnit",
            "no Name at all",
            json!({
                "InvoiceReceiver": "123456789012",
                "TaxInheritanceDisabled": false,
                "Rule": {"LinkedAccounts": ["210987654321"]},
            }),
            vec![Refused::absent("name")],
        ),
        // A LIST ELEMENT inside a structure member, and a member INSIDE a list element:
        // the entry's `name` reaches it, its message still names nothing.
        Case::new(
            "CreateInvoiceUnit",
            "a linked account outside the account-id pattern",
            {
                let mut request = create_request();
                request["Rule"] = json!({"LinkedAccounts": ["210987654321", "nope"]});
                request
            },
            vec![Refused::present(
                "rule.linkedAccounts.2.member",
                pattern(ACCOUNT_ID_EXPRESSION),
            )],
        )
        .rejecting("nope"),
        Case::new(
            "CreateInvoiceUnit",
            "a resource tag with an over-long value",
            {
                let mut request = create_request();
                request["ResourceTags"] = json!([{"Key": "k", "Value": long_value.clone()}]);
                request
            },
            vec![Refused::present(
                "resourceTags.1.member.value",
                "Member must have length less than or equal to 256",
            )],
        )
        .rejecting(long_value),
        // TWO spoiled members in one request: one entry each, in the walk's order.
        Case::new(
            "CreateInvoiceUnit",
            "a bad receiver and an over-long name together",
            {
                let mut request = create_request();
                request["InvoiceReceiver"] = json!("not-an-account-id");
                request["Name"] = json!(long_name.clone());
                request
            },
            vec![
                Refused::present("name", "Member must have length less than or equal to 50"),
                Refused::present("invoiceReceiver", pattern(ACCOUNT_ID_EXPRESSION)),
            ],
        )
        .rejecting("not-an-account-id")
        .rejecting(long_name),
        // `@range` on an integer member, at both ends of the published range.
        Case::new(
            "ListInvoiceUnits",
            "MaxResults below the range minimum",
            json!({"MaxResults": 0}),
            vec![Refused::present(
                "maxResults",
                "Member must have value greater than or equal to 1",
            )],
        ),
        Case::new(
            "ListInvoiceUnits",
            "MaxResults past the range maximum",
            json!({"MaxResults": 501}),
            vec![Refused::present(
                "maxResults",
                "Member must have value less than or equal to 500",
            )],
        ),
        // ONE member failing TWO constraints, on the tag plane: an entry per requirement,
        // both naming the same member in `name`.
        Case::new(
            "ListTagsForResource",
            "a ResourceArn too short and outside the pattern",
            json!({"ResourceArn": "arn:aws:s3:::b"}),
            vec![
                Refused::present(
                    "resourceArn",
                    "Member must have length greater than or equal to 20",
                ),
                Refused::present("resourceArn", pattern(TAGRIS_ARN_EXPRESSION)),
            ],
        )
        .rejecting("arn:aws:s3:::b"),
        // A `@required` LIST member absent, on the tag plane.
        Case::new(
            "UntagResource",
            "no ResourceTagKeys at all",
            json!({"ResourceArn": RESOURCE_ARN}),
            vec![Refused::absent("resourceTagKeys")],
        ),
        // A `@length` minimum on a LIST member, on the invoice-profile plane …
        Case::new(
            "BatchGetInvoiceProfile",
            "an empty AccountIds list",
            json!({"AccountIds": []}),
            vec![Refused::present(
                "accountIds",
                "Member must have length greater than or equal to 1",
            )],
        ),
        // … and a list ELEMENT at the top level of the input.
        Case::new(
            "BatchGetInvoiceProfile",
            "an account id outside the account-id pattern",
            json!({"AccountIds": ["not-an-account"]}),
            vec![Refused::present(
                "accountIds.1.member",
                pattern(ACCOUNT_ID_EXPRESSION),
            )],
        )
        .rejecting("not-an-account"),
        // An ENUM value set, on a member of a structure member …
        Case::new(
            "ListInvoiceSummaries",
            "a ResourceType outside the enum value set",
            json!({"Selector": {"ResourceType": "account_id", "Value": "111111111111"}}),
            vec![Refused::present(
                "selector.resourceType",
                enum_set(LIST_INVOICE_SUMMARIES_RESOURCE_TYPE_VALUES),
            )],
        )
        .rejecting("account_id"),
        // … and on a top-level member of another plane, whose set is a different one.
        Case::new(
            "UpdateProcurementPortalPreferenceStatus",
            "an EinvoiceDeliveryPreferenceStatus outside the enum value set",
            json!({
                "ProcurementPortalPreferenceArn": PREFERENCE_ARN,
                "EinvoiceDeliveryPreferenceStatus": "NOT_A_STATUS",
            }),
            vec![Refused::present(
                "einvoiceDeliveryPreferenceStatus",
                enum_set(PROCUREMENT_PORTAL_PREFERENCE_STATUS_VALUES),
            )],
        )
        .rejecting("NOT_A_STATUS"),
        // `@range`'s maximum on the procurement-portal plane's own `MaxResults`, a
        // different bound from the invoice-unit plane's.
        Case::new(
            "ListProcurementPortalPreferences",
            "MaxResults past the range maximum",
            json!({"MaxResults": 101}),
            vec![Refused::present(
                "maxResults",
                "Member must have value less than or equal to 100",
            )],
        ),
    ]
}

/// RULE: every `fieldList` entry names its member in `name` and states, in `message`, the
/// value it refused — withheld behind the marker, or the `null` an absent member was —
/// and the requirement that value failed, with no member path and nothing of the value;
/// the envelope states the same requirement under the member path.
#[tokio::test]
async fn a_field_list_entry_states_the_withheld_value_and_the_requirement() {
    let router = emulator();

    for case in cases() {
        let answer = call(&router, case.operation, &case.request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {answer:?}"
        );

        // One entry per refused member, each naming its member and stating the value and
        // the requirement.
        let entries = answer.entries();
        let stated: Vec<(String, String)> = entries
            .iter()
            .map(|(name, message)| (name.clone(), with_sorted_sets(message)))
            .collect();
        let wanted: Vec<(String, String)> = case
            .refused
            .iter()
            .map(|refused| {
                (
                    refused.path.to_string(),
                    with_sorted_sets(&refused.entry_message()),
                )
            })
            .collect();
        assert_eq!(
            stated, wanted,
            "case {case:?}: expected the fieldList {wanted:?}, got {answer:?}"
        );

        for (name, message) in &entries {
            // The message identifies no member: the entry's `name` does that, and a
            // quoted path is how the envelope's prose identifies one.
            assert!(
                !message.contains('\'') && !message.contains(name.as_str()),
                "case {case:?}: an entry's message identifies no member, got {message:?}"
            );
            assert!(
                message.starts_with("Value [redacted] failed to satisfy constraint: ")
                    || message.starts_with("Value null failed to satisfy constraint: "),
                "case {case:?}: an entry's message states the value it refused, got \
                 {message:?}"
            );
        }

        // The envelope, the other half of the same body, states the same requirements
        // under the member paths.
        let message = with_sorted_sets(&answer.member("message"));
        for refused in &case.refused {
            let clause = with_sorted_sets(&refused.clause());
            assert!(
                message.contains(clause.as_str()),
                "case {case:?}: expected the envelope to state {clause:?}, got {message:?}"
            );
        }

        // Nothing of the rejected value anywhere in the document, in either half.
        let document = answer.body.to_string();
        for value in &case.rejected {
            assert!(
                !document.contains(value.as_str()),
                "case {case:?}: the refusal must not reproduce the rejected value \
                 {value:?}, got {document}"
            );
        }
    }
}

/// The marker stands for a value that was WITHHELD, not for one that was absent: the
/// entry for a member the request left out states the `null` it was, and the entry for
/// the SAME member carrying a value the service refuses states the marker instead —
/// whatever the member, the operation and the value.
///
/// The rows are each member driven both ways, so the two forms are read off the same
/// member rather than off two members that might be worded by two rules.
#[tokio::test]
async fn an_absent_member_is_stated_null_and_a_carried_one_redacted() {
    let router = emulator();

    // (operation, the member, a request leaving it out, a request carrying a value it
    // refuses) — absence is found while READING the request, so it is the only violation
    // such a request earns and the two forms need a request each.
    let cases: Vec<(&str, &str, Value, Value)> = vec![
        (
            "CreateInvoiceUnit",
            "name",
            json!({
                "InvoiceReceiver": "123456789012",
                "TaxInheritanceDisabled": false,
                "Rule": {"LinkedAccounts": ["210987654321"]},
            }),
            {
                let mut request = create_request();
                request["Name"] = json!("");
                request
            },
        ),
        (
            "UntagResource",
            "resourceTagKeys",
            json!({"ResourceArn": RESOURCE_ARN}),
            // `ResourceTagKeyList` is `@length(0..200)` (`smithy_query(query="shape",
            // shape="ResourceTagKeyList")`), so 201 keys is a value the list member
            // itself refuses.
            json!({
                "ResourceArn": RESOURCE_ARN,
                "ResourceTagKeys": Value::Array((0..201).map(|index| json!(format!("k{index}"))).collect()),
            }),
        ),
        (
            "BatchGetInvoiceProfile",
            "accountIds",
            json!({}),
            json!({"AccountIds": []}),
        ),
    ];

    for (operation, member, left_out, carried) in cases {
        // (the request, the form its entry for `member` must take, what it must not say)
        for (request, expected, forbidden) in [
            (left_out, "Value null failed to satisfy constraint: ", REDACTED),
            (
                carried,
                "Value [redacted] failed to satisfy constraint: ",
                " null ",
            ),
        ] {
            let label = format!("{operation} {member} stated as {expected:?}");
            let answer = call(&router, operation, &request).await;
            let entries = answer.entries();
            let entry = entries
                .iter()
                .find(|(name, _)| name == member)
                .unwrap_or_else(|| {
                    panic!("case {label}: expected an entry for {member:?}, got {answer:?}")
                });
            assert!(
                entry.1.starts_with(expected) && !entry.1.contains(forbidden),
                "case {label}: expected an entry message opening {expected:?} and not \
                 carrying {forbidden:?}, got {:?}",
                entry.1
            );
        }
    }
}
