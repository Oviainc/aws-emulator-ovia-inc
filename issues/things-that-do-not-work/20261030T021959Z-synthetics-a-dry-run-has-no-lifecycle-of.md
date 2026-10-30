Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: synthetics.start-canary-dry-run-independent-of-schedule
Issue-Shape-Id: com.amazonaws.synthetics#Synthetics
Issue-Key: synthetics.start-canary-dry-run-independent-of-schedule|run#16|AssertionError|assert any(r.get("Status", {}).get("State") == "PASSED" for r in dry_runs), dry_runs
Issue-Commit: 310139f962ec3e10f1f82f7e1678ad8609e6968b

# synthetics: a dry run has no lifecycle of its own — GetCanary(DryRunId) echoes the main canary's status, and the dry run's CanaryRun never reaches PASSED

> _Note: this issue was identified against the code base at commit `310139f962ec3e10f1f82f7e1678ad8609e6968b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCanaryDryRun

## Description
Two distinct root causes in the emulated CloudWatch Synthetics dry-run path, both grounded in the failing check synthetics.start-canary-dry-run-independent-of-schedule. Both must be closed before this issue is resubmitted. (Related but NOT covering: open issue 20261029T184613Z-synthetics-4-net-new-root-causes-duplicate-associateresource.md, whose Cause 3 describes the same unconditional run-failure model for SCHEDULED runs; it does not carry this check's replay key and says nothing about the dry-run path, so Cause 2 below is listed here explicitly.)

=== Cause 1 of 2: GetCanary with DryRunId reports the MAIN canary's Status (and Timeline) instead of the dry run's own CanaryState lifecycle ===
AWS: Smithy (services/service-com-amazonaws-synthetics-synthetics/smithy-model/com-amazonaws-synthetics-synthetics.json) — GetCanaryRequest$DryRunId: "The DryRunId associated with an existing canary's dry run. You can use this DryRunId to retrieve information about the dry run." Canary$Status is a CanaryStatus whose State is the CanaryState enum (CREATING | READY | STARTING | RUNNING | UPDATING | STOPPING | STOPPED | ERROR | DELETING). Recorded behavior: with DryRunId set, Canary.Status.State is the DRY RUN's own lifecycle — it is in-progress (CREATING/RUNNING) while the dry run executes and settles to a terminal CanaryState (STOPPED, or ERROR) when the dry run finishes — while the same GetCanary WITHOUT DryRunId keeps reporting the scheduled canary's state (READY). Canary.Timeline likewise describes the dry run's own start/stop, not the canary's.
Emulator: GetCanary(Name, DryRunId) returns the LIVE canary's status verbatim. src/state.rs StoredCanary::to_dry_run_canary builds the projection from the dry run's stored configuration and then overwrites `canary.status = live.status` and `canary.timeline = live.timeline`, and src/ops/get_canary.rs returns that. Nothing in the store records a per-dry-run state at all (StoredDryRun holds only `id` and `config`), so a dry run started on a READY canary reports Status.State = READY immediately — a value that is NOT in the in-progress set, i.e. it looks terminal from the first poll, and it never changes for the dry run's whole life.
Why: a client cannot observe a dry run's progress or completion. Polling GetCanary(Name, DryRunId) for a terminal CanaryState returns "already finished" on the very first call (or, for a canary that happens to be RUNNING, reports in-progress forever regardless of the dry run), so the client proceeds to read results that do not exist yet — exactly what the check did: it exited the poll loop on the first iteration and then found the dry run's CanaryRun still RUNNING with no Timeline.Completed. The dry run's independence from the schedule, which is the entire point of StartCanaryDryRun, is unobservable.
Fix: the dry-run record and its clock in src/state.rs (StoredDryRun / StoredCanary::to_dry_run_canary / StoredCanary::settle) plus src/ops/start_canary_dry_run.rs: a dry run must carry its OWN CanaryState and timeline, set in-progress when StartCanaryDryRun records it and settled to a terminal CanaryState by the same clock that settles the run it started, and to_dry_run_canary must project that instead of copying the live canary's status/timeline.
Traces: synthetics.start-canary-dry-run-independent-of-schedule.

=== Cause 2 of 2: a dry run's CanaryRun can never be PASSED — every run the emulator starts is driven to FAILED/EXECUTION_FAILURE ===
AWS: CanaryRun$Status is a CanaryRunStatus whose State is the CanaryRunState enum (RUNNING | PASSED | FAILED); StateReasonCode EXECUTION_FAILURE is documented as "a non-critical failure occurred such as failing to save generated debug artifacts". Recorded behavior: a dry run of a sound canary (trivially-passing handler, execution role granting the S3/logs permissions) appears in GetCanaryRuns(Name, RunType=DRY_RUN) and completes with Status.State = PASSED, no StateReason/StateReasonCode, and Timeline.Completed set; it creates no RunType=CANARY_RUN entry.
Emulator: the run StartCanaryDryRun records (src/ops/start_canary_dry_run.rs, StoredRun { state: CanaryRunState::Running, outcome_due: now + RUN_SETTLE }) is settled unconditionally to CanaryRunState::Failed with StateReason = "This emulator has no canary runtime, so the canary script was not executed and the run produced no result." and StateReasonCode = "EXECUTION_FAILURE" by StoredCanary::settle in src/state.rs. PASSED is unreachable for a dry run, independent of the canary, its script or its role. Until the 500 ms RUN_SETTLE elapses the entry is reported as State RUNNING with no Timeline.Completed, which is what the check captured.
Why: the success side of StartCanaryDryRun cannot be emulated. A client that dry-runs a candidate configuration before promoting it (the documented use of the operation) always reads a failing verdict and refuses to promote; and because the blanket failure uses the same EXECUTION_FAILURE reason code a genuinely broken canary gets, a real dry-run failure is indistinguishable from the emulator's placeholder. DryRunConfigOutput$LastDryRunExecutionStatus, which src/state.rs dry_run_config_output derives from that run's state, inherits the same wrong value.
Fix: the run-outcome model in src/state.rs (StoredCanary::settle / close_open_run and the RUN_NOT_EXECUTED_* constants) as it applies to runs carrying a dry_run_id: the emulator must derive a verdict — PASSED for a dry run whose configuration and role are sound, FAILED with the appropriate reason code only when a modeled precondition is unmet — and set Timeline.Completed when it settles.
Traces: synthetics.start-canary-dry-run-independent-of-schedule.

## Evidence
Suite: simparity-behavioral-aws-v1, synthetics 10/23 passed (43%).

[mismatch] synthetics.start-canary-dry-run-independent-of-schedule:
  File "check_synthetics_start_canary_dry_run_independent_of_schedule.py", line 70, in run
    assert any(r.get("Status", {}).get("State") == "PASSED" for r in dry_runs), dry_runs
  AssertionError: [{'Id': '<run-uuid>', 'Name': '<canary>', 'Status': {'State': 'RUNNING'}, 'Timeline': {'Started': <ts>}, 'ArtifactS3Location': 's3://<bucket>', 'DryRunConfig': {'DryRunId': '<dry-run-uuid>'}}]
(The uuids, canary/bucket names and timestamp above are per-run values and are NOT part of the requirement.)

Cause 1: the check polls, per iteration, GetCanary(Name) expecting READY (that assertion held) and GetCanary(Name, DryRunId=<id>).Canary.Status.State expecting the DRY RUN's lifecycle to pass through {CREATING, STARTING, RUNNING, UPDATING, STOPPING} before a terminal state. The emulator answered a non-in-progress state on the FIRST poll, so the loop exited after ~0 s instead of waiting for the dry run — because src/state.rs StoredCanary::to_dry_run_canary (lines ~615-625) assigns `canary.status = live.status` / `canary.timeline = live.timeline` from the main canary record, which was READY. StoredDryRun (src/state.rs) stores only { id, config } — there is no per-dry-run state to report.

Cause 2: GetCanaryRuns(Name, RunType=DRY_RUN) returned the dry run's entry with Status.State='RUNNING' and no Timeline.Completed; expected a terminal 'PASSED'. src/ops/start_canary_dry_run.rs creates StoredRun{ state: Running, outcome_due: now + RUN_SETTLE (500 ms, src/state.rs) }, and src/state.rs StoredCanary::settle then sets every such run to CanaryRunState::Failed with StateReason='This emulator has no canary runtime, so the canary script was not executed and the run produced no result.' / StateReasonCode='EXECUTION_FAILURE' — so the entry goes RUNNING -> FAILED and never PASSED.

## Steps to Reproduce
1. Create an S3 artifact bucket and an IAM execution role granting the canary's logs/S3 permissions.
2. CreateCanary(Name=<name>, Code={ZipFile: trivially-passing handler}, ArtifactS3Location=s3://<bucket>, ExecutionRoleArn=<role>, Schedule={Expression: 'rate(1 hour)'}, RuntimeVersion=<a dry-run-capable syn-nodejs-puppeteer runtime>); poll GetCanary until Status.State == READY.
3. StartCanaryDryRun(Name=<name>) -> note DryRunConfig.DryRunId.
4. Cause 1: immediately call GetCanary(Name=<name>, DryRunId=<id>) -> emulator answers Canary.Status.State = 'READY' (the main canary's state) and keeps answering it; AWS answers the dry run's own CanaryState (in-progress, e.g. RUNNING, then terminal STOPPED). Compare with GetCanary(Name=<name>) without DryRunId, which also answers READY — the two responses are indistinguishable.
5. Cause 2: poll GetCanaryRuns(Name=<name>, RunType='DRY_RUN') -> the dry run's entry is Status.State='RUNNING' for ~500 ms and then 'FAILED' with StateReasonCode='EXECUTION_FAILURE' and StateReason='This emulator has no canary runtime, …' forever; AWS reports Status.State='PASSED' with Timeline.Completed set and no StateReason.
