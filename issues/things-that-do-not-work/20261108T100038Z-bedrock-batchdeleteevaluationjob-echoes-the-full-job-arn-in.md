Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.e2e-evaluation-job-runs-and-is-purged
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock.e2e-evaluation-job-runs-and-is-purged|run#19|AssertionError|assert any(item["jobIdentifier"] == job_id for item in delete_resp["evaluationJobs"]), (
Issue-Commit: ca5e86cb5ef0fd96914240d43edae6de07731cd9

# bedrock BatchDeleteEvaluationJob echoes the full job ARN in evaluationJobs[].jobIdentifier instead of the bare 12-character job id

> _Note: this issue was identified against the code base at commit `ca5e86cb5ef0fd96914240d43edae6de07731cd9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDeleteEvaluationJob

## Description
One net-new root cause in the bedrock control plane.

Cause 1 of 1: BatchDeleteEvaluationJob's success response reports each accepted job under `evaluationJobs[].jobIdentifier` as the full evaluation-job ARN, where Amazon Bedrock reports the bare job id. Failing check ids: bedrock.e2e-evaluation-job-runs-and-is-purged.

AWS: recorded behavior only for the VALUE of this member. BatchDeleteEvaluationJob answers HTTP 202 (Smithy: BatchDeleteEvaluationJob @http POST /evaluation-jobs/batch-delete, code 202) with required `errors` and required `evaluationJobs`; each `BatchDeleteEvaluationJobItem` has required `jobIdentifier` (target EvaluationJobIdentifier, @length max 1011, @pattern ^(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:evaluation-job/[a-z0-9]{12})$, @sensitive, doc "The Amazon Resource Name (ARN) of the evaluation job for deletion") and required `jobStatus`. The recording the check is grounded in shows the live service diverging from its own modeled ARN-shaped pattern on this member: for a request naming `arn:aws:<partition-scoped>:bedrock:<region>:<account>:evaluation-job/<jobid>`, the response item carries ONLY `<jobid>` — the 12-character resource-id segment of the ARN the request named — while `jobStatus` is `Deleting`. The check records this as confirmed against real AWS. The general rule is therefore: `evaluationJobs[].jobIdentifier` is the job's bare id, derived from the identifier the request supplied, not the identifier echoed verbatim. (`errors[].jobIdentifier` is not contradicted by this recording and the model documents it as the ARN, so it is out of scope of this cause.)

Emulator: returns the ARN verbatim. The replay shows `evaluationJobs` = [{'jobIdentifier': 'arn:aws:bedrock:<region>:<account>:evaluation-job/<jobid>', 'jobStatus': 'Deleting'}] where AWS returns [{'jobIdentifier': '<jobid>', 'jobStatus': 'Deleting'}]. `errors` was correctly empty and the status value `Deleting` is correct; only the identifier form diverges.

Why: a client written against AWS treats the echoed `jobIdentifier` as a bare job id — using it as a map key to correlate the batch result with the jobs it submitted, or composing an ARN/console URL from it. Against the emulator that correlation never matches (an ARN is compared against a bare id, so the job appears to be missing from the response), and any client that re-derives an ARN from the value builds a doubly-qualified identifier. Code that only logs the field is unaffected, which is why this surfaces as a silent correlation failure rather than an error.

Fix: the BatchDeleteEvaluationJob handler's response construction in the bedrock control-plane service — the per-job success item it pushes for every deletable job must carry the job's bare id rather than its stored ARN. The sibling `errors` branch and the ValidationException path are unchanged.

Traces: bedrock.e2e-evaluation-job-runs-and-is-purged (the check reached the batch-delete assertion, so the create/Completed/S3-output and the empty-`errors` assertions all passed; only the identifier form failed).

Neither open issue on this operation covers this cause: 20261026T233704Z-bedrock-batchdeleteevaluationjob-s-non-terminal-status-refusal-uses.md concerns the wording of the non-terminal-status refusal, and 20261030T151259Z-bedrock-batchdeleteevaluationjob-validation-message-omits-the-rejected-jobidentifiers.md concerns the validation message's omitted value. This one is the shape of the SUCCESS response.

## Evidence
Check: bedrock.e2e-evaluation-job-runs-and-is-purged [mismatch]

Assertion that failed (check_bedrock_e2e_evaluation_job_runs_and_is_purged.py:77):
    job_id = job_arn.rsplit("/", 1)[-1]
    assert any(item["jobIdentifier"] == job_id for item in delete_resp["evaluationJobs"])

AssertionError: expected job id '<jobid>' in batch-delete response, got
  [{'jobIdentifier': 'arn:aws:bedrock:<region>:<account>:evaluation-job/<jobid>', 'jobStatus': 'Deleting'}]

Expected (AWS, per the check's recorded real-AWS behavior): evaluationJobs[0].jobIdentifier == '<jobid>' (bare 12-char resource id taken from the ARN the request named), jobStatus == 'Deleting', errors == [].
Actual (emulator): evaluationJobs[0].jobIdentifier == the full ARN supplied in jobIdentifiers; jobStatus == 'Deleting'; errors == [].

Status code (202) and the error list are correct; no error code or message is involved in this divergence.

Source of the ARN echo: services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/batch_delete_evaluation_job.rs:235-238 builds the success item with `job_identifier: job.arn.clone()`.

Smithy (bedrock control plane): BatchDeleteEvaluationJobItem.jobIdentifier -> EvaluationJobIdentifier, required, @length max 1011, @pattern ^(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:evaluation-job/[a-z0-9]{12})$, @sensitive. The live service's recorded value does not satisfy that pattern, which the check documents explicitly.

## Steps to Reproduce
1. Create an evaluation job: CreateEvaluationJob with an automated evaluationConfig (taskType QuestionAndAnswer, dataset Builtin.BoolQ, metric Builtin.Accuracy), inferenceConfig naming a foundation-model ARN, and an outputDataConfig s3Uri. Capture the returned jobArn.
2. Poll GetEvaluationJob(jobIdentifier=jobArn) until status is a terminal one (Completed).
3. Call BatchDeleteEvaluationJob(jobIdentifiers=[jobArn]).
4. Inspect the 202 response: evaluationJobs[0].jobIdentifier.
   - AWS: the bare 12-character job id, i.e. jobArn.rsplit('/', 1)[-1].
   - Emulator: the full jobArn that was passed in.
5. Equivalent one-liner once a terminal job exists: compare delete_resp['evaluationJobs'][0]['jobIdentifier'] against jobArn.rsplit('/', 1)[-1] — it must be equal.

Replay to confirm the fix: bedrock.e2e-evaluation-job-runs-and-is-purged
