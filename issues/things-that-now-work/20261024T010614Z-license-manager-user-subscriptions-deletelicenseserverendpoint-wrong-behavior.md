Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# Deleted license server endpoint disappears instantly instead of being observable in DELETING and then DELETED

_Filed: 2026-10-24T01:06:14Z_
_Operation: DeleteLicenseServerEndpoint | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteLicenseServerEndpoint removes the record from the store synchronously (`state.remove_license_server_endpoint(&arn)`) and only then stamps `provisioning_status = DELETING` on the detached copy it returns.

Expected: deletion of a license server endpoint is asynchronous, which is why `LicenseServerEndpointProvisioningStatus` models `DELETING`, `DELETED` and `DELETION_FAILED` beside `PROVISIONING`/`PROVISIONED`. After the delete call returns `DELETING`, a client that polls `ListLicenseServerEndpoints` must still see that endpoint with `LicenseServerEndpointProvisioningStatus = DELETING`, and then reach the terminal `DELETED` (the way the emulator already models the `PROVISIONING -> PROVISIONED` transition it owes the clock via `schedule_transition`/`materialise_due`).

Actual: the single delete response is the ONLY place `DELETING` is ever observable; `ListLicenseServerEndpoints` reports nothing at all immediately after the call, and the `DELETED` status is never produced by any operation. A client that waits for the endpoint to reach `DELETED` before re-creating one for the same provider waits forever, and a client that polls the list sees the resource vanish between two reads with no terminal state.

## Evidence

Spec: `LicenseServerEndpointProvisioningStatus` enumerates DELETED, DELETING, DELETION_FAILED, PROVISIONED, PROVISIONING, PROVISIONING_FAILED (model; mirrored in `src/ops/types/shared_types.rs` line 191), so DELETING is an observable intermediate state and DELETED a terminal one. Handler: `src/ops/delete_license_server_endpoint.rs` lines 158-172; state: `src/state.rs::remove_license_server_endpoint` (line 690) drops the record, the order list, the index and the tags in one step, and `materialise_due` (line 510) only ever transitions PROVISIONING -> PROVISIONED, never DELETING -> DELETED. Tests: `tests/all/license_server_endpoints.rs` asserts the create/list round trip and the provisioning transition; `resource_resolution_refusals.rs::license_server_endpoints_round_trip_and_deletion_removes_them` pins the immediate removal, and nothing asserts a DELETING or DELETED endpoint is reported by the list.

## Steps to Reproduce

1. RegisterIdentityProvider, then CreateLicenseServerEndpoint for the returned IdentityProviderArn with LicenseServerSettings.ServerType=RDS_SAL.
2. Poll ListLicenseServerEndpoints until LicenseServerEndpointProvisioningStatus is PROVISIONED.
3. Call DeleteLicenseServerEndpoint with that LicenseServerEndpointArn and ServerType=RDS_SAL — the response carries LicenseServerEndpointProvisioningStatus=DELETING.
4. Immediately call ListLicenseServerEndpoints: the endpoint is absent. Expected: it is still listed with status DELETING, and a later read reports DELETED (or drops it only after the terminal status was observable).
