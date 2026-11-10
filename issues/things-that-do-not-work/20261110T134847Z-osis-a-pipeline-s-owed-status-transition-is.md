Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: osis.stop-start-pipeline-transitions
Issue-Shape-Id: com.amazonaws.osis#AmazonOpenSearchIngestionService
Issue-Key: osis.stop-start-pipeline-transitions|_wait_for_pipeline_status#14|Timeout|time.sleep(interval_s)
Issue-Commit: 3a275ae8ff7628ae729e6996643a6fe8b123024e

# osis: a pipeline's owed status transition is held for a wall-clock dwell, so the full StopPipeline/StartPipeline lifecycle never completes for a polling client

> _Note: this issue was identified against the code base at commit `3a275ae8ff7628ae729e6996643a6fe8b123024e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPipeline

## Description
Cause 1 of 1: the emulator materialises a pipeline's owed status transition only after a fixed wall-clock dwell, so the GetPipeline that immediately follows a lifecycle call ALWAYS still reports the transient status. Every lifecycle leg therefore costs a polling client a whole poll interval, and a flow that chains three legs (CreatePipeline -> ACTIVE, StopPipeline -> STOPPING -> STOPPED, StartPipeline -> STARTING -> ACTIVE) never reaches its last leg inside the suite's per-check budget: osis.stop-start-pipeline-transitions is killed with 'Check timed out after 60.0s' before it reaches a single assertion.

AWS: Smithy GetPipeline (GET /2022-01-01/osis/getPipeline/{PipelineName}, 200) answers Pipeline.Status, of enum com.amazonaws.osis#PipelineStatus = CREATING, ACTIVE, UPDATING, DELETING, CREATE_FAILED, UPDATE_FAILED, STARTING, START_FAILED, STOPPING, STOPPED. StopPipeline (PUT /2022-01-01/osis/stopPipeline/{PipelineName}, 200) and StartPipeline (PUT /2022-01-01/osis/startPipeline/{PipelineName}, 200) each answer with the Pipeline in its transient status and model ConflictException (409) for an ineligible status. The model states no provisioning duration, so the settling behaviour is recorded behavior only: against real AWS this check was recorded green and a client polling GetPipeline observes CREATING -> ACTIVE, then ACTIVE -> STOPPING -> STOPPED, then STOPPED -> STARTING -> ACTIVE, each leg settling on its own and all three legs finishing inside the check's budget, after which GetPipeline still reports the same Pipeline.IngestEndpointUrls it reported before the stop (the endpoint is retained across a stop/start cycle). The requirement is that a pipeline the service reports in a transient status does actually settle into the terminal one, observably, for a client that keeps reading GetPipeline.

Emulator: the owed transition is gated on a wall clock. crate::state::TRANSITION_SPAN is a fixed non-zero Duration and StoredPipeline::settle returns early while `now < pending.due_at`; CreatePipeline, StopPipeline and StartPipeline each arm pending = {to: <settled status>, due_at: now + TRANSITION_SPAN} while setting status to CREATING / STOPPING / STARTING. A client's follow-up GetPipeline arrives within a few milliseconds over the loopback endpoint, so it is served the transient status on every leg, and the client then sleeps its whole poll interval (20 s in this check) before the next read. Three transient legs therefore cost at least 60 s of pure polling and the check is killed at the 60.0 s budget. Nothing is refused and no status is wrong in isolation — the divergence is that the transition is owed to a wall clock a loopback client always beats, rather than to the next observation of the record.
Hypothesis: the dwell is the cause of this timeout rather than a hang in any single call — the arithmetic is grounded in the check body (three waits, `last = get_pipeline(...)['Pipeline']['Status']; if last in targets: return; time.sleep(20)`) and in the emulator source, and matched by the fact that every other osis check in this suite that waits through only ONE transient leg got past its wait and failed later for an already-filed reason. The replay reports no per-call latency, so it cannot be confirmed directly from the output.

Why: a client that drives the pipeline lifecycle — wait for ACTIVE, stop, wait for STOPPED, start, wait for ACTIVE — cannot complete against this emulator in bounded time. CI suites, IaC waiters and SDK waiters tuned to AWS's own settling behaviour stall on a pipeline the emulator reports as perpetually mid-flight on the read right after the call, and everything gated behind a settled status (StopPipeline's ACTIVE-only precondition, StartPipeline's eligible-status set, DeletePipeline, PutResourcePolicy's refusal of CREATING) is reachable only by a caller that happens to wait longer than the dwell. Because this check dies before its assertions, the stop/start round trip and the retention of Pipeline.IngestEndpointUrls across a stop/start cycle are both left unverified.

Fix: the pipeline lifecycle state machine — crate::state::TRANSITION_SPAN and StoredPipeline::settle in state.rs, plus the three handlers that arm a PendingTransition (create_pipeline.rs, stop_pipeline.rs, start_pipeline.rs). The transition must become observable to the NEXT request that reads the record rather than after a wall-clock dwell, while the lifecycle operation's own response keeps reporting the transient status it is modelled to report, so checks that depend on seeing CREATING/STOPPING/STARTING in the mutating call's own response still see it.

Related open issue, which does NOT cover this check: 20261030T093920Z-osis-pipeline-lifecycle-never-settles-in-time-for.md describes the same dwell but carries only the replay keys of osis.start-pipeline-when-already-active-rejected and osis.concurrent-start-pipeline-exactly-one-succeeds; osis.stop-start-pipeline-transitions is not in its Issue-Trace-Ids and the replay report did not mark this check as owned by it, so it is filed here with its own replay key. The other open osis issues are unrelated: 20261029T174701Z (blueprint catalog / CreatePipeline validation / not-found messages / ListPipelines), 20261029T191455Z (NotImplemented refusals), 20261030T055610Z (UpdatePipeline not-found message), 20261109T133548Z (GetPipeline IngestEndpointUrls hostname), 20261109T184855Z (unassumable sts_role_arn), plus the ValidatePipeline and GetPipelineBlueprint stubs.

Traces: osis.stop-start-pipeline-transitions — 'Error: Check timed out after 60.0s'. Code: services/service-com-amazonaws-osis-amazonopensearchingestionservice/src/state.rs:55 (TRANSITION_SPAN) and :194-206 (StoredPipeline::settle); src/ops/create_pipeline.rs:288-293; src/ops/stop_pipeline.rs:174-179; src/ops/start_pipeline.rs:189-194; src/ops/get_pipeline.rs:145-158 (the read calls store.settle(now) but can only materialise a transition already due).

## Evidence
run_behavioral_suite (simparity-behavioral-aws-v1): total 14/23 passed (60.9%), osis 14/22 (64%) with 8 failed, bootstrap_signal=false (the emulator IS serving — 14 osis checks passed). Of the 8 osis failures, 5 are marked ALREADY FILED (GetPipelineBlueprint and ValidatePipeline scaffold stubs) and 2 are marked OWNED by open issues; osis.stop-start-pipeline-transitions is the only surviving one.

Failing check: [mismatch] osis.stop-start-pipeline-transitions — 'Error: Check timed out after 60.0s'.

Check body (GROUP=state-machine, tags axis:happy-path, cap:c1, cap:c4, concern:representation, plane:control, timing:long), docstring: 'StopPipeline drives ACTIVE -> STOPPING -> STOPPED and StartPipeline drives it back STOPPED -> STARTING -> ACTIVE; the ingest endpoint is retained across the cycle.' Sequence: create an S3 bucket + IAM pipeline role, CreatePipeline(MinUnits=1, MaxUnits=1), then
  1. _wait_for_pipeline_status(name, {ACTIVE, CREATE_FAILED}, timeout_s=1200, interval_s=20) and assert ACTIVE,
  2. endpoint_before = get_pipeline(...)['Pipeline']['IngestEndpointUrls'],
  3. stop_pipeline(name); _wait_for_pipeline_status(name, {STOPPED, START_FAILED}) and assert STOPPED,
  4. start_pipeline(name); _wait_for_pipeline_status(name, {ACTIVE, START_FAILED}) and assert ACTIVE,
  5. assert get_pipeline(...)['Pipeline']['IngestEndpointUrls'] == endpoint_before.
The waiter is `while time.monotonic() < deadline: last = osis.get_pipeline(PipelineName=name)['Pipeline']['Status']; if last in target_statuses: return last; time.sleep(interval_s)` — it reads FIRST and then sleeps a full 20 s, so a leg whose first read is transient costs one whole interval.

Expected (recorded against real AWS): all three waits return their settled status and the check reaches step 5, where the IngestEndpointUrls read after the start/stop cycle equals the one read before it.
Actual: each of the three post-call reads is served the transient status, so each leg burns a 20 s interval; 3 x 20 s >= the 60 s per-check budget and the check is killed at 60.0 s having executed no assertion.

Emulator source: state.rs:55 `pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);` with the comment 'deliberately non-zero, so a caller reading straight after the call still observes the transient status'; state.rs:194-206 `fn settle(&mut self, now: SystemTime)` returns early while `now < pending.due_at`; create_pipeline.rs sets status CREATING and arms pending {to: ACTIVE, due_at: now + TRANSITION_SPAN}; stop_pipeline.rs sets STOPPING and arms {to: STOPPED, ...}; start_pipeline.rs sets STARTING and arms {to: ACTIVE, ...}.

For contrast, the osis checks in this same run that wait through only ONE transient leg got past their wait and failed on already-filed NotImplemented/stub or endpoint-hostname causes, not on the budget.

## Steps to Reproduce
1. Boot the emulator from this checkout and point a botocore osis client at it.
2. CreatePipeline with PipelineName matching ^[a-z][a-z0-9\-]+$ (3-28 chars), MinUnits=1, MaxUnits=1 and a valid PipelineConfigurationBody.
3. Immediately GET /2022-01-01/osis/getPipeline/{PipelineName} -> observe Pipeline.Status == 'CREATING'; the emulator holds the owed ACTIVE for TRANSITION_SPAN, so any read inside that window is transient.
4. Poll GetPipeline on a 20-second interval until Status is ACTIVE — it takes a full interval, not the first read. Record Pipeline.IngestEndpointUrls.
5. PUT /2022-01-01/osis/stopPipeline/{PipelineName}, then immediately GetPipeline -> 'STOPPING'; poll on the same 20 s interval until 'STOPPED' (another full interval).
6. PUT /2022-01-01/osis/startPipeline/{PipelineName}, then immediately GetPipeline -> 'STARTING'; poll until 'ACTIVE' (a third full interval). The three legs together exceed 60 seconds of polling, so a client on a 60 s budget never reaches step 7.
7. Re-replay behavioral check osis.stop-start-pipeline-transitions to confirm the fix: it must complete inside the 60 s budget, observing ACTIVE -> STOPPING -> STOPPED -> STARTING -> ACTIVE and an unchanged Pipeline.IngestEndpointUrls across the stop/start cycle.
