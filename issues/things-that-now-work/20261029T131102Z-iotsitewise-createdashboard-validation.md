Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7bcaea858dd8788316443d36e8dbe675c4b82465

# dashboardDefinition is stored without being a JSON literal, the check the crate applies to its other documented-JSON member

_Filed: 2026-10-29T13:11:02Z_
_Operation: CreateDashboard | Class: validation_

> _Note: this issue was identified against the code base at commit `7bcaea858dd8788316443d36e8dbe675c4b82465`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`dashboardDefinition` is documented as "The dashboard definition specified in a JSON literal." CreateDashboard moves the raw string onto the stored record with no check that it is a JSON document at all, so a body such as `"not json"` is accepted (201) and `DescribeDashboard`/`ListDashboards` then report it back as the dashboard's definition — a definition no IoT SiteWise Monitor client can render. UpdateDashboard writes the same member the same way (src/ops/update_dashboard.rs:212-217), so the stale value can also be installed on an existing dashboard.

The crate already enforces exactly this rule for its other member whose documentation names the JSON form: `UpdateGatewayCapabilityConfiguration` parses `capabilityConfiguration` with `serde_json` and refuses a non-document with `InvalidRequestException`, on the stated ground that "storing it would let a later read answer with something no gateway could apply". The modelled `@length(0..204800)` and `^.+$` pattern cannot express the JSON form, so — exactly as in the gateway case — it is the handler's to enforce; `CreateDashboard` models `InvalidRequestException` (400) for it.

## Evidence

The create stores the member unchecked — services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/create_dashboard.rs:255-267:
```
        let record = crate::state::DashboardRecord {
            id: dashboard_id.clone(),
            arn: crate::state::dashboard_arn(&region, &dashboard_id),
            project_id: input.project_id,
            name: input.dashboard_name,
            description: input.dashboard_description,
            definition: input.dashboard_definition,
```
(grep for `serde_json` in create_dashboard.rs and update_dashboard.rs: no matches.)

The modelled sentence that states the member's form — smithy_query(query="doc", shape="CreateDashboardRequest", member="dashboardDefinition"): "<p>The dashboard definition specified in a JSON literal.</p>"; and the shape carries no constraint that could express it — smithy_query(query="shape", shape="DashboardDefinition"): {"type": "string", "length": {"min": 0, "max": 204800}, "pattern": "^.+$"}.

The sibling handler of this crate that applies the rule to its own documented-JSON member — src/ops/update_gateway_capability_configuration.rs:196-205:
```
        // "The JSON document that defines the configuration for the gateway
        // capability": a body that is not a JSON document defines no configuration,
        // and storing it would let a later read answer with something no gateway
        // could apply. The modelled length bounds are already enforced before this
        // handler runs; this is the member's own documented form.
        if ::serde_json::from_str::<::serde_json::Value>(&input.capability_configuration).is_err() {
            return Err(invalid_request(
                "The capabilityConfiguration must be a JSON document.".to_string(),
            ));
        }
```

The other write path, identical — src/ops/update_dashboard.rs:212-217: "dashboard.apply_update(input.dashboard_name, input.dashboard_description, input.dashboard_definition, now,);"

Test coverage: no test sends a non-JSON definition; the shared fixture deliberately keeps the JSON form and says so — tests/all/dashboard_lifecycle.rs:98-108:
```
/// A dashboard definition literal of `length` characters: a JSON object whose one
/// property is padded out, so the row spans a size without leaving the form a
/// definition takes.
pub(crate) fn definition_of(length: usize) -> String {
    let envelope = r#"{"widgets":[{"title":""}]}"#;
```
so fixing this leaves every existing dashboard test passing.

## Steps to Reproduce

1. CreatePortal, then CreateProject in it, to get a projectId.
2. POST /dashboards (CreateDashboard) with {"projectId": "<projectId>", "dashboardName": "d1", "dashboardDefinition": "not a json literal"}.
3. Observed: 201, and GET /dashboards/{dashboardId} reports `dashboardDefinition` = "not a json literal".
4. Expected, by the member's own modelled sentence ("The dashboard definition specified in a JSON literal.") and by the refusal the sibling applies to its documented-JSON member (src/ops/update_gateway_capability_configuration.rs:201-205): InvalidRequestException (400), with nothing stored.
5. The same body sent to PUT /dashboards/{dashboardId} (UpdateDashboard) is likewise accepted and overwrites a valid definition.
