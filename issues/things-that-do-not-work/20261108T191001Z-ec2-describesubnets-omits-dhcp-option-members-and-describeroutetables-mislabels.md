Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-madl-traj-vpc-subnet-discovery-filters
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-madl-traj-vpc-subnet-discovery-filters ec2-pst-traj-route-filters-match-per-table-not-per-route
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeSubnets|2xx->2xx|$.DescribeSubnetsResponse.subnetSet.item[subnetId="subnet-00000000000000089"].dhcpOptionsId
Issue-Key: ec2|DescribeSubnets|2xx->2xx|$.DescribeSubnetsResponse.subnetSet.item[subnetId="subnet-00000000000000089"].dhcpOptionsSource
Issue-Key: ec2|DescribeSubnets|2xx->2xx|$.DescribeSubnetsResponse.subnetSet.item[subnetId="subnet-0000000000000008b"].dhcpOptionsId
Issue-Key: ec2|DescribeSubnets|2xx->2xx|$.DescribeSubnetsResponse.subnetSet.item[subnetId="subnet-0000000000000008b"].dhcpOptionsSource
Issue-Key: ec2|DescribeRouteTables|2xx->2xx|$.DescribeRouteTablesResponse.routeTableSet.item[routeTableId="rtb-0000000000000009c"].routeSet.item[*].state
Issue-Commit: e96438ac8652a0a4da19e4ddf11cb276239c17b7

# DescribeSubnets omits DHCP option members and DescribeRouteTables mislabels a blackhole route as active

> _Note: this issue was identified against the code base at commit `e96438ac8652a0a4da19e4ddf11cb276239c17b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSubnets

## Description
This issue bundles 2 root causes, both in ec2 VPC read paths, both net-new (neither cluster is owned by an open issue and neither is blocked by a stub). The related open issues 20261103T123451Z-ec2-describe-projections-omit-modeled-response-members.md and 20261104T062058Z-ec2-describe-projections-serve-synthesized-catalog-data-and.md cover other operations' projections and do NOT carry these replay keys. Both causes must be closed.

Cause 1 of 2: DescribeSubnets omits the subnet's DHCP-options identity.
AWS: recorded behavior only — the vendored Smithy model's com.amazonaws.ec2#Subnet shape in this checkout has no DhcpOptionsId/DhcpOptionsSource member, so the requirement comes from the recording: for every subnet returned, AWS emits dhcpOptionsId (the id of the DHCP option set in effect for the subnet, i.e. the one associated with the subnet's VPC when the subnet has no override) and dhcpOptionsSource (the scope that supplied it, 'vpc' when inherited from the VPC). The recorded step returns both members on each item of subnetSet, with the dhcpOptionsId naming the VPC's associated option set.
Emulator: both members are absent from every subnetSet.item (replay reports 'missing (expected ..., got None)'), for every subnet in the response.
Why: a client that reads a subnet's effective DHCP option set (to decide whether to override it, or to render the subnet's DNS configuration) sees no value at all and cannot tell an inherited option set from an absent one.
Fix: the DescribeSubnets response projection in the ec2 service crate must resolve the subnet's effective DHCP option set from the owning VPC's association and emit dhcpOptionsId plus dhcpOptionsSource per item; the same derivation already belongs beside the VPC state that DescribeVpcs reads.

Cause 2 of 2: a route whose target is no longer available is reported as active instead of blackhole.
AWS: Smithy com.amazonaws.ec2#Route member State (enum RouteState) documents it: "The blackhole state indicates that the route's target isn't available (for example, the specified gateway isn't attached to the VPC, or the specified NAT instance has been terminated)." So after the route's target stops being attached/available, DescribeRouteTables must report that route's state as 'blackhole' while the route itself stays in the table.
Emulator: the route keeps state 'active' after its target became unavailable — routeSet.item[...].state expected 'blackhole', got 'active' — and the cluster shows the same wrong state on three consecutive read steps of the fixture, so it is not a timing artifact.
Why: a client that audits route-table health, or that filters routes by state to find broken routes, concludes traffic is still routed when AWS would report the route dead; failover and cleanup logic keyed on 'blackhole' never fires.
Fix: route state must be derived from the target's current availability whenever route tables are read (or recomputed when a gateway/NAT/interface target is detached or deleted) in the ec2 route-table state and the DescribeRouteTables projection, instead of being frozen at creation time.

## Evidence
Cause 1 — fixture ec2-madl-traj-vpc-subnet-discovery-filters, step task#8, DescribeSubnets, expected 200, got 200: $.DescribeSubnetsResponse.subnetSet.item[subnetId="subnet-0000000000000008b"].dhcpOptionsId missing (expected the VPC's associated option-set id, recorded as 'dopt-03d13527b92584567'); .dhcpOptionsSource missing (expected 'vpc'); identically for item[subnetId="subnet-00000000000000089"]. The id value is environment-specific; the requirement is that both members are present and that dhcpOptionsId names the option set associated with the subnet's VPC, with dhcpOptionsSource 'vpc' when inherited.

Cause 2 — fixture ec2-pst-traj-route-filters-match-per-table-not-per-route, step task#11 (cluster also covers task#12 and task#13), DescribeRouteTables, expected 200, got 200: $.DescribeRouteTablesResponse.routeTableSet.item[routeTableId="rtb-0000000000000009c"].routeSet.item[2].state expected 'blackhole', got 'active'. The route-table id and route index are environment-specific; the requirement is that any route whose target is no longer available reports state 'blackhole'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture ec2-madl-traj-vpc-subnet-discovery-filters step task#8 (DescribeSubnets after the VPC/subnets are created): every subnetSet.item comes back without dhcpOptionsId and dhcpOptionsSource.
3. Replay fixture ec2-pst-traj-route-filters-match-per-table-not-per-route up to step task#11 (the route's target has been removed earlier in the fixture) and read the route table: the affected route still reports state 'active'; steps task#12 and task#13 repeat the read with the same wrong state.
4. Both steps must replay clean before the issue is closed.
