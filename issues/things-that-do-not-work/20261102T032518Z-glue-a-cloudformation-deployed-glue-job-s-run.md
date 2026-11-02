Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.deploy-job-missing-output-permission-rejected
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.deploy-job-missing-output-permission-rejected|_run_job_run_with_retry#16|Timeout|run_id = _start_job_run_with_retry(glue, job_name, arguments)
Issue-Commit: 7ae6661e06a7b508bcb27fb3544e971eb802ec55

# glue: a CloudFormation-deployed Glue job's run never executes, so the role's s3:PutObject permission is never enforced and the check exhausts its budget (1 cause)

> _Note: this issue was identified against the code base at commit `7ae6661e06a7b508bcb27fb3544e971eb802ec55`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
1 cause, grounded in 1 failing check whose replay key is carried by NO open issue.

Related open issue, named for the fixer: 20261029T185330Z-glue-job-runs-are-recorded-but-never-executed-e42472.md describes the same root cause for StartJobRun, but its Issue-Trace-Ids / Issue-Key list does NOT contain glue.deploy-job-missing-output-permission-rejected, so re-replaying that issue's trace list will not exercise this check. Other open issues on the same area that likewise do not carry this check's key: 20261025T000830Z-glue-job-runs-are-recorded-but-never-executed.md, 20261025T031335Z-glue-job-runs-never-execute-and-publish-no.md, 20261025T072821Z-glue-job-runs-are-terminated-failed-without-executing.md, 20261029T175551Z-glue-job-runs-reached-through-the-emr-glue.md, 20261025T022820Z-glue-job-runs-execute-no-script-so-no-side-effects.md. The cause is restated here so this check is covered; a fixer must make the check in trace_ids pass.

Cause 1 of 1: StartJobRun does not execute the job's Command, so a CloudFormation-deployed AWS::Glue::Job run neither produces its script's side effect nor reflects the run role's IAM permissions, and the check's success-retry loop can never terminate.
AWS: Smithy (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json): StartJobRun takes JobName (required) plus Arguments (GenericMap) and returns only JobRunId; its modeled errors are ConcurrentRunsExceededException, EntityNotFoundException, InvalidInputException, ResourceNumberLimitExceededException, OperationTimeoutException (all client, HTTP 400) and InternalServiceException (server, 500). GetJobRun returns a JobRun carrying JobRunState (enum JobRunState, values including STARTING, RUNNING, SUCCEEDED, FAILED, TIMEOUT, ERROR), ExecutionTime, CompletedOn and ErrorMessage. Beyond the model (recorded behavior only): Glue actually runs the job's Command.ScriptLocation under the job's Role, so the script's effects land (here the pythonshell script's s3:PutObject of s3://<bucket>/<output_key> with body 'ok'), the run reaches JobRunState SUCCEEDED with ErrorMessage absent, and a run whose role lacks exactly s3:PutObject reaches FAILED with the script's own refusal as ErrorMessage. That message's fixed text is 'ClientError: An error occurred (AccessDenied) when calling the PutObject operation: User: arn:aws:sts::<account-id>:assumed-role/<run-role-name>/GlueJobRunnerSession is not authorized to perform: s3:PutObject on resource: "<bucket-object-arn>" because no identity-based policy allows the s3:PutObject action'; the account id, run-role name and resource ARN are the variable parts, taken from the caller's account and from the request/state (they are not literal values from any one run). The denied run must also leave NO object, so a later s3:GetObject of that key refuses with code 'NoSuchKey' and message 'The specified key does not exist.'; and once an s3:PutObject statement is added to the same role, the same job must run to SUCCEEDED and write its object.
Emulator: the run is recorded and immediately settled to a terminal state without executing anything (the behaviour already documented on the related issue above: JobRunState FAILED, ExecutionTime 0, StartedOn ~= CompletedOn, and a constant emulator-authored ErrorMessage instead of the script's refusal). Consequently the check's _run_job_run_with_retry loop — which keeps restarting the job until a run reports SUCCEEDED — never gets SUCCEEDED, and the check ends 'Check timed out after 60.0s' without ever reaching its first assertion. Neither the positive control (good role, object written, SUCCEEDED), nor the negative (bad role, FAILED with the AccessDenied sentence and no object), nor the restore (grant s3:PutObject, same job SUCCEEDED) is reachable.
Why: a client that deploys a Glue job through CloudFormation and starts it can never observe a successful run, so the data the job was supposed to produce never exists; worse, a run refused for a missing s3:PutObject grant is indistinguishable from a healthy run, so an IAM misconfiguration reads exactly like success-then-failure for an unrelated reason. Any orchestration that retries until SUCCEEDED hangs rather than failing with a diagnosis.
Fix: the job-run execution component of the glue service crate — the StartJobRun handler and the job-run state machine it writes (services/service-com-amazonaws-glue-awsglue/src/ops/start_job_run.rs and the job-run state backing GetJobRun). It must execute the job's Command under the job's Role so script side effects and IAM refusals are real, drive JobRunState through RUNNING to SUCCEEDED/FAILED with ExecutionTime/CompletedOn set, leave ErrorMessage absent on success, and carry the underlying authorization refusal verbatim on failure. The constant emulator sentence must not be produced at all.
Traces: glue.deploy-job-missing-output-permission-rejected (verdict: mismatch, 'Check timed out after 60.0s').

## Evidence
Failing check: glue.deploy-job-missing-output-permission-rejected
Suite verdict: [mismatch] — Error: Check timed out after 60.0s

What the check does: deploys one CloudFormation stack with two AWS::Glue::Job resources (pythonshell, same ScriptLocation) differing only in their run role's inline policy — GlueJobGood has s3:GetObject on the script plus s3:PutObject on arn:aws:s3:::<bucket>/results/*, GlueJobBad has only s3:GetObject. It then calls StartJobRun on each and polls GetJobRun to a terminal JobRunState.

Expected (AWS):
  - good job: JobRunState == 'SUCCEEDED'; s3:GetObject(<bucket>, results/good.txt) body == b'ok'
  - bad job:  JobRunState == 'FAILED' and ErrorMessage ==
      'ClientError: An error occurred (AccessDenied) when calling the PutObject operation: User: arn:aws:sts::<account-id>:assumed-role/<bad-role-name>/GlueJobRunnerSession is not authorized to perform: s3:PutObject on resource: "arn:aws:s3:::<bucket>/results/bad.txt" because no identity-based policy allows the s3:PutObject action'
      (fixed text as quoted; <account-id>, <bad-role-name>, bucket and key are the request/state-derived parts)
  - s3:GetObject(<bucket>, results/bad.txt) refuses with ('NoSuchKey', 'The specified key does not exist.')
  - after iam:PutRolePolicy adds s3:PutObject to the bad role, the SAME job run reaches 'SUCCEEDED' and results/bad-restored.txt body == b'ok'

Actual (emulator):
  - no run ever reports SUCCEEDED; every StartJobRun settles terminal within ~500ms without executing Command.ScriptLocation, so the check's retry-until-SUCCEEDED helper (_run_job_run_with_retry, 120s budget) spins and the harness cuts the check at 60s. Not one of the four assertions above is ever reached, and no object is written at any of results/good.txt, results/bad.txt, results/bad-restored.txt.

Note: the per-run bucket names, role names, job names, stack name, run ids and timestamps in this check are per-environment values and are not part of the requirement; the JobRunState progression, the AccessDenied ErrorMessage template and the NoSuchKey refusal are.

## Steps to Reproduce
1. s3:CreateBucket <bucket>; s3:PutObject <bucket> scripts/write_output.py with a pythonshell script that reads --bucket and --output_key from sys.argv and calls s3.put_object(Bucket=bucket, Key=output_key, Body=b'ok').
2. iam:CreateRole GOOD trusting glue.amazonaws.com, attach arn:aws:iam::aws:policy/service-role/AWSGlueServiceRole, and put an inline policy allowing s3:GetObject on arn:aws:s3:::<bucket>/scripts/write_output.py and s3:PutObject on arn:aws:s3:::<bucket>/results/*.
3. iam:CreateRole BAD identically but with ONLY the s3:GetObject statement (no s3:PutObject).
4. cloudformation:CreateStack with two AWS::Glue::Job resources (Command Name=pythonshell, PythonVersion=3.9, ScriptLocation=s3://<bucket>/scripts/write_output.py, MaxCapacity=0.0625, GlueVersion=3.0), one with Role=GOOD and one with Role=BAD; wait for CREATE_COMPLETE.
5. glue:StartJobRun(JobName=<good job>, Arguments={'--bucket': <bucket>, '--output_key': 'results/good.txt'}) and poll glue:GetJobRun until JobRunState is terminal. Expected SUCCEEDED with no ErrorMessage; observed FAILED immediately with the emulator's constant sentence, so a retry-until-SUCCEEDED client never terminates.
6. s3:GetObject(<bucket>, results/good.txt) must return b'ok'; today the key does not exist.
7. glue:StartJobRun(JobName=<bad job>, Arguments={'--bucket': <bucket>, '--output_key': 'results/bad.txt'}) and poll to terminal. Expected FAILED whose ErrorMessage is the AccessDenied/PutObject sentence naming arn:aws:sts::<account>:assumed-role/<BAD role>/GlueJobRunnerSession and arn:aws:s3:::<bucket>/results/bad.txt; observed the same constant emulator sentence as the good run.
8. s3:GetObject(<bucket>, results/bad.txt) must refuse ('NoSuchKey', 'The specified key does not exist.').
9. iam:PutRolePolicy adding s3:PutObject on arn:aws:s3:::<bucket>/results/* to BAD, then repeat step 5 against the bad job with --output_key results/bad-restored.txt: it must now reach SUCCEEDED and write b'ok'.
