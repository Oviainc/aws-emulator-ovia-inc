Issue-Source: trace-sweep-divergence
Issue-Trace: qconnect-ai-guardrail-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: qconnect-ai-guardrail-lifecycle
Issue-Shape-Id: com.amazonaws.qconnect#WisdomService
Issue-Key: wisdom|GetAIGuardrail|2xx->2xx|$.aiGuardrail.aiGuardrailArn
Issue-Key: wisdom|GetAIGuardrail|2xx->2xx|$.aiGuardrail.blockedInputMessaging
Issue-Key: wisdom|GetAIGuardrail|2xx->2xx|$.aiGuardrail.blockedOutputsMessaging
Issue-Key: wisdom|GetAIGuardrail|2xx->2xx|$.aiGuardrail.contentPolicyConfig
Issue-Key: wisdom|GetAIGuardrail|2xx->2xx|$.aiGuardrail.visibilityStatus
Issue-Commit: f5df567e1e8a9531fafde50d7cd95a16d29857a0

# QConnect GetAIGuardrail serves the create-time guardrail snapshot and an unqualified aiGuardrailArn

> _Note: this issue was identified against the code base at commit `f5df567e1e8a9531fafde50d7cd95a16d29857a0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAIGuardrail

## Description
Reading back an AI Guardrail in the Amazon Q in Connect (wire service `wisdom`, model com.amazonaws.qconnect) emulator does not describe the guardrail's current working copy: the ARN is missing the version qualifier AWS reports, and every member a preceding mutation changed is served at its create-time value. Two causes, both observed on the same replayed read.

Cause 1 of 2 — aiGuardrailArn omits the `:$LATEST` version qualifier (fixture qconnect-ai-guardrail-lifecycle, step task#5).
AWS: recorded behavior only. The Smithy model declares GetAIGuardrail.aiGuardrail -> AIGuardrailData.aiGuardrailArn as `Arn` (required) whose pattern does not express a qualifier, so the model says nothing about it; the recorded 200 response shows AWS returning the AI-Guardrail ARN with the resource's version qualifier appended, i.e. `arn:<partition>:wisdom:<region>:<account>:ai-guardrail/<assistantId>/<aiGuardrailId>:$LATEST` for the unversioned working copy. The fixed text is the `arn:...:wisdom:...:ai-guardrail/` prefix and the trailing `:$LATEST` qualifier; the partition, region, account id, assistantId and aiGuardrailId are values from the request and the emulator's state, never literals from one run.
Emulator: returns the same ARN with no qualifier at all — `.../ai-guardrail/<assistantId>/<aiGuardrailId>` — at HTTP 200.
Why: a client that keys guardrail references, IAM resource matching or AI-agent/assistant guardrail associations off the ARN it read back builds an identifier AWS never issues, and a round-trip comparison of a stored ARN against a freshly read one never matches.
Fix: the GetAIGuardrail handler / AIGuardrailData serializer in the qconnect service crate's AI-Guardrail read path — the ARN must be rendered with the version qualifier of the version being read, not the bare resource path.

Cause 2 of 2 — the read does not reflect the guardrail's mutated state: visibilityStatus, blockedInputMessaging, blockedOutputsMessaging and contentPolicyConfig are all served at their create-time values (fixture qconnect-ai-guardrail-lifecycle, step task#5).
AWS: Smithy: UpdateAIGuardrail (POST /assistants/{assistantId}/aiguardrails/{aiGuardrailId}, output aiGuardrail -> AIGuardrailData) takes required `visibilityStatus` (VisibilityStatus: SAVED | PUBLISHED), required `blockedInputMessaging` and `blockedOutputsMessaging` (AIGuardrailBlockedMessaging, @length 1..500, @sensitive) and optional `contentPolicyConfig` (AIGuardrailContentPolicyConfig with required filtersConfig, @length 1..6). GetAIGuardrail is @readonly and returns the same AIGuardrailData, so a mutation to the guardrail is visible on the next read: the recorded 200 reports the post-update visibilityStatus PUBLISHED, the post-update blocked-input and blocked-output messages, and the contentPolicyConfig the mutation supplied (a filtersConfig entry with its type and its inputStrength/outputStrength).
Emulator: answers 200 but with the guardrail exactly as created — visibilityStatus `SAVED` where the recording has `PUBLISHED`, the create-time blocked-input and blocked-output messages where the recording has the updated ones, and `contentPolicyConfig` absent entirely where the recording carries a filtersConfig list. Note that UpdateAIGuardrail itself is still an unimplemented scaffold (services/service-com-amazonaws-qconnect-wisdomservice/src/ops/update_ai_guardrail.rs answers 501 NotImplemented); that stub has its own auto-filed issue, but this read-side divergence is reported separately and is not closed by it: the read path must serve whatever the guardrail's stored state says, including a visibilityStatus and policy configs that were set after creation.
Why: a client that publishes a guardrail and then reads it to confirm sees a guardrail still in SAVED with no content policy, so it concludes the publish and the content filters never took effect and either loops retrying or routes traffic through a guardrail it believes is unconfigured.
Fix: the AI-Guardrail state in the qconnect service crate plus the GetAIGuardrail handler that projects it — the stored guardrail record must hold visibilityStatus, both blocked-messaging strings and every optional policy config, and the read must return the current stored values rather than a creation snapshot. (Writing them is the UpdateAIGuardrail handler's half.)

Traces: qconnect-ai-guardrail-lifecycle#task#5 (GetAIGuardrail, expected 200 / actual 200, 5 diverging members).

## Evidence
Replayed step qconnect-ai-guardrail-lifecycle#task#5, operation GetAIGuardrail, verdict mismatch, expected_status 200, actual_status 200. Reported member diffs:
  $.aiGuardrail.aiGuardrailArn: expected an ARN ending in the `:$LATEST` version qualifier (recorded: 'arn:aws:wisdom:us-west-2:000000000000:ai-guardrail/<assistantId>/<aiGuardrailId>:$LATEST'), got the same ARN with no qualifier ('arn:aws:wisdom:us-west-2:000000000000:ai-guardrail/<assistantId>/<aiGuardrailId>').
  $.aiGuardrail.visibilityStatus: expected 'PUBLISHED', got 'SAVED'.
  $.aiGuardrail.blockedInputMessaging: expected the updated blocked-input message supplied by the preceding mutation, got the create-time message.
  $.aiGuardrail.blockedOutputsMessaging: expected the updated blocked-output message supplied by the preceding mutation, got the create-time message.
  $.aiGuardrail.contentPolicyConfig: expected a filtersConfig list (recorded: one entry with type and HIGH input/output strength), got None (member absent).
Model checked: services/service-com-amazonaws-qconnect-wisdomservice/smithy-model/com-amazonaws-qconnect-wisdomservice.json — GetAIGuardrail (@readonly, GET /assistants/{assistantId}/aiguardrails/{aiGuardrailId}, 200), UpdateAIGuardrail, AIGuardrailData (aiGuardrailArn/visibilityStatus/blockedInputMessaging/blockedOutputsMessaging required; contentPolicyConfig optional), VisibilityStatus = SAVED | PUBLISHED.
UpdateAIGuardrail scaffold confirmed at services/service-com-amazonaws-qconnect-wisdomservice/src/ops/update_ai_guardrail.rs:192 (returns Scaffolded::not_implemented).

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 fixture qconnect-ai-guardrail-lifecycle against the emulator.
2. The fixture creates an assistant, creates an AI Guardrail under it, mutates the guardrail (publishing it, replacing both blocked-messaging strings and adding a content policy), then at step task#5 issues GET /assistants/{assistantId}/aiguardrails/{aiGuardrailId}.
3. Compare the task#5 response body against the recording: the emulator answers 200 with visibilityStatus 'SAVED', the create-time blockedInputMessaging/blockedOutputsMessaging, no contentPolicyConfig member, and an aiGuardrailArn with no ':$LATEST' version qualifier.
