Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig__lambda.extension-action-point-invokes-lambda-during-deployment appconfig__lambda.pre-create-hosted-configuration-version-extension-transforms-stored-content appconfig__lambda.validator-lambda-rejects-noncompliant-content-rejected
Issue-Key: appconfig__lambda.extension-action-point-invokes-lambda-during-deployment|run#44|Timeout|dep1 = ac.start_deployment(
Issue-Key: appconfig__lambda.pre-create-hosted-configuration-version-extension-transforms-stored-content|_create_version_when_extension_is_live#42|Timeout|time.sleep(5.0)
Issue-Key: appconfig__lambda.validator-lambda-rejects-noncompliant-content-rejected|run#43|AssertionError|assert re.sub(r'"requestId": "[0-9a-f-]+"', '"requestId": "<id>"', message) == (
Issue-Commit: a0ca0e6d405fa8f821a988e0d013d6063af395f8

# appconfig__lambda: AppConfig extension action points never invoke their Lambda, so the extension and validator scenarios never settle

> _Note: this issue was identified against the code base at commit `a0ca0e6d405fa8f821a988e0d013d6063af395f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDeployment

## Description
Cause 1 of 1 — an AppConfig extension association whose action point targets a Lambda never invokes it, so the deployment-time and pre-create action points produce no effect.
AWS: CreateExtension with an Action at PRE_START_DEPLOYMENT/ON_DEPLOYMENT_START/PRE_CREATE_HOSTED_CONFIGURATION_VERSION and a Uri naming a Lambda function invokes that function synchronously at the action point; a PRE_CREATE_HOSTED_CONFIGURATION_VERSION extension can rewrite the content that CreateHostedConfigurationVersion stores, and a validator Lambda that returns a failure makes CreateHostedConfigurationVersion refuse the content (BadRequestException naming the validator).
Emulator: recorded behavior only — the extension's Lambda is never invoked, so the deployment never shows the action point's effect and the stored content is unchanged; the harness reports "Check timed out after 60.0s" for the two extension checks, and the validator negative never reaches its refusal.
Why: a client relying on AppConfig extensions for validation or deployment hooks gets no validation at all — non-compliant configuration is stored and deployed, which is the opposite of the guarantee the extension exists for.
Fix: the AppConfig extension-association action-point dispatch (the invoke of the associated Lambda) and the validator gate in CreateHostedConfigurationVersion.
Traces: all three check ids in trace_ids.

## Evidence
Error: Check timed out after 60.0s for appconfig__lambda.extension-action-point-invokes-lambda-during-deployment and appconfig__lambda.pre-create-hosted-configuration-version-extension-transforms-stored-content; appconfig__lambda.validator-lambda-rejects-noncompliant-content-rejected fails in its body instead of observing the refusal. Expected: the associated function is invoked at the action point, the stored content reflects its transform, and non-compliant content is refused.

## Steps to Reproduce
1. CreateApplication/CreateConfigurationProfile/CreateEnvironment; CreateExtension with an Action at ON_DEPLOYMENT_START whose Uri is a Lambda ARN; CreateExtensionAssociation; StartDeployment and look for the function's side effect.
2. CreateExtension at PRE_CREATE_HOSTED_CONFIGURATION_VERSION that rewrites content; CreateHostedConfigurationVersion and read the stored content back.
3. Attach a validator Lambda that rejects, then CreateHostedConfigurationVersion with non-compliant content.
