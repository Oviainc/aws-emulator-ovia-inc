Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: invoicing.list-invoice-units-asof-shows-historical-deleted-unit invoicing.update-invoice-unit-fields-and-last-modified invoicing.tag-resource-exceeds-quota-rejected invoicing.verify-procurement-portal-validation-wrong-code-rejected invoicing.create-procurement-portal-preference-duplicate-conflict-rejected invoicing.create-invoice-unit-client-token-idempotent
Issue-Shape-Id: com.amazonaws.invoicing#Invoicing
Issue-Key: invoicing.list-invoice-units-asof-shows-historical-deleted-unit|run#12|AssertionError|assert arn in historical_arns, "deleted unit should still appear in a pre-delete AsOf snapshot"
Issue-Key: invoicing.update-invoice-unit-fields-and-last-modified|run#36|AssertionError|assert any(u["InvoiceUnitArn"] == arn for u in mid_lifetime_listing), (
Issue-Key: invoicing.tag-resource-exceeds-quota-rejected|run#13|AssertionError|raise AssertionError("expected to hit a tag quota within 200 tags, but never did")
Issue-Key: invoicing.verify-procurement-portal-validation-wrong-code-rejected|run#6|AssertionError|assert send_err["Code"] == "ResourceNotFoundException", (
Issue-Key: invoicing.create-procurement-portal-preference-duplicate-conflict-rejected|run#5|AssertionError|assert bad_portal_err["Message"] == expected_portal_message, (
Issue-Key: invoicing|CreateInvoiceUnit|ValidationException
Issue-Commit: ead37dd00922f5e8ae35cfd4d29cc6e26f23d78c

# Invoicing: deleted invoice units vanish from AsOf history, per-resource tag quota unreachable, SendProcurementPortalValidation/VerifyProcurementPortalValidation answer 501, ProcurementPortalName enum set is stale, duplicate-name message diverges (5 causes)

> _Note: this issue was identified against the code base at commit `ead37dd00922f5e8ae35cfd4d29cc6e26f23d78c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInvoiceUnits

## Description
Five distinct root causes in the invoicing emulator, found by replaying the behavioral suite (9/21 passed). Each must be closed before this issue is done.

=== Cause 1 of 5: DeleteInvoiceUnit erases the unit from state, so a pre-delete AsOf snapshot no longer shows it ===
AWS: Smithy ListInvoiceUnits.AsOf (AsOfTimestamp) is documented in this service's own on-disk model as "The state of an invoice unit at a specified time. You can see legacy invoice units that are currently deleted if the AsOf time is set to before it was deleted. If an AsOf is not provided, the default value is the current time." So a ListInvoiceUnits whose AsOf is an instant at which the unit demonstrably existed (e.g. the server's own LastModified for that unit) MUST still report that unit after it has been deleted, while a listing with no AsOf must omit it.
Emulator: After DeleteInvoiceUnit, the unit is gone from point-in-time history entirely. A name-filtered ListInvoiceUnits with AsOf set to the unit's own server-reported LastModified (taken before the delete) returns an empty InvoiceUnits list — the check fails with "deleted unit should still appear in a pre-delete AsOf snapshot" and "a historical AsOf view between create and delete must still show the unit". The pre-creation AsOf leg passes, so AsOf is being honoured for creation but not for deletion.
Why: A client doing point-in-time invoice reconciliation (the stated purpose of AsOf) silently loses every unit that has since been deleted, and cannot distinguish "never existed" from "existed then, deleted later" — it will under-report historical billing scope with no error.
Fix: invoice-unit state plus the delete handler and the AsOf reader. State keeps no deletion tombstone: ComAmazonawsInvoicingInvoicingState.invoice_units is a live-only BTreeMap and StoredInvoiceUnit has created_at/current/superseded but no deleted_at, so StoredInvoiceUnit::version_as_of can never answer for a removed unit because the entry itself is dropped. The delete must retire the unit (record the deletion instant) rather than remove it, and ListInvoiceUnits/GetInvoiceUnit's AsOf walk must include units retired after the requested instant.
Traces: invoicing.list-invoice-units-asof-shows-historical-deleted-unit, invoicing.update-invoice-unit-fields-and-last-modified (step 9). Code: services/service-com-amazonaws-invoicing-invoicing/src/state.rs:28-47 (invoice_units), :147-183 (StoredInvoiceUnit, version_as_of); services/service-com-amazonaws-invoicing-invoicing/src/ops/list_invoice_units.rs:302-320; services/service-com-amazonaws-invoicing-invoicing/src/ops/delete_invoice_unit.rs.

=== Cause 2 of 5: the per-resource tag quota is set to the per-call array maximum, so TagResource never answers ServiceQuotaExceededException ===
AWS: recorded behavior only for the quota VALUE. The model gives the per-CALL bound (Smithy: TagResource.ResourceTags -> ResourceTagList @length(min:0,max:200)) and models the refusal (Smithy: TagResource errors include ServiceQuotaExceededException, httpError 402). The recording shows real AWS refusing the batch that pushes a resource past its per-RESOURCE tag quota with ServiceQuotaExceededException and the message "The request exceeds the maximum number of tags allowed per resource.", refusing it atomically (no tags from the rejected batch persist), and accepting the identical call again once headroom is freed by UntagResource. The per-resource quota is strictly lower than 200 — the recording reaches it while adding tags in batches of 10 and the whole loop is bounded at 200 tags.
Emulator: 200 tags are accepted on one invoice unit without any refusal; the check fails with "expected to hit a tag quota within 200 tags, but never did". The handler does enforce a cap, but the cap is MAX_RESOURCE_TAGS = 200, which equals the per-call @length maximum, making the quota unreachable by any legal sequence of calls. The refusal message it would emit is also emulator-authored ("A resource holds at most 200 tags; <arn> already carries N and the request adds M") rather than the recorded sentence.
Why: A client that relies on the quota as a back-pressure signal (and on the atomic all-or-nothing semantics of an over-quota TagResource) never sees 402/ServiceQuotaExceededException and instead accumulates tags the real service would have refused, so tag-budget handling is never exercised and the client diverges the moment it runs against AWS.
Fix: TagResource's quota check and the state constant it reads — services/service-com-amazonaws-invoicing-invoicing/src/state.rs:64 (MAX_RESOURCE_TAGS) and services/service-com-amazonaws-invoicing-invoicing/src/ops/tag_resource.rs:175-189. The per-resource quota must be independent of (and below) the per-call array maximum, and the 402's message should be the service's canned per-resource-tag sentence.
Traces: invoicing.tag-resource-exceeds-quota-rejected.

=== Cause 3 of 5: SendProcurementPortalValidation and VerifyProcurementPortalValidation answer 501 NotImplemented instead of the modeled ResourceNotFoundException ===
AWS: Smithy: SendProcurementPortalValidation and VerifyProcurementPortalValidation both declare ResourceNotFoundException (errorKind client, httpError 404). A well-formed ProcurementPortalPreferenceArn that names no existing preference is refused 404 ResourceNotFoundException by both operations; recorded message example: "Procurement portal preference with arn <arn> does not exist". Neither operation may succeed or fabricate validation state for a preference that does not exist.
Emulator: both operations return the scaffold's 501 with error code NotImplemented ("operation SendProcurementPortalValidation is scaffolded but not implemented in this emulator"). The check fails at the first assertion: expected ResourceNotFoundException, got 'NotImplemented'. Note the suite did NOT mark this check as blocked-by-an-unimplemented-operation (it surfaced as a mismatch because the check caught the ClientError), so no stub issue covers it.
Why: A client's not-found handling for the portal-validation flow is never exercised; it sees an unmodeled 501/NotImplemented, which botocore does not map to the modeled ResourceNotFoundException, so retry/branching logic takes the wrong path.
Fix: the two operations' handlers — services/service-com-amazonaws-invoicing-invoicing/src/ops/send_procurement_portal_validation.rs:157-175 (returns Scaffolded::not_implemented) and services/service-com-amazonaws-invoicing-invoicing/src/ops/verify_procurement_portal_validation.rs — resolve the ARN against procurement-portal-preference state and return the modeled 404 when it names nothing.
Traces: invoicing.verify-procurement-portal-validation-wrong-code-rejected.

=== Cause 4 of 5: the ProcurementPortalName enum value set is stale — ZIP and JAGGAER are rejected as invalid ===
AWS: recorded behavior only (the on-disk model is behind the live service). The recording shows real AWS refusing an unsupported ProcurementPortalName with 400 ValidationException whose message enumerates the accepted set as [ZIP, SAP_BUSINESS_NETWORK, JAGGAER, COUPA] — i.e. four supported portals.
Emulator: the same request is refused with the right code (ValidationException) but the accepted set is only [SAP_BUSINESS_NETWORK, COUPA]: "1 validation error detected: Value at 'procurementPortalName' failed to satisfy constraint: Member must satisfy enum value set: [SAP_BUSINESS_NETWORK, COUPA]". This is not a message-wording difference — it is the enforced value set: smithy_query(shape="ProcurementPortalName") on services/service-com-amazonaws-invoicing-invoicing/smithy-model/com-amazonaws-invoicing-invoicing.json returns values [SAP_BUSINESS_NETWORK, COUPA], so a caller passing ZIP or JAGGAER (both valid against real AWS) is refused by the emulator.
Why: Any client configuring a ZIP or JAGGAER procurement portal is rejected outright by the emulator for input real AWS accepts, and the enumerated set reported back to the operator is wrong.
Fix: the ProcurementPortalName enum in this service's Smithy model and whatever the request validator derives its accepted-value list from, so all four portal names are accepted and enumerated.
Traces: invoicing.create-procurement-portal-preference-duplicate-conflict-rejected.

=== Cause 5 of 5: CreateInvoiceUnit's duplicate-Name ValidationException message does not follow the service's template ===
AWS: recorded behavior only; the message rule is known because it is built from the request's own Name member — real AWS answers 400 ValidationException with "Invoice Unit already exists with name: <Name>" (the requested Name, in angle brackets). Code and status match; only the template diverges.
Emulator: the correct code (ValidationException, reason duplicateInvoiceUnit) with a differently-templated message: "An invoice unit named <Name> already exists in this account".
Why: A client or operator that matches the service's documented duplicate-name sentence (or parses the bracketed name out of it) does not recognise the emulator's refusal, and the name is not recoverable from the message by the same rule.
Fix: the duplicate-name branch of CreateInvoiceUnit's handler — services/service-com-amazonaws-invoicing-invoicing/src/ops/create_invoice_unit.rs:197-207 — emit the service's own template with the requested Name.
Traces: invoicing.create-invoice-unit-client-token-idempotent.

## Evidence
Suite: simparity-behavioral-aws-v1, invoicing 9/21 passed (43%), 12 failed.

Cause 1 — expected: name-filtered ListInvoiceUnits with AsOf = the unit's own pre-delete LastModified still contains the unit's ARN; actual: empty InvoiceUnits.
  AssertionError: deleted unit should still appear in a pre-delete AsOf snapshot (invoicing.list-invoice-units-asof-shows-historical-deleted-unit)
  AssertionError: a historical AsOf view between create and delete must still show the unit (invoicing.update-invoice-unit-fields-and-last-modified, step 9)

Cause 2 — expected: at some batch before 200 tags, TagResource -> HTTP 402 ServiceQuotaExceededException, message example "The request exceeds the maximum number of tags allowed per resource.", no tags from the rejected batch persisted, and the identical call succeeds after UntagResource frees headroom; actual: all 200 tags accepted, no refusal.
  AssertionError: expected to hit a tag quota within 200 tags, but never did (invoicing.tag-resource-exceeds-quota-rejected)

Cause 3 — expected: SendProcurementPortalValidation / VerifyProcurementPortalValidation on a well-formed but non-existent ProcurementPortalPreferenceArn -> HTTP 404, Code ResourceNotFoundException (message example "Procurement portal preference with arn <arn> does not exist"); actual: HTTP 501, Code NotImplemented, "operation SendProcurementPortalValidation is scaffolded but not implemented in this emulator".
  AssertionError: SendProcurementPortalValidation on a non-existent preference ARN should be rejected with ResourceNotFoundException, got 'NotImplemented' (invoicing.verify-procurement-portal-validation-wrong-code-rejected)

Cause 4 — expected: "1 validation error detected: Value at 'procurementPortalName' failed to satisfy constraint: Member must satisfy enum value set: [ZIP, SAP_BUSINESS_NETWORK, JAGGAER, COUPA]"; actual: "... enum value set: [SAP_BUSINESS_NETWORK, COUPA]". Corroborated statically: smithy_query(query="shape", shape="ProcurementPortalName") -> values [SAP_BUSINESS_NETWORK, COUPA].
  (invoicing.create-procurement-portal-preference-duplicate-conflict-rejected)

Cause 5 — expected ValidationException message 'Invoice Unit already exists with name: <sim2real-iu-XXXX>'; actual 'An invoice unit named sim2real-iu-XXXX already exists in this account' (names/ids vary per run; the template is the requirement).
  (invoicing.create-invoice-unit-client-token-idempotent)

## Steps to Reproduce
Boot the emulator and point an invoicing client at it (account id from sts:GetCallerIdentity).

Cause 1: CreateInvoiceUnit(Name=N, InvoiceReceiver=acct, Rule={LinkedAccounts:[acct],BillSourceAccounts:[]}); GetInvoiceUnit and keep LastModified as T; DeleteInvoiceUnit(arn); ListInvoiceUnits(Filters={Names:[N]}, AsOf=T) -> expect the ARN present (emulator returns none); ListInvoiceUnits(Filters={Names:[N]}) with no AsOf -> expect absent.

Cause 2: create an invoice unit, then TagResource in batches of 10 distinct keys, calling ListTagsForResource between batches. Expect a 402 ServiceQuotaExceededException before 200 tags are on the resource; the emulator accepts all 200. Then UntagResource the last successful batch and re-issue the rejected batch — it must succeed and its tags be visible.

Cause 3: invoicing.send_procurement_portal_validation(ProcurementPortalPreferenceArn="arn:aws:invoicing::<acct>:procurement-portal-preference/does-not-exist") and invoicing.verify_procurement_portal_validation(same arn, Code="000000") -> expect 404 ResourceNotFoundException from both; emulator answers 501 NotImplemented.

Cause 4: invoicing.create_procurement_portal_preference(ProcurementPortalName="NOT_A_REAL_PORTAL", BuyerDomain="NetworkID", BuyerIdentifier="AN01234567890", SupplierDomain="NetworkID", SupplierIdentifier="AN01234567890", EinvoiceDeliveryEnabled=False, PurchaseOrderRetrievalEnabled=True, Contacts=[{Name:...,Email:...}]) -> read the ValidationException message's enumerated value set; it must list ZIP, SAP_BUSINESS_NETWORK, JAGGAER, COUPA. Also confirm ProcurementPortalName="ZIP" and "JAGGAER" are no longer refused as invalid enum values.

Cause 5: CreateInvoiceUnit(Name=N, ...) twice with identical input -> the second is refused ValidationException; compare the message with 'Invoice Unit already exists with name: <N>'.
