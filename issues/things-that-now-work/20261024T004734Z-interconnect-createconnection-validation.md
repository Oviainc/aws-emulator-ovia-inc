Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 4ffec8ed9c134267b14eefb2b325da9a0803e3ce

# CreateConnection accepts any attachPoint without checking it is an attach point valid for the Environment

_Filed: 2026-10-24T00:47:34Z_
_Operation: CreateConnection | Class: validation_

> _Note: this issue was identified against the code base at commit `4ffec8ed9c134267b14eefb2b325da9a0803e3ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `CreateConnection$attachPoint` identifies a real attach point the caller has access to and that is valid for the named Environment — the same set `ListAttachPoints` returns ("Lists all Attach Points the caller has access to that are valid for the specified Environment"; `AttachPointType`'s only modelled value is `DirectConnectGateway`, and `AttachPoint$directConnectGateway` is a Direct Connect gateway UUID). A `directConnectGateway` UUID that names no gateway in the caller's account (or an `AttachPoint$arn` that names no existing connection) therefore cannot be attached, and AWS refuses the create with a modelled client/not-found rejection (`InterconnectClientException` / `ResourceNotFoundException`) rather than provisioning a Connection onto a nonexistent resource.

Actual: the handler validates only the environment id, the environment state and the bandwidth against `Environment$bandwidths.available`. `input.attach_point` is copied verbatim onto the stored `Connection` and never resolved against `store.attach_points[environmentId]`, against Direct Connect's gateways, or against the connection store for the `arn` variant. Any syntactically valid union member is accepted, so a caller can mint a Connection on a gateway that does not exist — it succeeds against the emulator and then fails against AWS, and the 'bad attach point' negative path of the service's primary create operation can never be exercised.

Secondary symptom of the same class: `Environment$remoteIdentifierType` documents "the type of identifying information that should be supplied to the `remoteAccount` parameter of a `CreateConnection` call for this specific Environment", yet the handler never consults it — a create on an environment that declares `account`/`email` is accepted with `remoteAccount` entirely absent, and a supplied `remoteAccount` is never checked against the declared identifier type (it is only stashed in `StoredConnection.remote_account`).

## Evidence

Spec (smithy_query): `CreateConnection` input requires `attachPoint` (AttachPoint union: `directConnectGateway` UUID-patterned, or `arn` patterned as an interconnect connection ARN) and models `InterconnectClientException` (400), `InterconnectValidationException` (400) and `ResourceNotFoundException` (404); `ListAttachPoints` is documented as listing the attach points "the caller has access to that are valid for the specified Environment"; `Environment$remoteIdentifierType` documents the `remoteAccount` requirement per environment.

Handler: services/service-com-amazonaws-interconnect-interconnect/src/ops/create_connection.rs — validation covers only environment lookup (lines ~213-220), environment state (~226-236), bandwidth vs `bandwidths.available` (~241-254) and the account quota (~261-276); the connection is then built with `attach_point: input.attach_point.clone()` (~309) with no lookup of the attach point and no use of `store.attach_points`. `input.remote_account` is persisted at ~343 but never validated.

Test coverage: no test asserts a refusal for an unknown/invalid attach point. The suite asserts the opposite (lenient) behaviour — tests/all/connection_lifecycle.rs `dxgw_attach_point(seed)` and tests/all/refusals_and_gateway_attach_points.rs `ANY_GATEWAY_UUID = "12345678-1234-1234-1234-123456789012"` are arbitrary UUIDs that create connections successfully.

Related but not covering: open issue 20261023T171538Z (ListAttachPoints never reflects a caller-created Direct Connect gateway) and 20261023T095130Z Cause 2 both scope their fix to the ListAttachPoints handler and its seed; neither files a CreateConnection input-validation gap, though the first explicitly notes the consequence that "a client that instead takes a seeded descriptor's identifier gets a gateway UUID that does not exist in the account, so what succeeds here fails against AWS".

## Steps to Reproduce

1. Boot the emulator and point an interconnect client at it; pick a creatable environment: env = next(e for e in ic.list_environments()['environments'] if e['state'] != 'unavailable' and e['bandwidths']['available'])
2. Create no Direct Connect gateway at all, then call ic.create_connection(environmentId=env['environmentId'], bandwidth=env['bandwidths']['available'][0], attachPoint={'directConnectGateway': '12345678-1234-1234-1234-123456789012'}) — a UUID that names no gateway in the account and is not in ic.list_attach_points(environmentId=env['environmentId'])['attachPoints']
3. EXPECTED: a modelled client rejection (InterconnectClientException / ResourceNotFoundException) because the attach point is not one valid for the Environment. OBSERVED: HTTP 200 with a fully provisioned Connection in state 'requested' whose attachPoint echoes the nonexistent gateway, readable afterwards via ic.get_connection and ic.list_connections
4. Repeat with the other union member: attachPoint={'arn': 'arn:aws:interconnect:us-east-1:<account>:connection/mcc-00000000'} (an ARN naming no connection) — also accepted
5. On an environment whose record carries remoteIdentifierType (e.g. 'account' or 'email'), call ic.create_connection(...) with remoteAccount omitted entirely, and again with a remoteAccount value of the wrong kind for the declared type — both are accepted with no validation
