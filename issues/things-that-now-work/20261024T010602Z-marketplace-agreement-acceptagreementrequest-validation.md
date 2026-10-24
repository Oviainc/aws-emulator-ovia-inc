Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# purchaseOrders are stored without resolving chargeId against the quote's charges, and duplicate charges are accepted

_Filed: 2026-10-24T01:06:02Z_
_Operation: AcceptAgreementRequest | Class: validation_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

AcceptAgreementRequest takes purchaseOrders, each a PurchaseOrder whose @required chargeId is "The unique identifier of the charge to associate the purchase order with". Expected: a chargeId that names no charge of the agreement being accepted is refused with 400 ValidationException (ValidationExceptionReason models INVALID_CHARGE_ID, MISSING_CHARGE_ID, INVALID_PURCHASE_ORDERS, DUPLICATE_CHARGES, INVALID_CHARGE_REVISION, MISSING_PURCHASE_ORDER_REFERENCE for exactly these members), and two purchase orders naming the SAME chargeId are refused (DUPLICATE_CHARGES). Actual: src/ops/accept_agreement_request.rs copies every supplied PurchaseOrder into the new agreement verbatim (only overwriting agreementId with the minted id) and performs no resolution at all: an arbitrary chargeId such as "ch-1" — which is not one of the ids the quote minted into ChargeSummary$expectedCharges and persisted on the agreement request (state::charge_summary mints ch-%016x ids) — is accepted with 200, as is a list repeating one chargeId, as is an entry with no purchaseOrderReference (nothing to associate). The emulator already holds everything needed to validate: AgreementRequest.charge_summary.expected_charges carries the charge ids for the request being accepted. This fails open: the association is persisted against a charge that does not exist, and the modeled charge-related ValidationException reasons are unreachable on this operation.

## Evidence

Spec: PurchaseOrder$chargeId (@required, "The unique identifier of the charge to associate the purchase order with"), PurchaseOrder$purchaseOrderReference, ChargeSummary$expectedCharges/ExpectedCharge$id, and ValidationExceptionReason values INVALID_CHARGE_ID / MISSING_CHARGE_ID / DUPLICATE_CHARGES / INVALID_PURCHASE_ORDERS / MISSING_PURCHASE_ORDER_REFERENCE in the Smithy model. Handler: src/ops/accept_agreement_request.rs lines ~257-267 (`purchase_orders: input.purchase_orders.unwrap_or_default().into_iter().map(|order| PurchaseOrder { agreement_id: Some(id.clone()), ..order }).collect()`) — no lookup of chargeId, no duplicate check, no reference check. State already stores the quote's charge ids (src/state.rs charge_summary()/mint_charge_id(), AgreementRequest.charge_summary). Test coverage: tests/all/agreement_lifecycle.rs line 781 accepts with purchaseOrders=[{chargeId:"ch-1", purchaseOrderReference:"PO-77"}] and asserts 200, i.e. the lenient behavior is asserted; no test asserts chargeId resolution, duplicate rejection, or the reference requirement.

## Steps to Reproduce

1. CreateAgreementRequest(intent=NEW, agreementProposalIdentifier="ap-offer1", requestedTerms=[{id:"cfg", configuration:{configurableUpfrontPricingTermConfiguration:{selectorValue:"P12M", dimensions:[{dimensionKey:"Users", dimensionValue:1}]}}}]) and note chargeSummary.expectedCharges[].id (e.g. "ch-0000000000000002")
2. AcceptAgreementRequest(agreementRequestId=<id>, purchaseOrders=[{chargeId:"ch-1", purchaseOrderReference:"PO-77"}]) -> observe 200 even though ch-1 is not a charge of this agreement request (expected 400 ValidationException, reason INVALID_CHARGE_ID / INVALID_PURCHASE_ORDERS)
3. Repeat with purchaseOrders=[{chargeId:<real id>, purchaseOrderReference:"PO-1"},{chargeId:<same real id>, purchaseOrderReference:"PO-2"}] -> observe 200 (expected 400 ValidationException, reason DUPLICATE_CHARGES)
4. Repeat with purchaseOrders=[{chargeId:<real id>}] (no purchaseOrderReference) -> observe 200 with an association that references nothing (expected 400 ValidationException, reason MISSING_PURCHASE_ORDER_REFERENCE)
