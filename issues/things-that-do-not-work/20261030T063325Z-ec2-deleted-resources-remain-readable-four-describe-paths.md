Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-public-ipv4-pool-lifecycle
Issue-Step: teardown#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-public-ipv4-pool-lifecycle ec2-instance-event-window-and-application-status-check-lifecycle ec2-instance-event-window-tag-association-lifecycle ec2-network-insights-access-scope-analysis-lifecycle ec2-ipam-consolidated-delete-and-deprovision-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribePublicIpv4Pools|4xx->2xx|$
Issue-Key: ec2|DescribePublicIpv4Pools|4xx->2xx|$.Response
Issue-Key: ec2|DescribePublicIpv4Pools|4xx->2xx|status
Issue-Key: ec2|DescribeInstanceEventWindows|4xx->2xx|$
Issue-Key: ec2|DescribeInstanceEventWindows|4xx->2xx|$.Response
Issue-Key: ec2|DescribeInstanceEventWindows|4xx->2xx|status
Issue-Key: ec2|DescribeNetworkInsightsAccessScopeAnalyses|4xx->2xx|$
Issue-Key: ec2|DescribeNetworkInsightsAccessScopeAnalyses|4xx->2xx|$.Response
Issue-Key: ec2|DescribeNetworkInsightsAccessScopeAnalyses|4xx->2xx|status
Issue-Key: ec2|DescribeIpamScopes|4xx->2xx|$
Issue-Key: ec2|DescribeIpamScopes|4xx->2xx|$.Response
Issue-Key: ec2|DescribeIpamScopes|4xx->2xx|status
Issue-Commit: 78d5907a1724d3304a3021142ffd4f12ae7136a6

# EC2 deleted resources remain readable: four describe paths answer 200 instead of the recorded NotFound refusal

> _Note: this issue was identified against the code base at commit `78d5907a1724d3304a3021142ffd4f12ae7136a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribePublicIpv4Pools

## Description
Four net-new wire divergences share one root cause: a delete/deprovision call is accepted, but the resource is never removed from (or never marked gone in) emulator state, so the describe that follows keeps returning the resource with HTTP 200. The recorded AWS behaviour is an HTTP 400 Query error whose Code/Message name the now-absent id. This issue bundles 4 causes; all 4 must be closed. Related open issue '20261030T000411Z-ec2-deleted-resources-stay-readable-post-delete-describes.md' describes the same family on other operations but does NOT carry these replay keys, so it does not cover them.

Cause 1 of 4: DescribePublicIpv4Pools after the pool is deleted.
AWS: recorded behavior only (the EC2 Smithy model declares no error shapes for this operation - DescribePublicIpv4Pools.errors == []). The recording shows HTTP 400 with Response.Errors.Error.Code 'InvalidPublicIpv4PoolID.NotFound' and Message "The pool ID '<poolId>' does not exist." The fixed text is "The pool ID '" ... "' does not exist."; the variable part is the pool id taken from the request (recorded run: ipv4pool-ec2-00000000000000024).
Emulator: HTTP 200 carrying the deleted pool in DescribePublicIpv4PoolsResponse.publicIpv4PoolSet (poolId, totalAddressCount 0, networkBorderGroup, empty tagSet). The fixture's poll for the NotFound code gave up after 56 attempts.
Why: a client that deletes a pool and then confirms the deletion never sees it disappear, so cleanup loops and 'wait until gone' helpers spin until their timeout and teardown logic believes the resource leaked.
Fix: the DeletePublicIpv4Pool/deprovision handler's state mutation plus the DescribePublicIpv4Pools handler's lookup (it must raise the NotFound refusal for an id that is no longer in state).
Traces: ec2-public-ipv4-pool-lifecycle#teardown#5.

Cause 2 of 4: DescribeInstanceEventWindows after DeleteInstanceEventWindow.
AWS: recorded behavior only - HTTP 400, Code 'InvalidInstanceEventWindowIDNotFound', Message 'Invalid Instance Event Window IDs [<id>].' Fixed text 'Invalid Instance Event Window IDs [' ... '].'; the bracketed list holds the ids from the request (recorded run: iew-00000000000000024).
Emulator: HTTP 200 with the window still present and state 'active'; the poll for the NotFound code gave up after 61 attempts.
Why: the deletion is unobservable, so a client cannot tell a deleted event window from a live one and will refuse to re-create the same window.
Fix: DeleteInstanceEventWindow state mutation and the DescribeInstanceEventWindows handler's id lookup.
Traces: ec2-instance-event-window-and-application-status-check-lifecycle#task#4 (same cluster: ec2-instance-event-window-tag-association-lifecycle#teardown#9).

Cause 3 of 4: DescribeNetworkInsightsAccessScopeAnalyses after the analysis is deleted.
AWS: recorded behavior only - HTTP 400, Code 'InvalidNetworkInsightsAccessScopeAnalysisId.NotFound', Message '<analysisId> does not exist'. Fixed text ' does not exist'; the leading value is the analysis id from the request (recorded run: nisa-00000000000000025).
Emulator: HTTP 200 returning the analysis with status 'failed' and an emulator-authored statusMessage; the poll for the NotFound code gave up after 56 attempts.
Why: delete appears to do nothing, so a client's wait-for-deletion step never completes.
Fix: DeleteNetworkInsightsAccessScopeAnalysis state mutation and the DescribeNetworkInsightsAccessScopeAnalyses lookup path.
Traces: ec2-network-insights-access-scope-analysis-lifecycle#task#5.

Cause 4 of 4: DescribeIpamScopes after DeleteIpamScope.
AWS: recorded behavior only - HTTP 400, Code 'InvalidIpamScopeId.NotFound', Message 'The IPAM scope (<ipamScopeId>) does not exist.' Fixed text 'The IPAM scope (' ... ') does not exist.'; the parenthesised value is the scope id from the request (recorded run: ipam-scope-0000000000000002a).
Emulator: HTTP 200 with the scope still listed, state 'create-complete'; the poll for the NotFound code gave up after 61 attempts.
Why: IPAM teardown sequences (scope -> pool -> ipam) cannot be driven, because the client never observes the scope leaving state.
Fix: DeleteIpamScope state mutation and the DescribeIpamScopes lookup path.
Traces: ec2-ipam-consolidated-delete-and-deprovision-lifecycle#task#10.

## Evidence
Cause 1: expected 400 / InvalidPublicIpv4PoolID.NotFound / "The pool ID 'ipv4pool-ec2-00000000000000024' does not exist."; actual 200 with publicIpv4PoolSet still containing the pool (poll: error code None not in ['InvalidPublicIpv4PoolID.NotFound'] after 56 attempts).
Cause 2: expected 400 / InvalidInstanceEventWindowIDNotFound / 'Invalid Instance Event Window IDs [iew-00000000000000024].'; actual 200 with instanceEventWindowSet item state 'active' (poll capped at 60s, 61 attempts).
Cause 3: expected 400 / InvalidNetworkInsightsAccessScopeAnalysisId.NotFound / 'nisa-00000000000000025 does not exist'; actual 200 with the analysis still present (poll: 56 attempts).
Cause 4: expected 400 / InvalidIpamScopeId.NotFound / 'The IPAM scope (ipam-scope-0000000000000002a) does not exist.'; actual 200 with ipamScopeSet item state 'create-complete' (poll capped at 60s, 61 attempts).

## Steps to Reproduce
Replay the simparity-wire-aws-v1 corpus against the emulator and inspect these steps: ec2-public-ipv4-pool-lifecycle#teardown#5, ec2-instance-event-window-and-application-status-check-lifecycle#task#4, ec2-network-insights-access-scope-analysis-lifecycle#task#5, ec2-ipam-consolidated-delete-and-deprovision-lifecycle#task#10. Each fixture deletes the resource in an earlier step and then describes it; the describe must answer the 400 NotFound refusal above instead of 200.
