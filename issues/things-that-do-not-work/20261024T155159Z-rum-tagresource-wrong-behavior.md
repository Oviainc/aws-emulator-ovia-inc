Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# The ARN-addressed tagging operations skip IAM authorization entirely

_Filed: 2026-10-24T15:51:59Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other operation in this crate evaluates the caller's permissions on every call through `crate::state::authorization` (identity-based policies plus the app monitor's resource policy) and refuses with `AccessDeniedException` 403. The three ARN-addressed tagging operations do not: `TagResource`, `UntagResource` and `ListTagsForResource` explicitly discard the request context (`let _ = ctx;`) and go straight to the store, so ANY credential — including a role whose policies grant nothing, or one carrying an explicit `Deny` on `rum:*` — can read a monitor's tags, add tags and remove tags.

Expected: a principal without `rum:TagResource` / `rum:UntagResource` / `rum:ListTagsForResource` on the app monitor's ARN (or one denied them) is refused with `AccessDeniedException` and HTTP 403, and the resource's tag map is left untouched. These are IAM-gated RUM actions like every other operation of the service; `AccessDeniedException` is an AWS common error, which is why the three operations' modeled error lists do not have to declare it.

Actual: the call succeeds with 204 (tag/untag) or 200 (list) regardless of the caller's policies, so tags can be read and silently rewritten by an unauthorized principal. One root cause, three operations — a fixer should wire all three through the same `authorization::authorize` path the sibling operations use (action `rum:TagResource`, `rum:UntagResource`, `rum:ListTagsForResource`, resource = the stored app-monitor ARN the request names, with the monitor's resource policy consulted as elsewhere).

## Evidence

Semantics: all AWS API actions are IAM-gated, and RUM publishes `rum:TagResource`, `rum:UntagResource` and `rum:ListTagsForResource` as actions on the app-monitor resource; an unauthorized caller gets the common `AccessDeniedException` with HTTP 403. Handlers: `src/ops/tag_resource.rs` (`let _ = ctx;` then `store.app_monitor_by_arn_mut`), `src/ops/untag_resource.rs` (`let _ = ctx;` then the same resolution and `record.tags.remove(key)`), `src/ops/list_tags_for_resource.rs` (`let _ = ctx;` then `app_monitor_by_arn`) — none calls `crate::state::authorization`. Contrast `src/ops/get_app_monitor.rs`, `put_rum_metrics_destination.rs`, `put_resource_policy.rs` etc., which all authorize. Test coverage: `tests/all/authorization.rs` claims "every CloudWatch RUM operation decides on every call" but its tables cover only the 11 Name-addressed operations (`rum:GetAppMonitor`, `rum:PutRumEvents`, `rum:CreateAppMonitor`, `rum:DeleteAppMonitor`, `rum:UpdateAppMonitor`, `rum:ListAppMonitors`, the two metrics-destination actions and the three resource-policy actions) — no tagging action appears anywhere in the suite.

## Steps to Reproduce

1. CreateAppMonitor(Name='tagged', Domain='example.com', Tags={'Team':'web'}) with an administrative credential -> 200.
2. Through IAM, create a role whose only policy is Allow on an unrelated action (or an explicit Deny on 'rum:*'), and get a session credential for it from STS AssumeRole.
3. Signed with that credential: GET /tags/arn:aws:rum:<region>:<account>:appmonitor/tagged -> emulator answers 200 with the tag map; AWS answers AccessDeniedException 403.
4. Signed with that credential: POST /tags/<same ARN> with {"Tags":{"Owner":"intruder"}} -> emulator answers 204 and the tag is persisted (visible via GetAppMonitor); AWS answers AccessDeniedException 403 and stores nothing.
5. Signed with that credential: DELETE /tags/<same ARN>?tagKeys=Team -> emulator answers 204 and the tag is gone; AWS answers AccessDeniedException 403.
