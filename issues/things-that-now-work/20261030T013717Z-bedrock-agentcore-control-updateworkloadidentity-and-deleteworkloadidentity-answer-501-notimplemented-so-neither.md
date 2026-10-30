Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control.update-workload-identity-nonexistent-returns-resource-not-found bedrock-agentcore-control.delete-workload-identity-nonexistent-returns-resource-not-found
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control.update-workload-identity-nonexistent-returns-resource-not-found|run#7|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Key: bedrock-agentcore-control.delete-workload-identity-nonexistent-returns-resource-not-found|run#7|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Commit: 80128cf5220e52d7fa07858b1a17d4e0aeba15d4

# UpdateWorkloadIdentity and DeleteWorkloadIdentity answer 501 NotImplemented, so neither their ResourceNotFoundException refusal nor their effect on the identity store is reachable (2 causes)

> _Note: this issue was identified against the code base at commit `80128cf5220e52d7fa07858b1a17d4e0aeba15d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateWorkloadIdentity

## Description
Two modeled workload-identity mutations answer 501 NotImplemented, so a call AWS refuses with 404 ResourceNotFoundException instead comes back with error code NotImplemented, and the whole update/delete surface of the identity is unobservable. 2 causes must both be fixed before this issue is closed.

Note: the replay report attributes other checks in this suite to unimplemented scaffold stubs for these same two handlers (services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/update_workload_identity.rs and .../delete_workload_identity.rs) and those stub issues are filed separately; the two checks below were reported as mismatches in their own right and are what this issue is grounded in. The open issue 20261030T002232Z-bedrock-agentcore-control-workload-identity-state-is-not-read-back-updates.md describes an earlier commit where these calls were ACCEPTED and silently discarded; at this commit they are refused outright, which is a different response and is what the checks observe.

Cause 1 of 2: UpdateWorkloadIdentity answers NotImplemented instead of AWS's 404 ResourceNotFoundException for an unknown identity (and never applies an update to a real one).
AWS: Smithy: UpdateWorkloadIdentity, POST /identities/UpdateWorkloadIdentity, @idempotent, input members name -> WorkloadIdentityNameType @required @length 3..255 @pattern ^[A-Za-z0-9_.-]+$ and allowedResourceOauth2ReturnUrls -> ResourceOauth2ReturnUrlListType (list); output members name, workloadIdentityArn, allowedResourceOauth2ReturnUrls, createdTime, lastUpdatedTime; modeled errors include ResourceNotFoundException (errorKind client, httpError 404). For a name that was never created, AWS refuses with 404 ResourceNotFoundException and message 'WorkloadIdentity with id <name> not found.' — fixed text 'WorkloadIdentity with id ' and ' not found.', with the single variable part being the name member taken from the request — and creates no phantom identity. For an existing identity the call succeeds and the supplied list fully replaces the stored one, so a following GetWorkloadIdentity returns exactly that list.
Emulator: the call is refused with error code NotImplemented ('operation ... is scaffolded but not implemented in this emulator'), so the check's assertion `code == "ResourceNotFoundException"` sees NotImplemented. Neither the not-found class/message nor the positive update path exists.
Why: a client cannot distinguish a missing identity from a broken service: botocore raises an unmodeled NotImplemented error that no retry/branch written against ResourceNotFoundException catches, and no workload identity can ever be reconfigured, so OAuth2 return-URL allow-lists are frozen at create time.
Fix: the UpdateWorkloadIdentity handler and its state write — resolve `name` against the workload-identity store, raise a workload-identity-worded ResourceNotFoundException (404) on a miss, and on a hit replace the stored allowedResourceOauth2ReturnUrls and bump lastUpdatedTime so GetWorkloadIdentity reads the new list.
Traces: bedrock-agentcore-control.update-workload-identity-nonexistent-returns-resource-not-found.

Cause 2 of 2: DeleteWorkloadIdentity answers NotImplemented instead of AWS's 404 ResourceNotFoundException for an unknown identity (and never deletes a real one).
AWS: Smithy: DeleteWorkloadIdentity, POST /identities/DeleteWorkloadIdentity, http code 204 with an empty output shape, @idempotent in the model but NOT silently successful on a missing resource: input member name -> WorkloadIdentityNameType @required; modeled errors include ResourceNotFoundException (errorKind client, httpError 404). Recorded behavior: deleting a name that does not exist — and re-deleting a name that was already deleted — is refused 404 ResourceNotFoundException with message 'WorkloadIdentity with name <name> not found.' (fixed text 'WorkloadIdentity with name ' and ' not found.'; the variable part is the request's name member — note the delete path says 'with name' where the get/update path says 'with id'). A successful delete answers 204 with an empty body and the identity is then absent from GetWorkloadIdentity.
Emulator: the call is refused with error code NotImplemented, so the check's assertion `code == "ResourceNotFoundException"` sees NotImplemented; the delete effect and the non-idempotent re-delete refusal are both unreachable.
Why: teardown is impossible and unobservable. A client gets an unmodeled error for a case AWS models as 404, cannot free a name for reuse, and any loop that waits for the identity to disappear never terminates.
Fix: the DeleteWorkloadIdentity handler and its state mutation — resolve `name`, refuse a miss with the 404 ResourceNotFoundException wording above, and on a hit remove the entry from the workload-identity store and answer 204 with an empty body.
Traces: bedrock-agentcore-control.delete-workload-identity-nonexistent-returns-resource-not-found.

## Evidence
Both findings are mismatches from the simparity-behavioral-aws-v1 replay against service bedrock-agentcore-control.
Cause 1 — check bedrock-agentcore-control.update-workload-identity-nonexistent-returns-resource-not-found:
  AssertionError: expected ResourceNotFoundException, got NotImplemented
  at `assert code == "ResourceNotFoundException"` after `client.update_workload_identity(name=<never-created name>, allowedResourceOauth2ReturnUrls=["https://example.com/cb"])`.
  Expected: 404 ResourceNotFoundException, message 'WorkloadIdentity with id <name> not found.' Got: error code NotImplemented.
Cause 2 — check bedrock-agentcore-control.delete-workload-identity-nonexistent-returns-resource-not-found:
  AssertionError: expected ResourceNotFoundException, got NotImplemented
  at `assert code == "ResourceNotFoundException"` after `client.delete_workload_identity(name=<never-created name>)`.
  Expected: 404 ResourceNotFoundException, message 'WorkloadIdentity with name <name> not found.' Got: error code NotImplemented. The same check then requires that deleting an existing identity answers success and that a second delete of that now-deleted name is refused again with the same 404 rather than a silent 2xx — neither is reachable while the operation answers NotImplemented.
The identity names in the run are per-run generated values that fill the one variable slot of each message template; the fixed text, the 404 status and the ResourceNotFoundException code are the requirement.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite; the two checks are bedrock-agentcore-control.update-workload-identity-nonexistent-returns-resource-not-found and bedrock-agentcore-control.delete-workload-identity-nonexistent-returns-resource-not-found.
2. Cause 1: call bedrock-agentcore-control UpdateWorkloadIdentity(name=<a name matching ^[A-Za-z0-9_.-]{3,255}$ that was never created>, allowedResourceOauth2ReturnUrls=['https://example.com/cb']). AWS answers 404 ResourceNotFoundException 'WorkloadIdentity with id <name> not found.'; the emulator answers error code NotImplemented. Then CreateWorkloadIdentity a real name, UpdateWorkloadIdentity it with a new allowedResourceOauth2ReturnUrls list and GetWorkloadIdentity: the read must return exactly the updated list.
3. Cause 2: call DeleteWorkloadIdentity(name=<a never-created name>). AWS answers 404 ResourceNotFoundException 'WorkloadIdentity with name <name> not found.'; the emulator answers error code NotImplemented. Then CreateWorkloadIdentity a real name, DeleteWorkloadIdentity it (expect 204, empty body), GetWorkloadIdentity on it (expect 404), and DeleteWorkloadIdentity it a second time (expect 404 ResourceNotFoundException, not a silent success).
