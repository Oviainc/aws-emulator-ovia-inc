Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: d1b341c944550181b7ac630538402367c666ba11

# The three lookahead @patterns are stated twice: as the modelled expression and as a hand-rolled character grammar

_Filed: 2026-10-29T09:53:57Z_
_Operation: CreateDashboard | Class: maintainability_

> _Note: this issue was identified against the code base at commit `d1b341c944550181b7ac630538402367c666ba11`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`src/state.rs` enforces the three `@pattern`s the boundary validator cannot apply (`DashboardName`/`ScheduledReportName`, `Description`, `WidgetTitle` — each opens with the negative lookahead `(?!.* {2})`) by HAND: `ConstrainedText::admits`, `admits_bounded_run`, `is_word_character` and `is_description_character` re-state each expression a second time as Rust character classes and quantifiers, beside the expression constants (`RESOURCE_NAME_PATTERN`, `DESCRIPTION_PATTERN`, `WIDGET_TITLE_PATTERN`) that the refusal itself quotes. One rule, two statements, in one crate: a model revision to any of the three constants leaves the hand-written grammar still judging the old expression, and the refusal would then advertise a pattern the service does not apply.

`fancy-regex` — which compiles the look-around the `regex` crate the boundary uses cannot — is ALREADY in Cargo.lock, and three sibling service crates use it for precisely this job. Compiling each constant once (e.g. a `OnceLock<fancy_regex::Regex>` per `ConstrainedText` variant) and matching against it would leave one statement of each rule.

Responses and tests stay unchanged: the refusal text is built from the same constants through `text_pattern_refusal`, and the answers are identical on every input the model admits — all three expressions' character classes exclude the newline, so the one semantic difference between a hand-rolled scan and a regex (`.` not matching `\n` inside the lookahead) cannot change a verdict: a value containing a newline is refused either way.

## Evidence

The hand-rolled statement and its own reason for existing (src/state.rs:1557-1567):
```
/// A constrained string of this service whose modelled `@pattern` the boundary validator
/// does NOT apply, so the request reaches a handler unchecked against it.
/// Each of these expressions opens with the negative lookahead `(?!.* {2})` — no two
/// consecutive spaces anywhere in the value — and the boundary's regex engine compiles no
/// lookahead; a pattern it cannot compile is treated as matching ... So the three grammars are
/// applied HERE
```
The second statement of each rule (src/state.rs:1645-1674):
```
    pub fn admits(self, value: &str) -> bool {
        if value.contains("  ") { return false; }
        match self {
            // `[a-zA-Z][a-zA-Z0-9 _-]{0,48}[a-zA-Z0-9_-]`.
            Self::ResourceName => admits_bounded_run(value, |character| character.is_ascii_alphabetic(), ... Some(48)),
            Self::Description => value.chars().all(is_description_character),
            Self::WidgetTitle => admits_bounded_run(value, is_word_character, ... None),
```
with `admits_bounded_run` at src/state.rs:1613-1632 and the classes at src/state.rs:1580-1607 — while the FIRST statement, the expression the refusal publishes, sits at src/state.rs:1545, :1550 and :1554-1555 and is read by `text_pattern_refusal` (src/state.rs:1690-1704) through `ConstrainedText::pattern` (src/state.rs:1636-1642).

The crate that replaces it is already a dependency of this workspace — Cargo.lock:2784 `name = "fancy-regex"` — and a sibling service applies the identical rule with it: services/service-com-amazonaws-vpclattice-mercurycontrolplane/src/state.rs:1072-1077:
```
/// The pattern itself is evaluated (with `fancy-regex`, which supports the look-around
/// the patterns are written with), so the refusal and the constraint it reports can
/// never drift apart. It has to happen here: the boundary validator compiles a
/// `@pattern` with the `regex` crate, which has no look-around, and treats a pattern it
/// cannot compile as matching
```
with the one-per-kind compilation cache at that file's lines 1107-1119.

Callers (one shared helper, four handlers): src/ops/create_dashboard.rs:189-195, src/ops/update_dashboard.rs:185-191, src/ops/create_scheduled_report.rs:210-216, src/ops/update_scheduled_report.rs:230-232.

Existing tests that pin the behaviour a refactor must not change: tests/all/name_pattern_enforcement.rs (header lines 3-33 — "for EVERY `@pattern` the model declares — including the ones whose expression carries a construct the boundary validator's regex engine cannot compile", with acceptance and refusal tables over all three shapes) and tests/all/validation_pattern_rendering.rs (the anchor-stripped rendering of the expression inside the refusal).

## Steps to Reproduce

1. Read src/state.rs:1545-1555 — the three modelled expressions as string constants, which the refusal quotes.
2. Read src/state.rs:1568-1674 — the same three expressions restated as Rust character classes and quantifiers (`ConstrainedText::admits`, `admits_bounded_run`, `is_word_character`, `is_description_character`).
3. Edit one constant (say drop ` _-` from RESOURCE_NAME_PATTERN) and observe that only the published text changes: `admits` still admits the old grammar, so the refusal advertises a requirement the service no longer applies.
4. Confirm the replacement is already available: `grep -n 'name = "fancy-regex"' Cargo.lock` and services/service-com-amazonaws-vpclattice-mercurycontrolplane/src/state.rs:1107-1119, which compiles a modelled `@pattern` with look-around once per kind.
5. Run `cargo test -p service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice` — tests/all/name_pattern_enforcement.rs and tests/all/validation_pattern_rendering.rs must pass byte-identically after the consolidation.
