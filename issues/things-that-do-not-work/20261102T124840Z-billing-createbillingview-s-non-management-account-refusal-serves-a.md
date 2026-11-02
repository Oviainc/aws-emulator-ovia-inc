Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: billing.create-billing-view-non-management-account-deterministic-rejection
Issue-Shape-Id: com.amazonaws.billing#AWSBilling
Issue-Key: billing.create-billing-view-non-management-account-deterministic-rejection|run#12|AssertionError|assert all(message == expected_message for _code, message in failures), failures
Issue-Commit: 557049f7e77b6e73b3fb7073013478147806c9e2

# CreateBillingView's non-management-account refusal serves a hand-rolled message instead of AWS's fixed management-account sentence

> _Note: this issue was identified against the code base at commit `557049f7e77b6e73b3fb7073013478147806c9e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBillingView

## Description
One cause, in the billing service, on the CreateBillingView management-account refusal path: the emulator picks the CORRECT error shape and HTTP status but serializes a `message` AWS never sends for that error class. The error message is part of the contract — clients, support tooling and negative-path tests that surface or match the denial text see wording real AWS does not produce. THIS BUNDLE HAS 1 CAUSE.

=== Cause 1 of 1: the management-account gate's ValidationException message is an account-id-interpolated two-sentence form, where AWS sends a fixed, caller-independent sentence pair (check billing.create-billing-view-non-management-account-deterministic-rejection) ===

AWS: recorded behavior only for the message; the refusal itself is modeled. Smithy (services/service-com-amazonaws-billing-awsbilling/smithy-model/com-amazonaws-billing-awsbilling.json): CreateBillingView(name: BillingViewName @length 1..128 @pattern ^[ a-zA-Z0-9_\+=\.\-@]+$ required, sourceViews: BillingViewSourceViewsList @length 1..10 required, description, dataFilterExpression, clientToken, resourceTags) declares ValidationException (errorKind client, httpError 400, members message/reason/fieldList) among its errors; the model documents no message text for it, so the recorded wording is the requirement. The recording shows real AWS refusing a CreateBillingView call made by an account that is not the AWS Organizations management account with HTTP 400, code `ValidationException`, and the message
  `User account is not an AWS Organization's management account. This operation is only permitted to management accounts.`
That message is ENTIRELY FIXED text for this error class: every word, including the apostrophe in `Organization's` and the two-sentence split, is constant. It interpolates NOTHING — no account id, no view name, no ARN, no source-view value from the request or from state. AWS deliberately does not name the caller's account here. The refusal is also deterministic and pre-state: the check fires five concurrent CreateBillingView calls with distinct names and asserts all five are refused with the same code AND the same message (never a mix of codes, never a stray success), and that a follow-up ListBillingViews(billingViewTypes=["CUSTOM"]) shows none of the five attempted names — the emulator does satisfy all of that (5/5 ValidationException, zero successes, zero leaked views); only the message text diverges.

Emulator: the gate builds its own sentence pair with `format!` and splices the caller's account id into it, serving `Account <account-id> is not the management account of an organization. Only the management account can create a billing view.` (the recorded run's `000000000000` is that run's emulated account, a per-run value, not part of the divergence — the divergence is that the template interpolates an account id at all, and that both sentences are worded differently from AWS's). Status 400, code `ValidationException` and `reason` are otherwise as modeled.

Why: a client that branches on, logs, or surfaces this refusal's text — the only human-readable discriminator for "you are not the management account", since ValidationException carries no reason value specific to it — reads a sentence that does not exist in production. Negative-path handling and assertions that are correct against AWS fail against the emulator even though the status and code look identical, and the account id leaked into the message makes the text caller-dependent where AWS's is constant.

Fix: the CreateBillingView handler's management-account refusal arm must emit AWS's fixed sentence pair for this error class rather than a locally formatted, account-id-interpolated one; the gate itself (its placement before any state mutation, its determinism under concurrency, and its 400/ValidationException classification) is correct and must not change. Note the separate open issue 20261101T141054Z-billing-createbillingview-event-propagation.md, which is about the gate being UNSATISFIABLE over the wire (nothing in src/ ever records an organization management account) — it does not touch the refusal's message text, and closing this one must not weaken or remove the gate. The fixed issue 20261029T211043Z-billing-refuses-with-the-right-codes-but-aws.md addressed the same class of defect on GetBillingPreferences/UpdateBillingPreferences/DisassociateSourceViews but never covered CreateBillingView, so this is a net-new message-text cause rather than a regression of that fix.

Traces: check billing.create-billing-view-non-management-account-deterministic-rejection. Code: services/service-com-amazonaws-billing-awsbilling/src/ops/create_billing_view.rs:192-203 (the `is_organization_management_account` else-arm and its `format!` message).

## Evidence
Behavioral suite simparity-behavioral-aws-v1: 18/19 passed; billing 18/19 (95%), 1 failed — a mismatch on refusal message text.

billing.create-billing-view-non-management-account-deterministic-rejection (mismatch):
  Setup: ListBillingViews(billingViewTypes=["PRIMARY"]) -> exactly one PRIMARY view arn.
  Call: five CONCURRENT CreateBillingView(name=<unique per attempt>, sourceViews=[<that PRIMARY arn>]) from a non-management account.
  successes: expected 0, got 0                       (OK)
  failures:  expected 5, got 5                       (OK)
  code:      expected 'ValidationException' x5, got 'ValidationException' x5   (OK)
  leaked CUSTOM views under the attempted names: expected none, got none       (OK)
  message:   expected "User account is not an AWS Organization's management account. This operation is only permitted to management accounts."  (entirely fixed text — no interpolation)
             got      "Account 000000000000 is not the management account of an organization. Only the management account can create a billing view."  (all five attempts identical)
  Assertion as reported: AssertionError: [('ValidationException', 'Account 000000000000 is not the management account of an organization. Only the management account can create a billing view.'), x5] from check_billing_create_billing_view_non_management_account_deterministic_rejection.py line 47 (`assert all(message == expected_message for _code, message in failures)`).
  The `000000000000` in the emulator's text is the run's emulated account id, i.e. a per-run value; the requirement is that AWS's message interpolates no account id at all and uses the quoted fixed sentence pair.

Model checked with smithy_query on services/service-com-amazonaws-billing-awsbilling/smithy-model/com-amazonaws-billing-awsbilling.json: protocol awsJson1_0, sdkId Billing; CreateBillingView is @idempotent and declares AccessDeniedException(403), BillingViewHealthStatusException(400), ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), ServiceQuotaExceededException(402), ThrottlingException(429) and ValidationException(400). ValidationException declares message (required, @length max 1024), reason (required enum) and fieldList; no doc on the operation or the error states any message text, so the recorded wording is the requirement.

Emulator source: services/service-com-amazonaws-billing-awsbilling/src/ops/create_billing_view.rs:192-203 returns CreateBillingViewErrorModeled::ValidationException with message = format!("Account {caller} is not the management account of an organization. Only the management account can create a billing view."), reason = VALIDATION_REASON_OTHER, field_list = None, before any state is touched.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Point an awsJson1_0 `billing` client at it and call ListBillingViews with billingViewTypes=["PRIMARY"]; take the single returned view's `arn`.
3. Call CreateBillingView with name=<any name matching ^[ a-zA-Z0-9_\+=\.\-@]+$ , 1..128 chars> and sourceViews=[<that PRIMARY arn>] as the emulated account, which is not an Organizations management account.
4. Observe HTTP 400 / ValidationException — correct — but message "Account <account-id> is not the management account of an organization. Only the management account can create a billing view." instead of AWS's fixed "User account is not an AWS Organization's management account. This operation is only permitted to management accounts."
5. Repeat step 3 five times concurrently (distinct names) to confirm the rest of the behavior is already right: all five must be refused with the same code AND the same message, none may succeed, and a following ListBillingViews(billingViewTypes=["CUSTOM"]) must list none of the attempted names.
6. Re-replay check billing.create-billing-view-non-management-account-deterministic-rejection to confirm the cause is closed.
