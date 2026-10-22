Issue-Source: trace-sweep-divergence
Issue-Trace: transfer-custom-identity-provider-server-config
Issue-Step: teardown#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transfer-custom-identity-provider-server-config transfer-server-delete-then-operate-errors transfer-server-error-paths transfer-server-lifecycle transfer-server-operations-on-nonexistent-id transfer-server-user-sshkey-lifecycle transfer-user-lifecycle-and-list-users transfer-list-operations-on-nonexistent-server transfer-list-operations-unknown-server-errors transfer-server-delete-error-path transfer-server-delete-error-paths
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer|DescribeServer|4xx->4xx|$.Message
Issue-Key: transfer|DescribeServer|4xx->4xx|$.ResourceType
Issue-Key: transfer|DescribeServer|4xx->4xx|status
Issue-Key: transfer|ListUsers|4xx->4xx|$.Message
Issue-Key: transfer|ListUsers|4xx->4xx|$.ResourceType
Issue-Key: transfer|ListUsers|4xx->4xx|status
Issue-Key: transfer|DeleteServer|4xx->4xx|$.Message
Issue-Key: transfer|DeleteServer|4xx->4xx|$.ResourceType
Issue-Key: transfer|DeleteServer|4xx->4xx|status
Issue-Key: transfer|DescribeUser|4xx->4xx|$.Message
Issue-Key: transfer|DescribeUser|4xx->4xx|$.ResourceType
Issue-Key: transfer|DescribeUser|4xx->4xx|status
Issue-Commit: 872e0ddf6cb614931de58689afe8393c02abd052

# Transfer not-found errors answer HTTP 404 and echo the identifier instead of the recorded 400 generic not-found

> _Note: this issue was identified against the code base at commit `872e0ddf6cb614931de58689afe8393c02abd052`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeServer

## Description
Every transfer operation addressed at a server or user that does not exist (never created, or already deleted) diverges the same way: the emulator answers HTTP 404, lowercases the ResourceType member, and renders the requested identifier into Message, where the recordings show AWS answering HTTP 400 with a capitalized ResourceType and a fixed generic message. One error-mapping defect, 4 divergence clusters covering 4 operations. No open transfer issue covers this.

Shared AWS grounding: Smithy models ResourceNotFoundException with required members Message, Resource and ResourceType and an @httpError(404) — but the recordings show the Transfer wire answering HTTP 400 with __type ResourceNotFoundException (the __type matched on every one of these steps; only the status and the two members diverged), so on the wire the status is 400 and the model's httpError does not describe the service. Recorded behavior also shows ResourceType carrying the capitalized resource name ('Server', 'User') and Message carrying a fixed generic string ('Unknown server', 'Unknown user') that does NOT contain the requested id — the identifier travels in the Resource member instead. That no-echo construction is the requirement here (it holds across both resource types and all four operations); the exact wording beyond it is an example.

Cause 1 of 4 — DescribeServer on an unknown/deleted ServerId.
AWS: HTTP 400, __type ResourceNotFoundException, ResourceType 'Server', Message the generic not-found string with no id in it.
Emulator: HTTP 404, ResourceType 'server', Message with the ServerId appended (e.g. 'Unknown server s-00000000000000001').
Why: clients branch on the HTTP status class and on ResourceType; a 404 where AWS sends 400 breaks retry/error-classification code, and an id-bearing message breaks log scrubbing and message-equality assertions.
Fix: the transfer ResourceNotFoundException serializer/status mapping and the DescribeServer handler's not-found construction.
Traces: transfer-custom-identity-provider-server-config#teardown#6, also transfer-server-delete-then-operate-errors#task#4, transfer-server-error-paths#task#4/#6, transfer-server-lifecycle#teardown#7, transfer-server-operations-on-nonexistent-id#task#1, transfer-server-user-sshkey-lifecycle#teardown#13, transfer-user-lifecycle-and-list-users#teardown#13.

Cause 2 of 4 — ListUsers on an unknown ServerId.
AWS: HTTP 400, __type ResourceNotFoundException, ResourceType 'Server', generic message with no id.
Emulator: HTTP 404, ResourceType 'server', message echoing the ServerId.
Why: list calls against a stale server id are misclassified exactly as in Cause 1, and a paginating client cannot distinguish the error class.
Fix: same error mapping, reached from the ListUsers handler's server lookup.
Traces: transfer-list-operations-on-nonexistent-server#task#2, also transfer-list-operations-unknown-server-errors#task#1.

Cause 3 of 4 — DeleteServer on an unknown/already-deleted ServerId.
AWS: HTTP 400, __type ResourceNotFoundException, ResourceType 'Server', generic message with no id.
Emulator: HTTP 404, ResourceType 'server', message echoing the ServerId.
Why: idempotent-teardown code that tolerates the AWS 400 not-found sees an unexpected 404 and aborts.
Fix: same error mapping, reached from the DeleteServer handler.
Traces: transfer-server-delete-error-path#task#3, also transfer-server-delete-error-paths#task#3, transfer-server-delete-then-operate-errors#task#3, transfer-server-error-paths#task#3/#9, transfer-server-operations-on-nonexistent-id#task#4.

Cause 4 of 4 — DescribeUser for a user that does not exist on an existing server.
AWS: HTTP 400, __type ResourceNotFoundException, ResourceType 'User', generic message with no identifier.
Emulator: HTTP 404, ResourceType 'user', Message echoing both the UserName and the ServerId.
Why: same misclassification for the user resource, and the leaked identifiers make the body non-deterministic for clients asserting on it.
Fix: same error mapping, reached from the DescribeUser handler's user lookup.
Traces: transfer-server-user-sshkey-lifecycle#task#9, also #task#11, transfer-user-lifecycle-and-list-users#teardown#9/#11.

## Evidence
Cause 1: transfer-custom-identity-provider-server-config#teardown#6 DescribeServer — status expected 400, got 404; $.Message expected 'Unknown server', got 'Unknown server s-00000000000000001'; $.ResourceType expected 'Server', got 'server' (8 steps in cluster).
Cause 2: transfer-list-operations-on-nonexistent-server#task#2 ListUsers — status expected 400, got 404; $.Message expected 'Unknown server', got 'Unknown server s-00000000000000000'; $.ResourceType expected 'Server', got 'server' (2 steps).
Cause 3: transfer-server-delete-error-path#task#3 DeleteServer — status expected 400, got 404; $.Message expected 'Unknown server', got 'Unknown server s-00000000000000001'; $.ResourceType expected 'Server', got 'server' (6 steps).
Cause 4: transfer-server-user-sshkey-lifecycle#task#9 DescribeUser — status expected 400, got 404; $.Message expected 'Unknown user', got 'Unknown user sim2realprobenosuchuser on server s-00000000000000001'; $.ResourceType expected 'User', got 'user' (4 steps).
In all clusters __type (ResourceNotFoundException) matched, so only status, ResourceType casing and the message construction diverge.

## Steps to Reproduce
1. Call DescribeServer with a well-formed but nonexistent ServerId (or one that was just deleted) -> emulator answers 404 with ResourceType 'server' and the id inside Message; AWS recordings show 400, ResourceType 'Server', generic message (Cause 1).
2. Call ListUsers with the same nonexistent ServerId (Cause 2) and DeleteServer with it (Cause 3) -> same divergence.
3. CreateServer, then DescribeUser on it with a UserName that was never created -> 404 / ResourceType 'user' / id-bearing message instead of 400 / 'User' / generic message (Cause 4).
Replay: fixtures transfer-custom-identity-provider-server-config, transfer-list-operations-on-nonexistent-server, transfer-server-delete-error-path, transfer-server-user-sshkey-lifecycle.
