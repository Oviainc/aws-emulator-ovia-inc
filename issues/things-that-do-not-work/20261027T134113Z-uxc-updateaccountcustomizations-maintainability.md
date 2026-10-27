Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 9c3b4064a1735206e464a3fb30963f5d1ed2eddb

# Front-door helpers are hand-rolled copies of `service-common`'s own (`unanchored`, `binds_to_body`, the refusal code/message)

_Filed: 2026-10-27T13:41:13Z_
_Operation: UpdateAccountCustomizations | Class: maintainability_

> _Note: this issue was identified against the code base at commit `9c3b4064a1735206e464a3fb30963f5d1ed2eddb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`src/service.rs` re-implements, inside this crate, three spans that the `service-common` crate (already in Cargo.lock) publishes as its single staged home, so the copies can drift from the shared rule:

1. `fn unanchored` (service.rs:130) is a byte-for-byte behavioural copy of `service_common::validation::unanchored` (services/common/src/validation.rs:620). The crate's own comment admits this is an inlined copy pending migration.
2. `fn binds_to_body` + `fn carried_in_the_body` (service.rs:540, service.rs:558) duplicate `service_common::front_door::binds_to_body` (services/common/src/front_door.rs:239) — same doc prose, same five `http_*` trait checks, same first-segment placement of a nested violation path. The only textual difference is the unreachable fallback of `split('.').next()` (`unwrap_or(violation_path)` here vs `unwrap_or_default()` there), which cannot differ because `split` always yields one item.
3. The front door's code and message constants `BAD_REQUEST_EXCEPTION` (service.rs:148) and `INVALID_REQUEST_BODY` (service.rs:161) duplicate `service_common::front_door::BAD_REQUEST_EXCEPTION` (front_door.rs:65) and `INVALID_REQUEST_BODY` (front_door.rs:73), values included.

SCOPE OF THE FIX — it must change no response: adopt the shared `unanchored`, `binds_to_body` and the two constants only. Do NOT swap this crate's `FrontDoor`/`read_request` for `service_common::front_door::FrontDoor`: the shared wrapper deliberately leaves codec failures to the framework, whereas this crate deliberately answers an unreadable body with `BadRequestException` (a previously fixed issue), and the shared wrapper has no channel for this crate's URI-bound modelled `ValidationException` + `fieldList` or for `BodyMembersSentAsNull`. Those stay local.

## Evidence

This crate admits the duplication itself — services/service-com-amazonaws-uxc-awsaccountuxsetting/src/service.rs:125-129:

    /// THE RULE'S STAGED HOME is `services/common`'s `validation::unanchored`, which states
    /// it once for every service and is table-tested there; this crate carries the inlined
    /// copy its ~26 peers carry (`grep -rln 'fn unanchored' services/*/src`), to be deleted
    /// by the change that owns that migration. Kept byte-identical in behaviour to that one,
    /// and pinned end-to-end by `tests/all/pattern_refusal_rendering.rs`.
    fn unanchored(regex: &str) -> String {            <- service.rs:130

and the shared home exists — services/common/src/validation.rs:620:

    pub fn unanchored(regex: &str) -> String {

The binding reader is duplicated too. services/service-com-amazonaws-uxc-awsaccountuxsetting/src/service.rs:540-564:

    fn binds_to_body(input_schema: &Schema, violation_path: &str) -> bool {
        let member_name = violation_path.split('.').next().unwrap_or(violation_path);
        input_schema.members().iter().any(|member| {
            member.member_name().is_some_and(|name| path::member(name) == member_name)
                && carried_in_the_body(member)
        })
    }
    ...
    fn carried_in_the_body(member: &Schema) -> bool {
        member.http_label().is_none() && member.http_query().is_none()
            && member.http_query_params().is_none() && member.http_header().is_none()
            && member.http_prefix_headers().is_none()
    }

versus services/common/src/front_door.rs:239-253:

    pub fn binds_to_body(input_schema: &Schema, violation_path: &str) -> bool {
        let member_name = violation_path.split('.').next().unwrap_or_default();
        input_schema.members().iter().any(|member| {
            member.member_name().is_some_and(|name| path::member(name) == member_name)
                && member.http_label().is_none() && member.http_query().is_none()
                && member.http_query_params().is_none() && member.http_header().is_none()
                && member.http_prefix_headers().is_none()
        })
    }

And the constants. service.rs:148 `const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";` and service.rs:161 `const INVALID_REQUEST_BODY: &str = "Invalid request body";` against services/common/src/front_door.rs:65 `pub const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";` and front_door.rs:73 `pub const INVALID_REQUEST_BODY: &str = "Invalid request body";`.

The replacement crate is already a workspace package in Cargo.lock:15775 `name = "service-common"` (services/common/Cargo.toml:2 `name = "service-common"`), whose own manifest documents these modules: services/common/Cargo.toml:16-26 "`front_door`: the operation wrapper, the schema it reads a member's HTTP binding off ... `validation`: ... the `Violation` / constraint-path vocabulary". This crate's Cargo.toml (lines 9-23) does not list it.

The behaviour the fix must preserve is already pinned by this crate's own suites: tests/all/pattern_refusal_rendering.rs (the unanchored `@pattern` rendering), tests/all/one_constraint_refusal_path.rs:184 `one_front_door_refuses_every_modelled_member_constraint` (body vs URI placement, i.e. `binds_to_body`), tests/all/body_validation_refusals.rs:85 and tests/all/body_boundary_refusal.rs:28 (the `BadRequestException` code in header and `__type`, and the member-free message).

Why NOT to migrate the wrapper itself — services/common/src/front_door.rs:188-191: "Any OTHER codec failure (a body that is not parseable at all, a member of the wrong type) is not a constraint violation, so it is left to the framework's own walk", while this crate's front door owns exactly that case (src/service.rs:66-72 "a document that is not well-formed JSON, and a member whose JSON type is not the type the model gives it ... both earn the same `BadRequestException` answer"), pinned by tests/all/unreadable_body_refusal.rs.

## Steps to Reproduce

1. Open services/service-com-amazonaws-uxc-awsaccountuxsetting/src/service.rs and read lines 125-141 (`unanchored`), 540-564 (`binds_to_body` / `carried_in_the_body`) and the constants at 148 and 161.
2. Open services/common/src/validation.rs:620 and services/common/src/front_door.rs:65, 73, 239-253 and compare: same logic, same doc prose, same constant values.
3. Confirm the replacement crate is available: `grep -n 'name = "service-common"' Cargo.lock` -> line 15775; note services/service-com-amazonaws-uxc-awsaccountuxsetting/Cargo.toml lists no `service-common` dependency.
4. Note the drift risk: a change to the shared `unanchored` (e.g. its escaped-`$` handling) or to the shared binding rule leaves this crate on the old copy, silently diverging from the ~26 peers that share it, even though tests/all/pattern_refusal_rendering.rs claims the copy is "byte-identical in behaviour to that one".
