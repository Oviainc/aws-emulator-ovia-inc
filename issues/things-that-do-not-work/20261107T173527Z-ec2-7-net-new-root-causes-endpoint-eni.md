Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.asvpce-streaming-vpce-eni-delete-rejected ec2.concurrent-modify-instance-capacity-reservation-attributes-race ec2.groups-for-capacity-reservation-reflects-fleet-association ec2.op-contract-modify-instance-connect-endpoint ec2.op-contract-modify-ipam-prefix-list-resolver-target ec2.op-contract-reject-vpc-endpoint-connections ec2.shutdown-api-termination-disabled-rejects-terminate-but-still-permits-stop
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.asvpce-streaming-vpce-eni-delete-rejected|_poll#8|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: ec2.concurrent-modify-instance-capacity-reservation-attributes-race|_poll_until#20|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s, last observed value: {last!r}")
Issue-Key: ec2.groups-for-capacity-reservation-reflects-fleet-association|_poll_until#16|AssertionError|raise AssertionError(
Issue-Key: ec2.op-contract-modify-instance-connect-endpoint|_poll_until#13|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Key: ec2.op-contract-modify-ipam-prefix-list-resolver-target|run#20|AssertionError|assert target is not None and target.get("LastSyncedVersion") is not None, (
Issue-Key: ec2.op-contract-reject-vpc-endpoint-connections|_poll_until#23|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s, last value: {last!r}")
Issue-Key: ec2.shutdown-api-termination-disabled-rejects-terminate-but-still-permits-stop|_view#12|Timeout|return None
Issue-Commit: edd8d27229a8bc99e308a4b32d393f13d6f32cb7

# EC2: 7 net-new root causes — endpoint ENI never in-use, capacity-reservation binding and group membership missing, ModifyInstanceConnectEndpoint a no-op, IPAM resolver target never syncs, no VPC endpoint connection on a customer service, termination-protection flow never converges

> _Note: this issue was identified against the code base at commit `edd8d27229a8bc99e308a4b32d393f13d6f32cb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyInstanceCapacityReservationAttributes

## Description
Seven distinct net-new root causes, all under the suite heading `### ec2`. Each has its own failing check id.

Cause 1 of 7: an interface VPC endpoint's requester-managed ENI never reaches Status `in-use`.
AWS: once an interface endpoint is `available`, the elastic network interface EC2 created for it is attached and DescribeNetworkInterfaces reports Status `in-use` (Smithy ec2 NetworkInterfaceStatus = available|associated|attaching|in-use|detaching) with InterfaceType `vpc_endpoint`, RequesterManaged true and Description `VPC Endpoint Interface <vpce-id>`; a direct DeleteNetworkInterface on it is refused `InvalidParameterValue` with the message "Network interface '<eni-id>' is currently in use." (fixed text, the ENI id taken from the request) and only deleting the endpoint releases it.
Emulator: with the endpoint `available`, the ENI stays `Status: 'available'` for the whole 120s window, so the in-use precondition — and with it the refusal of the direct delete — is never reachable.
Why: a client cannot tell an attached endpoint interface from a free one, and the protection that stops an operator from deleting an endpoint's ENI out from under it is untestable.
Fix: the VPC endpoint create/available transition must mark its requester-managed ENI in-use (endpoint state plus network-interface state), and DeleteNetworkInterface must refuse an in-use requester-managed interface.
Traces: ec2.asvpce-streaming-vpce-eni-delete-rejected.

Cause 2 of 7: ModifyInstanceCapacityReservationAttributes does not bind the instance to the reservation it names.
AWS: ModifyInstanceCapacityReservationAttributes on a stopped instance accepts a CapacityReservationSpecification whose CapacityReservationTarget names a targeted reservation and returns Return=true; DescribeInstances then reports that instance's CapacityReservationSpecification with CapacityReservationTarget.CapacityReservationId set (Smithy ec2 CapacityReservationSpecificationResponse returns capacityReservationTarget when the instance explicitly targets a reservation), and once started the reservation's AvailableInstanceCount drops to 0 while an untargeted sibling stays 1.
Emulator: after the modify calls succeed, DescribeInstances reports no CapacityReservationTarget at all — the polled target stays `None` for 60s — so the instance is bound to neither reservation and the consumption accounting never moves.
Why: targeted capacity reservations are unusable: a client cannot read back which reservation an instance will consume, and reservation utilisation is always wrong.
Fix: ModifyInstanceCapacityReservationAttributes must persist the specification onto the instance and DescribeInstances must serialise it (operation handler plus instance state), and starting the instance must decrement the targeted reservation's AvailableInstanceCount.
Traces: ec2.concurrent-modify-instance-capacity-reservation-attributes-race.

Cause 3 of 7: GetGroupsForCapacityReservation reports no resource group for a reservation that was added to one.
AWS: after Resource Groups GroupResources adds an active Capacity Reservation's ARN to an AWS::EC2::CapacityReservationPool group, GetGroupsForCapacityReservation(CapacityReservationId) returns one CapacityReservationGroups entry per group, each carrying GroupArn and OwnerId (Smithy ec2 CapacityReservationGroup: "Describes a resource group to which a Capacity Reservation has been added").
Emulator: CapacityReservationGroups stays `[]` for the full 60s window after a successful GroupResources call.
Why: capacity-reservation-group placement (launching into a pool rather than a single reservation) cannot be discovered or verified from EC2 at all.
Fix: GetGroupsForCapacityReservation must answer from the Resource Groups membership of the reservation's ARN rather than returning an empty list (operation handler plus the shared resource-group membership state).
Traces: ec2.groups-for-capacity-reservation-reflects-fleet-association.

Cause 4 of 7: ModifyInstanceConnectEndpoint is accepted but changes nothing.
AWS: ModifyInstanceConnectEndpoint(InstanceConnectEndpointId, SecurityGroupIds) returns Return=true and moves the endpoint through `update-in-progress` to `update-complete` (Smithy ec2 Ec2InstanceConnectEndpointState = create-in-progress|create-complete|create-failed|delete-in-progress|delete-complete|delete-failed|update-in-progress|update-complete|update-failed), after which DescribeInstanceConnectEndpoints reports the new SecurityGroupIds.
Emulator: the call succeeds, but for 120s the endpoint stays `State: 'create-complete'` with its original SecurityGroupIds — no update state is ever entered and the swap is lost.
Why: a client that rotates an EC2 Instance Connect Endpoint's security groups gets a success response and stale configuration, and no state ever signals the update.
Fix: the ModifyInstanceConnectEndpoint handler must apply SecurityGroupIds/IpAddressType/PreserveClientIp to the stored endpoint and drive the update-in-progress -> update-complete transition.
Traces: ec2.op-contract-modify-instance-connect-endpoint.

Cause 5 of 7: an IPAM prefix list resolver target never synchronises, so it has no version to pin.
AWS: a target created against a create-complete resolver synchronises the named managed prefix list and reports `LastSyncedVersion` — "the version of the prefix list that was last successfully synchronized by this target" (Smithy ec2 IpamPrefixListResolverTarget) — which ModifyIpamPrefixListResolverTarget can then pin as DesiredVersion with TrackLatestVersion=false.
Emulator: the target reaches `State: 'create-complete'` with TrackLatestVersion=true but carries neither LastSyncedVersion nor DesiredVersion, so the pin-to-synced-version behavior is unreachable.
Why: prefix-list resolver targets cannot be pinned or audited — a client cannot tell which prefix-list version a resolver actually published.
Fix: the resolver-target create/sync path must record the synchronised prefix-list version on the target and expose it through DescribeIpamPrefixListResolverTargets (resolver-target state plus its sync step).
Traces: ec2.op-contract-modify-ipam-prefix-list-resolver-target.

Cause 6 of 7: an interface endpoint onto a customer-owned endpoint service creates no VpcEndpointConnection on the service side.
AWS: when a consumer CreateVpcEndpoint targets an endpoint service configured with AcceptanceRequired=true, the provider's DescribeVpcEndpointConnections (filtered by service-id) reports a VpcEndpointConnection for that endpoint whose VpcEndpointState is `pendingAcceptance` (Smithy ec2 VpcEndpointConnection: ServiceId, VpcEndpointId, VpcEndpointOwner, VpcEndpointState); only then can RejectVpcEndpointConnections / AcceptVpcEndpointConnections act on it.
Emulator: the connection is never created — the polled state stays `'not-found'` for 120s — so neither the DryRun refusal nor the reject/accept transitions can be exercised.
Why: PrivateLink provider workflows (approve or reject consumer connections) are entirely unavailable, and a consumer endpoint appears connected to a service that knows nothing about it.
Fix: CreateVpcEndpoint against a customer endpoint service must create the service-side connection record in pendingAcceptance and DescribeVpcEndpointConnections must serve it (endpoint-service state plus the endpoint create handler).
Traces: ec2.op-contract-reject-vpc-endpoint-connections.

Cause 7 of 7: the DisableApiTermination terminate/stop flow never converges within the replay budget.
AWS: recorded behavior only — the recording completes this sequence (launch into a dedicated VPC, set DisableApiTermination, have TerminateInstances refused while StopInstances is still permitted) inside the suite's budget.
Emulator: the check produces no response at all; it ends as `Check timed out after 180.0s`, so the budget exhaustion is the only evidence and no individual operation is named. Hypothesis: one of the instance state transitions this flow waits on (running, then stopped) never settles.
Why: a client driving termination protection sees the flow hang rather than complete, so neither the refusal nor the permitted stop is observable.
Fix: the instance lifecycle state machine behind this flow (RunInstances -> running, ModifyInstanceAttribute DisableApiTermination, TerminateInstances refusal, StopInstances -> stopped) must converge; start from whichever wait does not settle when the check is re-replayed.
Traces: ec2.shutdown-api-termination-disabled-rejects-terminate-but-still-permits-stop.

## Evidence
Cause 1 — expected ENI Status `in-use` for the endpoint's interface; actual `... 'Description': 'VPC Endpoint Interface vpce-<id>', 'RequesterManaged': True, 'Status': 'available' ...` with `AssertionError: ENI eni-<id> in-use not met within 120.0s`.
Cause 2 — expected DescribeInstances CapacityReservationSpecification.CapacityReservationTarget.CapacityReservationId == one of the two reservations; actual `AssertionError: condition not met within 60s, last observed value: None`.
Cause 3 — expected one CapacityReservationGroups entry naming the group; actual `AssertionError: timed out after 60.0s waiting for GetGroupsForCapacityReservation to reflect group '<group>' for cr-<id>; last observed value: []`.
Cause 4 — expected State `update-complete` and SecurityGroupIds [sg2]; actual `{'State': 'create-complete', ..., 'SecurityGroupIds': ['<sg1>']}` after `AssertionError: condition not met within 120s`.
Cause 5 — expected a LastSyncedVersion to pin; actual `AssertionError: resolver target never reported a LastSyncedVersion to pin to: {... 'TrackLatestVersion': True, 'State': 'create-complete' ...}` (no LastSyncedVersion/DesiredVersion members).
Cause 6 — expected the service-side connection in `pendingAcceptance`; actual `AssertionError: condition not met within 120s, last value: 'not-found'`.
Cause 7 — expected the flow to complete; actual `Error: Check timed out after 180.0s` with no response recorded.
All ids, names and timestamps above are per-run values; the requirements are the states, members and refusals named in the description.

## Steps to Reproduce
1. Replay the behavioral suite (run_behavioral_suite) and look under `### ec2` for the six check ids in trace_ids.
2. Cause 1: CreateVpcEndpoint (Interface) for com.amazonaws.<region>.appstream.streaming in one subnet, wait for state `available`, then DescribeNetworkInterfaces on the endpoint's NetworkInterfaceIds — Status is `available`, not `in-use`; DeleteNetworkInterface on it is therefore not refused as in-use.
3. Cause 2: run and stop a t3.micro, create two targeted capacity reservations, call ModifyInstanceCapacityReservationAttributes naming one, then DescribeInstances — CapacityReservationSpecification carries no CapacityReservationTarget.
4. Cause 3: CreateCapacityReservation, wait for `active`, create a Resource Groups CapacityReservationPool group, GroupResources with the reservation ARN, then GetGroupsForCapacityReservation — empty.
5. Cause 4: CreateInstanceConnectEndpoint with sg1, wait for `create-complete`, ModifyInstanceConnectEndpoint with sg2, then DescribeInstanceConnectEndpoints — still create-complete with sg1.
6. Cause 5: create an advanced-tier IPAM, a managed prefix list, an IPAM prefix list resolver and a target with TrackLatestVersion=true; DescribeIpamPrefixListResolverTargets at create-complete — no LastSyncedVersion.
7. Cause 6: create an internal NLB, CreateVpcEndpointServiceConfiguration(AcceptanceRequired=true), CreateVpcEndpoint (Interface) against its ServiceName, then DescribeVpcEndpointConnections filtered by service-id — no connection.
8. Cause 7: re-replay ec2.shutdown-api-termination-disabled-rejects-terminate-but-still-permits-stop and record which wait does not settle before the budget expires.
