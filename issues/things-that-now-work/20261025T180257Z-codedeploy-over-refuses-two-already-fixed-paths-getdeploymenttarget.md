Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.get-deployment-target-not-started-rejected codedeploy.update-deployment-group-alarms-limit-exceeded-blocked
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.get-deployment-target-not-started-rejected|run#17|AssertionError|assert code_real_early == "DeploymentTargetDoesNotExistException", (
Issue-Key: codedeploy|UpdateDeploymentGroup|AlarmsLimitExceededException
Issue-Commit: 44def97d02edbb877c5041813b9a7a90ab5a8c58

# CodeDeploy over-refuses two already-"fixed" paths: GetDeploymentTarget answers DeploymentNotStartedException for a resolved member target, and UpdateDeploymentGroup enforces the 10-alarm deployment-group quota AWS does not apply (2 causes)

> _Note: this issue was identified against the code base at commit `44def97d02edbb877c5041813b9a7a90ab5a8c58`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDeploymentTarget

## Description
REOPENS 20261025T035705Z-codedeploy-refusal-messages-omit-request-derived-identifiers-getdeploymenttarget.md: its fix left the call refused.

That issue bundled exactly these two check ids and fixed only the MESSAGE content of two refusals (GetDeploymentTarget's target-ARN rendering, UpdateDeploymentGroup's duplicate-alarm ordinal). Both message fixes landed — the assertions it named now pass — but each check then advances to a call that the emulator still REFUSES where AWS does not refuse it that way. These are 2 net-new root causes in the CodeDeploy emulator, both in the same service, and BOTH must be closed before submitting.

---

Cause 1 of 2: GetDeploymentTarget refuses a target that IS a member of the deployment with DeploymentNotStartedException, where AWS answers DeploymentTargetDoesNotExistException (and, once the per-target record exists, succeeds).

AWS: Smithy GetDeploymentTarget (services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json) declares deploymentId and targetId required, returns deploymentTarget, and models both DeploymentTargetDoesNotExistException (HTTP 400, doc "The provided target ID does not belong to the attempted deployment") and DeploymentNotStartedException (HTTP 400, doc "The specified deployment has not started"). The model documents no message text for either (the `message` member's doc is only "The message that corresponds to the exception thrown by CodeDeploy"), so the modeled requirements are the 400 and the code. The check's recorded behavior supplies the state rule the model does not: across four replays against real AWS (us-west-2) covering a zero-instance group and a real-on-premises-instance group, reading a never-a-member id and the real member id immediately after CreateDeployment in every combination, DeploymentNotStartedException was NEVER observed from this operation at all. AWS has exactly two outcomes here, both keyed on the per-target RECORD and not on deployment progress: while the per-target record does not yet exist the call is refused HTTP 400 / DeploymentTargetDoesNotExistException with the message template "The target <targetArn> is not part of deployment <deploymentId>" — fixed text "The target " / " is not part of deployment ", with <targetArn> and <deploymentId> taken from the request and the emulator's state (an on-premises target rendered as the instanceArn GetOnPremisesInstance reports, an EC2-shaped id as arn:<partition>:ec2:<region>:<account>:instance/<instance-id>); and once the deployment has resolved its targets the SAME call for that member returns 200 with the target's own record, while a never-a-member id stays refused with DeploymentTargetDoesNotExistException forever.

Emulator: for a targetId that IS a resolved member of the deployment it returns HTTP 400 with error code DeploymentNotStartedException (message built by crate::state::deployment_not_started(&deployment_id)) whenever the deployment has not reached its started state. The never-a-member path is already correct — that id gets DeploymentTargetDoesNotExistException with the right ARN-bearing message — so the divergence is specifically the member-before-record branch: the handler resolves membership first, then gates on deployment progress with an `if !deployment.is_started()` arm that AWS has no equivalent of.

Why: a client cannot distinguish "this target is not mine" from "not ready yet". AWS gives one code for the whole not-yet-readable window (DeploymentTargetDoesNotExistException) and flips to success, so the standard poll is "retry while DeploymentTargetDoesNotExistException, stop on 200". Against the emulator that poll sees an unexpected DeploymentNotStartedException, which the client either treats as fatal or never retries on, so per-target deployment inspection — the only way to read an individual instance's or task set's outcome — breaks for exactly the member ids it is meant to serve.

Fix: the GetDeploymentTarget handler's membership/progress branching. The deployment-progress gate must not produce a distinct error code for a resolved member: an unreadable member must take the same DeploymentTargetDoesNotExistException path the never-a-member id takes, and become readable when its per-target record exists.

Traces: codedeploy.get-deployment-target-not-started-rejected. Code citations: services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/get_deployment_target.rs lines 240-262 (the correct DeploymentTargetDoesNotExistException arm at 240-253, then the diverging `if !deployment.is_started()` arm at 256-262).

---

Cause 2 of 2: UpdateDeploymentGroup refuses an over-quota alarm list with AlarmsLimitExceededException, where AWS accepts it and persists every entry.

AWS: Smithy UpdateDeploymentGroup models AlarmsLimitExceededException (HTTP 400, doc "The maximum number of alarms for a deployment group (10) was exceeded.") and takes alarmConfiguration (AlarmConfiguration, whose `alarms` member doc says "A maximum of 10 alarms can be added"). So the 10-alarm figure IS in the model as documentation — but the model documents a quota, not a synchronous input gate on this operation, and the check's recorded behavior shows AWS does not enforce it here: an 11-alarm enabled alarmConfiguration submitted to UpdateDeploymentGroup is ACCEPTED (2xx) and GetDeploymentGroup afterwards reports all 11 alarm names, neither refused nor silently truncated. The same documented quota IS enforced synchronously elsewhere — on CreateDeployment's overrideAlarmConfiguration, which refuses with AlarmsLimitExceededException (covered by its own check, codedeploy.create-deployment-alarms-limit-exceeded-rejected) — so this is an operation-scoped difference, not a claim that the quota is fictional. Hypothesis (neither the model nor the recording states the mechanism): the deployment-group alarm quota is applied asynchronously or at deployment time rather than at UpdateDeploymentGroup admission.

Emulator: HTTP 400 with error code AlarmsLimitExceededException and message "The maximum number of alarms for a deployment group (10) was exceeded: 11 alarms were specified" — fixed text "The maximum number of alarms for a deployment group (" / ") was exceeded: " / " alarms were specified", with the limit and the submitted count filled in. The call never reaches state, so nothing is persisted. The sibling validations on the same operation are already correct and still pass: enabled=true with an empty alarms list is refused InvalidAlarmConfigException, the duplicate-name refusal now carries the offending entry's ordinal, and a well-formed single-alarm update is applied and persisted.

Why: a legitimate configuration AWS accepts is rejected, so an operator or IaC tool that attaches more than ten alarms to a deployment group — valid against AWS, where the request succeeds and all alarms are stored — gets a hard 400 against the emulator and the deployment group keeps its previous alarm set. The failure mode is the worst kind for a converging tool: the write is refused, the tool retries the identical request, and it is refused again forever.

Fix: UpdateDeploymentGroup's alarm-configuration validation — the AlarmQuota::DeploymentGroup count check applied at admission must not refuse this operation; the list must be accepted and persisted in full (the malformed/empty and duplicate-name gates stay, since AWS does refuse those). Note the related FIXED issue 20261023T062751Z-codedeploy-wrong-batch-alarm-quota-thresholds-two-unimplemented.md: its Cause 2 separated the deployment-level alarm quota from the deployment-group one and explicitly left "the deployment-group quota used by CreateDeploymentGroup/UpdateDeploymentGroup" in place as correct; the recording shows UpdateDeploymentGroup does not gate on it.

Traces: codedeploy.update-deployment-group-alarms-limit-exceeded-blocked. Code citations: services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/update_deployment_group.rs lines 333-348 (crate::state::validate_alarm_configuration(configuration, crate::state::AlarmQuota::DeploymentGroup) -> AlarmConfigurationRejection::TooManyAlarms -> AlarmsLimitExceededException).

## Evidence
Behavioral suite simparity-behavioral-ga-prod-v1 replayed against the emulator built from this checkout. The emulator was up and serving (bootstrap_signal false): 56/91 checks passed overall, codedeploy 56/86. Of the 35 failures, 33 are already filed (marked OWNED by an open issue, or ALREADY FILED behind an unimplemented scaffold stub — CreateInternetGateway blocking 7, TagResource 2, BatchGetDeploymentInstances 1); the 2 below are the net-new survivors. Expected = behavior measured on real AWS by the check; Actual = emulator.

Cause 1 — codedeploy.get-deployment-target-not-started-rejected [mismatch]:
  Failing assertion (check_codedeploy_get_deployment_target_not_started_rejected.py line 72):
    assert code_real_early == "DeploymentTargetDoesNotExistException"
  AssertionError: expected DeploymentTargetDoesNotExistException for the real registered instance before its per-target record has been created, got 'DeploymentNotStartedException'
  expected (measured AWS, 4 replays us-west-2): HTTP 400, code DeploymentTargetDoesNotExistException, message of the form "The target <instanceArn> is not part of deployment <deploymentId>"; DeploymentNotStartedException was never observed from GetDeploymentTarget in any replay.
  actual (emulator): HTTP 400, code DeploymentNotStartedException.
  Progress note: the two preceding assertions in the same check — the never-a-member id's code AND its full ARN-bearing message (lines 65-68), which are what fixed issue 20261025T035705Z addressed — now PASS. The check advances past them and fails on the member-target branch.

Cause 2 — codedeploy.update-deployment-group-alarms-limit-exceeded-blocked [error]:
  Uncaught botocore.errorfactory.AlarmsLimitExceededException: An error occurred (AlarmsLimitExceededException) when calling the UpdateDeploymentGroup operation: The maximum number of alarms for a deployment group (10) was exceeded: 11 alarms were specified
  expected (measured AWS): UpdateDeploymentGroup(alarmConfiguration={enabled:true, alarms:[11 distinct names]}) is ACCEPTED (2xx), and the following GetDeploymentGroup reports all 11 names in deploymentGroupInfo.alarmConfiguration.alarms.
  actual (emulator): HTTP 400 AlarmsLimitExceededException; the request is refused and the stored alarm set is unchanged. The check does not wrap this call in its error helper precisely because AWS does not refuse it, so the refusal surfaces as an uncaught ClientError rather than a message mismatch.
  Progress note: all earlier legs now PASS — the enabled-but-empty refusal (InvalidAlarmConfigException, correct message), the duplicate-name refusal INCLUDING the ordinal that fixed issue 20261025T035705Z Cause 2 added, the unchanged-state read, and the well-formed single-alarm positive control with its persistence check. The check reaches its final leg and fails there.

Smithy grounding (services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json):
  - GetDeploymentTarget: input deploymentId + targetId both required; output deploymentTarget; models DeploymentTargetDoesNotExistException, DeploymentNotStartedException, DeploymentDoesNotExistException, DeploymentIdRequiredException, DeploymentTargetIdRequiredException, InvalidDeploymentIdException, InvalidDeploymentTargetIdException, InvalidInstanceNameException — all client errors, HTTP 400.
  - DeploymentTargetDoesNotExistException: client, 400, doc "The provided target ID does not belong to the attempted deployment."; DeploymentNotStartedException: client, 400, doc "The specified deployment has not started." Both carry only a `message` member documented as "The message that corresponds to the exception thrown by CodeDeploy" — no message text is modeled, so the quoted templates are recorded behavior.
  - UpdateDeploymentGroup: models AlarmsLimitExceededException and InvalidAlarmConfigException (both client, 400) among 32 errors; AlarmsLimitExceededException doc "The maximum number of alarms for a deployment group (10) was exceeded."; AlarmConfiguration.alarms doc "A list of alarms configured for the deployment or deployment group. A maximum of 10 alarms can be added." The model documents the quota but binds no synchronous admission gate to this operation.

Emulator source:
  - services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/get_deployment_target.rs:240-262 — membership resolves correctly to DeploymentTargetDoesNotExistException at 240-253, then lines 256-262 override a resolved member with DeploymentNotStartedException via crate::state::deployment_not_started(&deployment_id). The in-file comment at 232-239 states the intended design ("Only for a target the deployment DOES have is there a state to report, which is what DeploymentNotStartedException refuses until the clock has started the deployment") — that is the assumption the recordings contradict.
  - services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/update_deployment_group.rs:333-348 — crate::state::validate_alarm_configuration(configuration, crate::state::AlarmQuota::DeploymentGroup); the TooManyAlarms rejection is mapped to UpdateDeploymentGroupErrorModeled::AlarmsLimitExceededException before any state write.

## Steps to Reproduce
Cause 1 (GetDeploymentTarget — wrong code for a member target):
1. Create an IAM service role for codedeploy, an S3 revision bundle, and a Server-platform CodeDeploy application.
2. RegisterOnPremisesInstance(instanceName=<H>, iamUserArn=<arn>), then CreateDeploymentGroup whose on-premises tag filter matches <H>.
3. CreateDeployment(applicationName, deploymentGroupName, revision); note <deploymentId>.
4. GetOnPremisesInstance(instanceName=<H>) and keep instanceInfo.instanceArn.
5. Immediately GetDeploymentTarget(deploymentId=<deploymentId>, targetId='i-0123456789abcdef0') -> expect 400 DeploymentTargetDoesNotExistException, message "The target arn:<partition>:ec2:<region>:<account>:instance/i-0123456789abcdef0 is not part of deployment <deploymentId>". This already matches.
6. Immediately GetDeploymentTarget(deploymentId=<deploymentId>, targetId=<H>) -> expect 400 DeploymentTargetDoesNotExistException with message "The target <instanceArn> is not part of deployment <deploymentId>"; OBSERVE 400 DeploymentNotStartedException instead.
7. Poll ListDeploymentTargets(deploymentId=<deploymentId>) until <H> appears, then GetDeploymentTarget(deploymentId=<deploymentId>, targetId=<H>) -> expect 200 whose deploymentTarget.instanceTarget.targetId == <H>; and the step-5 call for 'i-0123456789abcdef0' must STILL be refused DeploymentTargetDoesNotExistException.

Cause 2 (UpdateDeploymentGroup — over-enforced alarm quota):
1. CreateApplication(computePlatform='Server') and CreateDeploymentGroup(..., alarmConfiguration={enabled:true, alarms:[{name:A}]}) with a codedeploy service role.
2. UpdateDeploymentGroup(applicationName, currentDeploymentGroupName, alarmConfiguration={enabled:true, alarms:[{name:B}]}) -> 2xx, and GetDeploymentGroup reports exactly {B}. This already matches.
3. UpdateDeploymentGroup(applicationName, currentDeploymentGroupName, alarmConfiguration={enabled:true, alarms:[11 distinct alarm names]}) -> expect 2xx (AWS accepts); OBSERVE 400 AlarmsLimitExceededException "The maximum number of alarms for a deployment group (10) was exceeded: 11 alarms were specified".
4. GetDeploymentGroup(applicationName, deploymentGroupName) -> expect deploymentGroupInfo.alarmConfiguration.alarms to contain all 11 submitted names (neither refused nor truncated); observe the pre-step-3 alarm set unchanged.

Both reproduce by replaying the two check ids in trace_ids against the emulator built from this checkout.
