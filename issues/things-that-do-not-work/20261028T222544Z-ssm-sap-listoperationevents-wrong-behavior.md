Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6dde6b5b59a4735e67d87b555115fcc549deec6e

# ListOperationEvents never reports the EC2-instance milestones the model documents for an operation

_Filed: 2026-10-28T22:25:44Z_
_Operation: ListOperationEvents | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6dde6b5b59a4735e67d87b555115fcc549deec6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The milestones `ListOperationEvents` reports are derived in `StoredOperation::events()`, which can only ever produce one or two events, both describing the APPLICATION the call named, and both carrying a single `Resource$ResourceType` spelling — `AWS::SystemsManagerSAP::Application` — that the model never mentions.

The model documents the opposite shape of data: `OperationEvent$Description`'s own example is an EC2-instance milestone ("Stop the EC2 instance i-abcdefgh987654321"), and `Resource$ResourceType`'s examples are `AWS::SystemsManagerSAP::Component` and `AWS::EC2::Instance`. So for a `StopApplication` with `IncludeEc2InstanceShutdown: true`, the documented event list includes a per-instance milestone; this crate's list holds only `"STOP_APPLICATION started for application <id>"` and `"STOP_APPLICATION completed for application <id>"`.

This is not a missing integration: the crate already resolves exactly the instances such a milestone would name (`StoredApplication::ec2_instance_ids`) and already asks EC2 to stop each one (`ec2_shutdown::stop_instances`, which walks the instances ONE AT A TIME precisely so each is independent). The event derivation deliberately drops them, and says so in a comment. As a consequence `event_resource_type`'s `None` branch — and therefore an absent `Resource$ResourceType` — is unreachable, since `record_operation` hardcodes `resource_type` to `APPLICATION`.

## Evidence

Model, smithy_query(query="doc", shape="com.amazonaws.ssmsap#Resource", member="ResourceType"):
  "<p>The resource type.</p> <p>Example of <code>ResourceType</code>: \"<code>AWS::SystemsManagerSAP::Component</code>\" or \"<code>AWS::EC2::Instance</code>\".</p>"
Model, smithy_query(query="shape", shape="com.amazonaws.ssmsap#OperationEvent"), member `Description`:
  "<p>A description of the operation event. For example, \"Stop the EC2 instance i-abcdefgh987654321\".</p>"

The crate produces neither. services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:1290-1293:
  `pub fn event_resource_type(resource_type: &str) -> Option<&'static str> {`
  `    (resource_type == APPLICATION_RESOURCE_TYPE)`
  `        .then_some(OPERATION_EVENT_RESOURCE_TYPE_APPLICATION)`
  `}`
with state.rs:219:
  `pub const OPERATION_EVENT_RESOURCE_TYPE_APPLICATION: &str = "AWS::SystemsManagerSAP::Application";`
and the whole event list, state.rs:1210-1238, is one start milestone plus at most one terminal milestone, both worded about the application:
  `format!("{} started for {kind} {}", self.op_type, self.resource_id)`
  `format!("{} {verb} for {kind} {}", self.op_type, self.resource_id)`

The omission is stated in the code itself — state.rs:1187-1191:
  `/// Nothing else is reported: the milestones an operation has are the ones of the`
  `/// resource the call named, so a change this service asked ANOTHER service to make on`
  `/// its own resource (the instance shutdown of`
  `/// [\`ec2_shutdown::stop_instances\`]) is a milestone of that resource there, and none`
  `/// of this record here.`
while the instances that milestone would name are already in hand at the point the stop is filed — services/service-com-amazonaws-ssmsap-ssmsap/src/ops/stop_application.rs:268-272:
  `let instances_to_shut_down = if input.include_ec2_instance_shutdown == Some(true) {`
  `    application.ec2_instance_ids()`
and services/service-com-amazonaws-ssmsap-ssmsap/src/state/ec2_shutdown.rs:68-73:
  `for instance_id in instance_ids {`
  `    let answer = client.stop_instances().instance_ids(instance_id).send().await;`

Test coverage: services/service-com-amazonaws-ssmsap-ssmsap/tests/all/list_operation_events.rs exercises the two application milestones and the `status`/`resourceID`/`resourceType` filters; no test covers an instance-level milestone, and tests/all/ec2_instance_shutdown.rs checks the instance state in EC2 but not the operation's event list.

## Steps to Reproduce

1. Register and activate an application whose registration names an EC2 instance
2. POST /stop-application with ApplicationId=<id> and IncludeEc2InstanceShutdown=true; keep the returned OperationId
3. Wait past the transition span and POST /list-operation-events with that OperationId
4. Observe exactly two events, both with Resource.ResourceType="AWS::SystemsManagerSAP::Application" and Description="STOP_APPLICATION started/completed for application <id>" — no event whose Description names the stopped instance and no event with Resource.ResourceType "AWS::EC2::Instance", though EC2 did move the instance (verified via DescribeInstances)
5. Compare with smithy_query(query="doc", shape="com.amazonaws.ssmsap#Resource", member="ResourceType") and the OperationEvent$Description example
