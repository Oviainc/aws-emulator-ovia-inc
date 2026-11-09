Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: keyspaces.native-cql-binary-protocol-lwt-paging-and-ttl keyspaces.native-cql-ttl-expiry
Issue-Shape-Id: com.amazonaws.keyspaces#KeyspacesService
Issue-Key: keyspaces.native-cql-binary-protocol-lwt-paging-and-ttl|__init__#13|AssertionError|raise AssertionError(
Issue-Key: keyspaces.native-cql-ttl-expiry|__init__#14|AssertionError|raise AssertionError(
Issue-Commit: 19c62db1539bfab1a25075fa0f8ae1a77a215876

# keyspaces: two more data-plane checks (binary-protocol LWT/paging/TTL and TTL expiry) cannot run — nothing listens on the published Cassandra CQL endpoint (port 9142), so TTL, paging and prepared-statement behavior are ungradable

> _Note: this issue was identified against the code base at commit `19c62db1539bfab1a25075fa0f8ae1a77a215876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Query

## Description
One root cause (1 of 1), grounded in two failing checks that are NOT carried by any open issue's replay key. The open issue 20261029T194003Z-keyspaces-serves-no-cassandra-native-protocol-cql-data.md describes the same absent CQL listener but its Issue-Trace-Ids/Issue-Key list only keyspaces.native-cql-round-trip, keyspaces.native-cql-lwt-concurrency and keyspaces.native-cql-unsupported-consistency-level-rejected; the two checks filed here are additional and grade row-level behavior those three do not (server-side TTL expiry, result paging, and prepared-statement result metadata). A fixer closing that issue should re-replay these two as well.

AWS: recorded behavior only — the keyspaces Smithy model (com.amazonaws.keyspaces#KeyspacesService) declares a control plane only (CreateKeyspace/CreateTable/CreateType/Get*/List*/Tag*); there is no modeled AWS-JSON operation that writes or reads a row. Amazon Keyspaces' data plane is the Apache Cassandra native binary protocol over TLS on port 9142 at cassandra.<region>.amazonaws.com. What the recordings show that endpoint doing: it accepts a TLS connection on 9142; answers a protocol-v4 OPTIONS frame with a SUPPORTED frame advertising PROTOCOL_VERSIONS 3/v3,4/v4,5/v5-beta and CQL_VERSION 3.4.4; on STARTUP returns an AUTHENTICATE frame naming org.apache.cassandra.auth.PasswordAuthenticator; completes SASL PLAIN against an IAM service-specific credential issued for ServiceName cassandra.amazonaws.com (AUTH_RESPONSE -> AUTH_SUCCESS); and then serves QUERY/PREPARE/EXECUTE frames against the keyspaces and tables the control plane created — including lightweight transactions (IF NOT EXISTS / IF <cond>), USING TTL so a row disappears once its TTL elapses, and paged result sets driven by a page size and a returned paging state.

Emulator: the emulator publishes that endpoint — the checks resolve cassandra.<region>.amazonaws.com:9142 through the harness endpoint mapping and are handed a local address — but nothing is listening. Every connect fails with ConnectionRefusedError: [Errno 111] Connection refused before a single frame is sent (one check reports it with the marker 'DATAPLANE_UNIMPLEMENTED: nothing accepted a TLS CQL connection on <host>:9142', the other with the same text without the marker). The control-plane steps that precede it succeed in both checks. Announcing an address the service is not reachable on is the emulator's own divergence, not the runner's environment.

Why: a client gets an emulated Keyspaces in which no row can be written or read, so these two checks' subjects are entirely unobservable: server-side TTL (a row written USING TTL must stop being returned after it expires, and must still be returned before it does), result paging (a SELECT over more rows than the page size must return a paging state and the remaining rows on the next page, with no row duplicated or lost), and prepared-statement metadata (PREPARE must return column types that match the table's schema, and EXECUTE must bind by that metadata). Application code under test cannot get past table creation, and a connect refusal at the socket is indistinguishable from a misconfigured endpoint, so a user debugging their own driver is sent the wrong way.

Fix: the service's data-plane listener — a TLS server bound to the CQL port that the emulator publishes for this service, speaking the Cassandra native binary protocol v4 (9-byte framing; OPTIONS/SUPPORTED, STARTUP/AUTHENTICATE, AUTH_RESPONSE/AUTH_SUCCESS SASL PLAIN against IAM service-specific credentials, QUERY/PREPARE/EXECUTE/RESULT, ERROR) and reading and writing the SAME keyspace/table/type state the keyspaces control-plane crate already owns. TTL expiry belongs in that query executor's row store (a per-cell/row expiry honored on read), paging in its RESULT serializer (page size + paging state), and prepared-statement column metadata in its PREPARE handler.

Traces: keyspaces.native-cql-binary-protocol-lwt-paging-and-ttl, keyspaces.native-cql-ttl-expiry.

## Evidence
keyspaces.native-cql-binary-protocol-lwt-paging-and-ttl [mismatch]:
  File "check_keyspaces_native_cql_binary_protocol_lwt_paging_and_ttl.py", line 509, in _connect -> line 206, in __init__
  AssertionError: DATAPLANE_UNIMPLEMENTED: nothing accepted a TLS CQL connection on 127.0.0.1:9142: ConnectionRefusedError: [Errno 111] Connection refused

keyspaces.native-cql-ttl-expiry [mismatch]:
  File "check_keyspaces_native_cql_ttl_expiry.py", line 298, in _connect -> line 112, in __init__
  AssertionError: nothing accepted a TLS CQL connection on 127.0.0.1:9142: ConnectionRefusedError: [Errno 111] Connection refused

Expected: a TLS listener accepts the connection on the CQL port the emulator publishes for this service and, per the recordings, answers a v4 OPTIONS frame with SUPPORTED (PROTOCOL_VERSIONS 3/v3,4/v4,5/v5-beta; CQL_VERSION 3.4.4), demands org.apache.cassandra.auth.PasswordAuthenticator on STARTUP, completes SASL PLAIN against a cassandra.amazonaws.com service-specific credential, and then serves QUERY/PREPARE/EXECUTE against the created table with TTL expiry and paging honored.
Actual: the TCP connect itself is refused; zero protocol frames are exchanged.

The host:port shown (127.0.0.1:9142) is this environment's mapping of cassandra.<region>.amazonaws.com:9142 and is not itself the requirement — the requirement is that the CQL endpoint the emulator publishes answers. All control-plane steps in both checks (IAM user + cassandra.amazonaws.com service-specific credential, CreateKeyspace, CreateTable, table reaching ACTIVE) succeeded first.

## Steps to Reproduce
1. Against the keyspaces control plane: CreateKeyspace(K); CreateTable(K, TB) with a text partition key and a value column; poll GetTable until status ACTIVE.
2. Via IAM: CreateUser(U), PutUserPolicy allowing cassandra:Select/cassandra:Modify, CreateServiceSpecificCredential(UserName=U, ServiceName="cassandra.amazonaws.com"); keep ServiceUserName/ServicePassword.
3. Resolve the endpoint the emulator publishes for cassandra.<region>.amazonaws.com port 9142 and open a TLS TCP connection to it.
4. Observe: the connect is refused (ConnectionRefusedError, Errno 111) — no TLS handshake, no CQL frame.
5. Expected instead: the connection is accepted; OPTIONS -> SUPPORTED with CQL_VERSION 3.4.4; STARTUP -> AUTHENTICATE org.apache.cassandra.auth.PasswordAuthenticator; SASL PLAIN with the step-2 credential -> AUTH_SUCCESS; then (a) INSERT ... USING TTL <n> followed by SELECT returns the row before <n> seconds elapse and returns no row after it expires; (b) a SELECT over more rows than the requested page size returns a paging state and the remaining rows on the next page, with no row duplicated or dropped; (c) PREPARE returns result/bind metadata whose column types match the table schema and EXECUTE binds against it; (d) INSERT ... IF NOT EXISTS applies once and reports not-applied thereafter.

Replay keyspaces.native-cql-binary-protocol-lwt-paging-and-ttl and keyspaces.native-cql-ttl-expiry to confirm the fix (in addition to the three checks named by 20261029T194003Z-keyspaces-serves-no-cassandra-native-protocol-cql-data.md).
