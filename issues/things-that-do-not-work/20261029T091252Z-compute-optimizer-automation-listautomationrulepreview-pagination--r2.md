Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 574d6c6b58670e36287ce665233caf9a4430399b

# The preview page token still digests the RAW recommendedActionTypes and organizationScope.accountIds lists, so a permuted spelling of one selection cannot continue its own walk

_Filed: 2026-10-29T09:12:52Z_
_Operation: ListAutomationRulePreview | Class: pagination_

> _Note: this issue was identified against the code base at commit `574d6c6b58670e36287ce665233caf9a4430399b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`preview_selection_fingerprint` is the single helper both preview operations bind into their page tokens, and it states its own rule: what it digests is "the selection as [`previewed_actions`] PERFORMS it", so that "[t]wo spellings of one selection therefore digest to one fingerprint and continue one walk ... while a selection that really is different digests differently".

The selection `previewed_actions` performs reads BOTH list-valued members by MEMBERSHIP only: an action is in scope when `account_in_scope` finds its account anywhere in the scope list, and it is selected when `recommended_action_types.contains(action_type)`. Neither the ORDER of the entries nor a repeated entry changes the selected set, and the order a page walks is the store's record order (`retrievable_actions`), not the request's. So `recommendedActionTypes: ["UpgradeEbsVolumeType", "SnapshotAndDeleteUnattachedEbsVolume"]` and `["SnapshotAndDeleteUnattachedEbsVolume", "UpgradeEbsVolumeType"]` are one selection over one sequence, as are `accountIds: ["A","B"]` and `["B","A"]`, and as is any list that repeats an entry.

The helper nevertheless hands those two members to `page_fingerprint` exactly as the request spelled them. Only the `criteria` member beside them is canonicalised first (`canonical_criteria`), and the scope is only derived far enough to collapse "no scope" with "a scope carrying no accountIds" (`preview_account_scope`). A caller that takes a `nextToken` on page 1 and re-sends the same logical request with its action types (or its scope account ids) in a different order — the normal outcome when the list is held in a set, or re-serialised by a client, console or CLI shell loop — is refused 400 `InvalidParameterValueException` "Member must be a token returned for the same rule type, action types and criteria" for a token that is a correct window onto exactly the sequence it is being presented to.

This is the remaining half of the normalisation the crate already decided it wants: the criteria member was brought in line (fixed issue 20261028T220821Z, "Preview page token digests the RAW optional criteria member instead of the defaulted selection"), while the two membership-read list members next to it were left raw. The fix is in the one helper, so both `ListAutomationRulePreview` and `ListAutomationRulePreviewSummaries` get it at once; no response member and no existing assertion changes, because only the opaque token's digest input is normalised.

## Evidence

The rule the helper states for itself — `src/state.rs:1522-1534`:
```
/// What it digests is the selection as [`previewed_actions`] PERFORMS it: the
/// accounts the rule type and `organizationScope` derive to
/// ([`preview_account_scope`]), and the criteria after defaulting and with every
/// criterion that constrains nothing spelled one way ([`canonical_criteria`]) —
/// never the raw optional request members. Two spellings of one selection
/// therefore digest to one fingerprint and continue one walk (`criteria` absent,
/// `criteria: {}` and a criterion holding no condition all constrain nothing; a
/// scope carrying no `accountIds` covers the same accounts as no scope at all),
/// while a selection that really is different digests differently.
```

The code under it, which canonicalises the criteria and digests the other two members raw — `src/state.rs:1535-1545`:
```
pub fn preview_selection_fingerprint(
    operation: &str,
    scope: Option<&[String]>,
    recommended_action_types: &[String],
    criteria: &Criteria,
) -> String {
    page_fingerprint(
        operation,
        &(scope, recommended_action_types, canonical_criteria(criteria)),
    )
}
```

That the selection reads both lists by MEMBERSHIP, so order and repeats cannot change it — `src/state.rs:1462-1473`:
```
        .filter(move |action| {
            action
                .account_id
                .as_deref()
                .is_some_and(|account_id| account_in_scope(account_id, scope))
        })
        .filter(move |action| {
            action
                .recommended_action_type
                .as_ref()
                .is_some_and(|action_type| recommended_action_types.contains(action_type))
        })
```
and `src/state.rs:2260-2262`:
```
pub fn account_in_scope(account_id: &str, scope: Option<&[String]>) -> bool {
    scope.is_none_or(|accounts| accounts.iter().any(|candidate| candidate == account_id))
}
```
while the scope handed to the fingerprint is the request's list verbatim — `src/state.rs:2246-2256`:
```
pub fn preview_account_scope(
    rule_type: &str,
    organization_scope: Option<&OrganizationScope>,
) -> Option<Vec<String>> {
    if rule_type == "OrganizationRule" {
        return organization_scope
            .and_then(|scope| scope.account_ids.clone())
            .filter(|accounts| !accounts.is_empty());
    }
    Some(vec![caller_account_id()])
}
```

The refusal a mismatching digest produces — `src/ops/list_automation_rule_preview.rs:226-251` (and the identical block at `src/ops/list_automation_rule_preview_summaries.rs:231-256`):
```
        let selection_fingerprint = crate::state::preview_selection_fingerprint(
            Self::NAME,
            scope.as_deref(),
            &input.recommended_action_types,
            &criteria,
        );
...
                if cursor.filter_fingerprint != selection_fingerprint {
                    return Err(invalid_parameter(crate::state::constraint_report(
                        "nextToken",
                        "Member must be a token returned for the same rule type, action types and \
                         criteria",
                    )));
                }
```

Test coverage: the suite pins the rule in the general form this finding uses, and its table of equivalent spellings contains no permuted list — `tests/all/preview_page_token_selection.rs:244-249`:
```
/// Two spellings of ONE selection, as `(case, one spelling, the other)`.
///
/// Every row names requests whose derived selection is identical — the accounts
/// covered, the requested action types and the conditions applied are the same —
/// so a token taken under either spelling is a window onto the same sequence.
fn equivalent_spellings() -> Vec<(&'static str, Value, Value)> {
```
The six rows cover only `criteria` spellings and scope presence (`:251-296`); the `different_selections` table's only action-type row is a genuinely smaller list — `tests/all/preview_page_token_selection.rs:303-307`:
```
        (
            "fewer action types",
            account_preview(json!({})),
            account_preview(json!({ "recommendedActionTypes": [UPGRADE] })),
        ),
```
No test anywhere re-sends a token under a reordered or repeated `recommendedActionTypes` / `organizationScope.accountIds`, so the behaviour reported here is pinned by nothing.

## Steps to Reproduce

1. Mount the emulator and call UpdateEnrollmentConfiguration with {"status": "Active"} so the caller is past the enrollment gate and its analysed inventory is recorded.
2. Call ListAutomationRulePreview with {"ruleType": "AccountRule", "recommendedActionTypes": ["UpgradeEbsVolumeType", "SnapshotAndDeleteUnattachedEbsVolume"], "maxResults": 1} — 200 with one previewResult and a nextToken.
3. Call ListAutomationRulePreview again with the SAME selection spelled with the two action types swapped: {"ruleType": "AccountRule", "recommendedActionTypes": ["SnapshotAndDeleteUnattachedEbsVolume", "UpgradeEbsVolumeType"], "maxResults": 1, "nextToken": "<token from step 2>"}.
4. Observe 400 InvalidParameterValueException "1 validation error detected: Value at 'nextToken' failed to satisfy constraint: Member must be a token returned for the same rule type, action types and criteria", although `previewed_actions` selects the identical actions in the identical store order for both spellings (src/state.rs:1468-1473).
5. Repeat with a repeated entry instead of a permutation ({"recommendedActionTypes": ["UpgradeEbsVolumeType", "UpgradeEbsVolumeType", "SnapshotAndDeleteUnattachedEbsVolume"]}) — same refusal.
6. Repeat for an organization-scoped walk: take a token under {"ruleType": "OrganizationRule", "organizationScope": {"accountIds": ["<A>", "<B>"]}, ...} (both accounts associated via AssociateAccounts) and present it with accountIds spelled ["<B>", "<A>"] — same refusal, via the raw scope list at src/state.rs:2251-2253.
7. Contrast the criteria member, which the same helper normalises so the walk continues: a token taken with no `criteria` is served for `"criteria": {}` (src/state.rs:1557-1567, pinned by tests/all/preview_page_token_selection.rs:251-255).
