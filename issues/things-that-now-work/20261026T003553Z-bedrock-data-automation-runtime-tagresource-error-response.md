Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 232a76c0adfd6241e7fe920f3ef518cb863b87b4

# TagResource's quota refusal echoes the caller-supplied resourceARN, which this crate states the service never does

_Filed: 2026-10-26T00:35:53Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `232a76c0adfd6241e7fe920f3ef518cb863b87b4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states, as a property of the whole service, that a refusal carries nothing from the request and that the caller-supplied ARN is never echoed back: `crate::state::INVOCATION_NOT_FOUND_MESSAGE` documents its fixed string with "the ARN the caller sent is not interpolated, because this service does not echo it back", and `service::clause_for_violation` words every boundary refusal the same way ("the offending value is NOT echoed", stated for `resourceARN` specifically). `TagResource` breaks that rule in its own refusal path: when the merge would take the resource past `MAX_TAGS` it builds the `ServiceQuotaExceededException` message with `format!("Cannot have more than {} tags on resource {}", crate::state::MAX_TAGS, input.resource_arn)`, splicing the caller's whole `resourceARN` into the message body. Every other refusal this crate emits — the four `ResourceNotFoundException` paths (`get_data_automation_status.rs:192`, `tag_resource.rs:193`, `list_tags_for_resource.rs:176`, `untag_resource.rs:190`), the tag-key `ValidationException` in all three tagging/submit paths, and the boundary-walk rewording in `service.rs` — carries no request value, so this one message is the sole place the service reads the other way for one logical class of failure. The sibling quota refusal in `invoke_data_automation_async.rs:297-300` ("Cannot have more than {} data automation invocations in progress") interpolates only the constant, which is the shape the rest of the crate uses.

## Evidence

Rule, stated service-wide in this crate: `services/service-com-amazonaws-bedrockdataautomationruntime-amazonbedrockkeystoneruntimeservice/src/state.rs:272-274` — "It is the service's own fixed string and carries NOTHING from the request: the ARN the caller sent is not interpolated, because this service does not echo it back. Kept in one place so the four readers cannot drift apart." Same rule restated for the refusal wording: `src/service.rs:55-59` — "Two differences from the framework's conventional wording, both shown by the recorded wire for three separate operations (`inputConfiguration.s3Uri`, `invocationArn`, `resourceARN`): the offending value is NOT echoed, and a `@pattern` is printed without its anchors." Code that breaks it: `src/ops/tag_resource.rs:204-213` — `if merged.len() > crate::state::MAX_TAGS { ... message: Some(format!("Cannot have more than {} tags on resource {}", crate::state::MAX_TAGS, input.resource_arn)) ... }`. Sibling that keeps the rule: `src/ops/invoke_data_automation_async.rs:294-303` interpolates only `crate::state::MAX_INVOCATIONS`, no request value. Test coverage: `tests/all/data_automation_invocations.rs:940-956` (`tagging_past_the_modelled_tag_bound_is_refused_and_changes_nothing`) asserts only the 402 status and the `ServiceQuotaExceededException` code for this path, never the message text, so no test pins the echoed ARN; `tests/all/data_automation_invocations.rs:1509-1514` is the test that pins the no-echo rule for the refusal wording (`!message.contains("Value '")`).

## Steps to Reproduce

1. POST / with `X-Amz-Target: AmazonBedrockKeystoneRuntimeService.InvokeDataAutomationAsync` and a submission carrying 200 tags (the modelled `TagList` maximum), e.g. body {"inputConfiguration":{"s3Uri":"s3://in-bucket/assets/doc.pdf"},"outputConfiguration":{"s3Uri":"s3://out-bucket/quota/"},"dataAutomationProfileArn":"arn:aws:bedrock:us-west-2:aws:data-automation-profile/us.data-automation-v1","dataAutomationConfiguration":{"dataAutomationProjectArn":"arn:aws:bedrock:us-west-2:000000000000:data-automation-project/invoices"},"tags":[... 200 distinct keys ...]}; keep the returned invocationArn.
2. Call `TagResource` with that ARN and one more distinct key: {"resourceARN":"<arn>","tags":[{"key":"one-too-many","value":"x"}]}.
3. Read the response body's `message`: it is "Cannot have more than 200 tags on resource arn:aws:bedrock:us-west-2:000000000000:data-automation-invocation/<id>" — the caller-supplied `resourceARN` spliced in.
4. Call `ListTagsForResource` with a bogus-but-well-formed ARN of the same shape and read its `message`: "Data Automation Invocation ARN not found." — no request value, per the rule quoted in state.rs:272-274.
