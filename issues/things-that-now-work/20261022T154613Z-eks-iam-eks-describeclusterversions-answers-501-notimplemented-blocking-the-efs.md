Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks__iam.efs-csi-driver-policy-denies-delete-of-untagged-access-point eks__iam.efs-csi-driver-policy-denies-tag-resource-outside-access-point-creation eks__iam.efs-csi-driver-policy-denies-untagged-access-point-create
Issue-Key: eks__iam|DescribeClusterVersions|NotImplemented
Issue-Commit: db1fbaa8559703996072fdf323a0d84f8f863f33

# EKS DescribeClusterVersions answers 501 NotImplemented, blocking the EFS/IAM CSI-policy checks before any IAM assertion runs

> _Note: this issue was identified against the code base at commit `db1fbaa8559703996072fdf323a0d84f8f863f33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeClusterVersions

## Description
Cause 1 of 1 — EKS DescribeClusterVersions is a scaffold stub, so the three cross-service EKS+IAM checks die on their first call and never reach the EFS CSI driver policy assertions.
AWS: Smithy (com-amazonaws-eks-awswesleyfrontend): DescribeClusterVersions is GET /cluster-versions returning `clusterVersions` (a ClusterVersionList) plus `nextToken`, with modeled errors InvalidParameterException (400), InvalidRequestException (400) and ServerException (500) only — a well-formed catalog query is a 200 with the supported Kubernetes versions. There is no modeled condition under which it refuses a plain call.
Emulator: HTTP 501 with error code `NotImplemented`, message example "operation DescribeClusterVersions is scaffolded but not implemented in this emulator".
Why: DescribeClusterVersions is the standard way a client picks the Kubernetes version to create a cluster with; a 501 there stops every provisioning flow at step one, and these checks' real subject (the EFS CSI driver IAM policy denying untagged access-point create/tag/delete) is never exercised.
Fix: the DescribeClusterVersions handler in the eks crate (services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/describe_cluster_versions.rs) and the version catalog it reads.
Note: an open issue already names this operation and handler file, filed from the `eks` service's own cluster of this stub; it does not carry these three eks__iam replay keys, so they are recorded here and must be re-replayed when that handler lands.
Traces: eks__iam.efs-csi-driver-policy-denies-delete-of-untagged-access-point, eks__iam.efs-csi-driver-policy-denies-tag-resource-outside-access-point-creation, eks__iam.efs-csi-driver-policy-denies-untagged-access-point-create.

## Evidence
Suite: simparity-behavioral-aws-v1; eks__iam 0/4.
All three checks fail identically at their first API call:
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeClusterVersions operation: operation DescribeClusterVersions is scaffolded but not implemented in this emulator
Expected: HTTP 200 with a clusterVersions list (Smithy DescribeClusterVersionsResponse: clusterVersions, nextToken).

## Steps to Reproduce
1. Boot the emulator.
2. eks.describe_cluster_versions() (optionally with defaultOnly=True).
3. Expect HTTP 200 and a `clusterVersions` list of supported Kubernetes versions; observe HTTP 501 with error code NotImplemented.
