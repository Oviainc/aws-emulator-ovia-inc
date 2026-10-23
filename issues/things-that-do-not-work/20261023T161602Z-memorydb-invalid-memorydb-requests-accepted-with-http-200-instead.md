Issue-Source: trace-sweep-divergence
Issue-Trace: memorydb-acl-lifecycle-and-state-machine
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: memorydb-acl-lifecycle-and-state-machine memorydb-default-user-and-open-access-guards memorydb-cluster-and-multiregion-request-validation memorydb-cluster-and-snapshot-not-found-envelopes memorydb-describe-maxresults-bounds memorydb-parameter-group-lifecycle memorydb-parameter-update-and-reset-rules memorydb-user-lifecycle-and-credential-rules
Issue-Shape-Id: com.amazonaws.memorydb#AmazonMemoryDB
Issue-Key: memorydb|CreateACL|4xx->2xx|$.__type
Issue-Key: memorydb|CreateACL|4xx->2xx|$.message
Issue-Key: memorydb|CreateACL|4xx->2xx|status
Issue-Key: memorydb|CreateCluster|4xx->2xx|$.__type
Issue-Key: memorydb|CreateCluster|4xx->2xx|$.message
Issue-Key: memorydb|CreateCluster|4xx->2xx|status
Issue-Key: memorydb|DescribeUsers|4xx->2xx|$.__type
Issue-Key: memorydb|DescribeUsers|4xx->2xx|$.message
Issue-Key: memorydb|DescribeUsers|4xx->2xx|status
Issue-Key: memorydb|CreateParameterGroup|4xx->2xx|$.__type
Issue-Key: memorydb|CreateParameterGroup|4xx->2xx|$.message
Issue-Key: memorydb|CreateParameterGroup|4xx->2xx|status
Issue-Key: memorydb|CreateUser|4xx->2xx|$.__type
Issue-Key: memorydb|CreateUser|4xx->2xx|$.message
Issue-Key: memorydb|CreateUser|4xx->2xx|status
Issue-Commit: 205916506edbd7602d3640584e73a4ccd0a8f366

# Invalid MemoryDB requests accepted with HTTP 200 instead of rejected

> _Note: this issue was identified against the code base at commit `205916506edbd7602d3640584e73a4ccd0a8f366`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateACL

## Description
Five causes, one shared fault: request-level guards that AWS enforces are missing, so the emulator accepts the request and answers HTTP 200 where AWS answers HTTP 400 with a modeled fault. These are the most damaging class of divergence here, because the emulator creates state AWS would have refused. All five causes must be closed before submitting.

Cause 1 of 5: CreateACL referencing a user that does not exist. AWS: HTTP 400 (recorded) with __type 'UserNotFoundFault', recorded message 'User <name> not found.' (Smithy: CreateACL errors include UserNotFoundFault, doc 'The specified user does not exist.'; input UserNames targets UserNameListInput). Emulator: HTTP 200 — the ACL is created with a dangling member. Why: a client that relies on CreateACL failing for an unknown user silently gets an ACL it believes validated, and the downstream cluster attach behaves differently from AWS. Fix: the CreateACL handler's UserNames existence validation, before ACL state is written. Traces: memorydb-acl-lifecycle-and-state-machine#task#5.

Cause 2 of 5: CreateCluster naming a parameter group that does not exist. AWS: HTTP 400 with __type 'ParameterGroupNotFoundFault' (Smithy: CreateCluster errors include ParameterGroupNotFoundFault), recorded message 'ParameterGroup not found: <name>'. Emulator: HTTP 200 — the cluster is created. Fix: the CreateCluster handler's ParameterGroupName existence validation. Traces: memorydb-cluster-and-multiregion-request-validation#task#16.

Cause 3 of 5: DescribeUsers with both UserName and Filters supplied. AWS: HTTP 400 with __type 'InvalidParameterCombinationException' (Smithy: DescribeUsers declares InvalidParameterCombinationException; doc 'The specified parameter combination is not valid.'), recorded message "User name can't be specified at the same time as filters.". Emulator: HTTP 200. Why: a client relying on the mutual exclusion gets a silently different result set instead of the documented fault. Fix: DescribeUsers input validation. Traces: memorydb-describe-maxresults-bounds#task#4.

Cause 4 of 5: CreateParameterGroup with a Family that is not a real parameter-group family. AWS: HTTP 400 __type 'InvalidParameterValueException', recorded message 'ParameterGroupFamily memorydb_redis99 is not a valid parameter group family.'. Emulator: HTTP 200 — the group is created against a nonexistent family. Fix: CreateParameterGroup validation against the engine/family catalog. Traces: memorydb-parameter-group-lifecycle#task#3.

Cause 5 of 5: CreateUser with a malformed access string. AWS: HTTP 400 __type 'InvalidParameterValueException', recorded message 'The access-string is invalid.' (Smithy: CreateUserRequest.AccessString, doc 'Access permissions string used for this user.'; no syntax constraint in the model, so the syntax rule is recorded behavior only). Emulator: HTTP 200 — the user is created with an access string AWS would reject. Fix: the CreateUser handler's access-string parser/validator. Traces: memorydb-user-lifecycle-and-credential-rules#task#4.

## Evidence
Cause 1: expected 400, got 200; $.__type missing (expected 'UserNotFoundFault'), $.message missing (memorydb-acl-lifecycle-and-state-machine task#5; cluster also covers memorydb-default-user-and-open-access-guards task#6). Cause 2: expected 400, got 200; $.__type missing (expected 'ParameterGroupNotFoundFault') (memorydb-cluster-and-multiregion-request-validation task#16; also memorydb-cluster-and-snapshot-not-found-envelopes task#9). Cause 3: expected 400, got 200; $.__type missing (expected 'InvalidParameterCombinationException') (memorydb-describe-maxresults-bounds task#4). Cause 4: expected 400, got 200; $.__type missing (expected 'InvalidParameterValueException') (memorydb-parameter-group-lifecycle task#3; also memorydb-parameter-update-and-reset-rules task#14). Cause 5: expected 400, got 200; $.__type missing (expected 'InvalidParameterValueException') (memorydb-user-lifecycle-and-credential-rules task#4).

## Steps to Reproduce
Replay trace set simparity-wire-aws-v1. (1) CreateACL with UserNames naming a user that was never created — expect 400/UserNotFoundFault, emulator returns 200. (2) CreateCluster with ParameterGroupName naming a nonexistent group — expect 400/ParameterGroupNotFoundFault, emulator returns 200. (3) DescribeUsers with UserName and Filters both set — expect 400/InvalidParameterCombinationException, emulator returns 200. (4) CreateParameterGroup with Family='memorydb_redis99' — expect 400/InvalidParameterValueException, emulator returns 200. (5) CreateUser with an invalid AccessString — expect 400/InvalidParameterValueException, emulator returns 200. Fixtures/steps: memorydb-acl-lifecycle-and-state-machine#task#5, memorydb-cluster-and-multiregion-request-validation#task#16, memorydb-describe-maxresults-bounds#task#4, memorydb-parameter-group-lifecycle#task#3, memorydb-user-lifecycle-and-credential-rules#task#4.
