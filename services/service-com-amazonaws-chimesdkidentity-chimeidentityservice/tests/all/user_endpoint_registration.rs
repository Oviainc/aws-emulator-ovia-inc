// SPDX-License-Identifier: CC0-1.0

//! The `AppInstanceUserEndpoint` lifecycle, end to end over HTTP.
//!
//! The RULES pinned here:
//!
//! * a register PERSISTS the endpoint under the `AppInstanceUser` it was addressed to and
//!   `DescribeAppInstanceUserEndpoint` reports every member back exactly as registered,
//!   under a freshly minted `EndpointId` that fits the modelled `String64`;
//! * a register resolves the Amazon Pinpoint application its `ResourceArn` names before
//!   it registers anything: an ARN in Pinpoint's `apps` space that names no application
//!   Pinpoint mints is refused `ForbiddenException`, an ARN that is not a Pinpoint
//!   application ARN at all leaves the endpoint registered but `INACTIVE` with
//!   `StatusReason` `INVALID_PINPOINT_ARN`, and one that does resolve registers an
//!   `ACTIVE` endpoint with no reason;
//! * one `ClientRequestToken` registers one endpoint: a repeat replays that endpoint's
//!   id, concurrent repeats all answer with that one id, and the same token presented
//!   with different parameters is refused `ConflictException`;
//! * an update writes the name and the opt-in and nothing else, and re-stamps the
//!   endpoint's last-updated instant;
//! * a deregister removes the endpoint, and every endpoint-keyed call addressed to an
//!   endpoint the user does not hold is refused `ForbiddenException`;
//! * `ListAppInstanceUserEndpoints` reports the endpoints of the ONE user it was asked
//!   about, in a stable key order, paged at `MaxResults`;
//! * a register past the store's endpoint bound is refused `ResourceLimitExceededException`
//!   and stores nothing.
//!
//! The tables span what those rules generalise over: the endpoint type (every modelled
//! value), the `ResourceArn`'s vendor, resource type, partition and application id (each
//! side of the application-id form), the `Name`, device-token and `AllowMessages` values
//! and sizes a register may carry, how many times a token is repeated, which member a
//! reused token differs in, which members an update carries, and how many endpoints a
//! listing pages over.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_chimesdkidentity_chimeidentityservice::state::MAX_APP_INSTANCE_USER_ENDPOINTS;

use crate::app_instance_lifecycle::{uri_encode, Emulator};

/// Every value of `AppInstanceUserEndpointType`.
///
/// Source: `smithy_query(query="shape",shape="AppInstanceUserEndpointType")` — the enum
/// is `APNS | APNS_SANDBOX | GCM`.
const ENDPOINT_TYPES: [&str; 3] = ["APNS", "APNS_SANDBOX", "GCM"];

/// The fixed sentence the service refuses a request with that addresses something the
/// caller may not act on, as the modelled `ForbiddenException` ("The client is
/// permanently forbidden from making the request") carries it.
const FORBIDDEN_MESSAGE: &str = "You are not allowed to make this request.";

/// One row of the member table: the case's label and the optional members a register
/// carries (`Name`, `VoipDeviceToken`, `AllowMessages`).
type MemberRow<'a> = (
    &'a str,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
);

/// One row of the reused-token table: the case's label and the single change it makes
/// to the request the token already carried.
type MismatchRow<'a> = (&'a str, Box<dyn Fn(&mut Value)>);

/// An application id of the form Pinpoint mints: 32 hexadecimal characters.
fn pinpoint_application_id(seed: u128) -> String {
    format!("{seed:032x}")
}

/// A well-formed Amazon Pinpoint application ARN.
///
/// Source: Pinpoint publishes `mobiletargeting` as its ARN namespace
/// (`smithy_query(query="service_info")` on the Pinpoint model) and writes an
/// application as `apps/<application-id>`;
/// `smithy_query(query="operation",operation="RegisterAppInstanceUserEndpoint")`
/// documents `ResourceArn` as that Pinpoint app ARN for every endpoint type.
fn pinpoint_arn(partition: &str, region: &str, application_id: &str) -> String {
    format!("arn:{partition}:mobiletargeting:{region}:123456789012:apps/{application_id}")
}

/// A Pinpoint application ARN that resolves, distinct per `seed`.
fn resolvable_pinpoint_arn(seed: u128) -> String {
    pinpoint_arn("aws", "us-east-1", &pinpoint_application_id(seed))
}

/// The required members of a register request.
fn register_body(token: &str, endpoint_type: &str, resource_arn: &str, device_token: &str) -> Value {
    json!({
        "ClientRequestToken": token,
        "Type": endpoint_type,
        "ResourceArn": resource_arn,
        "EndpointAttributes": { "DeviceToken": device_token },
    })
}

/// `POST /app-instance-users/{AppInstanceUserArn}/endpoints`.
async fn register(emulator: &Emulator, user_arn: &str, body: Value) -> (u16, Value) {
    emulator
        .call(
            "POST",
            &format!("/app-instance-users/{}/endpoints", uri_encode(user_arn)),
            Some(body),
        )
        .await
}

/// `GET /app-instance-users/{AppInstanceUserArn}/endpoints/{EndpointId}`.
async fn describe_endpoint(emulator: &Emulator, user_arn: &str, endpoint_id: &str) -> (u16, Value) {
    emulator
        .call(
            "GET",
            &format!(
                "/app-instance-users/{}/endpoints/{}",
                uri_encode(user_arn),
                uri_encode(endpoint_id)
            ),
            None,
        )
        .await
}

/// `PUT /app-instance-users/{AppInstanceUserArn}/endpoints/{EndpointId}`.
async fn update_endpoint(
    emulator: &Emulator,
    user_arn: &str,
    endpoint_id: &str,
    body: Value,
) -> (u16, Value) {
    emulator
        .call(
            "PUT",
            &format!(
                "/app-instance-users/{}/endpoints/{}",
                uri_encode(user_arn),
                uri_encode(endpoint_id)
            ),
            Some(body),
        )
        .await
}

/// `DELETE /app-instance-users/{AppInstanceUserArn}/endpoints/{EndpointId}`.
async fn deregister_endpoint(
    emulator: &Emulator,
    user_arn: &str,
    endpoint_id: &str,
) -> (u16, Value) {
    emulator
        .call(
            "DELETE",
            &format!(
                "/app-instance-users/{}/endpoints/{}",
                uri_encode(user_arn),
                uri_encode(endpoint_id)
            ),
            None,
        )
        .await
}

/// `GET /app-instance-users/{AppInstanceUserArn}/endpoints`, plus any query parameters.
async fn list_endpoints(emulator: &Emulator, user_arn: &str, query: &str) -> (u16, Value) {
    emulator
        .call(
            "GET",
            &format!(
                "/app-instance-users/{}/endpoints{query}",
                uri_encode(user_arn)
            ),
            None,
        )
        .await
}

/// Register and return the minted `EndpointId`, failing the test if it was refused.
async fn endpoint_id(emulator: &Emulator, user_arn: &str, body: Value) -> String {
    let (status, answer) = register(emulator, user_arn, body).await;
    assert_eq!(
        status, 201,
        "expected RegisterAppInstanceUserEndpoint to answer 201, got {status} {answer:?}"
    );
    assert_eq!(
        answer["AppInstanceUserArn"].as_str(),
        Some(user_arn),
        "expected the register to answer with {user_arn:?}, got {answer:?}"
    );
    answer["EndpointId"]
        .as_str()
        .unwrap_or_else(|| panic!("the register returned no EndpointId: {answer:?}"))
        .to_string()
}

/// An `AppInstance` and one user under it.
async fn user(emulator: &Emulator, label: &str) -> String {
    let instance = emulator
        .app_instance(&format!("ep-{label}"), &format!("tok-ep-{label}"))
        .await;
    emulator
        .user(&instance, label, &format!("tok-ep-{label}-user"))
        .await
}

/// RULE: a register persists the endpoint and `DescribeAppInstanceUserEndpoint` reports
/// every registered member back — the type, the resource ARN, the device tokens, the
/// name and the opt-in exactly as sent, each under its own freshly minted `EndpointId`,
/// stamped with a created instant and a last-updated instant equal to it.
#[tokio::test]
async fn a_registered_endpoint_round_trips_through_describe() {
    let emulator = Emulator::new();
    let user_arn = user(&emulator, "roundtrip").await;

    // Rows: (label, Name, VoipDeviceToken, AllowMessages) — the optional members, each
    // absent, empty, and at sizes and in scripts the model admits (`Name` is
    // `SensitiveString1600`, len 0..1600, pattern `.*`; `VoipDeviceToken` is
    // `NonEmptySensitiveString1600`, len 1..1600; `AllowMessages` is `ALL | NONE`).
    let long_name = "n".repeat(1600);
    let rows: Vec<MemberRow<'_>> = vec![
        ("no optional member at all", None, None, None),
        ("an empty name", Some(""), None, None),
        ("a plain name", Some("my-phone"), None, Some("ALL")),
        ("an opted-out endpoint", Some("silent"), None, Some("NONE")),
        (
            "a non-ASCII name",
            Some("téléphone-私の端末"),
            Some("voip-τoken"),
            Some("ALL"),
        ),
        (
            "a name at the modelled maximum",
            Some(long_name.as_str()),
            None,
            Some("NONE"),
        ),
        (
            "a voip token beside the device token",
            Some("ipad"),
            Some("voip-0123456789abcdef"),
            None,
        ),
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for endpoint_type in ENDPOINT_TYPES {
        for (index, (label, name, voip, allow)) in rows.iter().enumerate() {
            let case = format!("{endpoint_type}/{label}");
            let device_token = format!("device-token-{endpoint_type}-{index}");
            let resource_arn = resolvable_pinpoint_arn(index as u128);
            let mut body = register_body(
                &format!("tok-rt-{endpoint_type}-{index}"),
                endpoint_type,
                &resource_arn,
                &device_token,
            );
            if let Some(name) = name {
                body["Name"] = json!(name);
            }
            if let Some(voip) = voip {
                body["EndpointAttributes"]["VoipDeviceToken"] = json!(voip);
            }
            if let Some(allow) = allow {
                body["AllowMessages"] = json!(allow);
            }

            let minted_id = endpoint_id(&emulator, &user_arn, body).await;
            assert!(
                !minted_id.is_empty() && minted_id.len() <= 64,
                "case {case:?}: expected an EndpointId within the modelled String64 bound, \
                 got {minted_id:?}"
            );
            assert!(
                minted.insert(minted_id.clone()),
                "case {case:?}: expected a fresh EndpointId, but {minted_id:?} was already minted"
            );

            let (status, described) = describe_endpoint(&emulator, &user_arn, &minted_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the registered endpoint to describe, got \
                 {status} {described:?}"
            );
            let endpoint = &described["AppInstanceUserEndpoint"];
            assert_eq!(
                (
                    endpoint["AppInstanceUserArn"].as_str(),
                    endpoint["EndpointId"].as_str(),
                    endpoint["Type"].as_str(),
                    endpoint["ResourceArn"].as_str(),
                ),
                (
                    Some(user_arn.as_str()),
                    Some(minted_id.as_str()),
                    Some(endpoint_type),
                    Some(resource_arn.as_str()),
                ),
                "case {case:?}: expected the registered identity back, got {endpoint:?}"
            );
            assert_eq!(
                endpoint["Name"].as_str(),
                *name,
                "case {case:?}: expected Name {name:?}, got {:?}",
                endpoint["Name"]
            );
            assert_eq!(
                endpoint["AllowMessages"].as_str(),
                *allow,
                "case {case:?}: expected AllowMessages {allow:?}, got {:?}",
                endpoint["AllowMessages"]
            );
            assert_eq!(
                (
                    endpoint["EndpointAttributes"]["DeviceToken"].as_str(),
                    endpoint["EndpointAttributes"]["VoipDeviceToken"].as_str(),
                ),
                (Some(device_token.as_str()), *voip),
                "case {case:?}: expected the registered attributes back, got {:?}",
                endpoint["EndpointAttributes"]
            );

            let created = endpoint["CreatedTimestamp"].as_f64();
            let updated = endpoint["LastUpdatedTimestamp"].as_f64();
            assert!(
                created.is_some() && updated == created,
                "case {case:?}: expected a created instant and a last-updated instant equal \
                 to it on a freshly registered endpoint, got {created:?} and {updated:?}"
            );
        }
    }
}

/// What a `ResourceArn` makes of a register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resolution {
    /// It names a Pinpoint application: the endpoint is registered and active.
    Active,
    /// It is no Pinpoint application ARN at all: the endpoint is registered, and
    /// inactive because nothing behind it can deliver.
    Inactive,
    /// It is in Pinpoint's `apps` space but names no application Pinpoint mints: the
    /// register is refused.
    Forbidden,
}

/// RULE: a register resolves the Amazon Pinpoint application its `ResourceArn` names
/// before registering anything — an `apps` ARN that names no application Pinpoint mints
/// is refused with the modelled `ForbiddenException`, an ARN that is not a Pinpoint
/// application ARN leaves the endpoint registered but `INACTIVE` with `StatusReason`
/// `INVALID_PINPOINT_ARN`, and an ARN that resolves registers an `ACTIVE` endpoint whose
/// state carries no reason.
#[tokio::test]
async fn a_registers_resource_arn_decides_the_endpoints_state() {
    let emulator = Emulator::new();
    let user_arn = user(&emulator, "state").await;

    // Rows: (label, ResourceArn, what it resolves to). Every value is an ARN the
    // modelled `ChimeArn` pattern admits, so what is under test is the service's reading
    // of it rather than the boundary validator. The rows differ in vendor, resource
    // type, partition, region and application id — including each side of the
    // 32-hexadecimal application-id form, which is what Pinpoint mints.
    let hex32 = pinpoint_application_id(0x5eed);
    let rows: Vec<(&str, String, Resolution)> = vec![
        (
            "a Pinpoint application id, all zeroes",
            pinpoint_arn("aws", "us-east-1", &pinpoint_application_id(0)),
            Resolution::Active,
        ),
        (
            "a Pinpoint application id, every nibble set",
            pinpoint_arn("aws", "eu-west-1", &pinpoint_application_id(u128::MAX)),
            Resolution::Active,
        ),
        (
            "a Pinpoint application in another partition",
            pinpoint_arn("aws-us-gov", "us-gov-west-1", &hex32),
            Resolution::Active,
        ),
        (
            "an application id one character short",
            pinpoint_arn("aws", "us-east-1", &hex32[1..]),
            Resolution::Forbidden,
        ),
        (
            "an application id one character long",
            pinpoint_arn("aws", "us-east-1", &format!("{hex32}a")),
            Resolution::Forbidden,
        ),
        (
            "an application id of the right length but not hexadecimal",
            pinpoint_arn("aws", "us-east-1", &format!("{}z", &hex32[1..])),
            Resolution::Forbidden,
        ),
        (
            "an application id in upper case",
            pinpoint_arn("aws", "us-east-1", &hex32.to_uppercase()),
            Resolution::Forbidden,
        ),
        (
            "an apps ARN naming no application",
            pinpoint_arn("aws", "us-east-1", ""),
            Resolution::Forbidden,
        ),
        (
            "a Pinpoint resource that is not an application",
            "arn:aws:mobiletargeting:us-east-1:123456789012:templates/push/welcome".to_string(),
            Resolution::Inactive,
        ),
        (
            "an SNS topic",
            "arn:aws:sns:us-east-1:123456789012:push-topic".to_string(),
            Resolution::Inactive,
        ),
        (
            "an S3 bucket, no region and no account",
            "arn:aws:s3:::push-bucket".to_string(),
            Resolution::Inactive,
        ),
        ("the endpoint's own user", user_arn.clone(), Resolution::Inactive),
    ];

    for endpoint_type in ENDPOINT_TYPES {
        for (index, (label, resource_arn, resolution)) in rows.iter().enumerate() {
            let case = format!("{endpoint_type}/{label}");
            let body = register_body(
                &format!("tok-st-{endpoint_type}-{index}"),
                endpoint_type,
                resource_arn,
                "device-token-state",
            );

            if *resolution == Resolution::Forbidden {
                let (status, answer) = register(&emulator, &user_arn, body).await;
                assert_eq!(
                    (status, answer["Code"].as_str(), answer["Message"].as_str()),
                    (
                        403,
                        Some("Forbidden"),
                        Some("You are not authorized to perform this action.")
                    ),
                    "case {case:?}: expected ResourceArn {resource_arn:?} to be refused the \
                     modelled forbidden, got {status} {answer:?}"
                );
                continue;
            }

            // The other rows REGISTER: a resource ARN with no Pinpoint application
            // behind it leaves an inactive endpoint, it does not refuse the call.
            let minted_id = endpoint_id(&emulator, &user_arn, body).await;
            let (status, described) = describe_endpoint(&emulator, &user_arn, &minted_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the registered endpoint to describe, got \
                 {status} {described:?}"
            );
            let state = &described["AppInstanceUserEndpoint"]["EndpointState"];
            let expected = if *resolution == Resolution::Active {
                (Some("ACTIVE"), None)
            } else {
                (Some("INACTIVE"), Some("INVALID_PINPOINT_ARN"))
            };
            assert_eq!(
                (state["Status"].as_str(), state["StatusReason"].as_str()),
                expected,
                "case {case:?}: expected EndpointState {expected:?} for ResourceArn \
                 {resource_arn:?}, got {state:?}"
            );
        }
    }
}

/// RULE: one `ClientRequestToken` registers exactly one endpoint — every repeat of that
/// token, however many and however concurrent, answers with the id the first register
/// minted, while a different token registers a different endpoint.
#[tokio::test]
async fn one_client_request_token_registers_one_endpoint() {
    let emulator = Emulator::new();
    let user_arn = user(&emulator, "token").await;
    let resource_arn = resolvable_pinpoint_arn(7);
    let body = || register_body("tok-idem-shared", "APNS", &resource_arn, "device-token-idem");

    // Rows: how many times the same token is repeated in sequence.
    let first = endpoint_id(&emulator, &user_arn, body()).await;
    for repeat in 1..=3 {
        let replayed = endpoint_id(&emulator, &user_arn, body()).await;
        assert_eq!(
            replayed, first,
            "case repeat {repeat}: expected the token to replay {first:?}, got {replayed:?}"
        );
    }

    // Eight registers of the one token in flight together: the recorded contention case.
    // Every one of them has to answer with that same single endpoint.
    let concurrent = tokio::join!(
        endpoint_id(&emulator, &user_arn, body()),
        endpoint_id(&emulator, &user_arn, body()),
        endpoint_id(&emulator, &user_arn, body()),
        endpoint_id(&emulator, &user_arn, body()),
        endpoint_id(&emulator, &user_arn, body()),
        endpoint_id(&emulator, &user_arn, body()),
        endpoint_id(&emulator, &user_arn, body()),
        endpoint_id(&emulator, &user_arn, body()),
    );
    let distinct: BTreeSet<&String> = [
        &concurrent.0,
        &concurrent.1,
        &concurrent.2,
        &concurrent.3,
        &concurrent.4,
        &concurrent.5,
        &concurrent.6,
        &concurrent.7,
    ]
    .into_iter()
    .collect();
    assert_eq!(
        distinct,
        BTreeSet::from([&first]),
        "case 8 concurrent registers of one token: expected every call to answer with \
         {first:?}, got {distinct:?}"
    );

    // Rows: distinct tokens, same parameters — each registers an endpoint of its own.
    let mut minted = BTreeSet::from([first.clone()]);
    for other in ["tok-idem-second", "tok-idem-third"] {
        let mut other_body = body();
        other_body["ClientRequestToken"] = json!(other);
        let minted_id = endpoint_id(&emulator, &user_arn, other_body).await;
        assert!(
            minted.insert(minted_id.clone()),
            "case token {other:?}: expected a second endpoint, got the replayed {minted_id:?}"
        );
    }

    // The token is released with the endpoint it minted, so once that endpoint is
    // deregistered the same token registers a new one rather than replaying a gone id.
    let (status, answer) = deregister_endpoint(&emulator, &user_arn, &first).await;
    assert_eq!(
        status, 204,
        "expected the deregister to answer 204, got {status} {answer:?}"
    );
    let after_deregister = endpoint_id(&emulator, &user_arn, body()).await;
    assert_ne!(
        after_deregister, first,
        "case \"the token's endpoint was deregistered\": expected a newly registered \
         endpoint, got the gone {first:?}"
    );
}

/// RULE: a `ClientRequestToken` stands for ONE request, so presenting it again with any
/// different parameter is refused `ConflictException` (HTTP 409) and leaves the endpoint
/// that token already registered exactly as it was.
#[tokio::test]
async fn a_reused_token_carrying_different_parameters_is_refused() {
    let emulator = Emulator::new();
    let user_arn = user(&emulator, "mismatch").await;
    let resource_arn = resolvable_pinpoint_arn(11);
    let token = "tok-mismatch";
    let original = || {
        let mut body = register_body(token, "APNS", &resource_arn, "device-token-original");
        body["Name"] = json!("original");
        body["AllowMessages"] = json!("ALL");
        body["EndpointAttributes"]["VoipDeviceToken"] = json!("voip-original");
        body
    };
    let first = endpoint_id(&emulator, &user_arn, original()).await;

    // Rows: the same token with exactly one member changed — one row per member a
    // register persists.
    let rows: Vec<MismatchRow<'_>> = vec![
        (
            "a different Type",
            Box::new(|body: &mut Value| body["Type"] = json!("GCM")),
        ),
        (
            "a different ResourceArn",
            Box::new(|body: &mut Value| body["ResourceArn"] = json!(resolvable_pinpoint_arn(12))),
        ),
        (
            "a different DeviceToken",
            Box::new(|body: &mut Value| {
                body["EndpointAttributes"]["DeviceToken"] = json!("device-token-other");
            }),
        ),
        (
            "a different VoipDeviceToken",
            Box::new(|body: &mut Value| {
                body["EndpointAttributes"]["VoipDeviceToken"] = json!("voip-other");
            }),
        ),
        (
            "a different Name",
            Box::new(|body: &mut Value| body["Name"] = json!("renamed")),
        ),
        (
            "no Name where one was registered",
            Box::new(|body: &mut Value| {
                body.as_object_mut()
                    .expect("a JSON object body")
                    .remove("Name");
            }),
        ),
        (
            "a different AllowMessages",
            Box::new(|body: &mut Value| body["AllowMessages"] = json!("NONE")),
        ),
    ];

    for (label, change) in rows {
        let mut body = original();
        change(&mut body);
        let (status, answer) = register(&emulator, &user_arn, body).await;
        assert_eq!(
            status, 409,
            "case {label:?}: expected the reused token to be refused 409, got {status} {answer:?}"
        );
        assert_eq!(
            (answer["Code"].as_str(), answer["Message"].as_str()),
            (
                Some("Conflict"),
                Some(
                    "The provided ClientRequestToken is already associated with an existing \
                     resource."
                )
            ),
            "case {label:?}: expected the modelled conflict refusal, got {answer:?}"
        );

        // The endpoint the token did register is untouched by the refusal.
        let (status, described) = describe_endpoint(&emulator, &user_arn, &first).await;
        let endpoint = &described["AppInstanceUserEndpoint"];
        assert_eq!(
            (
                status,
                endpoint["Type"].as_str(),
                endpoint["Name"].as_str(),
                endpoint["AllowMessages"].as_str(),
                endpoint["EndpointAttributes"]["DeviceToken"].as_str(),
            ),
            (
                200,
                Some("APNS"),
                Some("original"),
                Some("ALL"),
                Some("device-token-original"),
            ),
            "case {label:?}: expected the registered endpoint to survive the refusal \
             unchanged, got {status} {described:?}"
        );
    }
}

/// RULE: an update writes the name and the opt-in and nothing else — a member the caller
/// leaves out keeps the value the endpoint already carries, the type, resource ARN,
/// attributes, created instant and state are never touched, and every update re-stamps
/// the last-updated instant.
#[tokio::test]
async fn an_update_writes_only_the_name_and_the_opt_in() {
    let emulator = Emulator::new();
    let user_arn = user(&emulator, "update").await;
    let resource_arn = resolvable_pinpoint_arn(21);

    // Rows: (label, Name sent, AllowMessages sent) — each member absent, and each at
    // every value the model admits.
    let rows: Vec<(&str, Option<&str>, Option<&str>)> = vec![
        ("neither member", None, None),
        ("the name alone", Some("renamed"), None),
        ("the opt-in alone", None, Some("NONE")),
        ("an empty name", Some(""), Some("ALL")),
        ("both members", Some("both"), Some("NONE")),
        ("a non-ASCII name", Some("名前"), Some("ALL")),
    ];

    for (index, (label, name, allow)) in rows.iter().enumerate() {
        let minted_id = endpoint_id(
            &emulator,
            &user_arn,
            {
                let mut body = register_body(
                    &format!("tok-upd-ep-{index}"),
                    "GCM",
                    &resource_arn,
                    "device-token-update",
                );
                body["Name"] = json!("registered");
                body["AllowMessages"] = json!("ALL");
                body
            },
        )
        .await;
        let (_, before) = describe_endpoint(&emulator, &user_arn, &minted_id).await;
        let before = &before["AppInstanceUserEndpoint"];

        let mut body = json!({});
        if let Some(name) = name {
            body["Name"] = json!(name);
        }
        if let Some(allow) = allow {
            body["AllowMessages"] = json!(allow);
        }
        let (status, answer) = update_endpoint(&emulator, &user_arn, &minted_id, body).await;
        assert_eq!(
            (
                status,
                answer["AppInstanceUserArn"].as_str(),
                answer["EndpointId"].as_str()
            ),
            (200, Some(user_arn.as_str()), Some(minted_id.as_str())),
            "case {label:?}: expected the update to answer with the endpoint it wrote, got \
             {status} {answer:?}"
        );

        let (status, after) = describe_endpoint(&emulator, &user_arn, &minted_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the updated endpoint to describe, got {status} {after:?}"
        );
        let after = &after["AppInstanceUserEndpoint"];
        assert_eq!(
            after["Name"].as_str(),
            Some(name.unwrap_or("registered")),
            "case {label:?}: expected Name {:?} after the update, got {:?}",
            name.unwrap_or("registered"),
            after["Name"]
        );
        assert_eq!(
            after["AllowMessages"].as_str(),
            Some(allow.unwrap_or("ALL")),
            "case {label:?}: expected AllowMessages {:?} after the update, got {:?}",
            allow.unwrap_or("ALL"),
            after["AllowMessages"]
        );
        assert_eq!(
            (
                after["Type"].as_str(),
                after["ResourceArn"].as_str(),
                after["EndpointAttributes"]["DeviceToken"].as_str(),
                after["EndpointState"]["Status"].as_str(),
                after["CreatedTimestamp"].as_f64(),
            ),
            (
                Some("GCM"),
                Some(resource_arn.as_str()),
                Some("device-token-update"),
                Some("ACTIVE"),
                before["CreatedTimestamp"].as_f64(),
            ),
            "case {label:?}: an update must leave everything but the name and the opt-in \
             alone, got {after:?}"
        );
        let stamped = after["LastUpdatedTimestamp"].as_f64();
        assert!(
            stamped.is_some_and(|at| before["LastUpdatedTimestamp"]
                .as_f64()
                .is_some_and(|was| at >= was)),
            "case {label:?}: expected the update to re-stamp the last-updated instant at or \
             after {:?}, got {stamped:?}",
            before["LastUpdatedTimestamp"]
        );
    }
}

/// RULE: a deregister removes the endpoint, and every endpoint-keyed call addressed to
/// an endpoint its user does not hold — deregistered, never registered, or registered
/// under another user — is refused with the modelled `ForbiddenException`.
#[tokio::test]
async fn an_endpoint_is_reachable_only_while_its_user_holds_it() {
    let emulator = Emulator::new();
    let instance = emulator.app_instance("ep-scope", "tok-ep-scope").await;
    let owner = emulator.user(&instance, "owner", "tok-ep-scope-owner").await;
    let other = emulator.user(&instance, "other", "tok-ep-scope-other").await;
    let resource_arn = resolvable_pinpoint_arn(31);
    let minted_id = endpoint_id(
        &emulator,
        &owner,
        register_body(
            "tok-ep-scope-reg",
            "GCM",
            &resource_arn,
            "device-token-scope",
        ),
    )
    .await;

    // Rows: (label, the user ARN addressed, the EndpointId addressed) — each pair names
    // no endpoint the store holds, before any deregister.
    let rows: Vec<(&str, &str, String)> = vec![
        ("another user's ARN", other.as_str(), minted_id.clone()),
        (
            "an id nobody registered",
            owner.as_str(),
            format!("{minted_id}x"),
        ),
        (
            "a well-formed id nobody registered",
            owner.as_str(),
            "00000000-0000-4000-8000-000000000000".to_string(),
        ),
    ];
    for (label, user_arn, endpoint) in rows {
        // Every endpoint-keyed operation refuses the same way.
        let (status, answer) = describe_endpoint(&emulator, user_arn, &endpoint).await;
        assert_eq!(
            (status, answer["Code"].as_str(), answer["Message"].as_str()),
            (403, Some("Forbidden"), Some(FORBIDDEN_MESSAGE)),
            "case {label:?}/Describe: expected the modelled forbidden refusal, got \
             {status} {answer:?}"
        );
        let (status, answer) =
            update_endpoint(&emulator, user_arn, &endpoint, json!({ "Name": "ghost" })).await;
        assert_eq!(
            (status, answer["Code"].as_str(), answer["Message"].as_str()),
            (403, Some("Forbidden"), Some(FORBIDDEN_MESSAGE)),
            "case {label:?}/Update: expected the modelled forbidden refusal, got \
             {status} {answer:?}"
        );
        let (status, answer) = deregister_endpoint(&emulator, user_arn, &endpoint).await;
        assert_eq!(
            (status, answer["Code"].as_str(), answer["Message"].as_str()),
            (403, Some("Forbidden"), Some(FORBIDDEN_MESSAGE)),
            "case {label:?}/Deregister: expected the modelled forbidden refusal, got \
             {status} {answer:?}"
        );
    }

    // A register addressed to a user ARN the service does not hold mints nothing.
    let (status, answer) = register(
        &emulator,
        &format!("{owner}x"),
        register_body(
            "tok-ep-scope-absent",
            "APNS",
            &resource_arn,
            "device-token-absent",
        ),
    )
    .await;
    assert_eq!(
        (status, answer["Code"].as_str()),
        (400, Some("NotFound")),
        "case \"a user that never existed\": expected the register to be refused, got \
         {status} {answer:?}"
    );

    // The deregister removes the endpoint: it is gone from the listing, and every call
    // addressed to it is refused the same way as one that never existed.
    let (status, answer) = deregister_endpoint(&emulator, &owner, &minted_id).await;
    assert_eq!(
        status, 204,
        "expected the deregister to answer 204, got {status} {answer:?}"
    );
    let (status, listed) = list_endpoints(&emulator, &owner, "").await;
    assert_eq!(
        (status, listed["AppInstanceUserEndpoints"].as_array()),
        (200, Some(&vec![])),
        "case \"after the deregister\": expected the user to hold no endpoint, got \
         {status} {listed:?}"
    );
    let (status, answer) = describe_endpoint(&emulator, &owner, &minted_id).await;
    assert_eq!(
        (status, answer["Code"].as_str(), answer["Message"].as_str()),
        (403, Some("Forbidden"), Some(FORBIDDEN_MESSAGE)),
        "case \"the endpoint was deregistered\": expected the modelled forbidden refusal, \
         got {status} {answer:?}"
    );

    // Deleting the owning user takes its endpoints with it, and so does deleting the
    // app instance above it.
    let doomed = endpoint_id(
        &emulator,
        &owner,
        register_body(
            "tok-ep-scope-doomed",
            "APNS",
            &resource_arn,
            "device-token-doomed",
        ),
    )
    .await;
    let (status, answer) = emulator
        .call(
            "DELETE",
            &format!("/app-instance-users/{}", uri_encode(&owner)),
            None,
        )
        .await;
    assert_eq!(
        status, 204,
        "expected DeleteAppInstanceUser to answer 204, got {status} {answer:?}"
    );
    let (status, answer) = describe_endpoint(&emulator, &owner, &doomed).await;
    assert_eq!(
        (status, answer["Code"].as_str()),
        (403, Some("Forbidden")),
        "case \"the owning user was deleted\": expected the endpoint to be gone, got \
         {status} {answer:?}"
    );

    let survivor = endpoint_id(
        &emulator,
        &other,
        register_body(
            "tok-ep-scope-survivor",
            "APNS",
            &resource_arn,
            "device-token-survivor",
        ),
    )
    .await;
    let (status, answer) = emulator
        .call(
            "DELETE",
            &format!("/app-instances/{}", uri_encode(&instance)),
            None,
        )
        .await;
    assert_eq!(
        status, 204,
        "expected DeleteAppInstance to answer 204, got {status} {answer:?}"
    );
    let (status, answer) = describe_endpoint(&emulator, &other, &survivor).await;
    assert_eq!(
        (status, answer["Code"].as_str()),
        (403, Some("Forbidden")),
        "case \"the app instance was deleted\": expected the endpoint to be gone, got \
         {status} {answer:?}"
    );
}

/// RULE: `ListAppInstanceUserEndpoints` reports the endpoints PERSISTED under the ONE
/// user it was asked about — their id, name, type and opt-in, and the state the service
/// computed — in a stable key order, paged at `MaxResults` with a `NextToken` that
/// resumes the walk and is absent on the last page.
#[tokio::test]
async fn a_listing_pages_over_the_endpoints_one_user_holds() {
    let emulator = Emulator::new();
    let instance = emulator.app_instance("ep-list", "tok-ep-list").await;
    let listed_user = emulator.user(&instance, "listed", "tok-ep-list-user").await;
    let bystander = emulator
        .user(&instance, "bystander", "tok-ep-list-other")
        .await;
    // A second user's endpoint, which no page of the first user's listing may report.
    let intruder = endpoint_id(
        &emulator,
        &bystander,
        register_body(
            "tok-ep-list-intruder",
            "APNS",
            &resolvable_pinpoint_arn(41),
            "device-token-intruder",
        ),
    )
    .await;

    // Rows: how many endpoints the user holds against the page size asked for — none,
    // one, fewer than a page, exactly a page, more than a page, and both ends of the
    // model's `MaxResults` range (min 1, max 50).
    let rows: &[(usize, Option<i32>)] = &[
        (0, None),
        (1, None),
        (1, Some(1)),
        (3, Some(1)),
        (3, Some(3)),
        (5, Some(2)),
        (4, Some(50)),
        (2, None),
    ];

    for (count, max_results) in rows.iter().copied() {
        let case = format!("{count} endpoint(s), MaxResults {max_results:?}");
        // A fresh user per row, so each row's collection is exactly the one it built.
        let user_arn = emulator
            .user(
                &instance,
                &format!("listed{count}x{}", max_results.unwrap_or(0)),
                &format!("tok-ep-list-{count}-{}", max_results.unwrap_or(0)),
            )
            .await;
        let mut registered: BTreeSet<String> = BTreeSet::new();
        for index in 0..count {
            let mut body = register_body(
                &format!("tok-ep-list-{count}-{}-{index}", max_results.unwrap_or(0)),
                ENDPOINT_TYPES[index % ENDPOINT_TYPES.len()],
                &resolvable_pinpoint_arn(100 + index as u128),
                &format!("device-token-{index}"),
            );
            body["Name"] = json!(format!("endpoint-{index}"));
            body["AllowMessages"] = json!(if index % 2 == 0 { "ALL" } else { "NONE" });
            registered.insert(endpoint_id(&emulator, &user_arn, body).await);
        }

        // Walk every page, following the token the listing issues.
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = String::new();
            if let Some(max) = max_results {
                query.push_str(&format!("?max-results={max}"));
            }
            if let Some(token) = next_token.as_deref() {
                query.push_str(if query.is_empty() { "?" } else { "&" });
                query.push_str(&format!("next-token={}", uri_encode(token)));
            }
            let (status, body) = list_endpoints(&emulator, &user_arn, &query).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the listing to answer 200, got {status} {body:?}"
            );
            let page = body["AppInstanceUserEndpoints"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: the listing reported no list: {body:?}"));
            if let Some(max) = max_results {
                assert!(
                    page.len() <= max as usize,
                    "case {case:?}: expected at most {max} summaries on a page, got {}",
                    page.len()
                );
            }
            for summary in page {
                assert_eq!(
                    summary["AppInstanceUserArn"].as_str(),
                    Some(user_arn.as_str()),
                    "case {case:?}: expected every summary to belong to {user_arn:?}, got \
                     {summary:?}"
                );
                let id = summary["EndpointId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: a summary with no id: {summary:?}"));
                assert_ne!(
                    id, intruder,
                    "case {case:?}: another user's endpoint was reported: {summary:?}"
                );
                assert!(
                    summary["Type"].as_str().is_some_and(|kind| ENDPOINT_TYPES
                        .contains(&kind))
                        && summary["Name"].as_str().is_some()
                        && summary["EndpointState"]["Status"].as_str() == Some("ACTIVE"),
                    "case {case:?}: expected the summary to carry the registered type, name \
                     and computed state, got {summary:?}"
                );
                seen.push(id.to_string());
            }
            pages += 1;
            next_token = body["NextToken"].as_str().map(str::to_string);
            if next_token.is_none() {
                break;
            }
            assert!(
                pages <= count + 1,
                "case {case:?}: the listing kept issuing tokens after {pages} page(s)"
            );
        }

        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(
            seen, sorted,
            "case {case:?}: expected the walk to report endpoints in key order, got {seen:?}"
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<String>>(),
            registered,
            "case {case:?}: expected the walk to report exactly the registered endpoints"
        );
        assert_eq!(
            seen.len(),
            count,
            "case {case:?}: expected {count} summaries over the whole walk, got {}",
            seen.len()
        );
    }

    // A `NextToken` this listing never issued is a bad request, not an empty page.
    let (status, answer) = list_endpoints(&emulator, &listed_user, "?next-token=deadbeef").await;
    assert_eq!(
        (status, answer["Code"].as_str()),
        (400, Some("BadRequest")),
        "case \"a token this listing never issued\": expected a refusal, got {status} {answer:?}"
    );
    // A listing addressed to a user the service does not hold reports nothing.
    let (status, answer) = list_endpoints(&emulator, &format!("{listed_user}x"), "").await;
    assert_eq!(
        (status, answer["Code"].as_str()),
        (400, Some("NotFound")),
        "case \"a user that never existed\": expected a refusal, got {status} {answer:?}"
    );
}

/// RULE: the store holds at most [`MAX_APP_INSTANCE_USER_ENDPOINTS`] endpoints under one
/// user; a register past that bound stores nothing and is refused with the modelled
/// `ResourceLimitExceededException`, while the endpoints already registered stay
/// readable and another user is unaffected by its neighbour being full.
#[tokio::test]
async fn a_register_past_the_endpoint_bound_is_refused() {
    let emulator = Emulator::new();
    let instance = emulator.app_instance("ep-bound", "tok-ep-bound").await;
    let full = emulator.user(&instance, "full", "tok-ep-bound-full").await;
    let roomy = emulator.user(&instance, "roomy", "tok-ep-bound-roomy").await;

    let mut registered: Vec<String> = Vec::new();
    for index in 0..MAX_APP_INSTANCE_USER_ENDPOINTS {
        registered.push(
            endpoint_id(
                &emulator,
                &full,
                register_body(
                    &format!("tok-ep-bound-{index}"),
                    ENDPOINT_TYPES[index % ENDPOINT_TYPES.len()],
                    &resolvable_pinpoint_arn(1_000 + index as u128),
                    &format!("device-token-bound-{index}"),
                ),
            )
            .await,
        );
    }

    // Rows: the first register past the bound, and a second one with another token —
    // the bound is not a one-off refusal.
    for over in 0..2 {
        let (status, answer) = register(
            &emulator,
            &full,
            register_body(
                &format!("tok-ep-over-{over}"),
                "APNS",
                &resolvable_pinpoint_arn(2_000 + over),
                "device-token-over",
            ),
        )
        .await;
        assert_eq!(
            (status, answer["Code"].as_str()),
            (400, Some("ResourceLimitExceeded")),
            "case \"register {over} past the bound\": expected the modelled limit refusal, \
             got {status} {answer:?}"
        );
    }

    // Nothing was stored past the bound, and everything below it is still readable.
    let (status, listed) = list_endpoints(&emulator, &full, "?max-results=50").await;
    assert_eq!(
        status, 200,
        "expected the full user's listing to answer 200, got {status} {listed:?}"
    );
    let first_page = listed["AppInstanceUserEndpoints"]
        .as_array()
        .unwrap_or_else(|| panic!("the listing reported no list: {listed:?}"));
    assert_eq!(
        first_page.len(),
        50,
        "expected a full page of 50 summaries, got {}",
        first_page.len()
    );
    let (status, described) = describe_endpoint(&emulator, &full, &registered[0]).await;
    assert_eq!(
        (status, described["AppInstanceUserEndpoint"]["EndpointId"].as_str()),
        (200, Some(registered[0].as_str())),
        "expected the first registered endpoint to survive the refusals, got \
         {status} {described:?}"
    );

    // A user whose neighbour is full still has room of its own.
    let neighbour = endpoint_id(
        &emulator,
        &roomy,
        register_body(
            "tok-ep-bound-neighbour",
            "GCM",
            &resolvable_pinpoint_arn(3_000),
            "device-token-neighbour",
        ),
    )
    .await;
    let (status, described) = describe_endpoint(&emulator, &roomy, &neighbour).await;
    assert_eq!(
        status, 200,
        "expected the neighbouring user's endpoint to register and describe, got \
         {status} {described:?}"
    );
}
