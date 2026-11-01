Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-accept-vpc-peering-connection-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-accept-vpc-peering-connection-lifecycle ec2-describe-aggregate-id-format-contract ec2-describe-capacity-reservations-contract ec2-catalog-get-managed-prefix-list-entries ec2-page-describe-instances-tag-filtered-two-pages
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateVpcPeeringConnection|2xx->2xx|$.CreateVpcPeeringConnectionResponse.vpcPeeringConnection.requesterVpcInfo.peeringOptions
Issue-Key: ec2|DescribeVpcPeeringConnections|2xx->2xx|$.DescribeVpcPeeringConnectionsResponse.vpcPeeringConnectionSet.item.status.code
Issue-Key: ec2|DescribeVpcPeeringConnections|2xx->2xx|$.DescribeVpcPeeringConnectionsResponse.vpcPeeringConnectionSet.item.status.message
Issue-Key: ec2|DescribeAggregateIdFormat|2xx->2xx|$.DescribeAggregateIdFormatResponse.statusSet.item[*]
Issue-Key: ec2|CancelCapacityReservation|2xx->4xx|status
Issue-Key: ec2|GetManagedPrefixListEntries|2xx->4xx|$.GetManagedPrefixListEntriesResponse
Issue-Key: ec2|GetManagedPrefixListEntries|2xx->4xx|status
Issue-Key: ec2|DescribeInstances|2xx->2xx|$
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item[*].instancesSet.item.instanceState.name
Issue-Commit: 030c70f3b5611e115edea7c769d1aba1b20ff8ad

# EC2 peering, id-format, prefix-list and capacity read paths diverge from AWS

> _Note: this issue was identified against the code base at commit `030c70f3b5611e115edea7c769d1aba1b20ff8ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcPeeringConnection

## Description
Six net-new root causes where an EC2 read or state transition does not match AWS: modeled members are dropped from a create projection, a deletion is never observable, a static id-format answer is emulator-authored, an AWS-managed prefix list cannot be read at all, a valid cancel is refused, and launched instances never become observable. Related open issues describe neighbouring symptoms but do NOT carry these replay keys: 20261029T200817Z-ec2-read-paths-omit-modeled-members-and-serve.md, 20261029T200817Z-ec2-capacity-reservations-never-reach-cancelled-lose-their.md, 20261029T175114Z-ec2-led-lifecycle-scenarios-never-reach-their-observable.md.

Cause 1 of 6: CreateVpcPeeringConnection omits VpcPeeringConnectionVpcInfo.peeringOptions.
AWS: Smithy (ec2, CreateVpcPeeringConnection) output member VpcPeeringConnection -> VpcPeeringConnection; VpcPeeringConnectionVpcInfo member PeeringOptions (xmlName 'peeringOptions', target VpcPeeringConnectionOptionsDescription, doc "Information about the VPC peering connection options for the accepter or requester VPC"). The recording shows AWS returning peeringOptions on both requesterVpcInfo and accepterVpcInfo with all three booleans present (allowEgressFromLocalClassicLinkToRemoteVpc, allowEgressFromLocalVpcToRemoteClassicLink, allowDnsResolutionFromRemoteVpc), defaulting to false for a fresh connection.
Emulator: the peeringOptions element is absent from the response (HTTP 200 otherwise shaped correctly).
Why: an SDK caller reading connection.RequesterVpcInfo.PeeringOptions gets nil and cannot tell "option off" from "not reported"; code that round-trips the structure into ModifyVpcPeeringConnectionOptions loses the current setting.
Fix: the CreateVpcPeeringConnection serializer/projection for VpcPeeringConnectionVpcInfo (and the shared peering-connection projection used by the describe path).
Traces: ec2-accept-vpc-peering-connection-lifecycle#task#5.

Cause 2 of 6: a deleted VPC peering connection never reaches the deleted state.
AWS: Smithy VpcPeeringConnectionStateReason{code,message}; the recording shows DescribeVpcPeeringConnections reporting status.code 'deleted' and status.message "Deleted by <accountId>" after the connection is deleted — the fixed text is "Deleted by " plus the deleting principal's account id taken from state.
Emulator: status.code stays 'pending-acceptance' and status.message stays "Pending Acceptance by <accountId>" for the full 56-attempt poll window, so the terminal state is never observable. Hypothesis: the delete path leaves the stored state untouched (DeleteVpcPeeringConnection is a scaffold stub in this build, filed separately), so the describe path has nothing to transition.
Why: any wait-for-deletion loop hangs, and a caller sees a connection it just deleted as still awaiting acceptance.
Fix: the VPC-peering state machine (delete handler must move the connection to deleted with AWS's status message, and the describe projection must serve it).
Traces: ec2-accept-vpc-peering-connection-lifecycle#teardown#8.

Cause 3 of 6: DescribeAggregateIdFormat serves an emulator-authored resource list without deadlines.
AWS: Smithy DescribeAggregateIdFormat output Statuses -> IdFormatList of IdFormat{deadline (DateTime, "The date in UTC at which you are permanently switched over to using longer IDs. If a deadline is not yet available for this resource type, this field is not returned"), resource, useLongIds}. The recording shows AWS's own account-wide list: a specific set of resource types, each with useLongIds true and, where one applies, a deadline.
Emulator: a different, alphabetically-enumerated resource list (bundle, conversion-task, customer-gateway, dhcp-options, ...) and no deadline member on any entry.
Why: a client enumerating long-id status per resource type reads entries AWS does not report and misses the ones it does, and never sees a deadline, so long-id migration checks are meaningless.
Fix: the DescribeAggregateIdFormat handler's static id-format table (resource set and per-resource deadline).
Traces: ec2-describe-aggregate-id-format-contract#task#1.

Cause 4 of 6: CancelCapacityReservation refuses a reservation it just created.
AWS: recorded behavior only — HTTP 200 for CancelCapacityReservation on the reservation created earlier in the same fixture.
Emulator: HTTP 400. Hypothesis: the cancel path applies a precondition (state or commitment check) that the emulator's own created reservation does not satisfy.
Why: the fixture's teardown cannot release the reservation, so a client can create capacity it can never cancel — capacity leaks and every create/cancel test fails.
Fix: the CancelCapacityReservation handler's precondition/state check.
Traces: ec2-describe-capacity-reservations-contract#teardown#7.

Cause 5 of 6: GetManagedPrefixListEntries cannot read an AWS-managed prefix list.
AWS: Smithy GetManagedPrefixListEntries input PrefixListId (required, PrefixListResourceId) / output Entries -> PrefixListEntrySet of PrefixListEntry{cidr,description}; HTTP 200 with the list's CIDR entries. The recording reads an AWS-managed (service-owned) prefix list and gets its entry set back.
Emulator: HTTP 400 and no GetManagedPrefixListEntriesResponse at all, for every one of the 16 reads in the fixture. Hypothesis: only customer-created prefix lists exist in state, so an AWS-managed id resolves to nothing and the handler refuses.
Why: security-group and route-table automation that resolves AWS-managed prefix lists (e.g. the per-region service ranges) cannot run at all against the emulator; an operation AWS answers 200 is a hard failure.
Fix: the managed-prefix-list state/seed (AWS-managed lists must exist and be readable) and the GetManagedPrefixListEntries lookup path.
Traces: ec2-catalog-get-managed-prefix-list-entries#task#1 (the same cause fails task#1..task#16 of that fixture).

Cause 6 of 6: instances launched in a paged DescribeInstances scenario never become observable as running.
AWS: recorded behavior only — HTTP 200 and the launched reservations present, with instancesSet.item.instanceState.name reaching 'running'.
Emulator: DescribeInstances answers 200 with an empty reservationSet; the poll for the sixth reservation's instanceState.name never leaves None and the 60s budget is exhausted after 61 attempts.
Why: a client that launches instances and waits for running never converges, and the pagination contract under a tag filter cannot be exercised at all.
Fix: the RunInstances/DescribeInstances state coupling (launched instances must be registered and must advance to running) and the tag-filtered DescribeInstances projection.
Traces: ec2-page-describe-instances-tag-filtered-two-pages#setup#11.

## Evidence
Replay of trace set simparity-wire-aws-v1 (285 fixtures, 2159 steps, 788 mismatched, score 53.3%). All six clusters are verdict=mismatch, service ec2, stub_path empty, owned_by empty.

1. ec2-accept-vpc-peering-connection-lifecycle#task#5 CreateVpcPeeringConnection, 200->200 — $.CreateVpcPeeringConnectionResponse.vpcPeeringConnection.requesterVpcInfo.peeringOptions: missing (expected {'allowEgressFromLocalClassicLinkToRemoteVpc': 'false', 'allowEgressFromLocalVpcToRemoteClassicLink': 'false', 'allowDnsResolutionFromRemoteVpc': 'false'}, got None).
2. ec2-accept-vpc-peering-connection-lifecycle#teardown#8 DescribeVpcPeeringConnections, 200->200 — status.code: expected 'deleted', got 'pending-acceptance'; status.message: expected 'Deleted by 000000000000', got 'Pending Acceptance by 000000000000'; poll: not in ['deleted'] after 56 attempts.
3. ec2-describe-aggregate-id-format-contract#task#1 DescribeAggregateIdFormat, 200->200 — statusSet.item[0]: expected {'deadline': '2018-08-13T02:00:00.000Z', 'resource': 'security-group', 'useLongIds': 'true'}, got {'resource': 'bundle', 'useLongIds': 'true'}; items [1]..[30] likewise diverge and no entry carries 'deadline'.
4. ec2-describe-capacity-reservations-contract#teardown#7 CancelCapacityReservation — status: expected 200, got 400.
5. ec2-catalog-get-managed-prefix-list-entries#task#1 GetManagedPrefixListEntries — status: expected 200, got 400; $.GetManagedPrefixListEntriesResponse: missing (expected an entrySet of 17 cidr items, got None); affected_count 16 (task#1..task#16 of the same fixture).
6. ec2-page-describe-instances-tag-filtered-two-pages#setup#11 DescribeInstances, 200->200 — $: poll capped at 60s, got {'DescribeInstancesResponse': {'reservationSet': '', ...}}; reservationSet.item[5].instancesSet.item.instanceState.name: poll not in ['running'] after 61 attempts.

(Recorded ids, account number, CIDR values and timestamps above are this run's values filling the templates, not requirements.)

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: replay ec2-accept-vpc-peering-connection-lifecycle step task#5 and inspect vpcPeeringConnection.requesterVpcInfo/accepterVpcInfo for peeringOptions.
3. Cause 2: replay the same fixture through teardown#8 — delete the peering connection, then poll DescribeVpcPeeringConnections for status.code 'deleted'.
4. Cause 3: replay ec2-describe-aggregate-id-format-contract step task#1 and compare the statusSet resource set and per-entry deadline with the recording.
5. Cause 4: replay ec2-describe-capacity-reservations-contract through teardown#7 — CancelCapacityReservation on the reservation created by the fixture must answer 200.
6. Cause 5: replay ec2-catalog-get-managed-prefix-list-entries step task#1 (and task#2..task#16) — GetManagedPrefixListEntries on an AWS-managed prefix list must answer 200 with its entrySet.
7. Cause 6: replay ec2-page-describe-instances-tag-filtered-two-pages step setup#11 — the launched reservations must appear and reach instanceState.name 'running' inside the poll budget.
