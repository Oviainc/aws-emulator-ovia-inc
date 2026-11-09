Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.addon-configuration-values-reach-the-in-cluster-workload eks.cluster-kubernetes-api-server-serves-advertised-version eks.eks-kubectl-namespace eks.e2e-cluster-delete-blocked-by-active-nodegroup-rejected eks.e2e-cluster-nodegroup-podidentity-teardown-flow eks.ekscc-update-cluster-config-while-creating-rejected
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.addon-configuration-values-reach-the-in-cluster-workload|_adopt_or_create_cluster#2|PublishedUrlRejected|return (ctx.endpoint_url(cluster["endpoint"]), ca_pem), cluster
Issue-Key: eks.cluster-kubernetes-api-server-serves-advertised-version|run#11|AssertionError|assert not (p := ctx.published_url_mismatch(endpoint)), p
Issue-Key: eks.eks-kubectl-namespace|_write_kubeconfig#9|PublishedUrlRejected|server = ctx.endpoint_url(cluster["endpoint"])
Issue-Key: eks.e2e-cluster-delete-blocked-by-active-nodegroup-rejected|_expect_rejection#13|AssertionError|raise AssertionError(f"expected the call to be rejected with ResourceInUseException, but it SUCCEEDED and returned {result!r}")
Issue-Key: eks.e2e-cluster-nodegroup-podidentity-teardown-flow|_error_code#20|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded with {result!r}")
Issue-Key: eks.ekscc-update-cluster-config-while-creating-rejected|run#10|AssertionError|assert message == want, f"expected message {want!r}, got {message!r}"
Issue-Commit: d115796fe5713602f1adc3e09d553e6de30aa6e5

# eks: 3 net-new root causes — cluster.endpoint is published as a real eks.amazonaws.com host, DeleteCluster destroys a cluster with an attached nodegroup, and the CREATING-cluster UpdateClusterConfig refusal is emulator-worded

> _Note: this issue was identified against the code base at commit `d115796fe5713602f1adc3e09d553e6de30aa6e5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCluster

## Description
Three independent defects in the emulator's EKS surface, all grounded in failing behavioral checks of this suite. None of them is marked ALREADY FILED or OWNED in the replay report.

Cause 1 of 3: Cluster.endpoint is published as a real-AWS Kubernetes API hostname the emulator does not serve.
AWS: Smithy: Cluster.endpoint — "The endpoint for your Kubernetes API server." On AWS that host is reachable and serves the cluster's Kubernetes API (/version, /api, /apis/...). Recorded behavior only for the address format: the recording's clients connect to the endpoint the API hands them and get Kubernetes responses.
Emulator: CreateCluster/DescribeCluster/DeleteCluster report endpoint as https://<32-hex-id>.<3-hex>.<region>.eks.amazonaws.com — an address owned by AWS, not one of the emulator's own hosts. The harness refuses the announced address: "PublishedUrlRejected: endpoint_url was given https://<ID>.<xxx>.us-west-2.eks.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". Every Kubernetes-side behavior behind the endpoint is therefore unreachable.
Why: a client that does what the API tells it (kubectl, a pod, an add-on probe) is sent to a host the emulator never answers on, so it either escapes to the internet or fails to connect; no in-cluster behavior can be observed even though the control plane reports ACTIVE.
Fix: the DescribeCluster/CreateCluster response projection for Cluster.endpoint (eks cluster serializer/state) must announce an address on a listener the emulator actually serves, and that listener must route the Kubernetes API path space for the cluster.
Traces: eks.addon-configuration-values-reach-the-in-cluster-workload, eks.cluster-kubernetes-api-server-serves-advertised-version, eks.eks-kubectl-namespace. The same cause is filed for other service headings (20261109T141609Z-ecr-eks-logs-a-cluster-s-published-api.md, 20261109T131727Z-eks-auth-the-pod-identity-data-plane-is.md); neither carries these eks-heading checks.

Cause 2 of 3: DeleteCluster succeeds while a managed nodegroup is still attached.
AWS: Smithy: DeleteCluster models ResourceInUseException (HTTP 409) and its documentation states "If you have managed node groups or Fargate profiles attached to the cluster, you must delete them first." The recorded behavior is a refusal with code ResourceInUseException and the cluster left untouched (status ACTIVE); only after the nodegroup is deleted does the very same DeleteCluster call succeed.
Emulator: the call returns HTTP 200 with the cluster body and status 'DELETING' while an ACTIVE nodegroup still exists — "expected the call to be rejected with ResourceInUseException, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200 ...}, 'cluster': {... 'status': 'DELETING' ...}}".
Why: a client that relies on the dependency guard silently destroys a control plane that still has nodes attached; orphaned nodegroups/ENIs are left behind and the negative control (delete the nodegroup first, then the cluster) can no longer be distinguished from the refused path.
Fix: the DeleteCluster handler's state gate — refuse with ResourceInUseException (409) while any nodegroup or Fargate profile is attached, and leave the cluster's status unchanged.
Traces: eks.e2e-cluster-delete-blocked-by-active-nodegroup-rejected, eks.e2e-cluster-nodegroup-podidentity-teardown-flow. Open issue 20261104T105252Z-eks-10-net-new-over-permissive-root-causes.md describes a related over-permissive DeleteCluster but does not carry these two checks' replay keys.

Cause 3 of 3: UpdateClusterConfig on a still-CREATING cluster is refused with an emulator-authored message.
AWS: Smithy: UpdateClusterConfig models InvalidRequestException (HTTP 400). Recorded behavior: code InvalidRequestException with the message exactly "Cluster is currently not in Active State, so it cannot be updated" — the whole sentence is fixed text, it embeds no name, status or other request/state value.
Emulator: same code (InvalidRequestException) but the message is "Cluster <name> is in CREATING status; it must be ACTIVE to update its configuration" (observed: "Cluster sim2real-ekscc-68001ede2bce is in CREATING status; it must be ACTIVE to update its configuration" — the cluster name is the request's value, the rest is the emulator's own wording).
Why: a client matching on the documented refusal sentence (log assertions, error-mapping tables, runbooks) does not recognise the refusal; the message also leaks a state name AWS does not put in this error.
Fix: the UpdateClusterConfig handler's cluster-state validation must emit AWS's fixed sentence for the not-ACTIVE refusal rather than composing its own.
Traces: eks.ekscc-update-cluster-config-while-creating-rejected.

## Evidence
Cause 1 — expected: an endpoint on a host the emulator serves, reachable for Kubernetes API calls. Actual: https://1B47F77D013F9D92C2E2DE568D5156ED.1b4.us-west-2.eks.amazonaws.com / https://05D19AF89D7BF3B0C247B3627E7A65ED.05d.us-west-2.eks.amazonaws.com; harness error 'simparity.published.PublishedUrlRejected: endpoint_url was given https://...eks.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener' (ports differ per run and are not part of the requirement).

Cause 2 — expected: DeleteCluster -> HTTP 409 ResourceInUseException while nodegroup attached; cluster stays ACTIVE. Actual: HTTP 200, body cluster.status='DELETING' (check assertion: "expected the call to be rejected with ResourceInUseException, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'cluster': {'name': 'sim2real-eks-block-...', ... 'status': 'DELETING' ...}}"). Names/ids in that body are per-run values.

Cause 3 — expected message: 'Cluster is currently not in Active State, so it cannot be updated' (code InvalidRequestException, HTTP 400). Actual message: 'Cluster sim2real-ekscc-68001ede2bce is in CREATING status; it must be ACTIVE to update its configuration' (code InvalidRequestException). Fixed text = AWS's full sentence; the emulator's variable part (the cluster name) is a request value AWS does not include.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite (run_behavioral_suite).
2. Cause 1: replay eks.cluster-kubernetes-api-server-serves-advertised-version (also eks.eks-kubectl-namespace, eks.addon-configuration-values-reach-the-in-cluster-workload): CreateCluster with two AZ-distinct subnets, wait ACTIVE, read cluster.endpoint from DescribeCluster and try to reach it — the host is <id>.<region>.eks.amazonaws.com, not an emulator host.
3. Cause 2: replay eks.e2e-cluster-delete-blocked-by-active-nodegroup-rejected: create a cluster, create a managed nodegroup, then call DeleteCluster(name=<cluster>) — it answers 200/DELETING instead of 409 ResourceInUseException.
4. Cause 3: replay eks.ekscc-update-cluster-config-while-creating-rejected: CreateCluster, then immediately UpdateClusterConfig(name=<cluster>, logging={clusterLogging:[{types:['api'],enabled:true}]}) while status is CREATING — compare the Message of the InvalidRequestException with AWS's sentence.
