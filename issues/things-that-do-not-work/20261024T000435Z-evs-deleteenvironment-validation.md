Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# DeleteEnvironment deletes an environment that still has hosts

_Filed: 2026-10-24T00:04:35Z_
_Operation: DeleteEnvironment | Class: validation_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The operation doc states: 'Amazon EVS environments will only be enabled for deletion once the hosts are deleted. You can delete hosts using the DeleteEnvironmentHost action.' Expected: a DeleteEnvironment call against an environment that still holds ESX hosts is refused (ValidationException, the operation's modelled client error) and the environment stays CREATED; only after every host is removed does the environment move to DELETING/DELETED. Actual: the handler never inspects `environment.hosts` — it flips the environment (and all ten VLANs) to DELETING and queues the move to DELETED regardless of how many hosts the environment holds. The hosts are silently dropped along with the record, so a caller that forgot to delete hosts first gets a success where AWS refuses.

## Evidence

Spec: DeleteEnvironment documentation (smithy_query doc shape=DeleteEnvironment) — 'environments will only be enabled for deletion once the hosts are deleted'; the operation models ValidationException (400). Handler: src/ops/delete_environment.rs lines 185-203 — the only condition checked is whether the environment is already DELETING/DELETED. Tests: no test in tests/all/ adds a host and then deletes its environment (grep for DeleteEnvironment hits environment_lifecycle.rs, entitlements.rs, connectors.rs, authorization.rs — none creates hosts first), so nothing pins the current behaviour.

## Steps to Reproduce

1. CreateEnvironment and wait for CREATED.
2. CreateEnvironmentHost to add one ESX host; confirm ListEnvironmentHosts reports it.
3. Call DeleteEnvironment for that environmentId.
4. Observe HTTP 200 with environmentState=DELETING instead of a ValidationException stating the hosts must be deleted first.
