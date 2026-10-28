Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 804429c0e5267423598a5b7dbc8d0525cc046b43

# ListGraphs refuses an unissued nextToken with the synthetic framework ValidationException, dropping the modelled `reason` its four sibling listings report

_Filed: 2026-10-28T02:35:37Z_
_Operation: ListGraphs | Class: error-response_

> _Note: this issue was identified against the code base at commit `804429c0e5267423598a5b7dbc8d0525cc046b43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All five implemented listings of this crate refuse a `nextToken` that this service did not issue, with the same message text. Four of them return the service's MODELLED `ValidationException`, which carries a `reason` member, and set it to `ILLEGAL_ARGUMENT`. `ListGraphs` instead returns `crate::scaffolded::Scaffolded::validation(..)`, which serializes the SYNTHETIC `smithy.framework#ValidationException` shape whose schema declares `message` and nothing else. So one of the five answers an error body that is missing the modelled `reason` member for the identical refusal condition.

The crate's own `scaffolded` module documents that this is the wrong channel when the service models its own `ValidationException`, and names the exact mechanism to use instead (`#[operation_error]`'s `extra_errors`) — a mechanism three other handlers in this crate already use to widen in a service shape their operation does not list. `ListGraphs`' own comment claims the scaffold channel "renders the same 400 `ValidationException` the boundary validator does", which is true of the boundary comparison but is not what its four siblings render for this very condition.

## Evidence

The odd one out — services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/list_graphs.rs:158-168:

    // A token this service did not issue cannot name a page. `ListGraphs` models no
    // client-input error, so the refusal goes through the scaffold channel, which
    // renders the same 400 `ValidationException` the boundary validator does.
    let start_after = match input.next_token.as_deref() {
        Some(token) => Some(decode_page_token(token).ok_or_else(|| {
            crate::scaffolded::Scaffolded::validation(format!(
                "nextToken '{token}' is not a pagination token this service issued"
            ))
        })?),

Its error enum does not list ValidationException — list_graphs.rs:90-96:

    #[smithy_macros::operation_error("com.amazonaws.neptunegraph#ListGraphs")]
    pub enum ListGraphsErrorModeled {
        InternalServerException(InternalServerException),
        ResourceNotFoundException(ResourceNotFoundException),
        ThrottlingException(ThrottlingException),
    }

The four siblings, same condition, same sentence, MODELLED shape with `reason` — src/ops/list_graph_snapshots.rs:167-177:

    Some(token) => Some(decode_page_token(token).ok_or_else(|| {
        ListGraphSnapshotsErrorModeled::ValidationException(ValidationException {
            message: format!(
                "nextToken '{token}' is not a pagination token this service issued"
            ),
            reason: Some("ILLEGAL_ARGUMENT".to_string()),
        })
    })?),

identically at src/ops/list_import_tasks.rs:166-176, src/ops/list_export_tasks.rs:155-165 and src/ops/list_private_graph_endpoints.rs:171-181.

The synthetic shape the scaffold channel serializes carries ONLY `message` — src/scaffolded.rs:136-144:

    pub static VALIDATION_EXCEPTION_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static(
            "smithy.framework#ValidationException",
            "smithy.framework",
            "ValidationException",
        ),
        ShapeType::Structure,
        &[&VALIDATION_EXCEPTION_MESSAGE],
    );

while the modelled shape carries two members — `smithy_query(query="shape",shape="ValidationException")`:

    "members": {"message": {"target": "String", "required": true}, "reason": {"target": "ValidationExceptionReason", "type": "enum"}}

and `ValidationExceptionReason` includes the value the siblings send: `["CONSTRAINT_VIOLATION", "ILLEGAL_ARGUMENT", "MALFORMED_QUERY", "QUERY_CANCELLED", "QUERY_TOO_LARGE", "UNSUPPORTED_OPERATION", "BAD_REQUEST"]`.

The crate's own instruction not to use the scaffold channel here — src/scaffolded.rs:215-220:

    /// If this service's model declares its own `ValidationException` (or another
    /// client-input error) use THAT: the modeled shape may carry members beyond
    /// `message` — a `reason`, a field list — and a real client reads them. When the
    /// operation already lists it, it is a plain variant on the generated error enum;
    /// `#[operation_error]`'s `extra_errors` is only for widening in a shape the
    /// SERVICE models that this OPERATION does not list.

…and the mechanism already in use in this crate for exactly that case — src/ops/start_import_task.rs:141-144:

    #[smithy_macros::operation_error(
        "com.amazonaws.neptunegraph#StartImportTask",
        extra_errors = ["AccessDeniedException"]
    )]

Test coverage: tests/all/graph_lifecycle.rs:558-568 asserts only the 400 status and the `ValidationException` error code for an unissued ListGraphs token, so switching to the modelled shape leaves that test green; tests/all/graph_snapshot_listing.rs, import_task_listing.rs, export_task_listing.rs and private_graph_endpoint_listing.rs cover the four siblings' modelled refusal.

## Steps to Reproduce

1. Start the emulator and call `GET /graphs?nextToken=zzz` — observe HTTP 400 with `__type: ValidationException` and a body carrying `message` only.
2. Call `GET /snapshots?nextToken=zzz` (and `/importtasks`, `/exporttasks`, `/graphs/{id}/endpoints/` with the same token) — observe HTTP 400 with the same `message` sentence PLUS `"reason": "ILLEGAL_ARGUMENT"`.
3. Compare src/ops/list_graphs.rs:158-168 (uses `crate::scaffolded::Scaffolded::validation`) with src/ops/list_graph_snapshots.rs:167-177, list_import_tasks.rs:166-176, list_export_tasks.rs:155-165 and list_private_graph_endpoints.rs:171-181 (all use the modelled `ValidationException { message, reason }`).
4. Read src/scaffolded.rs:136-144 (the synthetic schema declares `message` only) and src/scaffolded.rs:215-220 (the module's own instruction to prefer the modelled shape, naming `extra_errors`), then src/ops/start_import_task.rs:141-144 where this crate already uses `extra_errors` to widen in a service shape the operation does not list.
