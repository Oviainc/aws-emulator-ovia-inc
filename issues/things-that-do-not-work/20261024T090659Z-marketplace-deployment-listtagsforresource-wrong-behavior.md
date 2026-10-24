Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad

# No x-amzn-RequestId header on any response from this service

_Filed: 2026-10-24T09:06:59Z_
_Operation: ListTagsForResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every response this service emits — the 200 of `ListTagsForResource`, the 200 of `PutDeploymentParameter`, the 204s of `TagResource`/`UntagResource`, and every error envelope (404 `NotFoundException`, 400 `BadRequestException`/`ValidationException`, 403 `ForbiddenException`) — carries no request-id header.

Expected: a real restJson1 AWS response carries `x-amzn-RequestId` (a fresh id per call), which is what SDK logging, support escalation and `ServiceError::request_id()` read. The model itself implies the header is part of the service's surface: the service carries `@cors`, whose restJson1 exposed-header defaults include `x-amzn-requestid`.

Actual: no request-id header is set anywhere. `smithy-server-runtime` only mints one on the restXml pre-dispatch error path (vendor/smithy-server-runtime/src/pre_dispatch.rs lines 344-360), so a restJson1 service gets one only if it sets it itself — and this crate does not. A client calling the emulator sees `request_id() == None` on both success and failure, where against AWS it is always populated.

## Evidence

Model: `smithy_query(query="service_info")` reports protocol `restJson1` and a `@cors` trait on `com.amazonaws.marketplacedeployment#AWSMPDeploymentParametersService` (Smithy's restJson1 CORS defaults expose `x-amzn-requestid`). Well-established AWS behavior: every AWS API response carries `x-amzn-RequestId` (or `x-amz-request-id`).

Code: no request-id handling in this crate — services/service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice/src/service.rs sets only the front-door refusal and the refusal wording; `grep -i amzn-requestid` over the crate finds nothing, while sibling emulators that do emit it declare `const REQUEST_ID_HEADER: &str = "x-amzn-requestid"` in their own src/service.rs (e.g. services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/service.rs:84, services/service-com-amazonaws-ivsrealtime-.../src/service.rs:118), so this is a per-service seam that was simply never wired here.

Tests: no test in tests/all/ inspects response headers other than `x-amzn-errortype` (tests/all/not_found_error_code.rs, tests/all/resource_arn_validation.rs `call_reading_header`), so nothing asserts the current header-less behavior.

## Steps to Reproduce

1. Create a deployment parameter (seed an agreement, then POST /catalogs/AWSMarketplace/products/prod-x/deployment-parameters) and note the returned resourceArn.
2. GET /tags/{percent-encoded resourceArn} and inspect the response headers: observe HTTP 200 with no `x-amzn-RequestId`.
3. GET /tags/{an addressable ARN that was never minted} and inspect the 404 `NotFoundException` response headers: again no `x-amzn-RequestId`, where AWS returns one on every error too.
4. Repeat for POST /tags/{arn} (204), DELETE /tags/{arn}?tagKeys=k (204) and the PutDeploymentParameter 200 — none carries a request id.
