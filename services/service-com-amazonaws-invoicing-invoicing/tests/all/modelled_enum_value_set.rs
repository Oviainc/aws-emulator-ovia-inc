// SPDX-License-Identifier: CC0-1.0

//! The ONE place this service holds a member to the value set of the enum shape it
//! targets: the walk over every modelled input constraint it runs in front of every
//! operation, driven here end to end over the wire.
//!
//! Every call goes through the same `MultiServiceRouter` the production router uses:
//! `POST /` with `X-Amz-Target: Invoicing.<OperationName>` and a JSON body, so what
//! these tests see is the document an SDK client reads.
//!
//! The rule under test: a member that targets an enum shape admits only the values that
//! shape declares, and a request carrying any other value is refused with the modelled
//! 400 `ValidationException` whose clause names that member's path and enumerates that
//! same value set — while a value the shape DOES declare always gets past the walk.
//!
//! The rows span what the rule generalises over rather than one request:
//!
//! * the member's NESTING: a member of a structure member (`ListInvoiceSummaries`'
//!   `Selector`/`Filter`) and a member at the top level of the input
//!   (`UpdateProcurementPortalPreferenceStatus`' two plane statuses);
//! * the enum SHAPE, hence the value set the clause enumerates: three different shapes,
//!   of two, three and seven values;
//! * the FORM of the rejected value: a declared value in the wrong case, a value of a
//!   different enum of this same service, a value with surrounding space, the empty
//!   string, and one far longer than any declared value;
//! * how MANY such members one request spoils at once: one, and two together.
//!
//! The operations are deliberately drawn one from each place this rule used to be
//! applied — the handler that kept its own copy of two of these value sets, and a
//! sibling that never had one — so both are shown answering through the one walk.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use service_com_amazonaws_invoicing_invoicing::ops::types::{
    LIST_INVOICE_SUMMARIES_RESOURCE_TYPE_VALUES, PROCUREMENT_PORTAL_PREFERENCE_STATUS_VALUES,
    RECEIVER_ROLE_VALUES,
};

/// The `ValidationExceptionReason` a rejected member value is stated with.
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// A well-formed `ProcurementPortalPreferenceArnString` — it satisfies the member's
/// `@pattern` (`smithy_query(query="operation",
/// operation="UpdateProcurementPortalPreferenceStatus")`) — that names no stored
/// preference, so a request whose statuses the walk admits gets past it and is answered
/// about the resource instead.
const WELL_FORMED_PREFERENCE_ARN: &str =
    "arn:aws:invoicing::000000000000:procurement-portal-preference/NoSuchPreference";

/// An account id for the `Selector` of a `ListInvoiceSummaries` row: it satisfies
/// `StringWithoutNewLine`, so nothing but the enum member is in question.
const ACCOUNT_ID: &str = "111111111111";

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

/// One request, returning the HTTP status and the parsed body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
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
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code of a failure body (`__type` on the awsJson protocols), shape name
/// only.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A string member of a body.
fn member(body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The marker an enum-value-set requirement opens with: a refusal for a value outside
/// the set states the member path, then this requirement enumerating the whole set.
const ENUM_REQUIREMENT: &str = "Member must satisfy enum value set: [";

/// The clauses one envelope states, with the envelope's own count checked against the
/// number of clauses the row expects.
fn clauses(message: &str, expected: usize, case: &str) -> Vec<String> {
    let prefix = if expected == 1 {
        "1 validation error detected: ".to_string()
    } else {
        format!("{expected} validation errors detected: ")
    };
    let rest = message.strip_prefix(prefix.as_str()).unwrap_or_else(|| {
        panic!("case {case}: expected an envelope over {expected} clause(s), got {message:?}")
    });
    let stated: Vec<String> = rest.split("; ").map(str::to_string).collect();
    assert_eq!(
        stated.len(),
        expected,
        "case {case}: the envelope states one clause per refused member, got {message:?}"
    );
    stated
}

/// The clause `clauses` states for the member at `path`.
fn clause_for(clauses: &[String], path: &str, case: &str) -> String {
    let opening = format!("Value at '{path}' failed to satisfy constraint: ");
    clauses
        .iter()
        .find(|clause| clause.starts_with(opening.as_str()))
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: expected a clause for '{path}', got {clauses:?}"))
}

/// The value set an enum clause enumerates, sorted for comparison against the shape's
/// own declared set.
fn enumerated_set(clause: &str, path: &str, case: &str) -> Vec<String> {
    let opening = format!("Value at '{path}' failed to satisfy constraint: {ENUM_REQUIREMENT}");
    let set = clause
        .strip_prefix(opening.as_str())
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or_else(|| {
            panic!("case {case}: expected an enum value-set clause for '{path}', got {clause:?}")
        });
    let mut values: Vec<String> = set.split(", ").map(str::to_string).collect();
    values.sort();
    values
}

/// The message the `fieldList` entry for `path` must state, built from the requirement
/// the envelope's own clause for that member states: the value the entry refused —
/// withheld behind the `[redacted]` marker — and that same requirement, with the member
/// left to the entry's own `name`.
fn field_message(clause: &str, path: &str, case: &str) -> String {
    let opening = format!("Value at '{path}' failed to satisfy constraint: ");
    let requirement = clause.strip_prefix(opening.as_str()).unwrap_or_else(|| {
        panic!("case {case}: expected a clause for '{path}', got {clause:?}")
    });
    format!("Value [redacted] failed to satisfy constraint: {requirement}")
}

/// The value set a row expects the clause to enumerate, in the order a comparison uses.
fn sorted(values: &[&str]) -> Vec<String> {
    let mut sorted: Vec<String> = values.iter().map(|value| (*value).to_string()).collect();
    sorted.sort();
    sorted
}

/// One member a row spoils: the dotted path the refusal must name it by, and the value
/// set the shape it targets declares.
struct Spoiled {
    path: &'static str,
    declared: &'static [&'static str],
}

/// One row: a request carrying a value no enum of its spoiled members declares.
struct Case {
    operation: &'static str,
    /// What makes this row its own case, for the assertion messages.
    label: String,
    request: Value,
    spoiled: Vec<Spoiled>,
    /// The values the caller sent that the refusal must reproduce nowhere.
    rejected: Vec<String>,
}

impl std::fmt::Debug for Case {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.operation, self.label)
    }
}

/// A `ListInvoiceSummaries` body selecting one account, which a row then spoils.
fn summaries_request(resource_type: &str) -> Value {
    json!({"Selector": {"ResourceType": resource_type, "Value": ACCOUNT_ID}})
}

/// An `UpdateProcurementPortalPreferenceStatus` body naming a well-formed ARN, which a
/// row then spoils with a status.
fn status_request(status_member: &str, status: &str) -> Value {
    json!({
        "ProcurementPortalPreferenceArn": WELL_FORMED_PREFERENCE_ARN,
        status_member: status,
    })
}

/// The members of the two operations whose enum values this table spoils, with the path
/// the walk reports each by.
const SELECTOR_RESOURCE_TYPE: Spoiled = Spoiled {
    path: "selector.resourceType",
    declared: LIST_INVOICE_SUMMARIES_RESOURCE_TYPE_VALUES,
};
const FILTER_RECEIVER_ROLE: Spoiled = Spoiled {
    path: "filter.receiverRole",
    declared: RECEIVER_ROLE_VALUES,
};

/// Every row of the table.
fn cases() -> Vec<Case> {
    // Values no enum of this service declares, each a different FORM of wrong.
    let overlong = "X".repeat(200);
    let mut cases = Vec::new();

    // A member of a STRUCTURE member: `Selector$ResourceType`.
    for value in [
        // A declared value in the wrong case.
        "account_id",
        "Invoice_Id",
        // A value of a different enum of this same service.
        "ARN",
        // Surrounding space around a declared value.
        " ACCOUNT_ID",
        "ACCOUNT_ID ",
        // The empty string.
        "",
        overlong.as_str(),
    ] {
        cases.push(Case {
            operation: "ListInvoiceSummaries",
            label: format!("a ResourceType of {value:?}"),
            request: summaries_request(value),
            spoiled: vec![SELECTOR_RESOURCE_TYPE],
            rejected: vec![value.to_string()],
        });
    }

    // Another member of a structure member, of a THREE-value enum: `Filter$ReceiverRole`.
    for value in ["buyer", "PAYER", "RESELLER ", "", overlong.as_str()] {
        let mut request = summaries_request("ACCOUNT_ID");
        request["Filter"] = json!({"ReceiverRole": value});
        cases.push(Case {
            operation: "ListInvoiceSummaries",
            label: format!("a ReceiverRole of {value:?}"),
            request,
            spoiled: vec![FILTER_RECEIVER_ROLE],
            rejected: vec![value.to_string()],
        });
    }

    // TWO enum members of one request, in two different structures: both are stated.
    let mut both = summaries_request("ARN");
    both["Filter"] = json!({"ReceiverRole": "payer"});
    cases.push(Case {
        operation: "ListInvoiceSummaries",
        label: "an unmodelled ResourceType and ReceiverRole together".to_string(),
        request: both,
        spoiled: vec![SELECTOR_RESOURCE_TYPE, FILTER_RECEIVER_ROLE],
        rejected: vec!["ARN".to_string(), "payer".to_string()],
    });

    // A member at the TOP LEVEL of the input, of a seven-value enum, on each of the two
    // planes that carry one.
    for status_member in [
        "EinvoiceDeliveryPreferenceStatus",
        "PurchaseOrderRetrievalPreferenceStatus",
    ] {
        for value in [
            "active",
            "Validated",
            "NOT_A_STATUS",
            // A value of a different enum of this same service.
            "COUPA",
            " ACTIVE",
            "",
            overlong.as_str(),
        ] {
            cases.push(Case {
                operation: "UpdateProcurementPortalPreferenceStatus",
                label: format!("{status_member} of {value:?}"),
                request: status_request(status_member, value),
                spoiled: vec![Spoiled {
                    // The walk reports a member by its lowerCamel name.
                    path: match status_member {
                        "EinvoiceDeliveryPreferenceStatus" => "einvoiceDeliveryPreferenceStatus",
                        _ => "purchaseOrderRetrievalPreferenceStatus",
                    },
                    declared: PROCUREMENT_PORTAL_PREFERENCE_STATUS_VALUES,
                }],
                rejected: vec![value.to_string()],
            });
        }
    }

    // BOTH top-level status members at once.
    cases.push(Case {
        operation: "UpdateProcurementPortalPreferenceStatus",
        label: "both plane statuses outside the enum".to_string(),
        request: json!({
            "ProcurementPortalPreferenceArn": WELL_FORMED_PREFERENCE_ARN,
            "EinvoiceDeliveryPreferenceStatus": "enabled",
            "PurchaseOrderRetrievalPreferenceStatus": "disabled",
        }),
        spoiled: vec![
            Spoiled {
                path: "einvoiceDeliveryPreferenceStatus",
                declared: PROCUREMENT_PORTAL_PREFERENCE_STATUS_VALUES,
            },
            Spoiled {
                path: "purchaseOrderRetrievalPreferenceStatus",
                declared: PROCUREMENT_PORTAL_PREFERENCE_STATUS_VALUES,
            },
        ],
        rejected: vec!["enabled".to_string(), "disabled".to_string()],
    });

    cases
}

/// RULE: a member targeting an enum shape admits only the values that shape declares, so
/// whatever operation it belongs to and however deeply it is nested, any other value is
/// refused with the modelled 400 `ValidationException` whose clause names the member's
/// path and enumerates that shape's whole value set — one clause per spoiled member,
/// counted by the AWS envelope, in both halves of the body, and with nothing of the
/// rejected value anywhere in the document.
#[tokio::test]
async fn a_member_targeting_an_enum_admits_only_its_declared_values() {
    let router = emulator();

    for case in cases() {
        let label = format!("{case:?}");
        let (status, body) = call(&router, case.operation, &case.request).await;
        assert_eq!(
            status, 400,
            "case {label}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {label}: expected the modelled validation error, got {body}"
        );
        assert_eq!(
            member(&body, "reason"),
            FIELD_VALIDATION_FAILED,
            "case {label}: expected the fieldValidationFailed reason, got {body}"
        );

        // The envelope counts the clauses it states, one per spoiled member.
        let expected_count = case.spoiled.len();
        let message = member(&body, "message");
        let stated = clauses(&message, expected_count, &label);

        let field_list = body
            .get("fieldList")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            field_list.len(),
            expected_count,
            "case {label}: expected {expected_count} fieldList entry/entries, got {body}"
        );

        for spoiled in &case.spoiled {
            let clause = clause_for(&stated, spoiled.path, &label);
            let reported = enumerated_set(&clause, spoiled.path, &label);
            assert_eq!(
                reported,
                sorted(spoiled.declared),
                "case {label}: the clause for '{}' must enumerate the shape's whole value \
                 set, got {clause:?}",
                spoiled.path
            );
            // Both halves of the body state the SAME requirement for that member: the
            // envelope names the member path, the entry names it in its own `name` and
            // states the requirement about the value it refused, withheld behind the
            // `[redacted]` marker.
            let entry = field_list
                .iter()
                .find(|entry| member(entry, "name") == spoiled.path)
                .unwrap_or_else(|| {
                    panic!(
                        "case {label}: expected a fieldList entry for '{}', got {body}",
                        spoiled.path
                    )
                });
            let entry_message = member(entry, "message");
            let expected = field_message(&clause, spoiled.path, &label);
            assert_eq!(
                entry_message, expected,
                "case {label}: expected the entry for '{}' to state {expected:?}, got \
                 {entry_message:?}",
                spoiled.path
            );
        }

        let document = body.to_string();
        for rejected in &case.rejected {
            // Only a value that cannot occur inside the enumerated set is asserted
            // absent: the empty string is no value to look for, and a value that
            // overlaps a declared one would be found in the set the clause legitimately
            // enumerates.
            if !assertable_absent(&case, rejected) {
                continue;
            }
            assert!(
                !document.contains(rejected.as_str()),
                "case {label}: the refusal must not reproduce the rejected value \
                 {rejected:?}, got {document}"
            );
        }
    }
}

/// Whether `rejected`'s absence from the refusal document can be asserted: it must be a
/// value that neither occurs inside, nor contains, any value this row's clauses
/// enumerate.
fn assertable_absent(case: &Case, rejected: &str) -> bool {
    if rejected.is_empty() {
        return false;
    }
    !case.spoiled.iter().any(|spoiled| {
        spoiled
            .declared
            .iter()
            .any(|declared| declared.contains(rejected) || rejected.contains(*declared))
    })
}

/// RULE (the other side of it): a value the enum DOES declare is never refused for the
/// value set, whatever operation or member carries it — the request gets past the walk
/// and is answered about the resource it names.
///
/// The rows are every value each of the three enum shapes declares, so the set the walk
/// enforces and the set the model declares are shown to be one set from both directions.
#[tokio::test]
async fn every_declared_value_gets_past_the_walk() {
    let router = emulator();

    // (operation, the request carrying the declared value, the label)
    let mut cases: Vec<(&str, Value, String)> = Vec::new();
    for resource_type in LIST_INVOICE_SUMMARIES_RESOURCE_TYPE_VALUES {
        cases.push((
            "ListInvoiceSummaries",
            summaries_request(resource_type),
            format!("a ResourceType of {resource_type:?}"),
        ));
    }
    for role in RECEIVER_ROLE_VALUES {
        let mut request = summaries_request("ACCOUNT_ID");
        request["Filter"] = json!({"ReceiverRole": role});
        cases.push((
            "ListInvoiceSummaries",
            request,
            format!("a ReceiverRole of {role:?}"),
        ));
    }
    for status in PROCUREMENT_PORTAL_PREFERENCE_STATUS_VALUES {
        for status_member in [
            "EinvoiceDeliveryPreferenceStatus",
            "PurchaseOrderRetrievalPreferenceStatus",
        ] {
            cases.push((
                "UpdateProcurementPortalPreferenceStatus",
                status_request(status_member, status),
                format!("{status_member} of {status:?}"),
            ));
        }
    }

    for (operation, request, label) in cases {
        let (status, body) = call(&router, operation, &request).await;
        let message = member(&body, "message");
        assert!(
            !message.contains(ENUM_REQUIREMENT),
            "case {operation} {label}: a declared value must not be refused for the value \
             set, got {status} {body}"
        );
    }
}
