Issue-Source: trace-sweep-divergence
Issue-Trace: elasticache-catalog-describe-cache-parameters
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticache-catalog-describe-cache-parameters
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache|DescribeCacheParameters|2xx->2xx|$.DescribeCacheParametersResponse.DescribeCacheParametersResult.CacheNodeTypeSpecificParameters.CacheNodeTypeSpecificParameter[*]
Issue-Key: elasticache|DescribeCacheParameters|2xx->2xx|$.DescribeCacheParametersResponse.DescribeCacheParametersResult.Parameters.Parameter[*]
Issue-Key: elasticache|DescribeCacheParameters|2xx->2xx|$.DescribeCacheParametersResponse.DescribeCacheParametersResult.CacheNodeTypeSpecificParameters.CacheNodeTypeSpecificParameter
Issue-Key: elasticache|DescribeCacheParameters|2xx->4xx|$.DescribeCacheParametersResponse
Issue-Key: elasticache|DescribeCacheParameters|2xx->4xx|status
Issue-Commit: 232d6cb39af83c8f3e5d0960e0729eaeb270e0f7

# ElastiCache parameter catalog is incomplete: DescribeCacheParameters omits CacheNodeTypeSpecificParameters and returns the wrong Parameters set per family

> _Note: this issue was identified against the code base at commit `232d6cb39af83c8f3e5d0960e0729eaeb270e0f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCacheParameters

## Description
TEN net-new root causes, all in the elasticache crate, all in DescribeCacheParameters, one per cache-parameter-group family the corpus walks. In every one the operation answers 200 but the body is not the catalog AWS serves: the CacheNodeTypeSpecificParameters node is not the member-wrapped, populated list AWS returns, and the Parameters list is a shorter, differently-valued set than the family's real default parameter set. The tenth family is not known to the emulator at all and the call 404s. The one family-independent fault they share is the emulator's seeded parameter catalog (plus how the CacheNodeTypeSpecificParameters wrapper is serialised when it is empty), so a fixer must close all ten before submitting. None of the four open elasticache issues (20261023T155010Z-elasticache-control-plane-is-unusable-end-to-end.md, 20261026T234107Z-elasticache-serverless-usage-limit-refusal-still-answers-notimplemented.md, 20261029T174147Z-elasticache-createserverlesscache-with-an-out-of-range-cacheusagelimits-bound.md, 20261029T174147Z-elasticache-error-messages-diverge-from-aws-across-six.md) names the parameter catalog or carries any of the replay keys below.

Cause 1 of 10: memcached1.4 family (fixture elasticache-catalog-describe-cache-parameters, step task#1; the same cluster covers task#2 and task#3).
AWS: Smithy: DescribeCacheParameters -> CacheParameterGroupDetails {Marker:String, Parameters:ParametersList, CacheNodeTypeSpecificParameters:CacheNodeTypeSpecificParametersList}; errors CacheParameterGroupNotFoundFault (404/CacheParameterGroupNotFound), InvalidParameterValueException (400/InvalidParameterValue). The catalog CONTENT is recorded behavior only: for a default group of this family AWS returns the family's full default parameter set in Parameters, and a non-empty CacheNodeTypeSpecificParameters list whose members each carry ParameterName, MinimumEngineVersion, AllowedValues and a CacheNodeTypeSpecificValues list with one CacheNodeType/Value pair per supported node type (the recording shows entries such as max_cache_memory and the connection/memory limits, with AllowedValues '1-100000' and per-node-type values).
Emulator: 200, but CacheNodeTypeSpecificParameters[0] and [1] are absent (the replay reports them 'missing ... got None') and Parameters[0..19] differ or are absent, so the whole family entry is a stub rather than the family's catalog.
Why: a client that reads AllowedValues/MinimumEngineVersion before calling ModifyCacheParameterGroup, or that sizes a cluster from the node-type-specific values, gets no data at all for the node-type-specific parameters and wrong metadata for the rest; parameter-tuning tooling cannot run against the emulator.
Fix: the elasticache crate's seeded parameter catalog (the data the DescribeCacheParameters handler reads from crate::state) plus that handler's serialisation of the CacheNodeTypeSpecificParameters wrapper.
Traces: elasticache-catalog-describe-cache-parameters#task#1 (cluster also covers task#2, task#3).

Cause 2 of 10: redis2.6 family (MinimumEngineVersion 2.6.13 in the recording), step task#4.
AWS: as cause 1; recorded behavior only for content — CacheNodeTypeSpecificParameters is a LIST of members (the recording's first member has AllowedValues '0-', MinimumEngineVersion '2.6.13' and per-node-type values for cache.m4/m5/m6g/m7g/r4... node types), and Parameters holds this family's full default set.
Emulator: 200 with a type_mismatch at CacheNodeTypeSpecificParameters.CacheNodeTypeSpecificParameter (the node is not the list AWS returns) and 29 Parameters entries diverging.
Why: same as cause 1, for every caller that targets this family.
Fix: same component as cause 1.
Traces: elasticache-catalog-describe-cache-parameters#task#4.

Cause 3 of 10: redis2.8 family (MinimumEngineVersion 2.8.6), step task#5.
AWS: as cause 2, with this family's default parameter set and node-type-specific list.
Emulator: 200, CacheNodeTypeSpecificParameter type_mismatch and 35 Parameters entries diverging.
Why: same as cause 1.
Fix: same component as cause 1.
Traces: elasticache-catalog-describe-cache-parameters#task#5.

Cause 4 of 10: redis3.2 family (MinimumEngineVersion 3.2.4), step task#6 (cluster also covers task#7).
AWS: as cause 2.
Emulator: 200, CacheNodeTypeSpecificParameter type_mismatch and 38 Parameters entries diverging.
Why: same as cause 1.
Fix: same component as cause 1.
Traces: elasticache-catalog-describe-cache-parameters#task#6 (cluster also covers task#7).

Cause 5 of 10: redis4.0 family (MinimumEngineVersion 4.0.9), step task#8 (cluster also covers task#9).
AWS: as cause 2.
Emulator: 200, CacheNodeTypeSpecificParameter type_mismatch and 52 Parameters entries diverging.
Why: same as cause 1.
Fix: same component as cause 1.
Traces: elasticache-catalog-describe-cache-parameters#task#8 (cluster also covers task#9).

Cause 6 of 10: redis5.0 family (MinimumEngineVersion 5.0.0), step task#10 (cluster also covers task#11).
AWS: as cause 2.
Emulator: 200, CacheNodeTypeSpecificParameter type_mismatch and 57 Parameters entries diverging.
Why: same as cause 1.
Fix: same component as cause 1.
Traces: elasticache-catalog-describe-cache-parameters#task#10 (cluster also covers task#11).

Cause 7 of 10: redis6.x family (MinimumEngineVersion 6.0.5), step task#12 (cluster also covers task#13).
AWS: as cause 2; the recording's node-type-specific values for this family also cover the cache.c7gn and cache.caspianr1g node types.
Emulator: 200, CacheNodeTypeSpecificParameter type_mismatch and 62 Parameters entries diverging.
Why: same as cause 1.
Fix: same component as cause 1.
Traces: elasticache-catalog-describe-cache-parameters#task#12 (cluster also covers task#13).

Cause 8 of 10: redis7 family (MinimumEngineVersion 7.0.0), step task#14 (cluster also covers task#15, task#16, task#17).
AWS: as cause 7.
Emulator: 200, CacheNodeTypeSpecificParameter type_mismatch and 64 Parameters entries diverging.
Why: same as cause 1.
Fix: same component as cause 1.
Traces: elasticache-catalog-describe-cache-parameters#task#14 (cluster also covers task#15, task#16, task#17).

Cause 9 of 10: valkey8 family (MinimumEngineVersion 8.0.0), step task#18 (cluster also covers task#19).
AWS: as cause 7; the recording's node-type-specific values for this family additionally cover cache.c8gn node types.
Emulator: 200, CacheNodeTypeSpecificParameter type_mismatch and 67 Parameters entries diverging.
Why: same as cause 1.
Fix: same component as cause 1.
Traces: elasticache-catalog-describe-cache-parameters#task#18 (cluster also covers task#19).

Cause 10 of 10: valkey9 family (MinimumEngineVersion 9.0.0) is unknown to the emulator, so the call fails instead of returning the catalog. Step task#20 (cluster also covers task#21).
AWS: DescribeCacheParameters on this family's default group answers 200 with the DescribeCacheParametersResponse body (Parameters + a populated CacheNodeTypeSpecificParameters list), per the recording.
Emulator: HTTP 404 and no DescribeCacheParametersResponse at all — the default parameter group for the newest family is not seeded, so the handler takes its CacheParameterGroupNotFound path.
Why: this is a status-class divergence, not just a content one: a client enumerating families from DescribeCacheEngineVersions and then describing each one gets a hard 404 for the current engine generation, which looks like a missing resource rather than an emulator gap. The missing default.valkey9 / default.valkey9.cluster.on groups themselves are reported as a separate cause in the companion issue on the reference catalogs.
Fix: seed the valkey9 family (its default groups and its parameter set) in the same elasticache catalog the other nine causes come from.
Traces: elasticache-catalog-describe-cache-parameters#task#20 (cluster also covers task#21).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 84/139 steps passed, score 60.4%). Ten clusters, all verdict=mismatch, all service=elasticache, operation=DescribeCacheParameters, stub_path empty and owned_by empty (so none is filed elsewhere).

Cause 1 — fixture elasticache-catalog-describe-cache-parameters step task#1 (affects task#1..task#3), expected 200, actual 200:
  $.DescribeCacheParametersResponse.DescribeCacheParametersResult.CacheNodeTypeSpecificParameters.CacheNodeTypeSpecificParameter[0]: missing (expected a member with AllowedValues '1-100000', MinimumEngineVersion '1.4.5' and a CacheNodeTypeSpecificValues list with one CacheNodeType/Value pair per node type, got None)
  same for CacheNodeTypeSpecificParameter[1]; plus Parameters.Parameter[0]..[19] reported divergent.
Cause 2 — step task#4, expected 200, actual 200:
  $...CacheNodeTypeSpecificParameters.CacheNodeTypeSpecificParameter: type_mismatch (expected a list whose first member has MinimumEngineVersion '2.6.13', AllowedValues '0-' and per-node-type values); Parameters.Parameter[0]..[27] divergent.
Cause 3 — step task#5: same type_mismatch, first member MinimumEngineVersion '2.8.6'; Parameters.Parameter[0]..[34] divergent.
Cause 4 — step task#6 (affects task#6, task#7): same type_mismatch, MinimumEngineVersion '3.2.4'; Parameters.Parameter[0]..[37] divergent.
Cause 5 — step task#8 (affects task#8, task#9): same type_mismatch, MinimumEngineVersion '4.0.9'; Parameters.Parameter[0]..[51] divergent.
Cause 6 — step task#10 (affects task#10, task#11): same type_mismatch, MinimumEngineVersion '5.0.0'; Parameters.Parameter[0]..[56] divergent.
Cause 7 — step task#12 (affects task#12, task#13): same type_mismatch, MinimumEngineVersion '6.0.5'; Parameters.Parameter[0]..[61] divergent.
Cause 8 — step task#14 (affects task#14..task#17): same type_mismatch, MinimumEngineVersion '7.0.0'; Parameters.Parameter[0]..[63] divergent.
Cause 9 — step task#18 (affects task#18, task#19): same type_mismatch, MinimumEngineVersion '8.0.0'; Parameters.Parameter[0]..[66] divergent.
Cause 10 — step task#20 (affects task#20, task#21): status: expected 200, got 404; $.DescribeCacheParametersResponse: missing (expected the full result whose first node-type-specific member carries MinimumEngineVersion '9.0.0', got nothing).

Expected in every cause: HTTP 200, DescribeCacheParametersResponse carrying the family's complete default Parameters list and a CacheNodeTypeSpecificParameters wrapper containing one CacheNodeTypeSpecificParameter per node-type-scoped parameter.
Actual: HTTP 200 with an unpopulated/mis-shaped CacheNodeTypeSpecificParameters node and a shorter, differently-valued Parameters list (causes 1-9), and HTTP 404 CacheParameterGroupNotFound for the valkey9 family (cause 10).

Model evidence: smithy_query(query="operation", operation="DescribeCacheParameters", model_path="services/service-com-amazonaws-elasticache-amazonelasticachev9/smithy-model/com-amazonaws-elasticache-amazonelasticachev9.json") -> output CacheParameterGroupDetails {Marker, Parameters: ParametersList, CacheNodeTypeSpecificParameters: CacheNodeTypeSpecificParametersList}, errors CacheParameterGroupNotFoundFault 404/CacheParameterGroupNotFound, InvalidParameterValueException 400/InvalidParameterValue, InvalidParameterCombinationException 400/InvalidParameterCombination. The model does not state catalog contents; those are recorded behavior only. Node types, concrete per-node values and parameter values quoted above are the recording's fill-ins, not the requirement: the requirement is that each family's default set is complete and that node-type-scoped parameters are returned as a populated list.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Replay fixture elasticache-catalog-describe-cache-parameters (all of task#1..task#21) from trace set simparity-wire-aws-v1.
3. For each family (memcached1.4, redis2.6, redis2.8, redis3.2, redis4.0, redis5.0, redis6.x, redis7, valkey8) call DescribeCacheParameters(CacheParameterGroupName="default.<family>") and compare the response to the fixture: Parameters must contain the family's full default set and CacheNodeTypeSpecificParameters must contain one member per node-type-scoped parameter, each with its AllowedValues, MinimumEngineVersion and per-CacheNodeType values. Observe an empty/mis-shaped CacheNodeTypeSpecificParameters node and a shorter Parameters list.
4. Call DescribeCacheParameters(CacheParameterGroupName="default.valkey9"). Expect HTTP 200 with the valkey9 catalog; observe HTTP 404 CacheParameterGroupNotFound.
5. Re-replay the fixture after the fix and confirm all 21 steps pass.
