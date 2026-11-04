Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.cache-parameter-group-lifecycle elasticache.concurrent-duplicate-parameter-group elasticache.parameter-group-not-found-semantics elasticache.replica-count-argument-validation elasticache.slowlog-parameters-round-trip-and-reject-negative-length elasticache.snapshot-not-found-semantics elasticache.tags-invalid-arn elasticache.user-access-string-malformed-rejected
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.cache-parameter-group-lifecycle|_assert_error#17|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: elasticache|DeleteCacheParameterGroup|CacheParameterGroupNotFound
Issue-Key: elasticache.parameter-group-not-found-semantics|_assert_error#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: elasticache.replica-count-argument-validation|_assert_error#6|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: elasticache.slowlog-parameters-round-trip-and-reject-negative-length|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: elasticache.snapshot-not-found-semantics|_assert_error#8|AssertionError|assert actual == message, f"{expected}: expected message {message!r}, got {actual!r}"
Issue-Key: elasticache.tags-invalid-arn|_assert_error#9|AssertionError|assert actual == message, f"{expected}: expected message {message!r}, got {actual!r}"
Issue-Key: elasticache.user-access-string-malformed-rejected|_assert_error#16|AssertionError|assert actual == message, f"{what}: expected message {message!r}, got {actual!r}"
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# ElastiCache error messages diverge from AWS across six refusal paths (not-found, out-of-range parameter, invalid ARN, invalid access-string category)

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCacheParameterGroup

## Description
Six net-new root causes in the elasticache crate, all in the SAME service. In every one of them the emulator reaches the right decision (the call is refused) and emits the right error CODE and HTTP status, but the Error.Message text is the emulator's own wording rather than AWS's, so any client, test or tooling that matches on the message text (the only place AWS puts the offending member/id/limit for these error classes) behaves differently against the emulator. None of these is covered by the two open elasticache issues (20261023T155010Z-elasticache-control-plane-is-unusable-end-to-end.md, which owns the subnet-group/cluster lifecycle failures of this same replay, and 20261026T234107Z-elasticache-serverless-usage-limit-refusal-still-answers-notimplemented.md, which owns the serverless inverted-bounds check): neither carries any of the replay keys below and neither states these message requirements.

Cause 1 of 6: CacheParameterGroupNotFoundFault message wording, shared by every handler that resolves a CacheParameterGroupName.
AWS: Smithy: DeleteCacheParameterGroup / DescribeCacheParameters / ResetCacheParameterGroup / ModifyCacheParameterGroup / CreateCacheCluster all model CacheParameterGroupNotFoundFault {errorKind client, httpError 404, awsQueryError code CacheParameterGroupNotFound, httpResponseCode 404}. Recorded behavior for the text: AWS emits two sibling templates. On DeleteCacheParameterGroup, DescribeCacheParameters and ResetCacheParameterGroup the message is the fixed text "CacheParameterGroupnot found: " (AWS's real string has NO space between the type name and "not") followed by the one variable part, the CacheParameterGroupName taken verbatim from the request. On ModifyCacheParameterGroup and on CreateCacheCluster's CacheParameterGroupName resolution the fixed text is "CacheParameterGroup not found: " (with the space) followed by that same request-supplied name. No part of either message comes from emulator state.
Emulator: all of these answer the single template "CacheParameterGroup <name> not found." — the name is interpolated in the middle and a trailing period is added, so neither AWS template is produced. Observed: expected 'CacheParameterGroupnot found: sim2real-behav-ecpg-568086315c5b', got 'CacheParameterGroup sim2real-behav-ecpg-568086315c5b not found.' (DeleteCacheParameterGroup, double-delete), the identical divergence on the double-delete in the concurrency check, and the same on DescribeCacheParameters for an unknown group.
Why: a client that keys on the not-found message (or an AWS-targeted test fixture that asserts it) cannot recognise the emulator's refusal; and because the emulator collapses AWS's two sibling templates into one, it also erases the per-operation distinction a caller can observe on real AWS.
Fix: the shared message helper crate::state::cache_parameter_group_not_found_message in the elasticache crate's state module, which every one of these handlers calls, must produce AWS's templates (and the handlers that use the spaced variant must select it).
Traces: elasticache.cache-parameter-group-lifecycle, elasticache.concurrent-duplicate-parameter-group, elasticache.parameter-group-not-found-semantics. Call sites: services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/delete_cache_parameter_group.rs lines 204-215 (calls crate::state::cache_parameter_group_not_found_message).

Cause 2 of 6: ModifyReplicationGroup's ReplicationGroupNotFoundFault message uses the Describe template.
AWS: Smithy: ModifyReplicationGroup models ReplicationGroupNotFoundFault {errorKind client, httpError 404, awsQueryError code ReplicationGroupNotFoundFault}. Recorded behavior for the text: ModifyReplicationGroup's message is the fixed text "Replication group '" + the request's ReplicationGroupId + "' not found" — single-quoted id, no trailing period. DescribeReplicationGroups uses the OTHER template, "ReplicationGroup " + id + " not found.", and the emulator already matches that one (the Describe assertion in the same check passes).
Emulator: ModifyReplicationGroup answers the Describe wording. Observed: expected message "Replication group 'sim2real-behav-erg-replicas-6b9aa43764cc' not found", got 'ReplicationGroup sim2real-behav-erg-replicas-6b9aa43764cc not found.'
Why: the two operations are distinguishable on real AWS and are not on the emulator; a caller matching Modify's message sees an unrecognised refusal.
Fix: the ModifyReplicationGroup handler's not-found path in the elasticache crate must use its own message template instead of the shared Describe-style one.
Traces: elasticache.replica-count-argument-validation (the InvalidParameterValue assertions for Increase/DecreaseReplicaCount and the Describe assertion all pass; only the ModifyReplicationGroup message diverges).

Cause 3 of 6: ModifyCacheParameterGroup's out-of-range parameter-value message.
AWS: Smithy: ModifyCacheParameterGroup models InvalidParameterValueException {errorKind client, httpError 400, awsQueryError code InvalidParameterValue}. Recorded behavior for the text: the message is the fixed text "value:" + the rejected ParameterValue from the request + " is outside of range:" + the parameter's own AllowedValues string as DescribeCacheParameters reports it (for slowlog-max-len that is "0-"). Both variable parts come from the request and from the service's parameter metadata; the sentence is otherwise fixed.
Emulator: the code is right (InvalidParameterValue) but the message is "invalid parameter value, allowed values are:<AllowedValues>" — the rejected value is dropped entirely. Observed: expected message 'value:-5 is outside of range:0-', got 'invalid parameter value, allowed values are:0-'.
Why: the message is the only place the rejected value is reported, so a client cannot tell which of several ParameterNameValues in the batch was refused.
Fix: the ModifyCacheParameterGroup handler's parameter validation in the elasticache crate must word the refusal with AWS's template and include the offending value.
Traces: elasticache.slowlog-parameters-round-trip-and-reject-negative-length (the parameter is correctly left at Source=system, only the message diverges).

Cause 4 of 6: CreateSnapshot's CacheClusterNotFoundFault message names the type, not AWS's prose.
AWS: Smithy: CreateSnapshot models CacheClusterNotFoundFault {errorKind client, httpError 404, awsQueryError code CacheClusterNotFound}. Recorded behavior for the text: fixed text "Cache cluster not found: " followed by the request's CacheClusterId.
Emulator: "CacheCluster not found: <id>" — the shape name "CacheCluster" instead of AWS's prose "Cache cluster". Observed: expected message 'Cache cluster not found: sim2real-behav-ecc-missing-c2d3c0ff30a3', got 'CacheCluster not found: sim2real-behav-ecc-missing-c2d3c0ff30a3'. The DeleteSnapshot and CopySnapshot SnapshotNotFound messages in the same check match AWS, so this is specific to the cluster-resolution message on CreateSnapshot.
Why: message-matching clients and fixtures do not recognise the refusal.
Fix: the CreateSnapshot handler's cluster-resolution not-found message in the elasticache crate.
Traces: elasticache.snapshot-not-found-semantics.

Cause 5 of 6: InvalidARNFault message on the tagging operations.
AWS: Smithy: ListTagsForResource and AddTagsToResource both model InvalidARNFault {errorKind client, httpError 400, awsQueryError code InvalidARN}. Recorded behavior for the text: for a ResourceName that is not ARN-shaped at all, the message is entirely fixed text with NO variable part: "Input ARN string does not have 7 components." — AWS reports the structural rule it applied and does not echo the input.
Emulator: it echoes the input instead: "<ResourceName> is not a valid ElastiCache resource ARN." Observed: expected message 'Input ARN string does not have 7 components.', got 'not-a-real-arn is not a valid ElastiCache resource ARN.' (same on both ListTagsForResource and AddTagsToResource, which share one resolver).
Why: a client cannot tell which ARN rule was broken, and message assertions fail; the emulator also echoes caller input into an error where AWS does not.
Fix: the shared ARN resolution in the elasticache crate's state module that produces TagTargetRejection::InvalidArn (reached from services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/list_tags_for_resource.rs lines 170-177 and 294-296 via crate::state::...::resource_tags) must word the structural refusal as AWS does.
Traces: elasticache.tags-invalid-arn (the AddTagsToResource/ListTagsForResource positive control on a real parameter-group ARN passes).

Cause 6 of 6: ModifyUser cannot tell an unknown command category from free text in an AccessString.
AWS: Smithy: ModifyUser models InvalidParameterValueException {errorKind client, httpError 400, awsQueryError code InvalidParameterValue}; AccessString carries only pattern "\\S", so the whole RBAC grammar is enforced server-side and the message is the only diagnosis a client gets. Recorded behavior for the text: AWS distinguishes two refusal classes. An access string whose syntax parses but names a category that does not exist (e.g. "on ~sim2real:* +@<unknown>") is refused with the fixed text "Access string contains invalid category(s). Please verify the list of available category in user-api." (no variable part). An input that is not access-string syntax at all (e.g. "SIM2REAL THIS IS NOT AN ACCESS STRING") is refused with the fixed text "The access-string is invalid."
Emulator: it answers the second, generic message for BOTH classes. Observed: for AccessString "on ~sim2real:* +@sim2realnotacategory", expected message 'Access string contains invalid category(s). Please verify the list of available category in user-api.', got 'The access-string is invalid.' (the code InvalidParameterValue is correct, and the user's previously applied grant is correctly preserved).
Why: on the one authorization surface ElastiCache has, a client is told only "invalid" and cannot distinguish a typo'd category from a malformed grant, which is exactly the diagnosis AWS provides.
Fix: the ModifyUser (and CreateUser, which shares the grammar) access-string validation in the elasticache crate must classify an unparseable string separately from a parsed string naming unknown categories, and word each with its own AWS message.
Traces: elasticache.user-access-string-malformed-rejected (its free-text assertion and its effect-absent assertions pass; only the unknown-category message diverges).

## Evidence
All eight failing checks below are from this replay of suite simparity-behavioral-aws-v1 (39/152 passed). Each is a [mismatch]: the emulator answered, with the correct error code, and only the Error.Message diverged.

Cause 1 — CacheParameterGroupNotFound message:
  elasticache.cache-parameter-group-lifecycle, check_elasticache_cache_parameter_group_lifecycle.py line 108 -> _assert_error(delete_cache_parameter_group(<name>), "CacheParameterGroupNotFoundFault", f"CacheParameterGroupnot found: {name}")
    AssertionError: expected message 'CacheParameterGroupnot found: sim2real-behav-ecpg-568086315c5b', got 'CacheParameterGroup sim2real-behav-ecpg-568086315c5b not found.'
  elasticache.concurrent-duplicate-parameter-group, line 107 (double-delete): expected 'CacheParameterGroupnot found: sim2real-behav-ecpg-race-058e5c6e8b5f', got 'CacheParameterGroup sim2real-behav-ecpg-race-058e5c6e8b5f not found.'
  elasticache.parameter-group-not-found-semantics, line 104 (DescribeCacheParameters on an absent group): expected 'CacheParameterGroupnot found: sim2real-behav-ecpg-missing-9df10e15eb4c', got 'CacheParameterGroup sim2real-behav-ecpg-missing-9df10e15eb4c not found.' The same check further asserts the SPACED variant "CacheParameterGroup not found: <name>" for ModifyCacheParameterGroup and for CreateCacheCluster, which the single emulator template also cannot produce.
  Expected: HTTP 404, code CacheParameterGroupNotFound; message per the two templates named in the description (fixed text + the request's CacheParameterGroupName).
  Actual: HTTP 404, code CacheParameterGroupNotFound, message 'CacheParameterGroup <name> not found.'

Cause 2 — ModifyReplicationGroup not-found message:
  elasticache.replica-count-argument-validation, check line 83/84 -> _assert_error(modify_replication_group(ReplicationGroupId=<bogus>, ApplyImmediately=True), "ReplicationGroupNotFoundFault", f"Replication group '{bogus}' not found")
    AssertionError: expected message "Replication group 'sim2real-behav-erg-replicas-6b9aa43764cc' not found", got 'ReplicationGroup sim2real-behav-erg-replicas-6b9aa43764cc not found.'
  Expected: HTTP 404, code ReplicationGroupNotFoundFault, message "Replication group '<ReplicationGroupId>' not found".
  Actual: HTTP 404, code ReplicationGroupNotFoundFault, message 'ReplicationGroup <id> not found.' (DescribeReplicationGroups on the SAME id, asserted in the same check, matches AWS, so the shared template is being used where Modify's own is required.)

Cause 3 — ModifyCacheParameterGroup out-of-range message:
  elasticache.slowlog-parameters-round-trip-and-reject-negative-length, check line 39, after reading AllowedValues='0-' from DescribeCacheParameters for slowlog-max-len and sending ParameterNameValues=[{ParameterName: slowlog-max-len, ParameterValue: "-5"}]
    AssertionError: expected message 'value:-5 is outside of range:0-', got 'invalid parameter value, allowed values are:0-'
  Expected: HTTP 400, code InvalidParameterValue, message "value:<rejected value> is outside of range:<AllowedValues>".
  Actual: HTTP 400, code InvalidParameterValue, message 'invalid parameter value, allowed values are:<AllowedValues>' (rejected value omitted).

Cause 4 — CreateSnapshot CacheClusterNotFound message:
  elasticache.snapshot-not-found-semantics, check line 58 -> create_snapshot(CacheClusterId=<missing>, SnapshotName=<fresh>)
    AssertionError: CacheClusterNotFoundFault: expected message 'Cache cluster not found: sim2real-behav-ecc-missing-c2d3c0ff30a3', got 'CacheCluster not found: sim2real-behav-ecc-missing-c2d3c0ff30a3'
  Expected: HTTP 404, code CacheClusterNotFound, message "Cache cluster not found: <CacheClusterId>".
  Actual: HTTP 404, code CacheClusterNotFound, message 'CacheCluster not found: <CacheClusterId>'.

Cause 5 — InvalidARNFault message on tagging:
  elasticache.tags-invalid-arn, check line 77 -> list_tags_for_resource(ResourceName="not-a-real-arn") and add_tags_to_resource(ResourceName="not-a-real-arn", ...)
    AssertionError: InvalidARNFault: expected message 'Input ARN string does not have 7 components.', got 'not-a-real-arn is not a valid ElastiCache resource ARN.'
  Expected: HTTP 400, code InvalidARN, message 'Input ARN string does not have 7 components.' (wholly fixed text).
  Actual: HTTP 400, code InvalidARN, message '<ResourceName> is not a valid ElastiCache resource ARN.'

Cause 6 — ModifyUser unknown access-string category:
  elasticache.user-access-string-malformed-rejected, check line 171 -> modify_user(UserId=<active user>, AccessString="on ~sim2real:* +@sim2realnotacategory")
    AssertionError: AccessString with an unknown command category '+@sim2realnotacategory': expected message 'Access string contains invalid category(s). Please verify the list of available category in user-api.', got 'The access-string is invalid.'
  Expected: HTTP 400, code InvalidParameterValue, message 'Access string contains invalid category(s). Please verify the list of available category in user-api.' for a syntactically valid grant naming an unknown category; 'The access-string is invalid.' only for input that is not access-string syntax.
  Actual: HTTP 400, code InvalidParameterValue, message 'The access-string is invalid.' for both classes.

Code state in this checkout showing where the wording is produced: services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/delete_cache_parameter_group.rs lines 204-215 builds CacheParameterGroupNotFoundFault from crate::state::cache_parameter_group_not_found_message(&name) — one shared helper, which is why cause 1 is a single root cause across three checks; services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/list_tags_for_resource.rs lines 170-177 and 293-296 lift crate::state::TagTargetRejection::InvalidArn(message) produced by the shared resource_tags resolver, which is why cause 5 covers both tagging operations.

Model evidence: smithy_query(operation="DeleteCacheParameterGroup,DescribeCacheParameters,ModifyCacheParameterGroup,ModifyReplicationGroup,ListTagsForResource,AddTagsToResource,CreateSnapshot,ModifyUser") against services/service-com-amazonaws-elasticache-amazonelasticachev9/smithy-model/com-amazonaws-elasticache-amazonelasticachev9.json -> CacheParameterGroupNotFoundFault 404/CacheParameterGroupNotFound; ReplicationGroupNotFoundFault 404/ReplicationGroupNotFoundFault; InvalidParameterValueException 400/InvalidParameterValue; CacheClusterNotFoundFault 404/CacheClusterNotFound; InvalidARNFault 400/InvalidARN; ModifyUser.AccessString has only pattern "\\S" (no client-side grammar validation).

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the eight checks listed in trace_ids.
2. Cause 1: CreateCacheParameterGroup(CacheParameterGroupName=<fresh>, CacheParameterGroupFamily=redis7, Description=...), DeleteCacheParameterGroup it, then DeleteCacheParameterGroup the same name again. Expect 404 CacheParameterGroupNotFound with message "CacheParameterGroupnot found: <name>"; observe "CacheParameterGroup <name> not found.". Repeat with DescribeCacheParameters(CacheParameterGroupName=<never created>) (same unspaced template) and with ModifyCacheParameterGroup / CreateCacheCluster(CacheParameterGroupName=<never created>) (spaced template "CacheParameterGroup not found: <name>").
3. Cause 2: ModifyReplicationGroup(ReplicationGroupId=<never created>, ApplyImmediately=true). Expect 404 ReplicationGroupNotFoundFault with message "Replication group '<id>' not found"; observe 'ReplicationGroup <id> not found.'. Contrast with DescribeReplicationGroups(ReplicationGroupId=<same id>), which must keep its current wording 'ReplicationGroup <id> not found.'.
4. Cause 3: create a redis7 cache parameter group, read AllowedValues for slowlog-max-len via DescribeCacheParameters ("0-"), then ModifyCacheParameterGroup with ParameterNameValues=[{ParameterName: slowlog-max-len, ParameterValue: "-5"}]. Expect 400 InvalidParameterValue with message "value:-5 is outside of range:0-"; observe 'invalid parameter value, allowed values are:0-'. The parameter must remain Source=system (it already does).
5. Cause 4: CreateSnapshot(CacheClusterId=<never created>, SnapshotName=<fresh>). Expect 404 CacheClusterNotFound with message "Cache cluster not found: <CacheClusterId>"; observe 'CacheCluster not found: <CacheClusterId>'.
6. Cause 5: ListTagsForResource(ResourceName="not-a-real-arn") and AddTagsToResource(ResourceName="not-a-real-arn", Tags=[{Key:k,Value:v}]). Expect 400 InvalidARN with message 'Input ARN string does not have 7 components.'; observe the input echoed back instead.
7. Cause 6: CreateUser(UserId=<fresh>, UserName=<same>, Engine=redis, AccessString="on ~sim2real:* +@read", NoPasswordRequired=true), wait for Status=active, ModifyUser with AccessString="on ~sim2real:* +@read +@write", then ModifyUser with AccessString="on ~sim2real:* +@sim2realnotacategory". Expect 400 InvalidParameterValue with message 'Access string contains invalid category(s). Please verify the list of available category in user-api.'; observe 'The access-string is invalid.'. Then ModifyUser with AccessString="SIM2REAL THIS IS NOT AN ACCESS STRING" and confirm that call keeps the message 'The access-string is invalid.'.
