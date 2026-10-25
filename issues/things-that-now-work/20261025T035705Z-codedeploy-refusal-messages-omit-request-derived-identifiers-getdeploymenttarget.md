Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.get-deployment-target-not-started-rejected codedeploy.update-deployment-group-alarms-limit-exceeded-blocked
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.get-deployment-target-not-started-rejected|run#15|AssertionError|assert message_fake_early == (
Issue-Key: codedeploy.update-deployment-group-alarms-limit-exceeded-blocked|run#12|AssertionError|assert error2 == (
Issue-Commit: b495a64ceea4ad5841973a813296398afeecb563

# CodeDeploy refusal messages omit request-derived identifiers: GetDeploymentTarget names the target by bare id instead of its ARN, and UpdateDeploymentGroup's duplicate-alarm refusal omits the duplicate's position (2 causes)

> _Note: this issue was identified against the code base at commit `b495a64ceea4ad5841973a813296398afeecb563`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDeploymentTarget

## Description
Two net-new root causes in the CodeDeploy emulator. In BOTH, the emulator already answers the right HTTP status (400) and the right modeled error code, and the ONLY divergence is that its message drops a request-derived element AWS's message carries — the identifier FORM of the offending value (Cause 1) and the ordinal position of the offending list entry (Cause 2). These are not wording preferences: in each case the content AWS derives from the request is absent from the emulator's message, so the behavioral checks that read those elements back cannot pass. Severity is LOW for the bundle because the status and code (the hard requirements) already match.

Cause 1 of 2: GetDeploymentTarget's DeploymentTargetDoesNotExistException names the target by its bare target id instead of by its ARN.
AWS: Smithy GetDeploymentTarget (services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json) declares deploymentId and targetId required and models DeploymentTargetDoesNotExistException (HTTP 400, doc "The provided target ID does not belong to the attempted deployment"). The model documents no message text for the exception (its `message` member's doc is only "The message that corresponds to the exception thrown by CodeDeploy"), so the modeled requirement is the 400 and the code. The check's measured AWS behavior (four replays, us-west-2) adds one request-derived rule on top of that: the refusal identifies the target by its full resource ARN, not by the id the request carried — an EC2 instance target as arn:<partition>:ec2:<region>:<account>:instance/<instance-id>, an on-premises instance target as the instanceArn GetOnPremisesInstance reports. Recorded message example (an example, not a required string): "The target arn:aws:ec2:us-west-2:<account>:instance/i-0123456789abcdef0 is not part of deployment d-XXXXXXXXX".
Emulator: HTTP 400 with the correct code DeploymentTargetDoesNotExistException, but a message built from the bare id, e.g. "The deployment target i-0123456789abcdef0 does not belong to the deployment d-PO017YCLX". No ARN is rendered at all, for either an EC2-shaped id or a registered on-premises instance name.
Why: the identifier form is the part of this refusal a client can act on. A caller that parses the ARN out of the refusal to learn the partition/region/account/resource-type of the target it asked about (or that matches the refusal against ARNs it holds from GetOnPremisesInstance / ListDeploymentTargets) gets no ARN from the emulator and cannot correlate the rejection with the resource, while the same code path against AWS resolves it.
Fix: the CodeDeploy state module's refusal-message builder for this error — `crate::state::deployment_target_not_found(&deployment_id, &target_id)`, called from the handler, must render the target as its ARN (resolving an on-premises instance name to the registered instanceArn and an EC2 instance id to an ec2 instance ARN in the request's partition/region/account) rather than echoing the raw target id.
Traces: codedeploy.get-deployment-target-not-started-rejected (fails on the first early read, for a target id that never belongs to the deployment; the same assertion follows for the real registered on-premises instance). Code citations: services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/get_deployment_target.rs:238-249.

Cause 2 of 2: UpdateDeploymentGroup's duplicate-alarm InvalidAlarmConfigException omits which entry in the submitted alarm list is the duplicate.
AWS: Smithy UpdateDeploymentGroup models InvalidAlarmConfigException (HTTP 400); its doc enumerates the invalid-configuration causes including duplicate alarm names, and the model documents no message text. Measured AWS behavior adds the request-derived rule: the refusal names the duplicated alarm name AND its 1-based position in the alarms list submitted on the request. Recorded message example: "Name <alarm-name> of Alarm 2 already exists." for a two-element list whose second entry repeats the first.
Emulator: HTTP 400 with the correct code InvalidAlarmConfigException, but a message carrying only the name and no position, e.g. "The alarm configuration is invalid: the alarm name sim2real-alarm-facfc8579a3b was specified more than once".
Why: with a long alarm list the position is the only thing that tells the caller WHICH submitted entry to drop; a name alone is ambiguous when the same name appears several times or when several names collide, so a client that reports the offending list index back to its own user cannot. Note the emulator's sibling refusal in the same operation (enabled=true with an empty alarms list) already matches, so this is specifically the duplicate-detection path.
Why this is not covered by an existing issue: the open issue "CodeDeploy accepts invalid input and misclassifies rejections" (20261022T213118Z-codedeploy-accepts-invalid-input-and-misclassifies-rejections.md) covers wire validation gates on CreateDeploymentGroup/ListDeployments/StopDeployment/GetDeployment/RegisterApplicationRevision and carries no key for either path here; the open issue "CodeDeploy CreateDeploymentGroup and CreateDeployment accept input real AWS refuses" covers CREATE-time validation, not UpdateDeploymentGroup's alarm-duplicate path or GetDeploymentTarget.
Fix: UpdateDeploymentGroup's alarm-configuration validation (the duplicate-name gate) in the CodeDeploy service crate — carry the 1-based index of the offending alarms entry into the message it builds.
Traces: codedeploy.update-deployment-group-alarms-limit-exceeded-blocked (second negative, at check line 58; the first negative and the persistence/positive-control assertions were not reached).

## Evidence
Behavioral suite simparity-behavioral-ga-prod-v1 replayed against the emulator built from this checkout (emulator up and serving: 53/91 checks passed overall, bootstrap_signal false; codedeploy 53/86).

Cause 1 — codedeploy.get-deployment-target-not-started-rejected [mismatch]:
  expected (measured AWS): error code DeploymentTargetDoesNotExistException with a message of the form "The target <targetArn> is not part of deployment <deploymentId>", where <targetArn> is arn:<partition>:ec2:<region>:<account>:instance/i-0123456789abcdef0 for the never-a-member id.
  actual (emulator): error code DeploymentTargetDoesNotExistException (correct) with "The deployment target i-0123456789abcdef0 does not belong to the deployment d-PO017YCLX" — the target rendered as the bare id, no ARN.
  Assertion: check_codedeploy_get_deployment_target_not_started_rejected.py line 65.

Cause 2 — codedeploy.update-deployment-group-alarms-limit-exceeded-blocked [mismatch]:
  expected (measured AWS): ('InvalidAlarmConfigException', 'Name <alarm-name> of Alarm 2 already exists.')
  actual (emulator): ('InvalidAlarmConfigException', 'The alarm configuration is invalid: the alarm name sim2real-alarm-facfc8579a3b was specified more than once') — correct code, position of the duplicate entry absent.
  Assertion: check_codedeploy_update_deployment_group_alarms_limit_exceeded_blocked.py line 58. The preceding negative (enabled=true, alarms=[]) passed.

Smithy grounding (services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json): DeploymentTargetDoesNotExistException and InvalidAlarmConfigException are both client errors, HTTP 400, each with a single `message` member documented only as "The message that corresponds to the exception thrown by CodeDeploy" — the status and the error code are the modeled requirements and both already match; the divergence filed here is the request-derived content of the message (the target's ARN form; the duplicate entry's 1-based position), which is what the recordings show AWS deriving from the request.

Emulator source: services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/get_deployment_target.rs lines 238-249 raise the error with `crate::state::deployment_target_not_found(&deployment_id, &target_id)`, i.e. from the raw target id.

## Steps to Reproduce
Cause 1 (GetDeploymentTarget):
1. Create an IAM service role for codedeploy, an S3 revision bundle and a Server-platform CodeDeploy application.
2. RegisterOnPremisesInstance(instanceName=<name>, iamUserArn=<user>) and create a deployment group whose on-premises tag filter matches it.
3. CreateDeployment(applicationName, deploymentGroupName, revision) and note the deploymentId.
4. GetDeploymentTarget(deploymentId=<id>, targetId='i-0123456789abcdef0') -> expect 400 DeploymentTargetDoesNotExistException whose message names the target as arn:<partition>:ec2:<region>:<account>:instance/i-0123456789abcdef0; observe the correct code with the bare id 'i-0123456789abcdef0' in the message.
5. Same call with targetId=<the registered on-premises instance name> immediately after CreateDeployment -> expect the message to name it by the instanceArn GetOnPremisesInstance returns.

Cause 2 (UpdateDeploymentGroup):
1. CreateApplication(computePlatform='Server') and CreateDeploymentGroup(..., alarmConfiguration={enabled:true, alarms:[{name:A}]}).
2. UpdateDeploymentGroup(applicationName, currentDeploymentGroupName, alarmConfiguration={enabled:true, alarms:[{name:A},{name:A}]}) -> expect 400 InvalidAlarmConfigException whose message names A and the 1-based index of the duplicate entry ('... of Alarm 2 ...'); observe the correct code with no position in the message.
3. GetDeploymentGroup confirms the stored alarms are untouched (this part already matches).

Both reproduce by replaying the two check ids in trace_ids against the emulator built from this checkout.
