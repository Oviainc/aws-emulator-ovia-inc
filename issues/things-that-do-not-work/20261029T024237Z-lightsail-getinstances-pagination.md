Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: cd4f62d4622d99c848b6778d8f7c3e7dcd5cf5b1

# A pageToken that names no record is accepted as a cursor by the sixteen name-keyed listings, silently truncating the page instead of being refused as the crate's own token rule states

_Filed: 2026-10-29T02:42:37Z_
_Operation: GetInstances | Class: pagination_

> _Note: this issue was identified against the code base at commit `cd4f62d4622d99c848b6778d8f7c3e7dcd5cf5b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate pages two families of listing, and they disagree about what a `pageToken` is.

The log-backed and catalog-backed listings state, and enforce, that a token is only valid if it names a record of the collection it came from: `GetOperations`, `GetOperationsForResource`, `GetBundles`, `GetBlueprints`, `GetSetupHistory`, `GetRelationalDatabaseBlueprints`, `GetRelationalDatabaseBundles`, `GetRelationalDatabaseParameters`, `GetRelationalDatabaseEvents`, `GetRelationalDatabaseLogEvents` and `GetContainerLog` all answer the modelled `InvalidInputException` for a token that names nothing. `GetOperationsForResource` writes the rule out in full: "The asymmetry is the token's, not the name's: a token is only ever answered by the entry it names."

The sixteen name-keyed resource listings do the opposite: each takes the raw request string as a BTreeMap range start (`input.page_token.unwrap_or_default()` then `range(page_token..)`) and performs NO check at all. The consequence is not merely a different error code — it is silent data loss on a successful 200:

* `GetInstances` with `pageToken="zzz"` (a string no instance is named) answers `instances: []` and no `nextPageToken`, which a client reads as "this account holds no instances";
* `GetInstances` with `pageToken="m"` answers only the instances whose names sort at or after "m", silently dropping every earlier one, and still reports a well-formed page;
* a token minted by one listing is accepted by every other one, because none of them is scoped to the collection that issued it — a `GetDisks` token is a perfectly good `GetInstances` cursor.

So the crate contradicts itself: either the eleven listings that refuse an unnamed token are wrong, or these sixteen skip the check. The same handlers are the ones the already-filed maintainability issue (20261026T053257Z-lightsail-getinstances-maintainability) describes as hand-rolled copies — that issue names "refusing a token that names no record, which the catalog listings already do" as a change that would have to be made sixteen times, but files no defect for the behaviour itself. This issue is that defect.

Affected handlers (same block, same omission): GetInstances, GetDisks, GetDiskSnapshots, GetInstanceSnapshots, GetDomains, GetRelationalDatabases, GetRelationalDatabaseSnapshots, GetStaticIps, GetExportSnapshotRecords, GetCloudFormationStackRecords, GetActiveNames, GetAlarms, GetCertificates, GetBuckets, GetLoadBalancers, GetDistributions, GetKeyPairs.

## Evidence

The rule this crate states for a page token — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/get_operations_for_resource.rs:195-206:
```
            // A name the log holds nothing for and NO token reports an empty
            // page; the same name WITH a token is refused, because that token
            // names no entry of it. The asymmetry is the token's, not the
            // name's: a token is only ever answered by the entry it names.
            let (operations, next_page_token) = store
                .operations_for_resource_page(&resource_name, page_token.as_deref())
                .ok_or_else(|| {
                    invalid_page_token(
                        page_token.as_deref().unwrap_or_default(),
                        &resource_name,
                    )
                })?;
```
and the sibling that implements it the same way — src/ops/get_operations.rs:194-199:
```
            // An empty log and NO token reports an empty page; a token is
            // refused whenever it names no entry of the log, because a token is
            // only ever answered by the entry it names.
            let (operations, next_page_token) = store
                .operations_page(page_token.as_deref())
                .ok_or_else(|| invalid_page_token(page_token.as_deref().unwrap_or_default()))?;
```
with its refusal at src/ops/get_operations.rs:216-225:
```
fn invalid_page_token(token: &str) -> GetOperationsError {
    GetOperationsErrorModeled::InvalidInputException(InvalidInputException {
        message: Some(format!(
            "pageToken {token:?} does not name a recorded operation; a page token is the \
             operation id a previous page reported as nextPageToken"
```

The handler that skips the check — src/ops/get_instances.rs:165-188:
```
        // `pageToken` is the name the previous page stopped before, so a page is
        // the stored instances from that name onward in the store's own total
        // order. Using the name rather than an index means an instance deleted
        // between two pages cannot make the walk skip or repeat another one.
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
There is no `invalid_page_token` in this file at all — its only error constructor is `unavailable()` (src/ops/get_instances.rs:195-201).

The identical unchecked walk in the other fifteen: src/ops/get_disks.rs:172-192, get_disk_snapshots.rs:175-192, get_instance_snapshots.rs:178-195, get_domains.rs:182-199, get_relational_databases.rs:179-195, get_relational_database_snapshots.rs:179-196, get_static_ips.rs:176-195, get_export_snapshot_records.rs:173-195, get_cloud_formation_stack_records.rs:173-195, get_active_names.rs:178-196, get_alarms.rs:189-194, get_certificates.rs:200-251, get_buckets.rs:201-240, get_load_balancers.rs:174-201, get_distributions.rs:190-220, get_key_pairs.rs:180-204.

Test coverage: tests/all/paging.rs:37-86 (`walk_pages`) only ever presents a token a previous page reported (`token = Some(next.to_string())`, paging.rs:80), so no test presents an unrecognised token to any listing and nothing pins the lenient behaviour. The catalog side's refusal IS pinned (for example tests/all/blueprint_catalog.rs and tests/all/operations_log.rs drive the `invalid_page_token` paths).

## Steps to Reproduce

1. Call CreateInstances three times to create instances named "alpha", "beta" and "gamma" and wait for them to settle.
2. Call GetInstances with no pageToken — all three are reported, nextPageToken absent.
3. Call GetInstances with pageToken="zzz": HTTP 200 with instances=[] and no nextPageToken, i.e. the account appears to hold no instances.
4. Call GetInstances with pageToken="beta2": HTTP 200 with only "gamma" — "alpha" and "beta" are silently dropped from a page the client cannot tell from a legitimate one.
5. Call GetOperations with pageToken="zzz" for contrast: HTTP 400 InvalidInputException, "pageToken \"zzz\" does not name a recorded operation" (src/ops/get_operations.rs:216-225).
6. Call GetDisks, note its nextPageToken, and present that token to GetInstances: it is accepted as a cursor into the instance map, because no listing scopes its token to the collection that minted it.
