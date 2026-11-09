Issue-Source: trace-sweep-divergence
Issue-Trace: elasticache-replication-group-lifecycle-redis
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticache-replication-group-lifecycle-redis elasticache-vvd-traj-replication-group-auth-and-network-reference-mistakes elasticache-vvd-traj-valkey-engine-version-and-node-type-mistakes
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache|ModifyReplicationGroupShardConfiguration|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|CreateReplicationGroup|4xx->2xx|$.CreateReplicationGroupResponse.CreateReplicationGroupResult.ReplicationGroup.NodeGroups.NodeGroup.PrimaryEndpoint.Address
Issue-Key: elasticache|DescribeReplicationGroups|4xx->2xx|$.DescribeReplicationGroupsResponse.DescribeReplicationGroupsResult.ReplicationGroups.ReplicationGroup.NodeGroups.NodeGroup.PrimaryEndpoint.Address
Issue-Related: 20261108T134359Z-elasticache-replication-group-refusals-never-raised-create-accepts.md
Issue-Commit: 622e0f603a4886f3f4d7e4980155df7d3d956f5b

# ElastiCache replication-group validation: wrong id-length limit in the refusal text, unvalidated security groups and phantom groups

> _Note: this issue was identified against the code base at commit `622e0f603a4886f3f4d7e4980155df7d3d956f5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyReplicationGroupShardConfiguration

## Description
3 net-new wire causes on the elasticache replication-group request-validation path; the replay marked none of the three clusters as owned by an open issue and none as stub-blocked. Related open issues: 20261108T152921Z-elasticache-modifyreplicationgroupshardconfigurationm-stub (filed as 'Implement elasticache ModifyReplicationGroupShardConfiguration: unimplemented scaffold stub') covers the operation's missing implementation, not the text of the refusal it raises before reaching the stub; and 20261108T134359Z-elasticache-replication-group-refusals-never-raised-create-accepts.md describes the same two missing refusals as causes 2 and 3 here - the replay did not report it as owning these steps, so they are re-filed with their replay keys and the two issues should be closed together.

Cause 1 of 3: ModifyReplicationGroupShardConfiguration states a 50-character limit on ReplicationGroupId where AWS's limit is 40. AWS: Smithy - ModifyReplicationGroupShardConfiguration requires ReplicationGroupId (plain String, no @length in the model) and models InvalidParameterValueException (awsQueryError code InvalidParameterValue, HTTP 400); the limit itself is recorded behavior only. Recorded behavior: an over-long ReplicationGroupId is refused HTTP 400 with Error.Code 'InvalidParameterValue' and the message 'Replication Group ID cannot be longer than 40 characters.' - that whole sentence is fixed text, with the limit being AWS's 40-character replication-group id limit, not a per-request value. Emulator: HTTP 400 with the same code but the message 'Replication Group ID cannot be longer than 50 characters.', i.e. it advertises (and enforces) a 50-character ceiling. Why: a client that trusts the refusal builds ids of 41-50 characters, which AWS refuses in production while the emulator accepts them; the error text it asserts on also never matches. Fix: the replication-group id length rule in elasticache's shared validation (and the message template it formats), so the bound is 40 and the refusal text is AWS's sentence. Traces: elasticache-replication-group-lifecycle-redis#task#4.

Cause 2 of 3: CreateReplicationGroup does not validate SecurityGroupIds against EC2 and creates the group anyway. AWS: Smithy - CreateReplicationGroup.SecurityGroupIds targets SecurityGroupIdsList and the operation models InvalidParameterValueException (code InvalidParameterValue, HTTP 400). Recorded behavior: a request naming a security group that does not exist in the account is refused HTTP 400 with Error.Type 'Sender', Error.Code 'InvalidParameterValue' and the message 'Some security group Id not recognized by EC2: securityGroupIds[[<security-group-ids>]], awsAccountId[<account-id>]' - the fixed text is 'Some security group Id not recognized by EC2: securityGroupIds[[', ']], awsAccountId[' and ']', and the variable parts are the unrecognized SecurityGroupIds from the request and the caller's account id; nothing is created. Emulator: HTTP 200 with a full ReplicationGroup body and no ErrorResponse - 'status: expected 400, got 200', '$.ErrorResponse: missing'. Why: a client that mistypes or stales a security group id is told its cluster is being created and ends up with a group whose network reachability is not what it asked for, while its InvalidParameterValue branch is unreachable. Fix: CreateReplicationGroup's validation in the elasticache handler - resolve every SecurityGroupIds member against EC2 security-group state before writing any replication-group state. Traces: elasticache-vvd-traj-replication-group-auth-and-network-reference-mistakes#task#11.

Cause 3 of 3: DescribeReplicationGroups answers 200 for a ReplicationGroupId that does not exist. AWS: Smithy - DescribeReplicationGroups models ReplicationGroupNotFoundFault (errorKind client, awsQueryError code ReplicationGroupNotFoundFault, HTTP 404). Recorded behavior: a describe naming a group that was never created (or whose create was refused) is refused HTTP 404 with Error.Type 'Sender', Error.Code 'ReplicationGroupNotFoundFault' and the message 'ReplicationGroup <replication-group-id> not found.' - fixed text 'ReplicationGroup ' and ' not found.', variable part the ReplicationGroupId from the request. Emulator: HTTP 200 with a ReplicationGroup body and no ErrorResponse - 'status: expected 404, got 200'. Why: a client that uses DescribeReplicationGroups as its existence check - before create, after delete, or while polling teardown - reads success for a group that is not there, so it never detects deletion and never retries creation. Fix: the DescribeReplicationGroups handler's lookup in elasticache - a ReplicationGroupId that matches nothing must raise ReplicationGroupNotFoundFault (404) instead of returning a synthesised 200. Traces: elasticache-vvd-traj-replication-group-auth-and-network-reference-mistakes#task#13.

## Evidence
All quotes are from the wire replay of corpus simparity-wire-aws-v1 (verdict: mismatch). Ids, account ids and endpoint hostnames in the payloads are per-run values that fill AWS's templates; the statuses, error codes and the templates' fixed text (including the 40-character limit) are the requirement.

Cause 1 - fixture elasticache-replication-group-lifecycle-redis, step task#4, operation ModifyReplicationGroupShardConfiguration; expected status 400, actual status 400:
  $.ErrorResponse.Error.Message: value_mismatch (expected 'Replication Group ID cannot be longer than 40 characters.', got 'Replication Group ID cannot be longer than 50 characters.')

Cause 2 - fixture elasticache-vvd-traj-replication-group-auth-and-network-reference-mistakes, step task#11, operation CreateReplicationGroup (cluster also covers elasticache-vvd-traj-valkey-engine-version-and-node-type-mistakes#task#5); expected status 400, actual status 200:
  status: expected 400, got 200
  $.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'InvalidParameterValue', 'Message': 'Some security group Id not recognized by EC2: securityGroupIds[[<sg-id>]], awsAccountId[<account-id>]'}}, got None)

Cause 3 - same fixture, step task#13, operation DescribeReplicationGroups (cluster also covers elasticache-vvd-traj-valkey-engine-version-and-node-type-mistakes#task#9); expected status 404, actual status 200:
  status: expected 404, got 200
  $.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'ReplicationGroupNotFoundFault', 'Message': 'ReplicationGroup <replication-group-id> not found.'}}, got None)

Smithy cited: ModifyReplicationGroupShardConfiguration errors include InvalidParameterValueException (code InvalidParameterValue, 400); CreateReplicationGroup takes SecurityGroupIds (SecurityGroupIdsList) and models InvalidParameterValueException (400); DescribeReplicationGroups models ReplicationGroupNotFoundFault (code ReplicationGroupNotFoundFault, 404).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (git_clone, then replay_wire_corpus).
2. Cause 1: call elasticache ModifyReplicationGroupShardConfiguration with a ReplicationGroupId longer than the AWS limit - observe HTTP 400 InvalidParameterValue with the message naming a 50-character limit where AWS names 40. Replay fixture elasticache-replication-group-lifecycle-redis step task#4.
3. Cause 2: call CreateReplicationGroup with a valid id/description/node type and SecurityGroupIds naming a security group that does not exist in the account - observe HTTP 200 and a ReplicationGroup body where AWS answers 400 InvalidParameterValue 'Some security group Id not recognized by EC2: securityGroupIds[[<sg-id>]], awsAccountId[<account-id>]' and creates nothing. Replay fixture elasticache-vvd-traj-replication-group-auth-and-network-reference-mistakes step task#11 (cluster also covers elasticache-vvd-traj-valkey-engine-version-and-node-type-mistakes step task#5).
4. Cause 3: call DescribeReplicationGroups for a ReplicationGroupId that was never successfully created - observe HTTP 200 where AWS answers 404 ReplicationGroupNotFoundFault 'ReplicationGroup <id> not found.'. Step task#13 of the same fixture (cluster also covers elasticache-vvd-traj-valkey-engine-version-and-node-type-mistakes step task#9).
