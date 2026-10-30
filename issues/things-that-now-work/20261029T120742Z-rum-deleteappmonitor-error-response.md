Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: f2dfb6e6c8bef3e3a2af1e117b6fc51f81eb645a

# DeleteAppMonitor's handler documents the post-delete PutRumEvents refusal as ResourceNotFoundException, but the sibling handler and the crate's own test answer AccessDeniedException

_Filed: 2026-10-29T12:07:42Z_
_Operation: DeleteAppMonitor | Class: error-response_

> _Note: this issue was identified against the code base at commit `f2dfb6e6c8bef3e3a2af1e117b6fc51f81eb645a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The `DeleteAppMonitor` handler states, as the rule its own write establishes, that after the record is removed "a later `GetAppMonitor` by name and a later `PutRumEvents` with the old id both answer `ResourceNotFoundException`". Only the first half is true of this crate. `PutRumEvents` deliberately answers an UNRESOLVABLE `Id` — including the id of a monitor this very delete removed — with `AccessDeniedException` (HTTP 403) carrying `CALLER_LACKS_PERMISSIONS_MESSAGE`, and says so in its own handler comment, and the crate has a dedicated test suite asserting that 403 for exactly the "deleted while other monitors remain" case.

So two handlers of the same crate state opposite error contracts for the same observable sequence (delete a monitor, then PutRumEvents with its old id), and the delete's statement is the one no code implements. The code and the test are the authoritative side here: the `PutRumEvents` comment gives the reason (authorizing against a resource that no longer exists leaves no grant to establish, and the refusal must disclose nothing about existence) and the test pins the 403 body byte-for-byte against a never-minted id. The defect is therefore the delete handler's claim, which is a stale contract a reader — or a later fixer taking it as the crate's rule — would use to change `PutRumEvents` and break `put_rum_events_unresolvable_id.rs`.

## Evidence

The claim, `services/service-com-amazonaws-rum-rum/src/ops/delete_app_monitor.rs:179-182`:
```
        // "This immediately stops the collection of data": the record goes — with its
        // tags and its event buffer, which live on it — and its id leaves the index, so
        // a later `GetAppMonitor` by name and a later `PutRumEvents` with the old id both
        // answer `ResourceNotFoundException`.
```

The sibling handler it is wrong about, `services/service-com-amazonaws-rum-rum/src/ops/put_rum_events.rs:176-182` (its own stated rule):
```
        // That is not reported as a missing
        // resource: this write is authorized AGAINST the monitor the id names (its ARN and
        // its resource-based policy are what the decision below is made from), so an id
        // that resolves to nothing leaves no resource to establish the grant against and
        // the call is refused as an authorization failure, disclosing nothing about
        // whether such a monitor exists (`crate::state::caller_lacks_permissions`).
```
and the code under it, `services/service-com-amazonaws-rum-rum/src/ops/put_rum_events.rs:189-194`:
```
            let Some(record) = store.app_monitor_by_id(&region, &input.id) else {
                return Err(PutRumEventsErrorModeled::AccessDeniedException(
                    crate::state::caller_lacks_permissions(),
                )
                .into());
            };
```
(repeated under the write lock at `put_rum_events.rs:261-266`).

The test that pins the sibling's behaviour for precisely the case the delete comment describes, `services/service-com-amazonaws-rum-rum/tests/all/put_rum_events_unresolvable_id.rs:11-17`:
```
//! The RULE the assertions pin, over a SET of inputs rather than one request:
//! `PutRumEvents` is authorized against the app monitor its `Id` names, so an `Id` that
//! resolves to no stored monitor leaves no resource for the grant to be established
//! against and the write is refused with `AccessDeniedException` (HTTP 403) ... the same
//! answer whether the id was never minted or named a monitor that has since been deleted,
```
and `:181` (`async fn a_deleted_monitors_id_is_refused_the_same_way_as_one_never_minted`):
```
    let after_delete = assert_refused(&router, "the deleted monitor's id", &doomed).await;
```
where `assert_refused` asserts `reply.status == 403` and `code == "AccessDeniedException"` (`:58-69`).

Test coverage of the contradiction itself: none — no test reads the delete handler's comment, so nothing keeps the two statements in step.

## Steps to Reproduce

1. Read `services/service-com-amazonaws-rum-rum/src/ops/delete_app_monitor.rs:179-182`: the handler states that after the delete, `PutRumEvents` with the removed monitor's old id answers `ResourceNotFoundException`.
2. CreateAppMonitor(Name="events-doomed", Domain="example.com") and note the minted `Id`.
3. POST /appmonitors/{Id}/ with a valid PutRumEvents body: HTTP 200.
4. DELETE /appmonitor/events-doomed: HTTP 200.
5. POST /appmonitors/{Id}/ again with the same body. Observed: HTTP 403 `AccessDeniedException` with message "Caller principal does not have permissions for that resource." — not the HTTP 404 `ResourceNotFoundException` the delete handler's comment says this sequence answers.
6. For contrast, GET /appmonitor/events-doomed: HTTP 404 `ResourceNotFoundException`, so only the `PutRumEvents` half of the comment's claim is false.
7. Confirm the crate pins the 403 on purpose: `cargo test -p service-com-amazonaws-rum-rum a_deleted_monitors_id_is_refused_the_same_way_as_one_never_minted` passes, so the handler comment is the side that diverges from the crate.
