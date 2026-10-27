Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 503d6e01beb92b119e0de4be4193cbd830ada528

# TagResource judges its tag lengths only after the record is resolved, so an invalid tag set on an unknown ARN answers 404 instead of 400

_Filed: 2026-10-25T14:11:56Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `503d6e01beb92b119e0de4be4193cbd830ada528`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource` runs only the reserved-namespace key check on its input before resolving the `resourceArn`; the rest of the tag judgement (the key/value LENGTH bounds `is_tag_key`/`is_tag_value` and the resulting per-resource tag count) is reached only inside `merge_tags`, which runs AFTER the record has been looked up. So a request carrying a tag the member does not admit — an empty key, a key past `TAG_KEY_MAX_LEN`, a value past `TAG_VALUE_MAX_LEN` — gets two different answers depending on a fact about the store rather than about the request: HTTP 400 `ValidationException` when the ARN names a live record, and HTTP 404 `NotFoundException` when the ARN names no record (or names one the clock has already expired).

That contradicts the rule this crate states for the very helper it skips, and it diverges from the sibling handler: `PutDeploymentParameter` runs `validate_tags_member` over its own `tags` BEFORE it takes the store lock and before it resolves its target, so a bad tag there stays a 400 even when the target would have been a 404. Because `TagResource$tags` targets the model's UNCONSTRAINED `Tags` map (`"tags": {"target": "Tags"}`, `Tags` = `map<String,String>` with no `@length`), the centralized constraint walk refuses nothing here — this handler is the only judge, so the skipped call is the whole difference.

The fix is to call the helper the crate already has (`crate::state::validate_tags_member(tags, TagConstraints::Unconstrained)`) on the input, where the reserved-key check already runs; `merge_tags` keeps its own result-level check unchanged.

## Evidence

The rule the crate states for the skipped helper — services/service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice/src/state.rs:774-778:

    /// Because it judges only the input, a request carrying a tag the member does not
    /// admit is refused the SAME way whichever path the operation would then take, and
    /// whether or not the tags would have been applied at all: an operation runs this
    /// over its `tags` ahead of resolving anything, and nothing is stored either way.
    /// [`merge_tags`] runs it too, so no merge can store an entry this would refuse.

The sibling handler that does run it ahead of resolving — src/ops/put_deployment_parameter.rs:249-255 (before `state.lock()` at :268 and before the target 404 at :297-304):

        if let Some(tags) = input.tags.as_ref() {
            if let Err(why) =
                crate::state::validate_tags_member(tags, crate::state::TagConstraints::TagsMap)
            {
                return Err(PutDeploymentParameterErrorModeled::ValidationException(why).into());
            }
        }

TagResource's input check, which covers the reserved namespace ONLY — src/ops/tag_resource.rs:175-181:

        if let Some(tags) = input.tags.as_ref() {
            if let Err(why) =
                crate::state::validate_tag_keys_unreserved(tags.keys().map(String::as_str), "tags")
            {
                return Err(TagResourceErrorModeled::ValidationException(why).into());
            }
        }

…and the lookup that answers first for everything else — src/ops/tag_resource.rs:199-204, with the length judgement only afterwards at :214-223:

        let Some(record) = store.parameters.get_mut(&input.resource_arn) else {
            return Err(crate::state::parameter_not_found());
        };
        ...
        let merged = match crate::state::merge_tags(
            &record.tags,
            &input.tags.unwrap_or_default(),
            crate::state::TagConstraints::Unconstrained,
        ) {

The member really is unconstrained, so nothing refuses these tags before the handler — smithy_query(query="operation",operation="TagResource"): `"tags": {"target": "Tags", "type": "map"}` and smithy_query(query="shape",shape="Tags"): `{"type": "map", "key": "String", "value": "String"}` (no length, no pattern).

Test coverage: the length refusals are pinned only against a LIVE ARN — tests/all/unconstrained_tag_characters.rs:170-226 (`the_tagging_limits_still_refuse_what_no_tag_may_carry`) tags `arn` from `tagged_parameter(...)` and asserts 400 `ValidationException` with `fieldName == "tags"`. tests/all/not_found_error_code.rs:91-108 only ever sends a VALID tag set (`{"tags": {"owner": "integration-test"}}`) to an unknown ARN, so the unknown-ARN + invalid-tags combination is untested and nothing pins the 404.

## Steps to Reproduce

1. Start the emulator and create one deployment parameter: POST /catalogs/AWSMarketplace/products/prod-default-1/deployment-parameters with {"agreementId":"agmt-default-1","deploymentParameter":{"name":"DbPassword","secretString":"s"}} — note the returned resourceArn.
2. Call TagResource on that live ARN with a tag value past the bound: POST /tags/<urlencoded live ARN> with {"tags":{"k":"<257 characters>"}} — observe HTTP 400 ValidationException with fieldName "tags" (src/ops/tag_resource.rs:214-223).
3. Call TagResource with the SAME invalid tag set on an aws-marketplace ARN that names no stored record, e.g. arn:aws:aws-marketplace:us-east-1:<account>:deployment-parameter/dp-ffffffffffffffff — observe HTTP 404 NotFoundException instead of the 400 the same input got in step 2.
4. Repeat step 3 with an empty tag key ({"tags":{"":"v"}}) or a key past 128 characters: same 404.
5. For contrast, call PutDeploymentParameter with the same class of bad tag and an unresolved target (productId=prod-nope): observe HTTP 400 ValidationException, because that handler runs crate::state::validate_tags_member on the input before resolving anything (src/ops/put_deployment_parameter.rs:249-255).
