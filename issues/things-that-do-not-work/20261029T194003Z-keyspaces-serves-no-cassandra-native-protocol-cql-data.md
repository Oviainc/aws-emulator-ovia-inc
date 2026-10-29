Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: keyspaces.native-cql-round-trip keyspaces.native-cql-lwt-concurrency keyspaces.native-cql-unsupported-consistency-level-rejected
Issue-Shape-Id: com.amazonaws.keyspaces#KeyspacesService
Issue-Key: keyspaces.native-cql-round-trip|__init__#12|AssertionError|raise AssertionError(
Issue-Key: keyspaces.native-cql-lwt-concurrency|__init__#14|AssertionError|raise AssertionError(
Issue-Key: keyspaces.native-cql-unsupported-consistency-level-rejected|__init__#12|AssertionError|raise AssertionError(
Issue-Commit: baf896d7efe27c876c690dc5952f8e38980dff3b

# keyspaces serves no Cassandra native-protocol (CQL) data plane: nothing listens on the published cassandra.<region>.amazonaws.com:9142 endpoint, so no row can ever be written or read

> _Note: this issue was identified against the code base at commit `baf896d7efe27c876c690dc5952f8e38980dff3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Query

## Description
One root cause (1 of 1), grounded in three failing checks that all die at the same point: the TCP/TLS connect to the CQL endpoint.

AWS: the keyspaces Smithy model declares a control plane only — CreateKeyspace/CreateTable/CreateType/Get*/List*/Tag* and friends. There is deliberately NO AWS-JSON operation that puts or gets a row, so this behavior is recorded behavior only: Amazon Keyspaces' data plane is the Apache Cassandra native binary protocol over TLS on port 9142 at `cassandra.<region>.amazonaws.com`. What the recordings show that endpoint doing: it accepts a TLS connection on 9142; answers a protocol-v4 OPTIONS frame with a SUPPORTED frame advertising PROTOCOL_VERSIONS `3/v3,4/v4,5/v5-beta` and CQL_VERSION `3.4.4`; on STARTUP it returns an AUTHENTICATE frame naming `org.apache.cassandra.auth.PasswordAuthenticator`; it authenticates SASL PLAIN against an IAM service-specific credential issued for ServiceName `cassandra.amazonaws.com` (AUTH_RESPONSE -> AUTH_SUCCESS, with protocol error 0x0100 Bad credentials while the credential is still propagating); and it then serves QUERY/PREPARE/EXECUTE frames against the keyspaces and tables the control plane created, including lightweight transactions (IF NOT EXISTS / IF <cond>) and refusal of unsupported consistency levels with an ERROR frame (0x2200 Invalid).

Emulator: the target published that endpoint — the checks resolve `cassandra.<region>.amazonaws.com:9142` through the harness's endpoint mapping and are handed 127.0.0.1:9142 — but nothing is listening there. Every connect fails with `ConnectionRefusedError: [Errno 111] Connection refused`, before a single frame is sent. The control plane that precedes it works in all three checks: the IAM user, the `cassandra.amazonaws.com` service-specific credential, the keyspace and the table are all created and the table reaches ACTIVE; only the data-plane socket is absent.

Why: a client gets an emulated Keyspaces in which no data can ever be stored or retrieved. Every row-level behavior is unobservable — round-trip of an insert/select, prepared-statement result metadata, TTL, paging, lightweight-transaction serialization under concurrency, and the service's refusal of consistency levels it does not support. Application code under test cannot reach past table creation, and a connection-refused at the socket is indistinguishable from a misconfigured endpoint, so a user debugging their own client is sent the wrong way. It also means the service announces a reachable endpoint it does not answer on, which is the emulator's own divergence rather than the runner's environment.

Fix: a new data-plane listener for this service — a TLS server on the CQL port that speaks the Cassandra native binary protocol v4 (9-byte framing; OPTIONS/SUPPORTED, STARTUP/AUTHENTICATE, AUTH_RESPONSE/AUTH_SUCCESS SASL PLAIN against IAM service-specific credentials, QUERY/PREPARE/EXECUTE/RESULT, ERROR) and reads and writes the SAME keyspace/table/type state the control-plane crate already owns, so a table created by CreateTable is queryable and a dropped keyspace stops being. Consistency-level handling and lightweight transactions belong in that query executor. Until the listener exists and is bound to the published endpoint, none of the three checks can get past connect, so the row-level fidelity questions they ask cannot be graded.

Traces: keyspaces.native-cql-round-trip and keyspaces.native-cql-unsupported-consistency-level-rejected both abort with the marker `DATAPLANE_UNIMPLEMENTED: nothing accepted a TLS CQL connection on 127.0.0.1:9142`; keyspaces.native-cql-lwt-concurrency aborts with the same connect failure (its 8 parallel connections all refused). The endpoint is resolved by the checks via `ctx.endpoint_host(f"cassandra.{ctx.region}.amazonaws.com", 9142)`.

## Evidence
keyspaces.native-cql-round-trip:
  File "check_keyspaces_native_cql_round_trip.py", line 370, in _connect -> line 191, in __init__
  AssertionError: DATAPLANE_UNIMPLEMENTED: nothing accepted a TLS CQL connection on 127.0.0.1:9142: ConnectionRefusedError: [Errno 111] Connection refused

keyspaces.native-cql-unsupported-consistency-level-rejected:
  File "check_keyspaces_native_cql_unsupported_consistency_level_rejected.py", line 434, in _connect -> line 210, in __init__
  AssertionError: DATAPLANE_UNIMPLEMENTED: nothing accepted a TLS CQL connection on 127.0.0.1:9142: ConnectionRefusedError: [Errno 111] Connection refused

keyspaces.native-cql-lwt-concurrency:
  File "check_keyspaces_native_cql_lwt_concurrency.py", line 317, in _connect -> line 124, in __init__
  AssertionError: nothing accepted a TLS CQL connection on 127.0.0.1:9142: ConnectionRefusedError: [Errno 111] Connection refused

Expected: a TLS listener accepts the connection on the published CQL port and, per the recordings, answers a v4 OPTIONS frame with SUPPORTED (PROTOCOL_VERSIONS 3/v3,4/v4,5/v5-beta; CQL_VERSION 3.4.4), demands org.apache.cassandra.auth.PasswordAuthenticator on STARTUP, completes SASL PLAIN against a cassandra.amazonaws.com service-specific credential, and then serves QUERY/PREPARE/EXECUTE against the created table.
Actual: the TCP connect itself is refused; zero protocol frames are exchanged.

The host/port pair shown (127.0.0.1:9142) is this environment's mapping of cassandra.<region>.amazonaws.com:9142 and is not itself the requirement — the requirement is that the endpoint the emulator publishes for the CQL data plane answers. All control-plane steps in these checks (IAM CreateUser, CreateServiceSpecificCredential for cassandra.amazonaws.com, CreateKeyspace, CreateTable, table reaching ACTIVE) succeeded first.

## Steps to Reproduce
1. Against the emulator's keyspaces control plane: CreateKeyspace(K), CreateTable(K, TB, allColumns=[pk text, winner text], partitionKeys=[pk]); poll GetTable until status ACTIVE.
2. Via IAM: CreateUser(U), PutUserPolicy allowing cassandra:Select/cassandra:Modify, CreateServiceSpecificCredential(UserName=U, ServiceName="cassandra.amazonaws.com") and keep ServiceUserName/ServicePassword.
3. Resolve the data-plane endpoint the target publishes for cassandra.<region>.amazonaws.com port 9142 and open a TLS TCP connection to it.
4. Observe: the connect is refused (ConnectionRefusedError, Errno 111) — no TLS handshake, no CQL frame.
5. Expected instead: the connection is accepted; a v4 OPTIONS frame returns SUPPORTED with CQL_VERSION 3.4.4; STARTUP returns AUTHENTICATE org.apache.cassandra.auth.PasswordAuthenticator; SASL PLAIN with the credential from step 2 returns AUTH_SUCCESS; `INSERT ... IF NOT EXISTS` / `SELECT` against K.TB round-trip, and a QUERY at an unsupported consistency level is refused with an ERROR frame (0x2200 Invalid).

Replay keyspaces.native-cql-round-trip, keyspaces.native-cql-lwt-concurrency and keyspaces.native-cql-unsupported-consistency-level-rejected to confirm the fix.
