Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__network-firewall.nfwr-ready-firewall-endpoints-unrouted-until-named
Issue-Key: ec2__network-firewall|DescribeVpcEndpoints|InvalidVpcEndpointId.NotFound
Issue-Commit: 88a6b3d80dac2fa3529d7971147a50fe7f0d0905

# A READY firewall's SyncStates endpoints are not registered as EC2 VPC endpoints, so DescribeVpcEndpoints cannot find them

> _Note: this issue was identified against the code base at commit `88a6b3d80dac2fa3529d7971147a50fe7f0d0905`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFirewall

## Description
ONE root cause, in a cross-service check. Attribution note: the failing check id is `ec2__network-firewall.nfwr-ready-firewall-endpoints-unrouted-until-named`, whose leading token is the COMPOSITE `ec2__network-firewall` (the check declares SERVICES = ["ec2", "network-firewall"]). The per-service bootstrap gate cannot place that composite token by exact equality; both constituent services are bootstrapped (service-com-amazonaws-ec2-amazonec2 and service-com-amazonaws-networkfirewall-networkfirewall-20201112 are both in the bootstrapped set, neither is pending), and both answer this check's calls, so it is filed rather than deferred. It is filed under `network-firewall` because that is the side that must instantiate the endpoint; the wrong answer surfaces on the EC2 side.

Cause 1 of 1: Network Firewall mints a firewall-endpoint id into FirewallStatus.SyncStates[*].Attachment.EndpointId without creating a corresponding EC2 VPC endpoint, so EC2 does not know that id.
AWS: Smithy (com.amazonaws.networkfirewall#NetworkFirewall_20201112) models SyncState$Attachment -> Attachment, whose EndpointId member is documented "The identifier of the firewall endpoint that Network Firewall has instantiated in the subnet. You use this to identify the firewall endpoint in the VPC route tables, when you redirect traffic..." and whose SubnetId is "The unique identifier of the subnet that you've specified to be used for a firewall endpoint"; Attachment$Status is READY "the endpoint is available to handle network traffic". The endpoint is therefore a REAL, cross-service-visible object: for a READY firewall with one subnet mapping per Availability Zone, EC2 DescribeVpcEndpoints called with those endpoint ids must return one endpoint per zone, each with VpcEndpointType 'GatewayLoadBalancer', VpcId equal to the firewall's VPC and State 'available'. That it is addressable by EC2 is what makes the documented route-table redirection (CreateRoute with VpcEndpointId, and the DescribeRouteTables filter route.gateway-id=<endpoint>) possible at all. For the endpoint-id FORMAT and the VpcEndpointType/State values this is recorded behavior only: the recording shows distinct 'vpce-'-prefixed ids, type GatewayLoadBalancer, state available.
Emulator: CreateFirewall/DescribeFirewall do report a READY firewall whose SyncStates are keyed by the mapped zones and whose Attachments carry distinct 'vpce-' ids in the right subnets — but the ids exist only inside the Network Firewall service's own state. EC2 DescribeVpcEndpoints with those ids answers HTTP 400 InvalidVpcEndpointId.NotFound ("The VPC endpoint ID 'vpce-...' does not exist") instead of returning the endpoints, so the check cannot get past the cross-service read.
Why: the endpoint id is the handle a customer uses to route traffic through the firewall. A client that reads SyncStates and then does anything with the id — describing the endpoint, routing 0.0.0.0/0 to it with CreateRoute --vpc-endpoint-id, or finding the tables that reference it with the route.gateway-id filter — fails with a not-found error against an id the emulator itself just handed out. The firewall therefore reports itself READY while nothing can actually be pointed at it, and the emulator is internally inconsistent: one service vouches for an identifier another denies exists. Everything downstream of the DescribeVpcEndpoints read in this check is unverified as a result and must be re-replayed, not assumed, after the fix.
Fix: the Network Firewall firewall-endpoint instantiation path — whatever advances a firewall to READY and fills in SyncStates[*].Attachment must, for each mapped subnet, register the endpoint in the shared EC2 VPC-endpoint state as a GatewayLoadBalancer endpoint in the firewall's VPC with State 'available' (and remove it again on DeleteFirewall / DisassociateSubnets), rather than generating an id in isolation. The EC2 side's DescribeVpcEndpoints lookup is behaving correctly for state it has no record of.
Traces: ec2__network-firewall.nfwr-ready-firewall-endpoints-unrouted-until-named.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the built emulator. Per-service: ec2__network-firewall 2/4 (50%), 2 failed.

ec2__network-firewall.nfwr-ready-firewall-endpoints-unrouted-until-named [error]:
  The check creates a two-AZ VPC (10.2.0.0/16, one /24 per zone), a firewall policy, and a firewall with one subnet mapping in each zone, then polls it to READY. Up to that point the emulator agrees with AWS: FirewallStatus.SyncStates is keyed by the two mapped zones, each Attachment.SubnetId is in its own zone, and the two Attachment.EndpointId values are distinct and 'vpce-' prefixed. The next call is the cross-service read:
    ec2.describe_vpc_endpoints(VpcEndpointIds=<the two SyncStates endpoint ids>)
  Expected: the two endpoints, each (VpcEndpointId, VpcEndpointType, VpcId, State) == (<id>, 'GatewayLoadBalancer', <the firewall's VPC>, 'available').
  Actual: botocore.exceptions.ClientError: An error occurred (InvalidVpcEndpointId.NotFound) when calling the DescribeVpcEndpoints operation: The VPC endpoint ID 'vpce-840a2776bbc5ed496' does not exist

The specific endpoint id, subnet ids, VPC id, zone names and the 10.2.0.0/16 addressing are environment-specific and are NOT part of any requirement; the requirement is that every id Network Firewall publishes in SyncStates[*].Attachment.EndpointId for a READY firewall resolves in EC2 to a GatewayLoadBalancer endpoint in that firewall's VPC with State 'available'.

Not verified by this run (blocked behind the failure above, and to be re-replayed after the fix rather than closed unrun): that DescribeRouteTables with Filters=[{Name: 'route.gateway-id', Values: [<endpoint>]}] returns no table before any route names the endpoint; that CreateRoute with DestinationCidrBlock 0.0.0.0/0 and VpcEndpointId=<endpoint> returns Return=true; that the route reads back with the endpoint as its GatewayId; and that the route.gateway-id filter then returns exactly the one table that names it, and still nothing for the other endpoint.

Smithy consulted (model_path services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/smithy-model/com-amazonaws-networkfirewall-networkfirewall-20201112.json): DescribeFirewall (output Firewall + FirewallStatus), SyncState (member Attachment -> Attachment), Attachment (SubnetId -> AzSubnet pattern ^subnet-[0-9a-f]+$, EndpointId -> EndpointId documented as the identifier used to name the firewall endpoint in VPC route tables, Status -> AttachmentStatus with READY meaning the endpoint is available to handle network traffic).

## Steps to Reproduce
1. Create a VPC (e.g. 10.2.0.0/16) with one subnet in each of two Availability Zones.
2. aws network-firewall create-firewall-policy --firewall-policy-name p1 --firewall-policy '{"StatelessDefaultActions":["aws:forward_to_sfe"],"StatelessFragmentDefaultActions":["aws:forward_to_sfe"]}'
3. aws network-firewall create-firewall --firewall-name fw-out --firewall-policy-arn <policy-arn> --vpc-id <vpc> --subnet-mappings SubnetId=<subnet-az-a> SubnetId=<subnet-az-b>
4. Poll aws network-firewall describe-firewall --firewall-arn <arn> until FirewallStatus.Status is READY, then collect FirewallStatus.SyncStates[*].Attachment.EndpointId (one per zone).
5. aws ec2 describe-vpc-endpoints --vpc-endpoint-ids <the two endpoint ids>
   Emulator: 400 InvalidVpcEndpointId.NotFound ("The VPC endpoint ID 'vpce-...' does not exist").
   AWS: both endpoints returned, each with VpcEndpointType 'GatewayLoadBalancer', VpcId = <vpc>, State 'available'.
6. Re-replay after the fix (currently unreachable): aws ec2 describe-route-tables --filters Name=route.gateway-id,Values=<endpoint> must return no table; aws ec2 create-route --route-table-id <rt-in-vpc> --destination-cidr-block 0.0.0.0/0 --vpc-endpoint-id <endpoint> must return Return=true; the route must read back with GatewayId=<endpoint>; and the same route.gateway-id filter must then return exactly that route table, and still nothing for the second endpoint.
