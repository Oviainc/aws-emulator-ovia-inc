// SPDX-License-Identifier: CC0-1.0

//! What a `ValidationExceptionField` entry SAYS about the member it names.
//!
//! The rule under test: `ValidationExceptionField$message` is the fixed text of
//! the class the member failed in, reproduced exactly — one text for a value
//! that misses a constraint the model declares, one for a well-formed value a
//! rule of the service itself does not accept, one for a member that was absent
//! and is required of every request, and one for a member that was absent and
//! became required only through the `intent` the request stated. The variable
//! part of the entry is `name`, the rejected member's dotted request path taken
//! from the request. So the text depends on WHAT refused the member and not on
//! which member, which operation or which depth it sits at, and each text keeps
//! its own punctuation.
//!
//! Each row differs from the others in what the rule generalises over: the class
//! of the failure, the constraint missed, the member, the depth (top level, a
//! member of a list element), the operation, and whether the boundary walk or the
//! handler refused the request. The recorded cases — a present-but-
//! unacceptable member on `SendAgreementPaymentRequest` and on
//! `ListAgreementCancellationRequests`, the absent per-element agreement
//! identifier on `UpdatePurchaseOrders`, and the intent-conditional proposal
//! identifier on `CreateAgreementRequest` — are four rows among them.
//!
//! The harness (`emulator`, `call`, `term`) is the one `agreement_lifecycle`
//! already owns.

use serde_json::{json, Value};

use crate::agreement_lifecycle::{call, emulator, term, Answer};

/// The fixed text for a well-formed value a rule of the service itself does not
/// accept. Spelled out here rather than imported, so a change of wording in the
/// service fails this test instead of silently agreeing with it.
const FIELD_INVALID: &str = "Member is invalid";
/// The fixed text for a value that misses a constraint the model declares, which
/// the boundary walk refuses before any handler runs.
const FIELD_INVALID_CONSTRAINT: &str = "Member is invalid.";
/// The fixed text for a member that was absent from a request every one of which
/// has to carry it.
const FIELD_MISSING: &str = "Member is required.";
/// The fixed text for a member that was absent from a request whose `intent` is
/// what made it required.
const FIELD_MISSING_FOR_INTENT: &str = "Member must not be null for the provided intent.";

/// The class a refused member failed in, which is what decides the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// The value misses a constraint the model declares.
    InvalidConstraint,
    /// The value is well formed, and a rule of the service does not accept it.
    Unacceptable,
    /// The member was absent, and every request has to carry it.
    Absent,
    /// The member was absent, and the request's `intent` is what required it.
    AbsentForIntent,
}

impl Class {
    fn text(self) -> &'static str {
        match self {
            Self::InvalidConstraint => FIELD_INVALID_CONSTRAINT,
            Self::Unacceptable => FIELD_INVALID,
            Self::Absent => FIELD_MISSING,
            Self::AbsentForIntent => FIELD_MISSING_FOR_INTENT,
        }
    }
}

/// One refused request, and the single `fields` entry it must be answered with.
struct Row {
    /// What this row varies, for the assertion messages.
    case: &'static str,
    operation: &'static str,
    request: Value,
    /// The `ValidationExceptionReason` the body must carry.
    reason: &'static str,
    /// The message the body must carry.
    message: &'static str,
    /// The dotted request path the entry must name.
    field: &'static str,
    /// The class that path's entry must be worded in.
    class: Class,
}

/// The `fields` entries of an answer, as `(name, message)` pairs in the order
/// they were reported.
fn fields(answer: &Answer) -> Vec<(String, String)> {
    answer.body["fields"]
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

/// A `SendAgreementPaymentRequest` whose every member satisfies its modelled
/// constraints (`PaymentRequestName` is `@length(5..64)`,
/// `PositiveAmountUpto8Decimals` is `^[0-9]*(\.[0-9]{0,8})?$`,
/// `smithy_query(query="operation", operation="SendAgreementPaymentRequest")`),
/// so the only member that can be refused is the `agreementId` it charges under.
fn payment_request(agreement_id: &str) -> Value {
    json!({
        "agreementId": agreement_id,
        "termId": "VariablePaymentTerm",
        "name": "October usage",
        "chargeAmount": "10.00",
    })
}

fn rows() -> Vec<Row> {
    vec![
        // A value that misses a constraint the model declares, over the
        // constraints, the depths and the operations.
        Row {
            case: "a pattern violation at the top level",
            operation: "DescribeAgreement",
            request: json!({"agreementId": "not an agreement id!"}),
            reason: "INVALID_AGREEMENT_ID",
            message: "Provided agreement id is invalid.",
            field: "agreementId",
            class: Class::InvalidConstraint,
        },
        Row {
            case: "a value outside a modelled enum, nested in a structure",
            operation: "SearchAgreements",
            request: json!({"sort": {"sortOrder": "SIDEWAYS"}}),
            reason: "INVALID_SORT_ORDER",
            message: "Provided sort order is invalid.",
            field: "sort.sortOrder",
            class: Class::InvalidConstraint,
        },
        Row {
            case: "a number above the range its member admits",
            operation: "GetAgreementTerms",
            request: json!({"agreementId": "agmt-1", "maxResults": 51}),
            reason: "INVALID_MAX_RESULTS",
            message: "Provided max results is invalid.",
            field: "maxResults",
            class: Class::InvalidConstraint,
        },
        Row {
            case: "a string below the length its member admits",
            operation: "SendAgreementPaymentRequest",
            request: json!({
                "agreementId": "agmt-1",
                "termId": "VariablePaymentTerm",
                "name": "four",
                "chargeAmount": "10.00",
            }),
            reason: "INVALID_NAME",
            message: "Provided name is invalid.",
            field: "name",
            class: Class::InvalidConstraint,
        },
        Row {
            case: "a pattern violation on a member of a list element",
            operation: "UpdatePurchaseOrders",
            request: json!({"purchaseOrders": [{"chargeId": "not a charge id!"}]}),
            reason: "INVALID_CHARGE_ID",
            message: "Provided charge id is invalid.",
            field: "purchaseOrders.1.member.chargeId",
            class: Class::InvalidConstraint,
        },
        Row {
            // The SAME member and the same reason as the service-rule row
            // below, failing the constraint its shape declares instead
            // (`PartyType` is `@pattern ^[A-Za-z]+$`): the text follows what
            // refused the value, not the member it was about.
            case: "a party type outside the pattern its shape declares",
            operation: "ListAgreementCancellationRequests",
            request: json!({"partyType": "Not A Party!"}),
            reason: "INVALID_PARTY_TYPE",
            message: "Provided party type is invalid.",
            field: "partyType",
            class: Class::InvalidConstraint,
        },
        // A well-formed value a rule of the service itself does not accept.
        Row {
            case: "a party type naming neither party",
            operation: "ListAgreementCancellationRequests",
            request: json!({"partyType": "Bystander"}),
            reason: "INVALID_PARTY_TYPE",
            message: "Provided party type is invalid.",
            field: "partyType",
            class: Class::Unacceptable,
        },
        Row {
            case: "a catalog this marketplace does not expose",
            operation: "ListAgreementCancellationRequests",
            request: json!({"partyType": "Acceptor", "catalog": "AnotherCatalog"}),
            reason: "INVALID_CATALOG",
            message: "Provided catalog is invalid.",
            field: "catalog",
            class: Class::Unacceptable,
        },
        Row {
            case: "an id naming no agreement the caller can charge under",
            operation: "SendAgreementPaymentRequest",
            request: payment_request("agmt-nothing-holds-this"),
            reason: "INVALID_AGREEMENT_ID",
            message: "Provided agreement id is invalid.",
            field: "agreementId",
            class: Class::Unacceptable,
        },
        Row {
            case: "a member the stated intent does not admit at all",
            operation: "CreateAgreementRequest",
            request: json!({
                "intent": "NEW",
                "agreementProposalIdentifier": "ap-offer1",
                "sourceAgreementIdentifier": "agmt-1",
                "requestedTerms": [term("eula.v1")],
            }),
            reason: "INVALID_SOURCE_AGREEMENT_IDENTIFIER",
            message: "Provided source agreement identifier is invalid.",
            field: "sourceAgreementIdentifier",
            class: Class::Unacceptable,
        },
        Row {
            case: "the intent-conditional member present but outside its pattern",
            operation: "CreateAgreementRequest",
            request: json!({
                "intent": "NEW",
                "agreementProposalIdentifier": "not a proposal!",
                "requestedTerms": [term("eula.v1")],
            }),
            reason: "INVALID_AGREEMENT_PROPOSAL_IDENTIFIER",
            message: "Provided agreement proposal identifier is invalid.",
            field: "agreementProposalIdentifier",
            class: Class::InvalidConstraint,
        },
        // A member absent from a request every one of which has to carry it.
        Row {
            case: "a required scalar left out",
            operation: "DescribeAgreement",
            request: json!({}),
            reason: "MISSING_AGREEMENT_ID",
            message: "Agreement id is required.",
            field: "agreementId",
            class: Class::Absent,
        },
        Row {
            case: "a required list left out",
            operation: "UpdatePurchaseOrders",
            request: json!({}),
            reason: "MISSING_PURCHASE_ORDERS",
            message: "Purchase orders is required.",
            field: "purchaseOrders",
            class: Class::Absent,
        },
        Row {
            case: "a required member of a list element left out",
            operation: "SearchAgreements",
            request: json!({"filters": [{"values": ["x"]}]}),
            reason: "MISSING_FILTER_NAME",
            message: "Filter name is required.",
            field: "filters.1.member.name",
            class: Class::Absent,
        },
        Row {
            // Two defects at once: the element names a charge nothing holds AND
            // leaves out the agreement the association belongs to. The absent
            // member is what the refusal is about, so the entry is the
            // absent-member one at that member's own path.
            case: "an absent per-element member of a request with a second defect",
            operation: "UpdatePurchaseOrders",
            request: json!({
                "purchaseOrders": [
                    {"chargeId": "chg-1", "agreementId": "agmt-1", "chargeRevision": 1,
                     "purchaseOrderReference": "po-1"},
                    {"chargeId": "chg-nothing-holds-this", "chargeRevision": 1,
                     "purchaseOrderReference": "po-2"},
                ],
            }),
            reason: "MISSING_AGREEMENT_ID",
            message: "Agreement id is required.",
            field: "purchaseOrders.2.member.agreementId",
            class: Class::Absent,
        },
        // A member absent from a request whose intent is what required it:
        // "required if the intent is not AMEND" and "required for all non-NEW
        // intents" (`smithy_query(query="operation",
        // operation="CreateAgreementRequest")`), over every intent each rule
        // covers.
        Row {
            case: "the proposal identifier left out under the NEW intent",
            operation: "CreateAgreementRequest",
            request: json!({"intent": "NEW", "requestedTerms": [term("eula.v1")]}),
            reason: "MISSING_AGREEMENT_PROPOSAL_IDENTIFIER",
            message: "Agreement proposal identifier is required.",
            field: "agreementProposalIdentifier",
            class: Class::AbsentForIntent,
        },
        Row {
            case: "the same member left out under the REPLACE intent",
            operation: "CreateAgreementRequest",
            request: json!({
                "intent": "REPLACE",
                "sourceAgreementIdentifier": "agmt-1",
                "requestedTerms": [term("eula.v1")],
            }),
            reason: "MISSING_AGREEMENT_PROPOSAL_IDENTIFIER",
            message: "Agreement proposal identifier is required.",
            field: "agreementProposalIdentifier",
            class: Class::AbsentForIntent,
        },
        Row {
            case: "the source agreement left out under the AMEND intent",
            operation: "CreateAgreementRequest",
            request: json!({"intent": "AMEND", "requestedTerms": [term("eula.v1")]}),
            reason: "MISSING_SOURCE_AGREEMENT_IDENTIFIER",
            message: "Source agreement identifier is required.",
            field: "sourceAgreementIdentifier",
            class: Class::AbsentForIntent,
        },
        Row {
            case: "the same member left out under the REPLACE intent",
            operation: "CreateAgreementRequest",
            request: json!({
                "intent": "REPLACE",
                "agreementProposalIdentifier": "ap-offer1",
                "requestedTerms": [term("eula.v1")],
            }),
            reason: "MISSING_SOURCE_AGREEMENT_IDENTIFIER",
            message: "Source agreement identifier is required.",
            field: "sourceAgreementIdentifier",
            class: Class::AbsentForIntent,
        },
    ]
}

/// Every refused member is named in one `fields` entry whose message is the
/// fixed text of the class it failed in, at the modelled 400
/// `ValidationException` carrying that class's reason and message.
#[tokio::test]
async fn a_field_entry_states_the_fixed_text_of_its_class() {
    let router = emulator();
    for row in rows() {
        let case = row.case;
        let operation = row.operation;
        let answer = call(&router, operation, row.request.clone()).await;

        assert_eq!(
            answer.status, 400,
            "case {case:?} ({operation}): expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?} ({operation}): expected error code ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.reason(),
            row.reason,
            "case {case:?} ({operation}): expected reason {:?}, got {:?}",
            row.reason,
            answer.body
        );
        assert_eq!(
            answer.str_at("/message"),
            row.message,
            "case {case:?} ({operation}): expected message {:?}, got {:?}",
            row.message,
            answer.body
        );
        assert_eq!(
            fields(&answer),
            vec![(row.field.to_string(), row.class.text().to_string())],
            "case {case:?} ({operation}): expected one entry at {:?} reading {:?}, got {:?}",
            row.field,
            row.class.text(),
            answer.body
        );
    }
}

/// The texts are each other's contrast: ONE member, refused in three classes,
/// is worded in the three texts of those classes — so the wording belongs to
/// what refused the member, not to the member.
#[tokio::test]
async fn one_member_refused_in_three_classes_reads_in_three_texts() {
    let router = emulator();
    let member = "agreementId";

    // Present, and outside the pattern `AgreementId` declares: a constraint the
    // model states, so the boundary walk refuses it.
    let constraint = call(
        &router,
        "DescribeAgreement",
        json!({"agreementId": "not an agreement id!"}),
    )
    .await;
    // Well formed, and naming no agreement this account can charge under: a rule
    // of the service, so a handler refuses it.
    let unacceptable = call(
        &router,
        "SendAgreementPaymentRequest",
        payment_request("agmt-nothing-holds-this"),
    )
    .await;
    // Absent from a request that has to carry it.
    let absent = call(&router, "DescribeAgreement", json!({})).await;

    let text_of = |answer: &Answer| {
        fields(answer)
            .first()
            .map(|(_, message)| message.clone())
            .unwrap_or_default()
    };
    for (case, answer, expected) in [
        ("a constraint the model declares", &constraint, FIELD_INVALID_CONSTRAINT),
        ("a rule of the service", &unacceptable, FIELD_INVALID),
        ("a member left out", &absent, FIELD_MISSING),
    ] {
        assert_eq!(
            fields(answer),
            vec![(member.to_string(), expected.to_string())],
            "case {case:?}: expected one entry at {member:?} reading {expected:?}, got {:?}",
            answer.body
        );
    }

    let texts = [
        text_of(&constraint),
        text_of(&unacceptable),
        text_of(&absent),
    ];
    assert!(
        texts[0] != texts[1] && texts[1] != texts[2] && texts[0] != texts[2],
        "case {member:?}: the three classes must each have their own text, got {texts:?}"
    );
    assert_ne!(
        texts[1], FIELD_INVALID_CONSTRAINT,
        "case {member:?}: a value a rule of the service refuses must not read as a missed \
         modelled constraint, got {:?}",
        texts[1]
    );
}
