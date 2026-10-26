Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f2163892bf40cea6da7d1662f28c14ec75c797c6

# UpdateResourceCollection accepts an AppBoundaryKey outside the documented `Devops-guru-` prefix and stores it as analysis coverage

_Filed: 2026-10-26T23:24:49Z_
_Operation: UpdateResourceCollection | Class: validation_

> _Note: this issue was identified against the code base at commit `f2163892bf40cea6da7d1662f28c14ec75c797c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateResourceCollection` is the one write path that defines the account's tag-based analysis coverage. The model's documentation for the member it writes (`UpdateTagCollectionFilter$AppBoundaryKey`, and the identical text on `TagCollection$AppBoundaryKey`) states a hard rule about the key a client may use to define that coverage: it must begin with the prefix `Devops-guru-`.

The handler never applies that rule. It performs several other checks on the submitted tag filters — the stack-name cap, the one-app-boundary-key-per-account invariant, the per-key tag-value cap — and then writes every submitted `AppBoundaryKey` verbatim into `ResourceCoverage::tag_collections`. An `ADD` naming `AppBoundaryKey = "Team"` therefore answers 200 and the key becomes real coverage: `GetResourceCollection(AWS_TAGS)` reports it, `ListMonitoredResources` reports one monitored resource per (key, value) pair of it, `DescribeAccountHealth` bills `ResourceHours`/`AnalyzedResourceCount` for it, `DescribeResourceCollectionHealth(AWS_TAGS)`/`DescribeOrganizationResourceCollectionHealth(AWS_TAGS)` report a `TagHealth` entry for it, and `StartCostEstimation` will resolve and price a collection naming it. The modelled constraints cannot catch this: `AppBoundaryKey`'s pattern is `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, which admits `Team`, so the boundary validator passes it and the handler is the only place the documented rule could be enforced.

This also breaks the crate's own convention for this very member: the crate derives and enforces unmodelled rules straight out of this documentation paragraph elsewhere (the single app-boundary key per account, and the one-tag maximum for a cost estimate), but silently drops the prefix sentence of that same paragraph.

Note for the fixer: the fix changes one existing test row that currently pins acceptance — `tests/all/cost_estimation.rs:204` ("an app boundary key outside the Devops-guru- prefix", coverage `tags(&[("Team", &["payments"])])`, `Expect::Accepted`). That row states no reason for accepting the key; it only varies the key's form. Every other suite already uses a `Devops-guru-`/`devops-guru-` prefixed key for coverage writes, so nothing else is affected. Keys that appear only in READ filters (e.g. `"another-boundary"` in `tests/all/insight_search.rs:640`, `"other-boundary"` in `tests/all/events.rs:704`) are not coverage definitions and must stay accepted.

## Evidence

Model rule (smithy_query(query="doc", shape="UpdateTagCollectionFilter", member="AppBoundaryKey")), identical text on `TagCollection$AppBoundaryKey`:

> "The string used for a *key* in a tag that you use to define your resource coverage must begin with the prefix `Devops-guru-`. The tag *key* might be `DevOps-Guru-deployment-application` or `devops-guru-rds-application`. When you create a *key*, the case of characters in the *key* can be whatever you choose. After you create a *key*, it is case-sensitive."

The modelled constraints do not express it — smithy_query(query="shape", shape="AppBoundaryKey"): `{"type": "string", "length": {"min": 1, "max": 128}, "pattern": "^([\\p{L}\\p{Z}\\p{N}_.:/=+\\-@]*)$"}` — so `"Team"` is a well-formed value and reaches the handler.

Handler — the only tag-key checks it makes are the singleton and the value cap, and then it writes the key as submitted:

`src/ops/update_resource_collection.rs:278-281`:
```rust
            let submitted_keys: std::collections::BTreeSet<&str> = submitted_tags
                .iter()
                .map(|tag| tag.app_boundary_key.as_str())
                .collect();
```
`src/ops/update_resource_collection.rs:342-348`:
```rust
        for tag in submitted_tags {
            if adding {
                coverage
                    .tag_collections
                    .entry(tag.app_boundary_key.clone())
                    .or_default()
                    .extend(tag.tag_values.iter().cloned());
```
No prefix test exists anywhere in the crate: grep for `devops-guru-|prefix` over `src/` returns only page-token comments (`src/state.rs:1325,1453,2044,2655`) and a message example (`src/ops/start_cost_estimation.rs:361`) — no comment justifying the divergence.

Same-crate convention it breaks (the crate DOES enforce documentation-only rules from this same paragraph):
`src/state.rs:1977-1986`:
```rust
/// DevOps Guru's own invariant, not an emulator guard (so it does not live with the
/// store bounds below): `TagCollection`'s `AppBoundaryKey` documentation says "All
/// Amazon Web Services resources in your account and Region tagged with this key make
/// up your DevOps Guru application and analysis boundary", so an account has a single
/// analysis boundary and therefore a single such key.
pub const APP_BOUNDARY_KEYS_PER_ACCOUNT: usize = 1;
```
`src/state.rs:2439-2444`:
```rust
/// Emulator-enforced ceiling on `ResourceCollection.Tags`, from the documented AWS
/// rule rather than from the model: `TagCostEstimationResourceCollectionFilter`'s
/// documentation states "The maximum number of tags you can specify for a cost
/// estimate is one" ...
pub const MAX_COST_ESTIMATION_TAG_FILTERS: usize = 1;
```

Test coverage today: `tests/all/cost_estimation.rs:204-209` pins the opposite (`name: "an app boundary key outside the Devops-guru- prefix"`, `coverage: Some(tags(&[("Team", &["payments"])]))`, `expect: Expect::Accepted(&[(TAG, 1)])`); the coverage-write suites `tests/all/configuration_lifecycle.rs:428-473` and `tests/all/mutation_idempotency_and_tenancy.rs:234-315` only ever write `Devops-guru-*` keys.

## Steps to Reproduce

1. Start the emulator and point a `devops-guru` client at it.
2. Call UpdateResourceCollection with Action=ADD and ResourceCollection={"Tags":[{"AppBoundaryKey":"Team","TagValues":["payments"]}]}.
3. Observe HTTP 200: the key is accepted even though it does not begin with the documented `Devops-guru-` prefix (expected: ValidationException 400 naming ResourceCollection.Tags[].AppBoundaryKey).
4. Call GetResourceCollection with ResourceCollectionType=AWS_TAGS and observe `Tags[0].AppBoundaryKey == "Team"` now persisted as the account's analysis boundary.
5. Call ListMonitoredResources (no Filters) and DescribeAccountHealth and observe the non-conforming pair counted as a monitored resource / analyzed resource.
