Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appintegrations__cloudformation.deploy-appintegrations-application-and-event-integration-round-trip-and-delete appintegrations__cloudformation.deploy-appintegrations-event-integration-invalid-event-source-rejected
Issue-Key: appintegrations__cloudformation|CreateStack|ValidationError
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# appintegrations__cloudformation: AWS::AppIntegrations::{Application,EventIntegration} are not registered resource types

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::AppIntegrations::Application and AWS::AppIntegrations::EventIntegration are missing from the CloudFormation template resource-type registry.

AWS: recorded behavior only — CreateStack accepts both types, provisions the application and the event integration through Amazon AppIntegrations, and reaches CREATE_COMPLETE; an event integration naming an invalid event source instead rolls the stack back with AppIntegrations' own refusal as the resource failure reason.
Emulator: CreateStack answers 400 ValidationError `Template format error: Unrecognized resource types: [AWS::AppIntegrations::Application, AWS::AppIntegrations::EventIntegration]` (and `[AWS::AppIntegrations::EventIntegration]` for the single-resource template), so nothing is created and the invalid-event-source refusal is never reached.
Why: AppIntegrations applications and event integrations cannot be provisioned from a template; the user is told these AWS resource types do not exist.
Fix: register both types in the CloudFormation template resource-type registry and add a resource handler per type over the appintegrations crate, including EventSource validation on create.
Traces: the 2 checks in trace_ids.

## Evidence
Expected: CreateStack 200 + StackId and CREATE_COMPLETE (ROLLBACK_COMPLETE citing the event-source refusal for the negative leg).
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::AppIntegrations::Application, AWS::AppIntegrations::EventIntegration] / [AWS::AppIntegrations::EventIntegration]

## Steps to Reproduce
1. CreateStack with a template declaring AWS::AppIntegrations::Application or ::EventIntegration.
2. Observe the 400 ValidationError "Unrecognized resource types".
3. Re-replay the 2 checks in trace_ids.
