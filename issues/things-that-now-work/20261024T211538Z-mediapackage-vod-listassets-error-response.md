Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: e0e9a90161aff7ec754055c046e25b8f716c6f2d

# Model-constraint refusals answer the framework's generic 400 ValidationException instead of this service's 422 UnprocessableEntityException with the per-member errors map

_Filed: 2026-10-24T21:15:38Z_
_Operation: ListAssets | Class: error-response_

> _Note: this issue was identified against the code base at commit `e0e9a90161aff7ec754055c046e25b8f716c6f2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

MediaPackage VOD answers a request whose input members do not validate with HTTP 422 `UnprocessableEntityException`, carrying a `message` in the `<member_name> <phrase>` form plus an `errors` map that attributes each violation to the offending input member (or to `base` for a whole-request violation). This crate documents that rule from recordings (src/scaffolded.rs, "How this service words a request-validation refusal") and implements it for every violation its handlers check themselves (nonexistent packagingGroupId, junk ARNs, tag restrictions, forged nextToken, ...).

Violations the FRAMEWORK finds before the handler runs are never put into that envelope:

* `maxResults` outside the modelled `@range(min:1,max:1000)` on ListAssets, ListPackagingConfigurations and ListPackagingGroups. The handler's own refusal for this case (`crate::state::page_size`, which builds `UnprocessableEntity::member("maxResults", "must be between 1 and 1000")`) is dead code, because the boundary validator refuses the request first.
* An absent `@required` member on the creates and on the tag operations (e.g. `POST /packaging_groups {}` with no `id`, `POST /assets` with no `sourceArn`/`sourceRoleArn`/`packagingGroupId`, `DELETE /tags/{ResourceArn}` with no `tagKeys`).

Expected (AWS): HTTP 422, `x-amzn-errortype: UnprocessableEntityException`, body `{"message": "max_results must be between 1 and 1000", "errors": {"maxResults": [...]}}` — the same status, code and per-attribute attribution the service uses for every other parameter violation.
Actual (emulator): `src/service.rs` leaves `override_refusal` at the generated default (`let _ = refusal; None`), so the refusal renders in the framework's generic restJson1 envelope — HTTP 400, error code/`x-amzn-errortype` `ValidationException`, message `"N validation errors detected: ..."` — and with no `errors` member at all. A client branching on `UnprocessableEntityException`, or rendering per-field validation errors from `errors`, gets neither.

## Evidence

Spec/recorded rule: the model declares `UnprocessableEntityException` (errorKind client, httpError 422, member `Message` jsonName `message`) on every operation that takes parameters, and `MaxResults` carries `@range(min:1,max:1000)` with `@httpQuery(maxResults)` on all three listings (smithy_query operation=ListAssets). The 422-plus-`errors`-map rule is recorded behaviour: it is spelled out in src/scaffolded.rs lines ~182-205 and in the already-fixed issue 20261022T103701Z-mediapackage-vod-error-responses-misclassify-status-and-rebuild (recorded bodies `{'errors': {'packagingGroupId': ['does not exist']}, 'message': 'packaging_group_id does not exist'}` with `x-amzn-errortype: UnprocessableEntityException`).
Handler/seam: src/state.rs `page_size()` (lines ~880-899) comments that "the boundary validator already refuses anything outside it", confirming its 422 is unreachable; src/service.rs `override_refusal` returns `None`, so the framework's choice stands. The framework's default for a restJson1 `Refusal::Invalid` is HTTP 400 with code `ValidationException` (see services/service-com-amazonaws-apigateway-backplanecontrolservice/src/service/validation.rs, which reworded exactly this seam for its own service and notes "NOT the framework's default for a restJson1 service (`ValidationException`)").
Tests: tests/all/packaging_lifecycle.rs:630-638 and :1245-1252, and tests/all/packaging_configuration_listing.rs:403-410, assert only `(400..500).contains(&status)` for `maxResults` 0 and 1001 — deliberately loose, so no test pins the status, the error code or the presence of the `errors` map. No test exercises an absent required member.

## Steps to Reproduce

1. Boot the emulator and create a packaging group plus two assets (POST /packaging_groups {"id":"g"}; POST /assets with id/packagingGroupId/sourceArn/sourceRoleArn).
2. GET /assets?maxResults=0 (and ?maxResults=1001): observe HTTP 400 with x-amzn-errortype: ValidationException and a message of the form "1 validation error detected: ..." and no `errors` member, where AWS answers 422 / UnprocessableEntityException with errors {"maxResults": [...]}.
3. Repeat against GET /packaging_groups?maxResults=1001 and GET /packaging_configurations?maxResults=0: same envelope.
4. POST /packaging_groups with body {} (no required `id`): observe the same generic 400 ValidationException instead of 422 UnprocessableEntityException with errors {"id": [...]}.
5. POST /assets with body {"id":"a","packagingGroupId":"g"} (no sourceArn/sourceRoleArn): same.
