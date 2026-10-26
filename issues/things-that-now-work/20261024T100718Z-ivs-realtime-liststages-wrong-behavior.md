Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ca029f426fda2d51f4b9c87edd69d4539a13e33a

# Stages, encoder/ingest/storage configurations are not Region-scoped: a resource created through one Region's endpoint is listed, read, tagged and deleted through every other

_Filed: 2026-10-24T10:07:18Z_
_Operation: ListStages | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ca029f426fda2d51f4b9c87edd69d4539a13e33a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListStages documents exactly what its result set is: "Gets summary information about all stages in your account, **in the AWS region where the API request is processed**" (ListStorageConfigurations and ListEncoderConfigurations carry the same sentence). The emulator already knows the Region of each request — `crate::state::request_region(ctx)` reads it off the SigV4 credential scope and every create mints the resource ARN in it (`arn:aws:ivs:<region>:<account>:stage/<id>`) — but no read path ever consults it.

Expected: a stage created through a request signed for `us-east-1` is reported by ListStages only when the listing request is itself signed for `us-east-1`; a listing signed for `eu-west-1` reports an empty `stages` list, and GetStage / UpdateStage / DeleteStage / the tagging surface answer ResourceNotFoundException (404) for an ARN whose Region element is not the Region the request was processed in (a resource in another Region is simply not in this Region's account view).

Actual: the whole service shares one flat, Region-less store (`HashMap<String, StageRecord>` keyed by ARN only). ListStages walks `store.stages_in_order()` with no Region predicate, so stages minted in every Region are returned in one page; GetStage/UpdateStage/DeleteStage look up `store.stages.get(&input.arn)` by ARN alone, and `crate::state::resource_tags`/`resource_tags_mut` resolve a tagging ARN by its resource-type element only. The same is true of ListStorageConfigurations and of the encoder/ingest/storage configuration reads. So a client that creates a stage in one Region and then lists or mutates it through another Region's endpoint succeeds against the emulator and fails against AWS.

## Evidence

Spec: smithy_query(query="doc", shape="ListStages") — "all stages in your account, in the AWS region where the API request is processed"; same sentence on ListStorageConfigurations. ARN shapes (StageArn, StorageConfigurationArn, …) all carry a Region element, and the emulator fills it from the request (src/state.rs `request_region`, lines ~1537-1547; used by create_stage.rs:190, create_storage_configuration.rs:172, create_encoder_configuration.rs:200, create_ingest_configuration.rs:236).
Handler: src/ops/list_stages.rs `handle` (lines 144-195) — `store.stages_in_order()` filtered only by the page token; no Region term. src/ops/get_stage.rs:151, update_stage.rs:162, delete_stage.rs:152 key on `input.arn` alone. src/ops/list_storage_configurations.rs:193 likewise. src/state.rs `resource_tags`/`resource_tags_mut` (lines ~2222-2260) match only the ARN's resource-type element.
Tests: no test asserts Region scoping. tests/all/tagging.rs (`tags_round_trip_through_the_tagging_surface_for_any_admitted_stage`, lines 301-396) currently creates stages in `eu-west-1`/`ap-southeast-2` and then tags/reads them with requests signed for `us-east-1` (`AUTHORIZATION_REGION`), asserting success — i.e. the existing suite codifies the un-scoped behaviour and would need revisiting as part of the fix.

## Steps to Reproduce

1. POST /CreateStage {"name":"regional"} with an Authorization credential scope of .../us-east-1/ivs/aws4_request → note stage.arn (…:us-east-1:…:stage/<id>)
2. POST /ListStages {} with an Authorization credential scope of .../eu-west-1/ivs/aws4_request
3. Expected: HTTP 200 with "stages": [] — the us-east-1 stage is not in the eu-west-1 view
4. Actual: HTTP 200 listing the us-east-1 stage
5. POST /GetStage {"arn":"<the us-east-1 stage arn>"} signed for eu-west-1 → expected 404 ResourceNotFoundException, actual HTTP 200 with the stage
6. Same divergence for POST /tags/{arn}, GET /tags/{arn}, POST /DeleteStage and POST /ListStorageConfigurations
