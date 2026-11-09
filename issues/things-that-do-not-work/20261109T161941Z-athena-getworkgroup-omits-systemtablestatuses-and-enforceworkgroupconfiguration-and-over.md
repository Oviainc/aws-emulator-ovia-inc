Issue-Source: trace-sweep-divergence
Issue-Trace: athena-createworkgroup-required-and-optional-members
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: athena-createworkgroup-required-and-optional-members
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|GetWorkGroup|2xx->2xx|$.WorkGroup.Configuration.EnforceWorkGroupConfiguration
Issue-Key: athena|GetWorkGroup|2xx->2xx|$.WorkGroup.Configuration.SystemTableStatuses
Issue-Key: athena|GetWorkGroup|2xx->2xx|$.WorkGroup.CreationTime
Issue-Related: 20261104T063259Z-athena-workgroup-and-data-catalog-reads-omit-modeled.md
Issue-Commit: d38d8ab45cd7905c8c6149b13ae22553e33f698d

# Athena GetWorkGroup omits SystemTableStatuses and EnforceWorkGroupConfiguration and over-precises CreationTime (3 causes)

> _Note: this issue was identified against the code base at commit `d38d8ab45cd7905c8c6149b13ae22553e33f698d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWorkGroup

## Description
A workgroup read back after CreateWorkGroup is serialized from a lossy workgroup record: members AWS always serves in WorkGroup.Configuration are absent, and the WorkGroup.CreationTime timestamp is emitted at a precision AWS never uses. This affects every CreateWorkGroup shape exercised by the fixture - a workgroup created with required members only, and one created with an explicit configuration - so it is the GetWorkGroup/WorkGroup serializer and the CreateWorkGroup configuration defaults, not one request's values. 3 causes.

NOTE ON RELATED OPEN ISSUE: the open issue 20261104T063259Z-athena-workgroup-and-data-catalog-reads-omit-modeled.md already owns the omission of Configuration.EnableMinimumEncryptionConfiguration, Configuration.RequesterPaysEnabled, Configuration.PublishCloudWatchMetricsEnabled, Configuration.EngineVersion.Category and Configuration.QuerySchedulingType, and those members are ALSO absent on these two steps. It does NOT cover the three causes below (SystemTableStatuses, EnforceWorkGroupConfiguration, CreationTime precision), which is why these clusters replay unowned. Do not close this issue by fixing only the members that issue lists.

Cause 1 of 3: GetWorkGroup never serves Configuration.SystemTableStatuses.
AWS: recorded behavior only - the on-disk Smithy model (com.amazonaws.athena#WorkGroupConfiguration) does not declare a SystemTableStatuses member, so the model says nothing about it. The recorded AWS response for a newly created workgroup returns Configuration.SystemTableStatuses as a one-element list of {TableName, Enabled} entries: the QUERY_EXECUTIONS system table reported with Enabled false when the workgroup was created without enabling it. AWS returns it on both workgroup shapes recorded (required-members-only and explicit-configuration).
Emulator: the member is absent from the Configuration object entirely (expected a one-entry list for QUERY_EXECUTIONS with Enabled false, got None).
Why: a client reading effective workgroup settings cannot discover whether the workgroup's system tables are enabled; code that branches on the presence of that list treats the feature as unavailable rather than disabled, and a round-trip read of a workgroup is not equal to what AWS reports.
Fix: the athena crate's CreateWorkGroup configuration defaults (materialize the per-system-table status with its default disabled state) and the GetWorkGroup WorkGroupConfiguration serializer.
Traces: athena-createworkgroup-required-and-optional-members#task#2 and #task#4 (same omission on both workgroup shapes).

Cause 2 of 3: GetWorkGroup omits Configuration.EnforceWorkGroupConfiguration on a workgroup created with required members only.
AWS: Smithy: WorkGroupConfiguration.EnforceWorkGroupConfiguration targets BoxedBoolean and is documented as the workgroup-settings override ('If set to "true", the settings for the workgroup override client-side settings'). AWS serves its effective value on every GetWorkGroup, defaulting to true when CreateWorkGroup did not set it; the recorded response returns EnforceWorkGroupConfiguration true for a workgroup created without the member.
Emulator: the member is absent (expected true, got None).
Why: this flag decides whether a client's own ResultConfiguration/encryption settings are honoured or overridden by the workgroup. A client that reads it back sees unset where AWS states true, so it cannot tell whether its per-query overrides will be ignored, and enforcement-sensitive logic takes the wrong branch.
Fix: the athena crate's CreateWorkGroup configuration defaults (default the override flag rather than leaving it unset) and the GetWorkGroup serializer.
Traces: athena-createworkgroup-required-and-optional-members#task#2.

Cause 3 of 3: WorkGroup.CreationTime is serialized with sub-millisecond precision.
AWS: Smithy: WorkGroup.CreationTime targets com.amazonaws.athena#Date (timestamp) and Athena uses awsJson1_1, so the wire form is an epoch-seconds number. Recorded Athena timestamps carry at most millisecond resolution - i.e. at most three fractional digits on the epoch-seconds number. The fixture asserts the shape of the value, not a particular instant: the instant itself is a per-run value and is not a requirement.
Emulator: emits an epoch-seconds float with seven fractional digits (microsecond-or-finer precision), which fails the asserted timestamp shape.
Why: a client that formats or compares the creation time, or that round-trips the value through a type with millisecond resolution, gets a value AWS would never produce; SDK and tooling that assume Athena's millisecond-resolution timestamps can mis-render or reject it.
Fix: the athena crate's timestamp serialization for WorkGroup.CreationTime (truncate/round epoch-seconds to millisecond resolution in the GetWorkGroup/CreateWorkGroup response serializer), not the stored clock value.
Traces: athena-createworkgroup-required-and-optional-members#task#2 and #task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout. Both clusters are GetWorkGroup, expected status 200, actual status 200 - the divergence is entirely in the response body.

Cluster 1 - athena-createworkgroup-required-and-optional-members#task#2 (workgroup created with required members only), divergence key service=athena|GetWorkGroup|status=200->200:
  $.WorkGroup.Configuration.SystemTableStatuses: missing (expected a one-entry list for the QUERY_EXECUTIONS system table with Enabled false, got None)
  $.WorkGroup.Configuration.EnforceWorkGroupConfiguration: missing (expected true, got None)
  $.WorkGroup.CreationTime: value_mismatch (expected the asserted epoch-seconds timestamp shape, got a float with seven fractional digits)
  Also missing on this step, already owned by 20261104T063259Z-athena-workgroup-and-data-catalog-reads-omit-modeled.md: Configuration.EnableMinimumEncryptionConfiguration (expected false), Configuration.PublishCloudWatchMetricsEnabled (expected true), Configuration.RequesterPaysEnabled (expected false), Configuration.EngineVersion.Category (expected 'Presto'), Configuration.QuerySchedulingType (expected 'DEFAULT').

Cluster 2 - athena-createworkgroup-required-and-optional-members#task#4 (representative; also #task#6, workgroup created with an explicit configuration), divergence key service=athena|GetWorkGroup|status=200->200:
  $.WorkGroup.Configuration.SystemTableStatuses: missing (expected the same one-entry QUERY_EXECUTIONS list with Enabled false, got None)
  $.WorkGroup.CreationTime: value_mismatch (expected the asserted epoch-seconds timestamp shape, got a float with seven fractional digits)
  Also missing on these steps and already owned by the issue above: Configuration.EnableMinimumEncryptionConfiguration, Configuration.RequesterPaysEnabled, Configuration.EngineVersion.Category, Configuration.QuerySchedulingType.

Model basis: smithy_query on services/service-com-amazonaws-athena-amazonathena/smithy-model/com-amazonaws-athena-amazonathena.json - WorkGroupConfiguration declares EnforceWorkGroupConfiguration (BoxedBoolean) but declares no SystemTableStatuses member, and WorkGroup.CreationTime targets Date (timestamp).

## Steps to Reproduce
1. git clone the repository and boot the emulator built from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Inspect fixture athena-createworkgroup-required-and-optional-members, steps task#2 (representative), task#4 and task#6.
4. Minimal manual repro: call CreateWorkGroup with only the required Name, then GetWorkGroup on that name - the response's WorkGroup.Configuration must contain EnforceWorkGroupConfiguration (true by default) and a SystemTableStatuses list carrying the QUERY_EXECUTIONS entry with Enabled false, and WorkGroup.CreationTime must be an epoch-seconds number with at most millisecond resolution. Repeat with CreateWorkGroup supplying an explicit Configuration (ResultConfiguration plus the optional flags) and GetWorkGroup again: SystemTableStatuses must still be present and CreationTime must still be millisecond-resolution.
