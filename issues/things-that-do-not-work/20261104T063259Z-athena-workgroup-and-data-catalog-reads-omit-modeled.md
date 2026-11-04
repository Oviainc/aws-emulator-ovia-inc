Issue-Source: trace-sweep-divergence
Issue-Trace: athena-data-catalog-crud-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: athena-data-catalog-crud-lifecycle athena-data-catalog-update-round-trip-and-missing-name-rejected athena-update-data-catalog-contract athena-get-work-group-configuration-and-missing-error athena-resource-tagging-workflow athena-workgroup-lifecycle-and-query-execution
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|GetDataCatalog|2xx->2xx|$.DataCatalog.Parameters.catalog
Issue-Key: athena|GetDataCatalog|2xx->2xx|$.DataCatalog.Parameters.metadata-function
Issue-Key: athena|GetDataCatalog|2xx->2xx|$.DataCatalog.Parameters.record-function
Issue-Key: athena|GetDataCatalog|2xx->2xx|$.DataCatalog.Description
Issue-Key: athena|GetDataCatalog|4xx->2xx|$
Issue-Key: athena|GetDataCatalog|4xx->2xx|$.AthenaErrorCode
Issue-Key: athena|GetDataCatalog|4xx->2xx|$.ErrorCode
Issue-Key: athena|GetDataCatalog|4xx->2xx|$.Message
Issue-Key: athena|GetDataCatalog|4xx->2xx|$.__type
Issue-Key: athena|GetDataCatalog|4xx->2xx|status
Issue-Key: athena|GetWorkGroup|2xx->2xx|$.WorkGroup.Configuration.EnableMinimumEncryptionConfiguration
Issue-Key: athena|GetWorkGroup|2xx->2xx|$.WorkGroup.Configuration.EngineVersion.Category
Issue-Key: athena|GetWorkGroup|2xx->2xx|$.WorkGroup.Configuration.QuerySchedulingType
Issue-Key: athena|GetWorkGroup|2xx->2xx|$.WorkGroup.Configuration.RequesterPaysEnabled
Issue-Key: athena|GetWorkGroup|4xx->2xx|$
Issue-Key: athena|GetWorkGroup|4xx->2xx|$.AthenaErrorCode
Issue-Key: athena|GetWorkGroup|4xx->2xx|$.ErrorCode
Issue-Key: athena|GetWorkGroup|4xx->2xx|$.Message
Issue-Key: athena|GetWorkGroup|4xx->2xx|$.__type
Issue-Key: athena|GetWorkGroup|4xx->2xx|status
Issue-Key: athena|GetWorkGroup|2xx->2xx|$.WorkGroup.Configuration.PublishCloudWatchMetricsEnabled
Issue-Key: athena|GetWorkGroup|2xx->2xx|$.WorkGroup.Configuration.BytesScannedCutoffPerQuery
Issue-Key: athena|GetWorkGroup|2xx->2xx|$.WorkGroup.Description
Issue-Commit: fb394b2a45eb301a39bf46320599094d7846a70d

# Athena workgroup and data-catalog reads omit modeled members, drop applied updates and survive deletion (8 causes)

> _Note: this issue was identified against the code base at commit `fb394b2a45eb301a39bf46320599094d7846a70d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWorkGroup

## Description
The emulator's Athena control-plane resources (data catalogs, workgroups) are stored lossily: creation parameters are dropped, updates are never applied, deletions are not reflected in reads, and the read responses omit members AWS always returns. 8 causes.

Cause 1 of 8: CreateDataCatalog drops the Parameters map it was given.
AWS: Smithy: DataCatalog.Parameters (ParametersMap); for a LAMBDA catalog the required parameter sets are metadata-function + record-function, or the composite function. AWS round-trips every key it was given, so GetDataCatalog returns the catalog's full Parameters map.
Emulator: GetDataCatalog returns a Parameters map missing the keys that were submitted (the recorded catalog, metadata-function and record-function keys are absent; only a 'function' key survives).
Why: a client reading back a federated catalog cannot discover which Lambda functions serve it, so connector wiring is unresolvable.
Fix: the athena crate's CreateDataCatalog handler and its data-catalog state (Parameters storage).
Traces: athena-data-catalog-crud-lifecycle#task#4.

Cause 2 of 8: UpdateDataCatalog does not apply the new Description (and still drops Parameters).
AWS: Smithy: UpdateDataCatalog input Name (required), Type (required), Description, Parameters. A successful update is immediately visible to GetDataCatalog.
Emulator: GetDataCatalog still returns the pre-update Description, and the Parameters keys are still missing.
Why: an update reported as successful is silently discarded, so a client's configuration change never takes effect.
Fix: the athena crate's UpdateDataCatalog handler (mutation applied to stored state).
Traces: athena-data-catalog-crud-lifecycle#task#6.

Cause 3 of 8: a deleted data catalog is still readable.
AWS: after DeleteDataCatalog, GetDataCatalog refuses with HTTP 400 InvalidRequestException, AthenaErrorCode and ErrorCode both INVALID_INPUT, and Message of the fixed form "DataCatalog <name> was not found." (<name> from the request).
Emulator: HTTP 200 with the full DataCatalog still present (Status CREATE_COMPLETE) and no error members, on every one of 15 poll attempts.
Why: deletion is not observable; a client that waits for the resource to disappear loops forever and a recreate-after-delete flow sees a stale resource.
Fix: the athena crate's DeleteDataCatalog handler and its data-catalog state.
Traces: athena-data-catalog-crud-lifecycle#teardown#9.

Cause 4 of 8: UpdateDataCatalog's description change is invisible in the dedicated round-trip fixture too.
AWS: GetDataCatalog after UpdateDataCatalog returns the updated Description.
Emulator: returns the original Description.
Why: same as cause 2, on the UpdateDataCatalog contract path — the update is accepted and lost.
Fix: the athena crate's UpdateDataCatalog handler.
Traces: athena-data-catalog-update-round-trip-and-missing-name-rejected#task#4, athena-update-data-catalog-contract#task#4.

Cause 5 of 8: GetWorkGroup omits the configuration members AWS always serves.
AWS: Smithy: WorkGroupConfiguration.EnableMinimumEncryptionConfiguration (BoxedBoolean) and RequesterPaysEnabled (BoxedBoolean) are returned with their effective values (false when unset). Recorded behavior only: AWS also returns Configuration.EngineVersion.Category ('Presto') and Configuration.QuerySchedulingType ('DEFAULT'), neither of which the public Smithy model declares.
Emulator: all four members are absent from the Configuration object.
Why: a client that reads effective workgroup settings sees unset where AWS states a value, so encryption-minimum and requester-pays checks mis-evaluate.
Fix: the athena crate's GetWorkGroup serializer and the workgroup configuration defaults.
Traces: athena-get-work-group-configuration-and-missing-error#task#2.

Cause 6 of 8: a deleted workgroup is still readable.
AWS: after DeleteWorkGroup, GetWorkGroup refuses with HTTP 400 InvalidRequestException, AthenaErrorCode and ErrorCode both INVALID_INPUT, and the fixed Message 'WorkGroup is not found.' (no name interpolated).
Emulator: HTTP 200 with the workgroup still present (State ENABLED) and no error members, on all 15 poll attempts.
Why: deletion is unobservable; teardown and recreate flows cannot converge.
Fix: the athena crate's DeleteWorkGroup handler and its workgroup state.
Traces: athena-resource-tagging-workflow#teardown#9, athena-workgroup-lifecycle-and-query-execution#teardown#14.

Cause 7 of 8: GetWorkGroup also omits PublishCloudWatchMetricsEnabled on a freshly created workgroup.
AWS: Smithy: WorkGroupConfiguration.PublishCloudWatchMetricsEnabled, RequesterPaysEnabled and EnableMinimumEncryptionConfiguration are returned (false when unset), together with the recorded EngineVersion.Category and QuerySchedulingType.
Emulator: those five members are absent.
Why: as cause 5 — effective configuration cannot be read back after CreateWorkGroup.
Fix: the athena crate's CreateWorkGroup configuration defaults and the GetWorkGroup serializer.
Traces: athena-workgroup-lifecycle-and-query-execution#task#2.

Cause 8 of 8: UpdateWorkGroup's Description and BytesScannedCutoffPerQuery are not applied.
AWS: Smithy: WorkGroupConfiguration.BytesScannedCutoffPerQuery (@range min 10000000) and WorkGroup.Description. After UpdateWorkGroup sets a description and a scan cutoff, GetWorkGroup returns both.
Emulator: WorkGroup.Description and Configuration.BytesScannedCutoffPerQuery are absent, along with the default members of cause 7.
Why: a workgroup data-usage control a client configures is reported as unset, so a cost guardrail silently does not exist.
Fix: the athena crate's UpdateWorkGroup handler (mutation applied to stored state) and the GetWorkGroup serializer.
Traces: athena-workgroup-lifecycle-and-query-execution#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.
Cause 1 — athena-data-catalog-crud-lifecycle#task#4, GetDataCatalog: expected $.DataCatalog.Parameters.catalog, .metadata-function and .record-function present; actual all three missing.
Cause 2 — athena-data-catalog-crud-lifecycle#task#6, GetDataCatalog: expected $.DataCatalog.Description='updated federated lambda catalog'; actual 'federated lambda catalog'; the three Parameters keys still missing.
Cause 3 — athena-data-catalog-crud-lifecycle#teardown#9, GetDataCatalog: expected 400 with $.__type='InvalidRequestException', $.AthenaErrorCode='INVALID_INPUT', $.ErrorCode='INVALID_INPUT', $.Message='DataCatalog <name> was not found.'; actual 200 returning the DataCatalog (Status CREATE_COMPLETE) — poll: error code None not in ['InvalidRequestException'] after 15 attempts.
Cause 4 — athena-data-catalog-update-round-trip-and-missing-name-rejected#task#4, GetDataCatalog: expected Description='updated description'; actual 'initial description' (2 steps).
Cause 5 — athena-get-work-group-configuration-and-missing-error#task#2, GetWorkGroup: expected Configuration.EnableMinimumEncryptionConfiguration=False, Configuration.EngineVersion.Category='Presto', Configuration.QuerySchedulingType='DEFAULT', Configuration.RequesterPaysEnabled=False; actual all missing.
Cause 6 — athena-resource-tagging-workflow#teardown#9, GetWorkGroup: expected 400 InvalidRequestException with AthenaErrorCode/ErrorCode='INVALID_INPUT' and Message='WorkGroup is not found.'; actual 200 returning the WorkGroup (State ENABLED) after 15 poll attempts (2 steps).
Cause 7 — athena-workgroup-lifecycle-and-query-execution#task#2, GetWorkGroup: expected EnableMinimumEncryptionConfiguration=False, EngineVersion.Category='Presto', PublishCloudWatchMetricsEnabled=False, QuerySchedulingType='DEFAULT', RequesterPaysEnabled=False; actual all missing.
Cause 8 — athena-workgroup-lifecycle-and-query-execution#task#7, GetWorkGroup: expected Configuration.BytesScannedCutoffPerQuery=10000000 and WorkGroup.Description='updated description' (plus the cause-7 defaults); actual all missing.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Inspect the listed fixture#step pairs, representative athena-data-catalog-crud-lifecycle#task#4.
4. Minimal manual repro: CreateDataCatalog (type LAMBDA with metadata-function and record-function parameters) then GetDataCatalog (expect both keys); UpdateDataCatalog with a new Description then GetDataCatalog (expect the new text); DeleteDataCatalog then GetDataCatalog (expect 400 'DataCatalog <name> was not found.'). Likewise CreateWorkGroup then GetWorkGroup (expect RequesterPaysEnabled/PublishCloudWatchMetricsEnabled/EnableMinimumEncryptionConfiguration present), UpdateWorkGroup with a Description and BytesScannedCutoffPerQuery then GetWorkGroup, and DeleteWorkGroup then GetWorkGroup (expect 400 'WorkGroup is not found.').
