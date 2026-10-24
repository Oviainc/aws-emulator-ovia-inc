Issue-Severity: HIGH
Issue-Gap-Class: parameter-ignored
Issue-Commit: 2cb6c27e7920aa8b3ecbca8018912df677b4544f

# CreateHarvestJob ignores ClientToken, so a retried create is not replayed (it duplicates the job or conflicts)

_Filed: 2026-10-24T05:11:59Z_
_Operation: CreateHarvestJob | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `2cb6c27e7920aa8b3ecbca8018912df677b4544f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateHarvestJob` is `@idempotent` and binds `ClientToken` with `@idempotencyToken` on the `x-amzn-client-token` header ("A unique, case-sensitive token that you provide to ensure the idempotency of the request"). AWS therefore treats a repeat under the same token as the SAME request: an unchanged retry replays the original job's response (same `HarvestJobName`, `Arn`, `CreatedAt`, `ETag`), and a retry whose other members differ is refused with `ConflictException` / `ConflictExceptionType = IDEMPOTENT_PARAMETER_MISMATCH`.

The emulator drops the member entirely. `input.client_token` is never read in `handle`, and `crate::state::HarvestJobRecord` has no `client_token` / `fingerprint` field at all (unlike `ChannelGroup`, `Channel` and `OriginEndpoint`, which carry both and go through `crate::state::match_client_token`). The result:
- A retry under the same token that omitted `HarvestJobName` mints a SECOND job under a new server-derived name (the name is derived from the endpoint ARN + the current instant + the job count), so one logical create leaves two jobs that both appear in `ListHarvestJobs` and both consume the per-endpoint quota.
- A retry under the same token that named `HarvestJobName` is refused with `ConflictException` / `resource_already_exists()` instead of replaying the first response — an SDK retry of a request whose response was lost now fails.
- A changed request reusing a token is accepted instead of being refused with `IDEMPOTENT_PARAMETER_MISMATCH`.

## Evidence

Spec: Smithy `CreateHarvestJob` carries `@idempotent`; `CreateHarvestJobRequest$ClientToken` targets `IdempotencyToken` with `@idempotencyToken` and `@httpHeader("x-amzn-client-token")`; `ConflictException`/`ConflictExceptionType` enumerates `IDEMPOTENT_PARAMETER_MISMATCH`.

Handler: `src/ops/create_harvest_job.rs` — `handle` (lines 210-392) never mentions `input.client_token`; the record built at line 343 (`crate::state::HarvestJobRecord`) has no token or fingerprint field (`src/state.rs:781-801`). Contrast `src/ops/create_channel_group.rs:228-262`, `src/ops/create_channel.rs:367-403` and `src/ops/create_origin_endpoint.rs:353-395`, which each compute a `request_fingerprint` and call `crate::state::match_client_token`.

Tests: `tests/all/control_plane_validation.rs:87-233` covers idempotency for exactly "the three levels a create exists at" (channel group, channel, origin endpoint); `CreateHarvestJob` — the fourth create binding `ClientToken` — is not covered by it or by any other suite (grep for `x-amzn-client-token` in `tests/` finds only those two suites).

## Steps to Reproduce

1. Create a channel group, channel and origin endpoint with at least one HLS manifest.
2. POST `/channelGroup/{g}/channel/{c}/originEndpoint/{e}/harvestJob` with header `x-amzn-client-token: fixed-token-1`, a valid `ScheduleConfiguration`, `Destination` and `HarvestedManifests`, and NO `HarvestJobName`. Note the returned `HarvestJobName`/`Arn`.
3. Replay the byte-identical request with the same `x-amzn-client-token`. Expected: HTTP 200 replaying the first job (same name, ARN, CreatedAt, ETag). Actual: HTTP 200 with a NEW job name/ARN.
4. GET `/channelGroup/{g}/harvestJob`. Expected: one job. Actual: two.
5. Repeat steps 2-3 with an explicit `HarvestJobName: "job-1"`. Expected: the second call replays the first response. Actual: HTTP 409 ConflictException ("already exists").
6. Send a THIRD request with `x-amzn-client-token: fixed-token-1` but a different `Description`. Expected: HTTP 409 ConflictException with `ConflictExceptionType: IDEMPOTENT_PARAMETER_MISMATCH`. Actual: accepted (or refused for the unrelated name collision).
