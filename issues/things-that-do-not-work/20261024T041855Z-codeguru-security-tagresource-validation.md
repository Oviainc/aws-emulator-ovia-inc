Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 0ca8c03330eb2e28bf9c83ef6b425c1dfc77ebe9

# Tag keys in the reserved aws: namespace are accepted and stored instead of refused

_Filed: 2026-10-24T04:18:55Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `0ca8c03330eb2e28bf9c83ef6b425c1dfc77ebe9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: the `aws:` tag-key prefix is reserved for AWS system tags across every AWS service — a caller cannot create, edit or delete a tag whose key starts with `aws:`, and an attempt to set one on a resource is refused with a client validation error (HTTP 400 ValidationException here, the only 400 this operation models) leaving the resource's tag map unchanged.

Actual: TagResource (and CreateScan's `tags` input, which writes the same map) merges whatever keys the request carries straight into the scan's tag store after only the model's own `@length` checks. A request with tags {"aws:cloudformation:stack-name":"mine"} answers 204 and the reserved key is then reported verbatim by ListTagsForResource, so a client can mint system tags the real service refuses — and can also delete them again through UntagResource, which AWS does not permit either. The handler's only semantic check on `tags` is the emulator's own 200-tags-per-scan store bound.

## Evidence

Well-established AWS tagging semantics: keys beginning with `aws:` are reserved for AWS use and cannot be created or modified by a caller (AWS tagging best practices; services uniformly refuse such keys with a validation-class error). Smithy model: `TagKey` carries only @length 1..128 and no @pattern, so the boundary validator cannot catch this; ValidationException (400) is modeled for TagResource. Handler: src/ops/tag_resource.rs lines 189-219 counts keys for the store bound and then does `scan.tags.insert(key, value)` for every entry with no key inspection; src/ops/create_scan.rs lines 361-381 writes the same map from CreateScan. Test coverage: tests/all/resource_tagging.rs (`tags_written_through_tag_resource_read_back_and_can_be_removed`, `a_scans_tag_map_is_bounded`) only exercises ordinary keys plus the length bound — no test covers a reserved key.

## Steps to Reproduce

1. POST /uploadUrl {"scanName":"demo"} then POST /scans to create scan demo, noting scanNameArn
2. POST /tags/<scanNameArn> with {"tags":{"aws:cloudformation:stack-name":"mine"}}
3. Observe HTTP 204 instead of a 400 ValidationException
4. GET /tags/<scanNameArn> -> the reserved aws:-prefixed key is reported as one of the scan's tags
5. DELETE /tags/<scanNameArn>?tagKeys=aws:cloudformation:stack-name -> 204, removing a key AWS treats as system-owned
