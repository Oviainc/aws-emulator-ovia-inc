Issue-Source: trace-sweep-divergence
Issue-Trace: opensearchserverless-access-policy-errors
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearchserverless-access-policy-errors opensearchserverless-collection-error-paths opensearchserverless-collection-group-error-paths opensearchserverless-collection-lifecycle opensearchserverless-encryption-security-policy-lifecycle opensearchserverless-network-security-policy-lifecycle
Issue-Shape-Id: com.amazonaws.opensearchserverless#OpenSearchServerless
Issue-Key: aoss|CreateAccessPolicy|4xx->4xx|status
Issue-Key: aoss|GetAccessPolicy|4xx->4xx|status
Issue-Key: aoss|CreateCollection|4xx->4xx|$.message
Issue-Key: aoss|CreateCollection|4xx->4xx|status
Issue-Key: aoss|CreateCollectionGroup|4xx->4xx|$.message
Issue-Key: aoss|CreateCollectionGroup|4xx->4xx|status
Issue-Key: aoss|DeleteCollection|4xx->4xx|$.message
Issue-Key: aoss|DeleteCollection|4xx->4xx|status
Issue-Key: aoss|GetSecurityPolicy|4xx->4xx|status
Issue-Commit: 7f205ee96657d4151f96c4aa7296372f34590a63

# aoss client errors answer the modeled httpError status instead of HTTP 400

> _Note: this issue was identified against the code base at commit `7f205ee96657d4151f96c4aa7296372f34590a63`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAccessPolicy

## Description
This issue bundles SIX distinct causes in the opensearchserverless (wire token `aoss`) emulator, all the same fault seen on six different operations: every modeled client error is serialized with the HTTP status from its @httpError trait, where the aoss endpoint answers HTTP 400. Each cause must be closed before submitting.

Shared background. AWS: aoss is awsJson1_0 (Smithy service OpenSearchServerless, version 2021-11-01, sigv4Name aoss). The on-disk model annotates ConflictException with @httpError 409 and ResourceNotFoundException with @httpError 404, but recorded behavior shows the service answering HTTP 400 for these client errors, with the error identified only by the body's `__type` (com.amazon.aoss#ConflictException / com.amazon.aoss#ResourceNotFoundException). For awsJson1_0 the status is not how a client discriminates the error; the `__type` is. In every cause below the emulator's `__type` is CORRECT and only the status diverges. Where a message is quoted it is an EXAMPLE, not a requirement: no documented rule produces these conflict texts.

Cause 1 of 6: CreateAccessPolicy on a name that already exists.
AWS: HTTP 400, __type com.amazon.aoss#ConflictException (Smithy: CreateAccessPolicy errors = ConflictException[@httpError 409], ServiceQuotaExceededException[402], ValidationException[400], InternalServerException[500]).
Emulator: HTTP 409 with the right __type.
Why: a client or proxy that classifies on status treats a 409 as a retryable/CAS conflict and a 400 as a permanent request fault; SDK retry policies and any status-based gateway in front of the emulator therefore behave differently than against AWS.
Fix: the error serializer / status mapping for this service's awsJson1_0 error channel (crate::scaffolded error channel plus the service's error serializer), not the individual handler.
Traces: opensearchserverless-access-policy-errors#task#2.

Cause 2 of 6: GetAccessPolicy for a policy that does not exist.
AWS: HTTP 400, __type com.amazon.aoss#ResourceNotFoundException (Smithy: GetAccessPolicy errors = ResourceNotFoundException[@httpError 404], ValidationException[400], InternalServerException[500]).
Emulator: HTTP 404.
Why: same as Cause 1 — status-driven clients and caches treat 404 as an absent resource they may negative-cache, which AWS never signals here.
Fix: same status mapping in the error serializer.
Traces: opensearchserverless-access-policy-errors#task#4.

Cause 3 of 6: CreateCollection on a duplicate collection name.
AWS: HTTP 400, __type com.amazon.aoss#ConflictException (Smithy: CreateCollection errors include ConflictException[@httpError 409]). Recorded message example: "A collection with name '<name>' already exists."
Emulator: HTTP 409; its message also differs (example observed: 'Collection with name <name> already exists with id <generated-id>'), which additionally leaks a generated collection id. The message divergence alone is noted as an example — the status is the requirement.
Fix: the error serializer's status mapping; the conflict-message template lives in CreateCollection's handler (services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/create_collection.rs).
Traces: opensearchserverless-collection-error-paths#task#3.

Cause 4 of 6: CreateCollectionGroup on a duplicate group name.
AWS: HTTP 400, __type com.amazon.aoss#ConflictException (Smithy: CreateCollectionGroup errors include ConflictException[@httpError 409]). Recorded message example: "A collection group with name '<name>' already exists."
Emulator: HTTP 409, and the message again embeds a generated id (example: 'Collection group with name <name> already exists with id <generated-id>').
Fix: as Cause 3, with the message template in create_collection_group.rs.
Traces: opensearchserverless-collection-group-error-paths#task#2.

Cause 5 of 6: DeleteCollection on a collection that is not in a deletable state.
AWS: HTTP 400, __type com.amazon.aoss#ConflictException (Smithy: DeleteCollection errors = ConflictException[@httpError 409], ResourceNotFoundException[404], ValidationException[400], InternalServerException[500]; ConflictException is documented as "When deleting a resource, thrown when the resource is not in the ACTIVE, FAILED, or UPDATE_FAILED state"). Recorded message example: "Collection cannot be deleted because it is not in 'ACTIVE', 'FAILED', or 'UPDATE_FAILED' status."
Emulator: HTTP 409, with a message that names the collection id and current status instead (example only).
Fix: the error serializer's status mapping; the refusal branch lives in delete_collection.rs.
Traces: opensearchserverless-collection-lifecycle#task#5.

Cause 6 of 6: GetSecurityPolicy for a policy that does not exist.
AWS: HTTP 400, __type com.amazon.aoss#ResourceNotFoundException (Smithy: GetSecurityPolicy errors = ResourceNotFoundException[@httpError 404], ValidationException[400], InternalServerException[500]).
Emulator: HTTP 404. This cluster covers both the encryption and the network security-policy teardowns.
Fix: same status mapping in the error serializer.
Traces: opensearchserverless-encryption-security-policy-lifecycle#teardown#7 (also opensearchserverless-network-security-policy-lifecycle#teardown#7).

All six are one mechanism, so a single change in the error status mapping should close them; they are listed separately because each has its own recorded fixture step to re-replay.

## Evidence
Cause 1 — opensearchserverless-access-policy-errors#task#2 CreateAccessPolicy: expected status 400, got 409 (detail: "status: expected 400, got 409"; no body keys diverged, so __type/message matched).
Cause 2 — opensearchserverless-access-policy-errors#task#4 GetAccessPolicy: expected status 400, got 404 (body matched).
Cause 3 — opensearchserverless-collection-error-paths#task#3 CreateCollection: expected 400, got 409; $.message expected "A collection with name 'sim2real-probe-opensearchserverless-coll-7641c326' already exists.", got 'Collection with name sim2real-probe-opensearchserverless-coll-7641c326 already exists with id 8sh3tqaaaaaaaaaaaaaaaaaa'.
Cause 4 — opensearchserverless-collection-group-error-paths#task#2 CreateCollectionGroup: expected 400, got 409; $.message expected "A collection group with name 'sim2real-probe-cg-3f73b803' already exists.", got 'Collection group with name sim2real-probe-cg-3f73b803 already exists with id 8sh3tqaaaaaaaaaaaaaaaaaa'.
Cause 5 — opensearchserverless-collection-lifecycle#task#5 DeleteCollection: expected 400, got 409; $.message expected "Collection cannot be deleted because it is not in 'ACTIVE', 'FAILED', or 'UPDATE_FAILED' status.", got 'Collection with ID 8sh3tqaaaaaaaaaaaaaaaaaa is in status CREATING and cannot be deleted; it must be one of [ACTIVE, FAILED, UPDATE_FAILED]'.
Cause 6 — opensearchserverless-encryption-security-policy-lifecycle#teardown#7 and opensearchserverless-network-security-policy-lifecycle#teardown#7 GetSecurityPolicy: expected status 400, got 404 (body matched).

## Steps to Reproduce
Boot the emulator and replay the aoss fixtures (every step below is a recorded fixture step; the status is read off the raw HTTP response, not the SDK exception).
1 (Cause 1): CreateAccessPolicy(type='data', name=<existing>, policy=...) twice -> second call must be HTTP 400 with __type com.amazon.aoss#ConflictException; observe 409. Fixture opensearchserverless-access-policy-errors#task#2.
2 (Cause 2): GetAccessPolicy(type='data', name=<absent>) -> expect HTTP 400 / ResourceNotFoundException; observe 404. Fixture opensearchserverless-access-policy-errors#task#4.
3 (Cause 3): CreateCollection(name=<existing>) -> expect HTTP 400 / ConflictException; observe 409. Fixture opensearchserverless-collection-error-paths#task#3.
4 (Cause 4): CreateCollectionGroup(name=<existing>, standbyReplicas=...) -> expect HTTP 400 / ConflictException; observe 409. Fixture opensearchserverless-collection-group-error-paths#task#2.
5 (Cause 5): CreateCollection then DeleteCollection while status is CREATING -> expect HTTP 400 / ConflictException; observe 409. Fixture opensearchserverless-collection-lifecycle#task#5.
6 (Cause 6): GetSecurityPolicy(type='encryption', name=<deleted>) -> expect HTTP 400 / ResourceNotFoundException; observe 404. Fixtures opensearchserverless-encryption-security-policy-lifecycle#teardown#7 and opensearchserverless-network-security-policy-lifecycle#teardown#7.
