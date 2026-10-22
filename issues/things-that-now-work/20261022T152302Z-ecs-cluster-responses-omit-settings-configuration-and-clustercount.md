Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-account-settings-and-resource-tagging
Issue-Step: teardown#17
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-account-settings-and-resource-tagging ecs-agent-apis-and-exec-error-paths ecs-cluster-describe-and-list-lifecycle ecs-cluster-error-paths ecs-container-instance-lifecycle ecs-daemon-error-paths-and-empty-listings ecs-describe-clusters-missing-cluster-reports-failure ecs-describe-clusters-partial-failure ecs-describe-clusters-valid-and-missing ecs-express-gateway-service-error-paths ecs-start-task-error-paths ecs-update-nonexistent-cluster-errors ecs-cluster-update-configuration-lifecycle ecs-service-lifecycle-on-cluster ecs-task-set-external-deployment-workflow ecs-taskset-basic-lifecycle ecs-taskset-errors ecs-taskset-lifecycle-ec2 ecs-capacity-provider-management ecs-list-describe-multiple-clusters ecs-cluster-capacity-provider-lifecycle ecs-cluster-create-and-settings-update ecs-cluster-create-describe-list ecs-cluster-lifecycle-settings-update ecs-cluster-multi-isolation ecs-cluster-settings-lifecycle ecs-cluster-update-generic ecs-cluster-update-settings ecs-cluster-setup-poll-endpoint-and-teardown
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|DeleteCluster|2xx->2xx|$.cluster.settings[*]
Issue-Key: ecs|DeleteCluster|2xx->2xx|$.cluster.statistics[*]
Issue-Key: ecs|DeleteCluster|2xx->2xx|$.clusterCount
Issue-Key: ecs|CreateCluster|2xx->2xx|$.cluster.settings[*]
Issue-Key: ecs|CreateCluster|2xx->2xx|$.cluster.statistics[*]
Issue-Key: ecs|CreateCluster|2xx->2xx|$.clusterCount
Issue-Key: ecs|DeleteCluster|2xx->2xx|$.cluster.tags[*]
Issue-Key: ecs|DeleteCluster|2xx->2xx|$.cluster.configuration
Issue-Commit: 659e25b97046a77449047daf7fe310a5a83846e1

# ECS cluster responses omit settings, configuration and clusterCount while fabricating statistics

> _Note: this issue was identified against the code base at commit `659e25b97046a77449047daf7fe310a5a83846e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCluster

## Description
The emulator's Cluster projection used by CreateCluster and DeleteCluster is wrong in three systematic ways: it never emits the cluster's `settings`, it never emits `configuration`, and it always emits a fabricated eight-entry `statistics` list that AWS only computes when STATISTICS is asked for. It also drops the top-level `clusterCount` member the recordings carry. The same three defects show up on ten distinct replayed clusters, listed below as separate causes because each is a distinct fixture/step a fixer must re-replay.

THIS BUNDLE CONTAINS 10 CAUSES.

Cause 1 of 10 — DeleteCluster response: settings dropped, statistics fabricated, clusterCount missing.
AWS: Smithy: DeleteCluster -> DeleteClusterResponse.cluster : Cluster; Cluster.settings : ClusterSettings, Cluster.statistics : Statistics ("Additional information about your clusters that are separated by launch type"). The recording shows a deleted cluster carrying settings=[{name: containerInsights, value: disabled}] and statistics=[]; the recorded response also carries a top-level clusterCount (recorded behavior only — the model declares no clusterCount member, so its presence is taken from the recording).
Emulator: returns cluster.settings absent, cluster.statistics populated with runningEC2TasksCount/runningFargateTasksCount/pendingEC2TasksCount/pendingFargateTasksCount/activeEC2ServiceCount/activeFargateServiceCount/drainingEC2ServiceCount/drainingFargateServiceCount all '0', and no clusterCount. HTTP status matches (200).
Why: a client reading cluster.settings to learn whether Container Insights is on sees nothing at all, and a client iterating statistics sees eight metrics AWS did not return, so metric-name-keyed code finds values where AWS reports none.
Fix: the DeleteCluster handler's cluster projection (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/delete_cluster.rs) and the shared cluster->response mapping it calls.
Traces: ecs-account-settings-and-resource-tagging#teardown#17 (+11 clustered members, e.g. ecs-cluster-error-paths#teardown#6).

Cause 2 of 10 — CreateCluster response: same three defects on create.
AWS: Smithy: CreateCluster -> CreateClusterResponse.cluster : Cluster. Recording: a freshly created cluster reports settings=[{containerInsights, disabled}] (the account default), statistics=[], and clusterCount=1.
Emulator: settings absent, the same eight fabricated statistics entries present, clusterCount absent.
Why: every create-then-read workflow starts from a response that misdescribes the cluster it just made.
Fix: services/.../src/ops/create_cluster.rs cluster projection.
Traces: ecs-agent-apis-and-exec-error-paths#setup#1 (+13 members, e.g. ecs-cluster-error-paths#setup#1).

Cause 3 of 10 — DeleteCluster on a capacity-provider cluster: settings dropped, statistics fabricated.
AWS: as Cause 1; recorded response carries settings=[{containerInsights, disabled}], statistics=[].
Emulator: settings absent, eight statistics entries emitted.
Why: same as Cause 1.
Fix: services/.../src/ops/delete_cluster.rs.
Traces: ecs-capacity-provider-management#teardown#8 (+2 members).

Cause 4 of 10 — CreateCluster: statistics fabricated and clusterCount missing.
AWS: recorded CreateCluster response has statistics=[] and a top-level clusterCount (recorded behavior only).
Emulator: eight statistics entries, no clusterCount.
Why: same as Cause 2.
Fix: services/.../src/ops/create_cluster.rs.
Traces: ecs-cluster-capacity-provider-lifecycle#task#1 (+8 members, e.g. ecs-cluster-create-describe-list#setup#1).

Cause 5 of 10 — DeleteCluster after a settings update: statistics fabricated, clusterCount missing.
AWS: recorded response: statistics=[], clusterCount=0.
Emulator: eight statistics entries, no clusterCount.
Fix: services/.../src/ops/delete_cluster.rs.
Traces: ecs-cluster-create-and-settings-update#teardown#5 (+2 members).

Cause 6 of 10 — DeleteCluster of a tagged cluster: statistics fabricated, tags echoed, clusterCount missing.
AWS: Smithy: Cluster.tags : Tags. The recorded DeleteCluster response carries no tags (tags are only materialised for TAGS-including reads) and statistics=[], clusterCount=0.
Emulator: returns cluster.tags=[{key: purpose, value: probe}] plus the eight fabricated statistics and no clusterCount.
Why: a client cannot distinguish 'no tags on this resource' from 'tags not requested' once the emulator always echoes them.
Fix: services/.../src/ops/delete_cluster.rs projection and its include-field handling.
Traces: ecs-cluster-create-describe-list#teardown#4.

Cause 7 of 10 — CreateCluster: statistics fabricated (no clusterCount assertion on this step).
AWS: recorded statistics=[].
Emulator: eight statistics entries.
Fix: services/.../src/ops/create_cluster.rs.
Traces: ecs-cluster-setup-poll-endpoint-and-teardown#setup#1.

Cause 8 of 10 — DeleteCluster: statistics fabricated.
AWS: recorded statistics=[].
Emulator: eight statistics entries.
Fix: services/.../src/ops/delete_cluster.rs.
Traces: ecs-cluster-setup-poll-endpoint-and-teardown#teardown#4.

Cause 9 of 10 — CreateCluster of several clusters in one fixture: settings dropped, statistics fabricated.
AWS: recorded settings=[{containerInsights, disabled}], statistics=[].
Emulator: settings absent, eight statistics entries.
Fix: services/.../src/ops/create_cluster.rs.
Traces: ecs-list-describe-multiple-clusters#setup#1 (+1 member).

Cause 10 of 10 — DeleteCluster of a cluster whose execute-command configuration was set: configuration dropped, statistics fabricated, tags echoed, clusterCount missing.
AWS: Smithy: Cluster.configuration : ClusterConfiguration ("The execute command and managed storage configuration for the cluster"). Recording: configuration={executeCommandConfiguration:{logging: DEFAULT}}, statistics=[], no tags, clusterCount=0.
Emulator: configuration absent entirely, eight statistics entries, tags echoed, no clusterCount.
Why: a client that reads back the cluster's execute-command configuration gets nothing, so it cannot verify the setting it just stored.
Fix: services/.../src/ops/delete_cluster.rs projection plus the cluster record in services/.../src/state.rs that never carries configuration.
Traces: ecs-cluster-settings-lifecycle#teardown#6.

## Evidence
Representative (ecs-account-settings-and-resource-tagging#teardown#17, DeleteCluster, expected 200 / actual 200):
  $.cluster.settings[0]: missing (expected {'name': 'containerInsights', 'value': 'disabled'}, got None)
  $.cluster.statistics[0..7]: extra (expected None, got {'name': 'runningEC2TasksCount', 'value': '0'} ... {'name': 'drainingFargateServiceCount', 'value': '0'})
  $.clusterCount: missing (expected '<shaped-not-asserted>', got None)
CreateCluster (ecs-agent-apis-and-exec-error-paths#setup#1): same settings/statistics deltas, $.clusterCount expected 1, got None.
Configuration (ecs-cluster-settings-lifecycle#teardown#6): $.cluster.configuration: missing (expected {'executeCommandConfiguration': {'logging': 'DEFAULT'}}, got None).
Tags (ecs-cluster-create-describe-list#teardown#4): $.cluster.tags[0]: extra (expected None, got {'key': 'purpose', 'value': 'probe'}).

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 corpus against the emulator (replay_wire_corpus).
2. Inspect fixture ecs-account-settings-and-resource-tagging step teardown#17 (DeleteCluster) and ecs-agent-apis-and-exec-error-paths step setup#1 (CreateCluster).
3. Equivalently by hand: CreateCluster{clusterName:X} then DeleteCluster{cluster:X} and compare the returned cluster object against the recorded one — settings and configuration are absent, statistics carries eight invented entries, clusterCount is absent.
