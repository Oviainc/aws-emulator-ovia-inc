Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__iam__lakeformation__s3.glue-job-catalog-read-denied-until-lf-grant glue__iam__lakeformation__s3.glue-job-denied-on-governed-table-without-lf-data-admin-until-lf-grant
Issue-Key: glue__iam__lakeformation__s3.glue-job-catalog-read-denied-until-lf-grant|run#14|AssertionError|assert fixed["JobRunState"] == "SUCCEEDED", (
Issue-Key: glue__iam__lakeformation__s3.glue-job-denied-on-governed-table-without-lf-data-admin-until-lf-grant|run#14|AssertionError|assert fixed["JobRunState"] == "SUCCEEDED", (
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# glue__iam__lakeformation__s3: a Glue job run executes no script, so the post-Lake-Formation-grant run ends FAILED instead of SUCCEEDED

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
Cause 1 of 1: job runs are recorded but never executed, so a run that must succeed after a Lake Formation grant terminates FAILED.
AWS: StartJobRun runs the job script; GetJobRun then reports JobRunState SUCCEEDED (and the catalog/S3 reads the script performed are what the Lake Formation grant gates). The checks' contract is a state flip driven by a LakeFormation GrantPermissions write: the same job run must be FAILED (authorization) before the grant and SUCCEEDED after it.
Emulator: after the grant, the run still ends 'FAILED' with ErrorMessage 'This emulator records job runs but executes no job script, so the job run ended without running.' — the run outcome is independent of the grant, so the positive half of the contract can never be observed and the negative half proves nothing.
Why: a client cannot verify that a Lake Formation grant actually unblocks its ETL job — the emulator reports the same FAILED outcome whether the principal is authorized or not, which is indistinguishable from a real authorization failure and makes LF-permission debugging impossible.
Fix: the StartJobRun/GetJobRun lifecycle in the glue service crate must drive a run to SUCCEEDED when the job's role is authorized for the catalog/S3 resources the run reads (and to FAILED with an authorization error when it is not), instead of a fixed 'executes no script' failure.
Note: the related open issue 20261023T123230Z-glue-startjobrun-is-an-unimplemented-501-scaffold-and.md covers StartJobRun being a 501 scaffold under the `glue` token; here StartJobRun answers but the run is inert, and the replay report did not attribute these two checks to that issue.
Traces: glue__iam__lakeformation__s3.glue-job-catalog-read-denied-until-lf-grant, glue__iam__lakeformation__s3.glue-job-denied-on-governed-table-without-lf-data-admin-until-lf-grant.

## Evidence
Both checks: AssertionError: after the grant the run must succeed, it ended 'FAILED': 'This emulator records job runs but executes no job script, so the job run ended without running.' Expected: GetJobRun JobRunState == 'SUCCEEDED' after LakeFormation GrantPermissions, and a FAILED run with an authorization error before it.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Create a Glue database/table backed by S3 and a Glue job whose role has no Lake Formation grant; StartJobRun and poll GetJobRun (FAILED, for the wrong reason).
3. LakeFormation GrantPermissions (SELECT/DESCRIBE) to the job role, StartJobRun again and poll GetJobRun — observe FAILED with 'executes no job script' instead of SUCCEEDED.
4. Re-replay the two check ids in trace_ids to confirm.
