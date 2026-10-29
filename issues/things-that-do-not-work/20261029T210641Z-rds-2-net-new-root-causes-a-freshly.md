Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.describe-cluster-parameters rds.create-shard-group-wrong-cluster-state-rejected
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.describe-cluster-parameters|_poll_until#6|Timeout|time.sleep(interval_s)
Issue-Key: rds.create-shard-group-wrong-cluster-state-rejected|_wait_cluster_status#15|Timeout|time.sleep(poll_s)
Issue-Commit: 6074294cbfccbc2257fa0c432afede02752270ab

# rds: 2 net-new root causes — a freshly created DB cluster parameter group reports NO parameters, and CreateDBShardGroup accepts a cluster that is not AVAILABLE/CREATING

> _Note: this issue was identified against the code base at commit `6074294cbfccbc2257fa0c432afede02752270ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBClusterParameters

## Description
Two distinct root causes behind the two unowned `rds` failures of this replay (the remaining rds failures are marked OWNED by 20261029T195009Z-rds-6-net-new-root-causes-copydbparametergroup-copydbsnapshot.md / 20261029T195009Z-rds-8-net-new-root-causes-not-found.md / 20261024T030130Z-rds-state-and-validation-fidelity-ca-certificate-modification.md, or ALREADY FILED as scaffold stubs, and are skipped here).

Cause 1 of 2: DescribeDBClusterParameters reports an EMPTY Parameters list for a DB cluster parameter group that was just created.
AWS: a new cluster parameter group is pre-populated with its family's defaults — the service's own model says so on CreateDBClusterParameterGroup: "A DB cluster parameter group is initially created with the default parameters for the database engine used by instances in the DB cluster. To provide custom values for any of the parameters, you must modify the group after creating it using ModifyDBClusterParameterGroup", and "You can use ... the DescribeDBClusterParameters operation to verify that your DB cluster parameter group has been created". Smithy: DescribeDBClusterParameters(DBClusterParameterGroupName required, optional Source "engine-default | system | user", MaxRecords/Marker paginated over `Parameters`) -> DBClusterParameterGroupDetails; each Parameter carries ParameterName/ParameterValue/Source. With no Source filter, AWS returns the group's whole parameter set — the family defaults (Source `engine-default`/`system`) overlaid by any caller-set value (Source `user`) — so the set a fresh group reports equals the family's engine defaults published by DescribeEngineDefaultClusterParameters.
Emulator: the fresh group reports `Parameters: []` for the default (no Source) and for the `user`/`customer` source. This is deliberate in the handler: services/service-com-amazonaws-rds-amazonrdsv19/src/ops/describe_db_cluster_parameters.rs maps ParameterSourceFilter::Customer to `group.parameters` only, and the family defaults are served ONLY when the caller names an engine source; services/service-com-amazonaws-rds-amazonrdsv19/src/ops/create_db_cluster_parameter_group.rs inserts the record with `parameters: BTreeMap::new()` and its module doc states "a describe of a fresh group reports an empty Parameters list until ModifyDBClusterParameterGroup sets one". The check polls DescribeDBClusterParameters (paginated) until the group reports at least one parameter and never gets one, so it exhausts the harness's 60 s cap.
Why: a client that reads a parameter's effective value for a group, diffs a group against the engine defaults, or paginates the group's parameters gets zero records where AWS returns the full family set — and the "created with the default parameters" contract is unobservable. A console-style or IaC drift check built on this reports a group with no parameters at all.
Fix: the DescribeDBClusterParameters handler's source selection (and/or seeding the family defaults into the record at CreateDBClusterParameterGroup) — the default and `user`/`system` views must report the family's published defaults with their AWS Source, overlaid by values ModifyDBClusterParameterGroup set, while keeping the existing engine-default view. The sibling DB-parameter-group path (DescribeDBParameters) is owned by 20261029T195009Z-rds-6-net-new-root-causes-copydbparametergroup-copydbsnapshot.md, which does not carry this check or the cluster-parameter-group handler.
Traces: rds.describe-cluster-parameters.

Cause 2 of 2: CreateDBShardGroup accepts a DB cluster that is not in the AVAILABLE or CREATING state (e.g. stopped/stopping).
AWS: Smithy CreateDBShardGroup models InvalidDBClusterStateFault (HTTP 400, awsQueryError code `InvalidDBClusterStateFault`) and refuses a shard group on a cluster whose state does not allow it; the refusal is worded "You can't create the DB shard group. The DB cluster <DBClusterIdentifier> isn't in the AVAILABLE or CREATING state." — fixed text, with the cluster identifier taken from the request. Nothing is created: a following DescribeDBShardGroups for that identifier answers DBShardGroupNotFound. The identical call on the SAME cluster once it is started and available succeeds.
Emulator: services/service-com-amazonaws-rds-amazonrdsv19/src/ops/create_db_shard_group.rs guards only `cluster.is_deleting()` and its own comment states "Every other status accepts it — a provisioning status this emulator's clock has yet to settle included", so a stopped (or stopping) Limitless cluster is handed a shard group with HTTP 200 instead of the modelled state fault.
Why: an over-permissive accept — a client gets a shard group on a cluster AWS would have refused, so a deployment that is invalid against AWS looks healthy here, and the state-fault branch clients handle is never exercised.
Fix: the CreateDBShardGroup handler's cluster-state guard (accept only AVAILABLE/CREATING, refuse everything else with InvalidDBClusterStateFault and the message above, creating no record).
Traces: rds.create-shard-group-wrong-cluster-state-rejected.

## Evidence
Suite simparity-behavioral-aws-v1, service group `rds` 66/315 passed. Both checks below are unmarked in the replay report (neither OWNED nor ALREADY FILED).

Cause 1 — rds.describe-cluster-parameters [mismatch]: `Error: Check timed out after 60.0s`.
The check creates a cluster parameter group in the aurora-mysql default family, then polls `_poll_until(lambda: _group_param_names(rds, name), lambda p: len(p) > 0, timeout_s=60, interval_s=3, description=f"parameters populated for {name}")`, where `_group_param_names` pages `describe_db_cluster_parameters(DBClusterParameterGroupName=name)` and collects `ParameterName`s. Expected: the poll is satisfied on the first call and `default_names - group_names` is empty, i.e. the group reports every parameter DescribeEngineDefaultClusterParameters publishes for the family. Actual: every page comes back with `Parameters: []`, so the poll can never be satisfied and the check dies on the cap. Source confirmation (same behaviour, by construction): describe_db_cluster_parameters.rs — `ParameterSourceFilter::Customer => group.parameters.keys()...` with family defaults only under `ParameterSourceFilter::Engine`; create_db_cluster_parameter_group.rs — `parameters: ::std::collections::BTreeMap::new()`.
The check's second leg (unknown group name -> code `DBParameterGroupNotFound`, message "DBClusterParameterGroup not found: <name>") is never reached; the handler does word that refusal that way.

Cause 2 — rds.create-shard-group-wrong-cluster-state-rejected [mismatch]: `Error: Check timed out after 60.0s`.
The check asserts exactly `(code, message) == ("InvalidDBClusterStateFault", f"You can't create the DB shard group. The DB cluster {cluster_id} isn't in the AVAILABLE or CREATING state.")` for CreateDBShardGroup against a STOPPED Limitless aurora-postgresql cluster, then that DescribeDBShardGroups answers DBShardGroupNotFound, then that the identical call succeeds once the cluster is started.
What the timeout does and does not establish: the check's two status waits (`_wait_cluster_status(..., "available")` then `_wait_cluster_status(..., "stopped")`, poll_s=30) consume the whole 60 s cap because the emulator's STATUS_TRANSITION_DELAY (300 ms, src/state.rs) makes the first describe after each call still report the in-flight status, so the check never reached its CreateDBShardGroup assertion in this replay — the cap, not the refusal, is what ended it. What IS established, from the handler itself, is that the refusal the check asserts does not exist: the only state guard in create_db_shard_group.rs is `if cluster.is_deleting()`, so a stopped cluster is accepted. Fixing the guard is the work; re-replaying this check is how it is confirmed (the status waits also need the emulator to answer them promptly).

## Steps to Reproduce
Build and boot the emulator, then replay the checks in trace_ids.

Cause 1, manually:
1. `rds.describe_db_engine_versions(Engine="aurora-mysql", DefaultOnly=True)` -> note DBParameterGroupFamily F.
2. `rds.create_db_cluster_parameter_group(DBClusterParameterGroupName=G, DBParameterGroupFamily=F, Description="x")`.
3. `rds.describe_db_cluster_parameters(DBClusterParameterGroupName=G)` (and page it) -> expect the family's full default parameter set, each with its AWS Source; the emulator returns `Parameters: []`.
4. Compare with `rds.describe_engine_default_cluster_parameters(DBParameterGroupFamily=F)` -> the two name sets must be equal for a fresh group.
5. `rds.describe_db_cluster_parameters(DBClusterParameterGroupName=<unknown>)` -> expect 404 `DBParameterGroupNotFound`, message "DBClusterParameterGroup not found: <name>".

Cause 2, manually:
1. Create an aurora-postgresql cluster with a `*-limitless` EngineVersion, StorageType=aurora-iopt1, ClusterScalabilityType=limitless; wait for `available`.
2. `rds.stop_db_cluster(DBClusterIdentifier=C)`; wait until DescribeDBClusters reports `stopped`.
3. `rds.create_db_shard_group(DBShardGroupIdentifier=S, DBClusterIdentifier=C, MaxACU=16)` -> expect HTTP 400 `InvalidDBClusterStateFault` with "You can't create the DB shard group. The DB cluster C isn't in the AVAILABLE or CREATING state."; the emulator returns 200 and creates the shard group.
4. `rds.describe_db_shard_groups(DBShardGroupIdentifier=S)` -> expect `DBShardGroupNotFound`.
5. `rds.start_db_cluster(C)`, wait for `available`, repeat step 3 -> expect success.
