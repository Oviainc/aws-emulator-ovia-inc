Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.asg-registration-fails-with-mismatched-target-group-vpc autoscaling__ec2__elbv2.asghc-attach-a-nonexistent-target-group-rejected autoscaling__ec2__elbv2.asghc-target-groups-attached-through-both-apis-appear-in-both-views
Issue-Key: autoscaling__ec2__elbv2.asghc-attach-a-nonexistent-target-group-rejected|run#11|AssertionError|assert got == want, f"legacy attach of a missing group was answered {got!r}, expected {want!r}"
Issue-Related: 20261104T042502Z-autoscaling-attachloadbalancertargetgroups-stub.md
Issue-Commit: 620c90c23a321396910fd66a692ef2fcecf33f7f

# Implement auto-scaling AttachLoadBalancerTargetGroups: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `620c90c23a321396910fd66a692ef2fcecf33f7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AttachLoadBalancerTargetGroups

## Description
`AttachLoadBalancerTargetGroups` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/attach_load_balancer_target_groups.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `AttachLoadBalancerTargetGroups` call, so each one unblocks only when `AttachLoadBalancerTargetGroups` works: `autoscaling__ec2__elbv2.asg-registration-fails-with-mismatched-target-group-vpc`, `autoscaling__ec2__elbv2.asghc-attach-a-nonexistent-target-group-rejected`, `autoscaling__ec2__elbv2.asghc-target-groups-attached-through-both-apis-appear-in-both-views`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/attach_load_balancer_target_groups.rs`. Replaying the corpus, the emulator refused the `AttachLoadBalancerTargetGroups` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AttachLoadBalancerTargetGroups operation: operation AttachLoadBalancerTargetGroups is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the auto-scaling emulator
Call AttachLoadBalancerTargetGroups and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
