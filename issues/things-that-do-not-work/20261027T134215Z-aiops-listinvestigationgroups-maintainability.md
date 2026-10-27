Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 9c3b4064a1735206e464a3fb30963f5d1ed2eddb

# ListInvestigationGroups re-implements the per-region reader and the group-id shape rule instead of calling the state helpers that own them

_Filed: 2026-10-27T13:42:15Z_
_Operation: ListInvestigationGroups | Class: maintainability_

> _Note: this issue was identified against the code base at commit `9c3b4064a1735206e464a3fb30963f5d1ed2eddb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two rules the state module single-homes are implemented a second time inside the `ListInvestigationGroups` handler, so the copies can drift from the originals.

1. The per-region read. `crate::state`'s module doc declares that the store is "read per region through [`ComAmazonawsAiopsAiopsState::groups_in_region`]", and `groups_in_region`'s own doc names this operation as its consumer ("this is what a regional reader (`ListInvestigationGroups`) reports"). The handler never calls it: it builds the same `group.region == region` filter inline over a `BTreeMap::range`, so the helper's only caller in the crate is `group_count_in_region` (the quota). Two copies of "which groups belong to this Region" now exist, one of them in the file whose doc claims to be the helper's reason for existing.

2. What a group id looks like. `state::group_id_from_arn` decides it as `id.len() == GROUP_ID_LEN && id.bytes().all(|b| b.is_ascii_alphanumeric())`; `decode_page_token` in the handler repeats that exact expression to decide whether a page token carries a group id. The rule comes from one modelled pattern segment (`[A-Za-z0-9]{16}`), so it should be judged in one place.

This is a maintainability finding only: both copies currently agree, so no response and no test changes if the handler reaches for the state helpers (a `groups_in_region`-style reader that can start at a token id, and a shared `is_group_id` predicate) instead of its own copies.

## Evidence

The doctrine, in `services/service-com-amazonaws-aiops-aiops/src/state.rs:18-21`:
```
//! A group belongs to the Region its ARN names, and an account holds at most
//! [`MAX_INVESTIGATION_GROUPS_PER_REGION`] of them there — so the store is keyed by
//! id and read per region through
//! [`ComAmazonawsAiopsAiopsState::groups_in_region`].
```
The helper and its stated consumer — `src/state.rs:253-265`:
```
    /// Every group this account holds in `region`, in the store's stable id order.
    ///
    /// A group belongs to exactly one Region — the one its ARN names — so this is
    /// what a regional reader (`ListInvestigationGroups`) reports and what the
    /// per-region quota counts.
    pub fn groups_in_region<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a InvestigationGroup> + 'a {
        self.investigation_groups
            .values()
            .filter(move |group| group.region == region)
    }
```
The second copy, in the operation the doc names — `src/ops/list_investigation_groups.rs:217-222`:
```
        let store = state.lock();
        let mut remaining = match &start {
            Some(id) => store.investigation_groups.range(id.clone()..),
            None => store.investigation_groups.range::<String, _>(..),
        }
        .filter(|(_, group)| group.region == region);
```
`grep -n 'groups_in_region'` over the crate shows the helper has exactly one call site, the quota — `src/state.rs:269`: `self.groups_in_region(region).count()` — and none in `src/ops/list_investigation_groups.rs`.

The group-id shape rule, first copy — `src/state.rs:1025` (in `group_id_from_arn`):
```
    (id.len() == GROUP_ID_LEN && id.bytes().all(|b| b.is_ascii_alphanumeric())).then_some(id)
```
second copy — `src/ops/list_investigation_groups.rs:148-150` (in `decode_page_token`):
```
        .filter(|id| {
            id.len() == crate::state::GROUP_ID_LEN && id.bytes().all(|b| b.is_ascii_alphanumeric())
        })
```
Test coverage that must stay unchanged: `tests/all/investigation_groups.rs:552` `list_reports_exactly_the_groups_of_the_region_it_is_signed_for` (whose body also pins the token refusal at `:623` — "A token this operation never issued is refused, not silently ignored."), `tests/all/regional_identifier_resolution.rs` for the region scoping of identifier resolution, and `tests/all/investigation_group_singleton.rs` for the per-region quota that calls `group_count_in_region`.

## Steps to Reproduce

1. Read `services/service-com-amazonaws-aiops-aiops/src/state.rs:18-21` and `:253-265`: the module doc and the helper doc both state that the per-region read of the store happens in `groups_in_region`, naming `ListInvestigationGroups` as the reader
2. grep the crate for `groups_in_region`: the only call site is `src/state.rs:269` (`group_count_in_region`); `src/ops/list_investigation_groups.rs` contains none
3. Read `src/ops/list_investigation_groups.rs:217-222`: the handler re-implements the `group.region == region` filter inline
4. Compare `src/state.rs:1025` with `src/ops/list_investigation_groups.rs:148-150`: the `len() == GROUP_ID_LEN && bytes().all(is_ascii_alphanumeric)` group-id rule is spelled out twice
5. Change either copy (e.g. the id length) and only one of the two paths changes behaviour, which is the drift this finding is about
