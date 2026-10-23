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
Issue-Commit: 3113486ab2df4ef97dea8b3047048aff3439051f

# BatchGetInvoiceProfile returns a profile for the requester's own account instead of 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `3113486ab2df4ef97dea8b3047048aff3439051f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetInvoiceProfile

## Description
One cause.

AWS: Smithy: `BatchGetInvoiceProfile` is documented as "This gets the invoice profile associated with a set of accounts. The accounts must be linked accounts under the requester management account organization", and the operation models `AccessDeniedException` (httpError 403) — smithy_query operation=BatchGetInvoiceProfile against services/service-com-amazonaws-invoicing-invoicing/smithy-model/com-amazonaws-invoicing-invoicing.json. The requester's own (management/payer) account is not a linked account under itself, so a request naming it is REFUSED WHOLE with 403 `AccessDeniedException` rather than answered with a profile. The recorded refusal's message is built from the request — it names the requester and the account it was refused for; recorded example: "Account 000000000000 is not authorized to access resources for account 000000000000" (keyed as lowercase `message` in this recording, unlike this service's procurement-portal errors, which key it `Message`).

Emulator: HTTP 200 with a `Profiles` list, no `__type` and no `message` (replay: status expected 403, got 200). The authorization test is `ComAmazonawsInvoicingInvoicingState::organization_accounts()`, which unconditionally inserts `::engine::emulator_account_id()` into the permitted set (state.rs:401-413), so the caller's own account always passes the check in batch_get_invoice_profile.rs:179-188 and falls through to `crate::state::invoice_profile(account_id)` — a structure whose only populated member is the echoed `AccountId`. The correct 403 helper already exists and is unused on this path (`crate::state::not_authorized_for_account`, state.rs:754-762, whose message is exactly the recorded request-derived sentence).

Why: a client's authorization handling is inverted on the most common probe there is — asking about your own account. Instead of the 403 AWS answers, the caller gets a 200 and a profile object whose fields are all absent, so code that treats a 200 as "this account has an invoice profile" proceeds on a profile that does not exist, and no test of the AccessDenied path can ever run against the emulator.

Fix: the BatchGetInvoiceProfile handler's authorization gate and the membership set it consults — the requester's own account must not be treated as a linked account of its own organization, so naming it yields the modeled 403 built by the existing `not_authorized_for_account` helper.

Traces: invoicing-batch-get-invoice-profile#task#1.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, bootstrap_signal=false, state_reset_failures=0):

invoicing-batch-get-invoice-profile#task#1, BatchGetInvoiceProfile, verdict=mismatch, expected 403 / actual 200:
  "status: expected 403, got 200"
  "$.__type: missing (expected 'AccessDeniedException', got None)"
  "$.message: missing (expected 'Account 000000000000 is not authorized to access resources for account 000000000000', got None)"

Code, as checked out:
- services/service-com-amazonaws-invoicing-invoicing/src/ops/batch_get_invoice_profile.rs:174-197 — refuses only accounts absent from `store.organization_accounts()`, otherwise maps each requested account through `crate::state::invoice_profile`.
- services/service-com-amazonaws-invoicing-invoicing/src/state.rs:401-413 — `organization_accounts()` starts with `accounts.insert(::engine::emulator_account_id().to_string())`, so the caller's own account is always a member.
- services/service-com-amazonaws-invoicing-invoicing/src/state.rs:743-762 — `not_authorized_for_account()` already builds the recorded 403 message from the request; nothing on this path reaches it for the requester's own account.
- services/service-com-amazonaws-invoicing-invoicing/src/state.rs:692-708 — `invoice_profile()` returns a structure with only `AccountId` populated, which is what the spurious 200 carries.
- Model/doc: BatchGetInvoiceProfile errors include AccessDeniedException (403); operation doc requires the accounts to be linked accounts under the requester's management-account organization.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the simparity-wire-aws-v1 corpus.
2. POST Invoicing.BatchGetInvoiceProfile with `AccountIds` = [ the emulator's own account id (the account the caller authenticates as) ].
3. Observe HTTP 200 with `Profiles: [{"AccountId": "<that account>"}]`. AWS answers HTTP 403 with `__type` `AccessDeniedException` and a message naming the requester and the refused account, because the requester's own account is not a linked account under its own organization.
4. Replay fixture invoicing-batch-get-invoice-profile step task#1 to confirm.
