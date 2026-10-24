Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 1f3e823b77ae5163683e789c9c079862bba02cf9

# PurchaseOrder$chargeRevision is never validated against the charge's revision

_Filed: 2026-10-24T19:28:26Z_
_Operation: UpdatePurchaseOrders | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `1f3e823b77ae5163683e789c9c079862bba02cf9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PurchaseOrder` carries `chargeRevision` ("The revision of the charge", `@range(min: 1)`), and the charge it points at carries `Charge$revision`. The emulator never reads the member: `validate_purchase_orders` checks only that `chargeId` resolves, is not repeated in the list, and that `purchaseOrderReference` is present; the handler then stores the element with `..order.clone()`, so whatever `chargeRevision` the caller sent is kept but never compared with the charge and never surfaces anywhere. `Charge$revision` is always reported as the constant `INITIAL_CHARGE_REVISION`.

Expected: an association naming a `chargeRevision` that is not the charge's current revision is refused with the modeled `ValidationException` and `reason = INVALID_CHARGE_REVISION` (the model also carries `MISSING_CHARGE_REVISION`), which is the optimistic-concurrency check the member exists for.
Actual: HTTP 200 for any `chargeRevision` — e.g. 99 against a charge whose revision is 1 — and the association is applied.

The same shared helper is used by `AcceptAgreementRequest$purchaseOrders`, so a purchase order supplied at accept time is accepted with an arbitrary revision too.

## Evidence

Spec: `smithy_query(query="shape", shape="PurchaseOrder")` — members `chargeId` (required), `chargeRevision` (ChargeRevision, range min 1), `agreementId`, `purchaseOrderReference`; `smithy_query(query="shape", shape="Charge")` — `revision`; `ValidationExceptionReason` models `INVALID_CHARGE_REVISION` and `MISSING_CHARGE_REVISION`.
Handler: `src/ops/update_purchase_orders.rs` lines 170-239 — resolves `chargeId` to its agreement, refuses a contradicting `agreementId`, and plans the write; `chargeRevision` is never mentioned. `crate::state::validate_purchase_orders` (src/state.rs 2102-2130) checks chargeId existence, duplicate chargeId and missing purchaseOrderReference only. `crate::state::charges_of` (src/state.rs ~2831) always reports `revision: Some(INITIAL_CHARGE_REVISION)`.
Tests: `tests/all/purchase_order_updates.rs` / `purchase_order_associations.rs` exercise chargeId/reference refusals; no test sends or asserts anything about `chargeRevision`.

## Steps to Reproduce

1. CreateAgreementRequest + AcceptAgreementRequest, then ListAgreementCharges to get a chargeId and its revision (reported as 1).
2. POST X-Amz-Target: AWSMPCommerceService_v20200301.UpdatePurchaseOrders with {"purchaseOrders":[{"chargeId":"<ch-...>","chargeRevision":99,"purchaseOrderReference":"PO-1"}]}.
3. Observe HTTP 200 and ListAgreementCharges showing purchaseOrderReference "PO-1" on a charge whose revision is 1; expected HTTP 400 ValidationException with reason INVALID_CHARGE_REVISION.
4. Repeat with AcceptAgreementRequest's purchaseOrders member for the same result.
