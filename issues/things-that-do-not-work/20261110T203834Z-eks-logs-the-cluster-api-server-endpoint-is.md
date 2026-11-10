Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks__logs.audit-log-record-names-the-access-entry-username-for-allow-and-forbid
Issue-Key: eks__logs.audit-log-record-names-the-access-entry-username-for-allow-and-forbid|_routed_request#9|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Commit: d523205a25e89799b79f6d7553b80badf0bb176e

# eks__logs: the cluster API-server endpoint is published on a real eks.amazonaws.com host the emulator does not serve, so no Kubernetes audit-log record can be produced

> _Note: this issue was identified against the code base at commit `d523205a25e89799b79f6d7553b80badf0bb176e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCluster

## Description
Cause 1 of 1: Cluster.endpoint is published as a real-AWS Kubernetes API hostname the emulator does not serve, so the audit-log path that depends on driving the Kubernetes API cannot be exercised at all.
AWS: Smithy (com.amazonaws.eks#Cluster$endpoint): "The endpoint for your Kubernetes API server." On AWS that host serves the cluster's Kubernetes API, and with the `audit` control-plane log type enabled every authorized and forbidden request through it produces an audit record in the cluster's CloudWatch Logs group /aws/eks/<cluster>/cluster, whose `user.username` is the username of the matching EKS access entry. Recorded behavior: the check authenticates to the published endpoint, performs an allowed and a forbidden request, and then reads those two audit records back from CloudWatch Logs.
Emulator: DescribeCluster projects endpoint as https://<32-hex>.<3-hex>.<region>.eks.amazonaws.com, an AWS-owned address rather than one of the emulator's own hosts, and the harness refuses it before any request is made: "simparity.published.PublishedUrlRejected: endpoint_url was given https://<ID>.<xxx>.<region>.eks.amazonaws.com/api/v1/namespaces (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". The hex host id, the region and the port are per-run values; the requirement is that the announced host be an address the emulator serves.
Why: a client that follows the endpoint the API gives it is sent somewhere the emulator never answers, so neither the allow nor the forbid request ever reaches an API server and no audit record is ever emitted to CloudWatch Logs — the cross-service behavior (access-entry username appearing in the audit stream) is unobservable even though the cluster reports ACTIVE and logging reports enabled.
Fix: the Cluster response projection for `endpoint` (the EKS cluster serializer/state that mints the host) must announce an address on a listener the emulator serves, and that listener must route the cluster's Kubernetes API path space and feed the audit log group. Open issues 20261109T141609Z-ecr-eks-logs-a-cluster-s-published-api.md and 20261109T160052Z-eks-3-net-new-root-causes-cluster-endpoint.md describe the same shape of defect under other headings/check sets, but neither carries this check's replay key.
Traces: eks__logs.audit-log-record-names-the-access-entry-username-for-allow-and-forbid.

## Evidence
Expected: Cluster.endpoint on a host the emulator serves; an allowed and a forbidden Kubernetes request through it; two audit records in /aws/eks/<cluster>/cluster naming the access entry's username. Actual: the published endpoint is https://<32-hex>.<3-hex>.<region>.eks.amazonaws.com and the first request against it fails with 'simparity.published.PublishedUrlRejected: endpoint_url was given https://CC01C0EDA5535232F081916C10B413CF.cc0.us-west-2.eks.amazonaws.com/api/v1/namespaces (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:33871/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener' — the hex id and the port 33871 are per-run values, the divergence is the amazonaws.com host itself.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite (run_behavioral_suite).
2. Replay eks__logs.audit-log-record-names-the-access-entry-username-for-allow-and-forbid: CreateCluster with API authentication mode and clusterLogging types including 'audit', wait for ACTIVE, create an access entry with a username, then read cluster.endpoint from DescribeCluster and issue GET /api/v1/namespaces against it with the cluster CA and a bearer token.
3. Observe that the endpoint host is <id>.<region>.eks.amazonaws.com rather than an address the emulator serves, so the request never reaches an API server and no audit record is written to /aws/eks/<cluster>/cluster.
