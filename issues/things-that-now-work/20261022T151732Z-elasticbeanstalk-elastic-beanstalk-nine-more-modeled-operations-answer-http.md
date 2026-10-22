Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticbeanstalk.apply-environment-managed-action-missing-environment-name-rejected elasticbeanstalk.associate-environment-operations-role-unknown-environment-rejected elasticbeanstalk.compose-environments-insufficient-privileges-blocked elasticbeanstalk.compose-environments-mismatched-application-must-fail elasticbeanstalk.describe-environment-health-unknown-env-fails elasticbeanstalk.describe-instances-health-invalid-attribute-rejected elasticbeanstalk.rebuild-environment-unknown-environment-rejected elasticbeanstalk.restart-app-server-unknown-env-must-fail elasticbeanstalk.update-application-resource-lifecycle-persists elasticbeanstalk.foreign-resource-arn-for-tagging-rejected elasticbeanstalk.update-tags-exceeds-quota-rejected
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk.apply-environment-managed-action-missing-environment-name-rejected|run#4|AssertionError|assert code == "MissingParameter", (
Issue-Key: elasticbeanstalk.associate-environment-operations-role-unknown-environment-rejected|run#10|AssertionError|assert code in ("InvalidParameterValue", "ResourceNotFoundException"), (
Issue-Key: elasticbeanstalk.compose-environments-insufficient-privileges-blocked|run#9|AssertionError|assert code == "AccessDenied", f"expected AccessDenied, got {code!r}"
Issue-Key: elasticbeanstalk.compose-environments-mismatched-application-must-fail|run#14|AssertionError|assert code == "MissingParameter", f"unexpected error code: {code}"
Issue-Key: elasticbeanstalk.describe-environment-health-unknown-env-fails|run#5|AssertionError|assert code == "InvalidParameterValue", (
Issue-Key: elasticbeanstalk.describe-instances-health-invalid-attribute-rejected|run#4|AssertionError|assert control_code == "InvalidParameterValue", (
Issue-Key: elasticbeanstalk.rebuild-environment-unknown-environment-rejected|run#4|AssertionError|assert code == "InvalidParameterValue", (
Issue-Key: elasticbeanstalk.restart-app-server-unknown-env-must-fail|run#4|AssertionError|assert code == "InvalidParameterValue", (
Issue-Key: elasticbeanstalk.update-application-resource-lifecycle-persists|run#7|AssertionError|assert code == "InvalidParameterValue", (
Issue-Key: elasticbeanstalk.foreign-resource-arn-for-tagging-rejected|run#6|AssertionError|assert write_code == "InvalidParameterValue", (
Issue-Key: elasticbeanstalk.update-tags-exceeds-quota-rejected|run#7|AssertionError|assert code == "TooManyTagsException", f"expected TooManyTagsException, got {code!r}"
Issue-Commit: 659e25b97046a77449047daf7fe310a5a83846e1

# Elastic Beanstalk: nine more modeled operations answer HTTP 501 NotImplemented instead of their modeled success or rejection (9 causes)

> _Note: this issue was identified against the code base at commit `659e25b97046a77449047daf7fe310a5a83846e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RestartAppServer

## Description
Nine Elastic Beanstalk operations are still scaffold stubs whose handlers return Scaffolded::not_implemented, so every request — valid or invalid — answers HTTP 501 with error code NotImplemented and the message "operation <Op> is scaffolded but not implemented in this emulator". None of these nine is marked ALREADY FILED or OWNED in the replay report. Each cause below is one operation and needs its own handler implemented in its own file under services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/; the shared symptom (501) is the per-operation stub, not one central bug.

Cause 1 of 9: ApplyEnvironmentManagedAction.
AWS: Smithy models ApplyEnvironmentManagedActionRequest {EnvironmentName, EnvironmentId, ActionId required} -> ApplyEnvironmentManagedActionResult {ActionId, ActionDescription, ActionType, Status}, errors ElasticBeanstalkServiceException (400) and ManagedActionInvalidStateException (400). Recorded behavior only: a request carrying ActionId but neither EnvironmentName nor EnvironmentId is rejected with error code MissingParameter, since exactly one environment identifier is required by the service even though the model marks neither member required.
Emulator: 501 NotImplemented for that request.
Why: a client cannot apply a managed action, and the "which identifier is required" refusal is indistinguishable from "operation missing".
Fix: src/ops/apply_environment_managed_action.rs handler + its request validation.
Traces: elasticbeanstalk.apply-environment-managed-action-missing-environment-name-rejected.

Cause 2 of 9: AssociateEnvironmentOperationsRole.
AWS: Smithy input AssociateEnvironmentOperationsRoleMessage {EnvironmentName required len 4..40, OperationsRole required len 1..256}, empty output, modeled error InsufficientPrivilegesException (403). Recorded behavior only: against an environment that does not exist the call is rejected with InvalidParameterValue.
Emulator: 501 NotImplemented.
Why: operations-role association is unusable and the not-found refusal is unobservable.
Fix: src/ops/associate_environment_operations_role.rs handler + environment-identity lookup in state.
Traces: elasticbeanstalk.associate-environment-operations-role-unknown-environment-rejected.

Cause 3 of 9: ComposeEnvironments.
AWS: Smithy input ComposeEnvironmentsMessage {ApplicationName len 1..100, GroupName len 1..19, VersionLabels}, output EnvironmentDescriptionsMessage, modeled errors InsufficientPrivilegesException (403) and TooManyEnvironmentsException (400). Recorded behavior only: a caller without the required privileges is refused with AccessDenied, and version labels belonging to a different application are refused with a distinct client error rather than composing environments.
Emulator: 501 NotImplemented in both cases.
Why: group/multi-tier composition cannot be used, and an authorization denial is masked as an unimplemented operation.
Fix: src/ops/compose_environments.rs handler, its authorization gate and its cross-application version-label validation.
Traces: elasticbeanstalk.compose-environments-insufficient-privileges-blocked, elasticbeanstalk.compose-environments-mismatched-application-must-fail.

Cause 4 of 9: DescribeEnvironmentHealth.
AWS: Smithy input DescribeEnvironmentHealthRequest {EnvironmentName len 4..40, EnvironmentId, AttributeNames}, output DescribeEnvironmentHealthResult, modeled errors ElasticBeanstalkServiceException (400) and InvalidRequestException (400). Recorded behavior only: a well-formed but never-created EnvironmentName is rejected with error code InvalidParameterValue.
Emulator: 501 NotImplemented.
Why: enhanced-health reads are impossible and not-found is unobservable.
Fix: src/ops/describe_environment_health.rs handler.
Traces: elasticbeanstalk.describe-environment-health-unknown-env-fails.

Cause 5 of 9: DescribeInstancesHealth.
AWS: Smithy input DescribeInstancesHealthRequest {EnvironmentName len 4..40, EnvironmentId, AttributeNames: InstancesHealthAttributes, NextToken len 1..100}, output DescribeInstancesHealthResult {InstanceHealthList, RefreshedAt, NextToken}, modeled errors ElasticBeanstalkServiceException (400) and InvalidRequestException (400). Recorded behavior only: a modeled AttributeNames value against a nonexistent environment is rejected with InvalidParameterValue.
Emulator: 501 NotImplemented.
Why: per-instance health is unavailable; see the companion issue for the separate enum-refusal error-code divergence on the same operation.
Fix: src/ops/describe_instances_health.rs handler (the file exists and still carries the SCAFFOLDED marker).
Traces: elasticbeanstalk.describe-instances-health-invalid-attribute-rejected.

Cause 6 of 9: RebuildEnvironment.
AWS: Smithy input RebuildEnvironmentMessage {EnvironmentId, EnvironmentName len 4..40}, empty output, modeled error InsufficientPrivilegesException (403). Recorded behavior only: an unknown environment is rejected with InvalidParameterValue.
Emulator: 501 NotImplemented.
Why: environments cannot be rebuilt and the not-found refusal is unobservable.
Fix: src/ops/rebuild_environment.rs handler.
Traces: elasticbeanstalk.rebuild-environment-unknown-environment-rejected.

Cause 7 of 9: RestartAppServer.
AWS: Smithy input RestartAppServerMessage {EnvironmentId, EnvironmentName len 4..40}, empty output, models no errors. Recorded behavior only: an unknown environment is rejected with InvalidParameterValue.
Emulator: 501 NotImplemented.
Why: the app-server restart control is unusable.
Fix: src/ops/restart_app_server.rs handler (file exists, SCAFFOLDED marker present).
Traces: elasticbeanstalk.restart-app-server-unknown-env-must-fail.

Cause 8 of 9: UpdateApplicationResourceLifecycle.
AWS: Smithy input UpdateApplicationResourceLifecycleMessage {ApplicationName required len 1..100, ResourceLifecycleConfig required}, output ApplicationResourceLifecycleDescriptionMessage {ApplicationName, ResourceLifecycleConfig}, modeled error InsufficientPrivilegesException (403). Recorded behavior only: a ResourceLifecycleConfig that enables version lifecycle without a ServiceRole is rejected with InvalidParameterValue; an accepted config persists and is echoed back.
Emulator: 501 NotImplemented.
Why: application version-lifecycle policy cannot be configured, and the ServiceRole requirement is not enforced.
Fix: src/ops/update_application_resource_lifecycle.rs handler, its ServiceRole validation and application state.
Traces: elasticbeanstalk.update-application-resource-lifecycle-persists.

Cause 9 of 9: UpdateTagsForResource rejection paths.
AWS: Smithy input UpdateTagsForResourceMessage {ResourceArn required, TagsToAdd, TagsToRemove}, empty output, modeled errors InsufficientPrivilegesException (403), OperationInProgressException (400, awsQueryError OperationInProgressFailure), ResourceNotFoundException (400), ResourceTypeNotSupportedException (400), TooManyTagsException (400). Recorded behavior only: an ARN for a resource outside Elastic Beanstalk is rejected with InvalidParameterValue, and exceeding the per-resource tag quota is rejected with TooManyTagsException.
Emulator: 501 NotImplemented for both.
Why: tag mutation is unusable and neither the foreign-ARN nor the quota refusal is reachable. NOTE: the replay report separately filed a scaffold-stub issue for UpdateTagsForResource from the check elasticbeanstalk.update-tags-for-resource-add-remove; these two checks were NOT marked ALREADY FILED, and they add the ARN-validation and tag-quota requirements that a bare "implement the stub" item does not state. Close them together with that issue.
Fix: src/ops/update_tags_for_resource.rs handler, its ResourceArn validation and tag-count limit.
Traces: elasticbeanstalk.foreign-resource-arn-for-tagging-rejected, elasticbeanstalk.update-tags-exceeds-quota-rejected.

## Evidence
All nine causes produce HTTP 501 with code NotImplemented. Replay output, verbatim tails:
- AssertionError: expected MissingParameter when ActionId is given without EnvironmentName/EnvironmentId, got NotImplemented
- AssertionError: unexpected error code for a nonexistent environment: NotImplemented (AssociateEnvironmentOperationsRole)
- AssertionError: expected AccessDenied, got 'NotImplemented' (ComposeEnvironments)
- AssertionError: unexpected error code: NotImplemented (ComposeEnvironments, mismatched application)
- AssertionError: DescribeEnvironmentHealth on a well-formed but never-created environment name must fail with InvalidParameterValue, got 'NotImplemented'
- AssertionError: positive control (modeled attribute, fake env): expected InvalidParameterValue, got NotImplemented (DescribeInstancesHealth)
- AssertionError: unexpected error code for a nonexistent environment: NotImplemented (RebuildEnvironment)
- AssertionError: RestartAppServer on an unknown environment must fail with InvalidParameterValue, got 'NotImplemented'
- AssertionError: a lifecycle config without ServiceRole must be rejected with InvalidParameterValue, got 'NotImplemented' (UpdateApplicationResourceLifecycle)
- AssertionError: expected InvalidParameterValue for UpdateTagsForResource on a foreign ARN, got 'NotImplemented'
- AssertionError: expected TooManyTagsException, got 'NotImplemented'
Each message body is of the form "operation <Op> is scaffolded but not implemented in this emulator", matching Scaffolded::not_implemented in the corresponding src/ops/<op>.rs.

## Steps to Reproduce
1. Boot the emulator; point botocore at it.
2. For each operation, issue the request named in its cause and read the error code: e.g. eb.restart_app_server(EnvironmentName='sim2real-ghost-env') -> expect InvalidParameterValue, observe NotImplemented (501); eb.apply_environment_managed_action(ActionId='x') -> expect MissingParameter, observe NotImplemented; eb.update_tags_for_resource(ResourceArn='arn:aws:sqs:us-east-1:123456789012:q', TagsToAdd=[{'Key':'k','Value':'v'}]) -> expect InvalidParameterValue, observe NotImplemented.
3. Re-replay the eleven check ids in trace_ids.
