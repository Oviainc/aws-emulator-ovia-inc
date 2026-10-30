Issue-Source: trace-sweep-divergence
Issue-Trace: elasticache-cache-parameter-group-workflow
Issue-Step: task#10
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticache-cache-parameter-group-workflow elasticache-error-invalid-engine-create-user-group elasticache-resource-tagging-across-types elasticache-serverless-cache-lifecycle elasticache-snapshot-backup-and-copy elasticache-user-group-create-describe-and-premature-delete
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache|DescribeEngineDefaultParameters|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|ResetCacheParameterGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|CreateUserGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|ListTagsForResource|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|CreateServerlessCache|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticache|CreateSnapshot|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Related: 20261030T130325Z-elasticache-checks-identifier-validity-after-resolving-the-resource.md
Issue-Commit: 232d6cb39af83c8f3e5d0960e0729eaeb270e0f7

# ElastiCache refusal messages are the emulator's own wording on five validation paths and two resolution paths

> _Note: this issue was identified against the code base at commit `232d6cb39af83c8f3e5d0960e0729eaeb270e0f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEngineDefaultParameters

## Description
SEVEN net-new root causes, all in the elasticache crate. In each one the emulator reaches the right decision and emits the right HTTP status and error code, but Error.Message is its own wording rather than AWS's — and in two of them the wording reveals that the wrong CHECK fired (a default-group check instead of an identifier-syntax check; an enum check instead of a capability check). The message is where AWS states which rule was broken, so a client, SDK waiter or test that matches on it behaves differently. A fixer must close all seven. The open behavioral issue 20261029T174147Z-elasticache-error-messages-diverge-from-aws-across-six.md states the same requirement for causes 4 and 6 below from the behavioral corpus, but carries NONE of the wire replay keys below (owned_by was empty for every cluster here), so these wire steps are not gated by it; the other open elasticache issues (20261023T155010Z-..., 20261026T234107Z-..., 20261029T174147Z-elasticache-createserverlesscache-with-an-out-of-range-cacheusagelimits-bound.md) do not touch these paths.

Cause 1 of 7: DescribeEngineDefaultParameters' unknown-family refusal (fixture elasticache-cache-parameter-group-workflow, step task#10).
AWS: Smithy: DescribeEngineDefaultParameters models InvalidParameterValueException {client, httpError 400, awsQueryError InvalidParameterValue} and InvalidParameterCombinationException {client, 400, InvalidParameterCombination}; CacheParameterGroupFamily is a required unconstrained String, so the family vocabulary is enforced server-side and the message is the only diagnosis. Recorded behavior for the text: the fixed text 'CacheParameterGroupFamily ' + the request's CacheParameterGroupFamily + ' is not a valid parameter group family.' — the family name is the one variable part.
Emulator: 400 InvalidParameterValue with 'Invalid value <family> for CacheParameterGroupFamily.' — a generic enum-rejection template that names the member but not the rule.
Why: a client cannot distinguish an unsupported family from any other bad enum value, and message-matching fixtures do not recognise the refusal.
Fix: the DescribeEngineDefaultParameters handler's family validation in the elasticache crate.
Traces: elasticache-cache-parameter-group-workflow#task#10.

Cause 2 of 7: ResetCacheParameterGroup runs the default-group check before the identifier-syntax check (same fixture, step task#12).
AWS: Smithy: ResetCacheParameterGroup models InvalidParameterValueException 400/InvalidParameterValue, CacheParameterGroupNotFoundFault 404/CacheParameterGroupNotFound and InvalidCacheParameterGroupStateFault 400/InvalidCacheParameterGroupState; CacheParameterGroupName is a required unconstrained String. Recorded behavior: a CacheParameterGroupName that is not a valid identifier is refused with 400 InvalidParameterValue and the wholly fixed text 'The parameter CacheParameterGroupName is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens.' — the syntax rule is applied before the name is interpreted as a group (so a dotted default-group-looking name is refused as malformed, not as a default group).
Emulator: 400 InvalidParameterValue with 'Cannot modify a default parameter group.' — it classified the name as a default group and never applied the identifier rule.
Why: the caller is told it may not touch a default group when the real fault is the identifier syntax; it would 'fix' the request by choosing another group name rather than a legal identifier, and AWS's stated rule (letters/digits/hyphens, no trailing or doubled hyphen) is never reported.
Fix: the ResetCacheParameterGroup handler's input validation in the elasticache crate must apply the CacheParameterGroupName identifier rule before the default-group check, with AWS's fixed text.
Traces: elasticache-cache-parameter-group-workflow#task#12.

Cause 3 of 7: CreateUserGroup's unsupported-engine refusal reports the enum instead of the capability rule (fixture elasticache-error-invalid-engine-create-user-group, step task#1).
AWS: Smithy: CreateUserGroup models InvalidParameterValueException {client, 400, InvalidParameterValue}. Recorded behavior for the text: the fixed text 'Access control options are available for Redis version 6.0 onwards.' — wholly fixed, with no request value: AWS answers the capability rule (RBAC user groups exist only for Redis 6.0+) rather than enumerating allowed engines.
Emulator: 400 InvalidParameterValue with 'Invalid value <Engine> for Engine: must be one of redis | valkey.' — it echoes the request's engine and lists its own allowed set.
Why: the client is given a different rule than the one AWS applies (engine enum vs engine-version capability), so it draws the wrong conclusion about what would make the request valid, and the emulator echoes caller input into a message where AWS does not.
Fix: the CreateUserGroup handler's engine validation in the elasticache crate.
Traces: elasticache-error-invalid-engine-create-user-group#task#1.

Cause 4 of 7: the tagging operations' InvalidARN message echoes the input (fixture elasticache-resource-tagging-across-types, step task#7).
AWS: Smithy: ListTagsForResource models InvalidARNFault {client, httpError 400, awsQueryError InvalidARN}. Recorded behavior for the text: for a ResourceName that is not ARN-shaped the message is entirely fixed text with NO variable part — 'Input ARN string does not have 7 components.' — AWS reports the structural rule it applied.
Emulator: 400 InvalidARN with '<ResourceName> is not a valid ElastiCache resource ARN.'
Why: the client cannot tell which ARN rule was broken, and the emulator reflects caller input into an error where AWS does not. (Same requirement as cause 5 of the open behavioral issue 20261029T174147Z-elasticache-error-messages-diverge-from-aws-across-six.md, which does not carry this wire step.)
Fix: the shared ARN resolution in the elasticache crate's state module reached from services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/list_tags_for_resource.rs (the TagTargetRejection::InvalidArn path), which AddTagsToResource shares.
Traces: elasticache-resource-tagging-across-types#task#7.

Cause 5 of 7: CreateServerlessCache's unsupported-engine refusal (fixture elasticache-serverless-cache-lifecycle, step task#3).
AWS: Smithy: CreateServerlessCache models InvalidParameterValueException {client, 400, InvalidParameterValue}. Recorded behavior for the text: the wholly fixed text 'Specified engine does not support serverless cache.' — AWS answers the serverless-capability rule and does not echo the engine.
Emulator: 400 InvalidParameterValue with 'Invalid value <Engine> for Engine: must be one of memcached | redis | valkey.' — the generic enum template again, and it even names memcached, which does not support serverless cache.
Why: the message states a rule AWS does not apply here and implies a valid engine set that is wrong for this operation, so a client retries with an engine AWS also refuses.
Fix: the CreateServerlessCache handler's engine validation in the elasticache crate (distinguish 'not a known engine' from 'engine does not support serverless').
Traces: elasticache-serverless-cache-lifecycle#task#3.

Cause 6 of 7: CreateSnapshot's CacheClusterNotFound message names the shape, not AWS's prose (fixture elasticache-snapshot-backup-and-copy, step task#3).
AWS: Smithy: CreateSnapshot models CacheClusterNotFoundFault {client, httpError 404, awsQueryError CacheClusterNotFound}. Recorded behavior for the text: the fixed text 'Cache cluster not found: ' followed by the request's CacheClusterId.
Emulator: 404 CacheClusterNotFound with 'CacheCluster not found: <CacheClusterId>' — the shape name instead of AWS's prose.
Why: message-matching clients and fixtures do not recognise the refusal. (Same requirement as cause 4 of the open behavioral issue named above, which does not carry this wire step.)
Fix: the CreateSnapshot handler's cluster-resolution message in the elasticache crate.
Traces: elasticache-snapshot-backup-and-copy#task#3.

Cause 7 of 7: DeleteUserGroup's wrong-state refusal does not state the deletable states (fixture elasticache-user-group-create-describe-and-premature-delete, step teardown#3).
AWS: Smithy: DeleteUserGroup models InvalidUserGroupStateFault {client, httpError 400, awsQueryError InvalidUserGroupState} and InvalidParameterValueException {client, 400, InvalidParameterValue}. Recorded behavior for the text: the fixed text 'User group ' + the request's UserGroupId + ' has status ' + the group's CURRENT status from emulator/service state + '. Only active or create-failed user groups can be deleted.' — two variable parts (the id from the request, the status from state) and a fixed tail that names the admissible states.
Emulator: same status and code, but 'User group <id> is in status <status>; it must be active to be changed.' — a generic 'must be active to be changed' sentence that omits create-failed and is not delete-specific.
Why: a client waiting to delete a user group is told only 'must be active', so it waits forever on a create-failed group that AWS would let it delete; the retry condition AWS publishes is lost.
Fix: the DeleteUserGroup handler's state-guard message in the elasticache crate (it must be delete-specific and name both admissible states).
Traces: elasticache-user-group-create-describe-and-premature-delete#teardown#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up=true, 84/139 steps passed). Seven clusters, all verdict=mismatch, service=elasticache, stub_path empty, owned_by empty. In all seven the HTTP status and Error.Code matched; only $.ErrorResponse.Error.Message diverged.

Cause 1 — elasticache-cache-parameter-group-workflow#task#10, DescribeEngineDefaultParameters, 400/400:
  expected 'CacheParameterGroupFamily <family> is not a valid parameter group family.', got 'Invalid value <family> for CacheParameterGroupFamily.' (the recording's family token 'not-a-real-family' is the probe's value, not part of the template).
Cause 2 — elasticache-cache-parameter-group-workflow#task#12, ResetCacheParameterGroup, 400/400:
  expected 'The parameter CacheParameterGroupName is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens.', got 'Cannot modify a default parameter group.'
Cause 3 — elasticache-error-invalid-engine-create-user-group#task#1, CreateUserGroup, 400/400:
  expected 'Access control options are available for Redis version 6.0 onwards.', got 'Invalid value <Engine> for Engine: must be one of redis | valkey.' (recording's engine token 'notarealengine').
Cause 4 — elasticache-resource-tagging-across-types#task#7, ListTagsForResource, 400/400:
  expected 'Input ARN string does not have 7 components.', got '<ResourceName> is not a valid ElastiCache resource ARN.' (recording's ResourceName 'not-an-arn').
Cause 5 — elasticache-serverless-cache-lifecycle#task#3, CreateServerlessCache, 400/400:
  expected 'Specified engine does not support serverless cache.', got 'Invalid value <Engine> for Engine: must be one of memcached | redis | valkey.' (recording's engine token 'not-a-real-engine').
Cause 6 — elasticache-snapshot-backup-and-copy#task#3, CreateSnapshot, 404/404:
  expected 'Cache cluster not found: <CacheClusterId>', got 'CacheCluster not found: <CacheClusterId>' (the id is the request's and is the template's only variable part).
Cause 7 — elasticache-user-group-create-describe-and-premature-delete#teardown#3, DeleteUserGroup, 400/400:
  expected 'User group <UserGroupId> has status <current status>. Only active or create-failed user groups can be deleted.', got 'User group <UserGroupId> is in status <current status>; it must be active to be changed.' (the recording's id and the status 'creating' are the request's value and the state's value respectively).

Model evidence: smithy_query(query="operation", operation="DescribeEngineDefaultParameters,ResetCacheParameterGroup,CreateUserGroup,ListTagsForResource,CreateServerlessCache,CreateSnapshot,DeleteUserGroup", model_path="services/service-com-amazonaws-elasticache-amazonelasticachev9/smithy-model/com-amazonaws-elasticache-amazonelasticachev9.json") -> DescribeEngineDefaultParameters errors InvalidParameterValueException 400/InvalidParameterValue + InvalidParameterCombinationException 400/InvalidParameterCombination, CacheParameterGroupFamily required unconstrained String; ResetCacheParameterGroup errors CacheParameterGroupNotFoundFault 404/CacheParameterGroupNotFound, InvalidCacheParameterGroupStateFault 400/InvalidCacheParameterGroupState, InvalidParameterValueException 400/InvalidParameterValue; DeleteUserGroup errors InvalidUserGroupStateFault 400/InvalidUserGroupState, InvalidParameterValueException 400/InvalidParameterValue, UserGroupNotFoundFault 404/UserGroupNotFound. The message texts are not modelled; they are recorded behavior.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Cause 1: DescribeEngineDefaultParameters(CacheParameterGroupFamily="<not a supported family>"). Expect 400 InvalidParameterValue, message 'CacheParameterGroupFamily <family> is not a valid parameter group family.'; observe 'Invalid value <family> for CacheParameterGroupFamily.'
3. Cause 2: ResetCacheParameterGroup(CacheParameterGroupName="<a name that is not a legal identifier, e.g. one containing a dot>", ResetAllParameters=true). Expect 400 InvalidParameterValue with AWS's identifier-rule sentence; observe 'Cannot modify a default parameter group.'
4. Cause 3: CreateUserGroup(UserGroupId=<fresh>, Engine="<not a supported engine>", UserIds=[default]). Expect 400 InvalidParameterValue, message 'Access control options are available for Redis version 6.0 onwards.'; observe the enum message.
5. Cause 4: ListTagsForResource(ResourceName="not-an-arn"). Expect 400 InvalidARN, message 'Input ARN string does not have 7 components.'; observe the input echoed back.
6. Cause 5: CreateServerlessCache(ServerlessCacheName=<fresh>, Engine="<not a serverless-capable engine>"). Expect 400 InvalidParameterValue, message 'Specified engine does not support serverless cache.'; observe the enum message naming memcached.
7. Cause 6: CreateSnapshot(CacheClusterId=<never created>, SnapshotName=<fresh>). Expect 404 CacheClusterNotFound, message 'Cache cluster not found: <CacheClusterId>'; observe 'CacheCluster not found: <CacheClusterId>'.
8. Cause 7: CreateUserGroup and, while its Status is still creating, DeleteUserGroup the same UserGroupId. Expect 400 InvalidUserGroupState, message 'User group <id> has status creating. Only active or create-failed user groups can be deleted.'; observe 'User group <id> is in status creating; it must be active to be changed.'
9. Re-replay fixtures elasticache-cache-parameter-group-workflow, elasticache-error-invalid-engine-create-user-group, elasticache-resource-tagging-across-types, elasticache-serverless-cache-lifecycle, elasticache-snapshot-backup-and-copy and elasticache-user-group-create-describe-and-premature-delete and confirm the named steps pass.
