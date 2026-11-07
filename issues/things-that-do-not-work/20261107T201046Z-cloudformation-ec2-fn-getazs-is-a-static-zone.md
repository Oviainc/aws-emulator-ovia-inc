Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2.getazs-resolves-to-and-tracks-the-default-subnet-set
Issue-Key: cloudformation__ec2.getazs-resolves-to-and-tracks-the-default-subnet-set|run#20|AssertionError|assert mutated == expected_after, (
Issue-Commit: 47509ee38907103620e4b84df19d39bf76901d44

# cloudformation__ec2: Fn::GetAZs is a static zone list — it does not follow the account's default-subnet set when a default subnet is removed

> _Note: this issue was identified against the code base at commit `47509ee38907103620e4b84df19d39bf76901d44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: Fn::GetAZs resolves from a fixed region zone list instead of from the account's current default-subnet state.
AWS: Fn::GetAZs returns the Availability Zones of the given region in which the ACCOUNT holds a default subnet (AWS CloudFormation Fn::GetAZs documentation; ec2:DescribeSubnets with the default-for-AZ attribute is the authority). Because it is derived state, it TRACKS that set: after a default subnet is deleted in one zone, a template resolved afterwards returns the remaining zones — one fewer element, the deleted zone absent — and conversely gains a zone when a default subnet is added. Fn::GetAZs "" and Fn::GetAZs <region> are identical, and the result is a list (so Fn::Select[0] yields its first element). Recorded behavior for the tracking claim; the Smithy model does not describe intrinsics.
Emulator: the first resolution matches the account's default-subnet zones, but after the default subnet in one zone is deleted (and ec2:DescribeSubnets confirms that zone no longer has one) Fn::GetAZs still returns the original three zones, i.e. the value is a static region list rather than a projection of the account's default subnets.
Why: every template that sizes or places resources with Fn::GetAZs (subnet-per-AZ loops, Fn::Select over the zone list, ASG availability zones) is handed zones the account cannot actually place a default-subnet-backed resource in. A deploy that AWS would spread over the surviving zones instead references a zone with no default subnet, and the resource create fails — or silently lands somewhere the template did not intend — with no way for the client to detect the stale list.
Fix: the CloudFormation intrinsic-function evaluator for Fn::GetAZs — resolve it from the live EC2 default-subnet state (per region) at template-resolution time instead of a static zone table.
Traces: cloudformation__ec2.getazs-resolves-to-and-tracks-the-default-subnet-set.

## Evidence
From the behavioral replay — cloudformation__ec2.getazs-resolves-to-and-tracks-the-default-subnet-set [mismatch]:
  assert mutated == expected_after, (...)
  AssertionError: a subtractive change to the default subnet in 'us-west-2c' must move Fn::GetAZs: expected ['us-west-2a', 'us-west-2b'], got ['us-west-2a', 'us-west-2b', 'us-west-2c'] (it was ['us-west-2a', 'us-west-2b', 'us-west-2c'])

The earlier beats passed: the baseline Fn::GetAZs value equalled the zones ec2:DescribeSubnets reports as holding a default subnet, Fn::GetAZs "" equalled Fn::GetAZs <region>, and Fn::Select[0] returned the list's first element. After the default subnet in one zone is deleted (and the EC2 view is confirmed to have dropped that zone), the resolved list is unchanged — the value does not derive from the account's default-subnet state.

## Steps to Reproduce
Replay the behavioral suite and re-run cloudformation__ec2.getazs-resolves-to-and-tracks-the-default-subnet-set. Manually: ec2:DescribeSubnets to list the zones holding a default subnet; deploy a stack whose Outputs are {"Fn::GetAZs": ""}, {"Fn::GetAZs": <region>} and {"Fn::Select": [0, {"Fn::GetAZs": ""}]} and record them; delete the default subnet in one of those zones; re-deploy/ re-resolve the same template and compare the resolved zone list with ec2:DescribeSubnets (it must have lost exactly that zone).
