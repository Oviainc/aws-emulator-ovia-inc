Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 83df1cdc32a5bdf510e34eb14380f1c3f1e573c9

# The paginated-listing page walk (token decode, position resume, default/clamp maxResults, slice, next-token mint) is written out once per listing handler, ~27 times over

_Filed: 2026-10-27T14:55:57Z_
_Operation: ListPortals | Class: maintainability_

> _Note: this issue was identified against the code base at commit `83df1cdc32a5bdf510e34eb14380f1c3f1e573c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated listing in this crate repeats the same five-step page walk verbatim, differing only in the local variable name and the key the resume point is taken from: decode the hex `nextToken`, `position()` it in the ordered vector (refusing a token that is not in it), `input.max_results.unwrap_or(DEFAULT_MAX_RESULTS).max(1) as usize`, `start.saturating_add(max_results).min(len)`, then `.get(end).map(|x| ::hex::encode(key(x)))`. Each file also carries its own private `decode_token` and `invalid_token` copies with the same body and the same message string.

The copies are in list_portals.rs, list_gateways.rs, list_projects.rs, list_project_assets.rs, list_dashboards.rs, list_datasets.rs, list_dataset_data_segments.rs, list_assets.rs, list_asset_models.rs, list_asset_model_properties.rs, list_asset_model_composite_models.rs, list_asset_properties.rs, list_asset_relationships.rs, list_associated_assets.rs, list_access_policies.rs, list_actions.rs, list_executions.rs, list_computation_models.rs, list_computation_model_data_binding_usages.rs, list_interface_relationships.rs, list_enrichment_jobs.rs, list_pipelines.rs, list_queries.rs, list_searches.rs, list_tasks.rs and list_time_series.rs. Because the walk is duplicated rather than shared, the copies can (and in page-size handling already do) drift: some read `input.max_results.unwrap_or(DEFAULT_MAX_RESULTS)` while list_tasks.rs:210 and list_pipelines.rs:221 read `input.max_results.max(1)` with no default at all.

A fix is one generic helper in `crate::state` taking the ordered slice, the optional token, the optional page size, the default and a key function, returning `(start, end, next_token)` — every response and every existing paging test stays byte-identical.

## Evidence

Two copies, byte-identical apart from the identifier:

- services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/list_portals.rs:176-197 — "let start = match input.next_token.as_deref() { Some(token) => { let resume = decode_token(token)?; portals.iter().position(|portal| portal.id == resume).ok_or_else(invalid_token)? } None => 0, };" ... "let max_results = input.max_results.unwrap_or(DEFAULT_MAX_RESULTS).max(1) as usize; let end = start.saturating_add(max_results).min(portals.len());" ... "let next_token = portals.get(end).map(|portal| ::hex::encode(portal.id.as_bytes()));"
- services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/list_gateways.rs:176-197 — "let start = match input.next_token.as_deref() { Some(token) => { let resume = decode_token(token)?; gateways.iter().position(|gateway| gateway.id == resume).ok_or_else(invalid_token)? } None => 0, };" ... "let max_results = input.max_results.unwrap_or(DEFAULT_MAX_RESULTS).max(1) as usize; let end = start.saturating_add(max_results).min(gateways.len());" ... "let next_token = gateways.get(end).map(|gateway| ::hex::encode(gateway.id.as_bytes()));"

The already-drifted copies:
- services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/list_tasks.rs:210 — "let max_results = input.max_results.max(1) as usize;"
- services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/list_pipelines.rs:221 — "let max_results = input.max_results.max(1) as usize;"

Tests that pin the behaviour and must stay green unchanged: tests/all/list_portals.rs, tests/all/list_gateways.rs, tests/all/list_projects.rs, tests/all/list_project_assets_paging.rs, tests/all/list_datasets.rs, tests/all/list_dataset_data_segments.rs, tests/all/list_asset_models.rs, tests/all/list_asset_properties.rs, tests/all/list_asset_model_composite_models.rs, tests/all/list_access_policies.rs, tests/all/list_computation_models.rs, tests/all/list_dashboards.rs, tests/all/list_interface_relationships.rs, tests/all/list_pipelines.rs, tests/all/list_queries.rs, tests/all/list_searches.rs, tests/all/list_tasks.rs, tests/all/actions.rs, tests/all/executions.rs.

## Steps to Reproduce

1. Open src/ops/list_portals.rs:176-197 and src/ops/list_gateways.rs:176-197 side by side: the page walk and the per-file decode_token/invalid_token helpers are identical apart from the identifier.
2. grep -n 'let start = match input.next_token.as_deref()' src/ops/list_*.rs — 26 hits across 26 handlers.
3. grep -n 'let max_results = input.max_results' src/ops/list_*.rs — two of them omit the default, showing the copies have already drifted.
