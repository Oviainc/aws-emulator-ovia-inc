Issue-Source: trace-sweep-divergence
Issue-Trace: es-engine-version-and-instance-catalog
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: es-engine-version-and-instance-catalog esdef-list-instance-type-details-max-results-ignored opensearch-domain-version-capacity-and-software-update-management
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|ListInstanceTypeDetails|2xx->2xx|$.InstanceTypeDetails[*]
Issue-Commit: 67ed68e040d396203b765d4dc2b0964aebb19c3c

# es ListInstanceTypeDetails serves a synthesized c4-family instance-type catalog instead of the service's

> _Note: this issue was identified against the code base at commit `67ed68e040d396203b765d4dc2b0964aebb19c3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstanceTypeDetails

## Description
ONE root cause, reproduced by 3 divergence clusters. They are numbered below as Cause 1 of 3 .. Cause 3 of 3 so that every one of them is re-replayed before this issue is closed; all three are the same defect in the instance-type catalog ListInstanceTypeDetails reads from.

AWS: ListInstanceTypeDetails answers HTTP 200 with InstanceTypeDetails, the live catalog of instance types supported by the OpenSearch/Elasticsearch engine version named in the path. Smithy: com.amazonaws.opensearch#ListInstanceTypeDetails, GET /2021-01-01/opensearch/instanceTypeDetails/{EngineVersion} (EngineVersion @required, pattern ^Elasticsearch_[0-9]{1}\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\.[0-9]{1,2}$), output member InstanceTypeDetails -> InstanceTypeDetailsList of com.amazonaws.opensearch#InstanceTypeDetails{InstanceType (enum OpenSearchPartitionInstanceType), EncryptionEnabled, CognitoEnabled, AppLogsEnabled, AdvancedSecurityEnabled, WarmEnabled, InstanceRole (list), AvailabilityZones (list)}; MaxResults @range max 100, RetrieveAZs httpQuery retrieveAZs. The catalog CONTENT is not modelled, so for the content itself this is recorded behavior only: the recordings show the service returning the current generation families for the requested engine version - entries such as m7i.2xlarge.search, r7i.12xlarge.search, c8g.12xlarge.search and oi2.8xlarge.search, which do not even appear in the checked-in OpenSearchPartitionInstanceType enum - with InstanceRole carrying "coordinator" in addition to "data"/"master" for types that support dedicated coordinator nodes, and with AvailabilityZones absent/null when RetrieveAZs is not requested. The recorded list is also in the service's own order, so element N of the response is a specific instance type.

Emulator: it serves a catalog it synthesizes from the checked-in instance-type enum's older families - the list begins c4.2xlarge.search, c4.8xlarge.search, c4.large.search, c4.xlarge.search ... - with a constant InstanceRole of ["data", "master"] on every entry and never a "coordinator" role, and with the AvailabilityZones member omitted. Because the entries and their order both differ, essentially every index of InstanceTypeDetails mismatches: the representative cluster reports mismatches on ~190 of the ~231 recorded indices, starting at $.InstanceTypeDetails[0] (expected InstanceType m7i.2xlarge.search, got c4.2xlarge.search). The feature-flag booleans (EncryptionEnabled, CognitoEnabled, AppLogsEnabled, AdvancedSecurityEnabled, WarmEnabled) do match; the instance-type identities, their roles and the list membership/order do not.

Why: a client uses this operation to decide what it is allowed to put in ClusterConfig before calling CreateDomain/UpdateDomainConfig. Against the emulator every current-generation type (m7i/r7i/c8g/or1/oi2 ...) looks unsupported for the engine version while retired c4/i2/d2 types look supported, so capacity planners, console-style pickers and validation that pre-checks a requested InstanceType all reject valid configurations and accept invalid ones. Because InstanceRole never contains "coordinator", code that gates dedicated-coordinator-node provisioning on this catalog concludes the feature is unavailable on every instance type.

Fix: the data source behind the ListInstanceTypeDetails handler - the seeded instance-type catalog/state the handler projects (including each entry's InstanceRole set and the AvailabilityZones member, which must stay absent unless RetrieveAZs is requested) - not the response serializer, which already emits the right member names and the right booleans. The catalog must be keyed by the requested EngineVersion rather than derived from the modelled enum, since the recorded catalog contains families the enum does not list.

Traces (examples of the one defect):
Cause 1 of 3: fixture es-engine-version-and-instance-catalog, step task#3 - ListInstanceTypeDetails for an engine version; ~190 indices of InstanceTypeDetails diverge, $.InstanceTypeDetails[0] expected m7i.2xlarge.search got c4.2xlarge.search.
Cause 2 of 3: fixture esdef-list-instance-type-details-max-results-ignored, step task#1 - the single returned entry $.InstanceTypeDetails[0] is c4.2xlarge.search with InstanceRole [data, master] where the recording has m7i.2xlarge.search with InstanceRole [data, master, coordinator].
Cause 3 of 3: fixture opensearch-domain-version-capacity-and-software-update-management, step task#5 - same catalog served on the OpenSearch domain-capacity path; $.InstanceTypeDetails[0] expected m7i.2xlarge.search got c4.2xlarge.search, [2] expected c8g.12xlarge.search got c4.8xlarge.search, [3] expected r7i.12xlarge.search got c4.large.search.

## Evidence
All three clusters are status-clean (expected 200, got 200) and diverge only in the body.

Representative, es-engine-version-and-instance-catalog#task#3 (divergence_key service=es|ListInstanceTypeDetails|status=200->200|keys=$.InstanceTypeDetails[0],...,$.InstanceTypeDetails[9]):
  expected $.InstanceTypeDetails[0] = {'AdvancedSecurityEnabled': True, 'AppLogsEnabled': True, 'AvailabilityZones': None, 'CognitoEnabled': True, 'EncryptionEnabled': True, 'InstanceRole': ['data', 'master', 'coordinator'], 'InstanceType': 'm7i.2xlarge.search', 'WarmEnabled': True}
  actual   $.InstanceTypeDetails[0] = {'InstanceType': 'c4.2xlarge.search', 'EncryptionEnabled': True, 'CognitoEnabled': True, 'AppLogsEnabled': True, 'AdvancedSecurityEnabled': True, 'WarmEnabled': True, 'InstanceRole': ['data', 'master']}
  further indices: [2] expected c8g.12xlarge.search got c4.8xlarge.search; [3] expected oi2.8xlarge.search (InstanceRole ['data']) got c4.large.search (InstanceRole ['data','master']); [4] expected r7i.12xlarge.search got c4.xlarge.search. Mismatches continue through index ~230.

esdef-list-instance-type-details-max-results-ignored#task#1: $.InstanceTypeDetails[0] value_mismatch, expected m7i.2xlarge.search / InstanceRole ['data','master','coordinator'], got c4.2xlarge.search / InstanceRole ['data','master'].

opensearch-domain-version-capacity-and-software-update-management#task#5: $.InstanceTypeDetails[0] expected m7i.2xlarge.search got c4.2xlarge.search; [2] expected c8g.12xlarge.search got c4.8xlarge.search; [3] expected r7i.12xlarge.search got c4.large.search; [4] expected c8g.8xlarge.search got c4.xlarge.search.

Model cited: com.amazonaws.opensearch#InstanceTypeDetails members (InstanceType/EncryptionEnabled/CognitoEnabled/AppLogsEnabled/AdvancedSecurityEnabled/WarmEnabled/InstanceRole/AvailabilityZones) and com.amazonaws.opensearch#OpenSearchPartitionInstanceType, whose value list stops at t4g.* and contains none of m7i/r7i/c8g/oi2 - i.e. the catalog cannot be derived from the enum. Specific instance-type names, counts and the ~231-entry length quoted here are the recording's values, not the requirement; the requirement is that the served catalog is the service's catalog for the requested engine version, in its order, with the per-type InstanceRole set (coordinator included where supported).

## Steps to Reproduce
1. Replay fixture es-engine-version-and-instance-catalog, step task#3: GET /2021-01-01/opensearch/instanceTypeDetails/<EngineVersion> against the emulator.
2. Compare InstanceTypeDetails element-by-element with the recorded response: element 0 is c4.2xlarge.search instead of the recorded m7i.2xlarge.search, and no element carries "coordinator" in InstanceRole.
3. Replay esdef-list-instance-type-details-max-results-ignored#task#1 (single-entry page) and opensearch-domain-version-capacity-and-software-update-management#task#5 for the same divergence on the OpenSearch domain-capacity path.
4. Confirm the fix by re-replaying all three fixture steps listed in trace_ids - a catalog change that only re-orders the synthesized entries will still fail them.

## Resolution

Fixed: the data source. `ListInstanceTypeDetails` no longer derives its answer from the
modelled `OpenSearchPartitionInstanceType` value set. The service's own instance-type
catalog now lives in `services/service-com-amazonaws-opensearch-amazonopensearchservice/src/state.rs`
(`INSTANCE_TYPE_CATALOG`), keyed by the requested engine release: it carries the
current-generation families the modelled enum does not list (m7i, c8g, r7i, oi2 beside
or1), every value the enum does list, the engine releases that offer each family, and
the node roles an instance of each family serves - including the dedicated-coordinator
role for the families a coordinator node runs on and "data" alone for the
OpenSearch-optimized families. `ListInstanceTypeDetails`, `DescribeInstanceTypeLimits`
and the cluster-configuration validation of `CreateDomain`/`UpdateDomainConfig` all read
that one table, and `AvailabilityZones` is still reported only when `retrieveAZs` asks
for it. Held by `tests/all/instance_type_catalog.rs`.

Declined: the ORDER the list is published in, and with it the replay key
`es|ListInstanceTypeDetails|2xx->2xx|$.InstanceTypeDetails[*]`, which still reports a
difference. The order is not part of the modelled contract -
`com.amazonaws.opensearch#InstanceTypeDetailsList` is a bare list with no ordering trait
and no documented order - and the service's own internal ordering of its catalog is not
recoverable from anything it publishes: it is neither the modelled value set's order nor
a sort of any published attribute, and only a handful of its positions appear in any
evidence. Writing those recorded positions into the answer would be a memorised prefix
rather than a rule, so the catalog is published in the order AWS's supported-instance-
types documentation lists the families in (the current generations by family name, then
the warm tier, then the previous generations, each family's types smallest size first),
computed by `state::catalog_rank`. Anyone revisiting this cause needs the service's
published catalog order itself, not a rule that can be derived from the model.
