Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# GetDevices accepts more than 10 DeviceIds, which the member documents as its maximum

_Filed: 2026-10-24T05:25:51Z_
_Operation: GetDevices | Class: validation_

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The DeviceIds filter is documented as "One or more device IDs. The maximum is 10." The handler applies no bound on the list length.

Expected: a request carrying 11 or more deviceIds values is refused with the modeled ValidationException (HTTP 400) naming the DeviceIds member, before any listing is computed.

Actual: the request is accepted and a 200 page is returned for an arbitrarily long id list.

## Evidence

Model documentation for the member, mirrored in the handler's own contract header: src/ops/get_devices.rs line 50 — "`DeviceIds` (DeviceIdList) — One or more device IDs. The maximum is 10." — a documented constraint DeviceIdList does not express as @length. Handler: src/ops/get_devices.rs lines 158-202 — `input.device_ids.unwrap_or_default()` is used directly as a filter with no length check. tests/all/device_lifecycle.rs exercises only 1-2 deviceIds, so nothing asserts the cap.

## Steps to Reproduce

1. POST /global-networks to mint a global network id
2. GET /global-networks/{id}/devices?deviceIds=device-00000000000000001&...&deviceIds=device-00000000000000011 (11 distinct ids)
3. Observe HTTP 200 instead of the documented refusal (ValidationException, HTTP 400, naming DeviceIds)
