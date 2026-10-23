Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.cache-subnet-group-invalid-subnet elasticache.maintenance-window-scheduling-enforced elasticache.preferred-maintenance-window-invalid-format-rejected elasticache.server-side-maintenance-window-enforced elasticache.modify-cache-cluster-invalid-security-group-rejected elasticache.preferred-maintenance-window-string-preserved elasticache.create-cache-cluster-invalid-arguments
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.cache-subnet-group-invalid-subnet|_error#3|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED with {result!r}")
Issue-Key: elasticache.maintenance-window-scheduling-enforced|_error#8|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: elasticache.preferred-maintenance-window-invalid-format-rejected|_error#7|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: elasticache.server-side-maintenance-window-enforced|run#9|AssertionError|assert code == "InvalidParameterValue", f"unexpected error code {code!r} ({message!r})"
Issue-Key: elasticache.modify-cache-cluster-invalid-security-group-rejected|run#10|AssertionError|assert code == "InvalidParameterValue", f"expected InvalidParameterValue, got {code!r}"
Issue-Key: elasticache.preferred-maintenance-window-string-preserved|run#17|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: elasticache.create-cache-cluster-invalid-arguments|_assert_error#8|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# ElastiCache cache-cluster/subnet-group validation is over-permissive and its state gate and not-found texts diverge (5 causes)

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyCacheCluster

## Description
Five independent fidelity defects on the implemented cache-cluster and cache-subnet-group surface: input real ElastiCache refuses is accepted with HTTP 200, one refusal uses the wrong modeled error, one state gate refuses a call AWS accepts, and the not-found/identifier message rules differ. Each cause is separate and must be closed separately.

Cause 1 of 5: CreateCacheSubnetGroup accepts a subnet id that does not exist.
AWS: Smithy: CreateCacheSubnetGroup models InvalidSubnet (code `InvalidSubnet`, HTTP 400) and the recorded behavior for a syntactically subnet-shaped but nonexistent id is HTTP 400 with code `InvalidParameterValue` naming the offending ids; recorded example message: "Some input subnets in :[subnet-00000000deadbeef] are invalid." No group is created.
Emulator: returns HTTP 200 and creates the group, echoing the bogus subnet back (recorded response carried CacheSubnetGroup with Subnets=[{SubnetIdentifier: subnet-00000000deadbeef}] and a minted ARN).
Why: a client believes it has a usable subnet group; every downstream cluster placement into it is built on a subnet that does not exist, and the failure surfaces far from the call that caused it.
Fix: the CreateCacheSubnetGroup handler's subnet resolution/validation against EC2 subnet state.
Traces: elasticache.cache-subnet-group-invalid-subnet.

Cause 2 of 5: PreferredMaintenanceWindow format is never validated on CreateCacheCluster/ModifyCacheCluster.
AWS: Smithy: ModifyCacheCluster binds InvalidParameterValueException (code `InvalidParameterValue`, HTTP 400). Recorded behavior: a window not in ddd:hh24:mi-ddd:hh24:mi form is refused with HTTP 400 `InvalidParameterValue` — recorded example message "Invalid maintenance window format. Should be specified as a range ddd:hh24:mi-ddd:hh24:mi (24H Clock UTC). Example: Sun:23:15-Mon:03:15" — and the cluster's stored window is unchanged.
Emulator: accepts the malformed value with HTTP 200 and STORES it: the recorded responses show CacheCluster.PreferredMaintenanceWindow = 'notaday:99:99-notaday:100:00' and = 'not-a-valid-window', status 'modifying'.
Why: the cluster ends up with a schedule that can never fire, and a client that relies on AWS rejecting typos silently loses its maintenance window.
Fix: PreferredMaintenanceWindow validation shared by the CreateCacheCluster and ModifyCacheCluster handlers, applied before any state mutation.
Traces: elasticache.maintenance-window-scheduling-enforced, elasticache.preferred-maintenance-window-invalid-format-rejected, elasticache.server-side-maintenance-window-enforced.

Cause 3 of 5: ModifyCacheCluster with an unknown CacheSecurityGroupNames entry answers the wrong modeled error.
AWS: Smithy: ModifyCacheCluster models BOTH CacheSecurityGroupNotFoundFault (code `CacheSecurityGroupNotFound`, HTTP 404) and InvalidParameterValueException (code `InvalidParameterValue`, HTTP 400). Recorded behavior on a VPC-only account is the latter: HTTP 400 `InvalidParameterValue` with the account-wide cache-security-group closure, recorded example message "Use of cache security groups is not permitted in this API version for your account."; unrelated cluster state is untouched.
Emulator: answers code `CacheSecurityGroupNotFound` (HTTP 404), i.e. a per-name lookup failure instead of the account-level refusal.
Why: a client reads "that group does not exist, create it" and retries a path that can never work, instead of "this construct is unavailable on this account".
Fix: the ModifyCacheCluster handler's CacheSecurityGroupNames branch (same account gate as the cache-security-group surface).
Traces: elasticache.modify-cache-cluster-invalid-security-group-rejected.

Cause 4 of 5: a cluster left in `modifying` refuses further ModifyCacheCluster calls, and the state check runs before parameter validation.
AWS: Smithy: InvalidCacheClusterStateFault (code `InvalidCacheClusterState`, HTTP 400) is modeled for ModifyCacheCluster, but recorded behavior is that a cluster that has just accepted a modify still accepts a subsequent ModifyCacheCluster, and that an invalid parameter value is refused with HTTP 400 `InvalidParameterValue` (parameter validation first) rather than a state fault.
Emulator: after one successful modify the cluster stays `modifying` and the next ModifyCacheCluster is refused with code `InvalidCacheClusterState` (recorded example message "Cache cluster <id> is in modifying state, not available.").
Why: back-to-back configuration changes — the normal shape of an idempotent converge loop — fail against the emulator, and a malformed parameter is masked by a state error so the client retries forever instead of fixing its input.
Fix: the ModifyCacheCluster handler's precondition ordering plus the cluster-state transition that must settle `modifying` back to `available`.
Traces: elasticache.server-side-maintenance-window-enforced.

Cause 5 of 5: cache-cluster not-found and identifier-format message rules differ from the service's.
AWS: recorded behavior — CacheClusterNotFound (code `CacheClusterNotFound`, HTTP 404) carries the template "CacheCluster not found: <CacheClusterId>"; a malformed CacheClusterId is refused with `InvalidParameterValue` (HTTP 400) whose text is the service's fixed identifier rule, recorded as "The parameter CacheClusterIdentifier is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens."
Emulator: the status and code are right, but the messages are the emulator's own prose: "Cache cluster <id> not found." for the not-found template, and "Invalid value <id> for CacheClusterId: a name must contain from 1 to 50 alphanumeric characters or hyphens, must begin with a letter, and must not end with a hyphen or contain two consecutive hyphens." for the identifier rule (which also names a different member and invents a 1-50 bound).
Why: callers and tooling that match on these texts (and the identifier rule, which must name CacheClusterIdentifier) misreport the cause; the identifier text also advertises a constraint AWS does not state.
Fix: the message templates in the cache-cluster not-found path and in the CacheClusterId validator.
Traces: elasticache.preferred-maintenance-window-string-preserved, elasticache.create-cache-cluster-invalid-arguments.

## Evidence
1. CreateCacheSubnetGroup(SubnetIds=['subnet-00000000deadbeef']) -> HTTP 200: {'CacheSubnetGroup': {'CacheSubnetGroupName': 'sim2real-behav-ecsg-bad-...', 'VpcId': 'vpc-00000001', 'Subnets': [{'SubnetIdentifier': 'subnet-00000000deadbeef', ...}], 'ARN': 'arn:aws:elasticache:...:subnetgroup:...'}}. Check failed with "expected the call to be rejected, but it SUCCEEDED"; AWS answers 400 InvalidParameterValue "Some input subnets in :[subnet-00000000deadbeef] are invalid."
2. ModifyCacheCluster(PreferredMaintenanceWindow='notaday:99:99-notaday:100:00', ApplyImmediately=True) -> HTTP 200 with CacheCluster.PreferredMaintenanceWindow='notaday:99:99-notaday:100:00', CacheClusterStatus='modifying'; and ModifyCacheCluster(PreferredMaintenanceWindow='not-a-valid-window') -> HTTP 200 with PreferredMaintenanceWindow='not-a-valid-window'. Expected 400 InvalidParameterValue "Invalid maintenance window format. Should be specified as a range ddd:hh24:mi-ddd:hh24:mi (24H Clock UTC). Example: Sun:23:15-Mon:03:15".
3. ModifyCacheCluster(CacheSecurityGroupNames=['sim2real-nonexistent-cache-sg']) -> "expected InvalidParameterValue, got 'CacheSecurityGroupNotFound'" (400 expected, 404 observed).
4. After a successful ModifyCacheCluster, the next ModifyCacheCluster -> "unexpected error code 'InvalidCacheClusterState' ('Cache cluster sim2real-mw-... is in modifying state, not available.')" where 400 InvalidParameterValue was due.
5. ModifyCacheCluster on a deleted cluster -> code CacheClusterNotFound but message "Cache cluster sim2real-pmw-... not found." vs AWS "CacheCluster not found: sim2real-pmw-...". CreateCacheCluster(CacheClusterId='Sim2Real_Behav_Bad_Id') -> code InvalidParameterValue but message "Invalid value Sim2Real_Behav_Bad_Id for CacheClusterId: a name must contain from 1 to 50 alphanumeric characters or hyphens, ..." vs AWS "The parameter CacheClusterIdentifier is not a valid identifier. ...".

## Steps to Reproduce
1. elasticache CreateCacheSubnetGroup with SubnetIds=['subnet-00000000deadbeef'] -> expect 400 InvalidParameterValue naming the invalid subnet; observe 200 + a created group.
2. CreateCacheCluster (engine redis or memcached, cache.t3.micro, 1 node) with a valid PreferredMaintenanceWindow, wait for available, then ModifyCacheCluster with PreferredMaintenanceWindow='not-a-valid-window', ApplyImmediately=True -> expect 400 InvalidParameterValue and an unchanged stored window; observe 200 and the malformed value stored.
3. On an available cluster, ModifyCacheCluster with CacheSecurityGroupNames=['does-not-exist'] -> expect 400 InvalidParameterValue (cache-security-group closure); observe 404 CacheSecurityGroupNotFound.
4. On an available cluster, issue ModifyCacheCluster twice in sequence (second with a malformed PreferredMaintenanceWindow) -> expect the second to be validated, not blocked; observe 400 InvalidCacheClusterState 'in modifying state, not available'.
5. Delete a cluster, wait until gone, then ModifyCacheCluster on the dead id -> compare the message to "CacheCluster not found: <id>". Separately CreateCacheCluster with CacheClusterId='Sim2Real_Behav_Bad_Id' and compare the message to the CacheClusterIdentifier identifier rule.
