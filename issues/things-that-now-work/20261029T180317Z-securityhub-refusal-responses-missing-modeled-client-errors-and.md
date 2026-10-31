Issue-Source: trace-sweep-divergence
Issue-Trace: securityhub-insight-lifecycle
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: securityhub-insight-lifecycle securityhub-action-target-lifecycle securityhub-automation-rule-and-members-lifecycle
Issue-Shape-Id: com.amazonaws.securityhub#SecurityHubAPIService
Issue-Key: securityhub|GetInsightResults|4xx->2xx|$.Code
Issue-Key: securityhub|GetInsightResults|4xx->2xx|$.Message
Issue-Key: securityhub|GetInsightResults|4xx->2xx|$.RequestId
Issue-Key: securityhub|GetInsightResults|4xx->2xx|$.Type
Issue-Key: securityhub|GetInsightResults|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: securityhub|GetInsightResults|4xx->2xx|status
Issue-Key: securityhub|DeleteActionTarget|4xx->4xx|$.Message
Issue-Key: securityhub|DeleteActionTarget|4xx->4xx|$.RequestId
Issue-Key: securityhub|DeleteActionTarget|4xx->4xx|$.Type
Issue-Key: securityhub|BatchGetAutomationRules|2xx->2xx|$.UnprocessedAutomationRules[*].ErrorMessage
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# securityhub refusal responses: missing modeled client errors and emulator-authored error vocabulary

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInsightResults

## Description
Three securityhub refusal paths diverge from AWS: one does not refuse at all, and two emit emulator-authored text where AWS emits a fixed error envelope/vocabulary. 3 causes.

Cause 1 of 3 — GetInsightResults on an insight ARN that does not exist (fixture securityhub-insight-lifecycle, step task#8).
AWS: Smithy: GetInsightResults GET /insights/results/{InsightArn+}, errors InvalidInputException (httpError 400, members Message + Code), ResourceNotFoundException (404). Recorded behavior: for an insight ARN with no insight behind it SecurityHub answers HTTP 400 with body Code='InvalidInputException', Type='InvalidInputException', Message='Cannot find any insight for the given Arn', a RequestId, and header x-amzn-errortype. The whole message here is FIXED text — it interpolates nothing from the request; the only per-run values in the response are RequestId and the errortype header's trailing marker.
Emulator: answers HTTP 200 with a success body: status expected 400, got 200, and $.Code, $.Type, $.Message, $.RequestId and $headers.x-amzn-errortype are all absent.
Why: a client that deletes an insight (or mistypes an ARN) and then reads its results sees a successful empty result instead of a refusal, so error handling and retry/cleanup branches guarded on InvalidInputException never execute and the caller treats a dangling ARN as a live insight.
Fix: the GetInsightResults handler's lookup/validation path — it must resolve InsightArn against insight state and raise the modeled InvalidInputException (400) with the fixed message instead of returning a default success payload.

Cause 2 of 3 — DeleteActionTarget not-found error body is incomplete and re-worded (fixture securityhub-action-target-lifecycle, step task#8).
AWS: Smithy: DeleteActionTarget DELETE /actionTargets/{ActionTargetArn+}, error ResourceNotFoundException (httpError 404, members Message + Code). Recorded behavior: deleting an already-deleted/unknown custom action target answers 404 with Type='ResourceNotFoundException', a RequestId, and Message of the form 'Given Arn: <ActionTargetArn> does not exist' — fixed text 'Given Arn: ' and ' does not exist', with the ARN taken verbatim from the request's ActionTargetArn label.
Emulator: status 404 is right, but the envelope is wrong: $.Type is missing, $.RequestId is missing, and $.Message is emulator prose of the form 'Custom action target <arn> does not exist' instead of AWS's 'Given Arn: <arn> does not exist'.
Why: SDKs and parsers classify a REST-JSON securityhub error from Type/x-amzn-errortype; with Type absent the error degrades to an unmodeled 404 and `except ResourceNotFoundException` does not match, while the reworded message breaks assertions and log-matching built against AWS text.
Fix: the DeleteActionTarget handler's refusal path and the service's error serializer — populate Type and RequestId on every securityhub error body and emit the AWS message template.

Cause 3 of 3 — BatchGetAutomationRules reports unprocessed rules with prose instead of the exception name (fixture securityhub-automation-rule-and-members-lifecycle, step task#6).
AWS: Smithy: BatchGetAutomationRulesResponse.UnprocessedAutomationRules -> UnprocessedAutomationRule{RuleArn, ErrorCode:Integer, ErrorMessage:NonEmptyString}. Recorded behavior: for an automation-rule ARN that does not exist the batch succeeds (200) and the per-item ErrorMessage is the exception name 'ResourceNotFoundException' — fixed text, with the identifier carried in the sibling RuleArn member, not in the message.
Emulator: ErrorMessage is a composed sentence, 'No automation rule with ARN <rule-arn> exists in this account and Region', interpolating the requested ARN.
Why: callers branch on the per-item ErrorMessage to distinguish a missing rule from a throttle or access failure; emulator prose matches none of AWS's values, so partial-failure handling in a batch read misclassifies every unprocessed entry.
Fix: the BatchGetAutomationRules handler's unprocessed-item construction — set ErrorMessage to the exception name for the per-item failure class and leave the ARN to RuleArn.

Traces: securityhub-insight-lifecycle#task#8 (GetInsightResults), securityhub-action-target-lifecycle#task#8 (DeleteActionTarget), securityhub-automation-rule-and-members-lifecycle#task#6 (BatchGetAutomationRules).

## Evidence
Cause 1 — securityhub-insight-lifecycle#task#8 GetInsightResults: status: expected 400, got 200; $.Code missing (expected 'InvalidInputException'); $.Type missing (expected 'InvalidInputException'); $.Message missing (expected 'Cannot find any insight for the given Arn'); $.RequestId missing; $headers.x-amzn-errortype missing.
Cause 2 — securityhub-action-target-lifecycle#task#8 DeleteActionTarget (404 vs 404): $.Type missing (expected 'ResourceNotFoundException'); $.RequestId missing; $.Message value_mismatch — expected 'Given Arn: <action-target-arn> does not exist', got 'Custom action target <action-target-arn> does not exist'.
Cause 3 — securityhub-automation-rule-and-members-lifecycle#task#6 BatchGetAutomationRules (200 vs 200): $.UnprocessedAutomationRules[0].ErrorMessage value_mismatch — expected 'ResourceNotFoundException', got 'No automation rule with ARN <rule-arn> exists in this account and Region'.

## Steps to Reproduce
Replay the simparity-wire-aws-v1 corpus against the emulator (replay_wire_corpus). Cause 1: fixture securityhub-insight-lifecycle step task#8 — create an insight, delete it, then call GetInsightResults on its ARN; expect 400 InvalidInputException 'Cannot find any insight for the given Arn'. Cause 2: fixture securityhub-action-target-lifecycle step task#8 — DeleteActionTarget twice on the same custom action target ARN; inspect the second response's 404 body for Type/RequestId and the 'Given Arn: ... does not exist' message. Cause 3: fixture securityhub-automation-rule-and-members-lifecycle step task#6 — BatchGetAutomationRules with an automation-rule ARN that was never created and read UnprocessedAutomationRules[0].ErrorMessage.
