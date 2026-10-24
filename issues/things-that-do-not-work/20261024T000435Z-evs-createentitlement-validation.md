Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# CreateEntitlement accepts a connectorId that names no connector of the environment

_Filed: 2026-10-24T00:04:35Z_
_Operation: CreateEntitlement | Class: validation_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An entitlement is created 'for virtual machines in an Amazon EVS environment using the provided vCenter Server connector' (CreateEntitlement doc), and the operation models `ResourceNotFoundException` ('A service resource associated with the request could not be found'). Expected: a `connectorId` that is well-formed (`^(cnctr-[a-zA-Z0-9]{10})$`) but names no live connector of that environment — or names a connector that is not a VCENTER connector — is refused with ResourceNotFoundException and no entitlement is created. Actual: the handler resolves only the environment; `connectorId` is persisted verbatim as part of the entitlement key and never looked up, so a request naming a connector id that was never minted answers HTTP 200 with fully-formed `entitlements` records (status CREATING -> CREATED) that ListVmEntitlements then reports back. The emulator therefore tracks Windows Server usage through a connector that does not exist.

## Evidence

Spec: CreateEntitlement doc ('using the provided vCenter Server connector'), `connectorId` doc ('A unique ID for the connector associated with the entitlement'), modelled error ResourceNotFoundException. Handler: src/ops/create_entitlement.rs — the only lookup is `environment_in_region_mut` (lines 212-219); the module doc at lines 16-21 states outright that `connectorId` 'is not checked against a stored connector here' and defers the precondition to this operation's issue. `crate::state::connector_not_found` already exists in src/state.rs:270 and is used by UpdateEnvironmentConnector, so the refusal shape is available. Tests: tests/all/entitlements.rs and tests/all/error_wire_status.rs create entitlements with the literal connector id 'cnctr-0123456789' that no connector create ever minted — i.e. no test asserts the connector must exist.

## Steps to Reproduce

1. CreateEnvironment and wait for CREATED.
2. Without creating any connector, call CreateEntitlement(environmentId=<id>, connectorId="cnctr-0123456789", entitlementType=WINDOWS_SERVER, vmIds=["vm-1"]).
3. Observe HTTP 200 with an entitlement record instead of ResourceNotFoundException.
4. Call ListVmEntitlements with the same connectorId — the phantom entitlement is reported.
