// SPDX-License-Identifier: CC0-1.0

//! The setup an SES send-event-publication scenario performs, across the three services it
//! spans: an SQS queue, an SNS topic that queue is subscribed to, and the SESv2 sending
//! identity plus configuration set whose event destination names that topic.
//!
//! The rule under test: a request is served by the ONE emulated service whose endpoint and
//! wire form it addresses — resolved from the protocol, the endpoint prefix and the shape of
//! the request itself — and never from the SigV4 signing name alone. Four of the services
//! mounted below sign as `ses` (`smithy_query(query="service_info")` on each model): SES v1
//! (`awsQuery`, endpoint `email`, every operation a `POST /` naming its `Action`), SESv2
//! (`restJson1`, endpoint `email`, `/v2/email/…`), Pinpoint Email (`restJson1`, endpoint
//! `email`, `/v1/email/…`) and MailManager (`awsJson1_0` + `rpcv2Cbor`, endpoint
//! `mail-manager`, naming itself in an `X-Amz-Target` or an `rpcv2Cbor` URI). The router
//! resolves a request's routing key from the credential scope first, so all four see the SAME
//! key: a service that claims on that key alone claims every request in the family, every
//! such request has four claimants, and the router then refuses it `500 AmbiguousService` —
//! a code no AWS model carries, on a status an SDK classifies as retryable, so a client
//! spends its whole retry budget and the SESv2 surface answers nothing at all. Nothing an
//! event-publication scenario needs can then be set up: no verified sending identity, no
//! configuration set, no event destination.
//!
//! The sibling suite `ses_family_signing_name_routing.rs` tables that resolution across all
//! four wire forms. What this suite adds is the SETUP SEQUENCE itself, end to end over the
//! three services a published send event travels through, so the hand-off is asserted on the
//! values each service minted rather than on literals: the event destination names the topic
//! ARN SNS answered with, and the subscription names the queue ARN SQS answered with.
//!
//! The rows differ along what the rule generalises over: the identity form (apex domain,
//! subdomain, mixed-case domain, address), how many event types the destination matches (one,
//! a pair, every modelled type), and whether the destination is enabled — none of which may
//! change which service serves the request, or what the readers report back.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode, opt_element};

/// The Region every row signs for and addresses.
const REGION: &str = "us-east-1";

/// SNS's API version, which an `awsQuery` body carries beside its `Action`
/// (`smithy_query(query="service_info")`: version 2010-03-31).
const SNS_API_VERSION: &str = "2010-03-31";

/// Every value of the SESv2 `EventType` enum, in the model's own order
/// (`smithy_query(query="shape",shape="EventType")`), so the widest row matches the whole
/// published event surface rather than the two types a scenario happens to watch.
const EVERY_EVENT_TYPE: &[&str] = &[
    "SEND",
    "REJECT",
    "BOUNCE",
    "COMPLAINT",
    "DELIVERY",
    "OPEN",
    "CLICK",
    "RENDERING_FAILURE",
    "DELIVERY_DELAY",
    "SUBSCRIPTION",
];

/// Every value of the SESv2 `EventType` enum, ascending by name — the order a read of an
/// event destination reports its matching event types in, whatever order the definition
/// listed them.
const EVERY_EVENT_TYPE_ASCENDING: &[&str] = &[
    "BOUNCE",
    "CLICK",
    "COMPLAINT",
    "DELIVERY",
    "DELIVERY_DELAY",
    "OPEN",
    "REJECT",
    "RENDERING_FAILURE",
    "SEND",
    "SUBSCRIPTION",
];

/// The two routing refusals the router answers itself, which no row here may provoke:
/// `AmbiguousService` (several services claimed one request) and `UnknownService` (none did).
/// Neither is in any AWS model.
const ROUTER_REFUSALS: &[&str] = &["AmbiguousService", "UnknownService"];

/// One setup row: a sending identity, and the event destination its configuration set
/// carries.
struct Setup {
    /// What this row is, for the assertion messages.
    case: &'static str,
    /// The `EmailIdentity` to verify.
    identity: &'static str,
    /// The `IdentityType` the identity's own form implies: an identity with a local part is
    /// an address, anything else is a domain.
    identity_type: &'static str,
    /// How many Easy DKIM CNAME tokens SES publishes for it — three for a domain, none for
    /// an address, where DKIM signing does not apply.
    dkim_tokens: usize,
    /// `EventDestinationDefinition$MatchingEventTypes`, as the definition lists them.
    matching_event_types: &'static [&'static str],
    /// The same types as a read reports them: ascending by name, which is the order SES
    /// reports a destination's matching event types in. Written out per row rather than
    /// derived, so the expectation does not come from the behaviour under test.
    reported_event_types: &'static [&'static str],
    /// `EventDestinationDefinition$Enabled`.
    enabled: bool,
}

/// The rows, spanning the identity forms and the destination shapes the rule covers.
const SETUPS: &[Setup] = &[
    Setup {
        case: "an apex domain whose destination matches the send and delivery events",
        identity: "events-apex.example.com",
        identity_type: "DOMAIN",
        dkim_tokens: 3,
        matching_event_types: &["SEND", "DELIVERY"],
        reported_event_types: &["DELIVERY", "SEND"],
        enabled: true,
    },
    Setup {
        case: "a subdomain whose destination matches the bounce and complaint pair",
        identity: "mail.events-sub.example.org",
        identity_type: "DOMAIN",
        dkim_tokens: 3,
        matching_event_types: &["BOUNCE", "COMPLAINT"],
        reported_event_types: &["BOUNCE", "COMPLAINT"],
        enabled: true,
    },
    Setup {
        case: "a mixed-case domain whose destination matches every modelled event type",
        identity: "Events-Mixed.Example.NET",
        identity_type: "DOMAIN",
        dkim_tokens: 3,
        matching_event_types: EVERY_EVENT_TYPE,
        reported_event_types: EVERY_EVENT_TYPE_ASCENDING,
        enabled: true,
    },
    Setup {
        case: "an address identity whose destination matches one type and is disabled",
        identity: "sender@events-address.example.com",
        identity_type: "EMAIL_ADDRESS",
        dkim_tokens: 0,
        matching_event_types: &["RENDERING_FAILURE"],
        reported_event_types: &["RENDERING_FAILURE"],
        enabled: false,
    },
];

/// The six services this setup needs, over ONE shared engine, as `core/router/src/lib.rs`
/// mounts them: the three the sequence calls, plus the three that share SESv2's signing name
/// and whose claims are what the resolution has to tell apart. Without those three peers
/// mounted the collision does not exist, so a suite that left them out could not fail on it.
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
        service_com_amazonaws_mailmanager_mailmanagersvc::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name` — the signal
/// the router resolves a routing key from. The signature itself is never checked.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// The wire error code of an answer: `<Code>` for an XML body, `x-amzn-errortype` or
/// `__type` for a JSON one, and the empty string for a success. A namespaced or
/// URL-suffixed shape name is reduced to its local name, as the protocols allow either.
fn error_code(error_type: Option<&str>, body: &str) -> String {
    if body.trim_start().starts_with('<') {
        return opt_element(body, "Code").unwrap_or_default();
    }
    let raw = error_type
        .map(str::to_string)
        .or_else(|| {
            serde_json::from_str::<Value>(body)
                .ok()
                .and_then(|value| value.get("__type")?.as_str().map(str::to_string))
        })
        .unwrap_or_default();
    let local = raw.split('#').next_back().unwrap_or(&raw).to_string();
    local.split(':').next().unwrap_or(&local).to_string()
}

/// One answered request.
struct Answer {
    status: u16,
    /// The wire error code, or `""` on a success.
    code: String,
    /// The body as text, for the XML legs and for the assertion messages.
    text: String,
    /// The body parsed as JSON, or `Value::Null`.
    json: Value,
}

/// Send one request through the router, and refuse at the door the two answers the ROUTER
/// produces: a request that reached nobody, or that several services claimed. Every row
/// goes through here, so no leg of the sequence can pass on a routing refusal.
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
        .map(str::to_string);
    let text = String::from_utf8_lossy(response.body()).to_string();
    let code = error_code(error_type.as_deref(), &text);
    assert!(
        !ROUTER_REFUSALS.contains(&code.as_str()),
        "case {case:?}: {leg} must be served by exactly one service, got HTTP {status} \
         {code}: {text}"
    );
    let json = serde_json::from_str(&text).unwrap_or(Value::Null);
    Answer {
        status,
        code,
        text,
        json,
    }
}

/// One SESv2 call: `restJson1` under the `ses` credential scope, at the `email` endpoint,
/// in SESv2's own `/v2/email/…` path space.
async fn sesv2(
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
        .header("authorization", authorization("ses"))
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");
    send(router, case, &format!("SESv2 {method} {path}"), request).await
}

/// A SESv2 call that must be answered 200.
async fn sesv2_ok(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Value {
    let answered = sesv2(router, case, method, path, body).await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected HTTP 200 from SESv2 {method} {path}, got {} {}: {}",
        answered.status, answered.code, answered.text
    );
    answered.json
}

/// One SNS call: `awsQuery`, `POST /` with the `Action` in the form body.
async fn sns(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(&str, &str)],
) -> Answer {
    let mut body = format!("Action={action}&Version={SNS_API_VERSION}");
    for (name, value) in members {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://sns.{REGION}.amazonaws.com/"))
        .header("host", format!("sns.{REGION}.amazonaws.com"))
        .header("authorization", authorization("sns"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("a well-formed request");
    send(router, case, &format!("SNS {action}"), request).await
}

/// An SNS call that must be answered 200, as the XML document a query client reads.
async fn sns_ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let answered = sns(router, case, action, members).await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected HTTP 200 from SNS {action}, got {} {}: {}",
        answered.status, answered.code, answered.text
    );
    assert!(
        answered.text.trim_start().starts_with('<'),
        "case {case:?}: an awsQuery client must be answered an XML document from SNS \
         {action}, got {}",
        answered.text
    );
    answered.text
}

/// One SQS call: `awsJson1_0` with the operation in `X-Amz-Target`.
async fn sqs_ok(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://sqs.{REGION}.amazonaws.com/"))
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("authorization", authorization("sqs"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let answered = send(router, case, &format!("SQS {operation}"), request).await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {} {}: {}",
        answered.status, answered.code, answered.text
    );
    answered.json
}

/// The `DkimAttributes$Tokens` an answer reports, as the list a client reads.
fn dkim_tokens(body: &Value) -> Vec<String> {
    body.get("DkimAttributes")
        .and_then(|attributes| attributes.get("Tokens"))
        .and_then(Value::as_array)
        .map(|tokens| {
            tokens
                .iter()
                .filter_map(|token| token.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The queue created for one row, and the ARN a subscriber is addressed by.
async fn queue_for(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let created = sqs_ok(router, case, "CreateQueue", &json!({ "QueueName": name })).await;
    let queue_url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {created}"))
        .to_string();
    let attributes = sqs_ok(
        router,
        case,
        "GetQueueAttributes",
        &json!({ "QueueUrl": queue_url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let queue_arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueArn, got {attributes}"))
        .to_string();
    assert!(
        queue_arn.starts_with(&format!("arn:aws:sqs:{REGION}:"))
            && queue_arn.ends_with(&format!(":{name}")),
        "case {case:?}: the queue ARN must name this Region and the queue, got {queue_arn:?}"
    );
    queue_arn
}

#[tokio::test]
async fn the_setup_an_event_publication_scenario_performs_is_served_end_to_end() {
    let router = router();

    for (index, setup) in SETUPS.iter().enumerate() {
        let case = setup.case;
        let name = format!("ses-events-{index}");
        let configuration_set = format!("events-set-{index}");
        let destination_name = format!("sns-destination-{index}");

        // ---- The SQS queue the events are read out of, and the SNS topic it is
        // subscribed to: each leg addressed with the ARN the OTHER service minted.
        let queue_arn = queue_for(&router, case, &name).await;
        let topic = sns_ok(&router, case, "CreateTopic", &[("Name", &name)]).await;
        let topic_arn = element(&topic, "TopicArn");
        assert!(
            topic_arn.starts_with(&format!("arn:aws:sns:{REGION}:"))
                && topic_arn.ends_with(&format!(":{name}")),
            "case {case:?}: the topic ARN must name this Region and the topic, got \
             {topic_arn:?}"
        );
        let subscribed = sns_ok(
            &router,
            case,
            "Subscribe",
            &[
                ("TopicArn", &topic_arn),
                ("Protocol", "sqs"),
                ("Endpoint", &queue_arn),
                ("ReturnSubscriptionArn", "true"),
            ],
        )
        .await;
        let subscription_arn = element(&subscribed, "SubscriptionArn");
        assert!(
            subscription_arn.starts_with(&format!("{topic_arn}:")),
            "case {case:?}: the subscription must belong to the topic it was created on, \
             got {subscription_arn:?} for {topic_arn:?}"
        );

        // ---- The SESv2 sending identity: the leg every one of these scenarios begins
        // with, and the one the signing-name collision made unreachable.
        let created = sesv2_ok(
            &router,
            case,
            "POST",
            "/v2/email/identities",
            Some(json!({
                "EmailIdentity": setup.identity,
                "Tags": [{ "Key": "scenario", "Value": name }],
            })),
        )
        .await;
        assert_eq!(
            created["IdentityType"].as_str(),
            Some(setup.identity_type),
            "case {case:?}: CreateEmailIdentity must report the type the identity's own \
             form implies, got {created}"
        );
        let tokens = dkim_tokens(&created);
        assert_eq!(
            tokens.len(),
            setup.dkim_tokens,
            "case {case:?}: expected {} Easy DKIM token(s) to publish as CNAMEs, got \
             {tokens:?}",
            setup.dkim_tokens
        );
        assert_eq!(
            tokens.iter().collect::<BTreeSet<_>>().len(),
            setup.dkim_tokens,
            "case {case:?}: the Easy DKIM tokens must be a set of unique strings, got \
             {tokens:?}"
        );
        assert!(
            tokens.iter().all(|token| !token.is_empty()),
            "case {case:?}: no Easy DKIM token may be empty, got {tokens:?}"
        );

        // The identity read back through its own reader: the same type and the same token
        // set, so what a client publishes in DNS is what the service holds.
        let read_back = sesv2_ok(
            &router,
            case,
            "GET",
            &format!("/v2/email/identities/{}", setup.identity),
            None,
        )
        .await;
        assert_eq!(
            (
                read_back["IdentityType"].as_str(),
                dkim_tokens(&read_back)
            ),
            (Some(setup.identity_type), tokens.clone()),
            "case {case:?}: GetEmailIdentity must report what the create persisted, got \
             {read_back}"
        );

        // ---- The configuration set, and the event destination naming the topic SNS
        // minted above.
        sesv2_ok(
            &router,
            case,
            "POST",
            "/v2/email/configuration-sets",
            Some(json!({ "ConfigurationSetName": configuration_set })),
        )
        .await;
        sesv2_ok(
            &router,
            case,
            "POST",
            &format!("/v2/email/configuration-sets/{configuration_set}/event-destinations"),
            Some(json!({
                "EventDestinationName": destination_name,
                "EventDestination": {
                    "Enabled": setup.enabled,
                    "MatchingEventTypes": setup.matching_event_types,
                    "SnsDestination": { "TopicArn": topic_arn },
                },
            })),
        )
        .await;

        let destinations = sesv2_ok(
            &router,
            case,
            "GET",
            &format!("/v2/email/configuration-sets/{configuration_set}/event-destinations"),
            None,
        )
        .await;
        let listed = destinations["EventDestinations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected EventDestinations, got {destinations}"));
        let destination = listed
            .iter()
            .find(|entry| entry["Name"].as_str() == Some(destination_name.as_str()))
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected the destination {destination_name:?} in {destinations}")
            });
        let matched: Vec<String> = destination["MatchingEventTypes"]
            .as_array()
            .map(|types| {
                types
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            matched,
            setup
                .reported_event_types
                .iter()
                .map(|value| value.to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: the destination must match the event types it was created \
             with, got {destination}"
        );
        assert_eq!(
            destination["Enabled"].as_bool().unwrap_or(false),
            setup.enabled,
            "case {case:?}: the destination must report the enablement it was created \
             with, got {destination}"
        );
        assert_eq!(
            destination["SnsDestination"]["TopicArn"].as_str(),
            Some(topic_arn.as_str()),
            "case {case:?}: the destination must name the topic SNS minted, got \
             {destination}"
        );
    }
}

/// One refusal row: a setup call the owning service itself refuses.
struct Refusal {
    case: &'static str,
    method: &'static str,
    /// `None` for a path built from the row's own fixture below.
    path: &'static str,
    body: Option<fn() -> Value>,
    status: u16,
    code: &'static str,
    /// A substring the message must carry — what the caller has to change.
    names: &'static str,
}

#[tokio::test]
async fn a_setup_call_the_owning_service_refuses_answers_in_that_service_s_own_envelope() {
    // A request that resolves to one service is answered by THAT service, including when
    // the answer is a refusal: a modelled code on its modelled status, terminal, rather
    // than the router's own retryable refusal. The rows differ along which leg of the
    // setup is wrong and which modelled error the owning service states for it.
    let refusals: &[Refusal] = &[
        Refusal {
            case: "an event destination on a configuration set nothing created",
            method: "POST",
            path: "/v2/email/configuration-sets/events-absent-set/event-destinations",
            body: Some(|| {
                json!({
                    "EventDestinationName": "sns-destination",
                    "EventDestination": {
                        "Enabled": true,
                        "MatchingEventTypes": ["SEND"],
                        "SnsDestination": { "TopicArn": "arn:aws:sns:us-east-1:000000000000:absent" },
                    },
                })
            }),
            status: 404,
            code: "NotFoundException",
            names: "events-absent-set",
        },
        Refusal {
            case: "an event destination matching a type the model does not carry",
            method: "POST",
            path: "/v2/email/configuration-sets/events-refusals/event-destinations",
            body: Some(|| {
                json!({
                    "EventDestinationName": "sns-destination",
                    "EventDestination": {
                        "Enabled": true,
                        "MatchingEventTypes": ["DELIVERED"],
                        "SnsDestination": { "TopicArn": "arn:aws:sns:us-east-1:000000000000:events" },
                    },
                })
            }),
            status: 400,
            code: "BadRequestException",
            // The member whose value the model does not carry: a refusal for a value
            // outside a modelled value set names the member and the set, and
            // withholds the value.
            names: "eventDestination.matchingEventTypes",
        },
        Refusal {
            case: "a configuration set created twice",
            method: "POST",
            path: "/v2/email/configuration-sets",
            body: Some(|| json!({ "ConfigurationSetName": "events-refusals" })),
            status: 400,
            code: "AlreadyExistsException",
            names: "events-refusals",
        },
        Refusal {
            case: "an identity verified twice",
            method: "POST",
            path: "/v2/email/identities",
            body: Some(|| json!({ "EmailIdentity": "events-refusals.example.com" })),
            status: 400,
            code: "AlreadyExistsException",
            names: "events-refusals.example.com",
        },
        Refusal {
            case: "a read of an identity nothing verified",
            method: "GET",
            path: "/v2/email/identities/events-absent.example.com",
            body: None,
            status: 404,
            code: "NotFoundException",
            names: "events-absent.example.com",
        },
    ];

    let router = router();
    // The resources the duplicate rows collide with, created once here.
    sesv2_ok(
        &router,
        "refusal fixtures",
        "POST",
        "/v2/email/configuration-sets",
        Some(json!({ "ConfigurationSetName": "events-refusals" })),
    )
    .await;
    sesv2_ok(
        &router,
        "refusal fixtures",
        "POST",
        "/v2/email/identities",
        Some(json!({ "EmailIdentity": "events-refusals.example.com" })),
    )
    .await;

    for refusal in refusals {
        let case = refusal.case;
        let answered = sesv2(
            &router,
            case,
            refusal.method,
            refusal.path,
            refusal.body.map(|build| build()),
        )
        .await;
        assert_eq!(
            (answered.status, answered.code.as_str()),
            (refusal.status, refusal.code),
            "case {case:?}: expected SESv2's own HTTP {} {}, got {} {}: {}",
            refusal.status,
            refusal.code,
            answered.status,
            answered.code,
            answered.text
        );
        assert!(
            answered.text.contains(refusal.names),
            "case {case:?}: the refusal must name {:?}, got {}",
            refusal.names,
            answered.text
        );
    }

    // The SNS leg of the setup refuses in ITS envelope, which an awsQuery client is the
    // one that has to parse: a subscription on a topic nothing created.
    let answered = sns(
        &router,
        "a subscription on a topic nothing created",
        "Subscribe",
        &[
            (
                "TopicArn",
                &format!("arn:aws:sns:{REGION}:000000000000:events-absent-topic"),
            ),
            ("Protocol", "sqs"),
            (
                "Endpoint",
                &format!("arn:aws:sqs:{REGION}:000000000000:events-absent-queue"),
            ),
        ],
    )
    .await;
    assert_eq!(
        (answered.status, answered.code.as_str()),
        (404, "NotFound"),
        "a subscription on a topic nothing created must be refused by SNS itself, got {} \
         {}: {}",
        answered.status,
        answered.code,
        answered.text
    );
}
