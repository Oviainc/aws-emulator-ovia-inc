Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: billing.get-billing-preferences-access-denied-without-billing-access billing.update-billing-preferences-access-denied-rejected billing.disassociate-source-views-from-primary-view-rejected billing.update-billing-view-nonexistent-arn-rejected billing.get-billing-view-cross-account-arn-access-denied-rejected billing.get-credit-allocation-history-returns-list-for-own-account billing.list-source-views-for-nonexistent-billing-view-rejected billing.tag-resource-managed-billing-view-rejected
Issue-Shape-Id: com.amazonaws.billing#AWSBilling
Issue-Key: billing.get-billing-preferences-access-denied-without-billing-access|_error#3|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: billing.update-billing-preferences-access-denied-rejected|_error#6|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: billing.disassociate-source-views-from-primary-view-rejected|run#9|AssertionError|assert code == "ValidationException", code
Issue-Key: billing.update-billing-view-nonexistent-arn-rejected|run#16|KeyError|assert after["description"] == before["description"], (before["description"], after["description"])
Issue-Key: billing.get-billing-view-cross-account-arn-access-denied-rejected|run#14|AssertionError|assert message == (
Issue-Key: billing.get-credit-allocation-history-returns-list-for-own-account|run#11|AssertionError|assert message == (
Issue-Key: billing.list-source-views-for-nonexistent-billing-view-rejected|run#8|AssertionError|assert message == f"[{fake_arn}] is an invalid billing view arn", message
Issue-Key: billing.tag-resource-managed-billing-view-rejected|run#8|AssertionError|assert message == "Resource not found", message
Issue-Commit: 67b7a4ea014b7cfd9cdb5213b26731b0db184c45

# Billing: 7 behavioral divergences — no per-feature access gate on GetBillingPreferences/UpdateBillingPreferences, DisassociateSourceViews 501 stub, PRIMARY view omits description, AccessDenied names the wrong caller identity, credit-history date rule and not-found texts

> _Note: this issue was identified against the code base at commit `67b7a4ea014b7cfd9cdb5213b26731b0db184c45`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBillingPreferences

## Description
Seven distinct root causes in the Billing (AWSBilling, awsJson1_0) emulator, found by replaying the behavioral suite (11/19 passed; all 8 failures are billing). No open issue exists for this service (list_existing_issues query='billing' -> none), so none of these are already covered.

Cause 1 of 7: GetBillingPreferences applies no per-feature access gate.
AWS: GetBillingPreferences models AccessDeniedException (HTTP 403, 'You don't have sufficient access to perform this action' — Smithy: GetBillingPreferences errors = AccessDeniedException/InternalServerException/ThrottlingException/ValidationException; AccessDeniedException.httpError 403). Recorded behavior: for a caller without Billing-console access activation, features tied to that activation (BILLING_ALERTS, RI_SHARING_HISTORY) are refused with 403 AccessDeniedException, while a baseline feature (CREDIT_SHARING) succeeds — the gate is per feature, not per operation. The recorded message is an example of AWS's form: '[Exception=AccessDeniedException][AccountId = <id>][Operation = GetBillingPreferencesPaperwork][Message=No access to feature <FEATURE>]'; the requirement is the 403 + AccessDeniedException code for the gated features.
Emulator: returns HTTP 200 with {'billingPreferences': []} for BILLING_ALERTS and RI_SHARING_HISTORY (replay: 'expected a rejection, but the call SUCCEEDED and returned {'billingPreferences': [], ... HTTPStatusCode: 200}').
Why: a client cannot distinguish 'feature not accessible to this caller' from 'feature accessible and empty'. Code that gates UI/automation on the access error silently treats the account as having the feature enabled with no preferences, and negative-path tests pass against the emulator and fail on AWS.
Fix: GetBillingPreferences' handler/authorization step — services/service-com-amazonaws-billing-awsbilling/src/ops/get_billing_preferences.rs (handle() goes straight from input validation to state read; no access decision exists), plus the per-feature activation state it must consult in src/state.rs.
Traces: billing.get-billing-preferences-access-denied-without-billing-access.

Cause 2 of 7: UpdateBillingPreferences accepts writes AWS refuses, including a write to a read-only history feature.
AWS: UpdateBillingPreferences models AccessDeniedException (HTTP 403) (Smithy: UpdateBillingPreferences errors = AccessDeniedException/InternalServerException/ThrottlingException/ValidationException). Recorded behavior: both a normally-writable feature (RI_SHARING) and a history feature (RI_SHARING_HISTORY) are refused with 403 AccessDeniedException for a caller lacking Billing-console access activation — the access gate is reached before any feature-specific read-only-key validation, so both attempts are denied. Recorded message, as an example: '[Exception=AccessDeniedException][AccountId = <id>][Operation = UpdateBillingPreferencesPaperwork][Message=No access to feature <FEATURE>]'.
Emulator: both calls return HTTP 200 with an empty body (replay: 'the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ... content-length: 2}}'), i.e. a write to RI_SHARING_HISTORY is accepted outright.
Why: the emulator is overly permissive on a mutating control-plane call: a client's write appears to land, and a history feature that AWS never lets a caller write is mutable here, so emulator state diverges from anything AWS would produce.
Fix: UpdateBillingPreferences' handler/authorization + feature-writability validation — services/service-com-amazonaws-billing-awsbilling/src/ops/update_billing_preferences.rs, sharing the activation state added for Cause 1 in src/state.rs.
Traces: billing.update-billing-preferences-access-denied-rejected.

Cause 3 of 7: DisassociateSourceViews is an unimplemented scaffold stub.
AWS: DisassociateSourceViews is a modeled operation (Smithy: input arn (BillingViewArn, required) + sourceViews (BillingViewSourceViewsList @length 1..10, required); errors AccessDeniedException 403, BillingViewHealthStatusException 400, ConflictException 409, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400). Recorded behavior: targeting the account's PRIMARY view is refused with 400 ValidationException (PRIMARY has no mutable source-view association); recorded message, as an example: 'Only custom billing views can have source views disassociated'.
Emulator: answers the scaffold's NotImplemented (HTTP 501) — the replayed client saw error code 'NotImplemented' where ValidationException was expected.
Why: every DisassociateSourceViews call fails with a non-AWS error code and status, so neither the success path nor any modeled rejection can be exercised; a client cannot tell an unsupported-operation 501 from a service fault.
Fix: implement the handler — services/service-com-amazonaws-billing-awsbilling/src/ops/disassociate_source_views.rs (still carries the '//! SCAFFOLDED, NOT IMPLEMENTED' marker and returns crate::scaffolded::Scaffolded::not_implemented at line 177).
Traces: billing.disassociate-source-views-from-primary-view-rejected.

Cause 4 of 7: the account's default PRIMARY billing view carries no description, so GetBillingView omits the member.
AWS: BillingViewElement.description (BillingViewDescription, @length 0..1024, pattern ^([ a-zA-Z0-9_\+=\.\-@]+)?$, sensitive) — 'The description of the billing view' (Smithy: GetBillingView output billingView -> BillingViewElement). Recorded behavior: GetBillingView on the account's own PRIMARY view returns a description member, so a read-compare-read round trip over the projection can read resp['billingView']['description'].
Emulator: the member is absent from the response entirely, because the seeded PRIMARY view is created with description: None (src/state.rs, Default impl for ComAmazonawsBillingAwsbillingState) and GetBillingView projects view.description straight through (src/ops/get_billing_view.rs line 191). Replay: KeyError: 'description' when the check compares the PRIMARY view before/after a rejected UpdateBillingView.
Why: any client that reads the description of the default view — a console-style projection, a diff of a view before and after a mutation, an SDK model that assumes the documented field is populated — gets a missing key rather than a value, and the 'effect absent' assertion over the projection cannot be made at all.
Fix: the PRIMARY-view seed in services/service-com-amazonaws-billing-awsbilling/src/state.rs (beside PRIMARY_VIEW_NAME) so the default view carries a description, and the projection in src/ops/get_billing_view.rs / list_billing_views.rs.
Traces: billing.update-billing-view-nonexistent-arn-rejected.

Cause 5 of 7: the cross-account AccessDeniedException message names a caller identity that is not the one STS GetCallerIdentity reports.
AWS: AccessDeniedException (HTTP 403) for a well-formed billing-view ARN scoped to a foreign account. The message is built from the request and the caller: 'User: <caller identity ARN> is not authorized to perform: billing:GetBillingView on resource: <arn> because no resource-based policy allows the billing:GetBillingView action' — the identity in it is the caller's own identity ARN, i.e. the Arn that GetCallerIdentity returns for the same credentials. That rule, not a fixed string, is the requirement.
Emulator: the status (403) and code (AccessDeniedException) are correct, but the identity is synthesized as a hardcoded root ARN — src/state.rs caller_identity_arn() returns engine::emulator_arn("iam", "", "root") — which does not match the Arn the emulator's own STS GetCallerIdentity returns for the same credentials, so the two components disagree. Replay: the produced message was 'User: arn:aws:iam::000000000000:root is not authorized to perform: billing:GetBillingView on resource: arn:aws:billing::111122223333:billingview/primary because no resource-based policy allows the billing:GetBillingView action' while the expectation built from GetCallerIdentity differed.
Why: clients and tests that parse the denied principal out of an AccessDenied message (or assert it equals their own GetCallerIdentity Arn) read a principal the account never authenticated as; IAM-debugging flows point at the wrong identity.
Fix: cross_account_access_denied()/caller_identity_arn() in services/service-com-amazonaws-billing-awsbilling/src/state.rs must take the caller identity from the same source STS GetCallerIdentity serves, rather than minting a root ARN.
Traces: billing.get-billing-view-cross-account-arn-access-denied-rejected.

Cause 6 of 7: GetCreditAllocationHistory's date-range rule is non-strict and its ValidationException does not carry AWS's field/detail form.
AWS: GetCreditAllocationHistory input startDate/endDate (Timestamp, both required) and ValidationException (HTTP 400, members message + required reason + fieldList) — Smithy. Recorded behavior: an inverted range is refused 400 ValidationException whose message names the offending member and the constraint it violated, in the form 'InvalidArgument: startDate (DetailedCode=startDate must be strictly before endDate (endDate is exclusive))' — i.e. endDate is treated as exclusive and startDate == endDate is NOT accepted. (The model's own member doc says 'Must be on or before endDate', which the recording contradicts; the recording is the wire behavior.)
Emulator: refuses with the right code (ValidationException) but states the weaker non-strict rule — 'startDate must be on or before endDate' — and does not name the member/detail in AWS's form, so a startDate == endDate request that AWS rejects is accepted here.
Why: a client that relies on endDate being exclusive gets a different window boundary (an extra/duplicated billing month) and a degenerate zero-width range silently succeeds where AWS returns 400.
Fix: the date-range validation in GetCreditAllocationHistory's handler — services/service-com-amazonaws-billing-awsbilling/src/ops/get_credit_allocation_history.rs — must enforce startDate strictly before endDate and emit the member-named ValidationException (message/reason/fieldList).
Traces: billing.get-credit-allocation-history-returns-list-for-own-account.

Cause 7 of 7: ResourceNotFoundException texts for billing ARNs are built differently from the recorded service.
AWS: ResourceNotFoundException (HTTP 404; required members message, resourceId, resourceType) — Smithy. Recorded behavior, per operation: ListSourceViewsForBillingView on a well-formed but nonexistent billingview ARN answers a message of the form '[<arn>] is an invalid billing view arn'; TagResource and ListTagsForResource on a nonexistent billingview ARN answer the fixed text 'Resource not found', i.e. AWS does NOT echo the requested ARN there.
Emulator: the status and code match in both cases, only the text diverges — ListSourceViewsForBillingView returns 'Billing view <arn> does not exist' (src/ops/get_billing_view.rs line 173 shows the same formatting convention used across this crate), and TagResource/ListTagsForResource return 'Resource <arn> not found'.
Why: callers that branch on, log-match or snapshot the not-found text for billing resources see a different sentence, and the tagging APIs leak the requested ARN back into a message where the service returns a fixed string. Note also that this failure stops billing.tag-resource-managed-billing-view-rejected before its later assertions, so TagResource/ListTagsForResource against the managed PRIMARY view (AWS: 400 ValidationException, 'Managed billing views cannot be tagged') were never exercised and must be verified when this is fixed.
Fix: the not-found error construction for these operations — services/service-com-amazonaws-billing-awsbilling/src/ops/list_source_views_for_billing_view.rs, src/ops/tag_resource.rs, src/ops/list_tags_for_resource.rs and the shared helpers in src/state.rs.
Traces: billing.list-source-views-for-nonexistent-billing-view-rejected, billing.tag-resource-managed-billing-view-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1. Total 11/19 passed (57.9%); billing 11/19 (58%), 8 failed — 7 mismatches + 1 error. All failures are in service 'billing' (bootstrapped as service-com-amazonaws-billing-awsbilling).

Cause 1 — billing.get-billing-preferences-access-denied-without-billing-access [mismatch]
  expected: 403 AccessDeniedException for features=['BILLING_ALERTS'] and features=['RI_SHARING_HISTORY']
  actual:   HTTP 200, {'billingPreferences': [], ResponseMetadata: {HTTPStatusCode: 200, content-type: application/x-amz-json-1.0}}
  harness: AssertionError: expected a rejection, but the call SUCCEEDED and returned {'billingPreferences': [] ...}
  (positive control features=['CREDIT_SHARING'] is expected to succeed, so a blanket 403 is NOT the fix.)

Cause 2 — billing.update-billing-preferences-access-denied-rejected [mismatch]
  expected: 403 AccessDeniedException for UpdateBillingPreferences(feature='RI_SHARING', ...) and (feature='RI_SHARING_HISTORY', ...)
  actual:   HTTP 200, empty body (content-length 2)
  harness: AssertionError: expected a rejection, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200 ...}}

Cause 3 — billing.disassociate-source-views-from-primary-view-rejected [mismatch]
  expected: error code 'ValidationException' (HTTP 400)
  actual:   error code 'NotImplemented' (HTTP 501, scaffold stub)
  harness: AssertionError: NotImplemented  (assert code == "ValidationException")
  source: src/ops/disassociate_source_views.rs:177 -> Err(crate::scaffolded::Scaffolded::not_implemented("DisassociateSourceViews")), file header still '//! SCAFFOLDED, NOT IMPLEMENTED'.

Cause 4 — billing.update-billing-view-nonexistent-arn-rejected [error]
  The two UpdateBillingView rejections (400 ValidationException, 'The Billing View ARN does not correspond to a Custom Billing View. Only Custom Billing Views may be edited.') PASSED; the check then died reading the projection:
  harness: KeyError: 'description' at after["description"] == before["description"] on GetBillingView(arn=<own PRIMARY arn>)['billingView']
  source: src/state.rs Default impl seeds the PRIMARY view with description: None; src/ops/get_billing_view.rs:191 passes it through, so the member is omitted from the awsJson1_0 body.

Cause 5 — billing.get-billing-view-cross-account-arn-access-denied-rejected [mismatch]
  code matched (AccessDeniedException); the message's principal did not.
  actual message: 'User: arn:aws:iam::000000000000:root is not authorized to perform: billing:GetBillingView on resource: arn:aws:billing::111122223333:billingview/primary because no resource-based policy allows the billing:GetBillingView action'
  expected form:  'User: <sts.get_caller_identity()["Arn"]> is not authorized to perform: billing:GetBillingView on resource: <arn> because no resource-based policy allows the billing:GetBillingView action'
  source: src/state.rs caller_identity_arn() -> ::engine::emulator_arn("iam", "", "root"), independent of what STS reports.

Cause 6 — billing.get-credit-allocation-history-returns-list-for-own-account [mismatch]
  code matched (ValidationException) for startDate=now, endDate=now-300d.
  actual message:   'startDate must be on or before endDate'
  recorded message: 'InvalidArgument: startDate (DetailedCode=startDate must be strictly before endDate (endDate is exclusive))'
  => the emulator's rule is non-strict (accepts startDate == endDate) and omits the member/DetailedCode form.

Cause 7 — billing.list-source-views-for-nonexistent-billing-view-rejected [mismatch] + billing.tag-resource-managed-billing-view-rejected [mismatch]
  ListSourceViewsForBillingView(arn=arn:aws:billing::<acct>:billingview/sim2real-behav-does-not-exist): code ResourceNotFoundException matched;
    actual message   'Billing view arn:aws:billing::000000000000:billingview/sim2real-behav-does-not-exist does not exist'
    recorded message '[<arn>] is an invalid billing view arn'
  TagResource(resourceArn=arn:aws:billing::<acct>:billingview/custom-a1b2c3d4-...): code ResourceNotFoundException matched;
    actual message   'Resource arn:aws:billing::000000000000:billingview/custom-a1b2c3d4-e5f6-47a8-9b0c-d1e2f3a4b5c6 not found'
    recorded message 'Resource not found'
  Because that assertion fires first, the same check's later assertions — TagResource and ListTagsForResource on the managed PRIMARY view returning 400 ValidationException 'Managed billing views cannot be tagged' — were never reached and remain unverified.

## Steps to Reproduce
Boot the emulator and point a botocore/boto3 'billing' client (awsJson1_0, sigv4) plus an 'sts' client at it, then:

1. Cause 1: billing.get_billing_preferences(features=['BILLING_ALERTS']) and (features=['RI_SHARING_HISTORY']) -> observe HTTP 200 {'billingPreferences': []}; AWS returns 403 AccessDeniedException. Confirm billing.get_billing_preferences(features=['CREDIT_SHARING']) still succeeds (positive control).
2. Cause 2: billing.update_billing_preferences(feature='RI_SHARING', billingPreferencesPerKey=[{'key':'k','value':'ENABLED'}]) and the same with feature='RI_SHARING_HISTORY' -> observe HTTP 200 empty body; AWS returns 403 AccessDeniedException for both.
3. Cause 3: arn = billing.list_billing_views(billingViewTypes=['PRIMARY'])['billingViews'][0]['arn']; billing.disassociate_source_views(arn=arn, sourceViews=['arn:aws:billing::<acct>:billingview/not-a-source']) -> observe 501 NotImplemented; AWS returns 400 ValidationException.
4. Cause 4: billing.get_billing_view(arn=arn)['billingView'] -> observe no 'description' key for the account's PRIMARY view.
5. Cause 5: identity = sts.get_caller_identity(); billing.get_billing_view(arn='arn:aws:billing::111122223333:billingview/primary') -> 403 AccessDeniedException whose message names arn:aws:iam::<acct>:root instead of identity['Arn'].
6. Cause 6: billing.get_credit_allocation_history(accountId=<own>, startDate=now, endDate=now-300d) -> 400 ValidationException with message 'startDate must be on or before endDate'; also call it with startDate == endDate and observe it is accepted (AWS rejects: endDate is exclusive).
7. Cause 7: billing.list_source_views_for_billing_view(arn='arn:aws:billing::<acct>:billingview/sim2real-behav-does-not-exist') and billing.tag_resource(resourceArn='arn:aws:billing::<acct>:billingview/custom-a1b2c3d4-e5f6-47a8-9b0c-d1e2f3a4b5c6', resourceTags=[{'key':'k','value':'x'}]) -> ResourceNotFoundException with the emulator's texts above. Then re-run billing.tag-resource-managed-billing-view-rejected end to end to verify the managed-PRIMARY ValidationException path.

Or replay the suite: run_behavioral_suite() and re-run the eight billing checks listed in trace_ids.
