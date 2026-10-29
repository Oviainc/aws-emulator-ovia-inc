Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 40b622ed07403ef350d596bb19a34a2d990a98a6

# Preview page token digests the RAW optional criteria member instead of the defaulted selection, so a query cannot resume a walk over the identical result set

_Filed: 2026-10-28T22:08:21Z_
_Operation: ListAutomationRulePreview | Class: pagination_

> _Note: this issue was identified against the code base at commit `40b622ed07403ef350d596bb19a34a2d990a98a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both preview handlers compute the page-token fingerprint from the raw `Option<Criteria>` the request carried (`input.criteria.as_ref()`), but they perform the selection over the DEFAULTED value (`input.criteria.unwrap_or_default()`). `criteria` absent and `criteria: {}` therefore select byte-identical sequences — both become `Criteria::default()`, every member `None`, which constrains nothing — while digesting to two different fingerprints. A token minted for one of those two spellings is refused for the other with `InvalidParameterValueException` "Member must be a token returned for the same rule type, action types and criteria", even though the sequence the page is a window onto is exactly the same.

That contradicts the helper's own documented rule: `preview_selection_fingerprint` says it "digests exactly the members `previewed_actions` selects by", and `previewed_actions` is handed the defaulted criteria, not the raw member. It also diverges from every other paginated listing of this crate, which normalises its optional query member with `unwrap_or_default()` BEFORE digesting it, so absent and empty spell the same token: `ListAutomationRules` (`let filters = input.filters.unwrap_or_default();` … `fingerprint(&filters)`), `ListAutomationEvents` and `ListAutomationEventSummaries` all do. The two preview operations are the only ones that digest an un-normalised `Option`.

`ListAutomationRulePreviewSummaries` carries the identical defect (same two lines, same helper), so the root cause is shared; `ListAutomationRulePreview` is named here because it is the operation that pages the preview itself.

## Evidence

The rule the fingerprint helper states for itself — `src/state.rs:1499-1521`:
```
/// The fingerprint a preview page token carries: the operation that issued it
/// and the whole selection it was taken with.
///
/// Bound into the token ([`PageCursor::filter_fingerprint`]) so that resuming a
/// walk with a different rule type, scope, action-type list or criteria — or
/// with the other preview operation's token — is refused instead of answered
/// with a page of a different result set. It digests exactly the members
/// [`previewed_actions`] selects by, so the two preview operations cannot
/// disagree about which requests continue one walk.
pub fn preview_selection_fingerprint(
    operation: &str,
    rule_type: &str,
    organization_scope: Option<&OrganizationScope>,
    recommended_action_types: &[String],
    criteria: Option<&Criteria>,
) -> String {
```

What the handler actually digests — `src/ops/list_automation_rule_preview.rs:213-219`:
```
        let selection_fingerprint = crate::state::preview_selection_fingerprint(
            Self::NAME,
            &input.rule_type,
            input.organization_scope.as_ref(),
            &input.recommended_action_types,
            input.criteria.as_ref(),
        );
```
and what it actually selects by — `src/ops/list_automation_rule_preview.rs:252-283`:
```
        // A caller that sends no criteria constrains nothing, so every action in
        // scope carrying a requested action type is previewed.
        let criteria = input.criteria.unwrap_or_default();
...
            crate::state::previewed_actions(
                &guard,
                scope.as_deref(),
                &input.recommended_action_types,
                &criteria,
            )
```
(the sibling is identical: `src/ops/list_automation_rule_preview_summaries.rs:218-224` and `:259`, `:279-286`.)

The refusal that follows a mismatch — `src/ops/list_automation_rule_preview.rs:230-236`:
```
                if cursor.filter_fingerprint != selection_fingerprint {
                    return Err(invalid_parameter(crate::state::constraint_report(
                        "nextToken",
                        "Member must be a token returned for the same rule type, action types and \
                         criteria",
                    )));
                }
```

The sibling listings that DO normalise before digesting — `src/ops/list_automation_rules.rs:167` and `:180`:
```
        let filters = input.filters.unwrap_or_default();
...
        let filter_fingerprint = crate::state::fingerprint(&filters);
```
and `src/ops/list_automation_events.rs:170` and `:201-205`:
```
        let filters = input.filters.unwrap_or_default();
...
        let query_fingerprint = crate::state::fingerprint(&(
            &filters,
            input.start_time_inclusive.map(crate::state::instant_key),
            input.end_time_exclusive.map(crate::state::instant_key),
        ));
```

That an empty `criteria` object reaches the handler unrefused is settled by `src/state.rs:1496` (`criteria.and_then(criteria_violation)`), and `criteria_violation` reports nothing for a `Criteria` carrying no condition (`src/state.rs:2481-2586`).

Test coverage: `tests/all/page_token_refusals.rs:133-185` drives every paginated operation with the preview request `{"ruleType":"AccountRule","recommendedActionTypes":["UpgradeEbsVolumeType"]}` — always with `criteria` ABSENT — and `tests/all/shared_page_walk.rs` walks pages with one fixed body, so no test ever presents the same selection in its two spellings and the refusal reported here is pinned by nothing.

## Steps to Reproduce

1. Mount the emulator and call UpdateEnrollmentConfiguration with {"status": "Active"} so the enrollment gate is open and the account's recommended-action inventory exists.
2. Call ListAutomationRulePreview with {"ruleType": "AccountRule", "recommendedActionTypes": ["UpgradeEbsVolumeType"], "maxResults": 1} and keep the nextToken it returns.
3. Call ListAutomationRulePreview again with the SAME body plus "criteria": {} and that nextToken: the selection is identical (an empty criteria object constrains nothing, exactly as an absent one does), yet the call is refused 400 InvalidParameterValueException "1 validation error detected: Value at 'nextToken' failed to satisfy constraint: Member must be a token returned for the same rule type, action types and criteria".
4. Do it the other way round — take page 1 with "criteria": {} present and resume with it omitted — and the same refusal appears.
5. Compare with ListAutomationRules: take page 1 with {"maxResults": 1} and resume with {"maxResults": 1, "filters": [], "nextToken": <token>} — accepted, because that handler digests the filters AFTER unwrap_or_default (src/ops/list_automation_rules.rs:167,180).
