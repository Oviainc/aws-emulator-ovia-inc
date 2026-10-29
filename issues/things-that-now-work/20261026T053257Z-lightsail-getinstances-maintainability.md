Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 465e7599899ea277984bee5e564f3564a11f5f81

# The name-keyed page walk is hand-rolled in every resource listing, beside the crate's own shared catalog_page helper

_Filed: 2026-10-26T05:32:57Z_
_Operation: GetInstances | Class: maintainability_

> _Note: this issue was identified against the code base at commit `465e7599899ea277984bee5e564f3564a11f5f81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate pages two kinds of listing. The static catalogs go through ONE shared helper, `crate::state::catalog_page`, whose doc says it is "Shared by every listing this service pages (blueprints, instance bundles, the HTTPS setup history), so each one walks its records exactly the same way". The sixteen resource listings do not: each one hand-rolls the identical BTreeMap walk — take the token as the start key, iterate `range(page_token..)`, break when the accumulated page reaches that listing's `*_PAGE_SIZE` and set `nextPageToken` to the name that was not taken.

The copies differ only in the map walked, the page-size constant and the view built, so the one paging rule exists in sixteen places and the copies can drift (the comment above each is itself a hand-copied paragraph). A single generic helper beside `catalog_page` — "at most `page_size` records of this BTreeMap from the token onward, plus the name of the first record after them" — would leave every response and every existing test byte-identical, since the rule being factored is exactly the one `tests/all/paging.rs::walk_pages` already checks for all of them.

## Evidence

The crate's stated single-source intent for the catalog listings — services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:955-956:
```
/// Shared by every listing this service pages (blueprints, instance bundles, the
/// HTTPS setup history), so each one walks its records exactly the same way.
```
(`fn catalog_page<T: Clone>(...)` at src/state.rs:965).

Copy 1 — src/ops/get_instances.rs:169-187:
```
        let page_token = input.page_token.unwrap_or_default();

        crate::state::with_state(&*self.engine, |store| {
            let mut instances = Vec::new();
            let mut next_page_token = None;
            for (name, instance) in store.instances.range(page_token.clone()..) {
                if instances.len() == crate::state::INSTANCE_PAGE_SIZE {
                    next_page_token = Some(name.clone());
                    break;
                }
                instances.push(instance.view(store.attached_disk_views(name)));
            }
            GetInstancesOutput {
                instances: Some(instances),
                next_page_token,
            }
```

Copy 2 — src/ops/get_disks.rs:172-192:
```
        let page_token = input.page_token.unwrap_or_default();

        crate::state::with_state(&*self.engine, |store| {
            let mut disks = Vec::new();
            let mut next_page_token = None;
            for (name, disk) in store.disks.range(page_token..) {
                if disks.len() == crate::state::DISK_PAGE_SIZE {
                    next_page_token = Some(name.clone());
                    break;
                }
                disks.push(disk.view());
            }
            GetDisksOutput {
                disks: Some(disks),
                next_page_token,
            }
```

The same block again at: get_disk_snapshots.rs:175-192, get_instance_snapshots.rs:178-195, get_domains.rs:182-199, get_relational_databases.rs:179-195, get_relational_database_snapshots.rs:179-196, get_static_ips.rs:176-195, get_export_snapshot_records.rs:173-195, get_cloud_formation_stack_records.rs:173-195, get_active_names.rs:178-196, get_alarms.rs:189-216, get_certificates.rs:200-257, get_buckets.rs:201-246, get_load_balancers.rs:174-201 and get_distributions.rs:190-220.

Test coverage that pins the behaviour a fix must not change: tests/all/paging.rs:37-86 (`walk_pages`, the one checked walk over a Lightsail listing's pages: "no page carries more than `page_size` records, a `nextPageToken` names the FIRST record of the page after it ... a page that is not the last one is full"), driven by tests/all/disk_listing.rs, instance_snapshot_listing.rs, disk_snapshot_listing.rs, domain_listing.rs, static_ip_listing.rs, relational_database_listing.rs, relational_database_snapshot_listing.rs, distribution_listing.rs, load_balancer_listing.rs, active_names.rs, alarm_configuration.rs, bucket_lifecycle.rs and export_snapshot_records.rs.

## Steps to Reproduce

1. Open src/state.rs:941-990 and read `catalog_page`'s doc: one shared page walk "so each one walks its records exactly the same way".
2. Open src/ops/get_instances.rs:169-187 and src/ops/get_disks.rs:172-192 and compare: the same token-as-start-key walk, page-size break and nextPageToken assignment, written twice.
3. Repeat for the fourteen further listings named in the evidence; no two share a helper.
4. Observe that a change to the paging rule (for example refusing a token that names no record, which the catalog listings already do) has to be made sixteen times.
