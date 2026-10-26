// SPDX-License-Identifier: CC0-1.0

//! Updating the configuration of a Lightsail content delivery network (CDN)
//! distribution, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `UpdateDistribution` applies the configuration the
//! request carries to the ONE stored distribution it names — every member the
//! request carries replaces the stored value, every member it leaves out keeps
//! the value the distribution already has — rolls the change out again, and
//! answers one operation-log entry for the distribution it changed. A request the
//! two resources' rules do not allow is refused and changes nothing.

use std::collections::BTreeSet;

use serde_json::{json, Value};

// The harness, the wire error code, the minted-id check, the instance stand-up
// and the distribution/certificate reads this test target already exposes: a
// sibling suite drives the service through the same front door rather than
// standing up a second one.
use crate::disk_attachment::create_running_instance;
use crate::distribution_certificate_attachment::{
    create_certificate, distribution, wait_for_distribution_status, wait_for_issued_certificate,
};
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The `operationType` the log entry of an update carries, from the modelled
/// `OperationType` enum.
const OPERATION_TYPE: &str = "UpdateDistribution";

/// The `ResourceType` a distribution reports, from the modelled enum.
const RESOURCE_TYPE_DISTRIBUTION: &str = "Distribution";

/// The one Region a distribution lives in and its API is served from, from the
/// modelled `LightsailDistribution$location` documentation ("all distributions
/// are located in the `us-east-1` Region").
const DISTRIBUTION_REGION: &str = "us-east-1";

/// A Region that is NOT the one the distribution APIs are served from, for the
/// case that asserts the documented restriction.
const OTHER_REGION: &str = "us-west-2";

/// The Region the load balancer origin lives in, so a case can move the origin
/// to a resource outside the distribution's own Region.
const BALANCER_REGION: &str = "eu-west-1";

/// The `status` of a distribution whose configuration is still rolling out, and
/// the one it settles into. See `crate::state::DISTRIBUTION_STATUS_IN_PROGRESS`.
const DEPLOYED: &str = "Deployed";

/// The value Lightsail puts in a not-found body's `code` member, which a
/// Lightsail SDK client branches on rather than on the HTTP status.
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The `CertificateStatus` a certificate has to be in before a distribution may
/// serve HTTPS traffic with it, from the modelled `certificateName`
/// documentation ("Only certificates with a status of ISSUED can be attached to
/// a distribution").
const ISSUED: &str = "ISSUED";
/// The `CertificateStatus` a certificate is requested in.
const PENDING_VALIDATION: &str = "PENDING_VALIDATION";

/// The distribution every case updates.
const DISTRIBUTION: &str = "update-cdn";

/// The bundle the fixture distribution is created on, an id of the service's
/// published CDN plans (`GetDistributionBundles`).
const BUNDLE_ID: &str = "small_1_0";

/// The origin resources a case can move the distribution's origin to, one per
/// kind of resource an origin may be ("An origin can be a Lightsail instance,
/// bucket, container service, or load balancer").
const ORIGIN_INSTANCE: &str = "update-origin-instance";
const ORIGIN_OTHER_INSTANCE: &str = "update-origin-west";
const ORIGIN_BUCKET: &str = "update-origin-bucket";
const ORIGIN_BALANCER: &str = "update-origin-lb";

/// The certificate a case attaches, the domain it is for and its alternate
/// domain — so the attach publishes more than one domain.
const CERTIFICATE: &str = "update-cert";
const CERTIFICATE_DOMAIN: &str = "cdn.example.com";
const CERTIFICATE_ALTERNATIVE: &str = "www.cdn.example.com";

/// The published range `origin.responseTimeout` has to fall in, and the value a
/// request that names none gets, from the modelled `InputOrigin$responseTimeout`
/// documentation: "The minimum timeout is 1 second, the maximum is 60 seconds,
/// and the default (if you don't specify otherwise) is 30 seconds."
const MIN_RESPONSE_TIMEOUT: i64 = 1;
const MAX_RESPONSE_TIMEOUT: i64 = 60;
const DEFAULT_RESPONSE_TIMEOUT: i64 = 30;

/// The most per-path cache behaviours and custom error responses one
/// distribution holds (`crate::state::MAX_CACHE_BEHAVIORS_PER_DISTRIBUTION`,
/// `crate::state::MAX_CUSTOM_ERROR_RESPONSES_PER_DISTRIBUTION`).
const MAX_CACHE_BEHAVIORS: usize = 25;
const MAX_CUSTOM_ERROR_RESPONSES: usize = 25;

// ---------------------------------------------------------------------------
// The fixture
// ---------------------------------------------------------------------------

/// The resources every case updates against: the origins a case can point the
/// distribution at, and the `dnsName` the load balancer origin publishes — the
/// one origin a distribution reports an `originPublicDNS` for.
struct Fixture {
    balancer_dns: String,
}

/// Stand up the origins, the issued certificate and the distribution itself,
/// settled into [`DEPLOYED`] so each case starts from a stable record.
async fn stand_up(emulator: &Emulator) -> Fixture {
    create_running_instance(emulator, ORIGIN_INSTANCE, &format!("{DISTRIBUTION_REGION}a")).await;
    create_running_instance(
        emulator,
        ORIGIN_OTHER_INSTANCE,
        &format!("{OTHER_REGION}a"),
    )
    .await;
    emulator
        .ok(
            "CreateBucket",
            json!({"bucketName": ORIGIN_BUCKET, "bundleId": "small_1_0"}),
        )
        .await;
    emulator
        .ok_at(
            None,
            Some(BALANCER_REGION),
            "CreateLoadBalancer",
            json!({"loadBalancerName": ORIGIN_BALANCER, "instancePort": 80}),
        )
        .await;
    let balancer = emulator
        .ok_at(
            None,
            Some(BALANCER_REGION),
            "GetLoadBalancer",
            json!({"loadBalancerName": ORIGIN_BALANCER}),
        )
        .await;
    let balancer_dns = balancer["loadBalancer"]["dnsName"]
        .as_str()
        .unwrap_or_else(|| panic!("case standing up: expected a dnsName, got {balancer:?}"))
        .to_string();

    create_certificate(
        emulator,
        CERTIFICATE,
        CERTIFICATE_DOMAIN,
        &[CERTIFICATE_ALTERNATIVE],
    )
    .await;
    wait_for_issued_certificate(emulator, CERTIFICATE).await;

    emulator.ok("CreateDistribution", create_body()).await;
    wait_for_distribution_status(emulator, DISTRIBUTION, DEPLOYED).await;

    Fixture { balancer_dns }
}

/// The `CreateDistribution` body the fixture distribution is created with: every
/// member a later update can replace carries a value, so a case that replaces
/// one is replacing something.
fn create_body() -> Value {
    json!({
        "distributionName": DISTRIBUTION,
        "bundleId": BUNDLE_ID,
        "origin": {
            "name": ORIGIN_INSTANCE,
            "regionName": DISTRIBUTION_REGION,
            "protocolPolicy": "http-only",
            "responseTimeout": 15,
            "ipAddressType": "dualstack",
        },
        "defaultCacheBehavior": {"behavior": "cache"},
        "cacheBehaviors": [{"path": "/api/*", "behavior": "dont-cache"}],
        "customErrorResponses": [{
            "errorCode": 404,
            "responseCode": "404",
            "responsePagePath": "/404.html",
            "errorCachingMinTTL": 10,
        }],
        "ipAddressType": "dualstack",
        "tags": [{"key": "stack", "value": "cdn"}],
    })
}

// ---------------------------------------------------------------------------
// The update table
// ---------------------------------------------------------------------------

/// One `UpdateDistribution` request, and the members the rule says it replaces.
///
/// The rows differ along everything the rule generalises over: which member the
/// request carries, how many of them it carries at once, the value and the type
/// of each (a boolean, a string, a structure, a list, an empty list), the kind of
/// resource and the Region the origin moves to, whether a member that was set is
/// cleared, and a request that carries no configuration member at all.
struct Row {
    case: &'static str,
    /// The request body, minus `distributionName`.
    update: fn(&Fixture) -> Value,
    /// The members the update REPLACES. Every member the read-back carries that
    /// is not named here has to still report what it did before the call.
    expected: fn(&Fixture) -> Value,
}

/// The table. Applied in order to the one fixture distribution, so each row also
/// checks that the row before it left the rest of the record alone.
const ROWS: &[Row] = &[
    Row {
        case: "a boolean member: the distribution is disabled",
        update: |_| json!({"isEnabled": false}),
        expected: |_| json!({"isEnabled": false}),
    },
    Row {
        case: "a structure member set from nothing: the cache behaviour settings",
        update: |_| {
            json!({"cacheBehaviorSettings": {
                "defaultTTL": 86_400,
                "minimumTTL": 0,
                "maximumTTL": 31_536_000,
                "allowedHTTPMethods": "GET,HEAD,OPTIONS",
                "cachedHTTPMethods": "GET,HEAD",
                "forwardedCookies": {"option": "allow-list", "cookiesAllowList": ["session"]},
                "forwardedHeaders": {"option": "allow-list", "headersAllowList": ["Accept"]},
                "forwardedQueryStrings": {"option": true, "queryStringsAllowList": ["lang"]},
            }})
        },
        expected: |_| {
            json!({"cacheBehaviorSettings": {
                "defaultTTL": 86_400,
                "minimumTTL": 0,
                "maximumTTL": 31_536_000,
                "allowedHTTPMethods": "GET,HEAD,OPTIONS",
                "cachedHTTPMethods": "GET,HEAD",
                "forwardedCookies": {"option": "allow-list", "cookiesAllowList": ["session"]},
                "forwardedHeaders": {"option": "allow-list", "headersAllowList": ["Accept"]},
                "forwardedQueryStrings": {"option": true, "queryStringsAllowList": ["lang"]},
            }})
        },
    },
    Row {
        case: "two members at once: the default behaviour and the per-path behaviours",
        update: |_| {
            json!({
                "defaultCacheBehavior": {"behavior": "dont-cache"},
                "cacheBehaviors": [
                    {"path": "/static/*", "behavior": "cache"},
                    {"path": "/img/*", "behavior": "cache"},
                ],
            })
        },
        expected: |_| {
            json!({
                "defaultCacheBehavior": {"behavior": "dont-cache"},
                "cacheBehaviors": [
                    {"path": "/static/*", "behavior": "cache"},
                    {"path": "/img/*", "behavior": "cache"},
                ],
            })
        },
    },
    Row {
        case: "an empty list replaces the per-path behaviours with none",
        update: |_| json!({"cacheBehaviors": []}),
        expected: |_| json!({"cacheBehaviors": []}),
    },
    Row {
        case: "a list member: two custom error responses replace the one stored",
        update: |_| {
            json!({"customErrorResponses": [
                {
                    "errorCode": 403,
                    "responseCode": "200",
                    "responsePagePath": "/denied.html",
                    "errorCachingMinTTL": 60,
                },
                {
                    "errorCode": 500,
                    "responseCode": "503",
                    "responsePagePath": "/busy.html",
                    "errorCachingMinTTL": 5,
                },
            ]})
        },
        expected: |_| {
            json!({"customErrorResponses": [
                {
                    "errorCode": 403,
                    "responseCode": "200",
                    "responsePagePath": "/denied.html",
                    "errorCachingMinTTL": 60,
                },
                {
                    "errorCode": 500,
                    "responseCode": "503",
                    "responsePagePath": "/busy.html",
                    "errorCachingMinTTL": 5,
                },
            ]})
        },
    },
    Row {
        case: "two string members: the minimum viewer TLS version and the root object",
        update: |_| {
            json!({
                "viewerMinimumTlsProtocolVersion": "TLSv1.2_2021",
                "defaultRootObject": "index.html",
            })
        },
        expected: |_| {
            json!({
                "viewerMinimumTlsProtocolVersion": "TLSv1.2_2021",
                "defaultRootObject": "index.html",
            })
        },
    },
    Row {
        case: "the origin moves to an instance in another Region, at the published maximum timeout",
        update: |_| {
            json!({"origin": {
                "name": ORIGIN_OTHER_INSTANCE,
                "regionName": OTHER_REGION,
                "protocolPolicy": "https-only",
                "responseTimeout": MAX_RESPONSE_TIMEOUT,
            }})
        },
        // The whole origin is replaced, so the `ipAddressType` the create set is
        // gone rather than merged into the new one.
        expected: |_| {
            json!({"origin": {
                "name": ORIGIN_OTHER_INSTANCE,
                "resourceType": "Instance",
                "regionName": OTHER_REGION,
                "protocolPolicy": "https-only",
                "responseTimeout": MAX_RESPONSE_TIMEOUT,
                "isPrivateOriginAccessEnabled": false,
            }})
        },
    },
    Row {
        case: "the origin moves to a load balancer, which publishes a public DNS name",
        update: |_| {
            json!({"origin": {
                "name": ORIGIN_BALANCER,
                "regionName": BALANCER_REGION,
                "protocolPolicy": "http-only",
                "responseTimeout": MIN_RESPONSE_TIMEOUT,
                "ipAddressType": "ipv4",
            }})
        },
        expected: |fixture| {
            json!({
                "origin": {
                    "name": ORIGIN_BALANCER,
                    "resourceType": "LoadBalancer",
                    "regionName": BALANCER_REGION,
                    "protocolPolicy": "http-only",
                    "responseTimeout": MIN_RESPONSE_TIMEOUT,
                    "ipAddressType": "ipv4",
                    "isPrivateOriginAccessEnabled": false,
                },
                "originPublicDNS": fixture.balancer_dns,
            })
        },
    },
    Row {
        case: "the origin moves to a bucket with private origin access, naming no Region or timeout",
        update: |_| {
            json!({
                "origin": {"name": ORIGIN_BUCKET, "protocolPolicy": "https-only"},
                "enablePrivateOriginAccess": true,
            })
        },
        // The origin names no Region, so it is looked for in the distribution's
        // own, and no response timeout, so it waits the documented default. A
        // bucket publishes no public DNS name, so the load balancer's is gone.
        expected: |_| {
            json!({
                "origin": {
                    "name": ORIGIN_BUCKET,
                    "resourceType": "Bucket",
                    "regionName": DISTRIBUTION_REGION,
                    "protocolPolicy": "https-only",
                    "responseTimeout": DEFAULT_RESPONSE_TIMEOUT,
                    "isPrivateOriginAccessEnabled": true,
                },
                "originPublicDNS": Value::Null,
            })
        },
    },
    Row {
        case: "private origin access is turned off as the origin moves back to an instance",
        update: |_| {
            json!({
                "origin": {"name": ORIGIN_INSTANCE, "protocolPolicy": "http-only"},
                "enablePrivateOriginAccess": false,
            })
        },
        expected: |_| {
            json!({"origin": {
                "name": ORIGIN_INSTANCE,
                "resourceType": "Instance",
                "regionName": DISTRIBUTION_REGION,
                "protocolPolicy": "http-only",
                "responseTimeout": DEFAULT_RESPONSE_TIMEOUT,
                "isPrivateOriginAccessEnabled": false,
            }})
        },
    },
    Row {
        case: "a certificate of the account publishes the domains it covers",
        update: |_| json!({"certificateName": CERTIFICATE, "useDefaultCertificate": false}),
        expected: |_| {
            json!({
                "certificateName": CERTIFICATE,
                "alternativeDomainNames": [CERTIFICATE_DOMAIN, CERTIFICATE_ALTERNATIVE],
            })
        },
    },
    Row {
        case: "the default certificate drops the one attached and the domains it published",
        update: |_| json!({"useDefaultCertificate": true}),
        expected: |_| {
            json!({"certificateName": Value::Null, "alternativeDomainNames": []})
        },
    },
    Row {
        case: "the other value of a boolean member: the distribution is enabled again",
        update: |_| json!({"isEnabled": true}),
        expected: |_| json!({"isEnabled": true}),
    },
    Row {
        case: "a request that carries no configuration member changes nothing",
        update: |_| json!({}),
        expected: |_| json!({}),
    },
];

/// RULE: an update replaces exactly the members the request carries and leaves
/// every other member of the stored distribution alone, so the next read reports
/// the new configuration and the old values of everything the request said
/// nothing about — and the call answers one non-terminal operation-log entry for
/// the distribution, which settles into `Deployed`.
#[tokio::test]
async fn an_update_replaces_only_the_members_the_request_carries() {
    let emulator = Emulator::new();
    let fixture = stand_up(&emulator).await;

    for row in ROWS {
        let case = row.case;
        let before = distribution(&emulator, DISTRIBUTION).await;

        let mut body = (row.update)(&fixture);
        body["distributionName"] = json!(DISTRIBUTION);
        let answered = emulator.ok("UpdateDistribution", body.clone()).await;

        let operation = answered["operation"].clone();
        assert_eq!(
            operation["operationType"].as_str(),
            Some(OPERATION_TYPE),
            "case {case}: expected the {OPERATION_TYPE} operation type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceType"].as_str(),
            Some(RESOURCE_TYPE_DISTRIBUTION),
            "case {case}: expected the entry to be about the distribution, got {operation:?}"
        );
        assert_eq!(
            operation["resourceName"].as_str(),
            Some(DISTRIBUTION),
            "case {case}: expected the entry to name {DISTRIBUTION}, got {operation:?}"
        );
        assert_eq!(
            operation["status"].as_str(),
            Some("Started"),
            "case {case}: expected a Started operation, got {operation:?}"
        );
        // Changing a distribution's configuration is a rollout to the edge
        // locations, so the entry is still open when the call returns.
        assert_eq!(
            operation["isTerminal"].as_bool(),
            Some(false),
            "case {case}: expected a non-terminal operation, got {operation:?}"
        );
        assert_eq!(
            operation["location"]["regionName"].as_str(),
            Some(DISTRIBUTION_REGION),
            "case {case}: expected the entry located where the distribution is, got {operation:?}"
        );
        let id = operation["id"].as_str().unwrap_or_default();
        assert!(
            is_uuid_shaped(id),
            "case {case}: expected a uuid-shaped operation id, got {id:?}"
        );

        // The rollout the clock owes the distribution, then the record itself.
        let settled = wait_for_distribution_status(&emulator, DISTRIBUTION, DEPLOYED).await;
        let expected = (row.expected)(&fixture);
        let expected = expected
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: the expected members are an object"));
        let mut members: BTreeSet<String> = before
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: expected a distribution, got {before:?}"))
            .keys()
            .cloned()
            .collect();
        members.extend(expected.keys().cloned());
        for member in members {
            let want = expected
                .get(&member)
                .cloned()
                .unwrap_or_else(|| before[&member].clone());
            let got = settled[&member].clone();
            assert_eq!(
                got, want,
                "case {case}: expected {member} to be {want:?} after {body:?}, got {got:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The response timeout range
// ---------------------------------------------------------------------------

/// RULE: a distribution waits for its origin for the number of seconds the
/// request names, within the published range, and for the documented default
/// when the request names none.
#[tokio::test]
async fn the_published_response_timeout_range_is_served() {
    let emulator = Emulator::new();
    let _fixture = stand_up(&emulator).await;

    // Each published bound, the documented default, and a value between them.
    for sent in [
        Some(MIN_RESPONSE_TIMEOUT),
        Some(MAX_RESPONSE_TIMEOUT),
        Some(7),
        None,
    ] {
        let case = format!("origin.responseTimeout {sent:?}");
        let mut origin = json!({"name": ORIGIN_INSTANCE});
        if let Some(seconds) = sent {
            origin["responseTimeout"] = json!(seconds);
        }
        emulator
            .ok(
                "UpdateDistribution",
                json!({"distributionName": DISTRIBUTION, "origin": origin}),
            )
            .await;
        let reported = distribution(&emulator, DISTRIBUTION).await;
        let want = sent.unwrap_or(DEFAULT_RESPONSE_TIMEOUT);
        assert_eq!(
            reported["origin"]["responseTimeout"].as_i64(),
            Some(want),
            "case {case}: expected the distribution to wait {want} seconds, got {reported:?}"
        );
        wait_for_distribution_status(&emulator, DISTRIBUTION, DEPLOYED).await;
    }
}

// ---------------------------------------------------------------------------
// The refusal table
// ---------------------------------------------------------------------------

/// One `UpdateDistribution` request the rules do not allow, and how the service
/// refuses it.
struct Refusal {
    case: &'static str,
    /// The whole request body, including `distributionName`.
    body: fn() -> Value,
    /// The Region the request is signed for, or `None` for a request that
    /// carries no credential scope at all.
    region: Option<&'static str>,
    /// The wire error code (`__type`) the refusal carries.
    error: &'static str,
    /// Whether the body carries Lightsail's `DoesNotExist` code.
    does_not_exist: bool,
    /// A fragment the message has to name, so the caller can fix the request
    /// from the text alone.
    message_names: &'static str,
}

/// The table.
const REFUSALS: &[Refusal] = &[
    Refusal {
        case: "a distribution this account does not hold",
        body: || json!({"distributionName": "no-such-cdn", "isEnabled": false}),
        region: None,
        error: "NotFoundException",
        does_not_exist: true,
        message_names: "no-such-cdn",
    },
    Refusal {
        case: "an origin response timeout one second under the published minimum",
        body: || {
            json!({
                "distributionName": DISTRIBUTION,
                "origin": {
                    "name": ORIGIN_INSTANCE,
                    "responseTimeout": MIN_RESPONSE_TIMEOUT - 1,
                },
            })
        },
        region: None,
        error: "InvalidInputException",
        does_not_exist: false,
        message_names: "responseTimeout",
    },
    Refusal {
        case: "an origin response timeout one second over the published maximum",
        body: || {
            json!({
                "distributionName": DISTRIBUTION,
                "origin": {
                    "name": ORIGIN_INSTANCE,
                    "responseTimeout": MAX_RESPONSE_TIMEOUT + 1,
                },
            })
        },
        region: None,
        error: "InvalidInputException",
        does_not_exist: false,
        message_names: "responseTimeout",
    },
    Refusal {
        case: "an origin that names no resource",
        body: || {
            json!({
                "distributionName": DISTRIBUTION,
                "origin": {"protocolPolicy": "http-only"},
            })
        },
        region: None,
        error: "InvalidInputException",
        does_not_exist: false,
        message_names: "origin.name",
    },
    Refusal {
        case: "an origin name this account holds no resource under",
        body: || {
            json!({
                "distributionName": DISTRIBUTION,
                "origin": {"name": "no-such-origin"},
            })
        },
        region: None,
        error: "NotFoundException",
        does_not_exist: true,
        message_names: "no-such-origin",
    },
    Refusal {
        case: "a default cache behaviour that says nothing",
        body: || json!({"distributionName": DISTRIBUTION, "defaultCacheBehavior": {}}),
        region: None,
        error: "InvalidInputException",
        does_not_exist: false,
        message_names: "defaultCacheBehavior",
    },
    Refusal {
        case: "one more per-path cache behaviour than one distribution holds",
        body: || {
            let behaviors: Vec<Value> = (0..=MAX_CACHE_BEHAVIORS)
                .map(|index| json!({"path": format!("/p{index}/*"), "behavior": "cache"}))
                .collect();
            json!({"distributionName": DISTRIBUTION, "cacheBehaviors": behaviors})
        },
        region: None,
        error: "InvalidInputException",
        does_not_exist: false,
        message_names: "cacheBehaviors",
    },
    Refusal {
        case: "one more custom error response than one distribution holds",
        body: || {
            let responses: Vec<Value> = (0..=MAX_CUSTOM_ERROR_RESPONSES)
                .map(|index| {
                    json!({
                        "errorCode": 400 + index,
                        "responseCode": "200",
                        "responsePagePath": format!("/e{index}.html"),
                    })
                })
                .collect();
            json!({"distributionName": DISTRIBUTION, "customErrorResponses": responses})
        },
        region: None,
        error: "InvalidInputException",
        does_not_exist: false,
        message_names: "customErrorResponses",
    },
    Refusal {
        case: "private origin access without the origin it is about",
        body: || json!({"distributionName": DISTRIBUTION, "enablePrivateOriginAccess": true}),
        region: None,
        error: "InvalidInputException",
        does_not_exist: false,
        message_names: "enablePrivateOriginAccess",
    },
    Refusal {
        case: "private origin access on an origin that is not a bucket",
        body: || {
            json!({
                "distributionName": DISTRIBUTION,
                "origin": {"name": ORIGIN_INSTANCE},
                "enablePrivateOriginAccess": true,
            })
        },
        region: None,
        error: "InvalidInputException",
        does_not_exist: false,
        message_names: "enablePrivateOriginAccess",
    },
    Refusal {
        case: "both a certificate of the account and the default certificate",
        body: || {
            json!({
                "distributionName": DISTRIBUTION,
                "certificateName": CERTIFICATE,
                "useDefaultCertificate": true,
            })
        },
        region: None,
        error: "InvalidInputException",
        does_not_exist: false,
        message_names: "useDefaultCertificate",
    },
    Refusal {
        case: "a certificate this account does not hold",
        body: || {
            json!({"distributionName": DISTRIBUTION, "certificateName": "no-such-cert"})
        },
        region: None,
        error: "NotFoundException",
        does_not_exist: true,
        message_names: "no-such-cert",
    },
    Refusal {
        case: "a request made outside the one Region the distribution APIs are served from",
        body: || json!({"distributionName": DISTRIBUTION, "isEnabled": false}),
        region: Some(OTHER_REGION),
        error: "InvalidInputException",
        does_not_exist: false,
        message_names: DISTRIBUTION_REGION,
    },
];

/// RULE: a request the service's own rules do not allow is refused with the
/// modelled failure for that rule and changes nothing — every member of the
/// stored distribution still reports what it did before the call.
#[tokio::test]
async fn an_update_the_rules_do_not_allow_changes_nothing() {
    let emulator = Emulator::new();
    let _fixture = stand_up(&emulator).await;

    for refusal in REFUSALS {
        let case = refusal.case;
        let before = distribution(&emulator, DISTRIBUTION).await;
        let (status, body) = emulator
            .call_in(refusal.region, "UpdateDistribution", (refusal.body)())
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, refusal.error,
            "case {case}: expected {}, got {code} {body:?}",
            refusal.error
        );
        if refusal.does_not_exist {
            assert_eq!(
                body["code"].as_str(),
                Some(DOES_NOT_EXIST),
                "case {case}: expected the {DOES_NOT_EXIST} code, got {body:?}"
            );
        }
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| message.contains(refusal.message_names)),
            "case {case}: expected a message naming {}, got {body:?}",
            refusal.message_names
        );

        let after = distribution(&emulator, DISTRIBUTION).await;
        assert_eq!(
            after, before,
            "case {case}: expected the refused update to change nothing, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A certificate the authority has not issued yet
// ---------------------------------------------------------------------------

/// RULE: only an ISSUED certificate is one a distribution serves HTTPS traffic
/// with, so an update naming a certificate the authority has not issued yet is
/// refused with the modelled `OperationFailureException` and changes nothing.
///
/// The pending window is the issuance the clock owes a fresh certificate, so the
/// case does not ASSUME an update lands inside it: each attempt requests a fresh
/// certificate (whose create answer is asserted pending), and an attempt the
/// clock beat is retried rather than counted.
#[tokio::test]
async fn an_update_naming_a_certificate_the_authority_has_not_issued_is_refused() {
    /// How many attempts may be beaten by the clock before the case fails. Each
    /// attempt is a handful of in-process calls against a sub-second window, so
    /// one is normally enough.
    const ATTEMPTS: usize = 10;

    let emulator = Emulator::new();
    let _fixture = stand_up(&emulator).await;

    let mut observed = false;
    for attempt in 0..ATTEMPTS {
        let case = format!("attempt {attempt}");
        let certificate_name = format!("pending-update-cert-{attempt}");
        let created = create_certificate(
            &emulator,
            &certificate_name,
            "pending.example.com",
            &["www.pending.example.com"],
        )
        .await;
        // The create's own answer, so no clock has run yet.
        assert_eq!(
            created["certificateDetail"]["status"].as_str(),
            Some(PENDING_VALIDATION),
            "case {case}: expected a freshly requested certificate to be {PENDING_VALIDATION}, \
             got {created:?}"
        );
        let before = distribution(&emulator, DISTRIBUTION).await;
        let (status, body) = emulator
            .call(
                "UpdateDistribution",
                json!({
                    "distributionName": DISTRIBUTION,
                    "certificateName": certificate_name,
                    "useDefaultCertificate": false,
                }),
            )
            .await;
        if status == 200 {
            // The clock issued this certificate between the two calls, so this
            // attempt says nothing about the pending window: try again with a
            // fresh certificate.
            wait_for_distribution_status(&emulator, DISTRIBUTION, DEPLOYED).await;
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "OperationFailureException",
            "case {case}: expected OperationFailureException, got {code} {body:?}"
        );
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| message.contains(ISSUED)
                    && message.contains(&certificate_name)),
            "case {case}: expected a message naming the certificate and the {ISSUED} \
             requirement, got {body:?}"
        );
        let after = distribution(&emulator, DISTRIBUTION).await;
        assert_eq!(
            after, before,
            "case {case}: expected the refused update to change nothing, got {after:?}"
        );
        observed = true;
        break;
    }
    assert!(
        observed,
        "case the pending window: expected one of {ATTEMPTS} attempts to name a certificate the \
         authority had not issued yet, and every one of them was issued first"
    );
}
