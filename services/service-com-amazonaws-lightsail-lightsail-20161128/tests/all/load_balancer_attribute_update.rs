// SPDX-License-Identifier: CC0-1.0

//! `UpdateLoadBalancerAttribute` over the wire: one configuration option at a
//! time, read back through `GetLoadBalancer`.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `UpdateLoadBalancerAttribute` resolves the request's
//! `attributeValue` string as a value of the named attribute's OWN documented
//! domain — a non-blank ping path for `HealthCheckPath`, `true`/`false` for the
//! two toggles, an integer number of seconds inside the published range for the
//! stickiness cookie duration, a name `GetLoadBalancerTlsPolicies` reports for
//! `TlsPolicyName` — writes exactly that one option onto the load balancer the
//! request names in the Region it is made in, leaving every other option as it
//! was, and refuses a value outside that domain without changing anything.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire
/// protocol").
const TARGET_PREFIX: &str = "Lightsail_20161128";

/// The Region most cases are made in; the Region-scoping case uses a second one.
const REGION: &str = "us-east-1";

/// A second Region, for the case that proves the record a request addresses is
/// the one in the Region the request is made in.
const OTHER_REGION: &str = "eu-west-1";

/// The Regions the accepted-value table spreads its rows across, so the rule is
/// exercised in more than one Region and no single Region is asked for more load
/// balancers than the documented per-Region cap of 10 admits. Every entry is one
/// `GetRegions` reports.
const REGIONS: &[&str] = &[
    "us-east-1",
    "us-west-2",
    "eu-west-1",
    "ap-southeast-2",
    "us-east-2",
];

/// The modelled `LoadBalancerAttributeName` members
/// (`smithy_query(query="shape",shape="LoadBalancerAttributeName")`), which are
/// also the keys of `LoadBalancer$configurationOptions`.
const HEALTH_CHECK_PATH: &str = "HealthCheckPath";
/// See [`HEALTH_CHECK_PATH`].
const SESSION_STICKINESS_ENABLED: &str = "SessionStickinessEnabled";
/// See [`HEALTH_CHECK_PATH`].
const COOKIE_DURATION: &str = "SessionStickiness_LB_CookieDurationSeconds";
/// See [`HEALTH_CHECK_PATH`].
const HTTPS_REDIRECTION_ENABLED: &str = "HttpsRedirectionEnabled";
/// See [`HEALTH_CHECK_PATH`].
const TLS_POLICY_NAME: &str = "TlsPolicyName";

/// The published range of the session-stickiness cookie duration, in seconds:
/// the Elastic Load Balancing attribute Lightsail surfaces as
/// `SessionStickiness_LB_CookieDurationSeconds` is documented with "The range is
/// 1 second to 1 week (604800 seconds)".
const MIN_COOKIE_DURATION: i64 = 1;
/// See [`MIN_COOKIE_DURATION`].
const MAX_COOKIE_DURATION: i64 = 604_800;

/// One emulator with one Lightsail service mounted on one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing; routing is by `X-Amz-Target`, and the
        // `Authorization` header below carries only the credential SCOPE, which
        // is what names the Region a request is made in.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_lightsail_lightsail_20161128::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `operation` in `region` with `body`, answering its HTTP status and
    /// parsed body.
    async fn call(&self, region: &str, operation: &str, body: Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "127.0.0.1:9297")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261022/{region}/lightsail/\
                     aws4_request, SignedHeaders=host, Signature=not-verified"
                ),
            )
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call `operation` in `region` and require HTTP 200, answering the body.
    async fn ok(&self, region: &str, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.call(region, operation, body).await;
        assert!(
            status == 200,
            "case {operation} in {region}: expected HTTP 200, got {status} {parsed:?}"
        );
        parsed
    }

    /// Create the load balancer `name` in `region` with nothing optional asked
    /// for, so every configuration option starts at its documented default.
    async fn create(&self, region: &str, name: &str) {
        self.ok(
            region,
            "CreateLoadBalancer",
            json!({"loadBalancerName": name, "instancePort": 80}),
        )
        .await;
    }

    /// The stored load balancer `name` in `region`, as `GetLoadBalancer` reports
    /// it.
    async fn read(&self, region: &str, name: &str) -> Value {
        self.ok(region, "GetLoadBalancer", json!({"loadBalancerName": name}))
            .await["loadBalancer"]
            .clone()
    }

    /// The `configurationOptions` of the load balancer `name` in `region`, as a
    /// comparable map.
    async fn options(&self, region: &str, name: &str) -> BTreeMap<String, String> {
        let balancer = self.read(region, name).await;
        balancer["configurationOptions"]
            .as_object()
            .map(|map| {
                map.iter()
                    .map(|(key, value)| {
                        (
                            key.clone(),
                            value.as_str().unwrap_or("<not a string>").to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Set `attribute` to `value` on the load balancer `name` in `region`.
    async fn update(&self, region: &str, name: &str, attribute: &str, value: &str) -> Value {
        self.ok(
            region,
            "UpdateLoadBalancerAttribute",
            json!({
                "loadBalancerName": name,
                "attributeName": attribute,
                "attributeValue": value,
            }),
        )
        .await
    }
}

/// The wire error code of a failure body: awsJson carries it in `__type`, either
/// bare or namespaced.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The TLS security policy names `GetLoadBalancerTlsPolicies` reports, walked to
/// the end of its paging — the catalog's own source, so no policy name is
/// hand-written here.
async fn reported_policy_names(emulator: &Emulator) -> Vec<String> {
    let mut names = Vec::new();
    let mut page_token: Option<String> = None;
    loop {
        let body = match &page_token {
            None => json!({}),
            Some(token) => json!({"pageToken": token}),
        };
        let page = emulator
            .ok(REGION, "GetLoadBalancerTlsPolicies", body)
            .await;
        for policy in page["tlsPolicies"].as_array().unwrap_or(&Vec::new()) {
            names.push(
                policy["name"]
                    .as_str()
                    .expect("a reported policy carries a name")
                    .to_string(),
            );
        }
        match page["nextPageToken"].as_str() {
            Some(token) => page_token = Some(token.to_string()),
            None => break,
        }
    }
    assert!(
        !names.is_empty(),
        "case reading the TLS policy catalog: expected at least one policy, got none"
    );
    names
}

// ---------------------------------------------------------------------------
// The accepted-value table
// ---------------------------------------------------------------------------

/// One `UpdateLoadBalancerAttribute` call, and what the load balancer must then
/// report for that attribute.
///
/// The rows differ along everything the rule generalises over: which attribute
/// is named (every member of the modelled enum), the FORM of the value (a path,
/// a boolean literal, a decimal integer, a catalog name), the value itself
/// (including both published bounds of the cookie duration and both boolean
/// values), and whether the attribute has a dedicated response member beside its
/// `configurationOptions` entry.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    attribute: &'static str,
    /// `attributeValue` as sent.
    sent: &'static str,
    /// The `configurationOptions` entry the load balancer must then report.
    expected_option: &'static str,
    /// The dedicated top-level member that must agree with it, and the JSON
    /// value it must carry — `None` for an attribute the `LoadBalancer` shape
    /// exposes only among its configuration options.
    expected_member: Option<(&'static str, &'static str)>,
}

/// The table. Every expected value is the one the named attribute's own
/// `attributeValue` documentation states.
const ROWS: &[Row] = &[
    Row {
        case: "a one-segment health check path",
        attribute: HEALTH_CHECK_PATH,
        sent: "/health",
        expected_option: "/health",
        expected_member: Some(("healthCheckPath", "/health")),
    },
    Row {
        case: "a deep health check path, as the documentation's own example is",
        attribute: HEALTH_CHECK_PATH,
        sent: "/weather/us/wa/seattle",
        expected_option: "/weather/us/wa/seattle",
        expected_member: Some(("healthCheckPath", "/weather/us/wa/seattle")),
    },
    Row {
        case: "the root path, back to the documented default",
        attribute: HEALTH_CHECK_PATH,
        sent: "/",
        expected_option: "/",
        expected_member: Some(("healthCheckPath", "/")),
    },
    Row {
        case: "a health check path padded with whitespace, stored as the path itself",
        attribute: HEALTH_CHECK_PATH,
        sent: "  /padded  ",
        expected_option: "/padded",
        expected_member: Some(("healthCheckPath", "/padded")),
    },
    Row {
        case: "session stickiness activated",
        attribute: SESSION_STICKINESS_ENABLED,
        sent: "true",
        expected_option: "true",
        expected_member: None,
    },
    Row {
        case: "session stickiness deactivated, which is also what it already was",
        attribute: SESSION_STICKINESS_ENABLED,
        sent: "false",
        expected_option: "false",
        expected_member: None,
    },
    Row {
        case: "the cookie duration at its published floor",
        attribute: COOKIE_DURATION,
        sent: "1",
        expected_option: "1",
        expected_member: None,
    },
    Row {
        case: "the cookie duration at an hour",
        attribute: COOKIE_DURATION,
        sent: "3600",
        expected_option: "3600",
        expected_member: None,
    },
    Row {
        case: "the cookie duration at its published ceiling of one week",
        attribute: COOKIE_DURATION,
        sent: "604800",
        expected_option: "604800",
        expected_member: None,
    },
    Row {
        case: "HTTP to HTTPS redirection activated",
        attribute: HTTPS_REDIRECTION_ENABLED,
        sent: "true",
        expected_option: "true",
        expected_member: Some(("httpsRedirectionEnabled", "true")),
    },
    Row {
        case: "HTTP to HTTPS redirection deactivated",
        attribute: HTTPS_REDIRECTION_ENABLED,
        sent: "false",
        expected_option: "false",
        expected_member: Some(("httpsRedirectionEnabled", "false")),
    },
];

/// The whole rule for an accepted value: the named attribute takes it, every
/// other configuration option keeps the value it had, and the change survives a
/// read through `GetLoadBalancer`.
#[tokio::test]
async fn setting_one_attribute_writes_that_one_and_leaves_the_rest() {
    let emulator = Emulator::new();

    for (index, row) in ROWS.iter().enumerate() {
        let name = format!("attr-lb-{index}");
        // Each row runs in its own Region, in turn: the attribute is written on
        // the record of the Region the request is made in, whichever that is.
        let region = REGIONS[index % REGIONS.len()];
        emulator.create(region, &name).await;
        let before = emulator.options(region, &name).await;

        emulator.update(region, &name, row.attribute, row.sent).await;

        let after = emulator.options(region, &name).await;
        assert_eq!(
            after.get(row.attribute).map(String::as_str),
            Some(row.expected_option),
            "case {:?} in {region}: expected {} to report {:?}, got {after:?}",
            row.case,
            row.attribute,
            row.expected_option
        );
        // "You can only update one attribute at a time": nothing else moved.
        for (key, value) in &before {
            if key == row.attribute {
                continue;
            }
            assert_eq!(
                after.get(key),
                Some(value),
                "case {:?} in {region}: expected {key} to stay {value:?}, got {after:?}",
                row.case
            );
        }
        // The option is also reported by the dedicated member of the modelled
        // `LoadBalancer` shape, where there is one: one record, read two ways.
        if let Some((member, expected)) = row.expected_member {
            let balancer = emulator.read(region, &name).await;
            let actual = match &balancer[member] {
                Value::Bool(flag) => flag.to_string(),
                Value::String(text) => text.clone(),
                other => format!("{other:?}"),
            };
            assert_eq!(
                actual, expected,
                "case {:?} in {region}: expected {member} to report {expected:?}, \
                 got {actual:?}",
                row.case
            );
        }
    }
}

/// Every attribute is independent: setting all five on one load balancer, one
/// call at a time, leaves every one of them at the value its own call asked for.
#[tokio::test]
async fn the_attributes_accumulate_rather_than_replace_one_another() {
    let emulator = Emulator::new();
    let name = "every-attribute-lb";
    emulator.create(REGION, name).await;

    let policy = reported_policy_names(&emulator)
        .await
        .last()
        .expect("the catalog reports at least one policy")
        .clone();
    let wanted: Vec<(&str, String)> = vec![
        (HEALTH_CHECK_PATH, "/status".to_string()),
        (SESSION_STICKINESS_ENABLED, "true".to_string()),
        (COOKIE_DURATION, "7200".to_string()),
        (HTTPS_REDIRECTION_ENABLED, "true".to_string()),
        (TLS_POLICY_NAME, policy),
    ];

    for (attribute, value) in &wanted {
        emulator.update(REGION, name, attribute, value).await;
    }

    let options = emulator.options(REGION, name).await;
    for (attribute, value) in &wanted {
        assert_eq!(
            options.get(*attribute),
            Some(value),
            "case all five attributes set: expected {attribute} to report {value:?}, \
             got {options:?}"
        );
    }
}

/// `TlsPolicyName` accepts exactly what `GetLoadBalancerTlsPolicies` reports —
/// "Use the GetLoadBalancerTlsPolicies action to get a list of TLS policy names
/// that you can specify" — one row per entry of the published catalog, so the
/// rule is exercised on every policy and not only on the default one.
#[tokio::test]
async fn every_reported_tls_policy_name_can_be_set_as_the_attribute() {
    let emulator = Emulator::new();
    let names = reported_policy_names(&emulator).await;

    for (index, policy) in names.iter().enumerate() {
        let name = format!("tls-attr-lb-{index}");
        emulator.create(REGION, &name).await;
        emulator
            .update(REGION, &name, TLS_POLICY_NAME, policy)
            .await;

        let balancer = emulator.read(REGION, &name).await;
        assert_eq!(
            balancer["tlsPolicyName"].as_str(),
            Some(policy.as_str()),
            "case {policy}: expected the load balancer to report the policy it was given, \
             got {balancer:?}"
        );
        assert_eq!(
            balancer["configurationOptions"][TLS_POLICY_NAME].as_str(),
            Some(policy.as_str()),
            "case {policy}: expected the policy among the configuration options, \
             got {balancer:?}"
        );
    }
}

/// The published catalog governs `CreateLoadBalancer$tlsPolicyName` too — it
/// carries the same documented sentence — so a name the catalog does not report
/// is refused there as well, and no load balancer is created, while a name it
/// does report is accepted.
#[tokio::test]
async fn creating_a_load_balancer_with_an_unpublished_tls_policy_is_refused() {
    let emulator = Emulator::new();

    // The rows differ along the way the name misses the catalog: a plausible
    // name of a policy that does not exist, a published name in the wrong case,
    // and a truncated published name.
    let rows = [
        ("a policy that does not exist", "TLS-FS-1-2-Res-2099-01"),
        ("a published name in the wrong case", "tls-2016-08"),
        ("a truncated published name", "TLS-2016"),
    ];
    for (index, (case, policy)) in rows.iter().enumerate() {
        let name = format!("unpublished-policy-lb-{index}");
        let (status, body) = emulator
            .call(
                REGION,
                "CreateLoadBalancer",
                json!({
                    "loadBalancerName": name,
                    "instancePort": 80,
                    "tlsPolicyName": policy,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case:?}: expected an InvalidInputException, got {body:?}"
        );
        // The refused create stored nothing: the name still names no load
        // balancer.
        let (read_status, read_body) = emulator
            .call(REGION, "GetLoadBalancer", json!({"loadBalancerName": name}))
            .await;
        assert_eq!(
            read_status, 400,
            "case {case:?}: expected the refused create to store nothing, \
             got {read_status} {read_body:?}"
        );
        assert_eq!(
            error_code(&read_body),
            "NotFoundException",
            "case {case:?}: expected the name to name nothing, got {read_body:?}"
        );
    }

    // A name the catalog DOES report is accepted, so the refusals above are the
    // catalog boundary and not a blanket rejection.
    let published = reported_policy_names(&emulator)
        .await
        .first()
        .expect("the catalog reports at least one policy")
        .clone();
    emulator
        .ok(
            REGION,
            "CreateLoadBalancer",
            json!({
                "loadBalancerName": "published-policy-lb",
                "instancePort": 80,
                "tlsPolicyName": published,
            }),
        )
        .await;
    let balancer = emulator.read(REGION, "published-policy-lb").await;
    assert_eq!(
        balancer["tlsPolicyName"].as_str(),
        Some(published.as_str()),
        "case {published}: expected the created load balancer to report the policy, \
         got {balancer:?}"
    );
}

// ---------------------------------------------------------------------------
// The refused-value table
// ---------------------------------------------------------------------------

/// One `UpdateLoadBalancerAttribute` call whose `attributeValue` names no value
/// of the attribute's domain, and so is refused.
///
/// The rows differ along the way the value misses the domain: blank where a path
/// is required, a word where a boolean is required, a non-number and each value
/// one step past each published bound where an integer in range is required, and
/// a name the published catalog does not report.
#[derive(Debug, Clone, Copy)]
struct BadRow {
    case: &'static str,
    attribute: &'static str,
    sent: &'static str,
}

const BAD_ROWS: &[BadRow] = &[
    BadRow {
        case: "a health check path of nothing but whitespace",
        attribute: HEALTH_CHECK_PATH,
        sent: "   ",
    },
    BadRow {
        case: "a toggle set to a word that is not a boolean",
        attribute: SESSION_STICKINESS_ENABLED,
        sent: "yes",
    },
    BadRow {
        case: "a toggle set to a number",
        attribute: HTTPS_REDIRECTION_ENABLED,
        sent: "1",
    },
    BadRow {
        case: "a cookie duration that is not a number",
        attribute: COOKIE_DURATION,
        sent: "an-hour",
    },
    BadRow {
        case: "a cookie duration one second below the published floor",
        attribute: COOKIE_DURATION,
        sent: "0",
    },
    BadRow {
        case: "a cookie duration one second above the published ceiling",
        attribute: COOKIE_DURATION,
        sent: "604801",
    },
    BadRow {
        case: "a TLS policy name the published catalog does not report",
        attribute: TLS_POLICY_NAME,
        sent: "TLS-FS-1-2-Res-2099-01",
    },
];

/// A value outside the attribute's domain is refused as `InvalidInputException`
/// — HTTP 400 — and the load balancer still reports exactly what it did before
/// the refused call.
#[tokio::test]
async fn a_value_outside_the_attributes_domain_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let name = "refused-attr-lb";
    emulator.create(REGION, name).await;
    let before = emulator.options(REGION, name).await;

    for row in BAD_ROWS {
        let (status, body) = emulator
            .call(
                REGION,
                "UpdateLoadBalancerAttribute",
                json!({
                    "loadBalancerName": name,
                    "attributeName": row.attribute,
                    "attributeValue": row.sent,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status} {body:?}",
            row.case
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {:?}: expected an InvalidInputException, got {body:?}",
            row.case
        );
        let after = emulator.options(REGION, name).await;
        assert_eq!(
            after, before,
            "case {:?}: expected the refused call to change nothing, got {after:?}",
            row.case
        );
    }

    // Both published bounds of the range those rows step past are themselves
    // accepted, so the refusals above are the bound and not a narrower guard.
    for seconds in [MIN_COOKIE_DURATION, MAX_COOKIE_DURATION] {
        emulator
            .update(REGION, name, COOKIE_DURATION, &seconds.to_string())
            .await;
        let after = emulator.options(REGION, name).await;
        assert_eq!(
            after.get(COOKIE_DURATION).map(String::as_str),
            Some(seconds.to_string().as_str()),
            "case the bound {seconds}: expected it to be accepted, got {after:?}"
        );
    }
}

/// The attribute is set on the load balancer the request names IN THE REGION the
/// request is made in: a load balancer of another Region is not one this call
/// reaches, and neither is a name that names nothing.
#[tokio::test]
async fn a_load_balancer_the_region_does_not_hold_is_answered_does_not_exist() {
    let emulator = Emulator::new();
    let name = "regional-attr-lb";
    emulator.create(REGION, name).await;
    let before = emulator.options(REGION, name).await;

    for (case, region, requested) in [
        ("a name that names nothing", REGION, "no-such-lb"),
        ("the right name in another Region", OTHER_REGION, name),
    ] {
        let (status, body) = emulator
            .call(
                region,
                "UpdateLoadBalancerAttribute",
                json!({
                    "loadBalancerName": requested,
                    "attributeName": HEALTH_CHECK_PATH,
                    "attributeValue": "/health",
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected a NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case {case:?}: expected the DoesNotExist envelope, got {body:?}"
        );
    }

    let after = emulator.options(REGION, name).await;
    assert_eq!(
        after, before,
        "case the refused calls: expected the load balancer to be untouched, got {after:?}"
    );
}

/// The call writes the service's operation log: one entry for the load balancer
/// it reconfigured, which `GetOperation` and `GetOperationsForResource` then
/// report.
#[tokio::test]
async fn the_update_is_recorded_in_the_operation_log() {
    let emulator = Emulator::new();
    let name = "logged-attr-lb";
    emulator.create(REGION, name).await;

    let answered = emulator
        .update(REGION, name, SESSION_STICKINESS_ENABLED, "true")
        .await;
    let operations = answered["operations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        operations.len(),
        1,
        "case one load balancer reconfigured: expected one operation, got {answered:?}"
    );
    let operation = &operations[0];
    assert_eq!(
        operation["operationType"].as_str(),
        Some("UpdateLoadBalancerAttribute"),
        "case the answered operation: expected the UpdateLoadBalancerAttribute type, \
         got {operation:?}"
    );
    assert_eq!(
        operation["resourceType"].as_str(),
        Some("LoadBalancer"),
        "case the answered operation: expected a LoadBalancer resource, got {operation:?}"
    );
    assert_eq!(
        operation["resourceName"].as_str(),
        Some(name),
        "case the answered operation: expected the load balancer it acted on, got {operation:?}"
    );
    let id = operation["id"]
        .as_str()
        .expect("the answered operation carries an id");

    let read = emulator
        .ok(REGION, "GetOperation", json!({"operationId": id}))
        .await["operation"]
        .clone();
    assert_eq!(
        read["operationType"].as_str(),
        Some("UpdateLoadBalancerAttribute"),
        "case reading the logged operation back: expected the same type, got {read:?}"
    );

    let for_resource = emulator
        .ok(
            REGION,
            "GetOperationsForResource",
            json!({"resourceName": name}),
        )
        .await;
    let logged = for_resource["operations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        logged
            .iter()
            .any(|entry| entry["id"].as_str() == Some(id)),
        "case the resource's own log: expected it to carry the update, got {for_resource:?}"
    );
}
