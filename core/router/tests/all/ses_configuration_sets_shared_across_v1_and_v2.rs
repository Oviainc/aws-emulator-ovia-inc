// SPDX-License-Identifier: CC0-1.0

//! ONE set of configuration sets behind the account's email APIs.
//!
//! The rule under test: **a configuration set belongs to the ACCOUNT, not to the API that
//! recorded it.** Amazon SES publishes two APIs over one configuration-set collection — the
//! classic `awsQuery` surface (`CreateConfigurationSet`, `DescribeConfigurationSet`,
//! `ListConfigurationSets`, `DeleteConfigurationSet`, `CreateConfigurationSetTrackingOptions`,
//! `PutConfigurationSetDeliveryOptions`, `UpdateConfigurationSetSendingEnabled`,
//! `UpdateConfigurationSetReputationMetricsEnabled`) and the v2 REST surface
//! (`CreateConfigurationSet`, `GetConfigurationSet`, `ListConfigurationSets`,
//! `DeleteConfigurationSet`, the `PutConfigurationSet…Options` calls) — so a set recorded
//! through either one is described and listed by BOTH, a second create of its name on the
//! other surface is the naming conflict it is, an option written through one surface is
//! reported by the other in that API's own spelling of it, and a delete served on either
//! surface removes the set from both.
//!
//! The rows differ along what the rule generalises over: which surface RECORDED the set,
//! which surface CONFIGURED it and which option group it wrote (the tracking domain, the
//! delivery TLS policy, the sending switch, the reputation-metrics switch, or nothing at
//! all), which surface READ it back, and which surface DELETED it. Every expected value is
//! computed from what the row declares.
//!
//! This suite needs the two services over ONE engine, with the other members of the `ses`
//! signing-name family mounted beside them — the same assembly `core/router/src/lib.rs`
//! performs — because the hand-off being asserted exists only when one engine is shared, and
//! the claim that resolves each request to one service exists only when the peers are there
//! to compete for it.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{block_of, form_encode, members_of, opt_element};
use service_com_amazonaws_ses_simpleemailservice::state::IDENTITY_VERIFICATION_SPAN;

/// The Region every row signs for and addresses.
const REGION: &str = "us-east-1";

/// SES v1's API version, which an `awsQuery` body carries beside its `Action`
/// (`smithy_query(query="service_info")`: version 2010-12-01).
const SES_V1_API_VERSION: &str = "2010-12-01";

/// The domain this suite verifies before a row declares a tracking domain under it:
/// `CreateConfigurationSetTrackingOptions` refuses a domain that "is not verified in Amazon
/// SES", and a verified domain covers its subdomains.
const VERIFIED_DOMAIN: &str = "example.com";

/// Long enough for the identity's own clock-owed transition to come due:
/// `VerifyDomainIdentity` only ASKS for the verification, and the record stays pending for
/// `IDENTITY_VERIFICATION_SPAN` after it.
const PAST_THE_IDENTITY_SPAN: Duration =
    IDENTITY_VERIFICATION_SPAN.saturating_add(Duration::from_millis(250));

/// The two routing refusals the router answers itself, which no row here may provoke:
/// `AmbiguousService` (several services claimed one request) and `UnknownService` (none
/// did). Neither is in any AWS model.
const ROUTER_REFUSALS: &[&str] = &["AmbiguousService", "UnknownService"];

/// Which API a call is made on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surface {
    /// The classic `awsQuery` API.
    V1,
    /// The v2 REST API.
    V2,
}

impl Surface {
    /// The other one, which is the surface a row's hand-off is asserted through.
    fn other(self) -> Self {
        match self {
            Self::V1 => Self::V2,
            Self::V2 => Self::V1,
        }
    }
}

/// Which TLS policy a row writes on the set's delivery options.
///
/// The two APIs model the same two policies under DIFFERENT value sets — the classic
/// surface's `DeliveryOptions$TlsPolicy` carries `Require`/`Optional`
/// (`smithy_query(query="shape", shape="TlsPolicy")` of the SES v1 model), the v2 surface's
/// carries `REQUIRE`/`OPTIONAL` — so a row states the policy and each leg expects the
/// spelling its own API declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Policy {
    Require,
    Optional,
}

impl Policy {
    /// The classic API's spelling.
    fn v1(self) -> &'static str {
        match self {
            Self::Require => "Require",
            Self::Optional => "Optional",
        }
    }

    /// The v2 API's spelling.
    fn v2(self) -> &'static str {
        match self {
            Self::Require => "REQUIRE",
            Self::Optional => "OPTIONAL",
        }
    }

    /// The spelling `surface` declares.
    fn on(self, surface: Surface) -> &'static str {
        match surface {
            Surface::V1 => self.v1(),
            Surface::V2 => self.v2(),
        }
    }
}

/// The option group a row writes after the create, in the value the row writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Configured {
    /// The custom open-and-click redirect domain of the set's tracking options.
    TrackingDomain(&'static str),
    /// The TLS policy of the set's delivery options.
    TlsPolicy(Policy),
    /// Whether sending through the set is enabled.
    SendingEnabled(bool),
    /// Whether SES collects the set's reputation metrics.
    ReputationMetrics(bool),
}

/// One row: a configuration set, the surface that records it, the option one surface writes
/// on it, and the surface that deletes it.
struct Row {
    case: &'static str,
    /// The set's name, which both surfaces address it by.
    set: &'static str,
    /// The API that records it.
    created_on: Surface,
    /// The option a surface writes on it after the create, and which surface writes it, or
    /// `None` for a row that configures nothing at all.
    configured: Option<(Surface, Configured)>,
    /// The API that deletes it, which must remove it from both.
    deleted_on: Surface,
}

/// The rows, spanning both directions of the hand-off, every option group both models carry,
/// and a set that is never configured.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a set created with the classic API and never configured",
            set: "v1-plain",
            created_on: Surface::V1,
            configured: None,
            deleted_on: Surface::V2,
        },
        Row {
            case: "a set created with the v2 API and never configured",
            set: "v2_plain",
            created_on: Surface::V2,
            configured: None,
            deleted_on: Surface::V1,
        },
        Row {
            case: "a tracking domain written with the classic API",
            set: "v1-tracked",
            created_on: Surface::V1,
            configured: Some((Surface::V1, Configured::TrackingDomain("click.example.com"))),
            deleted_on: Surface::V2,
        },
        Row {
            case: "a tracking domain stated in the v2 create",
            set: "v2-tracked",
            created_on: Surface::V2,
            configured: Some((Surface::V2, Configured::TrackingDomain("open.example.com"))),
            deleted_on: Surface::V1,
        },
        Row {
            case: "a delivery TLS policy written with the classic API",
            set: "v1-tls-require",
            created_on: Surface::V1,
            configured: Some((Surface::V1, Configured::TlsPolicy(Policy::Require))),
            deleted_on: Surface::V2,
        },
        Row {
            case: "a delivery TLS policy written with the v2 API",
            set: "v2-tls-optional",
            created_on: Surface::V2,
            configured: Some((Surface::V2, Configured::TlsPolicy(Policy::Optional))),
            deleted_on: Surface::V1,
        },
        Row {
            case: "sending paused with the classic API on a set the v2 API created",
            set: "v2-paused-on-v1",
            created_on: Surface::V2,
            configured: Some((Surface::V1, Configured::SendingEnabled(false))),
            deleted_on: Surface::V2,
        },
        Row {
            case: "sending paused with the v2 API on a set the classic API created",
            set: "v1-paused-on-v2",
            created_on: Surface::V1,
            configured: Some((Surface::V2, Configured::SendingEnabled(false))),
            deleted_on: Surface::V1,
        },
        Row {
            case: "reputation metrics turned on with the classic API",
            set: "v1-reputation-on",
            created_on: Surface::V1,
            configured: Some((Surface::V1, Configured::ReputationMetrics(true))),
            deleted_on: Surface::V2,
        },
        Row {
            case: "reputation metrics turned off with the v2 API",
            set: "v2-reputation-off",
            created_on: Surface::V2,
            configured: Some((Surface::V2, Configured::ReputationMetrics(false))),
            deleted_on: Surface::V1,
        },
    ]
}

/// The services sharing the SigV4 signing name `ses`, over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These rows exercise routing and handler behaviour, not SigV4.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_ses_simpleemailservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_pinpointemail_amazonpinpointemailservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_mailmanager_mailmanagersvc::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `ses` — the signal the router
/// resolves a routing key from. The signature itself is never checked.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/ses/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// One answered request: its status and its body, as text and as JSON.
struct Answer {
    status: u16,
    text: String,
    json: Value,
}

/// Send one request, refusing at the door the two answers the ROUTER itself produces: a
/// request that reached nobody, or one several services claimed.
async fn send(
    router: &MultiServiceRouter,
    case: &str,
    leg: &str,
    request: http::Request<Bytes>,
) -> Answer {
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_default();
    let text = String::from_utf8_lossy(response.body()).to_string();
    for refusal in ROUTER_REFUSALS {
        assert!(
            !text.contains(refusal) && !error_type.contains(refusal),
            "case {case:?}: {leg} must be served by exactly one service, got HTTP {status}: \
             {text}"
        );
    }
    let json = serde_json::from_str(&text).unwrap_or(Value::Null);
    Answer { status, text, json }
}

/// One classic-API call: `awsQuery` under the `ses` credential scope, `POST /` with the
/// operation in the `Action` form field.
async fn v1(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    fields: &[(&str, String)],
) -> Answer {
    let mut body = format!("Action={action}&Version={SES_V1_API_VERSION}");
    for (name, value) in fields {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://email.{REGION}.amazonaws.com/"))
        .header("host", format!("email.{REGION}.amazonaws.com"))
        .header("authorization", authorization())
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("a well-formed request");
    send(router, case, &format!("SES v1 {action}"), request).await
}

/// [`v1`] where the call must succeed.
async fn v1_ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    fields: &[(&str, String)],
) -> String {
    let answered = v1(router, case, action, fields).await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: SES v1 {action} expected HTTP 200, got {}: {}",
        answered.status, answered.text
    );
    answered.text
}

/// One v2-API call: `restJson1` under the `ses` credential scope, in the `/v2/email/…` path
/// space.
async fn v2(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let payload = match &body {
        Some(value) => Bytes::from(value.to_string()),
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://email.{REGION}.amazonaws.com{path}"))
        .header("host", format!("email.{REGION}.amazonaws.com"))
        .header("authorization", authorization())
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");
    send(router, case, &format!("SESv2 {method} {path}"), request).await
}

/// [`v2`] where the call must succeed.
async fn v2_ok(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Value {
    let answered = v2(router, case, method, path, body).await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: SESv2 {method} {path} expected HTTP 200, got {}: {}",
        answered.status, answered.text
    );
    answered.json
}

/// The classic API's `DescribeConfigurationSet` for one set, with every attribute group this
/// suite reads asked for by the wire values of the model's own `ConfigurationSetAttribute`
/// enum.
async fn v1_describe(router: &MultiServiceRouter, case: &str, set: &str) -> Answer {
    v1(
        router,
        case,
        "DescribeConfigurationSet",
        &[
            ("ConfigurationSetName", set.to_string()),
            ("ConfigurationSetAttributeNames.member.1", "trackingOptions".to_string()),
            ("ConfigurationSetAttributeNames.member.2", "deliveryOptions".to_string()),
            (
                "ConfigurationSetAttributeNames.member.3",
                "reputationOptions".to_string(),
            ),
        ],
    )
    .await
}

/// Every configuration set the classic API lists, following its cursor to the end.
async fn v1_sets(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut fields: Vec<(&str, String)> = vec![("MaxItems", "2".to_string())];
        if let Some(token) = &token {
            fields.push(("NextToken", token.clone()));
        }
        let text = v1_ok(router, case, "ListConfigurationSets", &fields).await;
        let page = opt_element(&text, "ConfigurationSets").unwrap_or_default();
        seen.extend(
            members_of(&page)
                .into_iter()
                .filter_map(|member| opt_element(member, "Name")),
        );
        token = opt_element(&text, "NextToken");
        if token.is_none() {
            break;
        }
        assert!(
            seen.len() < 1_000,
            "case {case:?}: ListConfigurationSets did not terminate, saw {seen:?}"
        );
    }
    seen
}

/// Every configuration set the v2 API lists, on the method and URI every AWS SDK sends
/// `ListConfigurationSets` on (`GET /v2/email/configuration-sets`).
async fn v2_sets(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let listed = v2_ok(router, case, "GET", "/v2/email/configuration-sets", None).await;
    listed
        .get("ConfigurationSets")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|name| name.as_str().map(str::to_string))
        .collect()
}

/// The text of `member` inside the JSON object `document` holds at `group`, or `None`.
fn member_of(document: &Value, group: &str, member: &str) -> Option<String> {
    document
        .get(group)?
        .get(member)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The boolean `member` inside the JSON object `document` holds at `group`, or `None`.
fn flag_of(document: &Value, group: &str, member: &str) -> Option<bool> {
    document.get(group)?.get(member).and_then(Value::as_bool)
}

/// Record the set `row` declares, on the surface it declares it on. A v2 row that configures
/// its tracking domain states it in the create, because that is the call of that surface
/// which takes one.
async fn create(router: &MultiServiceRouter, row: &Row) {
    let tracking_in_the_create = match row.configured {
        Some((Surface::V2, Configured::TrackingDomain(domain))) => Some(domain),
        _ => None,
    };
    match row.created_on {
        Surface::V1 => {
            v1_ok(
                router,
                row.case,
                "CreateConfigurationSet",
                &[("ConfigurationSet.Name", row.set.to_string())],
            )
            .await;
        }
        Surface::V2 => {
            let mut body = json!({ "ConfigurationSetName": row.set });
            if let Some(domain) = tracking_in_the_create {
                body["TrackingOptions"] = json!({ "CustomRedirectDomain": domain });
            }
            v2_ok(
                router,
                row.case,
                "POST",
                "/v2/email/configuration-sets",
                Some(body),
            )
            .await;
        }
    }
}

/// Write the option the row declares, on the surface it declares it on. A v2 tracking domain
/// was already written by the create ([`create`]), so there is nothing left to do for it.
async fn configure(router: &MultiServiceRouter, row: &Row) {
    let Some((surface, configured)) = row.configured else {
        return;
    };
    let set = row.set.to_string();
    match (surface, configured) {
        (Surface::V1, Configured::TrackingDomain(domain)) => {
            v1_ok(
                router,
                row.case,
                "CreateConfigurationSetTrackingOptions",
                &[
                    ("ConfigurationSetName", set),
                    ("TrackingOptions.CustomRedirectDomain", domain.to_string()),
                ],
            )
            .await;
        }
        (Surface::V2, Configured::TrackingDomain(_)) => {}
        (Surface::V1, Configured::TlsPolicy(policy)) => {
            v1_ok(
                router,
                row.case,
                "PutConfigurationSetDeliveryOptions",
                &[
                    ("ConfigurationSetName", set),
                    ("DeliveryOptions.TlsPolicy", policy.v1().to_string()),
                ],
            )
            .await;
        }
        (Surface::V2, Configured::TlsPolicy(policy)) => {
            v2_ok(
                router,
                row.case,
                "PUT",
                &format!("/v2/email/configuration-sets/{set}/delivery-options"),
                Some(json!({ "TlsPolicy": policy.v2() })),
            )
            .await;
        }
        (Surface::V1, Configured::SendingEnabled(enabled)) => {
            v1_ok(
                router,
                row.case,
                "UpdateConfigurationSetSendingEnabled",
                &[
                    ("ConfigurationSetName", set),
                    ("Enabled", enabled.to_string()),
                ],
            )
            .await;
        }
        (Surface::V2, Configured::SendingEnabled(enabled)) => {
            v2_ok(
                router,
                row.case,
                "PUT",
                &format!("/v2/email/configuration-sets/{set}/sending"),
                Some(json!({ "SendingEnabled": enabled })),
            )
            .await;
        }
        (Surface::V1, Configured::ReputationMetrics(enabled)) => {
            v1_ok(
                router,
                row.case,
                "UpdateConfigurationSetReputationMetricsEnabled",
                &[
                    ("ConfigurationSetName", set),
                    ("Enabled", enabled.to_string()),
                ],
            )
            .await;
        }
        (Surface::V2, Configured::ReputationMetrics(enabled)) => {
            v2_ok(
                router,
                row.case,
                "PUT",
                &format!("/v2/email/configuration-sets/{set}/reputation-options"),
                Some(json!({ "ReputationMetricsEnabled": enabled })),
            )
            .await;
        }
    }
}

/// Delete the set on the surface the row declares.
async fn delete(router: &MultiServiceRouter, row: &Row) {
    match row.deleted_on {
        Surface::V1 => {
            v1_ok(
                router,
                row.case,
                "DeleteConfigurationSet",
                &[("ConfigurationSetName", row.set.to_string())],
            )
            .await;
        }
        Surface::V2 => {
            v2_ok(
                router,
                row.case,
                "DELETE",
                &format!("/v2/email/configuration-sets/{}", row.set),
                None,
            )
            .await;
        }
    }
}

/// What the row's option must read as on EACH surface, asserted in that API's own spelling.
async fn assert_the_option(router: &MultiServiceRouter, row: &Row) {
    let Some((_, configured)) = row.configured else {
        return;
    };
    let case = row.case;
    let described = v1_describe(router, case, row.set).await;
    let got = v2_ok(
        router,
        case,
        "GET",
        &format!("/v2/email/configuration-sets/{}", row.set),
        None,
    )
    .await;
    match configured {
        Configured::TrackingDomain(domain) => {
            let v1_domain = block_of(&described.text, "TrackingOptions")
                .and_then(|options| opt_element(options, "CustomRedirectDomain"));
            assert_eq!(
                v1_domain.as_deref(),
                Some(domain),
                "case {case:?}: the classic API must report the account's custom redirect \
                 domain, got {v1_domain:?} in {}",
                described.text
            );
            let v2_domain = member_of(&got, "TrackingOptions", "CustomRedirectDomain");
            assert_eq!(
                v2_domain.as_deref(),
                Some(domain),
                "case {case:?}: the v2 API must report the account's custom redirect domain, \
                 got {v2_domain:?} in {got}"
            );
        }
        Configured::TlsPolicy(policy) => {
            let v1_policy = block_of(&described.text, "DeliveryOptions")
                .and_then(|options| opt_element(options, "TlsPolicy"));
            assert_eq!(
                v1_policy.as_deref(),
                Some(policy.on(Surface::V1)),
                "case {case:?}: the classic API must report the account's TLS policy in its own \
                 value set, got {v1_policy:?} in {}",
                described.text
            );
            let v2_policy = member_of(&got, "DeliveryOptions", "TlsPolicy");
            assert_eq!(
                v2_policy.as_deref(),
                Some(policy.on(Surface::V2)),
                "case {case:?}: the v2 API must report the account's TLS policy in its own value \
                 set, got {v2_policy:?} in {got}"
            );
        }
        Configured::SendingEnabled(enabled) => {
            let v1_enabled = block_of(&described.text, "ReputationOptions")
                .and_then(|options| opt_element(options, "SendingEnabled"));
            assert_eq!(
                v1_enabled.as_deref(),
                Some(enabled.to_string().as_str()),
                "case {case:?}: the classic API must report the account's sending state, got \
                 {v1_enabled:?} in {}",
                described.text
            );
            let v2_enabled = flag_of(&got, "SendingOptions", "SendingEnabled");
            assert_eq!(
                v2_enabled,
                Some(enabled),
                "case {case:?}: the v2 API must report the account's sending state, got \
                 {v2_enabled:?} in {got}"
            );
        }
        Configured::ReputationMetrics(enabled) => {
            let v1_enabled = block_of(&described.text, "ReputationOptions")
                .and_then(|options| opt_element(options, "ReputationMetricsEnabled"));
            assert_eq!(
                v1_enabled.as_deref(),
                Some(enabled.to_string().as_str()),
                "case {case:?}: the classic API must report the account's reputation-metrics \
                 state, got {v1_enabled:?} in {}",
                described.text
            );
            let v2_enabled = flag_of(&got, "ReputationOptions", "ReputationMetricsEnabled");
            assert_eq!(
                v2_enabled,
                Some(enabled),
                "case {case:?}: the v2 API must report the account's reputation-metrics state, \
                 got {v2_enabled:?} in {got}"
            );
        }
    }
}

#[tokio::test]
async fn one_configuration_set_collection_behind_both_email_apis() {
    let router = router();

    // A verified domain for the row whose tracking domain the classic API writes: that call
    // refuses a domain this account has not verified. The verification is only ASKED for, so
    // the suite waits out the span the clock owes the record before relying on it.
    v1_ok(
        &router,
        "setup",
        "VerifyDomainIdentity",
        &[("Domain", VERIFIED_DOMAIN.to_string())],
    )
    .await;
    tokio::time::sleep(PAST_THE_IDENTITY_SPAN).await;

    let rows = rows();
    for row in &rows {
        let case = row.case;
        create(&router, row).await;

        // Both surfaces hold the set the moment the create answered: the one that recorded it
        // and the one that did not.
        let described = v1_describe(&router, case, row.set).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the classic API must describe the account's set, got {}: {}",
            described.status, described.text
        );
        let reported = block_of(&described.text, "ConfigurationSet")
            .and_then(|set| opt_element(set, "Name"));
        assert_eq!(
            reported.as_deref(),
            Some(row.set),
            "case {case:?}: the classic API must report the set under its own name, got \
             {reported:?} in {}",
            described.text
        );
        let got = v2(
            &router,
            case,
            "GET",
            &format!("/v2/email/configuration-sets/{}", row.set),
            None,
        )
        .await;
        assert_eq!(
            got.status, 200,
            "case {case:?}: the v2 API must report the account's set, got {}: {}",
            got.status, got.text
        );
        assert_eq!(
            got.json
                .get("ConfigurationSetName")
                .and_then(Value::as_str),
            Some(row.set),
            "case {case:?}: the v2 API must report the set under its own name, got {}",
            got.text
        );

        // And a create of that same name on the OTHER surface is the naming conflict it is,
        // because there is one set of this name and the account already holds it.
        let conflict = match row.created_on.other() {
            Surface::V1 => {
                let answered = v1(
                    &router,
                    case,
                    "CreateConfigurationSet",
                    &[("ConfigurationSet.Name", row.set.to_string())],
                )
                .await;
                (answered.status, answered.text)
            }
            Surface::V2 => {
                let answered = v2(
                    &router,
                    case,
                    "POST",
                    "/v2/email/configuration-sets",
                    Some(json!({ "ConfigurationSetName": row.set })),
                )
                .await;
                (answered.status, answered.text)
            }
        };
        assert_eq!(
            conflict.0, 400,
            "case {case:?}: a second create of the account's set must be refused, got {}: {}",
            conflict.0, conflict.1
        );
        assert!(
            conflict.1.contains("AlreadyExists"),
            "case {case:?}: a second create must be refused as a naming conflict, got {}",
            conflict.1
        );

        configure(&router, row).await;
        assert_the_option(&router, row).await;

        // Both listings report the set, whichever surface recorded it.
        for (surface, listed) in [
            ("the classic API", v1_sets(&router, case).await),
            ("the v2 API", v2_sets(&router, case).await),
        ] {
            assert!(
                listed.iter().any(|name| name == row.set),
                "case {case:?}: {surface} must list the account's set, got {listed:?}"
            );
        }
    }

    // Deleting on one surface takes the set from both — asserted after every row was created,
    // so a delete that took more than the set it names fails here too.
    for row in &rows {
        let case = row.case;
        delete(&router, row).await;

        let described = v1_describe(&router, case, row.set).await;
        assert_eq!(
            described.status, 400,
            "case {case:?}: the classic API must refuse a set the account no longer holds, got \
             {}: {}",
            described.status, described.text
        );
        assert!(
            described.text.contains("ConfigurationSetDoesNotExist"),
            "case {case:?}: the classic API must refuse it as a set that does not exist, got {}",
            described.text
        );
        let got = v2(
            &router,
            case,
            "GET",
            &format!("/v2/email/configuration-sets/{}", row.set),
            None,
        )
        .await;
        assert_eq!(
            got.status, 404,
            "case {case:?}: the v2 API must refuse a set the account no longer holds, got {}: {}",
            got.status, got.text
        );
        for (surface, listed) in [
            ("the classic API", v1_sets(&router, case).await),
            ("the v2 API", v2_sets(&router, case).await),
        ] {
            assert!(
                !listed.iter().any(|name| name == row.set),
                "case {case:?}: {surface} must not list a set the account no longer holds, got \
                 {listed:?}"
            );
        }
    }
}

/// One Pinpoint Email call: `restJson1` under the `ses` credential scope, in that API's own
/// `/v1/email/…` path space.
async fn pinpoint(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let payload = match &body {
        Some(value) => Bytes::from(value.to_string()),
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://email.{REGION}.amazonaws.com{path}"))
        .header("host", format!("email.{REGION}.amazonaws.com"))
        .header("authorization", authorization())
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");
    send(
        router,
        case,
        &format!("Pinpoint Email {method} {path}"),
        request,
    )
    .await
}

/// The THIRD API over the same collection: the Amazon Pinpoint Email API publishes the same
/// configuration sets, so a set recorded on any of the three is the account's set on all of
/// them — the rule this suite's table states for the two Amazon SES surfaces, applied to the
/// surface that shares their signing name and their resources.
///
/// Only its create and its read are asserted here, because they are the only
/// configuration-set operations of that API this emulator serves: its delete, its listing and
/// its four `PutConfigurationSet…Options` calls are still scaffolds with their own issues, so
/// there is nothing of them to hold to the rule yet.
#[tokio::test]
async fn the_pinpoint_email_api_answers_for_the_same_configuration_sets() {
    let router = router();

    // A set the classic API recorded is the account's, so the Pinpoint Email API reports it.
    let case = "a set the classic API recorded, read through the Pinpoint Email API";
    v1_ok(
        &router,
        case,
        "CreateConfigurationSet",
        &[("ConfigurationSet.Name", "classic-set".to_string())],
    )
    .await;
    let got = pinpoint(
        &router,
        case,
        "GET",
        "/v1/email/configuration-sets/classic-set",
        None,
    )
    .await;
    assert_eq!(
        got.status, 200,
        "case {case:?}: the Pinpoint Email API must report the account's set, got {}: {}",
        got.status, got.text
    );
    assert_eq!(
        got.json
            .get("ConfigurationSetName")
            .and_then(Value::as_str),
        Some("classic-set"),
        "case {case:?}: expected the set under its own name, got {}",
        got.text
    );

    // And one the Pinpoint Email API recorded is the account's too, so both Amazon SES
    // surfaces describe it and a second create of its name there is the naming conflict it is.
    let case = "a set the Pinpoint Email API recorded, read through both Amazon SES surfaces";
    let created = pinpoint(
        &router,
        case,
        "POST",
        "/v1/email/configuration-sets",
        Some(json!({ "ConfigurationSetName": "pinpoint-set" })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected the Pinpoint Email create to succeed, got {}: {}",
        created.status, created.text
    );
    let described = v1_describe(&router, case, "pinpoint-set").await;
    assert_eq!(
        described.status, 200,
        "case {case:?}: the classic API must describe the account's set, got {}: {}",
        described.status, described.text
    );
    let got = v2(
        &router,
        case,
        "GET",
        "/v2/email/configuration-sets/pinpoint-set",
        None,
    )
    .await;
    assert_eq!(
        got.status, 200,
        "case {case:?}: the v2 API must report the account's set, got {}: {}",
        got.status, got.text
    );
    let conflict = v2(
        &router,
        case,
        "POST",
        "/v2/email/configuration-sets",
        Some(json!({ "ConfigurationSetName": "pinpoint-set" })),
    )
    .await;
    assert_eq!(
        conflict.status, 400,
        "case {case:?}: a second create of the account's set must be refused, got {}: {}",
        conflict.status, conflict.text
    );

    // An option written on one of the SES surfaces is the account's configuration, so the
    // Pinpoint Email API reports it: sending paused on the v2 API is sending paused here.
    let case = "sending paused with the v2 API, read through the Pinpoint Email API";
    v2_ok(
        &router,
        case,
        "PUT",
        "/v2/email/configuration-sets/pinpoint-set/sending",
        Some(json!({ "SendingEnabled": false })),
    )
    .await;
    let got = pinpoint(
        &router,
        case,
        "GET",
        "/v1/email/configuration-sets/pinpoint-set",
        None,
    )
    .await;
    assert_eq!(
        got.json
            .get("SendingOptions")
            .and_then(|options| options.get("SendingEnabled"))
            .and_then(Value::as_bool),
        Some(false),
        "case {case:?}: expected the Pinpoint Email API to report the account's sending state, \
         got {}",
        got.text
    );
}
