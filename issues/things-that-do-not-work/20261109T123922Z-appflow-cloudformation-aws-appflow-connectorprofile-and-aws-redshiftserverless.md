Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appflow__cloudformation.deploy-appflow-connector-profile-redshift-bad-data-api-role-update-rejected-and-rolled-back appflow__cloudformation.deploy-appflow-connector-profile-redshift-serverless-round-trip-and-delete
Issue-Key: appflow__cloudformation|CreateStack|ValidationError
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# appflow__cloudformation: AWS::AppFlow::ConnectorProfile and AWS::RedshiftServerless::{Namespace,Workgroup} are not registered resource types

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::AppFlow::ConnectorProfile, AWS::RedshiftServerless::Namespace and AWS::RedshiftServerless::Workgroup are missing from the CloudFormation template resource-type registry.

AWS: recorded behavior only — CreateStack accepts all three types, provisions the Redshift Serverless namespace/workgroup and the AppFlow connector profile that targets it, and reaches CREATE_COMPLETE; an update that points the connector profile at a Data API role AppFlow cannot use instead rolls the stack back with AppFlow's own refusal as the resource failure reason.
Emulator: CreateStack answers 400 ValidationError `Template format error: Unrecognized resource types: [AWS::RedshiftServerless::Namespace, AWS::RedshiftServerless::Workgroup, AWS::AppFlow::ConnectorProfile]` for both legs, so nothing is created and the bad-role refusal is never reached.
Why: AppFlow connector profiles and Redshift Serverless namespaces/workgroups cannot be provisioned from a template at all; the user is told these AWS resource types do not exist.
Fix: register the three types in the CloudFormation template resource-type registry and add a resource handler per type over the appflow and redshift-serverless crates, including the connector-profile role validation on create/update.
Traces: the 2 checks in trace_ids.

## Evidence
Expected: CreateStack 200 + StackId and CREATE_COMPLETE (ROLLBACK_COMPLETE citing AppFlow's refusal for the bad Data API role update).
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::RedshiftServerless::Namespace, AWS::RedshiftServerless::Workgroup, AWS::AppFlow::ConnectorProfile]

## Steps to Reproduce
1. CreateStack with a template declaring AWS::AppFlow::ConnectorProfile and/or AWS::RedshiftServerless::Namespace, ::Workgroup.
2. Observe the 400 ValidationError "Unrecognized resource types".
3. Re-replay the 2 checks in trace_ids.
