Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-ssm-interface-endpoint-private-dns-converges-to-available-and-deletes-to-not-found
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-ssm-interface-endpoint-private-dns-converges-to-available-and-deletes-to-not-found ec2-lsrno-traj-cross-region-s3-endpoint-refused ec2-vpce-traj-iot-data-endpoint-create-refusals
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateVpcEndpoint|2xx->2xx|$.CreateVpcEndpointResponse.vpcEndpoint.dnsEntrySet.item[*]
Issue-Key: ec2|CreateVpcEndpoint|2xx->2xx|$.CreateVpcEndpointResponse.vpcEndpoint.dnsEntrySet.item[*].dnsName
Issue-Key: ec2|CreateVpcEndpoint|2xx->2xx|$.CreateVpcEndpointResponse.vpcEndpoint.dnsEntrySet.item[*].hostedZoneId
Issue-Key: ec2|CreateVpcEndpoint|2xx->2xx|$.CreateVpcEndpointResponse.vpcEndpoint.routeTableIdSet
Issue-Key: ec2|DescribeVpcEndpoints|2xx->2xx|$.DescribeVpcEndpointsResponse.vpcEndpointSet.item.dnsEntrySet.item[*]
Issue-Key: ec2|DescribeVpcEndpoints|2xx->2xx|$.DescribeVpcEndpointsResponse.vpcEndpointSet.item.dnsEntrySet.item[*].dnsName
Issue-Key: ec2|DescribeVpcEndpoints|2xx->2xx|$.DescribeVpcEndpointsResponse.vpcEndpointSet.item.policyDocument
Issue-Key: ec2|DescribeVpcEndpoints|2xx->2xx|$.DescribeVpcEndpointsResponse.vpcEndpointSet.item.routeTableIdSet
Issue-Key: ec2|DescribeVpcEndpoints|2xx->2xx|$.DescribeVpcEndpointsResponse.vpcEndpointSet
Issue-Commit: 10d8f420d9792c6a92213c81068536eef787b058

# EC2 VPC endpoint DNS entries, policy document and routeTableIdSet diverge

> _Note: this issue was identified against the code base at commit `10d8f420d9792c6a92213c81068536eef787b058`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcEndpoint

## Description
Three net-new causes on the VPC-endpoint read/write surface.

Cause 1 of 3 — CreateVpcEndpoint renders the wrong dnsEntrySet and omits routeTableIdSet.
AWS: for a private-DNS interface endpoint the create response's dnsEntrySet is ordered endpoint-specific name first, then the per-AZ endpoint name, then the service's regional public name, then the service's .api.aws name; the endpoint-specific names carry the <vpce-id>-<endpoint-suffix> form (an 8-character suffix AWS generates per endpoint) and the names AWS has not yet published carry hostedZoneId 'ZONEIDPENDING' at create time. An interface endpoint also renders routeTableIdSet as an empty element.
Emulator: the order is inverted (the regional public name is emitted first), the endpoint-specific names omit the generated suffix entirely, the fourth (.api.aws) entry is absent, hostedZoneId is a synthesized zone id instead of 'ZONEIDPENDING', and routeTableIdSet is absent.
Why: a client that takes dnsEntrySet[0] as the endpoint-specific name to put in its own resolver or TLS SNI gets the service's public name instead, which defeats the endpoint; and the .api.aws name is never offered.
Fix: the CreateVpcEndpoint response serializer and the DNS-entry generation in the endpoint's state.
Traces: ec2-ssm-interface-endpoint-private-dns-converges-to-available-and-deletes-to-not-found#task#5.

Cause 2 of 3 — DescribeVpcEndpoints repeats the dnsEntrySet divergence and re-serializes the policy document.
AWS: the read renders the same four ordered dnsEntrySet names (with the generated suffix, each per-AZ name carrying it too) and returns policyDocument as the stored document — the default endpoint policy is pretty-printed JSON with 2-space indentation and keys in the order Action, Effect, Principal, Resource inside a Statement array.
Emulator: the first/second/third names are rotated and suffix-less, the .api.aws entry is missing, routeTableIdSet is absent, and policyDocument is emitted as minified JSON.
Why: byte-comparing or re-submitting the policy a client just read produces a different document than AWS returns, and the DNS names are unusable as above.
Fix: the DescribeVpcEndpoints projection; store and return the policy document verbatim rather than re-serializing it.
Traces: ec2-ssm-interface-endpoint-private-dns-converges-to-available-and-deletes-to-not-found#task#6.

Cause 3 of 3 — DescribeVpcEndpoints returns an empty set for an existing gateway endpoint.
AWS: the recorded read returns the Gateway endpoint item (vpcEndpointType 'Gateway', serviceName com.amazonaws.<region>.s3, state 'available', its routeTableIdSet, the default policyDocument, dnsOptions.dnsRecordIpType 'service-defined', ipAddressType 'ipv4', privateDnsEnabled 'false', requesterManaged 'false').
Emulator: vpcEndpointSet is empty, so the endpoint the fixture created is invisible.
Why: a client cannot confirm or later delete the gateway endpoint it created, and route-table association checks fail.
Fix: the DescribeVpcEndpoints lookup/filtering for Gateway-type endpoints in state.
Traces: ec2-lsrno-traj-cross-region-s3-endpoint-refused#task#9 (cluster also covers ec2-vpce-traj-iot-data-endpoint-create-refusals#task#8).

## Evidence
Cause 1: $.CreateVpcEndpointResponse.vpcEndpoint.dnsEntrySet.item[0].dnsName expected 'vpce-<id>-<suffix>.ssm.<region>.vpce.amazonaws.com', got 'ssm.<region>.amazonaws.com'; item[2].dnsName expected 'ssm.<region>.amazonaws.com', got 'vpce-<id>-<az>.ssm.<region>.vpce.amazonaws.com'; item[2].hostedZoneId expected 'ZONEIDPENDING', got a synthesized zone id; item[3] missing (expected {'dnsName': 'ssm.<region>.api.aws', 'hostedZoneId': 'ZONEIDPENDING'}); .routeTableIdSet missing (expected '').
Cause 2: the same three dnsName rotations and the missing item[3] on DescribeVpcEndpoints, .routeTableIdSet missing, and .policyDocument value_mismatch — expected the 2-space pretty-printed document '{\n  "Statement": [\n    {\n      "Action": "*", ... }', got '{"Statement":[{"Action":"*","Effect":"Allow","Principal":"*","Resource":"*"}]}'.
Cause 3: $.DescribeVpcEndpointsResponse.vpcEndpointSet type_mismatch (expected the full Gateway endpoint item, got '').

## Steps to Reproduce
Replay the wire corpus (simparity-wire-aws-v1) and inspect ec2-ssm-interface-endpoint-private-dns-converges-to-available-and-deletes-to-not-found#task#5 and #task#6 (interface endpoint create then describe) and ec2-lsrno-traj-cross-region-s3-endpoint-refused#task#9 (gateway endpoint describe).
