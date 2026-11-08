Issue-Source: trace-sweep-divergence
Issue-Trace: invoicing-batch-get-invoice-profile
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: invoicing-batch-get-invoice-profile
Issue-Shape-Id: com.amazonaws.invoicing#Invoicing
Issue-Key: invoicing|BatchGetInvoiceProfile|4xx->2xx|$.__type
Issue-Key: invoicing|BatchGetInvoiceProfile|4xx->2xx|$.message
Issue-Key: invoicing|BatchGetInvoiceProfile|4xx->2xx|status
Issue-Commit: 89584cbdc793a0842735dd0ecc1a58bc9d0678bb

# BatchGetInvoiceProfile answers the requester's own account instead of refusing it with AccessDeniedException

> _Note: this issue was identified against the code base at commit `89584cbdc793a0842735dd0ecc1a58bc9d0678bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetInvoiceProfile

## Description
Cause 1 of 1: `BatchGetInvoiceProfile` treats the requester's OWN account as always readable, so a request that names only that account is answered 200 with an invoice profile instead of being refused 403 `AccessDeniedException`.

AWS: the Smithy model documents `BatchGetInvoiceProfile` as "This gets the invoice profile associated with a set of accounts. The accounts must be linked accounts under the requester management account organization" and models `AccessDeniedException` (httpError 403, awsQueryError code `InvoicingAccessDenied`, doc "You don't have sufficient access to perform this action.") among its errors (smithy_query operation=BatchGetInvoiceProfile; smithy_query shape=AccessDeniedException on services/service-com-amazonaws-invoicing-invoicing/smithy-model/com-amazonaws-invoicing-invoicing.json). The requester IS the management account, and a management account is not one of its own organization's LINKED accounts, so naming it is an account the requester may not be answered for and the whole request is refused. The recorded response confirms the refusal: HTTP 403 with `__type` = `AccessDeniedException` and `message` = "Account <requesterAccountId> is not authorized to access resources for account <requestedAccountId>". In that message the fixed text is "Account " / " is not authorized to access resources for account "; the two variable parts are account ids — the first is the identity the caller authenticates as (the account the emulator presents, `::engine::emulator_account_id()`), the second is the account id taken from the request's `AccountIds` (the first entry, in request order, that the requester may not read). Both happen to render as the same 12-digit id in this recording because the refused account IS the caller's own; neither literal value is part of the requirement.

Emulator: the step replays 200 with a `Profiles` list and no error body at all — `$.__type` missing (expected `AccessDeniedException`) and `$.message` missing (expected the sentence above). The emulator's authorization set unconditionally admits the caller's own account, so the 403 branch is never reached: `ComAmazonawsInvoicingInvoicingState::accounts_the_requester_may_read` takes `linked_accounts()` and then inserts `::engine::emulator_account_id()`, and the handler refuses only accounts absent from that set. The in-repo doc comment asserts this own-account admission is "recorded behaviour, which answers a request naming only the caller's account with that account's profile" — the recorded fixture says the opposite.

Why: a client relying on AWS's behavior gets a silent authorization bypass. Code that expects 403 for an account outside the organization's linked accounts — error handling, permission probes, and any logic that enumerates which accounts it may read by trying them — instead receives 200 and a synthesized `InvoiceProfile` carrying `AccountId`. The failure mode is the worst direction for an access check: the emulator grants access where AWS denies it, so tests that assert a denial pass vacuously and a client that branches on `AccessDeniedException` never takes that branch.

Fix: the operation's authorization decision — the readable-account set in the service state that `BatchGetInvoiceProfile`'s handler consults, and the handler's refusal branch that calls `not_authorized_for_account`. Membership must be derived from the organization's linked accounts only; the requester's own account must not be injected into the set. The refusal wording itself is already correct and reusable: `not_authorized_for_account` already builds exactly the recorded sentence from the requester identity plus the offending account id, and already leaves `resourceName` absent (correct, since the model types it `InvoiceUnitArnString`, an invoice-unit ARN, not an account). Only the set that decides WHEN to reach it is wrong. The refusal must reject the request WHOLE with 403 and report no profiles, not answer partially.

Traces: invoicing-batch-get-invoice-profile#task#1.

## Evidence
Fixture `invoicing-batch-get-invoice-profile`, step `task#1`, verdict mismatch, divergence_key `service=invoicing|BatchGetInvoiceProfile|status=403->200|keys=$.__type,$.message,status`:

  status: expected 403, got 200
  $.__type: missing (expected 'AccessDeniedException', got None)
  $.message: missing (expected 'Account 000000000000 is not authorized to access resources for account 000000000000', got None)

Expected (AWS, per recording + model): HTTP 403, error code `AccessDeniedException`, message "Account {requesterAccountId} is not authorized to access resources for account {refusedAccountId}" — fixed text as quoted, both ids substituted from the caller identity and the request. Actual (emulator): HTTP 200 with a `Profiles` array and no `__type`/`message` members.

Code path that produces the wrong verdict:
- services/service-com-amazonaws-invoicing-invoicing/src/state.rs:569-573 — `accounts_the_requester_may_read()` inserts `::engine::emulator_account_id()` into `linked_accounts()`, so the caller's own account is always considered readable.
- services/service-com-amazonaws-invoicing-invoicing/src/ops/batch_get_invoice_profile.rs:190-199 — the 403 branch fires only for an account missing from that set, so it is skipped for the caller's own account.
- services/service-com-amazonaws-invoicing-invoicing/src/state.rs:2087-2095 — `not_authorized_for_account()` already renders the recorded message template correctly; it is simply never invoked for this request.

No open issue covers this: the three open invoicing issues are the `GetInvoicePDF` scaffold stub, a `ListInvoiceSummaries` ReceiverRole/period validation gap, and a `SendProcurementPortalValidation` code-generation defect.

## Steps to Reproduce
1. Boot the emulator and replay fixture `invoicing-batch-get-invoice-profile`, step `task#1`.
2. Equivalently, with state holding no invoice unit that links the caller's own account as a `LinkedAccounts` member or `InvoiceReceiver`, call `BatchGetInvoiceProfile` with `AccountIds` naming only the account the emulator authenticates the caller as.
3. Observe HTTP 200 and a `Profiles` list containing a profile whose `AccountId` is that account.
4. AWS refuses the whole request: HTTP 403, `__type` `AccessDeniedException`, message "Account {requesterAccountId} is not authorized to access resources for account {requestedAccountId}", and no profiles.
