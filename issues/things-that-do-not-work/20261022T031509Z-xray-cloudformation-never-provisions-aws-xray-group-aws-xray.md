Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: xray.deploy-group-via-cloudformation-filters-traces xray.deploy-xray-group-cfn-filter-behaves xray.deploy-xray-group-cfn-then-filters-traces xray.deploy-xray-sampling-rule-cfn-applies xray.deploy-group-via-cloudformation-invalid-filter-expression-rejected
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray|GetGroup|InvalidRequestException
Issue-Key: xray.deploy-xray-group-cfn-filter-behaves|run#7|KeyError|assert f"/{group_name}/" in outputs["GroupARN"], (
Issue-Key: xray.deploy-xray-group-cfn-then-filters-traces|run#12|KeyError|group_arn = next(o["OutputValue"] for o in stack["Outputs"] if o["OutputKey"] == "GroupArn")
Issue-Key: xray.deploy-xray-sampling-rule-cfn-applies|run#7|KeyError|assert outputs["RuleARN"].endswith(rule_name), (
Issue-Key: xray.deploy-group-via-cloudformation-invalid-filter-expression-rejected|run#9|AssertionError|assert terminal_status == "ROLLBACK_COMPLETE", (
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# CloudFormation never provisions AWS::XRay::Group / AWS::XRay::SamplingRule: stacks reach CREATE_COMPLETE with no resource, no Outputs, and no rollback on resource failure

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three distinct root causes in the CloudFormation deploy path for X-Ray resource types. All five failing checks belong to the `xray` suite (`SERVICES = ["xray"]`, GROUP = "deploy"), so the issue carries service `xray`, but the defective component is the CloudFormation service's resource-provisioning layer: every one of these checks creates a stack whose only resources are AWS::XRay::Group / AWS::XRay::SamplingRule. No open issue covers any of these three causes (list_existing_issues query='cloudformation' returns nothing; the three open X-Ray cluster issues cover the group control plane, the trace data plane and NotImplemented stubs, none of them the CFN path).

Cause 1 of 3: an AWS::XRay::Group / AWS::XRay::SamplingRule resource declared in a template is never actually created, yet the stack still reports success.
AWS: CreateStack on a template containing AWS::XRay::Group invokes X-Ray CreateGroup during stack create; once the stack reaches CREATE_COMPLETE the group exists and GetGroup(GroupName=<template GroupName>) returns a Group whose FilterExpression is the template's. The same holds for AWS::XRay::SamplingRule and GetSamplingRules. Smithy (services/service-com-amazonaws-xray-awsxray/smithy-model/com-amazonaws-xray-awsxray.json): CreateGroup -> Group{GroupName, GroupARN, FilterExpression}; GetGroup errors InvalidRequestException (client, httpError 400). The CFN resource-type-to-API mapping itself is recorded behavior only.
Emulator: the stack_create_complete waiter returns, i.e. the stack is reported CREATE_COMPLETE, but the X-Ray resource does not exist — GetGroup is then refused with HTTP 400 InvalidRequestException (recorded message, an example: 'no group matches GroupName Some("sim2real-grp-07ce89deccba") / GroupARN None'), and GetSamplingRules holds no record for the deployed RuleName. The stack create is a no-op that only flips a status.
Why: a client that provisions its tracing groups/sampling rules by stack and then uses them (GetGroup, group.name filter expressions, GetSamplingRules/GetSamplingTargets) gets CREATE_COMPLETE and then a 400 on the very resource the stack claims to own; every downstream read and every group-scoped query is wrong, and deploy-driven tests of real infrastructure cannot run at all.
Fix: resource providers for AWS::XRay::Group and AWS::XRay::SamplingRule in the CloudFormation service's stack-resource provisioning layer (services/service-com-amazonaws-cloudformation-cloudformation), calling the corresponding X-Ray handlers on create and on stack delete.
Traces: xray.deploy-group-via-cloudformation-filters-traces (fails at the first GetGroup after the waiter), xray.deploy-xray-sampling-rule-cfn-applies, xray.deploy-xray-group-cfn-filter-behaves, xray.deploy-xray-group-cfn-then-filters-traces.

Cause 2 of 3: stack Outputs are not produced — DescribeStacks returns a Stack with no Outputs at all, and Ref / !GetAtt against these resources never resolve.
AWS: DescribeStacks returns, for a stack whose template declares Outputs, a Stack with an `Outputs` list holding one Output{OutputKey, OutputValue} per declared output; `{"Ref": "Group"}` on an AWS::XRay::Group resolves to the group's ARN and `!GetAtt Group.GroupARN` / `!GetAtt Rule.RuleARN` to the resource ARN, which embeds the GroupName / RuleName. Recorded behavior only for the attribute names; the Outputs member itself is modeled on DescribeStacks' Stack shape.
Emulator: `cfn.describe_stacks(StackName=...)["Stacks"][0]` has no `Outputs` key whatsoever (raises KeyError 'Outputs'), and where a harness defaults outputs to a dict the requested keys are simply absent (KeyError 'GroupARN', KeyError 'RuleARN').
Why: Outputs are the documented contract by which one stack hands identifiers to its consumers; a client reading an ARN out of a stack output gets a KeyError instead of a value, so cross-stack wiring and any post-deploy lookup by ARN is impossible even once Cause 1 is fixed.
Fix: output evaluation in the CloudFormation stack-create/describe path — resolve Ref and Fn::GetAtt against the provisioned resource's attributes and populate Stack.Outputs in the DescribeStacks serializer.
Traces: xray.deploy-xray-group-cfn-then-filters-traces (KeyError 'Outputs'), xray.deploy-xray-group-cfn-filter-behaves (KeyError 'GroupARN'), xray.deploy-xray-sampling-rule-cfn-applies (KeyError 'RuleARN').

Cause 3 of 3: a resource whose underlying create would be refused does not fail its stack — there is no CREATE_FAILED resource event, no rollback, and no failure reason naming the upstream service.
AWS: an AWS::XRay::Group whose FilterExpression is syntactically invalid (e.g. the unbalanced '(annotation.scenario = "x"') makes CreateGroup answer HTTP 400 InvalidRequestException; CloudFormation then marks the resource CREATE_FAILED with a ResourceStatusReason that names the upstream call ('Service: XRay', 'Status Code: 400'), and because the stack was created with the default OnFailure=ROLLBACK the stack reaches terminal status ROLLBACK_COMPLETE with the stack-level reason 'The following resource(s) failed to create: [TraceGroup]. Rollback requested by user.'; the group never exists afterwards. Smithy (xray): CreateGroup models only InvalidRequestException (400) and ThrottledException (429), so there is no success path for an unparseable expression. The rollback status and the resource/stack event reasons are recorded behavior.
Emulator: the stack reaches terminal status CREATE_COMPLETE instead — the invalid resource is accepted, nothing is marked CREATE_FAILED and no rollback occurs.
Why: a deploy that AWS refuses is reported as a successful deploy, so a client promotes a stack that provisioned nothing and has no failure event to diagnose; error propagation from a resource's underlying API call is the whole safety property of CFN create.
Note: the X-Ray side of this (CreateGroup accepting an invalid FilterExpression with 200) is already filed as Cause 2 of 20261022T013648Z-xray-x-ray-group-control-plane-no-implicit-default.md. This cause is the CloudFormation failure-propagation and rollback machinery, which that issue does not cover, and it remains broken independently: with Cause 1 in place no resource create is ever attempted, so no refusal can ever reach the stack.
Fix: resource-create error propagation in the CloudFormation stack-create state machine — map a resource provider's error to a CREATE_FAILED resource event with a reason naming the upstream service and status, drive the stack to ROLLBACK_IN_PROGRESS/ROLLBACK_COMPLETE under OnFailure=ROLLBACK, and record both events for DescribeStackEvents.
Traces: xray.deploy-group-via-cloudformation-invalid-filter-expression-rejected.

## Evidence
Cause 1: xray.deploy-group-via-cloudformation-filters-traces — the stack_create_complete waiter returned (stack reported CREATE_COMPLETE), then `botocore.errorfactory.InvalidRequestException: An error occurred (InvalidRequestException) when calling the GetGroup operation: no group matches GroupName Some("sim2real-grp-07ce89deccba") / GroupARN None`. Expected: HTTP 200 with Group{GroupName=<template GroupName>, FilterExpression='annotation.scenario = "<marker>"'}.
Cause 2: xray.deploy-xray-group-cfn-then-filters-traces — `group_arn = next(o["OutputValue"] for o in stack["Outputs"] ...)` raised `KeyError: 'Outputs'` on the Stack returned by DescribeStacks after StackStatus became CREATE_COMPLETE. xray.deploy-xray-group-cfn-filter-behaves — `assert f"/{group_name}/" in outputs["GroupARN"]` raised `KeyError: 'GroupARN'` for a template declaring `Outputs: GroupARN: Value: !GetAtt Group.GroupARN`. xray.deploy-xray-sampling-rule-cfn-applies — `assert outputs["RuleARN"].endswith(rule_name)` raised `KeyError: 'RuleARN'` for `Outputs: RuleARN: Value: !GetAtt Rule.RuleARN`.
Cause 3: xray.deploy-group-via-cloudformation-invalid-filter-expression-rejected — `AssertionError: stack with an invalid FilterExpression reached CREATE_COMPLETE instead of rolling back`. Expected terminal StackStatus ROLLBACK_COMPLETE, a TraceGroup CREATE_FAILED event whose reason contains 'Service: XRay' and 'Status Code: 400', a ROLLBACK_IN_PROGRESS stack event with reason 'The following resource(s) failed to create: [TraceGroup]. Rollback requested by user.', and GetGroup on the name refused with InvalidRequestException.

## Steps to Reproduce
1. Boot the emulator (run_behavioral_suite builds and boots it).
2. Cause 1: cfn.create_stack with a template whose only resource is {"Type": "AWS::XRay::Group", "Properties": {"GroupName": "g1", "FilterExpression": "annotation.scenario = \"m\""}}; wait for stack_create_complete; then call xray.get_group(GroupName="g1") — observe HTTP 400 InvalidRequestException instead of the group. Repeat with AWS::XRay::SamplingRule and xray.get_sampling_rules().
3. Cause 2: add `Outputs: {"GroupARN": {"Value": {"Fn::GetAtt": ["Group", "GroupARN"]}}}` (or {"Ref": "Group"}) to that template, create the stack, and inspect cfn.describe_stacks(StackName=...)["Stacks"][0] — there is no "Outputs" key.
4. Cause 3: create a stack whose AWS::XRay::Group has FilterExpression '(annotation.scenario = "x"' (unbalanced parenthesis) with default OnFailure; poll describe_stacks for a terminal status — observe CREATE_COMPLETE instead of ROLLBACK_COMPLETE, and describe_stack_events showing no CREATE_FAILED event for the TraceGroup resource.
5. Re-replay the five check ids in trace_ids to confirm the fix.
