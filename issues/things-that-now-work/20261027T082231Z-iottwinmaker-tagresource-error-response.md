Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 9ce68f8efb447c5711e42ec7e2cb128c2507b7ba

# TagResource answers the per-resource tag limit with ServiceQuotaExceededException (402) instead of the modelled TooManyTagsException (400)

_Filed: 2026-10-27T08:22:31Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `9ce68f8efb447c5711e42ec7e2cb128c2507b7ba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A `TagResource` call that would leave the resource carrying more than `MAX_TAGS_PER_RESOURCE` tags is refused with `ServiceQuotaExceededException` (HTTP 402) — an error `TagResource` does not model at all (it is pulled in through `extra_errors`). The model declares `TooManyTagsException` for this operation precisely for this failure mode, and this crate's own state module documents the rule that way: the constant's doc says such a call "is refused with the `TooManyTagsException` (HTTP 400) that operation models". The handler one module over does the opposite. A client that handles the modelled error set sees an undeclared 402 where the modelled 400 belongs.

## Evidence

The model declares the error and documents its trigger (`smithy_query(query="shape",shape="TooManyTagsException")`):
```
"TooManyTagsException": {"errorKind": "client", "httpError": 400, "doc": "<p>The number of tags exceeds the limit.</p>"}
```
and `smithy_query(query="digest")` lists TagResource's errors as `["AccessDeniedException", "ResourceNotFoundException", "TooManyTagsException"]` — `ServiceQuotaExceededException` is NOT one of them. The handler restates this in its own header:

`services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/tag_resource.rs:75`
```
//! * `TooManyTagsException` (HTTP 400) — The number of tags exceeds the limit.
```

This crate's state module states the rule:

`services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/state.rs:294-297`
```
/// largest tag set any of this service's shapes can carry on a resource. A
/// `TagResource` call that would leave a resource past it is refused with the
/// `TooManyTagsException` (HTTP 400) that operation models.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;
```

The handler refuses that very condition with a different, unmodelled error:

`services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/tag_resource.rs:225-239`
```
        let resulting = tags.len() + added;
        if resulting > crate::state::MAX_TAGS_PER_RESOURCE {
...
            return Err(TagResourceErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException { message: None },
            )
            .into());
```
`TooManyTagsException` is never constructed anywhere in `src/` (it only appears as a generated struct in `ops/types/tag_resource_types.rs:28` and as an enum variant at `ops/tag_resource.rs:117`).

Test coverage: `services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/tests/all/resource_tags.rs:361-367` and the assertion at `:464` pin the current 402, so that expectation moves with the fix:
```
/// ([`MAX_TAGS_PER_RESOURCE`]); otherwise it is refused as the QUOTA refusal
/// exceeding a per-resource quota earns — HTTP 402
/// `ServiceQuotaExceededException` — and stores nothing.
```

## Steps to Reproduce

1. CreateWorkspace `w` with no tags.
2. TagResource on the workspace ARN with 50 distinct tag keys — accepted.
3. TagResource on the same ARN with one further NEW key (`{"extra": "x"}`).
4. Observe HTTP 402 `ServiceQuotaExceededException`, an error TagResource does not model, where the modelled `TooManyTagsException` (HTTP 400, "The number of tags exceeds the limit.") belongs — and which `src/state.rs:294-296` says this path returns.
