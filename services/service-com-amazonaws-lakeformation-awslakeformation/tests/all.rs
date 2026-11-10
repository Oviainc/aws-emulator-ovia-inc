// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//!
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.

// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/creator_table_grants.rs"]
mod creator_table_grants;
#[path = "all/data_cells_filters.rs"]
mod data_cells_filters;
#[path = "all/data_lake_settings.rs"]
mod data_lake_settings;
#[path = "all/default_database_grant.rs"]
mod default_database_grant;
#[path = "all/glue_catalog.rs"]
mod glue_catalog;
#[path = "all/lf_tags.rs"]
mod lf_tags;
#[path = "all/permission_scope.rs"]
mod permission_scope;
#[path = "all/permissions.rs"]
mod permissions;
#[path = "all/registered_resources.rs"]
mod registered_resources;
#[path = "all/revoke_default_grant.rs"]
mod revoke_default_grant;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/table_column_scope.rs"]
mod table_column_scope;
#[path = "all/table_default_grants.rs"]
mod table_default_grants;
