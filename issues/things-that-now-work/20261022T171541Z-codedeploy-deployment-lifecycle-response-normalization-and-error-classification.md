Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.deployment-create-observe-terminal-status codedeploy.skip-wait-time-deployment-already-completed-rejected codedeploy.deployment-group-auto-rollback-empty-events-rejected codedeploy.deployment-group-tag-filter-empty-vs-absent-preserved codedeploy.get-deployment-invalid-id-rejected codedeploy.create-deployment-nonexistent-application-rejected codedeploy.list-on-premises-instances-invalid-registration-status-rejected
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.deployment-create-observe-terminal-status|run#7|AssertionError|assert status == "Failed", f"expected Failed for a zero-instance deployment group, got {status}"
Issue-Key: codedeploy.skip-wait-time-deployment-already-completed-rejected|run#6|AssertionError|assert status == "Failed"
Issue-Key: codedeploy.deployment-group-auto-rollback-empty-events-rejected|run#9|AssertionError|assert "autoRollbackConfiguration" not in info_off, (
Issue-Key: codedeploy.deployment-group-tag-filter-empty-vs-absent-preserved|run#12|AssertionError|assert "onPremisesInstanceTagFilters" in info_empty, info_empty
Issue-Key: codedeploy.get-deployment-invalid-id-rejected|run#3|AssertionError|assert code == "InvalidDeploymentIdException", (
Issue-Key: codedeploy.create-deployment-nonexistent-application-rejected|run#5|AssertionError|assert code == "ApplicationDoesNotExistException", (
Issue-Key: codedeploy.list-on-premises-instances-invalid-registration-status-rejected|run#3|AssertionError|assert code == "InvalidRegistrationStatusException", (
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# CodeDeploy deployment lifecycle, response normalization and error classification diverge from AWS (6 causes)

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDeploymentGroup

## Description
Six independent fidelity divergences in implemented CodeDeploy handlers: one in the deployment state machine, two in response normalization, and three in error classification/ordering.

Cause 1 of 6: a deployment against a deployment group that matches no instances terminates Succeeded instead of Failed.
AWS: recorded behavior only (measured by both checks). A Server-platform deployment whose deployment group selects zero instances runs to the terminal status `Failed`; GetDeployment reports deploymentInfo.status == "Failed".
Emulator: the deployment reaches terminal status "Succeeded".
Why: every deployment looks successful regardless of targeting, so a client (or a CI gate) that treats Succeeded as "artifact is live on at least one instance" is wrong, and the follow-on DeploymentAlreadyCompletedException path can never be exercised.
Fix: the deployment state machine in the CodeDeploy service state / CreateDeployment progression (services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/state.rs and src/ops/create_deployment.rs).
Traces: codedeploy.deployment-create-observe-terminal-status, codedeploy.skip-wait-time-deployment-already-completed-rejected.

Cause 2 of 6: an information-free autoRollbackConfiguration is echoed instead of normalized away.
AWS: recorded behavior only (measured on real AWS us-west-2 by the check). CreateDeploymentGroup with autoRollbackConfiguration {enabled:false, events:[]} carries no information, so AWS normalises it away and GetDeploymentGroup OMITS the `autoRollbackConfiguration` member entirely.
Emulator: GetDeploymentGroup returns autoRollbackConfiguration {"enabled": false, "events": []}.
Why: a client comparing a round-tripped deployment-group document (drift detection, IaC refresh) sees a member AWS does not send and reports spurious drift.
Fix: the CreateDeploymentGroup/GetDeploymentGroup projection (store-side normalization of autoRollbackConfiguration).
Traces: codedeploy.deployment-group-auto-rollback-empty-events-rejected.

Cause 3 of 6: an omitted onPremisesInstanceTagFilters is not normalized to an empty list on read.
AWS: recorded behavior only (measured by the check). CreateDeploymentGroup with an explicit empty `onPremisesInstanceTagFilters` list and with the member omitted entirely both round-trip through GetDeploymentGroup to the SAME representation: the key present with value [].
Emulator: GetDeploymentGroup omits the `onPremisesInstanceTagFilters` key altogether (observed deploymentGroupInfo contains only applicationName, deploymentGroupId, deploymentGroupName, deploymentConfigName, serviceRoleArn, computePlatform, terminationHookEnabled).
Why: a client indexing `info["onPremisesInstanceTagFilters"]` raises KeyError against the emulator where AWS always supplies the empty list, and absent-vs-empty comparisons invert.
Fix: the GetDeploymentGroup serializer / deployment-group projection.
Traces: codedeploy.deployment-group-tag-filter-empty-vs-absent-preserved.

Cause 4 of 6: GetDeployment has no deploymentId format gate.
AWS: Smithy GetDeployment models InvalidDeploymentIdException, DeploymentDoesNotExistException and DeploymentIdRequiredException (all HTTP 400). A malformed id is refused at input validation with InvalidDeploymentIdException (recorded message example: "Specified DeploymentId is not in the valid format: not-a-deployment-id"); only a well-formed-but-unknown id (e.g. "d-ABCDEFGHI") is discriminated on existence with DeploymentDoesNotExistException.
Emulator: answers DeploymentDoesNotExistException for the malformed id, collapsing the two classes.
Why: a client that retries/branches on "bad id" vs "unknown id" takes the wrong branch, and malformed ids are reported as a transient-looking absence.
Fix: GetDeployment's validation (format gate before the existence lookup), services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/get_deployment.rs.
Traces: codedeploy.get-deployment-invalid-id-rejected.

Cause 5 of 6: CreateDeployment validates the revision before the application's existence.
AWS: Smithy CreateDeployment models both ApplicationDoesNotExistException and RevisionRequiredException (HTTP 400). For a never-created applicationName AWS answers ApplicationDoesNotExistException — existence of the named application is checked before the revision member.
Emulator: answers RevisionRequiredException for a CreateDeployment against an application that does not exist.
Why: a client gets told to fix its revision when the real problem is a missing application, and "does this application exist" probes based on this operation report the wrong answer.
Fix: the precondition order in CreateDeployment's validation.
Traces: codedeploy.create-deployment-nonexistent-application-rejected.

Cause 6 of 6: ListOnPremisesInstances refuses an out-of-enum registrationStatus with an unmodeled ValidationException.
AWS: Smithy ListOnPremisesInstances declares `registrationStatus` (enum RegistrationStatus) and models InvalidRegistrationStatusException, InvalidTagFilterException and InvalidNextTokenException (all HTTP 400). An out-of-enum registrationStatus is refused with InvalidRegistrationStatusException.
Emulator: answers error code `ValidationException`, which this operation does not model.
Why: a client catching the modeled InvalidRegistrationStatusException (botocore exposes it on the client's exception factory) does not catch the emulator's error at all and the call escapes as an unmodeled failure.
Fix: ListOnPremisesInstances' validation — refuse with the modeled error rather than the generic validation channel.
Traces: codedeploy.list-on-premises-instances-invalid-registration-status-rejected.

## Evidence
Replayed behavioral suite, codedeploy service. Failing assertions, verbatim:

- codedeploy.deployment-create-observe-terminal-status: "expected Failed for a zero-instance deployment group, got Succeeded"
- codedeploy.skip-wait-time-deployment-already-completed-rejected (check line 33): `assert status == "Failed"` after waiting for a terminal status -> AssertionError (same zero-instance deployment reached Succeeded)
- codedeploy.deployment-group-auto-rollback-empty-events-rejected (check line 85): `assert "autoRollbackConfiguration" not in info_off` -> "an all-empty disabled auto-rollback config carries no information, so real AWS normalises it away and GetDeploymentGroup omits the member entirely; got {'enabled': False, 'events': []}"
- codedeploy.deployment-group-tag-filter-empty-vs-absent-preserved (check line 40): `assert "onPremisesInstanceTagFilters" in info_empty` -> got {'applicationName': ..., 'deploymentGroupId': ..., 'deploymentGroupName': ..., 'deploymentConfigName': 'CodeDeployDefault.OneAtATime', 'serviceRoleArn': ..., 'computePlatform': 'Server', 'terminationHookEnabled': False}
- codedeploy.get-deployment-invalid-id-rejected: "expected InvalidDeploymentIdException for a malformed id, got DeploymentDoesNotExistException"
- codedeploy.create-deployment-nonexistent-application-rejected: "CreateDeployment against a never-created application 'sim2real-cd-missing-app-...' returned 'RevisionRequiredException', expected ApplicationDoesNotExistException"
- codedeploy.list-on-premises-instances-invalid-registration-status-rejected: "expected InvalidRegistrationStatusException for an out-of-enum registrationStatus, got 'ValidationException'"

Smithy (services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json): GetDeployment errors = DeploymentDoesNotExistException, DeploymentIdRequiredException, InvalidDeploymentIdException (400); CreateDeployment errors include ApplicationDoesNotExistException and RevisionRequiredException (400); ListOnPremisesInstances errors = InvalidNextTokenException, InvalidRegistrationStatusException, InvalidTagFilterException (400). Names and ids in the quoted text are environment-specific examples.

## Steps to Reproduce
1. Boot the emulator; create an IAM service role, an application, and a deployment group whose EC2 tag filters match nothing.
2. CreateDeployment with a minimal S3 bundle, poll GetDeployment to a terminal status -> expect "Failed", observe "Succeeded".
3. CreateDeploymentGroup(..., autoRollbackConfiguration={"enabled": False, "events": []}), then GetDeploymentGroup -> expect no `autoRollbackConfiguration` member in deploymentGroupInfo.
4. CreateDeploymentGroup with `onPremisesInstanceTagFilters` omitted, then GetDeploymentGroup -> expect the key present with value [].
5. GetDeployment(deploymentId="not-a-deployment-id") -> expect 400 InvalidDeploymentIdException; GetDeployment(deploymentId="d-ABCDEFGHI") -> expect 400 DeploymentDoesNotExistException.
6. CreateDeployment(applicationName="never-created-app", deploymentGroupName="whatever") -> expect 400 ApplicationDoesNotExistException.
7. ListOnPremisesInstances(registrationStatus="NotAnEnumValue") on a client with parameter_validation disabled -> expect 400 InvalidRegistrationStatusException.

## Resolution note
Cause 3 was fixed as a rule over the deployment group's list members that the model does not scope to one compute platform: `ec2TagFilters`, `onPremisesInstanceTagFilters`, `autoScalingGroups` and `triggerConfigurations` now all report the list the group holds, the empty list included, so an omitted member and an explicitly empty one round-trip identically. Only `onPremisesInstanceTagFilters` carries a recording here; the other three are the same class and follow by the rule, with `triggerConfigurations` the least evidenced of them — a later recording of GetDeploymentGroup can confirm it or narrow the rule to the tag-filter and Auto Scaling members. `ecsServices` is deliberately left absent when empty, because the model scopes it to the ECS compute platform.
