Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# ListVmEntitlements answers an empty page for a connectorId that names no connector instead of ResourceNotFoundException

_Filed: 2026-10-24T00:04:35Z_
_Operation: ListVmEntitlements | Class: error-response_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`connectorId` is a required input of ListVmEntitlements and the operation models `ResourceNotFoundException` ('A service resource associated with the request could not be found'), which is the refusal for a required resource handle that resolves to nothing. Expected: a well-formed `connectorId` that names no connector of the environment is refused with ResourceNotFoundException naming the connector. Actual: the handler resolves only the environment and then uses `connectorId` purely as a filter over the environment's entitlement map, so an unknown connector id yields HTTP 200 with `entitlements: []` and no nextToken — indistinguishable from a real connector that has no entitlements yet.

## Evidence

Spec: ListVmEntitlements input (`connectorId` required, pattern `^(cnctr-[a-zA-Z0-9]{10})$`) and modelled errors ResourceNotFoundException (404) / ValidationException (400). Handler: src/ops/list_vm_entitlements.rs lines 206-230 — `environment_in_region` is the only lookup; the connector/type pair is applied as `.filter(...)`. `crate::state::connector_not_found` (src/state.rs:270) already exists for exactly this refusal and is used by UpdateEnvironmentConnector. Tests: tests/all/error_wire_status.rs lists entitlements with the never-minted id 'cnctr-0123456789' and only asserts the environment-not-found path; no test asserts the unknown-connector answer.

## Steps to Reproduce

1. CreateEnvironment and wait for CREATED; create no connector.
2. Call ListVmEntitlements(environmentId=<id>, connectorId="cnctr-0123456789", entitlementType=WINDOWS_SERVER).
3. Observe HTTP 200 with an empty entitlements list instead of ResourceNotFoundException for the connector.
