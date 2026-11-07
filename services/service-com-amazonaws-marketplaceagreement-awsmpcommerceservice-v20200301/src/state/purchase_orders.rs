// SPDX-License-Identifier: CC0-1.0

//! Purchase-order associations: the members an element of a `purchaseOrders`
//! list has to state, and how such a list is resolved against the charges it
//! names.

use super::*;

/// The request member every operation carrying purchase-order associations
/// names the list with (`AcceptAgreementRequestInput$purchaseOrders`,
/// `UpdatePurchaseOrdersInput$purchaseOrders`).
pub(super) const PURCHASE_ORDERS_MEMBER: &str = "purchaseOrders";

/// The `ValidationExceptionReason` for a list that associates one charge twice
/// (`smithy_query(query="shape", shape="ValidationExceptionReason")` lists
/// `DUPLICATE_CHARGES`).
///
/// Spelled out rather than resolved from a member the way [`member_reason`]
/// resolves the rest: no single element of such a list is the wrong one, so the
/// reason is about the list.
pub const REASON_DUPLICATE_CHARGES: &str = "DUPLICATE_CHARGES";

/// Why one purchase-order association was refused: the `ValidationException`
/// `reason` to report, and the dotted request path of the member it is about —
/// which element of the request's `purchaseOrders` list carried it, and which
/// member of that element.
#[derive(Debug)]
pub struct PurchaseOrderRefusal {
    pub reason: String,
    pub path: String,
}

/// The dotted request path of one member of the `index`-th element of a
/// request's `purchaseOrders` list — `purchaseOrders.1.member.chargeId` for the
/// FIRST order's `chargeId`.
///
/// Spelled through the framework's own path rule
/// ([`::aws_smithy_schema_server::constraints::path`]), exactly as
/// [`filter_member_path`] is, so a path this service builds for a rule the model
/// cannot express reads like one the boundary validator builds for a modelled
/// constraint.
pub fn purchase_order_member_path(index: usize, member: &str) -> String {
    use ::aws_smithy_schema_server::constraints::path;
    path::join(&[
        path::member(PURCHASE_ORDERS_MEMBER),
        path::element(index + 1),
        path::member(member),
    ])
}

/// The ids of the charges a quote minted, which is the set a purchase order's
/// `chargeId` has to name.
///
/// `ChargeSummary$expectedCharges` carries one `ExpectedCharge` per charge the
/// requested terms imply, and `ExpectedCharge$id` is the "Unique identifier of
/// the charge for a given agreement" — so these ids, and no others, are the
/// charges of the agreement that summary belongs to.
pub fn expected_charge_ids(summary: &ChargeSummary) -> BTreeSet<&str> {
    summary
        .expected_charges
        .iter()
        .flatten()
        .filter_map(|charge| charge.id.as_deref())
        .collect()
}

/// The state of ONE charge a purchase-order association can be made against:
/// everything an element of a `purchaseOrders` list restates about the charge it
/// names, and therefore everything [`validate_purchase_orders`] checks such an
/// element against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChargeAssociationTarget {
    /// The revision the charge is at, which an element's `chargeRevision`
    /// precondition has to agree with.
    pub revision: i64,
    /// The agreement holding the charge, which an element's `agreementId` has to
    /// name when it states one — `None` for a charge no agreement holds YET,
    /// which is every charge at the door that mints the agreement they will
    /// belong to, so no value a caller can state is the holder there.
    pub agreement_id: Option<String>,
}

/// The state of each charge a quote minted, which is what a purchase order's
/// members are checked against when the charges it names are the ones the call
/// being validated is about to mint.
///
/// Each of those charges comes into existence with the call that is being
/// validated, so each is at its first revision ([`INITIAL_CHARGE_REVISION`]) and
/// none of them is held by an agreement yet — the agreement that will hold them
/// is minted by that same call; the ids are the quote's own
/// ([`expected_charge_ids`]).
pub fn expected_charge_targets(
    summary: &ChargeSummary,
) -> BTreeMap<String, ChargeAssociationTarget> {
    expected_charge_ids(summary)
        .into_iter()
        .map(|id| {
            (
                id.to_string(),
                ChargeAssociationTarget {
                    revision: INITIAL_CHARGE_REVISION,
                    agreement_id: None,
                },
            )
        })
        .collect()
}

/// Refuse a purchase-order association list that cannot be resolved against
/// `charges` — the state of every charge the list can name
/// ([`ChargeAssociationTarget`]), keyed by charge id — BEFORE anything is
/// stored, so a list naming a charge that does not exist associates nothing.
///
/// `PurchaseOrder` is "information about a purchase order association to a
/// charge within an agreement": `chargeId` is "the unique identifier of the
/// charge to associate the purchase order with", `chargeRevision` is "the
/// revision of the charge", `agreementId` is "the unique identifier of the
/// agreement associated with this charge" and `purchaseOrderReference` is "the
/// purchase order reference to associate with the charge". An association
/// therefore resolves only when
///
/// * the charge it names is one of the agreement's own charges — otherwise
///   there is nothing to associate with (`INVALID_CHARGE_ID`);
/// * no earlier element of the same list named that charge already — a charge
///   carries ONE purchase order reference, so two elements naming it ask for two
///   ([`REASON_DUPLICATE_CHARGES`]);
/// * it carries the reference the association exists to record — an element
///   without one associates nothing (`MISSING_PURCHASE_ORDER_REFERENCE`);
/// * it states the revision that charge is at — a member that restates the
///   current state of the resource being written is the caller's precondition on
///   it, so a value other than the charge's own revision describes a charge this
///   store does not hold and is refused (`INVALID_CHARGE_REVISION`) rather than
///   written over, and an element that states no revision at all states no
///   precondition and is refused (`MISSING_CHARGE_REVISION`) rather than applied
///   blind. The model marks only `chargeId` `@required`
///   (`smithy_query(query="shape", shape="PurchaseOrder")`), but it publishes a
///   dedicated reason for each of these members being absent
///   (`MISSING_CHARGE_REVISION`, `MISSING_PURCHASE_ORDER_REFERENCE`): an
///   association is made of the charge, the revision it is at and the reference
///   to record, so every one of the three has to be stated;
/// * where it states `agreementId` at all — the member is optional — the value
///   is the agreement holding the charge the SAME element names. That member
///   restates which agreement the association belongs to, and the agreement is
///   resolved from the charge rather than named by the request, so any other
///   value contradicts the element's own `chargeId` and is refused
///   (`INVALID_AGREEMENT_ID`) rather than ignored. A charge no agreement holds
///   yet ([`ChargeAssociationTarget::agreement_id`] is `None`, which is every
///   charge at the door that MINTS the agreement they will belong to) is
///   contradicted by every value a caller can state, since the agreement does
///   not exist when the request is made.
///
/// Each refusal names the element that carried the offending member and the
/// member of it that failed ([`purchase_order_member_path`]), so a caller with
/// several orders can tell WHICH one was rejected, and the member-derived
/// reasons come from that member through [`member_reason`] — so a rule this
/// service enforces is reported under the same reason the boundary validator
/// reports a modelled constraint on the same member under.
pub fn validate_purchase_orders(
    orders: &[PurchaseOrder],
    charges: &BTreeMap<String, ChargeAssociationTarget>,
) -> Result<(), PurchaseOrderRefusal> {
    let mut associated: BTreeSet<&str> = BTreeSet::new();
    for (index, order) in orders.iter().enumerate() {
        let charge_path = purchase_order_member_path(index, "chargeId");
        let Some(target) = charges.get(order.charge_id.as_str()) else {
            return Err(PurchaseOrderRefusal {
                reason: member_reason(&charge_path, RefusalKind::Invalid),
                path: charge_path,
            });
        };
        if !associated.insert(order.charge_id.as_str()) {
            return Err(PurchaseOrderRefusal {
                reason: REASON_DUPLICATE_CHARGES.to_string(),
                path: charge_path,
            });
        }
        if order.purchase_order_reference.is_none() {
            let reference_path = purchase_order_member_path(index, "purchaseOrderReference");
            return Err(PurchaseOrderRefusal {
                reason: member_reason(&reference_path, RefusalKind::Missing),
                path: reference_path,
            });
        }
        // The precondition on the charge being written, in the two ways it can
        // fail: stated and wrong, or not stated at all. Both are refusals of the
        // same member, so both name its own path and differ only in the kind the
        // reason spells out.
        let revision_path = purchase_order_member_path(index, "chargeRevision");
        let Some(stated) = order.charge_revision else {
            return Err(PurchaseOrderRefusal {
                reason: member_reason(&revision_path, RefusalKind::Missing),
                path: revision_path,
            });
        };
        if stated != target.revision {
            return Err(PurchaseOrderRefusal {
                reason: member_reason(&revision_path, RefusalKind::Invalid),
                path: revision_path,
            });
        }
        // The other member that restates the state of the charge being written:
        // the agreement the charge belongs to. Naming the holder of some other
        // charge — or any agreement at all for a charge nothing holds yet —
        // contradicts this element's own `chargeId`, so the member is refused
        // rather than overwritten with the agreement the charge is actually of.
        if order
            .agreement_id
            .as_deref()
            .is_some_and(|named| target.agreement_id.as_deref() != Some(named))
        {
            let agreement_path = purchase_order_member_path(index, "agreementId");
            return Err(PurchaseOrderRefusal {
                reason: member_reason(&agreement_path, RefusalKind::Invalid),
                path: agreement_path,
            });
        }
    }
    Ok(())
}

/// A member an element has to state, as its name and the test of whether the
/// element states it.
pub(super) type RequiredMember = (&'static str, fn(&PurchaseOrder) -> bool);

/// The members an element of an `UpdatePurchaseOrders` list has to STATE, over
/// and above the ones the model constrains.
///
/// `PurchaseOrder` marks only `chargeId` `@required`
/// (`smithy_query(query="shape", shape="PurchaseOrder")`), but an update names
/// the association by the charge AND the agreement that holds it — "the unique
/// identifier of the agreement associated with this charge" — so an element
/// that leaves `agreementId` out names no association to update, and the
/// service publishes a dedicated reason for that member being absent
/// (`MISSING_AGREEMENT_ID`). The member is NOT required at the door that mints
/// the agreement (`AcceptAgreementRequest$purchaseOrders`), where the agreement
/// does not exist yet and no value a caller could state is its id, which is why
/// the requirement lives here rather than in [`validate_purchase_orders`].
pub(super) const UPDATE_PURCHASE_ORDER_REQUIRED_MEMBERS: &[RequiredMember] =
    &[("agreementId", |order| order.agreement_id.is_some())];

/// Refuse an `UpdatePurchaseOrders` list an element of which leaves out a member
/// the operation requires of every element
/// ([`UPDATE_PURCHASE_ORDER_REQUIRED_MEMBERS`]), BEFORE the request-level
/// refusal of the action and before anything is resolved against the store.
///
/// A member the caller left out is something the caller can repair, and the
/// modelled envelope is what says so: the `MISSING_<MEMBER>` reason the member
/// resolves to ([`member_reason`]) and one `fields` entry naming the element and
/// the member inside it ([`purchase_order_member_path`]), so a caller with a
/// long list learns WHICH element to fix. A refusal of the whole action names no
/// member at all ([`unsupported_action`]), so it cannot carry that information
/// and is answered only once every element states what it has to.
///
/// The walk is over the WHOLE list in the order the request sent it and reads
/// nothing but the request, so the answer does not depend on what the store
/// holds — an element naming a charge the store does not hold is still refused
/// on its own absent member, never on its `chargeId`.
pub fn update_purchase_orders_refusal(orders: &[PurchaseOrder]) -> Option<PurchaseOrderRefusal> {
    orders.iter().enumerate().find_map(|(index, order)| {
        UPDATE_PURCHASE_ORDER_REQUIRED_MEMBERS
            .iter()
            .find(|(_, stated)| !stated(order))
            .map(|(member, _)| {
                let path = purchase_order_member_path(index, member);
                PurchaseOrderRefusal {
                    reason: member_reason(&path, RefusalKind::Missing),
                    path,
                }
            })
    })
}
