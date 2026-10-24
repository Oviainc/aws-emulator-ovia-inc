Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b

# ListTagsForResource answers 404 for an instrumentation-configuration ARN, so tags written by CreateInstrumentationConfiguration are unreadable

_Filed: 2026-10-24T06:55:27Z_
_Operation: ListTagsForResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListTagsForResource` resolves ONLY service-level-objective ARNs. `CreateInstrumentationConfiguration` accepts `Tags` and stores them in the same tag map keyed by the configuration's minted ARN (`arn:aws:application-signals:<region>:<account>:instrumentationConfig/<...>/<locationHash>`), but the tag reader cannot resolve that ARN shape.

Expected: a `GET /tags?ResourceArn=<instrumentationConfig ARN>` for a live configuration answers 200 with the tags that configuration carries (the ARN is returned by the create as `ARN`, modelled with pattern `^arn:[^:]+:application-signals:[^:]+:[0-9]{12}:instrumentationConfig/.+/[0-9a-f]{16}$`, and `ResourceArn` is a bare `AmazonResourceName` that names any taggable resource of this service).

Actual: the ARN resolves to no resource, so the handler answers 404 `ResourceNotFoundException`. The tags the create persisted are dead state that no API can ever read back.

## Evidence

Spec: `CreateInstrumentationConfiguration` input `Tags` (list<Tag>, len 0..200) and output `ARN` with the `instrumentationConfig/...` pattern (OPERATIONS.md / smithy model); `ListTagsForResource` input `ResourceArn` carries no pattern restricting it to SLOs, and its only documented failure is a resource that does not exist.

Handler: `src/ops/list_tags_for_resource.rs` `handle` calls `store.tagged_resource_arn(&input.resource_arn)`; `src/state.rs` `tagged_resource_arn` -> `tagged_slo_name_in_arn(resource_arn)` -> `slo_name_in_arn` which requires the ARN resource field to be `slo/<name>`, so an `instrumentationConfig/...` resource field yields `None` -> 404. Meanwhile `src/ops/create_instrumentation_configuration.rs` lines ~288-295 insert the request's tags under `configuration.arn`.

Tests: `tests/all/resource_tags.rs` and `tests/all/instrumentation_configurations.rs` contain no reference to instrumentation-configuration tags (grep for `Tags`/`instrumentation` finds none), so nothing asserts this path.

## Steps to Reproduce

1. POST /create-instrumentation-configuration with InstrumentationType=BREAKPOINT, Service/Environment/SignalType/Location/CaptureConfiguration and Tags=[{"Key":"team","Value":"checkout"}]
2. Note the ARN in the 200 response (arn:aws:application-signals:<region>:<account>:instrumentationConfig/.../<hash>)
3. GET /tags?ResourceArn=<that ARN>
4. Observe 404 ResourceNotFoundException instead of 200 with the stored tag list
