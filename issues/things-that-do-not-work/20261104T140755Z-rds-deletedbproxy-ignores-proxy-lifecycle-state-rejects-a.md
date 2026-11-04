Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-proxy-lifecycle
Issue-Step: teardown#17
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-proxy-lifecycle
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DeleteDBProxy|2xx->4xx|status
Issue-Key: rds|DeleteDBProxy|4xx->2xx|status
Issue-Commit: c832bd0d619782e17fabc455ea6e3058ac5fd237

# rds DeleteDBProxy ignores proxy lifecycle state: rejects a valid delete with 400 and then accepts a delete of an already-deleted proxy with 200

> _Note: this issue was identified against the code base at commit `c832bd0d619782e17fabc455ea6e3058ac5fd237`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDBProxy

## Description
There are 2 causes to close. Both are DeleteDBProxy wire divergences in the rds emulator and share one fault: the DeleteDBProxy handler's state gate is driven by a proxy lifecycle that never reaches the state AWS is in at that point in the sequence, so the operation refuses the delete that AWS accepts and then accepts the delete that AWS reports as not-found. Neither step is covered by an existing issue: the open issue '20261030T001542Z-rds-deletedbproxy-never-removes-the-proxy-describedbproxies-keeps.md' owns only the DescribeDBProxies read-back at teardown#18, and '20261104T053605Z-rds-db-proxy-targets-never-become-registrable-or.md' owns only DescribeDBProxyTargets; neither carries a DeleteDBProxy replay key.

Cause 1 of 2: DeleteDBProxy on a proxy that AWS has brought up answers HTTP 400 instead of 200 with the DBProxy payload. (fixture rds-db-proxy-lifecycle, step teardown#17)
AWS: Smithy: DeleteDBProxy(DBProxyName @required, @length 1..63, @pattern ^[a-zA-Z](?:-?[a-zA-Z0-9]+)*$) -> DeleteDBProxyResponse{DBProxy}; modeled errors DBProxyNotFoundFault (httpError 404, awsQueryError code DBProxyNotFoundFault) and InvalidDBProxyStateFault (httpError 400, awsQueryError code InvalidDBProxyStateFault, doc 'The requested operation can't be performed while the proxy is in this state.'). Shape DBProxy.Status is the DBProxyStatus enum [available, modifying, incompatible-network, insufficient-resource-limits, creating, deleting, suspended, suspending, reactivating]. A proxy that has finished coming up is 'available', and a DeleteDBProxy naming it is ACCEPTED: HTTP 200 carrying the DBProxy structure with Status moved to 'deleting'. InvalidDBProxyStateFault is reserved for a proxy that is genuinely in a non-deletable state. The recording shows HTTP 200 for this step.
Emulator: the same request answers HTTP 400. The handler's state gate (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/delete_db_proxy.rs) raises InvalidDBProxyStateFault for any status other than available, and at this point in the sequence the emulator's proxy record has not left 'creating' — the create path parks the proxy in 'creating' with a pending transition to 'available' (create_db_proxy.rs sets status DB_PROXY_STATUS_CREATING plus PendingStatus::in_a_moment(DB_PROXY_STATUS_AVAILABLE)) that has not settled by the time the teardown delete arrives. The same stuck 'creating' lifecycle shows up one step earlier in this fixture as DescribeDBProxyTargetGroups reporting Status 'creating' where the recording has 'available'. Hypothesis: the pending-transition clock is only advanced by settle_due_transitions on a subsequent request and its due instant is longer than the replayed gap, so a proxy is never available when the sequence needs it.
Why: a client cannot delete a proxy it legitimately created. Every teardown and every CI fixture that creates a proxy and deletes it fails with a 400 the client is expected to treat as a real state conflict, so cleanup aborts and the proxy, its target group, its subnets, its secret and its IAM role are all leaked. Retry-on-InvalidDBProxyStateFault loops spin without ever converging because nothing in the emulator moves the proxy on.
Fix: the DeleteDBProxy handler's state gate together with the DB-proxy lifecycle in the service state store — the proxy must actually reach 'available' (and then 'deleting' and removal) so a delete of a created proxy is accepted with 200.
Traces: rds-db-proxy-lifecycle#teardown#17.

Cause 2 of 2: DeleteDBProxy for a proxy name AWS has already deleted answers HTTP 200 instead of HTTP 404 DBProxyNotFoundFault. (fixture rds-db-proxy-lifecycle, step teardown#22)
AWS: Smithy: DeleteDBProxy binds DBProxyNotFoundFault with httpError 404 and awsQueryError {code: DBProxyNotFoundFault, httpResponseCode: 404}; its doc is 'The specified proxy name doesn't correspond to a proxy owned by your Amazon Web Services account in the specified Amazon Web Services Region.' Once a proxy has been deleted, a further DeleteDBProxy naming it is a not-found: HTTP 404, error code DBProxyNotFoundFault, message = RDS's fixed proxy-not-found sentence for the DB-proxy resource whose only variable part is the DBProxyName taken from the request (the emulator already owns that template as crate::state::db_proxy_not_found_message, used on its own not-found branch). Nothing else in the message is per-run. The recording shows HTTP 404 for this step.
Emulator: the same request answers HTTP 200 and reports a successful deletion. Because cause 1 left the earlier delete rejected, the name is still live in the store and the handler falls through its not-found branch into the accept path, returning a DBProxy body.
Why: deletion is not idempotent in the direction AWS defines. A client that deletes a proxy twice — the normal shape of a retried or re-run teardown — is told the second delete succeeded, so it believes it deleted a proxy that no longer existed and cannot distinguish 'I removed it' from 'it was already gone'. Code that keys off DBProxyNotFoundFault to confirm teardown completion (or to decide a name is free for re-creation) never sees the 404 and takes the wrong branch, and a 200 with a DBProxy body for a non-existent name is a fabricated resource.
Fix: the DeleteDBProxy handler's not-found path and the DB-proxy state store, so a name that has been deleted answers 404 DBProxyNotFoundFault rather than a second success.
Traces: rds-db-proxy-lifecycle#teardown#22.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, '/__admin/health answered 200 and the data listener accepts'; rds answers — 413 of 977 steps passed). Both clusters came back with stub_path empty and owned_by empty, i.e. net-new.

Cause 1 — rds-db-proxy-lifecycle#teardown#17, operation DeleteDBProxy, verdict mismatch.
  divergence_key: service=rds|DeleteDBProxy|status=200->400|keys=status
  detail: 'status: expected 200, got 400'
  Expected (AWS): HTTP 200, body DeleteDBProxyResponse/DeleteDBProxyResult/DBProxy with Status 'deleting'.
  Actual (emulator): HTTP 400. Per delete_db_proxy.rs the only 400 branch is InvalidDBProxyStateFault, message template 'DB proxy <DBProxyName> is in state <status> and cannot be deleted' — the fixed text is the sentence frame, the variable parts are the DBProxyName from the request and the proxy's status from emulator state. AWS reserves this error for a genuinely non-deletable proxy; here the proxy had come up in the recording.
  Corroborating same-fixture evidence that the proxy never leaves 'creating': rds-db-proxy-lifecycle#task#10 DescribeDBProxyTargetGroups — '$...TargetGroups.member.Status: value_mismatch (expected "available", got "creating")'.

Cause 2 — rds-db-proxy-lifecycle#teardown#22, operation DeleteDBProxy, verdict mismatch.
  divergence_key: service=rds|DeleteDBProxy|status=404->200|keys=status
  detail: 'status: expected 404, got 200'
  Expected (AWS): HTTP 404, error code DBProxyNotFoundFault, message = RDS's fixed DB-proxy-not-found sentence with the requested DBProxyName interpolated as its only variable part.
  Actual (emulator): HTTP 200 with a DBProxy body for a name AWS considers gone.

The proxy name, ARNs, endpoint host, secret ARN, role path, subnet ids, account, region, timestamps and request ids in the recorded payloads are per-run values and are not part of the requirement; the requirements are the statuses (200 then 404), the error code DBProxyNotFoundFault, and the message templates described above.

Smithy consulted (services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json): DeleteDBProxy input {DBProxyName required, length 1..63, pattern ^[a-zA-Z](?:-?[a-zA-Z0-9]+)*$}, output {DBProxy}, errors [DBProxyNotFoundFault 404, InvalidDBProxyStateFault 400]; shapes DBProxyNotFoundFault, InvalidDBProxyStateFault, DBProxyStatus. Code consulted: src/ops/delete_db_proxy.rs lines 143-183, src/ops/create_db_proxy.rs lines 279-334.

## Steps to Reproduce
1. Boot the emulator and point an rds client at it (region us-west-2).
2. CreateDBProxy(DBProxyName=<name>, EngineFamily='MYSQL', Auth=[{AuthScheme:'SECRETS', SecretArn:<secret>}], RoleArn=<role>, VpcSubnetIds=[<two subnets>]) — returns 200 with Status 'creating'.
3. Run the fixture's normal proxy setup/target steps, then call DeleteDBProxy(DBProxyName=<name>).
   Cause 1: the emulator answers HTTP 400 (InvalidDBProxyStateFault, proxy still 'creating') where AWS answers HTTP 200 with the DBProxy and Status 'deleting'.
4. Call DeleteDBProxy(DBProxyName=<name>) again for the same name.
   Cause 2: the emulator answers HTTP 200 with a DBProxy body where AWS answers HTTP 404 with error code DBProxyNotFoundFault and its fixed proxy-not-found message carrying the requested DBProxyName.
5. Re-replay fixture rds-db-proxy-lifecycle steps teardown#17 and teardown#22 to confirm both causes are closed.
