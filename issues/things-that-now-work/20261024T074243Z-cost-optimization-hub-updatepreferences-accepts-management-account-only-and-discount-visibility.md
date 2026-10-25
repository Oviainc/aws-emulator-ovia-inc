Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cost-optimization-hub.preferences-member-account-discount-visibility-roundtrip cost-optimization-hub.update-preferences-invalid-discount-visibility-rejected cost-optimization-hub.preferences-savings-estimation-mode-roundtrip
Issue-Shape-Id: com.amazonaws.costoptimizationhub#CostOptimizationHubService
Issue-Key: cost-optimization-hub.preferences-member-account-discount-visibility-roundtrip|_client_error#13|AssertionError|raise AssertionError(
Issue-Key: cost-optimization-hub.update-preferences-invalid-discount-visibility-rejected|_error#19|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: cost-optimization-hub.preferences-savings-estimation-mode-roundtrip|_client_error#11|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# UpdatePreferences accepts management-account-only and discount-visibility-gated values that real AWS rejects with ValidationException

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdatePreferences

## Description
The emulator's Cost Optimization Hub UpdatePreferences handler validates only the shape/enum of its members and then unconditionally writes and echoes whatever it was given. Two distinct authorization/consistency rules that real AWS enforces on top of enum validation are missing, so inputs real AWS refuses are accepted with HTTP 200 and are persisted. Generic out-of-enum validation (memberAccountDiscountVisibility="Everyone") already behaves correctly — the gap is specifically the post-enum semantic rules. 2 causes.

Cause 1 of 2: memberAccountDiscountVisibility is writable by a non-management (member) account.
AWS: Smithy: UpdatePreferences input member memberAccountDiscountVisibility targets enum MemberAccountDiscountVisibility (values [All, None]); operation models ValidationException (errorKind client, httpError 400) with required member `message` plus optional `reason` and `fields`. The model does not express the management-account restriction, so for the restriction itself: recorded behavior only — the recording shows a well-formed UpdatePreferences(memberAccountDiscountVisibility=<All|None>) issued by a member/non-management account being rejected with HTTP 400 and error code ValidationException, the field left unchanged, and GetPreferences continuing not to report memberAccountDiscountVisibility at all for a member account. Example recorded message: "Only management accounts can update member account discount visibility." (the requirement is the 400/ValidationException and the no-op, with a message distinguishable from the generic input-validation message; the exact text is an example).
Emulator: the call SUCCEEDS with HTTP 200 and the response body echoes the written value, e.g. {'savingsEstimationMode': 'BeforeDiscounts', 'memberAccountDiscountVisibility': 'None'|'All', 'preferredCommitment': {...}}. A subsequent GetPreferences then reports memberAccountDiscountVisibility for an account that should never have it reported.
Why: a client (or a control-plane test, or an org-governance tool) relying on AWS's behavior concludes that a member account can change organization-wide discount visibility. Code that branches on this rejection — surfacing "run this from the management account" — never runs, and the emulator's persisted preferences diverge permanently from what the same calls would leave in AWS. Because the generic out-of-enum rejection and this restriction must be distinguishable, a client that classifies errors by code+message cannot be exercised at all.
Fix: UpdatePreferences' validation/authorization step in the cost-optimization-hub operation handler: after enum validation, reject any request that carries memberAccountDiscountVisibility when the calling account is not the organization's management account, returning ValidationException (400) and leaving stored preferences untouched; and GetPreferences' serializer must omit memberAccountDiscountVisibility for a non-management account.
Traces: cost-optimization-hub.preferences-member-account-discount-visibility-roundtrip (asserts rejection + field unchanged); cost-optimization-hub.update-preferences-invalid-discount-visibility-rejected (asserts the within-enum "All" value is rejected distinguishably from the out-of-enum "Everyone" value, and that GetPreferences still does not report the field).

Cause 2 of 2: savingsEstimationMode=AfterDiscounts is accepted without its member-account-discount-visibility prerequisite.
AWS: Smithy: UpdatePreferences input member savingsEstimationMode targets enum SavingsEstimationMode (values [BeforeDiscounts, AfterDiscounts]); operation models ValidationException (client, httpError 400). The prerequisite is not expressed in the model, so: recorded behavior only — the recording shows UpdatePreferences(savingsEstimationMode="AfterDiscounts") rejected with HTTP 400 / ValidationException on an account where member account discount visibility is not (and cannot be) enabled, with savingsEstimationMode left at its previous value. Example recorded message: "Invalid permission to update preferences. To use AfterDiscounts savings estimation mode, the management account must either enable member account discount visibility in Cost Optimization Hub, or enable linked account discount visibility in Cost Management Preferences." BeforeDiscounts is unrestricted and succeeds (positive control passes against the emulator).
Emulator: the call SUCCEEDS with HTTP 200 and returns {'savingsEstimationMode': 'AfterDiscounts', 'preferredCommitment': {...}}, persisting a mode that is unreachable in AWS for this account.
Why: downstream recommendation/savings figures are computed in a mode a real account could not select, so a client validates an unreachable configuration as supported and never handles the documented refusal; the stored preference state also diverges from AWS for every later GetPreferences.
Fix: the same UpdatePreferences validation step — gate AfterDiscounts on the account's discount-visibility prerequisite and answer ValidationException (400) without mutating state when it is not satisfied.
Traces: cost-optimization-hub.preferences-savings-estimation-mode-roundtrip.

## Evidence
Suite: 12/15 passed for cost-optimization-hub; all 3 failures are UpdatePreferences accepting input AWS refuses.

[mismatch] cost-optimization-hub.preferences-member-account-discount-visibility-roundtrip
Expected: ClientError, code ValidationException, HTTP 400; memberAccountDiscountVisibility unchanged on a following GetPreferences.
Actual: AssertionError: "real AWS rejects this call for a non-management account, but it SUCCEEDED and returned {'savingsEstimationMode': 'BeforeDiscounts', 'memberAccountDiscountVisibility': 'None', 'preferredCommitment': {'term': 'ThreeYears', 'paymentOption': 'AllUpfront'}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}".

[mismatch] cost-optimization-hub.update-preferences-invalid-discount-visibility-rejected
Expected: the within-enum value memberAccountDiscountVisibility="All" also rejected with ValidationException (400), distinguishable from the out-of-enum rejection; GetPreferences never reports the field.
Actual: AssertionError: "expected the call to be rejected, but it SUCCEEDED and returned {'savingsEstimationMode': 'BeforeDiscounts', 'memberAccountDiscountVisibility': 'All', 'preferredCommitment': {'term': 'OneYear', 'paymentOption': 'PartialUpfront'}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}".
Note: the preceding out-of-enum assertions (code ValidationException, fields=[{fieldName: memberAccountDiscountVisibility, message: 'Member must satisfy enum value set: [All, None]'}]) passed, so generic enum validation is correct and only the account-level restriction is missing.

[mismatch] cost-optimization-hub.preferences-savings-estimation-mode-roundtrip
Expected: UpdatePreferences(savingsEstimationMode="AfterDiscounts") -> ClientError code ValidationException (400); savingsEstimationMode stays BeforeDiscounts.
Actual: AssertionError: "real AWS rejects this call, but it SUCCEEDED and returned {'savingsEstimationMode': 'AfterDiscounts', 'preferredCommitment': {'term': 'ThreeYears', 'paymentOption': 'AllUpfront'}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}".
The BeforeDiscounts positive control and the GetPreferences echo in the same check passed, so UpdatePreferences is otherwise writable — the refusal, not the write path, is what is missing.

Model used: smithy_query operation=UpdatePreferences,GetPreferences and shape=SavingsEstimationMode,MemberAccountDiscountVisibility,ValidationException on services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/smithy-model/com-amazonaws-costoptimizationhub-costoptimizationhubservice.json.

## Steps to Reproduce
Boot the emulator and point a botocore client at it: coh = client('cost-optimization-hub', region='us-east-1').

Prerequisite for all three: ensure enrollment is Active — items = coh.list_enrollment_statuses()['items']; if empty or items[0]['status'] != 'Active': coh.update_enrollment_status(status='Active').

Cause 1:
1. coh.update_preferences(savingsEstimationMode='BeforeDiscounts')  # positive control, succeeds
2. coh.update_preferences(memberAccountDiscountVisibility='None')  (or 'All')
   Expected: ClientError, Error.Code == 'ValidationException', HTTP 400, no state change.
   Observed: HTTP 200 with the value echoed.
3. coh.get_preferences() -> observed: reports memberAccountDiscountVisibility; expected: member absent for a non-management account.
4. Also: coh.update_preferences(memberAccountDiscountVisibility='Everyone') correctly returns ValidationException with fields=[{'fieldName': 'memberAccountDiscountVisibility', 'message': 'Member must satisfy enum value set: [All, None]'}] — the within-enum call at step 2 must be rejected too, with a different message.

Cause 2:
1. coh.update_preferences(savingsEstimationMode='BeforeDiscounts'); coh.get_preferences() -> 'BeforeDiscounts' (both pass today).
2. coh.update_preferences(savingsEstimationMode='AfterDiscounts')
   Expected: ClientError, Error.Code == 'ValidationException', HTTP 400, savingsEstimationMode still 'BeforeDiscounts'.
   Observed: HTTP 200, 'AfterDiscounts' persisted and echoed by a following get_preferences().

Replay: the three checks listed in trace_ids.
