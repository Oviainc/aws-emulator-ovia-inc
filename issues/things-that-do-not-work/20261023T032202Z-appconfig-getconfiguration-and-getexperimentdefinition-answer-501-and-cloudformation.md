Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig.get-configuration-nonexistent-profile-not-found appconfig.get-experiment-definition-not-found-returns-resourcenotfound appconfig.appconfig-application-cloudformation-behaves appconfig.deploy-appconfig-experiment-via-cloudformation
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig.get-configuration-nonexistent-profile-not-found|run#14|AssertionError|assert err_app["Code"] == "BadRequestException", (
Issue-Key: appconfig.get-experiment-definition-not-found-returns-resourcenotfound|run#5|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Key: appconfig|GetApplication|ResourceNotFoundException
Issue-Key: appconfig.deploy-appconfig-experiment-via-cloudformation|_deploy_cfn#2|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stacks[0].get("Outputs", [])}
Issue-Commit: 7f205ee96657d4151f96c4aa7296372f34590a63

# AppConfig GetConfiguration and GetExperimentDefinition answer 501, and CloudFormation-deployed AppConfig resources are not real (4 causes)

> _Note: this issue was identified against the code base at commit `7f205ee96657d4151f96c4aa7296372f34590a63`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConfiguration

## Description
Four distinct root causes, each grounded in a replayed behavioral check. None is covered by an auto-filed stub issue (the auto-filed appconfig stub issues name UpdateApplication, CreateDeploymentStrategy, CreateExperimentDefinition, CreateExtension, ListHostedConfigurationVersions, UpdateEnvironment, ListConfigurationProfiles, UpdateConfigurationProfile, ListEnvironments, DeleteHostedConfigurationVersion, ListApplications, ListExtensions, TagResource, ValidateConfiguration and ListDeploymentStrategies — not these two operations). Numbered; close ALL FOUR before submitting.

=== Cause 1 of 4: GetConfiguration is an unimplemented scaffold stub ===
AWS: GetConfiguration (the deprecated legacy data-plane read) resolves the (Application, Configuration) pair first and refuses an unresolvable triple with BadRequestException, HTTP 400, echoing the exact identifiers given — e.g. 'Configuration Profile with Application Id <app> and Configuration Profile Id <cp> could not be found for account <account>.' It never answers ResourceNotFoundException here, and an unknown Environment produces the same message shape as a real Environment with nothing deployed; once a deployment completes the identical call returns 200 with the deployed bytes (Smithy: GetConfiguration GET /applications/{Application}/environments/{Environment}/configurations/{Configuration}; errors BadRequestException 400, ResourceNotFoundException 404, InternalServerException 500).
Emulator: HTTP 501 with code 'NotImplemented'. services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/get_configuration.rs is still the generated stub — its handler is `Err(crate::scaffolded::Scaffolded::not_implemented("GetConfiguration"))` and the SCAFFOLDED marker is on line 4.
Why: the whole legacy retrieval path is unusable, and a client that distinguishes a bad-request refusal from a not-found one gets neither — it gets an unmodeled 501 the SDK cannot classify.
Fix: implement the handler in src/ops/get_configuration.rs over appconfig state (resolve the application+profile pair, then the environment's completed deployment; serve Content/ContentType/ConfigurationVersion).
Traces: appconfig.get-configuration-nonexistent-profile-not-found.

=== Cause 2 of 4: GetExperimentDefinition is an unimplemented scaffold stub ===
AWS: GetExperimentDefinition returns the definition (Id/Name/Status/Treatments/Control/...) and refuses an identifier that resolves to nothing with ResourceNotFoundException, HTTP 404 (Smithy: GetExperimentDefinition errors BadRequestException 400, InternalServerException 500, ResourceNotFoundException 404; Status is ExperimentDefinitionStatus one of ACTIVE|ARCHIVED|IDLE).
Emulator: HTTP 501 'NotImplemented'; src/ops/get_experiment_definition.rs is the generated stub (handler returns Scaffolded::not_implemented("GetExperimentDefinition"), SCAFFOLDED marker on line 4).
Why: an experiment definition can never be read back, so no client can observe the IDLE -> ACTIVE transition a run is supposed to cause, and a not-found probe gets a 501 instead of a modeled 404.
Fix: implement the handler in src/ops/get_experiment_definition.rs over the experiment-definition store.
Traces: appconfig.get-experiment-definition-not-found-returns-resourcenotfound.

=== Cause 3 of 4: a CloudFormation stack of AppConfig resources reaches CREATE_COMPLETE without creating them, and Ref hands back the template Name ===
AWS: a stack declaring AWS::AppConfig::Application / ::Environment / ::ConfigurationProfile creates live AppConfig resources; `Ref` on an AWS::AppConfig::Application returns the AppConfig-generated application Id (the 4-7 lowercase-alphanumeric Id shape the model gives, pattern ^[a-z0-9]{4,7}$), and GetApplication on it succeeds (recorded behavior; appconfig's model carries cloudFormationName 'AppConfig').
Emulator: the stack reaches CREATE_COMPLETE and its Outputs resolve, but the value handed back for the application is the template's Name parameter, and GetApplication on it answers ResourceNotFoundException (404) — nothing was registered in AppConfig.
Why: a stack that reports success leaves no usable resource: every follow-on appconfig call 404s, so CloudFormation-provisioned AppConfig estates are indistinguishable from an empty account while the stack claims otherwise. An open issue describes the same shape for other resource types (20261023T004328Z-bedrock-agent-runtime-lambda-s3vectors-cloudformation-stacks-declaring-aws-lambda-function-reach-create.md); it does not cover the AppConfig resource types.
Fix: the CloudFormation resource providers for the AWS::AppConfig::* types — create the resource through appconfig's own create path and use the returned Id as the physical resource id that Ref resolves to.
Traces: appconfig.appconfig-application-cloudformation-behaves.

=== Cause 4 of 4: DescribeStacks Outputs entries for an AppConfig stack carry no OutputValue ===
AWS: after a stack reaches CREATE_COMPLETE, each DescribeStacks Stacks[0].Outputs entry carries both OutputKey and OutputValue, including values produced by Fn::GetAtt on a resource attribute (here ExperimentDefinition.Id and HostedConfigurationVersion.VersionNumber) — recorded behavior.
Emulator: the stack reaches CREATE_COMPLETE (the stack_create_complete waiter returns), but at least one Outputs entry has no OutputValue member at all: building {o['OutputKey']: o['OutputValue'] for o in Outputs} raises KeyError: 'OutputValue'.
Why: a caller cannot read any identifier out of the stack, so nothing deployed by CloudFormation can be addressed afterwards — the failure is a missing response member, not a wrong value. An open issue reports the same missing member for an ECS/ELBv2 stack (20261023T025808Z-cloudformation-ecs-elbv2-cloudformation-stack-outputs-for-an-ecs-elbv2-stack.md); it is grounded in different checks and does not carry this one's replay key.
Fix: the CloudFormation stack-outputs resolver/serializer — resolve Ref and Fn::GetAtt for the stack's resources and always emit OutputValue alongside OutputKey.
Traces: appconfig.deploy-appconfig-experiment-via-cloudformation.

## Evidence
Replay report (simparity-behavioral-aws-v1):

Cause 1 — AssertionError: expected BadRequestException for an unknown Application, got 'NotImplemented' (appconfig.get_configuration(Application=<bogus>, Environment=<real>, Configuration=<real>, ClientId=...)). Expected per the recording: code BadRequestException (HTTP 400), message 'Configuration Profile with Application Id <fake-app> and Configuration Profile Id <cp> could not be found for account <account>.'

Cause 2 — AssertionError: expected ResourceNotFoundException, got NotImplemented (appconfig.get_experiment_definition on an identifier that resolves to nothing). Expected: ResourceNotFoundException, HTTP 404.

Cause 3 — botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetApplication operation: No application with id sim2real-behav-cfnapp-2c039f09011e exists. The id passed came from the stack's own Outputs.ApplicationId (Value: !Ref App) after the stack reported CREATE_COMPLETE — i.e. Ref resolved to the template's AppName parameter, and no application exists under it. Expected: Ref -> the AppConfig application Id (^[a-z0-9]{4,7}$) and GetApplication 200.

Cause 4 — KeyError: 'OutputValue' raised at outputs = {o['OutputKey']: o['OutputValue'] for o in stacks[0].get('Outputs', [])} in _deploy_cfn, after cfn.get_waiter('stack_create_complete').wait(...) returned for a stack whose Outputs are Ref Application/Environment/ConfigurationProfile plus Fn::GetAtt ExperimentDefinition.Id and HostedConfigurationVersion.VersionNumber.

## Steps to Reproduce
Cause 1: create an application, environment and hosted configuration profile, then appconfig.get_configuration(Application='<nonexistent>', Environment=<env id>, Configuration=<profile id>, ClientId='c1'). Emulator answers 501 NotImplemented; AWS answers 400 BadRequestException naming both identifiers.
Cause 2: appconfig.get_experiment_definition(ApplicationIdentifier=<real app>, ExperimentDefinitionIdentifier='<nonexistent>'). Emulator 501; AWS 404 ResourceNotFoundException.
Cause 3: cloudformation.create_stack with a template declaring AWS::AppConfig::Application (+ Environment, ConfigurationProfile) and Outputs {ApplicationId: !Ref App}; wait for CREATE_COMPLETE, read Outputs.ApplicationId, then appconfig.get_application(ApplicationId=<that value>) -> ResourceNotFoundException.
Cause 4: cloudformation.create_stack with the AppConfig application/environment/profile/hosted-version/experiment-definition template whose Outputs mix Ref and Fn::GetAtt; wait for CREATE_COMPLETE, then describe_stacks and read each Outputs entry — at least one has no OutputValue key.

Replay the checks in trace_ids to confirm.
