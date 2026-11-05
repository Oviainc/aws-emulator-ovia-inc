Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 650142439453f256773dfdd9d373235adc353165

# state::validation_envelope hand-spells the AWS ValidationException envelope the framework already renders

_Filed: 2026-11-05T12:04:47Z_
_Operation: TagResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `650142439453f256773dfdd9d373235adc353165`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::validation_envelope` is the one place this crate wraps its handler-made refusal clauses in the AWS `ValidationException` envelope — every tag-key refusal (`map_key_rejection`, called from `tag_resource.rs:164`), every configuration refusal and every page-token refusal passes through it, and `service.rs`'s `override_refusal` renders boundary refusals through it as well.

Its body re-spells, as literals, the exact two sentences the framework's own `aws_smithy_schema_server::envelope` already produces: `"1 validation error detected: {clause}"` and `"{n} validation errors detected: "` joined by `"; "`. So the same rule — how many clauses there are, which count word to use, and what separates two clauses — exists twice: once in the vendored framework (which is what renders the SAME envelope on the path this module is deliberately kept in step with) and once in this crate's literals. They can drift: a framework that re-words the envelope would reword a boundary refusal while this crate kept answering the old sentence for the clauses a handler states itself, which is precisely the drift the module's own anti-drift rule for the OTHER half of the clause was written to prevent (`requirement_text` reads the requirement half out of the framework's rendering rather than spelling it, "so a constraint class the framework learns to check therefore cannot drift from this wording").

`service-common` — already a direct dependency of this crate (Cargo.toml:29) — exposes exactly this span as `validation_message::envelope_over(&[String])`, which derives the count word and the separator from the framework's own rendering instead of spelling them, and whose doc names this failure mode verbatim.

Fix shape: keep the `pub fn validation_envelope(&[String]) -> String` signature and API (tests import it) and delegate its body to the shared renderer. Every message stays byte-identical — the framework's literals and this crate's literals are the same text today — so no response and no existing test changes. The one behavioural difference is for the EMPTY slice (today `"0 validation errors detected: "`, a hollow message); no caller can reach it: `map_key_rejection` (state.rs:1021) and `cross_account_configurations_rejection` (state.rs:1295) guard on `!clauses.is_empty()`, `validation_message` always passes one clause, and `validation_exception_message` returns early for no violations (state.rs:817).

## Evidence

This crate's copy — services/service-com-amazonaws-aiops-aiops/src/state.rs:582-593:
```
/// The AWS `ValidationException` envelope over one or more clauses: how many
/// constraints were violated, then the clauses.
pub fn validation_envelope(clauses: &[String]) -> String {
    match clauses {
        [clause] => format!("1 validation error detected: {clause}"),
        many => format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        ),
    }
}
```

The framework's copy — vendor/smithy-schema-server/src/constraints.rs:1398-1416 (`aws-smithy-schema-server` is a direct dependency, Cargo.toml:13):
```
/// Render accumulated violations into the AWS `ValidationException` envelope.
/// `1 validation error detected: <clause>` / `N validation errors detected:
/// <clause>; <clause>`.
pub fn envelope(violations: &[Violation]) -> Option<String> {
    match violations.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", violations[0].message())),
        n => { ... Some(format!("{n} validation errors detected: {}", clauses.join("; "))) }
    }
}
```

The shared renderer that exists for exactly this case — services/common/src/validation_message.rs:1558-1584 (`service-common` is a direct dependency, services/service-com-amazonaws-aiops-aiops/Cargo.toml:29 `service-common = { path = "../common" }`):
```
/// The framework's envelope over clauses this module stated rather than over violations.
///
/// The count word and the separator are the framework's: they are read back out of
/// [`envelope`]'s own rendering ... so neither is spelled here and a grouped refusal
/// cannot drift from an ungrouped one.
///
/// `pub` for the same reason [`requirement`] is: a service whose clauses this module's
/// whole-refusal entry points cannot state words them itself and still has to put them in
/// AWS's envelope, and an envelope re-spelled per crate is how `1 validation error
/// detected: ` comes to differ between two services.
pub fn envelope_over(clauses: &[String]) -> Option<String> {
```

The same module's own stated anti-drift rule for the other half of the clause — state.rs:541-565:
```
/// Read off the FRAMEWORK's own rendering of `kind` ... A constraint class
/// the framework learns to check therefore cannot drift from this wording
pub fn requirement_text(kind: ViolationKind) -> String {
```

Caller reached from a handler — services/service-com-amazonaws-aiops-aiops/src/ops/tag_resource.rs:164:
```
        if let Some(why) = crate::state::map_key_rejection(Some(&input.tags), None) {
```
and state.rs:1021 `(!clauses.is_empty()).then(|| validation_envelope(&clauses))`.

Existing tests that pin the rendered messages and so cover the change: tests/all/modelled_map_key_constraints.rs:194,212,425 (the TagResource/create tag-key refusals, asserting `"1 validation error detected: ..."` and `"2 validation errors detected: ...; ..."`), tests/all/map_member_refusal_paths.rs:143,152, tests/all/error_message_rules.rs:179-188, tests/all/list_element_refusal_positions.rs:344 and tests/all/write_configuration_rejection_chain.rs:284 and tests/all/cross_account_configuration_names_an_account.rs:283 (these three call `state::validation_envelope` directly and keep working unchanged).

## Steps to Reproduce

1. Open services/service-com-amazonaws-aiops-aiops/src/state.rs:582-593 and read the two literal sentences `"1 validation error detected: "` / `"{n} validation errors detected: "` with the `"; "` join.
2. Open vendor/smithy-schema-server/src/constraints.rs:1398-1416 and read the identical two sentences in `envelope`, the renderer that serves the boundary-refusal path this crate's wording is kept in step with.
3. Open services/common/src/validation_message.rs:1558-1584 and read `envelope_over`, which derives those words from the framework instead of spelling them, and whose doc states that an envelope re-spelled per crate is how `1 validation error detected: ` comes to differ between services.
4. Confirm services/service-com-amazonaws-aiops-aiops/Cargo.toml:13 and :29 already depend on both crates, so no new dependency is needed.
5. Confirm state.rs:541-565 (`requirement_text`) reads the OTHER half of the same clause out of the framework precisely to avoid this drift — the envelope half is the one copy left spelled by hand.
