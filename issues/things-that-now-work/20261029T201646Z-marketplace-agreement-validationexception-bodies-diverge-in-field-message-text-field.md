Issue-Source: trace-sweep-divergence
Issue-Trace: marketplace-agreement-agreement-payment-request-workflow
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: marketplace-agreement-agreement-payment-request-workflow marketplace-agreement-validation-error-paths marketplace-agreement-charges-invoices-and-purchase-orders marketplace-agreement-create-and-accept-agreement-request
Issue-Shape-Id: com.amazonaws.marketplaceagreement#AWSMPCommerceService_v20200301
Issue-Key: marketplace-agreement|SendAgreementPaymentRequest|4xx->4xx|$.fields[*].message
Issue-Key: marketplace-agreement|ListAgreementCancellationRequests|4xx->4xx|$.fields[name="partyType"].message
Issue-Key: marketplace-agreement|UpdatePurchaseOrders|4xx->4xx|$.fields[*]
Issue-Key: marketplace-agreement|UpdatePurchaseOrders|4xx->4xx|$.message
Issue-Key: marketplace-agreement|UpdatePurchaseOrders|4xx->4xx|$.reason
Issue-Key: marketplace-agreement|CreateAgreementRequest|4xx->4xx|$.fields[*].message
Issue-Key: marketplace-agreement|CreateAgreementRequest|4xx->4xx|$.message
Issue-Commit: 0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f

# ValidationException bodies diverge in field message text, field name and check order

> _Note: this issue was identified against the code base at commit `0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendAgreementPaymentRequest

## Description
The crate builds `ValidationException` bodies whose per-field and top-level messages are not AWS's wording, and in one operation it runs its constraint checks in an order that reports a different member and a different `reason` than AWS does. Four causes; each must be closed before this issue is submitted.

Cause 1 of 4: the per-field `Member is invalid` text carries a trailing period (SendAgreementPaymentRequest).
AWS: `ValidationExceptionField` requires `name` and `message` (Smithy: ValidationExceptionField.name/.message, both @required, BoundedString @length 1..4096). For a member that is present but fails its constraint, the fixed field message is exactly `Member is invalid` — no trailing period. (Contrast the sibling class for an absent member, whose fixed text IS punctuated: `Member is required.`, as the UpdatePurchaseOrders recording in Cause 3 shows. The two texts differ in punctuation by design.) The variable part of the entry is `name`, the rejected input member's path taken from the request.
Emulator: emits `Member is invalid.` with a trailing period in `fields[0].message` while `name`, `reason`, `message` and status all match.
Why: the field message is the string clients and consoles surface to the user, and test suites and golden-file comparisons assert on it exactly; an extra character makes every such assertion fail and makes the emulator's bodies non-interchangeable with AWS's.
Fix: the crate's shared validation-field constructor for the invalid-member class must emit the unpunctuated fixed text `Member is invalid`, leaving the punctuated `Member is required.` for the absent-member class.
Traces: marketplace-agreement-agreement-payment-request-workflow#task#3.

Cause 2 of 4: the same unpunctuated/punctuated confusion on ListAgreementCancellationRequests.partyType.
AWS: `partyType` is a required member targeting `PartyType` with @length 1..32 and @pattern ^[A-Za-z]+$ (Smithy: ListAgreementCancellationRequests.partyType). A present-but-unacceptable value yields 400 `ValidationException` whose `fields` entry for `partyType` carries the fixed text `Member is invalid`, with `name` taken from the request.
Emulator: `fields[name="partyType"].message` is `Member is invalid.` with a trailing period; everything else on the response matches.
Why: as Cause 1 — the user-visible explanation of which member was rejected does not match AWS byte for byte.
Fix: same shared validation-field constructor; this handler reaches it through the list-operation input validation.
Traces: marketplace-agreement-validation-error-paths#task#2.

Cause 3 of 4: UpdatePurchaseOrders checks a per-element chargeId before the element's required agreementId, so it reports the wrong member and the wrong reason.
AWS: `purchaseOrders` is required, @length 1..86, of `PurchaseOrder` (Smithy: UpdatePurchaseOrders.purchaseOrders; PurchaseOrder.chargeId @required, .agreementId, both ResourceId @pattern ^[A-Za-z0-9_/-]+$). Recorded behavior: when an element is missing the agreement identifier the service reports the absent member first — 400 `ValidationException` with `reason` = `MISSING_AGREEMENT_ID`, `message` = the fixed text `Agreement id is required.`, and a single `fields` entry {name: the element's member path, message: `Member is required.`}. The `name` value is built from the request: the list member name, the zero-based element index and the member within the element (in this recording `purchaseOrders.1.member.agreementId`); index and member name are the variable parts, the `purchaseOrders.<i>.member.<member>` shape is the template.
Emulator: answers 400 but reports a different failure entirely — `reason` `INVALID_CHARGE_ID`, `message` `Provided charge id is invalid.`, `fields` = [{name: `purchaseOrders.1.member.chargeId`, message: `Member is invalid.`}]. It evaluates the element's chargeId against stored charges before it enforces the element's required agreement identifier, so a request with two independent defects is reported against the wrong member with a reason the client cannot act on.
Why: a client reads `fields[].name` and `reason` to decide which member of which element to fix. Pointed at `chargeId` when the actual missing member is the element's `agreementId`, it corrects the wrong field, resubmits, and is rejected again — and a client that treats `INVALID_CHARGE_ID` as a resource problem abandons the charge instead of completing its request.
Fix: UpdatePurchaseOrders' input validation must enforce the per-element required/absent-member checks across the whole list before it resolves any element's chargeId against state, so a missing agreement identifier surfaces as MISSING_AGREEMENT_ID with the absent-member field text.
Traces: marketplace-agreement-charges-invoices-and-purchase-orders#task#3.

Cause 4 of 4: CreateAgreementRequest uses its own wording for the conditionally-required agreementProposalIdentifier.
AWS: `intent` is a required enum of NEW/AMEND/REPLACE and `agreementProposalIdentifier` targets `AgreementProposalId` @pattern ^(at-|ap-)[A-Za-z0-9]+$ (Smithy: CreateAgreementRequest.intent/.agreementProposalIdentifier); the proposal identifier is required for the stated intent, and `ValidationExceptionReason` carries `MISSING_AGREEMENT_PROPOSAL_IDENTIFIER` for its absence. Recorded behavior: 400 `ValidationException` with top-level `message` = the fixed text `Agreement proposal identifier is required.` and a `fields` entry whose fixed text is `Member must not be null for the provided intent.` — wording that names the intent-conditional rule, with `name` taken from the request.
Emulator: top-level `message` is `Provided agreement proposal identifier is missing.` and the field message is `Member must not be null.`, dropping the `for the provided intent` clause. Status, reason and field name match.
Why: the emulator's field text states an unconditional null check, so a caller (or a generated test) cannot tell that the member became required only because of the `intent` it sent, and exact-match assertions on either string fail.
Fix: CreateAgreementRequest's input validation must emit the intent-conditional fixed texts — top-level `Agreement proposal identifier is required.` and field message `Member must not be null for the provided intent.`
Traces: marketplace-agreement-create-and-accept-agreement-request#task#1.

Open issue 20261022T212948Z-marketplace-agreement-intent-enum-and-proposal-validation-skipped.md discusses intent/proposal validation and 20261029T014620Z-marketplace-agreement-batchcreatebillingadjustmentrequest-error-response.md discusses a missing ValidationExceptionReason, but neither carries any of the replay keys above and neither covers these message texts or the UpdatePurchaseOrders check order.

## Evidence
Replay of trace set simparity-wire-aws-v1 (all four steps verdict mismatch):

Cause 1 — fixture marketplace-agreement-agreement-payment-request-workflow step task#3, SendAgreementPaymentRequest, expected 400 / actual 400:
  $.fields[0].message: expected 'Member is invalid', got 'Member is invalid.'

Cause 2 — fixture marketplace-agreement-validation-error-paths step task#2, ListAgreementCancellationRequests, expected 400 / actual 400:
  $.fields[name="partyType"].message: expected 'Member is invalid', got 'Member is invalid.'

Cause 3 — fixture marketplace-agreement-charges-invoices-and-purchase-orders step task#3, UpdatePurchaseOrders, expected 400 / actual 400:
  $.message: expected 'Agreement id is required.', got 'Provided charge id is invalid.'
  $.reason: expected 'MISSING_AGREEMENT_ID', got 'INVALID_CHARGE_ID'
  $.fields[0]: expected {'name': 'purchaseOrders.1.member.agreementId', 'message': 'Member is required.'}, got {'name': 'purchaseOrders.1.member.chargeId', 'message': 'Member is invalid.'}

Cause 4 — fixture marketplace-agreement-create-and-accept-agreement-request step task#1, CreateAgreementRequest, expected 400 / actual 400:
  $.message: expected 'Agreement proposal identifier is required.', got 'Provided agreement proposal identifier is missing.'
  $.fields[0].message: expected 'Member must not be null for the provided intent.', got 'Member must not be null.'

Fixed vs variable text: 'Member is invalid', 'Member is required.', 'Member must not be null for the provided intent.', 'Agreement id is required.' and 'Agreement proposal identifier is required.' are fixed texts of their error classes. The variable parts are the fields[].name paths, built from the request (member name, and for a list the element index and inner member, as in purchaseOrders.<index>.member.<member>). No per-run value is involved.

Model support: ValidationExceptionField.name/.message @required; ValidationExceptionReason includes MISSING_AGREEMENT_ID, INVALID_CHARGE_ID and MISSING_AGREEMENT_PROPOSAL_IDENTIFIER; PurchaseOrder.chargeId @required with agreementId optional in the model; CreateAgreementRequest.intent @required enum NEW/AMEND/REPLACE.

## Steps to Reproduce
1. Boot the emulator from the mainline checkout.
2. Cause 1: replay marketplace-agreement-agreement-payment-request-workflow step task#3 — SendAgreementPaymentRequest with a present-but-unacceptable member. The fields[0].message comes back as 'Member is invalid.' instead of 'Member is invalid'.
3. Cause 2: replay marketplace-agreement-validation-error-paths step task#2 — ListAgreementCancellationRequests with a partyType value that violates PartyType @pattern ^[A-Za-z]+$. The partyType field message comes back as 'Member is invalid.' instead of 'Member is invalid'.
4. Cause 3: replay marketplace-agreement-charges-invoices-and-purchase-orders step task#3 — UpdatePurchaseOrders with a purchaseOrders list whose second element (index 1) omits agreementId and whose chargeId does not resolve. The response reports reason INVALID_CHARGE_ID / message 'Provided charge id is invalid.' / field purchaseOrders.1.member.chargeId 'Member is invalid.' instead of reason MISSING_AGREEMENT_ID / message 'Agreement id is required.' / field purchaseOrders.1.member.agreementId 'Member is required.'.
5. Cause 4: replay marketplace-agreement-create-and-accept-agreement-request step task#1 — CreateAgreementRequest with an intent that requires agreementProposalIdentifier and that member omitted. The message comes back as 'Provided agreement proposal identifier is missing.' with field message 'Member must not be null.' instead of 'Agreement proposal identifier is required.' with 'Member must not be null for the provided intent.'.
