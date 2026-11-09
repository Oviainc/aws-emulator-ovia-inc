Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-describe-nat-gateways-pagination
Issue-Step: task#18
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-describe-nat-gateways-pagination ec2-describe-network-interfaces-vpc-filter-exact-set ec2-mr-inv-describe-regions-enumerates-scenario-regions ec2-mr-inv-describe-regions-rejects-unknown-region-name ec2-describe-images-self-owned-pagination-and-params
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeNatGateways|2xx->2xx|$.DescribeNatGatewaysResponse.natGatewaySet.item[*]
Issue-Key: ec2|DescribeNatGateways|2xx->2xx|$.DescribeNatGatewaysResponse.natGatewaySet.item[subnetId="subnet-00000000000000089"].attachedApplianceSet
Issue-Key: ec2|DescribeNatGateways|2xx->2xx|$.DescribeNatGatewaysResponse.natGatewaySet.item[subnetId="subnet-0000000000000008b"].attachedApplianceSet
Issue-Key: ec2|DescribeNatGateways|2xx->2xx|$.DescribeNatGatewaysResponse.natGatewaySet.item[subnetId="subnet-0000000000000008d"].attachedApplianceSet
Issue-Key: ec2|DescribeNatGateways|2xx->2xx|$.DescribeNatGatewaysResponse.natGatewaySet.item[subnetId="subnet-0000000000000008f"].attachedApplianceSet
Issue-Key: ec2|DescribeNatGateways|2xx->2xx|$.DescribeNatGatewaysResponse.natGatewaySet.item[subnetId="subnet-00000000000000091"].attachedApplianceSet
Issue-Key: ec2|DescribeNatGateways|2xx->2xx|$.DescribeNatGatewaysResponse.natGatewaySet.item[subnetId="subnet-00000000000000093"].attachedApplianceSet
Issue-Key: ec2|DescribeNatGateways|2xx->2xx|$.DescribeNatGatewaysResponse.natGatewaySet.item[subnetId="subnet-00000000000000095"].attachedApplianceSet
Issue-Key: ec2|DescribeNetworkInterfaces|2xx->2xx|$.DescribeNetworkInterfacesResponse.networkInterfaceSet.item[networkInterfaceId="eni-0000000000000008b"].publicDnsName
Issue-Key: ec2|DescribeNetworkInterfaces|2xx->2xx|$.DescribeNetworkInterfacesResponse.networkInterfaceSet.item[networkInterfaceId="eni-0000000000000008c"].publicDnsName
Issue-Key: ec2|DescribeNetworkInterfaces|2xx->2xx|$.DescribeNetworkInterfacesResponse.networkInterfaceSet.item[networkInterfaceId="eni-0000000000000008d"].publicDnsName
Issue-Key: ec2|DescribeNetworkInterfaces|2xx->2xx|$.DescribeNetworkInterfacesResponse.networkInterfaceSet.item[networkInterfaceId="eni-0000000000000008e"].publicDnsName
Issue-Key: ec2|DescribeNetworkInterfaces|2xx->2xx|$.DescribeNetworkInterfacesResponse.networkInterfaceSet.item[networkInterfaceId="eni-0000000000000008f"].publicDnsName
Issue-Key: ec2|DescribeNetworkInterfaces|2xx->2xx|$.DescribeNetworkInterfacesResponse.networkInterfaceSet.item[networkInterfaceId="eni-00000000000000090"].publicDnsName
Issue-Key: ec2|DescribeRegions|2xx->2xx|$.DescribeRegionsResponse.regionInfo.item[*].regionEndpoint
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.nextToken
Issue-Related: 20261023T155030Z-ec2-static-catalogs-list-entries-aws-does-not.md
Issue-Commit: 3b171590106c1f2d83787caf96ecc291950ccc13

# EC2 describe reads omit modeled members, publish foreign region endpoints and mispage self-owned images

> _Note: this issue was identified against the code base at commit `3b171590106c1f2d83787caf96ecc291950ccc13`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeNatGateways

## Description
This issue bundles FOUR distinct causes in EC2 read paths. All FOUR must be closed before submitting.

--- Cause 1 of 4: DescribeNatGateways omits attachedApplianceSet and injects per-address AZ members AWS does not return ---
AWS: Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json): shape NatGateway models AttachedAppliances (xmlName attachedApplianceSet). Recorded behavior: every natGatewaySet row carries attachedApplianceSet, emitted as an EMPTY element when no proxy appliance is attached. Shape NatGatewayAddress also models availabilityZone and availabilityZoneId, but the recording shows AWS returning a private NAT gateway's natGatewayAddressSet item with only isPrimary, networkInterfaceId, privateIp and status — the AZ members are not populated for this case.
Emulator: attachedApplianceSet is absent from every row, and each natGatewayAddressSet item carries extra availabilityZone / availabilityZoneId members. Replay reports the whole item[0] as a value_mismatch on the first page and, on the id-filtered read, 'attachedApplianceSet: missing (expected "", got None)' for all seven rows.
Why: absent is not the same as empty — a client that reads attachedApplianceSet to decide whether traffic is being inspected cannot distinguish 'no appliances' from 'this API does not report appliances', and strict/XML-schema-driven deserializers treat a missing modeled member differently from an empty list. The invented AZ members make a zonal private NAT gateway look like it is pinned to an EIP in a specific AZ.
Fix: the NatGateway projection/serializer in services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_nat_gateways.rs (and the NAT-gateway state record) — always emit attachedApplianceSet, and populate NatGatewayAddress AZ members only where AWS does.
Traces: ec2-describe-nat-gateways-pagination#task#18 (cluster also covers #task#19) and ec2-describe-nat-gateways-pagination#task#20.

--- Cause 2 of 4: DescribeNetworkInterfaces omits publicDnsName from every networkInterfaceSet row ---
AWS: recorded behavior only (DescribeNetworkInterfaces declares no modeled errors and the publicDnsName element is not in this model snapshot's NetworkInterface structure). The recording shows AWS emitting publicDnsName on EVERY networkInterfaceSet item, as an EMPTY element for an interface with no public DNS name.
Emulator: the member is absent from all six rows — 'publicDnsName: missing (expected "", got None)'.
Why: a client that reads publicDnsName to decide whether an interface is publicly addressable sees nothing and cannot tell 'no public name' from 'not reported'; code that indexes the field directly fails on a missing key rather than reading an empty string.
Fix: the NetworkInterface projection/serializer in services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_network_interfaces.rs — always emit publicDnsName, empty when the interface has none.
Traces: ec2-describe-network-interfaces-vpc-filter-exact-set#task#10.

--- Cause 3 of 4: DescribeRegions publishes regionEndpoint hostnames the emulator does not serve, and the wrong region's endpoint for one row ---
AWS: Smithy shape Region member Endpoint (xmlName regionEndpoint) — 'The Region service endpoint'; on real AWS each row's endpoint is that region's own service endpoint, so the value is per-row and derived from the row's regionName, never a constant.
Emulator: every regionInfo row carries a real-AWS hostname of the form ec2.<region>.amazonaws.com, which the replay flags host_not_own — the emulator announces an address other than the one it is reached on, so a client that follows the advertised endpoint leaves the emulated environment entirely. Worse, the row for the region the fixture asserts carries the WRONG region's endpoint: 'regionEndpoint: host_not_own (expected "ec2.ap-southeast-7.amazonaws.com", got "ec2.us-west-2.amazonaws.com")' — the handler falls back to the request's own region instead of deriving the endpoint from each row's regionName.
Why: DescribeRegions is exactly the call multi-region clients use to discover where to send subsequent requests. Both halves break that: a client either sends traffic to real AWS (and fails or, worse, succeeds against a live account) or sends every region's traffic to one region's endpoint and silently operates on the wrong region's resources.
Fix: the DescribeRegions handler and its region table in services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_regions.rs — derive regionEndpoint per row from that row's regionName, and resolve it to the address the emulator actually serves rather than to amazonaws.com.
Traces: ec2-mr-inv-describe-regions-enumerates-scenario-regions#task#1, ec2-mr-inv-describe-regions-enumerates-scenario-regions#task#2, ec2-mr-inv-describe-regions-rejects-unknown-region-name#task#4.

--- Cause 4 of 4: the self-owned, paginated DescribeImages read returns an imagesSet of the wrong shape and no continuation token ---
AWS: Smithy: DescribeImages' recorded behavior for Owners=self with MaxResults is a page of imagesSet items — each with imageId, imageLocation '<ownerId>/<name>', imageState 'available', imageOwnerId, creationDate, isPublic 'false', architecture, imageType 'machine', name, rootDeviceType 'ebs', rootDeviceName, blockDeviceMapping, virtualizationType 'hvm', hypervisor 'xen', platformDetails, usageOperation, deregistrationProtection 'disabled', freeTierEligible — plus a nextToken when more rows remain. The names/ids in the recording are per-run values; the page SHAPE and the presence of nextToken are the requirement.
Emulator: '$.DescribeImagesResponse.imagesSet: type_mismatch' against the recorded page, and the cluster also diverges on $.DescribeImagesResponse.nextToken — the emulator serves no self-owned image page and no continuation token.
Note for the fixer: this fixture's setup registers its AMIs via RegisterImage, which currently answers 501 (that is a separate, already-filed scaffold-stub issue for services/service-com-amazonaws-ec2-amazonec2/src/ops/register_image.rs). This DescribeImages cluster is reported with stub_path empty and owned_by empty, so it is filed here; it cannot be confirmed green until RegisterImage exists, and the self-owned filter plus MaxResults/NextToken paging in the DescribeImages handler must then be correct too.
Fix: services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_images.rs — the Owners=self projection and the MaxResults/NextToken pagination, over image state written by RegisterImage.
Traces: ec2-describe-images-self-owned-pagination-and-params#task#13.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up true, bootstrap_signal false, 2742/3942 steps passed, 1081 mismatched, 602 divergence clusters. Six net-new clusters, each with stub_path empty and owned_by empty:

1) ec2-describe-nat-gateways-pagination#task#18 (affects #task#19) — DescribeNatGateways, 200 vs 200.
   $.DescribeNatGatewaysResponse.natGatewaySet.item[0]: value_mismatch — expected {'attachedApplianceSet': '', 'availabilityMode': 'zonal', 'connectivityType': 'private', 'createTime': <shaped>, 'natGatewayAddressSet': {'item': {'isPrimary': 'true', 'networkInterfaceId': <shaped>, 'privateIp': <shaped>, 'status': 'succeeded'}}, ...}; got the same row WITHOUT attachedApplianceSet and WITH natGatewayAddressSet.item.availabilityZone 'us-west-2a' / availabilityZoneId 'usw2-az1'.
2) ec2-describe-nat-gateways-pagination#task#20 — DescribeNatGateways, 200 vs 200: '...natGatewaySet.item[subnetId="..."].attachedApplianceSet: missing (expected "", got None)' for all 7 rows.
3) ec2-describe-network-interfaces-vpc-filter-exact-set#task#10 — DescribeNetworkInterfaces, 200 vs 200: '...networkInterfaceSet.item[networkInterfaceId="eni-..."].publicDnsName: missing (expected "", got None)' for all 6 rows.
4) ec2-mr-inv-describe-regions-enumerates-scenario-regions#task#1 and #task#2 — DescribeRegions, 200 vs 200: regionInfo.item[*].regionEndpoint flagged host_not_own for 14 and 17 rows respectively (got 'ec2.ap-south-1.amazonaws.com', 'ec2.eu-west-1.amazonaws.com', 'ec2.us-east-1.amazonaws.com', ... — hosts the emulator does not serve).
5) ec2-mr-inv-describe-regions-rejects-unknown-region-name#task#4 — DescribeRegions, 200 vs 200: '$.DescribeRegionsResponse.regionInfo.item[33].regionEndpoint: host_not_own (expected "ec2.ap-southeast-7.amazonaws.com", got "ec2.us-west-2.amazonaws.com")'.
6) ec2-describe-images-self-owned-pagination-and-params#task#13 — DescribeImages, 200 vs 200: '$.DescribeImagesResponse.imagesSet: type_mismatch' against the recorded page (8 self-owned AMIs with imageLocation/blockDeviceMapping/platformDetails/usageOperation/deregistrationProtection/freeTierEligible), divergence_key also covering $.DescribeImagesResponse.nextToken.

Model evidence: smithy_query shape=NatGateway -> AttachedAppliances xmlName attachedApplianceSet; shape=NatGatewayAddress -> availabilityZone / availabilityZoneId; shape=Region -> Endpoint xmlName regionEndpoint 'The Region service endpoint'; operation=DescribeNetworkInterfaces / DescribeNatGateways -> paginated {NextToken, MaxResults}, errors [].

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator from this checkout.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: replay ec2-describe-nat-gateways-pagination step task#18 (and task#19, task#20) — DescribeNatGateways over private NAT gateways. Observe attachedApplianceSet absent from every row and natGatewayAddressSet items carrying availabilityZone/availabilityZoneId.
4. Cause 2: replay ec2-describe-network-interfaces-vpc-filter-exact-set step task#10 — DescribeNetworkInterfaces filtered by vpc-id. Observe publicDnsName absent from every row instead of present-and-empty.
5. Cause 3: replay ec2-mr-inv-describe-regions-enumerates-scenario-regions steps task#1 and task#2, and ec2-mr-inv-describe-regions-rejects-unknown-region-name step task#4 — DescribeRegions. Observe every regionEndpoint pointing at ec2.<region>.amazonaws.com (a host the emulator does not serve) and the asserted row reporting ec2.us-west-2.amazonaws.com instead of its own region's endpoint.
6. Cause 4: replay ec2-describe-images-self-owned-pagination-and-params through task#13 — DescribeImages with Owners=self and MaxResults. Observe an imagesSet of the wrong shape and no nextToken. (Its setup's RegisterImage calls answer 501 — already filed as a scaffold stub.)
