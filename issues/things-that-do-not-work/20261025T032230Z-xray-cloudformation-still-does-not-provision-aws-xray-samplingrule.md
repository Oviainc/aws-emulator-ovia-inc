Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: xray.deploy-xray-sampling-rule-cfn-then-serves-targets xray.deploy-xray-group-cfn-filter-behaves xray.deploy-xray-sampling-rule-cfn-applies
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray.deploy-xray-sampling-rule-cfn-then-serves-targets|run#14|AssertionError|assert record is not None, "template-defined sampling rule not found via GetSamplingRules"
Issue-Key: xray.deploy-xray-group-cfn-filter-behaves|_deploy_cfn#6|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in desc.get("Outputs", [])}
Issue-Key: xray.deploy-xray-sampling-rule-cfn-applies|_deploy_cfn#6|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in desc.get("Outputs", [])}
Issue-Commit: 01142ac518ef08b9ccb64fede7482782162b2994

# CloudFormation still does not provision AWS::XRay::SamplingRule, and !GetAtt outputs for X-Ray resources are now emitted with no OutputValue (2 causes)

> _Note: this issue was identified against the code base at commit `01142ac518ef08b9ccb64fede7482782162b2994`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two root causes in the CloudFormation deploy path for X-Ray resource types. All three failing checks belong to the xray suite (`SERVICES = ["xray"]`, GROUP = "deploy"), so the issue carries service `xray`, but the defective component is CloudFormation's resource-provisioning / output-evaluation layer. RELATED, NOT COVERING: the open issue 20261022T031509Z-xray-cloudformation-never-provisions-aws-xray-group-aws-xray.md describes the same area at commit 5598afbc (its Cause 2 was 'DescribeStacks returns a Stack with no Outputs at all'). The replay has MOVED: Outputs now exist but their entries lack OutputValue, which is a different failure key, and the suite did not mark these three checks as owned by that issue. They are filed here so neither cause is closed unrun; a fixer should reconcile with that issue.

Cause 1 of 2: an AWS::XRay::SamplingRule declared in a template is still never created, while the stack reports CREATE_COMPLETE.
AWS: CreateStack on a template whose resource is {"Type": "AWS::XRay::SamplingRule", "Properties": {"SamplingRule": {RuleName, ResourceARN, Priority, FixedRate, ReservoirSize, ServiceName, ServiceType, Host, HTTPMethod, URLPath, Version}}} invokes X-Ray CreateSamplingRule during stack create; once the stack reaches CREATE_COMPLETE, GetSamplingRules returns a SamplingRuleRecord for that RuleName carrying the template's FixedRate/ReservoirSize/Priority and a populated RuleARN, and deleting the stack removes it. Smithy (services/service-com-amazonaws-xray-awsxray/smithy-model/com-amazonaws-xray-awsxray.json): CreateSamplingRule -> SamplingRuleRecord{SamplingRule{RuleName, RuleARN, FixedRate, ReservoirSize, ...}}; GetSamplingRules -> SamplingRuleRecords. The CFN resource-type-to-API mapping is recorded behavior only.
Emulator: the stack_create_complete waiter returns, then a full paginated walk of GetSamplingRules holds NO record for the template's RuleName — the resource was never created, so the stack create is a status flip with no effect.
Why: a client that provisions its sampling rules by stack gets CREATE_COMPLETE and then an account with no such rule: GetSamplingRules cannot see it, GetSamplingTargets can never serve an allocation for it, and the sampling behavior the deploy was supposed to establish silently never takes effect.
Fix: a resource provider for AWS::XRay::SamplingRule in the CloudFormation stack-resource provisioning layer (services/service-com-amazonaws-cloudformation-cloudformation), calling the X-Ray CreateSamplingRule/DeleteSamplingRule handlers on stack create and delete.
Traces: xray.deploy-xray-sampling-rule-cfn-then-serves-targets (asserts 'template-defined sampling rule not found via GetSamplingRules' after the waiter returned). The same cause also starves xray.deploy-sampling-rule-via-cloudformation-active, which polls GetSamplingRules for the rule until it is cut off by the harness ceiling; it is not listed in trace_ids because its own poll budget exceeds that ceiling and it cannot confirm a fix.

Cause 2 of 2: stack Outputs for X-Ray resources are emitted as entries with an OutputKey and NO OutputValue, so Fn::GetAtt never resolves to a value.
AWS: DescribeStacks returns, for a stack declaring `Outputs: {GroupARN: {Value: !GetAtt Group.GroupARN}}` or `{RuleARN: {Value: !GetAtt Rule.RuleARN}}`, one Output per declared output with BOTH OutputKey and OutputValue populated; the value is the provisioned resource's ARN, which embeds the GroupName / RuleName. Recorded behavior for the attribute names; the Outputs member itself is modeled on DescribeStacks' Stack shape.
Emulator: `{o["OutputKey"]: o["OutputValue"] for o in desc.get("Outputs", [])}` raises KeyError: 'OutputValue' — the Outputs list is now present and carries an entry per declared output, but the entry has no OutputValue member at all, for both AWS::XRay::Group and AWS::XRay::SamplingRule GetAtt outputs. (This is a different observation from the previously filed 'no Outputs key whatsoever'.)
Why: outputs are the documented contract by which a stack hands identifiers to its consumers; a client reading an ARN out of a stack output gets an entry with no value instead of the ARN, so cross-stack wiring and every post-deploy lookup by ARN is impossible, and the half-populated Output is worse than an absent one because it looks like a successfully evaluated output.
Fix: output evaluation in the CloudFormation stack-create/describe path — resolve Ref and Fn::GetAtt against the provisioned resource's attributes and serialize OutputValue on every Output in the DescribeStacks response (services/service-com-amazonaws-cloudformation-cloudformation).
Traces: xray.deploy-xray-group-cfn-filter-behaves, xray.deploy-xray-sampling-rule-cfn-applies.

## Evidence
Cause 1: xray.deploy-xray-sampling-rule-cfn-then-serves-targets — after `cfn.get_waiter("stack_create_complete").wait(...)` returned, `assert record is not None` raised AssertionError: template-defined sampling rule not found via GetSamplingRules (the helper walks every page of GetSamplingRules). Expected a SamplingRuleRecord whose SamplingRule.RuleName is the template's RuleName, with FixedRate 0.31 / ReservoirSize 7 as declared.
Cause 2: xray.deploy-xray-group-cfn-filter-behaves — `return {o["OutputKey"]: o["OutputValue"] for o in desc.get("Outputs", [])}` raised KeyError: 'OutputValue' for a template declaring `Outputs: GroupARN: Value: !GetAtt Group.GroupARN`. xray.deploy-xray-sampling-rule-cfn-applies — the identical KeyError: 'OutputValue' in its own _deploy_cfn for `Outputs: RuleARN: Value: !GetAtt Rule.RuleARN`. Expected each Output to carry OutputValue = the resource ARN embedding the declared GroupName / RuleName.

## Steps to Reproduce
1. Boot the emulator (run_behavioral_suite builds and boots it).
2. Cause 1: cfn.create_stack with a template whose only resource is {"Type": "AWS::XRay::SamplingRule", "Properties": {"SamplingRule": {"RuleName": "r1", "ResourceARN": "*", "Priority": 9000, "FixedRate": 0.31, "ReservoirSize": 7, "ServiceName": "*", "ServiceType": "*", "Host": "*", "HTTPMethod": "*", "URLPath": "*", "Version": 1}}}; wait for stack_create_complete; then walk every page of xray.get_sampling_rules() — observe no record for "r1".
3. Cause 2: add `Outputs: {"RuleARN": {"Value": {"Fn::GetAtt": ["Rule", "RuleARN"]}}}` to that template (and separately an AWS::XRay::Group template with `!GetAtt Group.GroupARN`), create the stack, and inspect cfn.describe_stacks(StackName=...)["Stacks"][0]["Outputs"] — each entry has OutputKey but no OutputValue.
4. Re-replay the three check ids in trace_ids to confirm the fixes.
