Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: agent-registry-control.delete-registry-blocked-while-records-exist
Issue-Shape-Id: com.amazonaws.agentregistrycontrol#AgentRegistryControl
Issue-Key: agent-registry-control.delete-registry-blocked-while-records-exist|_error#12|AssertionError|raise AssertionError(f"expected the delete to be rejected, but it succeeded: {result!r}")
Issue-Commit: 2926e75b84d3dfdb431d7093667b6d3230f0ce6c

# DeleteRegistry deletes a non-empty registry instead of refusing it with ConflictException

> _Note: this issue was identified against the code base at commit `2926e75b84d3dfdb431d7093667b6d3230f0ce6c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteRegistry

## Description
Cause 1 of 1: DeleteRegistry does not enforce the empty-registry precondition, so a registry that still holds registry records is accepted for deletion.

AWS: DeleteRegistry refuses a registry that still contains registry records. The recorded behavior is a modeled `ConflictException` (Smithy: DeleteRegistry declares ConflictException, errorKind client, httpError 409 — "The request conflicts with the current state of the resource") with the message template `Cannot delete registry '<registryArn>' because it contains registry records. Delete all registry records before deleting the registry.` In that message the fixed text is `Cannot delete registry '` / `' because it contains registry records. Delete all registry records before deleting the registry.`; the only variable part is `<registryArn>`, the full ARN of the targeted registry taken from the emulator's state (the same value CreateRegistry returned as `registryArn`) — never a literal ARN from one run. The refusal must leave the registry untouched: a subsequent GetRegistry still reports `READY`. Once every record under the registry is deleted, DeleteRegistry succeeds with HTTP 202 and `status: DELETING` (Smithy: DeleteRegistry http code 202, output member `status` of enum RegistryStatus). Note the model's prose for DeleteRegistry ("removed along with its registry records") describes the asynchronous removal AFTER the delete is accepted; it does not license accepting a delete while records exist — the recorded behavior shows the precondition is checked first.

Emulator: the call succeeds. Replay records HTTP 202 with body `{"status": "DELETING"}` for a registry that holds one DRAFT record, and the check fails with `AssertionError: expected the delete to be rejected, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 202, ...}, 'status': 'DELETING'}`. No ConflictException is ever produced for this condition.

Why: a client relying on AWS's behavior loses the guard that protects populated registries. Code that calls DeleteRegistry expecting a 409 to tell it "records still exist, drain them first" instead sees a 202 and proceeds, and the registry plus all of its records are asynchronously destroyed. Error-handling paths keyed on ConflictException become dead code, and a caller cannot distinguish "deleted an empty registry" from "silently wiped a populated one".

Fix: the DeleteRegistry handler's precondition check / validation. Before transitioning the registry to DELETING it must consult the service state for registry records belonging to this registry and, when any exist, return the modeled ConflictException with the message above and make no mutation to the registry (no status change, no updated_at bump, no armed removal transition).

Traces: agent-registry-control.delete-registry-blocked-while-records-exist.

## Evidence
Check: agent-registry-control.delete-registry-blocked-while-records-exist (verdict: mismatch).

Setup: CreateRegistry -> wait READY; CreateRegistryRecord(recordType=CUSTOM, descriptors={custom:{data:'{}'}}) -> wait DRAFT. Then DeleteRegistry(registryId) on the now non-empty registry.

Expected (AWS): HTTP 409, Error.Code `ConflictException`, Error.Message `Cannot delete registry '<registryArn>' because it contains registry records. Delete all registry records before deleting the registry.` (<registryArn> = the registry's ARN from state). Registry unchanged, GetRegistry still `READY`. After DeleteRegistryRecord, DeleteRegistry returns HTTP 202 / `status: DELETING`.

Actual (emulator): HTTP 202, body `{"status":"DELETING"}`, no error. Harness output:
  AssertionError: expected the delete to be rejected, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 202, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '21', ...}}, 'status': 'DELETING'}

Code: services/service-com-amazonaws-agentregistrycontrol-agentregistrycontrol/src/ops/delete_registry.rs lines 163-192 — the handler looks the registry up (404 if absent) and then unconditionally sets REGISTRY_DELETING and arms the removal transition; there is no inspection of registry records and ConflictException (declared in the generated DeleteRegistryErrorModeled enum at lines 91-98) is never constructed.

## Steps to Reproduce
1. CreateRegistry(name=<unique>) and wait until GetRegistry reports status READY; keep registryArn and derive registryId from its last path segment.
2. CreateRegistryRecord(registryId, name=<unique>, recordType='CUSTOM', descriptors={'custom': {'data': '{}'}}) and wait until GetRegistryRecord reports status DRAFT.
3. Call DeleteRegistry(registryId) while that record still exists.
   - Expected: ClientError with HTTP 409, Code 'ConflictException', Message "Cannot delete registry '<registryArn>' because it contains registry records. Delete all registry records before deleting the registry."
   - Actual: HTTP 202 with {'status': 'DELETING'}.
4. Call GetRegistry(registryId): expected still 'READY'; the emulator has moved it to DELETING and will remove it.
5. Positive control: delete the record first (DeleteRegistryRecord), then DeleteRegistry(registryId) must return HTTP 202 with status 'DELETING' and the registry must eventually 404 on GetRegistry.
