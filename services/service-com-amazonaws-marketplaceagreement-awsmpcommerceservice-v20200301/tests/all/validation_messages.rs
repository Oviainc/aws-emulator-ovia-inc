// SPDX-License-Identifier: CC0-1.0

//! Two rules about a request this service will not serve, each over the CLASS of
//! such requests rather than one recorded example.
//!
//! * A refused input is answered 400 `ValidationException` whose `reason` names
//!   the offending member and whose `message` is the sentence that reason states
//!   ("Provided max results is invalid."), never the framework's constraint prose.
//!   `validation_envelope` pins that rule over the members of `DescribeAgreement`,
//!   `GetAgreementTerms`, `SearchAgreements`, `CreateAgreementRequest` and the
//!   absent members of `UpdatePurchaseOrders`; the rows here extend it to the
//!   operations and constraints it does not reach — the enum of
//!   `SendAgreementCancellationRequest`, the string length of
//!   `SendAgreementPaymentRequest`, the page size of
//!   `ListBillingAdjustmentRequests`, and a list and a list element's members on
//!   `UpdatePurchaseOrders` — so the rule is shown to hold whichever operation
//!   and whichever constraint refused the request.
//! * `Sort$sortBy` restricts two of its attributes to one party type, so the
//!   pairing of a sort attribute with the party type a search is scoped to is
//!   checked across the whole product of the two, not only the recorded pair.
//!
//! Every assertion goes over the router, so each case exercises routing,
//! deserialization, the validation seam, the handler and serialization — the
//! same path a client takes. The harness (`emulator`, `call`, `agreement_over`)
//! is the one `agreement_lifecycle` already owns.

use serde_json::{json, Value};

use crate::agreement_lifecycle::{agreement_over, call, emulator, term};

/// A valid `UpdatePurchaseOrders` entry: `chargeId` is its one required member
/// (`smithy_query(query="shape",shape="PurchaseOrder")`).
fn purchase_order(index: usize) -> Value {
    json!({"chargeId": format!("c-{index}")})
}

/// `count` valid purchase orders, so only the list's own length is at fault.
fn purchase_orders(count: usize) -> Value {
    Value::Array((0..count).map(purchase_order).collect())
}

/// A `SendAgreementPaymentRequest` whose `name` is `name` and whose every other
/// required member is valid, so `name` is the only member that can be refused.
fn payment_request(name: &str) -> Value {
    json!({
        "agreementId": "agmt-0000000000000001",
        "termId": "PaymentScheduleTerm",
        "name": name,
        "chargeAmount": "10.00",
    })
}

/// One request the service refuses, and the envelope it must be refused in.
struct Rejection {
    case: &'static str,
    operation: &'static str,
    body: Value,
    /// The `ValidationExceptionReason` value naming the offending member.
    reason: &'static str,
    /// The sentence that reason states.
    message: &'static str,
}

/// Constraints and operations `validation_envelope` does not reach, each
/// answered by the same reason-named sentence.
///
/// The constraints are the model's (`smithy_query(query="operation",…)` /
/// `query="shape"`): `AgreementCancellationRequestReasonCode` is an enum,
/// `PaymentRequestName` is `@length(5..64)`, `MaxResults` is `@range(1..50)`,
/// `PurchaseOrders` is `@length(1..86)`, `PurchaseOrder$chargeId` is a
/// `ResourceId` (`@pattern(^[A-Za-z0-9_/-]+$)`), `PurchaseOrder$chargeRevision`
/// is `@range(min: 1)` and `Intent` is an enum of NEW | AMEND | REPLACE.
#[tokio::test]
async fn a_refused_input_of_any_operation_is_named_by_its_reason() {
    let router = emulator();
    let cases = vec![
        Rejection {
            case: "a cancellation reason outside the enum",
            operation: "SendAgreementCancellationRequest",
            body: json!({"agreementId": "agmt-0000000000000001", "reasonCode": "MADE_UP_REASON"}),
            reason: "INVALID_REASON_CODE",
            message: "Provided reason code is invalid.",
        },
        Rejection {
            case: "a payment request name under its minimum length",
            operation: "SendAgreementPaymentRequest",
            body: payment_request("hi"),
            reason: "INVALID_NAME",
            message: "Provided name is invalid.",
        },
        Rejection {
            case: "a payment request name over its maximum length",
            operation: "SendAgreementPaymentRequest",
            body: payment_request(&"n".repeat(65)),
            reason: "INVALID_NAME",
            message: "Provided name is invalid.",
        },
        Rejection {
            case: "a page size over its maximum, on a paginated list",
            operation: "ListBillingAdjustmentRequests",
            body: json!({"maxResults": 51}),
            reason: "INVALID_MAX_RESULTS",
            message: "Provided max results is invalid.",
        },
        Rejection {
            case: "a page size under its minimum, on the search",
            operation: "SearchAgreements",
            body: json!({"maxResults": 0}),
            reason: "INVALID_MAX_RESULTS",
            message: "Provided max results is invalid.",
        },
        Rejection {
            case: "a list over its maximum length",
            operation: "UpdatePurchaseOrders",
            body: json!({"purchaseOrders": purchase_orders(87)}),
            reason: "INVALID_PURCHASE_ORDERS",
            message: "Provided purchase orders is invalid.",
        },
        Rejection {
            case: "a member of one list element outside its pattern",
            operation: "UpdatePurchaseOrders",
            body: json!({"purchaseOrders": [{"chargeId": "not a valid id!!"}]}),
            reason: "INVALID_CHARGE_ID",
            message: "Provided charge id is invalid.",
        },
        Rejection {
            case: "another member of a later list element under its minimum",
            operation: "UpdatePurchaseOrders",
            body: json!({"purchaseOrders": [purchase_order(1), {"chargeId": "c-2", "chargeRevision": 0}]}),
            reason: "INVALID_CHARGE_REVISION",
            message: "Provided charge revision is invalid.",
        },
        Rejection {
            case: "an enum value in the right words but the wrong case",
            operation: "CreateAgreementRequest",
            body: json!({"intent": "new", "requestedTerms": [{"id": "eula.v1"}]}),
            reason: "INVALID_INTENT",
            message: "Provided intent is invalid.",
        },
    ];

    for case in cases {
        let answer = call(&router, case.operation, case.body.clone()).await;
        assert_eq!(
            answer.status, 400,
            "case {:?} ({}): expected HTTP 400, got {} with {:?}",
            case.case, case.operation, answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {:?} ({}): expected ValidationException, got {:?}",
            case.case,
            case.operation,
            answer.body
        );
        assert_eq!(
            answer.reason(),
            case.reason,
            "case {:?} ({}): expected reason {:?}, got {:?}",
            case.case,
            case.operation,
            case.reason,
            answer.body
        );
        assert_eq!(
            answer.str_at("/message"),
            case.message,
            "case {:?} ({}): expected {:?}, got {:?}",
            case.case,
            case.operation,
            case.message,
            answer.body
        );
    }
}

/// The page-size bounds the model publishes are SERVED on every paginated
/// operation — the rejection above is of values outside them, not of the bounds.
///
/// `MaxResults` is modelled `@range(min: 1, max: 50)`, so 1 and 50 are served
/// and 0 and 51 refused. The two operations differ in what a served request
/// answers: `SearchAgreements` reports (HTTP 200), while
/// `ListBillingAdjustmentRequests` is still a stub and answers its 501, so there
/// the assertion is that it does not REFUSE the input.
#[tokio::test]
async fn the_published_page_size_bounds_are_served() {
    let router = emulator();
    for operation in ["SearchAgreements", "ListBillingAdjustmentRequests"] {
        for accepted in [1, 25, 50] {
            let answer = call(&router, operation, json!({"maxResults": accepted})).await;
            if operation == "SearchAgreements" {
                // An implemented report: a served bound answers the report
                // itself, so anything but 200 is a failure.
                assert_eq!(
                    answer.status, 200,
                    "case {operation} with maxResults {accepted}: a published bound must be \
                     reported, got {} with {:?}",
                    answer.status, answer.body
                );
            } else {
                assert_ne!(
                    answer.error_code(),
                    "ValidationException",
                    "case {operation} with maxResults {accepted}: a published bound must be \
                     served, got {} with {:?}",
                    answer.status,
                    answer.body
                );
            }
        }
        for refused in [0, 51] {
            let answer = call(&router, operation, json!({"maxResults": refused})).await;
            assert_eq!(
                answer.status, 400,
                "case {operation} with maxResults {refused}: expected HTTP 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.str_at("/message"),
                "Provided max results is invalid.",
                "case {operation} with maxResults {refused}: expected the max-results \
                 sentence, got {:?}",
                answer.body
            );
        }
    }
}

/// One pairing of a sort attribute with the party type a search is scoped to.
struct SortCase {
    case: &'static str,
    /// `None` sends no `sort` at all, which sorts by the documented default.
    sort_by: Option<&'static str>,
    /// `None` sends no `PartyType` filter, so the search is scoped to no party
    /// type.
    party_type: Option<&'static str>,
    /// Whether the model supports that attribute for that scope.
    supported: bool,
}

/// `Sort$sortBy` documents "`StartTime` and `LastUpdateTime` are supported only
/// when `PartyType` is `Proposer`", so a search scoped to a party type that
/// does not support the attribute it sorts on is refused — and every other
/// pairing is served from the stored agreements.
///
/// The table is the whole product of the three documented attributes and the
/// two documented party-type scopes (plus "no scope at all"), so no single
/// recorded pair can satisfy it.
#[tokio::test]
async fn a_sort_attribute_is_refused_outside_the_party_type_that_supports_it() {
    let router = emulator();
    let agreement = agreement_over(&router, "ap-offer1", vec![term("eula.v1")]).await;

    let cases = [
        SortCase {
            case: "no sort at all, so the documented default attribute",
            sort_by: None,
            party_type: Some("Acceptor"),
            supported: true,
        },
        SortCase {
            case: "the attribute both party types support, as the acceptor",
            sort_by: Some("EndTime"),
            party_type: Some("Acceptor"),
            supported: true,
        },
        SortCase {
            case: "the attribute both party types support, as the proposer",
            sort_by: Some("EndTime"),
            party_type: Some("Proposer"),
            supported: true,
        },
        SortCase {
            case: "the attribute both party types support, scoped to neither",
            sort_by: Some("EndTime"),
            party_type: None,
            supported: true,
        },
        SortCase {
            case: "a proposer-only attribute, as the acceptor",
            sort_by: Some("StartTime"),
            party_type: Some("Acceptor"),
            supported: false,
        },
        SortCase {
            case: "the other proposer-only attribute, as the acceptor",
            sort_by: Some("LastUpdateTime"),
            party_type: Some("Acceptor"),
            supported: false,
        },
        SortCase {
            case: "a proposer-only attribute, as the proposer",
            sort_by: Some("StartTime"),
            party_type: Some("Proposer"),
            supported: true,
        },
        SortCase {
            case: "the other proposer-only attribute, as the proposer",
            sort_by: Some("LastUpdateTime"),
            party_type: Some("Proposer"),
            supported: true,
        },
        SortCase {
            case: "a proposer-only attribute, scoped to no party type",
            sort_by: Some("StartTime"),
            party_type: None,
            supported: true,
        },
    ];

    for case in cases {
        // The agreement-type filter the recorded request carries, so the sort
        // pairing is judged against a filter list that selects something.
        let mut filters = vec![json!({"name": "AgreementType", "values": ["PurchaseAgreement"]})];
        if let Some(party_type) = case.party_type {
            filters.push(json!({"name": "PartyType", "values": [party_type]}));
        }
        let mut request = json!({"filters": filters, "maxResults": 5});
        if let Some(sort_by) = case.sort_by {
            request["sort"] = json!({"sortBy": sort_by, "sortOrder": "ASCENDING"});
        }
        let answer = call(&router, "SearchAgreements", request).await;

        if !case.supported {
            assert_eq!(
                answer.status, 400,
                "case {:?}: expected HTTP 400, got {} with {:?}",
                case.case, answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {:?}: expected ValidationException, got {:?}",
                case.case,
                answer.body
            );
            assert_eq!(
                answer.reason(),
                "UNSUPPORTED_FILTERS",
                "case {:?}: expected reason UNSUPPORTED_FILTERS, got {:?}",
                case.case,
                answer.body
            );
            assert_eq!(
                answer.str_at("/message"),
                "Provided combination of filters is not supported.",
                "case {:?}: expected the unsupported-combination message, got {:?}",
                case.case,
                answer.body
            );
            // The entry still names the member whose pairing with the filters
            // is unsupported, so a caller knows what to change.
            assert_eq!(
                answer.str_at("/fields/0/name"),
                "sort.sortBy",
                "case {:?}: expected the field entry to name sort.sortBy, got {:?}",
                case.case,
                answer.body
            );
            continue;
        }

        assert_eq!(
            answer.status, 200,
            "case {:?}: expected HTTP 200, got {} with {:?}",
            case.case, answer.status, answer.body
        );
        // A served search still reports what the store holds: the caller is both
        // parties of the agreement it accepted, so a search scoped to either
        // party type selects it.
        let reported: Vec<String> = answer.body["agreementViewSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| {
                summary["agreementId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        let expected: Vec<String> = vec![agreement.clone()];
        assert_eq!(
            reported, expected,
            "case {:?}: expected {expected:?}, got {:?}",
            case.case, answer.body
        );
    }
}
