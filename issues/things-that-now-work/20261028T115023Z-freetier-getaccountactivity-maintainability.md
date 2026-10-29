Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a

# The LanguageCode primary-subtag computation is written twice in this crate — once in state.rs and once, byte-identically, in the suite that pins it

_Filed: 2026-10-28T11:50:23Z_
_Operation: GetAccountActivity | Class: maintainability_

> _Note: this issue was identified against the code base at commit `0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::primary_subtag` is the computation that decides when two `LanguageCode` values name the SAME language, and it is what `LocalizedText::in_language` uses to resolve a code the catalog holds no translation for (state.rs:130-136). Both activity readers — `GetAccountActivity` (get_account_activity.rs:219-220) and `ListAccountActivities` (list_account_activities.rs:229) — reach it through `in_language`.

The same function is defined a second time, character for character, as a private helper inside `tests/all/activity_language_code.rs`, and that copy is what decides which pairs of modelled codes the suite requires to agree and which it requires to differ (`if primary_subtag(code) == primary_subtag(other) { continue; }`, line 256, and the `languages` set at line 272). So the rule the handlers implement is pinned against a private reimplementation of that same rule rather than against the rule itself: if the production notion of "same language" is ever changed in `state.rs` (a different separator, case folding, a longer tag), the test's copy keeps the old grouping and the suite goes on asserting a grouping the handlers no longer use — it can pass while the two sides disagree, or fail for a reason that is only the stale copy.

There is exactly one other place in the crate that needs this computation and exactly one place that states it, so the duplication is a straight copy, not two different rules that happen to coincide. The deduplication is mechanical: `state::primary_subtag` is currently a private `fn` (state.rs:77), so making it reachable and calling it from the suite removes the second definition. No handler branch, no response body and no assertion changes — the suite's rows, expectations and messages stay exactly as they are, because the copies compute the same thing today.

## Evidence

Copy 1 — the production rule, `services/service-com-amazonaws-freetier-awsfreetierservice/src/state.rs:74-81`:
```
/// The primary language subtag of a `LanguageCode` — `en` of `en-GB` — which is
/// what makes a code with no stored translation resolve to another variant of
/// the same language rather than straight to the base text.
fn primary_subtag(language_code: &str) -> &str {
    language_code
        .split_once('-')
        .map_or(language_code, |(primary, _)| primary)
}
```
Its call sites, `state.rs:130-136` (inside `LocalizedText::in_language`, which both activity readers call):
```
        let wanted = primary_subtag(code);
        if wanted == primary_subtag(DEFAULT_LANGUAGE_CODE) {
            return &self.base;
        }
        self.translations
            .iter()
            .find(|(stored, _)| primary_subtag(stored) == wanted)
```

Copy 2 — the byte-identical duplicate, `services/service-com-amazonaws-freetier-awsfreetierservice/tests/all/activity_language_code.rs:158-164`:
```
/// The primary language subtag of a `LanguageCode` — `en` of `en-GB` — which is
/// what makes two codes name the same language.
fn primary_subtag(language_code: &str) -> &str {
    language_code
        .split_once('-')
        .map_or(language_code, |(primary, _)| primary)
}
```
and where that duplicate decides the expectation, `activity_language_code.rs:251-273`:
```
        for (code, rendering) in &rendered {
            for (other, other_rendering) in &rendered {
                if code >= other {
                    continue;
                }
                if primary_subtag(code) == primary_subtag(other) {
                    continue;
                }
                assert_ne!(
```
```
        let languages: BTreeSet<&str> = LANGUAGE_CODE_VALUES
            .iter()
            .map(|code| primary_subtag(code))
            .collect();
```

A grep over the crate finds exactly these two definitions and no third:
```
tests/all/activity_language_code.rs:160: fn primary_subtag(language_code: &str) -> &str {
src/state.rs:77: fn primary_subtag(language_code: &str) -> &str {
```

Test coverage of both sides, which must stay green and unchanged: `tests/all/activity_language_code.rs:173` `detail_text_is_reported_in_the_requested_language` (uses the duplicate to group the 13 modelled codes), `tests/all/shared_rules.rs:176` `both_activity_readers_check_the_language_code_by_one_rule`, and `tests/all/free_tier.rs:814` `activity_text_is_reported_in_the_requested_language` — all of which drive the readers over the wire and assert the reported `title`/`description` per code.

## Steps to Reproduce

1. Read `services/service-com-amazonaws-freetier-awsfreetierservice/src/state.rs:77-81` — the crate's statement of "which primary subtag a LanguageCode names", used by `LocalizedText::in_language` (state.rs:130-136) and so by both `GetAccountActivity` and `ListAccountActivities`.
2. Read `services/service-com-amazonaws-freetier-awsfreetierservice/tests/all/activity_language_code.rs:160-164` — the same function body again, privately redefined.
3. Observe that the duplicate is what the suite uses at `activity_language_code.rs:256` and `:272` to decide which code pairs must report identical text (`en-GB` vs `en-US`) and which must differ, i.e. the production rule is checked against a copy of itself.
4. Change the production rule in `state.rs` only — e.g. make the subtag split case-insensitive or split on `_` as well — and re-run `cargo test -p service-com-amazonaws-freetier-awsfreetierservice`: the suite still groups codes by the OLD rule, because its own copy was not touched, so the two definitions have drifted with nothing reporting it.
5. Confirm the fix is response-neutral: `state::primary_subtag` is private (`fn`, state.rs:77); exposing it and calling it from the suite in place of the local copy leaves every request, every response body and every assertion in the three listed tests identical.
