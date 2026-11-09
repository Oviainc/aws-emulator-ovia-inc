Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudformation.deploy-asg-lifecyclehook-scheduledaction-warmpool-provisioned-and-deleted autoscaling__cloudformation.deploy-asg-warmpool-poolstate-invalid-enum-rejected
Issue-Key: autoscaling__cloudformation.deploy-asg-lifecyclehook-scheduledaction-warmpool-provisioned-and-deleted|stack_outputs#10|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: autoscaling__cloudformation.deploy-asg-warmpool-poolstate-invalid-enum-rejected|run#9|AssertionError|assert [g["AutoScalingGroupName"] for g in groups] == [stack_outputs(cf, good_id)["AsgRef"]], (
Issue-Commit: d38d8ab45cd7905c8c6149b13ae22553e33f698d

# autoscaling__cloudformation: 2 net-new causes — DescribeStacks Outputs entries carry no OutputValue, and a stack-created AWS::AutoScaling::AutoScalingGroup is invisible to DescribeAutoScalingGroups

> _Note: this issue was identified against the code base at commit `d38d8ab45cd7905c8c6149b13ae22553e33f698d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Two distinct defects found by the behavioral replay on the autoscaling__cloudformation checks. Both are net-new: the replay bound neither check to an open issue.

Cause 1 of 2: a stack that declares Outputs returns Output entries with no OutputValue member at all.
AWS: recorded behavior only — the recording shows DescribeStacks on a CREATE_COMPLETE stack returning, for every declared Output, an entry carrying both OutputKey and OutputValue (the resolved value of the template's Fn::Ref/Fn::GetAtt expression). An Output that CloudFormation has resolved always has an OutputValue; the member is only absent before the value exists, which cannot be true of a stack that already reported CREATE_COMPLETE.
Emulator: the Outputs list is served with entries that have an OutputKey but NO OutputValue key, so any client that reads o['OutputValue'] raises. The replay shows KeyError: 'OutputValue' raised while building {o['OutputKey']: o['OutputValue'] for o in stack.get('Outputs', [])}.
Why: stack outputs are the only supported way to hand a stack's generated physical ids (here, the ASG name) to the caller. With no OutputValue, every consumer of a stack's outputs — the CLI, CDK, nested stacks, cross-stack Fn::ImportValue, and ordinary SDK callers — either crashes on the missing key or sees the output as empty, so nothing downstream of a deploy can be addressed.
Fix: the CloudFormation DescribeStacks serializer and the stack-output resolution in the create/update path — resolve each declared Output's expression at stack completion and emit OutputKey + OutputValue (plus Description/ExportName when declared) in the response. Related but NOT covering these checks: open issues 20261029T172317Z-cloudformation-a-stack-deploying-an-auto-scaling-group.md and 20261024T015148Z-cloudformation-stack-outputs-are-unusable-and-create-complete.md describe the same symptom under the cloudformation service token; these two checks are listed under autoscaling__cloudformation and were not bound to either, so they are carried here.
Traces: autoscaling__cloudformation.deploy-asg-lifecyclehook-scheduledaction-warmpool-provisioned-and-deleted.

Cause 2 of 2: an Auto Scaling group that a stack created is not returned by DescribeAutoScalingGroups.
AWS: recorded behavior only — the recording shows that once a stack containing an AWS::AutoScaling::AutoScalingGroup reaches CREATE_COMPLETE, the group exists in the Auto Scaling service: DescribeAutoScalingGroups returns exactly that group, and its AutoScalingGroupName equals the stack output that exports the group's ref. Resource creation through CloudFormation and through the CreateAutoScalingGroup API land in the same state.
Emulator: DescribeAutoScalingGroups returns an empty AutoScalingGroups list for the control stack's group, even though the stack reported a terminal successful status ("AssertionError: DescribeAutoScalingGroups found [] for the control"). The CFN resource provisioning either never calls into Auto Scaling state or writes to state the Auto Scaling describe path does not read.
Why: a client that provisions infrastructure with CloudFormation and then operates it through the service APIs (attach a warm pool, set desired capacity, read instances) finds the resource does not exist, while the stack claims success. The divergence is silent: the deploy looks healthy and every follow-up call behaves as if nothing was created.
Fix: the CloudFormation resource provisioner for AWS::AutoScaling::AutoScalingGroup — it must create the group in the shared Auto Scaling state that DescribeAutoScalingGroups reads, under the name the stack's ref/output reports.
Traces: autoscaling__cloudformation.deploy-asg-warmpool-poolstate-invalid-enum-rejected.

## Evidence
Cause 1 — autoscaling__cloudformation.deploy-asg-lifecyclehook-scheduledaction-warmpool-provisioned-and-deleted
  Expected: after wait_stack() returns a terminal status, DescribeStacks(StackName=<stack_id>)['Stacks'][0]['Outputs'] contains entries with both OutputKey and OutputValue, so {o['OutputKey']: o['OutputValue'] ...} builds.
  Actual: KeyError: 'OutputValue'
    at check_...py line 222 -> stack_outputs(cf, stack_id)
    at check_...py line 109 -> return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}

Cause 2 — autoscaling__cloudformation.deploy-asg-warmpool-poolstate-invalid-enum-rejected
  Expected: [g['AutoScalingGroupName'] for g in DescribeAutoScalingGroups pages] == [stack_outputs(cf, good_id)['AsgRef']] — the control stack's ASG is visible with the name the stack exported.
  Actual: AssertionError: DescribeAutoScalingGroups found [] for the control (check_...py line 234).

## Steps to Reproduce
Replay the behavioral suite and re-run exactly these checks:

Cause 1 — autoscaling__cloudformation.deploy-asg-lifecyclehook-scheduledaction-warmpool-provisioned-and-deleted
  1. CreateStack with a template declaring an AWS::AutoScaling::AutoScalingGroup (plus a LifecycleHook, a ScheduledAction and a WarmPool) and an Outputs section exporting the group's ref.
  2. Poll DescribeStacks(StackName=<StackId>) until StackStatus is terminal.
  3. Read Stacks[0]['Outputs'] and index each entry's 'OutputValue'. Expect a resolved value per declared output; observe KeyError: 'OutputValue'.

Cause 2 — autoscaling__cloudformation.deploy-asg-warmpool-poolstate-invalid-enum-rejected
  1. CreateStack with the valid (control) template containing an AWS::AutoScaling::AutoScalingGroup and a WarmPool with a valid PoolState, and an output 'AsgRef' for the group.
  2. Wait for the stack to reach a terminal status.
  3. Page DescribeAutoScalingGroups and compare the returned AutoScalingGroupName list to ['<AsgRef output>']. Expect the one group; observe [].
