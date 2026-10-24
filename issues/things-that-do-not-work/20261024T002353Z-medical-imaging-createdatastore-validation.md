Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: aeaddd52642475574d853bf8e089296dd7f2f86c

# Validate the tags map supplied to CreateDatastore (key pattern and per-resource tag count)

_Filed: 2026-10-24T00:23:53Z_
_Operation: CreateDatastore | Class: validation_

> _Note: this issue was identified against the code base at commit `aeaddd52642475574d853bf8e089296dd7f2f86c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateDatastore` copies the caller's `tags` map into the new data store record verbatim (`tags: input.tags.clone().unwrap_or_default()`), applying none of the rules the tagging path applies:

1. No tag-key pattern/reserved-prefix check. `TagKey` is `@pattern ^(?!aws:)[a-zA-Z+-=._:/]+$`, but that pattern is never enforced (the framework drops a pattern the `regex` crate cannot compile, and `(?!…)` is unsupported). So a data store can be created with `aws:`-prefixed keys or keys containing characters AWS rejects, and `ListTagsForResource` then reports them.
2. No bound on the number of tags. `TagResource` in this same crate refuses past `MAX_TAGS_PER_RESOURCE` (50, the documented AWS per-resource tag limit), but `CreateDatastore` lets one call install an unbounded map — both a divergence from AWS (which refuses with `ValidationException`) and a hole in the emulator's own stated memory bound, since the create path can exceed the cap the tag path defends.

Expected: `CreateDatastore` with an invalid tag key, or with more than 50 tags, is refused with HTTP 400 `ValidationException` and no data store is created.
Actual: HTTP 200, the data store exists, and the illegal tag map is observable through `ListTagsForResource`.

## Evidence

Spec: `smithy_query(query="operation", operation="CreateDatastore")` → `tags` targets `TagMap`; `smithy_query(query="shape", shape="TagMap")` → key `TagKey` pattern `^(?!aws:)[a-zA-Z+-=._:/]+$`. AWS documents a 50-tag-per-resource limit, which this crate itself encodes as `MAX_TAGS_PER_RESOURCE = 50` (`src/state.rs` line 285).
Handler: `src/ops/create_datastore.rs` line 190 — `tags: input.tags.clone().unwrap_or_default()`, with no key or cardinality check anywhere in `handle`.
Contrast: `src/ops/tag_resource.rs` lines 184-197 enforces `MAX_TAGS_PER_RESOURCE`, so the two write paths disagree about what a legal tag map is.
Tests: `tests/all/resource_tagging.rs::a_resources_tag_map_is_bounded` bounds only the `TagResource` path; `tests/all/datastore_lifecycle.rs::a_created_datastore_reads_back_with_every_member_it_was_given` passes only small, legal tag maps. Uncovered.

## Steps to Reproduce

1. POST /datastore with {"clientToken": "t1", "datastoreName": "d1", "tags": {"aws:owner": "reserved"}} and observe HTTP 200 instead of 400 ValidationException
2. GET /datastore/{datastoreId} to read datastoreArn, then GET /tags/{datastoreArn} and observe the reserved key
3. POST /datastore with {"clientToken": "t2", "tags": {... 200 distinct keys ...}} and observe HTTP 200 — while POST /tags/{arn} with the same map is refused with 400 by TagResource
