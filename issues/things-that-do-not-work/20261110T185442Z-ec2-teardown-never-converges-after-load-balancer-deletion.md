Issue-Source: trace-sweep-divergence
Issue-Trace: elbv2err-create-load-balancer-input-rejections
Issue-Step: teardown#14
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbv2err-create-load-balancer-input-rejections
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: ec2|DeleteSubnet|0->None|$
Issue-Key: ec2|DeleteSubnet|0->None|status
Issue-Commit: c27f673b5184c286166c0255cbce219d9f1e7379

# EC2 teardown never converges after load balancer deletion: DeleteSubnet step is never served

> _Note: this issue was identified against the code base at commit `c27f673b5184c286166c0255cbce219d9f1e7379`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteSubnet

## Description
One cause.

Cause 1 of 1: the final DeleteSubnet of a load-balancer teardown is never answered, because the emulator never releases the load-balancer-managed network interfaces that gate subnet deletion, so the dependency-release poll and the preceding DeleteSubnet retries consume the whole fixture time budget.

AWS: after a load balancer is deleted, AWS releases the ELB-managed elastic network interfaces it placed in each attached subnet (the interfaces whose requester is the ELB service and which are marked requester-managed). A DescribeNetworkInterfaces read filtered on that VPC then returns an empty networkInterfaceSet within seconds, and DeleteSubnet for each freed subnet returns HTTP 200 with an empty DeleteSubnetResponse body. Smithy (ec2 DeleteSubnet): input SubnetId (required), output Unit, no modeled errors — so the dependency rule itself is recorded behavior only: while an interface is still attached AWS refuses the delete with a dependency-violation error, and once the interface is gone it answers 200. The decisive property is that the refusal is TRANSIENT and converges, so each teardown step completes in order and the recorded fixture runs to completion inside its 960s budget.

Emulator: the ELB-managed interfaces are never released after the load balancer is deleted, so the teardown never converges. The dependency-release poll runs to its 180s cap still seeing the interfaces present (teardown#12), the next DeleteSubnet is refused with HTTP 400 and keeps being refused (teardown#13), and by the time the last subnet's DeleteSubnet is reached at teardown#14 the fixture's time budget is exhausted: the replay records no status at all for the step (`status: expected , got None`) with `$: fixture budget 960s spent`. The emulator therefore returns neither the 200 AWS returns nor any error — the request is never served.

Why: a client relying on AWS's behavior deletes its load balancer, waits for the interfaces to be released, then deletes the subnets and the VPC. Against the emulator that wait never ends: the interface set never empties, every subsequent dependency-gated delete is refused forever, and the client's teardown loop hangs instead of terminating. Infrastructure-as-code destroy flows never finish and the account's subnets/VPCs leak on every run, which also makes any later fixture that re-creates those resources start from a dirty state.

Fix: the load-balancer deletion path's state cleanup (deleting a load balancer must release the ELB-managed interfaces it created in each subnet) together with DeleteSubnet's dependency bookkeeping in the ec2 state layer, so that a subnet whose last dependent is gone becomes deletable and the delete answers 200 instead of refusing indefinitely.

Related open issues (neither carries this step's replay key, so neither covers it): 20261103T123451Z-ec2-refuses-valid-requests-with-400-or-501.md describes DeleteSubnet refusing a subnet with no remaining dependencies in other fixtures, and 20261030T063325Z-ec2-instance-eni-and-volume-projections-omit-modeled.md / 20261103T123451Z-ec2-reads-resolve-resources-outside-the-request-s.md own the preceding DescribeNetworkInterfaces step of this same teardown. The non-convergence of the ELB-created interfaces, and the resulting unserved final DeleteSubnet, is filed here.

Traces: elbv2err-create-load-balancer-input-rejections#teardown#14 (the unserved DeleteSubnet). Its preceding steps teardown#12 (interface poll capped at 180s) and teardown#13 (DeleteSubnet 200 -> 400) are the stall that consumes the budget and are already owned by the issues named above.

## Evidence
Replay verdict (expected -> actual), fixture elbv2err-create-load-balancer-input-rejections, step teardown#14, service ec2, operation DeleteSubnet, verdict mismatch:
  status: expected (recorded step) -> got None — no response was produced at all (actual_status null)
  $: 'fixture budget 960s spent'

Upstream stall in the same teardown, for context:
  teardown#12 DescribeNetworkInterfaces 200 -> 200 but '$: poll capped at 180s' — the emulator still reports the two ELB-managed interfaces (description 'ELB app/<lb-name>/<id>', interfaceType 'load_balancer', requesterId 'amazon-elb', requesterManaged 'true') in the load balancer's subnets after the load balancer was deleted, where the recording expects the set to be empty.
  teardown#13 DeleteSubnet 200 -> 400 — the subnet delete is refused because those interfaces are still attached.

Expected on AWS: the interface set empties after the load balancer is deleted, and each DeleteSubnet returns HTTP 200 with an empty DeleteSubnetResponse. Actual on the emulator: the interface set never empties, DeleteSubnet is refused with 400 indefinitely, and the last DeleteSubnet of the fixture is never served. The interface ids, subnet ids, load-balancer name and addresses quoted above are per-run values from the recording and are not part of the requirement; the requirement is that ELB-managed interfaces are released on load-balancer deletion and that DeleteSubnet then answers 200.

## Steps to Reproduce
1. Replay the wire corpus simparity-wire-aws-v1 against the emulator.
2. Open fixture elbv2err-create-load-balancer-input-rejections and follow its teardown: the load balancer is deleted, then DescribeNetworkInterfaces is polled until the ELB-managed interfaces are released, then each subnet is deleted.
3. Observe teardown#12: the poll hits its 180s cap with the ELB-managed interfaces still present.
4. Observe teardown#13: DeleteSubnet answers HTTP 400 instead of 200.
5. Observe teardown#14 (the divergence filed here): the fixture's 960s budget is already spent, so this DeleteSubnet is never served — the replay records no status for the step.
