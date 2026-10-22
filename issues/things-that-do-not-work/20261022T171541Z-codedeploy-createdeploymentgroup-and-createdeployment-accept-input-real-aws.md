Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.create-deployment-alarms-limit-exceeded-rejected codedeploy.create-deployment-group-conflicting-tag-filters-rejected codedeploy.create-deployment-group-ecs-without-load-balancer-info-rejected codedeploy.deployment-group-blue-green-config-without-blue-green-style-rejected codedeploy.deployment-group-invalid-role-rejected
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.create-deployment-alarms-limit-exceeded-rejected|_error#6|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: codedeploy.create-deployment-group-conflicting-tag-filters-rejected|_error#14|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: codedeploy.create-deployment-group-ecs-without-load-balancer-info-rejected|_error#5|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: codedeploy.deployment-group-blue-green-config-without-blue-green-style-rejected|_error#15|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: codedeploy.deployment-group-invalid-role-rejected|_error#9|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# CodeDeploy CreateDeploymentGroup and CreateDeployment accept input real AWS refuses, returning 200 and creating the resource (5 causes)

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeploymentGroup

## Description
The implemented CreateDeploymentGroup and CreateDeployment handlers store their input as an opaque document: none of CodeDeploy's cross-member and limit preconditions is enforced, so five request classes that AWS refuses with a modeled 400 are instead accepted with HTTP 200 and the resource is created. Five distinct validation rules, five root causes.

Cause 1 of 5: CreateDeployment ignores the 50-alarm limit on overrideAlarmConfiguration.
AWS: Smithy CreateDeployment declares member `overrideAlarmConfiguration` (AlarmConfiguration) and models AlarmsLimitExceededException (HTTP 400, "The maximum number of alarms for a deployment group (10) was exceeded" family). Measured on real AWS: 51 alarms are refused with AlarmsLimitExceededException and NO deployment record is created; 10 alarms are accepted.
Emulator: returns HTTP 200 with a deploymentId for a 51-alarm overrideAlarmConfiguration.
Why: a client that relies on the limit refusal silently creates a deployment AWS would never have created, and its ListDeployments view diverges.
Fix: CreateDeployment's validation, services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/create_deployment.rs.
Traces: codedeploy.create-deployment-alarms-limit-exceeded-rejected.

Cause 2 of 5: CreateDeploymentGroup accepts both onPremisesInstanceTagFilters and onPremisesTagSet.
AWS: Smithy CreateDeploymentGroup declares both `onPremisesInstanceTagFilters` and `onPremisesTagSet` and models InvalidOnPremisesTagCombinationException (HTTP 400). The two target selectors are mutually exclusive; supplying both is refused and the group is not created (a subsequent GetDeploymentGroup answers DeploymentGroupDoesNotExistException).
Emulator: returns HTTP 200 with a deploymentGroupId.
Why: the group is created with an ambiguous target selection that AWS would never persist, so target resolution at deploy time differs from AWS.
Fix: CreateDeploymentGroup's cross-member validation, services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/create_deployment_group.rs.
Traces: codedeploy.create-deployment-group-conflicting-tag-filters-rejected.

Cause 3 of 5: CreateDeploymentGroup accepts an ECS-platform group with ecsServices but no loadBalancerInfo.
AWS: Smithy CreateDeploymentGroup models InvalidLoadBalancerInfoException (HTTP 400). An ECS compute-platform application with `ecsServices` set and `loadBalancerInfo` omitted is refused (recorded message example: "For ECS deployment group, loadBalancerInfo must be specified"); the same omission on a Server-platform group is legitimate and accepted.
Emulator: returns HTTP 200 with a deploymentGroupId.
Why: an ECS blue/green group that can never shift traffic is created, so the failure surfaces only later (or never) instead of at create time.
Fix: CreateDeploymentGroup's validation, keyed on the application's computePlatform.
Traces: codedeploy.create-deployment-group-ecs-without-load-balancer-info-rejected.

Cause 4 of 5: CreateDeploymentGroup does not syntax-check loadBalancerInfo target-group names.
AWS: target-group names in `loadBalancerInfo.targetGroupInfoList[].name` are syntax-checked at create time (max 32 characters) even though their existence is NOT resolved; an over-long name is refused with InvalidLoadBalancerInfoException (HTTP 400). Measured on real AWS (us-west-2) by the check.
Emulator: returns HTTP 200 with a deploymentGroupId for a target-group name over 32 characters.
Why: a client that depends on create-time name validation stores an unusable blue/green configuration.
Fix: CreateDeploymentGroup's validation of loadBalancerInfo.
Traces: codedeploy.deployment-group-blue-green-config-without-blue-green-style-rejected.

Cause 5 of 5: CreateDeploymentGroup accepts a non-existent / non-assumable serviceRoleArn.
AWS: Smithy CreateDeploymentGroup declares `serviceRoleArn` required and models InvalidRoleException (HTTP 400); a well-formed ARN naming a role that does not exist or cannot be assumed by codedeploy.amazonaws.com is refused and the group is not created.
Emulator: returns HTTP 200 with a deploymentGroupId.
Why: every emulator deployment group appears correctly wired regardless of IAM, so the single most common real-world misconfiguration is undetectable; checks that use the documented InvalidRoleException retry loop also lose their only signal.
Fix: CreateDeploymentGroup's validation, resolving the role against the IAM service state.
Traces: codedeploy.deployment-group-invalid-role-rejected.

## Evidence
Replayed behavioral suite, codedeploy service. All five checks fail with "expected a ClientError, but the call SUCCEEDED".

Expected (AWS): HTTP 400 with, respectively, AlarmsLimitExceededException, InvalidOnPremisesTagCombinationException, InvalidLoadBalancerInfoException, InvalidLoadBalancerInfoException, InvalidRoleException — and no resource created.
Actual (emulator):
- codedeploy.create-deployment-alarms-limit-exceeded-rejected: "expected a ClientError, but the call SUCCEEDED and returned {'deploymentId': 'd-J2SHOOT7N', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}" (51 alarms in overrideAlarmConfiguration)
- codedeploy.create-deployment-group-conflicting-tag-filters-rejected: "... SUCCEEDED and returned {'deploymentGroupId': '7035f4bb-...', 'HTTPStatusCode': 200}" (onPremisesInstanceTagFilters + onPremisesTagSet together)
- codedeploy.create-deployment-group-ecs-without-load-balancer-info-rejected: "... SUCCEEDED and returned {'deploymentGroupId': '1a788b63-...', 'HTTPStatusCode': 200}" (computePlatform=ECS, ecsServices set, no loadBalancerInfo)
- codedeploy.deployment-group-blue-green-config-without-blue-green-style-rejected (check line 171): "expected a rejection, but the call SUCCEEDED and returned {'deploymentGroupId': 'd118309e-...', 'HTTPStatusCode': 200}" (targetGroupInfoList name longer than 32 chars)
- codedeploy.deployment-group-invalid-role-rejected: "real AWS rejects this call, but it SUCCEEDED and returned {'deploymentGroupId': '862d6644-...', 'HTTPStatusCode': 200}" (serviceRoleArn arn:aws:iam::<acct>:role/sim2real-no-such-role-...)

Generated ids, timestamps and names above are environment-specific examples; the requirement is the status and error code. Smithy: CreateDeploymentGroup models AlarmsLimitExceededException, InvalidOnPremisesTagCombinationException, InvalidLoadBalancerInfoException and InvalidRoleException (all HTTP 400); CreateDeployment models AlarmsLimitExceededException (HTTP 400).

## Steps to Reproduce
1. Boot the emulator; create a service role via IAM and an application via CreateApplication.
2. CreateDeployment(applicationName=app, deploymentGroupName=dg, revision=<S3>, overrideAlarmConfiguration={enabled:true, alarms:[51 entries]}) -> expect 400 AlarmsLimitExceededException and ListDeployments still empty; observe 200 + deploymentId.
3. CreateDeploymentGroup(..., onPremisesInstanceTagFilters=[one filter], onPremisesTagSet={onPremisesTagSetList:[[same filter]]}) -> expect 400 InvalidOnPremisesTagCombinationException.
4. CreateApplication(computePlatform="ECS"), then CreateDeploymentGroup(deploymentStyle=BLUE_GREEN/WITH_TRAFFIC_CONTROL, ecsServices=[{serviceName, clusterName}]) with no loadBalancerInfo -> expect 400 InvalidLoadBalancerInfoException.
5. CreateDeploymentGroup(..., loadBalancerInfo={targetGroupInfoList:[{name: 33+ chars}]}) -> expect 400 InvalidLoadBalancerInfoException.
6. CreateDeploymentGroup(..., serviceRoleArn="arn:aws:iam::<acct>:role/does-not-exist") -> expect 400 InvalidRoleException, then GetDeploymentGroup on that name -> expect DeploymentGroupDoesNotExistException.
