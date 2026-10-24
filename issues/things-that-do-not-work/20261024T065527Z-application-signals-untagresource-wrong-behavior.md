Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b

# UntagResource answers 404 for an instrumentation-configuration ARN, so its tags can never be removed

_Filed: 2026-10-24T06:55:27Z_
_Operation: UntagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UntagResource` resolves the same SLO-only ARN rule as the tag reader, so a live instrumentation configuration's ARN names no resource.

Expected: `POST /untag-resource` with an instrumentation-configuration `ResourceArn` and `TagKeys` detaches those keys and answers 200 with an empty body.

Actual: the handler answers 404 `ResourceNotFoundException` and writes nothing, so tags attached by `CreateInstrumentationConfiguration` are permanently un-removable (they only disappear when the configuration record itself is dropped).

## Evidence

Spec: `UntagResource` input `ResourceArn` (AmazonResourceName, len 1..1024) with `ResourceNotFoundException` reserved for an ARN that names no resource; `CreateInstrumentationConfiguration` mints an `instrumentationConfig/...` ARN and takes `Tags`, so that ARN names a taggable resource of this service.

Handler: `src/ops/untag_resource.rs` `handle` -> `store.tagged_resource_arn(&input.resource_arn)`; `src/state.rs` `tagged_resource_arn`/`tagged_slo_name_in_arn` only accept an ARN whose resource field is `slo/<name>`.

Tests: `tests/all/resource_tags.rs` exercises only SLO ARNs (no `instrumentation` reference), so nothing asserts this path.

## Steps to Reproduce

1. POST /create-instrumentation-configuration with Tags=[{"Key":"team","Value":"checkout"}] and note the returned ARN
2. POST /untag-resource with {"ResourceArn":"<that ARN>","TagKeys":["team"]}
3. Observe 404 ResourceNotFoundException instead of 200, and the tag remains stored
