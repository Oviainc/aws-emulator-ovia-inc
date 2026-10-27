// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the sentences the capacity-provider surface REFUSES with.
//!
//! The rules under test:
//!
//! * `DeleteCapacityProvider` refuses a provider that functions are still associated with
//!   ("You cannot delete a capacity provider that is currently being used by Lambda
//!   functions") with the modelled `ResourceConflictException` (409), and the sentence
//!   states the use that blocks it: how many FUNCTIONS hold an association — one function
//!   counts once however many of its configurations name the provider — and the ARN of the
//!   association the caller has to remove, which is the qualified ARN of the published
//!   version that runs on the provider.
//! * `UpdateCapacityProvider` refuses a provider whose deletion is already under way with
//!   the same modelled 409, and the sentence is a fixed one that tells the caller to wait
//!   for the operation under way and try again; it states no internal lifecycle value.
//!
//! Every row drives real HTTP requests through the router and reads the provider back
//! afterwards, so a refusal that changed something fails here. The rows differ along what
//! the rules generalise over: how many functions hold an association, whether a function
//! holds one through its unpublished configuration, a published version or both, how the
//! provider is addressed, and which members the refused update states.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{
    create_function, json_call, make_activation_due, package_bytes, router_on, zip_file, Answer,
    ACCOUNT, REGION, ROLE,
};

/// Where the capacity-provider operations are bound
/// (`smithy_query(query="operation", operation="DeleteCapacityProvider")`:
/// `DELETE /2025-11-30/capacity-providers/{CapacityProviderName}`).
const CAPACITY_PROVIDERS_URI: &str = "/2025-11-30/capacity-providers";

/// Where `CreateFunction` and `PublishVersion` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The ARN a capacity provider of `name` carries, as the model's `CapacityProviderArn`
/// pattern spells it.
fn provider_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:capacity-provider:{name}")
}

/// The unqualified ARN a function of `name` carries.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// The `message` of a wire error, under either casing the JSON envelope uses.
fn message(answer: &Answer) -> String {
    answer
        .string("message")
        .or_else(|| answer.string("Message"))
        .unwrap_or_else(|| String::from_utf8_lossy(&answer.body).to_string())
}

/// A well-formed capacity-provider operator role ARN, satisfying the model's `RoleArn`
/// pattern.
const OPERATOR_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-capacity";

/// `CreateCapacityProvider` of a provider named `name`, stating the required
/// configuration alone.
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

/// `DeleteCapacityProvider` on the provider `reference` names.
async fn delete_provider(router: &MultiServiceRouter, reference: &str) -> Answer {
    json_call(
        router,
        "DELETE",
        &format!("{CAPACITY_PROVIDERS_URI}/{reference}"),
        &Value::Null,
    )
    .await
}

/// `UpdateCapacityProvider` on the provider `reference` names.
async fn update_provider(router: &MultiServiceRouter, reference: &str, body: &Value) -> Answer {
    json_call(
        router,
        "PUT",
        &format!("{CAPACITY_PROVIDERS_URI}/{reference}"),
        body,
    )
    .await
}

/// `GetCapacityProvider` on the provider `reference` names.
async fn get_provider(router: &MultiServiceRouter, reference: &str) -> Answer {
    json_call(
        router,
        "GET",
        &format!("{CAPACITY_PROVIDERS_URI}/{reference}"),
        &Value::Null,
    )
    .await
}

/// Create the function `name` configured to run on the capacity provider `provider`,
/// publishing a version when `publish` asks for one.
async fn create_managed_function(
    router: &MultiServiceRouter,
    name: &str,
    provider: &str,
    publish: bool,
) {
    let created = create_function(
        router,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Publish": publish,
            "Code": { "ZipFile": zip_file(&package_bytes(64)) },
            "CapacityProviderConfig": {
                "LambdaManagedInstancesCapacityProviderConfig": {
                    "CapacityProviderArn": provider_arn(provider),
                },
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {name}: CreateFunction must answer 201, got {} {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
}

/// Publish a version of `name`, so the function holds an association through a published
/// version as well as through its unpublished configuration.
async fn publish_version(router: &MultiServiceRouter, name: &str) {
    let published = json_call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/versions"),
        &json!({ "Description": "registered with the provider" }),
    )
    .await;
    assert_eq!(
        published.status,
        201,
        "case {name}: PublishVersion must answer 201, got {} {}",
        published.status,
        String::from_utf8_lossy(&published.body)
    );
}

/// One function of a delete row: its name, whether creating it publishes a version, and
/// whether a version is published afterwards.
struct ManagedFunction {
    name: &'static str,
    published_at_create: bool,
    publish_after: bool,
}

/// One row of the delete table: the functions associated with the provider, how the
/// provider is addressed, and the association the refusal must name.
struct DeleteCase {
    case: &'static str,
    functions: Vec<ManagedFunction>,
    reference: String,
    expected_count: usize,
    expected_association: String,
}

/// The rows: one function associated through its unpublished configuration alone, one
/// associated through a published version as well (the association the refusal names is
/// the version's), one addressed by the provider ARN rather than its name, and two
/// functions, which count as two.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            case: "one function, associated through its unpublished configuration only",
            functions: vec![ManagedFunction {
                name: "cp-unpublished",
                published_at_create: false,
                publish_after: false,
            }],
            reference: "delete-refusal".to_string(),
            expected_count: 1,
            expected_association: function_arn("cp-unpublished"),
        },
        DeleteCase {
            case: "one function with a published version registered with the provider",
            functions: vec![ManagedFunction {
                name: "cp-published",
                published_at_create: true,
                publish_after: false,
            }],
            reference: "delete-refusal".to_string(),
            expected_count: 1,
            expected_association: format!("{}:1", function_arn("cp-published")),
        },
        DeleteCase {
            case: "the same association, with the provider addressed by its ARN",
            functions: vec![ManagedFunction {
                name: "cp-by-arn",
                published_at_create: false,
                publish_after: true,
            }],
            reference: provider_arn("delete-refusal"),
            expected_count: 1,
            expected_association: format!("{}:1", function_arn("cp-by-arn")),
        },
        DeleteCase {
            case: "two functions, which are two associations",
            functions: vec![
                ManagedFunction {
                    name: "cp-first",
                    published_at_create: true,
                    publish_after: false,
                },
                ManagedFunction {
                    name: "cp-second",
                    published_at_create: true,
                    publish_after: false,
                },
            ],
            reference: "delete-refusal".to_string(),
            expected_count: 2,
            expected_association: format!("{}:1", function_arn("cp-first")),
        },
    ]
}

#[tokio::test]
async fn an_in_use_capacity_provider_is_refused_by_its_own_sentence() {
    for case in delete_cases() {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let provider = "delete-refusal";
        let created = create_provider(&router, provider).await;
        assert_eq!(
            created.status,
            202,
            "case {:?}: CreateCapacityProvider must answer 202, got {} {}",
            case.case,
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        for function in &case.functions {
            create_managed_function(&router, function.name, provider, function.published_at_create)
                .await;
            if function.publish_after {
                make_activation_due(&engine);
                publish_version(&router, function.name).await;
            }
        }
        make_activation_due(&engine);

        let refused = delete_provider(&router, &case.reference).await;
        assert_eq!(
            refused.status,
            409,
            "case {:?}: a provider still in use must be refused 409, got {} {}",
            case.case,
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert!(
            refused.error_code().contains("ResourceConflictException"),
            "case {:?}: expected the modelled ResourceConflictException, got {:?}",
            case.case,
            refused.error_code()
        );
        assert_eq!(
            message(&refused),
            format!(
                "The capacity provider is currently in use by {} function. To delete this \
                 capacity provider, first remove its association with {}.",
                case.expected_count, case.expected_association
            ),
            "case {:?}: unexpected refusal sentence",
            case.case
        );

        // The refusal changed nothing: the provider is still there and not on its way out.
        let fetched = get_provider(&router, provider).await;
        assert_eq!(
            (
                fetched.status,
                fetched.json()["CapacityProvider"]["State"]
                    .as_str()
                    .map(str::to_string)
            ),
            (200, Some("Active".to_string())),
            "case {:?}: a refused delete must leave the provider Active, got {} {}",
            case.case,
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
    }
}

#[tokio::test]
async fn an_update_during_a_deletion_is_refused_by_its_own_sentence() {
    // The rule has no variable part, so the rows vary what is asked for instead: each
    // member `UpdateCapacityProvider` can replace, and the two forms the provider is
    // addressed by. Every one of them earns the same fixed sentence.
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let provider = "mid-delete-refusal";
    let created = create_provider(&router, provider).await;
    assert_eq!(
        created.status,
        202,
        "CreateCapacityProvider must answer 202, got {} {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    make_activation_due(&engine);
    let deleted = delete_provider(&router, provider).await;
    assert_eq!(
        deleted.status,
        202,
        "DeleteCapacityProvider must be accepted, got {} {}",
        deleted.status,
        String::from_utf8_lossy(&deleted.body)
    );

    let requests = [
        (
            "the scaling configuration",
            json!({ "CapacityProviderScalingConfig": { "MaxVCpuCount": 30 } }),
        ),
        (
            "the propagated tags",
            json!({ "PropagateTags": { "Mode": "Explicit", "ExplicitTags": { "team": "platform" } } }),
        ),
        (
            "the telemetry configuration",
            json!({
                "TelemetryConfig": {
                    "LoggingConfig": {
                        "SystemLogLevel": "INFO",
                        "LogGroup": "/aws/lambda/capacity",
                    },
                },
            }),
        ),
        ("no member at all", json!({})),
    ];
    for (named, body) in requests {
        for (form, reference) in [
            ("the bare name", provider.to_string()),
            ("the provider ARN", provider_arn(provider)),
        ] {
            let refused = update_provider(&router, &reference, &body).await;
            assert_eq!(
                refused.status,
                409,
                "case {named} by {form}: an update during a deletion must be refused 409, got \
                 {} {}",
                refused.status,
                String::from_utf8_lossy(&refused.body)
            );
            assert!(
                refused.error_code().contains("ResourceConflictException"),
                "case {named} by {form}: expected the modelled ResourceConflictException, got \
                 {:?}",
                refused.error_code()
            );
            assert_eq!(
                message(&refused),
                "The capacity provider can't be updated because it's currently processing \
                 another operation. Wait for the current operation to complete and try again.",
                "case {named} by {form}: unexpected refusal sentence"
            );
        }
    }

    // The refused updates persisted nothing: the provider is still the one the delete
    // left, and the clock still owes its removal.
    let fetched = get_provider(&router, provider).await;
    assert_eq!(
        (
            fetched.status,
            fetched.json()["CapacityProvider"]["State"]
                .as_str()
                .map(str::to_string)
        ),
        (200, Some("Deleting".to_string())),
        "a refused update must leave the provider on its way out, got {} {}",
        fetched.status,
        String::from_utf8_lossy(&fetched.body)
    );
    assert!(
        fetched.json()["CapacityProvider"]["CapacityProviderScalingConfig"]["MaxVCpuCount"]
            .is_null(),
        "a refused update must persist none of its members, got {}",
        String::from_utf8_lossy(&fetched.body)
    );

    // And once the clock has made the removal due the provider is gone, so no state the
    // refusals left behind outlives it.
    make_removal_due(&engine);
    let gone = get_provider(&router, provider).await;
    assert_eq!(
        gone.status,
        404,
        "a read once the removal was due must be 404, got {} {}",
        gone.status,
        String::from_utf8_lossy(&gone.body)
    );
}

/// Move every stored provider's removal instant into the past, so the next request
/// materialises the removal the clock owes. This is what waiting would do, without
/// waiting.
fn make_removal_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for provider in store.capacity_providers.values_mut() {
        if provider.removal_due.is_some() {
            provider.removal_due = Some(past);
        }
    }
}
