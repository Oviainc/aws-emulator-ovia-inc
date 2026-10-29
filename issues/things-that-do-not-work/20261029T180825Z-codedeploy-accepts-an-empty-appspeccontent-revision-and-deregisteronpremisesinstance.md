Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.create-deployment-rejects-malformed-appspec-content codedeploy.on-premises-instance-deregister-reflects-in-status-filters
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.create-deployment-rejects-malformed-appspec-content|_error#8|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: codedeploy.on-premises-instance-deregister-reflects-in-status-filters|_hold_absent#22|AssertionError|assert name not in listed, (
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# CodeDeploy accepts an empty AppSpecContent revision, and DeregisterOnPremisesInstance leaves the record listed as Deregistered (2 causes)

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Two independent state/validation defects outside the deployment state machine: one request AWS refuses synchronously is accepted with 200, and one delete leaves a record behind that AWS removes. Two causes.

Cause 1 of 2: CreateDeployment accepts an appSpecContent carrying neither content nor sha256.
AWS: Smithy CreateDeployment models InvalidRevisionException (HTTP 400) and takes revision: RevisionLocation (AppSpecContent with content/sha256). Recorded behavior: appspec CONTENT is validated asynchronously by the deployment (terminal Failed with errorInformation.code INVALID_LAMBDA_CONFIGURATION), but an appSpecContent with no body at all is the one revision error refused synchronously, with InvalidRevisionException and the message "Content and Sha256 value of the string can't both be empty" (fixed text, no request values).
Emulator: the call SUCCEEDS — HTTP 200 with a minted deploymentId (response body {'deploymentId': 'd-...'}) and a deployment recorded against the group.
Why: the boundary between 'not a revision at all' (refuse the request) and 'a revision whose content is wrong' (accept, then fail the deployment) is exactly what callers build on; an emulator that mints a deploymentId for an empty revision lets a client poll a deployment AWS would never have created, and it also records a deployment in a group the check expects to stay empty.
Fix: CreateDeployment's revision validation (appSpecContent content/sha256 presence gate, before the deployment is persisted) in the codedeploy service crate.
Traces: codedeploy.create-deployment-rejects-malformed-appspec-content.

Cause 2 of 2: DeregisterOnPremisesInstance tombstones the instance instead of deleting it, so it is still listed under registrationStatus='Deregistered'.
AWS: Smithy ListOnPremisesInstances takes registrationStatus: RegistrationStatus and returns instanceNames; recorded behavior: after DeregisterOnPremisesInstance the record is GONE — GetOnPremisesInstance raises InstanceNotRegisteredException and the name appears under NEITHER 'Registered' NOR 'Deregistered', and re-registering the same name mints a brand-new instanceArn with no tags.
Emulator: the deregistered instance name keeps being returned by ListOnPremisesInstances(registrationStatus='Deregistered') for the full 30s hold the check applies.
Why: callers reconcile their host inventory from ListOnPremisesInstances; a name that never leaves the listing makes a deregistered host look like it still exists in a (nonexistent in AWS) deregistered state, and re-registration logic keyed on absence never runs.
Fix: DeregisterOnPremisesInstance's state mutation and the ListOnPremisesInstances registrationStatus filter (delete the record rather than mark it) in the codedeploy service crate's on-premises instance state.
Traces: codedeploy.on-premises-instance-deregister-reflects-in-status-filters.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against this checkout (89/135 passed; emulator serving, bootstrap_signal=false).

1. codedeploy.create-deployment-rejects-malformed-appspec-content — CreateDeployment(applicationName=<Lambda app>, deploymentGroupName=<dg>, deploymentConfigName='CodeDeployDefault.LambdaAllAtOnce', revision={'revisionType':'AppSpecContent','appSpecContent':{'content':''}}): expected ClientError InvalidRevisionException with message "Content and Sha256 value of the string can't both be empty"; got HTTP 200 and {'deploymentId': 'd-1XCO9HSFR'} (AssertionError: "expected a ClientError, but the call SUCCEEDED and returned ..."). The deploymentId value is per-run.
2. codedeploy.on-premises-instance-deregister-reflects-in-status-filters — AssertionError: "DeregisterOnPremisesInstance deletes the record, so <instanceName> must not appear under registrationStatus='Deregistered'; it did, 30s-hold aborted with 1 names listed".

Smithy (services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json): CreateDeployment models InvalidRevisionException, RevisionRequiredException and RevisionDoesNotExistException (all HTTP 400); ListOnPremisesInstances takes registrationStatus (RegistrationStatus enum) and returns instanceNames; GetOnPremisesInstance models InstanceNotRegisteredException. The quoted messages are AWS's recorded wording for those error classes (the model documents no message text).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite; both clusters reproduce.
2. Cause 1: CreateApplication(computePlatform='Lambda'), CreateDeploymentGroup(deploymentConfigName='CodeDeployDefault.LambdaAllAtOnce'), then CreateDeployment with revision={'revisionType':'AppSpecContent','appSpecContent':{'content':''}} -> expect 400 InvalidRevisionException "Content and Sha256 value of the string can't both be empty" and ListDeployments for the group still empty; observe 200 with a deploymentId. (Control: a malformed-but-present appspec must still be ACCEPTED and then fail the deployment with errorInformation.code INVALID_LAMBDA_CONFIGURATION.)
3. Cause 2: RegisterOnPremisesInstance(instanceName=N, iamUserArn=...); AddTagsToOnPremisesInstances; DeregisterOnPremisesInstance(instanceName=N); then ListOnPremisesInstances(registrationStatus='Registered') and ListOnPremisesInstances(registrationStatus='Deregistered') -> N must appear in neither, and GetOnPremisesInstance(N) must raise InstanceNotRegisteredException.
