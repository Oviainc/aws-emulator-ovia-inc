// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateCapacityProvider` ("Updates the configuration of an
//! existing capacity provider") and `DeleteCapacityProvider` ("Deletes a capacity
//! provider. You cannot delete a capacity provider that is currently being used by Lambda
//! functions").
//!
//! The rule under test for the update: it rewrites, on the ONE stored provider its
//! `CapacityProviderName` addresses in either documented form, exactly the configuration
//! members the request states — replacing each one WHOLE — leaves every member it omits at
//! its stored value, keeps the provider's identity and its clock-owed lifecycle, and
//! answers the record it wrote, so a following `GetCapacityProvider` reports the same
//! thing; a provider the account holds none of is the modelled 404 and a provider whose
//! deletion is already in progress is the modelled 409.
//!
//! The rule under test for the delete: it moves that same record into the model's
//! `Deleting` state and answers it, the removal itself being owed by the clock, and it
//! refuses a provider a function version is configured to run on with the modelled 409.
//!
//! Every row drives real HTTP requests through the router and reads the provider back
//! through `GetCapacityProvider`, so an answer that was never persisted fails here. The
//! rows differ along what the rule generalises over: which members the update states, the
//! configuration the provider was created with, the `CapacityProviderName` form it is
//! addressed by, whether its activation is already due, the scaling mode, `MaxVCpuCount`
//! and `TargetValue` at each published bound, the number of scaling policies and the
//! number of propagated tags.
//!
//! The clock-driven half — the activation a create owes and the removal a delete owes — is
//! reached by moving the stored due instant into the past, so nothing here sleeps.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{
    create_function, json_call, package_bytes, router_on, zip_file, Answer, ACCOUNT, REGION, ROLE,
};

/// Where the capacity-provider operations are bound, from
/// `smithy_query(query="operation", operation="UpdateCapacityProvider")`
/// (`PUT /2025-11-30/capacity-providers/{CapacityProviderName}`); the create is a `POST`
/// on the collection itself.
const CAPACITY_PROVIDERS_URI: &str = "/2025-11-30/capacity-providers";

/// The `CapacityProviderState` values this suite expects, from the model's
/// `CapacityProviderState` enum (`Pending | Active | Failed | Deleting`).
const STATE_PENDING: &str = "Pending";
/// See [`STATE_PENDING`].
const STATE_ACTIVE: &str = "Active";
/// See [`STATE_PENDING`].
const STATE_DELETING: &str = "Deleting";

/// The one `CapacityProviderPredefinedMetricType` the model declares, from
/// `smithy_query(query="shape", shape="CapacityProviderPredefinedMetricType")`.
const PREDEFINED_METRIC: &str = "LambdaCapacityProviderAverageCPUUtilization";

/// The bounds the model puts on `CapacityProviderScalingConfig$MaxVCpuCount`
/// (`@range(min: 2, max: 15000)`), from
/// `smithy_query(query="shape", shape="CapacityProviderScalingConfig")`.
const MAX_VCPU_MIN: i64 = 2;
const MAX_VCPU_MAX: i64 = 15_000;

/// The bounds the model puts on `TargetTrackingScalingPolicy$TargetValue`
/// (`@range(min: 0, max: 100)`), from
/// `smithy_query(query="shape", shape="TargetTrackingScalingPolicy")`.
const TARGET_VALUE_MIN: f64 = 0.0;
const TARGET_VALUE_MAX: f64 = 100.0;

/// The bounds the model puts on `CapacityProviderScalingConfig$ScalingPolicies`
/// (`@length(min: 1, max: 10)`), from the same shape.
const SCALING_POLICIES_MAX: usize = 10;

/// The most tags `PropagateTags$ExplicitTags` admits ("You can specify up to 40 tags",
/// `@length(min: 0, max: 40)`), from `smithy_query(query="shape", shape="PropagateTags")`.
const EXPLICIT_TAGS_MAX: usize = 40;

/// The members `UpdateCapacityProvider` takes beside the name it addresses, from
/// `smithy_query(query="operation", operation="UpdateCapacityProvider")` — which is
/// exactly the set an update may replace.
const UPDATABLE_MEMBERS: [&str; 3] = [
    "CapacityProviderScalingConfig",
    "PropagateTags",
    "TelemetryConfig",
];

/// A well-formed operator role ARN, satisfying the model's `RoleArn` pattern.
const OPERATOR_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-capacity";

/// A well-formed KMS key ARN, satisfying the model's `KMSKeyArnNonEmpty` pattern.
const KMS_KEY: &str =
    "arn:aws:kms:us-east-1:000000000000:key/0b1c2d3e-4f56-7890-abcd-ef1234567890";

/// The ARN a capacity provider of `name` carries, as the model's `CapacityProviderArn`
/// pattern spells it.
fn provider_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:capacity-provider:{name}")
}

/// `CreateCapacityProvider`.
async fn create_provider(router: &MultiServiceRouter, body: &Value) -> Answer {
    json_call(router, "POST", CAPACITY_PROVIDERS_URI, body).await
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

/// The `CapacityProvider` member of an answer, which every one of these operations
/// answers with.
fn provider_of(answer: &Answer) -> Value {
    answer
        .json()
        .get("CapacityProvider")
        .cloned()
        .unwrap_or(Value::Null)
}

/// A `String` member of the answered `CapacityProvider`.
fn provider_string(answer: &Answer, member: &str) -> Option<String> {
    provider_of(answer)
        .get(member)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The response body text, for a failure message.
fn text(answer: &Answer) -> String {
    String::from_utf8_lossy(&answer.body).to_string()
}

/// Drop the JSON members that are absent and read every number as the one JSON number
/// type, so a comparison states only the members that were configured — a member the
/// emulator invented shows up as a difference — and `2` and `2.0`, which JSON does not
/// distinguish, do not.
fn normalized(value: &Value) -> Value {
    match value {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(_, member)| !member.is_null())
                .map(|(name, member)| (name.clone(), normalized(member)))
                .collect(),
        ),
        Value::Array(members) => Value::Array(members.iter().map(normalized).collect()),
        Value::Number(number) => number
            .as_f64()
            .and_then(serde_json::Number::from_f64)
            .map_or_else(|| value.clone(), Value::Number),
        other => other.clone(),
    }
}

/// The configuration members of an answered `CapacityProvider`.
fn reported_configuration(answer: &Answer) -> Value {
    let provider = provider_of(answer);
    normalized(&json!({
        "VpcConfig": provider["VpcConfig"].clone(),
        "PermissionsConfig": provider["PermissionsConfig"].clone(),
        "InstanceRequirements": provider["InstanceRequirements"].clone(),
        "CapacityProviderScalingConfig": provider["CapacityProviderScalingConfig"].clone(),
        "KmsKeyArn": provider["KmsKeyArn"].clone(),
        "PropagateTags": provider["PropagateTags"].clone(),
        "TelemetryConfig": provider["TelemetryConfig"].clone(),
    }))
}

/// What the rule says the provider must report once `update` has been applied to the
/// provider `create` created: the created configuration, with each member the update
/// STATED replacing the stored one whole, and every member it omitted left alone.
fn expected_configuration(create: &Value, update: &Value) -> Value {
    let mut expected = json!({
        "VpcConfig": create["VpcConfig"].clone(),
        "PermissionsConfig": create["PermissionsConfig"].clone(),
        "InstanceRequirements": create["InstanceRequirements"].clone(),
        "CapacityProviderScalingConfig": create["CapacityProviderScalingConfig"].clone(),
        "KmsKeyArn": create["KmsKeyArn"].clone(),
        "PropagateTags": create["PropagateTags"].clone(),
        "TelemetryConfig": create["TelemetryConfig"].clone(),
    });
    for member in UPDATABLE_MEMBERS {
        if let Some(value) = update.get(member) {
            expected[member] = value.clone();
        }
    }
    normalized(&expected)
}

/// `count` subnet ids satisfying the model's `SubnetId` pattern (`^subnet-[0-9a-z]*$`).
fn subnets(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!(format!("subnet-{:08x}", 0x1000_0000usize + index)))
            .collect(),
    )
}

/// `count` scaling policies, each at a distinct target value inside the modelled range,
/// so a list of them is a list of distinct entries.
fn scaling_policies(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| {
                json!({
                    "PredefinedMetricType": PREDEFINED_METRIC,
                    // Inside `@range(min: 0, max: 100)` for every index below the
                    // modelled maximum length of the list.
                    "TargetValue": 1.5 + (index as f64),
                })
            })
            .collect(),
    )
}

/// `count` tags for `PropagateTags$ExplicitTags`.
fn explicit_tags(count: usize) -> Value {
    Value::Object(
        (0..count)
            .map(|index| (format!("tag-{index}"), json!(format!("value-{index}"))))
            .collect(),
    )
}

/// A create request body for `name` with the members of `extras` merged in.
fn create_request(name: &str, extras: &Value) -> Value {
    let mut body = json!({ "CapacityProviderName": name });
    if let (Value::Object(target), Value::Object(members)) = (&mut body, extras) {
        for (member, value) in members {
            target.insert(member.clone(), value.clone());
        }
    }
    body
}

/// The required configuration alone: the smallest provider the model admits.
fn minimal_base() -> Value {
    json!({
        "VpcConfig": { "SubnetIds": subnets(1), "SecurityGroupIds": [] },
        "PermissionsConfig": { "CapacityProviderOperatorRoleArn": OPERATOR_ROLE },
    })
}

/// Every optional member stated, so a row can show which of them an update leaves alone.
fn full_base() -> Value {
    json!({
        "VpcConfig": { "SubnetIds": subnets(2), "SecurityGroupIds": ["sg-0123abcd"] },
        "PermissionsConfig": { "CapacityProviderOperatorRoleArn": OPERATOR_ROLE },
        "InstanceRequirements": {
            "Architectures": ["x86_64"],
            "ExcludedInstanceTypes": ["t2.*"],
        },
        "CapacityProviderScalingConfig": {
            "MaxVCpuCount": 64,
            "ScalingMode": "Auto",
            "ScalingPolicies": [{
                "PredefinedMetricType": PREDEFINED_METRIC,
                "TargetValue": 55.5,
            }],
        },
        "KmsKeyArn": KMS_KEY,
        "PropagateTags": { "Mode": "Explicit", "ExplicitTags": { "team": "platform" } },
        "TelemetryConfig": {
            "LoggingConfig": { "SystemLogLevel": "INFO", "LogGroup": "/aws/lambda/capacity" },
        },
    })
}

/// Move every stored provider's activation instant into the past, so the next read
/// materialises the `Pending` -> `Active` transition the clock owes. This is what waiting
/// would do, without waiting.
fn make_activation_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for provider in store.capacity_providers.values_mut() {
        provider.active_due = past;
    }
}

/// Move every stored provider's REMOVAL instant into the past, so the next read
/// materialises the removal a delete made the clock owe.
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

/// The names of every provider the store holds.
fn stored_names(engine: &Arc<engine::InMemoryEngine>) -> Vec<String> {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    store.capacity_providers.keys().cloned().collect()
}

/// One row of the update table.
struct UpdateCase {
    name: &'static str,
    /// The configuration the provider is created with.
    base: Value,
    /// The `UpdateCapacityProvider` request body.
    update: Value,
    /// Address the provider by its ARN rather than by its bare name.
    by_arn: bool,
    /// Bring the provider's activation forward before the update, so the row updates an
    /// `Active` provider rather than a `Pending` one.
    activate: bool,
}

/// The rows. Each differs in which members the update states, the configuration created,
/// the name form, the lifecycle state at the time of the update, or the bound a scaling
/// value sits at.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        // Replaces a stated scaling configuration whole, at the top of both published
        // ranges and in the other scaling mode.
        UpdateCase {
            name: "replaces the scaling configuration of a fully configured provider",
            base: full_base(),
            update: json!({
                "CapacityProviderScalingConfig": {
                    "MaxVCpuCount": MAX_VCPU_MAX,
                    "ScalingMode": "Manual",
                    "ScalingPolicies": [{
                        "PredefinedMetricType": PREDEFINED_METRIC,
                        "TargetValue": TARGET_VALUE_MAX,
                    }],
                },
            }),
            by_arn: false,
            activate: true,
        },
        // Adds a member the create never stated, at the bottom of both published ranges.
        UpdateCase {
            name: "adds a scaling configuration to a provider created without one",
            base: minimal_base(),
            update: json!({
                "CapacityProviderScalingConfig": {
                    "MaxVCpuCount": MAX_VCPU_MIN,
                    "ScalingMode": "Auto",
                    "ScalingPolicies": [{
                        "PredefinedMetricType": PREDEFINED_METRIC,
                        "TargetValue": TARGET_VALUE_MIN,
                    }],
                },
            }),
            by_arn: false,
            activate: false,
        },
        // A stated member is replaced WHOLE: the stored vCPU count and policies go with
        // the configuration they belonged to.
        UpdateCase {
            name: "a scaling configuration stating only a mode replaces the stored one whole",
            base: full_base(),
            update: json!({ "CapacityProviderScalingConfig": { "ScalingMode": "Manual" } }),
            by_arn: false,
            activate: true,
        },
        // The scaling policy list at its modelled maximum length.
        UpdateCase {
            name: "the scaling policy list at its modelled maximum",
            base: full_base(),
            update: json!({
                "CapacityProviderScalingConfig": {
                    "MaxVCpuCount": 256,
                    "ScalingMode": "Auto",
                    "ScalingPolicies": scaling_policies(SCALING_POLICIES_MAX),
                },
            }),
            by_arn: true,
            activate: true,
        },
        // Only the telemetry configuration, addressed by the provider's ARN.
        UpdateCase {
            name: "replaces only the telemetry configuration, by arn",
            base: full_base(),
            update: json!({
                "TelemetryConfig": {
                    "LoggingConfig": { "SystemLogLevel": "WARN", "LogGroup": "/aws/lambda/other" },
                },
            }),
            by_arn: true,
            activate: true,
        },
        // Only the tag propagation, turned off.
        UpdateCase {
            name: "replaces only the tag propagation, with propagation disabled",
            base: full_base(),
            update: json!({ "PropagateTags": { "Mode": "None" } }),
            by_arn: false,
            activate: false,
        },
        // The explicit tag map at its documented maximum.
        UpdateCase {
            name: "tag propagation at its documented maximum number of explicit tags",
            base: minimal_base(),
            update: json!({
                "PropagateTags": {
                    "Mode": "Explicit",
                    "ExplicitTags": explicit_tags(EXPLICIT_TAGS_MAX),
                },
            }),
            by_arn: false,
            activate: true,
        },
        // Every member the operation models, at once, on a provider created with none of
        // them.
        UpdateCase {
            name: "states every member the operation models at once",
            base: minimal_base(),
            update: json!({
                "CapacityProviderScalingConfig": {
                    "MaxVCpuCount": 1024,
                    "ScalingMode": "Manual",
                    "ScalingPolicies": scaling_policies(3),
                },
                "PropagateTags": { "Mode": "Explicit", "ExplicitTags": explicit_tags(2) },
                "TelemetryConfig": { "LoggingConfig": { "SystemLogLevel": "DEBUG" } },
            }),
            by_arn: true,
            activate: false,
        },
        // A request that states nothing changes nothing.
        UpdateCase {
            name: "a request that states no member leaves every member alone",
            base: full_base(),
            update: json!({}),
            by_arn: false,
            activate: true,
        },
        // A target value whose decimal expansion does not terminate in binary: it is
        // stored as given and reported back to the bit.
        UpdateCase {
            name: "a fractional target value is reported back unchanged",
            base: full_base(),
            update: json!({
                "CapacityProviderScalingConfig": {
                    "MaxVCpuCount": 3,
                    "ScalingPolicies": [{
                        "PredefinedMetricType": PREDEFINED_METRIC,
                        "TargetValue": 33.333_333_333_333_336,
                    }],
                },
            }),
            by_arn: false,
            activate: true,
        },
    ]
}

#[tokio::test]
async fn an_update_rewrites_the_members_it_states_and_leaves_the_rest_alone() {
    for case in update_cases() {
        let label = case.name;
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);

        let name = "update-target";
        let created_request = create_request(name, &case.base);
        let created = create_provider(&router, &created_request).await;
        assert_eq!(
            created.status,
            202,
            "case {label:?}: the create must be accepted, got {} {}",
            created.status,
            text(&created)
        );
        let created_last_modified = provider_string(&created, "LastModified")
            .unwrap_or_else(|| panic!("case {label:?}: the create must report LastModified"));
        if case.activate {
            make_activation_due(&engine);
        }

        let arn = provider_arn(name);
        let reference = if case.by_arn { arn.clone() } else { name.to_string() };
        let updated = update_provider(&router, &reference, &case.update).await;
        assert_eq!(
            updated.status,
            202,
            "case {label:?}: the update must be accepted, got {} {}",
            updated.status,
            text(&updated)
        );

        let expected = expected_configuration(&created_request, &case.update);
        assert_eq!(
            reported_configuration(&updated),
            expected,
            "case {label:?}: the update must answer the configuration {expected}, got {}",
            text(&updated)
        );

        // The identity of the provider is untouched by a configuration change.
        assert_eq!(
            provider_string(&updated, "CapacityProviderArn"),
            Some(arn.clone()),
            "case {label:?}: expected the ARN minted at creation {arn:?}, got {}",
            text(&updated)
        );

        // The lifecycle is owed by the clock, never by the update: a provider whose
        // activation was already due reports `Active`, and one whose was not reports the
        // state the clock owes it.
        let state = provider_string(&updated, "State").unwrap_or_default();
        if case.activate {
            assert_eq!(
                state, STATE_ACTIVE,
                "case {label:?}: expected {STATE_ACTIVE} once the activation was due, got {}",
                text(&updated)
            );
        } else {
            assert!(
                state == STATE_PENDING || state == STATE_ACTIVE,
                "case {label:?}: expected the state the clock owed, got {state:?}"
            );
        }

        // "The date and time when the capacity provider was last modified": a call that
        // changed something moved it, and one that changed nothing left it.
        let update_last_modified = provider_string(&updated, "LastModified")
            .unwrap_or_else(|| panic!("case {label:?}: the update must report LastModified"));
        let states_a_member = UPDATABLE_MEMBERS
            .iter()
            .any(|member| case.update.get(member).is_some());
        if states_a_member {
            assert!(
                update_last_modified >= created_last_modified,
                "case {label:?}: expected LastModified at or after {created_last_modified:?}, \
                 got {update_last_modified:?}"
            );
        } else {
            assert_eq!(
                update_last_modified, created_last_modified,
                "case {label:?}: a call that changed nothing must leave LastModified at \
                 {created_last_modified:?}, got {update_last_modified:?}"
            );
        }

        // The update PERSISTED: the read reports the same configuration, and does so
        // stably, by either documented name form.
        for (form, addressed) in [("bare name", name.to_string()), ("arn", arn.clone())] {
            let fetched = get_provider(&router, &addressed).await;
            assert_eq!(
                (fetched.status, reported_configuration(&fetched)),
                (200, expected.clone()),
                "case {label:?}: a read by {form} must report {expected}, got {} {}",
                fetched.status,
                text(&fetched)
            );
            assert_eq!(
                (
                    provider_string(&fetched, "CapacityProviderArn"),
                    provider_string(&fetched, "LastModified"),
                ),
                (Some(arn.clone()), Some(update_last_modified.clone())),
                "case {label:?}: a read by {form} must report the stored ARN and \
                 LastModified, got {}",
                text(&fetched)
            );
        }

        // One record, under the one name, however it was addressed.
        assert_eq!(
            stored_names(&engine),
            vec![name.to_string()],
            "case {label:?}: the update must leave exactly the one provider"
        );
    }
}

/// One row of the refusal table: what to call, and the status and error code it must be
/// answered with.
struct RefusalCase {
    name: &'static str,
    method: &'static str,
    uri: String,
    body: Value,
    status: u16,
    code: Option<&'static str>,
}

#[tokio::test]
async fn an_update_no_capacity_provider_can_satisfy_is_refused_and_persists_nothing() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let name = "refusal-target";
    let created_request = create_request(name, &full_base());
    let created = create_provider(&router, &created_request).await;
    assert_eq!(
        created.status,
        202,
        "the create must be accepted, got {} {}",
        created.status,
        text(&created)
    );
    make_activation_due(&engine);
    let stored = reported_configuration(&get_provider(&router, name).await);

    let cases = vec![
        // A provider this account holds none of, by either documented name form.
        RefusalCase {
            name: "an update of a provider that does not exist",
            method: "PUT",
            uri: format!("{CAPACITY_PROVIDERS_URI}/absent-provider"),
            body: json!({ "CapacityProviderScalingConfig": { "MaxVCpuCount": 8 } }),
            status: 404,
            code: Some("ResourceNotFoundException"),
        },
        RefusalCase {
            name: "an update addressed by the arn of a provider that does not exist",
            method: "PUT",
            uri: format!("{CAPACITY_PROVIDERS_URI}/{}", provider_arn("absent-provider")),
            body: json!({ "PropagateTags": { "Mode": "None" } }),
            status: 404,
            code: Some("ResourceNotFoundException"),
        },
        // An ARN of another Lambda resource names no capacity provider at all. The
        // modelled `CapacityProviderName` pattern is what first refuses a value this far
        // from either documented form, so only the 400 is asserted here.
        RefusalCase {
            name: "an update addressed by the arn of another lambda resource",
            method: "PUT",
            uri: format!(
                "{CAPACITY_PROVIDERS_URI}/arn:aws:lambda:{REGION}:{ACCOUNT}:function:not-a-provider"
            ),
            body: json!({ "PropagateTags": { "Mode": "None" } }),
            status: 400,
            code: None,
        },
        // A scaling value one step outside each published range: the modelled
        // constraints are refused before the handler stores anything.
        RefusalCase {
            name: "a vcpu count one step below the modelled minimum",
            method: "PUT",
            uri: format!("{CAPACITY_PROVIDERS_URI}/{name}"),
            body: json!({
                "CapacityProviderScalingConfig": { "MaxVCpuCount": MAX_VCPU_MIN - 1 },
            }),
            status: 400,
            code: None,
        },
        RefusalCase {
            name: "a vcpu count one step above the modelled maximum",
            method: "PUT",
            uri: format!("{CAPACITY_PROVIDERS_URI}/{name}"),
            body: json!({
                "CapacityProviderScalingConfig": { "MaxVCpuCount": MAX_VCPU_MAX + 1 },
            }),
            status: 400,
            code: None,
        },
        RefusalCase {
            name: "a target value above the modelled maximum",
            method: "PUT",
            uri: format!("{CAPACITY_PROVIDERS_URI}/{name}"),
            body: json!({
                "CapacityProviderScalingConfig": {
                    "ScalingPolicies": [{
                        "PredefinedMetricType": PREDEFINED_METRIC,
                        "TargetValue": TARGET_VALUE_MAX + 0.5,
                    }],
                },
            }),
            status: 400,
            code: None,
        },
        RefusalCase {
            name: "a target value below the modelled minimum",
            method: "PUT",
            uri: format!("{CAPACITY_PROVIDERS_URI}/{name}"),
            body: json!({
                "CapacityProviderScalingConfig": {
                    "ScalingPolicies": [{
                        "PredefinedMetricType": PREDEFINED_METRIC,
                        "TargetValue": TARGET_VALUE_MIN - 0.5,
                    }],
                },
            }),
            status: 400,
            code: None,
        },
        RefusalCase {
            name: "an empty scaling policy list, below the modelled minimum length",
            method: "PUT",
            uri: format!("{CAPACITY_PROVIDERS_URI}/{name}"),
            body: json!({ "CapacityProviderScalingConfig": { "ScalingPolicies": [] } }),
            status: 400,
            code: None,
        },
    ];

    for case in cases {
        let answer = json_call(&router, case.method, &case.uri, &case.body).await;
        assert_eq!(
            answer.status,
            case.status,
            "case {:?}: expected HTTP {}, got {} {}",
            case.name,
            case.status,
            answer.status,
            text(&answer)
        );
        if let Some(code) = case.code {
            assert!(
                answer.error_code().contains(code),
                "case {:?}: expected the {code} error code, got {:?}",
                case.name,
                answer.error_code()
            );
        }
        // A refused update persists nothing: the provider still reports what it held.
        let fetched = get_provider(&router, name).await;
        assert_eq!(
            (fetched.status, reported_configuration(&fetched)),
            (200, stored.clone()),
            "case {:?}: a refused update must leave the provider as it was, got {} {}",
            case.name,
            fetched.status,
            text(&fetched)
        );
    }

    assert_eq!(
        stored_names(&engine),
        vec![name.to_string()],
        "a refused update persists no new provider"
    );
}

#[tokio::test]
async fn a_delete_moves_the_provider_into_deleting_and_the_clock_removes_it() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let name = "deleted-provider";
    let created = create_provider(&router, &create_request(name, &full_base())).await;
    assert_eq!(
        created.status,
        202,
        "the create must be accepted, got {} {}",
        created.status,
        text(&created)
    );
    make_activation_due(&engine);

    let deleted = delete_provider(&router, name).await;
    assert_eq!(
        (deleted.status, provider_string(&deleted, "State")),
        (202, Some(STATE_DELETING.to_string())),
        "the delete must answer the provider in {STATE_DELETING}, got {} {}",
        deleted.status,
        text(&deleted)
    );
    assert_eq!(
        provider_string(&deleted, "CapacityProviderArn"),
        Some(provider_arn(name)),
        "the delete must answer the provider it deleted, got {}",
        text(&deleted)
    );

    // The removal is owed by the CLOCK, so until it is due the provider is still there,
    // reported in the state the delete persisted.
    let fetched = get_provider(&router, name).await;
    assert_eq!(
        (fetched.status, provider_string(&fetched, "State")),
        (200, Some(STATE_DELETING.to_string())),
        "a read before the removal is due must report {STATE_DELETING}, got {} {}",
        fetched.status,
        text(&fetched)
    );

    // `DeleteCapacityProvider` is `@idempotent`: a repeat answers the same record.
    let again = delete_provider(&router, &provider_arn(name)).await;
    assert_eq!(
        (again.status, provider_string(&again, "State")),
        (202, Some(STATE_DELETING.to_string())),
        "a repeated delete must answer the same record, got {} {}",
        again.status,
        text(&again)
    );

    // An update of a provider whose deletion is in progress is the modelled conflict.
    let refused = update_provider(
        &router,
        name,
        &json!({ "CapacityProviderScalingConfig": { "MaxVCpuCount": 8 } }),
    )
    .await;
    assert_eq!(
        refused.status,
        409,
        "an update during a deletion must be refused 409, got {} {}",
        refused.status,
        text(&refused)
    );
    assert!(
        refused.error_code().contains("ResourceConflictException"),
        "an update during a deletion must be the modelled conflict, got {:?}",
        refused.error_code()
    );

    // Once the clock has made the removal due, the provider is gone: the read is the
    // modelled 404 and the name is free again.
    make_removal_due(&engine);
    let gone = get_provider(&router, name).await;
    assert_eq!(
        gone.status,
        404,
        "a read once the removal was due must be 404, got {} {}",
        gone.status,
        text(&gone)
    );
    assert!(
        gone.error_code().contains("ResourceNotFoundException"),
        "a read of a removed provider must be the modelled 404, got {:?}",
        gone.error_code()
    );
    assert!(
        stored_names(&engine).is_empty(),
        "a removed provider must be gone from the store, got {:?}",
        stored_names(&engine)
    );

    let recreated = create_provider(&router, &create_request(name, &minimal_base())).await;
    assert_eq!(
        recreated.status,
        202,
        "the name of a removed provider must be free again, got {} {}",
        recreated.status,
        text(&recreated)
    );
}

#[tokio::test]
async fn a_capacity_provider_a_function_runs_on_cannot_be_deleted() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let name = "in-use-provider";
    let created = create_provider(&router, &create_request(name, &minimal_base())).await;
    assert_eq!(
        created.status,
        202,
        "the create must be accepted, got {} {}",
        created.status,
        text(&created)
    );
    make_activation_due(&engine);

    // A function names the provider it runs on through
    // `FunctionConfiguration$CapacityProviderConfig`, whose one
    // `LambdaManagedInstancesCapacityProviderConfig` carries the provider's ARN.
    let function = create_function(
        &router,
        &json!({
            "FunctionName": "runs-on-the-provider",
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": zip_file(&package_bytes(64)) },
            "CapacityProviderConfig": {
                "LambdaManagedInstancesCapacityProviderConfig": {
                    "CapacityProviderArn": provider_arn(name),
                },
            },
        }),
    )
    .await;
    assert_eq!(
        function.status,
        201,
        "the function must be created, got {} {}",
        function.status,
        text(&function)
    );

    let refused = delete_provider(&router, name).await;
    assert_eq!(
        refused.status,
        409,
        "a provider a function runs on must not be deletable, got {} {}",
        refused.status,
        text(&refused)
    );
    assert!(
        refused.error_code().contains("ResourceConflictException"),
        "a provider in use must be the modelled conflict, got {:?}",
        refused.error_code()
    );

    // The refusal left the provider as it was: still there, and not on its way out.
    let fetched = get_provider(&router, name).await;
    assert_eq!(
        (fetched.status, provider_string(&fetched, "State")),
        (200, Some(STATE_ACTIVE.to_string())),
        "a refused delete must leave the provider {STATE_ACTIVE}, got {} {}",
        fetched.status,
        text(&fetched)
    );
    assert_eq!(
        stored_names(&engine),
        vec![name.to_string()],
        "a refused delete must leave the provider stored"
    );
}

#[tokio::test]
async fn a_target_value_round_trips_to_the_bit_through_an_update() {
    // The rule: a `TargetValue` inside `MetricTargetValue`'s `@range(min: 0, max: 100)`
    // is stored as the double it is and reported back unchanged — including the ones
    // whose decimal form has no finite binary expansion. The values stay inside 15
    // significant digits, which is where `serde_json`'s own parser — the one THIS test
    // reads the answer with — is itself exact.
    let values: [f64; 7] = [
        TARGET_VALUE_MIN,
        f64::from_bits(TARGET_VALUE_MIN.to_bits() + 1),
        0.1,
        1.0 / 3.0,
        62.5,
        99.75,
        TARGET_VALUE_MAX,
    ];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let name = "precision-provider";
    let created = create_provider(&router, &create_request(name, &minimal_base())).await;
    assert_eq!(
        created.status,
        202,
        "the create must be accepted, got {} {}",
        created.status,
        text(&created)
    );

    for value in values {
        let updated = update_provider(
            &router,
            name,
            &json!({
                "CapacityProviderScalingConfig": {
                    "ScalingPolicies": [{
                        "PredefinedMetricType": PREDEFINED_METRIC,
                        "TargetValue": value,
                    }],
                },
            }),
        )
        .await;
        assert_eq!(
            updated.status,
            202,
            "case {value:?}: the update must be accepted, got {} {}",
            updated.status,
            text(&updated)
        );
        for (form, answer) in [
            ("the update", updated),
            ("a later read", get_provider(&router, name).await),
        ] {
            let reported = provider_of(&answer)["CapacityProviderScalingConfig"]
                ["ScalingPolicies"][0]["TargetValue"]
                .as_f64();
            assert_eq!(
                reported,
                Some(value),
                "case {value:?}: {form} must report the target value unchanged, got {}",
                text(&answer)
            );
        }
    }
}
