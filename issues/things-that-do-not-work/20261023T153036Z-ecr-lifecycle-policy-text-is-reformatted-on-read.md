Issue-Source: trace-sweep-divergence
Issue-Trace: api-lifecycle-policy-preview-and-apply
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: api-lifecycle-policy-preview-and-apply
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|PutLifecyclePolicy|2xx->2xx|$.lifecyclePolicyText
Issue-Key: ecr|GetLifecyclePolicy|2xx->2xx|$.lastEvaluatedAt
Issue-Key: ecr|GetLifecyclePolicy|2xx->2xx|$.lifecyclePolicyText
Issue-Key: ecr|StartLifecyclePolicyPreview|2xx->2xx|$.lifecyclePolicyText
Issue-Key: ecr|GetLifecyclePolicyPreview|2xx->2xx|$.lifecyclePolicyText
Issue-Key: ecr|DeleteLifecyclePolicy|2xx->2xx|$.lastEvaluatedAt
Issue-Key: ecr|DeleteLifecyclePolicy|2xx->2xx|$.lifecyclePolicyText
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# ECR lifecycle policy text is reformatted on read and lastEvaluatedAt is never returned

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLifecyclePolicy

## Description
This issue bundles 5 DISTINCT root causes across the ECR lifecycle-policy surface. They share one fault — the stored lifecycle policy document is re-serialized by the emulator's JSON writer instead of being echoed as the client supplied it — and two of them additionally omit the modeled lastEvaluatedAt member. All five must be closed before submitting; each carries its own fixture and step.

AWS (common to all five): lifecyclePolicyText is an opaque document member (ecr#LifecyclePolicyText, length 100..30720). Every operation that returns it returns the EXACT text that was supplied to PutLifecyclePolicy, byte for byte; the recording shows the compact, separator-free form the client sent ('{"rules":[{"rulePriority":1,...}]}') coming back unchanged on put, get, preview-start, preview-get and delete.
Emulator (common to all five): the document is parsed and re-emitted by serde_json's default writer, inserting a space after every ':' and ',' ('{"rules": [{"rulePriority": 1, ...}]}'). The semantics survive but the bytes do not.
Why (common): the policy text is a document, not a structure — clients hash it, diff it and compare it against the file on disk to detect drift (Terraform/CDK/CloudFormation all do). A round-trip that changes the bytes makes every such comparison report a permanent difference, so a converged deployment looks perpetually out of date and re-applies on every run.
Fix (common): store the lifecycle policy document verbatim in lifecycle state and have the lifecycle-policy serializers echo the stored string rather than re-serializing a parsed value; parse only for validation.

Cause 1 of 5: PutLifecyclePolicy reformats the document in its own response. Traces: api-lifecycle-policy-preview-and-apply#task#4.
Cause 2 of 5: GetLifecyclePolicy reformats the document AND omits lastEvaluatedAt. AWS additionally: Smithy ecr#GetLifecyclePolicy output declares lastEvaluatedAt (ecr#EvaluationTimestamp, timestamp) and the recording carries it; the emulator omits the member, so a client cannot tell whether the policy has ever been evaluated. Fix additionally: record the policy's last evaluation instant in lifecycle state and emit it. Traces: api-lifecycle-policy-preview-and-apply#task#5.
Cause 3 of 5: StartLifecyclePolicyPreview reformats the document it echoes. Traces: api-lifecycle-policy-preview-and-apply#task#6.
Cause 4 of 5: GetLifecyclePolicyPreview reformats the document it echoes. Traces: api-lifecycle-policy-preview-and-apply#task#7.
Cause 5 of 5: DeleteLifecyclePolicy reformats the document in its farewell response AND omits lastEvaluatedAt, same as Cause 2. Traces: api-lifecycle-policy-preview-and-apply#task#8.

## Evidence
All five steps returned status 200 on both sides; the divergence is in the body.
Shared text divergence, on $.lifecyclePolicyText: expected '{"rules":[{"rulePriority":1,"description":"Expire old images","selection":{"tagStatus":"any","countType":"imageCountMoreThan","countNumber":100},"action":{"type":"expire"}}]}', got '{"rules": [{"rulePriority": 1, "description": "Expire old images", "selection": {"tagStatus": "any", "countType": "imageCountMoreThan", "countNumber": 100}, "action": {"type": "expire"}}]}' — i.e. identical JSON values, different bytes.
Cause 1 — fixture api-lifecycle-policy-preview-and-apply, step task#4 (PutLifecyclePolicy): $.lifecyclePolicyText value_mismatch as above.
Cause 2 — same fixture, step task#5 (GetLifecyclePolicy): $.lifecyclePolicyText value_mismatch as above, plus $.lastEvaluatedAt missing (expected a timestamp, got None).
Cause 3 — same fixture, step task#6 (StartLifecyclePolicyPreview): $.lifecyclePolicyText value_mismatch as above.
Cause 4 — same fixture, step task#7 (GetLifecyclePolicyPreview): $.lifecyclePolicyText value_mismatch as above.
Cause 5 — same fixture, step task#8 (DeleteLifecyclePolicy): $.lifecyclePolicyText value_mismatch as above, plus $.lastEvaluatedAt missing.

## Steps to Reproduce
Replay the wire corpus and re-run api-lifecycle-policy-preview-and-apply steps task#4 through task#8. Manually:
1. CreateRepository R.
2. PutLifecyclePolicy(repositoryName=R, lifecyclePolicyText=T) where T is compact JSON with no spaces after ':' or ',' -> assert the returned lifecyclePolicyText == T byte for byte.
3. GetLifecyclePolicy(R) -> assert the same, and assert lastEvaluatedAt is present.
4. StartLifecyclePolicyPreview(R) and GetLifecyclePolicyPreview(R) -> assert the echoed text == T.
5. DeleteLifecyclePolicy(R) -> assert the echoed text == T and lastEvaluatedAt is present.
