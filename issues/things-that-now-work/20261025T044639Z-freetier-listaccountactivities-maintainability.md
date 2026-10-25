Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 4244fec3063c58aa4a2476d7619962eecf74c002

# The page-cut/next-token rule is written out twice, once per paginated handler, while its sibling halves already live in crate::state

_Filed: 2026-10-25T04:46:39Z_
_Operation: ListAccountActivities | Class: maintainability_

> _Note: this issue was identified against the code base at commit `4244fec3063c58aa4a2476d7619962eecf74c002`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate already owns the pagination rule in ONE place for two of its three parts: `crate::state` holds `page_size` (resolve `maxResults` to a page size) and `encode_page_token`/`decode_page_token` (the token codec, kind-scoped so one listing's token is refused by the other). The third part of the same rule — cut the selected rows to the page, and issue a token carrying `position + 1` only when a selected row is still unreported — is NOT in `state`: it is copy-pasted into both paginated handlers, character for character apart from the surrounding comment. Because the two copies are independent, a later change to how a page is cut or when a token is issued (for instance issuing a token on the exact-fit boundary, or changing the offset the token carries) lands in one listing and not the other, and `ListAccountActivities` and `GetFreeTierUsage` would then page by two different rules even though they share the token codec and the page-size helper.

A second, smaller copy of the same kind sits in the two activity readers: the `languageCode` membership check against `LANGUAGE_CODE_VALUES` and its refusal message are written out identically in `get_account_activity.rs` and `list_account_activities.rs`, while the sibling per-row language resolution it guards (`LocalizedText::in_language`) is stated once in `state.rs`.

The fix is a pure hoist into `crate::state`, beside `page_size`/`encode_page_token`: one helper that takes the kind, the selected `(position, item)` rows and the page size and returns the page plus the optional token, and one helper for the language-code check. Every response byte and every existing test stays unchanged.

## Evidence

Copy 1 — services/service-com-amazonaws-freetier-awsfreetierservice/src/ops/list_account_activities.rs:242-250:
```rust
        let page = &selected[..page_size.min(selected.len())];
        // Issued only when a row the filter selects is still unreported: the
        // `@paginated` contract is that an absent token ends the iteration.
        let next_token = match page.last() {
            Some((position, _)) if selected.len() > page.len() => {
                Some(crate::state::encode_page_token(PAGE_KIND, position + 1))
            }
            _ => None,
        };
```

Copy 2 — services/service-com-amazonaws-freetier-awsfreetierservice/src/ops/get_free_tier_usage.rs:199-206:
```rust
        let page = &selected[..page_size.min(selected.len())];
        // Issued only when a record the filter selects is still unreported.
        let next_token = match page.last() {
            Some((position, _)) if selected.len() > page.len() => {
                Some(crate::state::encode_page_token(PAGE_KIND, position + 1))
            }
            _ => None,
        };
```

The halves of the SAME rule that are already single-sourced, in services/service-com-amazonaws-freetier-awsfreetierservice/src/state.rs:459-488:
```rust
/// Encode the position a page stopped at — the offset of the next unreported
/// row — into an opaque `nextToken`.
...
pub fn encode_page_token(kind: &str, offset: usize) -> String {
...
/// The page size one request asks for: its `maxResults`, or the modelled
/// default when the request carried none.
pub fn page_size(max_results: i32) -> usize {
```

Second duplicated rule — services/service-com-amazonaws-freetier-awsfreetierservice/src/ops/list_account_activities.rs:188-196:
```rust
        let language_code = input.language_code.as_deref();
        if let Some(language_code) = language_code {
            if !LANGUAGE_CODE_VALUES.contains(&language_code) {
                return Err(validation(format!(
                    "languageCode: {language_code} is not a language code; expected one of {}",
                    LANGUAGE_CODE_VALUES.join(", ")
                )));
            }
        }
```
and the identical block in services/service-com-amazonaws-freetier-awsfreetierservice/src/ops/get_account_activity.rs:192-200 (same condition, same message, same `LANGUAGE_CODE_VALUES.join(", ")` tail), whose own comment names the other copy: "// As on `ListAccountActivities`: the code selects the language of the" (get_account_activity.rs:188).

Test coverage that pins the behaviour the hoist must not change: services/service-com-amazonaws-freetier-awsfreetierservice/tests/all/free_tier.rs:134 `walk` ("holds exactly `page_size` items, the last page carries no `nextToken`, and the walk terminates"), :539 `activity_pages_reassemble_the_catalog_at_every_page_size`, :1199 `usage_pages_reassemble_the_records_at_every_page_size`, :1256 `an_absent_max_results_uses_the_modelled_default`, :595 `an_unissued_activity_token_is_refused`, :1287 `an_unissued_usage_token_is_refused`, :731 `activity_readers_check_the_language_code`, and tests/all/activity_language_code.rs:291 `an_unmodelled_language_code_is_refused_and_changes_nothing`.

## Steps to Reproduce

1. Open services/service-com-amazonaws-freetier-awsfreetierservice/src/ops/list_account_activities.rs and read lines 242-250.
2. Open services/service-com-amazonaws-freetier-awsfreetierservice/src/ops/get_free_tier_usage.rs and read lines 199-206: the same page-cut and next-token expression, differing only in the `PAGE_KIND` constant and the comment above it.
3. Open services/service-com-amazonaws-freetier-awsfreetierservice/src/state.rs lines 459-488 and note that the token codec and the page-size resolution of the same rule are already single-sourced there, so only the page-cut/token-issue third is duplicated.
4. Compare services/service-com-amazonaws-freetier-awsfreetierservice/src/ops/get_account_activity.rs:192-200 with list_account_activities.rs:188-196 for the second duplicated rule (the languageCode membership check and its message).
5. Change one copy only (for example make the token carry a different offset in `GetFreeTierUsage`) and observe that the activity listing keeps the old rule: nothing in the crate forces the two to agree.
