Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2.cfn-ec2-securitygroup-schema-structure-defers-to-ec2-api-and-warns
Issue-Key: cloudformation__ec2.cfn-ec2-securitygroup-schema-structure-defers-to-ec2-api-and-warns|run#17|AssertionError|assert len(failed) == 1, f"missing GroupDescription: expected one CREATE_FAILED event, got {failed!r}"
Issue-Commit: b4658095b9c4860859c50d7c6684da8fd2f8280e

# CloudFormation rolls a failed AWS::EC2::SecurityGroup create back without emitting a CREATE_FAILED resource event carrying the EC2 refusal

> _Note: this issue was identified against the code base at commit `b4658095b9c4860859c50d7c6684da8fd2f8280e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackEvents

## Description
One net-new root cause. Cause 1 of 1: a resource whose provisioning call was refused produces no CREATE_FAILED event.

AWS: when a stack resource's underlying service call is refused, CloudFormation records exactly one CREATE_FAILED event for that logical resource and puts the downstream service's refusal in ResourceStatusReason, wrapped in CloudFormation's API-error form that names the calling service, its HTTP status and the handler error code — for AWS::EC2::SecurityGroup without the required GroupDescription the wrapped parts are the EC2 message "The request must contain the parameter GroupDescription" (fixed text; the parameter name is the member missing from the request), service "Ec2", status "400", handler error code "InvalidRequest". The stack then reaches ROLLBACK_COMPLETE and the never-created resource reads DELETE_COMPLETE. Smithy com.amazonaws.cloudformation#DescribeStackEvents returns StackEvent{ResourceStatus, ResourceStatusReason}; the reason is how a client learns why provisioning failed. Recorded behavior only for the exact wrapper punctuation; the message, service name, status and handler code above are the requirement.

Emulator: the stack does reach ROLLBACK_COMPLETE, but DescribeStackEvents contains no CREATE_FAILED event at all for the security-group resource — the list of CREATE_FAILED reasons is empty, so no refusal reason is ever surfaced.

Why: a client (and every `cfn deploy`/CDK wrapper that prints the failure reason) sees a stack that rolled back for no stated reason. The underlying EC2 refusal — the one actionable fact — is lost, so the user cannot tell a schema mistake from a quota or permission failure.

Fix: the stack-event recording path of the CloudFormation provisioning engine (services/service-com-amazonaws-cloudformation-cloudformation/src/ — the resource-provisioning/rollback code that appends StackEvents): on a refused resource call, append a CREATE_FAILED event for that logical id whose ResourceStatusReason wraps the downstream service's code, status and message, before rolling the resource back to DELETE_COMPLETE.

Traces: cloudformation__ec2.cfn-ec2-securitygroup-schema-structure-defers-to-ec2-api-and-warns.

## Evidence
Replay output for cloudformation__ec2.cfn-ec2-securitygroup-schema-structure-defers-to-ec2-api-and-warns:
AssertionError: missing GroupDescription: expected one CREATE_FAILED event, got []

The preceding assertion (stack status == ROLLBACK_COMPLETE) passed, so the stack failed and rolled back but emitted no CREATE_FAILED event. Expected one CREATE_FAILED event whose reason parses to (message="The request must contain the parameter GroupDescription", service="Ec2", status="400", handler="InvalidRequest").

## Steps to Reproduce
1. cloudformation:CreateStack with a template containing a single AWS::EC2::SecurityGroup resource (logical id SG) that omits the required GroupDescription property.
2. Wait for the stack to reach ROLLBACK_COMPLETE.
3. cloudformation:DescribeStackEvents(StackName=<stack>) and filter ResourceStatus == CREATE_FAILED for logical id SG. Expected exactly one event whose ResourceStatusReason wraps the EC2 message "The request must contain the parameter GroupDescription" with service Ec2, status 400, handler InvalidRequest; the emulator returns none.
4. Re-replay cloudformation__ec2.cfn-ec2-securitygroup-schema-structure-defers-to-ec2-api-and-warns.
