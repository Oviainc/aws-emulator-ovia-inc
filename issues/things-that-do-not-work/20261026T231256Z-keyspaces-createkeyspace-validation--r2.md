Issue-Severity: HIGH
Issue-Gap-Class: validation
Issue-Commit: 58a1ce99e9431000682e2d92b8b10e21bce3c765

# CreateKeyspace accepts a MULTI_REGION regionList that names the same Region twice, so the keyspace is stored with a replication group it does not have and can never be widened

_Filed: 2026-10-26T23:12:56Z_
_Operation: CreateKeyspace | Class: validation_

> _Note: this issue was identified against the code base at commit `58a1ce99e9431000682e2d92b8b10e21bce3c765`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::resolve_replication` is the one place this crate states the replication rules, and it checks the list's size cap, that every entry is an AWS Region, that a MULTI_REGION specification carries a `regionList`, and that the list contains the current Region. It never collapses the list to the SET of Regions it denotes, and `CreateKeyspace` stores whatever it returns straight into `Keyspace::replication_regions`.

So `CreateKeyspace` with `replicationStrategy: MULTI_REGION` and `regionList: ["us-east-1","us-east-1"]` (signed for us-east-1) is accepted with 200: the modelled `length min 2` is satisfied by the repeated entry, both entries are real Regions, and the current Region is present. The keyspace is then persisted as MULTI_REGION with a two-entry replication group that denotes ONE Region.

`UpdateKeyspace` implements exactly the rule this path is missing — it dedupes the list and refuses one that denotes fewer than two Regions — so the same list is refused 400 through the sibling operation and accepted through the create.

Two observable consequences of the stored duplicate:

1. `GetKeyspace`/`ListKeyspaces` report `replicationRegions: ["us-east-1","us-east-1"]` and `replicationGroupStatuses` carries two entries for the same Region; `GetTable`/`GetTableAutoScalingSettings` likewise emit two `replicaSpecifications` entries for one Region (both are derived from `Keyspace::table_replica_regions()`), a shape `crate::state::resolve_replica_specifications` refuses on input ("names Region '{}' more than once, and a Region holds one set of per-Region settings").

2. The keyspace can never be widened again. `UpdateKeyspace` compares the DEDUPED request list against the NON-deduped stored list (`regions.len() == keyspace.replication_regions.len()`), so a genuine addition — `["us-east-1","us-west-2"]` onto the stored `["us-east-1","us-east-1"]` — has length 2 == 2 and is refused with "replicationSpecification.regionList adds no Amazon Web Services Region", even though it adds one. The same happens for a stored `[REGION, REGION, OTHER]` widened with `[REGION, OTHER, THIRD]`.

This is separate from the fixed `is_aws_region` gap (every entry here IS a real Region); the missing rule is that the list denotes a SET of at least two Regions.

## Evidence

Smithy, `CreateKeyspaceRequest$replicationSpecification` (smithy_query query=doc shape=CreateKeyspaceRequest member=replicationSpecification): "`regionList` - if the `replicationStrategy` is `MULTI_REGION`, the `regionList` requires the current Region and at least one additional Amazon Web Services Region where the keyspace is going to be replicated in." A list of `["us-east-1","us-east-1"]` names NO additional Region, yet it is accepted.

The helper, `services/service-com-amazonaws-keyspaces-keyspacesservice/src/state.rs:2559-2569` — the last checks before the list is returned verbatim:
```
    let Some(regions) = specification.region_list.clone() else { ... };
    if !regions.iter().any(|listed| listed == region) {
        return Err(format!(
            "replicationSpecification.regionList must contain the current Region '{region}'"
        ));
    }
    Ok((specification.replication_strategy.clone(), regions))
```
Its own doc comment (state.rs:2500-2505) claims to cover the whole sentence: "A `MULTI_REGION` keyspace's \"`regionList` requires the current Region and at least one additional Amazon Web Services Region\" ... so both are checked here." Only the current-Region half is.

The caller stores it unchanged: `src/ops/create_keyspace.rs:176-182` (`resolve_replication(input.replication_specification.as_ref(), &region)`) and `src/ops/create_keyspace.rs:256-262` (`store.insert_keyspace(&region, crate::state::Keyspace { ... replication_regions, ... })`).

The sibling that DOES apply the rule, `src/ops/update_keyspace.rs:228-252`:
```
        // A Region list is a SET of Regions: a repeated entry names the same Region twice,
        // so it is kept once ... The modelled minimum length of 2 is on the list, not on the
        // set it denotes, so a list that only repeats one Region is what breaks the documented
        // "the `regionList` requires the current Region and at least one additional Amazon
        // Web Services Region".
        ...
        if regions.len() < 2 {
            return Err(invalid(format!(
                "replicationSpecification.regionList names only Region '{}': a {} keyspace \
                 requires the current Region and at least one additional Amazon Web \
                 Services Region", ...
```
and its own claim that the create is held to the same rules, `src/ops/update_keyspace.rs:209-211`: "The same replication rules a create is held to — a `MULTI_REGION` specification carries a `regionList`, and that list contains the current Region — in the one place this crate states them."

The wedge, `src/ops/update_keyspace.rs:312-318` (deduped request length vs raw stored length):
```
        if regions.len() == keyspace.replication_regions.len() {
            return Err(invalid(format!(
                "replicationSpecification.regionList adds no Amazon Web Services Region: \
                 the keyspace is already replicated in {}", keyspace.replication_regions.join(", ")
```

The per-Region rule the stored duplicate contradicts, `src/state.rs:2409-2415`: "replicaSpecifications names Region '{}' more than once, and a Region holds one set of per-Region settings".

Test coverage: the refusal is pinned for `UpdateKeyspace` only — `tests/all/update_keyspace.rs:648-658`: `name: "regionList repeating one Region"`, `multi_region(&[REGION, REGION])`, `status: 400`, `code: "ValidationException"`, with the comment "A list that only repeats one Region denotes a single Region, which is one short of the documented 'at least one additional' Region." No `CreateKeyspace` test supplies a repeated Region: `tests/all/single_region_replication.rs:239-244` (`a_multi_region_keyspace_stores_the_supplied_region_list`) uses four rows of DISTINCT Regions, so nothing pins the accepting behaviour.

## Steps to Reproduce

1. CreateKeyspace {"keyspaceName":"dupe_ks","replicationSpecification":{"replicationStrategy":"MULTI_REGION","regionList":["us-east-1","us-east-1"]}} signed for us-east-1 — answers 200 (UpdateKeyspace refuses the identical list 400 ValidationException, tests/all/update_keyspace.rs:648)
2. GetKeyspace {"keyspaceName":"dupe_ks"} — replicationStrategy MULTI_REGION, replicationRegions ["us-east-1","us-east-1"], and replicationGroupStatuses carries two entries for the same Region
3. CreateTable in dupe_ks, then GetTable — replicaSpecifications holds two summaries for region us-east-1, a duplicate resolve_replica_specifications refuses on input
4. Wait for the keyspace to settle to ACTIVE, then UpdateKeyspace {"keyspaceName":"dupe_ks","replicationSpecification":{"replicationStrategy":"MULTI_REGION","regionList":["us-east-1","us-west-2"]}} — refused 400 "replicationSpecification.regionList adds no Amazon Web Services Region", although us-west-2 is new: the keyspace can never be widened
