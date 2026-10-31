Issue-Source: trace-sweep-divergence
Issue-Trace: acc-workload-identity-update-round-trip
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: acc-workload-identity-update-round-trip bedrock-agentcore-control-workload-identity-error-paths bedrock-agentcore-control-workload-identity-lifecycle
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|GetWorkloadIdentity|2xx->2xx|$.allowedResourceOauth2ReturnUrls[*]
Issue-Key: bedrock-agentcore-control|GetWorkloadIdentity|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|GetWorkloadIdentity|4xx->2xx|status
Issue-Commit: b2388491b42b5ba2f7f64857139a543aed866088

# Workload identity state is not read back: updates ignored, deletes not honored, not-found message invented

> _Note: this issue was identified against the code base at commit `b2388491b42b5ba2f7f64857139a543aed866088`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWorkloadIdentity

## Description
The workload-identity resource family in bedrock-agentcore-control has a read path that is not wired to its own mutations, and a not-found error of the emulator's own invention. GetWorkloadIdentity serves the state the directory had at CreateWorkloadIdentity time: UpdateWorkloadIdentity's new allowedResourceOauth2ReturnUrls are not persisted, DeleteWorkloadIdentity does not remove the entry, and when the entry genuinely is absent the 404 carries a sentence AWS does not use. 3 causes must all be fixed before this issue is closed.

Cause 1 of 3: UpdateWorkloadIdentity does not persist allowedResourceOauth2ReturnUrls, so GetWorkloadIdentity returns the create-time list.
AWS: UpdateWorkloadIdentity takes `name` (required) plus `allowedResourceOauth2ReturnUrls` (Smithy: UpdateWorkloadIdentity input members name -> WorkloadIdentityNameType @required @length 3..255 @pattern ^[A-Za-z0-9_.-]+$, allowedResourceOauth2ReturnUrls -> ResourceOauth2ReturnUrlListType, list), and returns the same member in its output alongside workloadIdentityArn/createdTime/lastUpdatedTime; the operation is @idempotent, i.e. a full replacement of the list rather than an append. GetWorkloadIdentity (Smithy: output members name, workloadIdentityArn, allowedResourceOauth2ReturnUrls, createdTime, lastUpdatedTime) is @readonly and must report the state left by the last successful update. Recorded behavior: after an update replaces the return-URL list, the subsequent read returns exactly the updated list, in the order supplied by the update request, and nothing from the create call.
Emulator: the read after the update returns the list captured at create time. In the recording $.allowedResourceOauth2ReturnUrls[0] came back as the create-time URL instead of the first updated URL, and $.allowedResourceOauth2ReturnUrls[1] was absent entirely (expected two entries, got one) — the update was accepted and then discarded.
Why: a client that reconfigures the OAuth2 return URLs of a workload identity and then reads the identity back to confirm sees the old allow-list. Any flow that validates a redirect URI against the identity's allow-list keeps honoring revoked URLs and rejecting newly added ones, so the reconfiguration silently never takes effect.
Fix: the UpdateWorkloadIdentity handler's state write (and the workload-identity store it writes into) — the updated list must replace the stored one, and GetWorkloadIdentity must read the stored value rather than a create-time snapshot.
Traces: acc-workload-identity-update-round-trip#task#3.

Cause 2 of 3: GetWorkloadIdentity's not-found message is the emulator's gateway-style sentence instead of AWS's workload-identity wording.
AWS: GetWorkloadIdentity models ResourceNotFoundException (Smithy: GetWorkloadIdentity errors include ResourceNotFoundException, errorKind client, httpError 404). Recorded behavior: the 404 body message is "WorkloadIdentity with id <name> not found." Fixed text: "WorkloadIdentity with id " and " not found."; the single variable part is the identity name taken from the request's `name` member. Status 404 and the ResourceNotFoundException code match already — only the message text diverges.
Emulator: returns 404 with "Failed to retrieve workload identity because it doesn't exist. Retry the request with a different resource identifier." — the gateway family's sentence template reused for workload identities, which neither names the resource type AWS names nor echoes the requested identifier.
Why: tooling that keys on the documented sentence, or parses the identity name out of the message, sees an unrecognised error text and cannot tell which identity was missing. Note the open issue 20261029T170911Z-bedrock-agentcore-control-not-found-and-invalid-state-error-messages-are.md covers the gateway, agent-runtime, harness and memory not-found messages; it does not cover GetWorkloadIdentity, which is this cause.
Fix: the GetWorkloadIdentity handler's not-found error construction — it must build a workload-identity-specific ResourceNotFoundException message rather than delegating to the shared gateway not-found constructor.
Traces: bedrock-agentcore-control-workload-identity-error-paths#task#1 (same cluster: bedrock-agentcore-control-workload-identity-lifecycle#setup#1).

Cause 3 of 3: GetWorkloadIdentity still answers 200 for an identity that was deleted.
AWS: DeleteWorkloadIdentity takes `name` and succeeds with HTTP 204 and an empty body (Smithy: DeleteWorkloadIdentity http code 204, output {}), and it is @idempotent. After it succeeds the identity no longer exists, so GetWorkloadIdentity on that name must fail with ResourceNotFoundException (httpError 404). Recorded behavior: the post-delete read returns 404.
Emulator: the post-delete read returns HTTP 200 with the deleted identity's body — the delete removed nothing from the workload-identity store, so the resource is resurrected on every subsequent read.
Why: this is the most severe of the three. A client cannot observe deletion at all: teardown appears to succeed but the identity remains resolvable, name-reuse checks still see the old identity, and a caller polling for the resource to disappear never terminates. Combined with cause 1 it means the store is effectively write-once at create time.
Fix: the DeleteWorkloadIdentity handler's state mutation — it must remove the entry from the workload-identity store so the GetWorkloadIdentity read path misses and raises ResourceNotFoundException.
Traces: bedrock-agentcore-control-workload-identity-lifecycle#teardown#7.

## Evidence
All three causes are mismatch clusters from the simparity-wire-aws-v1 replay against service bedrock-agentcore-control.
Cause 1 — acc-workload-identity-update-round-trip#task#3, GetWorkloadIdentity, status 200 vs 200 (status matched; body diverged): $.allowedResourceOauth2ReturnUrls[0] expected the first post-update URL, got the create-time URL; $.allowedResourceOauth2ReturnUrls[1] expected the second post-update URL, got None (list truncated to the create-time single entry). The specific URLs in the recording are that run's values filling the list; the requirement is that the read reflects the update request's list, in order and in full.
Cause 2 — bedrock-agentcore-control-workload-identity-error-paths#task#1, GetWorkloadIdentity, status 404 vs 404 (status and error shape matched; only $.message diverged): expected 'WorkloadIdentity with id <name> not found.' (recorded run's name: sim2real-probe-bedrock-agentcore-control-missing-workload-eac7f677), got "Failed to retrieve workload identity because it doesn't exist. Retry the request with a different resource identifier." The same cluster also covers bedrock-agentcore-control-workload-identity-lifecycle#setup#1. The identity name shown is the recorded run's value in the template's one variable slot; the fixed text is the requirement.
Cause 3 — bedrock-agentcore-control-workload-identity-lifecycle#teardown#7, GetWorkloadIdentity, status: expected 404, got 200 — a read issued after DeleteWorkloadIdentity succeeded still served the identity.

## Steps to Reproduce
1. Replay the wire corpus against the emulator (replay_wire_corpus).
2. Cause 1: fixture acc-workload-identity-update-round-trip, step task#3 — CreateWorkloadIdentity with one allowedResourceOauth2ReturnUrls entry, call UpdateWorkloadIdentity on the same name with a different two-entry list, then GetWorkloadIdentity and compare $.allowedResourceOauth2ReturnUrls against the update request.
3. Cause 2: fixture bedrock-agentcore-control-workload-identity-error-paths, step task#1 — GetWorkloadIdentity with a `name` that was never created; read $.message of the 404. Also reproducible at bedrock-agentcore-control-workload-identity-lifecycle#setup#1.
4. Cause 3: fixture bedrock-agentcore-control-workload-identity-lifecycle, step teardown#7 — CreateWorkloadIdentity, DeleteWorkloadIdentity on that name (expect 204), then GetWorkloadIdentity on the same name and observe the HTTP status: AWS answers 404 ResourceNotFoundException, the emulator answers 200 with the deleted body.
