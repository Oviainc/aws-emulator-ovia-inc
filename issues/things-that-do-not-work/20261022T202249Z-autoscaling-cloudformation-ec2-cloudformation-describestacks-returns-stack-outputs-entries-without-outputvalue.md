Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudformation__ec2.cfn-asg-availability-zones-derived-from-subnets autoscaling__cloudformation__ec2.cfn-rolling-update-honours-max-batch-size-then-rolls-back-on-a-failed-signal
Issue-Key: autoscaling__cloudformation__ec2.cfn-asg-availability-zones-derived-from-subnets|_deploy_cfn#6|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: autoscaling__cloudformation__ec2.cfn-rolling-update-honours-max-batch-size-then-rolls-back-on-a-failed-signal|_deploy_cfn#9|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Commit: dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c

# CloudFormation DescribeStacks returns stack Outputs entries without OutputValue, so a CREATE_COMPLETE stack exposes no resolved output values

> _Note: this issue was identified against the code base at commit `dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
One root cause, surfaced by two cross-service CloudFormation deploy checks in the autoscaling__cloudformation__ec2 group. Both deploy a stack, poll DescribeStacks until CREATE_COMPLETE, and then build a {OutputKey: OutputValue} map from Stacks[0].Outputs. The Outputs member IS present (iteration proceeds), but the individual Output structures carry no OutputValue member, so the map comprehension raises KeyError: 'OutputValue'.

Cause 1 of 1: DescribeStacks serializes Stack.Outputs entries without the OutputValue member.
AWS: DescribeStacks returns, per stack, a list of Output structures. Smithy (com.amazonaws.cloudformation): DescribeStacks output member Stacks -> Stack, Stack.Outputs -> Outputs (list of Output); shape Output has members OutputKey ('The key associated with the output'), OutputValue ('The value associated with the output'), Description, ExportName. For a stack that has reached CREATE_COMPLETE, AWS emits one Output entry per output declared in the template's Outputs section, each entry carrying both OutputKey and the RESOLVED OutputValue (the result of the output's Value expression, e.g. !Ref / Fn::GetAtt against the created resource). Recorded behavior only for the specific keys/values of these templates; the requirement is that every returned Output entry carries OutputValue, resolved, once the stack is CREATE_COMPLETE.
Emulator: DescribeStacks(StackName=<stack>) on a stack the emulator itself reports as CREATE_COMPLETE returns Stacks[0].Outputs entries that omit OutputValue entirely — `{o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}` raises KeyError: 'OutputValue' in both checks (at the shared _deploy_cfn helper, immediately after the CREATE_COMPLETE assertion passed). Output resolution is therefore not performed (or its result is not stored/serialized), while the stack status still claims success.
Why: stack outputs are the only supported way for a client to learn the physical ids/attributes of the resources CloudFormation created. A client that deploys a stack and then reads Stacks[0].Outputs[*].OutputValue — which every IaC-driven workflow, CLI script and SDK consumer does — crashes or sees an output with a key and no value, even though the emulator reported CREATE_COMPLETE. The failure is silent at deploy time and only appears on the first read after it.
Fix: the CloudFormation DescribeStacks handler/serializer for Stack.Outputs, and the stack state that should record each declared output's resolved value at create/update time (output expression resolution: !Ref / Fn::GetAtt against the stack's provisioned resources).
Traces: autoscaling__cloudformation__ec2.cfn-asg-availability-zones-derived-from-subnets (helper line 298: outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])} -> KeyError: 'OutputValue'), autoscaling__cloudformation__ec2.cfn-rolling-update-honours-max-batch-size-then-rolls-back-on-a-failed-signal (same helper, line 608 -> KeyError: 'OutputValue').

Relation to already-open issues (none of which covers this): 20261022T043627Z-fis-describestacks-omits-the-outputs-member-for-a-stack.md covers the Outputs MEMBER being absent from the Stack entirely (KeyError: 'Outputs'); here the member is present and the per-entry OutputValue is the one missing. 20261022T154846Z-ec2-emr-cloudformation-stack-outputs-return-synthesized-logical-names-instead.md and 20261022T154846Z-ec2-emr-iam-cloudformation-network-stack-outputs-carry-synthesized-logical-names.md cover OutputValue being present but carrying a synthesized logical name instead of a real physical id — again a present-but-wrong value, not an omitted member. The suite marked neither of these two checks as OWNED or ALREADY FILED.

## Evidence
Check autoscaling__cloudformation__ec2.cfn-asg-availability-zones-derived-from-subnets:
  File "check_autoscaling__cloudformation__ec2_cfn_asg_availability_zones_derived_from_subnets.py", line 298, in _deploy_cfn
    outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
  KeyError: 'OutputValue'

Check autoscaling__cloudformation__ec2.cfn-rolling-update-honours-max-batch-size-then-rolls-back-on-a-failed-signal:
  File "...cfn_rolling_update_honours_max_batch_size_then_rolls_back_on_a_failed_signal.py", line 608, in _deploy_cfn
    outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
  KeyError: 'OutputValue'

Control flow proving the stack was reported healthy first (shared _deploy_cfn helper):
  _poll_until(lambda: cfn.describe_stacks(StackName=name)["Stacks"][0]["StackStatus"], lambda s: s in {"CREATE_COMPLETE","CREATE_FAILED","ROLLBACK_COMPLETE","ROLLBACK_FAILED"}, ...)
  stack = cfn.describe_stacks(StackName=name)["Stacks"][0]
  if stack["StackStatus"] != "CREATE_COMPLETE": raise AssertionError(...)   # did NOT fire
  outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}   # <-- KeyError here

Expected (AWS): Stacks[0].Outputs == [{'OutputKey': '<declared output name>', 'OutputValue': '<resolved value>'}, ...] — one entry per declared template output, each with OutputValue present.
Actual (emulator): Stacks[0].Outputs entries exist but contain no 'OutputValue' member.
Smithy citation: com.amazonaws.cloudformation#Output members OutputKey, OutputValue, Description, ExportName; com.amazonaws.cloudformation#Stack member Outputs -> Outputs; DescribeStacks output Stacks -> Stacks.

## Steps to Reproduce
1. Boot the emulator.
2. With a botocore cloudformation client, create_stack a template that declares an Outputs section whose values reference stack resources (e.g. an AWS::AutoScaling::AutoScalingGroup with VPCZoneIdentifier plus `Outputs: AsgName: {Value: !Ref Asg}`), passing Capabilities ['CAPABILITY_IAM','CAPABILITY_NAMED_IAM'].
3. Poll describe_stacks(StackName=<name>) until Stacks[0].StackStatus == 'CREATE_COMPLETE'.
4. Read Stacks[0]['Outputs'] — the list is returned, but each entry has only 'OutputKey' (no 'OutputValue'), so `{o['OutputKey']: o['OutputValue'] for o in stack['Outputs']}` raises KeyError: 'OutputValue'.
Equivalently, re-replay the two checks listed in trace_ids.
