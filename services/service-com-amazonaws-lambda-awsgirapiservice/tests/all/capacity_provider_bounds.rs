// SPDX-License-Identifier: CC0-1.0

//! The published bounds of a capacity provider's configuration, and what
//! `GetCapacityProvider` reports on either side of them.
//!
//! The rule under test: the capacity-provider slice persists and reports what the model
//! admits and nothing else — a value AT a published bound of
//! `CapacityProviderMaxVCpuCount`, `MetricTargetValue`, `CapacityProviderSubnetIds`,
//! `CapacityProviderSecurityGroupIds` or `CapacityProviderName` is stored and reported
//! back unchanged, and a value ONE STEP OUTSIDE that bound is refused with a 400 and
//! persists nothing, so a later `GetCapacityProvider` finds no provider. The provider's
//! lifecycle is owed by the CLOCK alone: it stays `Pending` however often it is read
//! before its activation is due, and reports `Active` from the instant the stored due
//! instant has passed — which this suite reaches by moving that instant into the past
//! rather than by sleeping. Either documented `CapacityProviderName` form names the one
//! record, so reading one provider by its bare name and by its ARN answers the same
//! thing.
//!
//! `capacity_providers.rs` covers the configuration a create persists for a provider that
//! is accepted; this suite is about the EDGES of that class — the accepted bound beside
//! the refused step past it — and about the reads that follow.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateCapacityProvider` is bound, from
/// `smithy_query(query="operation", operation="CreateCapacityProvider")`
/// (`POST /2025-11-30/capacity-providers`).
const CAPACITY_PROVIDERS_URI: &str = "/2025-11-30/capacity-providers";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in
/// `us-east-1`, which is what routes a request here and what region the ARNs carry.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed operator role ARN, satisfying the model's `RoleArn` pattern.
const OPERATOR_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-capacity";

/// The one `CapacityProviderPredefinedMetricType` the model declares.
const PREDEFINED_METRIC: &str = "LambdaCapacityProviderAverageCPUUtilization";

/// The bounds the model puts on `CapacityProviderScalingConfig$MaxVCpuCount`
/// (`@range(min: 2, max: 15000)`), from
/// `smithy_query(query="shape", shape="CapacityProviderMaxVCpuCount")`.
const MAX_VCPU_MIN: i64 = 2;
const MAX_VCPU_MAX: i64 = 15_000;

/// The bounds the model puts on `TargetTrackingScalingPolicy$TargetValue`
/// (`@range(min: 0, max: 100)`), from
/// `smithy_query(query="shape", shape="MetricTargetValue")`.
const TARGET_VALUE_MIN: f64 = 0.0;
const TARGET_VALUE_MAX: f64 = 100.0;

/// The bounds the model puts on the two `CapacityProviderVpcConfig` lists:
/// `CapacityProviderSubnetIds` is `@length(min: 1, max: 16)` and
/// `CapacityProviderSecurityGroupIds` is `@length(min: 0, max: 5)`, from
/// `smithy_query(query="shape", shape="CapacityProviderVpcConfig")`.
const SUBNETS_MIN: usize = 1;
const SUBNETS_MAX: usize = 16;
const SECURITY_GROUPS_MIN: usize = 0;
const SECURITY_GROUPS_MAX: usize = 5;

/// The bounds the model puts on `CapacityProviderName` (`@length(min: 1, max: 140)`),
/// from `smithy_query(query="shape", shape="CapacityProviderName")`.
const NAME_MAX_LEN: usize = 140;

/// The `CapacityProviderState` values this suite expects, from the model's
/// `CapacityProviderState` enum (`Pending | Active | Failed | Deleting`).
const STATE_PENDING: &str = "Pending";
/// See [`STATE_PENDING`].
const STATE_ACTIVE: &str = "Active";

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The `CapacityProvider` member of the body, which both operations answer with.
    fn provider(&self) -> Value {
        self.json()
            .get("CapacityProvider")
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// A `String` member of the answered `CapacityProvider`.
    fn provider_string(&self, member: &str) -> Option<String> {
        self.provider()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// `CreateCapacityProvider`.
async fn create_provider(router: &MultiServiceRouter, body: &Value) -> Answer {
    call(router, "POST", CAPACITY_PROVIDERS_URI, body).await
}

/// `GetCapacityProvider` on the provider `reference` names.
async fn get_provider(router: &MultiServiceRouter, reference: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{CAPACITY_PROVIDERS_URI}/{reference}"),
        &Value::Null,
    )
    .await
}

/// The ARN a capacity provider of `name` carries, as the model's `CapacityProviderArn`
/// pattern spells it.
fn provider_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:capacity-provider:{name}")
}

/// `count` subnet ids, each satisfying the model's `SubnetId` pattern
/// (`^subnet-[0-9a-z]*$`).
fn subnets(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!(format!("subnet-{:08x}", 0x3000_0000usize + index)))
            .collect(),
    )
}

/// `count` security group ids, each satisfying the model's `SecurityGroupId` pattern
/// (`^sg-[0-9a-zA-Z]*$`).
fn security_groups(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!(format!("sg-{:08X}", 0x4000_0000usize + index)))
            .collect(),
    )
}

/// A create request for `name` carrying only the two required members, which every row
/// then overrides the member it is about.
fn base_request(name: &str) -> Value {
    json!({
        "CapacityProviderName": name,
        "VpcConfig": {
            "SubnetIds": subnets(1),
            "SecurityGroupIds": security_groups(0),
        },
        "PermissionsConfig": { "CapacityProviderOperatorRoleArn": OPERATOR_ROLE },
    })
}

/// Apply a row's patch to a base request: a top-level member per entry.
fn patched(name: &str, patch: &Value) -> Value {
    let mut request = base_request(name);
    if let (Value::Object(target), Value::Object(members)) = (&mut request, patch) {
        for (member, value) in members {
            target.insert(member.clone(), value.clone());
        }
    }
    request
}

/// One row of the bounds table: a member set to a value at a published bound (`accepted`)
/// or one step outside it, and the provider name the row uses.
struct BoundCase {
    name: &'static str,
    provider: String,
    patch: Value,
    accepted: bool,
}

/// The rows. They span every bounded member of the create request — the vCPU ceiling,
/// a scaling policy's target value, the two VPC list lengths and the provider name's own
/// length — with the value AT each published bound accepted and the value one step
/// outside it refused.
fn bound_cases() -> Vec<BoundCase> {
    vec![
        BoundCase {
            name: "the vCPU ceiling at its published minimum",
            provider: "vcpu-min".to_string(),
            patch: json!({
                "CapacityProviderScalingConfig": {
                    "MaxVCpuCount": MAX_VCPU_MIN,
                    "ScalingMode": "Manual",
                },
            }),
            accepted: true,
        },
        BoundCase {
            name: "the vCPU ceiling one below its published minimum",
            provider: "vcpu-below".to_string(),
            patch: json!({
                "CapacityProviderScalingConfig": { "MaxVCpuCount": MAX_VCPU_MIN - 1 },
            }),
            accepted: false,
        },
        BoundCase {
            name: "the vCPU ceiling at its published maximum",
            provider: "vcpu-max".to_string(),
            patch: json!({
                "CapacityProviderScalingConfig": {
                    "MaxVCpuCount": MAX_VCPU_MAX,
                    "ScalingMode": "Auto",
                },
            }),
            accepted: true,
        },
        BoundCase {
            name: "the vCPU ceiling one above its published maximum",
            provider: "vcpu-above".to_string(),
            patch: json!({
                "CapacityProviderScalingConfig": { "MaxVCpuCount": MAX_VCPU_MAX + 1 },
            }),
            accepted: false,
        },
        BoundCase {
            name: "a target value at its published minimum",
            provider: "target-min".to_string(),
            patch: json!({
                "CapacityProviderScalingConfig": {
                    "ScalingPolicies": [{
                        "PredefinedMetricType": PREDEFINED_METRIC,
                        "TargetValue": TARGET_VALUE_MIN,
                    }],
                },
            }),
            accepted: true,
        },
        BoundCase {
            name: "a target value below its published minimum",
            provider: "target-below".to_string(),
            patch: json!({
                "CapacityProviderScalingConfig": {
                    "ScalingPolicies": [{
                        "PredefinedMetricType": PREDEFINED_METRIC,
                        "TargetValue": TARGET_VALUE_MIN - 0.5,
                    }],
                },
            }),
            accepted: false,
        },
        BoundCase {
            name: "a target value at its published maximum",
            provider: "target-max".to_string(),
            patch: json!({
                "CapacityProviderScalingConfig": {
                    "ScalingPolicies": [{
                        "PredefinedMetricType": PREDEFINED_METRIC,
                        "TargetValue": TARGET_VALUE_MAX,
                    }],
                },
            }),
            accepted: true,
        },
        BoundCase {
            name: "a target value above its published maximum",
            provider: "target-above".to_string(),
            patch: json!({
                "CapacityProviderScalingConfig": {
                    "ScalingPolicies": [{
                        "PredefinedMetricType": PREDEFINED_METRIC,
                        "TargetValue": TARGET_VALUE_MAX + 0.5,
                    }],
                },
            }),
            accepted: false,
        },
        BoundCase {
            name: "the subnet list at its published maximum",
            provider: "subnets-max".to_string(),
            patch: json!({
                "VpcConfig": {
                    "SubnetIds": subnets(SUBNETS_MAX),
                    "SecurityGroupIds": security_groups(SECURITY_GROUPS_MIN),
                },
            }),
            accepted: true,
        },
        BoundCase {
            name: "one subnet more than the published maximum",
            provider: "subnets-above".to_string(),
            patch: json!({
                "VpcConfig": {
                    "SubnetIds": subnets(SUBNETS_MAX + 1),
                    "SecurityGroupIds": security_groups(0),
                },
            }),
            accepted: false,
        },
        BoundCase {
            name: "one subnet fewer than the published minimum",
            provider: "subnets-below".to_string(),
            patch: json!({
                "VpcConfig": {
                    "SubnetIds": subnets(SUBNETS_MIN - 1),
                    "SecurityGroupIds": security_groups(0),
                },
            }),
            accepted: false,
        },
        BoundCase {
            name: "the security group list at its published maximum",
            provider: "groups-max".to_string(),
            patch: json!({
                "VpcConfig": {
                    "SubnetIds": subnets(1),
                    "SecurityGroupIds": security_groups(SECURITY_GROUPS_MAX),
                },
            }),
            accepted: true,
        },
        BoundCase {
            name: "one security group more than the published maximum",
            provider: "groups-above".to_string(),
            patch: json!({
                "VpcConfig": {
                    "SubnetIds": subnets(1),
                    "SecurityGroupIds": security_groups(SECURITY_GROUPS_MAX + 1),
                },
            }),
            accepted: false,
        },
        BoundCase {
            name: "a name one character past its published maximum length",
            provider: "n".repeat(NAME_MAX_LEN + 1),
            patch: json!({}),
            accepted: false,
        },
    ]
}

#[tokio::test]
async fn a_value_at_a_published_bound_is_stored_and_one_past_it_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for case in bound_cases() {
        let request = patched(&case.provider, &case.patch);
        let answer = create_provider(&router, &request).await;
        if case.accepted {
            assert_eq!(
                answer.status,
                202,
                "case {:?}: expected 202 from CreateCapacityProvider, got {} {}",
                case.name,
                answer.status,
                answer.text()
            );
            // The value at the bound is STORED: the read reports the members the
            // request carried, not a value clamped or defaulted on the way in. Only the
            // members the row actually set are compared, so no row passes on a pair of
            // absent members, and `VpcConfig` is always one of them (every request
            // carries it, required).
            let read = get_provider(&router, &case.provider).await;
            assert_eq!(
                read.status,
                200,
                "case {:?}: expected 200 from GetCapacityProvider, got {} {}",
                case.name,
                read.status,
                read.text()
            );
            let mut compared: Vec<&String> = case
                .patch
                .as_object()
                .map(|members| members.keys().collect())
                .unwrap_or_default();
            let vpc_config = "VpcConfig".to_string();
            if !compared.contains(&&vpc_config) {
                compared.push(&vpc_config);
            }
            for member in compared {
                assert_eq!(
                    read.provider()[member],
                    request[member],
                    "case {:?}: expected {member} as supplied ({}), got {}",
                    case.name,
                    request[member],
                    read.provider()[member]
                );
            }
        } else {
            assert_eq!(
                answer.status,
                400,
                "case {:?}: expected 400 for a value outside the modelled constraint, got {} {}",
                case.name,
                answer.status,
                answer.text()
            );
            // A refused request persists nothing, so the provider it named is not there.
            let read = get_provider(&router, &case.provider).await;
            assert!(
                read.status == 404 || read.status == 400,
                "case {:?}: a refused create must leave no provider to read, got {} {}",
                case.name,
                read.status,
                read.text()
            );
            let held = {
                let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
                let store = state.lock();
                store.capacity_providers.contains_key(&case.provider)
            };
            assert!(
                !held,
                "case {:?}: a refused create must persist no record, but {:?} is stored",
                case.name, case.provider
            );
        }
    }
}

#[tokio::test]
async fn a_read_reports_the_state_the_clock_owed_and_nothing_more() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let created = create_provider(&router, &base_request("clock-provider")).await;
    // The create's own answer is taken before any read, so it states the `Pending` the
    // model documents for a provider whose compute resources are still being stood up.
    assert_eq!(
        (created.status, created.provider_string("State")),
        (202, Some(STATE_PENDING.to_string())),
        "a created provider starts {STATE_PENDING}, got {} {}",
        created.status,
        created.text()
    );

    // Before the activation is due the provider is still `Pending`, however often it is
    // read: a read never advances the lifecycle on its own. The stored due instant is
    // what says whether a read happened before it, so a slow machine that crosses it
    // mid-suite does not turn a correct emulator into a failure.
    for read in 1..=3 {
        let answer = get_provider(&router, "clock-provider").await;
        let still_pending = {
            let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
            let store = state.lock();
            store
                .capacity_providers
                .get("clock-provider")
                .is_some_and(|provider| SystemTime::now() < provider.active_due)
        };
        if still_pending {
            assert_eq!(
                (answer.status, answer.provider_string("State")),
                (200, Some(STATE_PENDING.to_string())),
                "read {read}: expected {STATE_PENDING} before the activation was due, got {} {}",
                answer.status,
                answer.text()
            );
        }
    }

    // What waiting would do, without waiting: move the stored due instant into the past.
    {
        let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
        let mut store = state.lock();
        let past = SystemTime::now() - Duration::from_secs(1);
        for provider in store.capacity_providers.values_mut() {
            provider.active_due = past;
        }
    }

    let active = get_provider(&router, "clock-provider").await;
    assert_eq!(
        (active.status, active.provider_string("State")),
        (200, Some(STATE_ACTIVE.to_string())),
        "expected {STATE_ACTIVE} once the activation was due, got {} {}",
        active.status,
        active.text()
    );
    // Nothing else moved with the state, and a settled provider stays settled.
    let again = get_provider(&router, "clock-provider").await;
    assert_eq!(
        again.provider(),
        active.provider(),
        "two reads of a settled provider must report the same thing, got {} then {}",
        active.provider(),
        again.provider()
    );
}

#[tokio::test]
async fn either_documented_name_form_reads_the_same_provider() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    // One name per documented form of `CapacityProviderName`, including the characters
    // its pattern admits beside letters (`[a-zA-Z0-9-_]`).
    for provider in ["forms-provider", "FORMS_provider-2", "p", "9"] {
        let created = create_provider(&router, &base_request(provider)).await;
        assert_eq!(
            created.status,
            202,
            "case {provider:?}: expected 202 from CreateCapacityProvider, got {} {}",
            created.status,
            created.text()
        );
        let by_name = get_provider(&router, provider).await;
        let by_arn = get_provider(&router, &provider_arn(provider)).await;
        assert_eq!(
            (by_name.status, by_arn.status),
            (200, 200),
            "case {provider:?}: both documented name forms must resolve, got {} and {}",
            by_name.text(),
            by_arn.text()
        );
        assert_eq!(
            by_arn.provider(),
            by_name.provider(),
            "case {provider:?}: the ARN form must report the same record as the name form, \
             got {} and {}",
            by_name.provider(),
            by_arn.provider()
        );
        assert_eq!(
            by_name.provider_string("CapacityProviderArn"),
            Some(provider_arn(provider)),
            "case {provider:?}: expected the ARN {:?}, got {:?}",
            provider_arn(provider),
            by_name.provider_string("CapacityProviderArn")
        );
    }
}
