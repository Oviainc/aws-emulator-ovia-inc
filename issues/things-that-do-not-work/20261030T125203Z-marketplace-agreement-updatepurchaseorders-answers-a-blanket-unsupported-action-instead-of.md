Issue-Source: trace-sweep-divergence
Issue-Trace: marketplace-agreement-charges-invoices-and-purchase-orders
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: marketplace-agreement-charges-invoices-and-purchase-orders
Issue-Shape-Id: com.amazonaws.marketplaceagreement#AWSMPCommerceService_v20200301
Issue-Key: marketplace-agreement|UpdatePurchaseOrders|4xx->4xx|$.fields
Issue-Related: 20261029T201646Z-marketplace-agreement-validationexception-bodies-diverge-in-field-message-text-field.md
Issue-Commit: 73df09ca417a0b7206ba1e112ed84e03c7401ae7

# UpdatePurchaseOrders answers a blanket UNSUPPORTED_ACTION instead of per-entry MISSING_AGREEMENT_ID validation

> _Note: this issue was identified against the code base at commit `73df09ca417a0b7206ba1e112ed84e03c7401ae7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdatePurchaseOrders

## Description
One root cause in the marketplace-agreement (AWSMPCommerceService_v20200301, awsJson1_0) emulator, grounded in one replayed wire fixture step. Model consulted: services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/smithy-model/com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301.json.

=== Cause 1 of 1: a purchaseOrders entry missing agreementId is refused with the wrong ValidationException reason/message and with no `fields` detail ===

AWS: Smithy: UpdatePurchaseOrders(purchaseOrders: PurchaseOrders @required, @length min 1 max 86) -> UpdatePurchaseOrdersOutput; errors ValidationException(400), ConflictException(409), ResourceNotFoundException(404), AccessDeniedException(403), ThrottlingException(429), InternalServerException(500). PurchaseOrder members: chargeId @required (ResourceId, len 1..64, pattern ^[A-Za-z0-9_/-]+$), chargeRevision (long, min 1), agreementId (ResourceId, same constraints, NOT marked @required in the model), purchaseOrderReference (len min 1). ValidationException carries members message, reason (enum ValidationExceptionReason) and fields (list of ValidationExceptionField{name @required, message @required}); ValidationExceptionReason includes both MISSING_AGREEMENT_ID and UNSUPPORTED_ACTION as distinct values.

That agreementId is nonetheless mandatory on every purchase-order entry is recorded behavior only (the model leaves it optional): the recording shows a 400 ValidationException whose reason is MISSING_AGREEMENT_ID, whose message is the fixed text 'Agreement id is required.', and whose `fields` list carries one entry per offending member — fixed message text 'Member is required.' with a `name` built from the request: the input member path to the offending member, i.e. 'purchaseOrders.<index>.member.agreementId', where <index> is the position of the entry in the submitted purchaseOrders list (the recording shows '.1.' for the entry it sent; the index is a value from the request, not part of the template). The requirement is the pairing: a per-member omission is reported as reason MISSING_AGREEMENT_ID with the member-specific message and a populated `fields` list locating the member.

Emulator: the same step is answered 400 (status matches) but as a generic account-level refusal: reason 'UNSUPPORTED_ACTION', message 'This action is not supported.', and `fields` absent entirely (null). So the handler never inspects the individual purchaseOrders entries for a missing agreementId and instead short-circuits every size-compliant request to one hard-coded unsupported-action refusal. Note the related open issue 20261022T212948Z-marketplace-agreement-intent-enum-and-proposal-validation-skipped.md (Cause 4) describes an EARLIER state of this handler, where it answered the unmodeled 'NotImplemented'; it does not cover and does not carry the replay key for this wire divergence, so the per-entry validation gap is filed here.

Why: a client cannot tell a malformed request from an unsupported one. 'UNSUPPORTED_ACTION' tells the caller its account/agreement is not eligible for purchase-order association — a non-retryable, non-fixable condition — so a caller that merely forgot agreementId on one entry will abandon the call instead of correcting the entry. Any caller that reads ValidationException.fields to point the user at the bad list element (the normal pattern for this batch-shaped operation) gets nothing, because the member is null, and reason-keyed branching on MISSING_AGREEMENT_ID never fires.

Fix: validation for UpdatePurchaseOrders — validate each submitted PurchaseOrder before the account-eligibility/unsupported-action path, and emit the modeled ValidationException with reason MISSING_AGREEMENT_ID, the member-level message, and a `fields` entry naming the offending member path; reserve UNSUPPORTED_ACTION for the genuinely unsupported case. The ValidationException serializer must also populate `fields`/`reason` here rather than dropping them.

Traces: marketplace-agreement-charges-invoices-and-purchase-orders#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, 35 steps, 27 passed, 8 mismatched, score 77.1%).

[mismatch] fixture marketplace-agreement-charges-invoices-and-purchase-orders, step task#3, operation UpdatePurchaseOrders
  Expected status 400, actual status 400 (status matches; body diverges):
    $.reason:  expected 'MISSING_AGREEMENT_ID', got 'UNSUPPORTED_ACTION'
    $.message: expected 'Agreement id is required.', got 'This action is not supported.'
    $.fields:  expected [{'name': 'purchaseOrders.1.member.agreementId', 'message': 'Member is required.'}], got None (member absent)

  Fixed text vs request-derived values in the expected body: 'Agreement id is required.' and 'Member is required.' are fixed text for this error class; in the field name 'purchaseOrders.1.member.agreementId' the leading member name, the literal '.member.' segment and the trailing member name are fixed, while the index '1' is taken from the position of the offending entry in the request's purchaseOrders list. __type is the modeled ValidationException and the HTTP status is 400 in both.

  Model: ValidationExceptionReason enumerates MISSING_AGREEMENT_ID and UNSUPPORTED_ACTION separately; ValidationException.fields is a list of ValidationExceptionField{name,message} (both @required); PurchaseOrder.agreementId is present but not @required in the model, so the mandatory-per-entry rule is recorded behavior only.

  Divergence key: service=marketplace-agreement|UpdatePurchaseOrders|status=400->400|keys=$.fields,$.message,$.reason (cluster size 1, owned_by empty, no stub_path).

## Steps to Reproduce
Replay the wire fixture directly:
  1. Boot the emulator from this checkout.
  2. Replay fixture 'marketplace-agreement-charges-invoices-and-purchase-orders' and stop at step 'task#3'.

Equivalently, by hand against the emulator's marketplace-agreement endpoint (us-east-1, awsJson1_0, target AWSMPCommerceService_v20200301.UpdatePurchaseOrders):
  1. Create/obtain an agreement and one of its charges, as the fixture's earlier steps do.
  2. Call UpdatePurchaseOrders with a size-compliant purchaseOrders list (1..86 entries) in which an entry supplies chargeId, chargeRevision and purchaseOrderReference but OMITS agreementId, e.g.
       mpa.update_purchase_orders(purchaseOrders=[{'chargeId': '<charge-id>', 'chargeRevision': 1, 'purchaseOrderReference': 'PO-1'}])
  3. Observed: HTTP 400 ValidationException with reason 'UNSUPPORTED_ACTION', message 'This action is not supported.', and no 'fields' member.
     Expected: HTTP 400 ValidationException with reason 'MISSING_AGREEMENT_ID', message 'Agreement id is required.', and fields=[{'name': 'purchaseOrders.<index>.member.agreementId', 'message': 'Member is required.'}] where <index> is the position of the offending entry in the submitted list.
  4. Control: an entry that DOES supply a resolvable agreementId must not be refused with MISSING_AGREEMENT_ID.
