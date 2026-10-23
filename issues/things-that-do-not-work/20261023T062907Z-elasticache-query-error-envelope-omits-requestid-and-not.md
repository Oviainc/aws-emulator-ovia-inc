Issue-Source: trace-sweep-divergence
Issue-Trace: elasticache-cache-parameter-group-workflow
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticache-cache-parameter-group-workflow elasticache-snapshot-backup-and-copy
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache|CreateCacheParameterGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|CreateCacheParameterGroup|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticache|DescribeEngineDefaultParameters|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|DescribeEngineDefaultParameters|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticache|DescribeCacheParameterGroups|4xx->2xx|$.ErrorResponse
Issue-Key: elasticache|DescribeCacheParameterGroups|4xx->2xx|status
Issue-Key: elasticache|CreateSnapshot|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|CreateSnapshot|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticache|CopySnapshot|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|CopySnapshot|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticache|DescribeServiceUpdates|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|DescribeServiceUpdates|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticache|BatchApplyUpdateAction|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|BatchApplyUpdateAction|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticache|BatchStopUpdateAction|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|BatchStopUpdateAction|4xx->4xx|$.ErrorResponse.RequestId
Issue-Commit: c0919b0ae1fd753648687abf4b08014f282092d3

# ElastiCache query error envelope omits RequestId and not-found semantics diverge on parameter-group, snapshot and service-update operations

> _Note: this issue was identified against the code base at commit `c0919b0ae1fd753648687abf4b08014f282092d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCacheParameterGroup

## Description
Eight divergences across the parameter-group, snapshot and service-update operations. The dominant, shared fault is that the emulator's awsQuery ErrorResponse envelope carries no RequestId element at all; one further cause is a deleted parameter group that is still described as present. Each cause is separate and must be closed separately. (replay: state_reset_failures=0.)

Cause 1 of 8: CreateCacheParameterGroup duplicate-name error omits RequestId from the envelope.
AWS: Smithy: CreateCacheParameterGroup binds CacheParameterGroupAlreadyExistsFault (awsQueryError code `CacheParameterGroupAlreadyExists`, HTTP 400). In the awsQuery protocol the fault is serialized as <ErrorResponse><Error>{Type,Code,Message}</Error><RequestId>...</RequestId></ErrorResponse>; the recording carries a RequestId.
Emulator: status 400 and Error.Code are right, but $.ErrorResponse.RequestId is absent. The message text also differs ('Cache parameter group X already exists.' vs the recorded 'Parameter group X already exists'); the recorded text is an example only — the requirement here is the envelope's RequestId element.
Why: every AWS SDK surfaces the request id for support/correlation and some clients log or assert on its presence; an envelope without it is not the protocol's error shape.
Fix: the shared awsQuery error serializer for elasticache — emit RequestId for every ErrorResponse, as the success path already does for ResponseMetadata.RequestId.
Traces: elasticache-cache-parameter-group-workflow#task#9.

Cause 2 of 8: DescribeEngineDefaultParameters invalid-family error omits RequestId.
AWS: Smithy: DescribeEngineDefaultParameters binds InvalidParameterValueException (code `InvalidParameterValue`, HTTP 400) and requires CacheParameterGroupFamily. Recorded behavior for an unknown family is HTTP 400 `InvalidParameterValue` with a RequestId in the envelope; recorded example message 'CacheParameterGroupFamily not-a-real-family is not a valid parameter group family.'
Emulator: 400 and the code match; $.ErrorResponse.RequestId is absent. (The message wording differs but both texts name the member and the offending value, so the message is not the finding.)
Why: as Cause 1 — the error envelope is incomplete on the validation path too.
Fix: same shared awsQuery error serializer.
Traces: elasticache-cache-parameter-group-workflow#task#10.

Cause 3 of 8: a deleted cache parameter group is still returned by DescribeCacheParameterGroups.
AWS: Smithy: DescribeCacheParameterGroups models CacheParameterGroupNotFoundFault (code `CacheParameterGroupNotFound`, HTTP 404). Recorded behavior after DeleteCacheParameterGroup is HTTP 404 with Error.Type `Sender`, Error.Code `CacheParameterGroupNotFound`; recorded example message 'CacheParameterGroup <name> not found.'
Emulator: answers HTTP 200 with the group still present (no ErrorResponse) at teardown time, i.e. the delete did not remove it from state.
Why: teardown/idempotency logic cannot observe deletion, quotas never free up, and a recreate of the same name will collide with a group the client believes it deleted.
Fix: the DeleteCacheParameterGroup handler's state mutation and the DescribeCacheParameterGroups lookup/not-found path.
Traces: elasticache-cache-parameter-group-workflow#teardown#15.

Cause 4 of 8: CreateSnapshot for an absent cache cluster omits RequestId.
AWS: Smithy: CreateSnapshot models CacheClusterNotFoundFault (code `CacheClusterNotFound`, HTTP 404). Recorded behavior: HTTP 404, Error.Code `CacheClusterNotFound`, with a RequestId element.
Emulator: status 404 and code match; $.ErrorResponse.RequestId is absent (message wording also differs: 'Cache cluster <id> not found.' vs recorded 'Cache cluster not found: <id>' — example only).
Fix: same shared awsQuery error serializer.
Why: as Cause 1.
Traces: elasticache-snapshot-backup-and-copy#task#3.

Cause 5 of 8: CopySnapshot for an absent source snapshot omits RequestId.
AWS: Smithy: CopySnapshot models SnapshotNotFoundFault (code `SnapshotNotFoundFault`, HTTP 404). Recorded behavior: HTTP 404 with that code and a RequestId.
Emulator: status and code match; $.ErrorResponse.RequestId absent (recorded example message 'Snapshot with name <name> not found' vs emulator 'Snapshot <name> not found.').
Why: as Cause 1.
Fix: same shared awsQuery error serializer.
Traces: elasticache-snapshot-backup-and-copy#task#4.

Cause 6 of 8: DescribeServiceUpdates for an unknown ServiceUpdateName omits RequestId.
AWS: Smithy: DescribeServiceUpdates models ServiceUpdateNotFoundFault (code `ServiceUpdateNotFoundFault`, HTTP 404). Recorded behavior: HTTP 404 with that code and a RequestId; recorded example message 'ServiceUpdate <name> not found.'
Emulator: status and code match; $.ErrorResponse.RequestId absent.
Why: as Cause 1.
Fix: same shared awsQuery error serializer.
Traces: elasticache-snapshot-backup-and-copy#task#6.

Cause 7 of 8: BatchApplyUpdateAction for an unknown ServiceUpdateName omits RequestId.
AWS: Smithy: BatchApplyUpdateAction models ServiceUpdateNotFoundFault (code `ServiceUpdateNotFoundFault`, HTTP 404) and InvalidParameterValueException (400). Recorded behavior for an unknown update name: HTTP 404 `ServiceUpdateNotFoundFault` with a RequestId.
Emulator: status and code match; $.ErrorResponse.RequestId absent.
Why: as Cause 1.
Fix: same shared awsQuery error serializer.
Traces: elasticache-snapshot-backup-and-copy#task#7.

Cause 8 of 8: BatchStopUpdateAction for an unknown ServiceUpdateName omits RequestId.
AWS: Smithy: BatchStopUpdateAction likewise models ServiceUpdateNotFoundFault (code `ServiceUpdateNotFoundFault`, HTTP 404). Recorded behavior: HTTP 404 with that code and a RequestId.
Emulator: status and code match; $.ErrorResponse.RequestId absent.
Why: as Cause 1.
Fix: same shared awsQuery error serializer.
Traces: elasticache-snapshot-backup-and-copy#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1, all clusters verdict=mismatch, stub_path empty, owned_by empty:
1. elasticache-cache-parameter-group-workflow#task#9 CreateCacheParameterGroup (duplicate name) — 400 both sides; $.ErrorResponse.RequestId missing (expected present, got None); Message 'Cache parameter group X already exists.' vs recorded 'Parameter group X already exists'.
2. elasticache-cache-parameter-group-workflow#task#10 DescribeEngineDefaultParameters(CacheParameterGroupFamily='not-a-real-family') — 400 both sides; $.ErrorResponse.RequestId missing.
3. elasticache-cache-parameter-group-workflow#teardown#15 DescribeCacheParameterGroups — expected HTTP 404 {Error:{Type:Sender, Code:CacheParameterGroupNotFound, Message:'CacheParameterGroup <name> not found.'}}; got HTTP 200 with $.ErrorResponse missing.
4. elasticache-snapshot-backup-and-copy#task#3 CreateSnapshot(absent CacheClusterId) — 404/CacheClusterNotFound both sides; $.ErrorResponse.RequestId missing.
5. elasticache-snapshot-backup-and-copy#task#4 CopySnapshot(absent SourceSnapshotName) — 404 both sides; $.ErrorResponse.RequestId missing.
6. elasticache-snapshot-backup-and-copy#task#6 DescribeServiceUpdates(absent ServiceUpdateName) — 404 both sides; $.ErrorResponse.RequestId missing.
7. elasticache-snapshot-backup-and-copy#task#7 BatchApplyUpdateAction(absent ServiceUpdateName) — 404 both sides; $.ErrorResponse.RequestId missing.
8. elasticache-snapshot-backup-and-copy#task#8 BatchStopUpdateAction(absent ServiceUpdateName) — 404 both sides; $.ErrorResponse.RequestId missing.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Causes 1,2: replay elasticache-cache-parameter-group-workflow steps task#9 (CreateCacheParameterGroup with an existing name) and task#10 (DescribeEngineDefaultParameters with CacheParameterGroupFamily='not-a-real-family'); inspect the raw XML for a <RequestId> child of <ErrorResponse>.
3. Cause 3: replay the same fixture through teardown#15 — DeleteCacheParameterGroup, then DescribeCacheParameterGroups for that name, expecting HTTP 404 CacheParameterGroupNotFound.
4. Causes 4-8: replay elasticache-snapshot-backup-and-copy steps task#3, task#4, task#6, task#7, task#8 (CreateSnapshot / CopySnapshot / DescribeServiceUpdates / BatchApplyUpdateAction / BatchStopUpdateAction against absent ids) and inspect each ErrorResponse for RequestId.
5. Confirm the fix by re-replaying every step named in Traces.
