Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4a5766ed2caf698065daa3a6a1a461ee27489a64

# A table's auto scaling identity omits the Region, so same-named tables in two Regions share one set of settings and one DeleteTable wipes the other Region's

_Filed: 2026-10-28T17:37:58Z_
_Operation: GetTableAutoScalingSettings | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4a5766ed2caf698065daa3a6a1a461ee27489a64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states, in its own state module, that a keyspace and the tables it holds are a resource of ONE Region and that the same name names a DIFFERENT resource in another Region — and every other identity it mints is Region-qualified (`keyspace_arn(region, ..)`, `table_arn(region, ..)`, `type_arn(region, ..)`, and `publish_auto_scaling`/`retire_auto_scaling`, which both take a `region` argument).

The auto scaling identity is the exception: `auto_scaling_resource_id(keyspace_name, table_name)` and `scalable_target_key(keyspace_name, table_name, capacity)` carry no Region at all, and `reported_auto_scaling(engine, keyspace_name, table_name, stored)` — the function `GetTableAutoScalingSettings` reports from, since this crate deliberately treats Application Auto Scaling as the one copy of a table's auto scaling settings — takes no `region` parameter whatsoever. The Application Auto Scaling store those targets live in is a flat map keyed by exactly that Region-less key, so two tables that differ ONLY in the Region their keyspace was created in are ONE scalable target per capacity, and one policy name.

Three consequences, all reachable through this service's own API with input the model allows (same keyspace and table names in two Regions is explicitly supported by this crate, and is the normal shape of a multi-Region deployment):

1. `CreateTable` in Region B OVERWRITES the capacity window and target-tracking policy Region A's `CreateTable` registered for its own table (`apply_managed_target` finds the existing key and mutates it), so `GetTableAutoScalingSettings` signed for Region A reports Region B's `minimumUnits`/`maximumUnits`/`targetValue`.
2. `DeleteTable` (and `DeleteKeyspace`, and an `UpdateTable` that moves a table out of PROVISIONED) in Region B calls `retire_auto_scaling`, which deregisters that one key — deleting the scalable target, its policies, its scheduled actions and their CloudWatch alarms. Region A's table still exists and still reports PROVISIONED capacity, but `GetTableAutoScalingSettings` for it now omits `autoScalingSpecification` entirely: the settings a caller configured are lost, with no call of their own having touched them.
3. Because the scalable target keeps the ARN minted by whichever Region registered it first, the surviving target reports a scalable-target ARN scoped to the other Region.

This is state shared between Regions that this crate's own invariant says must not be shared — not merely a cosmetic scoping detail: case 2 destroys settings a caller created.

## Evidence

The rule, stated in this crate's own store definition — `services/service-com-amazonaws-keyspaces-keyspacesservice/src/state.rs:819-823`:
```
    /// The keyspaces callers have created, partitioned by the Amazon Web Services Region
    /// of the request that created each one and keyed inside a Region by `keyspaceName` —
    /// "keyspace names must be unique within each Region" (`CreateKeyspace`), so a
    /// keyspace, with the tables and types it holds, is a resource of ONE Region and the
    /// same name names a different resource in another.
```
and re-stated by the handler that enforces it, `src/ops/create_keyspace.rs:204-207`:
```
        // "keyspace names must be unique within each Region": a create of a name already
        // taken IN THIS REGION is the `ConflictException` the model declares for this
        // operation, and the same name is free in every other Region.
```

The break — the auto scaling identity, `src/state.rs:2747-2766`:
```
/// The `ResourceId` a table's scalable targets are registered under:
/// `keyspace/<keyspaceName>/table/<tableName>`, the form Application Auto Scaling
/// documents for the `cassandra` namespace's table resource type.
pub fn auto_scaling_resource_id(keyspace_name: &str, table_name: &str) -> String {
    format!("keyspace/{keyspace_name}/table/{table_name}")
}

/// The identity of one capacity's scalable target: the `cassandra` namespace, the table's
/// resource id and that capacity's dimension.
fn scalable_target_key(
    keyspace_name: &str,
    table_name: &str,
    capacity: TableCapacity,
) -> application_auto_scaling::state::TargetKey {
```
No Region in either, while the sibling writers in the SAME module are Region-qualified — `src/state.rs:2844-2850`:
```
pub fn publish_auto_scaling<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    keyspace_name: &str,
    table_name: &str,
    configured: &AutoScalingSpecification,
) {
```
`src/state.rs:2869-2874`:
```
pub fn retire_auto_scaling<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    keyspace_name: &str,
    table_name: &str,
) {
```
and the reader takes no Region at all, `src/state.rs:2902-2907`:
```
pub fn reported_auto_scaling<Eng: Send + Sync + 'static>(
    engine: &Eng,
    keyspace_name: &str,
    table_name: &str,
    stored: Option<&AutoScalingSpecification>,
) -> Option<AutoScalingSpecification> {
```
called by the handler at `src/ops/get_table_auto_scaling_settings.rs:237-242`:
```
        let auto_scaling_specification = crate::state::reported_auto_scaling(
            &*self.engine,
            &table.keyspace_name,
            &table.table_name,
            table.auto_scaling_specification.as_ref(),
        );
```
Contrast the Region-qualified identity every other resource of the same table gets, `src/state.rs:2028-2034`:
```
pub fn table_arn(region: &str, keyspace_name: &str, table_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("/keyspace/{keyspace_name}/table/{table_name}"),
    )
}
```
The store the Region-less key lands in holds one entry per key for the whole engine — `services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/state.rs:35-39` and `:171`:
```
pub struct TargetKey {
    pub service_namespace: String,
    pub resource_id: String,
    pub scalable_dimension: String,
}
...
    pub targets: BTreeMap<TargetKey, ScalableTargetRecord>,
```
and retiring removes that entry with its policies, scheduled actions and alarms, `.../applicationautoscaling.../src/state.rs:2421-2436`:
```
            None => {
                if let Some(removed) = store.targets.remove(key) {
                    store.arn_index.remove(&removed.arn);
                    ...
                    store.policies.retain(|policy_key, _| &policy_key.target != key);
```
reached from `src/ops/delete_table.rs:272-277`:
```
        crate::state::retire_auto_scaling(
            &*self.engine,
            &region,
            &input.keyspace_name,
            &input.table_name,
        );
```

Test coverage: no existing test pins the cross-Region case, so nothing constrains the fix. `tests/all/auto_scaling_propagation.rs:27-28` signs every request for one Region (`const REGION: &str = "us-west-2";`), and `tests/all/regional_partitioning.rs:292-409` ("RULE 3: the tables, types and tags of a keyspace are the Region's too") calls `GetTableAutoScalingSettings` only to assert the modelled 404 from another Region — it never creates a same-named auto-scaled table in two Regions. `tests/all/auto_scaling_propagation.rs:10` pins the `ResourceId` FORM (`keyspace/<keyspaceName>/table/<tableName>`) that Application Auto Scaling reports, so that string must keep its shape.

## Steps to Reproduce

1. Start one engine with both Amazon Keyspaces and Application Auto Scaling mounted, as tests/all/auto_scaling_propagation.rs does
2. Signed for us-east-1: CreateKeyspace {"keyspaceName":"app"}, then CreateTable {"keyspaceName":"app","tableName":"ledger","schemaDefinition":{...},"capacitySpecification":{"throughputMode":"PROVISIONED","readCapacityUnits":5,"writeCapacityUnits":5},"autoScalingSpecification":{"readCapacityAutoScaling":{"minimumUnits":1,"maximumUnits":10,"scalingPolicy":{"targetTrackingScalingPolicyConfiguration":{"targetValue":70}}}}}
3. Signed for eu-west-1: the same CreateKeyspace/CreateTable for "app"/"ledger", but with minimumUnits 100, maximumUnits 500 and targetValue 50
4. Signed for us-east-1: GetTableAutoScalingSettings {"keyspaceName":"app","tableName":"ledger"} — it reports minimumUnits 100 / maximumUnits 500 / targetValue 50, i.e. the settings created in eu-west-1, not the ones created in us-east-1
5. Signed for eu-west-1: DeleteTable {"keyspaceName":"app","tableName":"ledger"}
6. Signed for us-east-1: GetTable shows the table still present and PROVISIONED, but GetTableAutoScalingSettings for it now answers with no autoScalingSpecification member at all — the settings the us-east-1 create registered are gone, and so are their scalable targets, policy and alarms in Application Auto Scaling
