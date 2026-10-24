// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListCapacityProviders`: "Returns a list of capacity providers
//! in your account."
//!
//! The rule under test: `ListCapacityProviders` reports the capacity-provider records
//! this account holds — each one exactly as `GetCapacityProvider` reports it — in a
//! stable order (by provider name), one page of at most `MaxItems` (and never more than
//! the 50 the model's `CapacityProvidersList` admits) starting after the `Marker` the
//! previous page answered, carrying a `NextMarker` only while providers remain, filtered
//! to the one `CapacityProviderState` a request names; and it changes nothing a later
//! caller sees.
//!
//! Every row drives real HTTP requests through the router, so a provider the listing
//! invented rather than read fails here. The rows differ along what the rule generalises
//! over: the number of providers held (none, a handful, more than one page), the order
//! they were created in against the order they are reported in, the optional
//! configuration each one carries, the page size (each published bound of `MaxItems`, a
//! size below, one past the maximum, and no size at all), where the page starts (no
//! marker, a marker naming a provider, a marker naming none) and the state filtered for
//! (each of the model's four `CapacityProviderState` values, and no filter).

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    ComAmazonawsLambdaAwsgirapiserviceState, CAPACITY_PROVIDER_ACTIVATION_DELAY,
};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateCapacityProvider` and `ListCapacityProviders` are bound, from
/// `smithy_query(query="operation", operation="ListCapacityProviders")`
/// (`GET /2025-11-30/capacity-providers`).
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

/// A well-formed operator role ARN, satisfying the model's `RoleArn` pattern
/// (`^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`).
const OPERATOR_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-capacity";

/// A second well-formed operator role ARN, so a row can differ in the role it names.
const OTHER_OPERATOR_ROLE: &str = "arn:aws:iam::000000000000:role/capacity-operator";

/// A well-formed KMS key ARN, satisfying the model's `KMSKeyArnNonEmpty` pattern
/// (`^arn:(aws[a-zA-Z-]*)?:[a-z0-9-.]+:.*$`).
const KMS_KEY: &str =
    "arn:aws:kms:us-east-1:000000000000:key/0b1c2d3e-4f56-7890-abcd-ef1234567890";

/// The one `CapacityProviderPredefinedMetricType` the model declares.
const PREDEFINED_METRIC: &str = "LambdaCapacityProviderAverageCPUUtilization";

/// The model's `CapacityProviderState` values, from
/// `smithy_query(query="shape", shape="CapacityProviderState")`
/// (`Pending | Active | Failed | Deleting`) — the catalog the `State` filter accepts.
const STATE_PENDING: &str = "Pending";
/// See [`STATE_PENDING`].
const STATE_ACTIVE: &str = "Active";
/// See [`STATE_PENDING`].
const STATE_FAILED: &str = "Failed";
/// See [`STATE_PENDING`].
const STATE_DELETING: &str = "Deleting";

/// The most providers one page can report: `MaxItems` is `MaxFiftyListItems`,
/// `@range(min: 1, max: 50)`, and the answered `CapacityProvidersList` is
/// `@length(min: 0, max: 50)`.
const MAX_ITEMS_MAX: usize = 50;
/// The bottom of that range.
const MAX_ITEMS_MIN: usize = 1;

/// The bounds the model puts on `CapacityProviderVpcConfig$SubnetIds`
/// (`@length(min: 1, max: 16)`) and `$SecurityGroupIds` (`@length(min: 0, max: 5)`).
const SUBNETS_MAX: usize = 16;
const SECURITY_GROUPS_MAX: usize = 5;

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

    /// The `CapacityProviders` the listing answered. The member is `@required`, so a
    /// successful answer always carries the list, empty or not.
    fn providers(&self) -> Vec<Value> {
        match self.json().get("CapacityProviders") {
            Some(Value::Array(entries)) => entries.clone(),
            other => panic!(
                "expected a CapacityProviders list, got {other:?}: {}",
                self.text()
            ),
        }
    }

    /// The ARNs of the providers the listing answered, in the order it reported them.
    fn arns(&self) -> Vec<String> {
        self.providers()
            .iter()
            .map(|provider| {
                provider
                    .get("CapacityProviderArn")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!("every listed provider carries its ARN, got {provider:?}")
                    })
                    .to_string()
            })
            .collect()
    }

    /// The `NextMarker` the listing answered, when it answered one.
    fn next_marker(&self) -> Option<String> {
        self.json()
            .get("NextMarker")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The `CapacityProvider` member of a `GetCapacityProvider` answer.
    fn provider(&self) -> Value {
        self.json()
            .get("CapacityProvider")
            .cloned()
            .unwrap_or(Value::Null)
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

/// `GetCapacityProvider` on the provider `name` names.
async fn get_provider(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{CAPACITY_PROVIDERS_URI}/{name}"),
        &Value::Null,
    )
    .await
}

/// `ListCapacityProviders` with the query string `query` (`""` for no parameters at
/// all); every parameter of this operation is bound to the query string.
async fn list_providers(router: &MultiServiceRouter, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{CAPACITY_PROVIDERS_URI}{query}"),
        &Value::Null,
    )
    .await
}

/// The ARN a capacity provider of `name` carries, as the model's `CapacityProviderArn`
/// pattern spells it.
fn provider_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:capacity-provider:{name}")
}

/// A subnet id satisfying the model's `SubnetId` pattern (`^subnet-[0-9a-z]*$`).
fn subnets(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!(format!("subnet-{:08x}", 0x1000_0000usize + index)))
            .collect(),
    )
}

/// A security group id satisfying the model's `SecurityGroupId` pattern
/// (`^sg-[0-9a-zA-Z]*$`).
fn security_groups(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!(format!("sg-{:08X}", 0x2000_0000usize + index)))
            .collect(),
    )
}

/// A `CreateCapacityProvider` request for `name` with the members of `extras` merged in.
fn create_request(name: &str, extras: &Value) -> Value {
    let mut body = json!({ "CapacityProviderName": name });
    if let (Value::Object(target), Value::Object(members)) = (&mut body, extras) {
        for (member, value) in members {
            target.insert(member.clone(), value.clone());
        }
    }
    body
}

/// The smallest create request the model admits: both required members, nothing else.
fn minimal_extras(role: &str) -> Value {
    json!({
        "VpcConfig": { "SubnetIds": subnets(1), "SecurityGroupIds": security_groups(0) },
        "PermissionsConfig": { "CapacityProviderOperatorRoleArn": role },
    })
}

/// The providers of the reporting table, in the order they are CREATED — deliberately
/// not the order they must be reported in. They differ in the optional configuration
/// they carry and in the case and length of their names, so the listing is shown to
/// report whole records in name order rather than in creation order.
fn reporting_providers() -> Vec<(&'static str, Value)> {
    vec![
        ("zeta-provider", minimal_extras(OPERATOR_ROLE)),
        (
            "alpha_provider",
            json!({
                "VpcConfig": {
                    "SubnetIds": subnets(3),
                    "SecurityGroupIds": security_groups(2),
                },
                "PermissionsConfig": { "CapacityProviderOperatorRoleArn": OTHER_OPERATOR_ROLE },
                "InstanceRequirements": {
                    "Architectures": ["arm64"],
                    "AllowedInstanceTypes": ["c7g.large", "m7g.*"],
                },
                "CapacityProviderScalingConfig": {
                    "MaxVCpuCount": 128,
                    "ScalingMode": "Auto",
                    "ScalingPolicies": [{
                        "PredefinedMetricType": PREDEFINED_METRIC,
                        "TargetValue": 62.5,
                    }],
                },
                "KmsKeyArn": KMS_KEY,
                "Tags": { "team": "platform" },
                "PropagateTags": {
                    "Mode": "Explicit",
                    "ExplicitTags": { "cost-center": "4711" },
                },
                "TelemetryConfig": {
                    "LoggingConfig": { "SystemLogLevel": "DEBUG", "LogGroup": "/aws/lambda/capacity" },
                },
            }),
        ),
        (
            "m-provider",
            json!({
                "VpcConfig": {
                    "SubnetIds": subnets(SUBNETS_MAX),
                    "SecurityGroupIds": security_groups(SECURITY_GROUPS_MAX),
                },
                "PermissionsConfig": { "CapacityProviderOperatorRoleArn": OPERATOR_ROLE },
                "CapacityProviderScalingConfig": {
                    "MaxVCpuCount": 2,
                    "ScalingMode": "Manual",
                    "ScalingPolicies": [{
                        "PredefinedMetricType": PREDEFINED_METRIC,
                        "TargetValue": 0.0,
                    }],
                },
            }),
        ),
        ("p", json!({
            "VpcConfig": { "SubnetIds": subnets(1), "SecurityGroupIds": security_groups(0) },
            "PermissionsConfig": { "CapacityProviderOperatorRoleArn": OPERATOR_ROLE },
            "PropagateTags": { "Mode": "None" },
        })),
        ("Upper-Provider", minimal_extras(OTHER_OPERATOR_ROLE)),
    ]
}

/// Create every provider of `providers`, failing the test on any create that is refused.
async fn create_all(router: &MultiServiceRouter, providers: &[(String, Value)]) {
    for (name, extras) in providers {
        let answer = create_provider(router, &create_request(name, extras)).await;
        assert_eq!(
            answer.status,
            202,
            "case {name:?}: expected 202 from CreateCapacityProvider, got {} {}",
            answer.status,
            answer.text()
        );
    }
}

/// Every provider the store holds, as (name, ARN, state, last modified) — the snapshot a
/// read must leave untouched.
fn stored_snapshot(
    engine: &Arc<engine::InMemoryEngine>,
) -> Vec<(String, String, String, Option<String>)> {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    store
        .capacity_providers
        .values()
        .map(|record| {
            (
                record.name.clone(),
                record.arn.clone(),
                record.state.clone(),
                Some(record.last_modified.clone()),
            )
        })
        .collect()
}

/// Wait past the instant a create made a provider's activation due, so every stored
/// provider has settled into one state. Nothing here sleeps on the emulator's behalf:
/// the transition is owed by the clock whether or not anybody calls.
async fn settle_activations() {
    tokio::time::sleep(CAPACITY_PROVIDER_ACTIVATION_DELAY + Duration::from_millis(100)).await;
}

/// The listing reports every stored provider, whole, in name order — the same record
/// `GetCapacityProvider` reports — and reports nothing when the account holds nothing.
#[tokio::test]
async fn list_capacity_providers_reports_every_stored_provider_as_get_does() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // An account holding no provider: the `@required` list is answered, empty, with no
    // marker to continue from.
    let empty = list_providers(&router, "").await;
    assert_eq!(
        (empty.status, empty.arns(), empty.next_marker()),
        (200, Vec::new(), None),
        "case empty account: expected an empty page and no marker, got {} {}",
        empty.status,
        empty.text()
    );

    let providers: Vec<(String, Value)> = reporting_providers()
        .into_iter()
        .map(|(name, extras)| (name.to_string(), extras))
        .collect();
    create_all(&router, &providers).await;
    settle_activations().await;

    // The order the rule names: by provider name, which is not the creation order above.
    let mut expected_names: Vec<String> =
        providers.iter().map(|(name, _)| name.clone()).collect();
    expected_names.sort();
    let expected_arns: Vec<String> = expected_names.iter().map(|n| provider_arn(n)).collect();

    let listed = list_providers(&router, "").await;
    assert_eq!(
        (listed.status, listed.arns(), listed.next_marker()),
        (200, expected_arns.clone(), None),
        "case every provider: expected {expected_arns:?} and no marker, got {} {}",
        listed.status,
        listed.text()
    );

    // Each listed entry is the record `GetCapacityProvider` answers for that provider —
    // every member, not a summary of its own making.
    for (index, name) in expected_names.iter().enumerate() {
        let fetched = get_provider(&router, name).await;
        assert_eq!(
            fetched.status, 200,
            "case {name:?}: expected 200 from GetCapacityProvider, got {} {}",
            fetched.status,
            fetched.text()
        );
        assert_eq!(
            listed.providers()[index],
            fetched.provider(),
            "case {name:?}: the listed entry must be the record Get reports, got {:?}",
            listed.providers()[index]
        );
    }

    // A read changes nothing a later caller sees: the same page comes back, and the store
    // holds exactly what it held before the listing. The snapshot is taken once the
    // activations the clock owed have been applied, since THAT change was due whether or
    // not anybody called.
    let before = stored_snapshot(&engine);
    let again = list_providers(&router, "").await;
    assert_eq!(
        (again.arns(), again.next_marker()),
        (expected_arns.clone(), None),
        "case repeated listing: the page must be stable, got {}",
        again.text()
    );
    assert_eq!(
        stored_snapshot(&engine),
        before,
        "case repeated listing: a listing must not change the stored providers"
    );
}

/// One paging row: the page size asked for (`None` asks for none at all) and the most
/// entries one page may then carry.
struct PagingCase {
    name: &'static str,
    max_items: Option<usize>,
    page_limit: usize,
}

/// The listing pages over every provider: each page carries at most the size asked for
/// (never more than the modelled maximum), the pages concatenate to the whole ordered
/// sequence with nothing repeated or skipped, and a marker is answered exactly while
/// providers remain.
#[tokio::test]
async fn list_capacity_providers_pages_over_every_provider() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // One provider more than the largest page the model admits, so a page size at and
    // above the maximum is shown to be capped rather than exceeded.
    let total = MAX_ITEMS_MAX + 1;
    let providers: Vec<(String, Value)> = (0..total)
        .map(|index| (format!("cp-{index:02}"), minimal_extras(OPERATOR_ROLE)))
        .collect();
    create_all(&router, &providers).await;

    let mut expected: Vec<String> = providers.iter().map(|(name, _)| name.clone()).collect();
    expected.sort();
    let expected_arns: Vec<String> = expected.iter().map(|n| provider_arn(n)).collect();

    let cases = [
        PagingCase {
            name: "one at a time, the smallest page the model admits",
            max_items: Some(MAX_ITEMS_MIN),
            page_limit: MAX_ITEMS_MIN,
        },
        PagingCase {
            name: "two at a time",
            max_items: Some(2),
            page_limit: 2,
        },
        PagingCase {
            name: "seven at a time, a size that does not divide the total",
            max_items: Some(7),
            page_limit: 7,
        },
        PagingCase {
            name: "at the largest page the model admits",
            max_items: Some(MAX_ITEMS_MAX),
            page_limit: MAX_ITEMS_MAX,
        },
        PagingCase {
            name: "no page size at all",
            max_items: None,
            page_limit: MAX_ITEMS_MAX,
        },
    ];

    for case in cases {
        let label = case.name;
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match (case.max_items, marker.as_deref()) {
                (Some(size), Some(from)) => format!("?MaxItems={size}&Marker={from}"),
                (Some(size), None) => format!("?MaxItems={size}"),
                (None, Some(from)) => format!("?Marker={from}"),
                (None, None) => String::new(),
            };
            let answer = list_providers(&router, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {label:?}: expected 200 for {query:?}, got {} {}",
                answer.status,
                answer.text()
            );
            let page = answer.arns();
            assert!(
                page.len() <= case.page_limit,
                "case {label:?}: a page carries at most {} entries, got {}",
                case.page_limit,
                page.len()
            );
            seen.extend(page.iter().cloned());
            pages += 1;
            assert!(
                pages <= total + 1,
                "case {label:?}: paging must terminate, got {pages} pages for {total} providers"
            );
            match answer.next_marker() {
                Some(next) => {
                    assert!(
                        seen.len() < expected_arns.len(),
                        "case {label:?}: a marker ({next:?}) is answered only while \
                         providers remain, having seen {}",
                        seen.len()
                    );
                    marker = Some(next);
                }
                None => break,
            }
        }
        assert_eq!(
            seen, expected_arns,
            "case {label:?}: the pages must concatenate to every provider in name order, \
             got {seen:?}"
        );
        assert_eq!(
            seen.iter().collect::<BTreeSet<_>>().len(),
            seen.len(),
            "case {label:?}: no provider may be reported twice, got {seen:?}"
        );
    }

    // A marker continues AFTER the provider it names, whether or not that name is one of
    // the providers held: it is the ordering position that decides the page.
    let marker_cases: [(&str, &str, usize); 4] = [
        ("a marker naming the first provider", "cp-00", 1),
        ("a marker naming a provider mid-sequence", "cp-24", 25),
        ("a marker naming no provider at all", "cp-24-gone", 25),
        ("a marker ordered past every provider", "zzz", total),
    ];
    for (label, marker, skipped) in marker_cases {
        let answer = list_providers(&router, &format!("?MaxItems=50&Marker={marker}")).await;
        let expected_tail: Vec<String> = expected_arns
            .iter()
            .skip(skipped)
            .take(MAX_ITEMS_MAX)
            .cloned()
            .collect();
        assert_eq!(
            (answer.status, answer.arns()),
            (200, expected_tail.clone()),
            "case {label:?}: expected the {} providers after {marker:?}, got {}",
            expected_tail.len(),
            answer.text()
        );
    }
}

/// The `State` filter reports only the providers in the state it names: "Filter capacity
/// providers by their current state."
#[tokio::test]
async fn list_capacity_providers_filters_by_state() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let providers: Vec<(String, Value)> = (0..5)
        .map(|index| (format!("state-{index}"), minimal_extras(OPERATOR_ROLE)))
        .collect();
    create_all(&router, &providers).await;
    let mut expected: Vec<String> = providers
        .iter()
        .map(|(name, _)| provider_arn(name))
        .collect();
    expected.sort();

    // No provider this store holds is `Failed` or `Deleting`, so a request filtered to
    // either reports none of them — not the whole account.
    for state in [STATE_FAILED, STATE_DELETING] {
        let answer = list_providers(&router, &format!("?State={state}")).await;
        assert_eq!(
            (answer.status, answer.arns(), answer.next_marker()),
            (200, Vec::new(), None),
            "case State={state}: expected no provider, got {}",
            answer.text()
        );
    }

    // A freshly created provider is `Pending` until the clock makes its activation due,
    // so the two live states PARTITION the account: every provider is reported under
    // exactly one of them, and every entry reported under a state carries it.
    let pending = list_providers(&router, &format!("?State={STATE_PENDING}")).await;
    let active = list_providers(&router, &format!("?State={STATE_ACTIVE}")).await;
    for (state, answer) in [(STATE_PENDING, &pending), (STATE_ACTIVE, &active)] {
        for provider in answer.providers() {
            assert_eq!(
                provider.get("State").and_then(Value::as_str),
                Some(state),
                "case State={state}: every entry must carry the state filtered for, got \
                 {provider:?}"
            );
        }
    }
    let mut partitioned: Vec<String> = pending.arns();
    partitioned.extend(active.arns());
    partitioned.sort();
    assert_eq!(
        partitioned, expected,
        "case the live states: {STATE_PENDING} and {STATE_ACTIVE} must partition the \
         account, got {partitioned:?}"
    );

    // Once every activation is due, the same filters report the other way round, and a
    // filtered listing pages exactly as an unfiltered one does.
    settle_activations().await;
    let settled = list_providers(&router, &format!("?State={STATE_ACTIVE}")).await;
    assert_eq!(
        (settled.status, settled.arns()),
        (200, expected.clone()),
        "case State={STATE_ACTIVE} once due: expected every provider, got {}",
        settled.text()
    );
    let none_pending = list_providers(&router, &format!("?State={STATE_PENDING}")).await;
    assert_eq!(
        (none_pending.status, none_pending.arns()),
        (200, Vec::new()),
        "case State={STATE_PENDING} once due: expected no provider, got {}",
        none_pending.text()
    );

    let mut seen: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let query = match marker.as_deref() {
            Some(from) => format!("?State={STATE_ACTIVE}&MaxItems=2&Marker={from}"),
            None => format!("?State={STATE_ACTIVE}&MaxItems=2"),
        };
        let answer = list_providers(&router, &query).await;
        assert!(
            answer.arns().len() <= 2,
            "case filtered paging: a page carries at most 2 entries, got {:?}",
            answer.arns()
        );
        seen.extend(answer.arns());
        match answer.next_marker() {
            Some(next) => marker = Some(next),
            None => break,
        }
    }
    assert_eq!(
        seen, expected,
        "case filtered paging: the pages must concatenate to every {STATE_ACTIVE} \
         provider, got {seen:?}"
    );
}

/// Concurrent listings each answer the whole account: a read takes no capacity away from
/// another caller, so eight callers listing at once all get the same page back, and each
/// one answers rather than waiting on the others.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_listings_each_report_every_stored_provider() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = Arc::new(router_on(&engine));

    let providers: Vec<(String, Value)> = (0..4)
        .map(|index| (format!("busy-{index}"), minimal_extras(OPERATOR_ROLE)))
        .collect();
    create_all(&router, &providers).await;
    settle_activations().await;

    let mut expected: Vec<String> = providers
        .iter()
        .map(|(name, _)| provider_arn(name))
        .collect();
    expected.sort();

    let callers = 8;
    let mut tasks = Vec::with_capacity(callers);
    for caller in 0..callers {
        let router = Arc::clone(&router);
        tasks.push(tokio::spawn(async move {
            // Each caller asks a different way, so the concurrent answers are not one
            // cached page: a whole page, a filtered page and a two-entry page.
            let query = match caller % 3 {
                0 => String::new(),
                1 => format!("?State={STATE_ACTIVE}"),
                _ => "?MaxItems=2".to_string(),
            };
            let answer = list_providers(&router, &query).await;
            (caller, query, answer.status, answer.arns())
        }));
    }

    for task in tasks {
        let (caller, query, status, arns) = task.await.expect("each concurrent caller answers");
        let expected_arns: Vec<String> = if query == "?MaxItems=2" {
            expected.iter().take(2).cloned().collect()
        } else {
            expected.clone()
        };
        assert_eq!(
            (status, arns.clone()),
            (200, expected_arns.clone()),
            "case caller {caller} ({query:?}): expected {expected_arns:?}, got {arns:?}"
        );
    }
}

/// A page size outside the range the model declares is refused, and each published bound
/// is served.
#[tokio::test]
async fn list_capacity_providers_refuses_a_page_size_outside_the_modelled_range() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let providers: Vec<(String, Value)> = (0..3)
        .map(|index| (format!("bounded-{index}"), minimal_extras(OPERATOR_ROLE)))
        .collect();
    create_all(&router, &providers).await;

    // `MaxItems` is `@range(min: 1, max: 50)`: below the minimum and past the maximum are
    // refused rather than clamped into a page.
    for (label, max_items) in [
        ("zero", "0"),
        ("negative", "-1"),
        ("one past the maximum", "51"),
        ("far past the maximum", "10000"),
    ] {
        let answer = list_providers(&router, &format!("?MaxItems={max_items}")).await;
        assert_eq!(
            answer.status,
            400,
            "case MaxItems {label}: expected 400, got {} {}",
            answer.status,
            answer.text()
        );
    }

    // Each published bound is served: the minimum answers one provider and a marker, the
    // maximum answers all three and none.
    let smallest = list_providers(&router, &format!("?MaxItems={MAX_ITEMS_MIN}")).await;
    assert_eq!(
        (
            smallest.status,
            smallest.arns().len(),
            smallest.next_marker()
        ),
        (200, 1, Some("bounded-0".to_string())),
        "case MaxItems at the minimum: expected one provider and a marker, got {}",
        smallest.text()
    );
    let largest = list_providers(&router, &format!("?MaxItems={MAX_ITEMS_MAX}")).await;
    assert_eq!(
        (largest.status, largest.arns().len(), largest.next_marker()),
        (200, providers.len(), None),
        "case MaxItems at the maximum: expected every provider and no marker, got {}",
        largest.text()
    );
}
