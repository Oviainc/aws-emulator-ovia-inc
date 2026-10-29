Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f0a30a453d4da98b65a93cfb3097ca05ddb2711e

# PostAgentProfile parses the caller's profile document while holding the store lock, the one thing both sibling profile readers are written not to do

_Filed: 2026-10-28T17:42:56Z_
_Operation: PostAgentProfile | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f0a30a453d4da98b65a93cfb3097ca05ddb2711e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

PostAgentProfile takes the service's state lock and then, still holding it, parses the submitted `agentProfile` blob: `not_a_profile(&input.agent_profile)` runs `super::get_profile::submitted_document(bytes)`, i.e. a full `serde_json` parse plus a recursive Ion document read (`DOCUMENT_NESTING_LIMIT` = 128 container levels) over bytes whose size and shape the caller chose. The read also runs inline on the async worker serving the request, not on the blocking pool.

The two other handlers that read a profile document in this crate do the opposite, and each says why in a comment: they copy the profile out of the store, DROP the guard, and only then read the document — and they do that read inside `tokio::task::spawn_blocking` so the async worker is not held either. So one `PostAgentProfile` of a large (or deeply nested) but perfectly well-formed profile serialises every other operation of this service — `DescribeProfilingGroup`, `ListProfilingGroups`, `GetProfile`, every tagging call — behind the same mutex, which is exactly the cost the sibling comments say must not be paid.

The ordering the handler documents ("The group the request addresses is resolved first ... so a name that names none is answered before the submitted data is looked at") is preserved by resolving the group under the lock, releasing it, parsing, and re-taking the lock to store, so no response changes.

## Evidence

The defect — src/ops/post_agent_profile.rs:288-315 (lock taken at 288, caller-sized parse at 308, still under the guard):
```
        let mut guard = state.lock();
        // The group the request addresses is resolved first: ...
        if !guard.groups.contains_key(&input.profiling_group_name) {
...
        // Submitted profiling data is PARSED before anything is stored, ...
        if let Some(refused) = not_a_profile(&input.agent_profile) {
```
and src/ops/post_agent_profile.rs:147-153, which is what runs under that guard:
```
fn not_a_profile(bytes: &[u8]) -> Option<NotAProfile> {
    if let Some(ion_type) = scalar_root_type(bytes) { ... }
    let Some(document) = super::get_profile::submitted_document(bytes) else {
```

Sibling 1 — src/ops/get_profile.rs:718-723:
```
        // The stored profile is taken out of the store and the guard released before the
        // document is read or written: re-encoding or bounding a document a caller posted
        // is work whose size that caller chose, and holding the store's lock across it
        // would make one profile's size every other caller's latency.
        let stored = {
            let guard = state.lock();
```
and src/ops/get_profile.rs:751-762 ("so it runs on the blocking pool instead of on the async worker serving this request" ... `::tokio::task::spawn_blocking(move || served_profile(...)).await`).

Sibling 2 — src/ops/batch_get_frame_metric_data.rs:603-608:
```
        // The profiles of the series are taken out of the store and the guard released
        // before any document is read: reading a document a caller posted is work whose
        // size that caller chose, and holding the store's lock across it would make one
        // profile's size every other caller's latency.
        let stored: Vec<crate::state::AgentProfile> = {
            let guard = state.lock();
```
and :653-676, which wraps the same document reading in `spawn_blocking` under a `tokio::time::timeout`.

Test coverage that must stay green and does not pin the lock discipline: tests/all/posted_profile_validation.rs::only_a_struct_stating_its_start_is_an_agent_profile, ::deeply_nested_data_is_refused_without_descending_into_it, ::a_refused_submission_consumes_no_idempotency_token and ::the_group_is_resolved_before_the_data_is_parsed (the last one pins the 404-before-400 ordering that the fix must keep).

## Steps to Reproduce

1. Open src/ops/post_agent_profile.rs:288 — the handler takes `state.lock()` into `guard`.
2. Follow to line 308: `not_a_profile(&input.agent_profile)` is called with `guard` still alive, and at line 151 it calls `super::get_profile::submitted_document(bytes)`, which runs a `serde_json` parse and a recursive Ion read over the caller's blob.
3. Compare with src/ops/get_profile.rs:718-723 and src/ops/batch_get_frame_metric_data.rs:603-608, where the same document reading is done only AFTER the guard is dropped, with the comment stating that holding the lock across it 'would make one profile's size every other caller's latency' — and with get_profile.rs:760 / batch_get_frame_metric_data.rs:672, where it is additionally moved onto `spawn_blocking`.
4. Post a large well-formed JSON profile (a struct carrying `start` plus megabytes of nested frames) to POST /profilingGroups/<name>/agentProfile and issue any other request of this service concurrently: the second request cannot take the store lock until the first request's parse finishes.
