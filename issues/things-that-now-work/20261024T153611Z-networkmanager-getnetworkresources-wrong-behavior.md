Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# GetNetworkResources never reports AwsRegion, so the awsRegion filter can never select an attachment

_Filed: 2026-10-24T15:36:11Z_
_Operation: GetNetworkResources | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every entry GetNetworkResources returns is built with `aws_region: None`, even for the kinds whose Region this service already stores. An attachment is created at exactly one edge location (`AttachmentRecord::edge_location`, set from the VPC/VPN ARN's Region by CreateVpcAttachment / CreateSiteToSiteVpnAttachment and from the transport's edge by CreateConnectAttachment), so its Region is known to the store, yet the NetworkResource entry for it omits `AwsRegion`.

Expected: GET /global-networks/{GlobalNetworkId}/network-resources lists each attachment with `AwsRegion` equal to the edge location it sits at, and the documented `awsRegion` query filter selects exactly the resources in that Region.

Actual: `AwsRegion` is absent on every entry of every kind, and because the handler compares the filter against the member the answer itself reports (`selects(input.aws_region, resource.aws_region)`), passing `awsRegion=<any value>` matches nothing at all — the call always answers HTTP 200 with an empty `NetworkResources` list, even when the global network holds attachments at that exact edge. A client using the region filter to walk a global network's inventory sees it as empty.

## Evidence

Smithy model: `NetworkResource$AwsRegion` ("The Amazon Web Services Region") is a member of the shape this operation returns, and `GetNetworkResourcesRequest$AwsRegion` is bound as `@httpQuery("awsRegion")` — so both the reported value and the filter are modeled.

Handler: services/.../src/ops/get_network_resources.rs lines 220-248 apply the filter with `selects(input.aws_region.as_deref(), resource.aws_region.as_deref())`.

State: src/state.rs `fn network_resource(..)` (around lines 2135-2157) hardcodes `aws_region: None` for every kind, and its own doc comment (lines 2118-2128) concedes the gap: "A kind that DOES live in one Region must report it here when it starts being stored — a resource whose Region is known but unreported would be silently under-selected by that filter (an attachment, for one, knows the edge location it sits at)." Attachments ARE stored now: CreateVpcAttachment, CreateConnectAttachment and CreateSiteToSiteVpnAttachment are implemented and each record carries `edge_location`, and `network_resources_of` (lines 1884-1912) lists them.

Test coverage: tests/all/network_resources.rs line 436 asserts the CURRENT behavior (`for absent in ["AwsRegion", "RegisteredGatewayArn", "Metadata"] { assert!(entry[absent].is_null()) }`), so the divergence is locked in by a test that will have to change with the fix. No filed issue (open or fixed) covers this operation.

## Steps to Reproduce

1. POST /global-networks to create a global network
2. POST /core-networks with a PolicyDocument whose core-network-configuration declares an edge location in us-east-1 (and wait for the core network to settle to AVAILABLE)
3. POST /core-networks/{CoreNetworkId}/vpc-attachments with VpcArn=arn:aws:ec2:us-east-1:000000000000:vpc/vpc-00000000000000001 and one subnet ARN in us-east-1
4. GET /global-networks/{GlobalNetworkId}/network-resources and observe the attachment entry has no AwsRegion member, although the attachment sits at edge location us-east-1
5. GET /global-networks/{GlobalNetworkId}/network-resources?awsRegion=us-east-1 and observe HTTP 200 with an empty NetworkResources list instead of the attachment that lives in that Region
