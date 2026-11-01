Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1c4735096deb56657f444de0710fc17cb291a0f1

# The two edge projections key an edge on different halves of its identity, so one dependent is reported twice with a dimension set ListServiceDependencies never reports — against the rule observed_dependents' own doc states

_Filed: 2026-10-29T13:44:35Z_
_Operation: ListServiceDependents | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1c4735096deb56657f444de0710fc17cb291a0f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`observed_dependents` and `observed_dependencies` are the two views of ONE recorded edge, and `observed_dependents`' doc states the invariant they must hold to: the dimensions it reports are "the same dimension set [`Self::observed_dependencies`] reports for the same edge seen from the caller, which is what keeps the two directions from disagreeing about one edge" (state.rs:2827-2829). The crate's own integration test names the same invariant: "One edge carries ONE reference set, whichever end reports it ... the dependency view and the dependent view of the same edge agree reference for reference" (tests/all/discovered_metric_references.rs:447-449).

The code under those two sentences cannot keep it, because the two functions build their dedup key out of DIFFERENT halves of the edge and then pick the identity the dimensions are read from by "whichever SLO was iterated first":

* `observed_dependencies` keys an edge on `(near operation, far identity, far operation)` and LEAVES THE NEAR-END IDENTITY OUT of the key (state.rs:2792-2796). Two SLOs that record the same near operation -> far entity/operation edge but hold near identities of different fullness therefore collapse into ONE entry, and `by_edge.entry(edge).or_insert_with(...)` (state.rs:2805) keeps the dimensions of the FIRST SLO in `self.slos` order (a `BTreeMap` keyed by SLO name, state.rs:244) rather than the fullest identity.
* `observed_dependents` keys an edge on `(near identity, near operation, far operation)` and leaves the FAR-END identity out (state.rs:2852-2857). The same two SLOs produce TWO entries there, each dimensioned by its own near identity (state.rs:2858-2865).

Two consequences, both of which the crate elsewhere says must not happen:

1. `ListServiceDependents` reports the SAME dependent twice, under a 3-attribute identity and under the 4-attribute identity that contains it. `observed_services`' own rule is that a strict-subset identity names the same entity and must not be reported separately: "the subset names the same entity as the fuller set, and `GetService` answers for it with exactly that fuller set ... Reporting both would list one service twice and hand the caller a summary whose `KeyAttributes` no `GetService` echoes back" (state.rs:2663-2668), enforced at state.rs:2702-2706. No such fold happens in `observed_dependents`.
2. One of those two `ListServiceDependents` entries carries an `Environment` near-end dimension that the single `ListServiceDependencies` entry for the same edge does not carry — so the two directions disagree about one edge's reference set, which is exactly what state.rs:2827-2829 says cannot happen, and what the test at tests/all/discovered_metric_references.rs:452-506 asserts.

The divergence is reachable only for an identity whose documented minimum does not already force every near-end dimension to be present, i.e. the `Resource`/`AWS::Resource` entity types, whose minimum identity is `Type`+`ResourceType`+`Identifier` (`identifying_attributes`, state.rs:1830-1836) while `Environment` is applicable to every type and so always contributes a near-end dimension (`attribute_states_identity`, state.rs:2351-2357, with `NEAR_END_DIMENSIONS` at state.rs:2321-2326). So one matching SLO may record `Environment` and another may not, and both are admitted by `entity_identity_refusal` / `attributes_contain`.

## Evidence

THE RULE `observed_dependents`' OWN DOC STATES — services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs:2824-2829:
```
    /// `MetricReferences` reports the standard metrics collected for THAT call
    /// ([`standard_metric_references`]). The metrics of a call are the INVOKING entity's, so
    /// the dimensions are the dependent's identity and operation on the near end and the
    /// invoked entity and operation on the far end — the same dimension set
    /// [`Self::observed_dependencies`] reports for the same edge seen from the caller, which
    /// is what keeps the two directions from disagreeing about one edge.
```

THE CALLER-SIDE KEY, WHICH OMITS THE NEAR-END IDENTITY AND TAKES THE FIRST SLO'S — state.rs:2789-2810:
```
            let identity = entity
                .key_attributes
                .unwrap_or_else(|| key_attributes.clone());
            let edge = format!(
                "{operation_name}\u{1d}{}\u{1d}{}",
                attribute_key(&dependency.dependency_key_attributes),
                dependency.dependency_operation_name,
            );
            let metric_references = standard_metric_references(metric_dimensions(
                &identity,
                Some(&operation_name),
                Some((
                    &dependency.dependency_key_attributes,
                    &dependency.dependency_operation_name,
                )),
            ));
            by_edge.entry(edge).or_insert_with(|| ServiceDependency {
```

THE CALLEE-SIDE KEY, WHICH DOES INCLUDE THE NEAR-END IDENTITY (so the same two SLOs are two entries here) — state.rs:2852-2866:
```
            let edge = format!(
                "{}\u{1d}{}\u{1d}{}",
                attribute_key(&dependent_key_attributes),
                entity.operation_name.clone().unwrap_or_default(),
                dependency.dependency_operation_name,
            );
            let metric_references = standard_metric_references(metric_dimensions(
                &dependent_key_attributes,
                entity.operation_name.as_deref(),
                Some((
                    &dependency.dependency_key_attributes,
                    &dependency.dependency_operation_name,
                )),
            ));
            by_edge.entry(edge).or_insert_with(|| ServiceDependent {
```

THE SIBLING RULE THAT REFUSES TO REPORT ONE ENTITY UNDER TWO IDENTITIES — state.rs:2663-2668 and 2702-2706:
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

WHY ONE EDGE'S NEAR IDENTITY CAN DIFFER BETWEEN TWO MATCHING SLOs — state.rs:1830-1836 (minimum identity of a resource entity is Type+ResourceType+Identifier) and state.rs:2348-2356 (`Environment` is restricted to no type, so it always contributes a dimension):
```
fn identifying_attributes(entity_type: &str) -> [&'static str; 2] {
    if RESOURCE_ENTITY_TYPES.contains(&entity_type) {
        [RESOURCE_TYPE_ATTRIBUTE, IDENTIFIER_ATTRIBUTE]
    } else {
        [NAME_ATTRIBUTE, ENVIRONMENT_ATTRIBUTE]
    }
}
```
```
/// `Environment` is restricted to no type, and a record designating no documented kind of
...
    attribute == ENVIRONMENT_ATTRIBUTE || identifying_attributes(entity_type).contains(&attribute)
```

WHERE THE READS PRODUCE IT — src/ops/list_service_dependents.rs:207-209 and src/ops/list_service_dependencies.rs:209-211:
```
        let observed = state
            .lock()
            .observed_dependents(&input.key_attributes, &input.start_time, &input.end_time);
```

TEST COVERAGE: tests/all/discovered_metric_references.rs:452-506 (`both_ends_of_an_edge_report_the_same_references`) asserts the invariant, but compares only `["ServiceDependencies"][0]` against `["ServiceDependents"][0]` and seeds each edge from exactly ONE SLO, so neither the duplicate dependent entry nor the differing dimension set is pinned. tests/all/service_dependency_metrics.rs seeds one identity per entity too (`seed()` at :168-285: every entity's SLOs carry one `KeyAttributes` map), and its `one_caller_reports_one_entry_per_recorded_edge` (:515) only checks the caller direction. So the mixed-identity case is unpinned in both directions.

## Steps to Reproduce

1. Start the emulator with this crate mounted.
2. POST /slo with {"Name":"a-edge","CreateRecommendedSlo":false,"SliConfig":{"SliMetricConfig":{"KeyAttributes":{"Type":"Resource","ResourceType":"AWS::S3::Bucket","Identifier":"assets"},"OperationName":"GetObject","MetricType":"LATENCY","DependencyConfig":{"DependencyKeyAttributes":{"Type":"RemoteService","Name":"payments","Environment":"prod"},"DependencyOperationName":"Charge"}}}} — the edge recorded with the 3-attribute resource identity.
3. POST /slo with {"Name":"b-edge","CreateRecommendedSlo":false,"SliConfig":{"SliMetricConfig":{"KeyAttributes":{"Type":"Resource","ResourceType":"AWS::S3::Bucket","Identifier":"assets","Environment":"prod"},"OperationName":"GetObject","MetricType":"LATENCY","DependencyConfig":{"DependencyKeyAttributes":{"Type":"RemoteService","Name":"payments","Environment":"prod"},"DependencyOperationName":"Charge"}}}} — the SAME edge, recorded with the same identity plus Environment.
4. POST /service-dependencies?StartTime=<now-3600>&EndTime=<now+3600> with body {"KeyAttributes":{"Type":"Resource","ResourceType":"AWS::S3::Bucket","Identifier":"assets"}} — observe exactly ONE ServiceDependencies entry, whose MetricReferences dimensions are ResourceType/ResourceIdentifier/Operation/RemoteService/RemoteOperation with NO Environment dimension (the first SLO by name, "a-edge", won the or_insert_with).
5. POST /service-dependents?StartTime=<same>&EndTime=<same> with body {"KeyAttributes":{"Type":"RemoteService","Name":"payments","Environment":"prod"}} — observe TWO ServiceDependents entries for that one edge: one with DependentKeyAttributes of 3 attributes and no Environment dimension, one with 4 attributes and an extra {"Name":"Environment","Value":"prod"} dimension.
6. Compare step 4 and step 5: the dependent direction reports a reference set for the edge that the dependency direction never reports (the disagreement state.rs:2827-2829 says cannot happen), and it lists one dependent twice under a subset identity and its superset — the reporting state.rs:2663-2668 says must not happen.
