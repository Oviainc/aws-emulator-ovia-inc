// SPDX-License-Identifier: CC0-1.0

//! The Lightsail load balancer control plane — create and read back — driven
//! over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `CreateLoadBalancer` stores ONE load balancer record per
//! (Region, name) — the caller's instance port, health check path, IP address
//! type, TLS policy, certificate request and tags kept verbatim, every
//! server-assigned member (ARN, support code, creation time, DNS name, listener
//! protocol and public ports, configuration options, lifecycle) computed from
//! the request plus the store's own allocators — and `GetLoadBalancer` reports
//! exactly that record in the Region the request is made in, with a name that
//! names nothing answered as the service's `DoesNotExist` rejection.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire
/// protocol").
const TARGET_PREFIX: &str = "Lightsail_20161128";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN
/// carries.
const ACCOUNT: &str = "000000000000";

/// The DNS suffix a load balancer's `dnsName` is published under
/// (`crate::state::LOAD_BALANCER_DNS_SUFFIX`).
const DNS_SUFFIX: &str = "lb.lightsail.local";

/// How many characters of the record's own id its `dnsName` carries
/// (`crate::state::LOAD_BALANCER_DNS_ID_LEN`), the length of the suffix ELB puts
/// between a load balancer's name and its Region.
const DNS_ID_LEN: usize = 10;

/// The public port a load balancer with no attached certificate listens on, from
/// the modelled `LoadBalancer$publicPorts` documentation ("For HTTP, use port
/// 80").
const HTTP_PORT: i64 = 80;

/// The health check path a load balancer uses when the request named none, from
/// the modelled `healthCheckPath` documentation ("Lightsail uses the root path
/// of your website (\"/\")").
const DEFAULT_HEALTH_CHECK_PATH: &str = "/";

/// The documented default IP address type (`ipAddressType`: "The default value
/// is dualstack").
const DEFAULT_IP_ADDRESS_TYPE: &str = "dualstack";

/// The documented per-Region cap (`CreateLoadBalancer`: "You can create up to 10
/// load balancers per AWS Region in your account").
const MAX_LOAD_BALANCERS_PER_REGION: usize = 10;

/// The session-stickiness cookie lifetime a new load balancer reports, from
/// ELB's own `stickiness.lb_cookie.duration_seconds` default of 86400 seconds.
const DEFAULT_COOKIE_DURATION: &str = "86400";

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

    /// Call `operation` with `body`, in `region` when one is given, answering its
    /// HTTP status and parsed body.
    async fn call(&self, region: Option<&str>, operation: &str, body: Value) -> (u16, Value) {
        let mut builder = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "127.0.0.1:9297")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("content-type", "application/x-amz-json-1.1");
        if let Some(region) = region {
            builder = builder.header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261022/{region}/lightsail/\
                     aws4_request, SignedHeaders=host, Signature=not-verified"
                ),
            );
        }
        let request = builder
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
        let (status, parsed) = self.call(Some(region), operation, body).await;
        assert_eq!(
            status, 200,
            "case {operation} in {region}: expected HTTP 200, got {status} {parsed:?}"
        );
        parsed
    }

    /// The stored load balancer `name` in `region`, as `GetLoadBalancer` reports
    /// it.
    async fn read(&self, region: &str, name: &str) -> Value {
        self.ok(region, "GetLoadBalancer", json!({"loadBalancerName": name}))
            .await["loadBalancer"]
            .clone()
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

/// Whether `value` has the shape of the UUID Lightsail's resource ids take: five
/// hex groups of 8-4-4-4-12.
fn is_uuid_shaped(value: &str) -> bool {
    let lengths: Vec<usize> = value.split('-').map(str::len).collect();
    lengths == vec![8, 4, 4, 4, 12]
        && value.chars().all(|c| c == '-' || c.is_ascii_hexdigit())
}

/// Poll `GetLoadBalancer` until `name` reports `want`, answering the whole
/// record.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_state(emulator: &Emulator, region: &str, name: &str, want: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let balancer = emulator.read(region, name).await;
        if balancer["state"].as_str() == Some(want) {
            return balancer;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for '{want}' on {name}: expected it within 5s, got {:?}",
            balancer["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// The create/read table
// ---------------------------------------------------------------------------

/// One `CreateLoadBalancer` request, and what the rule says the stored load
/// balancer then reports. The rows differ along everything the rule generalises
/// over: the Region the request is made in (and so the ARN, the location and the
/// DNS name), the instance port, whether a health check path is asked for,
/// which IP address type is asked for (including each member of the modelled
/// enum and the absent case that takes the documented default), how many tags
/// the create carries, whether a TLS policy is named, and whether a certificate
/// — with or without alternative names — is requested.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    region: &'static str,
    name: &'static str,
    instance_port: i64,
    /// `healthCheckPath` as sent, or `None` to send none at all.
    health_check_path: Option<&'static str>,
    /// `ipAddressType` as sent, or `None` to send none at all.
    ip_address_type: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
    tls_policy_name: Option<&'static str>,
    /// `(certificateName, certificateDomainName, certificateAlternativeNames)`,
    /// or `None` for a create that requests no certificate.
    certificate: Option<(&'static str, &'static str, &'static [&'static str])>,
}

/// The table. Every expected value is the one the modelled documentation names,
/// recorded beside the constants above.
const ROWS: &[Row] = &[
    Row {
        case: "nothing optional: the documented defaults",
        region: "us-east-1",
        name: "plain-lb",
        instance_port: 80,
        health_check_path: None,
        ip_address_type: None,
        tags: &[],
        tls_policy_name: None,
        certificate: None,
    },
    Row {
        case: "a custom health check path, ipv4 only, one tag",
        region: "us-west-2",
        name: "health-checked-lb",
        instance_port: 8080,
        health_check_path: Some("/healthz"),
        ip_address_type: Some("ipv4"),
        tags: &[("env", "test")],
        tls_policy_name: None,
        certificate: None,
    },
    Row {
        case: "ipv6 only, three tags, a TLS policy named",
        region: "eu-west-1",
        name: "ipv6-lb",
        instance_port: 443,
        health_check_path: Some("/"),
        ip_address_type: Some("ipv6"),
        tags: &[("env", "prod"), ("team", "payments"), ("tier", "edge")],
        tls_policy_name: Some("TLS-FS-1-2-Res-2020-10"),
        certificate: None,
    },
    Row {
        case: "a certificate with no alternative names",
        region: "ap-southeast-2",
        name: "cert-lb",
        instance_port: 3000,
        health_check_path: Some("/status"),
        ip_address_type: Some("dualstack"),
        tags: &[],
        tls_policy_name: None,
        certificate: Some(("site-cert", "example.com", &[])),
    },
    Row {
        case: "a certificate with two alternative names, at the lowest modelled port",
        region: "us-east-2",
        name: "alt-names-lb",
        instance_port: -1,
        health_check_path: Some("/deep/health/check"),
        ip_address_type: Some("ipv4"),
        tags: &[("owner", "team-a")],
        tls_policy_name: Some("TLS-2016-08"),
        certificate: Some((
            "multi-cert",
            "example.com",
            &["www.example.com", "blog.example.com"],
        )),
    },
    Row {
        case: "at the highest modelled port",
        region: "us-east-1",
        name: "top-port-lb",
        instance_port: 65535,
        health_check_path: None,
        ip_address_type: Some("dualstack"),
        tags: &[],
        tls_policy_name: None,
        certificate: None,
    },
];

impl Row {
    /// This row as the `CreateLoadBalancer` request body.
    fn request(&self) -> Value {
        let mut body = json!({
            "loadBalancerName": self.name,
            "instancePort": self.instance_port,
        });
        let map = body.as_object_mut().expect("the body is a JSON object");
        if let Some(path) = self.health_check_path {
            map.insert("healthCheckPath".to_string(), json!(path));
        }
        if let Some(ip_address_type) = self.ip_address_type {
            map.insert("ipAddressType".to_string(), json!(ip_address_type));
        }
        if let Some(policy) = self.tls_policy_name {
            map.insert("tlsPolicyName".to_string(), json!(policy));
        }
        if !self.tags.is_empty() {
            map.insert(
                "tags".to_string(),
                Value::Array(
                    self.tags
                        .iter()
                        .map(|(key, value)| json!({"key": key, "value": value}))
                        .collect(),
                ),
            );
        }
        if let Some((name, domain, alternatives)) = self.certificate {
            map.insert("certificateName".to_string(), json!(name));
            map.insert("certificateDomainName".to_string(), json!(domain));
            if !alternatives.is_empty() {
                map.insert(
                    "certificateAlternativeNames".to_string(),
                    json!(alternatives),
                );
            }
        }
        body
    }

    /// The health check path this row's load balancer must report.
    fn expected_health_check_path(&self) -> &'static str {
        self.health_check_path.unwrap_or(DEFAULT_HEALTH_CHECK_PATH)
    }

    /// The IP address type this row's load balancer must report.
    fn expected_ip_address_type(&self) -> &'static str {
        self.ip_address_type.unwrap_or(DEFAULT_IP_ADDRESS_TYPE)
    }
}

/// Assert everything the rule says the record reports for `row`, except the
/// lifecycle (which the clock moves) — so the same assertions hold on every
/// read.
fn assert_record(row: &Row, balancer: &Value) {
    let case = row.case;
    assert_eq!(
        balancer["name"].as_str(),
        Some(row.name),
        "case {case}: expected the requested name, got {balancer:?}"
    );

    // Server-assigned identity.
    let arn = balancer["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected an arn, got {balancer:?}"));
    let arn_prefix = format!("arn:aws:lightsail:{}:{ACCOUNT}:LoadBalancer/", row.region);
    let id = arn
        .strip_prefix(&arn_prefix)
        .unwrap_or_else(|| panic!("case {case}: expected an arn '{arn_prefix}<id>', got {arn:?}"));
    assert!(
        is_uuid_shaped(id),
        "case {case}: expected a uuid-shaped resource id, got {id:?}"
    );
    assert_eq!(
        balancer["supportCode"].as_str(),
        Some(format!("{ACCOUNT}/{id}").as_str()),
        "case {case}: expected the account/id support code, got {:?}",
        balancer["supportCode"]
    );
    assert!(
        balancer["createdAt"].as_f64().is_some_and(|at| at > 0.0),
        "case {case}: expected a creation timestamp, got {:?}",
        balancer["createdAt"]
    );
    assert_eq!(
        balancer["resourceType"].as_str(),
        Some("LoadBalancer"),
        "case {case}: expected the LoadBalancer resource type, got {balancer:?}"
    );

    // A load balancer is regional: Lightsail spreads it across the Region's
    // Availability Zones, so it names none of them.
    assert_eq!(
        balancer["location"]["regionName"].as_str(),
        Some(row.region),
        "case {case}: expected the Region of the request, got {:?}",
        balancer["location"]
    );
    assert!(
        balancer["location"]["availabilityZone"].is_null(),
        "case {case}: a load balancer is regional, so it must report no availability zone, \
         got {:?}",
        balancer["location"]
    );

    // The DNS name keeps the ELB shape under the emulator's own suffix.
    let dns_name = balancer["dnsName"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a dnsName, got {balancer:?}"));
    let dns_prefix = format!("{}-", row.name);
    let dns_suffix = format!(".{}.{DNS_SUFFIX}", row.region);
    let token = dns_name
        .strip_prefix(&dns_prefix)
        .and_then(|rest| rest.strip_suffix(&dns_suffix))
        .unwrap_or_else(|| {
            panic!("case {case}: expected '{dns_prefix}<token>{dns_suffix}', got {dns_name:?}")
        });
    assert_eq!(
        token.len(),
        DNS_ID_LEN,
        "case {case}: expected a {DNS_ID_LEN}-character dns token, got {token:?}"
    );
    assert!(
        token.chars().all(|c| c.is_ascii_alphanumeric()),
        "case {case}: expected an alphanumeric dns token, got {token:?}"
    );

    // What the caller asked for, kept verbatim.
    assert_eq!(
        balancer["instancePort"].as_i64(),
        Some(row.instance_port),
        "case {case}: expected the requested instance port, got {:?}",
        balancer["instancePort"]
    );
    assert_eq!(
        balancer["healthCheckPath"].as_str(),
        Some(row.expected_health_check_path()),
        "case {case}: expected the requested (or default) health check path, got {:?}",
        balancer["healthCheckPath"]
    );
    assert_eq!(
        balancer["ipAddressType"].as_str(),
        Some(row.expected_ip_address_type()),
        "case {case}: expected the requested (or default) ip address type, got {:?}",
        balancer["ipAddressType"]
    );
    assert_eq!(
        balancer["tlsPolicyName"].as_str(),
        row.tls_policy_name,
        "case {case}: expected the requested TLS policy, got {:?}",
        balancer["tlsPolicyName"]
    );
    let tags: Vec<(String, String)> = balancer["tags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a tag array, got {balancer:?}"))
        .iter()
        .map(|tag| {
            (
                tag["key"].as_str().unwrap_or_default().to_string(),
                tag["value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let want_tags: Vec<(String, String)> = row
        .tags
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    assert_eq!(
        tags, want_tags,
        "case {case}: expected the requested tags verbatim, got {tags:?}"
    );

    // A certificate the create requested exists but is NOT attached, so the
    // load balancer still serves plain HTTP on port 80 alone.
    let summaries = balancer["tlsCertificateSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a summary array, got {balancer:?}"));
    match row.certificate {
        None => assert!(
            summaries.is_empty(),
            "case {case}: a create that requested no certificate must report none, \
             got {summaries:?}"
        ),
        Some((certificate_name, _, _)) => {
            assert_eq!(
                summaries.len(),
                1,
                "case {case}: expected exactly the requested certificate, got {summaries:?}"
            );
            assert_eq!(
                summaries[0]["name"].as_str(),
                Some(certificate_name),
                "case {case}: expected the requested certificate name, got {summaries:?}"
            );
            assert_eq!(
                summaries[0]["isAttached"].as_bool(),
                Some(false),
                "case {case}: a certificate nothing attached must report isAttached false, \
                 got {summaries:?}"
            );
        }
    }
    assert_eq!(
        balancer["protocol"].as_str(),
        Some("HTTP"),
        "case {case}: with no attached certificate the protocol must be HTTP, got {:?}",
        balancer["protocol"]
    );
    assert_eq!(
        balancer["publicPorts"].as_array().map(|ports| ports
            .iter()
            .filter_map(Value::as_i64)
            .collect::<Vec<_>>()),
        Some(vec![HTTP_PORT]),
        "case {case}: with no attached certificate the only public port must be 80, got {:?}",
        balancer["publicPorts"]
    );
    assert_eq!(
        balancer["httpsRedirectionEnabled"].as_bool(),
        Some(false),
        "case {case}: HTTPS redirection is an UpdateLoadBalancerAttribute setting, so a \
         create must leave it off, got {:?}",
        balancer["httpsRedirectionEnabled"]
    );

    // Nothing is registered with a load balancer the create just made.
    assert_eq!(
        balancer["instanceHealthSummary"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "case {case}: nothing is attached yet, so the health summary must be empty, got {:?}",
        balancer["instanceHealthSummary"]
    );

    // The configuration options are the modelled `LoadBalancerAttributeName`
    // members this record holds a value for.
    let options: BTreeMap<String, String> = balancer["configurationOptions"]
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: expected a configuration map, got {balancer:?}"))
        .iter()
        .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
        .collect();
    let mut want_options = BTreeMap::new();
    want_options.insert(
        "HealthCheckPath".to_string(),
        row.expected_health_check_path().to_string(),
    );
    want_options.insert("SessionStickinessEnabled".to_string(), "false".to_string());
    want_options.insert(
        "SessionStickiness_LB_CookieDurationSeconds".to_string(),
        DEFAULT_COOKIE_DURATION.to_string(),
    );
    want_options.insert("HttpsRedirectionEnabled".to_string(), "false".to_string());
    if let Some(policy) = row.tls_policy_name {
        want_options.insert("TlsPolicyName".to_string(), policy.to_string());
    }
    assert_eq!(
        options, want_options,
        "case {case}: expected the configuration options the record holds, got {options:?}"
    );
}

/// Every row: create, read the record straight back while it is still
/// provisioning, then read it again once the clock has made it active.
#[tokio::test(flavor = "current_thread")]
async fn create_load_balancer_stores_the_requested_record() {
    let emulator = Emulator::new();
    for row in ROWS {
        let case = row.case;
        let created = emulator
            .ok(row.region, "CreateLoadBalancer", row.request())
            .await;

        // The create answers exactly one operation-log entry, still open until
        // the provisioning the clock owes the load balancer is due.
        let operations = created["operations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {created:?}"));
        assert_eq!(
            operations.len(),
            1,
            "case {case}: CreateLoadBalancer creates one resource, so it answers one \
             operation, got {operations:?}"
        );
        let operation = &operations[0];
        assert_eq!(
            operation["operationType"].as_str(),
            Some("CreateLoadBalancer"),
            "case {case}: expected the CreateLoadBalancer operation type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceType"].as_str(),
            Some("LoadBalancer"),
            "case {case}: expected the LoadBalancer resource type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceName"].as_str(),
            Some(row.name),
            "case {case}: expected the created load balancer's name, got {operation:?}"
        );
        assert_eq!(
            operation["status"].as_str(),
            Some("Started"),
            "case {case}: provisioning is still owed, so the operation is Started, \
             got {operation:?}"
        );
        assert_eq!(
            operation["isTerminal"].as_bool(),
            Some(false),
            "case {case}: provisioning is still owed, so the operation is not terminal, \
             got {operation:?}"
        );
        assert_eq!(
            operation["location"]["regionName"].as_str(),
            Some(row.region),
            "case {case}: expected the Region of the request, got {operation:?}"
        );

        // A read straight after the create still sees the transient state.
        let provisioning = emulator.read(row.region, row.name).await;
        assert_eq!(
            provisioning["state"].as_str(),
            Some("provisioning"),
            "case {case}: a load balancer that was just created is still provisioning, \
             got {provisioning:?}"
        );
        assert_record(row, &provisioning);

        // And the clock settles it into `active` without anybody asking twice.
        let active = wait_for_state(&emulator, row.region, row.name, "active").await;
        assert_record(row, &active);

        // Reading does not change what a later caller sees: the identity the
        // store assigned is the same on the second read.
        let again = emulator.read(row.region, row.name).await;
        assert_eq!(
            again["arn"], active["arn"],
            "case {case}: a read must not re-mint the ARN, got {again:?}"
        );
        assert_eq!(
            again["createdAt"], active["createdAt"],
            "case {case}: a read must not move the creation time, got {again:?}"
        );
        assert_eq!(
            again["dnsName"], active["dnsName"],
            "case {case}: a read must not re-mint the DNS name, got {again:?}"
        );
        assert_eq!(
            again["state"].as_str(),
            Some("active"),
            "case {case}: an active load balancer stays active, got {again:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// One request the model admits but the service refuses, and why.
#[derive(Debug, Clone, Copy)]
struct Refusal {
    case: &'static str,
    body: fn() -> Value,
    /// The fragment the refusal's message must name, so the caller can fix the
    /// request from the text alone.
    names: &'static str,
}

/// The refusal table. Each row breaks a rule the modelled documentation states
/// but the model itself cannot express.
const REFUSALS: &[Refusal] = &[
    Refusal {
        case: "certificateName with no certificateDomainName",
        body: || json!({
            "loadBalancerName": "half-cert-lb",
            "instancePort": 80,
            "certificateName": "site-cert",
        }),
        names: "certificateDomainName",
    },
    Refusal {
        case: "certificateDomainName with no certificateName",
        body: || json!({
            "loadBalancerName": "half-cert-lb",
            "instancePort": 80,
            "certificateDomainName": "example.com",
        }),
        names: "certificateName",
    },
    Refusal {
        case: "alternative names with no certificate at all",
        body: || json!({
            "loadBalancerName": "orphan-alt-lb",
            "instancePort": 80,
            "certificateAlternativeNames": ["www.example.com"],
        }),
        names: "certificateAlternativeNames",
    },
    Refusal {
        case: "a blank health check path, which no load balancer could report",
        body: || json!({
            "loadBalancerName": "blank-path-lb",
            "instancePort": 80,
            "healthCheckPath": "   ",
        }),
        names: "healthCheckPath",
    },
];

#[tokio::test(flavor = "current_thread")]
async fn create_load_balancer_refuses_a_request_the_service_rejects() {
    let emulator = Emulator::new();
    for refusal in REFUSALS {
        let case = refusal.case;
        let (status, body) = emulator
            .call(Some("us-east-1"), "CreateLoadBalancer", (refusal.body)())
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case}: expected the modelled InvalidInputException, got {body:?}"
        );
        let message = body["message"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a message, got {body:?}"));
        assert!(
            message.contains(refusal.names),
            "case {case}: expected a message naming '{}', got {message:?}",
            refusal.names
        );

        // A refused request creates nothing.
        let name = (refusal.body)()["loadBalancerName"]
            .as_str()
            .expect("every refusal row names a load balancer")
            .to_string();
        let (read_status, read_body) = emulator
            .call(
                Some("us-east-1"),
                "GetLoadBalancer",
                json!({"loadBalancerName": name}),
            )
            .await;
        assert_eq!(
            read_status, 400,
            "case {case}: a refused create must store nothing, got {read_status} {read_body:?}"
        );
        assert_eq!(
            read_body["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: a refused create must store nothing, got {read_body:?}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn get_load_balancer_refuses_a_name_it_does_not_hold() {
    let emulator = Emulator::new();
    for (case, region, name) in [
        ("nothing created at all", "us-east-1", "absent-lb"),
        ("another Region's name", "eu-west-1", "regional-lb"),
    ] {
        let (status, body) = emulator
            .call(
                Some(region),
                "GetLoadBalancer",
                json!({"loadBalancerName": name}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: Lightsail answers a missing resource with 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case}: expected the modelled NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected the DoesNotExist body code, got {body:?}"
        );
    }
}

/// A load balancer name identifies one load balancer in ONE Region: the same
/// name in another Region is a different resource, and re-using it in the same
/// Region is refused.
#[tokio::test(flavor = "current_thread")]
async fn a_load_balancer_name_is_unique_within_one_region() {
    let emulator = Emulator::new();
    let request = json!({"loadBalancerName": "regional-lb", "instancePort": 80});

    let mut arns = Vec::new();
    for region in ["us-east-1", "eu-west-1", "ap-northeast-1"] {
        emulator
            .ok(region, "CreateLoadBalancer", request.clone())
            .await;
        let balancer = emulator.read(region, "regional-lb").await;
        let arn = balancer["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {region}: expected an arn, got {balancer:?}"))
            .to_string();
        assert!(
            arn.contains(&format!(":{region}:")),
            "case {region}: expected an ARN in the Region of the request, got {arn:?}"
        );
        arns.push(arn);
    }
    let distinct: std::collections::BTreeSet<&String> = arns.iter().collect();
    assert_eq!(
        distinct.len(),
        arns.len(),
        "case per-Region records: one name in three Regions is three resources, got {arns:?}"
    );

    let (status, body) = emulator
        .call(Some("us-east-1"), "CreateLoadBalancer", request)
        .await;
    assert_eq!(
        status, 400,
        "case duplicate name: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "case duplicate name: expected the modelled InvalidInputException, got {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("regional-lb") && message.contains("us-east-1"),
        "case duplicate name: expected a message naming the load balancer and the Region, \
         got {message:?}"
    );
}

/// "You can create up to 10 load balancers per AWS Region in your account":
/// the cap is per Region, so the eleventh in one Region is refused while another
/// Region still has room.
#[tokio::test(flavor = "current_thread")]
async fn the_documented_per_region_cap_is_enforced() {
    let emulator = Emulator::new();
    for index in 0..MAX_LOAD_BALANCERS_PER_REGION {
        emulator
            .ok(
                "us-east-1",
                "CreateLoadBalancer",
                json!({"loadBalancerName": format!("capped-lb-{index}"), "instancePort": 80}),
            )
            .await;
    }

    let (status, body) = emulator
        .call(
            Some("us-east-1"),
            "CreateLoadBalancer",
            json!({"loadBalancerName": "one-too-many-lb", "instancePort": 80}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case one past the cap: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "case one past the cap: expected the modelled InvalidInputException, got {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(&MAX_LOAD_BALANCERS_PER_REGION.to_string()),
        "case one past the cap: expected a message naming the cap, got {message:?}"
    );

    // Another Region is unaffected: the cap is per Region.
    emulator
        .ok(
            "us-west-2",
            "CreateLoadBalancer",
            json!({"loadBalancerName": "one-too-many-lb", "instancePort": 80}),
        )
        .await;
    let elsewhere = emulator.read("us-west-2", "one-too-many-lb").await;
    assert_eq!(
        elsewhere["name"].as_str(),
        Some("one-too-many-lb"),
        "case another Region: the cap is per Region, so this create stands, got {elsewhere:?}"
    );
}
