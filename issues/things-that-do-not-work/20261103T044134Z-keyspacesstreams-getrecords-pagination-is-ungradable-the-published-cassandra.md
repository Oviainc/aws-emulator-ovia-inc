Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: keyspacesstreams.get-records-pagination-via-next-shard-iterator
Issue-Shape-Id: com.amazonaws.keyspacesstreams#KeyspacesStreams
Issue-Key: keyspacesstreams.get-records-pagination-via-next-shard-iterator|_cql_connect#11|NoHostAvailable|session = cluster.connect()
Issue-Commit: 02121d07d4d8504fb09a0cb0c72c31d6f51db2d2

# keyspacesstreams GetRecords pagination is ungradable: the published Cassandra CQL data-plane endpoint (cassandra.<region>.amazonaws.com:9142) refuses connections, so no change record is ever produced to page through

> _Note: this issue was identified against the code base at commit `02121d07d4d8504fb09a0cb0c72c31d6f51db2d2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRecords

## Description
One root cause (Cause 1 of 1), grounded in the single surviving failing keyspacesstreams check keyspacesstreams.get-records-pagination-via-next-shard-iterator.

Related open issue: 20261102T015812Z-keyspacesstreams-cdc-records-can-never-be-produced-no.md reports the same missing CQL listener and lists this check id in its trace ids, but the replay suite did NOT mark this check as OWNED by it — its replay key does not match (this check's variant of the CQL-connect helper builds its TLS context with ssl.SSLContext(PROTOCOL_TLS_CLIENT)/load_default_certs and check_hostname=False rather than ssl._create_unverified_context() with a pinned server_hostname, so the key that issue carries for this check no longer binds). The check is therefore unowned and filed here; a fixer closing either issue should re-replay this check id explicitly, since it is this issue's only grounding.

Cause 1 of 1 — the emulator publishes a Keyspaces Cassandra native-protocol (CQL) data-plane endpoint but nothing listens on it, so no row can be written, no CDC change record is ever generated, and GetRecords/nextShardIterator pagination over a backlog cannot be exercised at all.
Failing check: keyspacesstreams.get-records-pagination-via-next-shard-iterator.

AWS: the keyspacesstreams Smithy model (com.amazonaws.keyspacesstreams#KeyspacesStreams) models only the read side — GetRecords (input shardIterator required @length 1-4096, maxResults @range 1-1000; output changeRecords: RecordList, nextShardIterator @length 1-4096, iteratorDescription; errors AccessDeniedException 403, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400, InternalServerException 500) and GetShardIterator (streamArn required @length 37-1024, shardId required @length 28-65, shardIteratorType required enum, optional sequenceNumber @length 21-48). Each served record is shape Record with eventVersion, createdAt, origin (OriginType = USER|REPLICATION|TTL), partitionKeys, clusteringKeys, newImage, oldImage, sequenceNumber. There is deliberately NO modeled write operation: for the write side this is recorded behavior only — Amazon Keyspaces' data plane is the Apache Cassandra native binary protocol over TLS on port 9142 at cassandra.<region>.amazonaws.com, and the recording shows that each accepted row mutation on a CDC-enabled table is appended to the owning stream's shard as one row-level Record with a distinct, monotonically increasing sequenceNumber, that GetRecords never returns more than maxResults records, and that walking nextShardIterator forward collects every written row exactly once, in write order, with no duplicates.

Emulator: everything on the control plane succeeds — CreateKeyspace, CreateTable with a cdcSpecification, cdcStatus reaching ENABLED, GetStream reporting streamStatus ENABLED with its shard list — and then the check's TLS/TCP connect to the endpoint the emulator publishes for cassandra.<region>.amazonaws.com:9142 is refused outright: cassandra.cluster.NoHostAvailable: ('Unable to connect to any servers', {<mapped host:port>: ConnectionRefusedError(111, "Tried connecting to [...]. Last error: Connection refused")}), raised in cassandra.cluster.ControlConnection.connect before a single CQL frame is written. The CQL driver itself is present (the check calls _require_cassandra_driver() first and would have failed with an ImportError instead), so this is the emulator announcing an endpoint it does not answer on, not a missing runner dependency. Because no write can land, the stream stays empty: GetRecords answers 200 with an empty changeRecords list forever and nextShardIterator never advances over any backlog.

Why: a client gets an emulated Keyspaces CDC stream that can never contain anything. A change-data consumer under test cannot distinguish "no changes yet" from "changes are never captured", and the paging contract it depends on is unverifiable: whether maxResults is honored as an upper bound, whether nextShardIterator walks a backlog forward one page at a time, whether each record carries a distinct sequenceNumber, and whether records are delivered without duplication and in write order. A connect refused at the socket also sends users debugging their own client/TLS configuration instead of the emulator.

Fix: the Keyspaces CQL data-plane listener plus this service's change-capture producer. A TLS server must bind the published cassandra.<region>.amazonaws.com:9142 endpoint and speak the Cassandra native binary protocol (OPTIONS/SUPPORTED, STARTUP/AUTHENTICATE, AUTH_RESPONSE/AUTH_SUCCESS, QUERY/PREPARE/EXECUTE/RESULT, ERROR) over the keyspace/table state the keyspaces crate owns; the executor of those CQL writes must, for each accepted row mutation on a CDC-enabled table, append one Record per affected row to the owning stream's shard state in the keyspacesstreams state component, with origin, partitionKeys/clusteringKeys and newImage/oldImage per the table's streamViewType and a distinct monotonically increasing sequenceNumber — so that the existing GetShardIterator/GetRecords handlers (iterator positioning, maxResults capping and nextShardIterator paging) read real data instead of an always-empty shard.

Traces: keyspacesstreams.get-records-pagination-via-next-shard-iterator. The host/port in the replay output is this environment's mapping of cassandra.<region>.amazonaws.com:9142 and is not itself the requirement — the requirement is that the endpoint the emulator publishes for the CQL data plane answers, and that writes made over it surface as keyspacesstreams records that GetRecords pages through.

## Evidence
Check: [error] keyspacesstreams.get-records-pagination-via-next-shard-iterator

Terminal exception (per-run host/port is this environment's endpoint mapping, not part of the requirement):

  File "cluster.py", line 3636, in cassandra.cluster.ControlConnection.connect
  File "cluster.py", line 3681, in cassandra.cluster.ControlConnection._reconnect_internal
  cassandra.cluster.NoHostAvailable: ('Unable to connect to any servers', {'127.0.0.1:9142': ConnectionRefusedError(111, "Tried connecting to [('127.0.0.1', 9142)]. Last error: Connection refused")})

Expected: the published CQL endpoint accepts the TLS connection; three INSERTs into one partition (ck=c1, c2, c3) executed over it are captured by the CDC-enabled table's stream, and repeated GetRecords(shardIterator, maxResults=1) across the stream's shards returns at most 1 changeRecord per call, each with its own distinct sequenceNumber, and following nextShardIterator forward collects clusteringKeys.ck = [c1, c2, c3] in write order with no duplicates.
Actual: the TCP connect to the published CQL port is refused (Errno 111) — zero CQL frames exchanged, no row ever written, so GetRecords answers 200 with an empty changeRecords list and no pagination behavior is exercised. Preceding control-plane steps all succeed: CreateKeyspace, CreateTable with cdcSpecification, cdcStatus ENABLED, GetStream with streamStatus ENABLED and shard list.

The cassandra-driver / cassandra-sigv4 dependencies are installed: the check calls _require_cassandra_driver() before connecting and would have reported an ImportError instead of a connect refusal.

## Steps to Reproduce
1. keyspaces control plane: CreateKeyspace(K); CreateTable(K, T) with allColumns=[pk text, ck text, val text], partitionKeys=[pk], clusteringKeys=[ck] and a cdcSpecification (status ENABLED, viewType NEW_IMAGE); poll GetTable until cdcStatus is ENABLED and note the returned stream ARN.
2. keyspacesstreams GetStream(streamArn) — observe streamStatus ENABLED and its shard list. (Succeeds today.)
3. Resolve the endpoint the emulator publishes for cassandra.<region>.amazonaws.com port 9142 and open a TLS Cassandra native-protocol connection to it, authenticating with the caller's SigV4 identity.
4. Observe: the TCP connect is refused (ConnectionRefusedError, Errno 111) inside cassandra-driver's ControlConnection.connect — no TLS handshake, no CQL frame, so step 5 can never be attempted.
5. Expected instead: the connection is accepted; `INSERT INTO K.T (pk, ck, val) VALUES ('row1','c1','c1')` and the same for c2 and c3 succeed at LOCAL_QUORUM; GetShardIterator(streamArn, shardId, TRIM_HORIZON) per shard, then repeated GetRecords(shardIterator, maxResults=1) returns at most one changeRecord per call, each record carrying a distinct sequenceNumber, and walking nextShardIterator forward yields clusteringKeys.ck = [c1, c2, c3] in write order with no duplicates.
