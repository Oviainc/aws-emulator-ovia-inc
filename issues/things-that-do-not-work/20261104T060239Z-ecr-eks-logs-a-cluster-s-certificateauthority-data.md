Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__eks__logs.eks-nodegroup-pulls-private-ecr-image ecr__eks__logs.eks-irsa-role-pulls-ecr-image-without-imagepullsecret ecr__eks__logs.eks-pod-image-pull-fails-without-ecr-permission-rejected ecr__eks__logs.eks-container-insights-metrics-logs-delivery ecr__eks__logs.eks-container-insights-without-agent-permission-no-delivery ecr__eks__logs.deploy-eks-cluster-with-full-logging-cloudformation ecr__eks__logs.deploy-eks-cluster-without-logging-config-no-log-groups ecr__eks__logs.deploy-eks-nodegroup-with-ecr-readonly-policy
Issue-Key: ecr__eks__logs.eks-nodegroup-pulls-private-ecr-image|_create_pod#24|MaxRetryError|core_v1.create_namespaced_pod(namespace, body)
Issue-Key: ecr__eks__logs.eks-irsa-role-pulls-ecr-image-without-imagepullsecret|run#34|MaxRetryError|core_v1.create_namespaced_service_account(
Issue-Key: ecr__eks__logs.eks-pod-image-pull-fails-without-ecr-permission-rejected|_create_pod#26|MaxRetryError|core_v1.create_namespaced_pod(namespace, body)
Issue-Key: ecr__eks__logs.eks-container-insights-metrics-logs-delivery|run#21|AssertionError|raise AssertionError(
Issue-Key: ecr__eks__logs.eks-container-insights-without-agent-permission-no-delivery|run#22|AssertionError|raise AssertionError(
Issue-Key: ecr__eks__logs.deploy-eks-cluster-with-full-logging-cloudformation|_deploy_cfn#7|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Key: ecr__eks__logs.deploy-eks-cluster-without-logging-config-no-log-groups|_assert_absent_for#9|Timeout|time.sleep(interval_s)
Issue-Key: ecr__eks__logs.deploy-eks-nodegroup-with-ecr-readonly-policy|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ecr__eks__logs: a cluster's certificateAuthority.data is not a usable PEM certificate, the add-on catalogue has no amazon-cloudwatch-observability version for the cluster's Kubernetes version, and CloudFormation cannot provision an EKS estate (3 causes)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCluster

## Description
Three net-new root causes, all grounded in failing behavioral checks under the suite heading `ecr__eks__logs`.

Cause 1 of 3: the cluster's certificate-authority data cannot be loaded as a PEM certificate, so no Kubernetes client can reach the served API endpoint.
AWS: Smithy: eks#Cluster.certificateAuthority targets eks#Certificate, whose `data` member is "The Base64-encoded certificate data required to communicate with your cluster. Add this to the certificate-authority-data section of the kubeconfig file for your cluster." Base64-decoding it yields one or more PEM "-----BEGIN CERTIFICATE-----" blocks that sign the API server's certificate, so a client that pins that CA and connects to `endpoint` completes the TLS handshake.
Emulator: the value DescribeCluster/CreateCluster returns in certificateAuthority.data decodes to bytes that are not a loadable PEM certificate: every Kubernetes API call made with it fails in the TLS layer with SSLError(524297, '[X509] PEM lib') before any request is served, against the very endpoint the emulator published for the cluster.
Why: the Kubernetes data plane of an emulated EKS cluster is unreachable for any client that follows the documented kubeconfig construction - pod/serviceaccount creation, IRSA and image-pull scenarios all fail at connect time, and the failure looks like a client TLS misconfiguration rather than a server defect.
Fix: the EKS cluster's certificate-authority material, produced by CreateCluster and returned by DescribeCluster, must be a base64-encoded PEM certificate that actually signs the served API endpoint's certificate (services/service-com-amazonaws-eks-awswesleyfrontend/src, cluster creation/state and the kube API endpoint it advertises).
Traces: ecr__eks__logs.eks-nodegroup-pulls-private-ecr-image, ecr__eks__logs.eks-irsa-role-pulls-ecr-image-without-imagepullsecret, ecr__eks__logs.eks-pod-image-pull-fails-without-ecr-permission-rejected.

Cause 2 of 3: DescribeAddonVersions offers no amazon-cloudwatch-observability version for the Kubernetes version the emulator's own cluster reports.
AWS: DescribeAddonVersions(addonName="amazon-cloudwatch-observability", kubernetesVersion=<the cluster's version>) returns an `addons` entry whose `addonVersions` list carries the versions installable on that cluster, including the Classic v5.x line that Container Insights uses; CreateAddon then accepts one of those versions. Recorded behavior only for which exact versions exist.
Emulator: for the Kubernetes version its own CreateCluster assigns (observed: 1.36), DescribeAddonVersions returns no v5.x amazon-cloudwatch-observability version at all, so the add-on cannot be installed and both Container Insights checks abort before CreateAddon.
Why: the emulator's cluster surface is self-inconsistent - it hands out a Kubernetes version for which its own add-on catalogue offers nothing, so no client can install a managed add-on by discovering a compatible version, which is the documented flow.
Fix: the add-on version catalogue backing DescribeAddonVersions must cover the Kubernetes versions the service's CreateCluster can produce (services/service-com-amazonaws-eks-awswesleyfrontend/src, describe_addon_versions and its catalogue data).
Traces: ecr__eks__logs.eks-container-insights-metrics-logs-delivery, ecr__eks__logs.eks-container-insights-without-agent-permission-no-delivery.

Cause 3 of 3: a CloudFormation stack that provisions an EKS estate does not succeed.
AWS: a stack declaring AWS::IAM::Role + AWS::EKS::Cluster (optionally AWS::EKS::Nodegroup and AWS::ECR::PullThroughCacheRule) with valid properties reaches CREATE_COMPLETE, and the cluster is then readable through eks DescribeCluster.
Emulator: the create ends in ROLLBACK_COMPLETE instead of CREATE_COMPLETE, so no cluster is provisioned; the two sibling checks that deploy the same resource types never get past their deploy wait and return no verdict within the harness deadline.
Why: EKS cannot be provisioned by template at all - a client's IaC deploy fails, and the control-plane logging behavior it was meant to exercise (log group created only for enabled log types) is unreachable.
Fix: the CloudFormation resource providers for AWS::EKS::Cluster and AWS::EKS::Nodegroup, and the failure they report during create (services/service-com-amazonaws-cloudformation-cloudformation/src).
Traces: ecr__eks__logs.deploy-eks-cluster-with-full-logging-cloudformation, ecr__eks__logs.deploy-eks-cluster-without-logging-config-no-log-groups, ecr__eks__logs.deploy-eks-nodegroup-with-ecr-readonly-policy.

## Evidence
Cause 1: urllib3.exceptions.MaxRetryError: HTTPSConnectionPool(host=<published kube endpoint>, port=<port>): Max retries exceeded with url: /api/v1/namespaces/default/pods (Caused by SSLError(SSLError(524297, '[X509] PEM lib (_ssl.c:4109)'))) - the CA file is written as base64.b64decode(cluster["certificateAuthority"]["data"]) from the emulator's own DescribeCluster response; same failure for /api/v1/namespaces/default/serviceaccounts.
Cause 2: AssertionError: no Classic (v5.x) version of the amazon-cloudwatch-observability add-on is available for Kubernetes 1.36 in this account/region, raised after describe_addon_versions(addonName="amazon-cloudwatch-observability", kubernetesVersion=<cluster version>) returned no v5.x entry.
Cause 3: AssertionError: Stack sim2real-xsvc-stk-466de5a870ce ended in ROLLBACK_COMPLETE; the two sibling EKS CloudFormation checks end with "Check timed out after 60.0s".

## Steps to Reproduce
1. CreateCluster, wait for ACTIVE, DescribeCluster, base64-decode certificateAuthority.data into a file and use it as the CA bundle for an HTTPS call to the cluster's `endpoint` (e.g. GET /api/v1/namespaces/default/pods) - the TLS handshake fails with '[X509] PEM lib' (Cause 1).
2. For the same cluster, call DescribeAddonVersions(addonName="amazon-cloudwatch-observability", kubernetesVersion=<cluster version>) - no v5.x addonVersion is offered, so CreateAddon cannot be called (Cause 2).
3. CreateStack with a template declaring AWS::IAM::Role (trusting eks.amazonaws.com) + AWS::EKS::Cluster (RoleArn, ResourcesVpcConfig.SubnetIds) and optionally AWS::EKS::Nodegroup - the stack ends in ROLLBACK_COMPLETE (Cause 3).
