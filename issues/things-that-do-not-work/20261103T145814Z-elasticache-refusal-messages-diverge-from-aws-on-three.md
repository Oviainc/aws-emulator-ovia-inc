Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.cache-parameter-group-lifecycle elasticache.replica-count-argument-validation elasticache.concurrent-modify-user-group-membership-race
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.cache-parameter-group-lifecycle|run#19|AssertionError|assert refused == (
Issue-Key: elasticache.replica-count-argument-validation|run#10|AssertionError|assert message == f"Replication group '{bogus}' not found", message
Issue-Key: elasticache.concurrent-modify-user-group-membership-race|run#22|AssertionError|assert failures[0][2] == expected, f"expected {expected!r}, got {failures[0][2]!r}"
Issue-Commit: 34977689197ee7e7ec3a117f2c4937da4e6f0436

# ElastiCache refusal messages diverge from AWS on three paths: DeleteCacheParameterGroup not-found, ModifyReplicationGroup not-found, and ModifyUserGroup invalid-state (3 causes)

> _Note: this issue was identified against the code base at commit `34977689197ee7e7ec3a117f2c4937da4e6f0436`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCacheParameterGroup

## Description
Three net-new root causes in the elasticache crate, all in the SAME service. In each one the emulator reaches the correct decision (the call is refused), returns the correct HTTP status and the correct error code, and only the Error.Message text diverges: it is the emulator's own prose instead of AWS's sentence for that error class. For these error classes the message is the only place AWS reports the offending identifier and the state that blocked the call, so a client, fixture or tool that matches on it behaves differently against the emulator.

Relationship to already-open issues: 20261029T174147Z-elasticache-error-messages-diverge-from-aws-across-six.md describes the same wording defects as its cause 1 and cause 2 but does NOT carry these checks' current replay keys (the replay report lists both checks as unowned, so no open issue gates them); cause 3 below appears in no open issue at all. 20261023T155010Z-elasticache-control-plane-is-unusable-end-to-end.md owns the subnet-group/cluster lifecycle failures of this same replay and states none of these message requirements.

Cause 1 of 3: the CacheParameterGroupNotFoundFault message on DeleteCacheParameterGroup (and every handler sharing the same resolver).
AWS: Smithy (services/service-com-amazonaws-elasticache-amazonelasticachev9/smithy-model/com-amazonaws-elasticache-amazonelasticachev9.json): DeleteCacheParameterGroup models CacheParameterGroupNotFoundFault {errorKind client, httpError 404, awsQueryError code CacheParameterGroupNotFound, httpResponseCode 404}. The model does not state the text, so recorded behavior only for the wording: the message is the fixed text "CacheParameterGroupnot found: " — AWS's real string has NO space between the type name and "not" — followed by its single variable part, the CacheParameterGroupName copied verbatim from the request. No part of the message comes from emulator state.
Emulator: answers 404/CacheParameterGroupNotFound with the template "CacheParameterGroup <name> not found." — the name is interpolated in the middle and a trailing period is appended, so AWS's sentence is never produced.
Why: a client or AWS-targeted fixture that keys on the not-found sentence cannot recognise the emulator's refusal, and the emulator's trailing-period form is a sentence real AWS never emits for this error class.
Fix: the DeleteCacheParameterGroup handler's not-found path and the shared cache-parameter-group not-found message helper in the elasticache crate's state module (the helper every parameter-group resolver calls) must emit AWS's template.
Traces: elasticache.cache-parameter-group-lifecycle (the duplicate-name CacheParameterGroupAlreadyExists assertion, the create/describe round trip and the re-create after delete all pass; only this message diverges).

Cause 2 of 3: ModifyReplicationGroup's ReplicationGroupNotFoundFault message uses the Describe template.
AWS: Smithy: ModifyReplicationGroup models ReplicationGroupNotFoundFault {errorKind client, httpError 404, awsQueryError code ReplicationGroupNotFoundFault, httpResponseCode 404}. Recorded behavior only for the text: ModifyReplicationGroup's message is the fixed text "Replication group '" + the request's ReplicationGroupId + "' not found" — the id single-quoted, and NO trailing period. DescribeReplicationGroups uses a different sibling template, "ReplicationGroup " + the request's id + " not found.", which the emulator already matches (that assertion in the same check passes).
Emulator: ModifyReplicationGroup answers the Describe-style sentence "ReplicationGroup <id> not found." — unquoted id, type-name prefix, trailing period.
Why: the two operations' refusals are distinguishable on real AWS and are indistinguishable on the emulator; a caller matching Modify's sentence sees an unrecognised refusal, and the per-operation distinction a client can observe on AWS is erased.
Fix: the ModifyReplicationGroup handler's not-found path in the elasticache crate must use its own message template rather than the shared Describe-style one.
Traces: elasticache.replica-count-argument-validation (the InvalidParameterValue refusals for IncreaseReplicaCount and DecreaseReplicaCount and the DescribeReplicationGroups message all pass; only ModifyReplicationGroup's message diverges).

Cause 3 of 3: ModifyUserGroup's InvalidUserGroupStateFault message is emulator-authored prose.
AWS: Smithy: ModifyUserGroup models InvalidUserGroupStateFault {errorKind client, httpError 400, awsQueryError code InvalidUserGroupState, httpResponseCode 400}. Recorded behavior only for the text: the message is two fixed sentences around one variable part — "User group " + the request's UserGroupId + " has status " + the group's current Status as DescribeUserGroups reports it (here "modifying") + ". Only active user groups can be modified." The id comes from the request; the status word comes from emulator/service state; the rest is fixed.
Emulator: answers 400/InvalidUserGroupState with its own sentence "User group <id> is in status <status>; it must be active to be changed." — a semicolon-joined paraphrase that shares no fixed text with AWS's.
Why: this is the refusal a client gets when it races two membership changes on one user group; the message is the only place the blocking status is reported, so a client matching AWS's sentence (or retrying on it) does not recognise the conflict. The decision itself is right: exactly one of the two concurrent ModifyUserGroup calls wins and the loser is refused with InvalidUserGroupState.
Fix: the ModifyUserGroup handler's state-gate refusal in the elasticache crate (and the shared user-group state-gate message helper, if other user-group mutations route through it) must word the refusal as AWS does.
Traces: elasticache.concurrent-modify-user-group-membership-race (the one-winner/one-loser outcome, the InvalidUserGroupState code and the converged membership — user_b present exactly once — all pass; only the message diverges).

## Evidence
All three failing checks are [mismatch] verdicts from this replay of suite simparity-behavioral-aws-v1 (41/161 passed): the emulator answered each call, with the correct HTTP status and error code, and only Error.Message diverged. None of the three is marked ALREADY FILED or OWNED in the replay report.

Cause 1 — DeleteCacheParameterGroup CacheParameterGroupNotFound message:
  elasticache.cache-parameter-group-lifecycle, check_elasticache_cache_parameter_group_lifecycle.py line 111 -> after CreateCacheParameterGroup(<name>, family redis7) and DeleteCacheParameterGroup(<name>), a second DeleteCacheParameterGroup(CacheParameterGroupName=<name>).
    AssertionError: expected CacheParameterGroupNotFound with its exact message, got ('CacheParameterGroupNotFound', 'CacheParameterGroup sim2real-behav-ecpg-04903635c0e2 not found.')
  Expected: HTTP 404, code CacheParameterGroupNotFound, message = fixed text "CacheParameterGroupnot found: " + the request's CacheParameterGroupName (no space after the type name; no trailing period).
  Actual:   HTTP 404, code CacheParameterGroupNotFound, message "CacheParameterGroup <CacheParameterGroupName> not found."
  (The name shown, sim2real-behav-ecpg-04903635c0e2, is this run's generated group name — a value filling the template, not part of it.)

Cause 2 — ModifyReplicationGroup ReplicationGroupNotFoundFault message:
  elasticache.replica-count-argument-validation, check_elasticache_replica_count_argument_validation.py line 72 -> ModifyReplicationGroup(ReplicationGroupId=<never created>, ApplyImmediately=true).
    AssertionError: ReplicationGroup sim2real-behav-erg-replicas-78ef76bf126a not found.  (the asserted expectation was "Replication group '<id>' not found")
  Expected: HTTP 404, code ReplicationGroupNotFoundFault, message = "Replication group '" + the request's ReplicationGroupId + "' not found".
  Actual:   HTTP 404, code ReplicationGroupNotFoundFault, message "ReplicationGroup <id> not found."
  Differential control in the same check, which PASSES: DescribeReplicationGroups(ReplicationGroupId=<same id>) correctly answers ReplicationGroupNotFoundFault with "ReplicationGroup <id> not found.", and IncreaseReplicaCount/DecreaseReplicaCount on the same id correctly answer InvalidParameterValue with "Replication group <id> does not exist." — so the shared Describe-style template is being reused where Modify needs its own.

Cause 3 — ModifyUserGroup InvalidUserGroupState message:
  elasticache.concurrent-modify-user-group-membership-race, check_elasticache_concurrent_modify_user_group_membership_race.py line 89 -> two CreateUser calls (engine redis, AccessString "on ~* +@all", NoPasswordRequired), CreateUserGroup(UserGroupId=<id>, Engine=redis, UserIds=[default, user_a]), wait for Status=active, then two ModifyUserGroup(UserGroupId=<id>, UserIdsToAdd=[user_b]) calls released together from a 2-thread barrier.
    AssertionError: expected 'User group sim2real-ug-b27cfa421fd5 has status modifying. Only active user groups can be modified.', got 'User group sim2real-ug-b27cfa421fd5 is in status modifying; it must be active to be changed.'
  Expected: HTTP 400, code InvalidUserGroupState, message = "User group " + the request's UserGroupId + " has status " + the group's current Status + ". Only active user groups can be modified."
  Actual:   HTTP 400, code InvalidUserGroupState, message "User group <UserGroupId> is in status <status>; it must be active to be changed."
  (The id sim2real-ug-b27cfa421fd5 is this run's generated group id; "modifying" is the state the emulator reported for it.)

Model evidence: smithy_query(query="operation", operation="DeleteCacheParameterGroup,ModifyReplicationGroup,ModifyUserGroup", model_path="services/service-com-amazonaws-elasticache-amazonelasticachev9/smithy-model/com-amazonaws-elasticache-amazonelasticachev9.json") -> DeleteCacheParameterGroup errors include CacheParameterGroupNotFoundFault {client, httpError 404, awsQueryError CacheParameterGroupNotFound/404}; ModifyReplicationGroup errors include ReplicationGroupNotFoundFault {client, httpError 404, awsQueryError ReplicationGroupNotFoundFault/404}; ModifyUserGroup errors include InvalidUserGroupStateFault {client, httpError 400, awsQueryError InvalidUserGroupState/400}. The model carries the status and code requirements only; the message text for each class is recorded behavior.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the three checks listed in trace_ids (elasticache.cache-parameter-group-lifecycle, elasticache.replica-count-argument-validation, elasticache.concurrent-modify-user-group-membership-race).

2. Cause 1: CreateCacheParameterGroup(CacheParameterGroupName=<fresh name>, CacheParameterGroupFamily="redis7", Description="fidelity check parameter group"); confirm DescribeCacheParameterGroups returns it; DeleteCacheParameterGroup(<same name>); then DeleteCacheParameterGroup(<same name>) a second time. Expect HTTP 404, code CacheParameterGroupNotFound, message "CacheParameterGroupnot found: <name>" (no space after the type name, no trailing period). Observe "CacheParameterGroup <name> not found.".

3. Cause 2: ModifyReplicationGroup(ReplicationGroupId=<an id never created>, ApplyImmediately=true). Expect HTTP 404, code ReplicationGroupNotFoundFault, message "Replication group '<id>' not found". Observe "ReplicationGroup <id> not found.". As a control, DescribeReplicationGroups(ReplicationGroupId=<the same id>) must KEEP answering "ReplicationGroup <id> not found.", and IncreaseReplicaCount/DecreaseReplicaCount on that id must keep answering InvalidParameterValue "Replication group <id> does not exist.".

4. Cause 3: CreateUser twice (UserId=<fresh a>/<fresh b>, UserName same, Engine="redis", AccessString="on ~* +@all", NoPasswordRequired=true); CreateUserGroup(UserGroupId=<fresh group>, Engine="redis", UserIds=["default", <a>]); poll DescribeUserGroups until Status=="active"; then issue two ModifyUserGroup(UserGroupId=<group>, UserIdsToAdd=[<b>]) calls concurrently, released together so the second lands inside the first's modifying window. Exactly one must succeed and the other must be refused with HTTP 400, code InvalidUserGroupState and message "User group <UserGroupId> has status <current Status>. Only active user groups can be modified.". Observe the emulator's "User group <UserGroupId> is in status <status>; it must be active to be changed.". The converged membership (user <b> present exactly once, alongside "default" and <a>) already matches AWS and must stay that way.
