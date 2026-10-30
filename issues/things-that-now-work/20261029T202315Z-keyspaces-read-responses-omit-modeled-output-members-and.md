Issue-Source: trace-sweep-divergence
Issue-Trace: keyspaces-table-autoscaling-and-update
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: keyspaces-table-autoscaling-and-update keyspaces-table-create-active-transition-and-tags keyspaces-table-create-invalid-schema-rejected keyspaces-type-lifecycle-and-rejections
Issue-Shape-Id: com.amazonaws.keyspaces#KeyspacesService
Issue-Key: keyspaces|GetTable|2xx->2xx|$.comment
Issue-Key: keyspaces|GetTable|2xx->2xx|$.defaultTimeToLive
Issue-Key: keyspaces|GetTable|2xx->2xx|$.replicaSpecifications
Issue-Key: keyspaces|GetTable|2xx->2xx|$.schemaDefinition.clusteringKeys
Issue-Key: keyspaces|GetTable|2xx->2xx|$.schemaDefinition.staticColumns
Issue-Key: keyspaces|GetTable|2xx->2xx|$.warmThroughputSpecification
Issue-Key: keyspaces|GetTable|2xx->2xx|$.capacitySpecification.lastUpdateToPayPerRequestTimestamp
Issue-Key: keyspaces|GetTableAutoScalingSettings|2xx->2xx|$.replicaSpecifications
Issue-Key: keyspaces|GetType|2xx->2xx|$.maxNestingDepth
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# keyspaces read responses omit modeled output members and report maxNestingDepth as 0

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTable

## Description
This issue bundles 5 net-new root causes in the keyspaces crate. All are the same KIND of defect — a read operation's response is missing members Amazon Keyspaces always serializes, or serializes one with a value it never computes — but each is produced at a distinct serialization site, so each must be fixed separately. All five returned HTTP 200 with a matching status line; only the body diverged. Five causes, all 5 must be closed before submitting.

Cause 1 of 5: GetTable omits comment, defaultTimeToLive, replicaSpecifications, schemaDefinition.clusteringKeys, schemaDefinition.staticColumns and warmThroughputSpecification for a provisioned, single-Region table with auto scaling.
AWS: Smithy declares GetTableResponse members comment (Comment, whose member `message` is @required), defaultTimeToLive (@range 0..630720000), replicaSpecifications (ReplicaSpecificationSummaryList, @length min 0 — an empty list is a legal, modeled value), schemaDefinition.clusteringKeys and schemaDefinition.staticColumns (lists), and warmThroughputSpecification (WarmThroughputSpecificationSummary, whose readUnitsPerSecond, writeUnitsPerSecond and status are all @required). The recorded response for such a table carries ALL of them: comment as the structure {"message": ""} when no comment was given, defaultTimeToLive as 0 when TTL was never set, replicaSpecifications / clusteringKeys / staticColumns as empty JSON arrays rather than absent members, and warmThroughputSpecification as the structure with the table's current warm read/write units per second and status AVAILABLE (status is UPDATING while a change is being applied, per the model's doc for WarmThroughputStatus). Recorded behavior is the authority for the empty-value convention: AWS emits the member with its empty/zero value, it does not drop it.
Emulator: omits all six members from the GetTable body, so a decoder sees them as absent/None.
Why: a client reading comment.message, defaultTimeToLive, or iterating schemaDefinition.clusteringKeys / replicaSpecifications gets a null where AWS guarantees a value, which in strict SDKs is a deserialization or null-pointer failure rather than an empty result, and any caller that reads warm throughput to decide whether to pre-warm capacity sees the feature as unavailable on every table.
Fix: the GetTable response serializer — it must emit these members with their empty/zero/default values instead of skipping them, and must populate warmThroughputSpecification from the table's stored warm-throughput state.
Traces: keyspaces-table-autoscaling-and-update#task#3 (the same cluster also covers #task#6, the GetTable after UpdateTable).

Cause 2 of 5: GetTable additionally omits capacitySpecification.lastUpdateToPayPerRequestTimestamp after the table has been switched to PAY_PER_REQUEST.
AWS: Smithy declares CapacitySpecificationSummary.lastUpdateToPayPerRequestTimestamp (Timestamp), doc 'The timestamp of the last operation that changed the provisioned throughput capacity of a table.' The recorded response for a table whose throughputMode is PAY_PER_REQUEST carries this member as the epoch-seconds timestamp of that last throughput change, alongside the same always-present members in Cause 1 (comment, defaultTimeToLive, replicaSpecifications, warmThroughputSpecification).
Emulator: omits lastUpdateToPayPerRequestTimestamp entirely, along with the Cause-1 members, so the capacity summary never records that the mode changed.
Why: tooling that reports when a table last changed capacity mode, or that waits for a mode switch to be observable, has no field to read and concludes the switch never happened. The timestamp's VALUE is per-run; its presence for a PAY_PER_REQUEST table is the requirement.
Fix: the GetTable capacity-summary serializer plus the state that records the time of the last throughput-mode change.
Traces: keyspaces-table-create-active-transition-and-tags#task#3.

Cause 3 of 5: GetTable omits the same member set on a table created with an explicit provisioned capacity and schema.
AWS: as Causes 1 and 2 — the recorded response carries capacitySpecification.lastUpdateToPayPerRequestTimestamp, comment {"message": ""}, defaultTimeToLive 0, replicaSpecifications [], schemaDefinition.clusteringKeys [], schemaDefinition.staticColumns [] and warmThroughputSpecification {readUnitsPerSecond, writeUnitsPerSecond, status: AVAILABLE} derived from the table's provisioned capacity.
Emulator: omits all seven.
Why: as Causes 1 and 2; this cause is listed separately because it is the GetTable read of a freshly created table (a different fixture path than the auto-scaling and mode-switch reads) and must be re-replayed to confirm the serializer change covers it.
Fix: the GetTable response serializer (same site as Causes 1-2).
Traces: keyspaces-table-create-invalid-schema-rejected#task#5.

Cause 4 of 5: GetTableAutoScalingSettings omits replicaSpecifications.
AWS: Smithy declares GetTableAutoScalingSettingsResponse.replicaSpecifications (ReplicaAutoScalingSpecificationList, @length min 0). The recorded response for a single-Region table carries it as an empty JSON array, not an absent member.
Emulator: omits the member from the body.
Why: a caller that iterates per-replica auto scaling settings gets a null instead of an empty list, so multi-Region-aware code cannot distinguish 'no replicas' from 'field not served' and strict decoders fail.
Fix: the GetTableAutoScalingSettings response serializer — emit the empty list.
Traces: keyspaces-table-autoscaling-and-update#task#4.

Cause 5 of 5: GetType reports maxNestingDepth as 0 for a flat user-defined type.
AWS: Smithy declares GetTypeResponse.maxNestingDepth (Depth, integer, @default 0), doc 'The level of nesting implemented for this type.' The recorded response for a UDT whose fields are all scalar (no frozen<UDT> or collection-of-UDT field) is 1: the depth counts the type itself, so a type with no nested UDT has depth 1 and each level of UDT nesting adds one. The model's @default 0 is the protocol default for an unset member, not the value for an existing type.
Emulator: returns 0 — it is serving the shape default instead of computing the depth from the type's field definitions.
Why: a client that validates nesting against the service's 'maximum nesting depth' quota, or that sorts/filters types by depth, reads every type as depth 0 and so mis-computes how much nesting a new type may still add; a check for depth >= 1 on an existing type fails outright.
Fix: the GetType handler/serializer — compute maxNestingDepth from the stored field definitions (1 for a type with no UDT-typed field, plus the deepest nested UDT's depth) rather than emitting the shape default.
Traces: keyspaces-type-lifecycle-and-rejections#task#3.

## Evidence
Replay of simparity-wire-aws-v1, all five steps returned the expected HTTP 200 — only the response body diverged:

Cause 1 — keyspaces-table-autoscaling-and-update#task#3, GetTable (cluster also covers #task#6):
  $.comment: missing (expected {'message': ''}, got None)
  $.defaultTimeToLive: missing (expected 0, got None)
  $.replicaSpecifications: missing (expected [], got None)
  $.schemaDefinition.clusteringKeys: missing (expected [], got None)
  $.schemaDefinition.staticColumns: missing (expected [], got None)
  $.warmThroughputSpecification: missing (expected {'readUnitsPerSecond': 5, 'status': 'AVAILABLE', 'writeUnitsPerSecond': 5}, got None)

Cause 2 — keyspaces-table-create-active-transition-and-tags#task#3, GetTable:
  $.capacitySpecification.lastUpdateToPayPerRequestTimestamp: missing (expected a timestamp, got None)
  $.comment: missing (expected {'message': ''}, got None)
  $.defaultTimeToLive: missing (expected 0, got None)
  $.replicaSpecifications: missing (expected [], got None)
  $.warmThroughputSpecification: missing (expected {'readUnitsPerSecond': 12000, 'status': 'AVAILABLE', 'writeUnitsPerSecond': 4000}, got None)

Cause 3 — keyspaces-table-create-invalid-schema-rejected#task#5, GetTable:
  $.capacitySpecification.lastUpdateToPayPerRequestTimestamp, $.comment, $.defaultTimeToLive, $.replicaSpecifications, $.schemaDefinition.clusteringKeys, $.schemaDefinition.staticColumns, $.warmThroughputSpecification: all missing (got None)

Cause 4 — keyspaces-table-autoscaling-and-update#task#4, GetTableAutoScalingSettings:
  $.replicaSpecifications: missing (expected [], got None)

Cause 5 — keyspaces-type-lifecycle-and-rejections#task#3, GetType:
  $.maxNestingDepth: value_mismatch (expected 1, got 0)

The capacity/warm-throughput NUMBERS above are the recorded run's configured units and the timestamp is that run's clock — the requirement is that the members are present and derived from the table's own state, not those literal values.

## Steps to Reproduce
Against the emulator's keyspaces endpoint, decoding the raw JSON body (not an SDK model that hides absent members):

Causes 1 & 4: CreateKeyspace(K); CreateTable(K, TB, schemaDefinition with partition key only, capacitySpecification PROVISIONED, autoScalingSpecification set); poll until ACTIVE; GetTable(K, TB) — expect comment={'message':''}, defaultTimeToLive=0, replicaSpecifications=[], schemaDefinition.clusteringKeys=[], schemaDefinition.staticColumns=[], warmThroughputSpecification present with status AVAILABLE. Then GetTableAutoScalingSettings(K, TB) — expect replicaSpecifications=[]. Then UpdateTable on TB and GetTable again: the same members must still be present.

Cause 2: CreateTable with PROVISIONED capacity, wait ACTIVE, UpdateTable to capacitySpecification throughputMode=PAY_PER_REQUEST, then GetTable — expect capacitySpecification.lastUpdateToPayPerRequestTimestamp present.

Cause 3: CreateTable with an explicit provisioned capacity and a valid schema, wait ACTIVE, GetTable — expect the full member set above.

Cause 5: CreateKeyspace(K2); CreateType(K2, T, fieldDefinitions=[{name:f, type:text}]); poll until ACTIVE; GetType(K2, T) — expect maxNestingDepth=1. Create a second type with a frozen<T> field and expect its depth to be one greater.

Replaying the fixtures/steps in Issue-Trace/Issue-Step and Issue-Trace-Ids reproduces each cause.
