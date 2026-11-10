Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.nested-stack-parentage-and-cascade-delete
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.nested-stack-parentage-and-cascade-delete|run#28|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in parent_obj.get("Outputs", [])}
Issue-Commit: 22d8facbc3b601c7c60aed2bc6871542e8162809

# cloudformation: a nested stack's parent returns Outputs rows with OutputKey but no OutputValue, so Fn::GetAtt <Child>.Outputs.<Key> and Ref on an AWS::CloudFormation::Stack cannot be read back

> _Note: this issue was identified against the code base at commit `22d8facbc3b601c7c60aed2bc6871542e8162809`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
1 net-new root cause on the nested-stack output path.

Cause 1 of 1: DescribeStacks on a CREATE_COMPLETE parent stack returns its Outputs list with the OutputKey member present and the OutputValue member absent, for both an output built from Fn::GetAtt [<NestedStackLogicalId>, "Outputs.<ChildOutputKey>"] and one built from Ref on the AWS::CloudFormation::Stack resource.

AWS: Smithy cloudformation#Stack.Outputs is a list of cloudformation#Output, whose members are OutputKey ("The key associated with the output"), OutputValue ("The value associated with the output"), Description and ExportName. For every output of a stack that has reached CREATE_COMPLETE, AWS populates OutputValue with the resolved value: Fn::GetAtt on a nested AWS::CloudFormation::Stack with the attribute "Outputs.<Key>" resolves to the child stack's output of that name, and Ref on that resource resolves to the child stack's ARN. DescribeStacks models no errors and returns 200.

Emulator: the parent stack reaches CREATE_COMPLETE, but its Outputs rows are serialized without OutputValue at all — reading o["OutputValue"] over the returned rows raises KeyError: 'OutputValue'. The values that should be there are the child stack's resolved output (the nested template's Fn::GetAtt ChildParam.Value) and the child stack's ARN.

Why: every consumer of stack outputs breaks on the canonical nested-stack cross-reference. A client that deploys a parent/child pair and reads the parent's outputs (CLI `--query 'Stacks[0].Outputs'`, CDK cross-stack wiring, any SDK code building a dict keyed by OutputKey) gets an entry with no value, or a KeyError, instead of the child's value and the child stack ARN — so nothing downstream of a nested stack can be addressed.

Fix: the DescribeStacks output serializer together with the output-resolution step of the stack provisioner — resolve each Output's Value intrinsic (including Fn::GetAtt <NestedStack>.Outputs.<Key> and Ref on an AWS::CloudFormation::Stack) when the stack settles and emit it as Output.OutputValue.

Related open issues describe the same OutputValue omission on other paths but do not carry this check's replay key: 20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md, 20261107T085452Z-cloudformation-8-net-new-behavioral-causes-stack-outputs.md and 20261108T052651Z-cloudformation-3-net-new-causes-the-number-parameter.md.

Traces: cloudformation.nested-stack-parentage-and-cascade-delete.

## Evidence
Expected (AWS): DescribeStacks(parent) returns Outputs == [{OutputKey: "FromChild", OutputValue: "<seed>-child"}, {OutputKey: "ChildRef", OutputValue: "<child stack ARN>"}] — the Fn::GetAtt Child.Outputs.ChildValue output carries the child stack's resolved SSM parameter value and the Ref output carries the child stack ARN. (The seed string and the ARN are per-run values; the requirement is that OutputValue is present and resolved.)

Actual (emulator): the parent reaches CREATE_COMPLETE but each Outputs row has no OutputValue member:
  File "check_cloudformation_nested_stack_parentage_and_cascade_delete.py", line 187, in run
    outputs = {o["OutputKey"]: o["OutputValue"] for o in parent_obj.get("Outputs", [])}
  KeyError: 'OutputValue'

No error code or status diverges here — DescribeStacks answers 200; the divergence is the missing modeled member cloudformation#Output$OutputValue.

## Steps to Reproduce
1. Put a child template in S3 that declares an AWS::SSM::Parameter and exports Outputs ChildValue (Fn::GetAtt ChildParam.Value) and ChildParamName (Ref ChildParam).
2. CreateStack a parent template whose single resource is an AWS::CloudFormation::Stack pointing TemplateURL at that object, with Outputs FromChild = Fn::GetAtt [Child, "Outputs.ChildValue"] and ChildRef = Ref Child.
3. Wait for the parent to reach CREATE_COMPLETE, then call DescribeStacks on the parent and inspect Stacks[0].Outputs.
4. Every row has OutputKey but no OutputValue; AWS returns both.
5. Re-replay cloudformation.nested-stack-parentage-and-cascade-delete to confirm the fix.
