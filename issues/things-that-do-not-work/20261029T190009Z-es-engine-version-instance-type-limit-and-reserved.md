Issue-Source: trace-sweep-divergence
Issue-Trace: es-engine-version-and-instance-catalog
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: es-engine-version-and-instance-catalog eserr-describe-instance-type-limits-t2-micro-invalid-type opensearch-describe-reserved-instance-offerings-paginated opensearch-list-and-paginate-reserved-instance-offerings opensearch-paginate-reserved-instance-offerings opensearch-domain-version-capacity-and-software-update-management
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|ListVersions|2xx->2xx|$.Versions[*]
Issue-Key: es|GetCompatibleVersions|2xx->2xx|$.CompatibleVersions[*]
Issue-Key: es|DescribeInstanceTypeLimits|2xx->2xx|$.LimitsByRole.data.StorageTypes[*]
Issue-Key: es|DescribeInstanceTypeLimits|2xx->2xx|$.LimitsByRole.data.StorageTypes[*].StorageTypeLimits[*].LimitName
Issue-Key: es|DescribeInstanceTypeLimits|2xx->2xx|$.LimitsByRole.data.StorageTypes[*].StorageTypeLimits[*].LimitValues[*]
Issue-Key: es|DescribeInstanceTypeLimits|2xx->2xx|$.LimitsByRole.master.StorageTypes
Issue-Key: es|DescribeReservedInstanceOfferings|2xx->2xx|$.ReservedInstanceOfferings[*]
Issue-Key: es|DescribeInstanceTypeLimits|2xx->2xx|$.LimitsByRole.data.InstanceLimits.InstanceCountLimits.MaximumInstanceCount
Issue-Related: 20261029T190009Z-es-pagination-contract-is-wrong-empty-nexttoken-ignored.md
Issue-Commit: 92c3b140342b1bc1fab2a232b8cee3f25ea78ff5

# es: engine-version, instance-type, limit and reserved-instance catalogs are synthetic

> _Note: this issue was identified against the code base at commit `92c3b140342b1bc1fab2a232b8cee3f25ea78ff5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListVersions

## Description
5 net-new root causes. These operations return a static reference catalog; all five answer 200 with a catalog that is not the one AWS serves. Recorded behavior only - the model constrains the shape, not the contents.

Cause 1 of 5 - ListVersions. AWS returns the supported engine versions newest-first, led by the OpenSearch line (OpenSearch_3.7, OpenSearch_3.5, OpenSearch_3.3, OpenSearch_3.1, OpenSearch_2.19 ... down to Elasticsearch_1.5). The emulator returns the list oldest-first and led by the Elasticsearch line (Elasticsearch_1.5, Elasticsearch_2.3, ...), and its set is shorter than AWS's. Traces: es-engine-version-and-instance-catalog#task#1, opensearch-domain-version-capacity-and-software-update-management#task#2.

Cause 2 of 5 - GetCompatibleVersions. AWS returns one entry per upgradable source version whose TargetVersions is the set of versions that source may upgrade to - for Elasticsearch_7.10 that is exactly [OpenSearch_1.0, OpenSearch_1.1, OpenSearch_1.2, OpenSearch_1.3], and the unfiltered matrix starts at Elasticsearch_7.7. The emulator starts the matrix at Elasticsearch_1.5, enumerates every later version as a target, and for the domain-scoped call adds the whole OpenSearch 2.x line to Elasticsearch_7.10's targets. Traces: es-engine-version-and-instance-catalog#task#2, opensearch-domain-version-capacity-and-software-update-management#task#3.

Cause 3 of 5 - ListInstanceTypeDetails. AWS's catalog is the current generation (m7i, c8g, oi2, r7i, r8g, i8g, or1/or2, om2, ...), each entry carries an AvailabilityZones member and the InstanceRole list includes 'coordinator' where the type supports it. The emulator serves a legacy c4/c5/m4-era catalog, omits AvailabilityZones and never reports the coordinator role. Traces: es-engine-version-and-instance-catalog#task#3, opensearch-domain-version-capacity-and-software-update-management#task#5.

Cause 4 of 5 - DescribeInstanceTypeLimits. AWS orders each StorageTypeLimits block maximum-before-minimum (MaximumVolumeSize then MinimumVolumeSize, MaximumIops then MinimumIops, MaximumThroughput then MinimumThroughput) and the values are the service's real caps; the emulator emits minimum-before-maximum and different values, and reports InstanceLimits.InstanceCountLimits.MaximumInstanceCount 80 where AWS reports 20. It also omits whole storage types AWS lists and the master-role StorageTypes block. Traces: es-engine-version-and-instance-catalog#task#4, opensearch-domain-version-capacity-and-software-update-management#task#4.

Cause 5 of 5 - DescribeReservedInstanceOfferings. AWS's offerings are the current ones (c7i, or1, r8g, ... with 94608000 and 31536000 durations) and every offering carries a RecurringCharges entry, including a zero-amount Hourly charge for ALL_UPFRONT; the emulator serves a different offering set (m4, d2, ...) and returns RecurringCharges: [] for ALL_UPFRONT offerings. Trace: opensearch-describe-reserved-instance-offerings-paginated#task#1.

Why: these catalogs are what a client uses to decide what it may ask for. A client that picks an instance type, an engine version, an upgrade target, a volume size or a reserved-instance offering from the emulator's answer picks something AWS will reject - and conversely refuses to offer the current generation that AWS does support. Cause 4's inverted limit names are worse than wrong values: a client reading 'MinimumVolumeSize' gets the maximum, so it validates volume sizes against the wrong bound.

Fix: the static reference data behind these four readers (engine-version table, upgrade-compatibility matrix, instance-type catalog with its AZ and role attributes, per-instance/per-storage-type limit table, reserved-instance offering table) and the DescribeInstanceTypeLimits serializer's limit ordering.

## Evidence
Cause 1: "$.Versions[0]: missing (expected 'OpenSearch_3.7', got 'Elasticsearch_1.5')" (and [1]..[3] likewise).
Cause 2: "$.CompatibleVersions[0]: missing (expected {'SourceVersion': 'Elasticsearch_7.7', 'TargetVersions': [...4 OpenSearch 1.x + 3 ES 7.x]}, got {'SourceVersion': 'Elasticsearch_1.5', 'TargetVersions': [18 entries]})"; domain-scoped: expected Elasticsearch_7.10 -> 4 targets, got 13.
Cause 3: "$.InstanceTypeDetails[0]: missing (expected {... 'AvailabilityZones': None, 'InstanceRole': ['data', 'master', 'coordinator'], 'InstanceType': 'm7i.2xlarge.search' ...}, got {'InstanceType': 'c4.2xlarge.search', ... 'InstanceRole': ['data', 'master']})".
Cause 4: "StorageTypeLimits[0].LimitName: value_mismatch (expected 'MaximumVolumeSize', got 'MinimumVolumeSize')", "StorageTypeLimits[0].LimitValues[0]: value_mismatch (expected '100', got '10')", "InstanceCountLimits.MaximumInstanceCount: value_mismatch (expected 20, got 80)".
Cause 5: "$.ReservedInstanceOfferings[0]: missing (expected {...'InstanceType': 'c7i.xlarge.search', 'PaymentOption': 'ALL_UPFRONT', 'RecurringCharges': [{'RecurringChargeAmount': 0.0, 'RecurringChargeFrequency': 'Hourly'}]...}, got {...'InstanceType': 'm4.4xlarge.search', 'RecurringCharges': []})".

## Steps to Reproduce
1. Replay es-engine-version-and-instance-catalog steps task#1 (ListVersions), task#2 (GetCompatibleVersions), task#3 (ListInstanceTypeDetails) and task#4 (DescribeInstanceTypeLimits) and diff each against the recorded catalog.
2. Replay opensearch-domain-version-capacity-and-software-update-management steps task#2 through task#5 for the domain-scoped variants.
3. Replay opensearch-describe-reserved-instance-offerings-paginated step task#1 for the offerings catalog and the empty RecurringCharges.
