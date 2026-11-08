Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-automated-reasoning-policy-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-automated-reasoning-policy-lifecycle bedrock-automated-reasoning-policy-unset-vs-present-description-preserved bedrock-automated-reasoning-policy-tagging-and-annotations bedrock-guardrail-tagging-lifecycle
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|GetAutomatedReasoningPolicy|2xx->2xx|$.definitionHash
Issue-Key: bedrock|UpdateAutomatedReasoningPolicy|2xx->2xx|$.definitionHash
Issue-Key: bedrock|CreateAutomatedReasoningPolicyVersion|2xx->4xx|$.createdAt
Issue-Key: bedrock|CreateAutomatedReasoningPolicyVersion|2xx->4xx|$.definitionHash
Issue-Key: bedrock|CreateAutomatedReasoningPolicyVersion|2xx->4xx|$.description
Issue-Key: bedrock|CreateAutomatedReasoningPolicyVersion|2xx->4xx|$.name
Issue-Key: bedrock|CreateAutomatedReasoningPolicyVersion|2xx->4xx|$.policyArn
Issue-Key: bedrock|CreateAutomatedReasoningPolicyVersion|2xx->4xx|$.version
Issue-Key: bedrock|CreateAutomatedReasoningPolicyVersion|2xx->4xx|status
Issue-Key: bedrock|ListTagsForResource|2xx->4xx|$.tags
Issue-Key: bedrock|ListTagsForResource|2xx->4xx|status
Issue-Key: bedrock|TagResource|2xx->4xx|$
Issue-Key: bedrock|TagResource|2xx->4xx|status
Issue-Key: bedrock|ListTagsForResource|2xx->2xx|$.tags[key="env"]
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# bedrock resource state defects: definitionHash, policy version creation and tag persistence

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAutomatedReasoningPolicy

## Description
SIX causes, one shared fault family: the control plane's persisted state for a resource does not match what AWS keeps, so reads and follow-on writes are wrong even though the request/response shapes are right. Three sub-faults: the policy definition hash is computed differently, a version cannot be cut from a policy that exists, and tag state is not bound to the resource (not found on a policy that exists; not removed from a guardrail after untag).

Cause 1 of 6: GetAutomatedReasoningPolicy returns a definitionHash that does not match the hash AWS derives from the same policy definition.
AWS: 200 with definitionHash, a 128-character lowercase-hex digest of the policy definition. Smithy: AutomatedReasoningPolicyHash @length 128..128, @pattern ^[0-9a-z]{128}$, required in the output. The requirement is that the SAME definition always yields the SAME digest AWS yields, because the value is the optimistic-concurrency token fed back into UpdateAutomatedReasoningPolicy and CreateAutomatedReasoningPolicyVersion as lastUpdatedDefinitionHash.
Emulator: 200 with a well-formed but different 128-hex digest for the same definition — the digest input or algorithm differs from AWS's.
Why: the hash is a concurrency token, so a client that computed/stored AWS's hash (or compares the hash across a read-modify-write) cannot round-trip it; any fixture or client asserting a known definition's hash fails. Reported on 4 steps (also bedrock-automated-reasoning-policy-unset-vs-present-description-preserved#task#3 and #task#4).
Fix: the automated-reasoning-policy state's definition-hash computation (canonical form of the definition that is hashed, and the digest function).
Traces: bedrock-automated-reasoning-policy-lifecycle#task#2.

Cause 2 of 6: UpdateAutomatedReasoningPolicy returns the same wrongly-derived definitionHash for the updated definition.
AWS: 200 with the 128-hex digest of the post-update definition.
Emulator: 200 with a different 128-hex digest.
Why: the token returned from the update cannot be used against AWS-derived expectations; read-modify-write loops that re-use it diverge.
Fix: same hash computation, applied in the update path.
Traces: bedrock-automated-reasoning-policy-lifecycle#task#4.

Cause 3 of 6: CreateAutomatedReasoningPolicyVersion on an existing policy is refused 400 instead of cutting a version.
AWS: 200 with policyArn (the versioned '<policy-arn>:<version>' form), version, name, description, definitionHash and createdAt. Smithy: CreateAutomatedReasoningPolicyVersion requires lastUpdatedDefinitionHash (128-hex) and returns policyArn/version/name/definitionHash/createdAt as required members; version matches ^([1-9][0-9]{0,11})$.
Emulator: 400 with none of the output members — the version is never created.
Why: versioning is unusable, so the whole publish/promote flow (and any export or inference bound to a policy version) is blocked. Hypothesis: the refusal is a consequence of cause 1 — the caller's lastUpdatedDefinitionHash, read back from this emulator, is checked against a differently-recomputed hash — but the replay shows only the 400.
Fix: CreateAutomatedReasoningPolicyVersion's handler and its lastUpdatedDefinitionHash pre-condition, plus the policy-version state.
Traces: bedrock-automated-reasoning-policy-lifecycle#task#6.

Cause 4 of 6: ListTagsForResource on an automated-reasoning-policy ARN that exists answers 404 instead of returning the policy's tags.
AWS: 200 with tags, the list of {key, value} pairs currently on the resource. Smithy: ListTagsForResource resourceARN pattern explicitly admits ':automated-reasoning-policy/[a-zA-Z0-9]+(:[a-zA-Z0-9]+)?'; output tags is a TagList.
Emulator: 404 ResourceNotFoundException with no tags member — the policy ARN minted moments earlier is not recognised as a taggable resource. Reported on 3 steps of this fixture (task#2, task#4, task#6).
Why: tags on automated-reasoning policies are unreadable, so tag-based discovery, cost allocation and the tag-quota path are all unexercisable.
Fix: the tagging state's ARN resolution for automated-reasoning-policy ARNs (including the versioned ':<version>' suffix).
Traces: bedrock-automated-reasoning-policy-tagging-and-annotations#task#2.

Cause 5 of 6: TagResource on the same existing automated-reasoning-policy ARN answers 404 instead of tagging it.
AWS: 200 with an empty body (TagResource has no output members).
Emulator: 404 with body {'__type': 'ResourceNotFoundException', 'message': 'no resource <policyArn> exists in this account'}.
Why: tags can never be attached to an automated-reasoning policy, so cause 4 can never be satisfied either.
Fix: same ARN resolution in the tagging state/handler.
Traces: bedrock-automated-reasoning-policy-tagging-and-annotations#task#3.

Cause 6 of 6: after untagging a guardrail, ListTagsForResource still returns the removed tag.
AWS: 200 with the tag absent from tags once UntagResource has removed its key.
Emulator: 200 with the removed tag still present in tags.
Why: untag is not durable, so tag state diverges permanently after the first removal and any 'remove then verify' flow fails.
Fix: the tagging state's removal path for guardrail ARNs.
Traces: bedrock-guardrail-tagging-lifecycle#task#6.

Related open issue that does NOT cover these steps (no replay key for them): 20261029T172312Z-bedrock-reopens-bedrock-tagresource-an-automated-reasoning-policy-arn.md, which reports the same ARN-resolution refusal from the static pass.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline.

Cause 1 — bedrock-automated-reasoning-policy-lifecycle#task#2 GetAutomatedReasoningPolicy (200/200): $.definitionHash value_mismatch — expected the recorded 128-hex digest for that definition, got a different 128-hex digest. Cluster also covers bedrock-automated-reasoning-policy-lifecycle#task#5, bedrock-automated-reasoning-policy-unset-vs-present-description-preserved#task#3 and #task#4 (affected_count 4).

Cause 2 — bedrock-automated-reasoning-policy-lifecycle#task#4 UpdateAutomatedReasoningPolicy (200/200): $.definitionHash value_mismatch, same shape of divergence.

Cause 3 — bedrock-automated-reasoning-policy-lifecycle#task#6 CreateAutomatedReasoningPolicyVersion: expected 200, got 400; $.policyArn, $.version ('1'), $.name, $.description, $.definitionHash and $.createdAt all missing from the response.

Cause 4 — bedrock-automated-reasoning-policy-tagging-and-annotations#task#2 ListTagsForResource: expected 200 with $.tags [{'key': 'purpose', 'value': 'probe'}], got 404 with no tags member (affected_count 3: task#2, task#4, task#6).

Cause 5 — bedrock-automated-reasoning-policy-tagging-and-annotations#task#3 TagResource: expected 200 with an empty body {}, got 404 with {'__type': 'ResourceNotFoundException', 'message': 'no resource <automated-reasoning-policy arn> exists in this account'}.

Cause 6 — bedrock-guardrail-tagging-lifecycle#task#6 ListTagsForResource (200/200): $.tags[key="env"] reported 'extra' — expected absent after untag, got {'key': 'env', 'value': 'probe'} still present.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay bedrock-automated-reasoning-policy-lifecycle#task#2, #task#4 and #task#6; bedrock-automated-reasoning-policy-tagging-and-annotations#task#2 and #task#3; bedrock-guardrail-tagging-lifecycle#task#6.
3. Causes 1-2: create a policy with a known definition, read it back and compare definitionHash with the recorded digest for that definition. Cause 3: feed that definitionHash back as lastUpdatedDefinitionHash to CreateAutomatedReasoningPolicyVersion — it returns 400 instead of a version. Causes 4-5: TagResource then ListTagsForResource on the automated-reasoning-policy ARN just minted — both 404. Cause 6: TagResource a guardrail, UntagResource the key, then ListTagsForResource — the tag is still listed.
