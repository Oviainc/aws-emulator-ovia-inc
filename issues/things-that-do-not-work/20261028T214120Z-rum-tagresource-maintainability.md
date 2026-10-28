Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: ab8b9af343344bd9ec1c9404b5c1438075827ba1

# The tagging frontend's refusal-to-wire-code rule is implemented three times, once per ARN-addressed tagging operation

_Filed: 2026-10-28T21:41:20Z_
_Operation: TagResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `ab8b9af343344bd9ec1c9404b5c1438075827ba1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The three ARN-addressed tagging operations (`TagResource`, `UntagResource`, `ListTagsForResource`) each carry their OWN hand-written error channel that encodes the same rule: a not-found answers `NotFoundException` with `ResourceNotFoundException`'s payload/schema and 404, a bad request answers `BadRequestException` with `ValidationException`'s payload/schema and 400, and an IAM refusal answers `AccessDeniedException` with the modelled `AccessDeniedException` payload and 403. Each copy repeats the same four `match` arms across `Display`, `as_serializable`, `variant_schema`, `shape_name` and `http_status`, and two of the three also declare their own private copies of the `NOT_FOUND_ERROR_CODE` / `BAD_REQUEST_ERROR_CODE` constants.

The copies can drift: changing the code, the status or the carried schema of one refusal class in one file leaves the other two frontends answering the old way, even though the crate's own comments insist "one frontend cannot word one condition two ways". The crate already fixed exactly this shape of duplication twice (the `MaxResults` page-size rule and the `declared_arn` rule), by moving the rule into `crate::state` and having each operation reach it there — the same move applies here: one shared refusal type (or one shared set of `(code, status, schema)` mappings) in `crate::state`, with each operation's channel delegating to it. Every response byte and every existing test stays as it is.

## Evidence

Copy 1 — `services/service-com-amazonaws-rum-rum/src/ops/tag_resource.rs:127-128` and `:221-228`:
```
const NOT_FOUND_ERROR_CODE: &str = "NotFoundException";
const BAD_REQUEST_ERROR_CODE: &str = "BadRequestException";
```
```
    fn shape_name(&self) -> &'static str {
        match self {
            Self::Modeled(e) => ::smithy_server_runtime::OperationError::shape_name(e),
            Self::NotFound(_) => NOT_FOUND_ERROR_CODE,
            Self::BadRequest(_) => BAD_REQUEST_ERROR_CODE,
            Self::AccessDenied(_) => ACCESS_DENIED_ERROR_CODE,
        }
    }
```

Copy 2 — `services/service-com-amazonaws-rum-rum/src/ops/untag_resource.rs:111`, `:121` and `:273-279`, character-for-character the same mapping:
```
const NOT_FOUND_ERROR_CODE: &str = "NotFoundException";
...
const BAD_REQUEST_ERROR_CODE: &str = "BadRequestException";
```
```
    fn variant_schema(&self) -> &'static ::aws_smithy_schema::Schema {
        match self {
            Self::Modeled(e) => ::smithy_server_runtime::OperationError::variant_schema(e),
            Self::NotFound(_) => &super::types::SCHEMA_RESOURCE_NOT_FOUND_EXCEPTION,
            Self::BadRequest(_) => &super::types::SCHEMA_VALIDATION_EXCEPTION,
            Self::AccessDenied(_) => &super::types::SCHEMA_ACCESS_DENIED_EXCEPTION,
        }
```

Copy 3 — `services/service-com-amazonaws-rum-rum/src/ops/list_tags_for_resource.rs:123` and `:190-214`, the same rule minus the `BadRequest` arm:
```
const NOT_FOUND_ERROR_CODE: &str = "NotFoundException";
```
```
    fn variant_schema(&self) -> &'static ::aws_smithy_schema::Schema {
        match self {
            Self::Modeled(e) => ::smithy_server_runtime::OperationError::variant_schema(e),
            Self::NotFound(_) => &super::types::SCHEMA_RESOURCE_NOT_FOUND_EXCEPTION,
            Self::AccessDenied(_) => &super::types::SCHEMA_ACCESS_DENIED_EXCEPTION,
        }
    }
...
            Self::NotFound(_) => 404,
            Self::AccessDenied(_) => ACCESS_DENIED_STATUS,
```

The crate's own comments admit the copies are meant to be one thing — `untag_resource.rs:217-221` ("the channel is the modelled enum plus those two refusals, exactly as `TagResource` declares it"), `tag_resource.rs:156-160` ("exactly as `UntagResource` declares it"), `list_tags_for_resource.rs:140-143` ("the channel is the modelled enum plus that single refusal, exactly as [`crate::ops::untag_resource::UntagResourceError`] is built"), and `tag_resource.rs:123-126` ("The sibling [`crate::ops::untag_resource`] and [`crate::ops::list_tags_for_resource`] name the same two values").

The shared half already lives in state, which is the precedent for the move — `services/service-com-amazonaws-rum-rum/src/state.rs:1960-1961`:
```
pub const ACCESS_DENIED_ERROR_CODE: &str = "AccessDeniedException";
pub const ACCESS_DENIED_STATUS: u16 = 403;
```

Tests that pin these responses and must stay green unchanged: `tests/all/resource_tags.rs:47-63` (`NOT_FOUND_ERROR_CODE = "NotFoundException"`, `BAD_REQUEST_ERROR_CODE = "BadRequestException"`), `tests/all/list_tags_for_resource.rs:30-45`, `tests/all/resource_tagging.rs:47-50`, `tests/all/tagging_authorization.rs:35-38` (`ACCESS_DENIED_CODE`), `tests/all/tagging_no_content.rs:50-53`, and `tests/all/announced_media_type.rs:271-276`.

## Steps to Reproduce

1. Read `src/ops/tag_resource.rs:127-128` and `:184-240`, `src/ops/untag_resource.rs:111-121` and `:245-290`, and `src/ops/list_tags_for_resource.rs:111-224`: the same (refusal class -> wire code, HTTP status, carried schema) mapping is written out three times.
2. Change one of the three copies (for example the not-found code in `untag_resource.rs`) and observe that the other two tagging operations keep answering the old code — one frontend wording one condition two ways, which each file's own doc comment says must not happen.
3. Confirm the fix is behaviour-preserving: the three channels differ only in which arms they carry, so a shared refusal type in `crate::state` reproduces every current response byte and leaves the tests listed above untouched.
