// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for importing and reading back an IVS playback key pair.
//!
//! Every case goes through the real front door — `MultiServiceRouter::dispatch` with
//! a restJson1 request, exactly as a signed `aws ivs` call arrives — so what it
//! asserts is the emulator's HTTP behaviour, not a handler called directly.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs
//! (several key algorithms, several PEM encodings of one key, names and tag maps of
//! different sizes):
//!
//! 1. `ImportPlaybackKeyPair` persists the imported key as a playback key pair and
//!    answers it; `GetPlaybackKeyPair` answers that same pair afterwards, and a read
//!    never changes what a later read sees.
//! 2. A pair's `fingerprint` is the MD5 digest of the key's DER
//!    `SubjectPublicKeyInfo`, rendered as 16 colon-separated lowercase hex byte
//!    pairs — so it is derived from the imported material alone: the same key always
//!    fingerprints to the same value however its PEM is wrapped, and two different
//!    keys fingerprint differently.
//! 3. Importing material already imported is refused with `ConflictException`
//!    (HTTP 409) and adds nothing.
//! 4. Material that is not an ECDSA public key in a `PUBLIC KEY` PEM block is
//!    refused with `ValidationException` (HTTP 400) and persists nothing.

use std::collections::BTreeSet;
use std::sync::Arc;

use md5::{Digest, Md5};
use rcgen::{KeyPair, PKCS_ECDSA_P256_SHA256, PKCS_ECDSA_P384_SHA384, PKCS_ED25519};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivs_amazoninteractivevideoservice::EmulatedService;

/// A SigV4 `Authorization` header is how a request names the service and the region
/// it is addressed to, so every call carries one. The signature itself is never
/// checked here: request authentication is off unless a router turns it on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTIVS/20260101/us-east-1/ivs/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The ARN prefix every playback key pair this emulator mints must carry, from the
/// modelled `PlaybackKeyPairArn` pattern
/// `^arn:aws:ivs:[a-z0-9-]+:[0-9]+:playback-key/[a-zA-Z0-9-]+$` plus the canonical
/// emulator account id.
const KEY_PAIR_ARN_PREFIX: &str = "arn:aws:ivs:us-east-1:000000000000:playback-key/";

/// One IVS emulator over one shared engine, reached the way the router reaches it.
///
/// Each test suite under `tests/all/` is its own private module, so this is the
/// per-suite copy of that two-line harness rather than a shared one.
struct Ivs {
    router: MultiServiceRouter,
}

/// One answer from the emulator: the status, the wire error code (the restJson1
/// `__type` discriminator) and the parsed body.
struct Reply {
    status: u16,
    error_code: String,
    body: Value,
}

impl Ivs {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, operation: &str, body: &Value) -> Reply {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://ivs.us-east-1.amazonaws.com/{operation}"))
            .header("content-type", "application/json")
            .header("authorization", AUTHORIZATION)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        let error_code = parsed
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Reply {
            status,
            error_code,
            body: parsed,
        }
    }

    /// Call `operation` and require it to have succeeded, returning the body.
    async fn ok(&self, operation: &str, body: Value) -> Value {
        let reply = self.call(operation, &body).await;
        assert_eq!(
            reply.status, 200,
            "case {operation} {body}: expected HTTP 200, got {} {}",
            reply.status, reply.body
        );
        reply.body
    }
}

/// One way of writing a key's DER as PEM text, for the rows that vary the encoding.
type Encoding = fn(&[u8]) -> String;

/// A string member, read as the empty string when absent — which is what a client
/// sees for a member the wire omits.
fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Whether `arn` satisfies the modelled `PlaybackKeyPairArn` pattern: the fixed
/// `arn:aws:ivs:<region>:<account>:playback-key/` prefix followed by a non-empty
/// `[a-zA-Z0-9-]+` id, within the modelled 128-character bound.
fn arn_is_well_formed(arn: &str) -> bool {
    let Some(id) = arn.strip_prefix(KEY_PAIR_ARN_PREFIX) else {
        return false;
    };
    arn.len() <= 128 && !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// A freshly generated public key, as its DER `SubjectPublicKeyInfo`.
///
/// Generated rather than hard-coded, so what the tests import is a real public key
/// of the named algorithm.
fn public_key_der(algorithm: &'static rcgen::SignatureAlgorithm) -> Vec<u8> {
    KeyPair::generate_for(algorithm)
        .expect("the test key pair is generated")
        .public_key_der()
}

/// `der` as the PEM public-key block `openssl ... -pubout` writes: a 64-character
/// wrapped base64 body between the `PUBLIC KEY` armour lines.
fn pem_of(der: &[u8]) -> String {
    pem::encode(&pem::Pem::new("PUBLIC KEY", der.to_vec()))
}

/// The same key material, with its base64 body on ONE line instead of wrapped —
/// a different text carrying exactly the same key.
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

/// The same key material again, with CRLF line endings and surrounding whitespace —
/// the third text for one key.
fn crlf_pem(der: &[u8]) -> String {
    format!("\n  {}\n", pem_of(der).replace('\n', "\r\n").trim_end())
}

/// What rule 2 says the fingerprint of `der` is, computed here from the key material
/// alone: `openssl pkey -pubin -pubout -outform DER | md5`, with the digest's bytes
/// joined by colons.
fn expected_fingerprint(der: &[u8]) -> String {
    Md5::digest(der)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Whether `fingerprint` has the published shape: 16 colon-separated pairs of
/// lowercase hex digits, the form an MD5 key fingerprint is printed in.
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

/// Rule 1 and rule 2, over a table spanning the import surface: the algorithm of the
/// key, whether a name is sent and how long it is, and how many tags accompany it.
#[tokio::test]
async fn an_imported_key_pair_is_read_back_member_for_member() {
    let cases: Vec<(&str, &'static rcgen::SignatureAlgorithm, Value)> = vec![
        ("ecdsa p-256, every optional member omitted", &PKCS_ECDSA_P256_SHA256, json!({})),
        (
            "ecdsa p-384, named, one tag",
            &PKCS_ECDSA_P384_SHA384,
            json!({ "name": "demo-key", "tags": { "env": "test" } }),
        ),
        (
            "ecdsa p-256, named, several tags",
            &PKCS_ECDSA_P256_SHA256,
            json!({ "name": "viewer_token-key", "tags": { "env": "prod", "team": "video" } }),
        ),
        (
            "an empty name, which the modelled length 0..128 allows",
            &PKCS_ECDSA_P256_SHA256,
            json!({ "name": "" }),
        ),
        (
            "a 128-character name, the modelled upper bound",
            &PKCS_ECDSA_P384_SHA384,
            json!({ "name": "k".repeat(128) }),
        ),
    ];

    for (case, algorithm, request) in cases {
        let ivs = Ivs::new();
        let der = public_key_der(algorithm);
        let mut import = request.clone();
        import["publicKeyMaterial"] = json!(pem_of(&der));

        let imported = ivs.ok("ImportPlaybackKeyPair", import).await;
        let key_pair = imported["keyPair"].clone();

        let arn = text(&key_pair, "arn");
        assert!(
            arn_is_well_formed(&arn),
            "case {case:?}: arn must satisfy the modelled PlaybackKeyPairArn pattern, got {arn:?}"
        );
        let expected_name = request.get("name").and_then(Value::as_str).unwrap_or_default();
        assert_eq!(
            text(&key_pair, "name"),
            expected_name,
            "case {case:?}: expected the name sent, got {key_pair}"
        );
        assert_eq!(
            text(&key_pair, "fingerprint"),
            expected_fingerprint(&der),
            "case {case:?}: expected the MD5 of the key's DER, got {key_pair}"
        );
        assert!(
            fingerprint_is_well_formed(&text(&key_pair, "fingerprint")),
            "case {case:?}: expected 16 colon-separated lowercase hex pairs, got {key_pair}"
        );
        let expected_tags = request.get("tags").cloned().unwrap_or_else(|| json!({}));
        assert_eq!(
            key_pair["tags"], expected_tags,
            "case {case:?}: expected the tags sent, got {key_pair}"
        );
        // The private material is never part of the resource: only what was
        // imported, which is the public portion.
        assert!(
            !imported.to_string().contains("PRIVATE"),
            "case {case:?}: the response must carry no private material, got {imported}"
        );

        // Rule 1's round trip: the read answers what the import persisted, and a
        // second read answers the same again.
        for read in 1..=2 {
            let got = ivs
                .ok("GetPlaybackKeyPair", json!({ "arn": arn }))
                .await;
            assert_eq!(
                got["keyPair"], key_pair,
                "case {case:?}: read {read} must answer the imported pair, got {got}"
            );
        }
    }
}

/// Rule 2's other half: the fingerprint identifies the KEY, not the text it arrived
/// in, and different keys get different fingerprints.
#[tokio::test]
async fn the_fingerprint_follows_the_key_material_not_its_encoding() {
    let encodings: Vec<(&str, Encoding)> = vec![
        ("wrapped at 64 characters", pem_of),
        ("one-line base64 body", unwrapped_pem),
        ("crlf line endings and surrounding whitespace", crlf_pem),
    ];

    // One key, imported through each encoding into its own store: the fingerprint is
    // the same every time.
    for (algorithm_case, algorithm) in [
        ("ecdsa p-256", &PKCS_ECDSA_P256_SHA256),
        ("ecdsa p-384", &PKCS_ECDSA_P384_SHA384),
    ] {
        let der = public_key_der(algorithm);
        for (case, encode) in &encodings {
            let ivs = Ivs::new();
            let imported = ivs
                .ok(
                    "ImportPlaybackKeyPair",
                    json!({ "publicKeyMaterial": encode(&der) }),
                )
                .await;
            assert_eq!(
                text(&imported["keyPair"], "fingerprint"),
                expected_fingerprint(&der),
                "case {algorithm_case:?} {case:?}: one key must fingerprint the same whatever its PEM text, got {imported}"
            );
        }
    }

    // Distinct keys, imported into one store: distinct fingerprints and distinct
    // ARNs, with every name left the same so only the material differs.
    let ivs = Ivs::new();
    let mut fingerprints = BTreeSet::new();
    let mut arns = BTreeSet::new();
    for (case, algorithm) in [
        ("first ecdsa p-256 key", &PKCS_ECDSA_P256_SHA256),
        ("second ecdsa p-256 key", &PKCS_ECDSA_P256_SHA256),
        ("first ecdsa p-384 key", &PKCS_ECDSA_P384_SHA384),
        ("second ecdsa p-384 key", &PKCS_ECDSA_P384_SHA384),
    ] {
        let der = public_key_der(algorithm);
        let imported = ivs
            .ok(
                "ImportPlaybackKeyPair",
                json!({ "publicKeyMaterial": pem_of(&der), "name": "same-name" }),
            )
            .await;
        let key_pair = &imported["keyPair"];
        assert!(
            fingerprints.insert(text(key_pair, "fingerprint")),
            "case {case:?}: a different key must fingerprint differently, got {key_pair}"
        );
        assert!(
            arns.insert(text(key_pair, "arn")),
            "case {case:?}: a different key must get its own arn, got {key_pair}"
        );
    }
    assert_eq!(
        fingerprints.len(),
        4,
        "expected four distinct fingerprints, got {fingerprints:?}"
    );
}

/// Rule 3: a key already imported cannot be imported again, whatever name, tags or
/// PEM text the second call carries, and the second call adds nothing.
#[tokio::test]
async fn re_importing_a_key_is_refused_as_a_conflict() {
    let cases: Vec<(&str, Encoding, Value)> = vec![
        ("byte-identical material", pem_of, json!({})),
        ("a different name", pem_of, json!({ "name": "second-import" })),
        ("different tags", pem_of, json!({ "tags": { "env": "other" } })),
        ("the same key on one line", unwrapped_pem, json!({})),
        ("the same key with crlf endings", crlf_pem, json!({ "name": "crlf" })),
    ];

    for (case, encode, extra) in cases {
        let ivs = Ivs::new();
        let der = public_key_der(&PKCS_ECDSA_P384_SHA384);
        let first = ivs
            .ok(
                "ImportPlaybackKeyPair",
                json!({ "publicKeyMaterial": pem_of(&der), "name": "first-import" }),
            )
            .await;
        let arn = text(&first["keyPair"], "arn");

        let mut second = extra.clone();
        second["publicKeyMaterial"] = json!(encode(&der));
        let reply = ivs.call("ImportPlaybackKeyPair", &second).await;
        assert_eq!(
            reply.status, 409,
            "case {case:?}: expected HTTP 409, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code, "ConflictException",
            "case {case:?}: expected ConflictException, got {} {}",
            reply.error_code, reply.body
        );

        // Nothing was added or replaced: the first import is still exactly what the
        // store answers.
        let got = ivs.ok("GetPlaybackKeyPair", json!({ "arn": arn })).await;
        assert_eq!(
            got["keyPair"], first["keyPair"],
            "case {case:?}: the refused import must leave the first pair untouched, got {got}"
        );

        // And a DIFFERENT key still imports into the same store, so the refusal is
        // about the material, not about having imported once.
        let other = public_key_der(&PKCS_ECDSA_P384_SHA384);
        let accepted = ivs
            .ok(
                "ImportPlaybackKeyPair",
                json!({ "publicKeyMaterial": pem_of(&other) }),
            )
            .await;
        assert_eq!(
            text(&accepted["keyPair"], "fingerprint"),
            expected_fingerprint(&other),
            "case {case:?}: a different key must still import, got {accepted}"
        );
    }
}

/// Rule 4: material that is not an ECDSA public key is refused as invalid, and the
/// store is left empty — a refusal persists nothing.
#[tokio::test]
async fn material_that_is_not_an_ecdsa_public_key_is_refused() {
    let der = public_key_der(&PKCS_ECDSA_P384_SHA384);
    let private_pem = KeyPair::generate_for(&PKCS_ECDSA_P384_SHA384)
        .expect("the test key pair is generated")
        .serialize_pem();
    let cases: Vec<(&str, String)> = vec![
        ("the empty string", String::new()),
        ("prose, with no PEM armour at all", "not a key".to_string()),
        (
            "the base64 body with its armour stripped",
            pem_of(&der)
                .lines()
                .filter(|line| !line.starts_with("-----"))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        (
            "a PUBLIC KEY block whose body is not base64",
            "-----BEGIN PUBLIC KEY-----\n!!! not base64 !!!\n-----END PUBLIC KEY-----\n".to_string(),
        ),
        (
            "a PUBLIC KEY block whose body is not a SubjectPublicKeyInfo",
            pem_of(b"not der at all"),
        ),
        (
            "a truncated SubjectPublicKeyInfo",
            pem_of(&der[..der.len() / 2]),
        ),
        (
            "a SubjectPublicKeyInfo with a trailing byte",
            pem_of(&[der.clone(), vec![0x00]].concat()),
        ),
        ("a PRIVATE KEY block", private_pem),
        (
            // The algorithm of the key matters, not just its structure: an Ed25519
            // SubjectPublicKeyInfo is a well-formed public key of the WRONG kind for
            // a playback key pair, which signs viewer tokens with ECDSA.
            "an Ed25519 public key, which is not ECDSA",
            pem_of(&public_key_der(&PKCS_ED25519)),
        ),
    ];

    for (case, material) in cases {
        let ivs = Ivs::new();
        let reply = ivs
            .call(
                "ImportPlaybackKeyPair",
                &json!({ "publicKeyMaterial": material }),
            )
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {} {}",
            reply.error_code, reply.body
        );

        // Nothing was persisted: the real key still imports, and the pair it answers
        // is the only one the store holds.
        let imported = ivs
            .ok(
                "ImportPlaybackKeyPair",
                json!({ "publicKeyMaterial": pem_of(&der) }),
            )
            .await;
        assert_eq!(
            text(&imported["keyPair"], "fingerprint"),
            expected_fingerprint(&der),
            "case {case:?}: a refused import must persist nothing, got {imported}"
        );
    }
}

/// The reader's own refusal: an ARN of the modelled form that names no imported pair
/// is a 404, not an empty success.
#[tokio::test]
async fn reading_a_key_pair_that_was_never_imported_is_a_404() {
    let ivs = Ivs::new();
    let cases = [
        "an id nothing minted",
        "an id nothing minted, in another region",
    ];
    let arns = [
        format!("{KEY_PAIR_ARN_PREFIX}abcdef123456"),
        "arn:aws:ivs:eu-west-1:000000000000:playback-key/abcdef123456".to_string(),
    ];

    for (case, arn) in cases.iter().zip(arns) {
        let reply = ivs.call("GetPlaybackKeyPair", &json!({ "arn": arn })).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {} {}",
            reply.error_code, reply.body
        );
    }
}
