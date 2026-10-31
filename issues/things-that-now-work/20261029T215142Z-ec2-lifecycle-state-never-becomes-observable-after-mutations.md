Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-dry-run-validation-before-commit
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-dry-run-validation-before-commit ec2-interruptible-capacity-reservation-allocation ec2-capacity-reservation-usage-and-groups ec2-dry-run-delete-validation-across-network-resources ec2-network-acl-entries-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateInterruptibleCapacityReservationAllocation|2xx->2xx|$.CreateInterruptibleCapacityReservationAllocationResponse.status
Issue-Key: ec2|DescribeCapacityReservations|2xx->2xx|$
Issue-Key: ec2|DescribeCapacityReservations|2xx->2xx|$.DescribeCapacityReservationsResponse.capacityReservationSet.item.interruptibleCapacityAllocation.instanceCount
Issue-Key: ec2|DescribeCapacityReservations|2xx->2xx|$.DescribeCapacityReservationsResponse.capacityReservationSet.item.interruptibleCapacityAllocation.status
Issue-Key: ec2|GetCapacityReservationUsage|2xx->2xx|$.GetCapacityReservationUsageResponse.state
Issue-Key: ec2|DescribeNetworkAcls|2xx->2xx|$.DescribeNetworkAclsResponse.networkAclSet.item.entrySet.item[*]
Issue-Key: ec2|DescribeNetworkAcls|4xx->2xx|$
Issue-Key: ec2|DescribeNetworkAcls|4xx->2xx|$.Response
Issue-Key: ec2|DescribeNetworkAcls|4xx->2xx|status
Issue-Related: 20261029T200817Z-ec2-capacity-reservations-never-reach-cancelled-lose-their.md
Issue-Commit: 14bf84ae7f57c35d094a91bae03d265103651cef

# EC2 lifecycle state never becomes observable after mutations

> _Note: this issue was identified against the code base at commit `14bf84ae7f57c35d094a91bae03d265103651cef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateInterruptibleCapacityReservationAllocation

## Description
Six net-new clusters show mutations that the emulator accepts but whose effect is never visible in the following read: a wrong lifecycle status literal, counts that are not updated, a terminal state that is never reached, an ACL entry that is not stored and an ACL deletion that is not applied. All six are reported with an EMPTY owned_by and an empty stub_path. The open issues 20261029T200817Z-ec2-capacity-reservations-never-reach-cancelled-lose-their.md and 20261029T175114Z-ec2-led-lifecycle-scenarios-never-reach-their-observable.md own OTHER clusters; these six are not carried by any open issue's replay key. There are 6 causes; all 6 must be fixed before this issue is closed.

Cause 1 of 6 - CreateInterruptibleCapacityReservationAllocation status literal. AWS: the create response's status member is the allocation's lifecycle value in AWS's vocabulary, 'CREATING' on acceptance. Emulator: answers 200 with status 'pending'. Why: a client that switches on the documented status value never matches, so it either waits forever or treats the allocation as a different state. Fix: the CreateInterruptibleCapacityReservationAllocation handler's status projection (status vocabulary, not a generic pending literal). Traces: ec2-dry-run-validation-before-commit#task#6.

Cause 2 of 6 - DescribeCapacityReservations after that allocation. AWS: the parent reservation reflects the allocation - totalInstanceCount stays at the reserved count, interruptibleCapacityAllocation.instanceCount becomes the allocated count with status 'active', and unusedReservationBillingOwnerId is present. Emulator: totalInstanceCount is inflated (the allocation is added to the parent instead of drawn from it), interruptibleCapacityAllocation.instanceCount stays 0 with status 'pending', and unusedReservationBillingOwnerId is absent, so the fixture's poll caps out. Why: a client reading capacity to decide whether to launch sees capacity that does not exist and an allocation that never activates. Fix: the interruptible-allocation state transition on the capacity-reservation record plus the DescribeCapacityReservations projection. Traces: ec2-dry-run-validation-before-commit#task#7.

Cause 3 of 6 - the allocation never reaches its terminal state. AWS: after the allocation is cancelled, interruptibleCapacityAllocation.status reaches 'canceled' (and the reservation stops reporting the allocation). Emulator: the status stays 'pending' through 61 poll attempts. Why: teardown and any client waiting for cancellation never converges. Fix: the cancel path for interruptible allocations - apply the terminal transition to state. Traces: ec2-dry-run-validation-before-commit#teardown#9.

Cause 4 of 6 - GetCapacityReservationUsage after cancellation. AWS: the usage report's state follows the reservation, so a cancelled reservation reports state 'cancelled'. Emulator: reports 'active'. Why: a client reconciling usage against reservation state sees a cancelled reservation as live. Fix: GetCapacityReservationUsage must read the reservation's current state rather than a static literal. Traces: ec2-capacity-reservation-usage-and-groups#teardown#8.

Cause 5 of 6 - CreateNetworkAclEntry effect not visible in DescribeNetworkAcls. AWS: an entry created on a network ACL appears in entrySet with the submitted ruleNumber, protocol, ruleAction, egress, cidrBlock and portRange, alongside the default deny rules. Emulator: entrySet contains only the default rule pair, so the created entry is absent at that index. Why: a client that configures ACL rules and then reads them back cannot confirm or enumerate its own rules. Fix: the CreateNetworkAclEntry handler must persist the entry onto the ACL record read by DescribeNetworkAcls. Traces: ec2-dry-run-delete-validation-across-network-resources#task#10.

Cause 6 of 6 - DeleteNetworkAcl effect not visible. AWS: after the ACL is deleted, describing it by id answers 400 InvalidNetworkAclID.NotFound with Message fixed text "The networkAcl ID '<acl-id>' does not exist", the id taken from the request. Emulator: keeps answering 200 with the ACL body for 56 poll attempts, so the not-found condition never arrives. Why: a client cannot tell that a delete took effect, and idempotent cleanup loops never terminate. Fix: the delete path for network ACLs must remove the record from state (see also the DeleteNetworkAcl scaffold stub issue - the delete itself answers NotImplemented in other fixtures; this cluster is the read-back divergence). Traces: ec2-dry-run-delete-validation-across-network-resources#task#28.

## Evidence
Replay of simparity-wire-aws-v1; all six clusters verdict=mismatch, stub_path empty, owned_by empty.

1) ec2-dry-run-validation-before-commit#task#6 CreateInterruptibleCapacityReservationAllocation: 200/200, $.CreateInterruptibleCapacityReservationAllocationResponse.status expected 'CREATING', got 'pending' (cluster also covers ec2-interruptible-capacity-reservation-allocation#task#3).
2) ec2-dry-run-validation-before-commit#task#7 DescribeCapacityReservations: poll capped at 60s; capacityReservationSet.item.totalInstanceCount expected '1' got '2'; .interruptibleCapacityAllocation.instanceCount expected '1' got '0'; .interruptibleCapacityAllocation.status expected 'active' got 'pending'; .unusedReservationBillingOwnerId expected present, absent.
3) ec2-dry-run-validation-before-commit#teardown#9 DescribeCapacityReservations: .interruptibleCapacityAllocation.status poll: not in ['canceled'] after 61 attempt(s), got 'pending'.
4) ec2-capacity-reservation-usage-and-groups#teardown#8 GetCapacityReservationUsage: $.GetCapacityReservationUsageResponse.state expected 'cancelled', got 'active'.
5) ec2-dry-run-delete-validation-across-network-resources#task#10 DescribeNetworkAcls: networkAclSet.item.entrySet.item[1] expected {ruleNumber '100', protocol '6', ruleAction 'allow', egress 'false', cidrBlock '0.0.0.0/0', portRange {from '443', to '443'}}, got the default {ruleNumber '32767', protocol '-1', ruleAction 'deny', egress 'true'} (cluster also covers #task#12 and ec2-network-acl-entries-lifecycle#task#7).
6) ec2-dry-run-delete-validation-across-network-resources#task#28 DescribeNetworkAcls: status expected 400, got 200; expected $.Response.Errors.Error {Code 'InvalidNetworkAclID.NotFound', Message "The networkAcl ID '<acl-id>' does not exist"}; poll: error code None not in ['InvalidNetworkAclID.NotFound'] after 56 attempt(s), the deleted ACL still fully projected.

Generated ids, counts tied to the recorded request and timestamps above are per-run values; the requirements are the status vocabulary, the accounting of allocated vs total capacity, the terminal transitions, the persistence of created ACL entries and the not-found answer after deletion.

## Steps to Reproduce
1. Boot the emulator built from this checkout.
2. Replay trace set simparity-wire-aws-v1.
3. Inspect fixture ec2-dry-run-validation-before-commit step task#6 and the five fixture#step bindings in trace_ids.
4. Each step performs the recorded mutation and then reads it back; the read reports the wrong status literal, stale counts, a poll that caps at its budget, a missing ACL entry, or a 200 where the recording has 400 InvalidNetworkAclID.NotFound.
