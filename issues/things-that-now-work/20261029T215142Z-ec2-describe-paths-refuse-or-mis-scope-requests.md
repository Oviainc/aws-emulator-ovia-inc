Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-describe-capacity-reservation-fleets-contract
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-describe-capacity-reservation-fleets-contract ec2-describe-capacity-reservations-contract ec2-on-demand-capacity-reservation-visibility ec2-describe-addresses-attribute-boundary ec2-describe-local-gateway-route-tables-contract ec2-default-vpc-and-subnet-bootstrap ec2-delete-launch-template-versions-partial-failure ec2-launch-template-versioning
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeCapacityReservationFleets|2xx->4xx|$
Issue-Key: ec2|DescribeCapacityReservationFleets|2xx->4xx|$.DescribeCapacityReservationFleetsResponse
Issue-Key: ec2|DescribeCapacityReservationFleets|2xx->4xx|$.DescribeCapacityReservationFleetsResponse.capacityReservationFleetSet.item.state
Issue-Key: ec2|DescribeCapacityReservationFleets|2xx->4xx|status
Issue-Key: ec2|DescribeCapacityReservations|2xx->4xx|$.DescribeCapacityReservationsResponse
Issue-Key: ec2|DescribeCapacityReservations|2xx->4xx|status
Issue-Key: ec2|DescribeAddressesAttribute|2xx->4xx|$.DescribeAddressesAttributeResponse
Issue-Key: ec2|DescribeAddressesAttribute|2xx->4xx|status
Issue-Key: ec2|DescribeLocalGatewayRouteTables|2xx->4xx|$.DescribeLocalGatewayRouteTablesResponse
Issue-Key: ec2|DescribeLocalGatewayRouteTables|2xx->4xx|status
Issue-Key: ec2|DescribeVpcs|2xx->2xx|$.DescribeVpcsResponse.vpcSet.item
Issue-Key: ec2|DescribeLaunchTemplateVersions|2xx->2xx|$.DescribeLaunchTemplateVersionsResponse.launchTemplateVersionSet.item
Issue-Commit: 14bf84ae7f57c35d094a91bae03d265103651cef

# EC2 describe paths refuse or mis-scope requests AWS answers 200

> _Note: this issue was identified against the code base at commit `14bf84ae7f57c35d094a91bae03d265103651cef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCapacityReservationFleets

## Description
Six net-new read clusters answer a request the recording answers 200: four refuse it with 400 and two answer 200 with the wrong result set (extra or missing members of the collection). All six are reported with an EMPTY owned_by and an empty stub_path, so no open issue and no scaffold stub covers them. There are 6 causes; all 6 must be fixed before this issue is closed.

Cause 1 of 6 - DescribeCapacityReservationFleets with a tag filter. AWS: a `tag:<key>` filter is a valid filter on every EC2 describe that supports Filters, and AWS answers 200 with the fleets whose tag matches. Emulator: answers 400 InvalidParameterValue, Message "The filter 'tag:<key>' is invalid", so the whole fixture's poll for the fleet never succeeds and caps at its budget. Why: tag-scoped lookups are the normal way clients find their own resources; they become unusable. Fix: the DescribeCapacityReservationFleets filter validation - accept the tag:<key> filter family (shared filter resolver) instead of rejecting unknown names.
Traces: ec2-describe-capacity-reservation-fleets-contract#task#2.

Cause 2 of 6 - DescribeCapacityReservations with the same tag filter. AWS: 200 with the matching capacityReservationSet. Emulator: 400, and the whole DescribeCapacityReservationsResponse is absent. Why: a client cannot read back a reservation it just tagged. Fix: same filter resolver, DescribeCapacityReservations path.
Traces: ec2-describe-capacity-reservations-contract#task#2.

Cause 3 of 6 - DescribeAddressesAttribute at its boundary case. AWS: 200 with an empty addressSet. Emulator: 400 and no response body. Why: the empty-result case of a read is refused, so a client that lists attributes before setting any gets an error instead of an empty list. Fix: the DescribeAddressesAttribute handler's empty/boundary path.
Traces: ec2-describe-addresses-attribute-boundary#task#3.

Cause 4 of 6 - DescribeLocalGatewayRouteTables with no matching route table. AWS: 200 with an empty localGatewayRouteTableSet. Emulator: 400 and no response body. Why: same shape of defect - an empty read is turned into a client error. Fix: the DescribeLocalGatewayRouteTables handler's empty-result path.
Traces: ec2-describe-local-gateway-route-tables-contract#task#2.

Cause 5 of 6 - DescribeVpcs after default-VPC bootstrap. AWS: exactly one default VPC exists per region, so the filtered read returns a single vpcSet.item (isDefault true, the account's default CIDR). Emulator: returns two VPCs, both isDefault true with the same CIDR - the bootstrap seeds a default VPC and the fixture's own default-VPC creation adds another. Why: a client resolving 'the' default VPC gets two candidates and picks the wrong one; downstream subnet/route-table work targets the wrong VPC. Fix: default-VPC creation/bootstrap in the ec2 crate's state - creation must be idempotent against the existing default VPC rather than appending.
Traces: ec2-default-vpc-and-subnet-bootstrap#task#1.

Cause 6 of 6 - DescribeLaunchTemplateVersions after a second version is created. AWS: 200 listing every version (the recording shows versionNumber 2 then 1, newest first, each with its own launchTemplateData). Emulator: returns only version 1, as a single item rather than a list. Why: a client enumerating or diffing template versions cannot see the version it just created, and version-pinned launches resolve to the wrong data. Fix: the launch-template version store and the DescribeLaunchTemplateVersions projection (list serialization plus version ordering).
Traces: ec2-delete-launch-template-versions-partial-failure#task#4.

## Evidence
Replay of simparity-wire-aws-v1; all six clusters verdict=mismatch, stub_path empty, owned_by empty.

1) ec2-describe-capacity-reservation-fleets-contract#task#2 DescribeCapacityReservationFleets: status expected 200, got 400; body got {'Response': {'Errors': {'Error': {'Code': 'InvalidParameterValue', 'Message': "The filter 'tag:sim2real-fixture' is invalid"}}}}; the step's poll capped at 60s because the 200 body never arrived.
2) ec2-describe-capacity-reservations-contract#task#2 DescribeCapacityReservations: status expected 200, got 400; $.DescribeCapacityReservationsResponse missing (expected a capacityReservationSet.item with the reservation's arn/id/state/tagSet). Cluster also covers #task#6 and ec2-on-demand-capacity-reservation-visibility#task#2.
3) ec2-describe-addresses-attribute-boundary#task#3 DescribeAddressesAttribute: status expected 200, got 400; $.DescribeAddressesAttributeResponse missing (expected addressSet '').
4) ec2-describe-local-gateway-route-tables-contract#task#2 DescribeLocalGatewayRouteTables: status expected 200, got 400; $.DescribeLocalGatewayRouteTablesResponse missing (expected localGatewayRouteTableSet '').
5) ec2-default-vpc-and-subnet-bootstrap#task#1 DescribeVpcs: $.DescribeVpcsResponse.vpcSet.item type_mismatch - expected a single default VPC object, got a two-element list (two vpcIds, both isDefault 'true', both cidrBlock 172.31.0.0/16).
6) ec2-delete-launch-template-versions-partial-failure#task#4 DescribeLaunchTemplateVersions: $.DescribeLaunchTemplateVersionsResponse.launchTemplateVersionSet.item type_mismatch - expected a two-element list (versionNumber '2' then '1'), got the single version-1 object. Cluster also covers ec2-launch-template-versioning#task#5.

The tag key, generated ids and CIDR above are the recorded run's values; the requirement is that the tag filter is honored, empty reads answer 200 with an empty set, exactly one default VPC exists, and every launch-template version is listed.

## Steps to Reproduce
1. Boot the emulator built from this checkout.
2. Replay trace set simparity-wire-aws-v1.
3. Inspect fixture ec2-describe-capacity-reservation-fleets-contract step task#2 and the five fixture#step bindings in trace_ids.
4. Four steps report a status divergence (expected 200, got 400) with the response body absent; two report a type_mismatch on the collection member.
