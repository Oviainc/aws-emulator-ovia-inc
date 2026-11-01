Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: c1d5331cbdbc8645ce17750a515f2b0101f76c93

# Single-home the "store unavailable on this engine" refusal the five handlers each rebuild

_Filed: 2026-10-29T11:38:56Z_
_Operation: ListTagsForResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `c1d5331cbdbc8645ce17750a515f2b0101f76c93`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every implemented handler in this crate opens with the same engine-store-unavailable decision, written out by hand five times: the `let Some(state) = crate::state::service_state(&*self.engine) else { … }` branch, the choice of the operation's `InternalServerException` variant, the `Some(..)` message wrapping, and the verbatim message text `"the data automation runtime store is unavailable on this engine"`.

That is one rule (what this service answers when the engine holds no store of its type) implemented five times, so the copies can drift: changing the shape, the wording, or the status for one operation leaves the other four answering differently for the same logical failure. The crate already rejects exactly this duplication for its other cross-operation refusal — `crate::state::unknown_invocation_refusal` is single-homed precisely so "the operations that resolve an ARN cannot come to disagree about what one naming nothing answers" (state.rs:458-461), and each handler reaches it through `store.resolve_invocation[_mut]`, passing only its own error-variant constructor. The store-unavailable refusal is the same shape of decision (one shared shape + one fixed message + one status, differing only in the per-operation variant constructor) and can be single-homed the same way, e.g. a `crate::state` helper taking `impl FnOnce(InternalServerException) -> E` the way `unknown_invocation_refusal` takes the not-found constructor.

The fix is response-preserving: the shape, message and status emitted on that branch stay byte-for-byte what they are today, and no existing test changes.

## Evidence

Five copies of the same refusal, one per implemented handler (identical branch, identical message literal):

- `src/ops/get_data_automation_status.rs:171-180`: "let Some(state) = crate::state::service_state(&*self.engine) else { return Err(GetDataAutomationStatusErrorModeled::InternalServerException( InternalServerException { message: Some( \"the data automation runtime store is unavailable on this engine\".into(), ), }, ) .into()); };"
- `src/ops/invoke_data_automation_async.rs:252-261`: same block with `InvokeDataAutomationAsyncErrorModeled::InternalServerException` and the same message literal ("the data automation runtime store is unavailable on this engine", line 256).
- `src/ops/tag_resource.rs:170-179`: same block, `TagResourceErrorModeled::InternalServerException`, message literal at line 174.
- `src/ops/list_tags_for_resource.rs:158-167`: same block, `ListTagsForResourceErrorModeled::InternalServerException`, message literal at line 162.
- `src/ops/untag_resource.rs:168-177`: same block, `UntagResourceErrorModeled::InternalServerException`, message literal at line 172.

The crate's own stated rule against exactly this duplication, for the sibling refusal it DOES single-home — `src/state.rs:449-468`:

"/// `not_found` is the calling operation's own `ResourceNotFoundException` variant — the
/// shape itself is the one generated struct every operation here shares, so the variant
/// constructor is all that differs between callers. EVERYTHING else about the refusal
/// lives here once: the shape, the message, and the STATUS DECISION … the operations that resolve an ARN
/// cannot come to disagree about what one naming nothing answers."

and the single call site shape each handler uses for it, e.g. `src/ops/list_tags_for_resource.rs:173-176`: "let invocation = store.resolve_invocation( &input.resource_arn, ListTagsForResourceErrorModeled::ResourceNotFoundException, )?;"

Test coverage that must stay unchanged (all of these drive every handler through the `Some(state)` arm over the wire): `tests/all/data_automation_invocations.rs` (`each_submission_is_persisted_and_reported_back_from_state`, `tag_and_untag_edit_the_stored_tag_map`, `tagging_past_the_modelled_tag_bound_is_refused_and_changes_nothing`, `every_arn_keyed_operation_refuses_unknown_and_malformed_arns`), `tests/all/asset_processing_configuration.rs` (`an_accepted_submission_retains_the_processing_scope_it_asked_for`), `tests/all/tag_key_refusal_list_bound.rs` (both tests). The refused arm is unreachable on the shared `engine::InMemoryEngine` (`crate::state::service_state`, state.rs:937-943, only answers `None` for another engine type), so no test pins its wording either — which is what lets the copies drift unnoticed.

## Steps to Reproduce

1. grep -n 'the data automation runtime store is unavailable on this engine' services/service-com-amazonaws-bedrockdataautomationruntime-amazonbedrockkeystoneruntimeservice/src/ops/*.rs — five hits, one per implemented handler (get_data_automation_status.rs:175, invoke_data_automation_async.rs:256, tag_resource.rs:174, list_tags_for_resource.rs:162, untag_resource.rs:172).
2. Read any two of those handlers side by side: the `let Some(state) = crate::state::service_state(&*self.engine) else { … }` branch, the `InternalServerException { message: Some(..) }` construction and the message literal are identical; only the per-operation error-variant constructor differs.
3. Compare with the sibling refusal the crate single-homes: `src/state.rs:462-468` (`unknown_invocation_refusal`) holds the shape, the message and the status once and takes the operation's variant constructor as an argument, and each handler reaches it through `store.resolve_invocation[_mut](.., <Op>ErrorModeled::ResourceNotFoundException)?`.
4. Change the store-unavailable wording in one handler only and run `cargo test -p service-com-amazonaws-bedrockdataautomationruntime-amazonbedrockkeystoneruntimeservice`: the suite still passes, because the branch is unreachable on the in-memory engine — nothing stops the five copies from diverging.
