Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bcm-dashboards__cloudformation.deploy-bcm-dashboard-invalid-widget-title-rejected-and-rolled-back bcm-dashboards__cloudformation.deploy-bcm-dashboard-round-trip-and-delete bcm-dashboards__cloudformation.deploy-bcm-scheduled-report-round-trip-and-delete
Issue-Key: bcm-dashboards__cloudformation|CreateStack|ValidationError
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# bcm-dashboards__cloudformation: AWS::BCM::{Dashboard,ScheduledReport} are not registered resource types

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::BCM::Dashboard and AWS::BCM::ScheduledReport are missing from the CloudFormation template resource-type registry.

AWS: recorded behavior only — CreateStack accepts both types, provisions the Billing and Cost Management dashboard and scheduled report, and reaches CREATE_COMPLETE; a dashboard with an invalid widget title instead rolls the stack back with the service's own refusal as the resource failure reason.
Emulator: CreateStack answers 400 ValidationError `Template format error: Unrecognized resource types: [AWS::BCM::Dashboard]` / `[AWS::BCM::Dashboard, AWS::BCM::ScheduledReport]`, so nothing is created and the invalid-widget-title refusal is never reached.
Why: BCM dashboards and scheduled reports cannot be declared in a template at all; the user is told these AWS resource types do not exist.
Fix: register both types in the CloudFormation template resource-type registry and add a resource handler per type over the bcm-dashboards crate, including widget-title validation on create.
Traces: the 3 checks in trace_ids.

## Evidence
Expected: CreateStack 200 + StackId and CREATE_COMPLETE (ROLLBACK_COMPLETE citing the widget-title refusal for the negative leg).
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::BCM::Dashboard] / [AWS::BCM::Dashboard, AWS::BCM::ScheduledReport]

## Steps to Reproduce
1. CreateStack with a template declaring AWS::BCM::Dashboard or AWS::BCM::ScheduledReport.
2. Observe the 400 ValidationError "Unrecognized resource types".
3. Re-replay the 3 checks in trace_ids.
