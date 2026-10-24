Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: compute-optimizer-automation.list-automation-rule-preview-structure
Issue-Shape-Id: com.amazonaws.computeoptimizerautomation#ComputeOptimizerAutomationService
Issue-Key: compute-optimizer-automation.list-automation-rule-preview-structure|run#15|AssertionError|assert error.get("Message") == expected_message, (
Issue-Commit: 685e94dfb816aa33c7cd8ca632f897bdb6a87341

# compute-optimizer-automation: ListAutomationRulePreview enum rejection emits a generic constraint-validation message instead of the request-built 'Invalid RecommendedActionType value' message

> _Note: this issue was identified against the code base at commit `685e94dfb816aa33c7cd8ca632f897bdb6a87341`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAutomationRulePreview

## Description
One net-new root cause in this service, distinct from the two message-construction causes already filed in 20261023T134046Z-compute-optimizer-automation-error-messages-not-built-from.md (those cover UpdateAutomationRule not-found text and GetAutomationRule malformed-ARN access-denied text; neither touches ListAutomationRulePreview input validation). Note also that this check was previously blocked by the unimplemented ListAutomationRulePreviewSummaries stub; that stub now answers and the check progresses to this validation divergence.

Cause 1 of 1: an unrecognized value in ListAutomationRulePreview.recommendedActionTypes is rejected with the right error code but with a generic framework constraint-validation message instead of the service's own message naming the offending value and the valid set.

AWS: Smithy (services/service-com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice/smithy-model/com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice.json): ListAutomationRulePreview input member recommendedActionTypes is required, target RecommendedActionTypeList whose member targets the enum RecommendedActionType with values [SnapshotAndDeleteUnattachedEbsVolume, UpgradeEbsVolumeType]; the operation models InvalidParameterValueException (client, httpError 400). Recorded behavior: a value outside that enum is rejected 400 InvalidParameterValueException with a message assembled from the request and the modeled enum set, of the form 'Invalid RecommendedActionType value: <the value exactly as sent>. Valid values: SnapshotAndDeleteUnattachedEbsVolume, UpgradeEbsVolumeType' (recorded example value: NotARealActionType). Every component of that message comes from the request member and the model's enum values, so the construction rule is known and is a requirement, not an incidental recorded string.

Emulator: returns InvalidParameterValueException with HTTP 400 (correct code and status) but the generic shape-validation text '1 validation error detected: Value at 'recommendedActionTypes.1.member' failed to satisfy constraint: Member must satisfy enum value set: [SnapshotAndDeleteUnattachedEbsVolume, UpgradeEbsVolumeType]' — i.e. the generic constraint-violation formatter is answering instead of the operation's own parameter validation. It never echoes the rejected value, and it reports a member path (an index plus a '.member' segment) where AWS names the parameter type.

Why: a client that received only the generic text cannot tell WHICH value it sent was rejected — the offending token is absent, so retry/repair logic and operator tooling that parse the rejected value out of the message (to drop or map an unsupported action type) have nothing to parse. The indexed member path is also a different shape of diagnostic from AWS's, so sim-vs-real assertions on this rejection class diverge for every unrecognized enum value, not just this one.

Fix: ListAutomationRulePreview's input validation / its InvalidParameterValueException construction in the compute-optimizer-automation service — validate recommendedActionTypes members in the operation's own validation path and build the message from the supplied value and the RecommendedActionType enum values, rather than letting the shared generic enum-constraint formatter produce the response. The sibling ListAutomationRulePreviewSummaries takes the identical required recommendedActionTypes member and should be corrected with it.

Traces: compute-optimizer-automation.list-automation-rule-preview-structure (all preceding assertions pass: the happy-path preview list and the summaries list are well-typed, their totals agree for the identical filter, and the error CODE for the bad enum value is already InvalidParameterValueException; only the message diverges).

## Evidence
Suite: simparity-behavioral-aws-v1 — compute-optimizer-automation 17/20 (85%), 3 failed (2 of them OWNED by 20261023T134046Z-compute-optimizer-automation-error-messages-not-built-from.md and excluded here).

[mismatch] compute-optimizer-automation.list-automation-rule-preview-structure
  check_compute_optimizer_automation_list_automation_rule_preview_structure.py:107
  assert error.get("Message") == expected_message
  AssertionError: expected message 'Invalid RecommendedActionType value: NotARealActionType. Valid values: SnapshotAndDeleteUnattachedEbsVolume, UpgradeEbsVolumeType', got "1 validation error detected: Value at 'recommendedActionTypes.1.member' failed to satisfy constraint: Member must satisfy enum value set: [SnapshotAndDeleteUnattachedEbsVolume, UpgradeEbsVolumeType]"

  Request: ListAutomationRulePreview(ruleType='AccountRule', recommendedActionTypes=['NotARealActionType'], maxResults=10)
  Expected (AWS, built from request + modeled enum): HTTP 400, Code InvalidParameterValueException, Message 'Invalid RecommendedActionType value: <value>. Valid values: SnapshotAndDeleteUnattachedEbsVolume, UpgradeEbsVolumeType'
  Actual (emulator):                                HTTP 400, Code InvalidParameterValueException (matches), Message = generic indexed constraint-validation text that omits the rejected value

Smithy evidence: ListAutomationRulePreview.recommendedActionTypes required -> RecommendedActionTypeList -> enum RecommendedActionType values [SnapshotAndDeleteUnattachedEbsVolume, UpgradeEbsVolumeType]; modeled error InvalidParameterValueException (client, httpError 400, single `message` member).

## Steps to Reproduce
1. Ensure the account's Compute Optimizer base enrollment is Active (compute-optimizer UpdateEnrollmentStatus(status='Active')) and the Compute Optimizer Automation enrollment is Active (UpdateEnrollmentConfiguration(status='Active'); poll GetEnrollmentConfiguration until status == 'Active').
2. Positive control: ListAutomationRulePreview(ruleType='AccountRule', recommendedActionTypes=['SnapshotAndDeleteUnattachedEbsVolume'], maxResults=100) and ListAutomationRulePreviewSummaries with the same filter — both succeed and their totals agree.
3. ListAutomationRulePreview(ruleType='AccountRule', recommendedActionTypes=['NotARealActionType'], maxResults=10).
   Observe: 400 InvalidParameterValueException with Message "1 validation error detected: Value at 'recommendedActionTypes.1.member' failed to satisfy constraint: Member must satisfy enum value set: [SnapshotAndDeleteUnattachedEbsVolume, UpgradeEbsVolumeType]".
   Expected: 400 InvalidParameterValueException with Message 'Invalid RecommendedActionType value: NotARealActionType. Valid values: SnapshotAndDeleteUnattachedEbsVolume, UpgradeEbsVolumeType'.

Replay: run_behavioral_suite and re-run compute-optimizer-automation.list-automation-rule-preview-structure.
