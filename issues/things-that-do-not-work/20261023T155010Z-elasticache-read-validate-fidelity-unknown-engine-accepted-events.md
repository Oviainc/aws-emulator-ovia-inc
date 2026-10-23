Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.describe-cache-engine-versions elasticache.describe-events-chronological-ordering elasticache.replication-group-encryption-snapshot-and-tags-round-trip elasticache.cache-cluster-empty-vs-absent-description-preserved elasticache.server-side-maintenance-window-enforced elasticache.batch-update-action-unknown-service-update elasticache.cache-cluster-not-found-semantics elasticache.cache-parameter-group-lifecycle elasticache.concurrent-duplicate-parameter-group elasticache.global-replication-group-not-found-semantics elasticache.global-replication-group-topology-not-found elasticache.replica-count-argument-validation elasticache.replication-group-not-found-semantics elasticache.replication-group-user-group-ids-reject-bare-user-id elasticache.replication-group-duplicate elasticache.snapshot-lifecycle-from-cluster elasticache.user-group-default-required elasticache.reboot-cache-cluster-invalid-state elasticache.replica-count-state-gating elasticache.increase-replica-count-exceeds-limit-rejected elasticache.decrease-replica-count-noop-rejected elasticache.create-replication-group-invalid-shard-config-rejected elasticache.create-serverless-cache-usage-limit-out-of-range-rejected
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.describe-cache-engine-versions|_error#9|AssertionError|raise AssertionError(f"expected {expected} but the call SUCCEEDED")
Issue-Key: elasticache.describe-events-chronological-ordering|run#11|AssertionError|assert dates == sorted(dates, reverse=True), (
Issue-Key: elasticache.replication-group-encryption-snapshot-and-tags-round-trip|run#14|AssertionError|assert rg["KmsKeyId"] == key_arn, (rg["KmsKeyId"], key_arn)
Issue-Key: elasticache.cache-cluster-empty-vs-absent-description-preserved|run#11|AssertionError|assert by_name[empty_name]["Description"] == " ", (
Issue-Key: elasticache.server-side-maintenance-window-enforced|_error#8|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: elasticache.batch-update-action-unknown-service-update|_assert_error#8|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: elasticache.cache-cluster-not-found-semantics|_assert_error#18|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: elasticache.cache-parameter-group-lifecycle|_assert_error#11|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: elasticache.concurrent-duplicate-parameter-group|run#10|AssertionError|assert failure == ("CacheParameterGroupAlreadyExists", expected_message), failures
Issue-Key: elasticache.global-replication-group-not-found-semantics|_assert_error#3|AssertionError|assert got == message, f"{expected}: expected message {message!r}, got {got!r}"
Issue-Key: elasticache.global-replication-group-topology-not-found|_assert_error#3|AssertionError|assert got == message, f"{expected}: expected message {message!r}, got {got!r}"
Issue-Key: elasticache.replica-count-argument-validation|_assert_error#5|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: elasticache.replication-group-not-found-semantics|_assert_error#8|AssertionError|assert actual == message, f"{expected}: expected message {message!r}, got {actual!r}"
Issue-Key: elasticache.replication-group-user-group-ids-reject-bare-user-id|run#14|AssertionError|assert not_found_msg == expected, f"expected message {expected!r}, got {not_found_msg!r}"
Issue-Key: elasticache.replication-group-duplicate|_assert_error#5|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: elasticache.snapshot-lifecycle-from-cluster|_assert_error#17|AssertionError|assert actual == message, f"{expected}: expected message {message!r}, got {actual!r}"
Issue-Key: elasticache.user-group-default-required|_assert_error#12|AssertionError|assert got == message, f"{code}: expected message {message!r}, got {got!r}"
Issue-Key: elasticache.reboot-cache-cluster-invalid-state|_assert_error#7|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: elasticache.replica-count-state-gating|run#9|AssertionError|assert message == _NOT_AVAILABLE_MESSAGE, f"expected message {_NOT_AVAILABLE_MESSAGE!r}, got {message!r}"
Issue-Key: elasticache.increase-replica-count-exceeds-limit-rejected|run#11|AssertionError|assert message == "New replica count must be between 1 and 5 replicas.", message
Issue-Key: elasticache.decrease-replica-count-noop-rejected|run#10|AssertionError|assert message == "Requested new replica count already matches current number of replicas.", (
Issue-Key: elasticache.create-replication-group-invalid-shard-config-rejected|run#10|AssertionError|assert message == "NumCacheClusters can only be specified for one node group.", f"unexpected InvalidParameterCombination message: {message!r}"
Issue-Key: elasticache.create-serverless-cache-usage-limit-out-of-range-rejected|run#5|AssertionError|assert message == (
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# ElastiCache read/validate fidelity: unknown engine accepted, events in the wrong order, KMS alias not resolved, empty description preserved, and the service's error message templates replaced with the emulator's prose (7 causes)

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCacheEngineVersions

## Description
Seven independent fidelity defects on the implemented ElastiCache surface where the HTTP status and the error code are already right but the response content, the ordering, or the message rule is not. Each cause is separate and must be closed separately.

Cause 1 of 7: DescribeCacheEngineVersions accepts an engine name that does not exist.
AWS: Smithy: DescribeCacheEngineVersions models NO operation errors, so this is the service's common InvalidParameterValueException channel (code InvalidParameterValue, HTTP 400); recorded behavior for Engine='not-an-engine' is that refusal with the recorded example message "Invalid engine name: not-an-engine" — NOT an empty result set.
Emulator: answers HTTP 200 with a result set.
Why: discovery is how a client picks an engine/version pair; a typo silently yields a catalogue answer instead of a refusal, and a client cannot tell "no such engine" from "no versions for this engine".
Fix: Engine validation in the DescribeCacheEngineVersions handler, against the engine-version catalog it already reads. (Distinct from the open wire issue 20261023T062907Z-elasticache-catalog-describes-serve-synthesized-data-instead-of.md, which is about the catalog's CONTENT, not this missing validation.)
Traces: elasticache.describe-cache-engine-versions.

Cause 2 of 7: DescribeEvents returns events oldest-first.
AWS: Smithy: DescribeEvents -> EventsMessage.Events, paginated on Marker. Recorded behavior: the event list is strictly reverse-chronological (most recent Event.Date first) within the requested window.
Emulator: returns ascending order — recorded example: dates 15:24:31.464, 15:24:31.964, 15:24:41.981 in that order.
Why: a client reading "what just happened" takes the first page and gets the OLDEST events, and any "latest event" logic is wrong; with MaxRecords/Marker paging the newest events may never be reached.
Fix: the ordering applied by the DescribeEvents handler over the event log (sort descending by date before paging, so the Marker cursor keeps that order).
Traces: elasticache.describe-events-chronological-ordering.

Cause 3 of 7: a KMS alias passed as KmsKeyId is echoed back verbatim instead of the resolved key ARN.
AWS: Smithy: CreateReplicationGroup$KmsKeyId is a String and the reply/describe carries ReplicationGroup.KmsKeyId. Recorded behavior: a request naming KmsKeyId='alias/<name>' is stored resolved — DescribeReplicationGroups reports the key's ARN (arn:aws:kms:<region>:<account>:key/<uuid>) for the life of the group.
Emulator: DescribeReplicationGroups reports 'alias/<name>', the literal request value.
Why: the KmsKeyId a client reads back is meant to identify the key itself; an alias is re-targetable, so a client auditing which key encrypts the group (or comparing it to kms DescribeKey output) cannot resolve or match it.
Fix: KmsKeyId resolution in the CreateReplicationGroup handler (resolve an alias to the key ARN before storing it) and the stored value the ReplicationGroup projection reports.
Traces: elasticache.replication-group-encryption-snapshot-and-tags-round-trip.

Cause 4 of 7: an explicit empty-string Description is preserved instead of being coerced to a single space.
AWS: recorded behavior only — CreateCacheParameterGroup(Description='') is accepted and DescribeCacheParameterGroups reports Description=' ' (one space); a non-empty description is preserved byte for byte.
Emulator: reports Description='' for the same group.
Why: a client that round-trips descriptions (or diffs them against the service's stored value) sees a value the service never returns, and "empty" vs "absent" vs "blank" stop agreeing with the service.
Fix: the Description normalization in the CreateCacheParameterGroup handler / the cache-parameter-group record, applied before storing.
Traces: elasticache.cache-cluster-empty-vs-absent-description-preserved.

Cause 5 of 7: a malformed PreferredMaintenanceWindow is still accepted and stored on the CREATE path.
AWS: Smithy: CreateCacheCluster models InvalidParameterValueException (code InvalidParameterValue, HTTP 400). Recorded behavior: a window that is not ddd:hh24:mi-ddd:hh24:mi is refused with that error and no cluster is created.
Emulator: CreateCacheCluster returns HTTP 200 and stores the value — recorded example response carries PreferredMaintenanceWindow 'not-a-valid-window' with CacheClusterStatus 'available'.
Why: the cluster ends up with a schedule that can never fire and a client that relies on AWS rejecting a typo silently loses its maintenance window.
Fix: the shared PreferredMaintenanceWindow validation, applied on CreateCacheCluster as well as ModifyCacheCluster, before any state is written. NOTE: this is the same root cause as Cause 2 of the open issue 20261022T193451Z-elasticache-cache-cluster-subnet-group-validation-is-over.md, which already names this check id; this run's failure is on the CREATE call and was not matched to that issue's replay key, so it is recorded here. Close them together.
Traces: elasticache.server-side-maintenance-window-enforced.

Cause 6 of 7: the not-found / already-exists message TEMPLATES are the emulator's own prose rather than the service's.
AWS: recorded behavior — each of these errors carries a fixed template, recorded consistently across the suite (status and code are already correct in the emulator; only the template differs): ServiceUpdateNotFoundFault "ServiceUpdate <name> not found."; CacheClusterNotFound "CacheCluster not found: <id>"; CacheParameterGroupAlreadyExists "Parameter group <name> already exists"; GlobalReplicationGroupNotFoundFault "Global Replication Group not found" (no id at all); ReplicationGroupNotFoundFault "ReplicationGroup <id> not found."; ReplicationGroupAlreadyExistsFault "Replication group with specified name already exists." (no id); SnapshotAlreadyExistsFault "Snapshot with specified name already exists." (no id); DefaultUserRequired "Redis user group needs to contain a user with the user name default."
Emulator: substitutes its own wording, e.g. "Service update <name> not found.", "Cache cluster <id> not found.", "Cache parameter group <name> already exists.", "Global Replication Group <id> not found.", "Replication group <id> not found.", "Replication group <id> already exists.", "Snapshot <name> already exists.", "A user group must contain the default user." Note two distinct template rules are broken: capitalisation/word-form of the resource name, and whether the template interpolates the identifier at all.
Why: the requirement here is the TEMPLATE, not one recorded string — clients, retry wrappers and operator tooling that match these texts misreport the cause, and an id interpolated into a template AWS defines without one changes the string for every caller.
Fix: the message templates in the elasticache not-found / already-exists error constructors (one shared place per resource). The same defect for the cache-cluster template is Cause 5 of open issue 20261022T193451Z-elasticache-cache-cluster-subnet-group-validation-is-over.md; the other resources are not covered there.
Traces: elasticache.batch-update-action-unknown-service-update, elasticache.cache-cluster-not-found-semantics, elasticache.cache-parameter-group-lifecycle, elasticache.concurrent-duplicate-parameter-group, elasticache.global-replication-group-not-found-semantics, elasticache.global-replication-group-topology-not-found, elasticache.replica-count-argument-validation, elasticache.replication-group-not-found-semantics, elasticache.replication-group-user-group-ids-reject-bare-user-id, elasticache.replication-group-duplicate, elasticache.snapshot-lifecycle-from-cluster, elasticache.user-group-default-required.

Cause 7 of 7: the validation and state-gate message templates likewise diverge, and several advertise bounds the service does not state.
AWS: recorded behavior — RebootCacheCluster on a cluster in the wrong state: "Can only reboot cache clusters with state in: available, incompatible-parameters, snapshotting. Cluster <id> has state: <state>."; the replica-count state gate: "Replication group must be in available state to modify."; IncreaseReplicaCount above the limit: "New replica count must be between 1 and 5 replicas."; DecreaseReplicaCount no-op: "Requested new replica count already matches current number of replicas."; CreateReplicationGroup with NumCacheClusters plus a multi-shard topology: "NumCacheClusters can only be specified for one node group." Codes and statuses already match (InvalidCacheClusterState / InvalidReplicationGroupState / InvalidParameterValue / NoOperationFault / InvalidParameterCombination, HTTP 400).
Emulator: answers its own prose instead, e.g. "Cache cluster <id> is in creating state, not available.", "Replication group <id> is in modifying state, not available.", "Invalid value 6 for NewReplicaCount: a node group has between 0 and 5 replicas." (a different bound: 0-5 vs the service's 1-5), "Node group 0001 of replication group <id> already has 1 replicas.", "NumCacheClusters cannot be combined with NumNodeGroups (2); use ReplicasPerNodeGroup instead.", and for a serverless usage limit "Invalid value -5 for DataStorage limit. Must be between 1 and 5000. You can set the value to 0 to remove the limit."
Why: these messages are built from the request and state the rule that was violated, so the rule itself is the requirement — a client (or a human) reading "between 0 and 5 replicas" is told a constraint the service does not have, and the reboot message no longer names the states that WOULD be accepted.
Fix: the message templates in the reboot/state-gate and replica-count/shard-config validators, stating the service's rule (and its real bounds) rather than a locally invented one.
Traces: elasticache.reboot-cache-cluster-invalid-state, elasticache.replica-count-state-gating, elasticache.increase-replica-count-exceeds-limit-rejected, elasticache.decrease-replica-count-noop-rejected, elasticache.create-replication-group-invalid-shard-config-rejected, elasticache.create-serverless-cache-usage-limit-out-of-range-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against this checkout. Per cause:
1. DescribeCacheEngineVersions(Engine='not-an-engine') -> AssertionError: expected InvalidParameterValueException but the call SUCCEEDED (expected 400 InvalidParameterValue "Invalid engine name: not-an-engine").
2. DescribeEvents -> AssertionError: DescribeEvents did not return events in strict reverse-chronological order: [2026-10-23 15:24:31.464931, 2026-10-23 15:24:31.964931, 2026-10-23 15:24:41.981721].
3. CreateReplicationGroup(AtRestEncryptionEnabled=True, KmsKeyId='alias/<rg>') then DescribeReplicationGroups -> AssertionError: ('alias/sim2real-behav-ecrg-enc-2dfc9d038f10', 'arn:aws:kms:us-west-2:000000000000:key/9bc6d2b2-f824-416d-8a7a-11f1c8970762') i.e. got the alias, expected the key ARN kms CreateKey returned.
4. CreateCacheParameterGroup(Description='') then DescribeCacheParameterGroups -> AssertionError: explicit empty-string Description round-tripped as '', expected the real-AWS-observed coercion to ' ' (single space).
5. CreateCacheCluster(..., PreferredMaintenanceWindow='not-a-valid-window') -> AssertionError: expected this call to be rejected, but it SUCCEEDED and returned {'CacheCluster': {... 'PreferredMaintenanceWindow': 'not-a-valid-window', 'CacheClusterStatus': 'available' ...}, 'ResponseMetadata': {'HTTPStatusCode': 200}}.
6. Message-template failures (code correct, message wrong): "expected message 'ServiceUpdate ... not found.', got 'Service update ... not found.'"; "expected message 'CacheCluster not found: <id>', got 'Cache cluster <id> not found.'"; "expected message 'Parameter group <n> already exists', got 'Cache parameter group <n> already exists.'" (also on all three losers of the concurrent create race, where exactly one create did win); "GlobalReplicationGroupNotFoundFault: expected message 'Global Replication Group not found', got 'Global Replication Group <id> not found.'" (two checks); "expected message 'ReplicationGroup <id> not found.', got 'Replication group <id> not found.'" (three checks); "expected message 'Replication group with specified name already exists.', got 'Replication group <id> already exists.'"; "SnapshotAlreadyExistsFault: expected message 'Snapshot with specified name already exists.', got 'Snapshot <n> already exists.'"; "DefaultUserRequired: expected message 'Redis user group needs to contain a user with the user name default.', got 'A user group must contain the default user.'".
7. Validation/state-gate templates: "expected message 'Can only reboot cache clusters with state in: available, incompatible-parameters, snapshotting. Cluster <id> has state: creating.', got 'Cache cluster <id> is in creating state, not available.'"; "expected message 'Replication group must be in available state to modify.', got 'Replication group <id> is in modifying state, not available.'"; "AssertionError: Invalid value 6 for NewReplicaCount: a node group has between 0 and 5 replicas." against the expected 'New replica count must be between 1 and 5 replicas.'; "unexpected NoOperationFault message: 'Node group 0001 of replication group <id> already has 1 replicas.'"; "unexpected InvalidParameterCombination message: 'NumCacheClusters cannot be combined with NumNodeGroups (2); use ReplicasPerNodeGroup instead.'"; "AssertionError: Invalid value -5 for DataStorage limit. Must be between 1 and 5000. ...".

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Cause 1: elasticache DescribeCacheEngineVersions(Engine='not-an-engine') -> expect 400 InvalidParameterValue; observe 200.
3. Cause 2: create a cache cluster, let a few events accumulate, then DescribeEvents and compare Event.Date ordering (expect newest first).
4. Cause 3: kms CreateKey + CreateAlias; elasticache CreateReplicationGroup(AtRestEncryptionEnabled=True, KmsKeyId='alias/<name>'); wait available; DescribeReplicationGroups -> expect KmsKeyId = the key ARN; observe the alias.
5. Cause 4: CreateCacheParameterGroup(Description='') then DescribeCacheParameterGroups -> expect Description ' '; observe ''.
6. Cause 5: CreateCacheCluster(Engine='memcached', CacheNodeType='cache.t3.micro', NumCacheNodes=1, PreferredMaintenanceWindow='not-a-valid-window') -> expect 400 InvalidParameterValue and no cluster; observe 200 with the value stored.
7. Causes 6 and 7: trigger each refusal named in Traces (unknown service update, absent cache cluster / replication group / global replication group / snapshot, duplicate parameter group / replication group / snapshot, user group without 'default', reboot in 'creating', replica-count change while 'modifying', NewReplicaCount=6, a no-op DecreaseReplicaCount, NumCacheClusters with NumNodeGroups=2, a negative serverless DataStorage limit) and compare the Message against the template quoted above; the code and the 400/404 status already match.
8. Confirm the fix by re-replaying every check id listed in Traces.
