Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 511f9d11774345a530a86fa6799cabc26a1e1916

# A purchaseOrders element's agreementId is silently overwritten by AcceptAgreementRequest, while the sibling applying "the same rule" refuses a contradicting value

_Filed: 2026-10-29T09:17:50Z_
_Operation: AcceptAgreementRequest | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `511f9d11774345a530a86fa6799cabc26a1e1916`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PurchaseOrder$agreementId` is an optional member a caller may state on each element of `AcceptAgreementRequestInput$purchaseOrders`. The handler neither honours nor checks it: after the shared validator runs, every element is rewritten with `agreement_id: Some(id.clone())` — the id of the agreement this call is minting — so whatever the caller stated is dropped on the floor and the call answers 200.

The crate's other door for the same list, `UpdatePurchaseOrders`, states that it applies "the same rule `AcceptAgreementRequest` refuses its own `purchaseOrders` under" and then adds an explicit check on exactly this member, calling it "THE RULE THIS LOOP IS FOR": an `agreementId` that is not the agreement holding the named charge "contradicts the charge the same element names, so the member is refused rather than ignored" (`ValidationException`, reason `INVALID_AGREEMENT_ID`, `fields[0].name = purchaseOrders.N.member.agreementId`). So one member of one shape, validated by one shared helper, is refused at one door of this crate and silently discarded at the other — and at the accept door EVERY stated value is necessarily wrong, because the agreement being associated has not been minted yet, which is precisely the condition the sibling refuses.

## Evidence

AcceptAgreementRequest drops the stated value — `src/ops/accept_agreement_request.rs:321-329`:
```
            purchase_orders: purchase_orders
                .into_iter()
                .map(|order| PurchaseOrder {
                    // "The unique identifier of the agreement associated with
                    // this charge": the agreement being accepted.
                    agreement_id: Some(id.clone()),
                    ..order
                })
                .collect(),
```
Its own doc enumerates every refusal it applies and `agreementId` is not among them — `:216-220`: "A list that names a charge the request does not hold, names one twice, carries no reference to associate, or states no `chargeRevision`, or one other than the revision its charge is at, is refused before anything is stored — see `crate::state::validate_purchase_orders` for the rule."

The shared validator indeed never looks at `agreementId` — `src/state.rs:3764-3794` lists the four conditions (`INVALID_CHARGE_ID`, duplicate charges, `MISSING_PURCHASE_ORDER_REFERENCE`, `MISSING/INVALID_CHARGE_REVISION`) and `:3802-3846` checks only `charge_id`, duplication, `purchase_order_reference` and `charge_revision`.

The sibling refuses it — `src/ops/update_purchase_orders.rs:177-182` ("The same rule `AcceptAgreementRequest` refuses its own `purchaseOrders` under, applied to the charges of every agreement this call can reach") followed by `:220-231`:
```
220            // THE RULE THIS LOOP IS FOR: "the unique identifier of the agreement
221            // associated with this charge" — an element may state it, and a
222            // value that is not the agreement holding the charge contradicts the
223            // charge the same element names, so the member is refused rather
224            // than ignored.
225            if order
226                .agreement_id
227                .as_deref()
228                .is_some_and(|named| named != agreement_id)
229            {
230                return Err(refuse(index, "agreementId"));
231            }
```
That refusal is pinned for the update door only — `tests/all/purchase_order_updates.rs:402-423` expects `field: "purchaseOrders.1.member.agreementId"` / `"purchaseOrders.2.member.agreementId"` with reason `INVALID_AGREEMENT_ID`, and its header at `:20-21` states the rule ("or states an `agreementId` that is not …"). No accept-door test states the member at all: `tests/all/purchase_order_associations.rs:236,364` and `tests/all/purchase_order_required_members.rs:266,278` build elements from `chargeId`/`chargeRevision`/`purchaseOrderReference` only, so the current silent-overwrite behaviour is unpinned.

## Steps to Reproduce

1. CreateAgreementRequest with intent NEW and a prepaid term, and keep the returned agreementRequestId plus one chargeSummary.expectedCharges[].id.
2. POST AcceptAgreementRequest {"agreementRequestId": <id>, "purchaseOrders": [{"chargeId": <charge>, "chargeRevision": 1, "purchaseOrderReference": "PO-1", "agreementId": "agmt-ffffffffffffffff"}]}.
3. Observed: HTTP 200; the stated agreementId is discarded and the association is stored under the newly minted agreement id.
4. Issue the same element against the sibling door — UpdatePurchaseOrders {"purchaseOrders": [{"chargeId": <charge>, "chargeRevision": 1, "purchaseOrderReference": "PO-1", "agreementId": "agmt-ffffffffffffffff"}]} — and observe 400 ValidationException, reason INVALID_AGREEMENT_ID, fields[0].name purchaseOrders.1.member.agreementId.
5. The same member with the same contradicting value is refused at one door of the crate and silently dropped at the other.
