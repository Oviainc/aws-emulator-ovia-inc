Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr-serverless.auto-stop-configuration-stops-idle-application
Issue-Shape-Id: com.amazonaws.emrserverless#AwsToledoWebService
Issue-Key: emr-serverless.auto-stop-configuration-stops-idle-application|_poll_until#7|Timeout|time.sleep(interval_s)
Issue-Commit: ab6490681e1ab26d5567ff83a1d290924eb5dab0

# EMR Serverless autoStopConfiguration never materialises STOPPED within an observable window: the idle span is the only lifecycle span the emulator does not compress

> _Note: this issue was identified against the code base at commit `ab6490681e1ab26d5567ff83a1d290924eb5dab0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApplication

## Description
Cause 1 of 1: an application created with `autoStopConfiguration = {enabled: true, idleTimeoutMinutes: 1}` and then started never reports `STOPPED` to a polling client; it stays `STARTED` for the whole observation window and the check is cut off still observing `STARTED`.

AWS: `AutoStopConfig` (Smithy, com.amazonaws.emrserverless#AutoStopConfig) models `enabled` ("Enables the application to automatically stop after a certain amount of time being idle. Defaults to true.") and `idleTimeoutMinutes` @range(min: 1, max: 10080) ("The amount of idle time in minutes after which your application will automatically stop. Defaults to 15 minutes."). `ApplicationState` is CREATING | CREATED | STARTING | STARTED | STOPPING | STOPPED | TERMINATED. AWS therefore moves an idle application from STARTED to STOPPED with no StopApplication call once the idle timeout has elapsed, and GetApplication reports `application.state == "STOPPED"`. The transition itself is status 200 on GetApplication with a changed `state`; there is no error code involved.

Emulator: the auto-stop IS implemented, but it is the ONE clock-driven span in this service that is not compressed. Every other transition is deliberately shortened so a poller can observe it: `LIFECYCLE_TRANSITION = Duration::from_millis(500)` (CREATING/STARTING/STOPPING/CANCELLING -> settled) and `JOB_RUN_STAGE_TRANSITION = Duration::from_millis(750)` per job-run stage, both documented in state.rs as spans the emulator "CHOOSES". By contrast `ApplicationRecord::idle_timeout()` returns `Duration::from_secs(minutes * 60)` straight from the request member, and `auto_stop_due()` returns `due_at(last_activity_at, idle_timeout())`, where `last_activity_at` is stamped by StartApplication (`record.mark_activity(now)`). So the smallest reachable auto-stop, at the modelled minimum `idleTimeoutMinutes = 1`, is start-call time + 60 s of real wall clock — strictly longer than the window in which the emulator is polled. The result observed on replay is that `GetApplication` returns `state = "STARTED"` on every probe and `STOPPED` (with the emulator's `AUTO_STOPPED_STATE_DETAILS`) is never materialised before the client gives up; the check ends with no STOPPED observation at all.

Why: a client that relies on autoStopConfiguration to release capacity cannot verify or exercise that behaviour against the emulator. Anything that waits for the STARTED -> STOPPED edge (a capacity/cost test, a teardown that expects the application to park itself, a state-machine assertion that the application is no longer STARTED) hangs rather than progressing, and the emulator looks as though autoStopConfiguration were ignored even though the field is stored and echoed back. Because this is the only non-compressed span, the behaviour is also inconsistent with the rest of the service: a job run settles in ~3 s while the idle stop it feeds needs a full minute.

Fix: the application record's idle-stop clock in the service state — `ApplicationRecord::idle_timeout()` / `auto_stop_due()` and the constant family they sit beside (`LIFECYCLE_TRANSITION`, `JOB_RUN_STAGE_TRANSITION`). The idle span owed by `autoStopConfiguration` should be scaled the same way the other chosen spans are, so the STARTED -> STOPPED edge, its `stateDetails` and its `updatedAt` at the due instant are all reachable by a polling client; the stored `autoStopConfiguration` echoed by GetApplication must keep reporting the caller's `idleTimeoutMinutes` verbatim.

Traces: emr-serverless.auto-stop-configuration-stops-idle-application.

## Evidence
Check: emr-serverless.auto-stop-configuration-stops-idle-application [mismatch]. Replay output: "Check timed out after 60.0s" — the check polls `GetApplication` until `application.state == "STOPPED"` and is cut off before any such observation.

Sequence exercised: CreateApplication(releaseLabel="emr-7.9.0", type="SPARK", autoStopConfiguration={enabled: true, idleTimeoutMinutes: 1}) -> StartApplication -> poll GetApplication for STARTED (reached, via the emulator's 500 ms STARTING span) -> poll GetApplication for STOPPED (never reached).

Expected (AWS): GetApplication 200 with `application.state == "STOPPED"` once the application has been idle past `idleTimeoutMinutes`, with no StopApplication call.
Actual (emulator): GetApplication 200 with `application.state == "STARTED"` on every probe for the whole window; `STOPPED` is only owed at start-time + 60 s of literal wall clock, computed by `ApplicationRecord::auto_stop_due() = due_at(last_activity_at, Duration::from_secs(idleTimeoutMinutes * 60))`, so it never materialises in the observation window.

Contrast inside the same crate (services/service-com-amazonaws-emrserverless-awstoledowebservice/src/state.rs): `LIFECYCLE_TRANSITION = Duration::from_millis(500)`, `JOB_RUN_STAGE_TRANSITION = Duration::from_millis(750)` — both annotated as spans the emulator chooses and "deliberately not zero"; the idle-stop span alone is taken literally from the request. StartApplication stamps the idle baseline at services/service-com-amazonaws-emrserverless-awstoledowebservice/src/ops/start_application.rs (`record.mark_activity(now)`).

No existing open issue covers this edge: the other emr-serverless issues on file concern job-run execution and logs (20261022T154846Z, 20261024T013257Z, 20261025T015858Z, 20261026T072046Z, 20261029T090829Z), the job-run/session lifecycle bundle (20261029T174551Z-emr-serverless-job-run-lifecycle-and-interactive-session.md) and the ListSessions stub (20261029T174551Z-emr-serverless-listsessions-stub.md). None mentions application autoStopConfiguration.

## Steps to Reproduce
1. CreateApplication with name=<unique>, releaseLabel="emr-7.9.0", type="SPARK", clientToken=<unique>, autoStopConfiguration={"enabled": true, "idleTimeoutMinutes": 1}.
2. StartApplication(applicationId=<id>).
3. Poll GetApplication(applicationId=<id>) until application.state == "STARTED" (this succeeds).
4. Submit no job runs and start no sessions, then keep polling GetApplication(applicationId=<id>) and wait for application.state == "STOPPED".
5. Observe the application remains "STARTED" for the entire polling window — the auto-stop edge is only owed a full literal minute after the StartApplication call, so no STOPPED state is ever observed and the client's wait is cut off.
