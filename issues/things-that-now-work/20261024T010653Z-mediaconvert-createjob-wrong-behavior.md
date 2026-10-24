Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# Enforce the account input policy (PutPolicy) when a job is submitted

_Filed: 2026-10-24T01:06:52Z_
_Operation: CreateJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The account input policy is write-only state in the emulator: PutPolicy stores it and GetPolicy reads it back, but no job-submission path ever consults it. Expected: with an account policy that sets an input location to DISALLOWED (e.g. `httpInputs: DISALLOWED`), CreateJob whose `Settings.Inputs[].FileInput` names an input of that kind is refused with a modelled error (BadRequestException 400 / ForbiddenException 403 are both declared for CreateJob) and no job is persisted, while a job whose inputs are all of ALLOWED kinds is accepted. Actual: `src/ops/create_job.rs` never reads `state.policy` at all — it validates only that `Role` is non-empty and that the referenced `Queue` exists, then stores the job and answers HTTP 201 with Status SUBMITTED. The policy therefore has zero observable effect on anything: a client can set `{s3Inputs: ALLOWED, httpInputs: DISALLOWED, httpsInputs: DISALLOWED}` and still submit a job whose FileInput is `http://example.com/in.mp4`, which AWS refuses. The same is true for every other input-bearing path (the settings stored on a job template are also never checked against the policy).

## Evidence

Smithy model: `InputPolicy` doc — "An input policy allows or disallows a job you submit to run based on the conditions that you specify"; `Policy$HttpInputs` — "Allow or disallow jobs that specify HTTP inputs" (same for HttpsInputs/S3Inputs). So the policy's entire documented purpose is to gate job submission. Handler: services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/create_job.rs (handle() lines ~208-332) performs no policy lookup; src/state.rs defines INPUT_POLICY_DISALLOWED/INPUT_POLICY_ALLOWED and PolicyRecord, but the only consumer of those decisions is PutPolicy's own all-DISALLOWED cross-member check (`PolicyRecord::allows_no_input`) — nothing reads the stored decision on a submit path. Tests: tests/all/policy_lifecycle.rs only exercises PutPolicy/GetPolicy/DeletePolicy round-tripping (it never calls POST /2017-08-29/jobs), so no test asserts enforcement. Not covered by the existing issues: the fixed issue 20261022T124827Z-mediaconvert-mutations-not-applied-and-invalid-putpolicy-accepted covers only PutPolicy's all-DISALLOWED rejection, and neither open bundle mentions policy enforcement on CreateJob.

## Steps to Reproduce

1. Boot the emulator and point a mediaconvert client at it.
2. put_policy(Policy={'S3Inputs':'ALLOWED','HttpInputs':'DISALLOWED','HttpsInputs':'DISALLOWED'}) — succeeds with 200.
3. get_policy() — confirms the stored policy disallows HTTP/HTTPS inputs.
4. create_job(Role=<role>, Settings={'Inputs':[{'FileInput':'http://example.com/in.mp4'}],'OutputGroups':[{FILE_GROUP_SETTINGS -> 's3://b/out/', one MP4/H_264 output}]}).
5. Expect the submission to be refused by a modelled error (no job created; ListJobs stays empty). Observe HTTP 201 with a full Job body in status SUBMITTED.
6. Positive control: the same job with 's3://b/in.mp4' must still be accepted.
