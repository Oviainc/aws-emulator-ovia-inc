Issue-Source: trace-sweep-divergence
Issue-Trace: appconfig-error-paths-not-found-and-invalid
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appconfig-error-paths-not-found-and-invalid appconfig-experiment-list-operations appconfig-experiment-run-full-lifecycle appconfig-experiment-run-lifecycle appconfig-experiment-run-multiple-runs appconfig-experiment-run-treatment-overrides appconfig-multiple-experiment-definitions-isolation
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.flags.sim2real-probe-appconfig-errflag._createdAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.flags.sim2real-probe-appconfig-errflag._updatedAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.values.sim2real-probe-appconfig-errflag._createdAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.values.sim2real-probe-appconfig-errflag._updatedAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.flags.myflag._createdAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.flags.myflag._updatedAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.values.myflag._createdAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.values.myflag._updatedAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.flags.my-feature-flag._createdAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.flags.my-feature-flag._updatedAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.values.my-feature-flag._createdAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.values.my-feature-flag._updatedAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.flags.sim2real-probe-appconfig-runflag._createdAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.flags.sim2real-probe-appconfig-runflag._updatedAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.values.sim2real-probe-appconfig-runflag._createdAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.values.sim2real-probe-appconfig-runflag._updatedAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.flags.rolloutflag._createdAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.flags.rolloutflag._updatedAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.values.rolloutflag._createdAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.values.rolloutflag._updatedAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.flags.sim2real-probe-appconfig-ovflag._createdAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.flags.sim2real-probe-appconfig-ovflag._updatedAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.values.sim2real-probe-appconfig-ovflag._createdAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.values.sim2real-probe-appconfig-ovflag._updatedAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.flags.sim2real-probe-appconfig-isoflag1._createdAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.flags.sim2real-probe-appconfig-isoflag1._updatedAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.flags.sim2real-probe-appconfig-isoflag2._createdAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.flags.sim2real-probe-appconfig-isoflag2._updatedAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.values.sim2real-probe-appconfig-isoflag1._createdAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.values.sim2real-probe-appconfig-isoflag1._updatedAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.values.sim2real-probe-appconfig-isoflag2._createdAt
Issue-Key: appconfig|CreateHostedConfigurationVersion|2xx->2xx|$.values.sim2real-probe-appconfig-isoflag2._updatedAt
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# AppConfig CreateHostedConfigurationVersion omits the feature-flag _createdAt and _updatedAt metadata

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateHostedConfigurationVersion

## Description
When a hosted configuration version is created on an AWS.AppConfig.FeatureFlags profile, AWS's feature-flags helper rewrites the stored document and stamps each flag and each value with _createdAt/_updatedAt; the emulator stores the submitted content verbatim, so those keys are absent from the returned document. Seven replayed clusters, one per fixture that writes a feature-flag version (different flag names, same fault). Numbered; close ALL SEVEN before submitting.

=== Cause 1 of 7 ===
AWS: recorded behavior only — CreateHostedConfigurationVersion is POST /applications/{ApplicationId}/configurationprofiles/{ConfigurationProfileId}/hostedconfigurationversions with HTTP 201 and the configuration document as the body (the model carries no schema for feature-flag content). For a profile of Type 'AWS.AppConfig.FeatureFlags', the returned document has both $.flags.<flagKey>._createdAt/_updatedAt and $.values.<flagKey>._createdAt/_updatedAt populated with ISO-8601 instants (recorded example '2026-09-07T05:48:55.763Z').
Emulator: 201 with the submitted content echoed unchanged — all four keys missing (None) for flag 'sim2real-probe-appconfig-errflag'.
Why: a client that reads flag metadata (or diffs two versions by _updatedAt) gets a missing key; the stored document is not the one AWS would serve to appconfigdata, so configuration consumers see a different payload.
Fix: the CreateHostedConfigurationVersion handler must branch on the owning configuration profile's Type and, for AWS.AppConfig.FeatureFlags, stamp _createdAt/_updatedAt on every entry of flags and values before storing/returning the version.
Traces: appconfig-error-paths-not-found-and-invalid#task#7.

=== Cause 2 of 7 ===
AWS: as cause 1, flag 'myflag'.
Emulator: $.flags.myflag._createdAt/_updatedAt and $.values.myflag._createdAt/_updatedAt all missing.
Why / Fix: as cause 1.
Traces: appconfig-experiment-list-operations#setup#4.

=== Cause 3 of 7 ===
AWS: as cause 1, flag 'my-feature-flag'.
Emulator: the four metadata keys missing.
Why / Fix: as cause 1.
Traces: appconfig-experiment-run-full-lifecycle#setup#4.

=== Cause 4 of 7 ===
AWS: as cause 1, flag 'sim2real-probe-appconfig-runflag'.
Emulator: the four metadata keys missing.
Why / Fix: as cause 1.
Traces: appconfig-experiment-run-lifecycle#task#4.

=== Cause 5 of 7 ===
AWS: as cause 1, flag 'rolloutflag'.
Emulator: the four metadata keys missing.
Why / Fix: as cause 1.
Traces: appconfig-experiment-run-multiple-runs#setup#4.

=== Cause 6 of 7 ===
AWS: as cause 1, flag 'sim2real-probe-appconfig-ovflag'.
Emulator: the four metadata keys missing.
Why / Fix: as cause 1.
Traces: appconfig-experiment-run-treatment-overrides#setup#4.

=== Cause 7 of 7: multi-flag document ===
AWS: as cause 1 with two flags in one document ('sim2real-probe-appconfig-isoflag1' and '...isoflag2') — EACH flag and EACH value entry is stamped, so eight keys are present.
Emulator: all eight missing, confirming the stamping is absent per-entry and not merely for the first flag.
Why / Fix: as cause 1 (iterate every entry of flags and values).
Traces: appconfig-multiple-experiment-definitions-isolation#setup#4.

## Evidence
Wire replay (simparity-wire-aws-v1), all appconfig CreateHostedConfigurationVersion, expected 201 / actual 201:

Cause 1 — appconfig-error-paths-not-found-and-invalid#task#7: $.flags.sim2real-probe-appconfig-errflag._createdAt / ._updatedAt and $.values.sim2real-probe-appconfig-errflag._createdAt / ._updatedAt missing (expected '2026-09-07T05:48:55.763Z', got None).
Cause 2 — appconfig-experiment-list-operations#setup#4: $.flags.myflag._createdAt/._updatedAt, $.values.myflag._createdAt/._updatedAt missing (expected <shaped-not-asserted>).
Cause 3 — appconfig-experiment-run-full-lifecycle#setup#4: same four keys missing for 'my-feature-flag'.
Cause 4 — appconfig-experiment-run-lifecycle#task#4: same four missing for 'sim2real-probe-appconfig-runflag'.
Cause 5 — appconfig-experiment-run-multiple-runs#setup#4: same four missing for 'rolloutflag'.
Cause 6 — appconfig-experiment-run-treatment-overrides#setup#4: same four missing for 'sim2real-probe-appconfig-ovflag' (expected '2026-09-07T05:48:52.896Z').
Cause 7 — appconfig-multiple-experiment-definitions-isolation#setup#4: eight keys missing — flags/values x isoflag1/isoflag2 x _createdAt/_updatedAt (expected '2026-09-07T05:48:54.981Z').

## Steps to Reproduce
Boot the emulator and point an appconfig client at it.
1. create_application(Name=...).
2. create_configuration_profile(ApplicationId=<id>, Name=..., LocationUri='hosted', Type='AWS.AppConfig.FeatureFlags').
3. create_hosted_configuration_version(ApplicationId=<id>, ConfigurationProfileId=<cp>, ContentType='application/json', Content=json({'flags': {'myflag': {'name': 'myflag'}}, 'values': {'myflag': {'enabled': True}}, 'version': '1'})).
4. Read the returned document: flags.myflag._createdAt/_updatedAt and values.myflag._createdAt/_updatedAt must be present (ISO-8601); the emulator returns the content unchanged with those keys absent. Repeat with two flags for cause 7.
Re-replay appconfig-error-paths-not-found-and-invalid#task#7 plus every step in trace_ids.
