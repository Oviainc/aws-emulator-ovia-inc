Issue-Source: trace-sweep-divergence
Issue-Trace: codestar-connections-connection-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codestar-connections-connection-error-paths codestar-connections-connection-lifecycle-and-tags codestar-connections-host-connection-dependency-and-filtering codestar-connections-repository-link-kms-encryption-key-round-trip codestar-connections-repository-link-lifecycle codestar-connections-repository-link-error-paths codestar-connections-sync-status-and-blocker-error-paths
Issue-Shape-Id: com.amazonaws.codestarconnections#CodeStar_connections_20191201
Issue-Key: codestar-connections|CreateConnection|4xx->5xx|$.Message
Issue-Key: codestar-connections|CreateConnection|4xx->5xx|$.__type
Issue-Key: codestar-connections|CreateConnection|4xx->5xx|status
Issue-Key: codestar-connections|GetConnection|4xx->5xx|$.Message
Issue-Key: codestar-connections|GetConnection|4xx->5xx|$.__type
Issue-Key: codestar-connections|GetConnection|4xx->5xx|status
Issue-Key: codestar-connections|DeleteConnection|4xx->5xx|$.Message
Issue-Key: codestar-connections|DeleteConnection|4xx->5xx|$.__type
Issue-Key: codestar-connections|DeleteConnection|4xx->5xx|status
Issue-Key: codestar-connections|CreateConnection|2xx->5xx|status
Issue-Key: codestar-connections|ListConnections|2xx->5xx|$.Connections
Issue-Key: codestar-connections|ListConnections|2xx->5xx|status
Issue-Key: codestar-connections|ListConnections|4xx->5xx|$.Message
Issue-Key: codestar-connections|ListConnections|4xx->5xx|$.__type
Issue-Key: codestar-connections|ListConnections|4xx->5xx|status
Issue-Key: codestar-connections|CreateConnection|2xx->5xx|$.ConnectionArn
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# Implemented connection operations answer 501 NotImplemented instead of dispatching to their handlers

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConnection

## Description
Replaying the wire corpus (simparity-wire-aws-v1) against the emulator, the codestar-connections fixtures score 2/86 steps passed. Every connection-level operation answers HTTP 501 with __type `NotImplemented` and body message `operation amazonaws.codestar.connections.CodeStar_connections_20191201.<Op> is not implemented in this emulator`, even though the handler files in this checkout are fully written. The 11 clusters blocked by genuinely-unwritten scaffolds (CreateHost, GetHost, UpdateHost, DeleteHost, ListHosts, GetRepositoryLink, DeleteRepositoryLink, ListRepositoryLinks, GetResourceSyncStatus, GetSyncBlockerSummary, UpdateSyncBlocker) carry their own stub_path and are NOT part of this issue.

This issue bundles 7 root causes for this one service. They share a single suspected mechanism (requests are resolved to a not-implemented response before reaching the implemented handler), but each is a separate wire contract that must be re-replayed and confirmed green: three error paths, two success paths, one filter error path and one list path.

Related open issue: `20261023T110719Z-codestar-connections-every-operation-answers-501-notimplemented-so.md` reports the same dispatch gap from the BEHAVIORAL corpus. It carries only behavioral check ids in Issue-Trace-Ids and no wire replay key, so none of the clusters below are owned by it (the replay reported owned_by = [] for all seven); it does not gate these wire fixtures.

=== Cause 1 of 7: CreateConnection with a HostArn naming no host answers 501 instead of the modeled resource-not-found error ===
AWS: CreateConnection is modeled with input ConnectionName @required @length 1..32, optional ProviderType enum [Bitbucket|GitHub|GitHubEnterpriseServer|GitLab|GitLabSelfManaged], optional HostArn @length 0..256 @pattern ^arn:aws(-[\w]+)*:codestar-connections:.+:[0-9]{12}:host\/.+$, optional Tags @length 0..200; modeled errors LimitExceededException (httpError 429), ResourceNotFoundException (httpError 404), ResourceUnavailableException (httpError 400) — the model declares no NotImplemented. Smithy: CreateConnection.errors = LimitExceededException/ResourceNotFoundException/ResourceUnavailableException. On the wire the recording shows this refusal as HTTP 400 with __type `com.amazonaws.codestar.connections#ResourceNotFoundException` (recorded status takes precedence over the model's defaulted 404 for this protocol) and a Message built from the request, e.g. `No host exists with the ARN: arn:aws:codestar-connections:us-west-2:000000000000:host/00000000-0000-0000-0000-000000000000` — the rule is: name the ARN that was supplied.
Emulator: HTTP 501, __type `NotImplemented`, and no Message member at all. src/ops/create_connection.rs is implemented end to end (mints the id/ARN, enforces MAX_CONNECTIONS, stores PENDING, merges tags), and its 501 text does not match this crate's own scaffold wording in src/scaffolded.rs, so the response is produced ahead of the handler. Hypothesis: the generated routing table resolves the wire target to a not-implemented response rather than to ops::create_connection::CreateConnection::handle.
Why: a client that branches on ResourceNotFoundException to retry with a valid host instead sees 501 — a status SDK retry classifiers treat as a server/unsupported condition, not a caller error — so the error is neither handled nor corrected.
Fix: the service's operation dispatch/mounting for this crate (the generated ServiceDefinition routing consumed by src/service.rs and src/ops/mod.rs); the HostArn-existence refusal belongs in the CreateConnection handler's validation once dispatch lands.
Traces: codestar-connections-connection-error-paths#task#1.

=== Cause 2 of 7: GetConnection on an unknown connection ARN answers 501 instead of the modeled resource-not-found error ===
AWS: Smithy: GetConnection input ConnectionArn @required @length 0..256 @pattern ^arn:aws(-[\w]+)*:.+:.+:[0-9]{12}:.+$; output Connection; errors ResourceNotFoundException (httpError 404), ResourceUnavailableException (httpError 400). Recorded wire shape for an ARN that names nothing: HTTP 400 with __type `com.amazonaws.codestar.connections#ResourceNotFoundException` and a Message reporting that no connection exists with that ARN (recorded example: `No connection exists with the ARN`).
Emulator: HTTP 501 with __type `NotImplemented` and no Message, for an operation whose handler is present.
Why: get-after-delete and probe-before-use flows cannot distinguish "this connection does not exist" from "this API does not exist", so a client cannot recover by creating the connection.
Fix: same dispatch/mounting layer; confirm GetConnection reaches ops::get_connection.
Traces: codestar-connections-connection-error-paths#task#2.

=== Cause 3 of 7: DeleteConnection on an unknown connection ARN answers 501 instead of the modeled resource-not-found error ===
AWS: Smithy: DeleteConnection input ConnectionArn @required; output is empty; errors ResourceNotFoundException (httpError 404). Recorded wire shape: HTTP 400 with __type `com.amazonaws.codestar.connections#ResourceNotFoundException` plus a Message naming the missing connection.
Emulator: HTTP 501 with __type `NotImplemented`.
Why: idempotent-teardown logic that tolerates ResourceNotFoundException on a repeated delete aborts instead, leaving client-side cleanup wedged.
Fix: same dispatch/mounting layer; confirm DeleteConnection reaches ops::delete_connection.
Traces: codestar-connections-connection-error-paths#task#3.

=== Cause 4 of 7: CreateConnection success path answers 501, so nothing can be created ===
AWS: Smithy: CreateConnection output ConnectionArn @required @pattern ^arn:aws(-[\w]+)*:.+:.+:[0-9]{12}:.+$ plus Tags; a valid request returns HTTP 200 with a freshly minted ConnectionArn, and per the modeled docs the connection "is in pending status until the third-party connection handshake is completed from the console".
Emulator: HTTP 200 is never produced — the call answers HTTP 501 / NotImplemented, so no ARN is minted and no connection is persisted. This is the setup step of four separate fixtures (connection-lifecycle-and-tags, host-connection-dependency-and-filtering, repository-link-kms-encryption-key-round-trip, repository-link-lifecycle), so every downstream tag, repository-link and sync step in them is unreachable.
Why: the service is unusable: a client cannot obtain the ConnectionArn that every other operation and every CodePipeline integration takes as input.
Fix: same dispatch/mounting layer; the handler body already returns the 200 shape.
Traces: codestar-connections-connection-lifecycle-and-tags#setup#1 (also codestar-connections-host-connection-dependency-and-filtering#setup#3, codestar-connections-repository-link-kms-encryption-key-round-trip#setup#2, codestar-connections-repository-link-lifecycle#setup#1).

=== Cause 5 of 7: ListConnections answers 501 instead of returning the Connections list ===
AWS: Smithy: ListConnections input ProviderTypeFilter (ProviderType enum), HostArnFilter @pattern ^arn:aws(-[\w]+)*:codestar-connections:.+:[0-9]{12}:host\/.+$, MaxResults @range 0..100, NextToken @length 1..1024; output Connections (ConnectionList) + NextToken; paginated inputToken/outputToken NextToken, pageSize MaxResults. A call after a successful create returns HTTP 200 with one Connection member per stored connection, each carrying ConnectionArn, ConnectionName, ConnectionStatus (PENDING for an API-created connection), OwnerAccountId and ProviderType.
Emulator: HTTP 501 / NotImplemented with no Connections member in the body.
Why: discovery and pagination are impossible; a client enumerating connections gets an unsupported-operation error rather than an empty or populated page.
Fix: same dispatch/mounting layer; confirm ListConnections reaches ops::list_connections.
Traces: codestar-connections-connection-lifecycle-and-tags#task#3 (also codestar-connections-host-connection-dependency-and-filtering#task#5).

=== Cause 6 of 7: ListConnections with a HostArnFilter naming no host answers 501 instead of the modeled resource-not-found error ===
AWS: Smithy: ListConnections.errors = ResourceNotFoundException (httpError 404). Recorded wire shape when HostArnFilter names a host that does not exist: HTTP 400 with __type `com.amazonaws.codestar.connections#ResourceNotFoundException` and a Message naming the supplied host ARN (recorded example: `No host exists with the ARN: arn:aws:codestar-connections:us-west-2:000000000000:host/00000000-0000-0000-0000-000000000000`) — i.e. the filter's referent is validated before any listing is attempted, rather than returning an empty page.
Emulator: HTTP 501 with __type `NotImplemented` and no Message.
Why: a client cannot tell a bad filter argument (fixable) from an unsupported API, and loses the AWS guarantee that an unknown HostArnFilter is an error rather than an empty result set.
Fix: same dispatch/mounting layer, plus the HostArnFilter existence check in the ListConnections handler (src/ops/list_connections.rs) once requests reach it.
Traces: codestar-connections-host-connection-dependency-and-filtering#task#7.

=== Cause 7 of 7: CreateConnection answers 501 on the repository-link fixtures' setup, blocking the repository-link surface ===
AWS: as Cause 4 — HTTP 200 with a required ConnectionArn matching ^arn:aws(-[\w]+)*:.+:.+:[0-9]{12}:.+$. This cluster asserts the ARN is present and well shaped (the fixture marks it <shaped-not-asserted>, so the requirement is the member's presence and pattern, not a literal value).
Emulator: HTTP 501 / NotImplemented, with ConnectionArn absent from the response body.
Why: the repository-link and sync-configuration surfaces take a ConnectionArn as their anchor, so a client cannot reach any of them; this is the distinct wire step that gates the repository-link-error-paths, repository-link-lifecycle and sync-status fixtures.
Fix: same dispatch/mounting layer; verify the minted ARN carries the signed region and the emulator account id as the handler already computes.
Traces: codestar-connections-repository-link-error-paths#setup#1 (also codestar-connections-repository-link-lifecycle#task#8, codestar-connections-sync-status-and-blocker-error-paths#setup#1).

## Evidence
Replay summary (trace_set simparity-wire-aws-v1): emulator_up=true, bootstrap_signal=false, state_reset_failures=0, fixtures_replayed=12, steps_total=86, steps_passed=2, steps_mismatched=30, steps_blocked=54, score_pct=2.3, divergence_count=18. Eleven of the eighteen clusters carry a stub_path and are excluded; the seven below carry stub_path="" and owned_by=[].

Cause 1 — codestar-connections-connection-error-paths#task#1 (CreateConnection): expected HTTP 400, got 501. $.__type: expected 'com.amazonaws.codestar.connections#ResourceNotFoundException', got 'NotImplemented'. $.Message: missing (expected 'No host exists with the ARN: arn:aws:codestar-connections:us-west-2:000000000000:host/00000000-0000-0000-0000-000000000000', got None).

Cause 2 — codestar-connections-connection-error-paths#task#2 (GetConnection): expected HTTP 400, got 501. $.__type: expected 'com.amazonaws.codestar.connections#ResourceNotFoundException', got 'NotImplemented'. $.Message missing (expected 'No connection exists with the ARN').

Cause 3 — codestar-connections-connection-error-paths#task#3 (DeleteConnection): expected HTTP 400, got 501. $.__type: expected 'com.amazonaws.codestar.connections#ResourceNotFoundException', got 'NotImplemented'. $.Message missing (expected 'No connection exists with the ARN').

Cause 4 — codestar-connections-connection-lifecycle-and-tags#setup#1 (CreateConnection): expected HTTP 200, got 501. affected_count=4 (host-connection-dependency-and-filtering#setup#3, repository-link-kms-encryption-key-round-trip#setup#2, repository-link-lifecycle#setup#1).

Cause 5 — codestar-connections-connection-lifecycle-and-tags#task#3 (ListConnections): expected HTTP 200, got 501. $.Connections missing; expected one member shaped [{'ConnectionArn': '{{connection_arn}}', 'ConnectionName': ..., 'ConnectionStatus': 'PENDING', 'OwnerAccountId': '000000000000', 'ProviderType': 'GitHub'}], got None. affected_count=2 (host-connection-dependency-and-filtering#task#5).

Cause 6 — codestar-connections-host-connection-dependency-and-filtering#task#7 (ListConnections with HostArnFilter): expected HTTP 400, got 501. $.__type: expected 'com.amazonaws.codestar.connections#ResourceNotFoundException', got 'NotImplemented'. $.Message missing.

Cause 7 — codestar-connections-repository-link-error-paths#setup#1 (CreateConnection): expected HTTP 200, got 501. $.ConnectionArn missing (expected a shaped ARN, got None). affected_count=3 (repository-link-lifecycle#task#8, sync-status-and-blocker-error-paths#setup#1).

Source evidence that these are dispatch failures and not unwritten handlers: services/service-com-amazonaws-codestarconnections-codestar-connections-20191201/src/ops/create_connection.rs lines 154-219 contain a complete handler (state lookup, MAX_CONNECTIONS -> LimitExceededException, id/ARN minting, CONNECTION_STATUS_PENDING, tag merge, 200 output), and the module header at line 4 states "IMPLEMENTED". The observed 501 message ("... is not implemented in this emulator", naming the wire target amazonaws.codestar.connections.CodeStar_connections_20191201.<Op>) is not this crate's scaffold wording, so it is emitted before the handler runs.

## Steps to Reproduce
1. Clone this checkout, build and boot the emulator, and confirm /__admin/health answers 200 and the data listener accepts.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus). Observe score_pct 2.3 with 30 mismatched and 54 blocked steps.
3. Cause 1: replay fixture codestar-connections-connection-error-paths, step task#1 — CreateConnection with a HostArn for a nonexistent host. Observe HTTP 501 / __type NotImplemented instead of HTTP 400 / com.amazonaws.codestar.connections#ResourceNotFoundException with a Message naming the supplied host ARN.
4. Cause 2: same fixture, step task#2 — GetConnection on an unknown ConnectionArn. Expect HTTP 400 / ResourceNotFoundException; observe 501 / NotImplemented.
5. Cause 3: same fixture, step task#3 — DeleteConnection on an unknown ConnectionArn. Expect HTTP 400 / ResourceNotFoundException; observe 501 / NotImplemented.
6. Cause 4: replay codestar-connections-connection-lifecycle-and-tags, step setup#1 — CreateConnection(ConnectionName=..., ProviderType=GitHub). Expect HTTP 200 with a ConnectionArn; observe 501 / NotImplemented.
7. Cause 5: same fixture, step task#3 — ListConnections. Expect HTTP 200 with a Connections member carrying ConnectionStatus PENDING; observe 501 / NotImplemented with no Connections.
8. Cause 6: replay codestar-connections-host-connection-dependency-and-filtering, step task#7 — ListConnections with HostArnFilter naming no host. Expect HTTP 400 / ResourceNotFoundException; observe 501 / NotImplemented.
9. Cause 7: replay codestar-connections-repository-link-error-paths, step setup#1 — CreateConnection. Expect HTTP 200 with a well-shaped ConnectionArn; observe 501 / NotImplemented.
10. After fixing dispatch, re-replay the representative fixture/step in Issue-Trace/Issue-Step and every fixture#step in trace_ids; all seven must pass before this issue is closed.
