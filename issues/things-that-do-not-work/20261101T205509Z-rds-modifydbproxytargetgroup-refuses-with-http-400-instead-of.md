Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-proxy-lifecycle
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-proxy-lifecycle
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|ModifyDBProxyTargetGroup|2xx->4xx|$.ModifyDBProxyTargetGroupResponse
Issue-Key: rds|ModifyDBProxyTargetGroup|2xx->4xx|status
Issue-Commit: 92d1533bc984843777edac81f6082914baeb07b3

# rds ModifyDBProxyTargetGroup refuses with HTTP 400 instead of applying the connection-pool settings and answering 200

> _Note: this issue was identified against the code base at commit `92d1533bc984843777edac81f6082914baeb07b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyDBProxyTargetGroup

## Description
1 cause.

Cause 1 of 1: ModifyDBProxyTargetGroup on a proxy's one target group (named `default`) answers HTTP 400 with no success envelope at all, so the documented "create a proxy, then tune its connection pool" flow cannot complete. Fixture rds-db-proxy-lifecycle, step task#11.

AWS: Smithy (com.amazonaws.rds#ModifyDBProxyTargetGroup) — input `DBProxyName` and `TargetGroupName` are required (len 1..63, pattern ^[a-zA-Z](?:-?[a-zA-Z0-9]+)*$), `ConnectionPoolConfig` (ConnectionPoolConfiguration) carries only the settings the caller wants CHANGED; output is `ModifyDBProxyTargetGroupResponse` -> `ModifyDBProxyTargetGroupResult` -> `DBProxyTargetGroup` at HTTP 200 (SUCCESS_STATUS 200). The operation models exactly three errors: DBProxyNotFoundFault (HTTP 404), DBProxyTargetGroupNotFoundFault (HTTP 404) and InvalidDBProxyStateFault (HTTP 400, "The requested operation can't be performed while the proxy is in this state."). Recorded behavior: against AWS this step is a 200 whose DBProxyTargetGroup reports TargetGroupName `default`, IsDefault true, Status `available`, the proxy's TargetGroupArn, an UpdatedDate strictly later than CreatedDate, and the merged ConnectionPoolConfig (MaxConnectionsPercent, MaxIdleConnectionsPercent, ConnectionBorrowTimeout, an empty SessionPinningFilters list) — i.e. AWS ACCEPTS the target-group modification on a proxy that has only just been created, because the group has no provisioning of its own.

Emulator: the step answers HTTP 400 and serves no ModifyDBProxyTargetGroupResponse envelope at all — replay reports `status: expected 200, got 400` and `$.ModifyDBProxyTargetGroupResponse: missing (... got None)`. Hypothesis: the handler gates the modification on the proxy's status and this is its only 400 path reachable by this request — it returns InvalidDBProxyStateFault with the message `DB proxy <DBProxyName> is in state <status> and its target group cannot be modified` whenever the stored proxy's status is not `available`, and a newly created proxy only leaves `creating` once the emulator's own 300 ms status-transition delay has elapsed. The same window is visible one step earlier in the same fixture: at task#10 DescribeDBProxyTargetGroups reports Status `creating` where AWS reported `available` (that read-side divergence is already carried by the open issue 20261029T214632Z-rds-omits-vpc-subnet-and-availability-zone-attributes.md; no open issue covers this operation's 400). Neither the `NewName` refusal nor a ConnectionPoolConfig range refusal applies: the recorded request is accepted by AWS with TargetGroupName `default` and in-range pool values.

Why: a client that follows the RDS Proxy setup sequence — CreateDBProxy, RegisterDBProxyTargets, then ModifyDBProxyTargetGroup to set MaxConnectionsPercent / MaxIdleConnectionsPercent / ConnectionBorrowTimeout — gets a 400 refusal instead of the modified group, so the pool settings are never applied and every later read reports the defaults. Because the emulator reports the proxy as `creating` at exactly the same moment, an SDK caller cannot recover by waiting on a status it already observed as `available` against AWS; the call is simply unavailable for the whole post-create window, and any state the emulator would have written (ConnectionPoolConfig, UpdatedDate) is lost.

Fix: the ModifyDBProxyTargetGroup handler's proxy-state gate and the proxy status it reads — a target group modification must be accepted for a proxy this service holds (the group is created with the proxy and is ready exactly when the proxy record exists), and the operation must answer 200 with the merged ConnectionPoolConfig and a refreshed UpdatedDate. Reserve InvalidDBProxyStateFault for the states AWS actually refuses in (a proxy on its way out). Belongs in the operation's handler, with the status/transition semantics it reads in the service's proxy state.

Traces: rds-db-proxy-lifecycle#task#11 (this cause). Code citations: services/service-com-amazonaws-rds-amazonrdsv19/src/ops/modify_db_proxy_target_group.rs lines 191-206 (the `proxy.status != DB_PROXY_STATUS_AVAILABLE` refusal), lines 145 and 237-239 (SUCCESS_STATUS 200 and the success envelope that is never reached); services/service-com-amazonaws-rds-amazonrdsv19/src/state.rs line 175 (STATUS_TRANSITION_DELAY = 300 ms); services/service-com-amazonaws-rds-amazonrdsv19/src/state/db_proxies.rs lines 33-46 and 386-403 (proxy statuses and target_group_to_wire, which reports the PROXY's status for the group).

## Evidence
Fixture rds-db-proxy-lifecycle, step task#11, operation ModifyDBProxyTargetGroup (service rds).

Expected (recorded AWS): HTTP 200, body ModifyDBProxyTargetGroupResponse.ModifyDBProxyTargetGroupResult.DBProxyTargetGroup with DBProxyName = the proxy named in the request, TargetGroupName 'default', IsDefault 'true', Status 'available', TargetGroupArn arn:aws:rds:<region>:<account>:target-group:<proxy-name>, CreatedDate < UpdatedDate, and ConnectionPoolConfig {MaxConnectionsPercent, MaxIdleConnectionsPercent, ConnectionBorrowTimeout, SessionPinningFilters empty} as merged from the request.

Actual (emulator): HTTP 400; replay detail: "status: expected 200, got 400" and "$.ModifyDBProxyTargetGroupResponse: missing (expected {...ModifyDBProxyTargetGroupResult...}, got None)". No success envelope is served. divergence_key: service=rds|ModifyDBProxyTargetGroup|status=200->400|keys=$.ModifyDBProxyTargetGroupResponse,status. affected_count 1 (rds-db-proxy-lifecycle#task#11).

Supporting evidence for the cause: the immediately preceding step in the same fixture (rds-db-proxy-lifecycle#task#10, DescribeDBProxyTargetGroups) diverges as Status value_mismatch (expected 'available', got 'creating'), i.e. the emulator still holds the proxy in 'creating' at the moment this modification arrives, which is the single condition under which the handler answers 400 (InvalidDBProxyStateFault, message 'DB proxy <name> is in state <status> and its target group cannot be modified' — fixed text with the DBProxyName and status interpolated from the request and state).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Fixture rds-db-proxy-lifecycle: CreateDBProxy for a proxy (EngineFamily MYSQL), register its target, DescribeDBProxies / DescribeDBProxyTargets / DescribeDBProxyTargetGroups, then at step task#11 call ModifyDBProxyTargetGroup with DBProxyName=<that proxy>, TargetGroupName=default and ConnectionPoolConfig {MaxConnectionsPercent=50, MaxIdleConnectionsPercent=25, ConnectionBorrowTimeout=120} immediately after the create (no wait).
3. Observe HTTP 400 with no ModifyDBProxyTargetGroupResponse body, where the recording has HTTP 200 carrying the modified DBProxyTargetGroup (Status 'available', merged ConnectionPoolConfig, UpdatedDate > CreatedDate).
4. Re-replay rds-db-proxy-lifecycle#task#11 to confirm a fix.
