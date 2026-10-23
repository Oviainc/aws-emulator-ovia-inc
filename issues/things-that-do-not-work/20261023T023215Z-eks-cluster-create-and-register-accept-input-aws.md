Issue-Source: trace-sweep-divergence
Issue-Trace: eks-cluster-and-connector-registration-validation-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eks-cluster-and-connector-registration-validation-errors eks-cluster-create-invalid-subnet-error eks-register-cluster-invalid-connector-role-error
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|CreateCluster|4xx->2xx|$.message
Issue-Key: eks|CreateCluster|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: eks|CreateCluster|4xx->2xx|status
Issue-Key: eks|CreateCluster|4xx->2xx|$.clusterName
Issue-Key: eks|RegisterCluster|4xx->2xx|$.message
Issue-Key: eks|RegisterCluster|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: eks|RegisterCluster|4xx->2xx|status
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# EKS cluster create and register accept input AWS refuses, answering 200 instead of a modeled 4xx

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Three cluster-provisioning requests that AWS REFUSES are accepted by the emulator with HTTP 200 and a created resource. These are the most severe class of divergence in this sweep: the emulator does not merely mis-shape an error, it manufactures a cluster that could not exist on AWS, so every later step of a client flow runs against state AWS would never have. THREE causes; the issue is not complete until all three are closed.

Cause 1 of 3 - CreateCluster with a roleArn owned by another account is not denied.
AWS: recorded behavior only for the error class (Smithy: CreateCluster's modeled errors are ClientException 400, InvalidParameterException 400, ResourceInUseException 409, ResourceLimitExceededException 400, ServerException 500, ServiceUnavailableException 503, UnsupportedAvailabilityZoneException 400 - AccessDeniedException is not in that list). The recording shows EKS authorizing the iam:PassRole of `roleArn` and refusing a cross-account role with HTTP 403 and x-amzn-errortype AccessDeniedException, message 'Cross-account pass role is not allowed.' (a fixed sentence tied to that rule).
Emulator: HTTP 200 - the cluster is created, with no message and no x-amzn-errortype header at all.
Why: a policy/guardrail test asserting that cross-account role passing is denied passes against the emulator and concludes it is PERMITTED; the client also proceeds with a cluster that AWS would not have created.
Fix: the CreateCluster handler - authorize the roleArn pass-role (account of the ARN vs the request's account) before provisioning. The open behavioral issue 20261022T154613Z-eks-control-plane-operations-answer-501-notimplemented-or.md (cause 10) describes the ORDERING of this check against body validation and carries behavioral check ids only, not this fixture step; this wire step must be re-replayed here because on a well-formed body the check is absent entirely, not merely late.
Traces: eks-cluster-and-connector-registration-validation-errors#task#1.

Cause 2 of 3 - CreateCluster with a subnet ID that does not exist is not refused.
AWS: Smithy: CreateCluster models InvalidParameterException (httpError 400) and requires resourcesVpcConfig. The recording shows EKS resolving the requested subnets against EC2 and refusing an unknown subnet with HTTP 400, x-amzn-errortype InvalidParameterException, $.clusterName set to the requested cluster name, and a message that quotes the offending subnet id (example: "The subnet ID 'subnet-00000000deadbeef1' does not exist (Service: AmazonEC2; Status Code: 400; Error Code: InvalidSubnetID.NotFound; ...)") - the rule is that the unknown subnet id is named; the embedded request id/proxy text is an example only.
Emulator: HTTP 200 - the cluster is created referencing a subnet that does not exist.
Whey/Why: a client that builds a VPC and then a cluster gets no feedback that its networking inputs are wrong, so subnet-wiring bugs surface only in production; it also cannot test its InvalidParameterException handling.
Fix: the CreateCluster handler's resourcesVpcConfig validation - resolve subnetIds (and report via InvalidParameterException with clusterName set) before creating cluster state.
Traces: eks-cluster-create-invalid-subnet-error#task#1.

Cause 3 of 3 - RegisterCluster with a connector role that is not assumable by the SSM service principal is not refused.
AWS: Smithy: RegisterCluster models AccessDeniedException (403), ClientException (400), InvalidParameterException (400), ResourceInUseException (409), ResourceLimitExceededException (400), ResourcePropagationDelayException (428), ServerException (500), ServiceUnavailableException (503); connectorConfig is required. The recording shows HTTP 400 with x-amzn-errortype InvalidRequestException when the referenced role does not exist or lacks the ssm service principal in its trust policy (message example: 'Nonexistent role or missing ssm service principal in trust policy: <roleArn> ...'); the rule is that the role's existence and trust policy are checked and the offending roleArn is named.
Emulator: HTTP 200 - the external cluster registration succeeds.
Why: a client registering an EKS Anywhere/external cluster is told its connector IAM wiring is fine when AWS would reject it, so the failure moves to the connector agent instead of the API call.
Fix: the RegisterCluster handler's connectorConfig.roleArn validation (existence + ssm trust) ahead of registration.
Traces: eks-register-cluster-invalid-connector-role-error#task#1.

## Evidence
Corpus simparity-wire-aws-v1, 35/216 steps passed. All three causes are expected-4xx-got-200.

Cause 1: eks-cluster-and-connector-registration-validation-errors#task#1 CreateCluster - status: expected 403, got 200; $headers.x-amzn-errortype: missing (expected 'AccessDeniedException'); $.message: missing (expected 'Cross-account pass role is not allowed.').
Cause 2: eks-cluster-create-invalid-subnet-error#task#1 CreateCluster - status: expected 400, got 200; $headers.x-amzn-errortype: missing (expected 'InvalidParameterException'); $.clusterName: missing (expected 'sim2real-probe-eks-cluster-923ffa20'); $.message: missing (expected "The subnet ID 'subnet-00000000deadbeef1' does not exist (Service: AmazonEC2; Status Code: 400; Error Code: InvalidSubnetID.NotFound; ...)").
Cause 3: eks-register-cluster-invalid-connector-role-error#task#1 RegisterCluster - status: expected 400, got 200; $headers.x-amzn-errortype: missing (expected 'InvalidRequestException'); $.message: missing (expected 'Nonexistent role or missing ssm service principal in trust policy: arn:aws:iam::000000000000:role/sim2real-infra-lambda-exec-role ...').

## Steps to Reproduce
1. Boot the emulator and point an EKS client at it.
2. Cause 1: create_cluster(name=<new>, roleArn='arn:aws:iam::111122223333:role/anything' (an account other than the caller's), resourcesVpcConfig={'subnetIds':[<two valid subnets>]}) -> expect HTTP 403, x-amzn-errortype AccessDeniedException, message 'Cross-account pass role is not allowed.'; observe HTTP 200 and a created cluster.
3. Cause 2: create_cluster(name=<new>, roleArn=<in-account role>, resourcesVpcConfig={'subnetIds':['subnet-00000000deadbeef1']}) -> expect HTTP 400 InvalidParameterException naming that subnet id, with clusterName set; observe HTTP 200. Then describe_cluster(name=<new>) and confirm nothing should exist.
4. Cause 3: register_cluster(name=<new>, connectorConfig={'roleArn':'arn:aws:iam::000000000000:role/<a role with no ssm trust>','provider':'OTHER'}) -> expect HTTP 400 InvalidRequestException naming the roleArn; observe HTTP 200.
