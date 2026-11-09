Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup__cloudformation.deploy-logically-air-gapped-backup-vault backup__cloudformation.deploy-logically-air-gapped-vault-invalid-retention-window-rejected backup__cloudformation.deploy-report-plan backup__cloudformation.deploy-restore-testing-plan-and-selection backup__cloudformation.deploy-tiering-configuration backup__cloudformation.deploy-tiering-configuration-unsupported-resource-type-rejected
Issue-Key: backup__cloudformation|CreateStack|ValidationError
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# backup__cloudformation: AWS::Backup::{LogicallyAirGappedBackupVault,ReportPlan,RestoreTestingPlan,RestoreTestingSelection,TieringConfiguration} are unregistered resource types

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the CloudFormation template resource-type registry is missing five real AWS::Backup::* types, so CreateStack refuses every template declaring one.

AWS: recorded behavior only — AWS CloudFormation recognizes AWS::Backup::LogicallyAirGappedBackupVault, ::ReportPlan, ::RestoreTestingPlan, ::RestoreTestingSelection and ::TieringConfiguration. CreateStack answers 200 with a StackId and the stack reaches CREATE_COMPLETE; a vault with a retention window outside the allowed range, or a tiering configuration naming an unsupported resource type, instead rolls the stack back with AWS Backup's own refusal as the resource failure reason.
Emulator: CreateStack answers 400 ValidationError `Template format error: Unrecognized resource types: [...]` naming those types. No vault, report plan, restore-testing plan/selection or tiering configuration is created, so neither the lifecycles nor the two validation-refusal legs are reached.
Why: the Backup vault/report/restore-testing/tiering surface is completely undeployable by template, and the user is told these AWS resource types do not exist.
Fix: register the five types in the CloudFormation template resource-type registry and add a resource handler per type over the backup crate, including the create-time range and resource-type validation each enforces.
Traces: the 6 checks in trace_ids.

## Evidence
Expected: CreateStack 200 + StackId; CREATE_COMPLETE for the lifecycles, ROLLBACK_COMPLETE citing AWS Backup's refusal for the two negative legs.
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [...], observed as:
  [AWS::Backup::LogicallyAirGappedBackupVault]  (both the lifecycle and the invalid-retention-window check)
  [AWS::Backup::ReportPlan]
  [AWS::Backup::RestoreTestingPlan, AWS::Backup::RestoreTestingSelection]
  [AWS::Backup::TieringConfiguration]  (both the lifecycle and the unsupported-resource-type check)

## Steps to Reproduce
1. CreateStack with a template declaring any of AWS::Backup::LogicallyAirGappedBackupVault, ::ReportPlan, ::RestoreTestingPlan, ::RestoreTestingSelection, ::TieringConfiguration.
2. Observe the 400 ValidationError "Template format error: Unrecognized resource types: [...]".
3. Re-replay the 6 checks in trace_ids.
