Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4e0be56b24c0fc13c424c1a1b18bb5f0120957ed

# Per-group estimatedMonthlySavings double-counts a resource covered by several recommendation types

_Filed: 2026-10-27T02:53:46Z_
_Operation: ListRecommendationSummaries | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4e0be56b24c0fc13c424c1a1b18bb5f0120957ed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListRecommendationSummaries` builds each `RecommendationSummary` by summing the monthly saving of EVERY row in the group, without the per-resource de-duplication the model attaches to that shape. The model documents `RecommendationSummary` as "The summary of rightsizing recommendations, including de-duped savings from all types of recommendations." — the de-dup belongs to the summary item itself, whose only money member is `estimatedMonthlySavings`.

The handler applies the crate's own de-duplication helper (`crate::state::dedupe_by_resource`) only to the response-level `estimatedTotalDedupedSavings`, and passes the RAW (non-deduped) rows to `summarise`. The result is that two mutually exclusive recommendations for one resource (e.g. the seeded catalog's `Rightsize` and `MigrateToGraviton` recommendations for the same EC2 instance, which a caller can adopt only one of) both contribute their saving to the same group, so the group's headline savings figure is inflated. Because the seeded catalog always contains that shared resource, every grouped answer this operation produces today over-reports at least one group.

The handler's own comment one line below `summarise` states the per-resource rule it does not apply to the groups ("a resource covered by several recommendation types contributes its best saving once"), and the helper it would need is already written and already called from the same function and from `ListRecommendations`, so nothing new has to be designed: the groups are summed through a path that skips the helper the sibling path uses.

Note: `tests/all/recommendation_summaries.rs` currently PINS the non-deduped group totals (its oracle re-aggregates the non-deduped `ListRecommendations` rows), so fixing this requires updating that test's oracle; the Smithy sentence above contradicts the rule that test states.

## Evidence

Smithy model (via `smithy_query(query="doc", shape="RecommendationSummary")`):
  "The summary of rightsizing recommendations, including de-duped savings from all types of recommendations."
(`RecommendationSummary` is the member shape of `items`; its only savings member is `estimatedMonthlySavings` — "The estimated total savings resulting from modifications, on a monthly basis.")

Handler — the groups are built from the RAW rows:
`services/.../src/ops/list_recommendation_summaries.rs:291`
    let groups = summarise(&rows, dimension, mode);
`services/.../src/ops/list_recommendation_summaries.rs:372-382` (inside `summarise`, iterating every row of the view):
    for row in rows {
        let label = row.group_label(dimension);
        let savings = row.monthly_savings(mode);
        match groups.iter_mut().find(|(group, _, _)| *group == label) {
            Some((_, total, count)) => { *total += savings; *count += 1; }

The de-dup the same handler DOES apply, two lines later, only to the response total:
`services/.../src/ops/list_recommendation_summaries.rs:293-299`
    // De-duped: one recommendation per resource, so a resource covered by
    // several recommendation types contributes its best saving once. The
    // total is of the whole filtered view, not of the page being returned.
    let deduped = crate::state::dedupe_by_resource(&rows, mode);

The helper already exists in this crate and is the one both views need:
`services/.../src/state.rs:671-678`
    /// Keep one recommendation per resource: the one that saves the most under
    /// `mode`, because that is the recommendation for the resource. ...
    pub fn dedupe_by_resource(

The catalog that makes it observable (one resource, two alternative recommendations):
`services/.../src/state.rs:1128-1130`
    // Two recommendations for ONE instance: rightsizing it, or moving it to
    // Graviton. `includeAllRecommendations` decides whether a caller sees
    // both or only the better one.
`services/.../src/state.rs:1135` and `:1167` — both rows carry `resource_id: "i-0a1b2c3d4e5f60001"`, with savings 280.32-140.16 = 140.16 and 280.32-224.26 = 56.06 under the default `BeforeDiscounts` mode.

Test coverage that pins the current (non-deduped) grouping and would need its oracle updated:
`services/.../tests/all/recommendation_summaries.rs:6-11` — "a summary is the recommendation catalog aggregated along one `groupBy` dimension ... while `estimatedTotalDedupedSavings` is that total taken over one recommendation per resource"; `:97-113` (`expected_groups` sums every listed row) and `:275-279` ("the de-duped total must drop the second recommendation for the shared resource").

## Steps to Reproduce

1. Boot the emulator and call `UpdateEnrollmentStatus {"status": "Active"}`, then wait for the analysis delay so recommendations are visible.
2. Call `ListRecommendations {"includeAllRecommendations": true}` and note the two recommendations for resource `i-0a1b2c3d4e5f60001` in `us-east-1` with `estimatedMonthlySavings` 140.16 and 56.06.
3. Call `ListRecommendationSummaries {"groupBy": "Region"}`.
4. Observe the `us-east-1` group reports `estimatedMonthlySavings` 206.22 — 140.16 + 56.06 + 10.00 — i.e. both alternative recommendations for the one instance counted, where the de-duped savings for that region is 150.16 (140.16 + 10.00).
5. Compare with `estimatedTotalDedupedSavings` in the same response, which the handler computes through `dedupe_by_resource` and which therefore counts that instance once: the two figures in one response disagree about the same rows.
