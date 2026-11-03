Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-dvpc-traj-describe-vpcs-page-size-and-token-validation
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-dvpc-traj-describe-vpcs-page-size-and-token-validation ec2-pst-traj-route-table-page-size-mistakes ec2-sshr-traj-security-group-rules-api-takes-its-own-filter-names ec2-ipam-trajwire-read-side-mistakes ec2-ipam-trajwire-stale-pool-reference ec2-byvpc-traj-batched-ids-hit-per-call-limits ec2-ecc-traj-instance-reference-mistakes ec2-ecc-traj-running-sweep-state-filter-mistakes ec2-xrg-malformed-instance-id-rejected-but-filter-returns-empty ec2-dvpc-traj-filter-and-attribute-name-validation ec2-dvpc-traj-text-output-none-fed-back-as-id ec2-tgwc-traj-tgw-id-lookup-mistakes ec2-tgwc-traj-tgw-route-table-lookup-mistakes
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeVpcs|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DescribeVpcs|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeRouteTables|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DescribeRouteTables|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeSecurityGroupRules|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|GetIpamPoolAllocations|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|GetIpamPoolAllocations|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DescribeInstances|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DescribeTransitGatewayRouteTables|4xx->4xx|$.Response.Errors.Error.Message
Issue-Related: 20261029T200817Z-ec2-parameter-limit-refusals-report-emulator-authored-limits.md
Issue-Related: 20261103T071133Z-ec2-confuses-malformed-and-not-found-ids-and.md
Issue-Commit: ce4f28f15b4841beefbad6c596b9d2158570753e

# EC2 pagination and filter-validation refusals use emulator-authored codes, limits and text

> _Note: this issue was identified against the code base at commit `ce4f28f15b4841beefbad6c596b9d2158570753e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVpcs

## Description
The emulator's paging and filter checks answer their own error codes ('InvalidMaxResults', 'InvalidParameterValue' where AWS uses 'InvalidFilter'/'FilterLimitExceeded'), spell the parameter name with the wire casing instead of AWS's, enforce limits AWS does not use, and run paging validation before id resolution. Status 400 is right throughout; code, message and precedence are not.

Cause 1 of 10: DescribeVpcs MaxResults below the floor.
AWS: Smithy: DescribeVpcs.MaxResults targets DescribeVpcsMaxResults with range min 5 max 1000; a value of 4 is refused 400 Code 'InvalidParameterValue', Message 'Value ( 4 ) for parameter MaxResults is invalid. Expecting a value greater than or equal to 5.' — fixed text with the request's value and the modeled minimum substituted, and the parameter named MaxResults (AWS's member name).
Emulator: 400 Code 'InvalidMaxResults', Message 'Value ( 4 ) for parameter maxResults is invalid. Expecting a value greater than or equal to 5.' (lower-cased parameter name).
Why: 'InvalidMaxResults' is not an EC2 error code, so client error handling never matches it.
Fix: paging validation shared by the describe handlers (services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_vpcs.rs and its max-results helper).
Traces: ec2-dvpc-traj-describe-vpcs-page-size-and-token-validation#task#1 (cluster also covers task#2/#3/#4).

Cause 2 of 10: DescribeRouteTables MaxResults below the floor — same wrong code and lower-cased parameter name.
AWS: 400 'InvalidParameterValue', 'Value ( 4 ) for parameter MaxResults is invalid. Expecting a value greater than or equal to 5.'
Emulator: 'InvalidMaxResults' + 'maxResults'.
Why/Fix: as cause 1, in describe_route_tables.rs.
Traces: ec2-pst-traj-route-table-page-size-mistakes#task#3 (cluster also covers task#4).

Cause 3 of 10: DescribeSecurityGroupRules MaxResults below the floor.
AWS: 400 Code 'InvalidParameterValue' (message matched).
Emulator: Code 'InvalidMaxResults'.
Why/Fix: as cause 1, in describe_security_group_rules.rs.
Traces: ec2-sshr-traj-security-group-rules-api-takes-its-own-filter-names#task#6.

Cause 4 of 10: GetIpamPoolAllocations MaxResults message.
AWS: recorded behavior only — 400 Message 'The number of requested max results must be between 1000 and 100000.' (IPAM allocations use their own bounds and their own sentence).
Emulator: 'Value ( 5 ) for parameter maxResults is invalid. Expecting a value greater than or equal to 1000.' — the generic EC2 sentence with a different bound story and no upper bound.
Why: the caller is not told the accepted range AWS states.
Fix: paging validation in get_ipam_pool_allocations.rs.
Traces: ec2-ipam-trajwire-read-side-mistakes#task#7.

Cause 5 of 10: GetIpamPoolAllocations validation precedence.
AWS: recorded behavior only — when the request names an absent pool AND an out-of-range MaxResults, AWS refuses on the id: 400 Code 'InvalidIpamPoolId.NotFound', Message "The ipam-pool ID 'ipam-pool-0000000000000000f' does not exist".
Emulator: 400 Code 'InvalidMaxResults' with the max-results sentence — paging validation pre-empts resource resolution.
Why: the caller fixes the page size and then hits a second, different refusal; retry loops that key on NotFound never see it.
Fix: order of checks in get_ipam_pool_allocations.rs (resolve the pool id first).
Traces: ec2-ipam-trajwire-stale-pool-reference#task#8.

Cause 6 of 10: DescribeIpamPools with an unknown filter name.
AWS: recorded behavior only — 400 Code 'InvalidFilter' (message matched).
Emulator: 400 Code 'InvalidParameterValue'.
Why: AWS has a dedicated code for an unusable filter; clients that branch on InvalidFilter mis-handle the refusal.
Fix: filter validation in describe_ipam_pools.rs.
Traces: ec2-ipam-trajwire-read-side-mistakes#task#5.

Cause 7 of 10: DescribeInstances with too many filter values.
AWS: recorded behavior only — 400 Code 'FilterLimitExceeded', Message 'The maximum number of filter values specified on a single call is 200' (fixed text; the limit 200 is AWS's per-call cap).
Emulator: 400 Code 'InvalidParameterValue', Message "The filter 'vpc-id' carries 201 values, above the maximum of 64" — a cap of 64 that AWS does not impose and an emulator-authored sentence.
Why: requests with 65..200 filter values that AWS serves are refused, and the refusal a caller does get carries the wrong code and limit.
Fix: filter-value limit in describe_instances.rs / the shared filter helper.
Traces: ec2-byvpc-traj-batched-ids-hit-per-call-limits#task#1 (cluster also covers ec2-ecc-traj-instance-reference-mistakes#task#1, ec2-ecc-traj-running-sweep-state-filter-mistakes#task#11, ec2-xrg-malformed-instance-id-rejected-but-filter-returns-empty#task#2).

Cause 8 of 10: DescribeVpcs with Filter.Name sent as the literal 'null'.
AWS: recorded behavior only — 400 (code matched) Message "The filter 'null' is invalid" — AWS echoes the value it received.
Emulator: "The filter '' is invalid" — the value is dropped before it reaches the message template.
Why: the operator cannot see which filter name was rejected.
Fix: filter-name echo in describe_vpcs.rs / shared filter parsing.
Traces: ec2-dvpc-traj-filter-and-attribute-name-validation#task#1 (cluster also covers ec2-dvpc-traj-text-output-none-fed-back-as-id#task#1).

Cause 9 of 10: DescribeTransitGateways with an unsupported filter name.
AWS: recorded behavior only — 400 Message 'Value (vpc-id) for parameter Filter is invalid.' (fixed text 'Value (' + filter name + ') for parameter Filter is invalid.').
Emulator: "The filter 'vpc-id' is invalid".
Why: message diverges from AWS for every unsupported transit-gateway filter.
Fix: filter validation text in describe_transit_gateways.rs.
Traces: ec2-tgwc-traj-tgw-id-lookup-mistakes#task#5.

Cause 10 of 10: DescribeTransitGatewayRouteTables with an unsupported filter name — same divergence as cause 9.
AWS: 'Value (vpc-id) for parameter Filter is invalid.'
Emulator: "The filter 'vpc-id' is invalid".
Fix: describe_transit_gateway_route_tables.rs.
Traces: ec2-tgwc-traj-tgw-route-table-lookup-mistakes#task#3.

## Evidence
Replayed wire corpus simparity-wire-aws-v1. Representative: ec2-dvpc-traj-describe-vpcs-page-size-and-token-validation task#1 — expected 400 / 'InvalidParameterValue' / 'Value ( 4 ) for parameter MaxResults is invalid. Expecting a value greater than or equal to 5.'; actual 400 / 'InvalidMaxResults' / '...for parameter maxResults...'. Further clusters: DescribeRouteTables and DescribeSecurityGroupRules same wrong code; GetIpamPoolAllocations wrong range sentence and wrong precedence vs InvalidIpamPoolId.NotFound; DescribeIpamPools InvalidFilter vs InvalidParameterValue; DescribeInstances FilterLimitExceeded/200 vs InvalidParameterValue/64; DescribeVpcs drops the rejected filter name; both transit-gateway describes reword the invalid-filter sentence.

## Steps to Reproduce
1. Call DescribeVpcs with MaxResults=4 (fixture ec2-dvpc-traj-describe-vpcs-page-size-and-token-validation, task#1) and compare code and parameter casing.
2. Call DescribeInstances with 201 values on the vpc-id filter (ec2-byvpc-traj-batched-ids-hit-per-call-limits, task#1) and observe the 64-value cap and InvalidParameterValue instead of FilterLimitExceeded at 200.
3. Call GetIpamPoolAllocations naming an absent pool with MaxResults=5 (ec2-ipam-trajwire-stale-pool-reference, task#8) and observe paging validation pre-empting the NotFound refusal.
4. Repeat the remaining fixture#step tokens in trace_ids.
