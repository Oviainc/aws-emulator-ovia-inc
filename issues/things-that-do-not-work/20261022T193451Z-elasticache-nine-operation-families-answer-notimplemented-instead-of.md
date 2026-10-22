Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.cache-security-group-crud-lifecycle elasticache.cache-security-group-ingress-not-found elasticache.cache-security-group-unsupported elasticache.global-replication-group-not-found-semantics elasticache.global-replication-group-topology-not-found elasticache.purchase-reserved-offering-unknown elasticache.replica-count-argument-validation elasticache.user-group-unknown-member-rejected elasticache.create-user-weak-password-rejected elasticache.create-serverless-cache-usage-limit-out-of-range-rejected elasticache.serverless-cache-usage-limits-inverted-rejected elasticache.reboot-cache-cluster-invalid-state
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.cache-security-group-crud-lifecycle|run#6|AssertionError|assert create_code == "InvalidParameterValue", (
Issue-Key: elasticache.cache-security-group-ingress-not-found|_assert_closed#7|AssertionError|assert _fault_base(code) == _fault_base(expected_code), f"expected {expected_code}, got {code}"
Issue-Key: elasticache.cache-security-group-unsupported|_assert_error#5|AssertionError|assert _fault_base(code) == _fault_base(expected), f"expected {expected}, got {code}"
Issue-Key: elasticache.global-replication-group-not-found-semantics|_assert_error#3|AssertionError|assert _fault_base(code) == _fault_base(expected), f"expected {expected}, got {code}"
Issue-Key: elasticache.global-replication-group-topology-not-found|_assert_error#3|AssertionError|assert _fault_base(code) == _fault_base(expected), f"expected {expected}, got {code}"
Issue-Key: elasticache.purchase-reserved-offering-unknown|_assert_error#3|AssertionError|assert _fault_base(code) == _fault_base(expected), f"expected {expected}, got {code}"
Issue-Key: elasticache.replica-count-argument-validation|_assert_error#3|AssertionError|assert _fault_base(code) == _fault_base(expected), f"expected {expected}, got {code}"
Issue-Key: elasticache.user-group-unknown-member-rejected|_assert_error#5|AssertionError|assert _fault_base(code) == _fault_base(expected), f"expected {expected}, got {code}"
Issue-Key: elasticache.create-user-weak-password-rejected|run#5|AssertionError|assert code == "InvalidParameterValue", f"expected InvalidParameterValue, got {code!r}"
Issue-Key: elasticache.create-serverless-cache-usage-limit-out-of-range-rejected|run#4|AssertionError|assert code == "InvalidParameterValue", f"expected InvalidParameterValue, got {code}"
Issue-Key: elasticache.serverless-cache-usage-limits-inverted-rejected|_assert_error#3|AssertionError|assert _fault_base(code) == _fault_base(expected), (
Issue-Key: elasticache.reboot-cache-cluster-invalid-state|_assert_error#7|AssertionError|assert _fault_base(code) == _fault_base(expected), f"expected {expected}, got {code}"
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# ElastiCache: nine operation families answer NotImplemented instead of their modeled responses and errors (9 causes)

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCacheSecurityGroup

## Description
Nine distinct ElastiCache operation families reply with the emulator's scaffold error (error code `NotImplemented`, message "operation <Op> is scaffolded but not implemented in this emulator") where real ElastiCache returns a modeled response or a modeled error. Each cause below is a separate operation family and must be closed separately. Where the suite has already auto-filed a "stub" issue for the handler, the cause still records WHICH modeled error that handler must produce once implemented — that part is not covered by the stub issue.

Cause 1 of 9: the whole legacy CacheSecurityGroup surface (CreateCacheSecurityGroup, DescribeCacheSecurityGroups, AuthorizeCacheSecurityGroupIngress, RevokeCacheSecurityGroupIngress, DeleteCacheSecurityGroup).
AWS: Smithy model (com-amazonaws-elasticache-amazonelasticachev9) binds InvalidParameterValueException to CreateCacheSecurityGroup (awsQueryError code `InvalidParameterValue`, httpResponseCode 400). On a VPC-only account — the norm after EC2-Classic retirement — the recorded behavior is that EVERY operation on this surface is refused with HTTP 400 / `InvalidParameterValue` and the account-wide closure message, not a per-name CacheSecurityGroupNotFoundFault; recorded example message: "Use of cache security groups is not permitted in this API version for your account."
Emulator: every one of those calls answers error code `NotImplemented`.
Why: a client probing whether cache security groups are usable gets an unrecognized, unmodeled error code; SDK error handling for InvalidParameterValue never fires, and code that treats the closure as "legacy construct unavailable" instead sees a transport-level failure.
Fix: the cache-security-group handlers for this service plus their shared account-gating validation.
Traces: elasticache.cache-security-group-crud-lifecycle, elasticache.cache-security-group-ingress-not-found, elasticache.cache-security-group-unsupported.

Cause 2 of 9: global-replication-group reads and mutations on an unknown id (DescribeGlobalReplicationGroups, DeleteGlobalReplicationGroup, ModifyGlobalReplicationGroup, DisassociateGlobalReplicationGroup).
AWS: Smithy: DescribeGlobalReplicationGroups errors include GlobalReplicationGroupNotFoundFault (awsQueryError code `GlobalReplicationGroupNotFoundFault`, httpResponseCode 404). Recorded example message: "Global Replication Group not found".
Emulator: `NotImplemented`.
Why: callers cannot distinguish "no such global replication group" from a service failure, so retry/creation logic keyed on the not-found fault never triggers.
Fix: the four global-replication-group handlers and their id lookup.
Traces: elasticache.global-replication-group-not-found-semantics.

Cause 3 of 9: global-replication-group topology mutations on an unknown id (FailoverGlobalReplicationGroup, IncreaseNodeGroupsInGlobalReplicationGroup, DecreaseNodeGroupsInGlobalReplicationGroup, RebalanceSlotsInGlobalReplicationGroup).
AWS: same modeled fault as cause 2 — GlobalReplicationGroupNotFoundFault, HTTP 404, code `GlobalReplicationGroupNotFoundFault`; recorded example message "Global Replication Group not found".
Emulator: `NotImplemented`.
Why: the same loss of not-found semantics, on the topology plane.
Fix: the four topology handlers and their id lookup.
Traces: elasticache.global-replication-group-topology-not-found.

Cause 4 of 9: PurchaseReservedCacheNodesOffering with an offering id that does not exist.
AWS: Smithy: PurchaseReservedCacheNodesOffering errors include ReservedCacheNodesOfferingNotFoundFault (awsQueryError code `ReservedCacheNodesOfferingNotFound`, httpResponseCode 404). Recorded example message: "ReservedCacheNodesOfferingId <id> not found."
Emulator: `NotImplemented`.
Why: a purchase against a stale/unknown offering id is indistinguishable from an emulator outage, and the operation can never be exercised negatively.
Fix: the PurchaseReservedCacheNodesOffering handler and its offering lookup.
Traces: elasticache.purchase-reserved-offering-unknown.

Cause 5 of 9: IncreaseReplicaCount / DecreaseReplicaCount argument validation.
AWS: Smithy: both operations bind InvalidParameterValueException (code `InvalidParameterValue`, HTTP 400) as well as ReplicationGroupNotFoundFault; recorded behavior is that argument validation precedes the replication-group lookup, so an unknown ReplicationGroupId yields HTTP 400 `InvalidParameterValue` (recorded example message: "Replication group <id> does not exist.") while ordinary lookup-first operations on the same id yield ReplicationGroupNotFoundFault.
Emulator: `NotImplemented`.
Why: clients lose the documented ordering difference between these two operations and the rest of the replication-group surface, and cannot test replica scaling failures at all.
Fix: the IncreaseReplicaCount / DecreaseReplicaCount handlers and their input validation.
Traces: elasticache.replica-count-argument-validation.

Cause 6 of 9: CreateUserGroup referencing a user id that does not exist.
AWS: Smithy: CreateUserGroup errors include UserNotFoundFault (awsQueryError code `UserNotFound`, httpResponseCode 404); recorded example message "User <id> not found." The group must not be created.
Emulator: `NotImplemented`.
Why: a user group that references a ghost member cannot be refused, and the restore leg (same call succeeding once the user exists) is unverifiable.
Fix: the CreateUserGroup handler plus member resolution against user state.
Traces: elasticache.user-group-unknown-member-rejected.

Cause 7 of 9: CreateUser password-strength validation.
AWS: Smithy: CreateUser binds InvalidParameterValueException (code `InvalidParameterValue`, HTTP 400). Recorded behavior: a password below the enforced 16-character minimum is refused, recorded example message "Passwords length must be between 16-128 characters.", and no user is created (DescribeUsers then answers UserNotFound).
Emulator: `NotImplemented`. (The auto-filed stub issue for the CreateUser handler covers standing the operation up; this cause records the validation rule and error it must then enforce.)
Why: a client can "succeed" in no path at all here, and once the stub is implemented without this rule a weak password would be silently accepted.
Fix: the CreateUser handler's password validation.
Traces: elasticache.create-user-weak-password-rejected.

Cause 8 of 9: CreateServerlessCache usage-limit validation.
AWS: Smithy: CreateServerlessCache binds InvalidParameterValueException (code `InvalidParameterValue`, HTTP 400) and ServerlessCacheNotFoundFault (code `ServerlessCacheNotFoundFault`, HTTP 404). Recorded behavior: an out-of-range DataStorage.Maximum (e.g. -5) is refused — recorded example message "Invalid value for DataStorage limit. Must be between 1 and 5000. You can set the value to zero to remove the limit." — and an inverted limit (Minimum greater than Maximum, for DataStorage or ECPUPerSecond) is refused likewise, recorded example "Invalid values for DataStorage. Minimum must not be greater than Maximum."; nothing is created, so DescribeServerlessCaches answers ServerlessCacheNotFoundFault.
Emulator: `NotImplemented`. (A stub issue for the CreateServerlessCache handler is auto-filed; this cause records the two usage-limit rules it must enforce, which that issue does not state.)
Why: once implemented without these rules, out-of-range or inverted limits would be silently clamped/accepted and a client would believe a cache was provisioned within a bound it never had.
Fix: the CreateServerlessCache handler's CacheUsageLimits validation.
Traces: elasticache.create-serverless-cache-usage-limit-out-of-range-rejected, elasticache.serverless-cache-usage-limits-inverted-rejected.

Cause 9 of 9: RebootCacheCluster state gate.
AWS: Smithy: RebootCacheCluster errors are exactly CacheClusterNotFoundFault (code `CacheClusterNotFound`, HTTP 404) and InvalidCacheClusterStateFault (code `InvalidCacheClusterState`, HTTP 400). Recorded behavior: rebooting a cluster still in `creating` is refused with HTTP 400 `InvalidCacheClusterState` (recorded example message "Can only reboot cache clusters with state in: available, incompatible-parameters, snapshotting. Cluster <id> has state: creating."), the identical call succeeds once the cluster is `available`, and after deletion it answers CacheClusterNotFound.
Emulator: `NotImplemented`. (A stub issue for the RebootCacheCluster handler is auto-filed; this cause records the state gate and the two modeled errors it must answer.)
Why: reboot can be neither exercised nor refused, so clients that reboot during provisioning see an unmodeled error instead of the state fault they retry on.
Fix: the RebootCacheCluster handler plus the cluster-state precondition check.
Traces: elasticache.reboot-cache-cluster-invalid-state.

## Evidence
Replayed against the emulator, each call returned error code `NotImplemented` with message "operation <Op> is scaffolded but not implemented in this emulator":
- CreateCacheSecurityGroup: expected InvalidParameterValue (HTTP 400) + "Use of cache security groups is not permitted in this API version for your account." — got NotImplemented (elasticache.cache-security-group-crud-lifecycle: "expected CreateCacheSecurityGroup to be rejected with InvalidParameterValue, got 'NotImplemented'"); same for DescribeCacheSecurityGroups / Authorize / Revoke / Delete (cache-security-group-ingress-not-found, cache-security-group-unsupported: "expected InvalidParameterValueException, got NotImplemented").
- DescribeGlobalReplicationGroups: "expected GlobalReplicationGroupNotFoundFault, got NotImplemented" (HTTP 404 expected).
- FailoverGlobalReplicationGroup: "expected GlobalReplicationGroupNotFoundFault, got NotImplemented".
- PurchaseReservedCacheNodesOffering (offering 00000000-0000-0000-0000-000000000000): "expected ReservedCacheNodesOfferingNotFoundFault, got NotImplemented" (HTTP 404 expected).
- IncreaseReplicaCount: "expected InvalidParameterValueException, got NotImplemented" (HTTP 400 expected).
- CreateUserGroup(UserIds=['default', <ghost>]): "expected UserNotFoundFault, got NotImplemented" (HTTP 404 expected).
- CreateUser(Passwords=['short12']): "expected InvalidParameterValue, got 'NotImplemented'" (HTTP 400 expected).
- CreateServerlessCache(DataStorage.Maximum=-5): "expected InvalidParameterValue, got NotImplemented"; CreateServerlessCache(DataStorage Minimum=50 > Maximum=10): "expected InvalidParameterValueException, got NotImplemented" (HTTP 400 expected).
- RebootCacheCluster on a `creating` cluster: "expected InvalidCacheClusterStateFault, got NotImplemented" (HTTP 400 expected).

## Steps to Reproduce
Boot the emulator and replay the behavioral suite's elasticache checks, or directly:
1. elasticache CreateCacheSecurityGroup / DescribeCacheSecurityGroups / AuthorizeCacheSecurityGroupIngress / RevokeCacheSecurityGroupIngress / DeleteCacheSecurityGroup with any name -> expect HTTP 400 InvalidParameterValue (account-wide closure), observe NotImplemented.
2. DescribeGlobalReplicationGroups / DeleteGlobalReplicationGroup / ModifyGlobalReplicationGroup / DisassociateGlobalReplicationGroup with an unknown GlobalReplicationGroupId -> expect 404 GlobalReplicationGroupNotFoundFault.
3. FailoverGlobalReplicationGroup / IncreaseNodeGroupsInGlobalReplicationGroup / DecreaseNodeGroupsInGlobalReplicationGroup / RebalanceSlotsInGlobalReplicationGroup with an unknown id -> expect 404 GlobalReplicationGroupNotFoundFault.
4. PurchaseReservedCacheNodesOffering with a nonexistent ReservedCacheNodesOfferingId -> expect 404 ReservedCacheNodesOfferingNotFound.
5. IncreaseReplicaCount / DecreaseReplicaCount with an unknown ReplicationGroupId -> expect 400 InvalidParameterValue (argument validation before lookup).
6. CreateUserGroup with UserIds containing an id that was never created -> expect 404 UserNotFound and no group created.
7. CreateUser with a 7-character password -> expect 400 InvalidParameterValue and no user created.
8. CreateServerlessCache with CacheUsageLimits.DataStorage.Maximum=-5, and again with DataStorage Minimum=50/Maximum=10 -> expect 400 InvalidParameterValue both times.
9. CreateCacheCluster, then immediately RebootCacheCluster while status is `creating` -> expect 400 InvalidCacheClusterState.
