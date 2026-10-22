Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks__s3.e2e-manage-buckets-eks-sagemaker-lifecycle
Issue-Key: eks__s3|DescribeClusterVersions|NotImplemented
Issue-Commit: db1fbaa8559703996072fdf323a0d84f8f863f33

# EKS DescribeClusterVersions 501 aborts phase 1 of the S3/EKS/SageMaker lifecycle check

> _Note: this issue was identified against the code base at commit `db1fbaa8559703996072fdf323a0d84f8f863f33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeClusterVersions

## Description
Cause 1 of 1 — the multi-service lifecycle check fails in its first phase because EKS DescribeClusterVersions is a scaffold stub.
AWS: Smithy (com-amazonaws-eks-awswesleyfrontend): DescribeClusterVersions is GET /cluster-versions and returns 200 with `clusterVersions`/`nextToken`; its only modeled errors are InvalidParameterException (400), InvalidRequestException (400) and ServerException (500), so a plain catalog query always succeeds.
Emulator: HTTP 501, error code `NotImplemented` (message example "operation DescribeClusterVersions is scaffolded but not implemented in this emulator"), raised inside phase 1/7 "deploy".
Why: the deploy phase cannot choose a cluster version, so the whole S3 + EKS + SageMaker estate is never stood up and none of the later phases (bucket lifecycle, cross-service wiring, teardown) are graded.
Fix: the DescribeClusterVersions handler in the eks crate (services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/describe_cluster_versions.rs).
Note: an open issue already names this operation and handler file, filed from the `eks` service's own cluster of this stub; it does not carry this check's replay key, so this check is recorded here and must be re-replayed when that handler lands.
Traces: eks__s3.e2e-manage-buckets-eks-sagemaker-lifecycle.

## Evidence
Suite: simparity-behavioral-aws-v1; eks__s3 0/2.
_PhaseError: [phase 1/7 deploy] ClientError: An error occurred (NotImplemented) when calling the DescribeClusterVersions operation: operation DescribeClusterVersions is scaffolded but not implemented in this emulator.
Expected: HTTP 200 with a clusterVersions list.

## Steps to Reproduce
1. Boot the emulator.
2. Run the deploy phase of the S3/EKS/SageMaker lifecycle flow, whose first call is eks.describe_cluster_versions().
3. Expect HTTP 200 with `clusterVersions`; observe HTTP 501 NotImplemented and the phase aborting.
