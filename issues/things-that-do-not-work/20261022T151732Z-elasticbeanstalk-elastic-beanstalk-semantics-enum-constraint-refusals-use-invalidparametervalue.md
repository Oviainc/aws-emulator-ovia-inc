Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticbeanstalk.describe-instances-health-malformed-request-rejected elasticbeanstalk.delete-application-idempotent-unknown elasticbeanstalk.deploy-cloudformation-invalid-platform-arn-rejected
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk.describe-instances-health-malformed-request-rejected|run#6|AssertionError|assert code0 == "ValidationError", (
Issue-Key: elastic-beanstalk|DeleteApplication|InvalidParameterValue
Issue-Key: elasticbeanstalk.deploy-cloudformation-invalid-platform-arn-rejected|run#9|AssertionError|assert bad_status == "CREATE_FAILED", f"expected the bogus-platform stack to end CREATE_FAILED, got {bad_status!r}"
Issue-Commit: 659e25b97046a77449047daf7fe310a5a83846e1

# Elastic Beanstalk semantics: enum-constraint refusals use InvalidParameterValue instead of ValidationError, DeleteApplication on an unknown application is an error instead of a no-op, and a CloudFormation environment with a bogus PlatformArn reports CREATE_COMPLETE (3 causes)

> _Note: this issue was identified against the code base at commit `659e25b97046a77449047daf7fe310a5a83846e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstancesHealth

## Description
Three independent semantic divergences on paths the emulator DOES serve (no 501, no InvalidAction involved).

Cause 1 of 3: a value outside a modeled enum is refused with the wrong error code and a non-AWS message construction.
AWS: DescribeInstancesHealth$AttributeNames targets InstancesHealthAttributes, whose member enum is the set [HealthStatus, Color, Causes, ApplicationMetrics, RefreshedAt, LaunchedAt, System, Deployment, AvailabilityZone, InstanceType, All] (Smithy: DescribeInstancesHealthRequest$AttributeNames -> InstancesHealthAttributes). A value outside that set is refused with HTTP 400 and error code ValidationError — the framework-level request-validation code, raised BEFORE environment identity is resolved (the check confirms this by combining the bad enum value with a never-created EnvironmentName and still expecting ValidationError, not the not-found InvalidParameterValue). The message is built from the request by AWS's standard validation construction, so its shape is a requirement: "<N> validation error detected: Value '[<bad value>]' at '<memberPath>' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [<enum values>]]".
Emulator: HTTP 400 with code InvalidParameterValue and the message "Value 'NotARealAttribute' at 'attributeNames.1.member' failed to satisfy constraint: Member must satisfy enum value set: [...]" — right family of prose, wrong error code, and the construction is per-violation rather than AWS's "N validation error detected: ... at 'attributeNames' ... Member must satisfy constraint: [...]" envelope.
Why: a client that branches on error code treats a malformed request as a bad parameter VALUE on an existing resource (commonly retried or surfaced as a user-data problem) instead of a request-shape ValidationError; SDK waiters and retry policies classify the two differently.
Fix: services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/service.rs, Service::override_refusal — it maps every Refusal::Invalid violation that is not MissingRequiredMember to crate::scaffolded::INVALID_PARAMETER_VALUE, so constraint/enum violations lose the ValidationError code the framework default would have used. The mapping needs to distinguish a value-constraint violation (enum/length/pattern -> ValidationError) from the documented InvalidParameterValue cases, and build the message in AWS's "N validation error detected" form.
Traces: elasticbeanstalk.describe-instances-health-malformed-request-rejected.

Cause 2 of 3: DeleteApplication on an application that never existed errors instead of being a no-op.
AWS: Smithy DeleteApplication takes DeleteApplicationMessage {ApplicationName required len 1..100, TerminateEnvByForce} and returns an EMPTY output; the ONLY modeled error is OperationInProgressException (400, awsQueryError OperationInProgressFailure) — the model declares no not-found error for this operation. Recorded behavior: deleting a name that never existed succeeds (HTTP 200, empty body), does not create the application as a side effect, and a repeated delete of the same unknown name succeeds again.
Emulator: HTTP 400, code InvalidParameterValue, message "No Application named '<name>' found." (message quoted as an example).
Why: teardown and idempotent cleanup paths break — a client deleting an already-deleted application gets a hard failure, so retried or re-run infrastructure teardown aborts instead of converging.
Fix: the DeleteApplication handler in services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/delete_application.rs — remove the not-found refusal and make the delete idempotent over missing application state.
Traces: elasticbeanstalk.delete-application-idempotent-unknown.

Cause 3 of 3: a CloudFormation AWS::ElasticBeanstalk::Environment naming a nonexistent PlatformArn is created successfully.
AWS: recorded behavior only — a stack whose AWS::ElasticBeanstalk::Environment carries a PlatformArn of the form arn:aws:elasticbeanstalk:<region>::platform/<name>/<version> that does not resolve fails resource creation: the stack ends CREATE_FAILED, the failing resource is the Env resource itself, its failure reason names the platform, and no environment exists under the application afterwards. An otherwise identical stack with a valid platform succeeds (the check's positive control).
Emulator: the stack reaches CREATE_COMPLETE, so the invalid platform reference is accepted silently.
Why: an invalid platform reference is a configuration error a client depends on CloudFormation to surface. Reporting CREATE_COMPLETE makes a stack look healthy while the environment it promised is bogus, and any assertion on rollback or on failure reasons is unreachable.
Fix: the CreateEnvironment path's PlatformArn validation in the Elastic Beanstalk service (src/ops/create_environment.rs and the platform-version lookup in src/state.rs) — an unresolvable PlatformArn must be refused so the CloudFormation resource provisioner propagates a resource failure.
Traces: elasticbeanstalk.deploy-cloudformation-invalid-platform-arn-rejected.

## Evidence
Cause 1 — expected: code ValidationError, HTTP 400, message "1 validation error detected: Value '[NotARealAttribute]' at 'attributeNames' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [All, Color, Deployment, LaunchedAt, AvailabilityZone, Causes, RefreshedAt, HealthStatus, InstanceType, System, ApplicationMetrics]]". Actual (replay): "AssertionError: expected ValidationError for a malformed AttributeNames value even against an unknown EnvironmentName (the enum is validated before environment identity), got InvalidParameterValue (\"Value 'NotARealAttribute' at 'attributeNames.1.member' failed to satisfy constraint: Member must satisfy enum value set: [HealthStatus, Color, Causes, ApplicationMetrics, RefreshedAt, LaunchedAt, System, Deployment, AvailabilityZone, InstanceType, All]\")".
Cause 2 — expected: eb.delete_application(ApplicationName=<never created>) returns 200 and describe_applications stays empty, twice. Actual (replay): botocore.exceptions.ClientError: An error occurred (InvalidParameterValue) when calling the DeleteApplication operation: No Application named 'sim2real-behav-eb-app-never-created-<id>' found.
Cause 3 — expected: stack status CREATE_FAILED with the Env resource failing for a platform-related reason and no environment under the application. Actual (replay): "AssertionError: expected the bogus-platform stack to end CREATE_FAILED, got 'CREATE_COMPLETE'".

## Steps to Reproduce
Cause 1: eb.describe_instances_health(EnvironmentName='sim2real-ghost-env', AttributeNames=['NotARealAttribute']) -> read Error.Code; expect ValidationError, observe InvalidParameterValue.
Cause 2: name = 'never-created-app-<nonce>'; eb.describe_applications(ApplicationNames=[name]) -> []; eb.delete_application(ApplicationName=name) -> expect success, observe 400 InvalidParameterValue; repeat to confirm idempotence.
Cause 3: cfn.create_stack with Resources {App: AWS::ElasticBeanstalk::Application, Env: AWS::ElasticBeanstalk::Environment with PlatformArn 'arn:aws:elasticbeanstalk:<region>::platform/Sim2Real-Nonexistent-Platform/999.999.999'} and OnFailure='DO_NOTHING'; poll describe_stacks -> expect CREATE_FAILED with the Env resource's failure reason naming the platform, observe CREATE_COMPLETE.
Then re-replay the three check ids in trace_ids.
