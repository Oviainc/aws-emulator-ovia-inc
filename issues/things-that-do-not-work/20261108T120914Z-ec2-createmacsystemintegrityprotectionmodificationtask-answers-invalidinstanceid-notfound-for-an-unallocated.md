Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.create-mac-sip-modification-task-rejected-unknown-instance ec2.ec2mr-byvpc-e2e-multi-vpc-grouping-lifecycle ec2.ec2mr-pip-e2e-multi-eni-inventory-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.create-mac-sip-modification-task-rejected-unknown-instance|run#7|AssertionError|assert code_random == "InvalidInstanceID.Malformed", (
Issue-Key: ec2.ec2mr-byvpc-e2e-multi-vpc-grouping-lifecycle|phase#10|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: ec2.ec2mr-pip-e2e-multi-eni-inventory-lifecycle|phase#10|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 6596661b7bed7b2c68eacfc83c0e7a703969540b

# ec2: CreateMacSystemIntegrityProtectionModificationTask answers InvalidInstanceID.NotFound for an unallocated id, and a VPC/instance stack's delete never reaches DELETE_COMPLETE (2 causes)

> _Note: this issue was identified against the code base at commit `6596661b7bed7b2c68eacfc83c0e7a703969540b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMacSystemIntegrityProtectionModificationTask

## Description
Two net-new root causes under '### ec2'. Neither is marked OWNED or ALREADY FILED by the replay report.

Cause 1 of 2: a well-formed but unallocated instance id is refused with the wrong error code and message on CreateMacSystemIntegrityProtectionModificationTask.
AWS: this operation refuses ANY instance id it cannot resolve — the all-zero placeholder and a random, well-formed, unallocated i-<17 hex> alike — with error code InvalidInstanceID.Malformed and the message "The instance ID '<id>' is malformed"; only an existing non-Mac instance gets a different code. Fixed text: "The instance ID '" ... "' is malformed"; the variable part is the InstanceId copied verbatim from the request. Smithy: CreateMacSystemIntegrityProtectionModificationTask declares InstanceId (target InstanceId, required) and carries no modeled error shapes (ec2Query), so the code/message are recorded behavior.
Emulator: the all-zero placeholder is refused correctly, but a random well-formed unallocated id is refused InvalidInstanceID.NotFound instead — 'AssertionError: expected InvalidInstanceID.Malformed for a well-formed unallocated id, got \'InvalidInstanceID.NotFound\''. The emulator classifies by "does this id exist" where AWS classifies by "can this id be resolved on this operation at all".
Why: clients branch on EC2 error codes. Code that retries or re-resolves on InvalidInstanceID.NotFound (a transient, eventually-consistent condition) but fails fast on InvalidInstanceID.Malformed (a programming error) takes the wrong branch, and the message a caller logs says the opposite of AWS's.
Fix: the operation's instance-id validation/refusal path must emit InvalidInstanceID.Malformed with the malformed message for every unresolvable id, not only the all-zero placeholder (this operation's handler and its error classification).
Traces: ec2.create-mac-sip-modification-task-rejected-unknown-instance.

Cause 2 of 2: deleting a CloudFormation stack that owns a VPC, subnets and instances never reaches a terminal DELETE_COMPLETE.
AWS: DeleteStack on a stack whose resources are a VPC, subnet(s), security group(s) and EC2 instance(s) proceeds to DELETE_COMPLETE once nothing out-of-band holds the subnet — the instances terminate, their interfaces are released, the subnet and VPC are deleted — and DescribeStacks then reports the stack as absent (ValidationError, "Stack with id <name> does not exist"); a repeat DeleteStack is a clean no-op. Where an out-of-band interface does remain in the subnet, AWS reaches the terminal status DELETE_FAILED rather than staying non-terminal. Recorded behavior for the convergence; the CFN model states only the shapes.
Emulator: both e2e lifecycle checks pass deploy, solve and cleanup (including the assertion that the VPC holds no interfaces at all after cleanup) and then fail in the destroy phase: '[phase 4/4 destroy] stack deletion did not reach DELETE_COMPLETE — if it reports DELETE_FAILED on a subnet, cleanup left an interface behind'. The stack reaches neither GONE nor DELETE_FAILED inside the check's 600 s poll, so the delete neither finishes nor fails.
Why: a client cannot tear down an EC2 estate it built with CloudFormation. The stack and every resource under it leak, a second deploy of the same template collides on them, and because there is no terminal status the caller has nothing to react to — not even a failure it could repair.
Fix: the stack-delete executor's EC2 resource teardown (instance termination, interface release, subnet and VPC deletion) must drive the stack to a terminal status, and the terminal failure case must be reported DELETE_FAILED rather than left pending (stack lifecycle state plus the EC2 delete providers).
Traces: ec2.ec2mr-byvpc-e2e-multi-vpc-grouping-lifecycle, ec2.ec2mr-pip-e2e-multi-eni-inventory-lifecycle.

## Evidence
Cause 1 — Expected: CreateMacSystemIntegrityProtectionModificationTask(InstanceId='i-<17 random hex>', MacSystemIntegrityProtectionStatus='Disabled') is refused InvalidInstanceID.Malformed with message "The instance ID '<that id>' is malformed". Actual: error code 'InvalidInstanceID.NotFound' ('AssertionError: expected InvalidInstanceID.Malformed for a well-formed unallocated id, got \'InvalidInstanceID.NotFound\''). The all-zero id 'i-00000000000000000' is refused correctly in the same check, so only the unallocated-but-well-formed branch diverges.
Cause 2 — Expected: after cleanup leaves the VPC with no interfaces, DeleteStack drives the stack to DELETE_COMPLETE (DescribeStacks then answers ValidationError 'Stack with id <stack> does not exist') and a second destroy is a no-op. Actual: 'AssertionError: [phase 4/4 destroy] stack deletion did not reach DELETE_COMPLETE — if it reports DELETE_FAILED on a subnet, cleanup left an interface behind' in both checks; the status poll (600 s, terminal states GONE/DELETE_FAILED) never completes.

## Steps to Reproduce
1. Replay the behavioral suite and read the failures under '### ec2'.
2. Cause 1: call CreateMacSystemIntegrityProtectionModificationTask with a random well-formed instance id ('i-' + 17 hex chars) that was never allocated and MacSystemIntegrityProtectionStatus='Disabled'; observe InvalidInstanceID.NotFound instead of InvalidInstanceID.Malformed.
3. Cause 2: CreateStack a template with two VPCs, a subnet, a security group and two EC2 instances; after it reaches CREATE_COMPLETE, delete every out-of-band interface so the VPC holds none, then DeleteStack and poll DescribeStacks — the stack reaches neither DELETE_COMPLETE nor DELETE_FAILED.
