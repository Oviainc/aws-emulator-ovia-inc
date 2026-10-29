Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 69ff0d5dd7a30b3ec79ed56649baca6290c20cfe

# PublishMetrics words its missing-environment refusal with the control-plane builder the crate scopes to `api.` operations, because the crate's endpoint census records the wrong host prefix for it

_Filed: 2026-10-29T13:53:57Z_
_Operation: PublishMetrics | Class: error-response_

> _Note: this issue was identified against the code base at commit `69ff0d5dd7a30b3ec79ed56649baca6290c20cfe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate classifies the service's operations into exactly TWO endpoint classes and records that classification as a fact about the Smithy model:

* the environment data endpoint (`@endpoint` hostPrefix `env.`) — three operations, whose not-found refusal is worded by the fixed `ENVIRONMENT_ENDPOINT_NOT_FOUND_MESSAGE` and reshaped by the front door in `src/service/env_endpoint.rs`;
* the control plane (`@endpoint` hostPrefix `api.`) — "every other operation of this service", whose not-found refusal is worded by `state::environment_not_found_message`.

The model does not support that binary split: `PublishMetrics` carries a THIRD host prefix, `ops.`. Because the census in `env_endpoint.rs` asserts `api.` for every non-`env.` operation, nobody ever decided how an `ops.`-hosted operation words a name it cannot resolve, and `publish_metrics.rs` reached for the control-plane builder — whose own doc comment restricts it to the three `api.`-hosted, `Name`-addressed lookups and enumerates them (`GetEnvironment`, `UpdateEnvironment`, `DeleteEnvironment`), `PublishMetrics` being none of them.

The defect is therefore twofold and provable entirely from text in this crate plus the model: (1) the census sentence in `env_endpoint.rs` states something the model contradicts, so the crate holds no record that this service has a third endpoint class; (2) the refusal in `PublishMetrics` is worded by a helper used outside its own documented scope, so the sentence an `ops.`-hosted operation returns was inherited from a rule derived for a different endpoint rather than chosen for it. `PublishMetrics` models no 404, so the status/code (400 `ValidationException`) is the only modelled option and is not in question — only the sentence and the premise behind it are.

## Evidence

The model gives PublishMetrics a third host prefix:

    smithy_query(query="operation", operation="PublishMetrics") ->
      "http": {"code": 200, "method": "POST", "uri": "/metrics/environments/{EnvironmentName}"},
      "endpoint": {"hostPrefix": "ops."}

(for comparison: `ListEnvironments` -> `"endpoint": {"hostPrefix": "api."}`, `CreateCliToken` / `InvokeRestApi` -> `"endpoint": {"hostPrefix": "env."}`.)

The crate's census claims otherwise — services/service-com-amazonaws-mwaa-amazonmwaa/src/service/env_endpoint.rs:61-66:

    /// Source: this service's Smithy model —
    /// `smithy_query(query="operation",operation="CreateCliToken,CreateWebLoginToken,InvokeRestApi",...)`
    /// reports `endpoint.hostPrefix` `env.` for these three, and `api.` for every other
    /// operation of this service.
    pub const ENVIRONMENT_DATA_OPERATIONS: [&str; 3] =
        ["CreateCliToken", "CreateWebLoginToken", "InvokeRestApi"];

The builder scopes itself to the three `api.` lookups — services/service-com-amazonaws-mwaa-amazonmwaa/src/state.rs:1570-1583:

    /// The message an environment-not-found refusal of a CONTROL-PLANE operation carries: the
    /// sentence names the environment the REQUEST asked for.
    ///
    /// Every operation the model hosts on the control-plane endpoint (`@endpoint` host prefix
    /// `api.`) and addresses by `Name` — `GetEnvironment`, `UpdateEnvironment`,
    /// `DeleteEnvironment` — words the same condition through this one builder, so none of them
    /// can drift from the rest. The operations hosted on the ENVIRONMENT data endpoint
    /// (`@endpoint` host prefix `env.`: `CreateCliToken`, `CreateWebLoginToken`,
    /// `InvokeRestApi`) never reach this builder ...
    pub fn environment_not_found_message(name: &str) -> String {
        format!("Environment {name} not found.")
    }

Yet the `ops.`-hosted operation calls it — services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/publish_metrics.rs:360-366:

        let stored = store
            .environment_mut(&region, &input.environment_name)
            .ok_or_else(|| {
                validation(crate::state::environment_not_found_message(
                    &input.environment_name,
                ))
            })?;

Test coverage confirms the two-class assumption is what the suites pin, and that PublishMetrics is deliberately outside their tables — services/service-com-amazonaws-mwaa-amazonmwaa/tests/all/rejection_contract.rs:234-273 (`fn name_addressed_calls()`: "Every operation addressed by `Name` ... and the host prefix is the one the model's `@endpoint` trait declares for the operation — which is what decides the refusal's wording") lists only the three `api.` and the three `env.` operations; `tests/all/published_metrics.rs` asserts the 400 `ValidationException` for an unknown environment without asserting its sentence. So fixing the census and deciding the `ops.` wording changes no existing assertion about PublishMetrics' status or code.

## Steps to Reproduce

1. Run `smithy_query(query="operation", operation="PublishMetrics", model_path="services/service-com-amazonaws-mwaa-amazonmwaa/smithy-model/com-amazonaws-mwaa-amazonmwaa.json")` and read `"endpoint": {"hostPrefix": "ops."}`.
2. Read services/service-com-amazonaws-mwaa-amazonmwaa/src/service/env_endpoint.rs:61-64 and note the claim that the model reports `api.` for every operation other than the three `env.` ones — PublishMetrics falsifies it.
3. Read services/service-com-amazonaws-mwaa-amazonmwaa/src/state.rs:1570-1580: the builder documents itself as the wording for the operations the model hosts on `api.` and names the three it serves.
4. Read services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/publish_metrics.rs:360-366: the `ops.`-hosted PublishMetrics refuses an unresolvable `EnvironmentName` through that same builder, so its sentence is `Environment <Name> not found.`
5. POST /metrics/environments/NeverCreated with a valid MetricData batch: the answer is HTTP 400 `ValidationException` carrying the control-plane sentence, a wording the crate's own documentation scopes to the three `api.` lookups.
