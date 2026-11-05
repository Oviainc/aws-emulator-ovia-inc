Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.cluster-deletion-protection-delete-rejected rds.delete-cluster-deletion-protection-blocked rds.delete-deletion-protected-cluster-without-disable-rejected rds.deletion-protection-server-enforced-gate rds.delete-instance-deletion-protection rds.delete-instance-conflicting-snapshot-flags-rejected rds.reboot-force-failover-requires-multi-az-rejected rds.modify-cluster-capacity-invalid-value-rejected rds.modify-cluster-capacity-out-of-range-rejected rds.modify-current-capacity-out-of-range-error rds.create-cluster-conflicting-engine-mode-rejected rds.create-subnet-group-requires-subnets rds.apply-pending-maintenance-action rds.modify-instance-during-deletion-rejected
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.cluster-deletion-protection-delete-rejected|run#10|AssertionError|assert code == "InvalidParameterCombination", (
Issue-Key: rds.delete-cluster-deletion-protection-blocked|run#19|AssertionError|assert code == "InvalidParameterCombination", (
Issue-Key: rds.delete-deletion-protected-cluster-without-disable-rejected|run#9|AssertionError|assert code == "InvalidParameterCombination", (
Issue-Key: rds.deletion-protection-server-enforced-gate|run#20|AssertionError|assert code == "InvalidParameterCombination", (
Issue-Key: rds.delete-instance-deletion-protection|run#10|AssertionError|assert code == ERR_INVALID_PARAMETER_COMBINATION, f"unexpected error code: {code!r}"
Issue-Key: rds.delete-instance-conflicting-snapshot-flags-rejected|run#10|AssertionError|assert code == "InvalidParameterCombination", f"expected InvalidParameterCombination, got {code!r}"
Issue-Key: rds.reboot-force-failover-requires-multi-az-rejected|run#10|AssertionError|assert code == "InvalidParameterValue", f"expected InvalidParameterValue, got {code!r}"
Issue-Key: rds.modify-cluster-capacity-invalid-value-rejected|run#16|AssertionError|assert err.get("Code") == "InvalidParameterValue", (
Issue-Key: rds.modify-cluster-capacity-out-of-range-rejected|run#9|AssertionError|assert code == "InvalidParameterValue", (
Issue-Key: rds.modify-current-capacity-out-of-range-error|run#6|AssertionError|assert err.get("Code") == "InvalidParameterValue", err
Issue-Key: rds.create-cluster-conflicting-engine-mode-rejected|run#5|AssertionError|assert code == "InvalidParameterValue", f"unexpected error code: {code!r}"
Issue-Key: rds.create-subnet-group-requires-subnets|run#11|AssertionError|assert error.get("Code") == "InvalidSubnet", (
Issue-Key: rds.apply-pending-maintenance-action|run#14|AssertionError|assert bogus_code == ERR_RESOURCE_NOT_FOUND, f"unexpected error code: {bogus_code!r}"
Issue-Key: rds.modify-instance-during-deletion-rejected|run#16|AssertionError|assert message == expected, f"unexpected error message: {message!r} != {expected!r}"
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# rds: 9 net-new causes — refusals are classified under the wrong error code (state faults and ValidationException where AWS answers a parameter error)

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDBCluster

## Description
Nine distinct misclassifications. The call IS refused, but with the wrong code, so client code that branches on the error code takes the wrong path. Each is its own cause.

Cause 1 of 9: DeleteDBCluster on a deletion-protected cluster answers InvalidDBClusterStateFault. AWS answers InvalidParameterCombination (HTTP 400) — deletion protection is a request/parameter conflict, not a cluster-state fault; Smithy's DeleteDBCluster error list contains InvalidDBClusterStateFault for 'cluster is not in the available state', and InvalidParameterCombination is the service-common query-protocol error AWS uses for this refusal. Emulator message: "DB cluster <id> can't be deleted because deletion protection is enabled". Checks: rds.cluster-deletion-protection-delete-rejected, rds.delete-cluster-deletion-protection-blocked, rds.delete-deletion-protected-cluster-without-disable-rejected, rds.deletion-protection-server-enforced-gate.

Cause 2 of 9: DeleteDBInstance on a deletion-protected instance answers InvalidDBInstanceState (Smithy awsQueryError code InvalidDBInstanceState) where AWS answers InvalidParameterCombination with the fixed sentence 'Cannot delete protected DB Instance <id>, please disable deletion protection and try again.' Check: rds.delete-instance-deletion-protection.

Cause 3 of 9: DeleteDBInstance with conflicting snapshot flags (SkipFinalSnapshot=false together with no FinalDBSnapshotIdentifier, or both set) answers an unmodeled generic 'ValidationException'. AWS answers InvalidParameterCombination (HTTP 400); ValidationException is not in DeleteDBInstance's modeled error set at all. Check: rds.delete-instance-conflicting-snapshot-flags-rejected.

Cause 4 of 9: RebootDBInstance(ForceFailover=true) on a single-AZ instance answers 'ValidationException' where AWS answers InvalidParameterValue (HTTP 400) stating that ForceFailover requires a Multi-AZ instance. Check: rds.reboot-force-failover-requires-multi-az-rejected.

Cause 5 of 9: Serverless v2 capacity refusals are coded InvalidParameterCombination where AWS answers InvalidParameterValue. The emulator refuses with 'engine version <v> of aurora-mysql does not support the serverless engine mode: it supports provisioned' instead of naming the out-of-domain capacity value. Checks: rds.modify-cluster-capacity-invalid-value-rejected, rds.modify-cluster-capacity-out-of-range-rejected, rds.modify-current-capacity-out-of-range-error.

Cause 6 of 9: CreateDBCluster with a conflicting EngineMode answers InvalidParameterCombination where AWS answers InvalidParameterValue naming EngineMode (Aurora Serverless v1 is withdrawn for new clusters). Check: rds.create-cluster-conflicting-engine-mode-rejected.

Cause 7 of 9: CreateDBSubnetGroup with an empty SubnetIds list answers DBSubnetGroupDoesNotCoverEnoughAZs with the AZ-coverage sentence and an empty AZ list ("Current AZ coverage: . Add subnets to cover at least 2 AZs."). AWS answers InvalidSubnet (Smithy: InvalidSubnet, httpError 400) because the list itself is unusable — the AZ-coverage rule is only reached once there are subnets. Check: rds.create-subnet-group-requires-subnets.

Cause 8 of 9: ApplyPendingMaintenanceAction answers InvalidParameterValue where AWS answers the resource/state error the check expects for the named action and resource. Check: rds.apply-pending-maintenance-action.

Cause 9 of 9: ModifyDBInstance against an instance in 'deleting' answers the generic sentence 'Database instance is not in available state.' AWS words this refusal 'The specified database instance is currently in deleting state, which is a terminal state.' — the fixed text names the current state and that it is terminal, with the state taken from the instance's record. Check: rds.modify-instance-during-deletion-rejected.

Why: SDK callers and retry layers switch on the error code: a state fault is treated as retryable/transient ('wait and retry'), while a parameter error is terminal. Classifying deletion protection and parameter conflicts as state faults makes correct client code loop; answering an unmodeled ValidationException makes typed error handling (botocore errorfactory) fall through to a generic ClientError. Fix: the refusal-classification step in each handler (delete_db_cluster.rs, delete_db_instance.rs, reboot_db_instance.rs, modify_db_cluster.rs/modify_current_db_cluster_capacity.rs, create_db_cluster.rs, create_db_subnet_group.rs, apply_pending_maintenance_action.rs, modify_db_instance.rs) in the rds crate. Related open issue 20261029T214632Z-rds-answers-the-wrong-error-class-because-existence.md covers ordering of existence vs permission checks and does not carry these replay keys.

## Evidence
Verbatim replay output:
- 'unexpected error code \'InvalidDBClusterStateFault\': "DB cluster <id> can\'t be deleted because deletion protection is enabled"' and 'expected InvalidParameterCombination for DeleteDBCluster on a deletion-protected cluster, got \'InvalidDBClusterStateFault\'' (4 checks).
- 'unexpected error code: \'InvalidDBInstanceState\'' (delete-instance-deletion-protection).
- 'expected InvalidParameterCombination, got \'ValidationException\'' (delete-instance-conflicting-snapshot-flags-rejected).
- 'expected InvalidParameterValue, got \'ValidationException\'' (reboot-force-failover-requires-multi-az-rejected).
- 'expected InvalidParameterValue rejecting EngineMode=serverless ..., got \'InvalidParameterCombination\'' and {'Type':'Sender','Code':'InvalidParameterCombination','Message':'engine version 8.0.mysql_aurora.3.08.0 of aurora-mysql does not support the serverless engine mode: it supports provisioned'}.
- 'expected InvalidSubnet for an empty SubnetIds list, got \'DBSubnetGroupDoesNotCoverEnoughAZs\' (message="The DB subnet group doesn\'t meet Availability Zone (AZ) coverage requirement. Current AZ coverage: . Add subnets to cover at least 2 AZs.")'.
- 'unexpected error code: \'InvalidParameterValue\'' (apply-pending-maintenance-action).
- "unexpected error message: 'Database instance is not in available state.' != 'The specified database instance is currently in deleting state, which is a terminal state.'".

## Steps to Reproduce
1. create_db_cluster(DeletionProtection=True) then delete_db_cluster(...) — observe code InvalidDBClusterStateFault instead of InvalidParameterCombination.
2. create_db_instance(DeletionProtection=True) then delete_db_instance(...) — observe InvalidDBInstanceState.
3. delete_db_instance(SkipFinalSnapshot=False) with no FinalDBSnapshotIdentifier — observe ValidationException.
4. reboot_db_instance(ForceFailover=True) on a single-AZ instance — observe ValidationException.
5. modify_db_cluster(ServerlessV2ScalingConfiguration=<out-of-range>) / modify_current_db_cluster_capacity(Capacity=<out-of-range>) — observe InvalidParameterCombination.
6. create_db_cluster(EngineMode='serverless') on a current aurora-mysql version — observe InvalidParameterCombination.
7. create_db_subnet_group(SubnetIds=[]) — observe DBSubnetGroupDoesNotCoverEnoughAZs.
8. apply_pending_maintenance_action(...) — observe InvalidParameterValue.
9. delete_db_instance then immediately modify_db_instance — observe the 'not in available state' sentence.
