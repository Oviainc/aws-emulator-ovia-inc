Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: billing.get-billing-preferences-access-denied-without-billing-access billing.update-billing-preferences-access-denied-rejected billing.disassociate-source-views-from-primary-view-rejected
Issue-Shape-Id: com.amazonaws.billing#AWSBilling
Issue-Key: billing.get-billing-preferences-access-denied-without-billing-access|run#3|AssertionError|assert message == (
Issue-Key: billing.update-billing-preferences-access-denied-rejected|run#8|AssertionError|assert message == _denied_message(account_id, "RI_SHARING"), message
Issue-Key: billing.disassociate-source-views-from-primary-view-rejected|run#10|AssertionError|assert message == "Only custom billing views can have source views disassociated", message
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# billing refuses with the right codes but AWS's wrong message text: the per-feature AccessDenied names the public operation, and DisassociateSourceViews reuses the generic non-custom-view sentence

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBillingPreferences

## Description
Two distinct causes, both in the billing service, both on refusal paths where the emulator picks the CORRECT error shape and HTTP status but serializes a `message` AWS never sends for that error class. An error message is part of the contract: clients, SDK retry/branching helpers and tests that surface or match it see text real AWS does not produce. THIS BUNDLE HAS 2 CAUSES; both must be closed before it is submitted.

=== Cause 1 of 2: the per-feature billing-access AccessDeniedException embeds the PUBLIC operation name, where AWS embeds that operation's own backend operation token (checks billing.get-billing-preferences-access-denied-without-billing-access, billing.update-billing-preferences-access-denied-rejected) ===

AWS: recorded behavior only for the message; the error itself is modeled. Smithy (services/service-com-amazonaws-billing-awsbilling/smithy-model/com-amazonaws-billing-awsbilling.json): both GetBillingPreferences and UpdateBillingPreferences declare the service-common AccessDeniedException (errorKind client, httpError 403), whose only member is `message`; neither the operation docs nor the error docs state the message text. The recordings show real AWS refusing a caller that lacks Billing console/IAM access activation for a gated feature with HTTP 403, code `AccessDeniedException`, and a message built from the fixed bracketed template
  `[Exception=AccessDeniedException][AccountId = <account-id>][Operation = <operation-token>][Message=No access to feature <feature>]`
Fixed text: `[Exception=AccessDeniedException]`, `[AccountId = ` (note the spaces around `=`), `][Operation = `, `][Message=No access to feature `, and the closing `]`. Variable parts, taken from the request and the caller's identity: `<account-id>` is the caller's account id (the recorded run's 000000000000 is that run's account, not part of the requirement); `<feature>` is the BillingFeature the request named (recorded for BILLING_ALERTS and RI_SHARING_HISTORY on the read, RI_SHARING and RI_SHARING_HISTORY on the write); and `<operation-token>` is the operation's BACKEND token, which for this surface is the public operation name with the billing-preferences backend suffix appended — `GetBillingPreferencesPaperwork` for GetBillingPreferences and `UpdateBillingPreferencesPaperwork` for UpdateBillingPreferences. The suffix is the part that diverges; it is not a per-run value.

Emulator: the shared helper `state::feature_access_denied(operation, feature)` formats the same template but is called with `Self::NAME`, i.e. the public operation name, so it serves `[Operation = GetBillingPreferences]` and `[Operation = UpdateBillingPreferences]`. Everything else in the message (account id, feature, bracket layout) matches, and the code/status (AccessDeniedException / 403) and the per-feature nature of the gate are correct — the positive control on CREDIT_SHARING succeeds in both checks.

Why: a client that parses this message to learn WHICH backend operation denied it (the bracketed `[Operation = ...]` segment is the only machine-readable discriminator AWS gives here, since AccessDeniedException carries no other member) reads a token that does not exist in AWS output, so log scrapers, support tooling and assertions on the denial text diverge from production while the status and code look identical.

Fix: the shared message builder for this refusal and/or the token its two callers pass it — the operation token must be the operation's backend token, not `Operation::NAME`. It belongs in this service's state helper plus the two preference handlers that call it; the per-feature gate itself is correct and must not change.

Traces: checks billing.get-billing-preferences-access-denied-without-billing-access and billing.update-billing-preferences-access-denied-rejected. Code: services/service-com-amazonaws-billing-awsbilling/src/state.rs:1032-1040 (`feature_access_denied`, the format string), services/service-com-amazonaws-billing-awsbilling/src/ops/update_billing_preferences.rs:215-220 (call site passing `Self::NAME`), and the matching gate in services/service-com-amazonaws-billing-awsbilling/src/ops/get_billing_preferences.rs.

=== Cause 2 of 2: DisassociateSourceViews against a non-CUSTOM view answers the generic shared "does not correspond to a Custom Billing View" sentence instead of this operation's own AWS message (check billing.disassociate-source-views-from-primary-view-rejected) ===

AWS: recorded behavior only for the message; the refusal is modeled. Smithy: DisassociateSourceViews(arn: BillingViewArn required, sourceViews: BillingViewSourceViewsList @length 1..10 required) declares ValidationException (errorKind client, httpError 400) among its errors, and the operation doc says it "Removes the association between one or more source billing views and an existing billing view". The recording shows that disassociating source views from a view that is not CUSTOM — here the account's service-managed PRIMARY view, which has no caller-owned source-view association — is refused with HTTP 400, code `ValidationException`, and the message `Only custom billing views can have source views disassociated`. That message is entirely FIXED text for this error class: it interpolates no ARN, view name, account id or source-view value from the request. The refusal must also leave state untouched (the check re-reads sourceViewCount and ListSourceViewsForBillingView afterwards and both are unchanged, which the emulator does satisfy).

Emulator: returns the right code and status but the shared helper `state::not_a_custom_view(arn, action)` sentence — `The Billing View ARN does not correspond to a Custom Billing View. Only Custom Billing Views may be disassociated from their source views.` — i.e. a two-sentence form parameterised by a past-participle `action` phrase, reused verbatim from the UpdateBillingView/DeleteBillingView refusals (`"edited"`, `"deleted"`). AWS does not word this operation's refusal that way: the generic sentence belongs to the view-lifecycle writers, while DisassociateSourceViews has its own single-sentence message. (The emulator additionally attaches a `fieldList` entry naming `arn`; the recording does not assert on it, so this issue does not claim it is wrong — only the `message` text is the divergence.)

Why: a client or test that distinguishes "this ARN is not a custom view" refusals per operation, or that asserts on the documented refusal text for DisassociateSourceViews, sees a different sentence than production for an otherwise-identical 400/ValidationException, so negative-path handling that is correct against AWS fails against the emulator.

Fix: the DisassociateSourceViews handler's non-CUSTOM-view refusal must build its own message rather than borrow the shared lifecycle helper's `action`-parameterised sentence; the helper stays as-is for the view-lifecycle writers that do use that wording.

Traces: check billing.disassociate-source-views-from-primary-view-rejected. Code: services/service-com-amazonaws-billing-awsbilling/src/ops/disassociate_source_views.rs:208-213 (the `custom_view_mut` else-branch calling `state::not_a_custom_view(&input.arn, "disassociated from their source views")`), services/service-com-amazonaws-billing-awsbilling/src/state.rs:1059-1071 (the shared sentence).

No open issue covers either cause. The one open issue on this service, 20261029T170747Z-billing-serves-the-default-primary-view-and-its.md, is about the PRIMARY view's RENDERING (description/healthStatus/content-type) and about the ValidationException reason/fieldList members lost at the request boundary for GetBillingView — it says nothing about the message TEXT of these two refusals, and neither of these checks is reached by its fixture. Every other billing-family open issue is billingconductor or marketplace-agreement.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: 16/19 passed; billing 16/19 (84%), 3 failed, all three mismatches on refusal message text.

Cause 1 — billing.get-billing-preferences-access-denied-without-billing-access (mismatch):
  GetBillingPreferences(features=["BILLING_ALERTS"]) and GetBillingPreferences(features=["RI_SHARING_HISTORY"]) as a caller without Billing access activation.
  code: expected 'AccessDeniedException', got 'AccessDeniedException'  (OK)
  message: expected '[Exception=AccessDeniedException][AccountId = <account-id>][Operation = GetBillingPreferencesPaperwork][Message=No access to feature <feature>]'
           got      '[Exception=AccessDeniedException][AccountId = 000000000000][Operation = GetBillingPreferences][Message=No access to feature BILLING_ALERTS]'
  Only the [Operation = ...] token differs; the account id and feature segments are correct and are request/identity-derived values, not requirements.
  Positive control GetBillingPreferences(features=["CREDIT_SHARING"]) succeeded and returned a billingPreferences list, so the gate is genuinely per-feature.

Cause 1 — billing.update-billing-preferences-access-denied-rejected (mismatch):
  UpdateBillingPreferences(feature="RI_SHARING", billingPreferencesPerKey=[{key,value}]) and the same with feature="RI_SHARING_HISTORY".
  code: expected 'AccessDeniedException', got 'AccessDeniedException'  (OK)
  message: expected '[Exception=AccessDeniedException][AccountId = <account-id>][Operation = UpdateBillingPreferencesPaperwork][Message=No access to feature RI_SHARING]'
           got      '[Exception=AccessDeniedException][AccountId = 000000000000][Operation = UpdateBillingPreferences][Message=No access to feature RI_SHARING]'
  Same single divergence, same shared helper; the check asserts the access gate is reached BEFORE the read-only-history-feature validation, which the emulator does satisfy (both RI_SHARING and RI_SHARING_HISTORY are denied identically rather than one failing with the read-only error).

Cause 2 — billing.disassociate-source-views-from-primary-view-rejected (mismatch):
  DisassociateSourceViews(arn=<the account's PRIMARY billing view arn>, sourceViews=[<a well-formed billingview arn that is not a source of it>]).
  code: expected 'ValidationException', got 'ValidationException'  (OK)
  message: expected 'Only custom billing views can have source views disassociated'  (entirely fixed text)
           got      'The Billing View ARN does not correspond to a Custom Billing View. Only Custom Billing Views may be disassociated from their source views.'
  Side effects were correct: the preceding ListSourceViewsForBillingView/GetBillingView positive control and the post-refusal re-reads agreed (sourceViewCount and the source-view list unchanged).

Model checked with smithy_query on services/service-com-amazonaws-billing-awsbilling/smithy-model/com-amazonaws-billing-awsbilling.json: protocol awsJson1_0, sdkId Billing; AccessDeniedException is a service-common error (httpError 403) carried by both preference operations and declares only `message`; DisassociateSourceViews declares ValidationException (httpError 400) and is @idempotent. The model documents no message text for any of the three, so the recorded wording is the requirement.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Cause 1a: call billing GetBillingPreferences with features=["BILLING_ALERTS"] (then ["RI_SHARING_HISTORY"]) as the emulated account, which has no Billing console access activation for those features. Observe HTTP 403 / AccessDeniedException — correct — but a message whose bracketed operation segment is '[Operation = GetBillingPreferences]' instead of AWS's '[Operation = GetBillingPreferencesPaperwork]'. Confirm the gate is feature-specific, not blanket, by calling GetBillingPreferences with features=["CREDIT_SHARING"], which succeeds.
3. Cause 1b: call billing UpdateBillingPreferences with feature="RI_SHARING" and billingPreferencesPerKey=[{"key": "<any key>", "value": "ENABLED"}], then again with feature="RI_SHARING_HISTORY". Both answer 403 / AccessDeniedException with '[Operation = UpdateBillingPreferences]' instead of '[Operation = UpdateBillingPreferencesPaperwork]'.
4. Cause 2: call billing ListBillingViews with billingViewTypes=["PRIMARY"] to get the account's single PRIMARY view arn, then call DisassociateSourceViews with that arn and sourceViews=[ a well-formed arn:aws:billing::<account-id>:billingview/<id> that is not one of its sources ]. Observe HTTP 400 / ValidationException with message 'The Billing View ARN does not correspond to a Custom Billing View. Only Custom Billing Views may be disassociated from their source views.' instead of AWS's 'Only custom billing views can have source views disassociated'.
5. Re-replay checks billing.get-billing-preferences-access-denied-without-billing-access, billing.update-billing-preferences-access-denied-rejected and billing.disassociate-source-views-from-primary-view-rejected to confirm both causes are closed.
