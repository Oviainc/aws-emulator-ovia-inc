Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-gateway-vpc-endpoint-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-gateway-vpc-endpoint-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeVpcEndpointServices|2xx->2xx|$.DescribeVpcEndpointServicesResponse.serviceDetailSet.item.supportedIpAddressTypeSet.item[*]
Issue-Related: 20261030T063325Z-ec2-static-catalogs-diverge-from-aws-instance-type.md
Issue-Commit: 30059ba7efbea1c9406793a7f2ccf8806d86986b

# EC2 DescribeVpcEndpointServices merges Gateway and Interface service details

> _Note: this issue was identified against the code base at commit `30059ba7efbea1c9406793a7f2ccf8806d86986b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVpcEndpointServices

## Description
Cause 1 of 1: DescribeVpcEndpointServices returns one serviceDetail row that conflates the Gateway and Interface endpoint services for S3, with a mis-shaped service id and an incomplete supported-IP-address-type list (fixture ec2-gateway-vpc-endpoint-lifecycle, step task#3).
AWS: Smithy: DescribeVpcEndpointServices -> serviceDetailSet of ServiceDetail, whose ServiceType member (xmlName serviceType) is a list of ServiceTypeDetail, BaseEndpointDnsNames (xmlName baseEndpointDnsNameSet) a list of DNS names, SupportedIpAddressTypes (xmlName supportedIpAddressTypeSet, @length 0..2) the supported IP address types, and ServiceId the endpoint-service id. AWS publishes one serviceDetail entry per endpoint service, so the Gateway service and the Interface service are separate rows: the recorded response for the queried service has exactly one serviceType entry ({'serviceType': 'Gateway'}), a single base endpoint DNS name, and two supportedIpAddressTypeSet entries (the second being 'ipv6'). AWS endpoint-service ids have the vpce-svc- prefix followed by a 17-hex-digit suffix.
Emulator: answers 200 with a single row whose serviceType.item is a two-element list [{'serviceType': 'Gateway'}, {'serviceType': 'Interface'}], whose baseEndpointDnsNameSet.item is a two-element list ['s3.us-west-2.vpce.amazonaws.com', 's3.us-west-2.amazonaws.com'] where one name is recorded, whose supportedIpAddressTypeSet.item has only one entry (the 'ipv6' entry is missing), and whose serviceId does not match the recorded id shape (it emits a short 8-hex-digit suffix).
Why: a client picks an endpoint type from this list before calling CreateVpcEndpoint - a row that claims to be both Gateway and Interface makes the choice ambiguous, the extra/merged DNS names are wrong for whichever type is chosen, the missing 'ipv6' entry makes dual-stack endpoints look unsupported, and a service id of the wrong shape breaks clients that parse or round-trip it.
Fix: the DescribeVpcEndpointServices handler's endpoint-service catalog in the ec2 service crate - one ServiceDetail per (service name, endpoint type) with that type's own baseEndpointDnsNames and supportedIpAddressTypes - plus the endpoint-service id generator.
Traces: ec2-gateway-vpc-endpoint-lifecycle#task#3.

No open issue carries this replay key. The related open issue 20261027T074309Z-logs-syslog-configurations-are-ungradeable-ec2-describevpcendpointservices-omits.md reports a missing PrivateLink service in the same catalog but is bound to a different step and does not cover the Gateway/Interface merge.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 57.3%): cluster ec2-gateway-vpc-endpoint-lifecycle#task#3, DescribeVpcEndpointServices, expected 200 / actual 200, unowned and not stub-blocked.
$.DescribeVpcEndpointServicesResponse.serviceDetailSet.item.serviceType.item: type_mismatch - expected {'serviceType': 'Gateway'}, got [{'serviceType': 'Gateway'}, {'serviceType': 'Interface'}].
$.DescribeVpcEndpointServicesResponse.serviceDetailSet.item.baseEndpointDnsNameSet.item: value_mismatch - expected a single recorded name, got ['s3.us-west-2.vpce.amazonaws.com', 's3.us-west-2.amazonaws.com'].
$.DescribeVpcEndpointServicesResponse.serviceDetailSet.item.supportedIpAddressTypeSet.item[1]: missing - expected 'ipv6', got None.
$.DescribeVpcEndpointServicesResponse.serviceDetailSet.item.serviceId: value_mismatch against the recorded id shape - got 'vpce-svc-fffbb61c'.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture ec2-gateway-vpc-endpoint-lifecycle up to step task#3 (DescribeVpcEndpointServices for the S3 service in us-west-2).
3. Inspect serviceDetailSet: the single row reports both Gateway and Interface in serviceType, two merged baseEndpointDnsNameSet entries, only one supportedIpAddressTypeSet entry, and a vpce-svc- id with a short suffix, where AWS returns a Gateway-only row with one DNS name and two supported IP address types.
