Issue-Source: trace-sweep-divergence
Issue-Trace: neptune-db-cluster-lifecycle-and-custom-endpoint
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: neptune-db-cluster-lifecycle-and-custom-endpoint neptune-db-instance-and-engine-rejection-classes neptune-db-cluster-snapshot-lifecycle neptune-db-subnet-group-lifecycle neptune-event-subscription-and-events-by-source
Issue-Shape-Id: com.amazonaws.neptune#AmazonRDSv19
Issue-Key: neptune|CreateDBCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: neptune|DeleteDBSubnetGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: neptune|CreateDBClusterSnapshot|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: neptune|CreateDBClusterParameterGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: neptune|CreateDBSubnetGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: neptune|CreateDBSubnetGroup|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: neptune|CreateEventSubscription|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: c5b0e480c5d08ce7c34e019fd56b1aa2c0c8407c

# Neptune refusals are worded and classified differently from the service's own error responses

> _Note: this issue was identified against the code base at commit `c5b0e480c5d08ce7c34e019fd56b1aa2c0c8407c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBSubnetGroup

## Description
Seven Neptune refusal paths reach the right HTTP status but word the Error.Message in the emulator's own phrasing rather than the service's, and one of them also classifies the refusal under the wrong Error.Code. The error message and code are part of the contract, so each is a defect. THIS ISSUE HAS 7 CAUSES — all seven must be closed before submitting.

Cause 1 of 7: CreateDBCluster duplicate-identifier message appends the identifier AWS does not include. AWS: 400 with Error.Code DBClusterAlreadyExistsFault (Smithy CreateDBCluster error DBClusterAlreadyExistsFault, httpError 400) and the fixed message 'DB Cluster already exists' — the whole message is fixed text, with no variable part. Emulator: the same status and code but the message appends ': <cluster identifier>'. Why: a client matching the documented sentence, or displaying it, sees a message the service never emits. Fix: the CreateDBCluster handler's already-exists refusal. Traces: neptune-db-cluster-lifecycle-and-custom-endpoint#task#6, neptune-db-instance-and-engine-rejection-classes#task#5.

Cause 2 of 7: DeleteDBSubnetGroup's in-use refusal says 'database instance' where the blocking resource is a cluster. AWS: 400 InvalidDBSubnetGroupStateFault with the message "Cannot delete the subnet group '<subnet group name>' because at least one database cluster: <cluster identifier> is still using it." — fixed text around two variables (the subnet group name from the request and the blocking resource's identifier from state), and the resource noun names what is actually holding the group. Emulator: identical except the noun is 'database instance' when a DB cluster is the holder. Why: the message misidentifies which resource must be deleted first, so an operator deletes the wrong thing. Fix: the DeleteDBSubnetGroup handler's in-use refusal, which must name the holder kind it found in state. Traces: neptune-db-cluster-lifecycle-and-custom-endpoint#task#7.

Cause 3 of 7: CreateDBClusterSnapshot duplicate-identifier message is the emulator's own sentence. AWS: 400 DBClusterSnapshotAlreadyExistsFault (modelled, httpError 400) worded 'Cannot create the cluster snapshot because one with the identifier <snapshot identifier> already exists.' — fixed text around the request's snapshot identifier. Emulator: 'DB cluster snapshot already exists: <snapshot identifier>'. Why: same contract break as cause 1 for snapshot creation. Fix: the CreateDBClusterSnapshot handler's already-exists refusal. Traces: neptune-db-cluster-snapshot-lifecycle#task#2.

Cause 4 of 7: CreateDBClusterParameterGroup's invalid-name refusal is reworded as a generic constraint sentence. AWS: 400 with the message 'Invalid cluster parameter group name:  <supplied name>' — fixed prefix (including its double space) plus the rejected name from the request. Emulator: a different sentence, "Invalid value '<name>' for DBClusterParameterGroupName: it must contain from 1 to 255 letters, numbers, or hyphens, ...". Why: a client that keys recovery off the message text does not recognise the refusal. Fix: the CreateDBClusterParameterGroup handler's name validation. Traces: neptune-db-instance-and-engine-rejection-classes#task#6.

Cause 5 of 7: CreateDBSubnetGroup duplicate-name message is the emulator's own sentence. AWS: 400 DBSubnetGroupAlreadyExistsFault (modelled, awsQueryError code DBSubnetGroupAlreadyExists, httpError 400) worded "The DB subnet group '<subnet group name>' already exists." — fixed text quoting the requested name. Emulator: 'DB subnet group already exists: <name>'. Fix: the CreateDBSubnetGroup handler's already-exists refusal. Traces: neptune-db-subnet-group-lifecycle#task#3, neptune-db-subnet-group-lifecycle#task#4.

Cause 6 of 7: CreateDBSubnetGroup with an unusable subnet list is refused under the wrong Error.Code. AWS: the recorded refusal of a subnet list that Neptune cannot accept is 400 with Error.Code InvalidParameterValue and the message 'Some input subnets in :[<subnet ids>] are invalid.' — fixed text around the bracketed list of subnet ids from the request. Emulator: 400 with Error.Code DBSubnetGroupDoesNotCoverEnoughAZs and a wholly different sentence about Availability Zone coverage. Why: the code is the part clients branch on — an SDK's typed error handling catches DBSubnetGroupDoesNotCoverEnoughAZs where AWS raises the generic InvalidParameterValue, so the error is routed to the wrong handler. Fix: the CreateDBSubnetGroup handler's subnet validation and the error it selects. Traces: neptune-db-subnet-group-lifecycle#task#5.

Cause 7 of 7: CreateEventSubscription's unknown-category refusal is reworded. AWS: 404 SubscriptionCategoryNotFound (Smithy CreateEventSubscription error SubscriptionCategoryNotFoundFault, httpError 404) worded 'Category : <category> not found for source type <source type>.' — fixed text around the rejected category and the requested SourceType. Emulator: same status and code but the message becomes '<category> is not a published event category of <source type> (its categories are ...)'. Fix: the CreateEventSubscription handler's category validation. Traces: neptune-event-subscription-and-events-by-source#task#6.

## Evidence
Replay of simparity-wire-aws-v1; each cluster's HTTP status matched, the body did not.
Cause 1: $.ErrorResponse.Error.Message expected 'DB Cluster already exists', got 'DB Cluster already exists: <cluster id>' (2 affected steps).
Cause 2: expected "Cannot delete the subnet group '<sng>' because at least one database cluster: <cluster id> is still using it.", got the same sentence with 'database instance'.
Cause 3: expected 'Cannot create the cluster snapshot because one with the identifier <snap id> already exists.', got 'DB cluster snapshot already exists: <snap id>'.
Cause 4: expected 'Invalid cluster parameter group name:  <name>', got "Invalid value '<name>' for DBClusterParameterGroupName: it must contain from 1 to 255 letters, numbers, or hyphens, the first character must be a letter, and it cannot end with a hyphen or contain two consecutive hyphens".
Cause 5: expected "The DB subnet group '<sng>' already exists.", got 'DB subnet group already exists: <sng>' (2 affected steps).
Cause 6: $.ErrorResponse.Error.Code expected 'InvalidParameterValue', got 'DBSubnetGroupDoesNotCoverEnoughAZs'; Message expected 'Some input subnets in :[<subnet ids>] are invalid.', got 'the request names 1 distinct subnet(s): a DB subnet group must cover at least two Availability Zones, so it must name at least two subnets'.
Cause 7: expected 'Category : <category> not found for source type <source type>.', got '<category> is not a published event category of <source type> (its categories are ...)'.

## Steps to Reproduce
1. Replay neptune-db-cluster-lifecycle-and-custom-endpoint steps task#6 (duplicate cluster create) and task#7 (delete a subnet group still used by a cluster).
2. Replay neptune-db-cluster-snapshot-lifecycle step task#2 (duplicate snapshot identifier).
3. Replay neptune-db-instance-and-engine-rejection-classes step task#6 (malformed cluster parameter group name).
4. Replay neptune-db-subnet-group-lifecycle steps task#3 (duplicate subnet group) and task#5 (unusable subnet list).
5. Replay neptune-event-subscription-and-events-by-source step task#6 (unknown event category).
