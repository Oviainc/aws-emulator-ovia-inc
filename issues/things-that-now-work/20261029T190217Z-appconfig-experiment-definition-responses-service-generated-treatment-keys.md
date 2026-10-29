Issue-Source: trace-sweep-divergence
Issue-Trace: appconfig-error-paths-not-found-and-invalid
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appconfig-error-paths-not-found-and-invalid appconfig-experiment-definition-archive-then-destroy appconfig-experiment-definition-crud appconfig-experiment-definition-error-paths appconfig-experiment-definition-lifecycle appconfig-experiment-definition-list-filters appconfig-experiment-definition-multi-treatment appconfig-experiment-definitions-list-filtering appconfig-experiment-list-operations appconfig-experiment-run-full-lifecycle appconfig-experiment-run-lifecycle appconfig-experiment-run-treatment-overrides appconfig-multiple-experiment-definitions-isolation appconfig-experiment-definition-treatment-payload-round-trip
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig|CreateExperimentDefinition|2xx->2xx|$.Control.Key
Issue-Key: appconfig|CreateExperimentDefinition|2xx->2xx|$.CreatedAt
Issue-Key: appconfig|CreateExperimentDefinition|2xx->2xx|$.Treatments[*]
Issue-Key: appconfig|CreateExperimentDefinition|2xx->2xx|$.UpdatedAt
Issue-Key: appconfig|GetExperimentDefinition|2xx->2xx|$.Control.Key
Issue-Key: appconfig|GetExperimentDefinition|2xx->2xx|$.CreatedAt
Issue-Key: appconfig|GetExperimentDefinition|2xx->2xx|$.Treatments[Key="t1"]
Issue-Key: appconfig|GetExperimentDefinition|2xx->2xx|$.UpdatedAt
Issue-Key: appconfig|UpdateExperimentDefinition|2xx->2xx|$.Control.Key
Issue-Key: appconfig|UpdateExperimentDefinition|2xx->2xx|$.CreatedAt
Issue-Key: appconfig|UpdateExperimentDefinition|2xx->2xx|$.Treatments[*]
Issue-Key: appconfig|UpdateExperimentDefinition|2xx->2xx|$.UpdatedAt
Issue-Key: appconfig|GetExperimentDefinition|2xx->2xx|$.Treatments[Key="t2"]
Issue-Key: appconfig|GetExperimentDefinition|2xx->2xx|$.Treatments[*]
Issue-Key: appconfig|GetExperimentDefinition|2xx->2xx|$.Treatments[Key="T1"]
Issue-Commit: 473105e5105ba5cee28620541f40434a5fe4ef94

# AppConfig experiment definition responses: service-generated treatment keys upper-cased and timestamps serialized as epoch floats

> _Note: this issue was identified against the code base at commit `473105e5105ba5cee28620541f40434a5fe4ef94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExperimentDefinition

## Description
Every write and read of the AppConfig ExperimentDefinition shape diverges from AWS in the same two ways: the treatment keys the SERVICE generates are emitted upper-cased, and the two Iso8601DateTime members are emitted as bare JSON epoch-seconds floats instead of ISO-8601 date-time strings. 8 causes (one per diverging fixture step cluster) are bundled here because they are the same two defects in the one shared ExperimentDefinition serializer/handler.

Shared AWS behavior for all 8 causes:
- Treatment keys are server-assigned. Smithy: the input member CreateExperimentDefinition.Treatments targets TreatmentInputList of TreatmentInput, and TreatmentInput has ONLY Weight/Description/FlagValue - there is no Key member, and CreateExperimentDefinition.Control targets the same TreatmentInput. The OUTPUT shape Treatment does carry Key (target TreatmentKey, @pattern ^[a-zA-Z0-9]{1,8}$). Recorded behavior: AWS assigns the control treatment the key "c", and the Nth treatment in request order the key "t<N>" - "t1", "t2", ... - i.e. a lower-case letter plus, for treatments, the 1-based request index. The key is assigned once at create time and is then stable for the definition's lifetime (the same keys come back from Get/Update/List and from the run snapshot).
- Timestamps are ISO-8601 strings. Smithy: ExperimentDefinition.CreatedAt and ExperimentDefinition.UpdatedAt both target Iso8601DateTime with @timestampFormat("date-time"), so they serialize on the wire as an RFC-3339/ISO-8601 string with a trailing Z, never as a number. Recorded responses show the full-resource form carrying millisecond precision ("<date>T<time>.<millis>Z").

Shared Emulator behavior: the emulator upper-cases the generated keys - "C" for the control treatment and "T1"/"T2" for the treatments - and serializes CreatedAt/UpdatedAt as a JSON floating-point epoch-seconds number.

Shared Why: a client cannot address a treatment. Treatment keys are the handle used to target a treatment in TreatmentOverrides, in audience rules and in downstream flag evaluation; a client that reads "c"/"t1" from AWS and sends it back gets a not-found/no-match against the emulator, because the emulator only knows "C"/"T1". The key is also constrained by @pattern on write paths elsewhere in the model, so a round-trip of an emulator-produced key is not guaranteed to validate. Independently, an SDK that decodes CreatedAt/UpdatedAt per the model's date-time format either fails to parse the number or yields a nonsense instant, so no client can order, diff or display experiment-definition timestamps.

Shared Fix: the experiment-definition key-generation in the appconfig service state (generate "c" and "t<N>" lower-case at CreateExperimentDefinition time and preserve them across Update/Get/List/snapshot), and the ExperimentDefinition serializer for the CreatedAt/UpdatedAt members (render Iso8601DateTime as a date-time string for every operation that returns the shape).

Cause 1 of 8: CreateExperimentDefinition - fixture appconfig-error-paths-not-found-and-invalid, step task#8. 201/201, but $.Control.Key expected 'c' got 'C', $.Treatments[0].Key expected 't1' got 'T1', and $.CreatedAt/$.UpdatedAt returned as an epoch float where an ISO-8601 string is modeled. Cluster covers 16 steps across 11 fixtures.

Cause 2 of 8: GetExperimentDefinition - fixture appconfig-experiment-definition-archive-then-destroy, step task#7. 200/200; the treatment the fixture looks up by key ($.Treatments[Key="t1"]) is absent entirely because the emulator stored it as "T1"; Control.Key 'C' for 'c'; CreatedAt/UpdatedAt epoch floats.

Cause 3 of 8: UpdateExperimentDefinition - fixture appconfig-experiment-definition-crud, step task#7. 200/200; same upper-cased Control/treatment keys and epoch-float CreatedAt/UpdatedAt. Confirms the key is not re-normalised on update either.

Cause 4 of 8: UpdateExperimentDefinition with several treatments - fixture appconfig-experiment-definition-multi-treatment, step task#6. 200/200; BOTH generated treatment keys are upper-cased ('t1'->'T1', 't2'->'T2'), showing the defect is in the index-based generator and not a single-treatment special case.

Cause 5 of 8: GetExperimentDefinition with several treatments - fixture appconfig-experiment-definition-multi-treatment, step task#7. 200/200; key-addressed lookups for both 't1' and 't2' miss.

Cause 6 of 8: CreateExperimentDefinition with a rich flag payload - fixture appconfig-experiment-definition-treatment-payload-round-trip, step task#5. 201/201; the FlagValue AttributeValues round-trip correctly, so the ONLY divergences are the upper-cased keys and the epoch-float CreatedAt/UpdatedAt - i.e. these two defects are independent of the treatment payload.

Cause 7 of 8: GetExperimentDefinition of that same definition - fixture appconfig-experiment-definition-treatment-payload-round-trip, step task#6. 200/200; same two defects on the read path.

Cause 8 of 8: GetExperimentDefinition with several definitions in the account - fixture appconfig-multiple-experiment-definitions-isolation, step task#8. 200/200; the report shows the key divergence from both sides at once - $.Treatments[Key="t1"] missing AND $.Treatments[Key="T1"] extra - which pins the cause to key CASE and not to a lost treatment.

Related open issues (neither covers these steps): "AppConfig GetConfiguration and GetExperimentDefinition answer 501, and CloudFormation-deployed AppConfig resources are not real" reports GetExperimentDefinition answering 501; it now answers 200, so these response-fidelity divergences are net-new. "AppConfig accepts three classes of invalid configuration/experiment input that AWS refuses" is about input validation, not generated keys or timestamp format.

## Evidence
Cause 1 (appconfig-error-paths-not-found-and-invalid#task#8, CreateExperimentDefinition, expected 201 / actual 201):
  $.Control.Key: expected 'c', got 'C'
  $.Treatments[0]: expected {'FlagValue': {'Enabled': True}, 'Key': 't1', 'Weight': 100.0}, got {'Key': 'T1', 'Weight': 100.0, 'FlagValue': {'Enabled': True}}
  $.CreatedAt / $.UpdatedAt: expected an ISO-8601 date-time string (recorded form '<date>T<time>.<millis>Z'), got the number 1793300161.5220118
  (the recorded instants and the 17-digit float are per-run values; the requirement is the STRING date-time encoding, not any particular instant)

Cause 2 (appconfig-experiment-definition-archive-then-destroy#task#7, GetExperimentDefinition, 200/200):
  $.Treatments[Key="t1"]: missing (expected {'FlagValue': {'Enabled': True}, 'Key': 't1', 'Weight': 50.0}, got None)
  $.Control.Key: expected 'c', got 'C'
  $.CreatedAt/$.UpdatedAt: ISO-8601 string expected, epoch float returned

Cause 4 (appconfig-experiment-definition-multi-treatment#task#6, UpdateExperimentDefinition, 200/200):
  $.Treatments[0]: expected Key 't1', got 'T1'; $.Treatments[1]: expected Key 't2', got 'T2'

Cause 8 (appconfig-multiple-experiment-definitions-isolation#task#8, GetExperimentDefinition, 200/200):
  $.Treatments[Key="t1"]: missing;  $.Treatments[Key="T1"]: extra  -> same treatment, wrong key case

Cause 6 (appconfig-experiment-definition-treatment-payload-round-trip#task#5, CreateExperimentDefinition, 201/201):
  FlagValue.AttributeValues (StringValue/NumberValue/BooleanValue/StringArray/NumberArray, incl. non-ASCII) all match; only Key case and the CreatedAt/UpdatedAt encoding diverge.

Smithy support: TreatmentInput members = Weight, Description, FlagValue (NO Key) -> keys are service-generated; Treatment.Key target TreatmentKey @pattern ^[a-zA-Z0-9]{1,8}$; ExperimentDefinition.CreatedAt/UpdatedAt target Iso8601DateTime @timestampFormat("date-time").

## Steps to Reproduce
1. Replay fixture appconfig-error-paths-not-found-and-invalid and inspect step task#8 (Cause 1).
2. CreateApplication, CreateEnvironment and a feature-flag ConfigurationProfile, then CreateExperimentDefinition with one Control (no Key) and one Treatments entry (no Key).
3. Read the 201 body: AWS returns Control.Key == "c" and Treatments[0].Key == "t1"; the emulator returns "C" and "T1".
4. Read CreatedAt/UpdatedAt in the same body: AWS returns an ISO-8601 date-time string ending in 'Z'; the emulator returns a JSON number (epoch seconds).
5. Call GetExperimentDefinition and UpdateExperimentDefinition on it - the upper-cased keys and the numeric timestamps persist (fixtures appconfig-experiment-definition-crud#task#7, appconfig-experiment-definition-archive-then-destroy#task#7).
6. For the multi-treatment variant, replay appconfig-experiment-definition-multi-treatment steps task#6 and task#7: both 't1' and 't2' come back upper-cased.
7. Re-replay all 8 bundled fixture#step bindings listed under Traces to confirm the fix.
