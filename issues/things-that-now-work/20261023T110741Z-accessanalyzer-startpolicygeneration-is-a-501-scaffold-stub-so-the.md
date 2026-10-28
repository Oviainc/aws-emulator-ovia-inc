Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: accessanalyzer.start-policy-generation-completes-then-cancel-terminal-rejected
Issue-Shape-Id: com.amazonaws.accessanalyzer#AccessAnalyzer
Issue-Key: accessanalyzer.start-policy-generation-completes-then-cancel-terminal-rejected|run#15|AssertionError|assert code == "ValidationException", f"expected ValidationException for missing cloudTrailDetails, got {code}"
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# StartPolicyGeneration is a 501 scaffold stub, so the policy-generation job lifecycle never starts

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPolicyGeneration

## Description
One cause.

Cause 1 of 1: StartPolicyGeneration answers HTTP 501 `NotImplemented` for every request, so neither its input validation nor the job it mints exists, and every downstream call (GetGeneratedPolicy, CancelPolicyGeneration on a real jobId) is unreachable.

AWS: Smithy model (services/service-com-amazonaws-accessanalyzer-accessanalyzer/smithy-model/com-amazonaws-accessanalyzer-accessanalyzer.json) binds StartPolicyGeneration to PUT /policy/generation, success 200 with required output member `jobId`, input `policyGenerationDetails` (required) + optional `cloudTrailDetails` + `clientToken` (@idempotencyToken), and models ValidationException (HTTP 400) among its errors. Recorded behavior only for the specific rejection the check exercises: a StartPolicyGeneration call carrying `policyGenerationDetails` but NO `cloudTrailDetails` is refused synchronously with HTTP 400 `ValidationException` and no job is created (recorded message example: "Missing cloudTrailDetails"); with a well-formed `cloudTrailDetails` naming a real trail the call returns 200 with a `jobId` that GetGeneratedPolicy and CancelPolicyGeneration then track through IN_PROGRESS -> SUCCEEDED/CANCELED.

Emulator: every StartPolicyGeneration request returns HTTP 501 with error code `NotImplemented` and message "operation StartPolicyGeneration is scaffolded but not implemented in this emulator". The check sees code `NotImplemented` where it expects `ValidationException`.

Why: a client cannot generate a policy at all, and a client that relies on the documented 400 refusal for an incomplete request instead sees a 501, which botocore does not classify as a client error — validation-driven control flow (retry vs. fix-the-request) takes the wrong branch, and the whole jobId lifecycle is untestable.

Fix: the operation's handler — services/service-com-amazonaws-accessanalyzer-accessanalyzer/src/ops/start_policy_generation.rs (the `Err(crate::scaffolded::Scaffolded::not_implemented("StartPolicyGeneration"))` at line 172 is the only return path). It needs request validation (reject a request with no `cloudTrailDetails`, and an unknown `cloudTrailArn`, as 400 ValidationException), jobId minting in the service's state, and the job-status transitions that GetGeneratedPolicy/CancelPolicyGeneration read.

Traces: accessanalyzer.start-policy-generation-completes-then-cancel-terminal-rejected (fails on the first StartPolicyGeneration call of the check, the missing-cloudTrailDetails rejection).

## Evidence
Check accessanalyzer.start-policy-generation-completes-then-cancel-terminal-rejected [mismatch]:

  AssertionError: expected ValidationException for missing cloudTrailDetails, got NotImplemented

Expected (recorded AWS): HTTP 400, error code `ValidationException`, message example "Missing cloudTrailDetails"; no job created.
Actual (emulator): HTTP 501, error code `NotImplemented`, message "operation StartPolicyGeneration is scaffolded but not implemented in this emulator".

Handler evidence: services/service-com-amazonaws-accessanalyzer-accessanalyzer/src/ops/start_policy_generation.rs line 4 carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and line 172 is `Err(crate::scaffolded::Scaffolded::not_implemented("StartPolicyGeneration"))`.

## Steps to Reproduce
1. Boot the emulator and point an accessanalyzer client at it.
2. Call StartPolicyGeneration with only policyGenerationDetails={'principalArn': '<an existing role arn>'} and no cloudTrailDetails.
3. Observe HTTP 501 / NotImplemented. Real AWS answers HTTP 400 ValidationException and creates no job.
4. Call StartPolicyGeneration with a valid cloudTrailDetails naming an existing trail: real AWS returns 200 with a jobId; the emulator again returns 501, so GetGeneratedPolicy/CancelPolicyGeneration can never be exercised.
