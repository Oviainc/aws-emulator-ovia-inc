Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__eks__logs.ecr-cross-account-pull-denied-without-repo-policy-rejected ecr__eks__logs.deploy-eks-cluster-with-full-logging-cloudformation
Issue-Key: ecr__eks__logs|UploadLayerPart|NotImplemented
Issue-Key: ecr__eks__logs.deploy-eks-cluster-with-full-logging-cloudformation|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Commit: db1fbaa8559703996072fdf323a0d84f8f863f33

# ECR UploadLayerPart is a 501 stub, and a CloudFormation-provisioned EKS cluster never produces its control-plane log group (2 causes)

> _Note: this issue was identified against the code base at commit `db1fbaa8559703996072fdf323a0d84f8f863f33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UploadLayerPart

## Description
Two distinct causes in the ECR + EKS + CloudWatch Logs cross-service checks; both must be closed.

Cause 1 of 2 — ECR UploadLayerPart answers 501 NotImplemented, so no image layer can be pushed.
AWS: Smithy (com-amazonaws-ecr-amazonec2containerregistry-v20150921): UploadLayerPart takes repositoryName, uploadId, partFirstByte, partLastByte and layerPartBlob (@length 0..20971520) and returns 200 with registryId/repositoryName/uploadId/lastByteReceived; its modeled errors are InvalidLayerPartException, InvalidParameterException, LimitExceededException, RepositoryNotFoundException, UploadNotFoundException and KmsException (400) plus ServerException (500). A well-formed part upload against an initiated upload succeeds.
Emulator: HTTP 501, error code `NotImplemented` ("operation UploadLayerPart is scaffolded but not implemented in this emulator").
Why: the layer-upload sequence (InitiateLayerUpload -> UploadLayerPart -> CompleteLayerUpload -> PutImage) cannot complete, so no image exists in the registry; the check's real subject — a cross-account pull being denied without a repository policy — is never reached, and any client that pushes images to the emulator is blocked.
Fix: the UploadLayerPart handler and the upload/layer state in the ecr crate (services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921).
Traces: ecr__eks__logs.ecr-cross-account-pull-denied-without-repo-policy-rejected.

Cause 2 of 2 — an EKS cluster created with all five control-plane log types enabled never materialises /aws/eks/<cluster>/cluster in CloudWatch Logs.
AWS: recorded behavior only — a cluster whose logging.clusterLogging enables api, audit, authenticator, controllerManager and scheduler causes EKS to create the log group /aws/eks/<clusterName>/cluster, with at least one log stream per enabled type (prefixes kube-apiserver-, kube-apiserver-audit-, authenticator-, kube-controller-manager-, kube-scheduler-); DescribeLogGroups(logGroupNamePrefix=...) then lists that group.
Emulator: after the CloudFormation stack is deployed, DescribeLogGroups never lists the group and the check exhausts its time budget ("Check timed out after 60.0s") while polling for it. This sits downstream of the CloudFormation/AWS::EKS::Cluster no-op reported for the `eks` service (a stack reaching CREATE_COMPLETE without the cluster existing), so the cluster whose logging would create the group never exists; the missing side effect itself — EKS creating the log group and streams for the enabled log types — is also unimplemented.
Why: operators and clients that read control-plane logs (and anything asserting that enabling a log type has an effect) see an empty Logs account; an enabled-logging configuration is accepted with no observable consequence.
Fix: the EKS cluster-logging side effect (create /aws/eks/<name>/cluster and a stream per enabled type in the cloudwatchlogs state) plus the CloudFormation AWS::EKS::Cluster provisioning it depends on.
Traces: ecr__eks__logs.deploy-eks-cluster-with-full-logging-cloudformation.

## Evidence
Suite: simparity-behavioral-aws-v1; ecr__eks__logs 0/16.
Cause 1: ecr__eks__logs.ecr-cross-account-pull-denied-without-repo-policy-rejected — botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UploadLayerPart operation: operation UploadLayerPart is scaffolded but not implemented in this emulator. Expected: HTTP 200 with lastByteReceived.
Cause 2: ecr__eks__logs.deploy-eks-cluster-with-full-logging-cloudformation — [mismatch] Error: Check timed out after 60.0s. The check deploys a CFN stack (VPC + 2 subnets + IAM role + AWS::EKS::Cluster with Logging.ClusterLogging.EnabledTypes = api/audit/authenticator/controllerManager/scheduler) and then polls logs.describe_log_groups(logGroupNamePrefix='/aws/eks/<cluster>/cluster') until the group appears; the group never appears. The same CFN template shape is shown by eks.deploy-cfn-cluster-behaves to reach CREATE_COMPLETE with no cluster in EKS.

## Steps to Reproduce
1. Boot the emulator.
2. Cause 1: ecr.create_repository, ecr.initiate_layer_upload, then ecr.upload_layer_part(repositoryName=..., uploadId=..., partFirstByte=0, partLastByte=len(blob)-1, layerPartBlob=blob). Expect HTTP 200 with lastByteReceived; observe HTTP 501 NotImplemented.
3. Cause 2: create an EKS cluster (directly or via a CloudFormation AWS::EKS::Cluster) with logging.clusterLogging enabling all five types, then logs.describe_log_groups(logGroupNamePrefix='/aws/eks/<cluster>/cluster') and logs.describe_log_streams for the prefixes kube-apiserver-, kube-apiserver-audit-, authenticator-, kube-controller-manager-, kube-scheduler-. Expect the group and one stream per enabled type; observe no log group at all.
