Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.e2e-ec2mr-xrg-cross-region-connectivity-lifecycle ec2.ec2mr-inv2-e2e-ssh-reachable-instances-lifecycle ec2.e2e-route-server-bgp-propagation-flow ec2.shutdown-in-guest-halt-with-terminate-behavior-terminates-the-instance
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.e2e-ec2mr-xrg-cross-region-connectivity-lifecycle|phase#6|_PhaseError|raise _PhaseError(
Issue-Key: ec2.ec2mr-inv2-e2e-ssh-reachable-instances-lifecycle|phase#6|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: ec2.e2e-route-server-bgp-propagation-flow|_poll_until#37|Timeout|time.sleep(interval_s)
Issue-Key: ec2.shutdown-in-guest-halt-with-terminate-behavior-terminates-the-instance|_poll#12|Timeout|last = probe()
Issue-Commit: 563433aa8ea88fa1a8e8ad3e10fc27d869dea4a3

# ec2: 4 net-new causes where multi-step EC2 scenarios never converge — stack outputs carry no OutputValue, an EC2 estate stack rolls back, route-server BGP propagation never settles, and an in-guest halt never terminates the instance

> _Note: this issue was identified against the code base at commit `563433aa8ea88fa1a8e8ad3e10fc27d869dea4a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Four independent defects, each grounded in a failing behavioral check listed under `### ec2` that the replay report marks neither ALREADY FILED nor OWNED. Open issues that look adjacent but do NOT carry these replay keys and therefore do not cover them: 20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md and 20261024T015148Z-cloudformation-stack-outputs-are-unusable-and-create-complete.md (cause 1), 20261106T170358Z-ec2-route-server-route-table-diagnostic-interrupt-refusals.md (cause 3), 20261107T075601Z-ec2-createvpc-from-an-ipam-pool-is-a.md (cause 4).

Cause 1 of 4: a CREATE_COMPLETE EC2 stack's Outputs carry no OutputValue.
AWS: once a stack reaches CREATE_COMPLETE, DescribeStacks reports each declared output as a complete `Output` with `OutputKey` AND `OutputValue` (the resolved value of the template's `Value`, e.g. a VPC id, subnet id or security group id). An output that resolved is never returned with the value member absent.
Emulator: reading the deployed stack's outputs raises `KeyError: 'OutputValue'` — the Output entries exist but carry no value, so the scenario cannot learn the ids of the resources it just deployed and the whole cross-Region connectivity phase dies in phase 1 of 4 (deploy).
Why: stack outputs are the documented hand-off between a deployment and everything downstream; with no OutputValue, no client, script or IaC pipeline can consume a stack it successfully created.
Fix: output resolution/serialization on the DescribeStacks read path for the stack's resolved outputs.
Traces: ec2.e2e-ec2mr-xrg-cross-region-connectivity-lifecycle.

Cause 2 of 4: a valid EC2 estate stack (VPC/subnet/security group/instance for an SSH-reachability scenario) ends ROLLBACK_COMPLETE.
AWS: a template whose resources are all valid reaches CREATE_COMPLETE; a rollback happens only when a resource genuinely fails, and then the stack events name the failing resource and reason.
Emulator: the stack settles in `ROLLBACK_COMPLETE` instead of `CREATE_COMPLETE` in us-east-1 during phase 1 of 4 (deploy), so none of the estate exists and the scenario cannot proceed.
Why: a client cannot deploy a plain EC2 estate at all; the failure surfaces as a terminal rollback rather than as a usable stack, and nothing downstream of the deployment is reachable.
Fix: the resource provisioning path for this stack's EC2 resource types (the resource whose CREATE_FAILED drives the rollback is named in the stack events) and the CloudFormation create/rollback state machine.
Traces: ec2.ec2mr-inv2-e2e-ssh-reachable-instances-lifecycle.

Cause 3 of 4: route-server BGP propagation never settles, so the flow never reaches a verdict.
AWS: a route server, its endpoint and its peer each advance through their documented lifecycles to an available/established terminal state, and a prefix a peered device announces over eBGP appears in the route server's routing database and propagates to the associated route tables within the normal settling window. Every wait in the flow terminates.
Emulator: the end-to-end flow never converges — no state the scenario waits on becomes terminal, so the check exhausts its whole budget and is reported as `Check timed out after 180.0s` with no verdict.
Why: a client polling for the route server, endpoint, peer or learned route waits forever; nothing built on route-server propagation can be exercised against the emulator.
Fix: lifecycle progression and route propagation in the route-server state (CreateRouteServer/CreateRouteServerEndpoint/CreateRouteServerPeer state advance, GetRouteServerRoutingDatabase and route-table propagation reads).
Traces: ec2.e2e-route-server-bgp-propagation-flow.

Cause 4 of 4: an in-guest halt under InstanceInitiatedShutdownBehavior=terminate never terminates the instance.
AWS: when the guest itself halts/powers off and the instance's shutdown behavior is `terminate`, the instance leaves `running` and settles in `terminated` (with `stop` it settles in `stopped` and is restartable). DescribeInstances reports that terminal state within the normal settling window.
Emulator: the instance never reaches the terminal state the scenario waits on after the in-guest halt, so the check exhausts its budget and is reported as `Check timed out after 180.0s` with no verdict.
Why: a client cannot rely on guest-initiated shutdown reaching a terminal instance state, so any teardown, auto-terminate or self-destruct pattern hangs instead of completing.
Fix: the guest-initiated shutdown path — the instance-state machine must honour InstanceInitiatedShutdownBehavior when the guest powers off and publish the resulting state to DescribeInstances.
Traces: ec2.shutdown-in-guest-halt-with-terminate-behavior-terminates-the-instance.

## Evidence
Cause 1 — ec2.e2e-ec2mr-xrg-cross-region-connectivity-lifecycle [error]
  _PhaseError: [phase 1/4 deploy] KeyError: 'OutputValue'
  expected: DescribeStacks on the CREATE_COMPLETE stack returns each Output with both OutputKey and OutputValue
  actual:   the Output entries carry no OutputValue member

Cause 2 — ec2.ec2mr-inv2-e2e-ssh-reachable-instances-lifecycle [mismatch]
  AssertionError: [phase 1/4 deploy] us-east-1: stack 'sim2real-mrssh-e2e-<suffix>' reached 'ROLLBACK_COMPLETE', not CREATE_COMPLETE — read its stack events for the resource that failed

Cause 3 — ec2.e2e-route-server-bgp-propagation-flow [mismatch]
  Error: Check timed out after 180.0s
  expected: the route server, endpoint and peer reach their terminal states and the announced prefix appears in the routing database / propagated route table
  actual:   no waited-on state ever becomes terminal; the check is cut off at its 180s budget

Cause 4 — ec2.shutdown-in-guest-halt-with-terminate-behavior-terminates-the-instance [mismatch]
  Error: Check timed out after 180.0s
  expected: after an in-guest halt with InstanceInitiatedShutdownBehavior=terminate, DescribeInstances reports the instance 'terminated'
  actual:   the instance never leaves its pre-halt state; the check is cut off at its 180s budget

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite; the four checks below fail.
2. Cause 1: CreateStack for the cross-Region EC2 template, wait for CREATE_COMPLETE, then DescribeStacks and read Stacks[0].Outputs. Expect every Output to carry OutputValue; observe the member absent (KeyError).
3. Cause 2: CreateStack for the SSH-reachability EC2 estate template in us-east-1 and wait for a terminal status. Expect CREATE_COMPLETE; observe ROLLBACK_COMPLETE, then read DescribeStackEvents for the resource reported CREATE_FAILED.
4. Cause 3: CreateRouteServer / CreateRouteServerEndpoint / CreateRouteServerPeer, associate the route server with the VPC, run the in-VPC eBGP speaker that announces 192.0.2.0/24, and poll the route server states plus GetRouteServerRoutingDatabase and the propagated route table. Expect each to reach its terminal state and the prefix to appear.
5. Cause 4: RunInstances with InstanceInitiatedShutdownBehavior='terminate', wait running, halt the guest from inside, then poll DescribeInstances. Expect the instance to reach 'terminated'.
