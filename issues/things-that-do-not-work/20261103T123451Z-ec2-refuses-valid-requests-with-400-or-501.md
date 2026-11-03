Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-ipam-trajwire-teardown-order-mistakes
Issue-Step: setup#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-ipam-trajwire-teardown-order-mistakes ec2-ipam-trajwire-create-vpc-parameter-mistakes ec2-ipam-trajwire-reference-path-regional-child-pool ec2-ipam-trajwire-vpc-from-top-pool-then-child ec2-ipam-trajwire-child-pool-create-mistakes ec2-pip-traj-same-private-ip-in-two-vpcs ec2-pst-traj-instance-joined-to-public-subnet-by-subnet-filter ec2-ssm-interface-endpoint-private-dns-converges-to-available-and-deletes-to-not-found ec2-trajwire-deb-deb-security-group-from-another-vpc ec2-vpce-traj-iot-data-endpoint-create-refusals ec2-sshp-traj-nacl-entry-authoring-mistakes ec2-sshp-traj-subnet-nacl-is-always-explicitly-associated ec2-madl-traj-vpc-subnet-discovery-filters ec2-pst-traj-filter-name-and-value-mistakes
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateVpc|2xx->5xx|status
Issue-Key: ec2|CreateVpc|2xx->5xx|$.CreateVpcResponse
Issue-Key: ec2|CreateIpamPool|2xx->4xx|$.CreateIpamPoolResponse
Issue-Key: ec2|CreateIpamPool|2xx->4xx|status
Issue-Key: ec2|CreateNetworkInterface|2xx->4xx|status
Issue-Key: ec2|DeleteSubnet|2xx->4xx|status
Issue-Key: ec2|DeleteVpc|2xx->4xx|status
Issue-Key: ec2|CreateNetworkAclEntry|2xx->4xx|$.CreateNetworkAclEntryResponse
Issue-Key: ec2|CreateNetworkAclEntry|2xx->4xx|status
Issue-Key: ec2|DeleteNetworkAcl|2xx->4xx|$.DeleteNetworkAclResponse
Issue-Key: ec2|DeleteNetworkAcl|2xx->4xx|status
Issue-Key: ec2|DeleteSecurityGroup|2xx->4xx|status
Issue-Key: ec2|DescribeSubnets|2xx->4xx|$.DescribeSubnetsResponse
Issue-Key: ec2|DescribeSubnets|2xx->4xx|status
Issue-Commit: 10d8f420d9792c6a92213c81068536eef787b058

# EC2 refuses valid requests with 400 or 501 across create, delete and describe paths

> _Note: this issue was identified against the code base at commit `10d8f420d9792c6a92213c81068536eef787b058`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpc

## Description
Ten net-new causes sharing one fault: a legal request variant that AWS serves with 200 is rejected — either with a 400 refusal the recording does not contain, or with 501 NotImplemented from an operation that IS implemented for other request shapes. Because these are setup/teardown steps the whole remainder of each fixture is built on a failed prerequisite.

Cause 1 of 10 — CreateVpc answers 501 for the IPAM-pool-allocated request shape.
AWS: CreateVpc with Ipv4IpamPoolId (+Ipv4NetmaskLength) returns 200 with the vpc projection (state 'pending', the allocated cidrBlock, cidrBlockAssociationSet, dhcpOptionsId, isDefault 'false').
Emulator: answers 501 with no body.
Why: every IPAM-driven VPC provisioning flow is impossible, and a 501 tells the client the operation does not exist rather than that one parameter is unsupported.
Fix: the CreateVpc handler's IPAM-allocation branch.
Traces: ec2-ipam-trajwire-teardown-order-mistakes#setup#11, plus the 3-step cluster ec2-ipam-trajwire-create-vpc-parameter-mistakes#task#16.

Cause 2 of 10 — CreateIpamPool refuses a valid regional child pool with 400.
AWS: creating a child pool under a top-level pool in the IPAM's own locale returns 200 with the ipamPool projection (poolDepth '2', sourceIpamPoolId, state 'create-in-progress', ipamScopeType 'private').
Emulator: answers 400.
Why: the nested-pool topology every IPAM deployment uses cannot be built.
Fix: CreateIpamPool validation (the source-pool/locale acceptance rule).
Traces: ec2-ipam-trajwire-child-pool-create-mistakes#task#8.

Cause 3 of 10 — CreateNetworkInterface refuses a private IP already used in a DIFFERENT VPC.
AWS: private addresses are VPC-scoped, so the same address in another VPC returns 200.
Emulator: answers 400.
Why: address uniqueness is enforced account-wide instead of per VPC, which blocks legal overlapping-CIDR designs.
Fix: CreateNetworkInterface validation / the private-address index key in state (must include the VPC).
Traces: ec2-pip-traj-same-private-ip-in-two-vpcs#setup#6.

Cause 4 of 10 — DeleteSubnet refuses a subnet with no remaining dependencies.
AWS: once the occupants are gone DeleteSubnet returns 200.
Emulator: answers 400.
Why: teardown cannot complete, so repeated runs leak subnets.
Fix: DeleteSubnet's dependency bookkeeping in state (stale references survive the dependents' deletion).
Traces: ec2-pst-traj-instance-joined-to-public-subnet-by-subnet-filter#teardown#20 (cluster also covers teardown#9 of ec2-ssm-interface-endpoint-..., teardown#13 of ec2-trajwire-deb-deb-security-group-from-another-vpc and teardown#9 of ec2-vpce-traj-iot-data-endpoint-create-refusals).

Cause 5 of 10 — DeleteVpc refuses an emptied VPC.
AWS: after its subnets/endpoints/groups are deleted DeleteVpc returns 200.
Emulator: answers 400.
Why: same leak at the VPC level; the account never returns to a clean state.
Fix: DeleteVpc's dependency bookkeeping in state.
Traces: ec2-ssm-interface-endpoint-private-dns-converges-to-available-and-deletes-to-not-found#teardown#10 (cluster also covers teardown#12/#14 of ec2-trajwire-deb-deb-security-group-from-another-vpc and teardown#10 of ec2-vpce-traj-iot-data-endpoint-create-refusals).

Cause 6 of 10 — CreateNetworkAclEntry refuses a valid entry.
AWS: returns 200 with CreateNetworkAclEntryResponse carrying return 'true'.
Emulator: answers 400 with no body.
Why: NACL authoring is blocked, so no client can express a deny rule ahead of the default allow.
Fix: CreateNetworkAclEntry validation / handler.
Traces: ec2-sshp-traj-nacl-entry-authoring-mistakes#task#8.

Cause 7 of 10 — DeleteNetworkAcl refuses a disassociated, non-default ACL.
AWS: returns 200 with return 'true'.
Emulator: answers 400.
Why: ACLs accumulate and the fixture's teardown cannot complete.
Fix: DeleteNetworkAcl validation (association bookkeeping after a replace).
Traces: ec2-sshp-traj-subnet-nacl-is-always-explicitly-associated#task#14.

Cause 8 of 10 — DeleteSecurityGroup refuses a group with no remaining members.
AWS: returns 200 once nothing references the group.
Emulator: answers 400.
Why: security groups leak across runs.
Fix: DeleteSecurityGroup's reference bookkeeping in state.
Traces: ec2-trajwire-deb-deb-security-group-from-another-vpc#teardown#11.

Cause 9 of 10 — DescribeSubnets refuses a valid filtered read with 400.
AWS: returns 200 with the two matching subnets.
Emulator: answers 400 with no body.
Why: the discovery step every launch flow starts from fails outright.
Fix: the DescribeSubnets filter validation (the recorded filter combination is rejected).
Traces: ec2-madl-traj-vpc-subnet-discovery-filters#task#10 (cluster also covers ec2-pst-traj-filter-name-and-value-mistakes#task#14).

Cause 10 of 10 — the 501 NotImplemented class of answer on an implemented operation.
AWS: an unsupported parameter is answered with a modeled 400 parameter error, never with 501 — EC2 has no NotImplemented error class.
Emulator: returns HTTP 501 with Code 'NotImplemented' for the request shapes above.
Why: a client cannot distinguish 'this API does not exist' from 'this parameter combination is unsupported', and SDK retry policies treat 501 as terminal for the whole service.
Fix: the request-shape dispatch in the CreateVpc handler (and the shared scaffold that emits 501 from an implemented op).
Traces: ec2-ipam-trajwire-teardown-order-mistakes#setup#11, ec2-ipam-trajwire-create-vpc-parameter-mistakes#task#16.

## Evidence
Replay verdicts (expected -> actual):
Cause 1: CreateVpc status 200 -> 501, $.CreateVpcResponse missing (expected the full vpc projection with state 'pending' and the IPAM-allocated cidrBlock).
Cause 2: CreateIpamPool status 200 -> 400, $.CreateIpamPoolResponse missing (expected ipamPool with poolDepth '2', sourceIpamPoolId, state 'create-in-progress').
Cause 3: CreateNetworkInterface status 200 -> 400.
Cause 4: DeleteSubnet status 200 -> 400 (4-step cluster).
Cause 5: DeleteVpc status 200 -> 400 (4-step cluster).
Cause 6: CreateNetworkAclEntry status 200 -> 400, $.CreateNetworkAclEntryResponse missing (expected return 'true').
Cause 7: DeleteNetworkAcl status 200 -> 400, $.DeleteNetworkAclResponse missing (expected return 'true').
Cause 8: DeleteSecurityGroup status 200 -> 400.
Cause 9: DescribeSubnets status 200 -> 400, $.DescribeSubnetsResponse missing (expected subnetSet with both subnets).
Cause 10: actual Code 'NotImplemented' / HTTP 501 on CreateVpc, an operation implemented for the non-IPAM request shape.

## Steps to Reproduce
Replay the wire corpus (simparity-wire-aws-v1) and inspect ec2-ipam-trajwire-teardown-order-mistakes#setup#11, ec2-ipam-trajwire-create-vpc-parameter-mistakes#task#16, ec2-ipam-trajwire-child-pool-create-mistakes#task#8, ec2-pip-traj-same-private-ip-in-two-vpcs#setup#6, ec2-pst-traj-instance-joined-to-public-subnet-by-subnet-filter#teardown#20, ec2-ssm-interface-endpoint-private-dns-converges-to-available-and-deletes-to-not-found#teardown#10, ec2-sshp-traj-nacl-entry-authoring-mistakes#task#8, ec2-sshp-traj-subnet-nacl-is-always-explicitly-associated#task#14, ec2-trajwire-deb-deb-security-group-from-another-vpc#teardown#11, ec2-madl-traj-vpc-subnet-discovery-filters#task#10.
