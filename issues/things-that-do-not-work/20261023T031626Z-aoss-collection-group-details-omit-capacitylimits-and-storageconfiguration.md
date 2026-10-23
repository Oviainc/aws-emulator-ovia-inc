Issue-Source: trace-sweep-divergence
Issue-Trace: opensearchserverless-collection-group-error-paths
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearchserverless-collection-group-error-paths opensearchserverless-collection-in-group opensearchserverless-tag-resource-lifecycle
Issue-Shape-Id: com.amazonaws.opensearchserverless#OpenSearchServerless
Issue-Key: aoss|CreateCollectionGroup|2xx->2xx|$.createCollectionGroupDetail.capacityLimits
Issue-Key: aoss|CreateCollectionGroup|2xx->2xx|$.createCollectionGroupDetail.storageConfiguration
Issue-Key: aoss|BatchGetCollectionGroup|2xx->2xx|$.collectionGroupDetails[*].capacityLimits
Issue-Related: 20261023T031626Z-aoss-writes-are-not-reflected-in-stored-state.md
Issue-Commit: 7f205ee96657d4151f96c4aa7296372f34590a63

# aoss collection-group details omit capacityLimits and storageConfiguration

> _Note: this issue was identified against the code base at commit `7f205ee96657d4151f96c4aa7296372f34590a63`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCollectionGroup

## Description
This issue bundles TWO distinct causes in the opensearchserverless (`aoss`) emulator, sharing one fault: a collection group created without explicit capacity settings is serialized without the capacityLimits the service defaults in, and without the storageConfiguration block the service always returns. Each cause must be closed before submitting.

Cause 1 of 2: CreateCollectionGroup's response detail omits capacityLimits and storageConfiguration.
AWS: Smithy models CreateCollectionGroupDetail.capacityLimits (CollectionGroupCapacityLimits) and the input's capacityLimits as OPTIONAL; recorded behavior shows that when the request omits it, the service still returns a defaulted capacityLimits of maxIndexingCapacityInOCU / maxSearchCapacityInOCU, plus a storageConfiguration object carrying v2Enabled. storageConfiguration is NOT in this checkout's Smithy model (no CollectionGroupStorageConfiguration shape exists), so it is recorded behavior only: the live API returns it on the create detail.
Emulator: returns createCollectionGroupDetail with both members absent (null).
Why: a client reading the group's effective OCU ceiling straight off the create response gets nothing and cannot tell the defaulted capacity from an unset one; code that dereferences capacityLimits or storageConfiguration on the create detail breaks against the emulator while working on AWS.
Fix: CreateCollectionGroup's handler and serializer (services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/create_collection_group.rs) — materialize the default capacityLimits into the stored CollectionGroupRecord and emit storageConfiguration.
Traces: opensearchserverless-collection-group-error-paths#setup#1 (also reproduced at opensearchserverless-collection-in-group#setup#2 and opensearchserverless-tag-resource-lifecycle#setup#1).

Cause 2 of 2: BatchGetCollectionGroup's read path omits the same two members.
AWS: collectionGroupDetails[i] carries capacityLimits (defaulted when never set) and storageConfiguration (recorded behavior only, as above).
Emulator: collectionGroupDetails[0] has both absent.
Why: the read path is where clients poll a group's capacity, so the omission is not just a create-response artifact — nothing in the API ever exposes the group's effective limits.
Fix: the BatchGetCollectionGroup read/serialize path (batch_get_collection_group.rs) plus whatever default the state applies at creation, so stored and served records agree.
Traces: opensearchserverless-collection-group-error-paths#task#4.

## Evidence
Cause 1 — opensearchserverless-collection-group-error-paths#setup#1 CreateCollectionGroup (status 200 matched): $.createCollectionGroupDetail.capacityLimits expected {'maxIndexingCapacityInOCU': 96.0, 'maxSearchCapacityInOCU': 96.0}, got None; $.createCollectionGroupDetail.storageConfiguration expected {'v2Enabled': False}, got None. Cluster also covers opensearchserverless-collection-in-group#setup#2 and opensearchserverless-tag-resource-lifecycle#setup#1 (affected_count 3).
Cause 2 — opensearchserverless-collection-group-error-paths#task#4 BatchGetCollectionGroup (200): $.collectionGroupDetails[0].capacityLimits expected {'maxIndexingCapacityInOCU': 96.0, 'maxSearchCapacityInOCU': 96.0}, got None; $.collectionGroupDetails[0].storageConfiguration expected {'v2Enabled': False}, got None. (The defaulted numbers above are this recording's values; the requirement is that the members are present and reflect the group's effective settings.)

## Steps to Reproduce
Boot the emulator and replay the aoss fixtures.
1 (Cause 1): CreateCollectionGroup(name=<new>, standbyReplicas='DISABLED', description='...') with NO capacityLimits in the request -> expect createCollectionGroupDetail.capacityLimits populated with the service default and a storageConfiguration object; observe both null. Fixture opensearchserverless-collection-group-error-paths#setup#1.
2 (Cause 2): BatchGetCollectionGroup(ids=[<that id>]) -> expect collectionGroupDetails[0].capacityLimits and .storageConfiguration present; observe both null. Fixture opensearchserverless-collection-group-error-paths#task#4.
