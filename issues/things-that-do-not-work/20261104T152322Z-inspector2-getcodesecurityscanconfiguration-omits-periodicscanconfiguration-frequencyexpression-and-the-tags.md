Issue-Source: trace-sweep-divergence
Issue-Trace: inspector2-code-security-scan-configuration-lifecycle
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: inspector2-code-security-scan-configuration-lifecycle
Issue-Shape-Id: com.amazonaws.inspector2#Inspector2
Issue-Key: inspector2|GetCodeSecurityScanConfiguration|2xx->2xx|$.configuration.periodicScanConfiguration.frequencyExpression
Issue-Key: inspector2|GetCodeSecurityScanConfiguration|2xx->2xx|$.tags
Issue-Commit: e6e32aafbb62326e8cdd2e3915a7b6a44f1d783b

# inspector2 GetCodeSecurityScanConfiguration omits periodicScanConfiguration.frequencyExpression and the tags map

> _Note: this issue was identified against the code base at commit `e6e32aafbb62326e8cdd2e3915a7b6a44f1d783b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCodeSecurityScanConfiguration

## Description
Cause 1 of 1: GetCodeSecurityScanConfiguration answers 200 but its response body drops two modeled output members that AWS returns — the periodic schedule expression that was supplied at create time, and the resource's tag map.

AWS: Smithy GetCodeSecurityScanConfiguration (POST /codesecurity/scan-configuration/get, http code 200) output GetCodeSecurityScanConfigurationResponse binds scanConfigurationArn, name, configuration (CodeSecurityScanConfiguration), level, scopeSettings, createdAt, lastUpdatedAt and tags (target TagMap, a map of MapKey length 1..128 to MapValue length ..256). CodeSecurityScanConfiguration.periodicScanConfiguration targets PeriodicScanConfiguration, whose members are frequency (PeriodicScanFrequency enum) and frequencyExpression (FrequencyExpression, @length 1..256, documented as "The schedule expression for periodic scans, in cron format"); the model's frequency doc states that when frequencyExpression IS provided the schedule must match the frequency, i.e. the expression is a caller-supplied part of the configuration that the service stores. The read therefore echoes the stored configuration as it was created: a scan configuration created with a periodic frequencyExpression reads back with that same expression present under configuration.periodicScanConfiguration.frequencyExpression. For tags the recorded response carries the member present as an empty map ({}) for an untagged configuration — the member is serialized rather than elided.

Emulator: replaying the create-then-get lifecycle, the 200 body is missing both keys. The replay reports `$.configuration.periodicScanConfiguration.frequencyExpression: missing (expected '<shaped-not-asserted>', got None)` and `$.tags: missing (expected {}, got None)`. So periodicScanConfiguration is emitted but its frequencyExpression is not (the value supplied on CreateCodeSecurityScanConfiguration is either not persisted or not serialized), and tags is omitted entirely instead of being present as an empty map.

Why: a client that creates a configuration with a cron schedule and then reads it back cannot see the schedule it set — round-tripping the read into an update silently clears the expression, and any code that renders or diffs the schedule sees it as unset. Generated SDKs model tags as a map and idiomatic callers index or iterate it directly; an absent member deserializes to null/None rather than an empty map, so tag-reading code that is correct against AWS raises or branches to a "no such field" path for an untagged configuration. Status and error code are not at issue here (200 both ways); the defect is response shape fidelity on a successful read.

Fix: the GetCodeSecurityScanConfiguration read path for the code-security scan configuration resource — (a) its state/create path must persist periodicScanConfiguration.frequencyExpression from CreateCodeSecurityScanConfiguration (and UpdateCodeSecurityScanConfiguration) and the read must project it back into the nested PeriodicScanConfiguration structure, and (b) its serializer must always emit the tags member for the resource, as an empty map when the configuration carries no tags, rather than skipping an empty/None map. No open issue covers this operation: the nearest open inspector2 issue, "inspector2 filter surface: UpdateFilter mis-classifies a malformed filterArn as not-found, and ListFilters omits the required empty tags map" (20261029T171819Z-inspector2-filter-surface-updatefilter-mis-classifies-a-malformed.md), fixes the same empty-tags-map omission only for the ListFilters/filter surface and does not touch the code-security scan configuration read, so the omission must be fixed here as well.

Traces: inspector2-code-security-scan-configuration-lifecycle#task#2 (GetCodeSecurityScanConfiguration, 200 vs 200, keys $.configuration.periodicScanConfiguration.frequencyExpression and $.tags).

## Evidence
Fixture inspector2-code-security-scan-configuration-lifecycle, step task#2, operation GetCodeSecurityScanConfiguration.
Expected status 200; actual status 200.
Divergent keys (from replay detail):
  $.configuration.periodicScanConfiguration.frequencyExpression: missing (expected '<shaped-not-asserted>', got None)
  $.tags: missing (expected {}, got None)
Expected: the 200 body contains configuration.periodicScanConfiguration.frequencyExpression holding the cron schedule expression supplied when the configuration was created (FrequencyExpression, @length 1..256), and contains the tags member serialized as a map — {} when the configuration has no tags.
Actual: both JSON pointers are absent from the emulator's 200 body; frequencyExpression is not round-tripped out of the created configuration and tags is elided instead of emitted as an empty map.
Model citations: GetCodeSecurityScanConfiguration output members configuration/tags; CodeSecurityScanConfiguration.periodicScanConfiguration -> PeriodicScanConfiguration{frequency, frequencyExpression}; tags -> TagMap.
Divergence key: service=inspector2|GetCodeSecurityScanConfiguration|status=200->200|keys=$.configuration.periodicScanConfiguration.frequencyExpression,$.tags (affected_count 1).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire fixture corpus (trace set simparity-wire-aws-v1).
2. Replay fixture inspector2-code-security-scan-configuration-lifecycle: step task#1 creates a code-security scan configuration via CreateCodeSecurityScanConfiguration with a periodicScanConfiguration that carries both frequency and a cron frequencyExpression, and no tags.
3. Replay step task#2: call GetCodeSecurityScanConfiguration with the scanConfigurationArn returned by the create.
4. Inspect the 200 response body: configuration.periodicScanConfiguration contains no frequencyExpression key, and there is no top-level tags key.
5. Expected: frequencyExpression is present with the expression the create supplied, and tags is present as {}.
