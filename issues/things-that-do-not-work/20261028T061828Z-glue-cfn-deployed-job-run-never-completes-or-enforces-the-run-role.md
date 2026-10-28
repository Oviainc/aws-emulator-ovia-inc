Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.deploy-job-missing-output-permission-rejected
Issue-Key: glue.deploy-job-missing-output-permission-rejected|_run_job_run_with_retry#16|Timeout|time.sleep(interval_s)
Issue-Commit: 22cd2d7ee35d84122c85f8f0653a0b356f703d69

# Glue: a CloudFormation-deployed Glue job's run never completes or enforces the run role's IAM permissions (1 cause)

> _Note: this cause was identified against the code base at commit `22cd2d7ee35d84122c85f8f0653a0b356f703d69`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

> _Provenance: this record was SPLIT OUT of the two-cause report
> `20261028T061828Z-glue-2-net-new-root-causes-associateglossaryterms-accepts.md`, whose
> other cause (AssociateGlossaryTerms accepting a glossary term the catalog holds no live
> term under) is fixed and whose record now lives under `issues/things-that-now-work/`.
> This cause is UNFIXED and keeps its own replay key, so a replay still measures it._

## Operation
StartJobRun

## Description
Cause 1 of 1: a CloudFormation-declared AWS::Glue::Job plus StartJobRun never yields a completed run, so the run role's IAM permissions are never enforced.

AWS: Smithy models StartJobRun (input JobName required, Arguments GenericMap; output JobRunId) and GetJobRun (input JobName + RunId required; output JobRun), with EntityNotFoundException/ConcurrentRunsExceededException as the client refusals. Recorded behavior: a CFN stack declaring two AWS::Glue::Job resources of type pythonshell reaches CREATE_COMPLETE and both jobs are then addressable by glue:StartJobRun under their declared names; each started run progresses to a terminal JobRunState within the run budget; the run executes the script at Command.ScriptLocation under the job's Role, so (a) the run whose role carries s3:PutObject on the output prefix ends SUCCEEDED and the output object exists with the script's body, (b) the run whose role lacks exactly that statement ends FAILED with ErrorMessage whose fixed text is "ClientError: An error occurred (AccessDenied) when calling the PutObject operation: User: arn:aws:sts::<account>:assumed-role/<roleName>/GlueJobRunnerSession is not authorized to perform: s3:PutObject on resource: \"arn:aws:s3:::<bucket>/<key>\" because no identity-based policy allows the s3:PutObject action" (account, role name, bucket and key are the request/state values, the rest is fixed), and the output object is absent so s3:GetObject answers NoSuchKey / "The specified key does not exist.", and (c) after iam:PutRolePolicy adds the missing s3:PutObject statement, a fresh run of the SAME job ends SUCCEEDED and writes its output.

Emulator: nothing in that sequence completes — the check is killed by its 60s budget ("Check timed out after 60.0s") with no terminal job run ever observed, so no JobRunState, no ErrorMessage and no S3 side effect is produced. The emulator records the run and settles it into the modelled failure state carrying its own reason that no job script runs (services/service-com-amazonaws-glue-awsglue/src/state.rs, `JOB_RUN_NOT_EXECUTED_MESSAGE`), and the check's run-and-retry loop never observes the success it waits for.

Why: a client cannot observe any outcome of a Glue job run against the emulator — neither success and its side effects nor an IAM-denied failure and its ErrorMessage. Every pipeline that waits on GetJobRun hangs rather than finishing, and every negative test of a least-privilege Glue role passes vacuously because the denial never materializes.

Fix: the Glue job-run execution path — the StartJobRun handler and the job-run state machine that GetJobRun reports from (plus the CloudFormation AWS::Glue::Job provisioning that must make the declared job name addressable before the run starts, which services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioner.rs already performs through glue CreateJob), including evaluation of the run role's identity-based policy for the run's own S3 calls. Related open issues describe the same family but do NOT carry this check's replay key, so none of them covers it: 20261025T000830Z-glue-job-runs-are-recorded-but-never-executed.md, 20261025T031335Z-glue-job-runs-never-execute-and-publish-no.md, 20261025T072821Z-glue-job-runs-are-terminated-failed-without-executing.md, 20261025T022820Z-glue-cfn-declared-resources-and-job-run-execution.md, 20261025T050831Z-cloudformation-glue-iam-s3-cloudformation-never-provisions-aws-glue-job-after-create.md.

What a previous fixer found blocking it, so the next one starts from there: the recorded outcome is produced by EXECUTING the caller's pythonshell script, so it needs (1) an in-process Python runtime, which this workspace has nowhere — the only candidate crate, rustpython-vm 0.6.0, declares rust-version 1.95.0 and ships no AWS SDK; (2) the AWS SDK the script makes its S3 call through, which cannot be obtained in-process (no egress, no external processes); and (3) evaluation of the run role's identity-based policy, which no part of this emulator performs. Any fix has to serve those three things honestly; reporting one of the three recorded outcomes without running the script is not a fix.

Traces: glue.deploy-job-missing-output-permission-rejected.

## Evidence
Check glue.deploy-job-missing-output-permission-rejected.
Expected (AWS): the CFN stack with two AWS::Glue::Job (pythonshell) resources reaches CREATE_COMPLETE; the good job's run ends JobRunState=SUCCEEDED and s3:GetObject of results/good.txt returns b'ok'; the bad job's run ends JobRunState=FAILED with ErrorMessage 'ClientError: An error occurred (AccessDenied) when calling the PutObject operation: User: arn:aws:sts::<account>:assumed-role/<badRoleName>/GlueJobRunnerSession is not authorized to perform: s3:PutObject on resource: "arn:aws:s3:::<bucket>/results/bad.txt" because no identity-based policy allows the s3:PutObject action' and s3:GetObject of that key answers ('NoSuchKey', 'The specified key does not exist.'); after PutRolePolicy grants s3:PutObject, a new run of the same job ends SUCCEEDED and writes results/bad-restored.txt.
Actual (emulator): 'Error: Check timed out after 60.0s' — no terminal JobRunState is ever reported to the check's poll loop, so none of the three outcomes (SUCCEEDED + object, FAILED + AccessDenied ErrorMessage + absent object, restored SUCCEEDED) is produced and no S3 side effect appears.

## Steps to Reproduce
1. Create an S3 bucket and put the pythonshell script at s3://<bucket>/scripts/write_output.py (it reads --bucket/--output_key and calls s3:PutObject).
2. iam:CreateRole twice with a glue.amazonaws.com trust policy: the 'good' role gets inline s3:GetObject on the script plus s3:PutObject on arn:aws:s3:::<bucket>/results/*; the 'bad' role gets only s3:GetObject on the script.
3. cloudformation:CreateStack with two AWS::Glue::Job resources (Command.Name=pythonshell, ScriptLocation=the script, MaxCapacity=0.0625, GlueVersion=3.0), one per role; poll DescribeStacks to CREATE_COMPLETE.
4. glue:StartJobRun on the good job with Arguments {'--bucket': <bucket>, '--output_key': 'results/good.txt'} and poll glue:GetJobRun to a terminal JobRunState.
   Expected: SUCCEEDED and s3:GetObject('results/good.txt') == b'ok'.
   Observed: the run settles into the modelled failure state with the emulator's own "executes no job script" reason, so the check's retry loop is killed at its 60s budget.
5. glue:StartJobRun on the bad job with --output_key 'results/bad.txt' and poll to terminal.
   Expected: FAILED with the AccessDenied ErrorMessage quoted in the evidence and NoSuchKey for the object.
6. iam:PutRolePolicy adds s3:PutObject on arn:aws:s3:::<bucket>/results/* to the bad role; re-run the same job with --output_key 'results/bad-restored.txt'.
   Expected: SUCCEEDED and the object written.
7. Re-replay the check id in trace_ids.
