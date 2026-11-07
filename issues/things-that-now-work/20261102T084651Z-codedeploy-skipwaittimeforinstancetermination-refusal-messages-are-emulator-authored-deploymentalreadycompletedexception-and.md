Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.skip-wait-time-already-completed-rejected codedeploy.skip-wait-time-deployment-already-completed-rejected codedeploy.skip-wait-time-triggers-immediate-termination codedeploy.skip-wait-time-nonexistent-deployment-fails
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.skip-wait-time-already-completed-rejected|run#13|AssertionError|assert refusal == expected, f"expected {expected!r}, got {refusal!r}"
Issue-Key: codedeploy.skip-wait-time-deployment-already-completed-rejected|run#10|AssertionError|assert refusal == expected, f"expected {expected!r}, got {refusal!r}"
Issue-Key: codedeploy.skip-wait-time-triggers-immediate-termination|run#16|AssertionError|assert refusal == expected, (
Issue-Key: codedeploy.skip-wait-time-nonexistent-deployment-fails|run#20|AssertionError|assert real_refusal == expected, (
Issue-Commit: 86c9f1e4438a61fbe37b19dfcd3d77efce3d0679

# SkipWaitTimeForInstanceTermination refusal messages are emulator-authored: DeploymentAlreadyCompletedException and UnsupportedActionForDeploymentTypeException do not use AWS's sentence templates (2 causes)

> _Note: this issue was identified against the code base at commit `86c9f1e4438a61fbe37b19dfcd3d77efce3d0679`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SkipWaitTimeForInstanceTermination

## Description
Two refusal paths of SkipWaitTimeForInstanceTermination answer with the correct HTTP 400 and the correct modeled error code, but with a locally invented message instead of the sentence AWS words for that error class. Two independent message sites in one handler, two causes. Both are net-new: the open issue "CodeDeploy rejection paths return the right error codes with the wrong error messages (9 causes)" (20261029T180825Z-codedeploy-rejection-paths-return-the-right-error-codes.md) enumerates nine other message sites and does NOT cover either of these, and no other open codedeploy issue carries these replay keys.

Cause 1 of 2: DeploymentAlreadyCompletedException message for a terminal deployment.
AWS: Smithy (services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json) binds SkipWaitTimeForInstanceTermination -> errors DeploymentAlreadyCompletedException, DeploymentDoesNotExistException, DeploymentIdRequiredException, DeploymentNotStartedException, InvalidDeploymentIdException, UnsupportedActionForDeploymentTypeException, all client errors at HTTP 400; the model documents no message text, so this is recorded behavior only. Recorded behavior: calling the operation on a deployment that has already reached a terminal state is refused 400 DeploymentAlreadyCompletedException with the message template "The deployment <deploymentId> has already completed, current state <STATE>." — fixed text "The deployment ", " has already completed, current state " and the trailing period, with two variable parts taken from the request and state: <deploymentId> verbatim from the request, and <STATE> the deployment's INTERNAL state name in upper case, which is not the API status string: Succeeded -> SUCCEEDED, Failed -> FAILED, and Stopped -> ABORTED.
Emulator: same code, message "The deployment <deploymentId> is already complete with status <ApiStatus>" — none of AWS's fixed text is present, the trailing period is missing, and it renders the API status in mixed case rather than the internal state, so a Stopped deployment reports "status Stopped" where AWS reports "current state ABORTED.". Replayed three times with three different terminal states: 'The deployment d-EOSULW2VG is already complete with status Stopped' (AWS: '... has already completed, current state ABORTED.'), 'The deployment d-FIQO8VWGD is already complete with status Failed' (AWS: '... current state FAILED.'), 'The deployment d-HKSCSES7V is already complete with status Succeeded' (AWS: '... current state SUCCEEDED.'). The ids are per-run values filling the template, not part of it.
Why: a client or deployment-automation matcher written against AWS's refusal text does not recognize the refusal, and the Stopped -> ABORTED mapping is information AWS exposes here and the emulator never does, so tooling that reads the internal state out of this message gets the wrong state.
Fix: SkipWaitTimeForInstanceTermination's terminal-state guard / error construction in the codedeploy service crate (the already-completed branch and its internal-state rendering).
Traces: codedeploy.skip-wait-time-already-completed-rejected, codedeploy.skip-wait-time-deployment-already-completed-rejected, codedeploy.skip-wait-time-triggers-immediate-termination.

Cause 2 of 2: UnsupportedActionForDeploymentTypeException message for an in-place deployment.
AWS: Smithy models UnsupportedActionForDeploymentTypeException on SkipWaitTimeForInstanceTermination (client, HTTP 400); no message text is modeled, so recorded behavior only. Recorded behavior: calling the operation on a deployment whose group is IN_PLACE (there is no blue/green instance-termination wait to skip) is refused 400 UnsupportedActionForDeploymentTypeException with the template "Operation is not supported for the deployment <deploymentId> of type <DEPLOYMENT_TYPE>." — fixed text "Operation is not supported for the deployment ", " of type " and the trailing period, with <deploymentId> from the request and <DEPLOYMENT_TYPE> the group's deploymentStyle.deploymentType from state (IN_PLACE here).
Emulator: same code, message "The deployment <deploymentId> is not a blue/green deployment, so it has no instance termination wait to skip" — entirely emulator-authored prose; it names neither "Operation is not supported" nor the deployment type token, and has no trailing period.
Why: this refusal is the only signal distinguishing "wrong deployment type" from "deployment does not exist" for a caller that matches on text; the diverging wording breaks that discrimination, and the deployment-type token AWS reports is absent so a caller cannot tell which type was rejected.
Fix: SkipWaitTimeForInstanceTermination's deployment-type guard / error construction in the codedeploy service crate.
Traces: codedeploy.skip-wait-time-nonexistent-deployment-fails.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against this checkout: 89/135 passed, bootstrap_signal=false (the codedeploy emulator is serving — 89/131 of its checks pass). Expected = AWS behavior asserted by the check; Actual = emulator response. All four checks are unowned by any open issue and none is marked ALREADY FILED.

Cause 1:
1. codedeploy.skip-wait-time-already-completed-rejected — StopDeployment drives a blue/green deployment to Stopped, then SkipWaitTimeForInstanceTermination(deploymentId=<that id>):
   expected ('DeploymentAlreadyCompletedException', 'The deployment d-EOSULW2VG has already completed, current state ABORTED.')
   got      ('DeploymentAlreadyCompletedException', 'The deployment d-EOSULW2VG is already complete with status Stopped')
2. codedeploy.skip-wait-time-deployment-already-completed-rejected — deployment reaches Failed, then the same call:
   expected ('DeploymentAlreadyCompletedException', 'The deployment d-FIQO8VWGD has already completed, current state FAILED.')
   got      ('DeploymentAlreadyCompletedException', 'The deployment d-FIQO8VWGD is already complete with status Failed')
3. codedeploy.skip-wait-time-triggers-immediate-termination — deployment reaches a terminal state (Succeeded in this run), then the same call:
   expected ('DeploymentAlreadyCompletedException', 'The deployment d-HKSCSES7V has already completed, current state SUCCEEDED.')
   got      ('DeploymentAlreadyCompletedException', 'The deployment d-HKSCSES7V is already complete with status Succeeded')
   (The d-... ids are minted per run; they are the template's variable part, not a requirement. The fixed text 'The deployment ' / ' has already completed, current state ' / '.' and the uppercase internal-state token are the requirement.)

Cause 2:
4. codedeploy.skip-wait-time-nonexistent-deployment-fails — the check first confirms the existence refusal for a fake id is precise ('DeploymentDoesNotExistException', 'The deployment <fake id> could not be found' — this leg passes), then calls the same operation on a REAL in-place deployment:
   expected ('UnsupportedActionForDeploymentTypeException', 'Operation is not supported for the deployment d-ROY1GJ890 of type IN_PLACE.')
   got      ('UnsupportedActionForDeploymentTypeException', 'The deployment d-ROY1GJ890 is not a blue/green deployment, so it has no instance termination wait to skip')

Smithy (services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json): SkipWaitTimeForInstanceTermination input {deploymentId: DeploymentId}, empty output, errors DeploymentAlreadyCompletedException / DeploymentDoesNotExistException / DeploymentIdRequiredException / DeploymentNotStartedException / InvalidDeploymentIdException / UnsupportedActionForDeploymentTypeException — every one errorKind=client, httpError 400. The model carries no message text for any of them (only the generic exception-message doc), so the templates above are AWS's recorded wording for those error classes.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite; both clusters reproduce.
2. Cause 1 (Stopped -> ABORTED): CreateApplication(computePlatform='Server'); CreateDeploymentGroup with deploymentStyle {deploymentType: BLUE_GREEN, deploymentOption: WITH_TRAFFIC_CONTROL} and tag filters matching nothing; CreateDeployment with an S3 revision; StopDeployment(deploymentId) and poll GetDeployment until status == 'Stopped'; then SkipWaitTimeForInstanceTermination(deploymentId=<that id>) and read Error.Code and Error.Message.
3. Cause 1 (Failed -> FAILED): same setup but let the deployment reach 'Failed' on its own (group matches no instances), then SkipWaitTimeForInstanceTermination(deploymentId=<that id>).
4. Cause 1 (Succeeded -> SUCCEEDED): on a Lambda-platform application and group, create a deployment that reaches a terminal state, then SkipWaitTimeForInstanceTermination(deploymentId=<that id>).
5. Cause 2: CreateApplication(computePlatform='Server'); CreateDeploymentGroup WITHOUT a deploymentStyle (defaults to IN_PLACE); CreateDeployment with an S3 revision; SkipWaitTimeForInstanceTermination(deploymentId=<that real id>) and read Error.Code and Error.Message.
6. In each case compare against the AWS templates: 'The deployment <deploymentId> has already completed, current state <SUCCEEDED|FAILED|ABORTED>.' and 'Operation is not supported for the deployment <deploymentId> of type IN_PLACE.'
