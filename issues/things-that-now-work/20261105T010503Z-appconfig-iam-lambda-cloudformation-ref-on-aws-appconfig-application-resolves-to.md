Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig__iam__lambda.e2e-lambda-appconfig-stale-value-troubleshoot-lifecycle
Issue-Key: appconfig|CreateHostedConfigurationVersion|ResourceNotFoundException
Issue-Commit: 0d591ad7d2d99ea902e67a996a0a91443522c019

# CloudFormation Ref on AWS::AppConfig::Application resolves to the template Name, so the AppConfig/Lambda lifecycle stack reports CREATE_COMPLETE with no real AppConfig application

> _Note: this issue was identified against the code base at commit `0d591ad7d2d99ea902e67a996a0a91443522c019`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateHostedConfigurationVersion

## Description
One root cause, grounded in the replayed behavioral check appconfig__iam__lambda.e2e-lambda-appconfig-stale-value-troubleshoot-lifecycle. It is NOT marked ALREADY FILED and NOT marked OWNED by any open issue, so no filed issue carries its replay key.

=== Cause 1 of 1: an AWS::AppConfig::Application declared in a CloudFormation stack is never registered in AppConfig, and Ref on it hands back the template's Name instead of the AppConfig-generated Id ===

AWS: a stack declaring AWS::AppConfig::Application / ::Environment / ::ConfigurationProfile / ::DeploymentStrategy creates live AppConfig resources through those services' own create paths. `Ref` on an AWS::AppConfig::Application resolves to the physical resource id, which for AppConfig is the service-generated application Id, and the model constrains that Id shape: Smithy (com.amazonaws.appconfig#AmazonAppConfig) CreateHostedConfigurationVersion output member ApplicationId targets `Id` with pattern `^[a-z0-9]{4,7}$` — never a caller-supplied Name (the Name shape is @length 1-64 free text). Consequently a follow-on control-plane call that addresses that Id succeeds: CreateHostedConfigurationVersion POST /applications/{ApplicationId}/configurationprofiles/{ConfigurationProfileId}/hostedconfigurationversions returns HTTP 201 with the Version-Number header. It only answers ResourceNotFoundException (HTTP 404) when the application genuinely does not exist. The error-message template for that class, as AppConfig words it, is fixed text 'Application with Id ' + <the ApplicationId from the request> + ' could not be found for account ' + <the account id from the caller's credentials> + '.' — the id and account are values from the request/state, the rest is the fixed text.

Emulator: the stack reaches CREATE_COMPLETE and DescribeStacks returns non-empty Outputs for ApplicationId, EnvironmentId, ConfigurationProfileId, DeploymentStrategyId, FunctionName and RoleName (the check's `deploy` phase passes in full, including Lambda GetFunctionConfiguration on the stack's function, so the Lambda and IAM resource providers do work). But the value Outputs.ApplicationId carries — produced by {"Ref": "AppConfigApplication"} — is the template's ApplicationName parameter value (a `sim2real-scn-app-<hex>` string), not an Id matching ^[a-z0-9]{4,7}$. No application was created in AppConfig under it, so the very next control-plane call is refused: appconfig.CreateHostedConfigurationVersion(ApplicationId=<that Ref value>, ...) answers ResourceNotFoundException, HTTP 404, 'Application with Id <that Ref value> could not be found for account <account>.' The replay stops at phase 2/5 (setup) and the whole five-phase lifecycle — setup idempotency, the frozen diagnosis trajectory, cleanup, destroy — is never exercised.

Why: a client that provisions an AppConfig estate with CloudFormation gets a stack that reports success while the account holds nothing. Every identifier read out of the stack's Outputs is unusable, so each subsequent appconfig/appconfigdata call 404s even though the stack claims the resources exist; the failure surfaces far from the deploy, as a not-found on an unrelated operation. The wrong value is also detectably the wrong SHAPE (a 60-char name where the model requires 4-7 lowercase alphanumerics), so SDK/consumer code that validates or stores the id corrupts downstream state too.

Note on existing issues: 20261023T032202Z-appconfig-getconfiguration-and-getexperimentdefinition-answer-501-and-cloudformation.md describes the same shape as its Cause 3 (grounded in appconfig.appconfig-application-cloudformation-behaves, a YAML template with Ref outputs). It does NOT carry this check's replay key and does not cover the JSON template path exercised here, where Ref on the Application is consumed as the ApplicationId property of ::Environment and ::ConfigurationProfile and the environment/profile ids come from Fn::GetAtt EnvironmentId / ConfigurationProfileId — those GetAtt outputs also resolve to values no AppConfig resource answers to. Fixing that issue's Cause 3 should be verified against THIS check as well.

Fix: the CloudFormation resource providers for the AWS::AppConfig::* types (Application, Environment, ConfigurationProfile, DeploymentStrategy) — create each resource through appconfig's own create handler, record the returned service-generated Id as the physical resource id that Ref resolves to, and resolve Fn::GetAtt EnvironmentId / ConfigurationProfileId from those created resources rather than from template properties. Until a resource is actually created the stack must not reach CREATE_COMPLETE.

Traces: appconfig__iam__lambda.e2e-lambda-appconfig-stale-value-troubleshoot-lifecycle (phase 2/5 setup).

## Evidence
Replay report (simparity-behavioral-aws-v1), check appconfig__iam__lambda.e2e-lambda-appconfig-stale-value-troubleshoot-lifecycle, verdict [error]:

  _PhaseError: [phase 2/5 setup] ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the CreateHostedConfigurationVersion operation: Application with Id sim2real-scn-app-c2a20dad382d could not be found for account 000000000000.

The value `sim2real-scn-app-c2a20dad382d` is the stack's ApplicationName PARAMETER value (ctx.unique_name('sim2real-scn-app-')), handed back by the stack output ApplicationId whose template value is {"Ref": "AppConfigApplication"} on a resource of Type AWS::AppConfig::Application. (The hex suffix is a per-run value; the requirement is the shape, not this string.)

Expected (AWS): Outputs.ApplicationId is an AppConfig application Id matching ^[a-z0-9]{4,7}$ (Smithy: CreateHostedConfigurationVersion output ApplicationId -> Id, pattern ^[a-z0-9]{4,7}$), and appconfig.CreateHostedConfigurationVersion(ApplicationId=<it>, ConfigurationProfileId=<the stack's profile id>, Content=b'{"useNewBackend": true}', ContentType='application/json') answers HTTP 201 with VersionNumber 1. No ResourceNotFoundException.

Actual (emulator): HTTP 404, Error.Code 'ResourceNotFoundException', message 'Application with Id <Ref value> could not be found for account <account>.' Fixed text: 'Application with Id ' / ' could not be found for account ' / '.'; variable parts: the ApplicationId taken from the request and the account id from state. The message wording itself matches AWS for a genuinely-missing application — the defect is that the application is missing at all, and that Ref produced a Name where an Id is required.

Preceding phase 1/5 (deploy) PASSED: cfn.create_stack reached CREATE_COMPLETE, all six required Outputs were non-empty, and lambda.get_function_configuration on the stack's function returned the three logical-id-shaped environment variables plus STAGE=beta with no Layers key — so the Lambda/IAM providers create real resources while the AppConfig ones do not.

## Steps to Reproduce
1. cloudformation.create_stack with Capabilities=['CAPABILITY_IAM'] and a JSON template declaring:
   - AppConfigApplication: AWS::AppConfig::Application, Properties.Name = {"Ref": "ApplicationName"}
   - AppConfigEnvironment: AWS::AppConfig::Environment, Properties.ApplicationId = {"Ref": "AppConfigApplication"}
   - AppConfigProfile: AWS::AppConfig::ConfigurationProfile, Properties.ApplicationId = {"Ref": "AppConfigApplication"}, LocationUri 'hosted', Type 'AWS.Freeform'
   - DeploymentStrategy: AWS::AppConfig::DeploymentStrategy (LINEAR, GrowthFactor 100, durations 0)
   - FunctionRole (AWS::IAM::Role) and ProcessingFunction (AWS::Lambda::Function, inline ZipFile code)
   and Outputs ApplicationId={"Ref":"AppConfigApplication"}, EnvironmentId={"Fn::GetAtt":["AppConfigEnvironment","EnvironmentId"]}, ConfigurationProfileId={"Fn::GetAtt":["AppConfigProfile","ConfigurationProfileId"]}, DeploymentStrategyId={"Ref":"DeploymentStrategy"}, FunctionName, RoleName. Pass unique names for every name parameter.
2. Poll describe_stacks until StackStatus == CREATE_COMPLETE (it does).
3. Read outputs = {o['OutputKey']: o['OutputValue'] for o in describe_stacks(StackName=stack)['Stacks'][0]['Outputs']}. Observe outputs['ApplicationId'] equals the ApplicationName parameter, not a 4-7 char lowercase-alphanumeric Id.
4. appconfig.create_hosted_configuration_version(ApplicationId=outputs['ApplicationId'], ConfigurationProfileId=outputs['ConfigurationProfileId'], Content=b'{"useNewBackend": true}', ContentType='application/json').
   Emulator: ResourceNotFoundException (404) 'Application with Id <name> could not be found for account <account>.'
   AWS: HTTP 201, VersionNumber 1.
5. Equivalently, appconfig.get_application(ApplicationId=outputs['ApplicationId']) and appconfig.list_applications() show no application was ever created by the stack.

Replay appconfig__iam__lambda.e2e-lambda-appconfig-stale-value-troubleshoot-lifecycle to confirm; it must get past phase 2/5 (setup).
