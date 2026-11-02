Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__glue__s3.glue-connection-uses-role-with-glue-trust-positive
Issue-Key: emr__glue__s3.glue-connection-uses-role-with-glue-trust-positive|run#19|AssertionError|assert final["JobRunState"] == "SUCCEEDED", f"unexpected state: {final}"
Issue-Commit: ebea418ed7238d7557ed4699ab6be023240b4505

# emr__glue__s3: a Glue job run started against a role that trusts glue.amazonaws.com terminates FAILED without executing the script, so the Glue connection positive control cannot pass

> _Note: this issue was identified against the code base at commit `ebea418ed7238d7557ed4699ab6be023240b4505`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
Cause 1 of 1: StartJobRun records a run and settles it terminal FAILED instead of executing the job script.

AWS: recorded behavior only (Glue's StartJobRun/GetJobRun models carry no execution semantics): a job whose Role trusts glue.amazonaws.com and whose script and connection resolve runs the script and the run reaches JobRunState `SUCCEEDED`, with ExecutionTime > 0 and no ErrorMessage. A run that fails carries an ErrorMessage describing the actual failure (for a role Glue cannot assume, the assume-role error), which is what lets a caller tell a permission failure from a script failure.
Emulator: GetJobRun settles the run at JobRunState `FAILED` within ~0.5 s with ErrorMessage `This emulator records job runs but executes no job script, so the job run ended without running.` — an emulator-authored sentence that is not any AWS failure class, and SUCCEEDED is unreachable for any submission.
Why: a client waiting for a Glue job run to succeed never succeeds, and — worse for differential logic — a run that SHOULD fail for a specific reason (missing s3:GetObject, a role Glue cannot assume, a bad connection) is indistinguishable from one that merely never ran, because every run fails with the same sentence. The positive control a negative check needs to isolate its failure can never pass.
Fix: the Glue crate's job-run lifecycle (the StartJobRun handler and the run-state progression it schedules: services/service-com-amazonaws-glue-awsglue/src/ops/start_job_run.rs and the job-run state machine in the crate's state), so a valid submission executes the recorded workload and settles SUCCEEDED, and a failing submission carries Glue's own ErrorMessage for that failure class.
Traces: emr__glue__s3.glue-connection-uses-role-with-glue-trust-positive. The open issue 20261029T175551Z-glue-job-runs-reached-through-the-emr-glue.md describes Glue runs terminating FAILED without executing but carries no replay key for this check; this filing grounds the cause in the connection check so a fixer can re-replay it.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator (service heading `emr__glue__s3`, 2/46 checks passing).

emr__glue__s3.glue-connection-uses-role-with-glue-trust-positive: AssertionError `unexpected state:` with the GetJobRun projection {'Id': 'jr_…', 'JobName': 'sim2real-glue-connjob-…', 'JobMode': 'SCRIPT', 'JobRunState': 'FAILED', 'ErrorMessage': 'This emulator records job runs but executes no job script, so the job run ended without running.', 'ExecutionTime': 0, 'Timeout': 5, 'MaxCapacity': 0.0625, 'GlueVersion': '5.1'} — StartedOn and CompletedOn are 0.5 s apart, so the run never executed anything. Expected: the run reaches SUCCEEDED with no ErrorMessage (the job's role trusts glue.amazonaws.com and its connection resolves).

The same ErrorMessage sentence appears on every Glue run in this service's checks, which is what makes a specific failure class unobservable.

## Steps to Reproduce
Boot the emulator and replay emr__glue__s3.glue-connection-uses-role-with-glue-trust-positive from suite simparity-behavioral-aws-v1 (service heading `emr__glue__s3`):
1. Create an IAM role whose trust policy allows glue.amazonaws.com to sts:AssumeRole and attach the S3 access the script needs.
2. Create a Glue connection and a Glue job (JobMode SCRIPT) that uses that role and a script stored in S3.
3. StartJobRun, then poll GetJobRun and assert JobRunState SUCCEEDED with no ErrorMessage and a non-zero ExecutionTime.
