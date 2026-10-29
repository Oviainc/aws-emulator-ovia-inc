Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-attachImageWatermark ec2.op-contract-describe-launch-template-versions-not-found-rejected-superseded-stub ec2.op-contract-get-capacity-reservation-usage ec2.op-contract-move-capacity-reservation-instances-rejected ec2.describe-transit-gateway-id-not-found-rejected ec2.describe-managed-prefix-lists-contract ec2.describe-network-insights-paths-contract ec2.describe-transit-gateway-route-tables-max-results-out-of-range-rejected ec2.op-contract-describe-byoip-cidrs-duplicate ec2.ipam-discovered-accounts-max-results-below-minimum-rejected ec2.op-contract-get-vpn-connection-device-types ec2.op-contract-allocateHosts ec2.op-contract-create-restore-image-task-rejected ec2.op-contract-create-secondary-network ec2.create-instance-event-window-contract ec2.instance-event-window-lifecycle ec2.create-fpga-image-invalid-bucket-rejected ec2.op-contract-describeVolumeAttribute ec2.op-contract-delete-coip-pool ec2.op-contract-modify-capacity-reservation-fleet-state-conflict ec2.op-contract-move-byoip-cidr-to-ipam-rejected ec2.op-contract-describe-instance-event-windows ec2.op-contract-associate-dhcp-options ec2.enable-address-transfer-contract ec2.op-contract-modify-volume ec2.op-contract-deprovision-ipam-byoasn-invalid-ipam-rejected ec2.op-contract-import-image
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.op-contract-attachImageWatermark|run#4|AssertionError|assert code == "InvalidAMIID.Malformed", f"expected InvalidAMIID.Malformed, got {code}"
Issue-Key: ec2.op-contract-describe-launch-template-versions-not-found-rejected-superseded-stub|run#17|AssertionError|assert code == "InvalidLaunchTemplateId.Malformed", f"expected Malformed, got {code!r}"
Issue-Key: ec2.op-contract-get-capacity-reservation-usage|run#12|AssertionError|assert code == "InvalidCapacityReservationId.Malformed", (
Issue-Key: ec2.op-contract-move-capacity-reservation-instances-rejected|run#9|AssertionError|assert err3 == "InvalidCapacityReservationId.Malformed", (
Issue-Key: ec2.describe-transit-gateway-id-not-found-rejected|run#10|AssertionError|assert code_malformed == "InvalidTransitGatewayID.Malformed", (
Issue-Key: ec2.describe-managed-prefix-lists-contract|run#10|AssertionError|assert code == "InvalidMaxResults", f"expected InvalidMaxResults, got {code!r}"
Issue-Key: ec2.describe-network-insights-paths-contract|run#8|AssertionError|assert code == "InvalidMaxResults", f"expected InvalidMaxResults, got {code!r}"
Issue-Key: ec2.describe-transit-gateway-route-tables-max-results-out-of-range-rejected|run#6|AssertionError|assert code == "InvalidParameterValue", (
Issue-Key: ec2|DescribeByoipCidrs|ValidationError
Issue-Key: ec2.ipam-discovered-accounts-max-results-below-minimum-rejected|run#6|AssertionError|assert message == "The number of requested max results must be between 100 and 1000.", f"unexpected InvalidMaxResults message {message!r}"
Issue-Key: ec2.op-contract-get-vpn-connection-device-types|run#12|AssertionError|assert message == "MaxResults must be greater than 200.", (
Issue-Key: ec2.op-contract-allocateHosts|run#13|AssertionError|assert code2 == "InvalidRequest", f"expected InvalidRequest, got {code2!r}"
Issue-Key: ec2.op-contract-create-restore-image-task-rejected|run#9|AssertionError|assert code == "InvalidRequest", f"expected InvalidRequest, got {code!r}"
Issue-Key: ec2.op-contract-create-secondary-network|run#11|AssertionError|assert code == "InvalidSecondaryNetwork.Range", (
Issue-Key: ec2.create-instance-event-window-contract|run#14|AssertionError|assert error.get("Code") == "InvalidInstanceEventWindowCronExpression", (
Issue-Key: ec2.instance-event-window-lifecycle|run#4|AssertionError|assert both == "InvalidParameterValue", (
Issue-Key: ec2.create-fpga-image-invalid-bucket-rejected|run#9|AssertionError|assert key_code == "InvalidParameterValue", (
Issue-Key: ec2.op-contract-describeVolumeAttribute|run#26|AssertionError|assert code_malformed == "InvalidParameterValue", (
Issue-Key: ec2.op-contract-delete-coip-pool|run#7|AssertionError|assert code == "InvalidIpv4PoolCoipId.Malformed", f"expected InvalidIpv4PoolCoipId.Malformed, got {code!r}"
Issue-Key: ec2.op-contract-modify-capacity-reservation-fleet-state-conflict|run#20|AssertionError|assert conflict_code == "InvalidCapacityReservationFleetStateTransition", (
Issue-Key: ec2.op-contract-move-byoip-cidr-to-ipam-rejected|run#4|AssertionError|assert code == "InvalidIpamPoolId.NotFound", f"expected InvalidIpamPoolId.NotFound, got {code}"
Issue-Key: ec2.op-contract-describe-instance-event-windows|run#19|AssertionError|assert code == "InvalidInstanceEventWindowIDNotFound", (
Issue-Key: ec2.op-contract-associate-dhcp-options|run#15|AssertionError|assert code == "InvalidDhcpOptionsId.Malformed", f"unexpected error code: {code}"
Issue-Key: ec2.enable-address-transfer-contract|run#17|AssertionError|assert code == "InvalidElasticIpID.NotFound", f"unexpected error code: {code}"
Issue-Key: ec2.op-contract-modify-volume|run#17|AssertionError|assert code == "InvalidVolumeID.Malformed", f"unexpected error code: {code}"
Issue-Key: ec2.op-contract-deprovision-ipam-byoasn-invalid-ipam-rejected|run#5|AssertionError|assert error.get("Message") == f"The ipam ID '{bogus_ipam_id}' does not exist", error
Issue-Key: ec2.op-contract-import-image|run#17|AssertionError|assert code == "InvalidConversionTaskId", f"unexpected error code {code!r}"
Issue-Commit: abd75e982d55cd9e91541cb3d125feb2d8edb60f

# EC2 client-error classification: wrong error codes for malformed-vs-unknown ids, out-of-range MaxResults and several per-operation validations (8 causes)

> _Note: this issue was identified against the code base at commit `abd75e982d55cd9e91541cb3d125feb2d8edb60f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeManagedPrefixLists

## Description
Eight root causes, all in how EC2 classifies a refusal into an error code. In every case the emulator refuses (so the request is not silently accepted) but with a code no AWS client is written to catch.

Cause 1 of 8: a syntactically INVALID resource id is classified as unknown-resource instead of malformed.
AWS: recorded behavior only. EC2 splits id refusals in two: an id that does not match the resource's id grammar is `Invalid<Resource>ID.Malformed`; an id that matches the grammar but names nothing is `Invalid<Resource>ID.NotFound`. The message for the Malformed class is `Invalid id: "<the id as sent>"` (fixed prefix, the rejected id is the only variable part).
Emulator: answers the NotFound code for the malformed input — `InvalidAMIID.NotFound` for an AMI id of the wrong shape, `InvalidLaunchTemplateId.NotFound`, `InvalidCapacityReservationId.NotFound`, `InvalidTransitGatewayID.NotFound` for a mismatched-prefix transit-gateway id.
Why: callers branch on Malformed (fix your input) vs NotFound (create the resource / retry). Collapsing them makes a permanent input error look like a transient absence, so retry loops spin.
Fix: the shared id-parsing/validation layer that mints these codes, and the per-resource id grammars it consults.
Traces: ec2.op-contract-attachImageWatermark, ec2.op-contract-describe-launch-template-versions-not-found-rejected-superseded-stub, ec2.op-contract-get-capacity-reservation-usage, ec2.op-contract-move-capacity-reservation-instances-rejected, ec2.describe-transit-gateway-id-not-found-rejected.

Cause 2 of 8: out-of-range MaxResults is refused with a generic `ValidationError` instead of EC2's `InvalidMaxResults` / `InvalidParameterValue`.
AWS: Smithy models the bound — DescribeManagedPrefixLists.MaxResults targets PrefixListMaxResults with @range min 1 max 100; DescribeVpcBlockPublicAccessExclusions.MaxResults @range min 5 max 1000; DescribeTransitGatewayRouteTables documents a maximum of 1000. A value outside the range is refused with `InvalidMaxResults` (or `InvalidParameterValue` where the operation uses that code), 400.
Emulator: answers code `ValidationError`, a code the EC2 surface does not use for this.
Why: paginating clients that clamp on InvalidMaxResults never see it and surface an unmodelled error instead.
Fix: the MaxResults boundary validation shared by the Describe* handlers.
Traces: ec2.describe-managed-prefix-lists-contract, ec2.describe-network-insights-paths-contract, ec2.describe-transit-gateway-route-tables-max-results-out-of-range-rejected, ec2.op-contract-describe-byoip-cidrs-duplicate, ec2.ipam-discovered-accounts-max-results-below-minimum-rejected, ec2.op-contract-get-vpn-connection-device-types.

Cause 3 of 8: AllocateHosts refuses an unsatisfiable host request as `InvalidParameterValue` instead of `InvalidRequest`.
AWS: recorded behavior only — the refusal code is `InvalidRequest`.
Emulator: `InvalidParameterValue`.
Why: callers that treat InvalidParameterValue as a per-member input bug look for a bad member that does not exist.
Fix: AllocateHosts' validation.
Traces: ec2.op-contract-allocateHosts.

Cause 4 of 8: CreateRestoreImageTask classifies an unreachable S3 location as `InaccessibleStorageLocation` where AWS answers `InvalidRequest`; CreateFpgaImage classifies an InputStorageLocation missing its Key as `MissingParameter` where AWS answers `InvalidParameterValue`.
AWS: recorded behavior only; codes as stated above.
Emulator: codes as stated above.
Why: both are S3-location admission paths; a client cannot tell a malformed location from an inaccessible one.
Fix: the storage-location validation shared by the image-task handlers.
Traces: ec2.op-contract-create-restore-image-task-rejected, ec2.create-fpga-image-invalid-bucket-rejected.

Cause 5 of 8: the instance-event-window validations use the wrong codes in both directions.
AWS: recorded behavior only. A cron expression describing a window shorter than the 4-hour minimum is refused with `InvalidInstanceEventWindowCronExpression`; a request naming both TimeRanges and CronExpression is refused with `InvalidParameterValue`; an unknown window id is refused with code `InvalidInstanceEventWindowIDNotFound` (no dot between ID and NotFound in this family).
Emulator: answers `InvalidParameterValue` for the sub-4-hour cron, `InvalidParameterCombination` for TimeRanges+CronExpression, and `InvalidInstanceEventWindowId.NotFound` for the unknown id.
Why: three codes clients catch by name are unreachable.
Fix: the instance-event-window handlers' validation and their error-code table.
Traces: ec2.create-instance-event-window-contract, ec2.instance-event-window-lifecycle, ec2.op-contract-describe-instance-event-windows.

Cause 6 of 8: pool/IPAM/BYOIP id refusals use a neighbouring resource's code.
AWS: recorded behavior only. DeleteCoipPool refuses a malformed COIP pool id with `InvalidIpv4PoolCoipId.Malformed`; MoveByoipCidrToIpam refuses an unknown pool with `InvalidIpamPoolId.NotFound`; CreateSecondaryNetwork refuses an out-of-range range with `InvalidSecondaryNetwork.Range`.
Emulator: answers `InvalidPoolID.Malformed`, `InvalidParameterValue` and `InvalidParameterValue` respectively.
Why: resource-specific handling (create the pool, widen the range) is keyed off the specific code.
Fix: the COIP/IPAM/secondary-network handlers' error-code selection.
Traces: ec2.op-contract-delete-coip-pool, ec2.op-contract-move-byoip-cidr-to-ipam-rejected, ec2.op-contract-create-secondary-network, ec2.op-contract-deprovision-ipam-byoasn-invalid-ipam-rejected.

Cause 7 of 8: volume and address operations answer an id-shape code where AWS answers a parameter/state code, and vice versa.
AWS: recorded behavior only. DescribeVolumeAttribute with an unsupported Attribute value is refused with `InvalidParameterValue`; ModifyVolume on a volume that exists but cannot take the modification is refused with a state code, not `InvalidVolume.NotFound`; EnableAddressTransfer for an allocation that cannot be transferred is not refused with `InvalidAllocationID.NotFound`; AssociateDhcpOptions is not refused with `InvalidDhcpOptionID.NotFound` for the case under test.
Emulator: answers `InvalidVolumeID.Malformed`, `InvalidVolume.NotFound`, `InvalidAllocationID.NotFound` and `InvalidDhcpOptionID.NotFound` respectively.
Why: the caller is told the resource is absent when it exists, so it recreates resources it already has.
Fix: the volume/address/dhcp-options handlers' validation ordering and code selection.
Traces: ec2.op-contract-describeVolumeAttribute, ec2.op-contract-modify-volume, ec2.enable-address-transfer-contract, ec2.op-contract-associate-dhcp-options.

Cause 8 of 8: capacity-reservation-fleet and import-image refusals use unmodelled codes.
AWS: recorded behavior only. Modifying a cancelled capacity-reservation fleet is refused with `InvalidCapacityReservationFleetStateTransition`; ImportImage's refusal under test is not `InvalidID`.
Emulator: answers `IncorrectState` and `InvalidID` respectively.
Why: state-transition retry logic keyed on the specific transition code never fires.
Fix: the capacity-reservation-fleet and import-image handlers.
Traces: ec2.op-contract-modify-capacity-reservation-fleet-state-conflict, ec2.op-contract-import-image.

## Evidence
Replay output, verbatim diagnosis lines, grouped by cause.
Cause 1: `AssertionError: expected InvalidAMIID.Malformed, got InvalidAMIID.NotFound`; `expected Malformed, got 'InvalidLaunchTemplateId.NotFound'`; `expected InvalidCapacityReservationId.Malformed, got 'InvalidCapacityReservationId.NotFound'`; `expected InvalidCapacityReservationId.Malformed for a well-formed but nonexistent reservation id, got InvalidCapacityReservationId.NotFound`; `expected InvalidTransitGatewayID.Malformed for mismatched-prefix id, got 'InvalidTransitGatewayID.NotFound'`.
Cause 2: `expected InvalidMaxResults, got 'ValidationError'` (twice); `expected InvalidParameterValue for MaxResults=100000 (above documented maximum of 1000), got 'ValidationError'`; `expected InvalidMaxResults for MaxResults=101, got 'ValidationError'`; `unexpected error for out-of-range MaxResults: 'Value ( 1 ) for parameter maxResults is invalid. Expecting a value greater than or equal to 200.'`.
Cause 3: `expected InvalidRequest, got 'InvalidParameterValue'`.
Cause 4: `expected InvalidRequest, got 'InaccessibleStorageLocation'`; `expected CreateFpgaImage to reject an InputStorageLocation missing Key with InvalidParameterValue, got 'MissingParameter'`.
Cause 5: `expected InvalidInstanceEventWindowCronExpression for a sub-4-hour window, got 'InvalidParameterValue'`; `expected InvalidParameterValue for TimeRanges+CronExpression, got InvalidParameterCombination`; `expected InvalidInstanceEventWindowIDNotFound, got InvalidInstanceEventWindowId.NotFound`.
Cause 6: `expected InvalidIpv4PoolCoipId.Malformed, got 'InvalidPoolID.Malformed'`; `expected InvalidIpamPoolId.NotFound, got InvalidParameterValue`; `expected InvalidSecondaryNetwork.Range, got 'InvalidParameterValue'`; `AssertionError: {'Code': 'InvalidIpamId.NotFound', 'Message': 'The IPAM (ipam-00000000000000000) does not exist.'}`.
Cause 7: `expected InvalidParameterValue, got 'InvalidVolumeID.Malformed'`; `unexpected error code: InvalidVolume.NotFound`; `unexpected error code: InvalidAllocationID.NotFound`; `unexpected error code: InvalidDhcpOptionID.NotFound`.
Cause 8: `expected InvalidCapacityReservationFleetStateTransition modifying a cancelled fleet, got 'IncorrectState'`; `unexpected error code 'InvalidID'`.
The resource ids and numeric limits quoted above are values from the request/state; the code names and the fixed `Invalid id: "..."` prefix are the requirement.

## Steps to Reproduce
1. Boot the emulator; point an EC2 client at it.
2. Cause 1: DescribeImages/AttachImageWatermark with an AMI id of the wrong shape ('ami-1234567890abcdef0' against an id space that does not contain it) — observe InvalidAMIID.NotFound instead of InvalidAMIID.Malformed. Same for DescribeTransitGateways with a mismatched-prefix id.
3. Cause 2: DescribeManagedPrefixLists(MaxResults=101) and DescribeTransitGatewayRouteTables(MaxResults=100000) — observe ValidationError instead of InvalidMaxResults / InvalidParameterValue.
4. Cause 5: CreateInstanceEventWindow with a cron expression yielding a window under 4 hours, then one naming both TimeRanges and CronExpression, then DescribeInstanceEventWindows with an unknown id.
5. Causes 3, 4, 6, 7, 8: issue the corresponding call named in each cause's Traces line.
6. Re-replay the check ids in trace_ids.
