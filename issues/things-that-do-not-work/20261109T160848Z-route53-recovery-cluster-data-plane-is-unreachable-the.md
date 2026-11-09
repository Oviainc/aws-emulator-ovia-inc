Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53-recovery-cluster.access-denied-missing-permission-rejected route53-recovery-cluster.assertion-rule-blocks-violating-update-rejected route53-recovery-cluster.batch-update-service-limit-exceeded-rejected route53-recovery-cluster.concurrent-updates-same-routing-control-no-corruption route53-recovery-cluster.gating-rule-blocks-target-until-gate-open route53-recovery-cluster.list-routing-controls-nonexistent-control-panel-not-found route53-recovery-cluster.list-routing-controls-pagination-next-token route53-recovery-cluster.list-routing-controls-reflects-current-state route53-recovery-cluster.resource-not-found-nonexistent-routing-control route53-recovery-cluster.routing-control-independent-state-isolation route53-recovery-cluster.routing-control-state-toggle-round-trip route53-recovery-cluster.routing-controls-scoped-to-control-panel route53-recovery-cluster.safety-rule-override-bypasses-assertion-conflict route53-recovery-cluster.update-routing-control-state-idempotent-same-state-no-conflict route53-recovery-cluster.update-routing-control-states-batch-atomic-multi-control route53-recovery-cluster.update-routing-control-states-preserves-unspecified-controls-state route53-recovery-cluster.validation-exception-malformed-routing-control-arn-rejected
Issue-Shape-Id: com.amazonaws.route53recoverycluster#ToggleCustomerAPI
Issue-Key: route53-recovery-cluster.access-denied-missing-permission-rejected|_cluster_data_client#5|PublishedUrlRejected|return ctx.client("route53-recovery-cluster", endpoint_url=match["Endpoint"])
Issue-Key: route53-recovery-cluster.assertion-rule-blocks-violating-update-rejected|_cluster_data_client#3|PublishedUrlRejected|return ctx.client("route53-recovery-cluster", endpoint_url=match["Endpoint"])
Issue-Key: route53-recovery-cluster.batch-update-service-limit-exceeded-rejected|_cluster_data_client#3|PublishedUrlRejected|return ctx.client("route53-recovery-cluster", endpoint_url=match["Endpoint"])
Issue-Key: route53-recovery-cluster.concurrent-updates-same-routing-control-no-corruption|_cluster_data_client#3|PublishedUrlRejected|return ctx.client("route53-recovery-cluster", endpoint_url=match["Endpoint"])
Issue-Key: route53-recovery-cluster.gating-rule-blocks-target-until-gate-open|_cluster_data_client#3|PublishedUrlRejected|return ctx.client("route53-recovery-cluster", endpoint_url=match["Endpoint"])
Issue-Key: route53-recovery-cluster.list-routing-controls-nonexistent-control-panel-not-found|_cluster_data_client#5|PublishedUrlRejected|return ctx.client("route53-recovery-cluster", endpoint_url=match["Endpoint"])
Issue-Key: route53-recovery-cluster.list-routing-controls-pagination-next-token|_cluster_data_client#3|PublishedUrlRejected|return ctx.client("route53-recovery-cluster", endpoint_url=match["Endpoint"])
Issue-Key: route53-recovery-cluster.list-routing-controls-reflects-current-state|_cluster_data_client#5|PublishedUrlRejected|return ctx.client("route53-recovery-cluster", endpoint_url=match["Endpoint"])
Issue-Key: route53-recovery-cluster.resource-not-found-nonexistent-routing-control|_cluster_data_client#3|PublishedUrlRejected|return ctx.client("route53-recovery-cluster", endpoint_url=match["Endpoint"])
Issue-Key: route53-recovery-cluster.routing-control-independent-state-isolation|_cluster_data_client#3|PublishedUrlRejected|return ctx.client("route53-recovery-cluster", endpoint_url=match["Endpoint"])
Issue-Key: route53-recovery-cluster.routing-control-state-toggle-round-trip|_cluster_data_client#3|PublishedUrlRejected|return ctx.client("route53-recovery-cluster", endpoint_url=match["Endpoint"])
Issue-Key: route53-recovery-cluster.routing-controls-scoped-to-control-panel|_cluster_data_client#4|PublishedUrlRejected|return ctx.client("route53-recovery-cluster", endpoint_url=match["Endpoint"])
Issue-Key: route53-recovery-cluster.safety-rule-override-bypasses-assertion-conflict|_cluster_data_client#3|PublishedUrlRejected|return ctx.client("route53-recovery-cluster", endpoint_url=match["Endpoint"])
Issue-Key: route53-recovery-cluster.update-routing-control-state-idempotent-same-state-no-conflict|_cluster_data_client#3|PublishedUrlRejected|return ctx.client("route53-recovery-cluster", endpoint_url=match["Endpoint"])
Issue-Key: route53-recovery-cluster.update-routing-control-states-batch-atomic-multi-control|_cluster_data_client#3|PublishedUrlRejected|return ctx.client("route53-recovery-cluster", endpoint_url=match["Endpoint"])
Issue-Key: route53-recovery-cluster.update-routing-control-states-preserves-unspecified-controls-state|_cluster_data_client#3|PublishedUrlRejected|return ctx.client("route53-recovery-cluster", endpoint_url=match["Endpoint"])
Issue-Key: route53-recovery-cluster.validation-exception-malformed-routing-control-arn-rejected|_cluster_data_client#3|PublishedUrlRejected|return ctx.client("route53-recovery-cluster", endpoint_url=match["Endpoint"])
Issue-Commit: 4d9e3fb251011fe781d884d5ecb8db5bb1e34e6b

# route53-recovery-cluster data plane is unreachable: the cluster's published ClusterEndpoints are real-AWS hosts (https://<id>.route53-recovery-cluster.<region>.amazonaws.com/v1) the emulator does not serve, so all 17 data-plane checks fail at client construction

> _Note: this issue was identified against the code base at commit `4d9e3fb251011fe781d884d5ecb8db5bb1e34e6b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCluster

## Description
One root cause, blocking every route53-recovery-cluster (ToggleCustomerAPI) behavioral check in the suite (0/17 passed; the previous run of this service passed 11/17, so the whole data plane has become unaddressable).

Cause 1 of 1: the cluster data-plane endpoints the recovery-control-config plane publishes are synthesized as real AWS production hostnames instead of an address on the emulator's own listener.

AWS: route53-recovery-cluster has NO generic regional endpoint — a caller MUST address one of the five Regional endpoints the cluster publishes. Smithy (route53-recovery-control-config): Cluster.ClusterEndpoints -> list __listOfClusterEndpoint; ClusterEndpoint.Endpoint -> __stringMin1Max128PatternAZaZ09 (@length 1..128, @pattern ^[A-Za-z0-9:.\\/_-]*$), documented as "A cluster endpoint. Specify an endpoint and Amazon Web Services Region when you want to set or retrieve a routing control state in the cluster"; ClusterEndpoint.Region -> __stringMin1Max32PatternS. Cluster is documented as "A set of five redundant Regional endpoints against which you can execute API calls to update or get the state of routing controls." The REQUIREMENT the model states is that the value in ClusterEndpoint.Endpoint is an address at which that cluster's GetRoutingControlState / UpdateRoutingControlState / UpdateRoutingControlStates / ListRoutingControls can actually be invoked; on AWS it is a host AWS serves. Nothing in the model requires a particular hostname, and the hostname is per-environment — so for this emulator the requirement is that DescribeCluster/CreateCluster publish an endpoint on a host the emulator itself answers on (its own listener host:port, or a per-resource listener it stands up), exactly as the published-URL contract states: "a customer of the emulator needs http(s)://<own host>:<port>/<path>".

Emulator: DescribeCluster/CreateCluster project an Endpoint string of the form https://<cluster-id>.route53-recovery-cluster.<region>.<suffix>/v1 — an amazonaws.com (IPV4) or api.aws (DUALSTACK) host that the emulator does not listen on and that resolves, if at all, to real AWS. Every check builds its data-plane client from DescribeCluster(ClusterArn).Cluster.ClusterEndpoints, picks the entry whose Region matches the run region, and is refused before any data-plane request is sent: simparity.published.PublishedUrlRejected: "client was given https://<id>.route53-recovery-cluster.us-west-2.amazonaws.com/v1 (published by the emulator or built by the check): its host is not one of the emulator's own hosts". The cluster-id, region, port and scheme are per-run values; the defect is the host form, not those values. The five region names and the hostname template come from the recovery-control-config state module (the cluster_endpoints helper and its CLUSTER_ENDPOINT_REGIONS table), which formats the hostname from the cluster id, the region and an AWS DNS suffix with no reference to the engine's own published address.

Why: the entire route53-recovery-cluster data plane is unreachable for any client that follows AWS's documented discovery path (describe the cluster, then call the Regional endpoint). Routing-control toggles, batch updates, safety-rule gating, pagination and every negative/refusal class can neither be exercised nor relied upon; a client either hangs/fails DNS or, worse, is pointed at real AWS. It also masks previously observable fidelity defects: the three causes in open issue 20261029T203927Z-route53-recovery-cluster-data-plane-3-net-new.md (no IAM authorization gate on GetRoutingControlState, emulator-authored ServiceLimitExceededException wording, emulator-authored malformed-ARN ValidationException wording) can no longer be reached at all, because their checks now die at client construction rather than on the assertion that issue records.

Fix: the cluster-endpoint projection in the route53-recovery-control-config service's state/cluster record — the helper that mints ClusterEndpoints at CreateCluster time and the record projection DescribeCluster/ListClusters answer from must publish the emulator's own announced host and port (and the path prefix it serves the ToggleCustomerAPI data plane on) for each of the five Regional entries, keeping ClusterEndpoint.Region as the AWS region label and keeping the value inside the modeled @length 1..128 / ^[A-Za-z0-9:.\\/_-]*$ bounds. File/line citations: services/service-com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig/src/state.rs lines 97-107 (CLUSTER_ENDPOINT_REGIONS) and lines 125-145 (cluster_endpoints: format!("https://{cluster_id}.route53-recovery-cluster.{region}.{suffix}/v1")), consumed at services/service-com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig/src/ops/create_cluster.rs line 270.

Traces: all 17 failing checks listed in trace_ids; each one fails identically in the harness's _build_client/_published_target with PublishedUrlRejected on the DescribeCluster-published endpoint.

## Evidence
Suite: route53-recovery-cluster 0/17 (0%), 17 mismatches, 0 errored, 0 skipped. All 17 share one error detail.

Expected (AWS / emulator contract): DescribeCluster(ClusterArn=<arn>).Cluster.ClusterEndpoints is a list of five {Region, Endpoint} entries whose Endpoint is an address the serving environment answers on; constructing a route53-recovery-cluster client against the entry for the run region and calling GetRoutingControlState / UpdateRoutingControlState(s) / ListRoutingControls reaches the cluster data plane (HTTP 200 for a valid request, the modeled refusal otherwise).

Actual (emulator), e.g. route53-recovery-cluster.routing-control-state-toggle-round-trip and all 16 others:
  File "context.py", line 861, in _build_client
    target = self._published_target(endpoint_url, "client") if endpoint_url else None
  File "context.py", line 586, in _published_target
    raise PublishedUrlRejected(...)
  simparity.published.PublishedUrlRejected: client was given https://adb14d5d-2bbe-782d-4686-79aadf6207d9.route53-recovery-cluster.us-west-2.amazonaws.com/v1 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:34597/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener

(The uuid 'adb14d5d-...' is the per-run cluster id, 'us-west-2' the run region and '127.0.0.1:34597' the run's listener — per-run values, not requirements. The requirement is that the published host is one the emulator serves.)

Source of the published value: services/service-com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig/src/state.rs:130-145
  pub fn cluster_endpoints(cluster_id: &str, network_type: &str) -> Vec<(String, String)> {
      let suffix = if network_type == "DUALSTACK" { "api.aws" } else { "amazonaws.com" };
      ... format!("https://{cluster_id}.route53-recovery-cluster.{region}.{suffix}/v1")
stored at create_cluster.rs:270 and projected unchanged by DescribeCluster.

Regression context: the open issue 20261029T203927Z-route53-recovery-cluster-data-plane-3-net-new.md records 11/17 passing for this service, i.e. the data-plane client could previously be built; its three recorded assertion failures are now unreachable because every check stops at client construction.

## Steps to Reproduce
1. Create an ARC cluster on the emulator: route53-recovery-control-config CreateCluster(ClusterName="sim2real-behav-r53arc-shared") (NetworkType omitted => IPV4), and poll DescribeCluster until Cluster.Status == DEPLOYED.
2. Read Cluster.ClusterEndpoints from DescribeCluster(ClusterArn=<arn>).
   Observed: five entries whose Endpoint is https://<cluster-id>.route53-recovery-cluster.<region>.amazonaws.com/v1 for region in {ap-northeast-1, ap-southeast-2, eu-west-1, us-east-1, us-west-2} (api.aws when NetworkType=DUALSTACK).
   Expected: five entries whose Endpoint is on a host the emulator serves (its own announced host:port and the path it serves the route53-recovery-cluster data plane on), one per Regional label.
3. Pick the entry whose Region equals the run region and build a route53-recovery-cluster client with endpoint_url=<that Endpoint>.
   Observed: the client cannot be created against the emulator — the published URL's host is not one of the emulator's own hosts (PublishedUrlRejected). Pointing a real botocore client at it would leave the emulator entirely and address AWS.
4. Any data-plane call that would follow (CreateRoutingControl in the cluster's default control panel, then UpdateRoutingControlState(RoutingControlArn, "On") and GetRoutingControlState) is therefore never sent; all 17 route53-recovery-cluster checks fail at this step.
