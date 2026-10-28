Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4

# The accountId/onboardingGeneration recorded-member projection is built twice (create_domain.rs and get_domain.rs) although state.rs states every response projection lives there

_Filed: 2026-10-27T23:07:18Z_
_Operation: CreateDomain | Class: maintainability_

> _Note: this issue was identified against the code base at commit `c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`src/state.rs` states the crate's own rule for shared projections: helpers several operations share — "every response projection" among them — live in `state.rs` "rather than being copied into each `src/ops/*.rs`". The recorded-only `accountId` / `onboardingGeneration` pair is projected out of the same `Domain` record by two handlers, and the projection is written out twice, verbatim: the same two members, in the same order, read from the same two record fields with the same `as_wire().to_string()` conversion.

The two copies can drift: a change to how `onboarding_generation` is rendered, or the addition of a third recorded member, has to be made in two files, and nothing fails if only one is edited — the create response and the read response would then describe one stored record differently, which is exactly what `rendered()`'s own doc comment says the single projection exists to prevent.

The per-file member SCHEMAS must stay per-file (each names its own output shape and member index), but the member LIST is one computation and belongs beside the other `Domain` projections in `state.rs` (`Domain::encryption_context`, `WebAppConfigurationRecord::projected`, `Subscription::description`). A helper in `state.rs` taking the two schemas and the record returns the identical `Vec<RecordedMember>`, so every response byte and every existing test stays unchanged.

## Evidence

The rule the crate states (services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs:11-14):
```
//! Helpers several operations share (id minting, ARN building, the opaque pagination
//! cursor, and every response projection — `SubscriptionDescription`,
//! `WebAppConfiguration`, `EncryptionContext`) live here rather than being copied
//! into each `src/ops/*.rs`
```
Copy 1 (services/.../src/ops/create_domain.rs:172-178):
```
        members: vec![
            crate::state::RecordedMember::Text(&ACCOUNT_ID_MEMBER, stored.account_id.clone()),
            crate::state::RecordedMember::Text(
                &ONBOARDING_GENERATION_MEMBER,
                stored.onboarding_generation.as_wire().to_string(),
            ),
        ],
```
Copy 2 (services/.../src/ops/get_domain.rs:232-238):
```
            members: vec![
                crate::state::RecordedMember::Text(&ACCOUNT_ID_MEMBER, domain.account_id.clone()),
                crate::state::RecordedMember::Text(
                    &ONBOARDING_GENERATION_MEMBER,
                    domain.onboarding_generation.as_wire().to_string(),
                ),
            ],
```
get_domain.rs:76-77 already says this projection is `CreateDomain`'s: "`accountId` and `onboardingGeneration` — projected out of the stored record through [`crate::state::RecordedMembers`], exactly as `CreateDomain` does."

Tests that pin the responses this refactor must leave byte-identical: tests/all/domain_response_projection.rs:151 `domain_responses_project_the_owning_account_and_the_onboarding_generation` (asserts the create response's accountId/onboardingGeneration and that two later GetDomain reads report the same pair, over six create shapes and six Regions) and tests/all/domain_web_app_identity.rs, tests/all/encryption_context_projection.rs for the neighbouring projections already single-homed in state.rs.

## Steps to Reproduce

1. Read services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs:11-14 for the stated rule that every shared response projection lives in state.rs.
2. Compare services/.../src/ops/create_domain.rs:172-178 with services/.../src/ops/get_domain.rs:232-238: the same two recorded members, same order, same conversions, built from the same `Domain` fields.
3. Note that `Domain::encryption_context` (state.rs:384), `WebAppConfigurationRecord::projected` (state.rs:292) and `Subscription::description` (state.rs:708) are the sibling projections that DO live in state.rs.
4. Run `cargo test -p service-com-amazonaws-connecthealth-connecthealth` and confirm domain_response_projection.rs covers both responses, so moving the member list into one helper is verifiable without changing any assertion.
