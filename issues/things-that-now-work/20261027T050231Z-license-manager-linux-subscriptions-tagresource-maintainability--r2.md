Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 53db56bf3158c7205a85c81c618ec0b3c4193a64

# Collapse the two private copies of the test harness onto tests/all/harness.rs

_Filed: 2026-10-27T05:02:31Z_
_Operation: TagResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `53db56bf3158c7205a85c81c618ec0b3c4193a64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate has ONE shared wire harness for its integration suites — `tests/all/harness.rs`, declared as `mod harness;` in the single test target — yet two suites in that SAME binary carry their own private re-implementation of it: `tests/all/resource_tags.rs` (TagResource / ListTagsForResource) and `tests/all/linux_subscriptions.rs` (the two discovery lists). Each copy re-states the same logic: build the emulator (`InMemoryEngine` + `MultiServiceRouter` + `set_request_authentication(false)` + `register_service`), build the signed request and dispatch it, extract the wire error code with the `x-amzn-errortype` header -> `__type` body fallback, percent-encode the `resourceArn` `@httpLabel`, and seed a provider record directly through `engine.state::<…>()`.

The copies have ALREADY drifted, which is what makes this a defect rather than taste: `resource_tags::encode_label` encodes only `/` and `:`, while `harness::percent_encode` encodes everything outside the RFC 3986 unreserved set; and `resource_tags::register` derives the seeded ARN's id from `guard.providers.len() + 1` while `harness::seed_provider` takes the id from the caller. So the same rule ("what an SDK client's request looks like on the wire") is exercised two different ways depending on which suite a case lands in, and a fix to one copy silently leaves the others behind.

The newest suites already take the shared one — `tests/all/tag_removal.rs` says so in its own header — so the shared harness is the intended single implementation; the two private copies are the leftovers. Pointing both suites at `crate::harness` changes no request, no response and no assertion: the suites keep their own tables, their own constants and their own expectations.

## Evidence

Shared harness (the intended single copy), `tests/all/harness.rs:3`:
```
//! The shared wire harness for this crate's suites: one emulator, one request builder,
//! and one reply assertion vocabulary, so no suite grows its own.
```
`tests/all/harness.rs:83-98`:
```rust
fn build_harness(with_secrets_manager: bool) -> Harness {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
```
Second copy, `tests/all/resource_tags.rs:79-92`:
```rust
struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn harness() -> Harness {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
```
Third copy, `tests/all/linux_subscriptions.rs:174-187`: `struct Harness { router: MultiServiceRouter, engine: Arc<engine::InMemoryEngine> }` / `fn harness() -> Harness { … router.set_request_authentication(false); router.register_service(EmulatedService::new(Arc::clone(&engine))); … }`.

The error-code extraction, repeated verbatim in all three — `tests/all/harness.rs:223-227`, `tests/all/resource_tags.rs:165-169`, `tests/all/linux_subscriptions.rs:294-298`:
```rust
    let error_type = header_error.or_else(|| {
        body.get("__type")
            .and_then(Value::as_str)
            .map(str::to_string)
    });
```
The drift, `tests/all/harness.rs:324-335`:
```rust
pub fn percent_encode(value: &str) -> String {
    … b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => …
        other => encoded.push_str(&format!("%{other:02X}")),
```
versus `tests/all/resource_tags.rs:180-189`:
```rust
fn encode_label(value: &str) -> String {
    value.chars().map(|c| match c {
        '/' => "%2F".to_string(),
        ':' => "%3A".to_string(),
        other => other.to_string(),
```
and the seeding drift, `tests/all/resource_tags.rs:228-232` (`providers.len() + 1`) versus `tests/all/harness.rs:382-408` (`seed_provider(h, id, …)`).

All three live in ONE test binary — `tests/all.rs:16-25`:
```rust
#[path = "all/harness.rs"]
mod harness;
#[path = "all/linux_subscriptions.rs"]
mod linux_subscriptions;
…
#[path = "all/resource_tags.rs"]
mod resource_tags;
```
and a sibling suite already uses the shared one, `tests/all/tag_removal.rs:6`:
```
//! Every case drives a real request through the shared harness's `dispatch`, …
```
Tests covering the duplicated code: the `resource_tags.rs` tag-merge/read cases, the `linux_subscriptions.rs` discovery-list cases, and every suite already on `harness.rs` (`shared_tag_merge.rs::both_tag_merge_paths_obey_the_one_resultant_set_bound`, `tag_removal.rs`, `subscription_providers.rs`, `provider_status_transition.rs`, `provider_credential_validation.rs`, `error_taxonomy.rs`).

## Steps to Reproduce

1. Open `services/service-com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions/tests/all.rs` and note that `harness`, `resource_tags` and `linux_subscriptions` are modules of the SAME test target, so `crate::harness` is in scope for both suites.
2. `grep -n '^\(struct\|fn\|async fn\|pub fn\|pub async fn\) \(Harness\|harness\|Reply\|call\|send\|build_harness\)' tests/all/*.rs` -> three independent `struct Harness` / `fn harness()` / `struct Reply` / `call()` definitions (harness.rs, resource_tags.rs, linux_subscriptions.rs).
3. Compare `harness::percent_encode` (tests/all/harness.rs:324) with `resource_tags::encode_label` (tests/all/resource_tags.rs:180): the same `@httpLabel` encoding rule, implemented over two different character sets.
4. Compare `harness::seed_provider` (tests/all/harness.rs:382) with `resource_tags::register` (tests/all/resource_tags.rs:225): the same direct-seeding routine, with two different ARN-id rules.
5. Point `resource_tags.rs` and `linux_subscriptions.rs` at `crate::harness` and re-run `cargo test -p service-com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions`: every existing assertion and every emitted response must stay identical.
