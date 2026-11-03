Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-delete-placement-group ec2.op-contract-describe-placement-groups ec2.op-contract-describe-dhcp-options ec2.op-contract-describe-stale-security-groups-notfound-rejected ec2.op-contract-disable-address-transfer ec2.op-contract-delete-secondary-network ec2.op-contract-delete-subnet-cidr-reservation
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.op-contract-delete-placement-group|run#14|AssertionError|assert message == f"The placement group '{name}' is unknown.", message
Issue-Key: ec2.op-contract-describe-placement-groups|run#24|AssertionError|assert redelete_message == f"The placement group '{name}' is unknown.", redelete_message
Issue-Key: ec2.op-contract-describe-dhcp-options|run#12|AssertionError|assert not_found_message == "The dhcpOption ID 'dopt-00000000000000000' does not exist", not_found_message
Issue-Key: ec2.op-contract-describe-stale-security-groups-notfound-rejected|run#11|AssertionError|assert message == f"{fake_vpc} is not found for user.", message
Issue-Key: ec2.op-contract-disable-address-transfer|run#12|AssertionError|assert described == [], f"expected no transfer after disable, got {described}"
Issue-Related: 20261029T175114Z-ec2-accepts-with-200-a-range-of-requests.md
Issue-Related: 20261029T175114Z-ec2-refusal-messages-are-emulator-authored-prose-instead.md
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# EC2 placement-group, DHCP-options, stale-security-group, address-transfer and secondary-network paths: emulator-authored refusal text and non-AWS post-state (6 root causes)

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribePlacementGroups

## Description
Six further net-new root causes under `### ec2`, none marked ALREADY FILED or OWNED by. The EC2 model is ec2Query with no modelled per-operation errors, so each code/message is the recorded AWS behavior.

Cause 1 of 6: placement-group not-found refusals use the emulator's own sentence and capitalisation.
AWS: recorded behavior only. DeletePlacementGroup / DescribePlacementGroups on an unknown group is refused with AWS's placement-group wording, lower-cased resource noun, and the group NAME from the request as the only variable.
Emulator: answers `The Placement Group '<name>' is unknown.` (capitalised noun, emulator punctuation) on both the delete and the describe path.
Why: the message is what a caller parses to tell "this group never existed" from a permission or shape error; the divergent template breaks that and leaks a non-AWS style into logs.
Fix: the not-found message template shared by the DeletePlacementGroup and DescribePlacementGroups handlers.
Traces: ec2.op-contract-delete-placement-group, ec2.op-contract-describe-placement-groups

Cause 2 of 6: DescribeDhcpOptions' unknown-id refusal message diverges.
AWS: recorded behavior only. An unknown DHCP options set id is refused with AWS's own text; the id in the request is the only variable.
Emulator: answers `The dhcpOptions ID '<dopt-id>' does not exist` - the emulator's generic "<resource> ID '<id>' does not exist" template rather than AWS's.
Why: message-keyed error handling and byte-exact assertions on the DHCP-options path fail.
Fix: the not-found message template in the DescribeDhcpOptions handler.
Traces: ec2.op-contract-describe-dhcp-options

Cause 3 of 6: DescribeStaleSecurityGroups' unknown-VPC refusal message diverges.
AWS: recorded behavior only. An unknown VpcId is refused with AWS's own not-found text for the VPC, with the id taken from the request.
Emulator: answers `The vpc ID '<vpc-id>' does not exist` - again the generic emulator template.
Why: same as above; a caller that distinguishes a missing VPC from a missing permission by message cannot.
Fix: the VPC-resolution message template in the DescribeStaleSecurityGroups handler.
Traces: ec2.op-contract-describe-stale-security-groups-notfound-rejected

Cause 4 of 6: DisableAddressTransfer leaves the transfer enumerable.
AWS: recorded behavior only. After DisableAddressTransfer, DescribeAddressTransfers reports NO transfer for that allocation - the offer is withdrawn, not re-labelled.
Emulator: keeps the entry and merely sets AddressTransferStatus='disabled', still carrying TransferAccountId and TransferOfferExpirationTimestamp.
Why: a client that lists address transfers to decide whether an EIP is still offered to another account sees a live-looking offer AWS has already removed, and will not re-enable or re-associate.
Fix: the DisableAddressTransfer handler's state mutation and the DescribeAddressTransfers projection.
Traces: ec2.op-contract-disable-address-transfer

Cause 5 of 6: DeleteSecondaryNetwork accepts a delete AWS refuses and reports a terminal state immediately.
AWS: recorded behavior only. The delete this check issues is refused; there is no success path.
Emulator: returns HTTP 200 with State='delete-complete' and the CIDR association already 'disassociated'.
Why: the caller believes a refused delete succeeded and stops polling / skips the cleanup AWS requires.
Fix: the precondition guard in the DeleteSecondaryNetwork handler.
Traces: ec2.op-contract-delete-secondary-network

Cause 6 of 6: subnet-CIDR-reservation overlap refusal uses the wrong template.
AWS: recorded behavior only. An overlapping subnet CIDR reservation is refused with `Reservation CIDR <cidr> has overlap with existing reserved CIDR block` - fixed text with the requested CIDR as the only variable.
Emulator: answers `Value (<cidr>) for parameter cidr is invalid. The CIDR overlaps the existing subnet CIDR reservation '<scr-id>'.` - a different sentence that also exposes an internal reservation id the caller never supplied.
Why: byte-exact message handling fails, and the leaked id is a per-run value a client cannot rely on.
Fix: the overlap-detection message template in the CreateSubnetCidrReservation / DeleteSubnetCidrReservation path. (The open issue 20261029T175114Z-ec2-refusal-messages-are-emulator-authored-prose-instead names this check; it is recorded here because the delete-side check below is additionally unmarked in this replay.)
Traces: ec2.op-contract-delete-subnet-cidr-reservation

## Evidence
Replayed failing checks under `### ec2`:

1. ec2.op-contract-delete-placement-group -> AssertionError: The Placement Group 'sim2real-behav-pg-b60218c5030d' is unknown.
   ec2.op-contract-describe-placement-groups -> AssertionError: The Placement Group 'sim2real-behav-pg-6b36e147d232' is unknown.
2. ec2.op-contract-describe-dhcp-options -> AssertionError: The dhcpOptions ID 'dopt-00000000000000000' does not exist
3. ec2.op-contract-describe-stale-security-groups-notfound-rejected -> AssertionError: The vpc ID 'vpc-ffffffff' does not exist
4. ec2.op-contract-disable-address-transfer -> AssertionError: expected no transfer after disable, got [{'PublicIp': '54.0.0.2', 'AllocationId': 'eipalloc-00000000000000024', 'TransferAccountId': '111122223333', 'TransferOfferExpirationTimestamp': datetime.datetime(2026, 11, 10, ...), 'AddressTransferStatus': 'disabled'}]
5. ec2.op-contract-delete-secondary-network -> AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'SecondaryNetwork': {'SecondaryNetworkId': 'secnet-...', 'Type': 'rdma', 'State': 'delete-complete', 'Ipv4CidrBlockAssociations': [{... 'State': 'disassociated'}], ...}, ... 'HTTPStatusCode': 200 ...}
6. ec2.op-contract-delete-subnet-cidr-reservation -> AssertionError: expected message 'Reservation CIDR 10.98.1.0/28 has overlap with existing reserved CIDR block', got "Value (10.98.1.0/28) for parameter cidr is invalid. The CIDR overlaps the existing subnet CIDR reservation 'scr-0000000000000002d'."

## Steps to Reproduce
1. git_clone, then run_behavioral_suite; locate the checks in trace_ids under `### ec2`.
2. Standalone repros:
   - DeletePlacementGroup(GroupName='does-not-exist') and DescribePlacementGroups(GroupNames=['does-not-exist']): compare the message to AWS's placement-group not-found text (lower-cased noun, no emulator punctuation).
   - DescribeDhcpOptions(DhcpOptionsIds=['dopt-00000000000000000']): compare the message to AWS's.
   - DescribeStaleSecurityGroups(VpcId='vpc-ffffffff'): compare the message to AWS's.
   - AllocateAddress, EnableAddressTransfer to another account, DisableAddressTransfer, then DescribeAddressTransfers: the list must be empty.
   - CreateSecondaryNetwork, then DeleteSecondaryNetwork under the check's precondition: expect a refusal, never an immediate 'delete-complete'.
   - CreateSubnetCidrReservation twice with overlapping CIDRs: expect `Reservation CIDR <cidr> has overlap with existing reserved CIDR block`.
