Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotdeviceadvisor.get-suite-definition-by-version-returns-original-config iotdeviceadvisor.update-suite-definition-advances-version
Issue-Shape-Id: com.amazonaws.iotdeviceadvisor#IotSenateService
Issue-Key: iotdeviceadvisor.get-suite-definition-by-version-returns-original-config|run#14|AssertionError|assert historical["suiteDefinitionConfiguration"]["suiteDefinitionName"] == new_name, (
Issue-Key: iotdeviceadvisor.update-suite-definition-advances-version|run#20|AssertionError|assert latest["createdAt"] > created_at, (
Issue-Commit: ead37dd00922f5e8ae35cfd4d29cc6e26f23d78c

# iotdeviceadvisor suite-definition versioning: GetSuiteDefinition serves per-version config snapshots and UpdateSuiteDefinition never advances createdAt (2 causes)

> _Note: this issue was identified against the code base at commit `ead37dd00922f5e8ae35cfd4d29cc6e26f23d78c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSuiteDefinition

## Description
Two distinct root causes in the iotdeviceadvisor suite-definition version/timestamp model. Both are in the one live suite-definition record kept by services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/src/state.rs and the two handlers that read/write it.

Cause 1 of 2: GetSuiteDefinition with an explicit suiteDefinitionVersion serves a historical snapshot of the configuration instead of the current live configuration.
Failing check: iotdeviceadvisor.get-suite-definition-by-version-returns-original-config
AWS: Smithy (com.amazonaws.iotdeviceadvisor#GetSuiteDefinition) models GET /suiteDefinitions/{suiteDefinitionId} with an optional httpQuery member suiteDefinitionVersion (SuiteDefinitionVersion, @length 2..255) and output members suiteDefinitionVersion, latestVersion and suiteDefinitionConfiguration. The model says nothing about per-version configuration snapshots — recorded behavior only: Device Advisor keeps exactly ONE live configuration per suiteDefinitionId. Requesting an older version number still succeeds (HTTP 200) and echoes the requested version string back in suiteDefinitionVersion, but the suiteDefinitionConfiguration served is the CURRENT (post-update) content, and latestVersion is always the newest version.
Emulator: after CreateSuiteDefinition (v1) and UpdateSuiteDefinition (v2, which changes suiteDefinitionName), GetSuiteDefinition(suiteDefinitionId, suiteDefinitionVersion=v1) returns the pre-update configuration — the check observed the name 'sim2real-behav-suite-<id>' where AWS serves the current 'sim2real-behav-suite-<id>-v2'. The handler asks the record for the configuration recorded with that specific version (per-version map lookup) rather than the live one.
Why: a client that reads a suite definition by version number — a very common pattern when the version it last saw is cached, or when it round-trips the version from a Create/Update response — is handed stale configuration content on the emulator and current content on AWS. Logic that compares the config it reads back against what it just wrote, or that renders a suite's devices/rootGroup/name from a version-qualified read, silently diverges, and an emulator-based test of 'update then read my version' passes where AWS would show the new content.
Fix: GetSuiteDefinition's handler and the suite-definition record in the service state: the record should hold one live configuration and a version pointer set, not one configuration per version; a version-qualified read should validate/echo the version and serve the live configuration.
Traces: iotdeviceadvisor.get-suite-definition-by-version-returns-original-config

Cause 2 of 2: UpdateSuiteDefinition does not advance createdAt on the updated record.
Failing check: iotdeviceadvisor.update-suite-definition-advances-version
AWS: Smithy (com.amazonaws.iotdeviceadvisor#UpdateSuiteDefinition, PATCH /suiteDefinitions/{suiteDefinitionId}) outputs createdAt and lastUpdatedAt; GetSuiteDefinition outputs createdAt and lastModifiedAt (both Timestamp). The model documents createdAt only as 'Date (in Unix epoch time) when the suite definition was created' — recorded behavior only for the update semantics: real Device Advisor REWRITES createdAt on every UpdateSuiteDefinition, so for the one live record it behaves as a last-write timestamp, and a GetSuiteDefinition after an update shows a createdAt strictly greater than the one observed before the update (as does lastModifiedAt).
Emulator: createdAt is pinned to the original creation instant and an update never moves it. The check read identical values before and after the update (the same timestamp to the microsecond), so the assertion that createdAt advances failed. The handler/state treat createdAt as immutable (it is read straight off the stored record while only the last-modified field is stamped with the update instant).
Why: a client that uses createdAt from a version-qualified or unqualified read as the record's write stamp — ordering suite definitions by last write, cache invalidation keyed on createdAt, or 'has this suite changed since I last looked' comparisons — sees no change on the emulator where AWS shows a newer timestamp, so change detection built against AWS reports 'unchanged' against the emulator.
Fix: UpdateSuiteDefinition's handler / the suite-definition record's version-recording path in the service state: stamp BOTH createdAt and lastModifiedAt with the update instant for the one live record.
Traces: iotdeviceadvisor.update-suite-definition-advances-version

## Evidence
Cause 1 — check iotdeviceadvisor.get-suite-definition-by-version-returns-original-config (mismatch):
  Expected (AWS): GetSuiteDefinition(suiteDefinitionId, suiteDefinitionVersion=v1) -> HTTP 200, suiteDefinitionVersion == v1, latestVersion == v2, suiteDefinitionConfiguration.suiteDefinitionName == the CURRENT post-update name.
  Actual (emulator): AssertionError: expected the v1 read to serve the CURRENT config name 'sim2real-behav-suite-b2ee8778072d-v2' (no per-version snapshot), got 'sim2real-behav-suite-b2ee8778072d'  (names are per-run examples; the requirement is 'current config, not a snapshot').
  Code path: services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/src/ops/get_suite_definition.rs:172-184 resolves the configuration via definition.configuration(input.suite_definition_version) — a per-version lookup — and the record stores bytes per version (see the module doc of ops/update_suite_definition.rs:5-11, 'The versions already stored keep their bytes').

Cause 2 — check iotdeviceadvisor.update-suite-definition-advances-version (mismatch):
  Expected (AWS): after UpdateSuiteDefinition, GetSuiteDefinition.createdAt > the createdAt read before the update (and lastModifiedAt likewise).
  Actual (emulator): AssertionError: expected createdAt to advance past datetime.datetime(2026, 10, 22, 22, 41, 35, 170311, tzinfo=tzlocal()) after update, got datetime.datetime(2026, 10, 22, 22, 41, 35, 170311, tzinfo=tzlocal()) — byte-identical, i.e. never rewritten (the instant itself is a per-run example).
  Code path: ops/update_suite_definition.rs:215 definition.record_version(config, now) then :236 created_at: Some(stored.created_at.into()); the module doc at :9-10 states 'createdAt is the definition's own creation instant (an update never moves it)'.

The version-advance and name/latestVersion assertions in both checks passed, so the divergence is isolated to (1) which configuration a version-qualified read serves and (2) the createdAt write on update.

## Steps to Reproduce
Cause 1:
1. CreateSuiteDefinition with suiteDefinitionConfiguration {suiteDefinitionName: N, rootGroup: "", devicePermissionRoleArn: <any role ARN>, intendedForQualification: true}; keep suiteDefinitionId.
2. v1 = GetSuiteDefinition(suiteDefinitionId).suiteDefinitionVersion.
3. UpdateSuiteDefinition(suiteDefinitionId, same config but suiteDefinitionName = N + "-v2"); v2 = response.suiteDefinitionVersion (v2 != v1).
4. GetSuiteDefinition(suiteDefinitionId, suiteDefinitionVersion=v1).
   AWS: suiteDefinitionVersion == v1, latestVersion == v2, suiteDefinitionConfiguration.suiteDefinitionName == N + "-v2".
   Emulator: suiteDefinitionConfiguration.suiteDefinitionName == N (the pre-update snapshot).

Cause 2:
1. CreateSuiteDefinition as above; read before = GetSuiteDefinition(suiteDefinitionId) and keep before.createdAt and before.lastModifiedAt.
2. UpdateSuiteDefinition(suiteDefinitionId, config with a changed suiteDefinitionName).
3. latest = GetSuiteDefinition(suiteDefinitionId).
   AWS: latest.createdAt > before.createdAt and latest.lastModifiedAt > before.lastModifiedAt.
   Emulator: latest.createdAt == before.createdAt (unchanged).

Replay: run the behavioral suite and re-run checks iotdeviceadvisor.get-suite-definition-by-version-returns-original-config and iotdeviceadvisor.update-suite-definition-advances-version.
