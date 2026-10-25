Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__rds.cfn-rds-db-subnet-group-single-az-rejected-negative
Issue-Key: cloudformation__ec2__rds.cfn-rds-db-subnet-group-single-az-rejected-negative|run#11|AssertionError|assert any(e["ResourceStatus"] == "CREATE_FAILED" for e in group_events), (
Issue-Commit: 4244fec3063c58aa4a2476d7619962eecf74c002

# CloudFormation DescribeStackEvents emits no per-resource events, so a stack that failed on an RDS DB subnet group never names the failing resource or carries RDS's failure reason

> _Note: this issue was identified against the code base at commit `4244fec3063c58aa4a2476d7619962eecf74c002`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackEvents

## Description
One net-new root cause, grounded in the single failing check below. A closely related open issue, 20261025T015306Z-cloudformation-stack-events-and-resource-views-are-incomplete.md, describes the same family of defect in its Cause 1, but it does NOT carry this check's replay key (its Issue-Trace-Ids list no cloudformation__ec2__rds check), so this failure is filed here and should be re-replayed against that fix.

Cause 1 of 1: DescribeStackEvents returns only stack-level events; no event ever names a template resource's LogicalResourceId, so neither the resource that failed nor the reason it failed is reachable.

AWS: Smithy (com.amazonaws.cloudformation): DescribeStackEvents -> StackEvents of StackEvent, and StackEvent models LogicalResourceId, PhysicalResourceId, ResourceType, ResourceStatus and ResourceStatusReason ("Success/failure message associated with the resource") alongside the required StackId/StackName/EventId/Timestamp. Real CloudFormation records an event per resource transition: every declared resource emits at least CREATE_IN_PROGRESS and then CREATE_COMPLETE or CREATE_FAILED, and on a failed create the failing resource's event carries ResourceStatusReason wrapping the downstream provider's message. Recorded behavior for this scenario (measured against real CloudFormation in us-west-2, as documented inline in the check): the SubnetGroup resource emits a CREATE_FAILED event whose ResourceStatusReason is 'Resource handler returned message: "The DB subnet group doesn't meet Availability Zone (AZ) coverage requirement. Current AZ coverage: <az>. Add subnets to cover at least 2 AZs. (Service: Rds, Status Code: 400, Request ID: ...)" (RequestToken: ..., HandlerErrorCode: InvalidRequest)' — i.e. the reason text is built from the provider's own refusal and carries the stable substrings '(Service: Rds', 'Availability Zone (AZ) coverage requirement', 'at least 2 AZs' and 'HandlerErrorCode: InvalidRequest'. The successful resources (the VPC and its subnets) likewise emit CREATE_COMPLETE events carrying their PhysicalResourceId, which is what lets a client learn which VPC was created and then confirm it was rolled back.

Emulator: the whole event stream for the stack consists of four events whose LogicalResourceId is the STACK itself — CREATE_IN_PROGRESS, CREATE_FAILED, ROLLBACK_IN_PROGRESS, ROLLBACK_COMPLETE. Filtering the events for LogicalResourceId == 'SubnetGroup' (or 'Vpc') yields an empty list, so there is no CREATE_FAILED event for the failing resource, no ResourceStatusReason attributing the failure to RDS, and no PhysicalResourceId for the resources that did get created before the failure. The stack-level terminal status (ROLLBACK_COMPLETE) is reported correctly; only the per-resource event records are missing.

Why: a client relying on AWS's behavior cannot tell WHICH resource failed or WHY. Every deploy tool (the CLI's `cloudformation describe-stack-events`, CDK, Terraform's CFN resources, CI deploy wrappers) diagnoses a failed deploy by scanning the event stream for the first *_FAILED resource event and printing its ResourceStatusReason; against this emulator every failed stack looks like an unexplained stack-level failure. It also hides the provider classification (HandlerErrorCode: InvalidRequest vs a retryable handler error), so automation cannot decide whether to retry, and it makes the physical ids of partially created resources unobtainable, so rollback cannot be verified.

Fix: the CloudFormation provisioning engine must record a StackEvent per resource transition (create/update/delete, in progress and terminal) with LogicalResourceId, PhysicalResourceId, ResourceType, ResourceStatus and, for terminal failures, ResourceStatusReason built from the resource provider's refusal; the DescribeStackEvents handler must then serve those records interleaved with the stack-level events in reverse-chronological order.

Traces: cloudformation__ec2__rds.cfn-rds-db-subnet-group-single-az-rejected-negative.

## Evidence
Replayed suite check `cloudformation__ec2__rds.cfn-rds-db-subnet-group-single-az-rejected-negative` [mismatch]:

  AssertionError: the DB subnet group was not the resource that failed: [('sim2real-xsvc-stk-9a65d41f4cb0', 'CREATE_IN_PROGRESS'), ('sim2real-xsvc-stk-9a65d41f4cb0', 'CREATE_FAILED'), ('sim2real-xsvc-stk-9a65d41f4cb0', 'ROLLBACK_IN_PROGRESS'), ('sim2real-xsvc-stk-9a65d41f4cb0', 'ROLLBACK_COMPLETE')]

Every LogicalResourceId in the returned event list is the stack's own name; the template's 'SubnetGroup', 'Vpc', 'SubnetA' and 'SubnetB' resources appear in no event.

Expected (AWS): the event list additionally contains, for the template's resources, e.g.
  ('Vpc', 'CREATE_IN_PROGRESS'), ('Vpc', 'CREATE_COMPLETE', PhysicalResourceId='vpc-...'),
  ('SubnetGroup', 'CREATE_IN_PROGRESS'), ('SubnetGroup', 'CREATE_FAILED', ResourceStatusReason='Resource handler returned message: "The DB subnet group doesn't meet Availability Zone (AZ) coverage requirement. Current AZ coverage: us-west-2a. Add subnets to cover at least 2 AZs. (Service: Rds, Status Code: 400, Request ID: ...)" (RequestToken: ..., HandlerErrorCode: InvalidRequest)')
with the request id / request token / stack name being per-run values, and the quoted reason text an example of the rule 'the reason wraps the provider's message and HandlerErrorCode'.

Actual (emulator): four stack-level events only; the per-resource assertions, the ResourceStatusReason assertions, and the subsequent 'the VPC never reached CREATE_COMPLETE' rollback check are all unreachable.

Status/code facts: the stack itself does reach CREATE_FAILED -> ROLLBACK_COMPLETE, so this is not a rollback defect; it is purely the absence of per-resource StackEvent records.

## Steps to Reproduce
1. Boot the emulator and replay `cloudformation__ec2__rds.cfn-rds-db-subnet-group-single-az-rejected-negative`.
2. Or by hand: CreateStack with a template declaring an AWS::EC2::VPC, two AWS::EC2::Subnet resources placed in the SAME availability zone (via !GetAZs/Select), and an AWS::RDS::DBSubnetGroup over those two subnets.
3. Poll DescribeStacks until the stack reaches a terminal status (the emulator reports ROLLBACK_COMPLETE, as AWS does).
4. Call DescribeStackEvents(StackName=<stack>) and group the events by LogicalResourceId.
5. AWS: the group contains entries for 'Vpc', 'SubnetA', 'SubnetB' and 'SubnetGroup', with 'SubnetGroup' carrying a CREATE_FAILED event whose ResourceStatusReason quotes RDS's AZ-coverage refusal and 'HandlerErrorCode: InvalidRequest'. Emulator: every event's LogicalResourceId is the stack name, so the per-resource groups are empty.
