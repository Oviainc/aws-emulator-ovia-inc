Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.delete-transit-gateway-dry-run-does-not-delete-rejected ec2.op-contract-deleteTransitGateway ec2.op-contract-associateTrunkInterface ec2.op-contract-attachNetworkInterface ec2.op-contract-cancel-spot-fleet-requests ec2.op-contract-provision-ipam-pool-cidr ec2.placement-partition-group-serves-distinct-partition-numbers-to-the-guests
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeTransitGateways|InvalidTransitGatewayID.NotFound
Issue-Key: ec2|DeleteTransitGateway|InvalidTransitGatewayID.NotFound
Issue-Key: ec2.op-contract-associateTrunkInterface|run#22|AssertionError|assert code == "DryRunOperation", f"expected DryRunOperation, got {code}"
Issue-Key: ec2.op-contract-attachNetworkInterface|run#18|AssertionError|assert code == "InvalidInstanceID.Malformed", f"expected InvalidInstanceID.Malformed, got {code}"
Issue-Key: ec2.op-contract-cancel-spot-fleet-requests|run#18|AssertionError|assert resp2["UnsuccessfulFleetRequests"] == [], (
Issue-Key: ec2.op-contract-provision-ipam-pool-cidr|run#18|AssertionError|assert message == "The ipam-pool ID 'ipam-pool-00000000000000000' does not exist", message
Issue-Key: ec2.placement-partition-group-serves-distinct-partition-numbers-to-the-guests|run#24|AssertionError|assert api_partition[iid].get("PartitionNumber") == partition, (
Issue-Commit: 49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4

# EC2: 7 net-new refusal/representation causes — DryRun on DeleteTransitGateway really deletes, delete is not idempotent, AssociateTrunkInterface pre-empts DryRun, AttachNetworkInterface misclassifies a malformed id, CancelSpotFleetRequests refuses the second cancel, ProvisionIpamPoolCidr wording, Placement.PartitionNumber lost

> _Note: this issue was identified against the code base at commit `49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteTransitGateway

## Description
Seven independent net-new root causes in the EC2 emulator, all reached through the behavioral suite's `### ec2` checks. They are separate defects and must all be closed before this issue is submitted. None is covered by an open issue: the DryRun issues already filed (20261029T175114Z-ec2-dryrun-contract-authorized-dryrun-requests-answer-notimplemented.md) are about authorized DryRun requests answering 501 NotImplemented, which is NOT what these two DryRun causes show.

Cause 1 of 7 — DeleteTransitGateway with DryRun=true actually deletes the transit gateway.
AWS: Smithy: DeleteTransitGateway input carries `DryRun` (Boolean); the operation declares no modeled errors (EC2 query error codes are documented in the API reference, not the model). AWS evaluates DryRun before any state change and answers HTTP 412 with error code `DryRunOperation` and the fixed message `Request would have succeeded, but DryRun flag is set.` — fixed text, no variable part — leaving the gateway in `available`.
Emulator: the DryRun call mutates state. The immediately following DescribeTransitGateways on the same id is refused `InvalidTransitGatewayID.NotFound` with message `Transit Gateway <tgw-id> was deleted or does not exist.` (the id is the one from the request), i.e. the dry run performed the delete.
Why: a client that uses DryRun as a permission/pre-flight probe destroys the resource it was only asking about; every "check before you act" flow becomes a mutation.
Fix: the DeleteTransitGateway handler must evaluate the DryRun flag (and return the DryRunOperation refusal) before touching transit-gateway state, in the operation's handler/validation ahead of its state mutation.
Traces: ec2.delete-transit-gateway-dry-run-does-not-delete-rejected.

Cause 2 of 7 — DeleteTransitGateway is not idempotent once the gateway is deleted.
AWS: recorded behavior only (the model declares no errors): a repeat DeleteTransitGateway on a gateway already deleting/deleted is accepted and returns the TransitGateway member with its deleting/deleted state; only a gateway id that never existed is refused `InvalidTransitGatewayID.NotFound` with `Transit Gateway <tgw-id> was deleted or does not exist.`
Emulator: the second DeleteTransitGateway on the gateway it just deleted raises `InvalidTransitGatewayID.NotFound` — "Transit Gateway <tgw-id> was deleted or does not exist." — so the deleted gateway is purged from state instead of being retained in a terminal state.
Why: cleanup and retry paths (and every at-least-once delete) turn a successful teardown into a hard client error.
Fix: the DeleteTransitGateway handler plus the transit-gateway state store — a deleted gateway must remain readable in a terminal state rather than being removed.
Traces: ec2.op-contract-deleteTransitGateway.

Cause 3 of 7 — AssociateTrunkInterface evaluates the feature refusal before DryRun.
AWS: Smithy: AssociateTrunkInterface input carries `DryRun`; no modeled errors. With DryRun=true the answer is `DryRunOperation` / `Request would have succeeded, but DryRun flag is set.`; only the real (non-DryRun) call on an attached trunk ENI is refused `OperationNotPermitted`, because ENI trunking is reserved for AWS-managed integrations.
Emulator: the DryRun=true call answers `OperationNotPermitted`, so the DryRun contract is never reached.
Why: a client cannot distinguish "you may not do this at all" from "your dry run succeeded"; DryRun probes report a false hard failure.
Fix: in the AssociateTrunkInterface handler, order DryRun evaluation ahead of the operation's authorization/feature gate.
Traces: ec2.op-contract-associateTrunkInterface.

Cause 4 of 7 — AttachNetworkInterface misclassifies a malformed instance id as not-found.
AWS: Smithy: AttachNetworkInterface.InstanceId targets the `InstanceId` shape and is required; no modeled errors. An id whose SYNTAX is not an instance id is refused `InvalidInstanceID.Malformed` (shape validation), while a well-formed id that does not exist is refused `InvalidInstanceID.NotFound`.
Emulator: the malformed id is refused `InvalidInstanceID.NotFound` — the syntactic check is skipped and the id is sent straight to lookup.
Why: clients branch on these two codes (retryable typo vs a resource that is gone); the emulator collapses them, so input-validation errors are reported as missing resources.
Fix: id-syntax validation for InstanceId in the AttachNetworkInterface handler's input validation, before the state lookup.
Traces: ec2.op-contract-attachNetworkInterface.

Cause 5 of 7 — CancelSpotFleetRequests refuses the second cancellation of a fleet left in cancelled_running.
AWS: Smithy: CancelSpotFleetRequests returns `SuccessfulFleetRequests` and `UnsuccessfulFleetRequests`; it reports per-item outcomes rather than raising. After a cancel with TerminateInstances=false the fleet sits in `cancelled_running`; a second cancel with TerminateInstances=true is accepted — the request id appears in SuccessfulFleetRequests and the fleet moves on to `cancelled_terminating`/`cancelled`. Only an id that does not exist appears in UnsuccessfulFleetRequests, with code `fleetRequestIdDoesNotExist` and message `Spot Fleet Request cancellation failed. Spot Fleet Request not found.`
Emulator: the second cancel reports the fleet in UnsuccessfulFleetRequests with code `fleetRequestNotInCancellableState` and message `The spot fleet request '<sfr-id>' is in the state 'cancelled_running' and cannot be canceled` (fixed text around the request's fleet id and the state held in emulator state), so the fleet never reaches the terminal `cancelled` state.
Why: the documented two-step cancel (leave instances running, then terminate them) is unusable and a fleet can never be driven to its terminal state.
Fix: the CancelSpotFleetRequests handler's state machine — `cancelled_running` must be a cancellable state, and its per-item classification must reserve `fleetRequestNotInCancellableState` for states AWS refuses.
Traces: ec2.op-contract-cancel-spot-fleet-requests.

Cause 6 of 7 — ProvisionIpamPoolCidr's not-found message is not AWS's wording.
AWS: Smithy: ProvisionIpamPoolCidr.IpamPoolId targets `IpamPoolId` and is required; no modeled errors. A pool id that does not exist is refused `InvalidIpamPoolId.NotFound` with the message `The ipam-pool ID '<ipam-pool-id>' does not exist` — the fixed text is `The ipam-pool ID '` / `' does not exist`, naming the resource type by its hyphenated EC2 resource token `ipam-pool`; the only variable part is the pool id copied from the request.
Emulator: the message is `The IPAM pool ID '<ipam-pool-id>' does not exist` — the resource token is replaced by prose capitalisation (`IPAM pool`).
Why: callers and tests that match AWS's message text (and humans reading logs against AWS docs) do not match; the emulator's wording is not the API's.
Fix: the not-found message template in the ProvisionIpamPoolCidr handler's validation.
Traces: ec2.op-contract-provision-ipam-pool-cidr.

Cause 7 of 7 — an instance launched into a partition placement group has no PartitionNumber in DescribeInstances.
AWS: Smithy: RunInstances.Placement carries `PartitionNumber` and the Instance.Placement member reports it back; for an instance launched with Placement.GroupName=<partition group> and Placement.PartitionNumber=<n>, DescribeInstances reports Placement.PartitionNumber = <n> (the number from the launch request).
Emulator: DescribeInstances reports `PartitionNumber` absent/None for the instance launched with PartitionNumber=1 — the requested partition is accepted and then dropped.
Why: a client cannot tell which partition an instance was placed in, so partition-aware placement (the whole point of the strategy) is unobservable and spread assertions fail.
Fix: carry Placement.PartitionNumber into instance state in the RunInstances handler and project it in the DescribeInstances serializer.
Traces: ec2.placement-partition-group-serves-distinct-partition-numbers-to-the-guests.

## Evidence
Cause 1: after `delete_transit_gateway(TransitGatewayId=<tgw>, DryRun=True)` the check's DescribeTransitGateways raises `botocore.exceptions.ClientError: An error occurred (InvalidTransitGatewayID.NotFound) when calling the DescribeTransitGateways operation: Transit Gateway <tgw-id> was deleted or does not exist.` Expected: error code `DryRunOperation`, message `Request would have succeeded, but DryRun flag is set.`, gateway still `available`.
Cause 2: `ClientError: An error occurred (InvalidTransitGatewayID.NotFound) when calling the DeleteTransitGateway operation: Transit Gateway <tgw-id> was deleted or does not exist.` Expected: the repeat delete is accepted and returns the gateway in its terminal state.
Cause 3: `AssertionError: expected DryRunOperation, got OperationNotPermitted`.
Cause 4: `AssertionError: expected InvalidInstanceID.Malformed, got InvalidInstanceID.NotFound`.
Cause 5: `AssertionError: expected no unsuccessful cancellations, got [{'Error': {'Code': 'fleetRequestNotInCancellableState', 'Message': "The spot fleet request '<sfr-id>' is in the state 'cancelled_running' and cannot be canceled"}, 'SpotFleetRequestId': '<sfr-id>'}]`. Expected: UnsuccessfulFleetRequests == [] and the fleet reaching `cancelled`.
Cause 6: `AssertionError: The IPAM pool ID '<ipam-pool-id>' does not exist` raised against the expected `The ipam-pool ID '<ipam-pool-id>' does not exist`.
Cause 7: `AssertionError: DescribeInstances reports <instance-id> in partition None, not the requested 1`.

## Steps to Reproduce
Replay the behavioral suite and re-run exactly these checks: ec2.delete-transit-gateway-dry-run-does-not-delete-rejected, ec2.op-contract-deleteTransitGateway, ec2.op-contract-associateTrunkInterface, ec2.op-contract-attachNetworkInterface, ec2.op-contract-cancel-spot-fleet-requests, ec2.op-contract-provision-ipam-pool-cidr, ec2.placement-partition-group-serves-distinct-partition-numbers-to-the-guests. Each one exercises a single operation: create the resource (transit gateway / trunk ENI / zero-capacity Spot Fleet / IPAM pool / partition placement group), then issue the call described in the matching cause and compare the status, error code and message (or the described member) with AWS's.
