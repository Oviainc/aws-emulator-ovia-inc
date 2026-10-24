Issue-Severity: HIGH
Issue-Gap-Class: error-response
Issue-Commit: ceffded050eaaeab66f25b25154c54549736906e

# UntagResource never authorizes the caller, so an IAM-denied principal can remove tags

_Filed: 2026-10-24T02:47:39Z_
_Operation: UntagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `ceffded050eaaeab66f25b25154c54549736906e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model declares `AccessDeniedException` (HTTP 403) for `UntagResource`, and this crate implements a real IAM decision (`crate::state::authorize`, backed by `service_common::iam_authorization` over IAM's/STS's own stores) which `CreateExport`, `TagResource` and `ListTagsForResource` all call before touching state. `UntagResource`'s handler calls it for neither: it locks the store immediately and removes the named keys.

Expected: a caller whose identity-based policies do not allow `bcm-data-exports:UntagResource` on the resource ARN is refused with the modelled `AccessDeniedException` (403) BEFORE any state is written, exactly as `TagResource` is.
Actual: any caller — including one whose IAM policy explicitly denies the action, or allows only a different action/resource — gets HTTP 200 `{}` and the tags are really gone.

## Evidence

Spec: `smithy_query(query="operation", operation="UntagResource")` lists `AccessDeniedException` (HTTP 403) among its errors. Handler: `src/ops/untag_resource.rs` lines 141-170 — `handle` goes straight from `service_state(...)` to `store.exports.get_mut(&input.resource_arn)` with no `crate::state::authorize(...)` call, while `src/ops/tag_resource.rs` lines 157-166 and `src/ops/list_tags_for_resource.rs` lines 164-174 do call it. `src/service.rs` line 94-100 even states the invariant: "This service authorizes in the handlers whose model [declares 403] ... `UntagResource`, `ListTagsForResource`". Test coverage: `tests/all/front_door_authorization.rs` names `UntagResource` in its header (line 9) but its tables exercise only `CreateExport`, `ListTagsForResource` and `TagResource` — no assertion covers `UntagResource`, so the omission is untested.

## Steps to Reproduce

1. Register this service beside the IAM and STS emulators on one engine (as tests/all/front_door_authorization.rs does).
2. CreateExport with the account's ambient credential and keep the returned ExportArn.
3. TagResource that ARN with {"Key":"env","Value":"dev"} so the export carries a tag.
4. Create an IAM user with an inline policy that allows nothing for bcm-data-exports (or explicitly denies bcm-data-exports:UntagResource), and an access key for it.
5. Call UntagResource with that access key id in the SigV4 credential scope, ResourceArn = the export ARN, ResourceTagKeys = ["env"].
6. Observe HTTP 200 with an empty body instead of 403 AccessDeniedException; a follow-up ListTagsForResource with an allowed credential shows the tag was removed.
