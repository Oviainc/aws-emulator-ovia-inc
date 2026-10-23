Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: invoicing.batch-get-invoice-profile-unlinked-account-rejected invoicing.put-procurement-portal-preference-updates-version invoicing.verify-procurement-portal-validation-wrong-code-rejected invoicing.create-invoice-unit-client-token-idempotent
Issue-Shape-Id: com.amazonaws.invoicing#Invoicing
Issue-Key: invoicing|BatchGetInvoiceProfile|AccessDeniedException
Issue-Key: invoicing.put-procurement-portal-preference-updates-version|run#15|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: invoicing.verify-procurement-portal-validation-wrong-code-rejected|run#7|AssertionError|assert send_err["Message"] == expected_message, (
Issue-Key: invoicing|CreateInvoiceUnit|ValidationException
Issue-Commit: 326b33f63f33ac22c419b7169b10d47cfd8c011f

# Invoicing: own-account invoice profile refused 403, and not-found/duplicate refusals drop request-derived message text

> _Note: this issue was identified against the code base at commit `326b33f63f33ac22c419b7169b10d47cfd8c011f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetInvoiceProfile

## Description
THREE causes, all in the invoicing emulator. Numbered below; each carries its own failing behavioral check ids and must be closed before this issue is submitted.

Cause 1 of 3: BatchGetInvoiceProfile refuses the CALLER'S OWN account with 403 AccessDeniedException instead of answering its profile.
AWS: BatchGetInvoiceProfile is documented "This gets the invoice profile associated with a set of accounts. The accounts must be linked accounts under the requester management account organization" and its input AccountIds is required, len 1..1000 (Smithy: BatchGetInvoiceProfile input AccountIds -> AccountIdList @required @length 1..1000; errors AccessDeniedException httpError 403, ValidationException 400, ResourceNotFoundException 404). Behavior for the requester's own account is recorded behavior only: the replayed check, stated as confirmed against real AWS, shows BatchGetInvoiceProfile(AccountIds=[<caller account>]) SUCCEEDING with a Profiles entry whose AccountId is the caller's account, and 403 AccessDeniedException being reserved for a well-formed account id that is not a member of the caller's organization. So the caller's own account is readable; only a foreign account is denied.
Emulator: the request for the caller's own account alone is refused whole with 403 AccessDeniedException (observed message, an example: "Account 000000000000 is not authorized to access resources for account 000000000000" — the same account id on both sides of the sentence, which is the tell). The handler resolves membership through the state's linked_accounts set, which deliberately EXCLUDES the emulator's own account id on the theory that a management account is not a linked account under itself, so every caller — whose only identity is that account — is denied for its own profile.
Why: a client can only ever call this operation for accounts it can see, and in this emulator the only such account is the caller's own. The operation is therefore unusable: a caller that correctly asks for its own profile gets 403 and no Profiles at all, and the negative case the API actually denies (a foreign account) can no longer be distinguished from the positive case, because both answer 403 with the same shape.
Fix: BatchGetInvoiceProfile's handler and the membership predicate it reads — the caller's own account must resolve to a profile, with 403 kept for account ids outside the organization; the refusal must also still be whole-request (no partial Profiles) for a genuinely foreign id.
Traces: invoicing.batch-get-invoice-profile-unlinked-account-rejected (verdict error — the check's positive control raised AccessDeniedException before it could exercise the unlinked-account case).

Cause 2 of 3: ResourceNotFoundException on the procurement-portal plane carries a constant message that does not name the ARN that was asked about.
AWS: PutProcurementPortalPreference and SendProcurementPortalValidation both model ResourceNotFoundException with httpError 404 (Smithy: PutProcurementPortalPreference errors include ResourceNotFoundException httpError 404; its ProcurementPortalPreferenceArn is @required with @pattern ^arn:aws:invoicing::[0-9]{12}:procurement-portal-preference/[-a-zA-Z0-9]+$). The message text is recorded behavior only, and it is BUILT FROM THE REQUEST: it names the resource kind and the exact ARN the request supplied. Recorded example: "Procurement portal preference with arn arn:aws:invoicing::000000000000:procurement-portal-preference/sim2real-does-not-exist does not exist". The request-derived rule is the requirement; that literal sentence is only an example.
Emulator: status 404 and error code ResourceNotFoundException are correct, but the message is the fixed sentence "The resource could not be found." for every ARN, so the identifier is never interpolated. The not-found value is built by the service's shared no_such_resource constructor, which sets a constant message and puts the ARN in a separate member.
Why: a client or operator reading the refusal cannot tell WHICH preference was missing from the message, and any client that matches or logs AWS's request-derived sentence sees text that never varies with the request.
Fix: the service's shared not-found constructor in the invoicing state/error helpers (it already receives the ARN) plus the procurement-portal handlers that call it, so the 404 renders AWS's request-derived sentence naming the requested preference ARN. Related, do NOT assume covered: open issue "Invoicing procurement-portal error bodies omit the Message member and use constant text" (20261023T095223Z-invoicing-procurement-portal-error-bodies-omit-the-message.md) describes the same constructor for GetProcurementPortalPreference / SendProcurementPortalValidation / VerifyProcurementPortalValidation; it does not list PutProcurementPortalPreference, and these behavioral checks were not marked as owned by it.
Traces: invoicing.put-procurement-portal-preference-updates-version (Put on a well-formed non-existent ARN: code ResourceNotFoundException matched, message was "The resource could not be found."), invoicing.verify-procurement-portal-validation-wrong-code-rejected (SendProcurementPortalValidation on a non-existent ARN: same constant message).

Cause 3 of 3: CreateInvoiceUnit's duplicate-Name refusal renders the Name without the delimiters AWS's template uses.
AWS: CreateInvoiceUnit models ValidationException httpError 400 and its Name is @required, @length 1..50, @pattern ^(?! )[\p{L}\p{N}\p{Z}-_]*(?<! )$ (Smithy: CreateInvoiceUnit). The duplicate-name refusal message is recorded behavior only and is BUILT FROM THE REQUEST — it is a fixed template over the requested Name, with the name enclosed in angle brackets. Recorded example for Name=sim2real-iu-c5e7f8e5d785: "Invoice Unit already exists with name: <sim2real-iu-c5e7f8e5d785>". The template-over-the-request rule is the requirement, not that one name.
Emulator: status 400, code ValidationException and reason duplicateInvoiceUnit are all correct, but the handler formats "Invoice Unit already exists with name: {Name}" — the angle brackets of the template are missing, so the bare name is spliced into the prose.
Why: a client that parses the offending name out of the refusal by the delimiters AWS emits extracts nothing, and a name containing spaces (which the Name pattern permits internally) becomes unparseable from the message because nothing marks where it begins and ends.
Fix: the duplicate-name branch of the CreateInvoiceUnit handler's validation — render the service's template, delimiters included, over the requested Name.
Traces: invoicing.create-invoice-unit-client-token-idempotent (the rest of that check — ValidationException code, the name staying single in ListInvoiceUnits, and the name becoming reusable after delete — passed).

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from mainline: 15/21 passed, 5 mismatched, 1 errored, 0 skipped, 0 unloadable, bootstrap_signal=false. All failures are in service invoicing (15/21, 6 failed). None of the four checks below was marked ALREADY FILED or OWNED by an open issue.

Cause 1 — [error] invoicing.batch-get-invoice-profile-unlinked-account-rejected:
  botocore.errorfactory.AccessDeniedException: An error occurred (AccessDeniedException) when calling the BatchGetInvoiceProfile operation: Account 000000000000 is not authorized to access resources for account 000000000000
The raising call is the check's POSITIVE control, invoicing.batch_get_invoice_profile(AccountIds=[sts.get_caller_identity()['Account']]), which asserts `any(p['AccountId'] == account_id for p in own['Profiles'])`. The emulator never reached the check's negative case (a 12-digit non-member id, expected 403 AccessDeniedException).
Code, as checked out:
- services/service-com-amazonaws-invoicing-invoicing/src/ops/batch_get_invoice_profile.rs:178-193 — `let linked = store.linked_accounts(); if let Some(outsider) = requested.iter().find(|id| !linked.contains(**id)) { return Err(... AccessDeniedException(crate::state::not_authorized_for_account(outsider))) }`, with the module docs at lines 14-16 stating "The requester's OWN account is refused the same way".
- services/service-com-amazonaws-invoicing-invoicing/src/state.rs:393-412 — linked_accounts documents that the requester's own account (`::engine::emulator_account_id`) "is NEVER a member of this set ... not even when a stored configuration names it".
- services/service-com-amazonaws-invoicing-invoicing/src/state.rs:829-837 — not_authorized_for_account builds the message from emulator_account_id + the refused account, which is why both sides of the recorded sentence show the same id.

Cause 2 — [mismatch] invoicing.put-procurement-portal-preference-updates-version:
  AssertionError: expected message 'Procurement portal preference with arn arn:aws:invoicing::000000000000:procurement-portal-preference/sim2real-does-not-exist does not exist', got 'The resource could not be found.'
The preceding assertion `code == "ResourceNotFoundException"` passed, as did the positive control (Put on a real preference replaced Contacts/PurchaseOrderRetrievalEnabled and bumped Version to 2), so status and code are right and only the message diverges.
[mismatch] invoicing.verify-procurement-portal-validation-wrong-code-rejected:
  AssertionError: expected message 'Procurement portal preference with arn arn:aws:invoicing::000000000000:procurement-portal-preference/sim2real-does-not-exist does not exist', got 'The resource could not be found.'
  (raised on SendProcurementPortalValidation against the non-existent ARN, after its `Code == "ResourceNotFoundException"` assertion passed.)
Code, as checked out:
- services/service-com-amazonaws-invoicing-invoicing/src/state.rs:784-801 — `RESOURCE_NOT_FOUND_MESSAGE: &str = "The resource could not be found."` and `no_such_resource(arn)` sets that constant while putting `arn` in `resource_id`.
Model: smithy_query(query="operation", operation="PutProcurementPortalPreference", model_path="services/service-com-amazonaws-invoicing-invoicing/smithy-model/com-amazonaws-invoicing-invoicing.json") — ResourceNotFoundException httpError 404; ProcurementPortalPreferenceArn @required with the procurement-portal-preference ARN @pattern.

Cause 3 — [mismatch] invoicing.create-invoice-unit-client-token-idempotent:
  AssertionError: expected duplicate-name message 'Invoice Unit already exists with name: <sim2real-iu-c5e7f8e5d785>', got 'Invoice Unit already exists with name: sim2real-iu-c5e7f8e5d785'
The `code == "ValidationException"` assertion passed.
Code, as checked out:
- services/service-com-amazonaws-invoicing-invoicing/src/ops/create_invoice_unit.rs:224-231 — `message: Some(format!("Invoice Unit already exists with name: {}", input.name))`, reason `duplicateInvoiceUnit`; the comment at lines 222-223 claims it "states the service's own template over the requested Name".
Model: smithy_query(query="operation", operation="CreateInvoiceUnit", same model_path) — Name @required @length 1..50 @pattern ^(?! )[\p{L}\p{N}\p{Z}-_]*(?<! )$; ValidationException httpError 400.

## Steps to Reproduce
Boot the emulator built from mainline and replay the simparity-behavioral-aws-v1 invoicing checks (or drive the calls below with any AWS SDK pointed at the emulator).

Cause 1 (BatchGetInvoiceProfile own account):
1. account = sts.get_caller_identity()['Account'].
2. Call invoicing.BatchGetInvoiceProfile with AccountIds=[account].
3. Observe HTTP 403 AccessDeniedException (message names the same account on both sides). Expected: 200 with a Profiles entry whose AccountId == account.
4. Then call BatchGetInvoiceProfile with AccountIds=[account, "111111111111"] and observe that the 403 for a genuinely non-member id is indistinguishable from step 3. Replay check invoicing.batch-get-invoice-profile-unlinked-account-rejected.

Cause 2 (not-found message does not name the ARN):
5. fake = arn:aws:invoicing::<account>:procurement-portal-preference/sim2real-does-not-exist (an ARN no create produced).
6. Call invoicing.PutProcurementPortalPreference with ProcurementPortalPreferenceArn=fake, EinvoiceDeliveryEnabled=false, PurchaseOrderRetrievalEnabled=false, Contacts=[{Name,Email}]. Observe 404 ResourceNotFoundException with message "The resource could not be found."; AWS answers 404 whose message names the resource kind and that exact ARN. Replay check invoicing.put-procurement-portal-preference-updates-version.
7. Call invoicing.SendProcurementPortalValidation with ProcurementPortalPreferenceArn=fake and observe the same constant message; likewise VerifyProcurementPortalValidation with any Code. Replay check invoicing.verify-procurement-portal-validation-wrong-code-rejected.

Cause 3 (duplicate invoice-unit name message):
8. Call invoicing.CreateInvoiceUnit with Name=<fresh name>, InvoiceReceiver=<account>, Rule={LinkedAccounts:[<account>], BillSourceAccounts:[]} — it succeeds.
9. Repeat the identical call. Observe 400 ValidationException with message "Invoice Unit already exists with name: <name without angle brackets>"; AWS encloses the Name in angle brackets. Replay check invoicing.create-invoice-unit-client-token-idempotent.
