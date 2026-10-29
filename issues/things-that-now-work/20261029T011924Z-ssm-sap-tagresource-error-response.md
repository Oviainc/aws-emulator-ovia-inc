Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 6078ee6ad52df0e8ecffa919707cccb1e63193a4

# Tagging operations can answer an InternalServerException the model declares for neither the operation nor the service

_Filed: 2026-10-29T01:19:24Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `6078ee6ad52df0e8ecffa919707cccb1e63193a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The three tagging handlers (`TagResource`, `UntagResource`, `ListTagsForResource`) widen their generated error enum with `extra_errors = ["InternalServerException"]` and justify it with a comment asserting that the error is "Declared on the SERVICE rather than on this operation". That assertion contradicts the model this crate is generated from: the service's only common error is `ValidationException`, and these three operations declare only `ConflictException`, `ResourceNotFoundException` and `ValidationException`. So the store-unavailable path in each handler answers HTTP 500 with an error shape no client is told these operations can return, on the strength of a claim about the model that the model itself refutes. (Every other operation in the crate that returns `InternalServerException` genuinely declares it — e.g. `RegisterApplication`, `GetApplication`, `ListDatabases` — which is why only these three need the `extra_errors` escape hatch.)

## Evidence

Comment asserting the error is service-declared — services/service-com-amazonaws-ssmsap-ssmsap/src/ops/tag_resource.rs:87-100:
```
#[smithy_macros::operation_error(
    "com.amazonaws.ssmsap#TagResource",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum TagResourceErrorModeled {
    ConflictException(ConflictException),
    ResourceNotFoundException(ResourceNotFoundException),
    ValidationException(ValidationException),
    /// Declared on the SERVICE rather than on this operation: the store this emulator
    /// keeps its registrations and their tags in could not be reached, which is a
    /// server fault and not something the caller sent wrong.
    InternalServerException(InternalServerException),
}
```
The model refutes it — `smithy_query(query="service_info")` on services/service-com-amazonaws-ssmsap-ssmsap/smithy-model/com-amazonaws-ssmsap-ssmsap.json:
```
"sigv4Name": "ssm-sap", ... "common_errors": ["ValidationException"]
```
and the denormalized surface, services/service-com-amazonaws-ssmsap-ssmsap/OPERATIONS.md:214:
```
- **Errors:** `ConflictException (HTTP 409)`, `ResourceNotFoundException (HTTP 404)`, `ValidationException (HTTP 400)`
```
(same for `UntagResource`, OPERATIONS.md:222, and `ListTagsForResource`, OPERATIONS.md:165).

The error is reachable from the handler body — services/service-com-amazonaws-ssmsap-ssmsap/src/ops/tag_resource.rs:157-159:
```
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            TagResourceErrorModeled::InternalServerException(crate::state::store_unavailable())
        })?;
```
The same comment and the same `extra_errors` widening appear verbatim at src/ops/untag_resource.rs:89-102 and src/ops/list_tags_for_resource.rs:88-101.

Test coverage: tests/all/tag_resource.rs, tests/all/untag_resource.rs and tests/all/resource_tags.rs exercise the tagging operations' modelled refusals; none asserts a 500, so nothing pins the undeclared variant.

## Steps to Reproduce

1. Run `smithy_query(query="service_info", model_path="services/service-com-amazonaws-ssmsap-ssmsap/smithy-model/com-amazonaws-ssmsap-ssmsap.json")` and note `"common_errors": ["ValidationException"]` — `InternalServerException` is not a service-level error.
2. Run `smithy_query(query="operation", operation="TagResource,UntagResource,ListTagsForResource")` and note the error lists contain only ConflictException, ResourceNotFoundException and ValidationException.
3. Open services/service-com-amazonaws-ssmsap-ssmsap/src/ops/tag_resource.rs:87-100 and read the `extra_errors = ["InternalServerException"]` widening together with the comment claiming it is "Declared on the SERVICE rather than on this operation".
4. Observe that tag_resource.rs:157-159 returns that variant, so `TagResource` can answer HTTP 500 `InternalServerException` — a shape the model declares neither on the operation nor on the service.
