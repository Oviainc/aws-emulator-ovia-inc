Issue-Source: trace-sweep-divergence
Issue-Trace: fis-wait-experiment-runs-to-completed
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: fis-wait-experiment-runs-to-completed
Issue-Shape-Id: com.amazonaws.fis#FaultInjectionSimulator
Issue-Key: fis|GetExperiment|2xx->2xx|$.experiment.actions.pause.endTime
Issue-Key: fis|GetExperiment|2xx->2xx|$.experiment.actions.pause.startTime
Issue-Key: fis|GetExperiment|2xx->2xx|$.experiment.actions.pause.state.reason
Issue-Key: fis|GetExperiment|2xx->2xx|$.experiment.creationTime
Issue-Key: fis|GetExperiment|2xx->2xx|$.experiment.endTime
Issue-Key: fis|GetExperiment|2xx->2xx|$.experiment.startTime
Issue-Key: fis|GetExperiment|2xx->2xx|$.experiment.state.reason
Issue-Key: fis|CreateExperimentTemplate|2xx->2xx|$.experimentTemplate.creationTime
Issue-Key: fis|CreateExperimentTemplate|2xx->2xx|$.experimentTemplate.lastUpdateTime
Issue-Key: fis|StartExperiment|2xx->2xx|$.experiment.creationTime
Issue-Key: fis|StartExperiment|2xx->2xx|$.experiment.startTime
Issue-Commit: 3b4b4e2df28997ae7cbb85154b0c83cf06a43ef8

# FIS experiment records diverge: run timestamps carry host sub-second precision and state reasons use the emulator's own wording

> _Note: this issue was identified against the code base at commit `3b4b4e2df28997ae7cbb85154b0c83cf06a43ef8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetExperiment

## Description
Every FIS record this emulator projects onto the wire carries timestamps stamped straight off the host clock, so each timestamp member is emitted with a sub-second fractional tail the recorded AWS responses do not have, and the replay reports a shape divergence on each one even though it never asserts the instant itself. The same experiment read also reports experiment and action state reasons in the emulator's own wording instead of AWS's fixed text. 3 causes — numbered 1, 2 and 3 below; close all three before submitting.

Cause 1 of 3: CreateExperimentTemplate reports experimentTemplate.creationTime and .lastUpdateTime in a shape AWS does not emit.
AWS: Smithy model services/service-com-amazonaws-fis-faultinjectionsimulator/smithy-model/com-amazonaws-fis-faultinjectionsimulator.json — CreateExperimentTemplate POST /experimentTemplates -> 200, output ExperimentTemplate.creationTime: CreationTime (timestamp) and .lastUpdateTime: LastUpdateTime (timestamp); the service protocol is restJson1 and neither member carries a @timestampFormat trait, so both are epoch-seconds numbers on the wire. The INSTANT is a per-run value and is not a requirement; what the recording fixes is the SHAPE of the value, which the harness asserts as a shaped (not literal) expectation. The recorded responses carry an epoch-seconds value for these members with no sub-second tail.
Emulator: it stamps the record with ::std::time::SystemTime::now() and projects it with ::aws_smithy_types::DateTime::from(...) unrounded (services/service-com-amazonaws-fis-faultinjectionsimulator/src/state.rs lines 1506-1527), so the body carries a fractional epoch-seconds number with seven fractional digits (the replay observed 1794227006.2411215 for both members, which is the host clock of that run and not a requirement). The replay reports verdict mismatch, expected_status 200, actual_status 200, value_mismatch at $.experimentTemplate.creationTime and $.experimentTemplate.lastUpdateTime against the recording's shaped expectation. Hypothesis: the shape the recording carries is an epoch-seconds timestamp at the granularity FIS reports, and the raw sub-second tail is what fails it.
Why: a client that round-trips or compares these timestamps — a golden/snapshot test, a cache key, a diff of the template record returned by create against the one GetExperimentTemplate later reports, or a consumer that assumes FIS's reported granularity — sees values of a kind the real service never returns, and any assertion written against AWS's shape fails against the emulator.
Fix: where the template record is stamped and projected onto the wire — the ExperimentTemplate projection in crate::state (state.rs around lines 1506-1527) and the create handler that supplies `now`; the stamp must be normalised to the granularity AWS reports instead of passing the raw host clock through.
Traces: fis-wait-experiment-runs-to-completed#task#2.

Cause 2 of 3: StartExperiment reports experiment.creationTime and .startTime the same way.
AWS: same model — StartExperiment POST /experiments -> 200, output Experiment.creationTime: CreationTime (timestamp) and .startTime: ExperimentStartTime (timestamp), restJson1, no @timestampFormat, so epoch-seconds numbers; the recording asserts their shape and not the instant.
Emulator: StartExperiment stamps the minted record with SystemTime::now() (services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/start_experiment.rs, `let now = ::std::time::SystemTime::now()`) and the Experiment projection emits it unrounded (state.rs lines 2118-2143), so both members carry the fractional epoch value (observed 1794227006.2567885). Replay verdict: mismatch, expected_status 200, actual_status 200, value_mismatch at $.experiment.creationTime and $.experiment.startTime.
Why: same consequence as Cause 1, on the record every experiment run begins from — and because the run's whole timeline is derived from this stamp, the fractional tail propagates into every later read of the experiment.
Fix: the Experiment projection in crate::state (state.rs around lines 2118-2143) and the stamp StartExperiment hands it.
Traces: fis-wait-experiment-runs-to-completed#task#3.

Cause 3 of 3: GetExperiment reports the SAME timestamp shape for the experiment and its actions, and additionally reports experiment and action state reasons in the emulator's own wording rather than AWS's fixed text. Two distinct defects in one response, both to be fixed.
AWS: same model — GetExperiment GET /experiments/{id} -> 200 (errors ResourceNotFoundException 404, ValidationException 400), output Experiment with creationTime, startTime, endTime, and actions: ExperimentActionMap of ExperimentAction{state: ExperimentActionState{status, reason}, startTime: ExperimentActionStartTime, endTime: ExperimentActionEndTime}; ExperimentState{status: ExperimentStatus enum (pending|initiating|running|completed|stopping|stopped|failed|cancelled), reason: ExperimentStatusReason (length 0..512)}. All five timestamps are restJson1 epoch-seconds numbers with no @timestampFormat, asserted by shape. The model documents ExperimentState$reason and ExperimentActionState$reason only as 'the reason for the state', so the TEXT is recorded behavior only: the recording shows that for a run that reached the terminal completed status AWS reports experiment state.reason 'Experiment completed.' and, for an action that ran to the end of its span, action state.reason 'Action was completed.' Both are FIXED strings for that status — they carry no value from the request or from state — so the whole string including its final period is the requirement.
Emulator: (a) the five timestamps are emitted with the raw sub-second tail exactly as in Causes 1 and 2 (observed $.experiment.creationTime/startTime 1794227006.2567885, $.experiment.endTime and $.experiment.actions.pause.endTime 1794227011.7567885, $.experiment.actions.pause.startTime 1794227006.7567885), all reported value_mismatch against the recording's shaped expectation; (b) it reports experiment state.reason 'Experiment completed' — the constant COMPLETED_REASON in services/service-com-amazonaws-fis-faultinjectionsimulator/src/state.rs line 609, whose own doc comment says the wording was chosen rather than reproduced — instead of 'Experiment completed.', and action state.reason 'The action was completed' instead of 'Action was completed.'
Why: for (a), as above. For (b), state.reason is the field a client reads and displays to explain why a run is in the status it is in; an operator console shows text AWS never emits, and any client, test or log assertion that matches the reason for a completed run or a completed action against AWS's fixed string fails against the emulator. Because the reason strings are service constants, every completed experiment and every completed action is reported wrongly, not just this fixture's.
Fix: (a) the Experiment and ExperimentAction projections in crate::state (state.rs around lines 2118-2143 and the action progress projection in crate::state::action_progress) plus the settle that stamps the action and run end instants; (b) the experiment/action state-reason constants in crate::state (COMPLETED_REASON at state.rs line 609 and the action-completed reason used by crate::state::action_progress), which must carry AWS's fixed text for those statuses.
Traces: fis-wait-experiment-runs-to-completed#task#4 (representative, this issue's replay key).

Relation to open issues: 20261108T211946Z-fis-2-net-new-root-causes-stop-condition.md reports that the STOP-CONDITION halt reason is the emulator's own wording; it is the same class of defect but a different status and a different constant, and it does not cover the completed-experiment or completed-action reasons, nor any timestamp. No other open FIS issue (20261024T000450Z, 20261029T183321Z, 20261104T032609Z, 20261106T163756Z, 20261107T200341Z) touches the experiment record's timestamps or its terminal reasons.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 285 steps, 279 passed, 6 mismatched, 6 divergence clusters, all service=fis. The three clusters below all report empty stub_path and empty owned_by.

Cause 1 — CreateExperimentTemplate, fixture fis-wait-experiment-runs-to-completed step task#2, verdict mismatch, expected_status 200, actual_status 200:
  $.experimentTemplate.creationTime: value_mismatch (expected <shaped-not-asserted>, got 1794227006.2411215)
  $.experimentTemplate.lastUpdateTime: value_mismatch (expected <shaped-not-asserted>, got 1794227006.2411215)

Cause 2 — StartExperiment, fixture fis-wait-experiment-runs-to-completed step task#3, verdict mismatch, expected_status 200, actual_status 200:
  $.experiment.creationTime: value_mismatch (expected <shaped-not-asserted>, got 1794227006.2567885)
  $.experiment.startTime: value_mismatch (expected <shaped-not-asserted>, got 1794227006.2567885)

Cause 3 — GetExperiment, fixture fis-wait-experiment-runs-to-completed step task#4, verdict mismatch, expected_status 200, actual_status 200:
  $.experiment.creationTime: value_mismatch (expected <shaped-not-asserted>, got 1794227006.2567885)
  $.experiment.startTime: value_mismatch (expected <shaped-not-asserted>, got 1794227006.2567885)
  $.experiment.endTime: value_mismatch (expected <shaped-not-asserted>, got 1794227011.7567885)
  $.experiment.actions.pause.startTime: value_mismatch (expected <shaped-not-asserted>, got 1794227006.7567885)
  $.experiment.actions.pause.endTime: value_mismatch (expected <shaped-not-asserted>, got 1794227011.7567885)
  $.experiment.state.reason: value_mismatch
    expected: 'Experiment completed.'   actual: 'Experiment completed'
  $.experiment.actions.pause.state.reason: value_mismatch
    expected: 'Action was completed.'   actual: 'The action was completed'
  (Both reasons are fixed text for their status — no part of either string comes from the request or from state — so the entire string, final period included, is the requirement. The epoch values quoted above are that run's host clock and are NOT requirements; the requirement is the shape of the timestamp AWS emits for these members.)

Model consulted (smithy_query query=closure/operation, services/service-com-amazonaws-fis-faultinjectionsimulator/smithy-model/com-amazonaws-fis-faultinjectionsimulator.json):
  GetExperiment GET /experiments/{id} -> 200; errors ResourceNotFoundException(404), ValidationException(400)
  Experiment.creationTime: CreationTime (timestamp), .startTime: ExperimentStartTime (timestamp), .endTime: ExperimentEndTime (timestamp) — no @timestampFormat, restJson1 default epoch-seconds
  ExperimentAction.startTime/.endTime: ExperimentActionStartTime/ExperimentActionEndTime (timestamp), .state: ExperimentActionState{status: ExperimentActionStatus enum, reason: ExperimentActionStatusReason len 0..512}
  ExperimentState{status: ExperimentStatus enum (pending|initiating|running|completed|stopping|stopped|failed|cancelled), reason: ExperimentStatusReason len 0..512}
  CreateExperimentTemplate output ExperimentTemplate.creationTime/.lastUpdateTime (timestamp)
The model documents no wording for either reason member, so the recorded responses are the authority for the two strings.

Emulator code consulted: services/service-com-amazonaws-fis-faultinjectionsimulator/src/state.rs lines 1506-1527 (ExperimentTemplate projection: DateTime::from(self.creation_time) / DateTime::from(self.last_update_time)), lines 2118-2143 (Experiment projection: creation_time / start_time / end_time), line 609 (pub const COMPLETED_REASON: &str = "Experiment completed", documented as chosen wording rather than AWS's), and services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/start_experiment.rs (`let now = ::std::time::SystemTime::now()`).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Cause 1: POST /experimentTemplates (CreateExperimentTemplate) with any valid template and read $.experimentTemplate.creationTime / .lastUpdateTime from the raw JSON body (not through the CLI, which reformats timestamps). Observe an epoch-seconds number carrying a seven-digit sub-second tail, e.g. 1794227006.2411215, instead of the timestamp shape AWS emits for these members.
3. Cause 2: POST /experiments (StartExperiment) for that template and read $.experiment.creationTime / .startTime from the raw body — same fractional shape.
4. Cause 3: wait for the run to reach terminal status, then GET /experiments/{id} (GetExperiment). In the raw body: (a) $.experiment.creationTime, .startTime, .endTime and $.experiment.actions.<name>.startTime/.endTime all carry the same fractional tail; (b) $.experiment.state.reason is 'Experiment completed' where AWS reports 'Experiment completed.', and $.experiment.actions.<name>.state.reason is 'The action was completed' where AWS reports 'Action was completed.'
5. Re-replay fixture fis-wait-experiment-runs-to-completed steps task#2, task#3 AND task#4 after the fix; all three must pass.
