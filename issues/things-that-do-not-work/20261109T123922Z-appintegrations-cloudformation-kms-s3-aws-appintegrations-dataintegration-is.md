Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appintegrations__cloudformation__kms__s3.deploy-appintegrations-data-integration-source-uri-bucket-prefix-rejected appintegrations__cloudformation__kms__s3.deploy-appintegrations-data-integration-with-kms-and-s3-round-trip-and-delete
Issue-Key: appintegrations__cloudformation__kms__s3|CreateStack|ValidationError
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# appintegrations__cloudformation__kms__s3: AWS::AppIntegrations::DataIntegration is not a registered resource type

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::AppIntegrations::DataIntegration is missing from the CloudFormation template resource-type registry.

AWS: recorded behavior only — CreateStack accepts AWS::AppIntegrations::DataIntegration, provisions the data integration with its KMS key and S3 source URI, and reaches CREATE_COMPLETE; a SourceURI whose bucket prefix is not acceptable instead rolls the stack back with AppIntegrations' own refusal as the resource failure reason.
Emulator: CreateStack answers 400 ValidationError `Template format error: Unrecognized resource types: [AWS::AppIntegrations::DataIntegration]` for both legs, so neither the KMS/S3 round-trip nor the SourceURI refusal is reached.
Why: AppIntegrations data integrations cannot be declared in a template at all, so the KMS-key and S3-source wiring is undeployable.
Fix: register AWS::AppIntegrations::DataIntegration in the CloudFormation template resource-type registry and add its resource handler over the appintegrations crate, including SourceURI and KMS-key validation on create.
Traces: the 2 checks in trace_ids.

## Evidence
Expected: CreateStack 200 + StackId and CREATE_COMPLETE (ROLLBACK_COMPLETE citing the SourceURI refusal for the negative leg).
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::AppIntegrations::DataIntegration]

## Steps to Reproduce
1. CreateStack with a template declaring AWS::AppIntegrations::DataIntegration with a KMS key and an S3 SourceURI.
2. Observe the 400 ValidationError "Unrecognized resource types: [AWS::AppIntegrations::DataIntegration]".
3. Re-replay the 2 checks in trace_ids.
