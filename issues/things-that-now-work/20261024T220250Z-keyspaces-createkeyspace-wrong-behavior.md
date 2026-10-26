Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 564feb857476f3247a0693cb65aa2d44e3322d3a

# A SINGLE_REGION keyspace stores the caller's regionList instead of the Region it was created in

_Filed: 2026-10-24T22:02:50Z_
_Operation: CreateKeyspace | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `564feb857476f3247a0693cb65aa2d44e3322d3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::resolve_replication` is documented to give a SINGLE_REGION keyspace the Region it was created in, but the code under that doc returns the caller's `regionList` verbatim for every strategy other than MULTI_REGION — and only the MULTI_REGION branch checks that the list contains the current Region. So `CreateKeyspace {"replicationStrategy":"SINGLE_REGION","regionList":["eu-west-1","ap-south-1"]}` issued in us-east-1 is accepted and the keyspace record claims replication in two Regions it is not replicated in, neither of which is the Region it lives in.

Those phantom Regions are invisible to every reader (`GetKeyspace`/`ListKeyspaces` report `replicationRegions` only for a MULTI_REGION keyspace, and `Keyspace::table_replica_regions` returns `None`), yet they are live state: `UpdateKeyspace` compares the requested list against them and refuses any widening that "omits Region(s) ... the keyspace is already replicated in". The keyspace therefore cannot be widened to MULTI_REGION without naming Regions no response ever disclosed.

The correct store for this case is the one the crate already writes everywhere else: the Region of the call (`vec![region.to_string()]`), which is what the helper's own documentation promises and what `ensure_service_managed_catalog` stores for its single-Region keyspaces.

## Evidence

Helper doc, `src/state.rs:1964-1971`: "The replication strategy and region list a keyspace is created with. ... A `SINGLE_REGION` keyspace is replicated in the region it was created in."

The code under that doc, `src/state.rs:1995-2003`:
```
if specification.replication_strategy != MULTI_REGION_STRATEGY {
    return Ok((
        specification.replication_strategy.clone(),
        specification
            .region_list
            .clone()
            .unwrap_or_else(|| vec![region.to_string()]),
    ));
}
```
— the supplied `region_list` wins; only the MULTI_REGION path below it runs the `must contain the current Region` check (`src/state.rs:2009-2013`).

Sibling in the same module, `src/state.rs:1329-1332`: "// A system keyspace is served in the Region it is read in, so it is single-Region and reports no `replicationRegions`." with `replication_regions: vec![region.to_string()]` — the single-Region store the doc describes.

The consumer that makes the phantom Regions observable, `src/ops/update_keyspace.rs:295-311`: "// The list is the keyspace's replication Regions AFTER the addition, so it has to keep the Regions the keyspace is already replicated in" → `return Err(invalid(format!("replicationSpecification.regionList omits Region(s) {} the keyspace is already replicated in: ...")))`.

The reader that hides them, `src/ops/get_keyspace.rs:196-202`: `let multi_region = keyspace.replication_strategy == crate::state::MULTI_REGION_STRATEGY;` ... `replication_regions: multi_region.then(|| keyspace.replication_regions.clone())`.

Caller: `src/ops/create_keyspace.rs:176-182`. The modelled `regionList` minimum length is 2 (`ReplicationSpecification$regionList`, `length min 2`), so such a list passes the boundary validator.

Test coverage: no test supplies a `regionList` together with `SINGLE_REGION` — every SINGLE_REGION row in `tests/all/list_keyspaces.rs:140`, `tests/all/get_keyspace.rs:107`, `tests/all/multi_region_table_replicas.rs:117`, `tests/all/update_keyspace.rs:149` passes `{"replicationStrategy": "SINGLE_REGION"}` alone, so nothing pins the current behaviour.

## Steps to Reproduce

1. CreateKeyspace {"keyspaceName":"single_ks","replicationSpecification":{"replicationStrategy":"SINGLE_REGION","regionList":["eu-west-1","ap-south-1"]}} signed for us-east-1 — answers 200
2. GetKeyspace {"keyspaceName":"single_ks"} — reports replicationStrategy SINGLE_REGION and NO replicationRegions, so the two stored Regions are invisible
3. UpdateKeyspace {"keyspaceName":"single_ks","replicationSpecification":{"replicationStrategy":"MULTI_REGION","regionList":["us-east-1","eu-west-1"]}} — refused 400 ValidationException "replicationSpecification.regionList omits Region(s) ap-south-1 the keyspace is already replicated in", a Region no response ever reported and the keyspace was never replicated in
