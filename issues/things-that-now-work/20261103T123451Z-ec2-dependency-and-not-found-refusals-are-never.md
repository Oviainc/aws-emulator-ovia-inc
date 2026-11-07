Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-ipam-trajwire-child-pool-create-mistakes
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-ipam-trajwire-child-pool-create-mistakes ec2-ipam-trajwire-teardown-order-mistakes ec2-pst-traj-instance-joined-to-public-subnet-by-subnet-filter ec2-sshp-traj-subnet-nacl-is-always-explicitly-associated ec2-trajwire-deb-deb-security-group-from-another-vpc ec2-sshp-traj-prefix-list-source-is-opaque-to-cidr-filters ec2-sshp-traj-referenced-group-source-has-no-cidr ec2-ssm-interface-endpoint-private-dns-converges-to-available-and-deletes-to-not-found ec2-tgwc-traj-empty-tgw-route-table-search
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateIpamPool|4xx->2xx|$.Response
Issue-Key: ec2|CreateIpamPool|4xx->2xx|status
Issue-Key: ec2|DeleteIpamPool|4xx->2xx|$.Response
Issue-Key: ec2|DeleteIpamPool|4xx->2xx|status
Issue-Key: ec2|DescribeIpams|4xx->2xx|$
Issue-Key: ec2|DescribeIpams|4xx->2xx|$.Response
Issue-Key: ec2|DescribeIpams|4xx->2xx|status
Issue-Key: ec2|DeleteSubnet|4xx->2xx|$.Response
Issue-Key: ec2|DeleteSubnet|4xx->2xx|status
Issue-Key: ec2|DeleteNetworkAcl|4xx->2xx|$.Response
Issue-Key: ec2|DeleteNetworkAcl|4xx->2xx|status
Issue-Key: ec2|RunInstances|4xx->2xx|$.Response
Issue-Key: ec2|RunInstances|4xx->2xx|status
Issue-Key: ec2|AuthorizeSecurityGroupIngress|4xx->2xx|$.Response
Issue-Key: ec2|AuthorizeSecurityGroupIngress|4xx->2xx|status
Issue-Key: ec2|DescribeVpcEndpoints|4xx->2xx|$
Issue-Key: ec2|DescribeVpcEndpoints|4xx->2xx|$.Response
Issue-Key: ec2|DescribeVpcEndpoints|4xx->2xx|status
Issue-Key: ec2|DescribeTransitGateways|2xx->2xx|$.DescribeTransitGatewaysResponse.transitGatewaySet
Issue-Commit: 10d8f420d9792c6a92213c81068536eef787b058

# EC2 dependency and not-found refusals are never raised on delete and launch paths

> _Note: this issue was identified against the code base at commit `10d8f420d9792c6a92213c81068536eef787b058`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteIpamPool

## Description
Nine net-new causes sharing one fault: the emulator has no pre-condition gate on these paths, so a request AWS refuses with a 400 ec2-query Error is accepted with 200, and a resource AWS has removed stays readable. In every cause the expected body is an <Errors><Error><Code>/<Message> pair and the emulator returned the success projection instead.

Cause 1 of 9 — CreateIpamPool in a locale outside the IPAM's operating Regions is accepted.
AWS: refuses 400 with Code 'DependencyViolation' and Message 'The specified action is not valid. Ipam <ipam-id> does not contain the operating Region <region>.' The two sentences are fixed text; <ipam-id> is the IPAM from the request and <region> is the requested locale.
Emulator: answers 200 and creates the pool.
Why: a caller gets a pool that could never allocate in that Region and discovers the misconfiguration only at allocation time.
Fix: CreateIpamPool validation (locale vs the IPAM's operatingRegionSet).
Traces: ec2-ipam-trajwire-child-pool-create-mistakes#task#5.

Cause 2 of 9 — DeleteIpamPool succeeds while the pool still has provisioned CIDRs.
AWS: refuses 400 with Code 'DependencyViolation' and Message 'The specified action is not valid for <ipam-pool-id>. There are one or more pool cidrs provisioned in the pool.' Fixed text apart from the pool id taken from the request.
Emulator: answers 200 and deletes the pool with its provisioned CIDRs.
Why: teardown ordering bugs in client code are masked, and the CIDRs leak out of IPAM accounting.
Fix: DeleteIpamPool validation / the pool-CIDR dependency check in state.
Traces: ec2-ipam-trajwire-teardown-order-mistakes#task#13 (cluster also covers task#14).

Cause 3 of 9 — a deleted IPAM stays readable: DescribeIpams keeps answering 200.
AWS: once the IPAM is deleted, DescribeIpams for that id refuses 400 with Code 'InvalidIpamId.NotFound' and Message 'The IPAM (<ipam-id>) does not exist.' Fixed text with the id from the request in parentheses.
Emulator: keeps returning the full ipamSet item (state 'create-complete') for the whole 60s poll window.
Why: a client waiting for deletion to converge never sees it and hangs or times out.
Fix: DeleteIpam's state transition plus the DescribeIpams lookup.
Traces: ec2-ipam-trajwire-teardown-order-mistakes#teardown#20.

Cause 4 of 9 — DeleteSubnet succeeds while an instance/ENI still occupies the subnet.
AWS: refuses 400 with Code 'DependencyViolation' and Message "The subnet '<subnet-id>' has dependencies and cannot be deleted." Fixed text with the subnet id from the request in single quotes.
Emulator: answers 200 and deletes the subnet under the running instance.
Why: a client can orphan live ENIs and the subsequent reads describe a VPC whose topology cannot exist in AWS.
Fix: DeleteSubnet validation / the ENI-to-subnet dependency check in state.
Traces: ec2-pst-traj-instance-joined-to-public-subnet-by-subnet-filter#task#15.

Cause 5 of 9 — DeleteNetworkAcl succeeds while a subnet association remains.
AWS: refuses 400 with Code 'DependencyViolation' and Message "The networkAcl '<acl-id>' has dependencies and cannot be deleted." Fixed text with the ACL id from the request in single quotes (note the lower-cased resource noun 'networkAcl').
Emulator: answers 200 and deletes the associated ACL.
Why: the subnet is left pointing at an ACL that no longer exists.
Fix: DeleteNetworkAcl validation / the ACL-association dependency check in state.
Traces: ec2-sshp-traj-subnet-nacl-is-always-explicitly-associated#task#11.

Cause 6 of 9 — RunInstances accepts a security group from a different VPC than the subnet.
AWS: refuses 400 with Code 'InvalidParameter' and Message 'Security group <sg-id> and subnet <subnet-id> belong to different networks.' Fixed text with the group and subnet ids from the request.
Emulator: answers 200 and launches the instance.
Why: the client believes a cross-VPC security group binding is legal and builds on an instance AWS would never have created.
Fix: RunInstances validation (security-group VPC vs subnet VPC).
Traces: ec2-trajwire-deb-deb-security-group-from-another-vpc#task#7.

Cause 7 of 9 — AuthorizeSecurityGroupIngress accepts a prefix-list id that does not exist.
AWS: refuses 400 with Code 'InvalidPrefixListID.NotFound' and Message "The prefix list ID '<pl-id>' does not exist" (fixed text, no trailing period; the id comes from the request).
Emulator: answers 200 and records the rule, which then shows up in later DescribeSecurityGroups reads.
Why: an authored rule referencing a nonexistent prefix list is accepted, so the client's security posture differs from what AWS would allow.
Fix: AuthorizeSecurityGroupIngress validation of the PrefixListIds source.
Traces: ec2-sshp-traj-prefix-list-source-is-opaque-to-cidr-filters#task#5 (cluster also covers ec2-sshp-traj-referenced-group-source-has-no-cidr#task#6).

Cause 8 of 9 — a deleted VPC endpoint stays readable: DescribeVpcEndpoints keeps answering 200.
AWS: after DeleteVpcEndpoints the id is gone and DescribeVpcEndpoints refuses 400 with Code 'InvalidVpcEndpointId.NotFound' and Message "The Vpc Endpoint Id '<vpce-id>' does not exist" (fixed text with the id from the request).
Emulator: keeps returning the endpoint in state 'available' for the whole poll window.
Why: a client polling for deletion never converges; worse, it keeps routing to an endpoint AWS has removed.
Fix: DeleteVpcEndpoints' state transition plus the DescribeVpcEndpoints lookup.
Traces: ec2-ssm-interface-endpoint-private-dns-converges-to-available-and-deletes-to-not-found#task#8.

Cause 9 of 9 — a deleted transit gateway stays in DescribeTransitGateways.
AWS: after the TGW is deleted the filtered DescribeTransitGateways returns an EMPTY transitGatewaySet.
Emulator: still returns the full item with state 'available'.
Why: teardown verification passes against a resource that should be gone, and a client that re-attaches to it builds on a deleted gateway.
Fix: DeleteTransitGateway's state transition plus the DescribeTransitGateways lookup/filtering.
Traces: ec2-tgwc-traj-empty-tgw-route-table-search#teardown#13.

## Evidence
Replay verdicts (expected -> actual):
Cause 1: status 400 -> 200; expected $.Response.Errors.Error = {Code: 'DependencyViolation', Message: 'The specified action is not valid. Ipam ipam-... does not contain the operating Region us-east-1.'}, got no Response element.
Cause 2: status 400 -> 200; expected Code 'DependencyViolation', Message 'The specified action is not valid for ipam-pool-.... There are one or more pool cidrs provisioned in the pool.'
Cause 3: status 400 -> 200; expected Code 'InvalidIpamId.NotFound', Message 'The IPAM (ipam-...) does not exist.'; poll: error code None not in ['InvalidIpamId.NotFound'] after 13 attempts, poll capped at 60s, DescribeIpamsResponse.ipamSet.item still state 'create-complete'.
Cause 4: status 400 -> 200; expected Code 'DependencyViolation', Message "The subnet 'subnet-...' has dependencies and cannot be deleted."
Cause 5: status 400 -> 200; expected Code 'DependencyViolation', Message "The networkAcl 'acl-...' has dependencies and cannot be deleted."
Cause 6: status 400 -> 200; expected Code 'InvalidParameter', Message 'Security group sg-... and subnet subnet-... belong to different networks.'
Cause 7: status 400 -> 200; expected Code 'InvalidPrefixListID.NotFound', Message "The prefix list ID 'pl-...' does not exist".
Cause 8: status 400 -> 200; expected Code 'InvalidVpcEndpointId.NotFound', Message "The Vpc Endpoint Id 'vpce-...' does not exist"; poll: error code None not in ['InvalidVpcEndpointId.NotFound'] after 5 attempts.
Cause 9: 200 -> 200 but $.DescribeTransitGatewaysResponse.transitGatewaySet type_mismatch (expected '', got the full item with state 'available').

## Steps to Reproduce
Replay the wire corpus (simparity-wire-aws-v1) and inspect ec2-ipam-trajwire-child-pool-create-mistakes#task#5, ec2-ipam-trajwire-teardown-order-mistakes#task#13 and #teardown#20, ec2-pst-traj-instance-joined-to-public-subnet-by-subnet-filter#task#15, ec2-sshp-traj-subnet-nacl-is-always-explicitly-associated#task#11, ec2-trajwire-deb-deb-security-group-from-another-vpc#task#7, ec2-sshp-traj-prefix-list-source-is-opaque-to-cidr-filters#task#5, ec2-ssm-interface-endpoint-private-dns-converges-to-available-and-deletes-to-not-found#task#8, ec2-tgwc-traj-empty-tgw-route-table-search#teardown#13. Each fixture builds the dependency (pool CIDR, subnet occupant, ACL association, cross-VPC group, deleted resource) and then issues the call AWS refuses.
