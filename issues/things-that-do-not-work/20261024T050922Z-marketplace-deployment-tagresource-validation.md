Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2cb6c27e7920aa8b3ecbca8018912df677b4544f

# TagResource refuses tag keys/values that the model leaves unconstrained (spaces, empty values)

_Filed: 2026-10-24T05:09:22Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `2cb6c27e7920aa8b3ecbca8018912df677b4544f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource$tags` targets the UNCONSTRAINED `Tags` map (`map<String,String>` — no `@length`, no `@pattern`), and `ListTagsForResource$tags` reports through that same unconstrained map. Only `PutDeploymentParameter`'s own `tags`/response member targets the constrained `TagsMap` (max 50 entries, keys `TagKey` 1..128 `^[a-zA-Z0-9/_+=.:@-]+$`, values `TagValue` 1..256 same pattern).

The emulator nevertheless applies the `TagsMap` key/value constraints to `TagResource` input: `crate::state::merge_tags` runs every incoming key through `is_tag_key` and every value through `is_tag_value` and returns the modelled `ValidationException` (HTTP 400) on a miss. The practical consequence is that ordinary AWS tags are refused:

* `{"Cost Center": "us east"}` -> HTTP 400 `ValidationException`, nothing stored. AWS's documented tag character set for every taggable resource includes UTF-8 letters, numbers, SPACES and `+ - = . _ : / @`, so a space-bearing key or value is a normal tag.
* `{"k": ""}` (empty tag value) -> HTTP 400. AWS permits an empty tag value (the key is what must be non-empty).
* any non-ASCII (UTF-8 letter) key or value -> HTTP 400.

Expected: `TagResource` accepts a tag set the model places no constraint on — standard AWS tag keys/values, including spaces, unicode letters and an empty value — answers HTTP 204 and stores them, and a following `ListTagsForResource` reports them back through the unconstrained `Tags` map. Actual: HTTP 400 `ValidationException` (`fieldName: "tags"`) and the record is left untouched, so the tag is never stored and never listed.

Note this is only about tag keys/values. The per-resource CAP of 50 is a real AWS tagging limit and refusing a merge past it is fine; so is refusing an EMPTY key.

## Evidence

Model (`smithy_query(query="shape",shape="Tags,TagsMap")`): `Tags` = `map<String,String>` with no length/pattern; `TagsMap` carries `length max 50` plus the `TagKey`/`TagValue` `^[a-zA-Z0-9/_+=.:@-]+$` patterns and is referenced ONLY by `PutDeploymentParameterRequest$tags` / `PutDeploymentParameterResponse$tags` (`smithy_query(query="operation",operation="TagResource,ListTagsForResource")` shows both tagging members target `Tags`). Well-established AWS behavior: the tag character set AWS documents for taggable resources includes spaces and UTF-8 letters, and tag values may be empty. Handler: services/service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice/src/ops/tag_resource.rs lines 177-183 call `crate::state::merge_tags(&record.tags, &input.tags…)`; src/state.rs `is_tag_key`/`is_tag_value` (lines ~435-443) and `merge_tags` (lines ~457-492) reject anything outside `TAG_CHARACTER_CLASS = [a-zA-Z0-9/_+=.:@-]+` or outside 1..128 / 1..256. Tests: the current behavior is deliberately asserted — tests/all/deployment_parameters.rs `a_tag_write_the_service_could_not_report_is_refused_and_stores_nothing` (lines 878-918) expects 400 for "a key carrying a space" and "an empty value", and tests/all/validation_messages.rs `the_tag_character_class_this_service_states_carries_no_anchors` (lines 261-297) expects 400 for "a value carrying a space" — so fixing this means relaxing those rows, not adding uncovered behavior.

## Steps to Reproduce

1. Seed an agreement and POST /catalogs/AWSMarketplace/products/prod-tags/deployment-parameters with {"agreementId":"agmt-tags","deploymentParameter":{"name":"Tagged","secretString":"s"}} and note the returned resourceArn.
2. POST /tags/{percent-encoded resourceArn} with body {"tags":{"Cost Center":"us east 1"}} — a tag key and value inside AWS's documented tag character set but outside the TagsMap pattern the model applies only to PutDeploymentParameter.
3. Observe HTTP 400 ValidationException ("tag key 'Cost Center' is outside the 1..128 characters of [a-zA-Z0-9/_+=.:@-]+ …") instead of HTTP 204, although TagResource$tags targets the unconstrained Tags map.
4. GET /tags/{resourceArn} and observe the tag was not stored.
5. Repeat step 2 with {"tags":{"k":""}} (empty tag value, which AWS accepts) and observe the same 400.
