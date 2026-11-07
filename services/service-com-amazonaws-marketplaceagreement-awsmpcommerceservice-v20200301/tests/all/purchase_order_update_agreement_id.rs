// SPDX-License-Identifier: CC0-1.0

//! The member every element of an `UpdatePurchaseOrders` list has to NAME the
//! association by, end to end over HTTP.
//!
//! The rule under test: a member an operation requires of every element of a
//! list it is sent is checked over the WHOLE list before the request is refused
//! at the request level, and an element that leaves it out is answered in the
//! service's modelled `ValidationException` (HTTP 400,
//! `smithy_query(query="operation", operation="UpdatePurchaseOrders")`) under
//! the `ValidationExceptionReason` that names that member as absent
//! (`MISSING_AGREEMENT_ID`, `smithy_query(query="shape",
//! shape="ValidationExceptionReason")`), with the sentence of that class and one
//! `fields` entry locating the member — the list member, the element's 1-based
//! position, the step into the element and the member inside it. A member the
//! caller left out is something the caller can repair, which is exactly what a
//! refusal of the whole ACTION cannot say, because it names no member at all:
//! so the action refusal is answered only once every element states what it has
//! to, and `purchase_order_updates` is the suite for it.
//!
//! `UpdatePurchaseOrders` requires `agreementId` of each of its elements — "the
//! unique identifier of the agreement associated with this charge"
//! (`smithy_query(query="shape", shape="PurchaseOrder")`), the agreement whose
//! association the call updates — even though the model marks only `chargeId`
//! `@required`; the member is NOT required at the other door that carries the
//! same list (`AcceptAgreementRequest$purchaseOrders`), where the agreement does
//! not exist when the request is made, which `purchase_order_agreement_ids`
//! pins.
//!
//! Nothing is resolved against the store on the way: the element's own absent
//! member is reported whether or not its `chargeId` names a charge this service
//! holds, and the list associates nothing either way, which the charge report
//! (`ListAgreementCharges`) is read back for.
//!
//! The rows differ in what the rule generalises over: how long the list is (one
//! element, two, three, and the longest the model admits), the position of the
//! first element that leaves the member out (first, middle, last), how many of
//! the other elements state it, whether the element names a charge the store
//! holds, what else the element leaves out or states wrongly, and what form the
//! value takes where it IS stated (the agreement holding the charge, an id no
//! agreement is under, and the shortest and longest id the member admits). The
//! last test extends the absent-member wording to the other operations and
//! members it holds for.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::agreement_lifecycle::{call, emulator, Answer};
use crate::purchase_order_associations::{order_at, INITIAL_CHARGE_REVISION};
use crate::purchase_order_updates::{agreement_holding, assert_unsupported_action, references, update};

/// The `ValidationExceptionReason` the model publishes for the member being
/// absent (`smithy_query(query="shape", shape="ValidationExceptionReason")`).
const MISSING_AGREEMENT_ID: &str = "MISSING_AGREEMENT_ID";

/// The sentence this service's `ValidationException` states for that reason, and
/// what the `fields` entry states about a member the request left out — spelled
/// out here rather than imported, so a change of wording in the service fails
/// this test instead of agreeing with it.
const MISSING_AGREEMENT_ID_MESSAGE: &str = "Agreement id is required.";
/// See [`MISSING_AGREEMENT_ID_MESSAGE`].
const FIELD_MISSING: &str = "Member is required.";
/// See [`MISSING_AGREEMENT_ID_MESSAGE`]. What the entry states instead about a
/// member the request left out that the `intent` it stated is what required.
const FIELD_MISSING_FOR_INTENT: &str = "Member must not be null for the provided intent.";

/// A well-formed `ResourceId` no agreement of the store is ever minted under,
/// and one no charge of it is: the member is not resolved, so neither is
/// reported as unknown.
const UNHELD_AGREEMENT_ID: &str = "agmt-ffffffffffffffff";
const UNHELD_CHARGE_ID: &str = "ch-ffffffffffffffff";

/// `PurchaseOrder$agreementId` targets `ResourceId`, `@length(min: 1, max: 64)`
/// with pattern `^[A-Za-z0-9_/-]+$` (`smithy_query(query="shape",
/// shape="PurchaseOrder")`), so these two values are at its published bounds:
/// the shortest and the longest id a caller can state at all.
const SHORTEST_ID: &str = "a";
const LONGEST_ID: &str = "A012345678901234567890123456789012345678901234567890123456789012";

/// `PurchaseOrders` is `@length(min: 1, max: 86)`
/// (`smithy_query(query="operation", operation="UpdatePurchaseOrders")`), so a
/// list of this many elements is the longest one the model admits.
const MAX_PURCHASE_ORDERS: usize = 86;

/// The two literals above are at the published bounds of the member they are
/// stated for, so a row that states one is a row at that bound.
fn assert_published_bounds() {
    assert_eq!(
        SHORTEST_ID.chars().count(),
        1,
        "case {SHORTEST_ID:?}: expected the member's minimum length 1, got {}",
        SHORTEST_ID.chars().count()
    );
    assert_eq!(
        LONGEST_ID.chars().count(),
        64,
        "case {LONGEST_ID:?}: expected the member's maximum length 64, got {}",
        LONGEST_ID.chars().count()
    );
}

/// What one element states as its `agreementId`.
#[derive(Clone, Copy, Debug)]
enum Stated {
    /// The member is left out of the element entirely.
    Omitted,
    /// The agreement holding the charges the row's elements name.
    Holder,
    /// A value stated outright, whatever it names.
    Literal(&'static str),
}

/// What one element states about the REST of the association — the members
/// `purchase_order_required_members` is the suite for. A row varies them to show
/// the absent `agreementId` is what gets reported whatever else the element
/// carries.
#[derive(Clone, Copy, Debug)]
enum Rest {
    /// Both other members stated, the revision being the one the charge is at.
    Whole,
    /// `chargeRevision` left out.
    NoRevision,
    /// `purchaseOrderReference` left out.
    NoReference,
    /// `chargeRevision` stated as a revision no charge of this store is at.
    RevisionPastTheCharge,
}

/// One element of a row's list: which charge it names — `Some(position)` the
/// charge at that position of the row's agreement, `None` an id the store holds
/// no charge for — what it states about the agreement, and what it states about
/// the rest.
#[derive(Clone, Copy, Debug)]
struct Element {
    charge: Option<usize>,
    stated: Stated,
    rest: Rest,
}

/// The element every row is varied from: the first charge of the agreement, the
/// agreement holding it, and every other member of the association stated.
const WHOLE: Element = Element {
    charge: Some(0),
    stated: Stated::Holder,
    rest: Rest::Whole,
};

/// That element with the agreement left out of it.
const WITHOUT: Element = Element {
    stated: Stated::Omitted,
    ..WHOLE
};

/// One row: the charges the agreement holds, the elements of the list, and the
/// 1-based position of the element the refusal must name — `None` for a list
/// every element of which names its association, which is refused as the
/// unsupported action instead.
struct Row {
    case: String,
    charges: usize,
    elements: Vec<Element>,
    refused_at: Option<usize>,
}

/// One element naming the charge at `charge` of the agreement's charges.
fn at(charge: usize, stated: Stated, rest: Rest) -> Element {
    Element {
        charge: Some(charge),
        stated,
        rest,
    }
}

fn rows() -> Vec<Row> {
    let mut rows = vec![
        Row {
            case: "the only element leaving the member out".to_string(),
            charges: 1,
            elements: vec![WITHOUT],
            refused_at: Some(1),
        },
        Row {
            case: "the only element naming the agreement holding its charge".to_string(),
            charges: 1,
            elements: vec![WHOLE],
            refused_at: None,
        },
        Row {
            case: "the second of two leaving it out".to_string(),
            charges: 2,
            elements: vec![WHOLE, at(1, Stated::Omitted, Rest::Whole)],
            refused_at: Some(2),
        },
        Row {
            case: "the first of two leaving it out, the second naming it".to_string(),
            charges: 2,
            elements: vec![WITHOUT, at(1, Stated::Holder, Rest::Whole)],
            refused_at: Some(1),
        },
        Row {
            case: "the middle of three leaving it out".to_string(),
            charges: 3,
            elements: vec![
                WHOLE,
                at(1, Stated::Omitted, Rest::Whole),
                at(2, Stated::Holder, Rest::Whole),
            ],
            refused_at: Some(2),
        },
        Row {
            case: "the last of three leaving it out".to_string(),
            charges: 3,
            elements: vec![
                WHOLE,
                at(1, Stated::Holder, Rest::Whole),
                at(2, Stated::Omitted, Rest::Whole),
            ],
            refused_at: Some(3),
        },
        Row {
            case: "every element of three leaving it out, so the first is named".to_string(),
            charges: 3,
            elements: vec![
                WITHOUT,
                at(1, Stated::Omitted, Rest::Whole),
                at(2, Stated::Omitted, Rest::Whole),
            ],
            refused_at: Some(1),
        },
        Row {
            case: "an element naming a charge the store does not hold".to_string(),
            charges: 1,
            elements: vec![Element {
                charge: None,
                ..WITHOUT
            }],
            refused_at: Some(1),
        },
        Row {
            case: "a stored charge before an unheld one that leaves it out".to_string(),
            charges: 1,
            elements: vec![
                WHOLE,
                Element {
                    charge: None,
                    ..WITHOUT
                },
            ],
            refused_at: Some(2),
        },
        Row {
            case: "an unheld charge naming an id no agreement is under".to_string(),
            charges: 1,
            elements: vec![Element {
                charge: None,
                stated: Stated::Literal(UNHELD_AGREEMENT_ID),
                rest: Rest::Whole,
            }],
            refused_at: None,
        },
        Row {
            case: "an element leaving out the reference as well".to_string(),
            charges: 1,
            elements: vec![Element {
                rest: Rest::NoReference,
                ..WITHOUT
            }],
            refused_at: Some(1),
        },
        Row {
            case: "an element leaving out the revision as well".to_string(),
            charges: 1,
            elements: vec![Element {
                rest: Rest::NoRevision,
                ..WITHOUT
            }],
            refused_at: Some(1),
        },
        Row {
            case: "an element stating a revision no charge is at as well".to_string(),
            charges: 1,
            elements: vec![Element {
                rest: Rest::RevisionPastTheCharge,
                ..WITHOUT
            }],
            refused_at: Some(1),
        },
        Row {
            case: "the shortest id the member admits".to_string(),
            charges: 1,
            elements: vec![Element {
                stated: Stated::Literal(SHORTEST_ID),
                ..WHOLE
            }],
            refused_at: None,
        },
        Row {
            case: "the longest id the member admits".to_string(),
            charges: 2,
            elements: vec![
                WHOLE,
                at(1, Stated::Literal(LONGEST_ID), Rest::Whole),
            ],
            refused_at: None,
        },
    ];

    // The longest list the model admits, every element of it naming an
    // association — and the same list with its LAST element leaving the member
    // out, which is the furthest into a list the walk has to reach.
    let mut longest: Vec<Element> = (0..MAX_PURCHASE_ORDERS)
        .map(|_| Element {
            charge: None,
            ..WHOLE
        })
        .collect();
    longest[0] = WHOLE;
    rows.push(Row {
        case: format!("a list of {MAX_PURCHASE_ORDERS} elements, each naming an agreement"),
        charges: 1,
        elements: longest.clone(),
        refused_at: None,
    });
    let last = longest.len();
    longest[last - 1] = Element {
        charge: None,
        ..WITHOUT
    };
    rows.push(Row {
        case: format!("a list of {MAX_PURCHASE_ORDERS} elements, its last leaving it out"),
        charges: 1,
        elements: longest,
        refused_at: Some(last),
    });
    rows
}

/// The reference the element at `position` of a row's list carries.
fn reference(position: usize) -> String {
    format!("PO-{position}")
}

/// The JSON one element of a row's list is sent as.
fn element_json(element: Element, position: usize, holder: &str, charge_ids: &[String]) -> Value {
    let charge_id = match element.charge {
        Some(position) => charge_ids[position].clone(),
        None => format!("{UNHELD_CHARGE_ID}-{position}"),
    };
    let revision = match element.rest {
        Rest::NoRevision => None,
        Rest::RevisionPastTheCharge => Some(INITIAL_CHARGE_REVISION + 41),
        _ => Some(INITIAL_CHARGE_REVISION),
    };
    let mut json = order_at(&charge_id, &reference(position), revision);
    if matches!(element.rest, Rest::NoReference) {
        json.as_object_mut()
            .expect("an association is a JSON object")
            .remove("purchaseOrderReference");
    }
    match element.stated {
        Stated::Omitted => {}
        Stated::Holder => json["agreementId"] = json!(holder),
        Stated::Literal(literal) => json["agreementId"] = json!(literal),
    }
    json
}

/// Assert `answer` is the modelled refusal of the `agreementId` the element at
/// `position` of the request's list left out: the reason that names the member
/// as absent, the sentence of that class, and ONE `fields` entry at that
/// element's own dotted request path.
///
/// `pub(crate)` because `purchase_order_agreement_ids` drives the same door and
/// reads the refusal through this one spelling.
pub(crate) fn assert_missing_agreement_id(case: &str, answer: &Answer, position: usize) {
    let field = format!("purchaseOrders.{position}.member.agreementId");
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case:?}: expected a ValidationException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.reason(),
        MISSING_AGREEMENT_ID,
        "case {case:?}: expected the reason {MISSING_AGREEMENT_ID}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/message"),
        MISSING_AGREEMENT_ID_MESSAGE,
        "case {case:?}: expected the message {MISSING_AGREEMENT_ID_MESSAGE:?}, got {:?}",
        answer.body
    );
    let fields: Vec<(String, String)> = answer.body["fields"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            (
                entry["name"].as_str().unwrap_or_default().to_string(),
                entry["message"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(
        fields,
        vec![(field.clone(), FIELD_MISSING.to_string())],
        "case {case:?}: expected one entry naming {field:?}, got {:?}",
        answer.body
    );
}

/// Every list whose element leaves the agreement out is refused on THAT member,
/// at that element's own position, and every list whose elements each name one is
/// refused as the unsupported action instead — whatever the elements name, and
/// whatever the store holds. Nothing is associated either way.
#[tokio::test]
async fn an_update_names_the_agreement_of_every_association_it_carries() {
    assert_published_bounds();
    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case.as_str();
        let router = emulator();
        let (holder, charge_ids) =
            agreement_holding(&router, &format!("ap-offerUAI{index}"), row.charges).await;
        let unassociated: BTreeMap<String, Option<String>> = charge_ids
            .iter()
            .map(|charge_id| (charge_id.clone(), None))
            .collect();

        let orders: Vec<Value> = row
            .elements
            .iter()
            .enumerate()
            .map(|(position, element)| element_json(*element, position, &holder, &charge_ids))
            .collect();
        let answered = update(&router, Value::Array(orders)).await;

        match row.refused_at {
            Some(position) => assert_missing_agreement_id(case, &answered, position),
            None => assert_unsupported_action(case, &answered),
        }

        let reported = references(&router, &holder).await;
        assert_eq!(
            reported, unassociated,
            "case {case:?}: a refused update associates nothing, expected {unassociated:?}, got \
             {reported:?}"
        );
    }
}

/// One member left out of one operation's request, and the reason naming it.
struct Absent {
    case: &'static str,
    operation: &'static str,
    request: Value,
    reason: &'static str,
    field: &'static str,
    message: &'static str,
    /// The fixed text the entry states: the unconditional requirement
    /// ([`FIELD_MISSING`]), or the intent-conditional one for a member the
    /// request's `intent` is what made required.
    text: &'static str,
}

/// The absent-member class reads alike wherever it is reported: the sentence
/// names the member the reason names, and the entry states the requirement the
/// member is under.
///
/// The rows span the depths a member sits at and the operations that refuse one:
/// a top-level member of a single-resource read, a member the service requires
/// only for some intents — whose entry states that conditional requirement
/// instead ([`FIELD_MISSING_FOR_INTENT`]) — a member of an element of a list the
/// boundary walk refuses, and the member this operation requires of its own
/// elements, which is the one a handler refuses.
#[tokio::test]
async fn the_absent_member_class_states_the_requirement_whichever_member_it_is_about() {
    let router = emulator();
    let absent = [
        Absent {
            case: "a required member of a single-resource read",
            operation: "DescribeAgreement",
            request: json!({}),
            reason: "MISSING_AGREEMENT_ID",
            field: "agreementId",
            message: "Agreement id is required.",
            text: FIELD_MISSING,
        },
        Absent {
            case: "a member required for the intent the request states",
            operation: "CreateAgreementRequest",
            request: json!({"intent": "NEW", "requestedTerms": [{"id": "eula.v1"}]}),
            reason: "MISSING_AGREEMENT_PROPOSAL_IDENTIFIER",
            field: "agreementProposalIdentifier",
            message: "Agreement proposal identifier is required.",
            text: FIELD_MISSING_FOR_INTENT,
        },
        Absent {
            case: "a member of an element of a list",
            operation: "SearchAgreements",
            request: json!({"filters": [{"values": ["PurchaseAgreement"]}]}),
            reason: "MISSING_FILTER_NAME",
            field: "filters.1.member.name",
            message: "Filter name is required.",
            text: FIELD_MISSING,
        },
        Absent {
            case: "the member an update requires of its own elements",
            operation: "UpdatePurchaseOrders",
            request: json!({"purchaseOrders": [{
                "chargeId": "ch-0000000000000001",
                "chargeRevision": INITIAL_CHARGE_REVISION,
                "purchaseOrderReference": "PO-1",
            }]}),
            reason: "MISSING_AGREEMENT_ID",
            field: "purchaseOrders.1.member.agreementId",
            message: "Agreement id is required.",
            text: FIELD_MISSING,
        },
    ];

    for row in absent {
        let case = row.case;
        let answer = call(&router, row.operation, row.request.clone()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} ({}): expected HTTP 400, got {} with {:?}",
            row.operation, answer.status, answer.body
        );
        assert_eq!(
            answer.reason(),
            row.reason,
            "case {case:?} ({}): expected the reason {:?}, got {:?}",
            row.operation,
            row.reason,
            answer.body
        );
        assert_eq!(
            answer.str_at("/message"),
            row.message,
            "case {case:?} ({}): expected the message {:?}, got {:?}",
            row.operation,
            row.message,
            answer.body
        );
        assert_eq!(
            answer.str_at("/fields/0/name"),
            row.field,
            "case {case:?} ({}): expected the entry to name {:?}, got {:?}",
            row.operation,
            row.field,
            answer.body
        );
        assert_eq!(
            answer.str_at("/fields/0/message"),
            row.text,
            "case {case:?} ({}): expected the entry to state {:?}, got {:?}",
            row.operation,
            row.text,
            answer.body
        );
    }
}
