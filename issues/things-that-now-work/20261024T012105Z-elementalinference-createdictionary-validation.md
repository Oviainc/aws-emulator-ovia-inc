Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# Tags supplied at create time are stored unvalidated: reserved aws: keys and an unbounded number of tags are accepted

_Filed: 2026-10-24T01:21:05Z_
_Operation: CreateDictionary | Class: validation_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateDictionaryRequest$tags` (and `CreateFeedRequest$tags`) are stored verbatim: `input.tags.unwrap_or_default().into_iter().collect()`. The modeled `TagMap` only bounds key length (1..128) and value length (..256), which the boundary validator enforces; everything else AWS enforces service-side is missing:

* a tag key beginning with the reserved `aws:` prefix is accepted and reported back by `GetDictionary`/`GetFeed`, where the real service refuses it (AWS-wide rule: `aws:` is reserved for AWS-generated tags and a user request carrying one is rejected);
* there is no cap on the number of tags, so a create carrying hundreds of tags succeeds, where AWS enforces 50 user tags per resource and answers a 400 refusal beyond it.

Expected: HTTP 400 `ValidationException` (modeled for both create operations). Actual: HTTP 202 with the tags persisted and echoed by every later read of the resource.

## Evidence

Spec: `TagMap` = key `TagKey` (len 1..128) / value `TagValue` (len ..256) only — no prefix or cardinality rule, so the handler is the only place the service-side rule could live; `ValidationException` (HTTP 400) is modeled for CreateDictionary and CreateFeed. The `aws:` reserved prefix and the 50-tags-per-resource limit are well-established AWS tagging semantics (identical gaps are tracked for other services in this corpus, e.g. license-manager-user-subscriptions TagResource).
Handler: `src/ops/create_dictionary.rs` (`tags: input.tags.unwrap_or_default().into_iter().collect()`) and `src/ops/create_feed.rs` (same line) — no inspection of keys or of the map size.
Tests: `tests/all/response_members.rs:81-186` only check that an untagged resource reports an empty map and that supplied tags round-trip; no test exercises a reserved key or an over-large tag set.

## Steps to Reproduce

1. POST /v1/dictionary with {"name": "dict1", "language": "eng", "tags": {"aws:cloudformation:stack-name": "mine"}} — observe HTTP 202 instead of HTTP 400 ValidationException, and GET /v1/dictionary/{id} echoing the reserved key
2. POST /v1/dictionary with 60 distinct tag entries — observe HTTP 202 instead of a 400 refusal for exceeding the 50-tag limit
3. Repeat both against POST /v1/feed (`CreateFeedRequest$tags`) for the same result
