Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.outbound-connection-lifecycle opensearch.inbound-outbound-connection-accept-reject-lifecycle
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.outbound-connection-lifecycle|run#26|AssertionError|assert second_delete_code is None, (
Issue-Key: opensearch.inbound-outbound-connection-accept-reject-lifecycle|_poll_until#34|Timeout|time.sleep(interval_s)
Issue-Commit: a65e25aa520a046e331716f15afdae8f5023a950

# opensearch cross-cluster connections: a deleted connection is purged from state, so a repeat DeleteOutboundConnection is refused with ResourceNotFoundException instead of an idempotent success (2 causes)

> _Note: this issue was identified against the code base at commit `a65e25aa520a046e331716f15afdae8f5023a950`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteOutboundConnection

## Description
Two net-new root causes in the OpenSearch cross-cluster connection state machine (`CreateOutboundConnection` / `AcceptInboundConnection` / `RejectInboundConnection` / `DeleteInboundConnection` / `DeleteOutboundConnection` and the two Describe views). Both live in the one stored `crate::state::Connection` record and the retention span applied to it. There is an open issue titled "Implement opensearch DeleteOutboundConnection: unimplemented scaffold stub" (20261025T124157Z-opensearch-deleteoutboundconnection-stub.md); the handler is now implemented, so that issue does NOT cover either cause below — these are fidelity defects of the implemented handler and of the state retention it relies on.

Cause 1 of 2 — a second DeleteOutboundConnection on a connection id this account already deleted is refused instead of being a no-op.
AWS: Smithy: DeleteOutboundConnection is `DELETE /2021-01-01/opensearch/cc/outboundConnection/{ConnectionId}`, success 200 with `Connection` (OutboundConnection); modelled errors are DisabledOperationException (HTTP 409) and ResourceNotFoundException (HTTP 409). The model does not say when a just-deleted id stops resolving, so for that case: recorded behavior only — the recording shows a repeat DeleteOutboundConnection with the SAME ConnectionId, after the first delete has fully completed, returning success (no error code at all); the operation is idempotent for an id the caller itself deleted and must not resurrect the connection. ResourceNotFoundException with its modelled 409 remains correct only for an id this account never held.
Emulator: the second call is refused with error code `ResourceNotFoundException` (HTTP 409 per the model) — observed verbatim: "deleting the already-deleted outbound connection <connection-id> a second time raised 'ResourceNotFoundException', expected an idempotent no-op success". The fixed text of that refusal is the handler's own missing-connection message and its variable part is the request's `ConnectionId` (the recorded run's `cc-kxe06eyaaaaa` is one request's value filling that slot, not part of the requirement).
Why: a client's teardown path — and every retry/cleanup wrapper that re-issues a delete after a timeout or a partial failure — sees a hard 409 client error where AWS reports success, so idempotent cleanup code that works against AWS aborts or has to special-case the emulator. Cross-cluster teardown is exactly where delete retries happen.
Fix: `DeleteOutboundConnection`'s handler, services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/delete_outbound_connection.rs (the `settled_connection_mut(...) else { ResourceNotFoundException }` branch at the head of `handle`), together with the record-retention decision in cause 2.
Traces: opensearch.outbound-connection-lifecycle (assertion at check_opensearch_outbound_connection_lifecycle.py line 293).

Cause 2 of 2 — a resolved connection's record is dropped from the store entirely, so its id stops being addressable instead of coming to rest in its terminal status.
AWS: recorded behavior only (the model gives no retention window for a connection record). The recordings behind the two checks below state two things about a resolved connection: a deleted outbound connection either disappears from DescribeOutboundConnections or rests at `DELETED`, while its id still answers DeleteOutboundConnection as a no-op (cause 1); and the INBOUND view of a REJECTED connection is never removable at all — DeleteInboundConnection on it is refused with `ValidationException` ("existing status is REJECTED, unable to make necessary change"), and after DeleteOutboundConnection removes the outbound side the inbound record stays visible at `REJECTED` indefinitely (the recording polled for `DELETED` for 180 s and the record never moved).
Emulator: every connection record is removed from the store once a fixed retention span has elapsed after a delete — `CONNECTION_RETENTION: Duration = Duration::from_secs(5)` (services/.../src/state.rs ~line 2013), after which `settled_connection_mut` returns `None` and the id addresses nothing in any of the connection operations. That is the same purge that produces cause 1's wrong 409, and it also means a REJECTED connection's inbound record vanishes rather than remaining REJECTED.
Emulator (second grounding check): opensearch.inbound-outbound-connection-accept-reject-lifecycle never produces a verdict at all — the replay reports only "Check timed out after 60.0s" with no response diagnosis. Hypothesis: the check's terminal stage is a three-read freeze check asserting the inbound record of a rejected, outbound-deleted connection REMAINS `REJECTED` (5 s apart), which the 5 s purge above cannot satisfy; the surrounding stages poll at 5–20 s intervals, so the run exceeds the harness's 60 s budget before any assertion is scored. Please confirm which stage hangs while fixing, rather than taking the timeout itself as the requirement.
Why: a client cannot distinguish "the connection I deleted is gone" from "this id was never mine", and a rejected connection's audit record — which on AWS is permanent on the destination owner's side — disappears, so inventory/reconciliation code that lists inbound connections to find rejected requests silently sees none.
Fix: the connection record's lifecycle in services/service-com-amazonaws-opensearch-amazonopensearchservice/src/state.rs (`CONNECTION_RETENTION`, `settled_connection_mut`, `connection_successor`) and the handlers that read it — delete_outbound_connection.rs, delete_inbound_connection.rs, reject_inbound_connection.rs and the two Describe*Connections projections — so a resolved connection rests in its terminal status (`DELETED` / `REJECTED`) and its id keeps answering, instead of being dropped from state.
Traces: opensearch.outbound-connection-lifecycle, opensearch.inbound-outbound-connection-accept-reject-lifecycle.

## Evidence
Check opensearch.outbound-connection-lifecycle [mismatch]:
  AssertionError: deleting the already-deleted outbound connection cc-kxe06eyaaaaa a second time raised 'ResourceNotFoundException', expected an idempotent no-op success
  (check_opensearch_outbound_connection_lifecycle.py, line 293; the check's own note records the measured AWS behavior: "DeleteOutboundConnection on an already-deleted connection id is a clean idempotent no-op (no ResourceNotFoundException), so the invariant to hold is 'does not resurrect or error', not 'rejects'")
  Expected: HTTP 200, no error code, the connection not resurrected (a later DescribeOutboundConnections shows it absent or DELETED).
  Actual:   error code ResourceNotFoundException, HTTP 409 (the model's httpError for that shape), message = the handler's missing-connection text with the request's ConnectionId substituted.

Check opensearch.inbound-outbound-connection-accept-reject-lifecycle [mismatch]:
  Error: Check timed out after 60.0s — no response was scored. Its terminal stage asserts "expected the inbound record of a rejected, outbound-deleted connection to remain REJECTED" across three reads 5 s apart.

Emulator source backing both: services/service-com-amazonaws-opensearch-amazonopensearchservice/src/state.rs line 2013 `pub const CONNECTION_RETENTION: Duration = Duration::from_secs(5);` ("bounded, so the slot the record holds against CONNECTION_QUOTA is released") and services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/delete_outbound_connection.rs lines 162-169, where a `settled_connection_mut` miss becomes ResourceNotFoundException.

## Steps to Reproduce
1. Create two public OpenSearch domains that satisfy the cross-cluster prerequisites (m5.large.search, NodeToNodeEncryptionOptions.Enabled=true, EncryptionAtRestOptions.Enabled=true, DomainEndpointOptions.EnforceHTTPS=true, AdvancedSecurityOptions with InternalUserDatabaseEnabled and a MasterUserOptions credential) and wait until each reports Created=true, Processing=false.
2. CreateOutboundConnection(LocalDomainInfo.AWSDomainInformation={OwnerId,DomainName=local,Region}, RemoteDomainInfo=...=remote, ConnectionAlias=<fresh alias>, ConnectionMode="DIRECT"); note ConnectionId.
3. Poll DescribeOutboundConnections with Filters=[{Name:"connection-id",Values:[ConnectionId]}] until the status leaves VALIDATING (PENDING_ACCEPTANCE).
4. DeleteOutboundConnection(ConnectionId) and poll the same filter until the connection is absent or DELETED.
5. Call DeleteOutboundConnection(ConnectionId) a SECOND time. Emulator: 409 ResourceNotFoundException. AWS: 200, no error, and the connection is not resurrected.  → Cause 1.
6. For cause 2: create a second connection on the reversed (local, remote) pair, RejectInboundConnection(ConnectionId), poll DescribeInboundConnections until REJECTED, then DeleteOutboundConnection(ConnectionId). Re-read DescribeInboundConnections for that connection-id three times, 5 s apart. Emulator: the record disappears once the 5 s retention span elapses. AWS: it keeps reporting REJECTED, and DeleteInboundConnection on it is refused with ValidationException ("existing status is REJECTED, unable to make necessary change").
