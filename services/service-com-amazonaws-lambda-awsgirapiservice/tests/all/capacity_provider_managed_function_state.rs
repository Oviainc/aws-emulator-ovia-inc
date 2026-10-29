// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `State` a capacity-provider-managed function reports.
//!
//! The rule under test: a function that names a capacity provider is not directly
//! invocable at its UNPUBLISHED configuration — the compute it runs on belongs to the
//! provider, and what the provider runs is a published version registered with it — so
//! once the function has settled, every surface that reports a `State` for that
//! unpublished configuration reports the `State` enum's `ActiveNonInvocable` rather than
//! `Active`. A published version reports the lifecycle state it has, a function that names
//! no capacity provider settles into `Active`, and a function that has not settled yet is
//! `Pending` either way: the projection narrows the settled state of one configuration, it
//! does not replace the lifecycle.
//!
//! Every assertion is over real HTTP requests through the router, across each surface that
//! reports the state — `CreateFunction`'s own answer, `GetFunctionConfiguration`
//! unqualified and at each qualifier, `GetFunction`,
//! `PutFunctionScalingConfig`'s `FunctionState` (a surface only a function that names a
//! capacity provider has, because that operation serves no other kind) and
//! `ListFunctionVersionsByCapacityProvider` — so one surface disagreeing with the others
//! fails here. (`ListFunctions` is not among them: the model states that it "returns a
//! subset of the FunctionConfiguration fields" and names `State` as one of those it does
//! not report.) The rows differ along what the rule generalises over: whether the function
//! names a capacity provider, which qualifier is read, and whether the clock has settled
//! the function yet.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{
    create_function, json_call, make_activation_due, package_bytes, router_on, zip_file, Answer,
    ACCOUNT, REGION, ROLE,
};

/// Where `CreateFunction` and the function reads are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where the capacity-provider operations are bound.
const CAPACITY_PROVIDERS_URI: &str = "/2025-11-30/capacity-providers";

/// The `State` values this suite expects, from
/// `smithy_query(query="shape", shape="State")` (`Pending | Active | Inactive | Failed |
/// Deactivating | Deactivated | ActiveNonInvocable | Deleting`).
const STATE_PENDING: &str = "Pending";
/// See [`STATE_PENDING`].
const STATE_ACTIVE: &str = "Active";
/// See [`STATE_PENDING`].
const STATE_ACTIVE_NON_INVOCABLE: &str = "ActiveNonInvocable";

/// A well-formed capacity-provider operator role ARN.
const OPERATOR_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-capacity";

/// The provider the managed functions here run on.
const PROVIDER: &str = "managed-fleet";

/// The ARN a capacity provider of `name` carries.
fn provider_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:capacity-provider:{name}")
}

/// The unqualified ARN a function of `name` carries.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// `CreateCapacityProvider`, stating the required configuration alone.
async fn create_provider(router: &MultiServiceRouter, name: &str) -> Answer {
    json_call(
        router,
        "POST",
        CAPACITY_PROVIDERS_URI,
        &json!({
            "CapacityProviderName": name,
            "VpcConfig": { "SubnetIds": ["subnet-10000000"], "SecurityGroupIds": [] },
            "PermissionsConfig": { "CapacityProviderOperatorRoleArn": OPERATOR_ROLE },
        }),
    )
    .await
}

/// `GetFunctionConfiguration` on `reference`, with `query` appended.
async fn get_configuration(
    router: &MultiServiceRouter,
    reference: &str,
    query: &str,
) -> Answer {
    json_call(
        router,
        "GET",
        &format!("{FUNCTIONS_URI}/{reference}/configuration{query}"),
        &Value::Null,
    )
    .await
}

/// `GetFunction` on `reference`.
async fn get_function(router: &MultiServiceRouter, reference: &str) -> Answer {
    json_call(
        router,
        "GET",
        &format!("{FUNCTIONS_URI}/{reference}"),
        &Value::Null,
    )
    .await
}

/// `PutFunctionScalingConfig` on one published qualifier of `function`
/// (`PUT /2025-11-30/functions/{FunctionName}/function-scaling-config`, whose `Qualifier`
/// is `@required` and whose pattern admits `$LATEST.PUBLISHED` or a version number).
async fn put_scaling_config(
    router: &MultiServiceRouter,
    function: &str,
    qualifier: &str,
) -> Answer {
    json_call(
        router,
        "PUT",
        &format!(
            "/2025-11-30/functions/{function}/function-scaling-config?Qualifier={qualifier}"
        ),
        &json!({ "FunctionScalingConfig": { "MaxExecutionEnvironments": 4 } }),
    )
    .await
}

/// `ListFunctionVersionsByCapacityProvider` on the provider `name`.
async fn list_versions_on_provider(router: &MultiServiceRouter, name: &str) -> Answer {
    json_call(
        router,
        "GET",
        &format!("{CAPACITY_PROVIDERS_URI}/{name}/function-versions?MaxItems=50"),
        &Value::Null,
    )
    .await
}

/// Create the function `name`, publishing version 1, naming `provider` when it runs on
/// one.
async fn create_managed_function(
    router: &MultiServiceRouter,
    name: &str,
    provider: Option<&str>,
) -> Answer {
    let mut body = json!({
        "FunctionName": name,
        "Role": ROLE,
        "Runtime": "python3.12",
        "Handler": "index.handler",
        "Publish": true,
        "Code": { "ZipFile": zip_file(&package_bytes(64)) },
    });
    if let Some(provider) = provider {
        body["CapacityProviderConfig"] = json!({
            "LambdaManagedInstancesCapacityProviderConfig": {
                "CapacityProviderArn": provider_arn(provider),
            },
        });
    }
    let created = create_function(router, &body).await;
    assert_eq!(
        created.status,
        201,
        "case {name}: CreateFunction must answer 201, got {} {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    created
}

/// One row: the function under test, the settled `State` its unpublished configuration
/// reports, and the settled `State` its published version reports.
struct StateCase {
    case: &'static str,
    function: &'static str,
    provider: Option<&'static str>,
    settled: &'static str,
    settled_version: &'static str,
}

/// The rows: a function that runs on a capacity provider, and one that runs on none.
fn state_cases() -> Vec<StateCase> {
    vec![
        StateCase {
            case: "a function managed by a capacity provider",
            function: "managed-function",
            provider: Some(PROVIDER),
            settled: STATE_ACTIVE_NON_INVOCABLE,
            settled_version: STATE_ACTIVE,
        },
        StateCase {
            case: "a function that runs on no capacity provider",
            function: "ordinary-function",
            provider: None,
            settled: STATE_ACTIVE,
            settled_version: STATE_ACTIVE,
        },
    ]
}

#[tokio::test]
async fn every_surface_reports_one_state_for_a_capacity_provider_managed_function() {
    for case in state_cases() {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let created = create_provider(&router, PROVIDER).await;
        assert_eq!(
            created.status,
            202,
            "case {:?}: CreateCapacityProvider must answer 202, got {} {}",
            case.case,
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let create_answer = create_managed_function(&router, case.function, case.provider).await;

        // Before the clock settles the function it is `Pending`, whatever it will be
        // invocable as: the projection narrows the SETTLED state only.
        assert_eq!(
            create_answer.string("State"),
            Some(STATE_PENDING.to_string()),
            "case {:?}: a freshly created function must be {STATE_PENDING}, got {}",
            case.case,
            String::from_utf8_lossy(&create_answer.body)
        );
        let early = get_configuration(&router, case.function, "").await;
        assert_eq!(
            early.string("State"),
            Some(STATE_PENDING.to_string()),
            "case {:?}: a read before the activation is due must be {STATE_PENDING}, got {}",
            case.case,
            String::from_utf8_lossy(&early.body)
        );

        make_activation_due(&engine);

        // The unpublished configuration is what the projection narrows: an unqualified
        // read and an explicit `$LATEST` both address it, while a published version
        // reports the lifecycle state it has.
        for (surface, expected, answer) in [
            (
                "GetFunctionConfiguration unqualified",
                case.settled,
                get_configuration(&router, case.function, "").await,
            ),
            (
                "GetFunctionConfiguration at $LATEST",
                case.settled,
                get_configuration(&router, case.function, "?Qualifier=$LATEST").await,
            ),
            (
                "GetFunctionConfiguration at version 1",
                case.settled_version,
                get_configuration(&router, case.function, "?Qualifier=1").await,
            ),
            (
                "GetFunctionConfiguration by the version ARN",
                case.settled_version,
                get_configuration(&router, &format!("{}:1", function_arn(case.function)), "").await,
            ),
        ] {
            assert_eq!(
                (answer.status, answer.string("State")),
                (200, Some(expected.to_string())),
                "case {:?}: {surface} must report {expected}, got {} {}",
                case.case,
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
        }

        let fetched = get_function(&router, case.function).await;
        assert_eq!(
            (
                fetched.status,
                fetched.json()["Configuration"]["State"]
                    .as_str()
                    .map(str::to_string)
            ),
            (200, Some(case.settled.to_string())),
            "case {:?}: GetFunction must report {}, got {} {}",
            case.case,
            case.settled,
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );

        // "The current state of the function after applying the scaling configuration":
        // the state a read of the FUNCTION reports, whichever published qualifier the
        // bounds were set on, because that state belongs to the function. The operation
        // "Sets the scaling configuration for a Lambda Managed Instances function", so a
        // function that runs on no capacity provider is refused instead of reporting a
        // state at all.
        for qualifier in ["1", "$LATEST.PUBLISHED"] {
            let applied = put_scaling_config(&router, case.function, qualifier).await;
            if case.provider.is_none() {
                assert_eq!(
                    applied.status,
                    403,
                    "case {:?}: PutFunctionScalingConfig at {qualifier} must be refused 403, \
                     got {} {}",
                    case.case,
                    applied.status,
                    String::from_utf8_lossy(&applied.body)
                );
                continue;
            }
            assert_eq!(
                (applied.status, applied.string("FunctionState")),
                (202, Some(case.settled.to_string())),
                "case {:?}: PutFunctionScalingConfig at {qualifier} must report {}, got {} {}",
                case.case,
                case.settled,
                applied.status,
                String::from_utf8_lossy(&applied.body)
            );
        }

        // And the listing of the provider's own versions reports the lifecycle state of
        // each version it reports: a published version registered with the provider is the
        // one thing that provider runs, so it is not the configuration the projection
        // narrows.
        let on_provider = list_versions_on_provider(&router, PROVIDER).await;
        let listed_states: Vec<String> = on_provider.json()["FunctionVersions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|entry| entry.get("State").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        let expected = match case.provider {
            // The function's published version runs on the provider; its unpublished
            // configuration is not one of the versions this listing reports.
            Some(_) => vec![case.settled_version.to_string()],
            None => Vec::new(),
        };
        assert_eq!(
            listed_states,
            expected,
            "case {:?}: expected {expected:?} from ListFunctionVersionsByCapacityProvider, got {}",
            case.case,
            String::from_utf8_lossy(&on_provider.body)
        );
    }
}
