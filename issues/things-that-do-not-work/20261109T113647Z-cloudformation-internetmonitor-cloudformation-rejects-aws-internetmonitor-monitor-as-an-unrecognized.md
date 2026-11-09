Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__internetmonitor.deploy-cloudformation-internetmonitor-monitor-round-trip-update-and-delete cloudformation__internetmonitor.deploy-cloudformation-internetmonitor-monitor-name-pattern-rejected
Issue-Key: cloudformation__internetmonitor|CreateStack|ValidationError
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# CloudFormation rejects AWS::InternetMonitor::Monitor as an unrecognized resource type, so no Internet Monitor monitor can be deployed by a stack

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause, grounded in two failing behavioral checks the replay report marks neither ALREADY FILED nor OWNED. Both checks fail on their very first CreateStack call, so everything downstream of the deploy is unreachable.

Cause 1 of 1 — the CloudFormation resource-type registry does not contain AWS::InternetMonitor::Monitor, so any template declaring an Internet Monitor monitor is refused at template validation.

AWS: recorded behavior only (CloudFormation's resource-type catalogue is not in the Smithy model). `AWS::InternetMonitor::Monitor` is a supported CloudFormation resource type; a template that declares it passes template validation, CreateStack returns a StackId and the stack reaches CREATE_COMPLETE after the monitor is created through the Internet Monitor control plane. Its properties map onto operations this emulator already serves and that already pass 21/21 of their own behavioral checks: Smithy internetmonitor CreateMonitor.MonitorName (target ResourceName, required, @length 1..255, @pattern ^[a-zA-Z0-9_.-]+$), CreateMonitor.MaxCityNetworksToMonitor (@range 1..500000), CreateMonitor.Tags (TagMap, @length 0..200) and the monitor state the stack must read back through GetMonitor (required members MonitorName, MonitorArn, Resources, Status, CreatedAt, ModifiedAt). The resource supports Ref (the MonitorName) and the read-only GetAtt attributes MonitorArn and CreatedAt; UpdateStack changing only MaxCityNetworksToMonitor is an in-place update (UpdateMonitor), not a replacement, and stack delete removes the monitor so GetMonitor afterwards answers 404 ResourceNotFoundException. Equally, the property-level pattern is enforced during stack creation rather than silently accepted: a MonitorName violating ^[a-zA-Z0-9_.-]+$ makes the resource fail early validation and the stack settle ROLLBACK_COMPLETE, with a VALIDATION_ERROR operation event whose ValidationName is PROPERTY_VALIDATION, ValidationStatus FAILED, ValidationFailureMode FAIL, ValidationPath /Resources/<logical id>/Properties/MonitorName and ValidationStatusReason of the fixed form `Property value [<value>] does not match pattern: <pattern>` — the bracketed value being the rejected MonitorName from the request and the pattern being the member's modeled pattern.

Emulator: CreateStack is refused HTTP 400 with error code `ValidationError` and the message `Template format error: Unrecognized resource types: [AWS::InternetMonitor::Monitor]`. The fixed text is `Template format error: Unrecognized resource types: [` ... `]`; the bracketed list is the set of resource types taken from the submitted template. Because the refusal happens on the first call of both checks, none of the round-trip, Ref/GetAtt, in-place-update, delete-absence or property-pattern-rejection behavior above is ever reached.

Why: a client cannot provision Internet Monitor through infrastructure-as-code at all. A template real CloudFormation accepts comes back as a template-format error, so stacks, change sets and CDK/SAM apps that include a monitor fail at submit time; and because the type is unknown, the property-validation rollback path that AWS uses to reject a bad MonitorName never exists either, so there is no rejection semantics to rely on.

Fix: the CloudFormation service's template resource-type registry and the resource handler it dispatches to — register AWS::InternetMonitor::Monitor, map its Create/Update/Delete onto the existing internetmonitor CreateMonitor/UpdateMonitor/DeleteMonitor paths, expose Ref as MonitorName and the GetAtt attributes MonitorArn and CreatedAt, propagate stack-level Tags plus the resource's own Tags to the monitor, and run the modeled property constraints (MonitorName @pattern, MaxCityNetworksToMonitor @range) as PROPERTY_VALIDATION operation events so an invalid value rolls the stack back instead of being accepted.

Traces: cloudformation__internetmonitor.deploy-cloudformation-internetmonitor-monitor-round-trip-update-and-delete (happy-path round trip, update in place, delete) and cloudformation__internetmonitor.deploy-cloudformation-internetmonitor-monitor-name-pattern-rejected (name-pattern rollback plus its valid-name positive control). Both abort on the same CreateStack refusal.

## Evidence
Expected: CreateStack accepts a template whose Resources declare {"Type": "AWS::InternetMonitor::Monitor", "Properties": {"MonitorName": ..., "MaxCityNetworksToMonitor": 1, "Status": "ACTIVE"}}; the call returns a StackId, the stack reaches CREATE_COMPLETE with a Monitor CREATE_COMPLETE resource event, Ref resolves to the MonitorName, GetAtt MonitorArn/CreatedAt match GetMonitor, UpdateStack with MaxCityNetworksToMonitor=5 updates the same monitor (CreatedAt unchanged), and after DeleteStack GetMonitor answers ResourceNotFoundException. With an invalid MonitorName the stack instead ends ROLLBACK_COMPLETE carrying a PROPERTY_VALIDATION / FAILED / FAIL event at /Resources/Monitor/Properties/MonitorName.

Actual (both checks, on the first CreateStack):
botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::InternetMonitor::Monitor]

Note the Internet Monitor service itself answers correctly in this run (internetmonitor 21/21, 100%), so the gap is confined to CloudFormation's resource-type registry.

## Steps to Reproduce
1. Replay cloudformation__internetmonitor.deploy-cloudformation-internetmonitor-monitor-round-trip-update-and-delete and cloudformation__internetmonitor.deploy-cloudformation-internetmonitor-monitor-name-pattern-rejected against the emulator.
2. Equivalently, call cloudformation CreateStack with TemplateBody = {"AWSTemplateFormatVersion":"2010-09-09","Resources":{"Monitor":{"Type":"AWS::InternetMonitor::Monitor","Properties":{"MonitorName":"<valid-name>","MaxCityNetworksToMonitor":1,"Status":"ACTIVE"}}},"Outputs":{"MonitorRef":{"Value":{"Ref":"Monitor"}},"MonitorArn":{"Value":{"Fn::GetAtt":["Monitor","MonitorArn"]}},"MonitorCreatedAt":{"Value":{"Fn::GetAtt":["Monitor","CreatedAt"]}}}}.
3. The call is refused with ValidationError / "Template format error: Unrecognized resource types: [AWS::InternetMonitor::Monitor]" instead of returning a StackId.
4. Repeat step 2 with a MonitorName containing a space and a '!' — AWS would create the stack and roll it back with a PROPERTY_VALIDATION failure on /Resources/Monitor/Properties/MonitorName; the emulator gives the same template-format refusal, so the rejection path is unobservable.
