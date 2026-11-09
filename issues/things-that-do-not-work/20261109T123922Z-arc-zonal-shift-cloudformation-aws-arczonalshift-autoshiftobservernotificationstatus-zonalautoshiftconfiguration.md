Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: arc-zonal-shift__cloudformation.deploy-arc-zonal-shift-autoshift-observer-notification-status-invalid-status-rejected arc-zonal-shift__cloudformation.deploy-arc-zonal-shift-autoshift-observer-notification-status-round-trip-and-delete arc-zonal-shift__cloudformation.deploy-arc-zonal-shift-zonal-autoshift-configuration-round-trip-and-delete
Issue-Key: arc-zonal-shift__cloudformation|CreateStack|ValidationError
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# arc-zonal-shift__cloudformation: AWS::ARCZonalShift::{AutoshiftObserverNotificationStatus,ZonalAutoshiftConfiguration} are not registered resource types

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::ARCZonalShift::AutoshiftObserverNotificationStatus and AWS::ARCZonalShift::ZonalAutoshiftConfiguration are missing from the CloudFormation template resource-type registry.

AWS: recorded behavior only — CreateStack accepts both types, applies them through ARC Zonal Shift, and reaches CREATE_COMPLETE; an AutoshiftObserverNotificationStatus whose Status is outside the documented enum instead rolls the stack back with ARC Zonal Shift's own refusal as the resource failure reason.
Emulator: CreateStack answers 400 ValidationError `Template format error: Unrecognized resource types: [AWS::ARCZonalShift::AutoshiftObserverNotificationStatus]` / `[AWS::ARCZonalShift::ZonalAutoshiftConfiguration]`, so nothing is applied and the invalid-status refusal is never reached.
Why: zonal-autoshift configuration and autoshift observer notification status cannot be managed from a template; the user is told these AWS resource types do not exist.
Fix: register both types in the CloudFormation template resource-type registry and add a resource handler per type over the arc-zonal-shift crate, including the Status enum check on create and the account-singleton semantics of the notification-status resource.
Traces: the 3 checks in trace_ids.

## Evidence
Expected: CreateStack 200 + StackId and CREATE_COMPLETE (ROLLBACK_COMPLETE citing the refusal for the invalid-status leg).
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::ARCZonalShift::AutoshiftObserverNotificationStatus] / [AWS::ARCZonalShift::ZonalAutoshiftConfiguration]

## Steps to Reproduce
1. CreateStack with a template declaring AWS::ARCZonalShift::AutoshiftObserverNotificationStatus or ::ZonalAutoshiftConfiguration.
2. Observe the 400 ValidationError "Unrecognized resource types".
3. Re-replay the 3 checks in trace_ids.
