Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sagemaker.iam-role-missing-sagemaker-trust-blocks-training-job-negative
Issue-Key: s3__sagemaker.iam-role-missing-sagemaker-trust-blocks-training-job-negative|_create_training_job_with_retry#12|Timeout|time.sleep(poll_s)
Issue-Commit: d24b6997be17cc686e45f4a7148f10449cf3bf59

# s3+sagemaker: CreateTrainingJob with a role that does not trust sagemaker.amazonaws.com is not refused synchronously and no accepted training job reaches a terminal state, so the trust-boundary check never completes (times out)

> _Note: this issue was identified against the code base at commit `d24b6997be17cc686e45f4a7148f10449cf3bf59`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTrainingJob

## Description
One net-new root cause in the SageMaker training-job surface (role-trust admission + job execution). One failing check, one cause.

Cause 1 of 1: a CreateTrainingJob whose RoleArn names a role that does not trust `sagemaker.amazonaws.com` is not refused at the API, and an accepted training job never converges to a terminal status, so a client waiting on either outcome waits forever — the check exhausts its whole budget without ever getting a verdict.

AWS: CreateTrainingJob validates, synchronously, that SageMaker can assume the role it is handed. When the role's trust policy does not admit the `sagemaker.amazonaws.com` service principal the call is refused with HTTP 400 and error code `ValidationException`, message `Could not assume role <RoleArn>. Please ensure that the role exists and allows principal 'sagemaker.amazonaws.com' to assume the role.` — the two sentences are fixed text and `<RoleArn>` is the RoleArn value from the request, never a literal from one run. No job is created and nothing is ever written under the job's OutputDataConfig prefix. Conversely, a job created with an identically-permissioned role that DOES trust `sagemaker.amazonaws.com` runs and converges: DescribeTrainingJob reaches `TrainingJobStatus = "Completed"` within the job's StoppingCondition, and the output path `s3://<bucket>/<prefix>/<job-name>/output/` holds a `model.tar.gz`. Smithy (com.amazonaws.sagemaker, CreateTrainingJob): `RoleArn` is `@required` with `@length min 20 max 2048` and pattern `^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`; `TrainingJobName` is `@required`, length 1..63, pattern `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`; `AlgorithmSpecification`, `OutputDataConfig`, `ResourceConfig` and `StoppingCondition` are required; the output is `TrainingJobArn` with pattern `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:training-job/...`. The operation's modeled errors are only `ResourceInUse`, `ResourceLimitExceeded` and `ResourceNotFound` (each client-kind, HTTP 400 defaulted) — `ValidationException` is NOT a modeled error shape for this operation, so the code and the message above are recorded behavior only, exactly as the check asserts them.

Emulator: the check produces no per-call divergence at all — it never finishes. The replay reports `Error: Check timed out after 60.0s`, i.e. the emulator leaves the client with neither of the two outcomes AWS guarantees: the untrusted-role CreateTrainingJob is not refused with ValidationException (the check's `else: raise AssertionError("CreateTrainingJob with a role that does not trust sagemaker.amazonaws.com was expected to be rejected synchronously, but it was accepted")` is the branch for that), and the positive-control job created with the trusting role never reaches a terminal `TrainingJobStatus`, so `_wait_training_terminal(sm, good_job, timeout_s=1200)` cannot return and no `model.tar.gz` is ever written under `output-good/<job>/output/`. Hypothesis: with the role-trust validation absent, the untrusted job is admitted and parked in a non-terminal status, and the terminal-status wait (whether on the rejected job's cleanup stop or on the positive control) is what consumes the budget; the replay records only that no verdict was reached within it.

Why: a client relying on AWS's behavior is broken twice over. First, the most common SageMaker misconfiguration — handing a training job an execution role whose trust policy names the wrong service principal — is reported by AWS at the call site, immediately and with the role ARN in the message, so a deploy script or console flow can fail fast and tell the operator exactly which role to fix; against the emulator that call succeeds, so the misconfiguration is silently accepted and the error never surfaces where the operator can act on it. Second, because no accepted job ever reaches a terminal state, every waiter — `boto3`'s `training_job_completed_or_stopped` waiter, the SageMaker Python SDK's `estimator.fit()`, a CI pipeline step, a Step Functions `.sync` task — hangs instead of returning, so a pipeline tested against the emulator cannot distinguish "still training" from "will never finish", and the negative control's guarantee (an unassumable role writes NO object under its output prefix) is unverifiable.

Fix: the sagemaker crate's CreateTrainingJob handler — its validation must resolve the RoleArn against IAM and refuse, before creating any job, when the role's trust policy does not allow `sagemaker.amazonaws.com` to `sts:AssumeRole` (HTTP 400, `ValidationException`, the message above with the request's RoleArn interpolated) — and the training-job state machine/executor that must drive an accepted job to a terminal `TrainingJobStatus` and materialise its output artifact under OutputDataConfig.

Related open issues, named rather than assumed to cover this: 20261103T053228Z-s3-sagemaker-a-training-job-whose-role-does.md (carries `s3__sagemaker.sagemaker-execution-role-trust-policy-missing-negative`) and 20261029T211510Z-s3-sagemaker-sagemaker-jobs-never-execute-and-are-never-validated.md. Neither carries this check's replay key, and this check differs from both in its observable symptom: it does not report a wrong status/code, it never terminates at all, so a fixer who closes those two by adding a refusal on one sibling check will still leave this one timing out until an accepted job also reaches a terminal state.

Traces: s3__sagemaker.iam-role-missing-sagemaker-trust-blocks-training-job-negative.

## Evidence
Cause 1 — expected (real AWS):
1. `CreateTrainingJob(TrainingJobName=<bad-job>, RoleArn=<role trusting ec2.amazonaws.com only>, ...)` is refused synchronously: HTTP 400, `Error.Code = ValidationException`, `Error.Message = "Could not assume role <RoleArn>. Please ensure that the role exists and allows principal 'sagemaker.amazonaws.com' to assume the role."` (fixed text; `<RoleArn>` is the RoleArn from the request).
2. `ListObjectsV2(Bucket=<bucket>, Prefix="output-bad/<bad-job>/")` returns no `Contents` — the refused job wrote nothing.
3. Positive control: `CreateTrainingJob` with an identically-permissioned role that trusts `sagemaker.amazonaws.com` is accepted, `DescribeTrainingJob` converges to `TrainingJobStatus = "Completed"`, and `ListObjectsV2(Prefix="output-good/<good-job>/output/")` contains a key ending in `model.tar.gz`.

Actual (emulator):
  Error: Check timed out after 60.0s

No assertion was reached, so no single call reported a wrong status, code or message — the divergence is that neither guaranteed outcome occurs: the untrusted-role CreateTrainingJob is not refused with `ValidationException`/the "Could not assume role ..." message (it is accepted), and no accepted training job reaches a terminal `TrainingJobStatus`, so the terminal-status wait never returns and no model artifact is produced. Job names, bucket names, role ARNs and the chosen algorithm image are per-run values and are not part of the requirement; the requirement is the synchronous refusal with that code and message template, the absence of any output object for the refused job, and convergence of an accepted job to `Completed` with a `model.tar.gz` under its output prefix.

## Steps to Reproduce
Replay the behavioral suite and re-run exactly the check id in trace_ids: s3__sagemaker.iam-role-missing-sagemaker-trust-blocks-training-job-negative.

Manually:
1. Create a bucket (adding CreateBucketConfiguration.LocationConstraint when the region is not us-east-1) and upload a small CSV training file to `train/train.csv`.
2. Create two IAM roles with the IDENTICAL inline permission policy (`s3:GetObject`, `s3:PutObject`, `s3:ListBucket` on the bucket and `bucket/*`), differing only in their trust policy: role A trusts `sagemaker.amazonaws.com`, role B trusts `ec2.amazonaws.com`.
3. Call `CreateTrainingJob` with role B (built-in XGBoost image, `InputDataConfig` pointing at `s3://<bucket>/train/`, `OutputDataConfig.S3OutputPath` = `s3://<bucket>/output-bad/`, a `ResourceConfig` and a `StoppingCondition`).
   Expected: HTTP 400 `ValidationException` with message `Could not assume role <role B arn>. Please ensure that the role exists and allows principal 'sagemaker.amazonaws.com' to assume the role.` Observed: the call is accepted.
4. `ListObjectsV2(Bucket=<bucket>, Prefix="output-bad/<job>/")` must return no Contents.
5. Call `CreateTrainingJob` again with role A and `OutputDataConfig.S3OutputPath` = `s3://<bucket>/output-good/`, then poll `DescribeTrainingJob` for a terminal `TrainingJobStatus`.
   Expected: it converges to `Completed` and `output-good/<job>/output/model.tar.gz` exists. Observed: the status never becomes terminal, so the poll never returns and the whole flow exhausts its time budget.
