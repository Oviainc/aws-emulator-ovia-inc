Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-proxy-lifecycle
Issue-Step: teardown#18
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-proxy-lifecycle
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeDBProxies|4xx->2xx|$
Issue-Key: rds|DescribeDBProxies|4xx->2xx|status
Issue-Commit: 263537c8d0bbf6adc085b4147c3053be10a9244f

# rds DeleteDBProxy never removes the proxy: DescribeDBProxies keeps answering 200 available forever

> _Note: this issue was identified against the code base at commit `263537c8d0bbf6adc085b4147c3053be10a9244f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBProxies

## Description
There is 1 cause to close.

Cause 1 of 1: a deleted DB proxy is never retired from state, so the terminal not-found read never arrives.
AWS: Smithy: DeleteDBProxy(DBProxyName @required) -> DeleteDBProxyResponse{DBProxy}, errors DBProxyNotFoundFault (404, awsQueryError code DBProxyNotFoundFault) and InvalidDBProxyStateFault (400). DescribeDBProxies binds the same DBProxyNotFoundFault (404). After DeleteDBProxy the proxy transitions through deleting and is then gone; a DescribeDBProxies naming it answers HTTP 404 with Code DBProxyNotFoundFault. The message for that error class is RDS's fixed not-found sentence for the proxy resource with the requested DBProxyName interpolated as its only variable part - the proxy name comes from the request, nothing else in it is per-run.
Emulator: after the recorded DeleteDBProxy, DescribeDBProxies keeps answering HTTP 200 with the proxy still present and Status 'available' (not even 'deleting'). The fixture's teardown poll waited for DBProxyNotFoundFault, polled 61 times and was capped at 60s with the emulator still returning the full DBProxy structure (DBProxyName, DBProxyArn, Status available, Endpoint, Auth, RoleArn, timestamps).
Why: deletion is unobservable and, worse, un-effective. A client that waits for the proxy to disappear before tearing down its subnets, security groups, secret or IAM role hangs until its own timeout; a client that re-creates a proxy with the same name afterwards hits a spurious already-exists conflict; and any teardown-idempotency path that relies on DBProxyNotFoundFault never sees it. Status also never reflects the 'deleting' transition, so progress cannot be tracked.
Fix: the DeleteDBProxy handler and the DB-proxy state store (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/delete_db_proxy.rs and the proxy registry it mutates) must move the proxy to deleting and then remove it, so DescribeDBProxies/DeleteDBProxy for that name answer 404 DBProxyNotFoundFault thereafter.
Traces: rds-db-proxy-lifecycle#teardown#18.

## Evidence
Replay of simparity-wire-aws-v1 (emulator_up=true, rds answers).

rds-db-proxy-lifecycle#teardown#18, DescribeDBProxies: expected status 404, got 200.
Replay detail: '$: poll capped at 60s (expected None, got {DescribeDBProxiesResponse: {DescribeDBProxiesResult: {DBProxies: {member: {DBProxyName: <proxy>, DBProxyArn: arn:aws:rds:<region>:<account>:db-proxy:<proxy>, Status: available, EngineFamily: MYSQL, VpcSubnetIds: […], DefaultAuthScheme: NONE, Auth: {member: {AuthScheme: SECRETS, SecretArn: …, IAMAuth: DISABLED}}, RoleArn: …, Endpoint: …, RequireTLS: false, IdleClientTimeout: 1800, DebugLogging: false, CreatedDate: …, UpdatedDate: …, EndpointNetworkType: IPV4, TargetConnectionNetworkType: IPV4}}}}})' and '$: poll: error code None not in [DBProxyNotFoundFault] after 61 attempt(s)'.

Expected (AWS): HTTP 404, Code DBProxyNotFoundFault, message = RDS's fixed proxy-not-found sentence whose only variable part is the DBProxyName taken from the request. Actual (emulator): HTTP 200 with the proxy still listed, Status 'available'.

The proxy name, ARN, endpoint host, secret ARN, role path, account, region and timestamps in the recorded payload are per-run values and are not part of the requirement; the requirement is that after DeleteDBProxy the name resolves to 404 DBProxyNotFoundFault (via a 'deleting' Status first), instead of staying 'available'.

Smithy consulted: DeleteDBProxy errors [DBProxyNotFoundFault 404, InvalidDBProxyStateFault 400]; DescribeDBProxies errors [DBProxyNotFoundFault 404]; shape DBProxy.Status is the DBProxyStatus enum.

## Steps to Reproduce
1. Boot the emulator and point an rds client at it.
2. CreateDBProxy(DBProxyName=<name>, EngineFamily='MYSQL', Auth=[{AuthScheme:'SECRETS', SecretArn:<secret>}], RoleArn=<role>, VpcSubnetIds=[<two subnets>]) and wait for Status 'available'.
3. Call DeleteDBProxy(DBProxyName=<name>) and observe it returns the proxy.
4. Poll DescribeDBProxies(DBProxyName=<name>): the emulator keeps answering HTTP 200 with Status 'available' indefinitely (the fixture polled 61 times over 60s), instead of reporting 'deleting' and then failing with HTTP 404 DBProxyNotFoundFault.
5. Re-replay rds-db-proxy-lifecycle step teardown#18 to confirm.
