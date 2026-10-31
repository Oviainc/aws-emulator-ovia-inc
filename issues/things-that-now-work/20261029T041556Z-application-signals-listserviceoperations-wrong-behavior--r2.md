Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1fd13cac8c9d59f908c3acc103dd89ab8a3a1d61

# Each listed operation's MetricReferences are dimensioned by the identity of the SLOs naming THAT operation, not by the entity identity GetService reports — against the rule the function's own doc states

_Filed: 2026-10-29T04:15:56Z_
_Operation: ListServiceOperations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1fd13cac8c9d59f908c3acc103dd89ab8a3a1d61`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`observed_operations` is the projection `ListServiceOperations` reports. Its own doc states the rule it follows: "The identity used is the fullest one the stored SLOs hold for the entity, as [`Self::observed_service`] reports it, so the dimensions cannot disagree with the `GetService` answer for the same entity" (state.rs:2543-2545).

The code under that sentence does something else: it takes the longest identity PER OPERATION, not per entity. The `by_name` map is keyed by operation name and the `observed.len() > identity.len()` comparison is made against that operation's own accumulated identity (state.rs:2561-2564), so each reported entry is dimensioned by the fullest identity among only the SLOs that name that one operation. `observed_service` — the function the doc points at — takes the maximum over EVERY SLO monitoring the entity, into one `reported` identity used for the whole answer (state.rs:2442-2454), and `observed_services` goes further and folds a strict-subset identity into the fuller one so that one entity is never reported under two identities (state.rs:2480-2485 doc, state.rs:2519-2523 code).

Consequence, when two SLOs on one entity record different-sized identities on different operations: the dimension sets the three reads report for one entity in one window disagree. `GetService` reports the entity's MetricReferences dimensioned by the FULLER identity, while `ListServiceOperations` reports the operation of the SMALLER-identity SLO with the dimensions of that smaller identity — and the two operations of that single entity disagree with each other about the entity's own `Environment`/`Service` dimensions, although the reported `ServiceOperation` entries carry no identity of their own for a client to tell them apart by. That is exactly the disagreement the doc says cannot happen.

The divergence is observable whenever the identifying attributes of the request do not already force `Name`/`Environment` to be present in every matching SLO — i.e. for the `Resource`/`AWS::Resource` entity types, whose documented minimum identity is `Type`+`ResourceType`+`Identifier` (`identifying_attributes`, state.rs:1687-1693), so one matching SLO may carry `Environment` (or `Name`) and another may not. It survives any change to the near-end dimension table: with resource dimensions added, the fuller identity still contributes one dimension more than the smaller one.

## Evidence

THE RULE THE FUNCTION'S OWN DOC STATES — services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs:2540-2545:
```
    /// Each entry's `MetricReferences` reports the standard metrics collected for THAT
    /// operation ([`standard_metric_references`]): the dimensions are the entity's identity
    /// plus the operation, so two operations of one service carry different references. The
    /// identity used is the fullest one the stored SLOs hold for the entity, as
    /// [`Self::observed_service`] reports it, so the dimensions cannot disagree with the
    /// `GetService` answer for the same entity.
```

THE CODE UNDER IT, WHICH MAXIMISES PER OPERATION — state.rs:2552-2573:
```
        let mut by_name: BTreeMap<String, HashMap<String, String>> = BTreeMap::new();
        for slo in self.monitoring(key_attributes, window_start, window_end) {
            let entity = entity_of(slo);
            let Some(name) = entity.operation_name else {
                continue;
            };
            let observed = entity
                .key_attributes
                .unwrap_or_else(|| key_attributes.clone());
            let identity = by_name.entry(name).or_default();
            if observed.len() > identity.len() {
                *identity = observed;
            }
        }
        by_name
            .into_iter()
            .map(|(name, identity)| ServiceOperation {
                metric_references: standard_metric_references(metric_dimensions(
                    &identity,
                    Some(&name),
                    None,
                )),
```

THE SIBLING THE DOC POINTS AT, WHICH MAXIMISES OVER THE WHOLE ENTITY — state.rs:2442-2454:
```
        let mut reported = key_attributes.clone();
        let mut discovered = false;
        for slo in self.monitoring(key_attributes, window_start, window_end) {
            discovered = true;
            if let Some(observed) = entity_of(slo).key_attributes {
                if observed.len() > reported.len() {
                    reported = observed;
                }
            }
        }
        let (metric_references, service_groups) = if discovered {
            (
                standard_metric_references(metric_dimensions(&reported, None, None)),
```

THE OTHER SIBLING, WHICH REFUSES TO REPORT ONE ENTITY UNDER TWO IDENTITIES AT ALL — state.rs:2480-2485 and 2519-2523:
```
    /// An identity that is a STRICT SUBSET of another observed identity is not reported
    /// separately: as few as one of an entity's up to four `Attributes` entries identifies
    /// it ([`attributes_contain`]), so the subset names the same entity as the fuller set,
    /// and `GetService` answers for it with exactly that fuller set
    /// ([`Self::observed_service`]). Reporting both would list one service twice and hand
    /// the caller a summary whose `KeyAttributes` no `GetService` echoes back.
```
```
            .filter(|candidate| {
                !identities.iter().any(|other| {
                    other.len() > candidate.len() && attributes_contain(Some(other), candidate)
                })
            })
```

WHY A RESOURCE IDENTITY CAN DIFFER BETWEEN TWO MATCHING SLOs — state.rs:1687-1693 (`identifying_attributes`, the crate's own per-`Type` minimum):
```
fn identifying_attributes(entity_type: &str) -> [&'static str; 2] {
    if RESOURCE_ENTITY_TYPES.contains(&entity_type) {
        [RESOURCE_TYPE_ATTRIBUTE, IDENTIFIER_ATTRIBUTE]
    } else {
        [NAME_ATTRIBUTE, ENVIRONMENT_ATTRIBUTE]
    }
}
```
so a request naming `{Type, ResourceType, Identifier}` is served (`entity_identity_refusal`, state.rs:1721-1736) and matches both an SLO whose identity is exactly those three and one that also carries `Environment` (`attributes_contain`, state.rs:1616-1626).

WHERE THE READ PRODUCES IT — src/ops/list_service_operations.rs:200-202:
```
        let observed = state
            .lock()
            .observed_operations(&input.key_attributes, &input.start_time, &input.end_time);
```

TEST COVERAGE: tests/all/service_operation_metric_references.rs pins a dimension set per operation (rows() at :214-295), but every seeded entity is monitored by SLOs that all carry ONE identity — the only entity with two SLOs is `checkout`, whose two SLOs carry the same three-attribute identity (:104-119), and the two resource rows are each monitored by a single SLO read back with exactly that identity (:157, :164, :276, :282). So the mixed-identity case this issue is about is not pinned by any test, and tests/all/get_service.rs:328-362 drives a resource-typed monitored entity but asserts `Service.KeyAttributes` only.

## Steps to Reproduce

1. Start the emulator with this crate mounted.
2. POST /slo with {"Name":"assets-get-latency","CreateRecommendedSlo":false,"SliConfig":{"SliMetricConfig":{"KeyAttributes":{"Type":"Resource","ResourceType":"AWS::S3::Bucket","Identifier":"assets"},"OperationName":"GetObject","MetricType":"LATENCY"}}} — an SLO whose identity is the three documented resource-identity attributes.
3. POST /slo with {"Name":"assets-put-latency","CreateRecommendedSlo":false,"SliConfig":{"SliMetricConfig":{"KeyAttributes":{"Type":"Resource","ResourceType":"AWS::S3::Bucket","Identifier":"assets","Environment":"prod"},"OperationName":"PutObject","MetricType":"LATENCY"}}} — the same entity, one attribute fuller, on a second operation.
4. POST /service?StartTime=<now-3600>&EndTime=<now+3600> with body {"KeyAttributes":{"Type":"Resource","ResourceType":"AWS::S3::Bucket","Identifier":"assets"}} — observe Service.KeyAttributes is the FOUR-attribute identity and each of the three MetricReferences carries Dimensions [{"Name":"Environment","Value":"prod"}].
5. POST /service-operations?StartTime=<same>&EndTime=<same> with the same KeyAttributes body — observe the "GetObject" entry's references carry Dimensions [{"Name":"Operation","Value":"GetObject"}] with NO Environment dimension, while the "PutObject" entry carries [{"Name":"Environment","Value":"prod"},{"Name":"Operation","Value":"PutObject"}].
6. Compare step 4 and step 5: two operations of one entity report different dimensions for the entity itself, and the GetObject entry disagrees with the GetService answer for the same entity over the same window — the disagreement state.rs:2543-2545 says cannot happen.
