Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.get-onprem-instance-not-registered-rejected codedeploy.deregister-onprem-instance-missing-name-rejected codedeploy.batch-get-on-premises-instances-limit-exceeded-rejected codedeploy.create-deployment-revision-does-not-exist-rejected codedeploy.list-application-revisions-missing-bucket-filter-rejected codedeploy.skip-wait-time-nonexistent-deployment-fails codedeploy.skip-wait-time-triggers-immediate-termination codedeploy.add-tags-onprem-instance-not-registered-rejected codedeploy.delete-github-account-token-nonexistent-fails codedeploy.put-lifecycle-event-hook-status-deployment-not-exist-rejected codedeploy.register-onprem-instance-duplicate-name-blocked codedeploy.update-application-rename-to-existing-name-rejected
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.get-onprem-instance-not-registered-rejected|run#4|AssertionError|assert code == "InstanceNotRegisteredException", (
Issue-Key: codedeploy.deregister-onprem-instance-missing-name-rejected|run#3|AssertionError|assert code == "InstanceNameRequiredException", f"expected InstanceNameRequiredException, got {code!r}"
Issue-Key: codedeploy.batch-get-on-premises-instances-limit-exceeded-rejected|run#4|AssertionError|assert code == "BatchLimitExceededException", (
Issue-Key: codedeploy.create-deployment-revision-does-not-exist-rejected|run#9|AssertionError|assert code == "RevisionDoesNotExistException", f"unexpected error code: {code!r}"
Issue-Key: codedeploy.list-application-revisions-missing-bucket-filter-rejected|run#6|AssertionError|assert code == "BucketNameFilterRequiredException", f"got {code!r}"
Issue-Key: codedeploy.skip-wait-time-nonexistent-deployment-fails|run#13|AssertionError|assert code == "DeploymentDoesNotExistException", f"unexpected code: {code}"
Issue-Key: codedeploy.skip-wait-time-triggers-immediate-termination|run#14|AssertionError|assert code == "DeploymentAlreadyCompletedException", (
Issue-Key: codedeploy.add-tags-onprem-instance-not-registered-rejected|run#5|AssertionError|assert code == "InstanceNotRegisteredException", (
Issue-Key: codedeploy.delete-github-account-token-nonexistent-fails|run#3|AssertionError|assert code == "GitHubAccountTokenNameRequiredException", f"unexpected code: {code}"
Issue-Key: codedeploy.put-lifecycle-event-hook-status-deployment-not-exist-rejected|run#4|AssertionError|assert code == "DeploymentDoesNotExistException", (
Issue-Key: codedeploy.register-onprem-instance-duplicate-name-blocked|run#12|AssertionError|assert len(ok_results) == 1, f"expected exactly 1 winner, got {results}"
Issue-Key: codedeploy.update-application-rename-to-existing-name-rejected|run#12|AssertionError|assert code == "ApplicationAlreadyExistsException", code
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# CodeDeploy on-premises, application-revision and blue/green-wait operations answer HTTP 501 NotImplemented instead of their modeled behavior (6 causes)

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetOnPremisesInstance

## Description
Six CodeDeploy operations are still scaffold stubs that answer HTTP 501 with error code `NotImplemented` ("operation <X> is scaffolded but not implemented in this emulator"). Every one of them is reached by a behavioral check that expects a modeled 400 client error, so the emulator answers a server-side 501 where AWS answers a specific, modeled rejection. Six distinct root causes, one per operation.

Cause 1 of 6: GetOnPremisesInstance.
AWS: Smithy GetOnPremisesInstance (service com.amazonaws.codedeploy, model services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json) takes required `instanceName` and models errors InstanceNameRequiredException, InstanceNotRegisteredException, InvalidInstanceNameException, all HTTP 400. For a name that was never registered AWS answers 400 InstanceNotRegisteredException.
Emulator: answers 501 `NotImplemented`.
Why: a client that handles InstanceNotRegisteredException as "instance absent" instead sees an unretryable server fault, and any on-premises registration flow cannot distinguish absent from broken.
Fix: the operation's handler, services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/get_on_premises_instance.rs (the `Err(Scaffolded::not_implemented(..))` at the end of `handle`), plus the on-premises instance registry in the service state.
Traces: codedeploy.get-onprem-instance-not-registered-rejected.

Cause 2 of 6: DeregisterOnPremisesInstance.
AWS: Smithy DeregisterOnPremisesInstance models InstanceNameRequiredException and InvalidInstanceNameException (HTTP 400); a call with a missing/blank instanceName is refused with InstanceNameRequiredException.
Emulator: answers 501 `NotImplemented`.
Why: the required-member refusal is indistinguishable from an emulator fault, and deregistration is never performed.
Fix: services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/deregister_on_premises_instance.rs.
Traces: codedeploy.deregister-onprem-instance-missing-name-rejected.

Cause 3 of 6: BatchGetOnPremisesInstances.
AWS: Smithy BatchGetOnPremisesInstances takes required `instanceNames` and models BatchLimitExceededException (HTTP 400); a batch above the 100-name limit is refused with BatchLimitExceededException.
Emulator: answers 501 `NotImplemented` for a 101-name batch.
Why: a client cannot learn the batch limit and sees a server fault for a request AWS refuses as a client error.
Fix: services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/batch_get_on_premises_instances.rs, including the batch-size precondition.
Traces: codedeploy.batch-get-on-premises-instances-limit-exceeded-rejected.

Cause 4 of 6: GetApplicationRevision.
AWS: Smithy GetApplicationRevision takes required `applicationName` and `revision` and models RevisionDoesNotExistException (HTTP 400) among others; a revision that was never registered with the application is refused with RevisionDoesNotExistException (CodeDeploy tracks revisions by registration, it does not probe S3).
Emulator: answers 501 `NotImplemented`.
Why: revision bookkeeping cannot be observed at all, so a client cannot tell a never-registered revision from a service failure.
Fix: services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/get_application_revision.rs plus the per-application revision registry in state.
Traces: codedeploy.create-deployment-revision-does-not-exist-rejected.

Cause 5 of 6: ListApplicationRevisions.
AWS: Smithy ListApplicationRevisions (paginated, inputToken/outputToken `nextToken`, items `revisions`) models BucketNameFilterRequiredException (HTTP 400); supplying `s3KeyPrefix` without `s3Bucket` is refused with BucketNameFilterRequiredException.
Emulator: answers 501 `NotImplemented`.
Why: the cross-member filter precondition is unobservable and listing revisions is impossible.
Fix: services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/list_application_revisions.rs, including the bucket-filter precondition and pagination.
Traces: codedeploy.list-application-revisions-missing-bucket-filter-rejected.

Cause 6 of 6: SkipWaitTimeForInstanceTermination.
AWS: Smithy SkipWaitTimeForInstanceTermination models DeploymentAlreadyCompletedException, DeploymentDoesNotExistException, DeploymentIdRequiredException, DeploymentNotStartedException, InvalidDeploymentIdException and UnsupportedActionForDeploymentTypeException, all HTTP 400. A never-created deploymentId yields DeploymentDoesNotExistException; an in-place deployment yields UnsupportedActionForDeploymentTypeException; a terminal deployment yields DeploymentAlreadyCompletedException.
Emulator: answers 501 `NotImplemented` for all three.
Why: blue/green termination control is entirely unavailable and all three distinct rejection classes collapse into one server fault.
Fix: services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/skip_wait_time_for_instance_termination.rs (the scaffold marker and the `not_implemented` return are still in place).
Traces: codedeploy.skip-wait-time-nonexistent-deployment-fails, codedeploy.skip-wait-time-triggers-immediate-termination.

Replay-also (NOT additional causes, no extra work implied): five further failing checks hit scaffold stubs that each already have their own filed per-operation issue — AddTagsToOnPremisesInstances, DeleteGitHubAccountToken, PutLifecycleEventHookExecutionStatus, RegisterOnPremisesInstance and UpdateApplication. They are listed in trace_ids only so they get re-replayed once those stub issues land: codedeploy.add-tags-onprem-instance-not-registered-rejected (expects InstanceNotRegisteredException, got NotImplemented), codedeploy.delete-github-account-token-nonexistent-fails (expects GitHubAccountTokenNameRequiredException), codedeploy.put-lifecycle-event-hook-status-deployment-not-exist-rejected (expects DeploymentDoesNotExistException), codedeploy.register-onprem-instance-duplicate-name-blocked (expects exactly one winner of two concurrent registrations), codedeploy.update-application-rename-to-existing-name-rejected (expects ApplicationAlreadyExistsException).

## Evidence
Replayed behavioral suite, codedeploy service (2/137 passed).

Expected (AWS, per the Smithy model cited per cause): HTTP 400 with the operation's modeled error code.
Actual (emulator): HTTP 501, Code `NotImplemented`, Message "operation <Operation> is scaffolded but not implemented in this emulator" (message quoted as an example of the stub channel, not as a requirement).

Failing assertions, verbatim from the replay:
- codedeploy.get-onprem-instance-not-registered-rejected: "GetOnPremisesInstance for a never-registered name raised 'NotImplemented', expected 'InstanceNotRegisteredException'"
- codedeploy.deregister-onprem-instance-missing-name-rejected: "expected InstanceNameRequiredException, got 'NotImplemented'"
- codedeploy.batch-get-on-premises-instances-limit-exceeded-rejected: "expected BatchLimitExceededException for a 101-name batch (limit is 100), got 'NotImplemented'"
- codedeploy.create-deployment-revision-does-not-exist-rejected: assert code == "RevisionDoesNotExistException" -> "unexpected error code: 'NotImplemented'" (the call under test is GetApplicationRevision on an unregistered revision)
- codedeploy.list-application-revisions-missing-bucket-filter-rejected: assert code == "BucketNameFilterRequiredException" -> "got 'NotImplemented'"
- codedeploy.skip-wait-time-nonexistent-deployment-fails: assert code == "DeploymentDoesNotExistException" -> "unexpected code: NotImplemented"
- codedeploy.skip-wait-time-triggers-immediate-termination: "expected DeploymentAlreadyCompletedException for a terminal deployment, got NotImplemented"

Source evidence: services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/get_on_premises_instance.rs:4 and :157 and .../skip_wait_time_for_instance_termination.rs:4 and :180 both still carry the `SCAFFOLDED, NOT IMPLEMENTED` marker and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))`.

## Steps to Reproduce
1. Boot the emulator and point a botocore codedeploy client at it.
2. GetOnPremisesInstance(instanceName="never-registered-name") -> expect 400 InstanceNotRegisteredException, observe 501 NotImplemented.
3. DeregisterOnPremisesInstance with a blank instanceName (parameter_validation=False client) -> expect 400 InstanceNameRequiredException.
4. BatchGetOnPremisesInstances(instanceNames=[101 names]) -> expect 400 BatchLimitExceededException.
5. CreateApplication, then GetApplicationRevision for an S3 revision never registered -> expect 400 RevisionDoesNotExistException.
6. ListApplicationRevisions(applicationName=app, s3KeyPrefix="p") with no s3Bucket -> expect 400 BucketNameFilterRequiredException.
7. SkipWaitTimeForInstanceTermination(deploymentId="d-ABCDEFGHI") -> expect 400 DeploymentDoesNotExistException; call it on a real in-place deployment -> expect UnsupportedActionForDeploymentTypeException; on a terminal deployment -> expect DeploymentAlreadyCompletedException.
