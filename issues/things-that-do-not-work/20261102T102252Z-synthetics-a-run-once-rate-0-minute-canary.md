Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: synthetics.start-canary-transitions-running-then-stopped
Issue-Shape-Id: com.amazonaws.synthetics#Synthetics
Issue-Key: synthetics.start-canary-transitions-running-then-stopped|_wait_for_canary_state#16|Timeout|return status["State"]
Issue-Commit: 2c8d5d123f0b7ec8bc82f889cc1f7ca03650fb6a

# synthetics: a run-once (rate(0 minute)) canary never leaves RUNNING — StartCanary records no RUNNING→STOPPED due instant, so the one-shot lifecycle never completes

> _Note: this issue was identified against the code base at commit `2c8d5d123f0b7ec8bc82f889cc1f7ca03650fb6a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCanary

## Description
One net-new root cause in the emulated CloudWatch Synthetics service, grounded in the failing behavioral check `synthetics.start-canary-transitions-running-then-stopped`.

NOTE: an open issue, 20261029T184613Z-synthetics-4-net-new-root-causes-duplicate-associateresource.md, describes a similar lifecycle gap as its "Cause 4 of 4" and lists this check among its trace ids, but the replay no longer reports this check as owned by it (the check's failure signature has changed to a bare harness timeout with no frame, so its replay key no longer matches that issue's recorded key). This check is therefore still an open, unowned divergence; it is filed here so it is not lost. A fixer closing it should reconcile with that issue rather than treat the two as independent work.

=== Cause 1 of 1: StartCanary on a run-once schedule leaves CanaryState stuck at RUNNING forever ===
AWS: `CanaryState` (Smithy: com.amazonaws.synthetics#CanaryState) is the enum CREATING | READY | STARTING | RUNNING | UPDATING | STOPPING | STOPPED | ERROR | DELETING. `CanaryScheduleOutput$Expression` is documented in the model as: "Specifying `rate(0 minute)` or `rate(0 hour)` is a special value that causes the canary to run only once when it is started." So `StartCanary` (PATCH /canary/{Name}/start, modeled errors ConflictException 409 / ResourceNotFoundException 404 / ValidationException 400 / InternalServerException 500) drives READY -> STARTING/RUNNING and, for a run-once schedule, the canary settles back to STOPPED by itself once that single run finishes — no StopCanary call is involved. At least one run record is then readable through GetCanaryRuns. Recorded behavior only for the exact dwell times (how long STARTING/RUNNING is observable before the settle).
Emulator: the canary is parked in RUNNING permanently. `StartCanary::handle` (services/service-com-amazonaws-synthetics-synthetics/src/ops/start_canary.rs) sets `canary.state = CanaryState::Running` and schedules a due instant only for the RUN's outcome (`outcome_due: Some(now + RUN_SETTLE)`); it records no due instant for the canary itself. `StoredCanary::settle` (services/service-com-amazonaws-synthetics-synthetics/src/state.rs) materialises exactly two clock transitions — CREATING -> READY when `ready_due` has passed, and each open run's outcome when `outcome_due` has passed — and has no branch that moves `self.state` out of `CanaryState::Running`. The only path to STOPPED is an explicit StopCanary. A client polling GetCanary for {STOPPED, ERROR} after starting a `rate(0 minute)` canary therefore polls forever: the check was killed by its harness (`Check timed out after 60.0s`) while in that wait, having already successfully observed RUNNING.
Why: the on-demand canary lifecycle — the standard way to execute a canary once and wait for it to finish — can never complete against this emulator. Any client that starts a run-once canary and waits for it to settle blocks indefinitely instead of observing STOPPED; the schedule's `rate(0 ...)`/`DurationInSeconds` semantics are unobservable, and nothing distinguishes a canary still working from one that finished its single run. Because the hang is unbounded rather than a wrong value, it also costs the caller its own timeout budget rather than returning an error.
Fix: the canary lifecycle clock in src/state.rs (`StoredCanary::settle`, alongside the existing `ready_due`/`outcome_due` handling) together with the StartCanary handler: a started canary whose schedule expression is a run-once `rate(0 minute)`/`rate(0 hour)` must record a due instant at which it transitions RUNNING -> STOPPED once its single run has completed, while a recurring schedule keeps holding RUNNING until StopCanary. No status, error code or message is involved — this is purely a missing state transition.
Traces: synthetics.start-canary-transitions-running-then-stopped.

Out of scope here (owned elsewhere): the same check's later assertion that the recorded run reaches `Status.State = PASSED` fails because every run in this emulator is forced to FAILED/EXECUTION_FAILURE; that run-verdict cause is "Cause 3 of 4" of the open issue named above and is grounded in a different check (synthetics.canary-run-fails-without-execution-role-permissions), so it is not re-filed here. Closing only the run-verdict cause will NOT make this check pass, and closing only this transition will leave it failing on PASSED.

## Evidence
Suite: simparity-behavioral-aws-v1, synthetics 13/23 passed (57%), 10 failed.

Failing check (not marked ALREADY FILED and not marked OWNED by any open issue in this replay):
- [mismatch] synthetics.start-canary-transitions-running-then-stopped — `Error: Check timed out after 60.0s`.

What the check does, and where it hangs (check body quoted in the replay report):
1. creates an S3 artifact bucket + execution role, picks the latest non-deprecated syn-nodejs-puppeteer runtime;
2. CreateCanary(Schedule={"Expression": "rate(0 minute)"}) and waits for READY — succeeds;
3. StartCanary(Name) — succeeds (HTTP 200);
4. `_wait_for_canary_state(ctx, name, {"STARTING", "RUNNING"}, timeout_s=30)` — succeeds, so a real transitional state IS observed;
5. `_wait_for_canary_state(ctx, name, {"STOPPED", "ERROR"}, timeout_s=300, interval_s=2)` — GetCanary keeps answering `Canary.Status.State = "RUNNING"`; the state never changes and the harness kills the check at its 60 s budget. AWS settles the canary to STOPPED on its own once the single run finishes.

Source confirming the missing transition in the current checkout:
- services/service-com-amazonaws-synthetics-synthetics/src/ops/start_canary.rs:176-198 — `canary.state = CanaryState::Running;` then `begin_run(StoredRun { ..., outcome_due: Some(now + RUN_SETTLE), ... })`. Only the RUN gets a due instant; the canary gets none.
- services/service-com-amazonaws-synthetics-synthetics/src/state.rs:533-556 — `StoredCanary::settle` handles only `CanaryState::Creating` + `ready_due` and each run's `outcome_due`; there is no arm for `CanaryState::Running`, so RUNNING is terminal until StopCanary.
- services/service-com-amazonaws-synthetics-synthetics/src/state.rs:169 — `RUN_SETTLE = 500ms`, i.e. the run does settle promptly; it is the canary's own state that never moves.

Model citations: Smithy CanaryState enum = CREATING|READY|STARTING|RUNNING|UPDATING|STOPPING|STOPPED|ERROR|DELETING; CanaryScheduleOutput$Expression doc: "Specifying rate(0 minute) or rate(0 hour) is a special value that causes the canary to run only once when it is started." (services/service-com-amazonaws-synthetics-synthetics/smithy-model/com-amazonaws-synthetics-synthetics.json)

## Steps to Reproduce
1. CreateGroup/bucket/role setup as needed; create an S3 artifact bucket and an execution role.
2. synthetics.CreateCanary(Name=<name>, Code={ZipFile:<trivial handler>, Handler:"canary.handler"}, ArtifactS3Location="s3://<bucket>", ExecutionRoleArn=<role>, Schedule={"Expression": "rate(0 minute)"}, RuntimeVersion=<latest syn-nodejs-puppeteer>).
3. Poll GetCanary(Name) until Canary.Status.State == "READY".
4. StartCanary(Name) -> 200.
5. Poll GetCanary(Name).Canary.Status.State: it reports RUNNING and STAYS RUNNING indefinitely (observed well past the run's own settle, and past the check's 60 s budget). Expected (AWS): the canary runs once and settles to STOPPED on its own, with no StopCanary call, after which GetCanaryRuns returns at least one run record.
6. Contrast: a recurring schedule (e.g. rate(1 minute)) correctly stays RUNNING until StopCanary — only the run-once case is broken.
