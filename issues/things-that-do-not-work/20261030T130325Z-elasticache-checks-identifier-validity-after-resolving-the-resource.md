Issue-Source: trace-sweep-divergence
Issue-Trace: elasticache-replication-group-lifecycle-redis
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticache-replication-group-lifecycle-redis elasticache-global-datastore-workflow elasticache-user-and-user-group-rbac
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache|ModifyReplicationGroup|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticache|ModifyReplicationGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|ModifyReplicationGroup|4xx->4xx|status
Issue-Key: elasticache|CreateGlobalReplicationGroup|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticache|CreateGlobalReplicationGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|CreateGlobalReplicationGroup|4xx->4xx|status
Issue-Key: elasticache|DecreaseReplicaCount|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|TestFailover|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|StartMigration|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticache|StartMigration|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|StartMigration|4xx->4xx|status
Issue-Key: elasticache|ModifyUserGroup|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticache|ModifyUserGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|ModifyUserGroup|4xx->4xx|status
Issue-Key: elasticache|DeleteUserGroup|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticache|DeleteUserGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|DeleteUserGroup|4xx->4xx|status
Issue-Commit: 232d6cb39af83c8f3e5d0960e0729eaeb270e0f7

# ElastiCache checks identifier validity after resolving the resource, so refusals carry the wrong status, code and message

> _Note: this issue was identified against the code base at commit `232d6cb39af83c8f3e5d0960e0729eaeb270e0f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyReplicationGroup

## Description
SEVEN net-new root causes, all in the elasticache crate: the emulator orders input validation and state lookup differently from AWS, so a request that is BOTH syntactically invalid and names an absent resource is refused with the wrong HTTP status, the wrong error code and the wrong message. Five handlers skip the identifier-length rule and answer not-found; one applies a length rule AWS applies later (and with the wrong limit); one applies its own length rule before the lookup AWS does first. A fixer must close all seven. None of the four open elasticache issues covers these: 20261023T155010Z-elasticache-control-plane-is-unusable-end-to-end.md owns subnet-group/cluster lifecycle failures, 20261029T174147Z-elasticache-error-messages-diverge-from-aws-across-six.md owns six behavioral message-wording causes (and does state ModifyReplicationGroup's not-found WORDING, but not this precedence rule and none of the replay keys below).

Cause 1 of 7: ModifyReplicationGroup validates the ReplicationGroupId length before any lookup (fixture elasticache-replication-group-lifecycle-redis, step task#4).
AWS: Smithy: ModifyReplicationGroup models InvalidParameterValueException {client, httpError 400, awsQueryError InvalidParameterValue} and ReplicationGroupNotFoundFault {client, httpError 404, awsQueryError ReplicationGroupNotFoundFault}. Recorded behavior: for a ReplicationGroupId longer than 40 characters AWS answers 400 InvalidParameterValue with the wholly fixed text 'The parameter ReplicationGroupId is not a valid identifier because it is longer than 40 characters.' — the only variable parts of the template are the member name (ReplicationGroupId) and the limit (40), both fixed by the rule; the offending id is NOT echoed. The lookup never runs.
Emulator: 404 ReplicationGroupNotFoundFault, message 'ReplicationGroup <ReplicationGroupId> not found.' (id interpolated from the request) — it looked the id up first.
Why: a client that sent a malformed identifier is told the resource does not exist. That is a retryable/absent-resource signal rather than a permanent input error, so SDK retry and create-if-missing logic takes the wrong branch, and the caller is never told which member it got wrong.
Fix: the ModifyReplicationGroup handler's input validation in the elasticache crate must run the ReplicationGroupId identifier rule (max 40 characters) before the replication-group lookup and emit InvalidParameterValueException with AWS's template.
Traces: elasticache-replication-group-lifecycle-redis#task#4.

Cause 2 of 7: DecreaseReplicaCount reports non-existence where AWS reports the length rule (same fixture, step task#7).
AWS: Smithy: DecreaseReplicaCount models InvalidParameterValueException 400/InvalidParameterValue and ReplicationGroupNotFoundFault 404. Recorded behavior: for the same over-long id AWS answers 400 InvalidParameterValue with the wholly fixed text 'Replication Group ID cannot be longer than 40 characters.' (AWS words this operation's rule differently from ModifyReplicationGroup's; both are fixed text with no request values).
Emulator: 400 InvalidParameterValue — status and code correct — but the message is 'Replication group <id> does not exist.', an existence diagnosis with the request's id interpolated.
Why: the message is the only place AWS states the rule that was broken; a caller is told the group is missing when the real fault is the identifier it supplied, and the limit it must respect is never reported.
Fix: the DecreaseReplicaCount handler's input validation in the elasticache crate (apply the 40-character rule before the lookup and word the refusal as AWS does).
Traces: elasticache-replication-group-lifecycle-redis#task#7.

Cause 3 of 7: TestFailover applies the wrong limit and the wrong template (same fixture, step task#8).
AWS: Smithy: TestFailover models InvalidParameterValueException 400/InvalidParameterValue, ReplicationGroupNotFoundFault 404, NodeGroupNotFoundFault 404; NodeGroupId is constrained in the model (length 1-4, pattern ^\\d+$) while ReplicationGroupId is unconstrained, so the identifier rule is server-side. Recorded behavior: 400 InvalidParameterValue with the fixed text 'The parameter ReplicationGroupId is not a valid identifier because it is longer than 40 characters.'
Emulator: 400 InvalidParameterValue with 'Replication Group ID cannot be longer than 50 characters.' — it enforces a 50-character limit, not AWS's 40, and uses a template AWS does not use for this operation.
Why: the emulator ACCEPTS identifiers of 41-50 characters that AWS rejects, so a client validated against the emulator fails only in production; and when it does refuse, the limit it reports is wrong.
Fix: the TestFailover handler's ReplicationGroupId validation in the elasticache crate (limit 40, AWS's template).
Traces: elasticache-replication-group-lifecycle-redis#task#8.

Cause 4 of 7: StartMigration resolves the group before validating the id (same fixture, step task#9).
AWS: Smithy: StartMigration models InvalidParameterValueException 400/InvalidParameterValue and ReplicationGroupNotFoundFault 404/ReplicationGroupNotFoundFault. Recorded behavior: for the over-long id AWS answers 400 InvalidParameterValue with the fixed text 'Cluster ID cannot be longer than 50 characters.' (this operation's rule and wording differ again; no request value appears in it).
Emulator: 404 ReplicationGroupNotFoundFault, message 'ReplicationGroup <id> not found.'
Why: as cause 1 — a permanent input error is presented as an absent resource, and the applicable limit is never reported.
Fix: the StartMigration handler's input validation in the elasticache crate, ahead of the replication-group lookup.
Traces: elasticache-replication-group-lifecycle-redis#task#9.

Cause 5 of 7: ModifyUserGroup resolves the group before validating UserGroupId length (fixture elasticache-user-and-user-group-rbac, step task#6).
AWS: Smithy: ModifyUserGroup models InvalidParameterValueException {client, 400, InvalidParameterValue} and UserGroupNotFoundFault {client, 404, awsQueryError UserGroupNotFound}. Recorded behavior: for a UserGroupId at or over 40 characters AWS answers 400 InvalidParameterValue with the wholly fixed text 'User group id length must be less than 40.'
Emulator: 404 UserGroupNotFound, message 'User group <UserGroupId> not found.'
Why: as cause 1, on the RBAC surface: a caller with a too-long group id is told the group is absent and may then try to create it with the same invalid id.
Fix: the ModifyUserGroup handler's UserGroupId validation in the elasticache crate, ahead of the user-group lookup.
Traces: elasticache-user-and-user-group-rbac#task#6.

Cause 6 of 7: DeleteUserGroup has the same inverted order (same fixture, step task#7).
AWS: Smithy: DeleteUserGroup models InvalidParameterValueException 400/InvalidParameterValue, InvalidUserGroupStateFault 400/InvalidUserGroupState and UserGroupNotFoundFault 404/UserGroupNotFound. Recorded behavior: 400 InvalidParameterValue with the fixed text 'User group id length must be less than 40.'
Emulator: 404 UserGroupNotFound, message 'User group <UserGroupId> not found.'
Why: as cause 5; a delete that AWS refuses as malformed looks idempotently-already-gone on the emulator, which is the opposite conclusion for cleanup logic.
Fix: the DeleteUserGroup handler's UserGroupId validation in the elasticache crate, ahead of the lookup.
Traces: elasticache-user-and-user-group-rbac#task#7.

Cause 7 of 7: CreateGlobalReplicationGroup validates its own suffix BEFORE the lookup AWS performs first — the mirror image of causes 1-6 (fixture elasticache-global-datastore-workflow, step task#2).
AWS: Smithy: CreateGlobalReplicationGroup models ReplicationGroupNotFoundFault {client, httpError 404, awsQueryError ReplicationGroupNotFoundFault} and InvalidParameterValueException {client, 400, InvalidParameterValue}; both GlobalReplicationGroupIdSuffix and PrimaryReplicationGroupId are required. Recorded behavior: AWS resolves PrimaryReplicationGroupId first, so when it is absent the answer is 404 ReplicationGroupNotFoundFault with the message "Replication group '<PrimaryReplicationGroupId>' not found" — fixed text with the id single-quoted and no trailing period, the id taken from the request — even when the suffix is also too long.
Emulator: 400 InvalidParameterValue with 'Global Replication Group ID Suffix cannot be longer than 34 characters.' — it refuses on the suffix rule and never resolves the primary group.
Why: a client creating a global datastore against a mistyped primary group gets a suffix complaint instead of the not-found it must handle, and the two errors are handled on different code paths (400 permanent input error vs 404 missing dependency).
Fix: the CreateGlobalReplicationGroup handler in the elasticache crate must resolve PrimaryReplicationGroupId before applying the suffix-length rule, and word the not-found as AWS does.
Traces: elasticache-global-datastore-workflow#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up=true, 84/139 steps passed). Seven clusters, all verdict=mismatch, service=elasticache, stub_path empty, owned_by empty.

Cause 1 — elasticache-replication-group-lifecycle-redis#task#4, ModifyReplicationGroup: status expected 400, got 404; $.ErrorResponse.Error.Code: expected 'InvalidParameterValue', got 'ReplicationGroupNotFoundFault'; $.ErrorResponse.Error.Message: expected 'The parameter ReplicationGroupId is not a valid identifier because it is longer than 40 characters.', got 'ReplicationGroup <over-long id> not found.'
Cause 2 — elasticache-replication-group-lifecycle-redis#task#7, DecreaseReplicaCount: status 400 both sides, code matched; $.ErrorResponse.Error.Message: expected 'Replication Group ID cannot be longer than 40 characters.', got 'Replication group <over-long id> does not exist.'
Cause 3 — elasticache-replication-group-lifecycle-redis#task#8, TestFailover: status 400 both sides, code matched; $.ErrorResponse.Error.Message: expected 'The parameter ReplicationGroupId is not a valid identifier because it is longer than 40 characters.', got 'Replication Group ID cannot be longer than 50 characters.'
Cause 4 — elasticache-replication-group-lifecycle-redis#task#9, StartMigration: status expected 400, got 404; Code: expected 'InvalidParameterValue', got 'ReplicationGroupNotFoundFault'; Message: expected 'Cluster ID cannot be longer than 50 characters.', got 'ReplicationGroup <over-long id> not found.'
Cause 5 — elasticache-user-and-user-group-rbac#task#6, ModifyUserGroup: status expected 400, got 404; Code: expected 'InvalidParameterValue', got 'UserGroupNotFound'; Message: expected 'User group id length must be less than 40.', got 'User group <over-long id> not found.'
Cause 6 — elasticache-user-and-user-group-rbac#task#7, DeleteUserGroup: status expected 400, got 404; Code: expected 'InvalidParameterValue', got 'UserGroupNotFound'; Message: expected 'User group id length must be less than 40.', got 'User group <over-long id> not found.'
Cause 7 — elasticache-global-datastore-workflow#task#2, CreateGlobalReplicationGroup: status expected 404, got 400; Code: expected 'ReplicationGroupNotFoundFault', got 'InvalidParameterValue'; Message: expected "Replication group '<PrimaryReplicationGroupId>' not found", got 'Global Replication Group ID Suffix cannot be longer than 34 characters.'

In every quoted expectation the identifier shown in the recording is a per-run probe id (e.g. the 50+ character sim2real probe ids) and is NOT part of the requirement: AWS's length-rule messages contain no request value at all, and the not-found templates take the id verbatim from the request.

Model evidence: smithy_query(query="operation", operation="ModifyReplicationGroup,DecreaseReplicaCount,TestFailover,StartMigration,ModifyUserGroup,DeleteUserGroup,CreateGlobalReplicationGroup", model_path="services/service-com-amazonaws-elasticache-amazonelasticachev9/smithy-model/com-amazonaws-elasticache-amazonelasticachev9.json") -> InvalidParameterValueException {client, httpError 400, awsQueryError InvalidParameterValue} and ReplicationGroupNotFoundFault {client, httpError 404, awsQueryError ReplicationGroupNotFoundFault} on the replication-group operations; UserGroupNotFoundFault {client, httpError 404, awsQueryError UserGroupNotFound} and InvalidParameterValueException 400/InvalidParameterValue on ModifyUserGroup/DeleteUserGroup; TestFailover constrains only NodeGroupId (length 1-4, pattern ^\\d+$), so ReplicationGroupId length is a server-side rule; CreateGlobalReplicationGroup requires both GlobalReplicationGroupIdSuffix and PrimaryReplicationGroupId and models ReplicationGroupNotFoundFault 404.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Cause 1: ModifyReplicationGroup(ReplicationGroupId=<a 41+ character id that was never created>, ApplyImmediately=true). Expect 400 InvalidParameterValue, message 'The parameter ReplicationGroupId is not a valid identifier because it is longer than 40 characters.'; observe 404 ReplicationGroupNotFoundFault.
3. Cause 2: DecreaseReplicaCount with the same over-long id. Expect 400 InvalidParameterValue, message 'Replication Group ID cannot be longer than 40 characters.'; observe the non-existence message.
4. Cause 3: TestFailover(ReplicationGroupId=<same over-long id>, NodeGroupId="0001"). Expect 400 InvalidParameterValue, message 'The parameter ReplicationGroupId is not a valid identifier because it is longer than 40 characters.'; observe the 50-character message. Also send a 45-character id and confirm AWS rejects it (the emulator currently accepts it).
5. Cause 4: StartMigration(ReplicationGroupId=<same over-long id>, CustomerNodeEndpointList=[...]). Expect 400 InvalidParameterValue, message 'Cluster ID cannot be longer than 50 characters.'; observe 404 ReplicationGroupNotFoundFault.
6. Causes 5-6: ModifyUserGroup and DeleteUserGroup with a UserGroupId of 40+ characters that was never created. Expect 400 InvalidParameterValue, message 'User group id length must be less than 40.'; observe 404 UserGroupNotFound.
7. Cause 7: CreateGlobalReplicationGroup(GlobalReplicationGroupIdSuffix=<35+ characters>, PrimaryReplicationGroupId=<never created>). Expect 404 ReplicationGroupNotFoundFault, message "Replication group '<PrimaryReplicationGroupId>' not found"; observe 400 InvalidParameterValue about the suffix length.
8. Re-replay fixtures elasticache-replication-group-lifecycle-redis, elasticache-user-and-user-group-rbac and elasticache-global-datastore-workflow and confirm the named steps pass.
