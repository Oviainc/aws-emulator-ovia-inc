Issue-Source: trace-sweep-divergence
Issue-Trace: route53-recovery-cluster-routing-control-state-lifecycle
Issue-Step: setup#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53-recovery-cluster-routing-control-state-lifecycle
Issue-Shape-Id: com.amazonaws.route53recoverycluster#ToggleCustomerAPI
Issue-Key: route53-recovery-control-config|DescribeCluster|2xx->2xx|$.Cluster.ClusterEndpoints[*].Region
Issue-Commit: 159c2db91a4e6757ae457414a0d5109b84d38779

# DescribeCluster publishes ClusterEndpoints in a fixed alphabetical Region order, so the first entry is never the cluster's own Region

> _Note: this issue was identified against the code base at commit `159c2db91a4e6757ae457414a0d5109b84d38779`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCluster

## Description
Cause 1 of 1: the order in which a cluster's five Regional data-plane endpoints are published is wrong. The emulator emits them in a hard-coded alphabetical Region order, so Cluster.ClusterEndpoints[0].Region is always the alphabetically-first Region ('ap-northeast-1') rather than the Region the cluster is created in and addressed on, and a client that reads the first endpoint is pointed at the wrong Regional endpoint for the whole run.

AWS: the model constrains the members but not their order — Smithy (com.amazonaws.route53recoverycontrolconfig): Cluster.ClusterEndpoints -> list __listOfClusterEndpoint; ClusterEndpoint.Region -> __stringMin1Max32PatternS (@length 1..32, @pattern ^\S+$), documented 'The Amazon Web Services Region for a cluster endpoint'; ClusterEndpoint.Endpoint -> __stringMin1Max128PatternAZaZ09; Cluster is documented as 'A set of five redundant Regional endpoints against which you can execute API calls to update or get the state of routing controls.' So for ordering this is recorded behavior only: the recording shows DescribeCluster answering HTTP 200 with ClusterEndpoints[0].Region equal to the Region the cluster itself lives in and is being addressed on (us-west-2 in the recording — 'us-west-2' is the recorded run's Region, a per-environment value, not the requirement). Hypothesis: AWS orders the five Regional entries with the cluster's own/home Region first and the remaining redundant Regions after it. The requirement this issue states is the general one the recording supports: the first ClusterEndpoints entry is the entry for the cluster's own Region, not a constant Region chosen independently of the cluster.

Emulator: ClusterEndpoints is built by iterating a constant table in alphabetical order, so the first entry's Region is always 'ap-northeast-1' whatever Region the cluster belongs to. The replay polled this step 60 times and the value never changed: '$.Cluster.ClusterEndpoints[0].Region: poll: not in ['us-west-2'] after 60 attempt(s) (expected None, got 'ap-northeast-1')'. HTTP status is 200 as expected; only the list order diverges.

Why: a client that follows the common shortcut of using ClusterEndpoints[0] (the recording's consumer does exactly this) builds its route53-recovery-cluster data-plane client against a Region it is not operating in. Because this data plane has no generic regional endpoint, that is the difference between addressing the local Regional endpoint and addressing a remote one: latency and failover behaviour change, and region-pinned tooling (and any assertion that the first published endpoint is the local one) never converges — it polls to timeout, as the fixture step does.

Fix: the cluster-endpoint projection in the route53-recovery-control-config state module — the helper that mints the (Region, Endpoint) pairs at CreateCluster time must order them with the cluster's own Region first, and the record projection DescribeCluster/ListClusters answer from must preserve that order. File/line citations: services/service-com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig/src/state.rs lines 101-107 (CLUSTER_ENDPOINT_REGIONS, alphabetical) and lines 130-145 (cluster_endpoints maps that table in order), stored at src/ops/create_cluster.rs line 270 and projected unchanged by src/ops/describe_cluster.rs lines 185-187 (record.to_cluster()).

Traces: route53-recovery-cluster-routing-control-state-lifecycle#setup#3.

Note on already-open issues: 20261109T160848Z-route53-recovery-cluster-data-plane-is-unreachable-the.md and 20261109T193409Z-route53-recovery-control-config-publishes-real-aws-clusterendpoints.md both concern the HOST FORM of ClusterEndpoints[].Endpoint (real-AWS hostnames the emulator does not serve). Neither concerns the ORDER of the five entries, and neither carries this replay key, so this ordering defect is net-new; fixing the host form alone leaves this step failing.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture route53-recovery-cluster-routing-control-state-lifecycle, step setup#3, operation DescribeCluster, verdict mismatch.

  Expected: HTTP 200, $.Cluster.ClusterEndpoints[0].Region = the cluster's own Region (recorded: 'us-west-2')
  Actual:   HTTP 200, $.Cluster.ClusterEndpoints[0].Region = 'ap-northeast-1'
  detail: "$.Cluster.ClusterEndpoints[0].Region: poll: not in ['us-west-2'] after 60 attempt(s) (expected None, got 'ap-northeast-1')"
  divergence_key: service=route53-recovery-control-config|DescribeCluster|status=200->200|keys=$.Cluster.ClusterEndpoints[0].Region

The step polls, so this is not a settling delay: the value is stable and wrong for 60 attempts. The Region names themselves are not per-run values — they come from a constant table in the emulator:
  services/service-com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig/src/state.rs:101-107
    pub const CLUSTER_ENDPOINT_REGIONS: &[&str] = &["ap-northeast-1", "ap-southeast-2", "eu-west-1", "us-east-1", "us-west-2"];
  services/.../src/state.rs:130-145  cluster_endpoints() maps that slice in iteration order
  services/.../src/ops/describe_cluster.rs:185-187  projects record.to_cluster() unchanged

## Steps to Reproduce
1. Call route53-recovery-control-config CreateCluster(ClusterName=<name>) and poll DescribeCluster(ClusterArn=<arn>) until Cluster.Status == DEPLOYED.
2. Read Cluster.ClusterEndpoints from the DescribeCluster response.
   Expected (AWS, recorded): five {Region, Endpoint} entries whose FIRST entry is the one for the cluster's own Region (the recorded run's first entry Region is 'us-west-2', the Region the cluster was created in).
   Observed: five entries always ordered ap-northeast-1, ap-southeast-2, eu-west-1, us-east-1, us-west-2 — so ClusterEndpoints[0].Region is 'ap-northeast-1' regardless of the cluster's Region.
3. Repeat the DescribeCluster poll: the order never changes, so no amount of waiting makes the first entry the local Region.
4. Re-run the fixture to confirm: replay_wire_corpus on fixture route53-recovery-cluster-routing-control-state-lifecycle, step setup#3 must pass.
