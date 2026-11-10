Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks__iam__sts.eks-kubernetes-api-namespace-via-sts-bearer-token
Issue-Key: eks__iam__sts.eks-kubernetes-api-namespace-via-sts-bearer-token|run#17|AssertionError|assert caller_arn in entries, (
Issue-Commit: 9b14a11a85da2dc3914d7c191991ddabd7c195c3

# EKS CreateCluster with bootstrapClusterCreatorAdminPermissions never materializes the cluster-creator access entry, so ListAccessEntries is empty (1 cause)

> _Note: this issue was identified against the code base at commit `9b14a11a85da2dc3914d7c191991ddabd7c195c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Cause 1 of 1: `CreateCluster` accepts `accessConfig.bootstrapClusterCreatorAdminPermissions` as a flag it records on the cluster, but it never creates the access entry that flag is defined to create, so the whole access-entry surface for the creating principal is empty on a freshly created cluster.

AWS: Smithy (services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json) — `CreateCluster.accessConfig` targets `CreateAccessConfigRequest`, whose member `bootstrapClusterCreatorAdminPermissions` is documented: "Specifies whether or not the cluster creator IAM principal was set as a cluster admin access entry during cluster creation time. The default value is true." So when the flag is true (explicitly or by default), EKS materialises a real `AccessEntry` for the calling IAM principal, and that entry is readable through the normal access-entry readers: `ListAccessEntries` (GET /clusters/{clusterName}/access-entries) returns the creating principal's ARN in `accessEntries`; `DescribeAccessEntry` (GET /clusters/{clusterName}/access-entries/{principalArn}) returns an `AccessEntry` with `clusterName` = the cluster, `principalArn`/`username` = the caller's IAM ARN verbatim, `type` = "STANDARD", `kubernetesGroups` = [] (the bootstrap entry is given none) plus the server-generated `accessEntryArn`, `createdAt`, `modifiedAt`; and `ListAssociatedAccessPolicies` (GET /clusters/{clusterName}/access-entries/{principalArn}/access-policies) returns exactly one `AssociatedAccessPolicy` whose `policyArn` is the AWS-managed cluster-admin policy `arn:aws:eks::aws:cluster-access-policy/AmazonEKSClusterAdminPolicy` with `accessScope.type` = "cluster" and `accessScope.namespaces` present-but-empty. That entry is also the only reason the cluster's Kubernetes API authorises the creator's requests.

Emulator: the cluster is created and reaches ACTIVE with `accessConfig.authenticationMode` = API_AND_CONFIG_MAP as requested, but `ListAccessEntries(clusterName=<cluster>)` answers 200 with `accessEntries: []` — no entry for the creating principal (replay: caller ARN `arn:aws:iam::<account>:user/<user>`, entries `[]`). The handler states this by construction: services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/create_cluster.rs lines 446-458 read `accessConfig`, compute `bootstrap_cluster_creator_admin_permissions` (defaulting to true) and store it only as a field on the cluster record (it is even blanked out of the `accessConfig` a later read returns, line 458), and lines 587-591 initialise `access_entries` to an empty map with the comment that the creator's admin access "is reported on the cluster rather than as an `AccessEntry` record." Consequently `DescribeAccessEntry` for the creator refuses with 404 `ResourceNotFoundException` and `ListAssociatedAccessPolicies` reports no `AmazonEKSClusterAdminPolicy` association, instead of the one cluster-scoped association AWS creates.

Why: a client that creates a cluster the way the console and eksctl do — `bootstrapClusterCreatorAdminPermissions` true — and then reads back who can administer it sees nobody. Access-entry management flows break: listing admins returns an empty set, `DescribeAccessEntry` on the creator 404s, idempotent "ensure my own entry" logic concludes the entry is missing and tries to `CreateAccessEntry` for a principal AWS says already has one (and on AWS that collides), and policy-association audits find the cluster has no cluster-admin association at all. Downstream, because no entry exists, the cluster's Kubernetes API has no mapping from the creator's IAM identity to a Kubernetes subject, so every authenticated kubectl-style call through the cluster endpoint is unauthorised for the one principal AWS guarantees can administer the cluster.

Fix: in CreateCluster's handler (services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/create_cluster.rs) and the cluster record's access-entry state (services/service-com-amazonaws-eks-awswesleyfrontend/src/state.rs): when the effective `bootstrapClusterCreatorAdminPermissions` is true, write an `AccessEntry` record for the calling IAM principal (principalArn and username = the caller's ARN, type STANDARD, empty kubernetesGroups, generated accessEntryArn/createdAt/modifiedAt) together with one associated access policy `arn:aws:eks::aws:cluster-access-policy/AmazonEKSClusterAdminPolicy` scoped `{type: "cluster", namespaces: []}`, so the existing `ListAccessEntries`, `DescribeAccessEntry` and `ListAssociatedAccessPolicies` readers see it; the Kubernetes-side identity mapping must consult that entry.

Traces: eks__iam__sts.eks-kubernetes-api-namespace-via-sts-bearer-token — `AssertionError: bootstrapClusterCreatorAdminPermissions did not create an access entry for the creating principal arn:aws:iam::000000000000:user/emulator: []` (the account id and user name are this run's values, not part of the requirement). No open EKS issue covers the access-entry surface: the nearest ones (20261029T175608Z-eks-5-net-new-root-causes-in-refusal.md, which touches ListAssociatedAccessPolicies pagination tokens, and 20261102T083656Z-eks-add-on-lifecycle-is-unreachable-clusters-never.md) are about refusal wording/nextToken and add-on lifecycle, not about the bootstrap entry being created at all.

## Evidence
Expected (AWS): after CreateCluster(name=<c>, accessConfig={authenticationMode: API_AND_CONFIG_MAP, bootstrapClusterCreatorAdminPermissions: true}) by principal <caller-arn>:
  - ListAccessEntries(clusterName=<c>) -> 200 {"accessEntries": ["<caller-arn>"]}
  - DescribeAccessEntry(clusterName=<c>, principalArn=<caller-arn>) -> 200 accessEntry{clusterName=<c>, principalArn=<caller-arn>, username=<caller-arn>, type="STANDARD", kubernetesGroups=[], accessEntryArn/createdAt/modifiedAt present}
  - ListAssociatedAccessPolicies(clusterName=<c>, principalArn=<caller-arn>) -> 200 with exactly one association: policyArn="arn:aws:eks::aws:cluster-access-policy/AmazonEKSClusterAdminPolicy", accessScope={type: "cluster", namespaces: []}

Actual (emulator): cluster reaches ACTIVE, accessConfig.authenticationMode = API_AND_CONFIG_MAP, but
  - ListAccessEntries(clusterName=<c>) -> 200 {"accessEntries": []}
  - DescribeAccessEntry for the creator therefore refuses 404 ResourceNotFoundException, and ListAssociatedAccessPolicies reports no AmazonEKSClusterAdminPolicy association.

Failing check: eks__iam__sts.eks-kubernetes-api-namespace-via-sts-bearer-token
  AssertionError: bootstrapClusterCreatorAdminPermissions did not create an access entry for the creating principal arn:aws:iam::000000000000:user/emulator: []

Code evidence: services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/create_cluster.rs:446-458 (flag stored on the cluster record only) and :587-591 (`access_entries: BTreeMap::new()` with the comment that the creator's admin access "is reported on the cluster rather than as an `AccessEntry` record").

## Steps to Reproduce
1. sts:GetCallerIdentity to learn the calling principal ARN <caller-arn>.
2. iam:CreateRole with a trust policy for eks.amazonaws.com and attach arn:aws:iam::aws:policy/AmazonEKSClusterPolicy.
3. eks:CreateCluster(name=<c>, roleArn=<that role>, resourcesVpcConfig={subnetIds: two subnets in two AZs, endpointPublicAccess: true}, accessConfig={authenticationMode: "API_AND_CONFIG_MAP", bootstrapClusterCreatorAdminPermissions: true}).
4. Poll eks:DescribeCluster(name=<c>) until status == ACTIVE.
5. eks:ListAccessEntries(clusterName=<c>) — observe accessEntries == [] instead of ["<caller-arn>"].
6. eks:DescribeAccessEntry(clusterName=<c>, principalArn=<caller-arn>) — observe 404 ResourceNotFoundException instead of the STANDARD bootstrap entry.
7. eks:ListAssociatedAccessPolicies(clusterName=<c>, principalArn=<caller-arn>) — observe no arn:aws:eks::aws:cluster-access-policy/AmazonEKSClusterAdminPolicy association with accessScope {type: cluster, namespaces: []}.

Replay: run behavioral check eks__iam__sts.eks-kubernetes-api-namespace-via-sts-bearer-token.
