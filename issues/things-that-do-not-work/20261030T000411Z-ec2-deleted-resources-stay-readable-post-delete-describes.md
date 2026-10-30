Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-network-insights-access-scope-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-network-insights-access-scope-lifecycle ec2-traffic-mirror-filter-lifecycle ec2-route-server-delete-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeNetworkInsightsAccessScopes|4xx->2xx|$
Issue-Key: ec2|DescribeNetworkInsightsAccessScopes|4xx->2xx|$.Response
Issue-Key: ec2|DescribeNetworkInsightsAccessScopes|4xx->2xx|status
Issue-Key: ec2|GetNetworkInsightsAccessScopeContent|4xx->2xx|$
Issue-Key: ec2|GetNetworkInsightsAccessScopeContent|4xx->2xx|$.Response
Issue-Key: ec2|GetNetworkInsightsAccessScopeContent|4xx->2xx|status
Issue-Key: ec2|DescribeTrafficMirrorFilters|4xx->2xx|$
Issue-Key: ec2|DescribeTrafficMirrorFilters|4xx->2xx|$.Response
Issue-Key: ec2|DescribeTrafficMirrorFilters|4xx->2xx|status
Issue-Key: ec2|DescribeRouteServers|2xx->2xx|$
Issue-Key: ec2|DescribeRouteServers|2xx->2xx|$.DescribeRouteServersResponse.routeServerSet.item.state
Issue-Commit: 7c2160283d3342f6e10783a5c4b2d926b7b7bab3

# EC2 deleted resources stay readable: post-delete describes answer 200 instead of the NotFound refusal

> _Note: this issue was identified against the code base at commit `7c2160283d3342f6e10783a5c4b2d926b7b7bab3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeNetworkInsightsAccessScopes

## Description
A successful Delete* call does not remove (or retire) the resource in emulator state, so every read path that follows still resolves the deleted id. AWS makes the deletion observable immediately: the id stops resolving and the read refuses with that resource type's Invalid<Resource>Id.NotFound error, or (for resources with a terminal state) the describe reports the terminal state. This bundle carries 4 causes.

Cause 1 of 4: DescribeNetworkInsightsAccessScopes after DeleteNetworkInsightsAccessScope.
AWS: once the access scope is deleted, describing it by id is refused with HTTP 400 and error code InvalidNetworkInsightsAccessScopeId.NotFound. Message template: the fixed text " does not exist" preceded by the variable part - the requested nis-* id from the request (recorded: "nis-00000000000000022 does not exist"). Recorded behavior only for the message wording; the model declares no operation-level errors for this operation (Smithy: DescribeNetworkInsightsAccessScopes errors: []).
Emulator: answers HTTP 200 with a DescribeNetworkInsightsAccessScopesResponse whose networkInsightsAccessScopeSet still contains the deleted id, and keeps doing so for the whole poll window (56 attempts), so no error code is ever produced.
Why: a client that polls for the NotFound refusal to confirm deletion never terminates, and a client that lists scopes sees a resource that no longer exists.
Fix: the delete handler's state mutation plus the describe handler's id lookup for network-insights access scopes (services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_network_insights_access_scope.rs and describe_network_insights_access_scopes.rs).
Traces: ec2-network-insights-access-scope-lifecycle#task#5.

Cause 2 of 4: GetNetworkInsightsAccessScopeContent after the same delete.
AWS: refuses with HTTP 400 / InvalidNetworkInsightsAccessScopeId.NotFound, same message template (requested id + " does not exist").
Emulator: answers HTTP 200 with GetNetworkInsightsAccessScopeContentResponse carrying networkInsightsAccessScopeContent for the deleted id.
Why: content of a deleted scope is served as if live; deletion is unverifiable through this path too.
Fix: the id lookup in the GetNetworkInsightsAccessScopeContent handler, sharing the deletion-aware state introduced for Cause 1.
Traces: ec2-network-insights-access-scope-lifecycle#task#6.

Cause 3 of 4: DescribeTrafficMirrorFilters after DeleteTrafficMirrorFilter.
AWS: refuses with HTTP 400 and error code InvalidTrafficMirrorFilterId.NotFound; message is the requested tmf-* id followed by the fixed text " does not exist" (recorded: "tmf-00000000000000022 does not exist").
Emulator: answers HTTP 200 and still returns the filter in trafficMirrorFilterSet through the whole poll window.
Why: teardown and idempotency checks that key on the NotFound refusal pass against a resource that was never removed.
Fix: the traffic-mirror-filter state removal in the delete handler and the id lookup in describe_traffic_mirror_filters.rs. Note this is distinct from the already-open DeleteTrafficMirrorFilter stub issue (20261029T200817Z-ec2-deletetrafficmirrorfilter-stub.md): that issue covers the unimplemented delete operation, this cause covers the read path still resolving a removed id.
Traces: ec2-traffic-mirror-filter-lifecycle#teardown#12.

Cause 4 of 4: DescribeRouteServers after DeleteRouteServer.
AWS: the route server remains describable but its state advances to the terminal value "deleted".
Emulator: state stays "available" indefinitely (poll not in ['deleted'] after 61 attempts, poll capped at 60s).
Why: a client waiting for state == deleted before releasing dependent resources hangs.
Fix: the route-server lifecycle state transition owned by the delete handler and projected by describe_route_servers.rs.
Traces: ec2-route-server-delete-lifecycle#task#7.

## Evidence
Cause 1 - expected 400 {Code: InvalidNetworkInsightsAccessScopeId.NotFound, Message: '<nis-id> does not exist'}, actual 200 with the scope still listed (poll: error code None not in ['InvalidNetworkInsightsAccessScopeId.NotFound'] after 56 attempts).
Cause 2 - expected 400 InvalidNetworkInsightsAccessScopeId.NotFound, actual 200 with networkInsightsAccessScopeContent for the deleted id.
Cause 3 - expected 400 InvalidTrafficMirrorFilterId.NotFound '<tmf-id> does not exist', actual 200 with the filter still in trafficMirrorFilterSet.
Cause 4 - $.DescribeRouteServersResponse.routeServerSet.item.state: expected 'deleted', got 'available' (poll: not in ['deleted'] after 61 attempts).

## Steps to Reproduce
Replay the wire corpus simparity-wire-aws-v1 against the emulator and inspect: ec2-network-insights-access-scope-lifecycle#task#5 and #task#6, ec2-traffic-mirror-filter-lifecycle#teardown#12, ec2-route-server-delete-lifecycle#task#7. Each fixture creates the resource, deletes it, then reads it back by id.
