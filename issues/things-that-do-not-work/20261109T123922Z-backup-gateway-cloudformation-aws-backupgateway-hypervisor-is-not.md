Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup-gateway__cloudformation.deploy-backupgateway-hypervisor-invalid-name-rejected backup-gateway__cloudformation.deploy-backupgateway-hypervisor-round-trip-and-delete
Issue-Key: backup-gateway__cloudformation|CreateStack|ValidationError
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# backup-gateway__cloudformation: AWS::BackupGateway::Hypervisor is not a registered resource type

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::BackupGateway::Hypervisor is missing from the CloudFormation template resource-type registry.

AWS: recorded behavior only — CreateStack accepts a template declaring AWS::BackupGateway::Hypervisor, provisions the hypervisor through Backup Gateway, and reaches CREATE_COMPLETE; a hypervisor whose Name violates the documented pattern instead rolls the stack back with Backup Gateway's own refusal as the resource failure reason.
Emulator: CreateStack answers 400 ValidationError `Template format error: Unrecognized resource types: [AWS::BackupGateway::Hypervisor]` for both legs, so neither the round-trip nor the name-validation refusal is reached.
Why: Backup Gateway hypervisors cannot be provisioned from a template.
Fix: register AWS::BackupGateway::Hypervisor in the CloudFormation template resource-type registry and add its resource handler over the backup-gateway crate, including the Name pattern check on create.
Traces: the 2 checks in trace_ids.

## Evidence
Expected: CreateStack 200 + StackId; CREATE_COMPLETE for the round-trip, ROLLBACK_COMPLETE citing the name refusal for the invalid-name leg.
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::BackupGateway::Hypervisor]

## Steps to Reproduce
1. CreateStack with a template declaring AWS::BackupGateway::Hypervisor.
2. Observe the 400 ValidationError "Unrecognized resource types: [AWS::BackupGateway::Hypervisor]".
3. Re-replay the 2 checks in trace_ids.
