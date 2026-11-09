// SPDX-License-Identifier: CC0-1.0

//! The Lightsail CDN distribution control plane — `CreateDistribution` and
//! `GetDistributions` — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness is the one `instance_lifecycle.rs` already has, called
//! with a credential scope where a case needs a Region.
//!
//! The rule under test: `CreateDistribution` stores ONE distribution record per
//! name — the bundle, origin, cache behaviour, cache settings, per-path
//! behaviours, custom error responses, IP address type, certificate name,
//! minimum viewer TLS version, default root object and tags the request carried
//! kept verbatim, and every server-assigned member (ARN, support code, creation
//! time, domain name, status, the origin's resolved resource type and public
//! DNS) computed from that request plus this store's own allocators — and
//! `GetDistributions` answers from that one record, so what a create reported is
//! what a later read reports. Both operations are served by the N. Virginia
//! (`us-east-1`) endpoint ALONE, as the modelled `InvalidInputException`
//! documentation states for the domain and distribution APIs together, so a
//! request signed for any other Region is refused and stores nothing.
//!
//! So the cases are TABLES: one row per published bundle plan, per kind of origin
//! resource a distribution may pull from, per form each optional member takes
//! (absent, and each member of its modelled enum), per published bound of the
//! documented `responseTimeout` range, per caller-sized list size, and per Region
//! the origin is named in; the refusals are a table of the ways a request can be
//! outside what the service serves, including every Region the modelled
//! `RegionName` enum lists that is not the distribution Region.

use std::collections::BTreeSet;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::ops::types::REGION_NAME_VALUES;

use crate::distribution_certificate_attachment::{create_certificate, wait_for_issued_certificate};
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};
use crate::paging::{names, walk_pages};

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN and
/// support code carries.
const ACCOUNT: &str = "000000000000";

/// The Region every distribution is located in, from the modelled
/// `LightsailDistribution$location` documentation ("all distributions are
/// located in the `us-east-1` Region").
const DISTRIBUTION_REGION: &str = "us-east-1";

/// A second Region, used to name an origin that does not live where the request
/// is made.
const WEST_REGION: &str = "us-west-2";

/// The DNS suffix a distribution's `domainName` is published under
/// (`crate::state::DISTRIBUTION_DOMAIN_SUFFIX`), which is the emulator's own:
/// the announced name must not send a client off this machine.
const DOMAIN_SUFFIX: &str = "cdn.lightsail.local";

/// How many characters of the record's own id its `domainName` carries
/// (`crate::state::DISTRIBUTION_DOMAIN_ID_LEN`), the length CloudFront puts
/// after the leading `d` of a distribution domain name.
const DOMAIN_ID_LEN: usize = 13;

/// The documented `responseTimeout` range and default, from the modelled
/// `InputOrigin$responseTimeout` documentation ("The minimum timeout is 1
/// second, the maximum is 60 seconds, and the default (if you don't specify
/// otherwise) is 30 seconds").
const MIN_RESPONSE_TIMEOUT: i64 = 1;
/// See [`MIN_RESPONSE_TIMEOUT`].
const MAX_RESPONSE_TIMEOUT: i64 = 60;
/// See [`MIN_RESPONSE_TIMEOUT`].
const DEFAULT_RESPONSE_TIMEOUT: i64 = 30;

/// The documented default IP address type (`CreateDistribution`'s
/// `ipAddressType`: "The default value is dualstack").
const DEFAULT_IP_ADDRESS_TYPE: &str = "dualstack";

/// The status a distribution reports while the configuration a call stored is
/// still deploying, and once it has deployed — CloudFront's own distribution
/// status values, which a Lightsail distribution carries.
const STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`STATUS_IN_PROGRESS`].
const STATUS_DEPLOYED: &str = "Deployed";

/// How many distributions one `GetDistributions` page carries
/// (`crate::state::DISTRIBUTION_PAGE_SIZE`).
const PAGE_SIZE: usize = 10;

/// The most tags this service accepts on one resource
/// (`crate::state::MAX_TAGS`).
const MAX_TAGS: usize = 50;

/// The most per-path cache behaviours one distribution holds
/// (`crate::state::MAX_CACHE_BEHAVIORS_PER_DISTRIBUTION`).
const MAX_CACHE_BEHAVIORS: usize = 25;

/// The wire error code (awsJson `__type`) of a refusal of a request the model
/// admits but the service does not.
const INVALID_INPUT: &str = "InvalidInputException";

/// The wire error code of Lightsail's rejection of a name that addresses
/// nothing, and the `code` its body carries.
const NOT_FOUND: &str = "NotFoundException";
/// See [`NOT_FOUND`].
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The distribution bundles this service publishes, as `(bundleId,
/// transferPerMonthInGb, price)`.
///
/// Written out here independently of the service's own
/// `DISTRIBUTION_BUNDLE_SPECS` table, so these rows pin the published catalog
/// rather than restating whatever the code happens to hold.
///
/// SOURCE: the Lightsail pricing page's CDN distribution table — 50 GB of data
/// transfer for $2.50 a month, 200 GB for $5 and 500 GB for $10 — each the first
/// (`_1_0`) generation of its size.
const PUBLISHED_BUNDLES: &[(&str, i64, f64)] = &[
    ("small_1_0", 50, 2.5),
    ("medium_1_0", 200, 5.0),
    ("large_1_0", 500, 10.0),
];

/// Every member of the modelled `ViewerMinimumTlsProtocolVersionEnum`
/// (`smithy_query(query="shape",shape="ViewerMinimumTlsProtocolVersionEnum")`).
const TLS_VERSIONS: &[&str] = &[
    "TLSv1.1_2016",
    "TLSv1.2_2018",
    "TLSv1.2_2019",
    "TLSv1.2_2021",
];

/// Every member of the modelled `IpAddressType` enum
/// (`smithy_query(query="shape",shape="IpAddressType")`), which is the domain
/// `InputOrigin$ipAddressType` documents ("ipv4 for IPv4 only, ipv6 for IPv6
/// only, and dualstack for IPv4 and IPv6") for the connection to the origin.
const IP_ADDRESS_TYPES: &[&str] = &["dualstack", "ipv4", "ipv6"];

/// The address types the DISTRIBUTION itself may be given, from
/// `smithy_query(query="doc",shape="CreateDistributionRequest",member="ipAddressType")`:
/// "The possible values are `ipv4` for IPv4 only, and `dualstack` for IPv4 and
/// IPv6" — narrower than the enum the member targets.
const DISTRIBUTION_IP_ADDRESS_TYPES: &[&str] = &["ipv4", "dualstack"];

/// The instance blueprint, bundle and zone the origin instance in this suite is
/// created from: the origin's own configuration is not what any case is about.
const ORIGIN_BLUEPRINT: &str = "amazon_linux_2023";
const ORIGIN_INSTANCE_BUNDLE: &str = "nano_3_0";
const ORIGIN_ZONE: &str = "us-east-1a";

/// The origin resources every case pulls from, one per kind of resource an
/// origin may be ("An origin can be a Lightsail instance, bucket, container
/// service, or load balancer").
const ORIGIN_INSTANCE: &str = "origin-instance";
const ORIGIN_BUCKET: &str = "origin-bucket";
const ORIGIN_CONTAINER_SERVICE: &str = "origin-service";
const ORIGIN_LOAD_BALANCER: &str = "origin-lb";
/// A container service in [`WEST_REGION`], so a case can name an origin that is
/// not in the Region the request is made in.
const ORIGIN_WEST_SERVICE: &str = "west-service";

/// Now, in epoch seconds, for the window a reported `createdAt` must fall in.
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default()
}

/// Create one of each kind of origin resource, and answer the `dnsName` the load
/// balancer origin publishes — the one origin a distribution can report an
/// `originPublicDNS` for.
async fn create_origins(emulator: &Emulator) -> String {
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [ORIGIN_INSTANCE],
                "availabilityZone": ORIGIN_ZONE,
                "blueprintId": ORIGIN_BLUEPRINT,
                "bundleId": ORIGIN_INSTANCE_BUNDLE,
            }),
        )
        .await;
    emulator
        .ok(
            "CreateBucket",
            json!({"bucketName": ORIGIN_BUCKET, "bundleId": "small_1_0"}),
        )
        .await;
    // `CreateContainerService` answers the modelled 201 for a resource it
    // created, not 200, so these two cannot go through `ok`.
    let (status, created) = emulator
        .call_at(
            None,
            None,
            "CreateContainerService",
            json!({"serviceName": ORIGIN_CONTAINER_SERVICE, "power": "nano", "scale": 1}),
        )
        .await;
    assert_eq!(
        status, 201,
        "the container service origin must be created, got {status} {created:?}"
    );
    emulator
        .ok(
            "CreateLoadBalancer",
            json!({"loadBalancerName": ORIGIN_LOAD_BALANCER, "instancePort": 80}),
        )
        .await;
    let (status, created) = emulator
        .call_at(
            None,
            Some(WEST_REGION),
            "CreateContainerService",
            json!({"serviceName": ORIGIN_WEST_SERVICE, "power": "nano", "scale": 1}),
        )
        .await;
    assert_eq!(
        status, 201,
        "the container service origin in {WEST_REGION} must be created, got {status} {created:?}"
    );

    let balancer = emulator
        .ok(
            "GetLoadBalancer",
            json!({"loadBalancerName": ORIGIN_LOAD_BALANCER}),
        )
        .await;
    balancer["loadBalancer"]["dnsName"]
        .as_str()
        .expect("the load balancer origin publishes a dnsName")
        .to_string()
}

/// The `cacheBehaviorSettings` a row that carries them sends: every member of
/// the modelled `CacheSettings`, with each forwarding option a member of the
/// modelled `ForwardValues` enum and the forwarded header a member of the
/// modelled `HeaderEnum`.
fn cache_settings() -> Value {
    json!({
        "defaultTTL": 86_400,
        "minimumTTL": 0,
        "maximumTTL": 31_536_000,
        "allowedHTTPMethods": "GET,HEAD,OPTIONS",
        "cachedHTTPMethods": "GET,HEAD",
        "forwardedCookies": {"option": "allow-list", "cookiesAllowList": ["session"]},
        "forwardedHeaders": {"option": "allow-list", "headersAllowList": ["Accept"]},
        "forwardedQueryStrings": {"option": true, "queryStringsAllowList": ["lang"]},
    })
}

// ---------------------------------------------------------------------------
// The create/read table
// ---------------------------------------------------------------------------

/// One `CreateDistribution` request, and what the rule says the stored
/// distribution then reports.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    name: &'static str,
    /// The Region the request is signed for, or `None` for a request that
    /// carries no credential scope at all.
    request_region: Option<&'static str>,
    origin_name: &'static str,
    /// `origin.regionName` as sent, absent when `None`.
    origin_region: Option<&'static str>,
    /// The Region the origin must be reported in — the request's own when the
    /// request named none.
    expected_origin_region: &'static str,
    /// The `ResourceType` the named origin must resolve to.
    expected_origin_type: &'static str,
    /// Whether the origin must report the load balancer's published DNS name.
    expects_origin_public_dns: bool,
    protocol_policy: Option<&'static str>,
    origin_ip_address_type: Option<&'static str>,
    /// `origin.responseTimeout` as sent, absent when `None`.
    response_timeout: Option<i64>,
    behavior: &'static str,
    bundle_id: &'static str,
    /// `ipAddressType` as sent, absent when `None`.
    ip_address_type: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
    /// The per-path cache behaviours as `(path, behavior)`.
    cache_behaviors: &'static [(&'static str, &'static str)],
    /// The custom error responses as `(errorCode, responseCode,
    /// responsePagePath, errorCachingMinTTL)`.
    custom_error_responses: &'static [(i64, &'static str, &'static str, i64)],
    /// Whether the request carries a full `cacheBehaviorSettings` object.
    carries_cache_settings: bool,
    certificate_name: Option<&'static str>,
    viewer_min_tls: Option<&'static str>,
    default_root_object: Option<&'static str>,
    /// `enablePrivateOriginAccess` as sent, absent when `None`.
    enable_private_origin_access: Option<bool>,
}

impl Row {
    /// This row's request body.
    fn request(&self) -> Value {
        let mut origin = json!({"name": self.origin_name});
        if let Some(region) = self.origin_region {
            origin["regionName"] = json!(region);
        }
        if let Some(policy) = self.protocol_policy {
            origin["protocolPolicy"] = json!(policy);
        }
        if let Some(timeout) = self.response_timeout {
            origin["responseTimeout"] = json!(timeout);
        }
        if let Some(ip_address_type) = self.origin_ip_address_type {
            origin["ipAddressType"] = json!(ip_address_type);
        }

        let mut body = json!({
            "distributionName": self.name,
            "origin": origin,
            "defaultCacheBehavior": {"behavior": self.behavior},
            "bundleId": self.bundle_id,
        });
        if let Some(ip_address_type) = self.ip_address_type {
            body["ipAddressType"] = json!(ip_address_type);
        }
        if !self.tags.is_empty() {
            body["tags"] = Value::Array(
                self.tags
                    .iter()
                    .map(|(key, value)| json!({"key": key, "value": value}))
                    .collect(),
            );
        }
        if !self.cache_behaviors.is_empty() {
            body["cacheBehaviors"] = Value::Array(
                self.cache_behaviors
                    .iter()
                    .map(|(path, behavior)| json!({"path": path, "behavior": behavior}))
                    .collect(),
            );
        }
        if !self.custom_error_responses.is_empty() {
            body["customErrorResponses"] = Value::Array(
                self.custom_error_responses
                    .iter()
                    .map(|(error_code, response_code, page, ttl)| {
                        json!({
                            "errorCode": error_code,
                            "responseCode": response_code,
                            "responsePagePath": page,
                            "errorCachingMinTTL": ttl,
                        })
                    })
                    .collect(),
            );
        }
        if self.carries_cache_settings {
            body["cacheBehaviorSettings"] = cache_settings();
        }
        if let Some(certificate_name) = self.certificate_name {
            body["certificateName"] = json!(certificate_name);
        }
        if let Some(version) = self.viewer_min_tls {
            body["viewerMinimumTlsProtocolVersion"] = json!(version);
        }
        if let Some(root_object) = self.default_root_object {
            body["defaultRootObject"] = json!(root_object);
        }
        if let Some(enabled) = self.enable_private_origin_access {
            body["enablePrivateOriginAccess"] = json!(enabled);
        }
        body
    }

    /// The `origin.responseTimeout` the distribution must report: what the
    /// request asked for, or the documented default.
    fn expected_response_timeout(&self) -> i64 {
        self.response_timeout.unwrap_or(DEFAULT_RESPONSE_TIMEOUT)
    }

    /// The `ipAddressType` the distribution must report: what the request asked
    /// for, or the documented default.
    fn expected_ip_address_type(&self) -> &'static str {
        self.ip_address_type.unwrap_or(DEFAULT_IP_ADDRESS_TYPE)
    }

    /// The `origin.isPrivateOriginAccessEnabled` the distribution must report: a
    /// request that asked for nothing enabled nothing.
    fn expected_private_origin_access(&self) -> bool {
        self.enable_private_origin_access.unwrap_or(false)
    }
}

/// The create cases. The rows differ along every dimension the rule generalises
/// over: the published bundle plan, the kind of resource the origin is, whether
/// the origin names its own Region, whether the request is signed at all, the
/// origin protocol policy and IP address type, the response timeout (absent and
/// both published bounds), the default cache behaviour, the distribution's IP
/// address type (absent and members of the modelled enum), the number of tags,
/// per-path cache behaviours and custom error responses, whether cache settings
/// are carried, and whether a certificate, a minimum viewer TLS version, a
/// default root object or private origin access is asked for.
const ROWS: &[Row] = &[
    Row {
        case: "the cheapest plan on an instance origin, nothing optional at all",
        name: "cdn-instance-plain",
        request_region: None,
        origin_name: ORIGIN_INSTANCE,
        origin_region: None,
        expected_origin_region: DISTRIBUTION_REGION,
        expected_origin_type: "Instance",
        expects_origin_public_dns: false,
        protocol_policy: None,
        origin_ip_address_type: None,
        response_timeout: None,
        behavior: "cache",
        bundle_id: "small_1_0",
        ip_address_type: None,
        tags: &[],
        cache_behaviors: &[],
        custom_error_responses: &[],
        carries_cache_settings: false,
        certificate_name: None,
        viewer_min_tls: None,
        default_root_object: None,
        enable_private_origin_access: None,
    },
    Row {
        case: "a bucket origin with private origin access, per-path behaviours and a custom error page",
        name: "cdn-bucket-private",
        request_region: Some(DISTRIBUTION_REGION),
        origin_name: ORIGIN_BUCKET,
        origin_region: Some(DISTRIBUTION_REGION),
        expected_origin_region: DISTRIBUTION_REGION,
        expected_origin_type: "Bucket",
        expects_origin_public_dns: false,
        protocol_policy: Some("http-only"),
        origin_ip_address_type: Some("ipv4"),
        response_timeout: Some(MIN_RESPONSE_TIMEOUT),
        behavior: "dont-cache",
        bundle_id: "medium_1_0",
        ip_address_type: Some("ipv4"),
        tags: &[("env", "prod"), ("team", "web")],
        cache_behaviors: &[("var/www/html/*.html", "cache"), ("var/www/html/api/", "dont-cache")],
        custom_error_responses: &[(404, "404", "/404.html", 10), (403, "403", "/denied.html", 300)],
        carries_cache_settings: true,
        certificate_name: None,
        viewer_min_tls: Some("TLSv1.2_2021"),
        default_root_object: Some("index.html"),
        enable_private_origin_access: Some(true),
    },
    Row {
        case: "a load balancer origin reports the published DNS, at the maximum response timeout",
        name: "cdn-balancer-https",
        request_region: Some(DISTRIBUTION_REGION),
        origin_name: ORIGIN_LOAD_BALANCER,
        origin_region: None,
        expected_origin_region: DISTRIBUTION_REGION,
        expected_origin_type: "LoadBalancer",
        expects_origin_public_dns: true,
        protocol_policy: Some("https-only"),
        origin_ip_address_type: Some("dualstack"),
        response_timeout: Some(MAX_RESPONSE_TIMEOUT),
        behavior: "cache",
        bundle_id: "large_1_0",
        ip_address_type: Some("dualstack"),
        tags: &[("only-key", "")],
        cache_behaviors: &[("var/www/html/images/*.jpg", "cache")],
        custom_error_responses: &[],
        carries_cache_settings: false,
        certificate_name: Some("site-cert"),
        viewer_min_tls: Some("TLSv1.1_2016"),
        default_root_object: None,
        enable_private_origin_access: Some(false),
    },
    Row {
        case: "a container service origin in another Region, named by origin.regionName",
        name: "cdn-west-service",
        request_region: Some(DISTRIBUTION_REGION),
        origin_name: ORIGIN_WEST_SERVICE,
        origin_region: Some(WEST_REGION),
        expected_origin_region: WEST_REGION,
        expected_origin_type: "ContainerService",
        expects_origin_public_dns: false,
        protocol_policy: Some("https-only"),
        origin_ip_address_type: Some("ipv6"),
        response_timeout: Some(45),
        behavior: "dont-cache",
        bundle_id: "small_1_0",
        // The distribution's own address type, whose documented domain is
        // `ipv4` and `dualstack`; the `ipv6` above is the ORIGIN connection's,
        // whose member documents all three.
        ip_address_type: Some("ipv4"),
        tags: &[],
        cache_behaviors: &[],
        custom_error_responses: &[(500, "502", "/oops.html", 5)],
        carries_cache_settings: true,
        certificate_name: Some("west-cert"),
        viewer_min_tls: Some("TLSv1.2_2018"),
        default_root_object: Some("home.html"),
        enable_private_origin_access: None,
    },
    Row {
        case: "a bucket origin whose Region the request did not name, on an unsigned request",
        name: "cdn-bucket-default-region",
        request_region: None,
        origin_name: ORIGIN_BUCKET,
        origin_region: None,
        expected_origin_region: DISTRIBUTION_REGION,
        expected_origin_type: "Bucket",
        expects_origin_public_dns: false,
        protocol_policy: None,
        origin_ip_address_type: None,
        response_timeout: None,
        behavior: "cache",
        bundle_id: "medium_1_0",
        ip_address_type: None,
        tags: &[("region", "east")],
        cache_behaviors: &[],
        custom_error_responses: &[],
        carries_cache_settings: false,
        certificate_name: None,
        viewer_min_tls: Some("TLSv1.2_2019"),
        default_root_object: None,
        enable_private_origin_access: Some(false),
    },
];

/// Request every certificate the rows name and wait until the certificate
/// authority has issued each one.
///
/// A distribution's `certificateName` names an SSL/TLS certificate of the
/// account that has been ISSUED, so a row that names one needs that certificate
/// to exist and be issued before its create is made. The names come from the
/// rows themselves, so this cannot drift from the table, and the stand-up and
/// the issuance wait are this test target's own, shared with the suites that
/// pin the attachment.
async fn create_row_certificates(emulator: &Emulator) {
    let names: BTreeSet<&str> = ROWS.iter().filter_map(|row| row.certificate_name).collect();
    for name in &names {
        create_certificate(emulator, name, &format!("{name}.example.com"), &[]).await;
    }
    for name in &names {
        wait_for_issued_certificate(emulator, name).await;
    }
}

/// Every tag the record reports, as `(key, value)` pairs in the store's order.
fn reported_tags(distribution: &Value) -> Vec<(String, String)> {
    distribution["tags"]
        .as_array()
        .map(|tags| {
            tags.iter()
                .map(|tag| {
                    (
                        tag["key"].as_str().unwrap_or_default().to_string(),
                        tag["value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Assert `distribution` is what `row`'s request says the stored record holds.
/// `balancer_dns` is the `dnsName` the load balancer origin publishes.
fn assert_record(row: &Row, distribution: &Value, balancer_dns: &str, created_after: f64) {
    let case = row.case;

    // What the caller asked for, kept verbatim.
    assert_eq!(
        distribution["name"].as_str(),
        Some(row.name),
        "case {case}: expected the requested name, got {distribution:?}"
    );
    assert_eq!(
        distribution["bundleId"].as_str(),
        Some(row.bundle_id),
        "case {case}: expected the requested bundle, got {distribution:?}"
    );
    assert_eq!(
        distribution["defaultCacheBehavior"]["behavior"].as_str(),
        Some(row.behavior),
        "case {case}: expected the requested default cache behaviour, got {distribution:?}"
    );
    assert_eq!(
        distribution["ipAddressType"].as_str(),
        Some(row.expected_ip_address_type()),
        "case {case}: expected the requested IP address type (or the documented default), \
         got {distribution:?}"
    );
    let expected_tags: Vec<(String, String)> = row
        .tags
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    assert_eq!(
        reported_tags(distribution),
        expected_tags,
        "case {case}: expected the requested tags, got {:?}",
        distribution["tags"]
    );
    let expected_behaviors: Vec<Value> = row
        .cache_behaviors
        .iter()
        .map(|(path, behavior)| json!({"path": path, "behavior": behavior}))
        .collect();
    assert_eq!(
        distribution["cacheBehaviors"],
        Value::Array(expected_behaviors),
        "case {case}: expected the requested per-path cache behaviours, got {:?}",
        distribution["cacheBehaviors"]
    );
    let expected_errors: Vec<Value> = row
        .custom_error_responses
        .iter()
        .map(|(error_code, response_code, page, ttl)| {
            json!({
                "errorCode": error_code,
                "responseCode": response_code,
                "responsePagePath": page,
                "errorCachingMinTTL": ttl,
            })
        })
        .collect();
    assert_eq!(
        distribution["customErrorResponses"],
        Value::Array(expected_errors),
        "case {case}: expected the requested custom error responses, got {:?}",
        distribution["customErrorResponses"]
    );
    if row.carries_cache_settings {
        assert_eq!(
            distribution["cacheBehaviorSettings"],
            cache_settings(),
            "case {case}: expected the requested cache settings, got {:?}",
            distribution["cacheBehaviorSettings"]
        );
    } else {
        assert!(
            distribution["cacheBehaviorSettings"].is_null(),
            "case {case}: a request that set no cache settings gets none invented for it, \
             got {:?}",
            distribution["cacheBehaviorSettings"]
        );
    }
    assert_eq!(
        distribution["certificateName"].as_str(),
        row.certificate_name,
        "case {case}: expected the requested certificate name, got {distribution:?}"
    );
    assert_eq!(
        distribution["viewerMinimumTlsProtocolVersion"].as_str(),
        row.viewer_min_tls,
        "case {case}: expected the requested minimum viewer TLS version, got {distribution:?}"
    );
    assert_eq!(
        distribution["defaultRootObject"].as_str(),
        row.default_root_object,
        "case {case}: expected the requested default root object, got {distribution:?}"
    );

    // The origin: the caller's members kept verbatim, plus the resource type
    // this store resolved the name to.
    let origin = &distribution["origin"];
    assert_eq!(
        origin["name"].as_str(),
        Some(row.origin_name),
        "case {case}: expected the requested origin name, got {origin:?}"
    );
    assert_eq!(
        origin["regionName"].as_str(),
        Some(row.expected_origin_region),
        "case {case}: expected the origin's Region (the request's own when it named none), \
         got {origin:?}"
    );
    assert_eq!(
        origin["resourceType"].as_str(),
        Some(row.expected_origin_type),
        "case {case}: expected the resource type the origin name resolves to, got {origin:?}"
    );
    assert_eq!(
        origin["protocolPolicy"].as_str(),
        row.protocol_policy,
        "case {case}: expected the requested origin protocol policy, got {origin:?}"
    );
    assert_eq!(
        origin["ipAddressType"].as_str(),
        row.origin_ip_address_type,
        "case {case}: expected the requested origin IP address type, got {origin:?}"
    );
    assert_eq!(
        origin["responseTimeout"].as_i64(),
        Some(row.expected_response_timeout()),
        "case {case}: expected the requested response timeout (or the documented default), \
         got {origin:?}"
    );
    assert_eq!(
        origin["isPrivateOriginAccessEnabled"].as_bool(),
        Some(row.expected_private_origin_access()),
        "case {case}: expected the requested private origin access, got {origin:?}"
    );
    if row.expects_origin_public_dns {
        assert_eq!(
            distribution["originPublicDNS"].as_str(),
            Some(balancer_dns),
            "case {case}: a load balancer origin reports the DNS name it publishes, \
             got {distribution:?}"
        );
    } else {
        assert!(
            distribution["originPublicDNS"].is_null(),
            "case {case}: this store publishes no public DNS name for a {} origin, so the \
             member is left out, got {:?}",
            row.expected_origin_type,
            distribution["originPublicDNS"]
        );
    }

    // What the service assigns, each computed from the request plus the store's
    // own allocators.
    assert_eq!(
        distribution["resourceType"].as_str(),
        Some("Distribution"),
        "case {case}: expected the Distribution resource type, got {distribution:?}"
    );
    assert_eq!(
        distribution["location"]["regionName"].as_str(),
        Some(DISTRIBUTION_REGION),
        "case {case}: every distribution is located in {DISTRIBUTION_REGION}, got {:?}",
        distribution["location"]
    );
    assert!(
        distribution["location"]["availabilityZone"].is_null(),
        "case {case}: a distribution is global, so it reports no availability zone, got {:?}",
        distribution["location"]
    );
    let arn_prefix = format!("arn:aws:lightsail:{DISTRIBUTION_REGION}:{ACCOUNT}:Distribution/");
    let arn = distribution["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected an arn, got {distribution:?}"));
    let arn_id = arn
        .strip_prefix(&arn_prefix)
        .unwrap_or_else(|| panic!("case {case}: expected an arn under {arn_prefix:?}, got {arn:?}"));
    assert!(
        is_uuid_shaped(arn_id),
        "case {case}: expected a uuid-shaped arn id, got {arn_id:?}"
    );
    assert_eq!(
        distribution["supportCode"].as_str(),
        Some(format!("{ACCOUNT}/{arn_id}").as_str()),
        "case {case}: the support code must name the account and this distribution's id, \
         got {distribution:?}"
    );
    let domain_name = distribution["domainName"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a domainName, got {distribution:?}"));
    let label = domain_name
        .strip_suffix(&format!(".{DOMAIN_SUFFIX}"))
        .unwrap_or_else(|| {
            panic!("case {case}: expected a domain name under .{DOMAIN_SUFFIX}, got {domain_name:?}")
        });
    assert_eq!(
        label.len(),
        DOMAIN_ID_LEN + 1,
        "case {case}: expected the CloudFront shape d<{DOMAIN_ID_LEN} characters>, \
         got {domain_name:?}"
    );
    assert!(
        label.starts_with('d') && label.chars().all(|c| c.is_ascii_alphanumeric()),
        "case {case}: expected the CloudFront shape d<token>, got {domain_name:?}"
    );
    let created_at = distribution["createdAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case}: expected a createdAt, got {distribution:?}"));
    assert!(
        created_at >= created_after - 300.0 && created_at <= now_epoch_seconds() + 300.0,
        "case {case}: expected the creation time to be now, got {created_at}"
    );
    assert_eq!(
        distribution["isEnabled"].as_bool(),
        Some(true),
        "case {case}: a created distribution is enabled, got {distribution:?}"
    );
    assert_eq!(
        distribution["ableToUpdateBundle"].as_bool(),
        Some(true),
        "case {case}: a distribution that has never been moved between bundles can be, \
         got {distribution:?}"
    );
    // The domains a distribution accepts HTTPS traffic for are the ones the
    // certificate it names covers, and a distribution that names no certificate
    // of its own publishes none. Every certificate these rows name is requested
    // for `<certificate name>.example.com` alone — see `create_row_certificates`.
    let expected_domains: Vec<Value> = row
        .certificate_name
        .map(|name| vec![json!(format!("{name}.example.com"))])
        .unwrap_or_default();
    assert_eq!(
        distribution["alternativeDomainNames"],
        Value::Array(expected_domains.clone()),
        "case {case}: expected the domains of the certificate the create named, \
         {expected_domains:?}, got {:?}",
        distribution["alternativeDomainNames"]
    );
}

/// Poll `GetDistributions` until `name` reports `want`, answering the record.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_status(emulator: &Emulator, name: &str, want: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let distribution = read(emulator, name).await;
        if distribution["status"].as_str() == Some(want) {
            return distribution;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for '{want}' on {name}: expected it within 5s, got {:?}",
            distribution["status"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The one distribution `GetDistributions` reports for `name`.
async fn read(emulator: &Emulator, name: &str) -> Value {
    let body = emulator
        .ok("GetDistributions", json!({"distributionName": name}))
        .await;
    let reported = body["distributions"]
        .as_array()
        .unwrap_or_else(|| panic!("case reading {name}: expected a distributions array, got {body:?}"));
    assert_eq!(
        reported.len(),
        1,
        "case reading {name}: a named read answers exactly that distribution, got {reported:?}"
    );
    reported[0].clone()
}

/// Every row: create, read the record straight back while it is still
/// deploying, then read it again once the clock has deployed it.
#[tokio::test(flavor = "current_thread")]
async fn create_distribution_stores_the_requested_record() {
    let emulator = Emulator::new();
    let balancer_dns = create_origins(&emulator).await;
    create_row_certificates(&emulator).await;

    // Nothing has been created yet, so a listing is an empty list rather than an
    // absent member.
    let empty = emulator.ok("GetDistributions", json!({})).await;
    assert_eq!(
        empty["distributions"],
        json!([]),
        "case the empty account: expected an empty distributions list, got {empty:?}"
    );

    for row in ROWS {
        let case = row.case;
        let before = now_epoch_seconds();
        let created = emulator
            .ok_at(None, row.request_region, "CreateDistribution", row.request())
            .await;

        // The create answers the distribution it stored, still deploying.
        let distribution = created["distribution"].clone();
        assert_eq!(
            distribution["status"].as_str(),
            Some(STATUS_IN_PROGRESS),
            "case {case}: a distribution that was just created is still deploying, \
             got {distribution:?}"
        );
        assert_record(row, &distribution, &balancer_dns, before);

        // And exactly one operation-log entry, open until the deployment the
        // clock owes the distribution is due.
        let operation = created["operation"].clone();
        assert_eq!(
            operation["operationType"].as_str(),
            Some("CreateDistribution"),
            "case {case}: expected the CreateDistribution operation type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceType"].as_str(),
            Some("Distribution"),
            "case {case}: expected the Distribution resource type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceName"].as_str(),
            Some(row.name),
            "case {case}: expected the created distribution's name, got {operation:?}"
        );
        assert_eq!(
            operation["status"].as_str(),
            Some("Started"),
            "case {case}: the deployment is still owed, so the operation is Started, \
             got {operation:?}"
        );
        assert_eq!(
            operation["isTerminal"].as_bool(),
            Some(false),
            "case {case}: the deployment is still owed, so the operation is not terminal, \
             got {operation:?}"
        );
        assert_eq!(
            operation["location"]["regionName"].as_str(),
            Some(DISTRIBUTION_REGION),
            "case {case}: the log entry is located where the distribution is, got {operation:?}"
        );

        // A read straight after the create answers from the same one record, and
        // still sees the transient status.
        let in_progress = read(&emulator, row.name).await;
        assert_eq!(
            in_progress["status"].as_str(),
            Some(STATUS_IN_PROGRESS),
            "case {case}: a read straight after the create still sees the deployment, \
             got {in_progress:?}"
        );
        assert_record(row, &in_progress, &balancer_dns, before);

        // And the clock deploys it without anybody asking twice.
        let deployed = wait_for_status(&emulator, row.name, STATUS_DEPLOYED).await;
        assert_record(row, &deployed, &balancer_dns, before);

        // Reading does not change what a later caller sees: the identity the
        // store assigned is the same on the second read.
        let again = read(&emulator, row.name).await;
        assert_eq!(
            again["arn"], deployed["arn"],
            "case {case}: a read must not re-mint the ARN, got {again:?}"
        );
        assert_eq!(
            again["createdAt"], deployed["createdAt"],
            "case {case}: a read must not move the creation time, got {again:?}"
        );
        assert_eq!(
            again["domainName"], deployed["domainName"],
            "case {case}: a read must not re-mint the domain name, got {again:?}"
        );
        assert_eq!(
            again["status"].as_str(),
            Some(STATUS_DEPLOYED),
            "case {case}: a deployed distribution stays deployed, got {again:?}"
        );
    }

    // Every row's distribution is in the one listing, and no two share an
    // identity the store minted.
    let listed = emulator.ok("GetDistributions", json!({})).await;
    let names: BTreeSet<String> = listed["distributions"]
        .as_array()
        .expect("the listing carries an array")
        .iter()
        .map(|distribution| distribution["name"].as_str().unwrap_or_default().to_string())
        .collect();
    let expected: BTreeSet<String> = ROWS.iter().map(|row| row.name.to_string()).collect();
    assert_eq!(
        names, expected,
        "case the full listing: expected every created distribution, got {names:?}"
    );
    let domains: BTreeSet<String> = listed["distributions"]
        .as_array()
        .expect("the listing carries an array")
        .iter()
        .map(|distribution| {
            distribution["domainName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        domains.len(),
        ROWS.len(),
        "case the full listing: each distribution gets its own domain name, got {domains:?}"
    );
}

// ---------------------------------------------------------------------------
// The published bundle catalog
// ---------------------------------------------------------------------------

/// Every bundle the pricing table publishes may be named by a create, and is
/// what the stored distribution then reports — including the plans no other case
/// in this suite cites.
#[tokio::test(flavor = "current_thread")]
async fn every_published_bundle_creates_a_distribution_on_that_plan() {
    let emulator = Emulator::new();
    create_origins(&emulator).await;

    for (bundle_id, transfer, price) in PUBLISHED_BUNDLES {
        let name = format!("cdn-{bundle_id}");
        let created = emulator
            .ok(
                "CreateDistribution",
                json!({
                    "distributionName": name,
                    "origin": {"name": ORIGIN_INSTANCE},
                    "defaultCacheBehavior": {"behavior": "cache"},
                    "bundleId": bundle_id,
                }),
            )
            .await;
        assert_eq!(
            created["distribution"]["bundleId"].as_str(),
            Some(*bundle_id),
            "case bundle {bundle_id} ({transfer}GB, ${price}/month): expected it applied to \
             the distribution, got {created:?}"
        );
        let stored = read(&emulator, &name).await;
        assert_eq!(
            stored["bundleId"].as_str(),
            Some(*bundle_id),
            "case bundle {bundle_id}: expected the read to report the stored plan, got {stored:?}"
        );
    }
}

/// Every address type a distribution may be given, and every member of the
/// modelled `ViewerMinimumTlsProtocolVersionEnum`, is kept as asked.
#[tokio::test(flavor = "current_thread")]
async fn every_modelled_enum_member_is_kept_verbatim() {
    let emulator = Emulator::new();
    create_origins(&emulator).await;

    for (index, ip_address_type) in DISTRIBUTION_IP_ADDRESS_TYPES.iter().enumerate() {
        let tls_version = TLS_VERSIONS[index % TLS_VERSIONS.len()];
        let name = format!("cdn-enum-{index}");
        emulator
            .ok(
                "CreateDistribution",
                json!({
                    "distributionName": name,
                    "origin": {"name": ORIGIN_INSTANCE},
                    "defaultCacheBehavior": {"behavior": "cache"},
                    "bundleId": "small_1_0",
                    "ipAddressType": ip_address_type,
                    "viewerMinimumTlsProtocolVersion": tls_version,
                }),
            )
            .await;
        let stored = read(&emulator, &name).await;
        assert_eq!(
            stored["ipAddressType"].as_str(),
            Some(*ip_address_type),
            "case ipAddressType {ip_address_type}: expected it kept verbatim, got {stored:?}"
        );
        assert_eq!(
            stored["viewerMinimumTlsProtocolVersion"].as_str(),
            Some(tls_version),
            "case viewerMinimumTlsProtocolVersion {tls_version}: expected it kept verbatim, \
             got {stored:?}"
        );
    }

    // Every member of the `IpAddressType` enum, on the member whose own
    // documentation admits all three: the connection to the ORIGIN.
    for (index, ip_address_type) in IP_ADDRESS_TYPES.iter().enumerate() {
        let name = format!("cdn-origin-enum-{index}");
        emulator
            .ok(
                "CreateDistribution",
                json!({
                    "distributionName": name,
                    "origin": {"name": ORIGIN_INSTANCE, "ipAddressType": ip_address_type},
                    "defaultCacheBehavior": {"behavior": "cache"},
                    "bundleId": "small_1_0",
                }),
            )
            .await;
        let stored = read(&emulator, &name).await;
        assert_eq!(
            stored["origin"]["ipAddressType"].as_str(),
            Some(*ip_address_type),
            "case origin.ipAddressType {ip_address_type}: expected it kept verbatim, got \
             {stored:?}"
        );
    }

    // The remaining TLS versions, so every member of that enum is exercised too.
    for (index, tls_version) in TLS_VERSIONS.iter().enumerate() {
        let name = format!("cdn-tls-{index}");
        emulator
            .ok(
                "CreateDistribution",
                json!({
                    "distributionName": name,
                    "origin": {"name": ORIGIN_INSTANCE},
                    "defaultCacheBehavior": {"behavior": "dont-cache"},
                    "bundleId": "small_1_0",
                    "viewerMinimumTlsProtocolVersion": tls_version,
                }),
            )
            .await;
        let stored = read(&emulator, &name).await;
        assert_eq!(
            stored["viewerMinimumTlsProtocolVersion"].as_str(),
            Some(*tls_version),
            "case viewerMinimumTlsProtocolVersion {tls_version}: expected it kept verbatim, \
             got {stored:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------

/// A listing walks every stored distribution in one total order, a page at a
/// time, and the walk terminates.
#[tokio::test(flavor = "current_thread")]
async fn get_distributions_pages_every_stored_distribution() {
    let emulator = Emulator::new();
    create_origins(&emulator).await;

    // More than two full pages, so the walk needs a third.
    let total = PAGE_SIZE * 2 + 3;
    let mut created = Vec::new();
    for index in 0..total {
        let name = format!("cdn-page-{index:03}");
        emulator
            .ok(
                "CreateDistribution",
                json!({
                    "distributionName": name,
                    "origin": {"name": ORIGIN_INSTANCE},
                    "defaultCacheBehavior": {"behavior": "cache"},
                    "bundleId": "small_1_0",
                }),
            )
            .await;
        created.push(name);
    }

    // The paging contract itself — a page no fuller than the page size, a token
    // naming the first record of the page after it, a full page before every
    // token and none on the last page — is checked by the one shared walk every
    // listing suite in this target drives (`crate::paging::walk_pages`).
    let (records, page_sizes) = walk_pages(
        &emulator,
        "paging",
        None,
        "GetDistributions",
        "distributions",
        PAGE_SIZE,
    )
    .await;
    let walked = names(&records);

    let mut expected = created.clone();
    expected.sort();
    assert_eq!(
        walked, expected,
        "case paging: expected every created distribution once, in name order, got {walked:?}"
    );
    assert_eq!(
        page_sizes,
        vec![PAGE_SIZE, PAGE_SIZE, total - PAGE_SIZE * 2],
        "case paging: expected full pages until the last, got {page_sizes:?}"
    );
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
        case: "a bundle this service does not publish",
        body: || {
            json!({
                "distributionName": "cdn-refused",
                "origin": {"name": ORIGIN_INSTANCE},
                "defaultCacheBehavior": {"behavior": "cache"},
                "bundleId": "enormous_9_9",
            })
        },
        names: "bundleId",
    },
    Refusal {
        case: "an origin that names no resource",
        body: || {
            json!({
                "distributionName": "cdn-refused",
                "origin": {"name": "never-created"},
                "defaultCacheBehavior": {"behavior": "cache"},
                "bundleId": "small_1_0",
            })
        },
        names: "never-created",
    },
    Refusal {
        case: "an origin in a Region the named resource is not in",
        body: || {
            json!({
                "distributionName": "cdn-refused",
                "origin": {"name": ORIGIN_INSTANCE, "regionName": WEST_REGION},
                "defaultCacheBehavior": {"behavior": "cache"},
                "bundleId": "small_1_0",
            })
        },
        names: ORIGIN_INSTANCE,
    },
    Refusal {
        case: "an origin with no name at all",
        body: || {
            json!({
                "distributionName": "cdn-refused",
                "origin": {"protocolPolicy": "http-only"},
                "defaultCacheBehavior": {"behavior": "cache"},
                "bundleId": "small_1_0",
            })
        },
        names: "origin.name",
    },
    Refusal {
        case: "a response timeout below the documented minimum",
        body: || {
            json!({
                "distributionName": "cdn-refused",
                "origin": {"name": ORIGIN_INSTANCE, "responseTimeout": MIN_RESPONSE_TIMEOUT - 1},
                "defaultCacheBehavior": {"behavior": "cache"},
                "bundleId": "small_1_0",
            })
        },
        names: "responseTimeout",
    },
    Refusal {
        case: "a response timeout above the documented maximum",
        body: || {
            json!({
                "distributionName": "cdn-refused",
                "origin": {"name": ORIGIN_INSTANCE, "responseTimeout": MAX_RESPONSE_TIMEOUT + 1},
                "defaultCacheBehavior": {"behavior": "cache"},
                "bundleId": "small_1_0",
            })
        },
        names: "responseTimeout",
    },
    Refusal {
        case: "a default cache behaviour that says nothing",
        body: || {
            json!({
                "distributionName": "cdn-refused",
                "origin": {"name": ORIGIN_INSTANCE},
                "defaultCacheBehavior": {},
                "bundleId": "small_1_0",
            })
        },
        names: "defaultCacheBehavior.behavior",
    },
    Refusal {
        case: "private origin access on an instance origin",
        body: || {
            json!({
                "distributionName": "cdn-refused",
                "origin": {"name": ORIGIN_INSTANCE},
                "defaultCacheBehavior": {"behavior": "cache"},
                "bundleId": "small_1_0",
                "enablePrivateOriginAccess": true,
            })
        },
        names: "enablePrivateOriginAccess",
    },
    Refusal {
        case: "private origin access on a load balancer origin",
        body: || {
            json!({
                "distributionName": "cdn-refused",
                "origin": {"name": ORIGIN_LOAD_BALANCER},
                "defaultCacheBehavior": {"behavior": "cache"},
                "bundleId": "small_1_0",
                "enablePrivateOriginAccess": true,
            })
        },
        names: "enablePrivateOriginAccess",
    },
];

/// Every refusal: HTTP 400, the modelled `InvalidInputException` or Lightsail's
/// `DoesNotExist` envelope, a message that names what is wrong, and NOTHING
/// stored.
#[tokio::test(flavor = "current_thread")]
async fn a_request_outside_what_the_service_serves_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    create_origins(&emulator).await;

    for refusal in REFUSALS {
        let case = refusal.case;
        let (status, body) = emulator
            .call("CreateDistribution", (refusal.body)())
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        let code = error_code(&body);
        assert!(
            code == INVALID_INPUT || code == NOT_FOUND,
            "case {case}: expected {INVALID_INPUT} or {NOT_FOUND}, got {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(refusal.names),
            "case {case}: expected a message naming {:?}, got {message:?}",
            refusal.names
        );
    }

    // Nothing any refused request named was stored.
    let listed = emulator.ok("GetDistributions", json!({})).await;
    assert_eq!(
        listed["distributions"],
        json!([]),
        "case after the refusals: a refused create stores nothing, got {listed:?}"
    );
}

/// A name that already names a distribution is refused rather than replacing the
/// record, and the stored one is untouched.
#[tokio::test(flavor = "current_thread")]
async fn a_name_that_is_already_taken_is_refused() {
    let emulator = Emulator::new();
    create_origins(&emulator).await;

    let name = "cdn-twice";
    let first = emulator
        .ok(
            "CreateDistribution",
            json!({
                "distributionName": name,
                "origin": {"name": ORIGIN_INSTANCE},
                "defaultCacheBehavior": {"behavior": "cache"},
                "bundleId": "small_1_0",
            }),
        )
        .await;
    let stored_arn = first["distribution"]["arn"].clone();

    let (status, body) = emulator
        .call(
            "CreateDistribution",
            json!({
                "distributionName": name,
                "origin": {"name": ORIGIN_BUCKET},
                "defaultCacheBehavior": {"behavior": "dont-cache"},
                "bundleId": "large_1_0",
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case a taken name: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_INPUT,
        "case a taken name: expected {INVALID_INPUT}, got {body:?}"
    );
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("distributionName"),
        "case a taken name: expected a message naming the member, got {body:?}"
    );

    let stored = read(&emulator, name).await;
    assert_eq!(
        stored["arn"], stored_arn,
        "case a taken name: the stored distribution is untouched, got {stored:?}"
    );
    assert_eq!(
        stored["bundleId"].as_str(),
        Some("small_1_0"),
        "case a taken name: the refused create did not move the bundle, got {stored:?}"
    );
    assert_eq!(
        stored["origin"]["resourceType"].as_str(),
        Some("Instance"),
        "case a taken name: the refused create did not move the origin, got {stored:?}"
    );
}

/// A caller-sized list past the bound this store keeps is refused, and nothing
/// is stored.
#[tokio::test(flavor = "current_thread")]
async fn a_list_past_its_bound_is_refused() {
    let emulator = Emulator::new();
    create_origins(&emulator).await;

    let too_many_tags: Vec<Value> = (0..=MAX_TAGS)
        .map(|index| json!({"key": format!("key-{index}"), "value": "v"}))
        .collect();
    let too_many_behaviors: Vec<Value> = (0..=MAX_CACHE_BEHAVIORS)
        .map(|index| json!({"path": format!("var/www/html/{index}/"), "behavior": "cache"}))
        .collect();
    let cases: &[(&str, &str, Value)] = &[
        ("more tags than this service accepts", "tags", json!(too_many_tags)),
        (
            "more per-path cache behaviours than one distribution holds",
            "cacheBehaviors",
            json!(too_many_behaviors),
        ),
    ];

    for (case, member, entries) in cases {
        let mut body = json!({
            "distributionName": "cdn-bounded",
            "origin": {"name": ORIGIN_INSTANCE},
            "defaultCacheBehavior": {"behavior": "cache"},
            "bundleId": "small_1_0",
        });
        body[*member] = entries.clone();
        let (status, answered) = emulator.call("CreateDistribution", body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            INVALID_INPUT,
            "case {case}: expected {INVALID_INPUT}, got {answered:?}"
        );
        assert!(
            answered["message"]
                .as_str()
                .unwrap_or_default()
                .contains(member),
            "case {case}: expected a message naming {member}, got {answered:?}"
        );
    }

    let listed = emulator.ok("GetDistributions", json!({})).await;
    assert_eq!(
        listed["distributions"],
        json!([]),
        "case after the bounds: a refused create stores nothing, got {listed:?}"
    );
}

/// A name that addresses no distribution is Lightsail's `DoesNotExist`
/// rejection, never an empty 200 — whatever its length, case or separators.
#[tokio::test(flavor = "current_thread")]
async fn a_read_of_a_name_that_names_nothing_is_refused() {
    let emulator = Emulator::new();
    create_origins(&emulator).await;
    emulator
        .ok(
            "CreateDistribution",
            json!({
                "distributionName": "cdn-present",
                "origin": {"name": ORIGIN_INSTANCE},
                "defaultCacheBehavior": {"behavior": "cache"},
                "bundleId": "small_1_0",
            }),
        )
        .await;

    // Names nothing created, differing along what the rule generalises over: the
    // length, the case, the digits and the separators. Every one satisfies the
    // modelled `ResourceName` pattern, so the walk passes it to the handler.
    let missing = [
        "ab",
        "never-created",
        "Probe_7",
        "cdn-present-but-longer",
        "a-name-with-many-hyphen-separated-words-and-digits-0123456789",
    ];
    for name in missing {
        let (status, body) = emulator
            .call("GetDistributions", json!({"distributionName": name}))
            .await;
        assert_eq!(
            status, 400,
            "case the missing name {name}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            NOT_FOUND,
            "case the missing name {name}: expected {NOT_FOUND}, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some(DOES_NOT_EXIST),
            "case the missing name {name}: expected the body code {DOES_NOT_EXIST}, got {body:?}"
        );
        assert!(
            body["message"].as_str().unwrap_or_default().contains(name),
            "case the missing name {name}: expected a message naming it, got {body:?}"
        );
    }

    // And the one distribution that does exist is still readable.
    let stored = read(&emulator, "cdn-present").await;
    assert_eq!(
        stored["name"].as_str(),
        Some("cdn-present"),
        "case after the missing names: the stored distribution still reads back, got {stored:?}"
    );
}

// ---------------------------------------------------------------------------
// The Region restriction
// ---------------------------------------------------------------------------

/// The refusal the documentation states verbatim, so its text is a requirement
/// and not an example: `smithy_query(query="doc",shape="InvalidInputException")`
/// reads "Domain and distribution APIs are only available in the N. Virginia
/// (us-east-1) Amazon Web Services Region. Please set your Amazon Web Services
/// Region configuration to us-east-1 to create, view, or edit these resources."
const REGION_REFUSAL: &str = "Domain and distribution APIs are only available in the N. Virginia \
                              (us-east-1) Amazon Web Services Region. Please set your Amazon Web \
                              Services Region configuration to us-east-1 to create, view, or \
                              edit these resources.";

/// An Amazon Web Services Region the modelled `RegionName` enum does NOT list,
/// so a case can sit outside the catalog: a scope naming a Region Lightsail does
/// not serve at all is no more the distribution Region than any other, so it is
/// refused too.
const OFF_ENUM_REGION: &str = "il-central-1";

/// Every Region a distribution call can be made in that is NOT the distribution
/// Region.
///
/// Taken from the crate's own `REGION_NAME_VALUES` — the modelled `RegionName`
/// enum's values — minus the distribution Region, so the table cannot drift from
/// the model as the catalog grows, plus [`OFF_ENUM_REGION`].
fn other_regions() -> Vec<&'static str> {
    assert!(
        REGION_NAME_VALUES.contains(&DISTRIBUTION_REGION),
        "case the Region catalog: expected the modelled RegionName enum to list the \
         distribution Region, got {REGION_NAME_VALUES:?}"
    );
    assert!(
        !REGION_NAME_VALUES.contains(&OFF_ENUM_REGION),
        "case the Region catalog: expected {OFF_ENUM_REGION} to sit OUTSIDE the modelled \
         RegionName enum, got {REGION_NAME_VALUES:?}"
    );
    let mut regions: Vec<&'static str> = REGION_NAME_VALUES
        .iter()
        .copied()
        .filter(|region| *region != DISTRIBUTION_REGION)
        .collect();
    regions.push(OFF_ENUM_REGION);
    regions
}

/// Every distribution operation refuses a request whose credential scope names a
/// Region other than the distribution Region — the create and the read alike,
/// since the documented restriction covers "create, view, or edit" — with the
/// modelled `InvalidInputException` (HTTP 400) carrying the documented text, and
/// changes nothing in the store.
#[tokio::test(flavor = "current_thread")]
async fn every_distribution_api_is_refused_outside_the_distribution_region() {
    let emulator = Emulator::new();
    create_origins(&emulator).await;
    emulator
        .ok(
            "CreateDistribution",
            json!({
                "distributionName": "cdn-east",
                "origin": {"name": ORIGIN_INSTANCE},
                "defaultCacheBehavior": {"behavior": "cache"},
                "bundleId": "small_1_0",
            }),
        )
        .await;
    let before = wait_for_status(&emulator, "cdn-east", STATUS_DEPLOYED).await;

    for region in other_regions() {
        // One row per distribution operation this change implements, each with a
        // request that WOULD be served in the distribution Region.
        let calls: &[(&str, &str, Value)] = &[
            (
                "creating a distribution",
                "CreateDistribution",
                json!({
                    "distributionName": "cdn-refused-region",
                    "origin": {"name": ORIGIN_INSTANCE, "regionName": DISTRIBUTION_REGION},
                    "defaultCacheBehavior": {"behavior": "cache"},
                    "bundleId": "small_1_0",
                }),
            ),
            (
                "reading one distribution",
                "GetDistributions",
                json!({"distributionName": "cdn-east"}),
            ),
            ("listing the distributions", "GetDistributions", json!({})),
        ];

        for (what, operation, body) in calls {
            let case = format!("{what} in {region}");
            let (status, answered) = emulator
                .call_in(Some(region), operation, body.clone())
                .await;
            assert_eq!(
                status, 400,
                "case {case}: expected a 400 refusal, got {status} {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                INVALID_INPUT,
                "case {case}: expected the modelled {INVALID_INPUT}, got {answered:?}"
            );
            assert_eq!(
                answered["message"].as_str(),
                Some(REGION_REFUSAL),
                "case {case}: expected the documented restriction, got {:?}",
                answered["message"]
            );
        }

        // The refusals changed nothing: the stored distribution is exactly as it
        // was, and the name the refused create named is still not a distribution
        // of this account.
        let after = read(&emulator, "cdn-east").await;
        assert_eq!(
            after, before,
            "case the store after the refusals in {region}: expected the stored distribution \
             to be untouched, got {after:?}"
        );
        let (status, answered) = emulator
            .call(
                "GetDistributions",
                json!({"distributionName": "cdn-refused-region"}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case the store after the refusals in {region}: a refused create stores nothing, \
             so reading it must be refused, got {status} {answered:?}"
        );
        assert_eq!(
            answered["code"].as_str(),
            Some(DOES_NOT_EXIST),
            "case the store after the refusals in {region}: a refused create stores nothing, \
             so reading it must answer code {DOES_NOT_EXIST}, got {answered:?}"
        );
    }

    // And both operations ARE served the two ways a request names the
    // distribution Region: the scope says so, or there is no scope to say
    // otherwise (the service's default Region IS the distribution Region).
    for (case, region) in [
        ("the us-east-1 credential scope", Some(DISTRIBUTION_REGION)),
        ("no credential scope at all", None),
    ] {
        let name = format!("cdn-served-{}", region.unwrap_or("unsigned"));
        let (status, created) = emulator
            .call_in(
                region,
                "CreateDistribution",
                json!({
                    "distributionName": name,
                    "origin": {"name": ORIGIN_INSTANCE},
                    "defaultCacheBehavior": {"behavior": "cache"},
                    "bundleId": "small_1_0",
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: CreateDistribution must be served, got {status} {created:?}"
        );
        let (status, read_back) = emulator
            .call_in(region, "GetDistributions", json!({"distributionName": name}))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: GetDistributions must be served, got {status} {read_back:?}"
        );
        assert_eq!(
            read_back["distributions"][0]["name"].as_str(),
            Some(name.as_str()),
            "case {case}: expected the created distribution, got {read_back:?}"
        );
    }
}
