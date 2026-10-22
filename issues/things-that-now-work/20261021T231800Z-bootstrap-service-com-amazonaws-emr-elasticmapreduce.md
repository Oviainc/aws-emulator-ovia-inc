Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 16fab4a727e8e34e48fe5cd80cb0080f7932bd9f

# Bootstrap the EMR (elasticmapreduce) emulator

_Filed: 2026-10-21T23:18:00Z_
_Operation: service-com-amazonaws-emr-elasticmapreduce | Class: missing_

> _Note: this issue was identified against the code base at commit `16fab4a727e8e34e48fe5cd80cb0080f7932bd9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 65 operations in `services/service-com-amazonaws-emr-elasticmapreduce/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and stores nothing, so no EMR client workflow can run at all. This issue defines the FIRST, SMALL slice to implement: the job-flow (cluster) lifecycle plus step submission, which is the core end-to-end loop of EMR.

### Happy path to implement (in order)

1. **RunJobFlow** — input: `Name` (required), `Instances` (JobFlowInstancesConfig, required), plus `ReleaseLabel`, `LogUri`, `Applications`, `Configurations`, `Steps` (list<StepConfig>), `Tags`, `ServiceRole`, `JobFlowRole`, `VisibleToAllUsers`, `StepConcurrencyLevel`, `AutoScalingRole`, `ScaleDownBehavior`, `CustomAmiId`. Must mint a fresh cluster id (`j-` + uppercase alphanumerics), persist a full Cluster record built from the request (NOT echoed back blindly — it must be retrievable later), create one Step record per entry in `Steps`, and return `JobFlowId` + `ClusterArn` (`arn:aws:elasticmapreduce:<region>:<account>:cluster/<JobFlowId>`).
2. **DescribeCluster** — input: `ClusterId` (required). Must look the cluster up in state and return the stored `Cluster` with the `Name`, `ReleaseLabel`, `LogUri`, `Applications`, `Tags`, `ServiceRole`, `VisibleToAllUsers`, `ClusterArn` and `Status` that RunJobFlow persisted. Unknown `ClusterId` must raise `InvalidRequestException` (HTTP 400), not an empty 200.
3. **AddJobFlowSteps** — input: `JobFlowId` (required), `Steps` (list<StepConfig>, required), `ExecutionRoleArn`. Must append Step records (id `s-...`) to the named cluster and return `StepIds` in the same order as the submitted steps. Unknown/terminated job flow must error rather than silently succeed.
4. **ListSteps** — input: `ClusterId` (required), optional `StepStates`, `StepIds`, `Marker`. Must return `StepSummary` entries computed from the persisted steps of that cluster (EMR returns them newest-first), honouring the `StepStates` and `StepIds` filters, and emit/accept `Marker` for pagination (`@paginated(inputToken=Marker, outputToken=Marker, items=Steps)`).
5. **DescribeStep** — input: `ClusterId` (required), `StepId` (required). Must return the stored `Step` including `Name`, `Config` (Jar/Args/MainClass/Properties as submitted), `ActionOnFailure`, and `Status`. A step id that belongs to a different cluster or does not exist must raise `InvalidRequestException`.
6. **ListClusters** — optional `ClusterStates`, `CreatedAfter`, `CreatedBefore`, `Marker`. Must return `ClusterSummary` for every persisted cluster, filtered by state and creation-time window, with `Marker` pagination.
7. **TerminateJobFlows** — input: `JobFlowIds` (required). Must transition each named cluster's `Status.State` to `TERMINATING`/`TERMINATED` (recording `Timeline.EndDateTime` and a `StateChangeReason` of `USER_REQUEST`) so a subsequent DescribeCluster reflects the termination and ListClusters with `ClusterStates=[RUNNING,WAITING]` no longer returns it.

Every one of these MUST compute its response from persisted state. No canned bodies, no echoing the input back, no minting an id that is never stored. A RunJobFlow followed by DescribeCluster/ListSteps must return exactly what was created.

### State Model

Add to `ComAmazonawsEmrElasticmapreduceState` (`src/state.rs`), reached through `crate::state::service_state(&*self.engine)`:

- `clusters: BTreeMap<String /* ClusterId, "j-XXXX" */, ClusterRecord>` — insertion/creation ordered so ListClusters is deterministic.
- `ClusterRecord { id, arn, name, release_label, log_uri, service_role, job_flow_role, auto_scaling_role, scale_down_behavior, custom_ami_id, visible_to_all_users: bool, auto_terminate: bool, termination_protected: bool, step_concurrency_level: i32, applications: Vec<Application>, configurations: Vec<Configuration>, bootstrap_actions: Vec<BootstrapActionConfig>, instances: JobFlowInstancesConfig-derived summary (instance_collection_type, master/slave instance types, instance_count, ec2_key_name, ec2_subnet_id, keep_job_flow_alive_when_no_steps), tags: Vec<Tag>, status: ClusterStatusRecord, steps: Vec<StepRecord>, next_step_seq: u64 }`
- `ClusterStatusRecord { state: ClusterState, state_change_reason: Option<{code, message}>, timeline: { creation_date_time, ready_date_time, end_date_time } }`
- `StepRecord { id /* "s-XXXX" */, name, config: { jar, main_class, args: Vec<String>, properties: Map<String,String> }, action_on_failure, status: { state: StepState, state_change_reason, failure_details, timeline: { creation_date_time, start_date_time, end_date_time } }, execution_role_arn: Option<String> }`
- `next_cluster_seq: u64` for id minting.

State lives in the single shared engine, so a write by RunJobFlow is visible to every later request.

The other 58 operations (Studios, notebook executions, security configurations, instance fleets/groups, scaling policies, sessions, tagging, persistent app UIs) stay stubbed at 501 and will be filed as individual gap issues later — keep this first slice small.

## Evidence

`services/service-com-amazonaws-emr-elasticmapreduce/OPERATIONS.md` (65 operations, protocol `awsJson1_1`, `X-Amz-Target: ElasticMapReduce.<Op>`, sigv4 name `elasticmapreduce`) documents each contract used above: RunJobFlow (line 359 — `Name`/`Instances` required, output `JobFlowId` + `ClusterArn`), DescribeCluster (line 99 — in `ClusterId` required, out `Cluster`, errors `InternalServerException`/`InvalidRequestException` 400), AddJobFlowSteps (line 39 — in `JobFlowId`+`Steps` required, out `StepIds`), ListSteps (line 265 — `@paginated(inputToken=Marker, outputToken=Marker, items=Steps)`, filters `StepStates`/`StepIds`), DescribeStep (line 136 — in `ClusterId`+`StepId`, out `Step`), ListClusters (line 209 — `@paginated` on `Marker`, filters `ClusterStates`/`CreatedAfter`/`CreatedBefore`), TerminateJobFlows (line 407 — in `JobFlowIds` required, empty output). `grep -rn 'SCAFFOLDED, NOT IMPLEMENTED' services/service-com-amazonaws-emr-elasticmapreduce/src/ops/` matches all 65 handler files, e.g. `src/ops/run_job_flow.rs:4`, `src/ops/describe_cluster.rs:4`, `src/ops/list_steps.rs:4`, each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501). No tests assert any EMR behavior; only the generated `tests/state_roundtrip.rs` exists.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI EMR client at it (`aws --endpoint-url http://localhost:<port> emr ...`).
2. `RunJobFlow` with Name="demo", ReleaseLabel="emr-6.15.0", Instances={InstanceCount:3, MasterInstanceType:"m5.xlarge", SlaveInstanceType:"m5.xlarge", KeepJobFlowAliveWhenNoSteps:true} and one StepConfig — expect 200 with a `j-...` JobFlowId and a matching ClusterArn (today: HTTP 501 NotImplemented).
3. `DescribeCluster` with that ClusterId — expect the Cluster back with Name="demo", ReleaseLabel="emr-6.15.0", the ClusterArn from step 2, and a Status whose Timeline.CreationDateTime is set.
4. `AddJobFlowSteps` with the same JobFlowId and two StepConfigs — expect 200 with two `s-...` StepIds.
5. `ListSteps` with that ClusterId — expect all three steps (the RunJobFlow step plus the two added), with the names and Config.Jar/Args exactly as submitted.
6. `DescribeStep` with ClusterId plus one of the returned StepIds — expect that step's stored Name, Config and Status.
7. `ListClusters` with ClusterStates=["RUNNING","WAITING"] — expect a ClusterSummary for the created cluster.
8. `TerminateJobFlows` with JobFlowIds=[that id] — expect 200; then `DescribeCluster` shows Status.State TERMINATING/TERMINATED with StateChangeReason.Code=USER_REQUEST, and `ListClusters` with ClusterStates=["RUNNING","WAITING"] no longer lists it.
9. `DescribeCluster` with ClusterId="j-DOESNOTEXIST" — expect InvalidRequestException (HTTP 400), not an empty 200 or 501.
