Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7f16775ab75866f4d67c2e0f0f0192fd6ebf8c3d

# A resource-typed entity's MetricReferences carry no dimension of its own identity — an empty `Dimensions` list where the crate's own rule says an absent fact is an absent member

_Filed: 2026-10-29T01:52:16Z_
_Operation: ListServices | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7f16775ab75866f4d67c2e0f0f0192fd6ebf8c3d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every topology projection reports its `MetricReferences` through the single helper `crate::state::standard_metric_references`, whose dimension set comes from `metric_dimensions`. The near-end dimension table (`NEAR_END_DIMENSIONS`) maps only two attributes — `Environment` -> `Environment` and `Name` -> `Service`.

But this crate's own per-`Type` identity rule (`identifying_attributes`) says a `Resource` / `AWS::Resource` entity is identified by `ResourceType` and `Identifier`, NOT by `Name`; `entity_identity_refusal` enforces exactly that pair for such an entity, and `tests/all/list_services.rs` seeds a four-attribute resource entity as a discovered service. Neither of those two identifying attributes has a near-end dimension, although the same module maps both of them for the FAR end of a call (`FAR_END_DIMENSIONS`: `RemoteResourceType`, `RemoteResourceIdentifier`).

Two observable consequences, both from the same line:

1. A resource identity that carries no `Environment` (legal: `{"Type":"AWS::Resource","ResourceType":...,"Identifier":...}` passes `entity_identity_refusal`, and `Attributes` is `length 1..4`) yields an EMPTY dimension vector, which `standard_metric_references` still reports as a present member: `dimensions: Some(dimensions.clone())` (state.rs:2228). So the response carries `"Dimensions": []`. That contradicts both the member's own doc ("An array of one or more dimensions that further define the metric") and the rule this crate states and applies everywhere else — `metric_dimensions`' own doc says "an absent fact is an absent dimension", and the two sibling derivations in the same module write `(!groups.is_empty()).then_some(groups)` (state.rs:2368) and `(!queries.is_empty()).then_some(queries)` (state.rs:3417) for exactly this case.

2. Because the identifying attributes contribute nothing, two DIFFERENT resource entities in one environment are reported with byte-identical `MetricReferences` (dimension set `[{Environment: prod}]` for both an `orders-table` and a `customers-table` DynamoDB entity), which contradicts the rule stated immediately above the catalogue: "the references of two entities differ exactly as their stored identities do".

The helper is shared, so `GetService` answers the same way for the same entity; this is filed on `ListServices` because it is the read that reports every discovered resource entity with no identity supplied by the caller (and because GetService/wrong-behavior already carries an open issue about a different defect).

## Evidence

THE RULE THE MODULE STATES FOR THESE REFERENCES — services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs:2118-2120:
```
///
/// Every dimension VALUE is read out of the stored record the reference describes, so the
/// references of two entities differ exactly as their stored identities do.
```

THE NEAR-END TABLE THAT OMITS A RESOURCE'S IDENTITY — state.rs:2149-2159:
```
const NEAR_END_DIMENSIONS: [(&str, &str); 2] = [
    (ENVIRONMENT_ATTRIBUTE, "Environment"),
    (NAME_ATTRIBUTE, "Service"),
];

/// [`NEAR_END_DIMENSIONS`], for the far end of a call.
const FAR_END_DIMENSIONS: [(&str, &str); 3] = [
    (NAME_ATTRIBUTE, "RemoteService"),
    (RESOURCE_TYPE_ATTRIBUTE, "RemoteResourceType"),
    (IDENTIFIER_ATTRIBUTE, "RemoteResourceIdentifier"),
];
```

THE CRATE'S OWN PER-TYPE IDENTITY RULE THAT THE NEAR-END TABLE CONTRADICTS — state.rs:1664-1670:
```
fn identifying_attributes(entity_type: &str) -> [&'static str; 2] {
    if RESOURCE_ENTITY_TYPES.contains(&entity_type) {
        [RESOURCE_TYPE_ATTRIBUTE, IDENTIFIER_ATTRIBUTE]
    } else {
        [NAME_ATTRIBUTE, ENVIRONMENT_ATTRIBUTE]
    }
}
```

THE "ABSENT FACT IS AN ABSENT DIMENSION" RULE, STATED ON THE FUNCTION THAT BUILDS THE SET — state.rs:2167-2174:
```
/// A dimension whose attribute the stored identity does not carry is LEFT OUT rather than
/// reported empty — `Dimension.Value` is `length 1..1024`
/// (`smithy_query(shape="Dimension")`), so an absent fact is an absent dimension, and the
/// dimension set states exactly what the record states.
fn metric_dimensions(
```

THE LINE THAT BREAKS IT — state.rs:2222-2232:
```
fn standard_metric_references(dimensions: Vec<Dimension>) -> Vec<MetricReference> {
    STANDARD_METRICS
        .iter()
        .map(|(metric_name, metric_type)| MetricReference {
            namespace: APPLICATION_SIGNALS_NAMESPACE.to_string(),
            metric_type: (*metric_type).to_string(),
            dimensions: Some(dimensions.clone()),
```

THE SAME MODULE'S SIBLING DERIVATIONS, WHICH DO OMIT AN EMPTY LIST — state.rs:2368 (`service_groups`) and state.rs:3415-3417 (`assessed_metric_graph`):
```
        (!groups.is_empty()).then_some(groups)
```
```
        // An absent fact is an absent member: an objective whose indicator holds no query
        // contributes no query rather than an empty list.
        metric_data_queries: (!queries.is_empty()).then_some(queries),
```

WHERE THE READ PRODUCES IT — src/ops/list_services.rs:199-202 -> state.rs:2501-2510 (`observed_services`):
```
            .map(|key_attributes| ServiceSummary {
                key_attributes: key_attributes.clone(),
                attribute_maps: None,
                service_groups: self.service_groups(key_attributes),
                metric_references: standard_metric_references(metric_dimensions(
                    key_attributes,
                    None,
                    None,
                )),
```
(the same helper call in `observed_service`, state.rs:2429-2436, for `GetService`)

WHAT THE MODEL SAYS THE MEMBER HOLDS — `smithy_query(query="shape", shape="MetricReference")`, member `Dimensions`: "An array of one or more dimensions that further define the metric." (the list shape itself states only `length max 30`, so an empty array is not refused at the boundary and reaches the client).

TEST COVERAGE: tests/all/list_services.rs:184-195 and :217-222 seed and assert a four-attribute `Resource` identity as a discovered service, but assert identities only. tests/all/discovered_metric_references.rs:192-265 is the file that pins dimension sets, and every monitored entity it seeds is a `Service`-typed one (`service_entity`, line 188) — a resource appears only as the FAR END of a dependency edge (lines 240-248), which is the branch that does get `RemoteResourceType`/`RemoteResourceIdentifier` dimensions. tests/all/get_service.rs:328-362 drives a resource-typed MONITORED entity but asserts `Service.KeyAttributes` only. So nothing pins either symptom.

## Steps to Reproduce

1. Start the emulator with this crate mounted.
2. POST /slo with {"Name":"orders-table-latency","SliConfig":{"SliMetricConfig":{"KeyAttributes":{"Type":"Resource","ResourceType":"AWS::DynamoDB::Table","Identifier":"orders-table","Environment":"prod"},"MetricType":"LATENCY"}},"CreateRecommendedSlo":false}
3. POST /slo again with {"Name":"customers-table-latency", ...} identical except "Identifier":"customers-table".
4. GET /services?StartTime=<now-3600>&EndTime=<now+3600> — observe that BOTH ServiceSummaries entries report the same three MetricReferences with the same dimension set [{"Name":"Environment","Value":"prod"}]: nothing in the references distinguishes the two tables, although their stored identities differ in the attribute that identifies a resource.
5. POST /slo with {"Name":"log-group-latency","SliConfig":{"SliMetricConfig":{"KeyAttributes":{"Type":"AWS::Resource","ResourceType":"AWS::Logs::LogGroup","Identifier":"/aws/lambda/x"},"MetricType":"LATENCY"}},"CreateRecommendedSlo":false} — a legal identity with no Environment.
6. GET /services over the same window (and POST /service with those KeyAttributes) — observe that entry's MetricReferences each carry "Dimensions": [], a present-but-empty member, where every other derived list in this crate omits the member when it has nothing to state.
