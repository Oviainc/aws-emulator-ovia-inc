// SPDX-License-Identifier: CC0-1.0

//! The setup an SES send-event-publication scenario performs when its events are published
//! through a Firehose stream into Amazon S3, across the four services it spans: the bucket,
//! the IAM roles, the Firehose stream delivering to that bucket, and the SESv2 sending
//! identity plus configuration set whose event destination names that stream.
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
//! key: a service claiming on that key alone claims every request in the family, every such
//! request then has four claimants, and the router refuses it `500 AmbiguousService` — a code
//! no AWS model carries, on a status an SDK classifies as retryable, so a client spends its
//! whole retry budget and the SESv2 surface answers nothing. With that, neither
//! `CreateEmailIdentity` nor `CreateConfigurationSet` can be served, and no event destination
//! can be wired at all.
//!
//! Two sibling suites already pin that resolution: `ses_family_signing_name_routing.rs` tables
//! it across all four wire forms, and `ses_event_destination_setup_across_services.rs` drives
//! the setup whose destination is an SNS topic. What this suite adds is the FIREHOSE form of
//! that destination, which neither covers: the stream and the bucket behind it are created
//! through Firehose and Amazon S3 on the same engine, so the destination SESv2 stores names
//! the stream ARN Firehose minted rather than a literal, and the reader reports it back.
//!
//! The rows differ along what the rule generalises over: the identity form (apex domain,
//! subdomain, mixed-case domain, address), how many event types the destination matches (one,
//! a pair, every modelled type), whether it is enabled, and which of the two S3 destination
//! forms the stream behind it was configured with — none of which may change which service
//! serves the request, or what the readers report back.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode};

/// The Region every request below is signed for and addressed at.
const REGION: &str = "us-east-1";

/// IAM's API version, which an `awsQuery` body carries beside its `Action`
/// (`smithy_query(query="service_info")` on the IAM model: version 2010-05-08).
const IAM_API_VERSION: &str = "2010-05-08";

/// Every value of the SESv2 `EventType` enum, in the model's own order
/// (`smithy_query(query="shape",shape="EventType")` on the SESv2 model), so the widest row
/// matches the whole published event surface rather than the types one scenario watches.
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

/// The two routing refusals the router answers itself, which no leg here may provoke:
/// `AmbiguousService` (several services claimed one request) and `UnknownService` (none did).
/// Neither is in any AWS model.
const ROUTER_REFUSALS: &[&str] = &["AmbiguousService", "UnknownService"];

/// One setup row: the sending identity, the Firehose stream its events are published
/// through, and the event destination that names that stream.
struct Setup {
    /// What this row is, for the assertion messages.
    case: &'static str,
    /// The slug the row's own resources are named from, so no row reads another's.
    slug: &'static str,
    /// The `EmailIdentity` to verify.
    identity: &'static str,
    /// The `IdentityType` the identity's own form implies: an identity with a local part is
    /// an address, anything else is a domain.
    identity_type: &'static str,
    /// Which `CreateDeliveryStream` member carries the stream's S3 destination, and so which
    /// member of `DestinationDescription` reports it back
    /// (`smithy_query(query="shape",shape="DestinationDescription")` on the Firehose model):
    /// the deprecated `S3DestinationConfiguration` -> `S3DestinationDescription`, or
    /// `ExtendedS3DestinationConfiguration` -> `ExtendedS3DestinationDescription`.
    extended_s3: bool,
    /// `EventDestinationDefinition$MatchingEventTypes`.
    matching_event_types: &'static [&'static str],
    /// `EventDestinationDefinition$Enabled`.
    enabled: bool,
}

/// The rows, spanning the identity forms, the destination shapes and the two stream
/// configurations the rule covers.
const SETUPS: &[Setup] = &[
    Setup {
        case: "an apex domain whose destination matches the send and delivery events",
        slug: "fh-apex",
        identity: "firehose-apex.example.com",
        identity_type: "DOMAIN",
        extended_s3: false,
        matching_event_types: &["SEND", "DELIVERY"],
        enabled: true,
    },
    Setup {
        case: "a subdomain whose destination matches one event type",
        slug: "fh-sub",
        identity: "mail.firehose-sub.example.org",
        identity_type: "DOMAIN",
        extended_s3: true,
        matching_event_types: &["SEND"],
        enabled: true,
    },
    Setup {
        case: "a mixed-case domain whose destination matches every modelled event type",
        slug: "fh-mixed",
        identity: "Firehose-Mixed.Example.NET",
        identity_type: "DOMAIN",
        extended_s3: false,
        matching_event_types: EVERY_EVENT_TYPE,
        enabled: true,
    },
    Setup {
        case: "an address identity whose destination matches a triple and is disabled",
        slug: "fh-address",
        identity: "sender@firehose-address.example.com",
        identity_type: "EMAIL_ADDRESS",
        extended_s3: true,
        matching_event_types: &["BOUNCE", "COMPLAINT", "REJECT"],
        enabled: false,
    },
];

/// The seven services this setup needs, over ONE shared engine, as `core/router/src/lib.rs`
/// mounts them: the four the sequence calls, plus the three that share SESv2's signing name
/// and whose claims are what the resolution has to tell apart. Without those three peers
/// mounted the collision does not exist, so a suite that left them out could not fail on it.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These legs exercise routing and handler behaviour, not SigV4.
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
        service_com_amazonaws_firehose_firehose_20150804::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
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
        return crate::query_wire::opt_element(body, "Code").unwrap_or_default();
    }
    let raw = error_type
        .map(str::to_string)
        .or_else(|| {
            serde_json::from_str::<Value>(body).ok().and_then(|value| {
                value
                    .get("__type")
                    .or_else(|| value.get("Code"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
        })
        .unwrap_or_default();
    let local = raw.split('#').next_back().unwrap_or(&raw).to_string();
    local.split(':').next().unwrap_or(&local).to_string()
}

/// The error message an answer carries, whichever casing its envelope uses.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
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
/// produces: a request that reached nobody, or that several services claimed. Every leg
/// goes through here, so no step of the sequence can pass on a routing refusal.
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

/// One request at the endpoint host `signing_name` resolves to, signed for that name.
fn request_to(
    signing_name: &str,
    host: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> http::Request<Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("authorization", authorization(signing_name));
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder.body(body).expect("a well-formed request")
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
    let request = request_to(
        "ses",
        &format!("email.{REGION}.amazonaws.com"),
        method,
        path,
        &[("content-type", "application/json")],
        payload,
    );
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

/// One Firehose call: `awsJson1_1`, `POST /` with the operation in `X-Amz-Target`.
async fn firehose_ok(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let target = format!("Firehose_20150804.{operation}");
    let request = request_to(
        "firehose",
        &format!("firehose.{REGION}.amazonaws.com"),
        "POST",
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", &target),
        ],
        Bytes::from(body.to_string()),
    );
    let answered = send(router, case, &format!("Firehose {operation}"), request).await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected HTTP 200 from Firehose {operation}, got {} {}: {}",
        answered.status, answered.code, answered.text
    );
    answered.json
}

/// One IAM call that must succeed: `awsQuery`, `POST /` with the parameters in the form
/// body, answering the XML document a query client reads.
async fn iam_ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    parameters: &[(&str, &str)],
) -> String {
    let mut body = format!("Action={action}&Version={IAM_API_VERSION}");
    for (name, value) in parameters {
        body.push_str(&format!("&{name}={}", form_encode(value)));
    }
    let request = request_to(
        "iam",
        &format!("iam.{REGION}.amazonaws.com"),
        "POST",
        "/",
        &[("content-type", "application/x-www-form-urlencoded")],
        Bytes::from(body),
    );
    let answered = send(router, case, &format!("IAM {action}"), request).await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected HTTP 200 from IAM {action}, got {} {}: {}",
        answered.status, answered.code, answered.text
    );
    assert!(
        answered.text.trim_start().starts_with('<'),
        "case {case:?}: an awsQuery client must be answered an XML document from IAM \
         {action}, got {}",
        answered.text
    );
    answered.text
}

/// `s3:CreateBucket` — `PUT /{bucket}` at the S3 endpoint, answering the ARN that names the
/// created bucket.
///
/// The ARN is composed here rather than read back: `CreateBucket` models no output member
/// carrying one (unlike the Firehose stream ARN and the IAM role ARN below, which are the
/// values those services minted). The Region every request here is signed for is the one S3
/// creates a bucket in when the create names no `LocationConstraint`, so the body is empty.
async fn create_bucket(router: &MultiServiceRouter, case: &str, bucket: &str) -> String {
    let request = request_to(
        "s3",
        &format!("s3.{REGION}.amazonaws.com"),
        "PUT",
        &format!("/{bucket}"),
        &[("content-type", "application/xml")],
        Bytes::new(),
    );
    let answered = send(router, case, &format!("S3 CreateBucket {bucket}"), request).await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected HTTP 200 from S3 CreateBucket, got {} {}: {}",
        answered.status, answered.code, answered.text
    );
    format!("arn:aws:s3:::{bucket}")
}

/// A trust policy with one `Allow` statement letting `service` assume the role.
///
/// Written compactly, as the pieces an IAM trust policy for a service principal carries.
fn trust_policy(service: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"Service\":\"{service}\"}},\"Action\":\"sts:AssumeRole\"}}]}}"
    )
}

/// Create `role_name` trusting `service`, answering the role ARN IAM minted for it.
async fn create_role(
    router: &MultiServiceRouter,
    case: &str,
    role_name: &str,
    service: &str,
) -> String {
    let policy = trust_policy(service);
    let created = iam_ok(
        router,
        case,
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("AssumeRolePolicyDocument", policy.as_str()),
        ],
    )
    .await;
    let arn = element(&created, "Arn");
    assert!(
        arn.starts_with("arn:aws:iam::") && arn.ends_with(&format!(":role/{role_name}")),
        "case {case:?}: the role ARN must name the role that was created, got {arn}"
    );
    arn
}

/// The Firehose stream `name` delivering to `bucket_arn` with `role_arn`, answering the
/// stream ARN Firehose minted. `extended` picks which of the two modelled S3 destination
/// members the configuration is carried in.
async fn delivery_stream(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    bucket_arn: &str,
    role_arn: &str,
    extended: bool,
) -> String {
    let configuration = json!({
        "RoleARN": role_arn,
        "BucketARN": bucket_arn,
        "Prefix": "ses-events/",
    });
    let mut create = json!({ "DeliveryStreamName": name, "DeliveryStreamType": "DirectPut" });
    let (configuration_member, description_member) = if extended {
        (
            "ExtendedS3DestinationConfiguration",
            "ExtendedS3DestinationDescription",
        )
    } else {
        ("S3DestinationConfiguration", "S3DestinationDescription")
    };
    create[configuration_member] = configuration;

    let created = firehose_ok(router, case, "CreateDeliveryStream", &create).await;
    let stream_arn = created["DeliveryStreamARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a DeliveryStreamARN, got {created}"))
        .to_string();
    assert!(
        stream_arn.starts_with(&format!("arn:aws:firehose:{REGION}:"))
            && stream_arn.ends_with(&format!(":deliverystream/{name}")),
        "case {case:?}: the stream ARN must name the stream that was created, got {stream_arn}"
    );

    // The round trip: the stream the create wrote is readable, under the same ARN and with
    // the destination the create configured.
    let described = firehose_ok(
        router,
        case,
        "DescribeDeliveryStream",
        &json!({ "DeliveryStreamName": name }),
    )
    .await;
    let description = &described["DeliveryStreamDescription"];
    assert_eq!(
        description["DeliveryStreamARN"].as_str(),
        Some(stream_arn.as_str()),
        "case {case:?}: the reader must report the ARN the create answered with, got \
         {described}"
    );
    assert_eq!(
        description["Destinations"][0][description_member]["BucketARN"].as_str(),
        Some(bucket_arn),
        "case {case:?}: the stream's {description_member} must name the bucket it delivers \
         to, got {described}"
    );
    stream_arn
}

/// The event destination reported under `name`, out of a
/// `GetConfigurationSetEventDestinations` answer.
fn reported_destination(case: &str, answered: &Value, name: &str) -> Value {
    answered["EventDestinations"]
        .as_array()
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .find(|destination| destination["Name"].as_str() == Some(name))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected an event destination named {name}, got {answered}")
        })
        .clone()
}

/// The `MatchingEventTypes` a reported destination carries, as a set.
fn matched_types(destination: &Value) -> BTreeSet<String> {
    destination["MatchingEventTypes"]
        .as_array()
        .map(|types| {
            types
                .iter()
                .filter_map(|event_type| event_type.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn the_setup_a_firehose_published_scenario_performs_is_served_end_to_end() {
    let router = router();

    // The two roles the setup names: the one Firehose assumes to write to the bucket (which
    // Firehose resolves against IAM when the stream is written), and the one SES assumes to
    // publish events into the stream.
    let delivery_role = create_role(
        &router,
        "setup",
        "ses-firehose-delivery",
        "firehose.amazonaws.com",
    )
    .await;
    let publishing_role = create_role(
        &router,
        "setup",
        "ses-event-publishing",
        "ses.amazonaws.com",
    )
    .await;

    for setup in SETUPS {
        let case = setup.case;
        let bucket = format!("{}-events", setup.slug);
        let stream = format!("{}-stream", setup.slug);
        let configuration_set = format!("{}-set", setup.slug);
        let destination_name = format!("{}-destination", setup.slug);

        let bucket_arn = create_bucket(&router, case, &bucket).await;
        let stream_arn = delivery_stream(
            &router,
            case,
            &stream,
            &bucket_arn,
            &delivery_role,
            setup.extended_s3,
        )
        .await;

        // The two calls the issue names, each signed for `ses` at the `email` endpoint: with
        // the claim resolved they reach SESv2 alone and are answered by it.
        let created = sesv2_ok(
            &router,
            case,
            "POST",
            "/v2/email/identities",
            Some(json!({ "EmailIdentity": setup.identity })),
        )
        .await;
        assert_eq!(
            created["IdentityType"].as_str(),
            Some(setup.identity_type),
            "case {case:?}: the verified identity's type must follow the identity's form, \
             got {created}"
        );
        assert!(
            created["VerifiedForSendingStatus"].is_boolean(),
            "case {case:?}: CreateEmailIdentity must report VerifiedForSendingStatus, got \
             {created}"
        );

        let set = sesv2_ok(
            &router,
            case,
            "POST",
            "/v2/email/configuration-sets",
            Some(json!({ "ConfigurationSetName": configuration_set })),
        )
        .await;
        assert_eq!(
            set,
            json!({}),
            "case {case:?}: CreateConfigurationSet's response carries no members, got {set}"
        );

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
                    "KinesisFirehoseDestination": {
                        "IamRoleArn": publishing_role,
                        "DeliveryStreamArn": stream_arn,
                    },
                },
            })),
        )
        .await;

        // The read-back: the destination SESv2 stored names the stream ARN Firehose minted
        // and the role ARN IAM minted, with the event types and the flag it was created
        // with — and no destination of another kind.
        let answered = sesv2_ok(
            &router,
            case,
            "GET",
            &format!("/v2/email/configuration-sets/{configuration_set}/event-destinations"),
            None,
        )
        .await;
        let destination = reported_destination(case, &answered, &destination_name);
        assert_eq!(
            destination["KinesisFirehoseDestination"]["DeliveryStreamArn"].as_str(),
            Some(stream_arn.as_str()),
            "case {case:?}: the destination must name the stream Firehose minted, got \
             {destination}"
        );
        assert_eq!(
            destination["KinesisFirehoseDestination"]["IamRoleArn"].as_str(),
            Some(publishing_role.as_str()),
            "case {case:?}: the destination must name the role IAM minted, got {destination}"
        );
        assert_eq!(
            matched_types(&destination),
            setup
                .matching_event_types
                .iter()
                .map(|event_type| (*event_type).to_string())
                .collect::<BTreeSet<_>>(),
            "case {case:?}: the destination must match the event types it was created with, \
             got {destination}"
        );
        assert_eq!(
            destination["Enabled"].as_bool(),
            Some(setup.enabled),
            "case {case:?}: the destination must carry the enabled flag it was created with, \
             got {destination}"
        );
        assert!(
            destination["SnsDestination"].is_null(),
            "case {case:?}: a Firehose destination carries no SnsDestination, got \
             {destination}"
        );

        // And the identity is readable back through its own reader, so the create that was
        // routed here persisted it rather than answering out of the request.
        let read_back = sesv2_ok(
            &router,
            case,
            "GET",
            &format!("/v2/email/identities/{}", setup.identity),
            None,
        )
        .await;
        assert_eq!(
            read_back["IdentityType"].as_str(),
            Some(setup.identity_type),
            "case {case:?}: the reader must report the identity's type, got {read_back}"
        );
    }
}

#[tokio::test]
async fn a_second_destination_of_one_name_is_refused_in_sesv2_s_own_envelope() {
    let router = router();

    let delivery_role = create_role(
        &router,
        "setup",
        "ses-firehose-delivery",
        "firehose.amazonaws.com",
    )
    .await;
    let publishing_role = create_role(
        &router,
        "setup",
        "ses-event-publishing",
        "ses.amazonaws.com",
    )
    .await;

    let case = "an event destination created twice under one name";
    let bucket_arn = create_bucket(&router, case, "fh-refusal-events").await;
    let stream_arn = delivery_stream(
        &router,
        case,
        "fh-refusal-stream",
        &bucket_arn,
        &delivery_role,
        false,
    )
    .await;

    let configuration_set = "fh-refusal-set";
    let destination_name = "fh-refusal-destination";
    sesv2_ok(
        &router,
        case,
        "POST",
        "/v2/email/configuration-sets",
        Some(json!({ "ConfigurationSetName": configuration_set })),
    )
    .await;
    let destination = |enabled: bool, types: Value| {
        json!({
            "EventDestinationName": destination_name,
            "EventDestination": {
                "Enabled": enabled,
                "MatchingEventTypes": types,
                "KinesisFirehoseDestination": {
                    "IamRoleArn": publishing_role,
                    "DeliveryStreamArn": stream_arn,
                },
            },
        })
    };
    let path = format!("/v2/email/configuration-sets/{configuration_set}/event-destinations");
    sesv2_ok(
        &router,
        case,
        "POST",
        &path,
        Some(destination(true, json!(["SEND"]))),
    )
    .await;

    // The second create of the same name is refused by SESv2 itself — its modelled code on
    // its modelled status, terminal — and not by the router.
    let refused = sesv2(
        &router,
        case,
        "POST",
        &path,
        Some(destination(false, json!(["BOUNCE"]))),
    )
    .await;
    assert_eq!(
        (refused.status, refused.code.as_str()),
        (400, "AlreadyExistsException"),
        "case {case:?}: a destination name a set already holds must be refused 400 \
         AlreadyExistsException, got {} {}: {}",
        refused.status,
        refused.code,
        refused.text
    );
    let message = error_message(&refused.json);
    assert!(
        message.contains(destination_name) && message.contains(configuration_set),
        "case {case:?}: the refusal must name the destination and the configuration set, \
         got {message:?}"
    );

    // And the refused create changed nothing: the stored destination is still the one the
    // first create wrote.
    let answered = sesv2_ok(&router, case, "GET", &path, None).await;
    assert_eq!(
        answered["EventDestinations"].as_array().map(Vec::len),
        Some(1),
        "case {case:?}: a refused create must add no destination, got {answered}"
    );
    let stored = reported_destination(case, &answered, destination_name);
    assert_eq!(
        (
            stored["Enabled"].as_bool(),
            matched_types(&stored),
            stored["KinesisFirehoseDestination"]["DeliveryStreamArn"].as_str(),
        ),
        (
            Some(true),
            BTreeSet::from(["SEND".to_string()]),
            Some(stream_arn.as_str()),
        ),
        "case {case:?}: a refused create must leave the stored destination unchanged, got \
         {stored}"
    );
}
