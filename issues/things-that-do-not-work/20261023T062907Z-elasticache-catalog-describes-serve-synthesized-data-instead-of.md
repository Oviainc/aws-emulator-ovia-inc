Issue-Source: trace-sweep-divergence
Issue-Trace: elasticache-cache-parameter-group-workflow
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticache-cache-parameter-group-workflow elasticache-catalog-describe-cache-engine-versions elasticache-catalog-describe-cache-parameter-groups elasticache-catalog-describe-reserved-cache-nodes-offerings elasticache-catalog-describe-service-updates
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache|DescribeCacheEngineVersions|2xx->2xx|$.DescribeCacheEngineVersionsResponse.DescribeCacheEngineVersionsResult.CacheEngineVersions.CacheEngineVersion.CacheEngineDescription
Issue-Key: elasticache|DescribeCacheEngineVersions|2xx->2xx|$.DescribeCacheEngineVersionsResponse.DescribeCacheEngineVersionsResult.CacheEngineVersions.CacheEngineVersion.CacheEngineVersionDescription
Issue-Key: elasticache|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.CacheNodeTypeSpecificParameters
Issue-Key: elasticache|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Marker
Issue-Key: elasticache|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*]
Issue-Key: elasticache|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].AllowedValues
Issue-Key: elasticache|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].ChangeType
Issue-Key: elasticache|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].DataType
Issue-Key: elasticache|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].Description
Issue-Key: elasticache|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].IsModifiable
Issue-Key: elasticache|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].MinimumEngineVersion
Issue-Key: elasticache|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].ParameterName
Issue-Key: elasticache|DescribeEngineDefaultParameters|2xx->2xx|$.DescribeEngineDefaultParametersResponse.DescribeEngineDefaultParametersResult.EngineDefaults.Parameters.Parameter[*].ParameterValue
Issue-Key: elasticache|DescribeCacheEngineVersions|2xx->2xx|$.DescribeCacheEngineVersionsResponse.DescribeCacheEngineVersionsResult.CacheEngineVersions.CacheEngineVersion[*]
Issue-Key: elasticache|DescribeCacheParameterGroups|2xx->2xx|$.DescribeCacheParameterGroupsResponse.DescribeCacheParameterGroupsResult.CacheParameterGroups.CacheParameterGroup[*]
Issue-Key: elasticache|DescribeReservedCacheNodesOfferings|2xx->2xx|$.DescribeReservedCacheNodesOfferingsResponse.DescribeReservedCacheNodesOfferingsResult.ReservedCacheNodesOfferings.ReservedCacheNodesOffering[*]
Issue-Key: elasticache|DescribeServiceUpdates|2xx->2xx|$.DescribeServiceUpdatesResponse.DescribeServiceUpdatesResult.ServiceUpdates.ServiceUpdate[*]
Issue-Commit: c0919b0ae1fd753648687abf4b08014f282092d3

# ElastiCache catalog describes serve synthesized data instead of the service catalog

> _Note: this issue was identified against the code base at commit `c0919b0ae1fd753648687abf4b08014f282092d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCacheEngineVersions

## Description
Six divergences in the read-only catalog surface. These operations return service-static, account-independent data (engine versions, engine default parameters, default parameter groups, reserved-node offerings, service updates), and the emulator answers HTTP 200 with its own synthesized tables whose membership, ordering and field conventions differ from the service's. Each cause is separate and must be closed separately.

Cause 1 of 6: DescribeCacheEngineVersions engine descriptions use the wrong convention.
AWS: Smithy: DescribeCacheEngineVersions -> CacheEngineVersionMessage.CacheEngineVersions[].CacheEngineDescription / CacheEngineVersionDescription; the operation models no errors. Recorded behavior returns CacheEngineDescription 'Redis' for the redis family (and the matching version description).
Emulator: returns CacheEngineDescription 'Redis OSS' and CacheEngineVersionDescription 'Redis OSS version 7.1'.
Why: callers that select an engine by matching the catalog's description strings (and tests pinned to the service's naming) do not find the entry they expect.
Fix: the static engine-version catalog table behind the DescribeCacheEngineVersions handler.
Traces: elasticache-cache-parameter-group-workflow#task#2.

Cause 2 of 6: DescribeEngineDefaultParameters returns a different parameter set and omits the node-type-specific parameters.
AWS: Smithy: DescribeEngineDefaultParameters -> EngineDefaults{Parameters, CacheNodeTypeSpecificParameters, Marker}, paginated on EngineDefaults.Marker. Recorded behavior returns the family's full default parameter list plus a populated CacheNodeTypeSpecificParameters (CacheNodeTypeSpecificParameter[] each with CacheNodeTypeSpecificValues per node type) and a Marker.
Emulator: the EngineDefaults it returns diverges across nearly every parameter slot (ParameterName/ParameterValue/AllowedValues/DataType/Description/MinimumEngineVersion/IsModifiable/ChangeType at indices 0-19), omits Marker, and its CacheNodeTypeSpecificParameters has the wrong type/content.
Why: a client reading engine defaults to decide what it may modify sees parameters that do not exist, misses ones that do, and cannot page the list.
Fix: the engine-defaults catalog table and the EngineDefaults serializer/pagination in the DescribeEngineDefaultParameters handler.
Traces: elasticache-cache-parameter-group-workflow#task#3.

Cause 3 of 6: the DescribeCacheEngineVersions catalog itself has the wrong membership and order.
AWS: recorded behavior — the memcached/redis version list contains the service's set, e.g. memcached 1.4.14, 1.4.24, 1.4.33, 1.4.34 ... with CacheEngineDescription 'memcached' and CacheEngineVersionDescription 'memcached version <v>' (lowercase engine naming).
Emulator: the list is shifted and differently cased — it starts at memcached 1.4.5 and reports 'Memcached' / 'Memcached version 1.4.5', so every index of CacheEngineVersion[0..32] mismatches.
Why: version-pinning clients pick a version the service does not offer, or fail to find the one they require.
Fix: the static engine-version catalog table (same table as Cause 1).
Traces: elasticache-catalog-describe-cache-engine-versions#task#1.

Cause 4 of 6: DescribeCacheParameterGroups omits the cluster-mode default parameter groups.
AWS: Smithy: DescribeCacheParameterGroups -> CacheParameterGroupsMessage.CacheParameterGroups[]. Recorded behavior includes, for each redis family, both the plain default and the cluster-mode-on default — e.g. default.redis3.2 AND default.redis3.2.cluster.on with Description 'Customized default parameter group for redis3.2 with cluster mode on' and an ARN of the form arn:aws:elasticache:<region>:<account>:parametergroup:<name>.
Emulator: the list contains only the plain per-family defaults (default.redis4.0, default.redis6.x, default.valkey7, ...); the '.cluster.on' entries are absent, so indices 6, 8, 10, 12, 14, 16, 18, 19, 20 all mismatch.
Why: a client creating a cluster-mode-enabled replication group references default.redis<v>.cluster.on and gets a not-found for a group the service always provides.
Fix: the seeded default-parameter-group catalog in the elasticache state bootstrap.
Traces: elasticache-catalog-describe-cache-parameter-groups#task#1.

Cause 5 of 6: DescribeReservedCacheNodesOfferings offerings are generated, and OfferingType does not match the price fields.
AWS: recorded behavior — each offering pairs OfferingType with a consistent price shape: 'No Upfront' carries FixedPrice 0.0 with a RecurringCharges.RecurringCharge{RecurringChargeFrequency 'Hourly', RecurringChargeAmount <rate>}, 'All Upfront' carries a non-zero FixedPrice with a 0.0 recurring charge; offerings are returned in the service's order with its ReservedCacheNodesOfferingIds.
Emulator: offerings are minted with random ids and the pairing is inverted/empty — the first offering is 'All Upfront' with FixedPrice 1261.44 and RecurringCharges serialized as an empty string, the second is 'No Upfront' with FixedPrice 0 — so ReservedCacheNodesOffering[0..5] all mismatch. (Offering ids and prices themselves are environment-specific examples; the defect is the inconsistent OfferingType/price/RecurringCharges shape and the generated catalog.)
Why: a client that filters offerings by OfferingType and then reads the price fields computes a cost from a combination the service never returns, and RecurringCharges as an empty string is not a parseable RecurringCharges structure.
Fix: the reserved-node offering catalog table and the RecurringCharges serializer in the DescribeReservedCacheNodesOfferings handler.
Traces: elasticache-catalog-describe-reserved-cache-nodes-offerings#task#1.

Cause 6 of 6: DescribeServiceUpdates returns synthesized updates instead of the service's update catalog.
AWS: Smithy: DescribeServiceUpdates -> ServiceUpdatesMessage.ServiceUpdates[] with ServiceUpdateName/Type/Severity/Status/Engine/EngineVersion and the release/end/recommended-apply dates. Recorded behavior lists the service's published updates, e.g. ServiceUpdateName 'elasticache-redis-oss-v6-eol-replication-groups' with ServiceUpdateType 'engine-major-version-update', ServiceUpdateStatus 'available', Engine 'redis'.
Emulator: emits entries whose names are generated from the current clock ('elasticache-20260426-001') with ServiceUpdateStatus 'expired' and ServiceUpdateType 'security-update', mismatching all 73 entries. (Dates/ids vary per environment; the defect is that the catalog is generated rather than the service's fixed set, so no modeled update name is resolvable.)
Why: BatchApplyUpdateAction/BatchStopUpdateAction take a ServiceUpdateName — a client that discovers a name here cannot act on it, and a client that knows a real update name never finds it.
Fix: the service-update catalog table behind the DescribeServiceUpdates handler (shared with the update-action handlers).
Traces: elasticache-catalog-describe-service-updates#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1, all six clusters verdict=mismatch at HTTP 200, stub_path empty, owned_by empty:
1. elasticache-cache-parameter-group-workflow#task#2 DescribeCacheEngineVersions — CacheEngineDescription expected 'Redis', got 'Redis OSS'; CacheEngineVersionDescription got 'Redis OSS version 7.1'.
2. elasticache-cache-parameter-group-workflow#task#3 DescribeEngineDefaultParameters — EngineDefaults.CacheNodeTypeSpecificParameters type_mismatch (expected CacheNodeTypeSpecificParameter[] with CacheNodeTypeSpecificValues per cache.* node type); EngineDefaults.Marker missing; Parameters.Parameter[0..19] mismatch on ParameterName/ParameterValue/AllowedValues/DataType/Description/MinimumEngineVersion/IsModifiable/ChangeType.
3. elasticache-catalog-describe-cache-engine-versions#task#1 — CacheEngineVersion[0] expected {EngineVersion '1.4.14', CacheEngineDescription 'memcached', CacheEngineVersionDescription 'memcached version 1.4.14'}, got {EngineVersion '1.4.5', 'Memcached', 'Memcached version 1.4.5'}; indices 0-32 mismatch.
4. elasticache-catalog-describe-cache-parameter-groups#task#1 — CacheParameterGroup[6] expected default.redis3.2.cluster.on ('Customized default parameter group for redis3.2 with cluster mode on'), got default.redis4.0; same shift at indices 8,10,12,14,16,18,19,20.
5. elasticache-catalog-describe-reserved-cache-nodes-offerings#task#1 (cluster also covers task#2, task#3) — ReservedCacheNodesOffering[0] expected OfferingType 'No Upfront' with FixedPrice 0.0 and RecurringCharge{Hourly,0.141}, got OfferingType 'All Upfront', FixedPrice '1261.44', RecurringCharges ''.
6. elasticache-catalog-describe-service-updates#task#1 — ServiceUpdate[0] expected ServiceUpdateName 'elasticache-redis-oss-v6-eol-replication-groups' (type engine-major-version-update, status available, engine redis), got 'elasticache-20260426-001' (security-update, expired, memcached); 73 entries mismatch.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Cause 1/2: replay fixture elasticache-cache-parameter-group-workflow steps task#2 (DescribeCacheEngineVersions) and task#3 (DescribeEngineDefaultParameters for a redis family) and compare CacheEngine*Description and the EngineDefaults parameter set / CacheNodeTypeSpecificParameters / Marker.
3. Cause 3: replay elasticache-catalog-describe-cache-engine-versions#task#1 and compare the memcached version list and its description casing.
4. Cause 4: replay elasticache-catalog-describe-cache-parameter-groups#task#1 and check for default.redis<v>.cluster.on entries.
5. Cause 5: replay elasticache-catalog-describe-reserved-cache-nodes-offerings#task#1 and check OfferingType against FixedPrice/UsagePrice/RecurringCharges consistency.
6. Cause 6: replay elasticache-catalog-describe-service-updates#task#1 and check that update names are the service's published names, not clock-derived.
7. Confirm the fix by re-replaying every step named in Traces.
