Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 145ab011fad305b9f8c024d3a43ce6653fc244ab

# ListClusters refuses an unissued nextToken with the synthetic ValidationException, omitting the modeled required `reason`

_Filed: 2026-10-23T23:29:20Z_
_Operation: ListClusters | Class: error-response_

> _Note: this issue was identified against the code base at commit `145ab011fad305b9f8c024d3a43ce6653fc244ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An unparseable/unissued `next-token` is refused with `crate::scaffolded::Scaffolded::validation(..)`, i.e. the synthetic `smithy.framework#ValidationException` that serializes only `message`.

Expected: HTTP 400 ValidationException whose body carries the modeled members — `message` and the @required `reason` — as every other ValidationException this service returns does (DeleteCluster's deletion-protection refusal, PutClusterPolicy's parse refusal, UntagResource's bad-ARN refusal all fill `reason`).

Actual: body carries `message` only, so a client that branches on `reason` sees it absent for this failure mode. Status (400) and wire code (ValidationException) are correct; only the body shape diverges.

## Evidence

Spec: `ValidationException` declares `message` and `reason` as @required; it is a service-common error available to ListClusters. Handler: `src/ops/list_clusters.rs` lines 162-171 (`Scaffolded::validation`) against the synthetic single-member schema in `src/scaffolded.rs` lines 117-178; contrast `src/state.rs::invalid_resource_arn` (lines 970-983) and `src/ops/delete_cluster.rs` lines 212-223, which fill `reason`. Tests: `tests/all/cluster_lifecycle.rs::an_unissued_next_token_is_refused` asserts the 400 but not the modeled `reason` member.

## Steps to Reproduce

1. GET /cluster?next-token=not-a-cursor
2. Observe 400 with body {"message": "nextToken is not a cursor this service issued: not-a-cursor"}.
3. Expected a modeled ValidationException body including the required `reason` member (and optionally `fieldList` naming nextToken).
