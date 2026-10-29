Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: af767dd3eb1ad507e674c736ee4881cbe95a30bf

# ListInvestigationGroups refuses a forged nextToken with the synthetic framework ValidationException instead of the service's modelled one

_Filed: 2026-10-28T14:51:30Z_
_Operation: ListInvestigationGroups | Class: error-response_

> _Note: this issue was identified against the code base at commit `af767dd3eb1ad507e674c736ee4881cbe95a30bf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The `nextToken` refusal in `ListInvestigationGroups` is returned through `crate::scaffolded::Scaffolded::validation`, i.e. the synthetic `smithy.framework#ValidationException` shape that lives outside this service's model. The crate's own error-channel documentation states, in as many words, that this wrapper variant exists only for a service whose model carries NO client-input error, and that when the service models its own `ValidationException` the handler must return THAT shape — widening the operation's generated error enum with `#[operation_error]`'s `extra_errors` when the operation itself does not list it. This service does model `ValidationException` (`com.amazonaws.aiops#ValidationException`, @error client / @httpError 400) and the model attaches it to `ListInvestigationGroups` as a service-common error, so the documented path applies exactly here. Every sibling handler in this crate that refuses a bad input does it the modelled way (create/update's configuration chain, PutInvestigationGroupPolicy's policy-document check), so this one refusal is the only one in the crate answered from an out-of-model shape. The rendered status (400) and wire code ("ValidationException") happen to coincide today because the modelled shape carries only `message`, so there is no behavioural impact — the defect is that the operation answers a shape its own model does not declare, which is the condition the crate documents as not allowed, and which silently diverges the moment the modelled shape grows a member or the rendering of a synthetic variant differs (the wrapper's own doc already notes the synthetic variants carry no `awsQueryError` and omit the error-type header the boundary path sets).

## Evidence

The refusal, using the out-of-model shape — `services/service-com-amazonaws-aiops-aiops/src/ops/list_investigation_groups.rs:148-159`:
```
fn decode_page_token(token: &str) -> Result<String, ListInvestigationGroupsError> {
    hex::decode(token)
        .ok()
        ...
        .ok_or_else(|| {
            crate::scaffolded::Scaffolded::validation(crate::state::validation_message(
                "nextToken",
                NEXT_TOKEN_REQUIREMENT,
            ))
        })
}
```
The crate's rule, stated twice in its own error channel — `services/service-com-amazonaws-aiops-aiops/src/scaffolded.rs:39-42`:
```
//! An implemented operation stops needing the 501, but the wrapper is also the only
//! place it can return an error its own model does not declare — `validation` above,
//! for a service whose model carries no client-input error. Delete the wrapper and the
//! handler has no way to refuse a bad request at all.
```
and `services/service-com-amazonaws-aiops-aiops/src/scaffolded.rs:354-359`:
```
    /// If this service's model declares its own `ValidationException` (or another
    /// client-input error) use THAT: the modeled shape may carry members beyond
    /// `message` — a `reason`, a field list — and a real client reads them. When the
    /// operation already lists it, it is a plain variant on the generated error enum;
    /// `#[operation_error]`'s `extra_errors` is only for widening in a shape the
    /// SERVICE models that this OPERATION does not list.
```
The model does declare it, and binds it to this operation — `smithy_query(query="service_info")`: `"common_errors": ["AccessDeniedException", "ConflictException", "ForbiddenException", "InternalServerException", "ResourceNotFoundException", "ThrottlingException", "ValidationException"]`; `smithy_query(query="operation",operation="ListInvestigationGroups")` errors include `{"shape": "ValidationException", "errorKind": "client", "httpError": 400}`; `smithy_query(query="shape",shape="ValidationException")`: `"shape_id": "com.amazonaws.aiops#ValidationException", "errorKind": "client", "httpError": 400`.
The generated enum that was left un-widened — `src/ops/list_investigation_groups.rs:89-95`:
```
#[smithy_macros::operation_error("com.amazonaws.aiops#ListInvestigationGroups")]
#[derive(Debug)]
pub enum ListInvestigationGroupsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
}
```
How every sibling refusal in this crate is answered instead — `src/ops/put_investigation_group_policy.rs:173-180`:
```
        if let Some(why) = crate::state::policy_document_rejection(&input.policy) {
            return Err(
                PutInvestigationGroupPolicyErrorModeled::ValidationException(ValidationException {
                    message: Some(why),
                })
                .into(),
            );
```
Test coverage: `tests/all/investigation_groups.rs:623-639` pins the forged-token refusal as HTTP 400 with `error_code(&body) == "ValidationException"` — which the modelled shape renders identically, so returning the modelled error leaves this test (and every response byte) unchanged.

## Steps to Reproduce

1. Create an investigation group so the listing has state (POST /investigationGroups with a name and roleArn)
2. GET /investigationGroups?nextToken=not-a-token — the emulator answers 400 with error code ValidationException
3. Observe in src/ops/list_investigation_groups.rs:153-158 that the error value is `crate::scaffolded::Scaffolded::validation(..)`, i.e. the synthetic `smithy.framework#ValidationException` shape, not `com.amazonaws.aiops#ValidationException`
4. Compare with src/scaffolded.rs:39-42 and :354-359, which state that variant is only for a service whose model carries no client-input error and that a modelled ValidationException must be used (via `extra_errors` when the operation does not list it), and with `smithy_query(query="service_info")`, which lists ValidationException among this service's common errors
