Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 145ab011fad305b9f8c024d3a43ce6653fc244ab

# ListStreams refuses an unissued nextToken with the synthetic ValidationException, omitting the modeled required `reason`

_Filed: 2026-10-23T23:29:20Z_
_Operation: ListStreams | Class: error-response_

> _Note: this issue was identified against the code base at commit `145ab011fad305b9f8c024d3a43ce6653fc244ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Same divergence as ListClusters: an unissued `next-token` is refused with `crate::scaffolded::Scaffolded::validation(..)`, the synthetic `smithy.framework#ValidationException` that carries only `message`.

Expected: HTTP 400 ValidationException with the modeled body — `message` plus the @required `reason` — matching every other ValidationException this service emits.

Actual: body carries `message` only; the required `reason` member is absent.

## Evidence

Spec: `ValidationException` members `message` (@required) and `reason` (@required); available to ListStreams as a service-common error. Handler: `src/ops/list_streams.rs` lines 165-174 (`Scaffolded::validation`) over the synthetic schema in `src/scaffolded.rs` lines 117-178. Tests: `tests/all/stream_listing.rs::an_unissued_next_token_is_refused` asserts the refusal but not the modeled `reason` member.

## Steps to Reproduce

1. Create a cluster and wait for ACTIVE.
2. GET /stream/{clusterId}?next-token=not-a-cursor
3. Observe 400 with a body containing only `message`. Expected the modeled ValidationException body including `reason`.
