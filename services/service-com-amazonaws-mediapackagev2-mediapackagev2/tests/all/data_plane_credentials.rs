// SPDX-License-Identifier: CC0-1.0

//! What authorizes a request to one of the data-plane URLs mediapackagev2 publishes, over
//! the SET of credentials a caller can present to one.
//!
//! The rule: a request to a published data-plane hostname is authenticated by the
//! EMULATOR, exactly as a control-plane request is, so a SigV4 credential whose signature
//! the emulator cannot verify is refused `403` wherever it is presented and in whichever
//! carrier; a request that presents no credential at all is answered by the service itself
//! — `403` with its access-denied body for an ingest push, since MediaPackage vends no
//! ingest credential, and the origin endpoint's resource policy for an egress fetch. The
//! two refusals are therefore DISTINGUISHABLE: the emulator's names the credential or the
//! signature it would not accept, the service's says access is denied.
//!
//! The data-plane rows run with request authentication ON, the emulator's own default
//! (`--auth on`), because that is the switch that makes a signature verifiable at all. The
//! data-plane cases in `rejection_rule_and_negative_paths.rs` cover the same URLs with it
//! off, where the emulator verifies no signature anywhere and a credentialed push is
//! served.
//!
//! The rows differ along the credential a caller presents — absent, a well-formed header
//! whose signature does not verify, one with no signing date, one naming a key id the
//! emulator does not hold, one scoped to another service, and the same classes carried in a
//! presigned URL's query instead of a header — not along the resource names, which are not
//! what the rule generalises over.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every credential scope in this suite names.
const REGION: &str = "us-west-2";

/// The names the fixtures are created under. Nothing in the rule depends on them.
const GROUP: &str = "dpg";
const CHANNEL: &str = "dpc";
const ENDPOINT: &str = "dpe";

/// The SigV4 signing name of this service, which a data-plane credential's scope must name.
const SIGNING_NAME: &str = "mediapackagev2";

/// A signature value no key could have produced over any request: 64 hex zeroes, the length
/// SigV4's HMAC-SHA256 signature has.
const UNVERIFIABLE_SIGNATURE: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// How long a presigned URL in this suite is valid for, inside the seven days AWS allows.
const PRESIGNED_EXPIRES_SECS: u64 = 900;

/// How far in the past the ELAPSED presigned window is signed, so its validity has run out
/// whatever the clock reads when the case runs.
const ELAPSED_WINDOW_AGE: Duration = Duration::from_secs(3600);

/// How a request presents (or fails to present) a SigV4 credential.
///
/// Every presented variant is well formed — the algorithm, a credential scope, the signed
/// headers and a signature are all there — and none of them can verify, because no caller
/// here holds the secret the emulator minted for this run. What differs is WHICH part the
/// emulator cannot accept, and in which carrier it arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Credential {
    /// No `Authorization` header and no presigned query — what an anonymous player, or an
    /// encoder that was never given credentials, sends.
    Absent,
    /// A header scoped to the key id the emulator published, with a signature that is not
    /// one over this request.
    HeaderWrongSignature,
    /// The same header with no `x-amz-date`, so there is no signing instant to verify
    /// against.
    HeaderNoSigningDate,
    /// A header scoped to a key id the emulator does not hold.
    HeaderUnknownKeyId,
    /// A header whose credential scope names another service's signing name.
    HeaderOtherService,
    /// The same unverifiable credential carried in a presigned URL's query.
    PresignedWrongSignature,
    /// A presigned query whose validity window has elapsed.
    PresignedExpired,
}

impl Credential {
    /// Whether this presentation carries a credential for the emulator to verify.
    fn is_presented(self) -> bool {
        self != Self::Absent
    }

    /// Whether the credential travels in the query rather than in a header.
    fn is_presigned(self) -> bool {
        matches!(self, Self::PresignedWrongSignature | Self::PresignedExpired)
    }
}

/// One HTTP answer, reduced to what a caller branches on.
#[derive(Debug)]
struct Answer {
    status: u16,
    body: String,
    json: Value,
    /// The request URI as it was dispatched, which for a presigned case carries the
    /// credential: part of what a failing case has to report.
    uri: String,
}

impl Answer {
    fn string(&self, member: &str) -> &str {
        self.json.get(member).and_then(Value::as_str).unwrap_or("")
    }

    /// The error code and sentence the answer reports, in one string, however the envelope
    /// names them: the emulator's refusals use `__type`/`message`, the service's data-plane
    /// answers use `code`/`message`.
    fn reported(&self) -> String {
        format!(
            "{} {} {}",
            self.string("__type"),
            self.string("code"),
            self.string("message")
        )
    }
}

/// A MediaPackageV2 emulator plus the key id it published for this run.
struct Emulator {
    router: MultiServiceRouter,
    /// The access key id the emulator minted: a credential scoped to it is one the emulator
    /// HOLDS, so what it then refuses is the signature rather than the key.
    access_key_id: String,
}

impl Emulator {
    /// A router with authentication OFF, so this suite's fixtures can be created without
    /// signing them (which is not what it is about), and the key id authentication will
    /// verify against once [`Self::enforce_signatures`] turns it on.
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(true);
        let access_key_id = match router.vended_credentials().into_iter().next() {
            Some(emulator_server::VendedCredentials::AwsAccessKey { access_key_id, .. }) => {
                access_key_id
            }
            other => panic!(
                "with authentication on the emulator must vend a SigV4 key pair, got {other:?}"
            ),
        };
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mediapackagev2_mediapackagev2::EmulatedService::new(engine),
        );
        Self {
            router,
            access_key_id,
        }
    }

    /// Turn request authentication on, as the emulator's own default (`--auth on`) does, so
    /// a presented credential is verified from here on.
    fn enforce_signatures(&mut self) {
        self.router.set_request_authentication(true);
    }

    /// Send one request, presenting `credential`, and reduce the answer.
    async fn send(
        &self,
        method: &str,
        path: &str,
        host: &str,
        payload: &[u8],
        credential: Credential,
    ) -> Answer {
        // A presigned credential travels in the URL, so the path is settled before the
        // request is built.
        let path = match credential.is_presigned() {
            false => path.to_string(),
            true => format!("{path}?{}", self.presigned_query(credential)),
        };
        let mut request = http::Request::builder()
            .method(method)
            .uri(&path)
            .header("host", host)
            .header("content-type", "application/json");
        if credential.is_presented() && !credential.is_presigned() {
            request = request.header("authorization", self.authorization(credential));
            if credential != Credential::HeaderNoSigningDate {
                request = request.header("x-amz-date", amz_date(signing_instant(credential)));
            }
        }
        let request = request
            .body(bytes::Bytes::from(payload.to_vec()))
            .expect("a well-formed test request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = String::from_utf8_lossy(&response.into_body()).to_string();
        let json = serde_json::from_str(&body).unwrap_or(Value::Null);
        Answer {
            status,
            body,
            json,
            uri: path,
        }
    }

    /// The `Authorization` header this credential presents: the SigV4 algorithm, a
    /// credential scope, the one signed header, and a signature.
    fn authorization(&self, credential: Credential) -> String {
        let (key_id, service) = match credential {
            Credential::HeaderUnknownKeyId => ("AKIDNOTTHISEMULATORS", SIGNING_NAME),
            Credential::HeaderOtherService => (self.access_key_id.as_str(), "mediapackage"),
            _ => (self.access_key_id.as_str(), SIGNING_NAME),
        };
        let date = amz_date(signing_instant(credential));
        let scope_date = &date[..SCOPE_DATE_LEN];
        format!(
            "AWS4-HMAC-SHA256 Credential={key_id}/{scope_date}/{REGION}/{service}/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature={UNVERIFIABLE_SIGNATURE}"
        )
    }

    /// The query a presigned URL carries instead of an `Authorization` header, with the same
    /// fields in the same names AWS presigns with.
    fn presigned_query(&self, credential: Credential) -> String {
        // Both rows carry a window of the same length; the elapsed one is signed an hour
        // ago ([`signing_instant`]), so the two differ only in whether it has run out.
        let date = amz_date(signing_instant(credential));
        let scope_date = &date[..SCOPE_DATE_LEN];
        format!(
            "X-Amz-Algorithm=AWS4-HMAC-SHA256\
             &X-Amz-Credential={key}%2F{scope_date}%2F{REGION}%2F{SIGNING_NAME}%2Faws4_request\
             &X-Amz-Date={date}&X-Amz-Expires={PRESIGNED_EXPIRES_SECS}\
             &X-Amz-SignedHeaders=host&X-Amz-Signature={UNVERIFIABLE_SIGNATURE}",
            key = self.access_key_id
        )
    }

    /// One control-plane call. The fixtures are created before authentication is enforced,
    /// so the credential here is only the scope the service reads its region from.
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Answer {
        let payload = body.map(|b| b.to_string()).unwrap_or_default();
        self.send(
            method,
            path,
            &format!("{SIGNING_NAME}.{REGION}.amazonaws.com"),
            payload.as_bytes(),
            Credential::HeaderWrongSignature,
        )
        .await
    }
}

/// How many characters of a SigV4 date are the credential scope's date (`YYYYMMDD`).
const SCOPE_DATE_LEN: usize = 8;

/// The instant a credential is signed at: now, except for the elapsed-window row, which is
/// signed far enough in the past that its validity has run out whatever the clock reads.
fn signing_instant(credential: Credential) -> SystemTime {
    match credential {
        Credential::PresignedExpired => SystemTime::now() - ELAPSED_WINDOW_AGE,
        _ => SystemTime::now(),
    }
}

/// `at` in `YYYYMMDDTHHMMSSZ`, the format SigV4 writes a signing date in, rendered through
/// the SDK's own timestamp type rather than any date arithmetic written here.
fn amz_date(at: SystemTime) -> String {
    let text = aws_smithy_types::DateTime::from(at)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .expect("an instant renders as a date-time");
    // `Format::DateTime` is RFC 3339 (`2026-01-01T00:00:00.123Z`); SigV4 writes the same
    // instant without separators or a fraction.
    let whole_seconds = text.split('.').next().unwrap_or(&text);
    let compact: String = whole_seconds
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == 'T')
        .collect();
    format!("{compact}Z")
}

/// The host and the path of a published URL.
fn split_url(url: &str) -> (String, String) {
    let rest = url
        .strip_prefix("https://")
        .unwrap_or_else(|| panic!("a published URL is https, got {url:?}"));
    match rest.split_once('/') {
        Some((host, path)) => (host.to_string(), format!("/{path}")),
        None => (rest.to_string(), "/".to_string()),
    }
}

/// A resource policy granting one action to every principal, as `PutOriginEndpointPolicy`
/// takes it.
fn grant_policy(action: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": "*",
            "Action": action,
        }],
    })
    .to_string()
}

/// Create the group, the channel and the origin endpoint, and report the published ingest
/// URL and HLS manifest URL.
async fn fixtures(emulator: &Emulator) -> (String, String) {
    let group = emulator
        .call(
            "POST",
            "/channelGroup",
            Some(json!({"ChannelGroupName": GROUP})),
        )
        .await;
    assert_eq!(
        group.status, 200,
        "the channel group must be created, got {} with body {}",
        group.status, group.body
    );
    let channel = emulator
        .call(
            "POST",
            &format!("/channelGroup/{GROUP}/channel"),
            Some(json!({"ChannelName": CHANNEL, "InputType": "CMAF"})),
        )
        .await;
    assert_eq!(
        channel.status, 200,
        "the channel must be created, got {} with body {}",
        channel.status, channel.body
    );
    let ingest_url = channel
        .json
        .get("IngestEndpoints")
        .and_then(Value::as_array)
        .and_then(|list| list.first())
        .and_then(|first| first.get("Url"))
        .and_then(Value::as_str)
        .expect("a channel publishes ingest URLs")
        .to_string();
    let endpoint = emulator
        .call(
            "POST",
            &format!("/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint"),
            Some(json!({
                "OriginEndpointName": ENDPOINT,
                "ContainerType": "TS",
                "HlsManifests": [{"ManifestName": "index"}],
            })),
        )
        .await;
    assert_eq!(
        endpoint.status, 200,
        "the origin endpoint must be created, got {} with body {}",
        endpoint.status, endpoint.body
    );
    let manifest_url = endpoint
        .json
        .get("HlsManifests")
        .and_then(Value::as_array)
        .and_then(|list| list.first())
        .and_then(|first| first.get("Url"))
        .and_then(Value::as_str)
        .expect("an origin endpoint publishes manifest URLs")
        .to_string();
    (ingest_url, manifest_url)
}

/// Every credential class this suite presents, in both carriers. None of them can verify,
/// so each must be refused `403` — and by a DIFFERENT answer from the one a push carrying
/// no credential gets, which is the service's own.
const UNVERIFIABLE: &[Credential] = &[
    Credential::HeaderWrongSignature,
    Credential::HeaderNoSigningDate,
    Credential::HeaderUnknownKeyId,
    Credential::HeaderOtherService,
    Credential::PresignedWrongSignature,
    Credential::PresignedExpired,
];

/// An ingest push is answered by whoever can judge the credential it presents: the service
/// refuses a push carrying none with its access-denied body, and the emulator refuses every
/// credential it cannot verify — in either carrier — with a different `403`.
#[tokio::test]
async fn an_ingest_push_presenting_a_credential_is_refused_unless_the_emulator_verifies_it() {
    let mut emulator = Emulator::new();
    let (ingest_url, _manifest_url) = fixtures(&emulator).await;
    emulator.enforce_signatures();
    let (host, path) = split_url(&ingest_url);
    let object = format!("{path}/0.mp4");

    // The refusal of a push that presents nothing: the service's own, since there is no
    // signature to verify. Every credentialed refusal must be distinguishable from it.
    let unauthenticated = emulator
        .send("PUT", &object, &host, b"segment", Credential::Absent)
        .await;
    assert_eq!(
        unauthenticated.status, 403,
        "case {:?}: expected HTTP 403, got {} with body {}",
        Credential::Absent,
        unauthenticated.status,
        unauthenticated.body
    );
    assert!(
        unauthenticated.body.to_lowercase().contains("access denied"),
        "case {:?}: the refusal must say access denied, got {}",
        Credential::Absent,
        unauthenticated.body
    );

    for credential in UNVERIFIABLE {
        let answer = emulator
            .send("PUT", &object, &host, b"segment", *credential)
            .await;
        assert_eq!(
            answer.status, 403,
            "case {credential:?}: expected HTTP 403, got {} with body {} (uri {})",
            answer.status, answer.body, answer.uri
        );
        assert_ne!(
            answer.body, unauthenticated.body,
            "case {credential:?}: a refused credential must not answer the same body as a push \
             that presented none, got {}",
            answer.body
        );
    }
}

/// The emulator's two refusal classes are distinguishable from each other as well: a
/// credential it HOLDS whose signature is not one over the request is refused for the
/// signature, whichever carrier brought it, which is not the answer a credential it cannot
/// read at all gets.
#[tokio::test]
async fn a_wrong_signature_is_a_different_refusal_from_a_credential_that_cannot_be_read() {
    let mut emulator = Emulator::new();
    let (ingest_url, _manifest_url) = fixtures(&emulator).await;
    emulator.enforce_signatures();
    let (host, path) = split_url(&ingest_url);
    let object = format!("{path}/0.mp4");

    // Both rows name the key id the emulator published and carry a complete, well-formed
    // credential, so the one thing it can refuse them for is the signature.
    for credential in [
        Credential::HeaderWrongSignature,
        Credential::PresignedWrongSignature,
    ] {
        let answer = emulator
            .send("PUT", &object, &host, b"segment", credential)
            .await;
        assert_eq!(
            answer.status, 403,
            "case {credential:?}: expected HTTP 403, got {} with body {} (uri {})",
            answer.status, answer.body, answer.uri
        );
        assert!(
            answer.reported().to_lowercase().contains("signature"),
            "case {credential:?}: the refusal must name the signature it would not accept, got {}",
            answer.body
        );
    }

    // And a credential the emulator cannot read as one of its own answers differently.
    let wrong_signature = emulator
        .send(
            "PUT",
            &object,
            &host,
            b"segment",
            Credential::HeaderWrongSignature,
        )
        .await;
    for credential in [
        Credential::HeaderUnknownKeyId,
        Credential::HeaderNoSigningDate,
        Credential::HeaderOtherService,
    ] {
        let answer = emulator
            .send("PUT", &object, &host, b"segment", credential)
            .await;
        assert_eq!(
            answer.status, 403,
            "case {credential:?}: expected HTTP 403, got {} with body {}",
            answer.status, answer.body
        );
        assert_ne!(
            answer.body, wrong_signature.body,
            "case {credential:?}: a credential the emulator cannot read must not answer the same \
             body as one whose signature is merely wrong, got {}",
            answer.body
        );
    }
}

/// An egress fetch stays gated by the origin endpoint's resource policy with authentication
/// on — an anonymous fetch is `403` without a grant and `404` with one, since nothing is
/// packaged — while a credential the emulator cannot verify is refused before the policy is
/// ever consulted, grant or no grant.
#[tokio::test]
async fn an_egress_fetch_is_gated_by_the_policy_and_by_the_credential_it_presents() {
    let mut emulator = Emulator::new();
    let (_ingest_url, manifest_url) = fixtures(&emulator).await;
    let policy_path =
        format!("/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}/policy");
    let (host, path) = split_url(&manifest_url);
    emulator.enforce_signatures();

    // Before any grant: the anonymous fetch is refused by the policy, and every credential
    // is refused before the policy is reached.
    let anonymous = emulator.send("GET", &path, &host, b"", Credential::Absent).await;
    assert_eq!(
        anonymous.status, 403,
        "case {:?} before any grant: expected HTTP 403, got {} with body {}",
        Credential::Absent,
        anonymous.status,
        anonymous.body
    );
    for credential in UNVERIFIABLE {
        let answer = emulator.send("GET", &path, &host, b"", *credential).await;
        assert_eq!(
            answer.status, 403,
            "case {credential:?} before any grant: expected HTTP 403, got {} with body {} \
             (uri {})",
            answer.status, answer.body, answer.uri
        );
    }

    // The grant has to be stored while authentication is off: signing a control-plane
    // request is not what this suite is about.
    let mut emulator = emulator;
    emulator.router.set_request_authentication(false);
    let granted = emulator
        .call(
            "POST",
            &policy_path,
            Some(json!({"Policy": grant_policy("mediapackagev2:GetObject")})),
        )
        .await;
    assert_eq!(
        granted.status, 200,
        "the grant must be stored, got {} with body {}",
        granted.status, granted.body
    );
    emulator.enforce_signatures();

    // After the grant the anonymous fetch is authorized and reaches the object lookup (404,
    // nothing packaged), while every unverifiable credential is still refused: the signature
    // is judged before the policy.
    let anonymous = emulator.send("GET", &path, &host, b"", Credential::Absent).await;
    assert_eq!(
        anonymous.status, 404,
        "case {:?} after the GetObject grant: expected HTTP 404, got {} with body {}",
        Credential::Absent,
        anonymous.status,
        anonymous.body
    );
    for credential in UNVERIFIABLE {
        let answer = emulator.send("GET", &path, &host, b"", *credential).await;
        assert_eq!(
            answer.status, 403,
            "case {credential:?} after the GetObject grant: expected HTTP 403, got {} with body \
             {} (uri {})",
            answer.status, answer.body, answer.uri
        );
    }
}
