Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__glue__s3.glue-job-authored-catalog-table-is-read-by-athena-and-a-partition-denied-role-fails athena__glue__s3.glue-job-role-missing-trust-policy-negative
Issue-Key: athena__glue__s3.glue-job-authored-catalog-table-is-read-by-athena-and-a-partition-denied-role-fails|run#14|AssertionError|assert good["JobRunState"] == "SUCCEEDED", (
Issue-Key: athena__glue__s3.glue-job-role-missing-trust-policy-negative|run#23|AssertionError|assert good_state == "SUCCEEDED", f"control job with correctly-trusted role ended as {good_state!r}"
Issue-Commit: 9b689e7a3706c4d3e9e7bd76513ebd3e34560844

# athena__glue__s3: a Glue job run reached through the Athena-catalog checks never executes its script — the control run ends FAILED with an emulator-authored ErrorMessage (1 cause)

> _Note: this issue was identified against the code base at commit `9b689e7a3706c4d3e9e7bd76513ebd3e34560844`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
ONE cause, grounded in two athena__glue__s3 checks. The divergence is in GLUE (a bootstrapped service that answers), not in Athena: both checks fail on the Glue job run, before any Athena call is reached. The same defect family is filed for other check sets (20261029T185330Z-glue-job-runs-are-recorded-but-never-executed-e42472.md, 20261030T000623Z-glue-job-runs-never-execute-so-a-role.md); neither names these two checks, so these replay units are not gated by any open issue.

Cause 1 of 1: StartJobRun terminates the run FAILED without executing the script, so no positive job run exists and the run role's IAM/trust configuration is never exercised.
AWS: Smithy: StartJobRun returns JobRunId and the run advances JobRunState STARTING -> RUNNING -> SUCCEEDED when the job's script succeeds and its Role is assumable by glue.amazonaws.com; GetJobRun.JobRun.ErrorMessage is populated only on failure. A run whose script writes a catalog table makes that table readable afterwards; a run whose role is NOT trusted by glue.amazonaws.com is the only one that fails, and it fails for the role reason.
Emulator: every StartJobRun settles 'FAILED' with the emulator-authored ErrorMessage 'This emulator records job runs but executes no job script, so the job run ended without running.' The correctly-trusted control run fails identically to the deliberately mis-trusted one, so the two are indistinguishable, and the catalog table the script was to author is never created.
Why: no client can obtain a successful Glue run, and a negative test for role trust is worthless because the positive control fails for an unrelated, emulator-specific reason. The failure reason describes the emulator instead of the caller's job.
Fix: the StartJobRun / job-run state machine and run executor in the glue service crate — execute the job's Command.ScriptLocation (or model its effects), reach SUCCEEDED, enforce the run role's trust/permissions as the only cause of failure, and leave ErrorMessage unset on success.
Traces: athena__glue__s3.glue-job-authored-catalog-table-is-read-by-athena-and-a-partition-denied-role-fails, athena__glue__s3.glue-job-role-missing-trust-policy-negative

## Evidence
athena__glue__s3.glue-job-authored-catalog-table-is-read-by-athena-and-a-partition-denied-role-fails: AssertionError: the catalog-writing run ended 'FAILED': 'This emulator records job runs but executes no job script, so the job run ended without running.' (expected JobRunState 'SUCCEEDED').
athena__glue__s3.glue-job-role-missing-trust-policy-negative: AssertionError: control job with correctly-trusted role ended as 'FAILED' (expected 'SUCCEEDED'; the mis-trusted arm must be the only failure).

## Steps to Reproduce
1. Create an S3 bucket and upload a Glue ETL script that writes a catalog table / output objects.
2. Create an IAM role whose trust policy allows sts:AssumeRole for glue.amazonaws.com.
3. glue:CreateJob with that Role and Command.ScriptLocation set to the uploaded script; glue:StartJobRun; poll glue:GetJobRun to terminal.
4. Observe JobRunState 'FAILED' with ErrorMessage 'This emulator records job runs but executes no job script, so the job run ended without running.' and no table/objects authored.
5. Repeat with a role whose trust policy omits glue.amazonaws.com and observe the identical terminal state and message.
