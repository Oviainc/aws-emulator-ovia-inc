Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 30fa86cd244b801d1c6064a0a321e2bf32f37967

# DeleteLicenseServerEndpoint never answers its modelled ResourceNotFoundException for an unknown endpoint ARN

_Filed: 2026-10-24T13:52:14Z_
_Operation: DeleteLicenseServerEndpoint | Class: error-response_

> _Note: this issue was identified against the code base at commit `30fa86cd244b801d1c6064a0a321e2bf32f37967`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `DeleteLicenseServerEndpoint` addresses exactly one resource — the license server endpoint named by `LicenseServerEndpointArn` — and the model declares `ResourceNotFoundException` (HTTP 404, "The resource couldn't be found") for it. A well-formed ARN in the caller's own account/Region that names no endpoint is that documented failure mode, so it should be reported as 404 `ResourceNotFoundException`.

Actual: both failure paths in the handler answer `crate::scaffolded::Scaffolded::unauthorized()` — the non-modelled `UnauthorizedException` code with HTTP 400 and a fixed sentence. No code path in this operation can ever construct the modelled `ResourceNotFoundException`, so a client that branches on the documented 404 (or an SDK's `ResourceNotFoundException` error variant) never sees it, and an SDK deserialises an error code that is not in the model at all. The same `unauthorized()` answer is also given when the stored endpoint exists but its `ServerType` differs from the request's, which hides a request/resource mismatch behind an authorization code.

Note this is distinct from the tagging surface's `BadRequestException`/"Invalid input resource arn" wording, which this emulator documents as recorded behaviour for `ResourceArn`; nothing documents an authorization refusal for a license-server endpoint ARN.

## Evidence

Spec: `smithy_query(query="operation", operation="DeleteLicenseServerEndpoint")` lists `ResourceNotFoundException` (`httpError` 404) among its errors, and the operation's only resource reference is `LicenseServerEndpointArn` (`@required`) — there is no other resource the modelled 404 could be about.

Handler: `src/ops/delete_license_server_endpoint.rs:167-180` — `.ok_or_else(crate::scaffolded::Scaffolded::unauthorized)?` for an ARN the store holds no endpoint for (and for a `ServerType` mismatch); `src/scaffolded.rs:188-223` renders that as HTTP 400 with code `UnauthorizedException`.

Test coverage: `tests/all/resource_resolution_refusals.rs:347-373` pins the current 400/`UnauthorizedException` answer for three classes of unknown endpoint ARN, i.e. the divergence is asserted rather than fixed.

## Steps to Reproduce

1. Start the emulator and create nothing.
2. POST /license-server/DeleteLicenseServerEndpoint with `{"LicenseServerEndpointArn":"arn:aws:license-manager-user-subscriptions:us-east-1:<account>:license-server-endpoint/lse-000000009999","ServerType":"RDS_SAL"}`.
3. Observe HTTP 400 with error code `UnauthorizedException` instead of HTTP 404 `ResourceNotFoundException`.
4. Create an endpoint via CreateLicenseServerEndpoint, then delete it with a `ServerType` other than the stored one and observe the same 400 `UnauthorizedException`.
