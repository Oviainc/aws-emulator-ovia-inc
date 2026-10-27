Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: ba63228bbc1032132c1fbdbb0b54a8c844ce19f7

# Refuse a path-label ARN that is not an ARN at all with BadRequestException, not NotFoundException

_Filed: 2026-10-24T03:44:44Z_
_Operation: DescribeConnector | Class: error-response_

> _Note: this issue was identified against the code base at commit `ba63228bbc1032132c1fbdbb0b54a8c844ce19f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every kafkaconnect operation that takes a resource ARN in its URI path (`connectorArn`, `customPluginArn`, `workerConfigurationArn`, `resourceArn`) treats ANY value the store does not hold as a missing resource: HTTP 404 `NotFoundException` with `Invalid parameter <member>: The requested resource doesn’t exist.`

Expected (real service): a value that is not a well-formed kafkaconnect ARN at all (e.g. `foo`, `arn:aws:s3:::bucket`, `arn:aws:kafkaconnect` truncated) is incorrect INPUT, not a resource that is absent, so the service answers HTTP 400 `BadRequestException` naming the member it refused (the same `Invalid parameter <member>: Invalid ARN.` form this emulator already renders for the ARN-typed body members `bucketArn` and `serviceExecutionRoleArn`). HTTP 404 `NotFoundException` is reserved for a well-formed ARN naming a resource this account does not hold.

Actual (emulator): the handlers do a single `HashMap` lookup on the raw label and fall straight to `NotFoundException`, so a syntactically invalid ARN and a valid-but-absent ARN are indistinguishable to a client: both are 404 with the resource-doesn't-exist reason.

This is one shared seam across the read/delete/tag family, so fixing it means adding the ARN-syntax check (the existing `crate::state::is_arn_for(.., ARN_SERVICE)` / `INVALID_ARN` pair) ahead of the store lookup in: describe_connector.rs, describe_custom_plugin.rs, describe_worker_configuration.rs, delete_connector.rs, delete_custom_plugin.rs, delete_worker_configuration.rs, list_tags_for_resource.rs, tag_resource.rs and untag_resource.rs (the last three go through `state.tags_of` / `attach_tags` / `remove_tags`, which answer `TagWriteRefusal::NoSuchResource` for any unknown string).

## Evidence

Spec: the model declares BOTH errors for these operations — `BadRequestException` (HTTP 400, "Bad request due to incorrect input. Correct your request and then retry it.") and `NotFoundException` (HTTP 404, "Resource not found due to incorrect input."); DescribeConnector's `connectorArn` is an `@httpLabel` `__string` with no `@pattern`, so the framework cannot refuse a non-ARN at the boundary and the handler must.
Service wording already in the crate: src/state.rs documents the service's own recorded rejections, including `Invalid parameter bucketArn: Invalid ARN.` — i.e. kafkaconnect does validate ARN syntax on ARN-typed members and blames the member with a 400; the constant `crate::state::INVALID_ARN` exists for exactly that, and CreateCustomPlugin/CreateConnector use it (create_custom_plugin.rs lines 184-195, create_connector.rs lines 237-248). Sibling AWS service MSK (kafka) likewise answers 400 for a malformed cluster ARN.
Handler: src/ops/describe_connector.rs lines 194-202 — `state.connectors.get(&input.connector_arn)` and, on miss, `NotFoundException` with `RESOURCE_DOES_NOT_EXIST`; no ARN-syntax check anywhere in the handler. Same shape in describe_custom_plugin.rs (171-179), describe_worker_configuration.rs (174-187), delete_connector.rs (170-178), delete_custom_plugin.rs (167-175), delete_worker_configuration.rs (173-186), list_tags_for_resource.rs (167-175), tag_resource.rs (180-190), untag_resource.rs (175-184).
Tests: no test passes a malformed ARN. tests/all/validation_messages.rs defines `unheld_arn(kind)` (line 133) as a WELL-FORMED kafkaconnect ARN and asserts 404 for it; tests/all/worker_configuration_create_and_describe.rs `describing_a_worker_configuration_arn_the_service_never_minted_is_a_not_found` and tests/all/worker_configuration_delete_and_tags.rs `tagging_an_arn_the_service_never_minted_is_a_not_found` do the same. Nothing pins the non-ARN case, so no existing assertion contradicts the fix.

## Steps to Reproduce

1. GET /v1/connectors/not-an-arn — the emulator answers 404 NotFoundException `Invalid parameter connectorArn: The requested resource doesn’t exist.`; the real service answers 400 BadRequestException naming connectorArn as an invalid ARN.
2. GET /v1/tags/not-an-arn (ListTagsForResource) and POST /v1/tags/not-an-arn with a `tags` map (TagResource) — both answer 404 instead of 400.
3. DELETE /v1/custom-plugins/arn%3Aaws%3As3%3A%3A%3Asome-bucket (an ARN of another service) — answers 404 instead of a 400 refusing the member.
4. For contrast, GET /v1/connectors/arn%3Aaws%3Akafkaconnect%3Aus-east-1%3A000000000000%3Aconnector%2Fghost%2F00000000-0000-0000-0000-000000000000 — 404 NotFoundException here is correct and must stay.
