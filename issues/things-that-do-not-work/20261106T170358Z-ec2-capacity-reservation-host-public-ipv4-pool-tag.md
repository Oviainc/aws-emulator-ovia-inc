Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.capacity-reservation-family-error-semantics ec2.deploy-cdk-vpc-public-and-isolated-subnets-route-differently ec2.host-reservation-purchase-preview-cost-accuracy ec2.op-contract-create-public-ipv4-pool ec2.op-contract-delete-public-ipv4-pool ec2.groups-for-capacity-reservation-reflects-fleet-association
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.capacity-reservation-family-error-semantics|run#5|AssertionError|assert not problems, (
Issue-Key: ec2.deploy-cdk-vpc-public-and-isolated-subnets-route-differently|run#43|AssertionError|assert mutate_got == not_found, (
Issue-Key: ec2.host-reservation-purchase-preview-cost-accuracy|run#24|AssertionError|assert code == "InvalidHostID.NotFound", f"expected InvalidHostID.NotFound, got {code!r}"
Issue-Key: ec2.op-contract-create-public-ipv4-pool|run#17|AssertionError|assert code == "InvalidPublicIpv4PoolID.NotFound", (
Issue-Key: ec2.op-contract-delete-public-ipv4-pool|run#12|AssertionError|assert code_after_delete == "InvalidPublicIpv4PoolID.NotFound", f"unexpected code: {code_after_delete}"
Issue-Key: ec2.groups-for-capacity-reservation-reflects-fleet-association|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Commit: 7ff327d821bcd1f37fa31be7f3b9efbce7107294

# ec2 capacity-reservation / host / public-IPv4-pool / tag paths: 5 net-new causes — wrong refusal codes on four reads and writes, and the capacity-reservation resource-group join never settles

> _Note: this issue was identified against the code base at commit `7ff327d821bcd1f37fa31be7f3b9efbce7107294`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCapacityReservation

## Description
Five net-new root causes across EC2's capacity-reservation, dedicated-host, public-IPv4-pool and tagging paths. Each is a separate defect; all five must be closed before this issue is submitted. Near neighbours that do NOT cover these: 20261029T175114Z-ec2-client-error-classification-wrong-error-codes-for.md and 20261104T142958Z-ec2-ten-ec2-refusals-carry-the-wrong-error-code.md (different operations), 20261031T221631Z-ec2-a-minted-cancellation-quote-is-never-usable.md and 20261105T224104Z-ec2-10-net-new-behavioral-root-causes-peering.md (capacity-reservation quote/split paths, not these).

Cause 1 of 5: CreateCapacityReservation answers ValidationError for an out-of-enum InstancePlatform instead of InvalidParameterValue.
AWS: Smithy: CreateCapacityReservation.InstancePlatform is required and targets the enum CapacityReservationInstancePlatform (no length/pattern constraint, so botocore does not intercept the value client-side). A value outside that enum is a parameter-value rejection: EC2 answers the ec2Query error code InvalidParameterValue (HTTP 400). CreateCapacityReservation declares no modeled errors (errors []), so the code is recorded behavior; the message names the offending parameter and value.
Emulator: the same call is refused with the error code ValidationError — a code EC2's ec2Query protocol does not use for this operation.
Why: every client branches on the error code. InvalidParameterValue is the documented, retry-never, fix-the-input signal; ValidationError is not in EC2's vocabulary at all, so SDK error-shape matching, CLI error handling and IaC providers all fall through to a generic failure path and report an unclassified error.
Fix: classify out-of-enum request members as InvalidParameterValue in CreateCapacityReservation's input validation (and the enum-validation helper it shares), rather than emitting a generic ValidationError.
Traces: ec2.capacity-reservation-family-error-semantics.

Cause 2 of 5: a mutation naming a deleted VPC id is refused as a malformed id instead of as not-found.
AWS: a well-formed id of a resource that no longer exists is refused by its existence check, not by format validation, and a mutation is refused the same way a read is: CreateTags naming a deleted VPC answers code InvalidVpcID.NotFound with the message "The vpc ID '<vpc-id>' does not exist" — fixed text "The vpc ID '" ... "' does not exist", with the id from the request as the only variable part. Recorded behavior only (the EC2 model declares no modeled errors for CreateTags).
Emulator: the same CreateTags answers code InvalidID with the message "The ID '<vpc-id>' is not valid" — a format/id-shape rejection, with a generic message that does not name the resource kind.
Why: InvalidID says "your id is malformed, it never could exist"; InvalidVpcID.NotFound says "that VPC is gone". A client reconciling state after a delete (CDK/CloudFormation drift, tagging sweeps, cleanup loops) treats the first as a client bug to fix and the second as a resource it can stop tracking — so it either aborts or retries forever instead of converging.
Fix: resolve the id against resource state before format-only rejection on the tagging mutation path (CreateTags handler/validation) and emit the per-resource NotFound code and message template.
Traces: ec2.deploy-cdk-vpc-public-and-isolated-subnets-route-differently.

Cause 3 of 5: GetHostReservationPurchasePreview answers InvalidHostId.Malformed for a real, released host id.
AWS: an AWS-issued dedicated-host id that has been released is well-formed, so it reaches the existence check: GetHostReservationPurchasePreview naming it answers code InvalidHostID.NotFound (HTTP 400). Format rejection (InvalidHostId.Malformed) is reserved for ids EC2 does not accept as well-formed. Recorded behavior only for the code (the operation declares no modeled errors).
Emulator: the same call answers InvalidHostId.Malformed — it rejects the id on shape, so the existence check is unreachable for any host id the emulator did not issue in this run.
Why: the codes mean opposite things to a client — "your input is syntactically wrong" versus "that host is gone" — and they are spelled differently (HostID vs HostId), so code-equality branching fails as well. A caller cleaning up or re-pricing reservations after releasing hosts cannot distinguish a typo from a released host.
Fix: run the host existence lookup before/instead of shape rejection in the GetHostReservationPurchasePreview handler's validation, and emit InvalidHostID.NotFound for well-formed-but-absent host ids.
Traces: ec2.host-reservation-purchase-preview-cost-accuracy.

Cause 4 of 5: DescribePublicIpv4Pools uses the delete path's not-found code.
AWS: the read and the mutation have DIFFERENT not-found codes for the same resource. DescribePublicIpv4Pools with an explicit PoolIds filter naming an absent pool answers code InvalidPublicIpv4PoolID.NotFound; DeletePublicIpv4Pool on an absent pool answers code InvalidPublicIpv4Pool.NotFound. Both carry the message "The pool ID '<ipv4pool-ec2-id>' does not exist." — fixed text with the pool id from the request. Smithy: DescribePublicIpv4Pools.PoolIds targets PublicIpv4PoolIdStringList; the operation declares no modeled errors, so the codes are recorded behavior.
Emulator: DescribePublicIpv4Pools answers InvalidPublicIpv4Pool.NotFound — the delete path's code — so the two paths are indistinguishable and the read's documented code is never emitted.
Why: a client that proves a pool is gone by catching the read's specific code (the documented way to tell "absent" from "empty list") never matches, so a deletion looks unconfirmed; conversely a caller branching on the delete code may treat a read failure as a failed delete.
Fix: emit the read-path code InvalidPublicIpv4PoolID.NotFound from the DescribePublicIpv4Pools handler, keeping InvalidPublicIpv4Pool.NotFound on DeletePublicIpv4Pool.
Traces: ec2.op-contract-create-public-ipv4-pool, ec2.op-contract-delete-public-ipv4-pool.

Cause 5 of 5: the capacity-reservation → resource-group join never becomes observable, so the scenario never reaches a verdict.
AWS: a capacity reservation created with EndDateType=unlimited reaches State 'active' promptly (DescribeCapacityReservations), it can then be added to an AWS::EC2::CapacityReservationPool resource group with ResourceGroups GroupResources, and GetGroupsForCapacityReservation then reports exactly that group in CapacityReservationGroups (GroupArn ending in the group name). Recorded behavior only for the timing.
Emulator: the check is killed at its 60 s budget without reaching any assertion. The two bounded waits it spends that budget on are (a) DescribeCapacityReservations reporting State 'active' for the new reservation and (b) GetGroupsForCapacityReservation reporting the group the reservation ARN was just added to. Hypothesis: the reservation never leaves its initial state and/or GetGroupsForCapacityReservation never reflects a resource-group membership, so neither wait can ever be satisfied.
Why: a client cannot observe either the reservation becoming usable or its resource-group membership, so capacity-reservation-group targeting (shared/pooled capacity workflows) is unobservable and any caller that waits on 'active' or on the group listing hangs until its own timeout instead of proceeding.
Fix: advance capacity-reservation state to 'active' in the capacity-reservation state model, and have GetGroupsForCapacityReservation report resource-group membership from that state (the operation's handler plus the state it reads).
Traces: ec2.groups-for-capacity-reservation-reflects-fleet-association.

## Evidence
Cause 1 — ec2.capacity-reservation-family-error-semantics (mismatch):
  call: create_capacity_reservation(InstanceType='t3.micro', InstancePlatform='Not A Real Platform', InstanceCount=1, AvailabilityZone=<az>)
  AssertionError: unexpected capacity-reservation error semantics:
  create_capacity_reservation: expected ['InvalidParameterValue'], got ValidationError

Cause 2 — ec2.deploy-cdk-vpc-public-and-isolated-subnets-route-differently (mismatch):
  AssertionError: CreateTags on the deleted VPC vpc-00000000000000026 failed with ('InvalidID', "The ID 'vpc-00000000000000026' is not valid"), expected ('InvalidVpcID.NotFound', "The vpc ID 'vpc-00000000000000026' does not exist") - a mutation on a dead id must be refused the same way a read is

Cause 3 — ec2.host-reservation-purchase-preview-cost-accuracy (mismatch):
  call: get_host_reservation_purchase_preview / host lookup on an AWS-issued host id that was released
  AssertionError: expected InvalidHostID.NotFound, got 'InvalidHostId.Malformed'

Cause 4 — ec2.op-contract-create-public-ipv4-pool (mismatch):
  call: describe_public_ipv4_pools(PoolIds=['ipv4pool-ec2-00000000000000026']) after DeletePublicIpv4Pool
  AssertionError: expected InvalidPublicIpv4PoolID.NotFound describing the deleted pool 'ipv4pool-ec2-00000000000000026', got 'InvalidPublicIpv4Pool.NotFound'
  ec2.op-contract-delete-public-ipv4-pool (mismatch): AssertionError: unexpected code: InvalidPublicIpv4Pool.NotFound (expected InvalidPublicIpv4PoolID.NotFound on the describe after delete)

Cause 5 — ec2.groups-for-capacity-reservation-reflects-fleet-association (mismatch):
  Error: Check timed out after 60.0s
  the check's only waits are DescribeCapacityReservations -> State=='active' (60 s) and GetGroupsForCapacityReservation -> at least one group after ResourceGroups GroupResources(cr-arn) (60 s); neither produced a verdict

## Steps to Reproduce
Replay the behavioral suite and re-run exactly these checks:
1. ec2.capacity-reservation-family-error-semantics — CreateCapacityReservation with InstancePlatform='Not A Real Platform' (InstanceType t3.micro, InstanceCount 1, a valid AZ); expect InvalidParameterValue.
2. ec2.deploy-cdk-vpc-public-and-isolated-subnets-route-differently — create a VPC, delete it, then call CreateTags naming the deleted VPC id; expect InvalidVpcID.NotFound / "The vpc ID '<id>' does not exist".
3. ec2.host-reservation-purchase-preview-cost-accuracy — AllocateHosts, ReleaseHosts the returned host id, then call GetHostReservationPurchasePreview with that released host id; expect InvalidHostID.NotFound.
4. ec2.op-contract-create-public-ipv4-pool and ec2.op-contract-delete-public-ipv4-pool — CreatePublicIpv4Pool, DeletePublicIpv4Pool, then DescribePublicIpv4Pools(PoolIds=[pool]); expect InvalidPublicIpv4PoolID.NotFound on the describe and InvalidPublicIpv4Pool.NotFound on a repeat delete.
5. ec2.groups-for-capacity-reservation-reflects-fleet-association — CreateCapacityReservation(EndDateType='unlimited'), wait for State 'active', create an AWS::EC2::CapacityReservationPool resource group, GroupResources the reservation ARN, then poll GetGroupsForCapacityReservation for that group.
