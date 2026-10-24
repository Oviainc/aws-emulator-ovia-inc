// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Kinesis Video Signaling data plane.
//!
//! Both operations are driven over the wire, through the same `MultiServiceRouter`
//! `core/router` builds, so the assertions cover routing, deserialization, boundary
//! validation, the handler and serialization — not just the handler body.
//!
//! The rule under test: every answer is a function of the signaling-channel registry in
//! `crate::state` plus the request. A channel in the registry is served; one that is not
//! is `ResourceNotFoundException`; a `ChannelARN` that is not a signaling-channel ARN is
//! `InvalidArgumentException`. The tables below span channels (region, account, name,
//! creation id), clients, TTLs and payloads; the ARN from the issue report is one row.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use engine::InMemoryEngine;
use service_com_amazonaws_kinesisvideosignaling_awsacuitysignalingservice as signaling;
use signaling::state::{
    ComAmazonawsKinesisvideosignalingAwsacuitysignalingserviceState as SignalingState,
    IceServerRecord, SignalingChannel, MAX_ICE_CREDENTIAL_TTL_SECS,
    MAX_ICE_ISSUANCES_PER_CHANNEL, MAX_OFFERS_PER_CHANNEL, MIN_ICE_CREDENTIAL_TTL_SECS,
};
use smithy_server_runtime::MultiServiceRouter;

const GET_ICE_SERVER_CONFIG: &str = "/v1/get-ice-server-config";
const SEND_ALEXA_OFFER_TO_MASTER: &str = "/v1/send-alexa-offer-to-master";

/// The emulator a test drives: the shared engine, and a router carrying this one service.
fn emulator() -> (Arc<InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(signaling::EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// A request addressed to this service the way an AWS SDK addresses it: the SigV4
/// credential scope names the `kinesisvideo` signing name, and the path names the route.
fn request(path: &str, body: &str) -> http::Request<Bytes> {
    http::Request::builder()
        .method("POST")
        .uri(format!("https://kinesisvideo.us-east-1.amazonaws.com{path}"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/kinesisvideo/aws4_request, \
             SignedHeaders=host, Signature=00",
        )
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed")
}

/// One call: the HTTP status and the parsed JSON body.
async fn call(router: &MultiServiceRouter, path: &str, body: &str) -> (u16, serde_json::Value) {
    let response = router.dispatch(request(path, body)).await;
    let status = response.status().as_u16();
    let errortype = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body();
    let mut parsed: serde_json::Value = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "response body is not JSON ({error}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    // restJson1 carries the error shape in `__type`; the boundary path also sets the
    // `x-amzn-errortype` header. Fold the header in so one accessor reads both.
    if let (Some(errortype), Some(object)) = (errortype, parsed.as_object_mut()) {
        object
            .entry("__type".to_string())
            .or_insert(serde_json::Value::String(errortype));
    }
    (status, parsed)
}

/// The modelled error shape a failure answered with.
fn error_code(body: &serde_json::Value) -> String {
    body.get("__type")
        .and_then(serde_json::Value::as_str)
        .map(|code| code.rsplit(['#', '/']).next().unwrap_or(code).to_string())
        .unwrap_or_else(|| format!("no __type in {body}"))
}

/// Put a channel in the registry, the way `CreateSignalingChannel` will once the control
/// plane is implemented, and let `prepare` set what that channel holds.
fn seed_channel(
    engine: &InMemoryEngine,
    arn: &str,
    prepare: impl FnOnce(&mut SignalingChannel),
) {
    let mut channel = SignalingChannel::from_arn(arn)
        .unwrap_or_else(|| panic!("{arn} must parse as a signaling channel ARN"));
    prepare(&mut channel);
    let registered = engine
        .state::<SignalingState>()
        .lock()
        .register_channel(channel);
    assert!(
        registered,
        "case {arn:?}: the registry must accept the seeded channel"
    );
}

/// Base64, the encoding `MessagePayload` and `Answer` carry SDP in: the SDK's own
/// standard-alphabet codec (`aws_smithy_types::base64`), the one the wire serializer uses.
fn b64(text: &str) -> String {
    aws_smithy_types::base64::encode(text)
}

/// The modelled `IceServer$Username` / `IceServer$Password` constraints, checked by hand
/// because this test target has no regex crate: `@pattern ^[a-zA-Z0-9_.-]+$` with
/// `@length 1..256` (`smithy_query(query="shape", shape="IceServer")`).
fn is_modelled_ice_token(value: &str) -> bool {
    // `@length` counts CHARACTERS, so the bound is checked on `chars().count()` rather
    // than on the byte length, which would reject a long multi-byte value for the wrong
    // reason (the pattern check below already refuses any non-ASCII character).
    !value.is_empty()
        && value.chars().count() <= 256
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// One `GetIceServerConfig` row: a channel, how it is configured, and who asks.
struct IceCase {
    case: &'static str,
    arn: &'static str,
    region: &'static str,
    /// What the channel record holds; `None` leaves the registry default in place.
    channel_ttl_secs: Option<i32>,
    client_id: Option<&'static str>,
    username: Option<&'static str>,
    service: Option<&'static str>,
    /// The `Ttl` the answer must carry.
    expected_ttl: i32,
    /// What the issued username must identify the credentials to.
    expected_label: &'static str,
}

fn ice_request_body(case: &IceCase) -> String {
    let mut body = serde_json::Map::new();
    body.insert("ChannelARN".into(), case.arn.into());
    if let Some(client_id) = case.client_id {
        body.insert("ClientId".into(), client_id.into());
    }
    if let Some(username) = case.username {
        body.insert("Username".into(), username.into());
    }
    if let Some(service) = case.service {
        body.insert("Service".into(), service.into());
    }
    serde_json::Value::Object(body).to_string()
}

/// The rule: a channel in the registry is answered with one TURN credential set, whose
/// relay URIs are in the channel's own region, whose `Ttl` is the channel's lifetime held
/// inside the modelled 30..86400 range, and which is RECORDED — so the same client asking
/// again inside that lifetime is answered with the very same credentials.
#[tokio::test]
async fn get_ice_server_config_issues_recorded_turn_credentials_per_channel_and_client() {
    // The rows differ along what the rule generalises over: region, account, channel name
    // and creation id, whether a `ClientId` / `Username` / `Service` is named, and the
    // channel's credential lifetime — including each published bound of the modelled
    // `IceServer$Ttl` range and one value past each.
    let cases = [
        IceCase {
            case: "the channel from the issue report, with a viewer client id",
            arn: "arn:aws:kinesisvideo:us-east-1:123456789012:channel/demo-channel/1700000000",
            region: "us-east-1",
            channel_ttl_secs: None,
            client_id: Some("viewer-1"),
            username: None,
            service: Some("TURN"),
            expected_ttl: 300,
            expected_label: "viewer-1",
        },
        IceCase {
            case: "another region and account, naming an optional Username",
            arn: "arn:aws:kinesisvideo:eu-west-2:000000000001:channel/alexa.channel_2/1",
            region: "eu-west-2",
            channel_ttl_secs: Some(MIN_ICE_CREDENTIAL_TTL_SECS),
            client_id: Some("viewer_2"),
            username: Some("operator.one"),
            service: None,
            expected_ttl: MIN_ICE_CREDENTIAL_TTL_SECS,
            expected_label: "operator.one",
        },
        IceCase {
            case: "no ClientId: the credentials are the channel's own",
            arn: "arn:aws:kinesisvideo:ap-southeast-1:999999999999:channel/ch-3/42",
            region: "ap-southeast-1",
            channel_ttl_secs: Some(MAX_ICE_CREDENTIAL_TTL_SECS),
            client_id: None,
            username: None,
            service: None,
            expected_ttl: MAX_ICE_CREDENTIAL_TTL_SECS,
            expected_label: "ch-3",
        },
        IceCase {
            case: "a lifetime one second under the modelled minimum is held at it",
            arn: "arn:aws:kinesisvideo:us-west-2:123456789012:channel/short-ttl/1700000001",
            region: "us-west-2",
            channel_ttl_secs: Some(MIN_ICE_CREDENTIAL_TTL_SECS - 1),
            client_id: Some("viewer-3"),
            username: None,
            service: Some("TURN"),
            expected_ttl: MIN_ICE_CREDENTIAL_TTL_SECS,
            expected_label: "viewer-3",
        },
        IceCase {
            case: "a lifetime one second over the modelled maximum is held at it",
            arn: "arn:aws:kinesisvideo:sa-east-1:123456789012:channel/long-ttl/1700000002",
            region: "sa-east-1",
            channel_ttl_secs: Some(MAX_ICE_CREDENTIAL_TTL_SECS + 1),
            client_id: Some("viewer-4"),
            username: None,
            service: Some("TURN"),
            expected_ttl: MAX_ICE_CREDENTIAL_TTL_SECS,
            expected_label: "viewer-4",
        },
    ];

    let (engine, router) = emulator();
    let mut hosts: Vec<String> = Vec::new();

    for case in &cases {
        seed_channel(&engine, case.arn, |channel| {
            if let Some(ttl) = case.channel_ttl_secs {
                channel.ice_credential_ttl_secs = ttl;
            }
        });

        let body = ice_request_body(case);
        let (status, answer) = call(&router, GET_ICE_SERVER_CONFIG, &body).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with {answer}",
            case.case
        );

        let servers = answer
            .get("IceServerList")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("case {:?}: no IceServerList in {answer}", case.case));
        assert_eq!(
            servers.len(),
            1,
            "case {:?}: expected one ICE server, got {servers:?}",
            case.case
        );
        let server = &servers[0];

        let uris: Vec<&str> = server
            .get("Uris")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("case {:?}: no Uris in {server}", case.case))
            .iter()
            .map(|uri| {
                uri.as_str()
                    .unwrap_or_else(|| panic!("case {:?}: a Uri is not a string", case.case))
            })
            .collect();
        let host_suffix = format!(".kinesisvideo.{}.amazonaws.com:443", case.region);
        assert_eq!(
            uris.len(),
            3,
            "case {:?}: expected three relay URIs, got {uris:?}",
            case.case
        );
        for expected in [
            format!("turn:{{host}}{host_suffix}?transport=udp"),
            format!("turns:{{host}}{host_suffix}?transport=udp"),
            format!("turns:{{host}}{host_suffix}?transport=tcp"),
        ]
        .iter()
        .enumerate()
        {
            let (index, shape) = expected;
            let (scheme, transport) = shape
                .split_once("{host}")
                .map(|(scheme, rest)| {
                    (
                        scheme,
                        rest.rsplit_once('?').map(|(_, q)| q).unwrap_or_default(),
                    )
                })
                .expect("the expected shape names a host");
            let uri = uris[index];
            assert!(
                uri.starts_with(scheme)
                    && uri.contains(&host_suffix)
                    && uri.ends_with(transport),
                "case {:?}: URI {index} must be {scheme}<relay>{host_suffix}?{transport}, got {uri:?}",
                case.case
            );
        }
        let host = uris[0]
            .trim_start_matches("turn:")
            .split(':')
            .next()
            .expect("the URI names a host")
            .to_string();
        hosts.push(host);

        let ttl = server
            .get("Ttl")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or_else(|| panic!("case {:?}: no Ttl in {server}", case.case));
        assert_eq!(
            ttl,
            i64::from(case.expected_ttl),
            "case {:?}: expected Ttl {}, got {ttl}",
            case.case,
            case.expected_ttl
        );
        assert!(
            (i64::from(MIN_ICE_CREDENTIAL_TTL_SECS)..=i64::from(MAX_ICE_CREDENTIAL_TTL_SECS))
                .contains(&ttl),
            "case {:?}: Ttl {ttl} is outside the modelled 30..86400 range",
            case.case
        );

        let username = server
            .get("Username")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("case {:?}: no Username in {server}", case.case));
        let password = server
            .get("Password")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("case {:?}: no Password in {server}", case.case));
        assert!(
            is_modelled_ice_token(username),
            "case {:?}: Username {username:?} breaks the modelled IceServer$Username constraints",
            case.case
        );
        assert!(
            is_modelled_ice_token(password),
            "case {:?}: Password breaks the modelled IceServer$Password constraints \
             (length {})",
            case.case,
            password.len()
        );
        assert!(
            username.ends_with(case.expected_label),
            "case {:?}: Username {username:?} must identify {:?}",
            case.case,
            case.expected_label
        );
        let (expiry, _) = username
            .split_once('.')
            .unwrap_or_else(|| panic!("case {:?}: Username {username:?} has no expiry", case.case));
        let expiry: u64 = expiry
            .parse()
            .unwrap_or_else(|_| panic!("case {:?}: {expiry:?} is not an expiry instant", case.case));
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs();
        assert!(
            expiry > now && expiry <= now + u64::from(case.expected_ttl.unsigned_abs()),
            "case {:?}: Username expiry {expiry} must be within the issued Ttl of {now}",
            case.case
        );

        // The issuance is RECORDED: asking again inside the lifetime answers with the
        // same credentials rather than a freshly randomised set. The `Ttl` is the lifetime
        // the issuance has LEFT, so a repeat call reports the same or less — never more.
        let (repeat_status, repeat) = call(&router, GET_ICE_SERVER_CONFIG, &body).await;
        assert_eq!(
            repeat_status, 200,
            "case {:?}: a repeat call must succeed, got {repeat_status}",
            case.case
        );
        let repeated = &repeat["IceServerList"][0];
        // An issuance is re-served only while it still has at least the modelled minimum
        // `Ttl` left, because that remainder is what goes on the wire: a row whose WHOLE
        // lifetime is that minimum therefore stops being reusable within a second of being
        // minted, and the repeat call legitimately mints a fresh set. For those rows what
        // is pinned is that the call is still served inside the modelled range; the rows
        // with a longer lifetime must answer with the very same recorded credentials,
        // which is where the recording is proved. (Keyed on the row's lifetime, never on
        // the clock, so neither branch depends on how long the dispatch took.)
        let repeat_is_reusable = case.expected_ttl > MIN_ICE_CREDENTIAL_TTL_SECS + 1;
        if repeat_is_reusable {
            for member in ["Uris", "Username", "Password"] {
                assert_eq!(
                    repeated.get(member),
                    server.get(member),
                    "case {:?}: a repeat call must answer with the recorded {member}, got {repeat}",
                    case.case
                );
            }
        }
        let repeat_ttl = repeated
            .get("Ttl")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or_else(|| panic!("case {:?}: no Ttl in {repeated}", case.case));
        let repeat_ttl_floor = i64::from(MIN_ICE_CREDENTIAL_TTL_SECS);
        let repeat_ttl_ceiling = if repeat_is_reusable {
            // The remainder of the issuance already served: the same or less, never more.
            ttl
        } else {
            i64::from(MAX_ICE_CREDENTIAL_TTL_SECS)
        };
        assert!(
            (repeat_ttl_floor..=repeat_ttl_ceiling).contains(&repeat_ttl),
            "case {:?}: a repeat call must report a lifetime between {repeat_ttl_floor} and \
             {repeat_ttl_ceiling}, got {repeat_ttl}",
            case.case
        );

        // And the record really is in state, exactly one per (channel, client), holding
        // what the LAST call served (for a reusable issuance that is also what the first
        // call served, which the member comparison above already pinned).
        let served_username = repeated
            .get("Username")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("case {:?}: no Username in {repeated}", case.case));
        let served_password = repeated
            .get("Password")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("case {:?}: no Password in {repeated}", case.case));
        let state = engine.state::<SignalingState>();
        let store = state.lock();
        let channel = store
            .resolve_channel(case.arn)
            .unwrap_or_else(|error| panic!("case {:?}: {error:?}", case.case));
        let records: Vec<&IceServerRecord> = channel
            .ice_servers
            .iter()
            .filter(|record| record.client_id.as_deref() == case.client_id)
            .collect();
        assert_eq!(
            records.len(),
            1,
            "case {:?}: expected one recorded issuance for {:?}, got {}",
            case.case,
            case.client_id,
            records.len()
        );
        assert_eq!(
            (records[0].username.as_str(), records[0].password.as_str()),
            (served_username, served_password),
            "case {:?}: the answer must be read from the recorded issuance",
            case.case
        );
    }

    // Each channel is served by its own relay address, so two channels never share one.
    let mut unique = hosts.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        hosts.len(),
        "each channel must be served by its own relay address, got {hosts:?}"
    );
}

/// Two viewers of one channel hold two different credential sets, and each is answered
/// with its own.
#[tokio::test]
async fn get_ice_server_config_issues_one_credential_set_per_client() {
    const ARN: &str = "arn:aws:kinesisvideo:us-east-1:123456789012:channel/shared/1700000000";
    let (engine, router) = emulator();
    seed_channel(&engine, ARN, |_| {});

    let mut issued: Vec<(String, String)> = Vec::new();
    for client in ["viewer-a", "viewer-b", "viewer-c"] {
        let body = format!(r#"{{"ChannelARN":"{ARN}","ClientId":"{client}","Service":"TURN"}}"#);
        let (status, answer) = call(&router, GET_ICE_SERVER_CONFIG, &body).await;
        assert_eq!(status, 200, "case {client:?}: expected 200, got {answer}");
        let server = &answer["IceServerList"][0];
        issued.push((
            server["Username"].as_str().unwrap_or_default().to_string(),
            server["Password"].as_str().unwrap_or_default().to_string(),
        ));
    }

    for (left, right) in [(0, 1), (0, 2), (1, 2)] {
        assert_ne!(
            issued[left], issued[right],
            "two viewers must not share one credential set: {:?} vs {:?}",
            issued[left], issued[right]
        );
    }

    let state = engine.state::<SignalingState>();
    let store = state.lock();
    let channel = store.resolve_channel(ARN).expect("the channel is registered");
    assert_eq!(
        channel.ice_servers.len(),
        3,
        "expected one recorded issuance per viewer, got {:?}",
        channel.ice_servers
    );
}

/// An issuance the clock has expired is reissued rather than returned: the record is
/// replaced, and the caller gets credentials that are valid now.
#[tokio::test]
async fn get_ice_server_config_reissues_an_expired_credential_set() {
    const ARN: &str = "arn:aws:kinesisvideo:us-east-1:123456789012:channel/expiring/1700000000";
    let (engine, router) = emulator();
    let stale = IceServerRecord {
        uris: vec!["turn:stale.invalid:443?transport=udp".to_string()],
        username: "1.stale".to_string(),
        password: "stalepassword".to_string(),
        ttl_secs: 300,
        issued_at: SystemTime::now() - Duration::from_secs(3_600),
        client_id: Some("viewer-1".to_string()),
    };
    seed_channel(&engine, ARN, |channel| {
        channel.ice_servers.push(stale.clone());
    });

    let body = format!(r#"{{"ChannelARN":"{ARN}","ClientId":"viewer-1"}}"#);
    let (status, answer) = call(&router, GET_ICE_SERVER_CONFIG, &body).await;
    assert_eq!(status, 200, "expected 200, got {status} with {answer}");
    let server = &answer["IceServerList"][0];
    assert_ne!(
        server["Password"].as_str(),
        Some(stale.password.as_str()),
        "an expired issuance must be replaced, got {server}"
    );

    let state = engine.state::<SignalingState>();
    let store = state.lock();
    let channel = store.resolve_channel(ARN).expect("the channel is registered");
    assert_eq!(
        channel.ice_servers.len(),
        1,
        "the expired issuance must be replaced, not kept beside the new one: {:?}",
        channel.ice_servers
    );
    assert!(
        channel.ice_servers[0].is_valid_at(SystemTime::now()),
        "the reissued credential set must be valid now: {:?}",
        channel.ice_servers[0]
    );
}

/// A credential set already in the registry is reported with the lifetime it is actually
/// honoured for: `Ttl` is the lifetime the record has LEFT, held inside the modelled
/// `IceServer$Ttl` range (30..86400), so a record seeded outside that range can neither
/// put an out-of-range `Ttl` on the wire nor report a lifetime that differs from the one
/// the registry enforces.
#[tokio::test]
async fn get_ice_server_config_reports_a_seeded_record_inside_the_modelled_ttl_range() {
    // The rows span a record seeded under the modelled minimum, over the maximum, and at
    // each published bound; the record is put straight into the registry (as the control
    // plane seeding a channel would), not minted through the request path.
    let cases = [
        ("under the modelled minimum", 1_i32, MIN_ICE_CREDENTIAL_TTL_SECS),
        (
            "at the modelled minimum",
            MIN_ICE_CREDENTIAL_TTL_SECS,
            MIN_ICE_CREDENTIAL_TTL_SECS,
        ),
        (
            "at the modelled maximum",
            MAX_ICE_CREDENTIAL_TTL_SECS,
            MAX_ICE_CREDENTIAL_TTL_SECS,
        ),
        (
            "over the modelled maximum",
            MAX_ICE_CREDENTIAL_TTL_SECS + 1,
            MAX_ICE_CREDENTIAL_TTL_SECS,
        ),
        ("negative, which is no lifetime at all", -7, MIN_ICE_CREDENTIAL_TTL_SECS),
    ];

    for (case, seeded_ttl_secs, expected_ttl) in cases {
        let arn = format!(
            "arn:aws:kinesisvideo:us-east-1:123456789012:channel/seeded-{}/1700000000",
            seeded_ttl_secs.unsigned_abs()
        );
        let (engine, router) = emulator();
        let seeded = IceServerRecord {
            uris: vec!["turn:seeded.invalid:443?transport=udp".to_string()],
            username: "1700000000.seeded".to_string(),
            password: "seededpassword".to_string(),
            ttl_secs: seeded_ttl_secs,
            issued_at: SystemTime::now(),
            client_id: Some("viewer-1".to_string()),
        };
        seed_channel(&engine, &arn, |channel| {
            channel.ice_servers.push(seeded.clone());
        });

        let body = format!(r#"{{"ChannelARN":"{arn}","ClientId":"viewer-1"}}"#);
        let (status, answer) = call(&router, GET_ICE_SERVER_CONFIG, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {answer}"
        );
        let server = &answer["IceServerList"][0];
        let ttl = server
            .get("Ttl")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or_else(|| panic!("case {case:?}: no Ttl in {server}"));
        assert_eq!(
            ttl,
            i64::from(expected_ttl),
            "case {case:?}: expected Ttl {expected_ttl}, got {ttl}"
        );

        // It is still the seeded issuance that was reported, not a fresh mint: the record
        // is valid under the lifetime the registry enforces, so it is returned as held.
        assert_eq!(
            server["Password"].as_str(),
            Some(seeded.password.as_str()),
            "case {case:?}: the seeded issuance must be the one reported, got {server}"
        );
        assert_eq!(
            server["Username"].as_str(),
            Some(seeded.username.as_str()),
            "case {case:?}: the seeded issuance must be the one reported, got {server}"
        );
    }
}

/// RULE: the reported `Ttl` is the lifetime the issuance has LEFT, not the lifetime it
/// was minted with — a viewer must never be told credentials are good for longer than the
/// relay will honour them — and an issuance with less than the modelled minimum left is
/// reissued rather than re-served (reporting under 30 would break `IceServer$Ttl`'s
/// range).
///
/// The clock is advanced by seeding `issued_at` in the past, which is the same thing from
/// the handler's side and needs no sleep.
#[tokio::test]
async fn get_ice_server_config_reports_the_lifetime_an_issuance_has_left() {
    // (case, the issuance's lifetime, how long ago it was issued, the Ttl owed, whether
    // the seeded credentials must still be the ones answered)
    let cases: [(&str, i32, u64, i64, bool); 5] = [
        ("issued just now, so the whole lifetime is left", 300, 0, 300, true),
        ("a third of the way through", 300, 100, 200, true),
        (
            "one second before the modelled minimum is all that is left",
            300,
            269,
            31,
            true,
        ),
        (
            "exactly the modelled minimum left, the last point it can be re-served",
            300,
            270,
            i64::from(MIN_ICE_CREDENTIAL_TTL_SECS),
            true,
        ),
        (
            "under the modelled minimum left, so it is reissued",
            300,
            271,
            300,
            false,
        ),
    ];

    for (case, ttl_secs, issued_secs_ago, expected_ttl, served_from_state) in cases {
        let arn = format!(
            "arn:aws:kinesisvideo:us-east-1:123456789012:channel/remaining-{issued_secs_ago}/1700000000"
        );
        let (engine, router) = emulator();
        let seeded = IceServerRecord {
            uris: vec!["turn:seeded.invalid:443?transport=udp".to_string()],
            username: "1700000000.seeded".to_string(),
            password: "seededpassword".to_string(),
            ttl_secs,
            issued_at: SystemTime::now() - Duration::from_secs(issued_secs_ago),
            client_id: Some("viewer-1".to_string()),
        };
        seed_channel(&engine, &arn, |channel| {
            channel.ice_credential_ttl_secs = ttl_secs;
            channel.ice_servers.push(seeded.clone());
        });

        let body = format!(r#"{{"ChannelARN":"{arn}","ClientId":"viewer-1"}}"#);
        let (status, answer) = call(&router, GET_ICE_SERVER_CONFIG, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {answer}"
        );
        let server = &answer["IceServerList"][0];
        let ttl = server
            .get("Ttl")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or_else(|| panic!("case {case:?}: no Ttl in {server}"));
        assert_eq!(
            ttl, expected_ttl,
            "case {case:?}: expected Ttl {expected_ttl}, got {ttl} in {server}"
        );
        assert!(
            (i64::from(MIN_ICE_CREDENTIAL_TTL_SECS)..=i64::from(MAX_ICE_CREDENTIAL_TTL_SECS))
                .contains(&ttl),
            "case {case:?}: Ttl {ttl} is outside the modelled 30..86400 range"
        );
        let answered_seeded_credentials =
            server["Password"].as_str() == Some(seeded.password.as_str());
        assert_eq!(
            answered_seeded_credentials, served_from_state,
            "case {case:?}: expected the seeded credentials to be re-served = \
             {served_from_state}, got {server}"
        );
    }
}

/// One refusal row: a request body, and the answer the service owes it.
struct RefusalCase {
    case: &'static str,
    body: &'static str,
    status: u16,
    code: &'static str,
}

/// The rule, on the failing side: an ARN that is not a signaling-channel ARN is the
/// caller's mistake (`InvalidArgumentException`, 400), a well-formed ARN no channel
/// carries is a missing resource (`ResourceNotFoundException`, 404), and a client the
/// channel does not admit is `InvalidClientException` (400).
#[tokio::test]
async fn get_ice_server_config_refuses_what_the_registry_cannot_serve() {
    const REGISTERED: &str =
        "arn:aws:kinesisvideo:us-east-1:123456789012:channel/demo-channel/1700000000";
    const RESTRICTED: &str =
        "arn:aws:kinesisvideo:us-east-1:123456789012:channel/restricted/1700000001";

    let cases = [
        RefusalCase {
            case: "a well-formed channel ARN nobody created",
            body: r#"{"ChannelARN":"arn:aws:kinesisvideo:us-east-1:123456789012:channel/never-created/1700000009","ClientId":"viewer-1"}"#,
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "a channel ARN in another account",
            body: r#"{"ChannelARN":"arn:aws:kinesisvideo:us-east-1:210987654321:channel/demo-channel/1700000000"}"#,
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "not an ARN at all",
            body: r#"{"ChannelARN":"not-an-arn"}"#,
            status: 400,
            code: "InvalidArgumentException",
        },
        RefusalCase {
            case: "an ARN of another AWS service",
            body: r#"{"ChannelARN":"arn:aws:kinesis:us-east-1:123456789012:stream/demo/1"}"#,
            status: 400,
            code: "InvalidArgumentException",
        },
        RefusalCase {
            // This service's `ResourceARN` names the `aws` partition LITERALLY
            // (`smithy_query(query="shape", shape="ResourceARN")`:
            // `^arn:aws:kinesisvideo:…`), where the peer Kinesis Video models admit any
            // `aws*` partition — so a partition AWS itself publishes is still not a
            // channel ARN here, whether the boundary or `ChannelArn::parse` refuses it.
            case: "a channel ARN in the GovCloud partition",
            body: r#"{"ChannelARN":"arn:aws-us-gov:kinesisvideo:us-gov-west-1:123456789012:channel/demo-channel/1700000000"}"#,
            status: 400,
            code: "InvalidArgumentException",
        },
        RefusalCase {
            case: "a channel ARN in the China partition",
            body: r#"{"ChannelARN":"arn:aws-cn:kinesisvideo:cn-north-1:123456789012:channel/demo-channel/1700000000","ClientId":"viewer-1"}"#,
            status: 400,
            code: "InvalidArgumentException",
        },
        RefusalCase {
            case: "a Kinesis Video ARN for a resource that is not a signaling channel",
            body: r#"{"ChannelARN":"arn:aws:kinesisvideo:us-east-1:123456789012:stream/demo-stream/1700000000"}"#,
            status: 400,
            code: "InvalidArgumentException",
        },
        RefusalCase {
            case: "no ChannelARN at all",
            body: r#"{"ClientId":"viewer-1"}"#,
            status: 400,
            code: "InvalidArgumentException",
        },
        RefusalCase {
            case: "a Service the enum does not admit",
            body: r#"{"ChannelARN":"arn:aws:kinesisvideo:us-east-1:123456789012:channel/demo-channel/1700000000","ClientId":"viewer-1","Service":"STUN"}"#,
            status: 400,
            code: "InvalidArgumentException",
        },
        RefusalCase {
            case: "a ClientId outside the modelled pattern",
            body: r#"{"ChannelARN":"arn:aws:kinesisvideo:us-east-1:123456789012:channel/demo-channel/1700000000","ClientId":"viewer one!"}"#,
            status: 400,
            code: "InvalidArgumentException",
        },
        RefusalCase {
            // The optional `Username` targets the modelled `Username` shape, which carries
            // the same `@pattern ^[a-zA-Z0-9_.-]+$` as `ClientId`
            // (`smithy_query(query="shape", shape="Username")`), so a label the issued
            // `IceServer$Username` could not carry is refused at the boundary rather than
            // reaching the credential mint.
            case: "a Username outside the modelled pattern",
            body: r#"{"ChannelARN":"arn:aws:kinesisvideo:us-east-1:123456789012:channel/demo-channel/1700000000","ClientId":"viewer-1","Username":"op:1 x+y/z="}"#,
            status: 400,
            code: "InvalidArgumentException",
        },
        RefusalCase {
            case: "a client the channel does not admit",
            body: r#"{"ChannelARN":"arn:aws:kinesisvideo:us-east-1:123456789012:channel/restricted/1700000001","ClientId":"stranger"}"#,
            status: 400,
            code: "InvalidClientException",
        },
        RefusalCase {
            case: "no ClientId on a channel that admits only named viewers",
            body: r#"{"ChannelARN":"arn:aws:kinesisvideo:us-east-1:123456789012:channel/restricted/1700000001"}"#,
            status: 400,
            code: "InvalidClientException",
        },
    ];

    let (engine, router) = emulator();
    seed_channel(&engine, REGISTERED, |_| {});
    seed_channel(&engine, RESTRICTED, |channel| {
        channel.viewer_client_ids.insert("known-viewer".to_string());
    });

    for case in &cases {
        let (status, body) = call(&router, GET_ICE_SERVER_CONFIG, case.body).await;
        assert_eq!(
            status, case.status,
            "case {:?}: expected HTTP {}, got {status} with {body}",
            case.case, case.status
        );
        assert_eq!(
            error_code(&body),
            case.code,
            "case {:?}: expected {}, got {body}",
            case.case,
            case.code
        );
    }

    // The admitted viewer of the restricted channel is still served.
    let (status, body) = call(
        &router,
        GET_ICE_SERVER_CONFIG,
        &format!(r#"{{"ChannelARN":"{RESTRICTED}","ClientId":"known-viewer"}}"#),
    )
    .await;
    assert_eq!(
        status, 200,
        "the channel's own viewer must be served, got {status} with {body}"
    );
}

/// One `SendAlexaOfferToMaster` row.
struct OfferCase {
    case: &'static str,
    arn: &'static str,
    sender: &'static str,
    payload: String,
    /// The answer the channel's master supplies, seeded into state.
    master_answer: String,
}

/// The rule: the `Answer` is the one the master connected to that channel supplied, read
/// back out of the offer the call appended to the channel's log — never a constant, and
/// never the offer echoed back.
#[tokio::test]
async fn send_alexa_offer_to_master_answers_from_the_channel_state() {
    let cases = [
        OfferCase {
            case: "the channel from the issue report",
            arn: "arn:aws:kinesisvideo:us-east-1:123456789012:channel/demo-channel/1700000000",
            sender: "alexa-1",
            payload: b64("v=0\r\no=- 1 1 IN IP4 203.0.113.1\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n"),
            master_answer: b64("v=0\r\no=- 2 2 IN IP4 198.51.100.7\r\na=recvonly\r\n"),
        },
        OfferCase {
            case: "a second sender on another channel, with a long offer",
            arn: "arn:aws:kinesisvideo:eu-central-1:000000000001:channel/alexa.channel_2/7",
            sender: "alexa.sender_2",
            payload: b64(&"a=candidate:1 1 udp 2130706431 192.0.2.4 51820 typ host\r\n".repeat(20)),
            master_answer: b64("v=0\r\no=- 3 3 IN IP4 198.51.100.9\r\na=sendrecv\r\n"),
        },
        OfferCase {
            case: "a one-byte offer on a third channel",
            arn: "arn:aws:kinesisvideo:ap-northeast-1:999999999999:channel/tiny/1",
            sender: "a",
            payload: b64("x"),
            master_answer: b64("y"),
        },
        OfferCase {
            // The modelled pattern `^[a-zA-Z0-9+/=]+$` ADMITS `=` without mandating it, so
            // an offer whose base64 carries no padding is content the member allows and
            // must be delivered, not refused.
            case: "an offer whose base64 carries no padding",
            arn: "arn:aws:kinesisvideo:us-west-1:123456789012:channel/unpadded/2",
            sender: "alexa-unpadded",
            payload: b64("a=sendonly").trim_end_matches('=').to_string(),
            master_answer: b64("v=0\r\no=- 4 4 IN IP4 198.51.100.11\r\n"),
        },
    ];

    let (engine, router) = emulator();
    for case in &cases {
        seed_channel(&engine, case.arn, |channel| {
            channel.master_answer = Some(case.master_answer.clone());
        });
    }

    for case in &cases {
        let body = serde_json::json!({
            "ChannelARN": case.arn,
            "SenderClientId": case.sender,
            "MessagePayload": case.payload,
        })
        .to_string();

        let (status, answer) = call(&router, SEND_ALEXA_OFFER_TO_MASTER, &body).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with {answer}",
            case.case
        );
        let returned = answer
            .get("Answer")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("case {:?}: no Answer in {answer}", case.case));
        assert_eq!(
            returned, case.master_answer,
            "case {:?}: the Answer must be the master's, got {returned:?}",
            case.case
        );
        assert_ne!(
            returned, case.payload,
            "case {:?}: the Answer must not be the offer echoed back",
            case.case
        );
        assert!(
            (1..=10_000).contains(&returned.len()),
            "case {:?}: Answer length {} is outside the modelled 1..10000",
            case.case,
            returned.len()
        );

        // Delivered twice: the same master answer, and both offers in the log.
        let (repeat_status, repeat) = call(&router, SEND_ALEXA_OFFER_TO_MASTER, &body).await;
        assert_eq!(
            repeat_status, 200,
            "case {:?}: a redelivery must succeed, got {repeat_status}",
            case.case
        );
        assert_eq!(
            repeat.get("Answer"),
            answer.get("Answer"),
            "case {:?}: the master's answer must not change between deliveries",
            case.case
        );

        let state = engine.state::<SignalingState>();
        let store = state.lock();
        let channel = store
            .resolve_channel(case.arn)
            .unwrap_or_else(|error| panic!("case {:?}: {error:?}", case.case));
        let from_sender: Vec<_> = channel.offers_from(case.sender).collect();
        assert_eq!(
            from_sender.len(),
            2,
            "case {:?}: expected both offers in the sender's log, got {}",
            case.case,
            from_sender.len()
        );
        for (index, offer) in from_sender.iter().enumerate() {
            assert_eq!(
                offer.message_payload, case.payload,
                "case {:?}: offer {index} must hold the payload as sent",
                case.case
            );
            assert_eq!(
                offer.answer.as_deref(),
                Some(case.master_answer.as_str()),
                "case {:?}: offer {index} must record the answer it was served",
                case.case
            );
        }
    }

    // Each channel's log holds only its own offers.
    let state = engine.state::<SignalingState>();
    let store = state.lock();
    for case in &cases {
        let channel = store
            .resolve_channel(case.arn)
            .unwrap_or_else(|error| panic!("case {:?}: {error:?}", case.case));
        assert_eq!(
            channel.offers.len(),
            2,
            "case {:?}: a channel's log must hold only its own offers, got {:?}",
            case.case,
            channel.offers
        );
    }
}

/// The rule, on the failing side: the channel must resolve, and `MessagePayload` must be
/// base64-encoded content. A channel with no master connected has no answer to deliver.
#[tokio::test]
async fn send_alexa_offer_to_master_refuses_what_it_cannot_deliver() {
    const WITH_MASTER: &str =
        "arn:aws:kinesisvideo:us-east-1:123456789012:channel/demo-channel/1700000000";
    const WITHOUT_MASTER: &str =
        "arn:aws:kinesisvideo:us-east-1:123456789012:channel/masterless/1700000001";

    let cases = [
        RefusalCase {
            case: "a well-formed channel ARN nobody created",
            body: r#"{"ChannelARN":"arn:aws:kinesisvideo:us-east-1:123456789012:channel/never-created/1","SenderClientId":"alexa-1","MessagePayload":"eA=="}"#,
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "not an ARN at all",
            body: r#"{"ChannelARN":"not-an-arn","SenderClientId":"alexa-1","MessagePayload":"eA=="}"#,
            status: 400,
            code: "InvalidArgumentException",
        },
        RefusalCase {
            case: "a Kinesis Video ARN for a resource that is not a signaling channel",
            body: r#"{"ChannelARN":"arn:aws:kinesisvideo:us-east-1:123456789012:stream/demo-stream/1","SenderClientId":"alexa-1","MessagePayload":"eA=="}"#,
            status: 400,
            code: "InvalidArgumentException",
        },
        RefusalCase {
            case: "a payload of base64 characters that is not base64 (one quantum short)",
            body: r#"{"ChannelARN":"arn:aws:kinesisvideo:us-east-1:123456789012:channel/demo-channel/1700000000","SenderClientId":"alexa-1","MessagePayload":"A"}"#,
            status: 400,
            code: "InvalidArgumentException",
        },
        RefusalCase {
            case: "a payload whose length is not a whole number of base64 quanta",
            body: r#"{"ChannelARN":"arn:aws:kinesisvideo:us-east-1:123456789012:channel/demo-channel/1700000000","SenderClientId":"alexa-1","MessagePayload":"AAAAA"}"#,
            status: 400,
            code: "InvalidArgumentException",
        },
        RefusalCase {
            case: "a payload with padding in the middle",
            body: r#"{"ChannelARN":"arn:aws:kinesisvideo:us-east-1:123456789012:channel/demo-channel/1700000000","SenderClientId":"alexa-1","MessagePayload":"A=AA"}"#,
            status: 400,
            code: "InvalidArgumentException",
        },
        RefusalCase {
            // A payload that carries `=` states its own padding, so a WRONG one is
            // refused rather than completed: `eA=` is one `=` short of the padded
            // encoding of its single byte, and no encoder emits it.
            case: "a payload whose own padding is short",
            body: r#"{"ChannelARN":"arn:aws:kinesisvideo:us-east-1:123456789012:channel/demo-channel/1700000000","SenderClientId":"alexa-1","MessagePayload":"eA="}"#,
            status: 400,
            code: "InvalidArgumentException",
        },
        RefusalCase {
            case: "a payload outside the modelled base64 pattern",
            body: r#"{"ChannelARN":"arn:aws:kinesisvideo:us-east-1:123456789012:channel/demo-channel/1700000000","SenderClientId":"alexa-1","MessagePayload":"not base64!"}"#,
            status: 400,
            code: "InvalidArgumentException",
        },
        RefusalCase {
            case: "no SenderClientId",
            body: r#"{"ChannelARN":"arn:aws:kinesisvideo:us-east-1:123456789012:channel/demo-channel/1700000000","MessagePayload":"eA=="}"#,
            status: 400,
            code: "InvalidArgumentException",
        },
        RefusalCase {
            case: "no MessagePayload",
            body: r#"{"ChannelARN":"arn:aws:kinesisvideo:us-east-1:123456789012:channel/demo-channel/1700000000","SenderClientId":"alexa-1"}"#,
            status: 400,
            code: "InvalidArgumentException",
        },
        RefusalCase {
            case: "a channel with no master connected",
            body: r#"{"ChannelARN":"arn:aws:kinesisvideo:us-east-1:123456789012:channel/masterless/1700000001","SenderClientId":"alexa-1","MessagePayload":"eA=="}"#,
            status: 404,
            code: "ResourceNotFoundException",
        },
    ];

    let (engine, router) = emulator();
    seed_channel(&engine, WITH_MASTER, |channel| {
        channel.master_answer = Some(b64("v=0\r\na=recvonly\r\n"));
    });
    seed_channel(&engine, WITHOUT_MASTER, |_| {});

    for case in &cases {
        let (status, body) = call(&router, SEND_ALEXA_OFFER_TO_MASTER, case.body).await;
        assert_eq!(
            status, case.status,
            "case {:?}: expected HTTP {}, got {status} with {body}",
            case.case, case.status
        );
        assert_eq!(
            error_code(&body),
            case.code,
            "case {:?}: expected {}, got {body}",
            case.case,
            case.code
        );
    }

    let state = engine.state::<SignalingState>();
    let store = state.lock();
    // An offer to a channel with no master cannot be delivered, so it is refused and
    // nothing is written: a request the service could not serve leaves no record.
    let masterless = store
        .resolve_channel(WITHOUT_MASTER)
        .expect("the channel is registered");
    assert!(
        masterless.offers.is_empty(),
        "an offer that could not be delivered must not be logged, got {:?}",
        masterless.offers
    );
    // A refused request writes nothing to the channel it named.
    let served = store
        .resolve_channel(WITH_MASTER)
        .expect("the channel is registered");
    assert!(
        served.offers.is_empty(),
        "a refused offer must not be logged, got {:?}",
        served.offers
    );
}

/// A request whose header values are not valid UTF-8 is still routed and served.
///
/// `can_handle` runs for EVERY inbound request, so nothing on it may unwind or decline on
/// a value it does not understand: the claim reads only the SigV4 credential scope and the
/// request path, so an exotic header value cannot stop a request this service binds from
/// being served.
#[tokio::test]
async fn a_request_carrying_non_utf8_header_values_is_still_served() {
    const ARN: &str =
        "arn:aws:kinesisvideo:us-east-1:123456789012:channel/odd-headers/1700000000";
    let (engine, router) = emulator();
    seed_channel(&engine, ARN, |_| {});

    let body = format!(r#"{{"ChannelARN":"{ARN}","ClientId":"viewer-1","Service":"TURN"}}"#);
    let mut probe = request(GET_ICE_SERVER_CONFIG, &body);
    // `obs-text` bytes: a legal HTTP header value that is not valid UTF-8.
    probe.headers_mut().insert(
        "x-amz-raw-trailer",
        http::HeaderValue::from_bytes(&[0xff, 0xfe, 0x80])
            .expect("obs-text bytes are a legal header value"),
    );

    let response = router.dispatch(probe).await;
    let status = response.status().as_u16();
    let rendered = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "a request with a non-UTF-8 header value must still be served, got {status} with \
         {rendered}"
    );
}

/// A channel's offer log is BOUNDED: every delivery stores the caller's payload, so the
/// log keeps at most `MAX_OFFERS_PER_CHANNEL` records and drops the oldest. A caller
/// looping this route therefore cannot grow the store without limit, and every delivery
/// is still answered out of the record it stored.
#[tokio::test]
async fn send_alexa_offer_to_master_bounds_the_channel_offer_log() {
    const ARN: &str = "arn:aws:kinesisvideo:us-east-1:123456789012:channel/busy/1700000000";
    let (engine, router) = emulator();
    let master = b64("v=0\r\no=- 9 9 IN IP4 198.51.100.11\r\na=recvonly\r\n");
    seed_channel(&engine, ARN, |channel| {
        channel.master_answer = Some(master.clone());
    });

    // Past the cap, so the oldest offers must have been dropped to make room.
    let deliveries = MAX_OFFERS_PER_CHANNEL + 5;
    for index in 0..deliveries {
        let body = serde_json::json!({
            "ChannelARN": ARN,
            "SenderClientId": format!("alexa-{index}"),
            "MessagePayload": b64(&format!("v=0\r\na=mid:{index}\r\n")),
        })
        .to_string();
        let (status, answer) = call(&router, SEND_ALEXA_OFFER_TO_MASTER, &body).await;
        assert_eq!(
            status, 200,
            "delivery {index}: expected HTTP 200, got {status} with {answer}"
        );
        assert_eq!(
            answer.get("Answer").and_then(serde_json::Value::as_str),
            Some(master.as_str()),
            "delivery {index}: the Answer must be the master's, got {answer}"
        );
    }

    let state = engine.state::<SignalingState>();
    let store = state.lock();
    let channel = store.resolve_channel(ARN).expect("the channel is registered");
    assert_eq!(
        channel.offers.len(),
        MAX_OFFERS_PER_CHANNEL,
        "the log must hold at most {MAX_OFFERS_PER_CHANNEL} offers after {deliveries} \
         deliveries, got {}",
        channel.offers.len()
    );
    let senders: Vec<&str> = channel
        .offers
        .iter()
        .map(|offer| offer.sender_client_id.as_str())
        .collect();
    assert!(
        !senders.contains(&"alexa-0"),
        "the oldest offer must be dropped at the cap, got {senders:?}"
    );
    let newest = format!("alexa-{}", deliveries - 1);
    assert!(
        senders.contains(&newest.as_str()),
        "the newest offer must be kept, got {senders:?}"
    );
}

/// A channel's credential records are BOUNDED too: one issuance is held per `ClientId`,
/// which is the caller's free-form string, so the record count is capped at
/// `MAX_ICE_ISSUANCES_PER_CHANNEL` however many viewers ask — while every call is still
/// answered from the record it just stored.
#[tokio::test]
async fn get_ice_server_config_bounds_recorded_issuances_per_channel() {
    const ARN: &str = "arn:aws:kinesisvideo:us-east-1:123456789012:channel/crowded/1700000000";
    let (engine, router) = emulator();
    seed_channel(&engine, ARN, |_| {});

    let viewers = MAX_ICE_ISSUANCES_PER_CHANNEL + 10;
    let mut last_password = String::new();
    for index in 0..viewers {
        let body =
            format!(r#"{{"ChannelARN":"{ARN}","ClientId":"viewer-{index}","Service":"TURN"}}"#);
        let (status, answer) = call(&router, GET_ICE_SERVER_CONFIG, &body).await;
        assert_eq!(
            status, 200,
            "viewer {index}: expected HTTP 200, got {status} with {answer}"
        );
        last_password = answer["IceServerList"][0]["Password"]
            .as_str()
            .unwrap_or_else(|| panic!("viewer {index}: no Password in {answer}"))
            .to_string();
    }

    let state = engine.state::<SignalingState>();
    let store = state.lock();
    let channel = store.resolve_channel(ARN).expect("the channel is registered");
    assert_eq!(
        channel.ice_servers.len(),
        MAX_ICE_ISSUANCES_PER_CHANNEL,
        "the channel must hold at most {MAX_ICE_ISSUANCES_PER_CHANNEL} issuances after \
         {viewers} viewers, got {}",
        channel.ice_servers.len()
    );
    let newest = channel
        .ice_servers
        .last()
        .expect("the issuance just recorded is present");
    let newest_viewer = format!("viewer-{}", viewers - 1);
    assert_eq!(
        newest.client_id.as_deref(),
        Some(newest_viewer.as_str()),
        "the newest issuance must be the one just recorded, got {:?}",
        newest.client_id
    );
    assert_eq!(
        newest.password, last_password,
        "the last answer must be read from the issuance just recorded"
    );
}
