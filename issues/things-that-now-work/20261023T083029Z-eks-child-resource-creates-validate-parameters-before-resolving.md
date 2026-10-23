Issue-Source: trace-sweep-divergence
Issue-Trace: eks-capability-error-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eks-capability-error-paths eks-capability-not-found-error-paths eks-compute-resource-creation-on-missing-cluster eks-nodegroup-and-fargate-profile-errors-unknown-cluster eks-nodegroup-create-nonexistent-cluster-error
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|CreateCapability|4xx->4xx|$.message
Issue-Key: eks|CreateCapability|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: eks|CreateCapability|4xx->4xx|status
Issue-Key: eks|CreateNodegroup|4xx->4xx|$.message
Issue-Related: 20261023T023215Z-eks-returns-the-wrong-modeled-error-class-and.md
Issue-Commit: 294bb54d0ff7456773c5757da3560684137bcaef

# EKS child-resource creates validate parameters before resolving the cluster, returning 400 InvalidParameterException instead of 404

> _Note: this issue was identified against the code base at commit `294bb54d0ff7456773c5757da3560684137bcaef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCapability

## Description
Replaying the wire corpus (simparity-wire-aws-v1) against the EKS emulator (services/service-com-amazonaws-eks-awswesleyfrontend) shows TWO distinct root causes with the same shape: a create of a child resource under a cluster that does not exist is answered with a 400 parameter complaint about a sibling member (roleArn, subnets) instead of the 404 not-found that AWS returns, because the emulator's generic/dependency validation runs ahead of cluster resolution. Both causes must be closed; the issue is NOT complete until both are.

Cause 1 of 2 — CreateCapability on a nonexistent cluster is refused for roleArn instead of being reported as a missing cluster.
AWS: recorded behavior only for the not-found class — Smithy (com-amazonaws-eks-awswesleyfrontend): CreateCapability POST /clusters/{clusterName}/capabilities models AccessDeniedException(403), InvalidParameterException(400), InvalidRequestException(400), ResourceInUseException(409), ResourceLimitExceededException(400), ServerException(500), ThrottlingException(429) — ResourceNotFoundException is NOT in that list, so the not-found path is recorded behavior. The recording shows HTTP 404 with the not-found message built from the request ('No cluster found for name: <clusterName>.'); clusterName is an @httpLabel so the cluster must be resolved before the body is judged. That construction rule is the requirement, not the literal recorded name.
Emulator: answers HTTP 400 with x-amzn-errortype InvalidParameterException and a roleArn trust-policy complaint (example: "roleArn Nonexistent role or missing eks service principal in trust policy: arn:aws:iam::000000000000:role/..."). Status and error code both diverge.
Why: a client that keys on the 404/not-found class to decide "create the cluster first, then retry" instead sees a retry-less 400 about a role it did supply, so cluster-then-capability provisioning never recovers, and a policy test would wrongly conclude the role is bad.
Fix: the CreateCapability handler in the eks crate's ops module — resolve clusterName (the httpLabel) before running roleArn/body validation.
Traces: eks-capability-error-paths#task#3 (also eks-capability-not-found-error-paths#task#4, same cluster).

Cause 2 of 2 — CreateNodegroup on a nonexistent cluster is refused for its subnets instead of being reported as a missing cluster.
AWS: recorded behavior only for the not-found class — Smithy: CreateNodegroup POST /clusters/{clusterName}/node-groups models ClientException(400), InvalidParameterException(400), InvalidRequestException(400), ResourceInUseException(409), ResourceLimitExceededException(400), ServerException(500), ServiceUnavailableException(503); ResourceNotFoundException is not modeled, and the recording shows HTTP 404 with x-amzn-errortype ResourceNotFoundException and the request-built message 'No cluster found for name: <clusterName>.'. The recording also leaves the request's path/body members out of the error body (nodegroupName null).
Emulator: answers HTTP 400, x-amzn-errortype InvalidParameterException, with an EC2 subnet complaint (example: "The subnet ID 'subnet-00000000000000000' does not exist (Service: AmazonEC2; Status Code: 400; Error Code: InvalidSubnetID.NotFound)") and echoes nodegroupName into the error body where AWS leaves it null.
Why: a client provisioning a nodegroup against a cluster it has not created yet gets a 400 blaming its subnets; its create-parent-then-child recovery never fires, and nodegroup-dependent flows fail with a misleading cause.
Fix: the CreateNodegroup handler — resolve clusterName before validating subnets/nodeRole, and stop echoing request members into the not-found error body.
Traces: eks-compute-resource-creation-on-missing-cluster#task#2 (also eks-nodegroup-and-fargate-profile-errors-unknown-cluster#task#1, eks-nodegroup-create-nonexistent-cluster-error#task#1).

Related open issues (neither carries these fixtures' replay keys, so these steps must be re-replayed here): '20261022T154613Z-eks-control-plane-operations-answer-501-notimplemented-or.md' (same validate-before-resolve ordering, from the behavioral suite) and '20261023T023215Z-eks-not-found-errors-on-a-missing-cluster.md' (the request-member echo on not-found bodies).

## Evidence
Replay: simparity-wire-aws-v1, 35/216 steps passed, 23 divergence clusters (all eks).

Cause 1 — fixture eks-capability-error-paths step task#3, operation CreateCapability: status expected 404, got 400; $.message value_mismatch (expected 'No cluster found for name: sim2real-probe-eks-ghost-cluster-43c7cc88.', got 'roleArn Nonexistent role or missing eks service principal in trust policy: arn:aws:iam::000000000000:role/sim2real-infra-lambda-exec-role'); $headers.x-amzn-errortype got 'InvalidParameterException'. Cluster also covers eks-capability-not-found-error-paths#task#4.

Cause 2 — fixture eks-compute-resource-creation-on-missing-cluster step task#2, operation CreateNodegroup: status expected 404, got 400; $headers.x-amzn-errortype value_mismatch (expected 'ResourceNotFoundException', got 'InvalidParameterException'); $.message value_mismatch (expected 'No cluster found for name: sim2real-probe-eks-missing-cluster-d33198a9.', got "The subnet ID 'subnet-00000000000000000' does not exist (Service: AmazonEC2; Status Code: 400; Error Code: InvalidSubnetID.NotFound)"); $.nodegroupName type_mismatch (expected None, got 'sim2real-probe-eks-nodegroup-d33198a9'). Cluster also covers eks-nodegroup-and-fargate-profile-errors-unknown-cluster#task#1 and eks-nodegroup-create-nonexistent-cluster-error#task#1.

No state_reset_failures were reported for this replay, so neither divergence is an artifact of leaked fixture state.

## Steps to Reproduce
1. Boot the emulator and point an EKS client at its data listener.
2. Cause 1: create_capability(clusterName=<a name never created>, capabilityName=<any>, type='ACK', roleArn=<any role arn>, deletePropagationPolicy=<any valid value>). Expect HTTP 404 with the not-found error class and a message built from the request ('No cluster found for name: <clusterName>.'); observe HTTP 400 InvalidParameterException complaining about roleArn. Re-replay fixture eks-capability-error-paths step task#3.
3. Cause 2: create_nodegroup(clusterName=<a name never created>, nodegroupName=<any>, subnets=[<any subnet id>], nodeRole=<any role arn>). Expect HTTP 404 x-amzn-errortype ResourceNotFoundException with 'No cluster found for name: <clusterName>.' and no nodegroupName echoed into the error body; observe HTTP 400 InvalidParameterException about the subnet id. Re-replay fixture eks-compute-resource-creation-on-missing-cluster step task#2.
4. Confirm both by re-replaying the wire corpus: these steps must pass on status and x-amzn-errortype.
