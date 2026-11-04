// SPDX-License-Identifier: CC0-1.0

//! The SESv2 and Amazon SNS setup an event-publication scenario performs, driven through the
//! WHOLE mounted emulator: an SNS topic, the topic access policy that permits the SES service
//! principal to publish to it, a SESv2 configuration set, and the SNS event destination that
//! set carries.
//!
//! The RULE under test: a request signed with a SigV4 signing name that SEVERAL emulated
//! services share is served by the ONE service whose own endpoint prefix and wire form it
//! addresses — for a REST service, the URI path space its model binds — and never by the
//! signing name alone, so what the caller reads is that service's own modelled answer.
//!
//! Four mounted crates sign as `ses` (`smithy_query(query="service_info")` on each model): SES
//! v1 (`awsQuery`, endpoint `email`, every operation a `POST /` naming its `Action`), SESv2
//! (`restJson1`, endpoint `email`, `/v2/email/…`), Pinpoint Email (`restJson1`, endpoint
//! `email`, `/v1/email/…`) and MailManager (`awsJson1_0` + `rpcv2Cbor`, endpoint
//! `mail-manager`). The router resolves a request's routing key from the SigV4 credential scope
//! first, so all four see the SAME key: a service claiming on that key alone claims every
//! request in the family, every such request then has four claimants, and the router refuses it
//! `500 AmbiguousService` — a code no AWS model carries, on a status an SDK classifies as
//! retryable, so a client burns its whole retry budget and surfaces no answer. With
//! `CreateConfigurationSet` answered that way nothing an event-publication scenario needs can be
//! configured at all: no configuration set, and so no event destination to hang off it.
//!
//! What this suite adds to its siblings is the pair of things only it reaches. The ambiguity
//! exists only with every service mounted, so the router is the one `core/router/src/lib.rs`
//! assembles for the binary — the service set the reported failure arose in — rather than the
//! handful of crates a sibling suite mounts. And the sequence it drives is the SNS-side one: the
//! topic's own access policy, which is what decides whether a published event can reach the
//! topic at all, is set, read back, and read again after the SESv2 legs.
//!
//! The rows differ along what the rule generalises over: which routing signal addresses the
//! SESv2 legs (a SigV4 credential scope, or the endpoint host with no credential scope at all),
//! the SHAPE of the policy document granting the publish (one statement or a list, the principal
//! and the action as a bare string or a list, the action's letter case, a grant scoped by
//! condition, a grant written beside a statement about another action), the configuration set's
//! own options, and how many event types the destination matches (one, a pair, every modelled
//! type) and whether it is enabled.
//!
//! What this suite does NOT assert is the publication itself: no email is sent here and no event
//! is published, so nothing below says anything about when one would be. What it pins is the
//! CONFIGURATION the two services hold afterwards — the grant SNS reports for the topic, and the
//! destination SESv2 reports for the set, each naming what the other minted — which is the
//! sequence that could not begin at all while the router refused it.

use std::collections::BTreeMap;
use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{decoded_entries, element, form_encode, opt_element};
use crate::ses_family_signing_name_routing::{
    authorization, email_host, BOOT_STACK_SIZE, REGION, ROUTER_REFUSALS,
};

/// SNS's API version, which an `awsQuery` body carries beside its `Action`
/// (`smithy_query(query="service_info")`: version 2010-03-31).
const SNS_API_VERSION: &str = "2010-03-31";

/// The service principal Amazon SES publishes an event as, which is what a topic policy has to
/// permit for a published event to reach the topic.
const SES_SERVICE_PRINCIPAL: &str = "ses.amazonaws.com";

/// The action publishing to a topic performs on it, spelled the way SNS names its own actions in
/// a topic policy.
const PUBLISH_ACTION: &str = "SNS:Publish";

/// The topic attribute carrying the access policy document.
const POLICY_ATTRIBUTE: &str = "Policy";

/// Every value of the SESv2 `EventType` enum, in the model's own order
/// (`smithy_query(query="shape",shape="EventType")`), so the widest row matches the whole
/// published event surface rather than the types one scenario happens to watch.
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

/// What a row's policy document is built from: the topic the grant is about, and the account of
/// the ARN SNS minted for it — read off that ARN rather than written out, so a statement scoped
/// by condition names the account this emulator really answers as.
struct TopicFacts {
    arn: String,
    account: String,
}

/// One setup row: how its SESv2 legs are addressed, the topic policy it grants the publish
/// with, and the configuration set and event destination it builds.
struct Setup {
    /// What this row is, for the assertion messages.
    case: &'static str,
    /// Whether the SESv2 legs carry a SigV4 credential scope naming the shared signing name, or
    /// address the SES endpoint host with no credential scope at all. Both are signals the rule
    /// resolves a single claimant from, and neither may resolve to a peer of this family.
    signed: bool,
    /// The access policy this row sets on the topic.
    policy: fn(&TopicFacts) -> Value,
    /// `TrackingOptions$CustomRedirectDomain` the configuration set is created with.
    custom_redirect_domain: &'static str,
    /// `SendingOptions$SendingEnabled` it is created with.
    sending_enabled: bool,
    /// `EventDestinationDefinition$MatchingEventTypes`, as the definition lists them.
    matching_event_types: &'static [&'static str],
    /// The same types as a read reports them: ascending by name, which is the order SES
    /// reports a destination's matching event types in. Written out per row rather than
    /// derived, so the expectation does not come from the behaviour under test.
    reported_event_types: &'static [&'static str],
    /// `EventDestinationDefinition$Enabled`.
    enabled: bool,
}

/// The rows, spanning the routing signals and the policy-document shapes the rule covers.
const SETUPS: &[Setup] = &[
    Setup {
        case: "the grant for an SNS event destination, signed with the shared signing name",
        signed: true,
        policy: |topic| {
            document(vec![json!({
                "Sid": "AllowSESEventPublishing",
                "Effect": "Allow",
                "Principal": { "Service": SES_SERVICE_PRINCIPAL },
                "Action": PUBLISH_ACTION,
                "Resource": topic.arn,
            })])
        },
        custom_redirect_domain: "clicks.events-apex.example.com",
        sending_enabled: true,
        matching_event_types: &["SEND", "DELIVERY"],
        reported_event_types: &["DELIVERY", "SEND"],
        enabled: true,
    },
    Setup {
        case: "the same grant with its principal and action as lists, addressed by host alone",
        signed: false,
        policy: |topic| {
            document(vec![json!({
                "Effect": "Allow",
                "Principal": { "Service": [SES_SERVICE_PRINCIPAL] },
                "Action": [PUBLISH_ACTION],
                "Resource": [topic.arn],
            })])
        },
        custom_redirect_domain: "clicks.mail.events-sub.example.org",
        sending_enabled: false,
        matching_event_types: &["BOUNCE", "COMPLAINT"],
        reported_event_types: &["BOUNCE", "COMPLAINT"],
        enabled: true,
    },
    Setup {
        case: "the grant scoped by condition to this account's own sending",
        signed: true,
        policy: |topic| {
            document(vec![json!({
                "Effect": "Allow",
                "Principal": { "Service": SES_SERVICE_PRINCIPAL },
                "Action": PUBLISH_ACTION,
                "Resource": topic.arn,
                "Condition": { "StringEquals": { "AWS:SourceAccount": topic.account } },
            })])
        },
        custom_redirect_domain: "links.events-conditioned.example.net",
        sending_enabled: true,
        matching_event_types: EVERY_EVENT_TYPE,
        reported_event_types: EVERY_EVENT_TYPE_ASCENDING,
        enabled: true,
    },
    Setup {
        case: "a document whose Statement is one object, with the action in lower case",
        signed: false,
        policy: |topic| {
            json!({
                "Version": "2012-10-17",
                "Statement": {
                    "Effect": "Allow",
                    "Principal": { "Service": SES_SERVICE_PRINCIPAL },
                    "Action": "sns:Publish",
                    "Resource": topic.arn,
                },
            })
        },
        custom_redirect_domain: "opens.events-single.example.com",
        sending_enabled: true,
        matching_event_types: &["OPEN"],
        reported_event_types: &["OPEN"],
        enabled: false,
    },
    Setup {
        case: "the grant beside a statement about another action",
        signed: true,
        policy: |topic| {
            document(vec![
                json!({
                    "Effect": "Allow",
                    "Principal": { "AWS": topic.account },
                    "Action": "SNS:Subscribe",
                    "Resource": topic.arn,
                }),
                json!({
                    "Effect": "Allow",
                    "Principal": { "Service": SES_SERVICE_PRINCIPAL },
                    "Action": [PUBLISH_ACTION, "SNS:GetTopicAttributes"],
                    "Resource": topic.arn,
                }),
            ])
        },
        custom_redirect_domain: "clicks.events-paired.example.org",
        sending_enabled: false,
        matching_event_types: &["DELIVERY_DELAY", "SUBSCRIPTION"],
        reported_event_types: &["DELIVERY_DELAY", "SUBSCRIPTION"],
        enabled: true,
    },
];

/// A policy document around `statements`, in the form the policy grammar's list spelling takes.
fn document(statements: Vec<Value>) -> Value {
    json!({ "Version": "2012-10-17", "Statement": statements })
}

/// A policy document's statements: one object, or a list of them — both of which the policy
/// grammar admits and SNS stores.
fn statements(policy: &Value) -> Vec<Value> {
    match policy.get("Statement") {
        Some(Value::Array(many)) => many.clone(),
        Some(one) => vec![one.clone()],
        None => Vec::new(),
    }
}

/// One policy element's values: a bare string, or a list of them.
fn values_of(member: Option<&Value>) -> Vec<String> {
    match member {
        Some(Value::String(one)) => vec![one.clone()],
        Some(Value::Array(many)) => many
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// Whether `policy` — the document a topic's `Policy` attribute carries — ALLOWS the SES service
/// principal to publish to `topic_arn`: a statement whose `Effect` is `Allow`, whose
/// `Principal.Service` names that principal, whose `Action` names the publish action (an action
/// name is compared without regard to case) and whose `Resource` names this topic or every
/// resource.
///
/// A reader of the grant, not an authorization engine: deciding a request against a document is
/// the service's own job, and every condition these rows write holds for the account and topic
/// the row itself created. It is here to tell a document that really permits the publish from
/// one that only mentions the topic — the difference the setup under test turns on.
fn permits_ses_publish(policy: &Value, topic_arn: &str) -> bool {
    statements(policy).iter().any(|statement| {
        statement.get("Effect").and_then(Value::as_str) == Some("Allow")
            && values_of(statement.get("Principal").and_then(|principal| principal.get("Service")))
                .iter()
                .any(|principal| principal == SES_SERVICE_PRINCIPAL)
            && values_of(statement.get("Action"))
                .iter()
                .any(|action| action.eq_ignore_ascii_case(PUBLISH_ACTION))
            && values_of(statement.get("Resource"))
                .iter()
                .any(|resource| resource == topic_arn || resource == "*")
    })
}

/// One answered request.
struct Answer {
    status: u16,
    /// The wire error code, or `""` on a success.
    code: String,
    /// The body as text, for the XML legs and for the assertion messages.
    text: String,
}

/// The wire error code of an answer: `<Code>` for an XML body, the `x-amzn-errortype` header or
/// `__type` for a JSON one, and the empty string for a success. A namespaced or URL-suffixed
/// shape name is reduced to its local name, as the protocols allow either.
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

/// Send one request through the router, and refuse at the door the answers the ROUTER itself
/// produces: a request several services claimed, or one that reached nobody. Every leg of every
/// row goes through here, so no step of the sequence can pass on a routing refusal.
fn send(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    leg: &str,
    request: http::Request<bytes::Bytes>,
) -> Answer {
    let response = runtime.block_on(router.dispatch(request));
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
    Answer {
        status,
        code,
        text,
    }
}

/// One SESv2 call: `restJson1` at the `email` endpoint, in SESv2's own `/v2/email/…` path space,
/// addressed either with a credential scope naming the shared signing name or by host alone.
fn sesv2(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    setup: &Setup,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Value {
    let payload = match body {
        Some(value) => bytes::Bytes::from(value.to_string()),
        None => bytes::Bytes::new(),
    };
    let host = email_host();
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", &host)
        .header("content-type", "application/json");
    if setup.signed {
        builder = builder.header("authorization", authorization("ses"));
    }
    let request = builder.body(payload).expect("a well-formed request");
    let leg = format!("SESv2 {method} {path}");
    let answered = send(router, runtime, setup.case, &leg, request);
    assert_eq!(
        answered.status, 200,
        "case {:?}: expected HTTP 200 from {leg}, got {} {}: {}",
        setup.case, answered.status, answered.code, answered.text
    );
    if answered.text.is_empty() {
        return Value::Null;
    }
    serde_json::from_str(&answered.text).unwrap_or_else(|error| {
        panic!(
            "case {:?}: {leg} must answer a JSON document, got {} ({error})",
            setup.case, answered.text
        )
    })
}

/// One SNS call: `awsQuery`, `POST /` with the `Action` in the form body, answered as the XML
/// document a query client reads.
fn sns(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let mut body = format!("Action={action}&Version={SNS_API_VERSION}");
    for (name, value) in members {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let host = format!("sns.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", &host)
        .header("authorization", authorization("sns"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let leg = format!("SNS {action}");
    let answered = send(router, runtime, case, &leg, request);
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected HTTP 200 from {leg}, got {} {}: {}",
        answered.status, answered.code, answered.text
    );
    assert!(
        answered.text.trim_start().starts_with('<'),
        "case {case:?}: an awsQuery client must be answered an XML document from {leg}, got {}",
        answered.text
    );
    answered.text
}

/// The `Policy` attribute the topic reports, as the document a client reads it as.
fn reported_policy(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    topic_arn: &str,
) -> (String, Value) {
    let read = sns(
        router,
        runtime,
        case,
        "GetTopicAttributes",
        &[("TopicArn", topic_arn)],
    );
    let attributes: BTreeMap<String, String> = decoded_entries(&read);
    let policy = attributes
        .get(POLICY_ATTRIBUTE)
        .unwrap_or_else(|| {
            panic!("case {case:?}: a topic must report a {POLICY_ATTRIBUTE} attribute, got {read}")
        })
        .clone();
    let parsed = serde_json::from_str(&policy).unwrap_or_else(|error| {
        panic!("case {case:?}: the reported {POLICY_ATTRIBUTE} must be a JSON document, got {policy} ({error})")
    });
    (policy, parsed)
}

/// The event destinations the configuration set reports, as the list a client reads.
fn reported_destinations(reported: &Value, case: &str) -> Vec<Value> {
    reported["EventDestinations"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected an EventDestinations list, got {reported}")
        })
        .clone()
}

#[test]
fn the_sesv2_sns_event_destination_setup_is_served_through_the_whole_emulator() {
    std::thread::Builder::new()
        .name("sesv2-sns-event-destination".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            // Signature verification is the front door's concern, not this suite's: what the
            // credential scope is here for is the routing signal it carries.
            router.set_request_authentication(false);

            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            for (index, setup) in SETUPS.iter().enumerate() {
                let case = setup.case;
                let topic_name = format!("ses-events-{index}");
                let configuration_set = format!("events-set-{index}");
                let destination_name = format!("sns-destination-{index}");

                // ---- The topic the events would be published to.
                let created = sns(
                    &router,
                    &runtime,
                    case,
                    "CreateTopic",
                    &[("Name", &topic_name)],
                );
                let topic_arn = element(&created, "TopicArn");
                assert!(
                    topic_arn.starts_with(&format!("arn:aws:sns:{REGION}:"))
                        && topic_arn.ends_with(&format!(":{topic_name}")),
                    "case {case:?}: the topic ARN must name this Region and the topic, got \
                     {topic_arn:?}"
                );
                let account = topic_arn
                    .split(':')
                    .nth(4)
                    .unwrap_or_else(|| panic!("case {case:?}: no account in {topic_arn}"))
                    .to_string();
                let facts = TopicFacts {
                    arn: topic_arn.clone(),
                    account,
                };

                // A topic its owner has set no policy on reports the document SNS generates,
                // which grants the OWNING ACCOUNT every topic action and names no service
                // principal at all — so it does not permit the SES principal to publish, which
                // is why a scenario has to grant that itself.
                let (_, default_policy) = reported_policy(&router, &runtime, case, &topic_arn);
                assert!(
                    !permits_ses_publish(&default_policy, &topic_arn),
                    "case {case:?}: the policy a topic carries before its owner sets one must \
                     not permit {SES_SERVICE_PRINCIPAL} to publish, got {default_policy}"
                );

                // ---- The grant, set through SNS and read back.
                let granted = (setup.policy)(&facts).to_string();
                sns(
                    &router,
                    &runtime,
                    case,
                    "SetTopicAttributes",
                    &[
                        ("TopicArn", &topic_arn),
                        ("AttributeName", POLICY_ATTRIBUTE),
                        ("AttributeValue", &granted),
                    ],
                );
                let (stored, parsed) = reported_policy(&router, &runtime, case, &topic_arn);
                assert_eq!(
                    stored, granted,
                    "case {case:?}: the topic must report the policy its owner set, got {stored}"
                );
                assert!(
                    permits_ses_publish(&parsed, &topic_arn),
                    "case {case:?}: the policy the topic reports must permit \
                     {SES_SERVICE_PRINCIPAL} to publish to {topic_arn}, got {stored}"
                );

                // ---- The configuration set the event destination hangs off: the leg the
                // signing-name collision answered `AmbiguousService`, before any handler ran.
                let answered = sesv2(
                    &router,
                    &runtime,
                    setup,
                    "POST",
                    "/v2/email/configuration-sets",
                    Some(&json!({
                        "ConfigurationSetName": configuration_set,
                        "TrackingOptions": { "CustomRedirectDomain": setup.custom_redirect_domain },
                        "SendingOptions": { "SendingEnabled": setup.sending_enabled },
                    })),
                );
                assert!(
                    answered.is_object(),
                    "case {case:?}: CreateConfigurationSet must answer its modelled document, \
                     got {answered}"
                );

                let read_back = sesv2(
                    &router,
                    &runtime,
                    setup,
                    "GET",
                    &format!("/v2/email/configuration-sets/{configuration_set}"),
                    None,
                );
                assert_eq!(
                    (
                        read_back["ConfigurationSetName"].as_str(),
                        read_back["TrackingOptions"]["CustomRedirectDomain"].as_str(),
                        read_back["SendingOptions"]["SendingEnabled"].as_bool(),
                    ),
                    (
                        Some(configuration_set.as_str()),
                        Some(setup.custom_redirect_domain),
                        Some(setup.sending_enabled),
                    ),
                    "case {case:?}: GetConfigurationSet must report what the create persisted, \
                     got {read_back}"
                );

                // ---- The event destination naming the topic SNS minted above.
                sesv2(
                    &router,
                    &runtime,
                    setup,
                    "POST",
                    &format!(
                        "/v2/email/configuration-sets/{configuration_set}/event-destinations"
                    ),
                    Some(&json!({
                        "EventDestinationName": destination_name,
                        "EventDestination": {
                            "Enabled": setup.enabled,
                            "MatchingEventTypes": setup.matching_event_types,
                            "SnsDestination": { "TopicArn": topic_arn },
                        },
                    })),
                );

                let reported = sesv2(
                    &router,
                    &runtime,
                    setup,
                    "GET",
                    &format!(
                        "/v2/email/configuration-sets/{configuration_set}/event-destinations"
                    ),
                    None,
                );
                let destinations = reported_destinations(&reported, case);
                assert_eq!(
                    destinations.len(),
                    1,
                    "case {case:?}: the set must report the one destination it was given, got \
                     {reported}"
                );
                let destination = &destinations[0];
                let matched: Vec<&str> = destination["MatchingEventTypes"]
                    .as_array()
                    .map(|types| types.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                assert_eq!(
                    (
                        destination["Name"].as_str(),
                        destination["Enabled"].as_bool(),
                        matched.as_slice(),
                        destination["SnsDestination"]["TopicArn"].as_str(),
                    ),
                    (
                        Some(destination_name.as_str()),
                        Some(setup.enabled),
                        setup.reported_event_types,
                        Some(topic_arn.as_str()),
                    ),
                    "case {case:?}: the event destination must report what was attached, naming \
                     the topic SNS minted, got {destination}"
                );
                for other in [
                    "KinesisFirehoseDestination",
                    "CloudWatchDestination",
                    "EventBridgeDestination",
                    "PinpointDestination",
                ] {
                    assert!(
                        destination.get(other).is_none(),
                        "case {case:?}: a destination given only an SnsDestination must report \
                         no {other}, got {destination}"
                    );
                }

                // ---- And the grant is still the caller's own: configuring an event
                // destination does not rewrite the topic's access policy.
                let (after, parsed_after) = reported_policy(&router, &runtime, case, &topic_arn);
                assert_eq!(
                    after, granted,
                    "case {case:?}: attaching an event destination must leave the topic policy \
                     its owner set, got {after}"
                );
                assert!(
                    permits_ses_publish(&parsed_after, &topic_arn),
                    "case {case:?}: the topic must still permit {SES_SERVICE_PRINCIPAL} to \
                     publish after the destination is attached, got {after}"
                );
            }
        })
        .expect("spawn the setup thread")
        .join()
        .expect("the SESv2 and SNS setup must be served end to end");
}
