Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.emr-serverless-start-job-run-on-stopped-non-autostart-application-is-rejected
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr.emr-serverless-start-job-run-on-stopped-non-autostart-application-is-rejected|run#33|AssertionError|assert final == "SUCCESS", (
Issue-Commit: 71cff0481dbaf9b41262283aed7350a64aff149f

# EMR Serverless StartJobRun's accepted run terminates FAILED, so the autoStart state-gate check cannot grade its refusal (1 cause)

> _Note: this issue was identified against the code base at commit `71cff0481dbaf9b41262283aed7350a64aff149f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
One cause.

Attribution note: the failing check's id leads with the token `emr` (the file lives in behavioral/emr/ and declares SERVICES=['emr']), but it drives the separate emr-serverless model (com.amazonaws.emrserverless#AwsToledoWebService) through ctx.client('emr-serverless'); the service token is taken from the check id as the gate requires. Both `emr` and `emr-serverless` are stood up and answering.

Related open issues, named here rather than duplicated silently: 20261024T013257Z-emr-serverless-job-runs-never-execute-the-submitted.md and 20261022T154846Z-emr-serverless-job-runs-are-recorded-but-never-executed.md describe the same emulator behavior reached from athena__emr-serverless__glue__s3 checks and are filed under service `emr-serverless`. This replay key was NOT marked owned by either, and it is the one that proves the gap also voids EMR Serverless's autoStart state gate, so it is filed with its own grounding; a fixer closing those should re-replay the check listed below before closing this one.

Cause 1 of 1: a byte-identical StartJobRun that is correctly REFUSED while the application is not STARTED, and then accepted once StartApplication brings it to STARTED, never reaches a successful terminal state — the run settles on FAILED announcing that no workload is executed, which makes the state gate's refusal unprovable.
AWS: Smithy com.amazonaws.emrserverless#JobRunState enumerates SUBMITTED, PENDING, SCHEDULED, QUEUED, RUNNING, SUCCESS, FAILED, CANCELLING, CANCELLED; com.amazonaws.emrserverless#JobRun models `state` and `stateDetails` as required. A sparkSubmit run whose entryPoint resolves and whose executionRoleArn grants the S3 actions the script needs progresses through the non-terminal states and settles on SUCCESS; FAILED is reserved for a run whose driver actually ran and exited non-zero, or one that could not be placed. The refusal leg this check exercises is itself AWS behavior the emulator already matches: StartJobRun against an application in CREATED with autoStartConfiguration.enabled=false is refused with ValidationException and the fixed message "Job run cannot be started as Application is not in STARTED state and auto-start is disabled" (no variable parts), it records no job run, and it does not implicitly start the application. Recorded behavior only for the specific intermediate-state progression.
Emulator: the refusal leg, the empty ListJobRuns and the unchanged application state all pass; StartApplication reaches STARTED; the re-submitted identical request is accepted and allocates a jobRunId, and GetJobRun then reports terminal state 'FAILED' with stateDetails 'This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running.' The run's side effect is correspondingly absent — no part file is committed under the script's S3 output prefix.
Why: FAILED is also the state AWS uses for a genuine job failure, so a client cannot distinguish a run the service rejected from one it ran and lost; any poller or orchestrator that gates on SUCCESS takes its error path forever. Specifically for this check, the positive control is what proves the refusal above was a STATE gate rather than a blanket refusal of that request: with the control permanently FAILED, an emulator that refused every StartJobRun regardless of application state would be indistinguishable from a correct one, so the autoStart gate's fidelity cannot be graded at all.
Fix: the job-run lifecycle behind StartJobRun/GetJobRun in the emr-serverless crate's state layer — an accepted run must advance through the non-terminal JobRunState values to SUCCESS instead of being short-circuited to FAILED, with stateDetails appropriate to a completed run. (The assertions that follow in this check read Spark-written S3 bytes, which only a real Spark execution produces; they are not part of this request.)
Traces: emr.emr-serverless-start-job-run-on-stopped-non-autostart-application-is-rejected — assertion `final == "SUCCESS"` on the positive control, after the ValidationException refusal leg passed.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against this checkout: 28/101 passed; service emr 27/84.

[mismatch] emr.emr-serverless-start-job-run-on-stopped-non-autostart-application-is-rejected
check_emr_emr_serverless_start_job_run_on_stopped_non_autostart_application_is_rejected.py line 289:
  assert final == "SUCCESS", (...)
  AssertionError: the byte-identical StartJobRun must succeed once the application is STARTED — otherwise the refusal above proves nothing about the state gate — got 'FAILED' (stateDetails: 'This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running.')

Passed before it (so these are NOT the divergence): the application settles in CREATED with autoStartConfiguration.enabled=false; StartJobRun is refused with Error.Code "ValidationException" and the exact message "Job run cannot be started as Application is not in STARTED state and auto-start is disabled"; ListJobRuns returns []; the application state is unchanged; StartApplication reaches STARTED.

Expected (AWS): after StartApplication, the identical StartJobRun request is accepted and GetJobRun eventually reports jobRun.state == 'SUCCESS', having passed through non-terminal JobRunState values.
Actual (emulator): GetJobRun reports terminal state 'FAILED' with the stateDetails quoted above; no job output is produced.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-behavioral-ga-prod-v1 behavioral suite, check emr.emr-serverless-start-job-run-on-stopped-non-autostart-application-is-rejected. By hand:
2. Create an S3 bucket; upload a one-line PySpark script that writes its first argument to the S3 URI given as its second argument.
3. Create an IAM role trusted by emr-serverless.amazonaws.com granting the bucket's object actions.
4. emr-serverless CreateApplication(releaseLabel='emr-6.15.0', type='SPARK', autoStartConfiguration={'enabled': False}); poll GetApplication until state leaves CREATING — it settles in CREATED.
5. StartJobRun with that executionRoleArn and jobDriver.sparkSubmit pointing at the script. Observe the correct ValidationException refusal, that ListJobRuns is empty, and that the application is still CREATED.
6. StartApplication; poll GetApplication until state == 'STARTED'.
7. Re-submit the IDENTICAL StartJobRun request and poll GetJobRun until jobRun.state is SUCCESS/FAILED/CANCELLED.
8. Observe the terminal state is FAILED with stateDetails 'This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running.'; AWS reaches SUCCESS.
