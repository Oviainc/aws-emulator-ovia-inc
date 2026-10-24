Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d5cb6525aec300e57e039668d831e20baadf847e

# CreateScan on an existing scan name replaces the scan's whole tag map, silently deleting tags added by TagResource

_Filed: 2026-10-24T02:28:10Z_
_Operation: CreateScan | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d5cb6525aec300e57e039668d831e20baadf847e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: AWS tagging semantics are additive — only UntagResource removes a tag key. Re-running a scan (CreateScan with an existing scanName) with a `tags` map must not delete tag keys the resource already carries that the request does not mention.

Actual: src/ops/create_scan.rs lines 361-363 does `scan.tags = tags.clone().into_iter().collect();` for an existing scan — a wholesale REPLACE. Any key previously written by TagResource (or by the original CreateScan) and absent from the new request is dropped, and a later ListTagsForResource no longer reports it. This is unrecoverable data loss on a resource attribute: the caller asked to start a revision, not to untag the scan.

## Evidence

Smithy model: CreateScanRequest$tags is "An array of key-value pairs used to tag a scan" (a tagging input), and the service models TagResource/UntagResource/ListTagsForResource over the same ScanNameArn resource; nothing documents CreateScan as removing tags. Handler: src/ops/create_scan.rs line 361-363 (replace) vs src/ops/tag_resource.rs lines 217-219 (merge) and src/ops/untag_resource.rs lines 185-187 (the only documented removal path). Tests: tests/all/resource_tagging.rs covers Tag/Untag/List round trips but never re-runs CreateScan over a tagged scan, so nothing asserts the surviving keys.

## Steps to Reproduce

1. POST /uploadUrl {"scanName":"demo"}; POST /scans creating scan demo with tags {"a":"1"}
2. POST /tags/<scanNameArn> with tags {"b":"2"}; GET /tags/<scanNameArn> -> {"a":"1","b":"2"}
3. Wait for the run to settle, POST /uploadUrl again and POST /scans for scanName=demo with tags {"c":"3"}
4. GET /tags/<scanNameArn> -> only {"c":"3"}: keys a and b were deleted by a call that was not UntagResource
