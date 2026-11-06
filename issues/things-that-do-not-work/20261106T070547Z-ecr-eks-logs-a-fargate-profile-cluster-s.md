Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__eks__logs.eks-fargate-execution-role-pulls-ecr-image ecr__eks__logs.eks-fargate-logging-without-execution-role-permission-no-delivery ecr__eks__logs.eks-fargate-pod-logs-delivered-to-cloudwatch
Issue-Key: ecr__eks__logs.eks-fargate-execution-role-pulls-ecr-image|_create_pod#23|MaxRetryError|core_v1.create_namespaced_pod(namespace, body)
Issue-Key: ecr__eks__logs.eks-fargate-logging-without-execution-role-permission-no-delivery|_create_aws_observability_namespace#20|MaxRetryError|core_v1.create_namespace(body)
Issue-Key: ecr__eks__logs.eks-fargate-pod-logs-delivered-to-cloudwatch|_create_aws_observability_namespace#18|MaxRetryError|core_v1.create_namespace(body)
Issue-Commit: 77172d4b913d3cd289b60abc3e2fa4659d3ee8a2

# ecr__eks__logs: a Fargate-profile cluster's certificateAuthority.data is not a loadable PEM certificate, so the served Kubernetes endpoint cannot be reached (1 cause)

> _Note: this issue was identified against the code base at commit `77172d4b913d3cd289b60abc3e2fa4659d3ee8a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCluster

## Description
One net-new root cause, grounded in three failing behavioral checks under the suite heading `ecr__eks__logs`. These three checks are the EKS *Fargate-profile* path and are NOT carried by any open issue's replay keys. The closely related open issue 20261104T060239Z-ecr-eks-logs-a-cluster-s-certificateauthority-data.md describes the same defect class for the node-group/IRSA path (its Issue-Trace-Ids list eks-nodegroup-pulls-private-ecr-image, eks-irsa-role-pulls-ecr-image-without-imagepullsecret, eks-pod-image-pull-fails-without-ecr-permission-rejected); it does not name any of the three checks below, so they are filed here and should be re-replayed alongside it.

Cause 1 of 1: the certificate-authority material the emulator publishes for a cluster cannot be loaded as a PEM certificate, so every Kubernetes API call made with the documented kubeconfig construction fails in the TLS layer before any request is served.
AWS: Smithy (services/service-com-amazonaws-eks-awswesleyfrontend): eks#Cluster.certificateAuthority targets eks#Certificate, whose `data` member is documented as "The Base64-encoded certificate data required to communicate with your cluster. Add this to the certificate-authority-data section of the kubeconfig file for your cluster." Base64-decoding that value therefore yields one or more PEM `-----BEGIN CERTIFICATE-----` blocks for a CA that signs the API server certificate presented at the cluster's `endpoint`, so a client that pins exactly that CA bundle and connects to `endpoint` completes the TLS handshake and receives a normal Kubernetes API response (2xx, or a Kubernetes Status object for a refusal). Recorded behavior only for the concrete certificate bytes, which are per-cluster material and never a fixed requirement.
Emulator: the value returned in certificateAuthority.data decodes to bytes that OpenSSL refuses to load as a certificate. Writing the decoded bytes to a CA file (what the kubernetes client does) and calling the published endpoint fails at connect time with urllib3.exceptions.MaxRetryError wrapping SSLError(SSLError(524297, '[X509] PEM lib (_ssl.c:4109)')) for both /api/v1/namespaces and /api/v1/namespaces/default/pods. The host/port in that message are the per-run endpoint the emulator itself advertised for the cluster; the fixed, requirement-bearing part is the '[X509] PEM lib' load failure of the emulator-supplied CA. No HTTP status or AWS error code is reached at all — the control-plane CreateCluster/CreateFargateProfile/DescribeCluster calls all returned 200 and an ACTIVE cluster, so the divergence is purely in the certificate material those 200s carry.
Why: a client that follows the documented kubeconfig construction finds the Kubernetes data plane of an emulated EKS cluster unreachable. Every Fargate-profile scenario — pod creation under a Fargate selector, pod-execution-role driven private-ECR image pulls, and Fargate pod log delivery to CloudWatch — fails at connect time, and the failure surfaces as a client-side TLS misconfiguration rather than as a server defect, so it is actively misleading to debug.
Fix: the EKS cluster's certificate-authority material — produced by CreateCluster, echoed by DescribeCluster, and used to serve the advertised kube API endpoint — must be a base64-encoded PEM certificate that actually signs the serving certificate of that endpoint. This belongs in the EKS cluster state/creation path and the kube API endpoint it advertises (services/service-com-amazonaws-eks-awswesleyfrontend/src), not in the checks' TLS setup.
Traces: ecr__eks__logs.eks-fargate-execution-role-pulls-ecr-image (create pod via /api/v1/namespaces/default/pods), ecr__eks__logs.eks-fargate-pod-logs-delivered-to-cloudwatch (/api/v1/namespaces), ecr__eks__logs.eks-fargate-logging-without-execution-role-permission-no-delivery (/api/v1/namespaces).

## Evidence
All three checks error identically, differing only in the per-run host/port and the requested path:

[error] ecr__eks__logs.eks-fargate-execution-role-pulls-ecr-image
  urllib3.exceptions.MaxRetryError: HTTPSConnectionPool(host=<advertised kube endpoint>, port=<advertised port>): Max retries exceeded with url: /api/v1/namespaces/default/pods (Caused by SSLError(SSLError(524297, '[X509] PEM lib (_ssl.c:4109)')))

[error] ecr__eks__logs.eks-fargate-pod-logs-delivered-to-cloudwatch
  urllib3.exceptions.MaxRetryError: HTTPSConnectionPool(host=<advertised kube endpoint>, port=<advertised port>): Max retries exceeded with url: /api/v1/namespaces (Caused by SSLError(SSLError(524297, '[X509] PEM lib (_ssl.c:4109)')))

[error] ecr__eks__logs.eks-fargate-logging-without-execution-role-permission-no-delivery
  urllib3.exceptions.MaxRetryError: HTTPSConnectionPool(host=<advertised kube endpoint>, port=<advertised port>): Max retries exceeded with url: /api/v1/namespaces (Caused by SSLError(SSLError(524297, '[X509] PEM lib (_ssl.c:4109)')))

Expected: the CA bundle written from base64.b64decode(DescribeCluster.cluster.certificateAuthority.data) loads as a PEM certificate and validates the certificate served at cluster.endpoint, so the request completes with a Kubernetes API response.
Actual: OpenSSL cannot load the emulator-supplied CA ('[X509] PEM lib'), so the handshake never happens and no Kubernetes request is issued. The failing address is the one the emulator published for the cluster, so this is not a runner network or name-resolution problem; CreateCluster, CreateFargateProfile and DescribeCluster all answered 200 with an ACTIVE cluster beforehand.

## Steps to Reproduce
1. CreateRole for an EKS cluster role (trusting eks.amazonaws.com, AmazonEKSClusterPolicy) and a pod execution role (trusting eks-fargate-pods.amazonaws.com, AmazonEKSFargatePodExecutionRolePolicy).
2. CreateCluster with private subnets; wait for status ACTIVE.
3. CreateFargateProfile with the pod execution role and a selector on namespace `default`; wait for ACTIVE.
4. DescribeCluster and build a kubeconfig the documented way: endpoint = cluster.endpoint, certificate-authority-data = cluster.certificateAuthority.data (base64-decoded into a CA file).
5. Issue any Kubernetes API call with that config, e.g. GET/POST /api/v1/namespaces or POST /api/v1/namespaces/default/pods.
   -> the TLS handshake fails with SSLError(524297, '[X509] PEM lib (_ssl.c:4109)') because the decoded CA data is not a loadable PEM certificate. Expected: the call is served over TLS validated by that CA.
