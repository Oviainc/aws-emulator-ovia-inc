Issue-Source: trace-sweep-divergence
Issue-Trace: codeconnections-connection-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeconnections-connection-errors codeconnections-connection-lifecycle codeconnections-host-delete-blocked-by-connection codeconnections-host-lifecycle codeconnections-list-filters-and-errors codeconnections-list-tags-for-resource-not-found codeconnections-repository-link-errors
Issue-Shape-Id: com.amazonaws.codeconnections#CodeConnections_20231201
Issue-Key: codeconnections|GetConnection|4xx->4xx|status
Issue-Key: codeconnections|DeleteConnection|4xx->4xx|status
Issue-Key: codeconnections|CreateConnection|4xx->4xx|status
Issue-Key: codeconnections|DeleteHost|4xx->4xx|status
Issue-Key: codeconnections|GetHost|4xx->4xx|status
Issue-Key: codeconnections|ListConnections|4xx->4xx|status
Issue-Key: codeconnections|ListTagsForResource|4xx->4xx|status
Issue-Key: codeconnections|GetRepositoryLink|4xx->4xx|status
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# codeconnections client-error responses use the modeled httpError status instead of HTTP 400

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConnection

## Description
SHARED FAULT (8 causes): the emulator picks the HTTP status of a modeled client error from that error shape's @httpError trait (ResourceNotFoundException -> 404). On the wire CodeConnections is an awsJson1_0 service that answers these client errors with HTTP 400 and carries the error identity ONLY in the body (`__type` plus `Message`). In every cause below the response BODY matched the recording exactly and only the status line diverged, so the error identity each handler raises is correct and the status mapping is the defect. Fix the status selection ONCE for the service, then re-replay all eight bindings.

Cause 1 of 8 — GetConnection on a connection ARN that resolves to nothing (fixture codeconnections-connection-errors, step task#1).
AWS: HTTP 400, body __type ResourceNotFoundException, message is the service's fixed not-found sentence (recorded behavior for the status; Smithy: GetConnection errors = ResourceNotFoundException (@httpError 404 in the model) and ResourceUnavailableException (@httpError 400) — the awsJson1_0 wire status for both is 400).
Emulator: HTTP 404 with the identical body.
Why: a client that branches on the HTTP status (SDK error classification, API-gateway/proxy retry rules, `if status == 400` validation paths) classifies the refusal differently than against AWS; a 404 is also indistinguishable from "no such endpoint/route" for middleboxes.
Fix: the service's awsJson1_0 error-envelope status selection (the status emitted for a modeled client error), not the handler — services/service-com-amazonaws-codeconnections-codeconnections-20231201/src/ops/get_connection.rs already raises the right error shape.
Traces: codeconnections-connection-errors#task#1 (also codeconnections-connection-lifecycle#task#9).

Cause 2 of 8 — DeleteConnection on an unknown connection ARN (fixture codeconnections-connection-errors, step task#2).
AWS: HTTP 400 with body __type ResourceNotFoundException (Smithy: DeleteConnection errors = ResourceNotFoundException).
Emulator: HTTP 404, body matches.
Why: same misclassification of a not-found refusal.
Fix: same error-envelope status selection.
Traces: codeconnections-connection-errors#task#2 (also codeconnections-host-delete-blocked-by-connection#teardown#7).

Cause 3 of 8 — CreateConnection naming a HostArn that resolves to no host (fixture codeconnections-connection-errors, step task#3).
AWS: HTTP 400 with body __type ResourceNotFoundException (Smithy: CreateConnection errors = LimitExceededException, ResourceNotFoundException, ResourceUnavailableException).
Emulator: HTTP 404, body matches.
Why: a create refused for a missing host reads as a routing failure instead of a request refusal.
Fix: same error-envelope status selection.
Traces: codeconnections-connection-errors#task#3.

Cause 4 of 8 — DeleteHost on a host ARN that no longer exists (fixture codeconnections-host-delete-blocked-by-connection, step teardown#8).
AWS: HTTP 400 with body __type ResourceNotFoundException (Smithy: DeleteHost errors = ResourceNotFoundException, ResourceUnavailableException).
Emulator: HTTP 404, body matches.
Why: teardown/idempotency logic that treats 400-class refusals as "already gone" and 404 as a transport problem diverges.
Fix: same error-envelope status selection.
Traces: codeconnections-host-delete-blocked-by-connection#teardown#8.

Cause 5 of 8 — GetHost on a deleted host ARN (fixture codeconnections-host-lifecycle, step teardown#11).
AWS: HTTP 400 with body __type ResourceNotFoundException (Smithy: GetHost errors = ResourceNotFoundException, ResourceUnavailableException).
Emulator: HTTP 404, body matches.
Why: same misclassification on the read path used to poll a host's lifecycle.
Fix: same error-envelope status selection.
Traces: codeconnections-host-lifecycle#teardown#11.

Cause 6 of 8 — ListConnections filtered by a HostArn that resolves to no host (fixture codeconnections-list-filters-and-errors, step task#6).
AWS: HTTP 400 with body __type ResourceNotFoundException (Smithy: ListConnections errors = ResourceNotFoundException).
Emulator: HTTP 404, body matches.
Why: a paginated listing that refuses an unknown filter value is reported with the wrong class, so pagination loops that retry on 404 behave differently.
Fix: same error-envelope status selection.
Traces: codeconnections-list-filters-and-errors#task#6.

Cause 7 of 8 — ListTagsForResource on an ARN that resolves to no resource (fixture codeconnections-list-tags-for-resource-not-found, step task#1).
AWS: HTTP 400 with body __type ResourceNotFoundException (Smithy: ListTagsForResource errors = ResourceNotFoundException).
Emulator: HTTP 404, body matches.
Why: tag-reconciliation clients distinguish "resource gone" (a 400-class modeled refusal) from a 404 transport/routing failure.
Fix: same error-envelope status selection.
Traces: codeconnections-list-tags-for-resource-not-found#task#1.

Cause 8 of 8 — GetRepositoryLink for a repository-link id that does not exist (fixture codeconnections-repository-link-errors, step task#4).
AWS: HTTP 400 with body __type ResourceNotFoundException (Smithy: GetRepositoryLink errors = AccessDeniedException, ConcurrentModificationException, InternalServerException, InvalidInputException, ResourceNotFoundException, ThrottlingException).
Emulator: HTTP 404, body matches.
Why: same misclassification on the repository-link read path.
Fix: same error-envelope status selection.
Traces: codeconnections-repository-link-errors#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator, status line only (bodies matched):
- GetConnection: expected 400, got 404 (codeconnections-connection-errors#task#1; cluster also covers codeconnections-connection-lifecycle#task#9)
- DeleteConnection: expected 400, got 404 (codeconnections-connection-errors#task#2; also codeconnections-host-delete-blocked-by-connection#teardown#7)
- CreateConnection: expected 400, got 404 (codeconnections-connection-errors#task#3)
- DeleteHost: expected 400, got 404 (codeconnections-host-delete-blocked-by-connection#teardown#8)
- GetHost: expected 400, got 404 (codeconnections-host-lifecycle#teardown#11)
- ListConnections: expected 400, got 404 (codeconnections-list-filters-and-errors#task#6)
- ListTagsForResource: expected 400, got 404 (codeconnections-list-tags-for-resource-not-found#task#1)
- GetRepositoryLink: expected 400, got 404 (codeconnections-repository-link-errors#task#4)
In each case the recorded body and the emulator's body agree on the exception type and message; only the HTTP status differs. The same wrong-status mechanism also shows up (bundled under the message issue) on CreateRepositoryLink (expected 400, got 409), GetRepositoryLink (repository-link-lifecycle#task#11), GetRepositorySyncStatus and UpdateSyncBlocker.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture codeconnections-connection-errors step task#1 (GetConnection with a well-formed connection ARN that was never created): the recorded response is HTTP 400 with ResourceNotFoundException in the body; the emulator answers HTTP 404 with the same body.
3. Repeat for the other seven bindings listed in trace_ids — each is the same pattern on a different operation (DeleteConnection, CreateConnection with an unknown HostArn, DeleteHost, GetHost, ListConnections with an unknown HostArnFilter, ListTagsForResource, GetRepositoryLink).
