// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for importing an IVS RealTime public key and reading it back.
//!
//! Every case goes through the real front door — `MultiServiceRouter::dispatch` with a
//! restJson1 request, exactly as a signed `aws ivs-realtime` call arrives — so what it
//! asserts is the emulator's HTTP behaviour, not a handler called directly.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs
//! (several key algorithms, several PEM encodings of one key, names and tag maps of
//! different sizes, several regions):
//!
//! 1. `ImportPublicKey` persists the imported key and answers it; `GetPublicKey` answers
//!    that same key afterwards, from the region's view that holds it, and a read never
//!    changes what a later read sees.
//! 2. A key's `fingerprint` is the MD5 digest of the key's DER `SubjectPublicKeyInfo`,
//!    rendered as 16 colon-separated lowercase hex byte pairs — so it is derived from the
//!    imported material alone: the same key always fingerprints to the same value however
//!    its PEM is wrapped, and two different keys fingerprint differently.
//! 3. Importing material this region's view already holds is refused with
//!    `ConflictException` (HTTP 409) and adds nothing.
//! 4. Material that is not an ECDSA public key in a `PUBLIC KEY` PEM block is refused
//!    with `ValidationException` (HTTP 400) and persists nothing.
//! 5. An ARN of the modelled form naming no imported key is `ResourceNotFoundException`
//!    (HTTP 404), never an empty success.
//! 6. A public key is one of the kinds the `/tags/{resourceArn}` surface resolves, so the
//!    tags it was imported with are what that surface reads, and what that surface writes
//!    is what `GetPublicKey` reports.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use md5::{Digest, Md5};
use rcgen::{KeyPair, PKCS_ECDSA_P256_SHA256, PKCS_ECDSA_P384_SHA384, PKCS_ED25519};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The region most cases are signed for; the cross-region rows name [`OTHER_REGION`].
const REGION: &str = "us-east-1";

/// A second region, so a key is read back only through the endpoint that holds it.
const OTHER_REGION: &str = "eu-west-1";

/// The account id every ARN this emulator mints carries (`engine::mock_account_id`).
const ACCOUNT: &str = "000000000000";

/// The published maximum length of `PublicKeyName`, from
/// `smithy_query(query="shape",shape="PublicKeyName")` (`@length(0..128)`).
const MAX_NAME_LENGTH: usize = 128;

/// What a tagging write answers: the recorded AWS responses to `TagResource` and
/// `UntagResource` are 204 with no body.
const NO_CONTENT_STATUS: u16 = 204;

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches
/// it.
///
/// Each test suite under `tests/all/` is its own private module, so this is the per-suite
/// copy of that two-line harness rather than a shared one.
struct RealTime {
    router: MultiServiceRouter,
}

/// One answer from the emulator: the status, the wire error code (the restJson1 `__type`
/// discriminator) and the parsed body.
struct Reply {
    status: u16,
    error_code: String,
    body: Value,
}

impl RealTime {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call `operation` as an SDK does: `POST /<operation>`, signed for `region`.
    async fn call_in(&self, region: &str, operation: &str, body: &Value) -> Reply {
        self.send(
            region,
            "POST",
            &format!("/{operation}"),
            Some(bytes::Bytes::from(body.to_string())),
        )
        .await
    }

    /// [`Self::call_in`] for [`REGION`], which is where most cases live.
    async fn call(&self, operation: &str, body: &Value) -> Reply {
        self.call_in(REGION, operation, body).await
    }

    /// Call `operation` in [`REGION`] and require it to have succeeded.
    async fn ok(&self, operation: &str, body: Value) -> Value {
        let reply = self.call(operation, &body).await;
        assert_eq!(
            reply.status, 200,
            "case {operation} {body}: expected HTTP 200, got {} {}",
            reply.status, reply.body
        );
        reply.body
    }

    /// One of the three operations bound to `/tags/{resourceArn}`, carrying the ARN in
    /// the URI the way a client does: percent-encoded into one path segment.
    async fn tagging(
        &self,
        method: &str,
        arn: &str,
        query: &str,
        body: Option<Value>,
    ) -> Reply {
        let path = format!("/tags/{}{query}", encode_path_label(arn));
        self.send(
            REGION,
            method,
            &path,
            body.map(|body| bytes::Bytes::from(body.to_string())),
        )
        .await
    }

    async fn send(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<bytes::Bytes>,
    ) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{region}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://ivsrealtime.{region}.amazonaws.com{path}"
            ))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(body.unwrap_or_default())
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        let error_code = text(&parsed, "__type");
        Reply {
            status,
            error_code,
            body: parsed,
        }
    }
}

/// One way of writing a key's DER as PEM text, for the rows that vary the encoding.
type Encoding = fn(&[u8]) -> String;

/// A string member, read as the empty string when absent — which is what a client sees
/// for a member the wire omits.
fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Percent-encode one `@httpLabel` value the way an SDK encodes a non-greedy label: an
/// ARN's `:` and `/` are escaped, so the whole ARN is ONE path segment.
fn encode_path_label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Whether `arn` satisfies the modelled `PublicKeyArn` pattern
/// `^arn:aws:ivs:[a-z0-9-]+:[0-9]+:public-key/[a-zA-Z0-9-]+$` for `region`, within the
/// modelled 128-character bound.
fn arn_is_well_formed(arn: &str, region: &str) -> bool {
    let prefix = format!("arn:aws:ivs:{region}:{ACCOUNT}:public-key/");
    let Some(id) = arn.strip_prefix(&prefix) else {
        return false;
    };
    arn.len() <= 128 && !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// A freshly generated public key, as its DER `SubjectPublicKeyInfo`.
///
/// Generated rather than hard-coded, so what the tests import is a real public key of the
/// named algorithm.
fn public_key_der(algorithm: &'static rcgen::SignatureAlgorithm) -> Vec<u8> {
    KeyPair::generate_for(algorithm)
        .expect("the test key pair is generated")
        .public_key_der()
}

/// `der` as the PEM public-key block `openssl ... -pubout` writes: a 64-character wrapped
/// base64 body between the `PUBLIC KEY` armour lines, with LF line endings.
fn pem_of(der: &[u8]) -> String {
    // The codec's own line ending is its choice, and the three encodings here differ on
    // exactly that, so each one states which it wants rather than inheriting a default.
    pem::encode(&pem::Pem::new("PUBLIC KEY", der.to_vec())).replace("\r\n", "\n")
}

/// The same key material, with its base64 body on ONE line instead of wrapped — a
/// different text carrying exactly the same key.
fn unwrapped_pem(der: &[u8]) -> String {
    let encoded = pem_of(der);
    let mut lines = encoded.lines();
    let begin = lines.next().unwrap_or_default().to_string();
    let mut body = String::new();
    let mut end = String::new();
    for line in lines {
        if line.starts_with("-----END") {
            end = line.to_string();
        } else {
            body.push_str(line);
        }
    }
    format!("{begin}\n{body}\n{end}\n")
}

/// The same key material again, with CRLF line endings — the third text for one key, and
/// the one the modelled `\r?\n` in `PublicKeyMaterial`'s pattern admits beside LF.
fn crlf_pem(der: &[u8]) -> String {
    pem_of(der).replace('\n', "\r\n")
}

/// What rule 2 says the fingerprint of `der` is, computed here from the key material
/// alone: `openssl pkey -pubin -pubout -outform DER | md5`, with the digest's bytes joined
/// by colons.
fn expected_fingerprint(der: &[u8]) -> String {
    Md5::digest(der)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Whether `fingerprint` has the published shape: 16 colon-separated pairs of lowercase
/// hex digits, the form an MD5 key fingerprint is printed in.
fn fingerprint_is_well_formed(fingerprint: &str) -> bool {
    let groups: Vec<&str> = fingerprint.split(':').collect();
    groups.len() == 16
        && groups.iter().all(|group| {
            group.len() == 2
                && group
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        })
}

/// The not-found sentence this service states for an ARN it holds no resource for.
fn not_found(identifier: &str) -> String {
    format!("Resource: {identifier} not found")
}

/// Rule 1 and rule 2, over a table spanning the import surface: the algorithm of the key,
/// whether a name is sent and how long it is, and how many tags accompany it.
#[tokio::test]
async fn an_imported_public_key_is_read_back_member_for_member() {
    let cases: Vec<(&str, &'static rcgen::SignatureAlgorithm, Value)> = vec![
        (
            "ecdsa p-384, every optional member omitted",
            &PKCS_ECDSA_P384_SHA384,
            json!({}),
        ),
        (
            "ecdsa p-256, named, one tag",
            &PKCS_ECDSA_P256_SHA256,
            json!({ "name": "token-signing-key", "tags": { "env": "test" } }),
        ),
        (
            "ecdsa p-384, named, several tags",
            &PKCS_ECDSA_P384_SHA384,
            json!({ "name": "stage_token-key", "tags": { "env": "prod", "team": "video" } }),
        ),
        (
            "an empty name, which the modelled length 0..128 allows",
            &PKCS_ECDSA_P256_SHA256,
            json!({ "name": "" }),
        ),
        (
            "a name at the published maximum length",
            &PKCS_ECDSA_P384_SHA384,
            json!({ "name": "k".repeat(MAX_NAME_LENGTH) }),
        ),
    ];

    for (case, algorithm, request) in cases {
        let realtime = RealTime::new();
        let der = public_key_der(algorithm);
        let material = pem_of(&der);
        let mut import = request.clone();
        import["publicKeyMaterial"] = json!(material);

        let imported = realtime.ok("ImportPublicKey", import).await;
        let public_key = imported["publicKey"].clone();

        let arn = text(&public_key, "arn");
        assert!(
            arn_is_well_formed(&arn, REGION),
            "case {case:?}: arn must satisfy the modelled PublicKeyArn pattern, got {arn:?}"
        );
        let expected_name = request
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            text(&public_key, "name"),
            expected_name,
            "case {case:?}: expected the name sent, got {public_key}"
        );
        assert_eq!(
            text(&public_key, "publicKeyMaterial"),
            material,
            "case {case:?}: expected the material imported, got {public_key}"
        );
        assert_eq!(
            text(&public_key, "fingerprint"),
            expected_fingerprint(&der),
            "case {case:?}: expected the MD5 of the key's DER, got {public_key}"
        );
        assert!(
            fingerprint_is_well_formed(&text(&public_key, "fingerprint")),
            "case {case:?}: expected 16 colon-separated lowercase hex pairs, got {public_key}"
        );
        let expected_tags = request.get("tags").cloned().unwrap_or_else(|| json!({}));
        assert_eq!(
            public_key["tags"], expected_tags,
            "case {case:?}: expected the tags sent, got {public_key}"
        );
        // Only the public half is ever handed to this service, so nothing private can be
        // in what it stores or reports.
        assert!(
            !imported.to_string().contains("PRIVATE"),
            "case {case:?}: the response must carry no private material, got {imported}"
        );

        // Rule 1's round trip: the read answers what the import persisted, and a second
        // read answers the same again.
        for read in 1..=2 {
            let got = realtime.ok("GetPublicKey", json!({ "arn": arn })).await;
            assert_eq!(
                got["publicKey"], public_key,
                "case {case:?}: read {read} must answer the imported key, got {got}"
            );
        }

        // And the region holding it is the only one that answers about it: the same ARN
        // offered to another region's endpoint names nothing there.
        let elsewhere = realtime
            .call_in(OTHER_REGION, "GetPublicKey", &json!({ "arn": arn }))
            .await;
        assert_eq!(
            (elsewhere.status, elsewhere.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a key of {REGION} must not be read through {OTHER_REGION}, \
             got {} {}",
            elsewhere.status,
            elsewhere.body
        );
    }
}

/// Rule 2's other half: the fingerprint identifies the KEY, not the text it arrived in,
/// and different keys get different fingerprints.
#[tokio::test]
async fn the_fingerprint_follows_the_key_material_not_its_encoding() {
    let encodings: Vec<(&str, Encoding)> = vec![
        ("wrapped at 64 characters", pem_of),
        ("one-line base64 body", unwrapped_pem),
        ("crlf line endings", crlf_pem),
    ];

    // One key, imported through each encoding into its own store: the fingerprint is the
    // same every time, and so is what the read answers.
    for (algorithm_case, algorithm) in [
        ("ecdsa p-256", &PKCS_ECDSA_P256_SHA256),
        ("ecdsa p-384", &PKCS_ECDSA_P384_SHA384),
    ] {
        let der = public_key_der(algorithm);
        for (case, encode) in &encodings {
            let realtime = RealTime::new();
            let imported = realtime
                .ok(
                    "ImportPublicKey",
                    json!({ "publicKeyMaterial": encode(&der) }),
                )
                .await;
            assert_eq!(
                text(&imported["publicKey"], "fingerprint"),
                expected_fingerprint(&der),
                "case {algorithm_case:?} {case:?}: one key must fingerprint the same \
                 whatever its PEM text, got {imported}"
            );
            let arn = text(&imported["publicKey"], "arn");
            let got = realtime.ok("GetPublicKey", json!({ "arn": arn })).await;
            assert_eq!(
                got["publicKey"], imported["publicKey"],
                "case {algorithm_case:?} {case:?}: the read must answer the imported key, \
                 got {got}"
            );
        }
    }

    // Distinct keys, imported into one store: distinct fingerprints and distinct ARNs,
    // with every name left the same so only the material differs.
    let realtime = RealTime::new();
    let mut fingerprints = BTreeSet::new();
    let mut arns = BTreeSet::new();
    for (case, algorithm) in [
        ("first ecdsa p-256 key", &PKCS_ECDSA_P256_SHA256),
        ("second ecdsa p-256 key", &PKCS_ECDSA_P256_SHA256),
        ("first ecdsa p-384 key", &PKCS_ECDSA_P384_SHA384),
        ("second ecdsa p-384 key", &PKCS_ECDSA_P384_SHA384),
    ] {
        let der = public_key_der(algorithm);
        let imported = realtime
            .ok(
                "ImportPublicKey",
                json!({ "publicKeyMaterial": pem_of(&der), "name": "same-name" }),
            )
            .await;
        let public_key = &imported["publicKey"];
        assert!(
            fingerprints.insert(text(public_key, "fingerprint")),
            "case {case:?}: a different key must fingerprint differently, got {public_key}"
        );
        assert!(
            arns.insert(text(public_key, "arn")),
            "case {case:?}: a different key must get its own arn, got {public_key}"
        );
    }
    assert_eq!(
        fingerprints.len(),
        4,
        "expected four distinct fingerprints, got {fingerprints:?}"
    );
}

/// Rule 3: a key already imported cannot be imported again, whatever name, tags or PEM
/// text the second call carries, and the second call adds nothing.
#[tokio::test]
async fn re_importing_a_key_is_refused_as_a_conflict() {
    let cases: Vec<(&str, Encoding, Value)> = vec![
        ("byte-identical material", pem_of, json!({})),
        ("a different name", pem_of, json!({ "name": "second-import" })),
        ("different tags", pem_of, json!({ "tags": { "env": "other" } })),
        ("the same key on one line", unwrapped_pem, json!({})),
        (
            "the same key with crlf endings",
            crlf_pem,
            json!({ "name": "crlf" }),
        ),
    ];

    for (case, encode, extra) in cases {
        let realtime = RealTime::new();
        let der = public_key_der(&PKCS_ECDSA_P384_SHA384);
        let first = realtime
            .ok(
                "ImportPublicKey",
                json!({ "publicKeyMaterial": pem_of(&der), "name": "first-import" }),
            )
            .await;
        let arn = text(&first["publicKey"], "arn");

        let mut second = extra.clone();
        second["publicKeyMaterial"] = json!(encode(&der));
        let reply = realtime.call("ImportPublicKey", &second).await;
        assert_eq!(
            (
                reply.status,
                reply.error_code.as_str(),
                text(&reply.body, "message").as_str()
            ),
            (
                409,
                "ConflictException",
                "A public-key resource already exists with the given publicKeyMaterial"
            ),
            "case {case:?}: expected the modelled conflict, got {} {}",
            reply.status,
            reply.body
        );

        // Nothing was added or replaced: the first import is still exactly what the store
        // answers.
        let got = realtime.ok("GetPublicKey", json!({ "arn": arn })).await;
        assert_eq!(
            got["publicKey"], first["publicKey"],
            "case {case:?}: the refused import must leave the first key untouched, got {got}"
        );

        // A DIFFERENT key still imports into the same store, so the refusal is about the
        // material, not about having imported once.
        let other = public_key_der(&PKCS_ECDSA_P384_SHA384);
        let accepted = realtime
            .ok(
                "ImportPublicKey",
                json!({ "publicKeyMaterial": pem_of(&other) }),
            )
            .await;
        assert_eq!(
            text(&accepted["publicKey"], "fingerprint"),
            expected_fingerprint(&other),
            "case {case:?}: a different key must still import, got {accepted}"
        );

        // And so does the SAME key through another region's endpoint: a public key is a
        // regional resource, so the conflict is within one region's view.
        let other_region = realtime
            .call_in(
                OTHER_REGION,
                "ImportPublicKey",
                &json!({ "publicKeyMaterial": pem_of(&der) }),
            )
            .await;
        assert_eq!(
            other_region.status, 200,
            "case {case:?}: the same key must import in {OTHER_REGION}, got {} {}",
            other_region.status, other_region.body
        );
        assert!(
            arn_is_well_formed(
                &text(&other_region.body["publicKey"], "arn"),
                OTHER_REGION
            ),
            "case {case:?}: the second region's key must carry its own region's arn, got {}",
            other_region.body
        );
    }
}

/// Rule 4: material that is not an ECDSA public key is refused as invalid, and the store
/// is left empty — a refusal persists nothing.
///
/// `handler_refused` marks the rows whose text satisfies the modelled PEM armour pattern,
/// so the refusal is this operation's own and carries its sentence; the other rows are
/// refused by the modelled constraint itself, which words its own message.
#[tokio::test]
async fn material_that_is_not_an_ecdsa_public_key_is_refused() {
    let der = public_key_der(&PKCS_ECDSA_P384_SHA384);
    let private_pem = KeyPair::generate_for(&PKCS_ECDSA_P384_SHA384)
        .expect("the test key pair is generated")
        .serialize_pem();
    let cases: Vec<(&str, String, bool)> = vec![
        ("the empty string", String::new(), false),
        ("prose, with no PEM armour at all", "not a key".to_string(), false),
        (
            "the base64 body with its armour stripped",
            pem_of(&der)
                .lines()
                .filter(|line| !line.starts_with("-----"))
                .collect::<Vec<_>>()
                .join("\n"),
            false,
        ),
        (
            "a PUBLIC KEY block whose body is not base64",
            "-----BEGIN PUBLIC KEY-----\n!!! not base64 !!!\n-----END PUBLIC KEY-----\n"
                .to_string(),
            false,
        ),
        (
            "a PUBLIC KEY block whose body is not a SubjectPublicKeyInfo",
            pem_of(b"not der at all"),
            true,
        ),
        (
            "a truncated SubjectPublicKeyInfo",
            pem_of(&der[..der.len() / 2]),
            true,
        ),
        (
            "a SubjectPublicKeyInfo with a trailing byte",
            pem_of(&[der.clone(), vec![0x00]].concat()),
            true,
        ),
        ("a PRIVATE KEY block", private_pem, false),
        (
            // The algorithm of the key matters, not just its structure: an Ed25519
            // SubjectPublicKeyInfo is a well-formed public key of the WRONG kind for
            // signing stage participant tokens, which uses ECDSA.
            "an Ed25519 public key, which is not ECDSA",
            pem_of(&public_key_der(&PKCS_ED25519)),
            true,
        ),
    ];

    for (case, material, handler_refused) in cases {
        let realtime = RealTime::new();
        let reply = realtime
            .call("ImportPublicKey", &json!({ "publicKeyMaterial": material }))
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the modelled validation refusal, got {} {}",
            reply.status,
            reply.body
        );
        if handler_refused {
            assert_eq!(
                text(&reply.body, "message"),
                "Request member: publicKeyMaterial failed to satisfy the constraints: \
                 could not decode as a PEM file",
                "case {case:?}: expected the import's own refusal sentence, got {}",
                reply.body
            );
        }

        // Nothing was persisted: the real key still imports, and the key it answers is
        // the one that material fingerprints to.
        let imported = realtime
            .ok(
                "ImportPublicKey",
                json!({ "publicKeyMaterial": pem_of(&der) }),
            )
            .await;
        assert_eq!(
            text(&imported["publicKey"], "fingerprint"),
            expected_fingerprint(&der),
            "case {case:?}: a refused import must persist nothing, got {imported}"
        );
    }
}

/// Rule 5: an ARN of the modelled form that names no imported key is a 404 with this
/// service's not-found sentence, not an empty success.
///
/// The rows differ in WHY nothing is held: an id nothing minted, a key of another region,
/// and an id minted for a resource of a different kind.
#[tokio::test]
async fn reading_a_public_key_that_was_never_imported_is_a_404() {
    let realtime = RealTime::new();
    let stage = realtime.ok("CreateStage", json!({ "name": "unrelated" })).await;
    let stage_id = text(&stage["stage"], "arn")
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();

    let cases: Vec<(&str, String)> = vec![
        (
            "an id nothing minted",
            format!("arn:aws:ivs:{REGION}:{ACCOUNT}:public-key/abcdef123456"),
        ),
        (
            "an id nothing minted, in another region",
            format!("arn:aws:ivs:{OTHER_REGION}:{ACCOUNT}:public-key/abcdef123456"),
        ),
        (
            "a public-key arn carrying the id of a stage this store does hold",
            format!("arn:aws:ivs:{REGION}:{ACCOUNT}:public-key/{stage_id}"),
        ),
    ];

    for (case, arn) in cases {
        let reply = realtime.call("GetPublicKey", &json!({ "arn": arn })).await;
        assert_eq!(
            (
                reply.status,
                reply.error_code.as_str(),
                text(&reply.body, "message")
            ),
            (404, "ResourceNotFoundException", not_found(&arn)),
            "case {case:?}: expected the modelled not-found, got {} {}",
            reply.status,
            reply.body
        );
    }
}

/// Rule 6: the `/tags/{resourceArn}` surface resolves a public key, so both its sides
/// reach the record `ImportPublicKey` persisted.
///
/// The rows differ in the tag set the import carried and in the set the write then
/// applies, which is what the surface generalises over.
#[tokio::test]
async fn a_public_keys_tags_are_reached_through_the_tagging_surface() {
    let rows: Vec<(&str, Value, HashMap<String, String>)> = vec![
        (
            "imported untagged, one tag written",
            json!({}),
            HashMap::from([("team".to_string(), "video".to_string())]),
        ),
        (
            "imported with one tag, two written",
            json!({ "env": "test" }),
            HashMap::from([
                ("env".to_string(), "prod".to_string()),
                ("cost-centre".to_string(), "42".to_string()),
            ]),
        ),
    ];

    for (case, imported_tags, written) in rows {
        let realtime = RealTime::new();
        let der = public_key_der(&PKCS_ECDSA_P384_SHA384);
        let imported = realtime
            .ok(
                "ImportPublicKey",
                json!({ "publicKeyMaterial": pem_of(&der), "tags": imported_tags }),
            )
            .await;
        let arn = text(&imported["publicKey"], "arn");

        // The read side answers for the record the import persisted.
        let listed = realtime.tagging("GET", &arn, "", None).await;
        assert_eq!(
            (listed.status, listed.body["tags"].clone()),
            (200, imported_tags.clone()),
            "case {case:?}: ListTagsForResource must answer the tags the import carried, \
             got {} {}",
            listed.status,
            listed.body
        );

        // The write side reaches the same record, and `GetPublicKey` reports what it
        // wrote.
        let tagged = realtime
            .tagging("POST", &arn, "", Some(json!({ "tags": written })))
            .await;
        assert_eq!(
            tagged.status, NO_CONTENT_STATUS,
            "case {case:?}: TagResource must answer {NO_CONTENT_STATUS}, got {} {}",
            tagged.status, tagged.body
        );
        let got = realtime.ok("GetPublicKey", json!({ "arn": arn })).await;
        let mut expected: HashMap<String, String> =
            serde_json::from_value(imported_tags.clone()).expect("the row's tags are a map");
        expected.extend(written.clone());
        assert_eq!(
            serde_json::from_value::<HashMap<String, String>>(got["publicKey"]["tags"].clone())
                .expect("the key reports its tags as a map"),
            expected,
            "case {case:?}: GetPublicKey must report the tags TagResource wrote, got {got}"
        );

        // And the removing write, read back through the key itself.
        let query = {
            let mut keys: Vec<String> = written.keys().cloned().collect();
            keys.sort();
            keys.iter()
                .map(|key| format!("tagKeys={key}"))
                .collect::<Vec<String>>()
                .join("&")
        };
        let untagged = realtime
            .tagging("DELETE", &arn, &format!("?{query}"), None)
            .await;
        assert_eq!(
            untagged.status, NO_CONTENT_STATUS,
            "case {case:?}: UntagResource must answer {NO_CONTENT_STATUS}, got {} {}",
            untagged.status, untagged.body
        );
        let got = realtime.ok("GetPublicKey", json!({ "arn": arn })).await;
        let remaining: HashMap<String, String> =
            serde_json::from_value(got["publicKey"]["tags"].clone())
                .expect("the key reports its tags as a map");
        for key in written.keys() {
            assert!(
                !remaining.contains_key(key),
                "case {case:?}: {key:?} must be gone after UntagResource, got {got}"
            );
        }
    }
}
