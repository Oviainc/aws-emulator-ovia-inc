Issue-Source: trace-sweep-divergence
Issue-Trace: appconfig-experiment-run-conflict-and-notfound-errors
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appconfig-experiment-run-conflict-and-notfound-errors appconfig-experiment-run-multiple-runs appconfig-experiment-run-rerun-history appconfig-experiment-run-treatment-overrides appconfig-experiment-run-conflict-errors appconfig-experiment-run-full-lifecycle appconfig-experiment-run-lifecycle
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig|StartExperimentRun|2xx->2xx|$.ExperimentDefinitionSnapshot.Control.Key
Issue-Key: appconfig|StartExperimentRun|2xx->2xx|$.ExperimentDefinitionSnapshot.Treatments[*]
Issue-Key: appconfig|StartExperimentRun|2xx->2xx|$.ExposurePercentage
Issue-Key: appconfig|StartExperimentRun|2xx->2xx|$.StartedAt
Issue-Key: appconfig|StartExperimentRun|2xx->2xx|$.UpdatedAt
Issue-Key: appconfig|StopExperimentRun|2xx->2xx|$.EndedAt
Issue-Key: appconfig|StopExperimentRun|2xx->2xx|$.ExperimentDefinitionSnapshot.Control.Key
Issue-Key: appconfig|StopExperimentRun|2xx->2xx|$.ExperimentDefinitionSnapshot.Treatments[*]
Issue-Key: appconfig|StopExperimentRun|2xx->2xx|$.ExposurePercentage
Issue-Key: appconfig|StopExperimentRun|2xx->2xx|$.StartedAt
Issue-Key: appconfig|StopExperimentRun|2xx->2xx|$.UpdatedAt
Issue-Key: appconfig|GetExperimentRun|2xx->2xx|$.ExperimentDefinitionSnapshot.Control.Key
Issue-Key: appconfig|GetExperimentRun|2xx->2xx|$.ExperimentDefinitionSnapshot.Treatments[*]
Issue-Key: appconfig|GetExperimentRun|2xx->2xx|$.StartedAt
Issue-Key: appconfig|GetExperimentRun|2xx->2xx|$.UpdatedAt
Issue-Key: appconfig|UpdateExperimentRun|2xx->2xx|$.ExperimentDefinitionSnapshot.Control.Key
Issue-Key: appconfig|UpdateExperimentRun|2xx->2xx|$.ExperimentDefinitionSnapshot.Treatments[*]
Issue-Key: appconfig|UpdateExperimentRun|2xx->2xx|$.StartedAt
Issue-Key: appconfig|UpdateExperimentRun|2xx->2xx|$.UpdatedAt
Issue-Key: appconfig|GetExperimentRun|2xx->2xx|$.EndedAt
Issue-Key: appconfig|GetExperimentRun|2xx->2xx|$.ExposurePercentage
Issue-Commit: 473105e5105ba5cee28620541f40434a5fe4ef94

# AppConfig experiment run responses: snapshot treatment keys upper-cased, timestamps as epoch floats, ExposurePercentage omitted

> _Note: this issue was identified against the code base at commit `473105e5105ba5cee28620541f40434a5fe4ef94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExperimentRun

## Description
Every operation that returns the AppConfig ExperimentRun shape (StartExperimentRun, StopExperimentRun, GetExperimentRun, UpdateExperimentRun) diverges from AWS in three ways: the ExperimentDefinitionSnapshot carries upper-cased treatment keys, the four Iso8601DateTime members are serialized as bare epoch-seconds floats, and ExposurePercentage is omitted when the run's exposure is zero. 8 causes are bundled; they are the same three defects in the one shared ExperimentRun handler/serializer.

Shared AWS behavior:
- Snapshot treatment keys. Smithy: ExperimentRun.ExperimentDefinitionSnapshot targets ExperimentDefinitionSnapshot, whose Treatments target TreatmentList of Treatment and whose Control targets Treatment; Treatment.Key targets TreatmentKey (@pattern ^[a-zA-Z0-9]{1,8}$). The snapshot preserves the keys the service generated for the definition - recorded behavior: "c" for the control and "t<N>" for the Nth treatment, lower-case.
- Timestamps. Smithy: ExperimentRun.StartedAt, UpdatedAt and EndedAt all target Iso8601DateTime with @timestampFormat("date-time"), so each is an RFC-3339/ISO-8601 string ending in 'Z', never a number. Recorded behavior: StartedAt is recorded at nanosecond precision and UpdatedAt/EndedAt at millisecond precision; both forms are strings.
- ExposurePercentage. Smithy: ExperimentRun.ExposurePercentage targets NullablePercentage (float, @range min 0.0 max 100.0). Recorded behavior: a run started WITHOUT an ExposurePercentage in the request comes back with ExposurePercentage present and equal to 0.0 - AWS materialises the zero exposure rather than leaving the member out - and the same 0.0 is still present after the run is stopped and on subsequent reads.

Shared Emulator behavior: the snapshot's Control.Key is "C" and its Treatments[N].Key are "T1"/"T2"; StartedAt/UpdatedAt/EndedAt are JSON floating-point epoch seconds; and ExposurePercentage is absent from the body when it was not supplied on the request (the member is dropped instead of being emitted as 0.0).

Shared Why: a client reading a run cannot act on it. The snapshot's treatment keys are the identifiers used to express TreatmentOverrides and to interpret which variant a user was served, so a client that echoes the keys AWS gave it ('c'/'t1') does not match the emulator's state. An SDK decoding StartedAt/UpdatedAt/EndedAt per the modeled date-time format either throws or yields a wrong instant, so run duration and ordering cannot be computed. And a missing ExposurePercentage reads as "unknown exposure" rather than "0% exposed": code that branches on exposure, or that reads-modifies-writes the run, sees an absent member where AWS guarantees a number in [0,100].

Shared Fix: the ExperimentRun handler and serializer in the appconfig service - carry the definition's lower-case generated keys into the snapshot unchanged, render every Iso8601DateTime member as a date-time string, and emit ExposurePercentage (defaulting to 0.0 for a run started with no exposure) on Start/Stop/Get/Update.

Cause 1 of 8: StartExperimentRun - fixture appconfig-experiment-run-conflict-and-notfound-errors, step task#10. 201/201; snapshot Control.Key 'C' for 'c' and Treatments[0].Key 'T1' for 't1'; StartedAt/UpdatedAt epoch floats; $.ExposurePercentage missing where 0.0 is expected. Cluster covers 6 steps across 4 fixtures.

Cause 2 of 8: StopExperimentRun - fixture appconfig-experiment-run-conflict-and-notfound-errors, step task#13. 200/200; same three defects plus EndedAt as an epoch float - the zero exposure is still dropped on the stop response. Cluster covers 6 steps across 4 fixtures.

Cause 3 of 8: StartExperimentRun with an explicit exposure - fixture appconfig-experiment-run-conflict-errors, step task#8. 201/201; here ExposurePercentage IS present (it was supplied), so only the upper-cased snapshot keys and the epoch-float StartedAt/UpdatedAt diverge - which isolates the ExposurePercentage defect to the zero/default case.

Cause 4 of 8: StopExperimentRun of that run - fixture appconfig-experiment-run-conflict-errors, step task#11. 200/200; upper-cased snapshot keys and epoch-float StartedAt/UpdatedAt/EndedAt.

Cause 5 of 8: GetExperimentRun on a RUNNING run - fixture appconfig-experiment-run-full-lifecycle, step task#9. 200/200; upper-cased snapshot keys and epoch-float StartedAt/UpdatedAt on the read path.

Cause 6 of 8: UpdateExperimentRun - fixture appconfig-experiment-run-full-lifecycle, step task#10. 200/200; same defects after an exposure update, confirming the snapshot keys are not re-normalised on update.

Cause 7 of 8: GetExperimentRun on a stopped (DONE) run - fixture appconfig-experiment-run-full-lifecycle, step task#16. 200/200; EndedAt is also an epoch float, so the defect covers all four timestamp members of the shape.

Cause 8 of 8: GetExperimentRun with TreatmentOverrides in play - fixture appconfig-experiment-run-treatment-overrides, step task#9. 200/200; all three defects together (upper-cased snapshot keys, epoch-float StartedAt/UpdatedAt, ExposurePercentage missing where 0.0 is expected) on a run whose overrides are keyed by treatment key - the case where the key-case defect is directly client-visible.

Related open issue (does not cover these steps): "Implement appconfig StopExperimentRun: unimplemented scaffold stub" - StopExperimentRun now answers 200/409 rather than a stub status, so these response-fidelity divergences are net-new and are not closed by implementing the stub.

## Evidence
Cause 1 (appconfig-experiment-run-conflict-and-notfound-errors#task#10, StartExperimentRun, expected 201 / actual 201):
  $.ExperimentDefinitionSnapshot.Control.Key: expected 'c', got 'C'
  $.ExperimentDefinitionSnapshot.Treatments[0]: expected {'FlagValue': {'Enabled': True}, 'Key': 't1', 'Weight': 50.0}, got {'Key': 'T1', 'Weight': 50.0, 'FlagValue': {'Enabled': True}}
  $.ExposurePercentage: missing (expected 0.0, got None)
  $.StartedAt / $.UpdatedAt: expected ISO-8601 date-time strings (recorded '<date>T<time>.<frac>Z'), got the number 1793300168.3459668

Cause 2 (appconfig-experiment-run-conflict-and-notfound-errors#task#13, StopExperimentRun, 200/200):
  $.EndedAt, $.StartedAt, $.UpdatedAt: ISO-8601 strings expected, epoch floats returned
  $.ExposurePercentage: missing (expected 0.0)
  $.ExperimentDefinitionSnapshot.Control.Key / Treatments[0].Key: 'C'/'T1' for 'c'/'t1'

Cause 3 (appconfig-experiment-run-conflict-errors#task#8, StartExperimentRun, 201/201): ExposurePercentage NOT reported missing (it was supplied on the request); only key case + timestamp encoding diverge -> the exposure defect is specific to the defaulted-zero case.

Cause 7 (appconfig-experiment-run-full-lifecycle#task#16, GetExperimentRun, 200/200): $.EndedAt also an epoch float -> all four Iso8601DateTime members of ExperimentRun are affected.

All recorded instants and the 17-digit floats are per-run values; the requirements are the STRING date-time encoding, the lower-case generated keys, and the PRESENCE of ExposurePercentage == 0.0.

Smithy support: ExperimentRun.StartedAt/UpdatedAt/EndedAt target Iso8601DateTime @timestampFormat("date-time"); ExperimentRun.ExposurePercentage targets NullablePercentage (float, @range 0.0-100.0); ExperimentDefinitionSnapshot.Treatments -> TreatmentList of Treatment, Treatment.Key -> TreatmentKey @pattern ^[a-zA-Z0-9]{1,8}$; TreatmentInput (the write shape) has no Key member, so keys are service-generated.

## Steps to Reproduce
1. Replay fixture appconfig-experiment-run-conflict-and-notfound-errors and inspect step task#10 (Cause 1).
2. Create an application/environment/feature-flag profile and a CreateExperimentDefinition with one control and one treatment (no Keys - the service assigns them).
3. Call StartExperimentRun WITHOUT ExposurePercentage. In the 201 body: AWS returns ExposurePercentage == 0.0, StartedAt/UpdatedAt as ISO-8601 strings, and ExperimentDefinitionSnapshot keys 'c'/'t1'. The emulator omits ExposurePercentage, returns epoch-second numbers, and returns 'C'/'T1'.
4. Call GetExperimentRun and UpdateExperimentRun (appconfig-experiment-run-full-lifecycle#task#9 and #task#10): the same three divergences persist.
5. Call StopExperimentRun and re-read the run (appconfig-experiment-run-full-lifecycle#task#16): EndedAt is also a number.
6. Repeat step 3 WITH an explicit ExposurePercentage (appconfig-experiment-run-conflict-errors#task#8): ExposurePercentage is now present, which isolates the defaulted-zero case.
7. Re-replay all 8 bundled fixture#step bindings listed under Traces to confirm the fix.
