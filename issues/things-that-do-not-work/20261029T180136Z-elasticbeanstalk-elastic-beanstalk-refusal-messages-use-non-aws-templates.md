Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticbeanstalk.apply-environment-managed-action-missing-environment-name-rejected elasticbeanstalk.describe-instances-health-unknown-environment-must-fail elasticbeanstalk.associate-operations-role-on-terminated-environment-rejected elasticbeanstalk.environment-update-while-launching-is-rejected elasticbeanstalk.list-tags-for-resource-unsupported-type elasticbeanstalk.create-application-version-lifecycle elasticbeanstalk.compose-environments-mismatched-application-must-fail elasticbeanstalk.describe-instances-health-invalid-attribute-rejected elasticbeanstalk.describe-instances-health-malformed-request-rejected
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk.apply-environment-managed-action-missing-environment-name-rejected|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: elasticbeanstalk.describe-instances-health-unknown-environment-must-fail|run#22|AssertionError|assert message2 == expected2, f"expected message {expected2!r}, got {message2!r}"
Issue-Key: elasticbeanstalk.associate-operations-role-on-terminated-environment-rejected|run#29|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: elasticbeanstalk.environment-update-while-launching-is-rejected|run#13|AssertionError|assert message == expected, (
Issue-Key: elasticbeanstalk.list-tags-for-resource-unsupported-type|run#13|AssertionError|assert message == expected_message, (
Issue-Key: elasticbeanstalk.create-application-version-lifecycle|_assert_invalid_parameter#18|AssertionError|assert err.get("Message") == expected_message, f'expected the message {expected_message!r}, got {err.get("Message")!r}'
Issue-Key: elasticbeanstalk.compose-environments-mismatched-application-must-fail|run#16|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: elasticbeanstalk.describe-instances-health-invalid-attribute-rejected|run#10|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Key: elasticbeanstalk.describe-instances-health-malformed-request-rejected|run#7|AssertionError|assert message0 == ENUM_MESSAGE, f"expected message {ENUM_MESSAGE!r}, got {message0!r}"
Issue-Commit: 9ba218dfac78ec8aaaf32d0b8457457c6bd04b91

# Elastic Beanstalk refusal messages use non-AWS templates on seven rejection paths (7 causes)

> _Note: this issue was identified against the code base at commit `9ba218dfac78ec8aaaf32d0b8457457c6bd04b91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateEnvironment

## Description
SEVEN distinct root causes in the elasticbeanstalk emulator, all in the same service, all grounded in failing behavioral checks of this suite. In every one the HTTP status and the Error.Code already match AWS; only the Error.Message template diverges, and in each case the message is BUILT FROM THE REQUEST and the service's state (the environment name, the version label, the application name, the ARN, the modelled enum value set), so the TEMPLATE — not any one recorded string — is the requirement. The recorded strings below are that template applied to one run's generated names. Adjacent open/fixed issues do NOT cover these checks: 20261023T082928Z-elasticbeanstalk-elastic-beanstalk-not-found-errors-use-non-aws.md (fixed) corrected the TerminateEnvironment / SwapEnvironmentCNAMEs / DescribePlatformVersion not-found templates only, and 20261023T205650Z-elasticbeanstalk-elastic-beanstalk-control-plane-guards-are-not-enforced.md owns a different set of checks. SEVEN causes must be closed before submitting.

Cause 1 of 7: the "neither EnvironmentName nor EnvironmentId given" refusal is worded the emulator's way.
AWS: Smithy: ApplyEnvironmentManagedAction input (EnvironmentName, EnvironmentId, ActionId required) and DescribeInstancesHealth input (EnvironmentName, EnvironmentId) each make the two environment selectors optional in the model, so the "exactly one of" rule is a service-side validation; recorded behavior: InvalidParameterValue / HTTP 400 with the fixed text 'You must specify either EnvironmentName or EnvironmentId.' (no variable part).
Emulator: same code and status, Error.Message 'Either EnvironmentName or EnvironmentId must be specified.'
Why: a client (or an operator transcript diff) that matches AWS's wording to tell a missing-selector rejection from any other 400 does not match.
Fix: the shared environment-selector validation the elasticbeanstalk handlers call (services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/, reached from apply_environment_managed_action.rs and describe_instances_health.rs).
Traces: elasticbeanstalk.apply-environment-managed-action-missing-environment-name-rejected, elasticbeanstalk.describe-instances-health-unknown-environment-must-fail.

Cause 2 of 7: AssociateEnvironmentOperationsRole against a Terminated environment reports a state error instead of not-found.
AWS: Smithy: AssociateEnvironmentOperationsRole(EnvironmentName required, OperationsRole required) models only InsufficientPrivilegesException (403); recorded behavior: a TERMINATED environment is treated as absent — InvalidParameterValue / HTTP 400 with the template "No Environment found for EnvironmentName = '<EnvironmentName>'." where <EnvironmentName> is the requested name (fixed text otherwise).
Emulator: InvalidParameterValue but Error.Message 'Environment <name> is not in a state from which an operations role can be associated. Current status: Terminated.' — it keeps the terminated record visible to this guard and emits a state-machine message AWS never emits.
Why: a client cannot distinguish "this environment is gone" from "this environment is busy" and so retries a call that can never succeed; the refusal also leaks a status AWS does not report here.
Fix: the terminated-environment lookup in the AssociateEnvironmentOperationsRole handler (src/ops/associate_environment_operations_role.rs) — a Terminated environment must resolve as not-found, not as a bad-state.
Traces: elasticbeanstalk.associate-operations-role-on-terminated-environment-rejected.

Cause 3 of 7: UpdateEnvironment's bad-state refusal is worded the emulator's way.
AWS: Smithy: UpdateEnvironment -> EnvironmentDescription, errors InsufficientPrivilegesException (403) / TooManyBucketsException (400); recorded behavior: updating an environment that is not Ready gives InvalidParameterValue / HTTP 400 with the template 'Environment named <EnvironmentName> is in an invalid state for this operation. Must be Ready.' — fixed text around the requested name, and notably AWS does NOT name the current status.
Emulator: InvalidParameterValue with 'Environment <name> is not in a state from which it can be updated. Current status: Launching.'
Why: the operator-visible text differs from every AWS SDK/CLI transcript, and a client keying on AWS's 'Must be Ready.' phrase to decide to wait does not recognise the refusal.
Fix: the state guard's error construction in the UpdateEnvironment handler (src/ops/update_environment.rs).
Traces: elasticbeanstalk.environment-update-while-launching-is-rejected.

Cause 4 of 7: ListTagsForResource's unsupported-resource-type message is worded the emulator's way.
AWS: Smithy: ListTagsForResource(ResourceArn required) models ResourceTypeNotSupportedException — "The type of the specified Amazon Resource Name (ARN) isn't supported for this operation" — httpError 400, awsQueryError code ResourceTypeNotSupportedException; recorded behavior: Error.Message template "Tags aren't supported for the resource type of the ARN '<ResourceArn>'." with the requested ARN substituted.
Emulator: same code/status, Error.Message "The type of the resource ARN '<ResourceArn>' is not supported for tagging."
Why: tagging clients that surface or match AWS's wording diverge.
Fix: the unsupported-type error construction in the ListTagsForResource handler (src/ops/list_tags_for_resource.rs).
Traces: elasticbeanstalk.list-tags-for-resource-unsupported-type.

Cause 5 of 7: UpdateApplicationVersion on a deleted/unknown version label uses the generic not-found template instead of the application-scoped one.
AWS: Smithy: UpdateApplicationVersion(ApplicationName required, VersionLabel required) models no operation-specific error; the API reference documents InvalidParameterValue for an unknown application or version label. Recorded behavior: InvalidParameterValue / HTTP 4xx with the template "Application version with label '<VersionLabel>' not found in application '<ApplicationName>'" — note it names BOTH the label and the application, and carries NO trailing period.
Emulator: InvalidParameterValue with "No Application Version named '<VersionLabel>' found." — the application component is dropped and a period added.
Why: a client that parses the application name out of the refusal, or matches the documented wording, mis-handles a stale version label; the same label can exist in another application, so the emulator's text is also ambiguous.
Fix: the version-lookup error construction in the UpdateApplicationVersion handler (src/ops/update_application_version.rs) / the crate's shared version-not-found builder.
Traces: elasticbeanstalk.create-application-version-lifecycle.

Cause 6 of 7: ComposeEnvironments' rejection for a version that carries no usable manifest reports an emulator-specific diagnosis.
AWS: Smithy: ComposeEnvironments(ApplicationName, GroupName, VersionLabels) -> EnvironmentDescriptionsMessage, errors InsufficientPrivilegesException (403) / TooManyEnvironmentsException (400); recorded behavior: when the named version cannot be composed for the requested application the call fails with the fixed text 'No valid application version found.' (no variable part at all).
Emulator: a bespoke sentence naming env.yaml and the version label: "The environment manifest (env.yaml) naming the environment to compose is not available from the source bundle of version label '<VersionLabel>'."
Why: callers matching AWS's fixed message to detect the compose-input error see an unknown message; the emulator also discloses a manifest-level diagnosis AWS does not give, so a client written against it breaks on real AWS.
Fix: the input-validation error construction in the ComposeEnvironments handler (src/ops/compose_environments.rs).
Traces: elasticbeanstalk.compose-environments-mismatched-application-must-fail.

Cause 7 of 7: DescribeInstancesHealth's enum-constraint validation message lists the allowed AttributeNames in the wrong order.
AWS: Smithy: DescribeInstancesHealth.AttributeNames targets InstancesHealthAttributes whose member enum InstancesHealthAttribute is declared in the model as [HealthStatus, Color, Causes, ApplicationMetrics, RefreshedAt, LaunchedAt, System, Deployment, AvailabilityZone, InstanceType, All]; recorded behavior: the refusal is the standard framework validation message "1 validation error detected: Value '[<bad value>]' at 'attributeNames' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [All, Color, Deployment, LaunchedAt, AvailabilityZone, Causes, RefreshedAt, HealthStatus, InstanceType, System, ApplicationMetrics]]" — the fixed part is the whole sentence INCLUDING that value-set sequence (AWS's own order, which differs from the model's declaration order); the only variable part is the rejected value echoed at 'attributeNames'.
Emulator: the same sentence but with the value set rendered in the model's declaration order ([HealthStatus, Color, Causes, ApplicationMetrics, RefreshedAt, LaunchedAt, System, Deployment, AvailabilityZone, InstanceType, All]).
Why: the message is a fixed string for this error class, so any client or test asserting on AWS's validation text fails; it also tells operators a different "valid values" ordering than AWS does.
Fix: the enum-constraint validation message rendering for AttributeNames in the DescribeInstancesHealth handler / the crate's shared enum-validation formatter (src/ops/describe_instances_health.rs), which must emit AWS's fixed sequence rather than iterate the generated enum.
Traces: elasticbeanstalk.describe-instances-health-invalid-attribute-rejected, elasticbeanstalk.describe-instances-health-malformed-request-rejected.

## Evidence
Cause 1: expected Error.Message 'You must specify either EnvironmentName or EnvironmentId.', got 'Either EnvironmentName or EnvironmentId must be specified.' (AssertionError in both apply-environment-managed-action-missing-environment-name-rejected and describe-instances-health-unknown-environment-must-fail; code InvalidParameterValue and HTTP 400 matched).
Cause 2: expected "No Environment found for EnvironmentName = 'sim2real-eb6-env-99871136f704'.", got 'Environment sim2real-eb6-env-99871136f704 is not in a state from which an operations role can be associated. Current status: Terminated.' (the name is this run's generated value filling the template).
Cause 3: expected 'Environment named sim2real-behav-eb-env-273548c8df5d is in an invalid state for this operation. Must be Ready.', got 'Environment sim2real-behav-eb-env-273548c8df5d is not in a state from which it can be updated. Current status: Launching.'
Cause 4: expected "Tags aren't supported for the resource type of the ARN 'arn:aws:elasticbeanstalk:us-west-2::solutionstack/not-a-real-stack'.", got "The type of the resource ARN 'arn:aws:elasticbeanstalk:us-west-2::solutionstack/not-a-real-stack' is not supported for tagging."
Cause 5: expected "Application version with label 'sim2real-behav-v-bf124aaa8aa9' not found in application 'sim2real-behav-eb-app-a3e4422dfeaa'", got "No Application Version named 'sim2real-behav-v-bf124aaa8aa9' found."
Cause 6: expected 'No valid application version found.', got "The environment manifest (env.yaml) naming the environment to compose is not available from the source bundle of version label 'v-b04a5b136027'."
Cause 7: expected "1 validation error detected: Value '[NotARealAttribute]' at 'attributeNames' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [All, Color, Deployment, LaunchedAt, AvailabilityZone, Causes, RefreshedAt, HealthStatus, InstanceType, System, ApplicationMetrics]]", got the same sentence with the set rendered as [HealthStatus, Color, Causes, ApplicationMetrics, RefreshedAt, LaunchedAt, System, Deployment, AvailabilityZone, InstanceType, All].

## Steps to Reproduce
1. Boot the emulator and point an AWS client at it (region us-west-2).
2. Cause 1: eb.apply_environment_managed_action(ActionId='x') with no EnvironmentName/EnvironmentId, and eb.describe_instances_health() with neither selector -> compare Error.Message.
3. Cause 2: create an application + environment, wait for Ready, eb.terminate_environment, wait for Terminated, then eb.associate_environment_operations_role(EnvironmentName=<that name>, OperationsRole=<any role arn>) -> compare Error.Message.
4. Cause 3: create an environment and, while it is still Launching, call eb.update_environment(EnvironmentName=..., OptionSettings=[...]) -> compare Error.Message.
5. Cause 4: eb.list_tags_for_resource(ResourceArn='arn:aws:elasticbeanstalk:us-west-2::solutionstack/not-a-real-stack') -> compare Error.Message.
6. Cause 5: create an application version, delete it, then eb.update_application_version(ApplicationName, VersionLabel, Description='x') -> compare Error.Message.
7. Cause 6: create a version whose bundle carries no usable env.yaml for the requested application, call eb.compose_environments(ApplicationName, VersionLabels=[label]) -> compare Error.Message.
8. Cause 7: eb.describe_instances_health(EnvironmentName=<any>, AttributeNames=['NotARealAttribute']) -> compare the enum value set order inside the validation message.
9. Re-replay the checks listed in trace_ids.
