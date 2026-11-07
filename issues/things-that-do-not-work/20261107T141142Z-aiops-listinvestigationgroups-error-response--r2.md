Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 323c298260f641866db820e53cda367b812962dd

# A boundary refusal of the @sensitive nextToken is worded "Value null at ...", the subject this crate reserves for a member the request never carried

_Filed: 2026-11-07T14:11:42Z_
_Operation: ListInvestigationGroups | Class: error-response_

> _Note: this issue was identified against the code base at commit `323c298260f641866db820e53cda367b812962dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::violation_clause` decides between the two clause subjects this service words a `ValidationException` with by looking at `Violation::value` ALONE. The framework sets `value = None` in two unrelated cases: the member was absent, and the member is `@sensitive` (where the value is deliberately redacted but the member WAS present). `ListInvestigationGroups.nextToken` is the one `@sensitive` member of this model (`SensitiveStringWithLengthLimits`, `@length 0..2048`), so a request that carries a `nextToken` longer than 2048 characters is refused at the boundary with a clause whose subject is `Value null at 'nextToken'` — the crate's own wording for a member the request did not carry at all — glued to a LENGTH requirement that only a present value can violate.

The crate's own rules say the opposite in two places: `absent_member_clause`'s doc states the `Value null` subject means "there was no value: the request carried nothing at that path", and the crate's intent for every boundary refusal (state.rs:390-396) is to name the member and the requirement and merely WITHHOLD the value — which is exactly what `constraint_clause` produces (`Value at '<member>' failed to satisfy constraint: ...`), and also exactly what the framework itself renders for a sensitive violation (`Violation::message`, constraints.rs:169-174). So the sensitive case should take the `constraint_clause` branch; today it takes the absent-member branch and tells the caller its request omitted a member it in fact sent.

The fix is one predicate: treat `violation.sensitive` as "there WAS a value, it is withheld" rather than as "no value".

## Evidence

The branch, which reads only `value` — services/service-com-amazonaws-aiops-aiops/src/state.rs:800-805:
```
    let requirement = requirement_text(violation.kind.clone());
    match violation.value {
        // No value at that path: the member was absent from the request.
        None => absent_member_clause(&violation.path, &requirement),
        Some(_) => constraint_clause(&violation.path, &requirement),
    }
```

What the framework means by `value: None` — vendor/smithy-schema-server/src/constraints.rs:61-70:
```
    /// The offending value as displayed, or `None` when there was no value —
    /// i.e. the member was absent.
    ...
    pub value: Option<String>,
    /// The member is `@sensitive`: `value` is `None` and the message names only the path, so the
    /// value never reaches a response or an author's refusal override.
    pub sensitive: bool,
```
and the redaction that produces it — constraints.rs:864-872:
```
    /// Hides the value of every violation recorded since `from` when the member is `@sensitive`.
    fn redact_since(&mut self, from: usize, sensitive: bool) {
        if !sensitive { return; }
        for violation in &mut self.violations[from..] {
            violation.value = None;
            violation.sensitive = true;
```
(the `@sensitive` marker is captured onto the member's own trait map by codegen — vendor/smithy-macros/src/codegen.rs:887-890: `// "@sensitive" is a marker too, on the member or its target.` / `if traits.contains_key("smithy.api#sensitive") { c.sensitive = true; }`), while the framework's OWN rendering of such a violation keeps the present-value subject — constraints.rs:169-174:
```
        if self.sensitive {
            return format!(
                "Value at '{}' failed to satisfy constraint: {requirement}",
                self.path
            );
```

The rule this crate states for the `Value null` subject — state.rs:573-580:
```
/// [`constraint_clause`] for a member that was ABSENT from the request.
///
/// The subject is `Value null`, because there was no value: the request carried
/// nothing at that path, which is a fact about the request's SHAPE rather than a value
/// echoed back from it.
pub fn absent_member_clause(member_path: &str, requirement: &str) -> String {
    format!("Value null at '{member_path}'{CONSTRAINT_SEPARATOR}{requirement}")
```
and the path this reaches the wire through — src/service.rs:120-126:
```
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: crate::state::validation_exception_message(violations)?,
```

The member that reaches it (`smithy_query(query="operation",operation="ListInvestigationGroups")`):
```
"nextToken": {"target": "SensitiveStringWithLengthLimits", "httpQuery": "nextToken", "length": {"min": 0, "max": 2048}, "sensitive": true}
```

Test coverage: the crate's refusal table pins the two subjects as mutually exclusive — tests/all/error_message_rules.rs:168-176:
```
/// One clause of a validation message: the member and the requirement, no value.
fn clause(member_path: &str, requirement: &str) -> String {
    format!("Value at '{member_path}' failed to satisfy constraint: {requirement}")
}

/// One clause for a member the request did not carry at all.
fn absent_clause(member_path: &str) -> String {
    format!("Value null at '{member_path}' failed to satisfy constraint: Member must not be null")
}
```
Every row of that table uses `clause(..)` for a present-but-invalid member (e.g. :387-390 `maxResults=999`) and `absent_clause(..)` only for the omitted `policy` member (:426-433). NO row sends an over-long `nextToken`, so the one member whose violations are redacted is untested and falls into the absent-member branch unnoticed.

## Steps to Reproduce

1. Start the emulator and issue `GET /investigationGroups?nextToken=<2049 'a' characters>` signed for any Region (aiops endpoint).
2. The boundary walk refuses the request: the `nextToken` member carries `@length 0..2048` and `@sensitive`, so `redact_since` (vendor/smithy-schema-server/src/constraints.rs:864-872) clears `value` and sets `sensitive = true`.
3. `service.rs:120-126` hands the violations to `state::validation_exception_message`, which calls `violation_clause` (state.rs:784-806); because `value` is `None`, the clause is built by `absent_member_clause`.
4. Observe the 400 body message: `1 validation error detected: Value null at 'nextToken' failed to satisfy constraint: Member must have length less than or equal to 2048` — the `Value null` subject state.rs:573-580 reserves for a member the request carried nothing for, although the request carried 2049 characters there.
5. Compare with the framework's own rendering for a sensitive violation (constraints.rs:169-174, `Value at '<path>' failed to satisfy constraint: ...`) and with the crate's `clause(..)` grammar used for every other present-but-invalid member (tests/all/error_message_rules.rs:168-171).
