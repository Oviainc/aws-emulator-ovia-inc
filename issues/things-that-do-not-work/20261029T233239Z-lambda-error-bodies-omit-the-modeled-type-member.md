Issue-Source: trace-sweep-divergence
Issue-Trace: lam-err-revision-id-function-mutations
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lam-err-revision-id-function-mutations lambda-function-core-lifecycle lambda-function-crud-with-tag-round-trip lambda-function-lifecycle-crud lambda-resource-policy-nonexistent-function lambda-function-scaling-config-non-capacity-provider-function lambda-invoke-modes-comparison lambda-publish-version-then-invoke-pinned-version lambda-concurrency-and-provisioned-concurrency lambda-concurrency-and-scaling-config lambda-function-scaling-config-errors lambda-function-scaling-config-nonexistent-function
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|UpdateFunctionConfiguration|4xx->4xx|$.Type
Issue-Key: lambda|UpdateFunctionCode|4xx->4xx|$.Type
Issue-Key: lambda|CreateFunction|4xx->4xx|$.Type
Issue-Key: lambda|GetFunctionConfiguration|4xx->4xx|$.Type
Issue-Key: lambda|GetFunction|4xx->4xx|$.Type
Issue-Key: lambda|DeleteFunction|4xx->4xx|$.Type
Issue-Key: lambda|PutProvisionedConcurrencyConfig|4xx->4xx|$.Type
Issue-Key: lambda|GetProvisionedConcurrencyConfig|4xx->4xx|$.Type
Issue-Key: lambda|PutFunctionScalingConfig|4xx->4xx|$.Type
Issue-Commit: 6fcb2038be76f91f904ab31cfab5a8d60c9030f7

# Lambda error bodies omit the modeled Type member on function lifecycle and concurrency operations

> _Note: this issue was identified against the code base at commit `6fcb2038be76f91f904ab31cfab5a8d60c9030f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFunctionConfiguration

## Description
Shared fault: the Lambda error serializer never emits the `Type` member, so every modeled client exception body is short one member. Ten net-new divergence clusters below are the same defect observed on ten different operations.

AWS: every Lambda control-plane exception shape declares a `Type` member next to the message. Smithy (lambda, awsgirapiservice): ResourceNotFoundException {Type:String, Message:String} httpError 404; PreconditionFailedException {Type:String, message:String} httpError 412; ResourceConflictException {Type:String, message:String} httpError 409; InvalidParameterValueException {Type:String, message:String} httpError 400. In every recorded 4xx the value is the fixed string `User` (the fault owner); the message is the variable part.

Emulator: the status code and message are produced, but `Type` is absent from the JSON body on every one of these errors.

Why: a client that classifies a failure as caller-side vs service-side by reading `Type` (the AWS SDKs and the recorded wire contract both expose it) sees the member missing and cannot classify the failure; the error body is not shape-conformant for the modeled exception.

Fix: the shared Lambda error serializer that renders modeled exceptions (one place, used by all handlers) must emit `Type` alongside the message for client-kind errors.

Cause 1 of 10: UpdateFunctionConfiguration, 412 PreconditionFailedException — fixture lam-err-revision-id-function-mutations step task#7; body has the right status and message but no `Type: "User"`.
Cause 2 of 10: UpdateFunctionCode, 412 PreconditionFailedException — fixture lam-err-revision-id-function-mutations step task#11; `Type` missing.
Cause 3 of 10: CreateFunction, 409 ResourceConflictException — fixture lambda-function-core-lifecycle step task#7; `Type` missing.
Cause 4 of 10: GetFunctionConfiguration, 404 ResourceNotFoundException — fixture lambda-function-core-lifecycle step teardown#9; `Type` missing.
Cause 5 of 10: GetFunction, 404 ResourceNotFoundException — fixture lambda-function-crud-with-tag-round-trip step teardown#17; `Type` missing.
Cause 6 of 10: CreateFunction, 400 InvalidParameterValueException — fixture lambda-function-lifecycle-crud step task#7; `Type` missing.
Cause 7 of 10: DeleteFunction, 404 ResourceNotFoundException — fixture lambda-function-lifecycle-crud step teardown#16; `Type` missing.
Cause 8 of 10: PutProvisionedConcurrencyConfig, 400 InvalidParameterValueException — fixture lambda-concurrency-and-provisioned-concurrency step task#11; `Type` missing.
Cause 9 of 10: GetProvisionedConcurrencyConfig, 404 ResourceNotFoundException — fixture lambda-concurrency-and-provisioned-concurrency step task#13; `Type` missing.
Cause 10 of 10: PutFunctionScalingConfig, 404 ResourceNotFoundException — fixture lambda-function-scaling-config-errors step task#1; `Type` missing.

## Evidence
Replay detail for every cluster: `$.Type: missing (expected 'User', got None)`. Expected and actual HTTP status agree in all ten (412/412, 412/412, 409/409, 404/404, 404/404, 400/400, 404/404, 400/400, 404/404, 404/404) — only the body member differs. Smithy citation: ResourceNotFoundException/PreconditionFailedException/ResourceConflictException/InvalidParameterValueException each declare a `Type` member.

## Steps to Reproduce
Replay the wire corpus (replay_wire_corpus) and inspect the listed fixture/step pairs, e.g. fixture lam-err-revision-id-function-mutations step task#7: call UpdateFunctionConfiguration with a stale RevisionId and read the 412 body — it contains the message but no `Type` member.
