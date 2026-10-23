Issue-Source: trace-sweep-divergence
Issue-Trace: opensearchserverless-collection-group-lifecycle
Issue-Step: teardown#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearchserverless-collection-group-lifecycle opensearchserverless-collection-in-group
Issue-Shape-Id: com.amazonaws.opensearchserverless#OpenSearchServerless
Issue-Key: aoss|BatchGetCollectionGroup|2xx->2xx|$.collectionGroupDetails[*]
Issue-Key: aoss|BatchGetCollectionGroup|2xx->2xx|$.collectionGroupErrorDetails[*]
Issue-Commit: 7f205ee96657d4151f96c4aa7296372f34590a63

# aoss BatchGetCollectionGroup never reports unknown ids in collectionGroupErrorDetails

> _Note: this issue was identified against the code base at commit `7f205ee96657d4151f96c4aa7296372f34590a63`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetCollectionGroup

## Description
This issue bundles TWO distinct causes in the opensearchserverless (`aoss`) emulator around BatchGetCollectionGroup's error channel. Each cause must be closed before submitting.

Cause 1 of 2: after DeleteCollectionGroup, BatchGetCollectionGroup still returns the group as a detail instead of a NOT_FOUND error entry.
AWS: BatchGetCollectionGroup (Smithy: output collectionGroupDetails + collectionGroupErrorDetails, errors ValidationException[400]/InternalServerException[500] only — a missing id is reported in-band, never as an exception) returns HTTP 200 with the deleted id absent from collectionGroupDetails and present in collectionGroupErrorDetails as an entry whose errorCode is NOT_FOUND and whose id echoes the request (CollectionGroupErrorDetail documents errorCode example NOT_FOUND and errorMessage example "The specified Collection Group is not found.").
Emulator: returns the full CollectionGroupDetail for the deleted group (id, arn, name, description, capacityLimits, numberOfCollections, generation) and an empty collectionGroupErrorDetails — so the delete is invisible to the read path.
Why: a client cannot observe deletion of a collection group and cannot distinguish a live group from a deleted one; teardown/recreate and drift-detection loops see a resource that should be gone.
Fix: DeleteCollectionGroup's state write must remove (or tombstone) the record, and BatchGetCollectionGroup's read path must route ids with no live record into collectionGroupErrorDetails (services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/batch_get_collection_group.rs and delete_collection_group.rs).
Traces: opensearchserverless-collection-group-lifecycle#teardown#6.

Cause 2 of 2: for an id with no record at all, BatchGetCollectionGroup returns 200 with BOTH lists effectively empty.
AWS: the unknown id appears in collectionGroupErrorDetails with errorCode NOT_FOUND and the requested id echoed back; the recorded errorMessage is an example ("The specified Collection Group is not found."), the errorCode and the entry's presence are the requirement.
Emulator: collectionGroupErrorDetails is absent/empty, so the caller gets a success with no information about the id it asked for.
Why: the batch contract is that every requested id comes back in exactly one of the two lists; a client that zips its request ids against the response — the normal way to use a batch API — silently loses ids instead of seeing NOT_FOUND.
Fix: the collectionGroupErrorDetails population in BatchGetCollectionGroup's handler (batch_get_collection_group.rs), for both ids and names input.
Traces: opensearchserverless-collection-in-group#teardown#8.

## Evidence
Cause 1 — opensearchserverless-collection-group-lifecycle#teardown#6 BatchGetCollectionGroup (200): $.collectionGroupDetails[0] expected None, got {'id': '8sh3tqaaaaaaaaaaaaaaaaaa', 'arn': 'arn:aws:aoss:us-west-2:000000000000:collection-group/8sh3tqaaaaaaaaaaaaaaaaaa', 'name': 'sim2real-probe-cg-372e2521', 'standbyReplicas': 'DISABLED', 'description': 'initial group description', 'tags': [], 'capacityLimits': {...}, 'numberOfCollections': 0, 'generation': 'CLASSIC'}; $.collectionGroupErrorDetails[0] expected {'errorCode': 'NOT_FOUND', 'errorMessage': 'The specified Collection Group is not found.', 'id': <requested id>}, got None.
Cause 2 — opensearchserverless-collection-in-group#teardown#8 BatchGetCollectionGroup (200): $.collectionGroupErrorDetails[0] expected {'errorCode': 'NOT_FOUND', 'errorMessage': 'The specified Collection Group is not found.', 'id': <requested id>}, got None.

## Steps to Reproduce
Boot the emulator and replay the aoss fixtures.
1 (Cause 1): CreateCollectionGroup(name=N) -> id I; DeleteCollectionGroup(id=I); BatchGetCollectionGroup(ids=[I]) -> expect 200 with collectionGroupDetails empty and one collectionGroupErrorDetails entry {errorCode NOT_FOUND, id I}; observe the deleted group returned as a detail and no error entry. Fixture opensearchserverless-collection-group-lifecycle#teardown#6.
2 (Cause 2): BatchGetCollectionGroup(ids=[<id with no live group>]) -> expect one collectionGroupErrorDetails entry with errorCode NOT_FOUND echoing the id; observe an empty error list. Fixture opensearchserverless-collection-in-group#teardown#8.
