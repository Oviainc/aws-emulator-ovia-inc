Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy__ecs__elbv2.create-ecs-deployment-group-nonexistent-service-rejected
Issue-Key: codedeploy__ecs__elbv2.create-ecs-deployment-group-nonexistent-service-rejected|_error#7|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: 165722d258ec8cbd86766e596a6f87959b068084

# CodeDeploy CreateDeploymentGroup does not resolve ecsServices against ECS: a group naming a nonexistent cluster/service is accepted (200) instead of refused InvalidECSServiceException

> _Note: this issue was identified against the code base at commit `165722d258ec8cbd86766e596a6f87959b068084`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeploymentGroup

## Description
The CreateDeploymentGroup handler stores `ecsServices` as opaque input: it never resolves the named cluster/service against the ECS service's state, so an ECS blue/green deployment group naming a cluster and service that do not exist is created and returned with HTTP 200 instead of being refused.

Cause 1 of 1: ecsServices is not resolved against ECS at create time.
AWS: Smithy CreateDeploymentGroup declares `ecsServices` (ECSServiceList of {serviceName, clusterName}) and models InvalidECSServiceException (client error, HTTP 400). CodeDeploy resolves the pair by calling ECS DescribeServices under the group's serviceRoleArn; when the cluster or service cannot be loaded the call is refused with InvalidECSServiceException, no deployment group is created (ListDeploymentGroups for the application stays empty), and the message wraps ECS's own refusal. Message template, fixed text verbatim with the variable parts substituted from the request and from the ECS refusal: "Could not load ECS service information for cluster: <clusterName>, service: <serviceName>. Cause: <ECS cause>. (Service: AmazonECS; Status Code: <ecs status>; Error Code: <ecs error code>; Request ID: <request id>; Proxy: null)" — here <clusterName>/<serviceName> are the request's ecsServices entry, <ECS cause>/<ecs error code>/<ecs status> come from the ECS call (for a missing cluster: "Cluster not found", ClusterNotFoundException, 400) and <request id> is per-request. No literal name, id or request id from any single run is part of the requirement.
Emulator: returns HTTP 200 and creates the group, so the check's `_error()` helper reports "expected a ClientError, but the call SUCCEEDED and returned 'sim2real-xsvc-dg-...'". The subsequent ListDeploymentGroups therefore also lists a group AWS would never have persisted.
Why: a client that relies on create-time resolution of the ECS service (every CFN/CDK and Terraform ECS blue/green path does) gets a deployment group that looks correctly wired but points at nothing; the misconfiguration surfaces only at deploy time, or never, and no InvalidECSServiceException is ever observable. Typo'd cluster/service names are undetectable.
Fix: CreateDeploymentGroup's validation — resolve each `ecsServices` entry against the ECS service's state (cluster + service must exist) before persisting, and map the failure to InvalidECSServiceException (400) with the message above; in services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/create_deployment_group.rs.
Traces: codedeploy__ecs__elbv2.create-ecs-deployment-group-nonexistent-service-rejected.

Related but not covering this: open issue 20261022T171541Z-codedeploy-createdeploymentgroup-and-createdeployment-accept-input-real-aws.md bundles five other CreateDeploymentGroup/CreateDeployment validation gaps (alarm limit, tag-filter combination, missing loadBalancerInfo on ECS, target-group name syntax, serviceRoleArn resolution). None of them is the resolution of ecsServices against ECS, and this check is not carried by that issue's replay keys.

## Evidence
Replayed behavioral suite, heading `### codedeploy__ecs__elbv2`.

[mismatch] codedeploy__ecs__elbv2.create-ecs-deployment-group-nonexistent-service-rejected
  File "check_codedeploy__ecs__elbv2_create_ecs_deployment_group_nonexistent_service_rejected.py", line 29, in run
    code, msg = _error(lambda: _create_ecs_deployment_group(...))
  AssertionError: expected a ClientError, but the call SUCCEEDED and returned 'sim2real-xsvc-dg-d725afdab212'

Expected (AWS): HTTP 400, Error Code InvalidECSServiceException, Message "Could not load ECS service information for cluster: <ghost cluster>, service: <ghost service>. Cause: Cluster not found. (Service: AmazonECS; Status Code: 400; Error Code: ClusterNotFoundException; Request ID: <request-id>; Proxy: null)", and ListDeploymentGroups(applicationName=app) == [].
Actual (emulator): HTTP 200 with a deploymentGroupId/name; the group is created.
The check's positive control (same request against a real idle CODE_DEPLOY-controlled ECS service, asserting ecsServices == [{serviceName, clusterName}]) is never reached because the negative leg already succeeded. Generated group/cluster/service names and the request id above are environment-specific; the requirement is the 400, the error code, and the message template.

Smithy: CreateDeploymentGroup input member `ecsServices` -> ECSServiceList; modeled error InvalidECSServiceException, errorKind client, httpError 400.

## Steps to Reproduce
1. Boot the emulator. Create an IAM role assumable by codedeploy.amazonaws.com and a CodeDeploy application with computePlatform="ECS".
2. Create an internal ALB with two target groups (blue/green) and an HTTP:80 listener forwarding to blue (elbv2), so loadBalancerInfo is valid.
3. Call CreateDeploymentGroup(applicationName=<app>, deploymentGroupName=<new>, serviceRoleArn=<role>, deploymentStyle={deploymentType:BLUE_GREEN, deploymentOption:WITH_TRAFFIC_CONTROL}, ecsServices=[{serviceName:"sim2real-nonexistent-service", clusterName:"<cluster-that-does-not-exist>"}], loadBalancerInfo={targetGroupPairInfoList:[{targetGroups:[{name:blue},{name:green}], prodTrafficRoute:{listenerArns:[<listener>]}}]}, blueGreenDeploymentConfiguration={...}).
4. Expect HTTP 400 InvalidECSServiceException with the "Could not load ECS service information for cluster: ..., service: ... Cause: Cluster not found. (Service: AmazonECS; ...)" message; observe HTTP 200 and a deploymentGroupId.
5. Call ListDeploymentGroups(applicationName=<app>): expect [], observe the rejected group listed.
6. Control: create a real ECS cluster, register a task definition, create a CODE_DEPLOY-controlled service with desiredCount=0, and repeat step 3 naming them — this must succeed and GetDeploymentGroup must echo ecsServices == [{serviceName, clusterName}].
