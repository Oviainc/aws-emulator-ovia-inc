Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild.build-queue-timeout-in-minutes-enforced
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild.build-queue-timeout-in-minutes-enforced|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Commit: 51ca4c2291cddeccaa5c76d2f3d0dac1588f79fd

# codebuild: a build is never held QUEUED behind a full reserved-capacity fleet and queuedTimeoutInMinutes is never enforced (2 causes)

> _Note: this issue was identified against the code base at commit `51ca4c2291cddeccaa5c76d2f3d0dac1588f79fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBuild

## Description
Two net-new root causes in the CodeBuild build lifecycle, both exposed by the check `codebuild.build-queue-timeout-in-minutes-enforced`, which exhausts its 600 s budget without ever observing the queue semantics it grades.

Cause 1 of 2: a build started against a project pinned to a reserved-capacity compute fleet that is already fully occupied is not held in the QUEUED phase.
AWS: `StartBuild` on a project whose `environment.fleet.fleetArn` names a fleet with `baseCapacity` 1 and `overflowBehavior` QUEUE, while another build of that project already occupies the fleet's single slot, leaves the second build with `buildStatus` IN_PROGRESS and `currentPhase` QUEUED (Smithy: `ProjectEnvironment$fleet` -> `ProjectFleet$fleetArn`; `CreateFleetInput$overflowBehavior` / `FleetOverflowBehavior` documents QUEUE as "your build remains in the queue until a compute instance from your fleet becomes available"; `BuildPhase$phaseType`/`StatusType`). The build leaves QUEUED only when fleet capacity frees up; it does not run concurrently with the build holding the slot. Recorded behavior shows the first build sitting in `currentPhase` BUILD for the length of its buildspec while the second waits in QUEUED.
Emulator: fleet capacity has no bearing on a build at all. Every `StartBuild` stores a build whose terminal transition is due a fixed 750 ms later (`BUILD_SETTLE_MILLIS`), and the settle step moves it straight from IN_PROGRESS/QUEUED to `currentPhase` COMPLETED with the status its buildspec projection computed — there is no capacity accounting, no slot, and no admission control keyed on the fleet. A second build therefore never sits QUEUED behind the first, and the first never dwells in BUILD either (its phases go SUBMITTED/QUEUED -> COMPLETED within ~750 ms, and the already-open issues 20261109T141609Z-codebuild-codepipeline-ecr-3-net-new-causes-batchgetbuilds.md and 20261108T090606Z-codebuild-codepipeline-ecr-pipeline-driven-builds-never-execute.md track that builds never execute). The replay consequence is that the check's wait for the holder build to report `currentPhase` BUILD never succeeds and the check is killed at its 600 s ceiling.
Why: a client that sizes a reserved-capacity fleet and relies on QUEUE overflow to serialize builds sees every build admitted immediately. Concurrency limits derived from fleet capacity are invisible, queue-depth dashboards read zero, and any code that polls for the QUEUED phase to decide whether to wait, cancel or scale never sees it.
Fix: the StartBuild handler and the build state machine (fleet-capacity admission when `environment.fleet`/`fleetOverride` is set, and a build that stays in QUEUED until a slot is free), in the CodeBuild build state.
Traces: codebuild.build-queue-timeout-in-minutes-enforced.

Cause 2 of 2: `queuedTimeoutInMinutes` / `queuedTimeoutInMinutesOverride` is stored but never enforced, so a build that outwaits its queue window never times out.
AWS: a build that exceeds its queue window while still QUEUED completes — `buildComplete` true, `buildStatus` FAILED, `currentPhase` COMPLETED — with the QUEUED phase itself carrying `phaseStatus` TIMED_OUT (Smithy: `StartBuildInput$queuedTimeoutInMinutesOverride` targets `TimeOut` with @range min 5 max 480, documented as "The number of minutes a build is allowed to be queued before it times out"; `BuildPhase$phaseStatus` targets `StatusType` whose values include TIMED_OUT). TIMED_OUT as the overall `buildStatus` is the separate `timeoutInMinutes` limit on an active phase, not this one; recorded behavior for the queue window is overall FAILED with the QUEUED phase TIMED_OUT.
Emulator: `queuedTimeoutInMinutes` is only defaulted onto the stored project (480 minutes) and echoed back on reads; the override is accepted and discarded. No queue deadline is stored next to the build and nothing materializes it: the only due instant a stored build carries is the fixed terminal settle, so no build ever completes because its queue window elapsed, and no build ever reports a QUEUED phase whose `phaseStatus` is TIMED_OUT. Combined with cause 1, a build cannot even reach the state the timeout applies to.
Why: a client that bounds its queue wait with `queuedTimeoutInMinutesOverride` to fail fast when capacity is unavailable gets no refusal and no terminal build attributable to the queue; retry/alerting logic keyed on FAILED-with-QUEUED-TIMED_OUT never fires, and a queued build looks indistinguishable from one that ran.
Fix: the StartBuild handler's queue-timeout resolution (override, then project value, then the service default) plus the build state machine's clock materialization, so a build still QUEUED past its window becomes FAILED/COMPLETED with the QUEUED phase TIMED_OUT.
Traces: codebuild.build-queue-timeout-in-minutes-enforced. Source citations for both causes: services/service-com-amazonaws-codebuild-codebuild-20161006/src/state.rs lines 78-86 (BUILD_SETTLE_MILLIS), 2444-2462 (StoredBuild::settle, the only transition a build gets), 3919-3965 (DEFAULT_QUEUED_TIMEOUT_MINUTES defaulted and never read again).

## Evidence
Check codebuild.build-queue-timeout-in-minutes-enforced, verdict mismatch, error: "Check timed out after 600.0s".

Expected (AWS): CreateFleet(baseCapacity=1, overflowBehavior=QUEUE) -> fleet ACTIVE; build A on a project pinned to that fleet reaches currentPhase BUILD; build B started with queuedTimeoutInMinutesOverride=5 reports buildStatus IN_PROGRESS, currentPhase QUEUED while A holds the slot; after the 5-minute window (and not before ~4.5 minutes) B reports buildComplete=true, buildStatus=FAILED, currentPhase=COMPLETED, and the phase with phaseType QUEUED has phaseStatus TIMED_OUT.

Actual (emulator): build A never reports currentPhase BUILD — StoredBuild::settle flips a build from IN_PROGRESS (phases SUBMITTED, QUEUED) to currentPhase COMPLETED 750 ms after StartBuild, so the poll for BUILD never succeeds and the check is killed at its 600 s ceiling. Build B, had it been started, would likewise settle in 750 ms rather than waiting on the fleet's single slot: no capacity accounting exists, and queuedTimeoutInMinutes/queuedTimeoutInMinutesOverride is defaulted to 480 on the project and never consulted, so no QUEUED phase ever takes phaseStatus TIMED_OUT and no build ever completes FAILED because its queue window elapsed.

Message-bearing requirement: none of these are error responses — the divergence is in the reported state machine (buildStatus, currentPhase, phases[].phaseStatus). The fixed text is the enum values FAILED / COMPLETED / QUEUED / TIMED_OUT; the variable parts (build ids, fleet ARN, project name, timestamps) come from the request and the emulator's state and are not part of the requirement.

## Steps to Reproduce
1. codebuild.create_fleet(name=<n>, baseCapacity=1, environmentType=LINUX_CONTAINER, computeType=BUILD_GENERAL1_SMALL, overflowBehavior=QUEUE); poll batch_get_fleets until status.statusCode == ACTIVE.
2. codebuild.create_project with source {type: NO_SOURCE, buildspec: "version: 0.2\nphases:\n  build:\n    commands:\n      - sleep 600\n"}, artifacts NO_ARTIFACTS, environment {type: LINUX_CONTAINER, image: aws/codebuild/standard:7.0, computeType: BUILD_GENERAL1_SMALL, fleet: {fleetArn: <fleet arn>}}.
3. codebuild.start_build(projectName=<p>) -> build A. Poll batch_get_builds([A]) for currentPhase == BUILD: on AWS it arrives; on the emulator A is COMPLETED within a second and BUILD is never reported.
4. codebuild.start_build(projectName=<p>, queuedTimeoutInMinutesOverride=5) -> build B. Poll batch_get_builds([B]): on AWS currentPhase == QUEUED while A holds the fleet slot; on the emulator B settles to COMPLETED immediately.
5. Keep polling B for buildComplete: on AWS, after ~5 minutes, buildStatus == FAILED, currentPhase == COMPLETED and the QUEUED phase has phaseStatus == TIMED_OUT; on the emulator no such transition ever occurs.
