Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-gateway-vpc-endpoint-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-gateway-vpc-endpoint-lifecycle ec2-lsrno-traj-cross-region-s3-endpoint-refused ec2-lsrno-traj-isolated-subnet-routes-only-same-region-s3 ec2-nfwr-traj-find-tables-routing-to-an-endpoint ec2-nfwr-traj-route-target-to-firewall-endpoint-mistakes
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateVpcEndpoint|2xx->5xx|$.CreateVpcEndpointResponse
Issue-Key: ec2|CreateVpcEndpoint|2xx->5xx|status
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# EC2 CreateVpcEndpoint answers 501 NotImplemented for a gateway endpoint

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcEndpoint

## Description
One net-new cause, observed in two independent clusters.

Cause 1 of 1: creating a Gateway VPC endpoint answers HTTP 501 NotImplemented instead of creating it.
AWS: Smithy com.amazonaws.ec2#CreateVpcEndpoint accepts VpcId (required), ServiceName, VpcEndpointType, RouteTableIds (xmlName RouteTableId), PolicyDocument and TagSpecifications, and returns CreateVpcEndpointResult with a VpcEndpoint member; the operation models no errors. The recording shows HTTP 200 and a vpcEndpoint whose state is 'available', vpcEndpointType 'Gateway', serviceName the regional S3 service name, routeTableIdSet carrying the requested route table, dnsOptions.dnsRecordIpType 'service-defined', ipAddressType 'ipv4', privateDnsEnabled 'false', requesterManaged 'false' and the default full-access policyDocument.
Emulator: HTTP 501 with no CreateVpcEndpointResponse body; the operation is dispatched but this request shape is not served. CreateVpcEndpoint is NOT reported as an unimplemented scaffold for these steps (no stub path), so the 501 comes from inside the handler.
Why: every gateway-endpoint trajectory stops at creation — the endpoint never exists, so the route-table route to the service is never added (see the companion route-table issue filed in this sweep) and no client can reach S3 through a gateway endpoint. A 501 is also the wrong class for a request AWS serves: SDK retry/erroring logic treats it as an emulator gap rather than a client error.
Fix: the CreateVpcEndpoint handler — the Gateway endpoint path (with RouteTableIds) must build and persist the endpoint and project it, instead of falling through to NotImplemented.
Traces: ec2-gateway-vpc-endpoint-lifecycle#task#4 (cluster also covers ec2-lsrno-traj-cross-region-s3-endpoint-refused#task#6 and ec2-lsrno-traj-isolated-subnet-routes-only-same-region-s3#task#6); ec2-nfwr-traj-find-tables-routing-to-an-endpoint#setup#4 (cluster also covers ec2-nfwr-traj-route-target-to-firewall-endpoint-mistakes#setup#3). Related open issues 20261104T044826Z-ec2-lambda-s3-createvpcendpoint-route-table-associations and 20261104T044826Z-ec2-lambda-s3-sqs-createvpcendpoint-refuses-route-table-associations describe a similar refusal but do not carry these replay keys.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up).
ec2-gateway-vpc-endpoint-lifecycle#task#4, CreateVpcEndpoint, expected 200 / actual 501: "status: expected 200, got 501"; $.CreateVpcEndpointResponse missing (expected vpcEndpoint with state 'available', vpcEndpointType 'Gateway', serviceName 'com.amazonaws.us-west-2.s3', routeTableIdSet {'item': 'rtb-...'}, dnsOptions.dnsRecordIpType 'service-defined', ipAddressType 'ipv4', privateDnsEnabled 'false', requesterManaged 'false', the default Allow-* policyDocument and the request's Name tag). 3 steps affected.
ec2-nfwr-traj-find-tables-routing-to-an-endpoint#setup#4, CreateVpcEndpoint, expected 200 / actual 501: "status: expected 200, got 501". 2 steps affected.
Both clusters report an empty stub_path, i.e. the operation is not a scaffold stub.
Model consulted: smithy_query operation=CreateVpcEndpoint on services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire fixture corpus (trace set simparity-wire-aws-v1).
3. Inspect ec2-gateway-vpc-endpoint-lifecycle task#4 and ec2-nfwr-traj-find-tables-routing-to-an-endpoint setup#4: both issue CreateVpcEndpoint for a Gateway endpoint with RouteTableIds and receive HTTP 501 where the recording has HTTP 200.
