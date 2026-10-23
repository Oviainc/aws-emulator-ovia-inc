Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bcm-pricing-calculator.bill-estimate-generation-full-lifecycle-gated bcm-pricing-calculator.preferences-update-get-roundtrip bcm-pricing-calculator.bill-scenario-create-get-delete-lifecycle bcm-pricing-calculator.bill-scenario-usage-modification-create-list-delete-roundtrip bcm-pricing-calculator.concurrent-create-bill-scenario-same-client-token-dedupes bcm-pricing-calculator.create-bill-scenario-duplicate-client-token-mismatch-conflict-rejected bcm-pricing-calculator.tag-resource-reserved-prefix-key-rejected
Issue-Shape-Id: com.amazonaws.bcmpricingcalculator#AWSBCMPricingCalculator
Issue-Key: bcm-pricing-calculator.bill-estimate-generation-full-lifecycle-gated|run#3|AssertionError|assert "memberAccountRateTypeSelections" in preferences, (
Issue-Key: bcm-pricing-calculator.preferences-update-get-roundtrip|run#4|AssertionError|assert "memberAccountRateTypeSelections" in original, (
Issue-Key: bcm-pricing-calculator.bill-scenario-create-get-delete-lifecycle|_member_account_refusal#8|AssertionError|raise AssertionError(f"expected a member-account refusal, call succeeded with {result!r}")
Issue-Key: bcm-pricing-calculator.bill-scenario-usage-modification-create-list-delete-roundtrip|run#5|AssertionError|raise AssertionError(
Issue-Key: bcm-pricing-calculator.concurrent-create-bill-scenario-same-client-token-dedupes|run#13|AssertionError|assert all(e is not None for e in errors), (
Issue-Key: bcm-pricing-calculator.create-bill-scenario-duplicate-client-token-mismatch-conflict-rejected|_error#6|AssertionError|raise AssertionError("expected the call to be rejected, but it succeeded")
Issue-Key: bcm-pricing-calculator.tag-resource-reserved-prefix-key-rejected|run#8|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# BCM Pricing Calculator: GetPreferences returns an empty body, and the member-account gate on BillScenario/UpdatePreferences/TagResource validation is missing

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPreferences

## Description
Four distinct root causes in the bcm-pricing-calculator emulator, all grounded in failing behavioral checks of this one service. A fixer must close ALL FOUR before submitting.

=== Cause 1 of 4: GetPreferences answers with an empty JSON body ({}), reporting no rate-type selections at all ===
Failing checks: bcm-pricing-calculator.bill-estimate-generation-full-lifecycle-gated, bcm-pricing-calculator.preferences-update-get-roundtrip

AWS: GetPreferences is a read of the account's effective rate-type preferences. Smithy (com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator.json): GetPreferences has an empty input and output GetPreferencesResponse with members managementAccountRateTypeSelections, memberAccountRateTypeSelections, standaloneAccountRateTypeSelections, each targeting RateTypes with @length min 1 max 3; modeled errors DataUnavailableException/AccessDeniedException/InternalServerException/ThrottlingException/ValidationException. The recorded behavior shows GetPreferences succeeding on an untouched account and reporting the account's own selection — the recordings use GetPreferences as a positive control and require memberAccountRateTypeSelections to be present in the response of a fresh account that never called UpdatePreferences. So the effective selections are always reported, not only after a write.

Emulator: HTTP 200 with a body of exactly two bytes (content-length: 2, i.e. `{}`); the parsed response carries only ResponseMetadata and no selection members. The handler copies crate::state::Preferences out of the store and emits each member as-is, so an account whose preferences were never written reports nothing.

Why: a client reading its rate-type preferences gets no selections back and cannot tell "no preference exists" from "the service answered nothing". Any caller that reads a selection before deciding what to write (the read-modify-write pattern these recordings exercise) sees a KeyError/absent member where AWS always supplies a value, and a @length min 1 list is never legitimately empty.

Fix: the GetPreferences handler and the preferences record in this service's state — the stored preferences record needs the account's effective selections present for an account that never wrote them, so the read reports all three members.

Traces: bcm-pricing-calculator.bill-estimate-generation-full-lifecycle-gated (assertion "GetPreferences did not return the expected shape"), bcm-pricing-calculator.preferences-update-get-roundtrip ("got keys ['ResponseMetadata']"). Handler: services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/src/ops/get_preferences.rs lines 133-157.

=== Cause 2 of 4: the BillScenario surface is not gated to management/payer accounts — CreateBillScenario and ListBillScenarios succeed where AWS refuses ===
Failing checks: bcm-pricing-calculator.bill-scenario-create-get-delete-lifecycle, bcm-pricing-calculator.bill-scenario-usage-modification-create-list-delete-roundtrip, bcm-pricing-calculator.concurrent-create-bill-scenario-same-client-token-dedupes, bcm-pricing-calculator.create-bill-scenario-duplicate-client-token-mismatch-conflict-rejected

AWS: recorded behavior. Against the account class these traces were recorded in (an AWS Organizations member account), CreateBillScenario and ListBillScenarios are both refused deterministically with HTTP 400 and error code ValidationException. The recorded message is "Operation not permitted for member accounts. This API is only allowed for regular or payer accounts." (example text — the requirement is the 400/ValidationException refusal of the whole BillScenario subject, read as well as write). The model corroborates that this is a documented account-class gate rather than a quirk: GetPreferences/UpdatePreferences model the three-way split management/member/standalone rate-type selections directly, and ValidationException carries a `reason` member of enum ValidationExceptionReason whose values include invalidRequestFromMember. Smithy: CreateBillScenario and ListBillScenarios both list ValidationException (httpError 400).

Emulator: both calls succeed. CreateBillScenario returns HTTP 200 with a full minted scenario (id, name, billInterval, status READY, createdAt, expiresAt) and persists it; ListBillScenarios likewise answers instead of refusing. Five concurrent CreateBillScenario racers all succeeded (errors list [None, None, None, None, None]). The handler implements the full mint/idempotency/quota path with no account-class check anywhere.

Why: a client that correctly handles the member-account refusal never exercises that path against the emulator, so its error handling is never covered; worse, the emulator ACCEPTS a mutation AWS rejects, so code validated against the emulator creates bill scenarios that real AWS will refuse, and the caller's control flow diverges at deploy time rather than in test. This is an overly-permissive divergence: the strongest class of fidelity defect.

Fix: the account-class gate belongs in the CreateBillScenario and ListBillScenarios handlers (or in shared request-admission validation for the BillScenario subject), returning the modeled ValidationException with reason invalidRequestFromMember, consistent with whatever account class this service's state presents the caller as.

Traces: bcm-pricing-calculator.bill-scenario-create-get-delete-lifecycle ("expected a member-account refusal, call succeeded with {...status: READY...}"), bcm-pricing-calculator.bill-scenario-usage-modification-create-list-delete-roundtrip ("CreateBillScenario unexpectedly succeeded in a member account"), bcm-pricing-calculator.concurrent-create-bill-scenario-same-client-token-dedupes ("expected every racer to be refused ... but at least one create_bill_scenario call succeeded"), bcm-pricing-calculator.create-bill-scenario-duplicate-client-token-mismatch-conflict-rejected ("expected the call to be rejected, but it succeeded"). Handler: services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/src/ops/create_bill_scenario.rs lines 182-308.

=== Cause 3 of 4: UpdatePreferences does not refuse the mutation, and ValidationException carries no `reason` ===
Failing check: bcm-pricing-calculator.preferences-update-get-roundtrip

AWS: recorded behavior. UpdatePreferences from the account class these traces were recorded in is rejected with HTTP 400, error code ValidationException, and the structured `reason` INVALID_REQUEST_FROM_MEMBER; GetPreferences afterwards must show all three selections unchanged. Smithy: UpdatePreferences input/output carry managementAccountRateTypeSelections / memberAccountRateTypeSelections / standaloneAccountRateTypeSelections (RateTypes, @length 1..3); it models ValidationException (httpError 400), and ValidationException has members message (required), reason (enum ValidationExceptionReason, values include invalidRequestFromMember) and fieldList. The recorded message "Operation not permitted for member accounts. This API is only allowed for regular or payer accounts." is an example; the requirements are the 400/ValidationException and the populated `reason`.

Emulator: this run never reached the UpdatePreferences call because Cause 1 failed the check's positive control first, so the refusal is unverified and the shipped handler applies no account-class gate (same gap as Cause 2). Hypothesis: UpdatePreferences currently stores the selections and answers 200. Once Cause 1 is fixed this check will proceed to the UpdatePreferences assertion, so the gate and the `reason` member must be in place for it to pass — fixing Cause 1 alone will not close this check.

Why: a client whose preference write AWS refuses sees it accepted by the emulator, and a client that branches on ValidationException.reason gets no reason at all, so it cannot distinguish an account-class refusal from a malformed-field refusal.

Fix: the UpdatePreferences handler's validation — refuse with the modeled ValidationException including `reason`, and leave the stored preferences untouched on refusal (the state must not be mutated before the gate runs).

Traces: bcm-pricing-calculator.preferences-update-get-roundtrip (asserts error Code ValidationException and response reason INVALID_REQUEST_FROM_MEMBER, then re-reads all three selections unchanged). Handler: services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/src/ops/update_preferences.rs.

=== Cause 4 of 4: TagResource does not reject a reserved `aws:` tag-key prefix — it answers NotImplemented ===
Failing check: bcm-pricing-calculator.tag-resource-reserved-prefix-key-rejected

AWS: recorded behavior. TagResource with a tag key in the reserved `aws:` namespace is refused with HTTP 400 and error code ValidationException; the recorded message is "System tags cannot be added/updated by requester" (example text — the rule is that a requester-supplied key in the reserved `aws:` prefix is refused). The rejected tag is not applied: ListTagsForResource afterwards returns no tags. A non-reserved key tags successfully. Smithy: TagResource input arn (Arn, required, @length 20..2048, pattern ^arn:aws[-a-z0-9]*:bcm-pricing-calculator:[-a-z0-9]*:[0-9]{12}:[-a-z0-9/:_]+$) and tags (Tags, required, @length 0..200); modeled errors include ValidationException (httpError 400) and ResourceNotFoundException (404).

Emulator: the call returns error code 'NotImplemented' instead of ValidationException — TagResource is still a scaffold stub, so neither the reserved-prefix refusal nor the positive-control tagging path exists.

Why: a client relying on the reserved-prefix refusal gets an unmodeled NotImplemented code it has no handler for, and once the stub is replaced the reserved-prefix rule must still be present or the emulator will silently ACCEPT a system tag AWS refuses.

Fix: the TagResource handler and its tag-key validation, plus the tag state it writes so ListTagsForResource shows nothing after a refusal and the accepted key after a success. Note: the replay report already filed a separate issue for TagResource being an unimplemented scaffold stub (naming services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/src/ops/tag_resource.rs) covering two ERRORED checks; this cause is the additional reserved-`aws:`-prefix validation requirement that the bare implementation does not cover, and this check was reported as a mismatch, not under that issue.

Traces: bcm-pricing-calculator.tag-resource-reserved-prefix-key-rejected ("expected ValidationException, got 'NotImplemented'"). Handler: services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/src/ops/tag_resource.rs.

## Evidence
Suite: simparity-behavioral-aws-v1, 2/17 passed for bcm-pricing-calculator (15 failed; 8 of those errored checks are ALREADY FILED against unimplemented scaffold stubs ListWorkloadEstimateUsage / DeleteWorkloadEstimate / TagResource / BatchCreateWorkloadEstimateUsage and are excluded here).

Cause 1 — GetPreferences:
  Expected: HTTP 200 with managementAccountRateTypeSelections / memberAccountRateTypeSelections / standaloneAccountRateTypeSelections populated (RateTypes, @length 1..3).
  Actual:   HTTP 200, content-type application/x-amz-json-1.0, content-length: 2 (body `{}`); parsed response == {'ResponseMetadata': {...}} only.
  Assertion: "GetPreferences did not return the expected shape: {'ResponseMetadata': ...}" / "got keys ['ResponseMetadata']".

Cause 2 — CreateBillScenario / ListBillScenarios:
  Expected: HTTP 400, Error.Code == "ValidationException" (recorded message example: "Operation not permitted for member accounts. This API is only allowed for regular or payer accounts."), for BOTH the create and the independent list read.
  Actual:   HTTP 200 success. CreateBillScenario returned {'id': '<uuid>', 'name': 'sim2real-bs-...', 'billInterval': {'start': ..., 'end': ...}, 'status': 'READY', 'createdAt': ..., 'expiresAt': ...} and persisted the scenario. Five concurrent same-clientToken racers all succeeded: errors == [None, None, None, None, None].
  Assertions: "expected a member-account refusal, call succeeded with {...}"; "CreateBillScenario unexpectedly succeeded in a member account"; "expected every racer to be refused ... but at least one create_bill_scenario call succeeded"; "expected the call to be rejected, but it succeeded".

Cause 3 — UpdatePreferences:
  Expected: HTTP 400, Error.Code == "ValidationException" with a populated ValidationException.reason (recorded: INVALID_REQUEST_FROM_MEMBER; model enum ValidationExceptionReason includes invalidRequestFromMember), and all three selections unchanged on the following GetPreferences.
  Actual:   not reached in this run — the check aborted on the Cause 1 GetPreferences positive control, and no account-class gate exists in the handlers. Unverified, so it must be implemented for this check to pass once Cause 1 is fixed.

Cause 4 — TagResource with a reserved key:
  Expected: HTTP 400, Error.Code == "ValidationException" (recorded message example: "System tags cannot be added/updated by requester"), rejected tag NOT applied (ListTagsForResource -> {}), and a non-reserved key {'env': 'ok'} applied successfully.
  Actual:   Error.Code == 'NotImplemented' — "operation TagResource is scaffolded but not implemented in this emulator".
  Assertion: "expected ValidationException, got 'NotImplemented'".

## Steps to Reproduce
Boot the emulator from this checkout and replay, against the bcm-pricing-calculator endpoint:

Cause 1: call GetPreferences with no prior UpdatePreferences. Observe a 200 whose body is `{}` (content-length 2) with none of the three rate-type selection members present. Replay check bcm-pricing-calculator.bill-estimate-generation-full-lifecycle-gated and bcm-pricing-calculator.preferences-update-get-roundtrip.

Cause 2: call CreateBillScenario(name="sim2real-bs-abc123"). Observe a 200 with a minted id and status READY instead of a 400 ValidationException. Then call ListBillScenarios and observe a 200 instead of the identical 400 refusal. Repeat CreateBillScenario five times concurrently with the SAME clientToken and observe every racer succeed. Replay checks bcm-pricing-calculator.bill-scenario-create-get-delete-lifecycle, bcm-pricing-calculator.bill-scenario-usage-modification-create-list-delete-roundtrip, bcm-pricing-calculator.concurrent-create-bill-scenario-same-client-token-dedupes, bcm-pricing-calculator.create-bill-scenario-duplicate-client-token-mismatch-conflict-rejected.

Cause 3: call UpdatePreferences(managementAccountRateTypeSelections=["BEFORE_DISCOUNTS"], memberAccountRateTypeSelections=["AFTER_DISCOUNTS"], standaloneAccountRateTypeSelections=["AFTER_DISCOUNTS_AND_COMMITMENTS"]). Expect a 400 ValidationException carrying reason INVALID_REQUEST_FROM_MEMBER and a following GetPreferences showing the selections unchanged. Replay check bcm-pricing-calculator.preferences-update-get-roundtrip (fix Cause 1 first so the check reaches this call).

Cause 4: CreateWorkloadEstimate(name="sim2real-we-abc123"), then TagResource(arn="arn:aws:bcm-pricing-calculator::<account>:workload-estimate/<id>", tags={"aws:reserved": "x"}). Observe error code NotImplemented instead of a 400 ValidationException; then ListTagsForResource must be empty and TagResource with {"env": "ok"} must succeed. Replay check bcm-pricing-calculator.tag-resource-reserved-prefix-key-rejected.
