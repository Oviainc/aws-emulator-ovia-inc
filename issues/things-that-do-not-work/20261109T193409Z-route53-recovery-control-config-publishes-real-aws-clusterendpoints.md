Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53-recovery-control-config.routing-control-state-toggle-via-recovery-cluster-data-plane route53-recovery-control-config.safety-rule-threshold-exceeds-controls-rejected
Issue-Shape-Id: com.amazonaws.route53recoverycontrolconfig#Route53RecoveryControlConfig
Issue-Key: route53-recovery-control-config.routing-control-state-toggle-via-recovery-cluster-data-plane|_cluster_data_client#4|PublishedUrlRejected|return ctx.client("route53-recovery-cluster", endpoint_url=match["Endpoint"])
Issue-Key: route53-recovery-control-config.safety-rule-threshold-exceeds-controls-rejected|run#6|PublishedUrlRejected|ep_url = ctx.endpoint_url(ep["Endpoint"])
Issue-Commit: 7e082a9dfef7650059a714e1b731386462df20dc

# route53-recovery-control-config publishes real-AWS ClusterEndpoints, so its own cross-plane checks cannot reach the cluster data plane

> _Note: this issue was identified against the code base at commit `7e082a9dfef7650059a714e1b731386462df20dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCluster

## Description
One net-new root cause in this service, blocking the two route53-recovery-control-config checks that follow AWS's documented cluster-endpoint discovery path into the data plane.

Related (does NOT cover these checks): open issue 20261109T160848Z-route53-recovery-cluster-data-plane-is-unreachable-the.md records the same endpoint-projection defect, but it is filed under service route53-recovery-cluster and its Issue-Keys list only the 17 route53-recovery-cluster data-plane checks. The two checks here are route53-recovery-control-config checks and are carried by no open issue, so they are filed here; fixing the projection should close both.

Cause 1 of 1: CreateCluster/DescribeCluster synthesize Cluster.ClusterEndpoints[].Endpoint as a real AWS production hostname rather than an address on the emulator's own listener.

AWS: route53-recovery-cluster has no generic regional endpoint — a caller MUST address one of the Regional endpoints the cluster itself publishes. Smithy (route53-recovery-control-config): Cluster.ClusterEndpoints -> list of ClusterEndpoint; ClusterEndpoint.Endpoint -> __stringMin1Max128PatternAZaZ09 (@length 1..128, @pattern ^[A-Za-z0-9:.\\/_-]*$), documented as "A cluster endpoint. Specify an endpoint and Amazon Web Services Region when you want to set or retrieve a routing control state in the cluster"; ClusterEndpoint.Region -> __stringMin1Max32PatternS; Cluster is documented as "A set of five redundant Regional endpoints against which you can execute API calls to update or get the state of routing controls." The modeled requirement is therefore that the value in ClusterEndpoint.Endpoint is an address at which that cluster's GetRoutingControlState / UpdateRoutingControlState / UpdateRoutingControlStates / ListRoutingControls can actually be invoked. No particular hostname is modeled, and the host is per-environment, so for this emulator the requirement is that the published endpoint names a host the emulator itself answers on (its own announced host:port, or a per-resource listener), with Region kept as the AWS region label and the string kept inside @length 1..128 / the modeled pattern.

Emulator: DescribeCluster (and CreateCluster, from which the value is stored) project an Endpoint of the form https://<cluster-id>.route53-recovery-cluster.<region>.<suffix>/v1 — suffix amazonaws.com for IPV4 and api.aws for DUALSTACK — a host the emulator does not listen on and that resolves, if at all, to real AWS. Both checks read Cluster.ClusterEndpoints from DescribeCluster, select the entry whose Region matches the run region, and are refused before any data-plane request leaves the client: simparity.published.PublishedUrlRejected "<client|endpoint_url> was given https://<cluster-id>.route53-recovery-cluster.<region>.amazonaws.com/v1 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://<own host>:<port>/<path>". The cluster id, the region label, the scheme and the loopback port in that text are per-run values; the defect is the host FORM, not those values.

Why: a client that follows AWS's documented discovery path (DescribeCluster, then call the Regional endpoint) cannot reach this emulator's recovery-cluster data plane at all — it either fails DNS/hangs or, worse, is pointed at real AWS and mutates production routing-control state. It also makes the cross-plane behavior these two checks assert unobservable: a routing control created on the config plane can never be read as Off or toggled On/Off on the data plane, and the gating that an unsatisfiable ATLEAST safety rule must impose on UpdateRoutingControlState can never be exercised (that gating defect itself is open issue 20261030T111123Z, and is now masked because the check dies at client construction instead of on its assertion).

Fix: the cluster-endpoint projection in this service's state module and the record the create handler stores — the helper that mints ClusterEndpoints at CreateCluster time, and the projection DescribeCluster/ListClusters answer from, must publish the emulator's own announced host and port plus the path prefix on which it serves the ToggleCustomerAPI data plane, one entry per Regional label. File/line citations under Traces.

Traces: route53-recovery-control-config.routing-control-state-toggle-via-recovery-cluster-data-plane (refused building the client from the DescribeCluster-published endpoint) and route53-recovery-control-config.safety-rule-threshold-exceeds-controls-rejected (refused in ctx.endpoint_url on the same value). Source of the published value: services/service-com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig/src/state.rs:101-107 (CLUSTER_ENDPOINT_REGIONS) and :130-145 (cluster_endpoints, format!("https://{cluster_id}.route53-recovery-cluster.{region}.{suffix}/v1")), stored by services/service-com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig/src/ops/create_cluster.rs around line 270 and projected unchanged by describe_cluster.

## Evidence
Suite: route53-recovery-control-config 17/21 (81%), 3 mismatched, 1 errored. Two of the failures share this single error detail.

Expected (AWS / emulator published-URL contract): DescribeCluster(ClusterArn=<arn>).Cluster.ClusterEndpoints is a list of five {Region, Endpoint} entries whose Endpoint is an address the serving environment answers on; building a route53-recovery-cluster client against the entry for the run region and calling GetRoutingControlState / UpdateRoutingControlState reaches the data plane (HTTP 200 for a valid request, the modeled refusal otherwise).

Actual (emulator):
  [mismatch] route53-recovery-control-config.routing-control-state-toggle-via-recovery-cluster-data-plane
    File "context.py", line 861, in _build_client
      target = self._published_target(endpoint_url, "client") if endpoint_url else None
    File "context.py", line 586, in _published_target
      raise PublishedUrlRejected(...)
    simparity.published.PublishedUrlRejected: client was given https://6213ff87-b93c-1f94-ec8c-be42c5698553.route53-recovery-cluster.us-west-2.amazonaws.com/v1 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:39301/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener

  [mismatch] route53-recovery-control-config.safety-rule-threshold-exceeds-controls-rejected
    File "context.py", line 572, in endpoint_route
      return self._published_target(url, "endpoint_url"), {}
    File "context.py", line 586, in _published_target
      raise PublishedUrlRejected(...)
    simparity.published.PublishedUrlRejected: endpoint_url was given https://da1516a5-bf23-4715-231f-2ba94504e17d.route53-recovery-cluster.us-west-2.amazonaws.com/v1 ... its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:37485/<resource path>, ...

The cluster uuids, 'us-west-2' and the 127.0.0.1 ports above are per-run values, not requirements. The requirement is that the published host is one the emulator serves.

Source of the published value, services/service-com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig/src/state.rs:130-145:
  pub fn cluster_endpoints(cluster_id: &str, network_type: &str) -> Vec<(String, String)> {
      let suffix = if network_type == "DUALSTACK" { "api.aws" } else { "amazonaws.com" };
      CLUSTER_ENDPOINT_REGIONS.iter().map(|region| ((*region).to_string(),
          format!("https://{cluster_id}.route53-recovery-cluster.{region}.{suffix}/v1")))
          .collect()
  }

## Steps to Reproduce
1. route53-recovery-control-config CreateCluster(ClusterName="sim2real-r53arc-cluster-<x>") (NetworkType omitted => IPV4); poll DescribeCluster until Cluster.Status == DEPLOYED.
2. Read DescribeCluster(ClusterArn=<arn>).Cluster.ClusterEndpoints.
   Observed: five entries with Endpoint = https://<cluster-id>.route53-recovery-cluster.<region>.amazonaws.com/v1 for region in {ap-northeast-1, ap-southeast-2, eu-west-1, us-east-1, us-west-2} (api.aws when NetworkType=DUALSTACK).
   Expected: five entries whose Endpoint is on a host the emulator serves (its own announced host:port plus the path it serves the route53-recovery-cluster data plane on), one per Regional label.
3. Pick the entry whose Region equals the run region and construct a route53-recovery-cluster client with endpoint_url = that Endpoint.
   Observed: refused — the published URL's host is not one of the emulator's own hosts (PublishedUrlRejected). A real botocore client built on it would leave the emulator and address AWS.
4. Consequently neither of the two cross-plane sequences can run: (a) CreateControlPanel + CreateRoutingControl on the config plane, then GetRoutingControlState (expect "Off") and UpdateRoutingControlState On/Off with read-back; (b) CreateSafetyRule with an AssertionRule RuleConfig {Type: ATLEAST, Threshold: 3} over 2 AssertedControls, then UpdateRoutingControlState on those controls (expect a ValidationException naming the rule) and the same transition succeeding again after DeleteSafetyRule.
