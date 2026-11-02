Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__iam__lakeformation__s3.e2e-troubleshoot-glue-job-table-read-failure-lifecycle glue__iam__lakeformation__s3.glue-deleted-job-drops-its-run-history-and-refuses-new-runs glue__iam__lakeformation__s3.glue-get-job-runs-lists-newest-first-and-error-message-only-on-failed glue__iam__lakeformation__s3.glue-update-job-raising-max-concurrent-runs-to-two-refuses-only-the-third-run glue__iam__s3.gjwr-job-run-of-a-never-uploaded-script-fails-until-it-is-uploaded glue__iam__s3.gjwr-role-name-reuse-through-the-other-region-client-rejected glue__iam__lakeformation__s3.lf-get-tables-lists-only-granted-governed-tables glue__iam__lakeformation__s3.lf-select-grant-alone-restores-get-table-no-describe-required glue__iam__lakeformation__s3.lf-database-iap-revoked-get-database-denied-until-a-table-grant codecommit__codepipeline__glue__iam__lambda.refarch-glue-create-job-duplicate-name-rejected codecommit__codepipeline__glue__iam__lambda.refarch-glue-job-command-and-defaults-round-trip
Issue-Key: glue__iam__lakeformation__s3.e2e-troubleshoot-glue-job-table-read-failure-lifecycle|phase#5|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: glue__iam__lakeformation__s3.glue-deleted-job-drops-its-run-history-and-refuses-new-runs|run#10|AssertionError|assert done["JobRunState"] == "SUCCEEDED", f"the control run ended {done['JobRunState']!r}"
Issue-Key: glue__iam__lakeformation__s3.glue-get-job-runs-lists-newest-first-and-error-message-only-on-failed|run#11|AssertionError|assert second["JobRunState"] == "SUCCEEDED", (
Issue-Key: glue__iam__lakeformation__s3.glue-update-job-raising-max-concurrent-runs-to-two-refuses-only-the-third-run|run#13|AssertionError|assert warm["JobRunState"] == "SUCCEEDED", f"the warm-up run ended {warm['JobRunState']!r}"
Issue-Key: glue__iam__s3.gjwr-job-run-of-a-never-uploaded-script-fails-until-it-is-uploaded|run#9|AssertionError|assert control_run["JobRunState"] == "SUCCEEDED", (
Issue-Key: glue__iam__s3.gjwr-role-name-reuse-through-the-other-region-client-rejected|run#7|AssertionError|assert control["JobRunState"] == "SUCCEEDED", (
Issue-Key: glue__iam__lakeformation__s3.lf-get-tables-lists-only-granted-governed-tables|run#13|AssertionError|assert before == [], f"with no grants the reader must list no governed table, got {before!r}"
Issue-Key: glue__iam__lakeformation__s3.lf-select-grant-alone-restores-get-table-no-describe-required|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS refuses it, so there is no error")
Issue-Key: glue__iam__lakeformation__s3.lf-database-iap-revoked-get-database-denied-until-a-table-grant|_settled#10|Timeout|time.sleep(3)
Issue-Key: codecommit__codepipeline__glue__iam__lambda.refarch-glue-create-job-duplicate-name-rejected|run#16|AssertionError|assert (code, message) == expected, (
Issue-Key: codecommit__codepipeline__glue__iam__lambda.refarch-glue-job-command-and-defaults-round-trip|run#16|AssertionError|assert job['Command'].get('PythonVersion') == '3', (
Issue-Commit: 3879dd05c1f9087ace2750498c91b002c8086d49

# glue: job runs never execute (every run ends FAILED), catalog reads ignore Lake Formation permissions, CreateJob duplicate-with-changes and Command defaults diverge

> _Note: this issue was identified against the code base at commit `3879dd05c1f9087ace2750498c91b002c8086d49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
This issue carries 4 root causes in Glue. Not covered by 20261025T000319Z-glue-iam-lakeformation-s3-a-glue-job-run.md or 20261022T131025Z-glue-createjob-is-a-501-stub-gettable-ignores.md, which carry other checks' replay keys.

Cause 1 of 4: StartJobRun never runs the job, so every run ends FAILED with an emulator-authored ErrorMessage.
AWS: a job run of a valid script reaches JobRunState SUCCEEDED, and ErrorMessage is present only on a FAILED run (Smithy: StartJobRun declares ConcurrentRunsExceededException, EntityNotFoundException, InvalidInputException; the run's terminal state is reported through GetJobRun/GetJobRuns).
Emulator: every run — including the positive control each check runs first — terminates FAILED with ErrorMessage 'This emulator records job runs but executes no job script, so the job run ended without running.'
Why: nothing downstream of a job run can be observed: run history ordering, the ErrorMessage-only-on-FAILED rule, MaxConcurrentRuns admission, a run that must fail only because its script was never uploaded, and the whole troubleshoot-a-failed-read workflow all collapse, because the SUCCEEDED case never exists.
Fix: the StartJobRun/GetJobRun state machine must drive a run to SUCCEEDED (and reserve FAILED plus ErrorMessage for genuine failures such as a missing script or a denied catalog read).
Traces: glue__iam__lakeformation__s3.e2e-troubleshoot-glue-job-table-read-failure-lifecycle, glue__iam__lakeformation__s3.glue-deleted-job-drops-its-run-history-and-refuses-new-runs, glue__iam__lakeformation__s3.glue-get-job-runs-lists-newest-first-and-error-message-only-on-failed, glue__iam__lakeformation__s3.glue-update-job-raising-max-concurrent-runs-to-two-refuses-only-the-third-run, glue__iam__s3.gjwr-job-run-of-a-never-uploaded-script-fails-until-it-is-uploaded, glue__iam__s3.gjwr-role-name-reuse-through-the-other-region-client-rejected.

Cause 2 of 4: Glue catalog reads ignore Lake Formation permissions.
AWS: for a Lake-Formation-governed database, GetTables lists only the tables the caller holds a grant on (none, with no grants), GetTable is refused for a table the caller has no LF grant on and is restored by a SELECT grant alone (no DESCRIBE required), and GetDatabase is refused once IAMAllowedPrincipals is revoked until a grant is made.
Emulator: GetTables lists every governed table with no grants at all ('usage_daily', 'usage_events'), GetTable succeeds where AWS refuses it, and the GetDatabase case never converges and times out at 60 s — the catalog handlers never consult LF.
Why: fine-grained Lake Formation access control is unenforceable, and a caller sees table metadata AWS withholds.
Fix: the Glue catalog read handlers (GetTable, GetTables, GetDatabase) must evaluate Lake Formation permissions for the caller before answering.
Traces: glue__iam__lakeformation__s3.lf-get-tables-lists-only-granted-governed-tables, glue__iam__lakeformation__s3.lf-select-grant-alone-restores-get-table-no-describe-required, glue__iam__lakeformation__s3.lf-database-iap-revoked-get-database-denied-until-a-table-grant.

Cause 3 of 4: a second CreateJob for the same name with a changed configuration is answered with the wrong error class.
AWS: CreateJob declares BOTH AlreadyExistsException and IdempotentParameterMismatchException (Smithy: CreateJob errors include AlreadyExistsException and IdempotentParameterMismatchException, each HTTP 400). Re-submitting the same job name with a DIFFERENT configuration (here only Role changed) is refused IdempotentParameterMismatchException with 'Job already submitted with different configuration.' — fixed text naming no job.
Emulator: answers AlreadyExistsException 'Job already exists: <job-name>'.
Why: the two codes imply different retry semantics — a caller that recreates its execution role between runs must learn its configuration diverged, not that the name is taken — so it retries or gives up wrongly.
Fix: the CreateJob handler must compare the submitted configuration against the stored job and pick the idempotency-mismatch error.
Traces: codecommit__codepipeline__glue__iam__lambda.refarch-glue-create-job-duplicate-name-rejected.

Cause 4 of 4: Command.PythonVersion is not defaulted inside the structure the caller supplied.
AWS: CreateJob/GetJob return Command (Smithy: CreateJob.Command targets JobCommand) with PythonVersion = '3' filled in by the service when the caller sent no PythonVersion; the default is added INSIDE the caller's Command structure.
Emulator: GetJob answers Command.PythonVersion = None — the Command is echoed through verbatim.
Why: a client that reads back the effective job definition (a deploy that diffs desired against actual) sees a missing field and reports spurious drift.
Fix: the CreateJob handler's JobCommand defaulting, surfaced by GetJob.
Traces: codecommit__codepipeline__glue__iam__lambda.refarch-glue-job-command-and-defaults-round-trip.

## Evidence
Cause 1: "the control job ended 'FAILED': 'This emulator records job runs but executes no job script, so the job run ended without running.'" (also 'the control run ended FAILED', 'the second run ended FAILED', 'the warm-up run ended FAILED', and '[phase 3/5 solve] after the grant the job ended FAILED').
Cause 2: "with no grants the reader must list no governed table, got ['usage_daily', 'usage_events']"; 'the call succeeded; real AWS refuses it, so there is no error' for GetTable; 'Error: Check timed out after 60.0s' for the revoked-IAMAllowedPrincipals GetDatabase poll.
Cause 3: "raised ('AlreadyExistsException', 'Job already exists: <job>'), expected ('IdempotentParameterMismatchException', 'Job already submitted with different configuration.')".
Cause 4: 'Command.PythonVersion came back None, expected "3"'.

## Steps to Reproduce
1. Cause 1: CreateJob with a valid script in S3 and StartJobRun; poll GetJobRun for the terminal state.
2. Cause 2: create an LF-governed database with two tables, revoke IAMAllowedPrincipals, and call GetTables/GetTable/GetDatabase as a principal with no LF grant; then grant SELECT only and retry GetTable.
3. Cause 3: CreateJob twice with the same Name and a different Role.
4. Cause 4: CreateJob with a Command omitting PythonVersion, then GetJob.
5. Or replay the check ids in trace_ids.
