Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__eks__logs.eks-fargate-execution-role-pulls-ecr-image ecr__eks__logs.eks-fargate-logging-without-execution-role-permission-no-delivery ecr__eks__logs.eks-fargate-pod-logs-delivered-to-cloudwatch ecr__eks__logs.eks-irsa-role-pulls-ecr-image-without-imagepullsecret ecr__eks__logs.eks-nodegroup-pulls-private-ecr-image ecr__eks__logs.eks-pod-image-pull-fails-without-ecr-permission-rejected
Issue-Key: ecr__eks__logs.eks-fargate-execution-role-pulls-ecr-image|_k8s_core_v1#23|PublishedUrlRejected|host, route = ctx.endpoint_route(cluster["endpoint"])
Issue-Key: ecr__eks__logs.eks-fargate-logging-without-execution-role-permission-no-delivery|_k8s_core_v1#19|PublishedUrlRejected|host, route = ctx.endpoint_route(cluster["endpoint"])
Issue-Key: ecr__eks__logs.eks-fargate-pod-logs-delivered-to-cloudwatch|_k8s_core_v1#17|PublishedUrlRejected|host, route = ctx.endpoint_route(cluster["endpoint"])
Issue-Key: ecr__eks__logs.eks-irsa-role-pulls-ecr-image-without-imagepullsecret|_k8s_core_v1#33|PublishedUrlRejected|host, route = ctx.endpoint_route(cluster["endpoint"])
Issue-Key: ecr__eks__logs.eks-nodegroup-pulls-private-ecr-image|_k8s_core_v1#24|PublishedUrlRejected|host, route = ctx.endpoint_route(cluster["endpoint"])
Issue-Key: ecr__eks__logs.eks-pod-image-pull-fails-without-ecr-permission-rejected|_k8s_core_v1#26|PublishedUrlRejected|host, route = ctx.endpoint_route(cluster["endpoint"])
Issue-Commit: 5252be52e0cc9f5d73bee5aa197523afb271707f

# ecr__eks__logs: a cluster's published API-server endpoint is a real-AWS eks.amazonaws.com host the emulator does not serve, so no Kubernetes-side behaviour can be exercised (6 checks)

> _Note: this issue was identified against the code base at commit `5252be52e0cc9f5d73bee5aa197523afb271707f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCluster

## Description
Cause 1 of 1: EKS announces cluster.endpoint on a host that is not the emulator's.
AWS: DescribeCluster returns Cluster$endpoint — the address of the cluster's Kubernetes API server — and a client addresses that endpoint directly (kubectl, a pod-spec apply, a logs/metrics agent install). Smithy (EKS): Cluster$endpoint is the endpoint for the cluster's Kubernetes API server.
Emulator: the published endpoint is 'https://<32-hex-id>.<3-char>.<region>.eks.amazonaws.com' — a real-AWS hostname that is not one of the emulator's own hosts, so the announced address is refused (a customer of the emulator needs http(s)://<own host>:<port>/<path>). Every check that must talk to the cluster's API server — Fargate-profile pod runs, IRSA pulls, nodegroup pulls, pod-level pull failures and Fargate log delivery — dies at the first use of the endpoint.
Why: a client that follows the address the API handed it is sent to AWS's DNS name instead of the emulator; the Kubernetes API is unreachable, so nothing a pod does (image pull, log delivery, permission denial) can be observed through the documented path.
Fix: the EKS Cluster endpoint serializer/state (publish the host and port the emulator actually serves the cluster's Kubernetes API on). Distinct from the open issue 20261104T060239Z-ecr-eks-logs-a-cluster-s-certificateauthority-data.md (certificateAuthority.data / add-on catalogue), which owns other checks in this service.
Traces: ecr__eks__logs.eks-fargate-execution-role-pulls-ecr-image, ecr__eks__logs.eks-fargate-logging-without-execution-role-permission-no-delivery, ecr__eks__logs.eks-fargate-pod-logs-delivered-to-cloudwatch, ecr__eks__logs.eks-irsa-role-pulls-ecr-image-without-imagepullsecret, ecr__eks__logs.eks-nodegroup-pulls-private-ecr-image, ecr__eks__logs.eks-pod-image-pull-fails-without-ecr-permission-rejected.

## Evidence
simparity.published.PublishedUrlRejected: endpoint_url was given https://4F97035DD37EA8F9BBBC791CAF04F465.4f9.us-west-2.eks.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener

The same refusal occurs in all six checks, each with a different generated cluster id and a different emulator listener port.

Expected: DescribeCluster's endpoint names a host:port the emulator serves, and a Kubernetes API request to it succeeds.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Re-replay the six checks in trace_ids (service ecr__eks__logs).
3. Or: CreateCluster, then DescribeCluster and inspect cluster.endpoint — it is an eks.amazonaws.com name the emulator does not listen on.
