Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: dc6b9d697210888fba6fd46cd7393ea22255dcd4

# chargeRevision is neither required nor matched against the charge's revision, so a stale or bogus revision is accepted

_Filed: 2026-10-24T12:48:09Z_
_Operation: UpdatePurchaseOrders | Class: validation_

> _Note: this issue was identified against the code base at commit `dc6b9d697210888fba6fd46cd7393ea22255dcd4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

PurchaseOrder$chargeRevision ("The revision of the charge", @range(min:1)) is the concurrency token of a purchase-order association, and the service models dedicated refusals for it: ValidationExceptionReason includes both INVALID_CHARGE_REVISION and MISSING_CHARGE_REVISION. Expected: an element whose chargeRevision does not match the charge's current revision (ListAgreementCharges reports it) is refused with 400 ValidationException reason INVALID_CHARGE_REVISION naming that element's path, and an element that omits chargeRevision altogether is refused with MISSING_CHARGE_REVISION.

Actual: crate::state::validate_purchase_orders() checks only (a) that chargeId names one of the agreement's charges, (b) that no charge is named twice, and (c) that purchaseOrderReference is present. chargeRevision is never inspected: src/ops/update_purchase_orders.rs copies the element through (`PurchaseOrder { agreement_id: Some(...), ..order.clone() }`) and stores whatever revision the caller sent (or none), while every charge this store holds reports revision 1 (state::INITIAL_CHARGE_REVISION). So UpdatePurchaseOrders(chargeRevision=99) and UpdatePurchaseOrders with no chargeRevision at all both answer 200 and associate the reference. The same helper is used by AcceptAgreementRequest's purchaseOrders list, so the same input is accepted there.

## Evidence

Spec: PurchaseOrder$chargeRevision in the Smithy model (@range min 1, "The revision of the charge"); ValidationExceptionReason values INVALID_CHARGE_REVISION and MISSING_CHARGE_REVISION, which exist only because the service refuses these two cases; Charge$revision reported by ListAgreementCharges. Handler: src/ops/update_purchase_orders.rs (validate_purchase_orders + plan loop, which only cross-checks agreementId) and src/state.rs validate_purchase_orders() / INITIAL_CHARGE_REVISION. Tests: tests/all/purchase_order_associations.rs:310-320 accepts an element with chargeRevision 1 (the matching value) and tests/all/purchase_order_updates.rs:383 passes chargeRevision 2 only inside a case refused for a MISSING_PURCHASE_ORDER_REFERENCE — no test asserts a mismatching or missing revision is refused.

## Steps to Reproduce

1. CreateAgreementRequest + AcceptAgreementRequest to mint an agreement with at least one charge; note the chargeId and its revision from ListAgreementCharges (revision 1)
2. UpdatePurchaseOrders(purchaseOrders=[{chargeId:<charge>, chargeRevision: 99, purchaseOrderReference:"PO-1"}]) -> observe 200 and the association recorded (expected 400 ValidationException, reason INVALID_CHARGE_REVISION, field purchaseOrders.1.member.chargeRevision)
3. UpdatePurchaseOrders(purchaseOrders=[{chargeId:<charge>, purchaseOrderReference:"PO-2"}]) with no chargeRevision -> observe 200 (expected 400 ValidationException, reason MISSING_CHARGE_REVISION)
4. ListAgreementCharges(agreementId=<id>) -> the charge still reports revision 1 while carrying the reference associated under revision 99
