Issue-Source: trace-sweep-divergence
Issue-Trace: elbv2err-create-load-balancer-input-rejections
Issue-Step: teardown#14
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbv2err-create-load-balancer-input-rejections
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: ec2|DeleteSubnet|0->None|$
Issue-Key: ec2|DeleteSubnet|0->None|status
Issue-Commit: 244ef584de6f4d4e7e65c275cb04e3258cdd44a3

# EC2 never releases a deleted load balancer's requester-managed ENIs, so subnet teardown never converges

> _Note: this issue was identified against the code base at commit `244ef584de6f4d4e7e65c275cb04e3258cdd44a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteSubnet

## Description
One cause: the EC2 network-interface state never reclaims the ENIs an ELB load balancer created, so a subnet that held a (now deleted) load balancer can never be deleted and the fixture's whole time budget is consumed before its last teardown step is ever issued.

Cause 1 of 1: DeleteSubnet of a subnet whose only occupants were an ELB's requester-managed ENIs never succeeds; the step is never answered at all because the fixture's 960s budget is spent first.
AWS: recorded behavior only (the EC2 Smithy model declares no errors for DeleteSubnet: DeleteSubnet input is {SubnetId required, DryRun}, output Unit). The recording shows the AWS sequence after the load balancer is deleted: the requester-managed `load_balancer` ENIs owned by `amazon-elb` (requesterId 'amazon-elb', requesterManaged 'true', description 'ELB app/<lb-name>/<lb-id>') are released, so DescribeNetworkInterfaces on the subnet filter converges to an empty networkInterfaceSet within the poll window and the following DeleteSubnet calls each answer HTTP 200 with an empty DeleteSubnetResponse. The release is the service's own asynchronous cleanup: a client only has to poll until the interfaces disappear.
Emulator: the ELB-created ENIs persist indefinitely after the load balancer is gone. In the same fixture the preceding read (teardown#12 DescribeNetworkInterfaces) still returns both `load_balancer` ENIs when the recording expects none, so the poll runs to its 180s cap, the first DeleteSubnet (teardown#13) answers HTTP 400 instead of 200, and by teardown#14 the fixture's 960s budget is exhausted — that DeleteSubnet gets NO response at all (replay reports expected status (recorded, empty) vs actual None, detail '$: fixture budget 960s spent').
Why: a client that deletes a load balancer and then tears down its VPC can never finish. The subnet (and transitively the VPC) is permanently undeletable, every retry loop that waits for the ENIs to drain spins until its own timeout, and the account leaks subnets/VPCs across runs. Because the emulator never converges, the failure mode is a hang rather than a refusal a client can interpret.
Fix: the load-balancer deletion path's state cleanup — deleting an ELBv2 load balancer must remove the requester-managed ENIs it projected into EC2 state (and the DeleteSubnet dependency bookkeeping must then see the subnet as empty). Related already-open issues cover adjacent symptoms but not this convergence failure: 20261103T123451Z-ec2-refuses-valid-requests-with-400-or-501.md (the teardown#13 DeleteSubnet 400) and 20261103T123451Z-ec2-reads-resolve-resources-outside-the-request-s.md / 20261030T063325Z-ec2-instance-eni-and-volume-projections-omit-modeled.md (the teardown#12 DescribeNetworkInterfaces read); neither owns this step's replay key.
Traces: elbv2err-create-load-balancer-input-rejections#teardown#14 (the never-answered DeleteSubnet).

## Evidence
Replay verdict (expected -> actual) for elbv2err-create-load-balancer-input-rejections#teardown#14, service ec2, operation DeleteSubnet:
  status: expected (recorded, empty) -> actual None
  $: 'fixture budget 960s spent' (expected None, got None)
Context from the same fixture's teardown, showing why the budget was spent:
  teardown#12 DescribeNetworkInterfaces 200 -> 200 but '$: poll capped at 180s': the emulator still returns the two ELB ENIs (interfaceType 'load_balancer', requesterId 'amazon-elb', requesterManaged 'true', description 'ELB app/<lb>/<id>', status 'available') where the recording expects the networkInterfaceSet to have drained to empty.
  teardown#13 DeleteSubnet: expected 200, got 400.
No response was produced for teardown#14 at all, i.e. the emulator never reaches the state in which the subnet is deletable.

## Steps to Reproduce
1. Replay the wire corpus simparity-wire-aws-v1 against the emulator built from this checkout.
2. Inspect fixture elbv2err-create-load-balancer-input-rejections, teardown steps #12, #13 and #14.
3. Equivalently, by hand: create an internal ALB in a VPC's two subnets, DeleteLoadBalancer, then poll DescribeNetworkInterfaces filtered on those subnets — the requester-managed 'load_balancer' ENIs owned by amazon-elb never disappear — and call DeleteSubnet on each subnet: it keeps answering 400 instead of 200, so the teardown never completes.
