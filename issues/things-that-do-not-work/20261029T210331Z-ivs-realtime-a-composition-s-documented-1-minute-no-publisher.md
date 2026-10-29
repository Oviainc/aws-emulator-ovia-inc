Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ivs-realtime.composition-auto-shutdown-without-publisher
Issue-Shape-Id: com.amazonaws.ivsrealtime#AmazonInteractiveVideoServiceRealTime
Issue-Key: ivs-realtime.composition-auto-shutdown-without-publisher|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Commit: 11484c95ce1698b1f3bedb856134c4b0d271842b

# A Composition's documented 1-minute no-publisher auto-finalize never yields an observable STOPPED state with endTime

> _Note: this issue was identified against the code base at commit `11484c95ce1698b1f3bedb856134c4b0d271842b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetComposition

## Description
One root cause, grounded in the one failing behavioral check of this suite that is not already filed.

Cause 1 of 1 — a composition that is auto-finalized by the documented no-participant timeout never reports an observable terminal state: a caller polling GetComposition after StartComposition against an empty stage sees no STOPPED/FAILED at all, because the emulator reaches STOPPED only half a second before it deletes the record.

AWS: Smithy: StartComposition documentation states a Composition "is an ephemeral resource" and that "Composition stops and the resource is deleted: ... After a 1-minute timeout, if there are no participants in the stage when StartComposition is called" and "After a 1-minute timeout, when all participants are disconnected from the stage"; GetComposition returns the Composition shape whose `state` (CompositionState) and `endTime` members report the finalized broadcast, and models ResourceNotFoundException (404) only for a composition that does not exist. Recorded behavior: around the 1-minute idle mark the composition is reported with state "STOPPED" and a populated `endTime`, and that record stays readable long enough that a client polling GetComposition every 5 seconds reliably observes it (HTTP 200) before the ephemeral resource disappears.

Emulator: the empty-stage timeout itself is honored (COMPOSITION_EMPTY_STAGE_TIMEOUT = 60s), but the terminal record is immediately thrown away: the clock-due stop begins at start+60s, STOPPED is reached one COMPOSITION_STOP_SPAN later and the record is reclaimed one further span after that — with COMPOSITION_STOP_SPAN = 500ms, STOPPED is reportable only inside a ~500ms window, after which GetComposition answers 404 ResourceNotFoundException with the message "Resource: <composition arn> not found" (fixed text "Resource: " + the composition ARN from the request + " not found"). In this replay the polling probe observed no terminal state whatsoever for the composition it had just started against a stage that never had a publisher: the check ran out its 60-second budget with every GetComposition still answering a non-terminal state, never STOPPED and never FAILED, and therefore never a populated endTime. Hypothesis: a 5-second poller lands on either side of the 500ms STOPPED window, so for all practical purposes the auto-finalized state is unobservable and the next read is a hard 404.

Why: the no-publisher auto-finalize is the documented way an IVS Composition ends when nobody publishes, and a client waits for it before deleting the EncoderConfiguration/StorageConfiguration the composition referenced. Against this emulator that wait never completes with a terminal state — it either never resolves or resolves into a 404 that is indistinguishable from "this composition never existed" — so cleanup logic and any consumer that needs endTime (billing/recording bookkeeping) breaks.

Fix: the composition lifecycle in the service state (services/service-com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime/src/state.rs — CompositionRecord::state_at, stop_began_at for the empty-stage deadline, is_reclaimed/materialize_due and the COMPOSITION_STOP_SPAN constant) must leave a clock-due, empty-stage stop readable as STOPPED with endTime for a span an ordinary poller can observe, and GetComposition/ListCompositions must report that state before the ephemeral record is reclaimed. Note the open issue 20261029T194619Z-ivs-realtime-composition-ingest-storage-lifecycle-diverges-a.md (Cause 1) reports the same one-second reclaim for a composition a caller stopped with StopComposition; it does not carry this check id and does not cover the clock-due empty-stage path exercised here, where no terminal state is observed at all.

Traces: ivs-realtime.composition-auto-shutdown-without-publisher.

## Evidence
Suite: simparity-behavioral-aws-v1, ivs-realtime 40/69 passed.

[mismatch] ivs-realtime.composition-auto-shutdown-without-publisher — "Check timed out after 60.0s". The check creates an S3 bucket, a stage (no participant ever joins), an encoder configuration and a storage configuration, calls StartComposition with one s3 destination, then polls GetComposition every 5s expecting composition.state in (STOPPED, FAILED) with a populated endTime and the transition to be observed near the documented ~60s idle window. No poll in the whole run reported STOPPED or FAILED, so the terminal state and endTime were never readable.

Code: src/state.rs:1636 COMPOSITION_EMPTY_STAGE_TIMEOUT = 60s; src/state.rs:1623 COMPOSITION_STOP_SPAN = 500ms; src/state.rs:1496-1521 state_at reports STOPPING until stop_began_at + 500ms and STOPPED after; src/state.rs:1701-1706 is_reclaimed removes the record at stop_began_at + 1000ms, after which GetComposition answers 404 ResourceNotFoundException "Resource: <composition arn> not found".

Expected: GetComposition 200 with composition.state "STOPPED" and endTime set, readable for a span a 5-second poller observes. Actual: no terminal state ever reported; the STOPPED window is ~500ms wide and is followed by a 404.

## Steps to Reproduce
1. Create an S3 bucket in the emulator's region.
2. CreateStage(name=<n>) — do not connect any participant to it.
3. CreateEncoderConfiguration(name=<n>); CreateStorageConfiguration(name=<n>, s3={bucketName: <bucket>}).
4. StartComposition(stageArn=<stage arn>, destinations=[{s3: {storageConfigurationArn: <storage arn>, encoderConfigurationArns: [<encoder arn>]}}]).
5. Poll GetComposition(arn=<composition arn>) every 5 seconds.
Expected: at about 60 seconds after the start the composition is reported with state "STOPPED" and a populated endTime, and that answer is observable across more than one poll before the ephemeral resource goes away.
Actual: every poll keeps reporting a non-terminal state; STOPPED is reportable only in a ~500ms window at start+60.0s..60.5s and the record is deleted at start+61.0s, after which GetComposition answers 404 ResourceNotFoundException "Resource: <composition arn> not found". The documented auto-finalize therefore never produces an observable STOPPED/endTime.
