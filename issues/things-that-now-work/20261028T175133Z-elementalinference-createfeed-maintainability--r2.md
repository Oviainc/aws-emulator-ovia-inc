Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 9d7fd75a9416876f17d684b80aaadc94961fb271

# The per-Region create quota rule is implemented twice, once per resource kind

_Filed: 2026-10-28T17:51:33Z_
_Operation: CreateFeed | Class: maintainability_

> _Note: this issue was identified against the code base at commit `9d7fd75a9416876f17d684b80aaadc94961fb271`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule a create is held to — "this Region already holds the maximum of N records", then "this account already holds records in the maximum of MAX_REGIONS Regions when this request would open a new partition" — is written out twice in `crate::state`, once for feeds (`feed_quota_refusal` + `feed_region_count`, read by `CreateFeed`) and once for dictionaries (`dictionary_quota_refusal` + `dictionary_region_count`, read by `CreateDictionary`). The two copies are the same two-clause branch and the same distinct-Region count over a different `BTreeMap`; only the map, the bound constant and the two sentences differ. Each copy can therefore drift from the other — a correction to the comparison (`>=` vs `>`), to the new-partition test, or to the order the two clauses are decided in lands in one resource kind and not the other, exactly the drift the crate already removed for the page-size rule (`crate::state::resolve_page_size`) and for the output-list rules (`crate::state::output_list_refusal`). One parameterised refusal (map/iterator + bound + the two message builders) keeps every response byte-identical, since the sentences stay per resource kind.

## Evidence

Copy 1 — `src/state.rs:1160-1176`:
```
pub fn feed_quota_refusal(&self, region: &str) -> Option<String> {
    let held = self.feeds_in(region).count();
    if held >= MAX_FEEDS { return Some(format!("This account already holds the maximum of {MAX_FEEDS} feeds in this Region. ...")); }
    let new_partition = !self.feeds.values().any(|feed| feed.region == region);
    if new_partition && self.feed_region_count() >= MAX_REGIONS { return Some(format!("This account already holds feeds in the maximum of {MAX_REGIONS} Regions. ...")); }
    None
}
```
Copy 2 — `src/state.rs:1181-1201`:
```
pub fn dictionary_quota_refusal(&self, region: &str) -> Option<String> {
    let held = self.dictionaries_in(region).count();
    if held >= MAX_DICTIONARIES { return Some(format!("This account already holds the maximum of {MAX_DICTIONARIES} dictionaries in this Region. ...")); }
    let new_partition = !self.dictionaries.values().any(|dictionary| dictionary.region == region);
    if new_partition && self.dictionary_region_count() >= MAX_REGIONS { return Some(format!("This account already holds dictionaries in the maximum of {MAX_REGIONS} Regions. ...")); }
    None
}
```
And the distinct-Region count, duplicated the same way — `src/state.rs:1270-1276` `fn feed_region_count(&self) -> usize { self.feeds.values().map(|feed| feed.region.as_str()).collect::<std::collections::BTreeSet<&str>>().len() }` versus `src/state.rs:1280-1286` `fn dictionary_region_count(&self) -> usize { self.dictionaries.values().map(|dictionary| dictionary.region.as_str()).collect::<std::collections::BTreeSet<&str>>().len() }`. The doc on the second even records that it is a copy: "on the same rule as [`Self::feed_region_count`]" (`src/state.rs:1278-1279`).

Call sites: `src/ops/create_feed.rs:370` `if let Some(message) = state.feed_quota_refusal(&region)` and `src/ops/create_dictionary.rs:261` `if let Some(message) = state.dictionary_quota_refusal(&region)`.

Tests that pin the behaviour a refactor must leave unchanged: `tests/all/feed_lifecycle.rs:855` `create_feed_refuses_past_the_feed_quota`, `tests/all/dictionary_lifecycle.rs:340` `create_dictionary_refuses_past_the_dictionary_quota`, `tests/all/region_isolation.rs:386` `a_regions_feed_quota_is_not_consumed_by_another_regions_feeds` and `:426` `a_regions_dictionary_quota_is_not_consumed_by_another_regions_dictionaries`, `tests/all/completed_deletion.rs:346` `a_completed_deletion_holds_no_quota`, `tests/all/create_replay_is_idempotent.rs:486` `a_replayed_create_at_the_quota_still_answers_the_resource_it_minted`.

## Steps to Reproduce

1. Open services/service-com-amazonaws-elementalinference-elementalinference/src/state.rs and read `feed_quota_refusal` (1160-1176) beside `dictionary_quota_refusal` (1181-1201): the same two clauses, in the same order, over a different map and bound
2. Read `feed_region_count` (1270-1276) beside `dictionary_region_count` (1280-1286): identical bodies apart from the map they walk
3. Observe that a change to either clause (the comparison, the new-partition test, or their order) has to be made twice to keep CreateFeed and CreateDictionary agreeing, while the tests above pin only the messages, which a parameterised single implementation would keep byte-identical
