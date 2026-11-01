// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ACME data plane an ACM ACME endpoint publishes: the JWS
//! interactions a client performs at the URL `AcmeEndpoint$EndpointUrl` advertises, and
//! the account state they leave for the control plane to report.
//!
//! RULE 1 (registration): a JWS `POST` to the directory's `newAccount` whose signature
//! checks out under the key its protected header embeds, whose `url` is the resource it
//! was sent to, whose nonce this endpoint issued and has not seen back, and which
//! presents an external account binding MAC-signed with the credentials
//! `GetAcmeExternalAccountBindingCredentials` reported, registers an account: `201` with
//! the account's own URL in `Location`. An account IS its key pair, so the same key
//! registering again is answered `200` with the SAME URL, and a different key gets its
//! own account. Every account is then reported by `DescribeAcmeAccount` and
//! `ListAcmeAccounts`.
//!
//! RULE 2 (single-use nonce): a nonce is accepted once. A JWS carrying a nonce this
//! endpoint has already seen back — or never issued — is refused `400` with the problem
//! type `urn:ietf:params:acme:error:badNonce`, and that refusal itself carries a
//! `Replay-Nonce` the server then accepts.
//!
//! RULE 3 (an account's authority is its key, and it can be withdrawn): a request to an
//! account's own URL is served only while that account is `VALID`; from the instant
//! `RevokeAcmeAccount` (or the client's own deactivation) moves it out of `VALID`, the
//! same request signed with the same key is refused `400`
//! `urn:ietf:params:acme:error:unauthorized` and the account's stored contacts are left
//! exactly as the last accepted update left them — while another account on the same
//! endpoint goes on updating itself.
//!
//! RULE 4 (the binding is what authorizes a registration): a registration presenting no
//! external account binding is refused `400`
//! `urn:ietf:params:acme:error:externalAccountRequired`; one whose credential this
//! endpoint does not account for — a forged MAC, another binding's key, a key id it never
//! issued, a binding whose authority has been withdrawn — is a credential the server
//! REJECTED and is refused `403` `urn:ietf:params:acme:error:unauthorized`, while one
//! whose FORM is wrong (bound to another key, signed for another url) is a defect in the
//! request and is refused `400` `urn:ietf:params:acme:error:malformed`.
//!
//! Every assertion goes through [`MultiServiceRouter::dispatch`], the same path the
//! emulator serves production traffic on, and every URL and credential the client half
//! uses is one a DIFFERENT operation advertised (`CreateAcmeEndpoint` ->
//! `DescribeAcmeEndpoint` -> the directory, `CreateAcmeExternalAccountBinding` ->
//! `GetAcmeExternalAccountBindingCredentials`), so an answer that is not computed from the
//! persisted state cannot pass.
//!
//! The tables span what the rules generalise over: the ACCOUNT KEY (four key types and
//! signature algorithms — ECDSA on two curves, Ed25519 and RSA — each a different JWK
//! family), the contact lists a registration and an update carry, how a nonce can fail to
//! be live, how an external account binding can fail to authorize, both terminal account
//! statuses, and the page sizes `ListAcmeAccounts` is paged with. The client half of the
//! wire is built with the same maintained cryptography libraries the rest of this
//! workspace uses, never by the code under test.

use std::collections::{BTreeMap, BTreeSet};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::acme_directory_endpoint::{activate_endpoints_now, body_json, split_url};
use crate::acme_domain_validation_lifecycle::endpoint_body;
use crate::acme_endpoint_lifecycle::create_endpoint;
use crate::acme_external_account_binding_lifecycle::create_binding;
use crate::certificate_lifecycle::{call, error_code, error_message, router_with_engine, ACCOUNT};

/// `bytes` as one base64url segment, through the workspace's own JOSE coding — so the
/// client half of this wire and the server half cannot disagree about the encoding.
fn base64url(bytes: &[u8]) -> String {
    ::service_common::jwt::encode_base64url(bytes)
}

/// One ACME account key pair, as a client holds one: the `alg` it signs with, the public
/// half as the JWK a registration embeds, and the private half to sign with.
enum AccountKey {
    /// ECDSA on a NIST curve, signing in the fixed `R || S` form JWS uses.
    Ecdsa {
        algorithm: &'static str,
        curve: &'static str,
        coordinate_bytes: usize,
        pair: ::ring::signature::EcdsaKeyPair,
    },
    /// Ed25519 (RFC 8037).
    Ed25519 {
        pair: ::ring::signature::Ed25519KeyPair,
    },
    /// RSA, signing RSASSA-PKCS1-v1_5 over SHA-256.
    Rsa {
        modulus: Vec<u8>,
        exponent: Vec<u8>,
        pair: ::aws_lc_rs::signature::RsaKeyPair,
    },
}

impl AccountKey {
    /// A fresh ECDSA key pair on the curve `algorithm` is defined over.
    fn ecdsa(algorithm: &'static str) -> Self {
        let (signing, curve, coordinate_bytes): (
            &'static ::ring::signature::EcdsaSigningAlgorithm,
            &'static str,
            usize,
        ) = match algorithm {
            "ES384" => (
                &::ring::signature::ECDSA_P384_SHA384_FIXED_SIGNING,
                "P-384",
                48,
            ),
            _ => (
                &::ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
                "P-256",
                32,
            ),
        };
        let rng = ::ring::rand::SystemRandom::new();
        let document = ::ring::signature::EcdsaKeyPair::generate_pkcs8(signing, &rng)
            .expect("the operating system CSPRNG yields an ECDSA key pair");
        let pair = ::ring::signature::EcdsaKeyPair::from_pkcs8(signing, document.as_ref(), &rng)
            .expect("a freshly generated ECDSA key pair parses");
        Self::Ecdsa {
            algorithm,
            curve,
            coordinate_bytes,
            pair,
        }
    }

    /// A fresh Ed25519 key pair.
    fn ed25519() -> Self {
        let rng = ::ring::rand::SystemRandom::new();
        let document = ::ring::signature::Ed25519KeyPair::generate_pkcs8(&rng)
            .expect("the operating system CSPRNG yields an Ed25519 key pair");
        Self::Ed25519 {
            pair: ::ring::signature::Ed25519KeyPair::from_pkcs8(document.as_ref())
                .expect("a freshly generated Ed25519 key pair parses"),
        }
    }

    /// A fresh RSA-2048 key pair. `ring`, which the service verifies through, generates
    /// no RSA key, so the pair comes from the other maintained cryptography library this
    /// workspace already builds.
    fn rsa() -> Self {
        use ::aws_lc_rs::encoding::AsDer as _;
        use ::x509_cert::der::Decode as _;

        let generated =
            ::aws_lc_rs::rsa::PrivateDecryptingKey::generate(::aws_lc_rs::rsa::KeySize::Rsa2048)
                .expect("the library yields an RSA-2048 key pair");
        let pkcs8 = generated
            .as_der()
            .expect("the private half encodes")
            .as_ref()
            .to_vec();
        let pair = ::aws_lc_rs::signature::RsaKeyPair::from_pkcs8(&pkcs8)
            .expect("a freshly generated RSA key pair parses");
        // The public half as `(n, e)`, read out of the DER the library hands back with
        // the same structures this crate reads an imported certificate's key with.
        let spki_der = generated
            .public_key()
            .as_der()
            .expect("the public half encodes")
            .as_ref()
            .to_vec();
        let spki = ::x509_cert::spki::SubjectPublicKeyInfoOwned::from_der(&spki_der)
            .expect("the public half is a SubjectPublicKeyInfo");
        let key = ::pkcs1::RsaPublicKey::from_der(
            spki.subject_public_key
                .as_bytes()
                .expect("the key bit string is whole"),
        )
        .expect("an RSA public key");
        Self::Rsa {
            modulus: key.modulus.as_bytes().to_vec(),
            exponent: key.public_exponent.as_bytes().to_vec(),
            pair,
        }
    }

    /// The `alg` a JWS this key signs names (RFC 7518 §3.1).
    fn algorithm(&self) -> &'static str {
        match self {
            Self::Ecdsa { algorithm, .. } => algorithm,
            Self::Ed25519 { .. } => "EdDSA",
            Self::Rsa { .. } => "RS256",
        }
    }

    /// This key's PUBLIC half, as the JWK a registration embeds (RFC 7518 §6, RFC 8037
    /// §2).
    fn jwk(&self) -> Value {
        match self {
            Self::Ecdsa {
                curve,
                coordinate_bytes,
                pair,
                ..
            } => {
                use ::ring::signature::KeyPair as _;
                let width = *coordinate_bytes;
                let point = pair.public_key().as_ref();
                let coordinate = |from: usize| {
                    base64url(
                        point
                            .get(from..from + width)
                            .expect("an uncompressed point carries both coordinates"),
                    )
                };
                json!({
                    "crv": curve,
                    "kty": "EC",
                    "x": coordinate(1),
                    "y": coordinate(1 + width),
                })
            }
            Self::Ed25519 { pair } => {
                use ::ring::signature::KeyPair as _;
                json!({ "crv": "Ed25519", "kty": "OKP", "x": base64url(pair.public_key().as_ref()) })
            }
            Self::Rsa {
                modulus, exponent, ..
            } => json!({ "e": base64url(exponent), "kty": "RSA", "n": base64url(modulus) }),
        }
    }

    /// `signing_input` signed by this key's private half.
    fn sign(&self, signing_input: &str) -> Vec<u8> {
        let message = signing_input.as_bytes();
        match self {
            Self::Ecdsa { pair, .. } => {
                let rng = ::ring::rand::SystemRandom::new();
                pair.sign(&rng, message)
                    .expect("the key signs")
                    .as_ref()
                    .to_vec()
            }
            Self::Ed25519 { pair } => pair.sign(message).as_ref().to_vec(),
            Self::Rsa { pair, .. } => {
                let mut signature = vec![0_u8; pair.public_modulus_len()];
                pair.sign(
                    &::aws_lc_rs::signature::RSA_PKCS1_SHA256,
                    &::aws_lc_rs::rand::SystemRandom::new(),
                    message,
                    &mut signature,
                )
                .expect("the key signs");
                signature
            }
        }
    }
}

/// Which key a JWS identifies itself by (RFC 8555 §6.2).
enum Identification<'a> {
    /// The key itself, as a registration carries it.
    Jwk,
    /// The account that holds it, as every other request carries it.
    KeyId(&'a str),
}

/// The flattened JWS a client posts: the protected header, the payload, and the signature
/// over both (RFC 7515 §7.2.2).
///
/// `payload` of `None` is the empty payload of a POST-as-GET (RFC 8555 §6.3).
fn jws(
    key: &AccountKey,
    identification: &Identification<'_>,
    nonce: &str,
    url: &str,
    payload: Option<&Value>,
) -> Value {
    let mut protected = json!({ "alg": key.algorithm(), "nonce": nonce, "url": url });
    match identification {
        Identification::Jwk => protected["jwk"] = key.jwk(),
        Identification::KeyId(account_url) => protected["kid"] = json!(account_url),
    }
    let protected = base64url(protected.to_string().as_bytes());
    let payload = match payload {
        Some(payload) => base64url(payload.to_string().as_bytes()),
        None => String::new(),
    };
    let signing_input = format!("{protected}.{payload}");
    json!({
        "protected": protected,
        "payload": payload,
        "signature": base64url(&key.sign(&signing_input)),
    })
}

/// The `externalAccountBinding` member of a registration: a JWS MAC-signed with the key
/// the CA issued, whose payload is the account key being registered (RFC 8555 §7.3.4).
///
/// `mac_key` is signed with as the octets the credential's base64url text encodes, which
/// is how a client reads what `GetAcmeExternalAccountBindingCredentials` handed it.
fn external_account_binding(key_id: &str, mac_key: &str, url: &str, account_jwk: &Value) -> Value {
    let protected = base64url(
        json!({ "alg": "HS256", "kid": key_id, "url": url })
            .to_string()
            .as_bytes(),
    );
    let payload = base64url(account_jwk.to_string().as_bytes());
    let signing_input = format!("{protected}.{payload}");
    let octets = ::service_common::jwt::decode_base64url(mac_key)
        .expect("the MAC key is base64url text");
    let tag = ::ring::hmac::sign(
        &::ring::hmac::Key::new(::ring::hmac::HMAC_SHA256, &octets),
        signing_input.as_bytes(),
    );
    json!({
        "protected": protected,
        "payload": payload,
        "signature": base64url(tag.as_ref()),
    })
}

/// POST `body` to `url` exactly as an ACME client does: the URL's own host, the JOSE
/// media type, and NO AWS credential of any kind.
async fn post(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    body: &Value,
) -> ::http::Response<Bytes> {
    let (host, path) = split_url(case, url);
    let request = ::http::Request::builder()
        .method("POST")
        .uri(path)
        .header("host", host)
        .header("content-type", "application/jose+json")
        .body(Bytes::from(body.to_string()))
        .expect("a valid request");
    router.dispatch(request).await
}

/// One response header, as text. Field names are matched case-insensitively, as HTTP
/// defines them (RFC 9110 §5.1).
fn header(response: &::http::Response<Bytes>, name: &str) -> String {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// One endpoint's ACME surface, as a client reaches it: the directory's resource map and
/// the external-account-binding credentials to register with.
struct Enrollment {
    endpoint_arn: String,
    binding_arn: String,
    directory: Value,
    key_id: String,
    mac_key: String,
}

impl Enrollment {
    /// The `newAccount` URL the directory names.
    fn new_account(&self) -> String {
        self.directory["newAccount"]
            .as_str()
            .expect("the directory names newAccount")
            .to_string()
    }

    /// The `newNonce` URL the directory names.
    fn new_nonce(&self) -> String {
        self.directory["newNonce"]
            .as_str()
            .expect("the directory names newNonce")
            .to_string()
    }
}

/// Stand one ACME endpoint up and read everything a client needs off the operations that
/// publish it: the directory at the advertised `EndpointUrl`, and the credentials of an
/// external account binding created on it.
async fn enrol(
    router: &MultiServiceRouter,
    engine: &::std::sync::Arc<::engine::InMemoryEngine>,
    case: &str,
    endpoint: &Value,
) -> Enrollment {
    let endpoint_arn = create_endpoint(router, case, endpoint).await;
    activate_endpoints_now(engine);
    let (status, described) = call(
        router,
        "DescribeAcmeEndpoint",
        &json!({ "AcmeEndpointArn": endpoint_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeAcmeEndpoint must succeed, got {status} {described}"
    );
    let endpoint_url = described["AcmeEndpoint"]["EndpointUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the endpoint carried no EndpointUrl"))
        .to_string();

    let (host, path) = split_url(case, &endpoint_url);
    let request = ::http::Request::builder()
        .method("GET")
        .uri(path)
        .header("host", host)
        .body(Bytes::new())
        .expect("a valid request");
    let response = router.dispatch(request).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "case {case:?}: the advertised directory must be served, got {}",
        body_json(case, &response)
    );
    let directory = body_json(case, &response);

    let binding = create_binding(
        router,
        case,
        &json!({
            "AcmeEndpointArn": endpoint_arn,
            "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/AcmeIssuer"),
        }),
    )
    .await;
    let binding_arn = binding["AcmeExternalAccountBindingArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the binding carried no ARN"))
        .to_string();
    let (status, credentials) = call(
        router,
        "GetAcmeExternalAccountBindingCredentials",
        &json!({ "AcmeExternalAccountBindingArn": binding_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the binding's credentials must be reported, got {status} {credentials}"
    );
    Enrollment {
        endpoint_arn,
        binding_arn,
        directory,
        key_id: credentials["KeyId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no KeyId"))
            .to_string(),
        mac_key: credentials["MacKey"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no MacKey"))
            .to_string(),
    }
}

/// A fresh nonce from the `newNonce` resource, as a client takes one before every POST.
async fn fresh_nonce(router: &MultiServiceRouter, case: &str, enrollment: &Enrollment) -> String {
    let url = enrollment.new_nonce();
    let (host, path) = split_url(case, &url);
    let request = ::http::Request::builder()
        .method("HEAD")
        .uri(path)
        .header("host", host)
        .body(Bytes::new())
        .expect("a valid request");
    let response = router.dispatch(request).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "case {case:?}: HEAD {url} must answer 200"
    );
    let nonce = header(&response, "replay-nonce");
    assert!(
        !nonce.is_empty(),
        "case {case:?}: HEAD {url} must answer a Replay-Nonce"
    );
    nonce
}

/// A registration payload: the terms agreement a client sends, `contact` when the row has
/// contacts, and the external account binding over the account key.
fn registration(
    enrollment: &Enrollment,
    key: &AccountKey,
    contacts: &[&str],
    binding: Option<Value>,
) -> Value {
    let mut payload = json!({ "termsOfServiceAgreed": true });
    if !contacts.is_empty() {
        payload["contact"] = json!(contacts);
    }
    if let Some(binding) = binding {
        payload["externalAccountBinding"] = binding;
    } else {
        payload["externalAccountBinding"] = external_account_binding(
            &enrollment.key_id,
            &enrollment.mac_key,
            &enrollment.new_account(),
            &key.jwk(),
        );
    }
    payload
}

/// Register `key` with `enrollment`, asserted to be accepted, returning
/// `(status, account_url, account document)`.
async fn register(
    router: &MultiServiceRouter,
    case: &str,
    enrollment: &Enrollment,
    key: &AccountKey,
    contacts: &[&str],
) -> (u16, String, Value) {
    let nonce = fresh_nonce(router, case, enrollment).await;
    let url = enrollment.new_account();
    let body = jws(
        key,
        &Identification::Jwk,
        &nonce,
        &url,
        Some(&registration(enrollment, key, contacts, None)),
    );
    let response = post(router, case, &url, &body).await;
    let status = response.status().as_u16();
    let document = body_json(case, &response);
    assert!(
        status == 200 || status == 201,
        "case {case:?}: a correctly signed newAccount must be accepted, got {status} {document}"
    );
    let location = header(&response, "location");
    assert!(
        !location.is_empty(),
        "case {case:?}: an accepted registration must answer the account URL in Location, got {document}"
    );
    (status, location, document)
}

/// `DescribeAcmeAccount` for one account, as `(status, body)`.
async fn describe_account(
    router: &MultiServiceRouter,
    endpoint_arn: &str,
    account_url: &str,
) -> (u16, Value) {
    call(
        router,
        "DescribeAcmeAccount",
        &json!({ "AcmeEndpointArn": endpoint_arn, "AccountUrl": account_url }),
    )
    .await
}

/// The `AcmeAccount` `DescribeAcmeAccount` reports, asserted to succeed.
async fn account_detail(
    router: &MultiServiceRouter,
    case: &str,
    endpoint_arn: &str,
    account_url: &str,
) -> Value {
    let (status, response) = describe_account(router, endpoint_arn, account_url).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeAcmeAccount({account_url}) must succeed, got {status} {response}"
    );
    response["AcmeAccount"].clone()
}

/// The contacts an account reports, through the control plane.
fn reported_contacts(account: &Value) -> Vec<String> {
    account["Contacts"]
        .as_array()
        .map(|contacts| {
            contacts
                .iter()
                .map(|contact| contact.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Every account `ListAcmeAccounts` reports for `endpoint_arn`, paging with `max_results`
/// per page: the account URLs in the order they were reported.
async fn list_accounts(
    router: &MultiServiceRouter,
    case: &str,
    endpoint_arn: &str,
    max_results: Option<i64>,
) -> Vec<String> {
    let mut urls = Vec::new();
    let mut next: Option<String> = None;
    for _page in 0..64 {
        let mut body = json!({ "AcmeEndpointArn": endpoint_arn });
        if let Some(max) = max_results {
            body["MaxResults"] = json!(max);
        }
        if let Some(token) = next.as_ref() {
            body["NextToken"] = json!(token);
        }
        let (status, response) = call(router, "ListAcmeAccounts", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListAcmeAccounts({body}) must succeed, got {status} {response}"
        );
        let page = response["AcmeAccounts"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no AcmeAccounts list in {response}"))
            .clone();
        if let Some(max) = max_results {
            let served = i64::try_from(page.len()).unwrap_or(i64::MAX);
            assert!(
                served <= max,
                "case {case:?}: a page may hold at most MaxResults={max} accounts, got {served}"
            );
        }
        for summary in &page {
            urls.push(
                summary["AccountUrl"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: a summary carried no AccountUrl"))
                    .to_string(),
            );
        }
        next = response["NextToken"].as_str().map(str::to_string);
        if next.is_none() {
            return urls;
        }
    }
    panic!("case {case:?}: paging must terminate");
}

/// The problem type a refusal on the ACME wire carries.
fn problem_type(document: &Value) -> String {
    document["type"].as_str().unwrap_or_default().to_string()
}

/// The account keys the registration table spans: every JWK family and signature
/// algorithm an ACME client may present
/// (`::service_common::jwt::VERIFIABLE_SIGNATURE_ALGORITHMS`), each generated fresh.
fn account_keys() -> Vec<(&'static str, AccountKey)> {
    vec![
        ("an ECDSA P-256 account key (ES256)", AccountKey::ecdsa("ES256")),
        ("an ECDSA P-384 account key (ES384)", AccountKey::ecdsa("ES384")),
        ("an Ed25519 account key (EdDSA)", AccountKey::ed25519()),
        ("an RSA-2048 account key (RS256)", AccountKey::rsa()),
    ]
}

/// The contact lists the registration table spans, used in turn by the keys above: none
/// at all, one mailbox, and several in several scripts. An account IS its key, so each row
/// of the table registers with a key of its own.
const CONTACT_ROWS: &[&[&str]] = &[
    &[],
    &["mailto:ops@example.com"],
    &[
        "mailto:security@example.com",
        "mailto:\u{904b}\u{7528}@example.jp",
        "mailto:admin@example.org",
    ],
];

/// RULE 1: a correctly signed, correctly bound registration is accepted and the account it
/// mints is reported by the control plane — for every account key family, each with a
/// contact list of its own; the same key registering again is the SAME account.
#[tokio::test]
async fn new_account_registers_an_account_the_control_plane_reports() {
    let (router, engine) = router_with_engine();
    // `Contact=NOT_REQUIRED`, so the contact-less row is a registration this endpoint
    // admits; the `REQUIRED` half of that rule is asserted below.
    let mut endpoint = endpoint_body();
    endpoint["Contact"] = json!("NOT_REQUIRED");
    let enrollment = enrol(&router, &engine, "registration", &endpoint).await;

    let mut registered: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut thumbprints = BTreeSet::new();
    for (row, (case, key)) in account_keys().into_iter().enumerate() {
        let contacts = CONTACT_ROWS[row % CONTACT_ROWS.len()];
        let case = format!("{case}, {} contacts", contacts.len());
        let (status, account_url, document) =
            register(&router, &case, &enrollment, &key, contacts).await;
        assert_eq!(
            status, 201,
            "case {case:?}: a key no account holds must be REGISTERED, got {status} {document}"
        );
        assert_eq!(
            document["status"].as_str(),
            Some("valid"),
            "case {case:?}: a fresh registration is valid on the ACME wire, got {document}"
        );

        // The account URL is one this endpoint serves, under the same base as its
        // directory, and carries nothing the client sent.
        let accounts_base = enrollment.new_account().replace("new-account", "acct/");
        assert!(
            account_url.starts_with(&accounts_base),
            "case {case:?}: expected an account URL under {accounts_base}, got {account_url:?}"
        );
        for sent in contacts {
            assert!(
                !account_url.contains(sent),
                "case {case:?}: a server-assigned URL must carry nothing the client sent, got {account_url:?}"
            );
        }

        // Read back through the control plane: a different operation, reporting the
        // record the data plane wrote.
        let account = account_detail(&router, &case, &enrollment.endpoint_arn, &account_url).await;
        assert_eq!(
            account["Status"].as_str(),
            Some(state::ACME_ACCOUNT_STATUS_VALID),
            "case {case:?}: expected a VALID account, got {account}"
        );
        assert_eq!(
            account["AccountUrl"].as_str(),
            Some(account_url.as_str()),
            "case {case:?}: the account must report the URL the registration was answered with, got {account}"
        );
        assert_eq!(
            account["AcmeExternalAccountBindingArn"].as_str(),
            Some(enrollment.binding_arn.as_str()),
            "case {case:?}: the account must record the binding it registered against, got {account}"
        );
        assert_eq!(
            reported_contacts(&account),
            contacts
                .iter()
                .map(|contact| (*contact).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: expected the registered contacts, got {account}"
        );
        let thumbprint = account["PublicKeyThumbprint"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            thumbprint,
            ::service_common::jwt::jwk_thumbprint(&key.jwk()).unwrap_or_default(),
            "case {case:?}: the thumbprint must be the one of the key that registered, got {account}"
        );
        assert!(
            thumbprints.insert(thumbprint),
            "case {case:?}: every account key must get its own identity"
        );

        // The SAME key again: one account, answered 200 with the same URL — an account is
        // its key pair, so the second registration registers nothing.
        let (again_status, again_url, again_document) =
            register(&router, &case, &enrollment, &key, contacts).await;
        assert_eq!(
            (again_status, again_url.as_str()),
            (200, account_url.as_str()),
            "case {case:?}: a key that already holds an account must be answered 200 with that \
             account's URL, got {again_status} {again_document}"
        );
        registered.insert(
            account_url,
            contacts
                .iter()
                .map(|contact| (*contact).to_string())
                .collect(),
        );
    }

    // Every account the data plane registered is listed, once, and paging cuts the same
    // set at every page size — including the issue's own MaxResults=1 walk.
    let expected: Vec<String> = registered.keys().cloned().collect();
    for max_results in [Some(1), Some(2), Some(100), None] {
        let case = format!("MaxResults={max_results:?}");
        let listed = list_accounts(&router, &case, &enrollment.endpoint_arn, max_results).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: paging must report every registered account exactly once in \
             account-URL order, expected {expected:?}, got {listed:?}"
        );
    }
}

/// RULE 2: a nonce is accepted once. The rows differ along how a presented nonce fails to
/// be live, and the `badNonce` refusal's own `Replay-Nonce` is then accepted.
#[tokio::test]
async fn a_nonce_is_accepted_once_and_a_bad_nonce_carries_a_usable_one() {
    let (router, engine) = router_with_engine();
    let enrollment = enrol(&router, &engine, "the nonce endpoint", &endpoint_body()).await;
    let other = enrol(&router, &engine, "another endpoint", &endpoint_body()).await;
    let key = AccountKey::ecdsa("ES256");
    let url = enrollment.new_account();
    let contacts = ["mailto:nonce@example.com"];

    // One registration, which SPENDS the nonce it carried.
    let spent = fresh_nonce(&router, "nonce", &enrollment).await;
    let body = jws(
        &key,
        &Identification::Jwk,
        &spent,
        &url,
        Some(&registration(&enrollment, &key, &contacts, None)),
    );
    let response = post(&router, "the first registration", &url, &body).await;
    assert!(
        matches!(response.status().as_u16(), 200 | 201),
        "case \"the first registration\": must be accepted, got {} {}",
        response.status(),
        body_json("the first registration", &response)
    );

    // The table: every way a nonce a client presents is not one this endpoint is holding.
    let never_issued = base64url(&[7_u8; 16]);
    let elsewhere = fresh_nonce(&router, "nonce", &other).await;
    let rows: Vec<(&str, String)> = vec![
        ("the nonce the first registration already spent", spent),
        ("a nonce this server never issued", never_issued),
        ("a nonce another endpoint issued", elsewhere),
        ("the empty-looking nonce of a client that invented one", "0".repeat(32)),
    ];
    for (case, nonce) in rows {
        let body = jws(
            &key,
            &Identification::Jwk,
            &nonce,
            &url,
            Some(&registration(&enrollment, &key, &contacts, None)),
        );
        let response = post(&router, case, &url, &body).await;
        let status = response.status().as_u16();
        let document = body_json(case, &response);
        assert_eq!(
            status, 400,
            "case {case:?}: a nonce that is not live must be refused, got {status} {document}"
        );
        assert_eq!(
            problem_type(&document),
            "urn:ietf:params:acme:error:badNonce",
            "case {case:?}: expected the badNonce problem type, got {document}"
        );
        // RFC 8555 §6.5: that refusal MUST carry a fresh nonce, and the server MUST then
        // accept it — which is what lets a client retry without another round trip.
        let handed_back = header(&response, "replay-nonce");
        assert!(
            !handed_back.is_empty(),
            "case {case:?}: a badNonce refusal must carry a Replay-Nonce, got {document}"
        );
        let retry = jws(
            &key,
            &Identification::Jwk,
            &handed_back,
            &url,
            Some(&registration(&enrollment, &key, &contacts, None)),
        );
        let response = post(&router, case, &url, &retry).await;
        let status = response.status().as_u16();
        let retried = body_json(case, &response);
        assert!(
            matches!(status, 200 | 201),
            "case {case:?}: the nonce the refusal handed back must be accepted, got {status} {retried}"
        );
    }
}

/// RULE 3: an account's authority is its key and ends when the account leaves `VALID` —
/// for a revocation through the control plane and for the client's own deactivation, with
/// a still-valid sibling account as the control.
#[tokio::test]
async fn an_account_that_is_not_valid_can_no_longer_write() {
    let (router, engine) = router_with_engine();
    let enrollment = enrol(&router, &engine, "the revoked-account endpoint", &endpoint_body())
        .await;

    // The table: both ways an account leaves `VALID`, and the status each leaves behind.
    let rows: &[(&str, bool, &str)] = &[
        (
            "an account RevokeAcmeAccount revoked",
            true,
            state::ACME_ACCOUNT_STATUS_REVOKED,
        ),
        (
            "an account its own client deactivated",
            false,
            state::ACME_ACCOUNT_STATUS_DEACTIVATED,
        ),
    ];
    for (case, revoke_through_control_plane, expected_status) in rows {
        let key = AccountKey::ecdsa("ES256");
        let sibling = AccountKey::ed25519();
        let (_, account_url, _) = register(
            &router,
            case,
            &enrollment,
            &key,
            &["mailto:first@example.com"],
        )
        .await;
        let (_, sibling_url, _) = register(
            &router,
            case,
            &enrollment,
            &sibling,
            &["mailto:sibling@example.com"],
        )
        .await;

        // The update RFC 8555 §7.3.2 defines, accepted while the account is valid and
        // visible through the control plane.
        let updated = ["mailto:updated@example.com", "mailto:second@example.com"];
        let update = |nonce: &str, account_url: &str, key: &AccountKey| {
            jws(
                key,
                &Identification::KeyId(account_url),
                nonce,
                account_url,
                Some(&json!({ "contact": updated })),
            )
        };
        let nonce = fresh_nonce(&router, case, &enrollment).await;
        let response = post(
            &router,
            case,
            &account_url,
            &update(&nonce, &account_url, &key),
        )
        .await;
        let status = response.status().as_u16();
        let document = body_json(case, &response);
        assert_eq!(
            status, 200,
            "case {case:?}: a valid account must be able to update its contacts, got {status} {document}"
        );
        let account = account_detail(&router, case, &enrollment.endpoint_arn, &account_url).await;
        assert_eq!(
            reported_contacts(&account),
            updated.iter().map(|c| (*c).to_string()).collect::<Vec<_>>(),
            "case {case:?}: the update must be visible through DescribeAcmeAccount, got {account}"
        );

        // Out of `VALID`: through the control plane, or through the account's own
        // deactivation request.
        if *revoke_through_control_plane {
            let (status, response) = call(
                &router,
                "RevokeAcmeAccount",
                &json!({ "AcmeEndpointArn": enrollment.endpoint_arn, "AccountUrl": account_url }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: RevokeAcmeAccount must succeed, got {status} {response}"
            );
            // `@idempotent`: a second revocation is the state the first one reached.
            let (again, response) = call(
                &router,
                "RevokeAcmeAccount",
                &json!({ "AcmeEndpointArn": enrollment.endpoint_arn, "AccountUrl": account_url }),
            )
            .await;
            assert_eq!(
                again, 200,
                "case {case:?}: revoking an already revoked account must be idempotent, got {again} {response}"
            );
        } else {
            let nonce = fresh_nonce(&router, case, &enrollment).await;
            let body = jws(
                &key,
                &Identification::KeyId(&account_url),
                &nonce,
                &account_url,
                Some(&json!({ "status": "deactivated" })),
            );
            let response = post(&router, case, &account_url, &body).await;
            assert_eq!(
                response.status().as_u16(),
                200,
                "case {case:?}: an account may deactivate itself, got {}",
                body_json(case, &response)
            );
        }

        // A REVOKED or DEACTIVATED account is still an account: it is REPORTED with that
        // status rather than reported missing.
        let account = account_detail(&router, case, &enrollment.endpoint_arn, &account_url).await;
        assert_eq!(
            account["Status"].as_str(),
            Some(*expected_status),
            "case {case:?}: expected the terminal status, got {account}"
        );

        // The identical update, signed with the same key: refused, and the contacts stand
        // exactly as the last accepted update left them.
        let nonce = fresh_nonce(&router, case, &enrollment).await;
        let response = post(
            &router,
            case,
            &account_url,
            &update(&nonce, &account_url, &key),
        )
        .await;
        let status = response.status().as_u16();
        let document = body_json(case, &response);
        assert_eq!(
            status, 400,
            "case {case:?}: an account that is no longer valid may not write, got {status} {document}"
        );
        assert_eq!(
            problem_type(&document),
            "urn:ietf:params:acme:error:unauthorized",
            "case {case:?}: expected the unauthorized problem type, got {document}"
        );
        let after = account_detail(&router, case, &enrollment.endpoint_arn, &account_url).await;
        assert_eq!(
            reported_contacts(&after),
            updated.iter().map(|c| (*c).to_string()).collect::<Vec<_>>(),
            "case {case:?}: a refused update must leave the contacts as they were, got {after}"
        );

        // Nor can the same key register its way back in: an account that has left VALID
        // is not handed back to its key by a fresh registration either.
        let nonce = fresh_nonce(&router, case, &enrollment).await;
        let new_account_url = enrollment.new_account();
        let body = jws(
            &key,
            &Identification::Jwk,
            &nonce,
            &new_account_url,
            Some(&registration(
                &enrollment,
                &key,
                &["mailto:first@example.com"],
                None,
            )),
        );
        let response = post(&router, case, &new_account_url, &body).await;
        let status = response.status().as_u16();
        let document = body_json(case, &response);
        assert_eq!(
            status, 400,
            "case {case:?}: a key whose account is no longer valid must not re-register, \
             got {status} {document}"
        );
        assert_eq!(
            problem_type(&document),
            "urn:ietf:params:acme:error:unauthorized",
            "case {case:?}: expected the unauthorized problem type, got {document}"
        );
        assert_eq!(
            account_detail(&router, case, &enrollment.endpoint_arn, &account_url).await,
            after,
            "case {case:?}: the refused registration must leave the account as it stood"
        );

        // The sibling account on the same endpoint is untouched and still updates.
        let nonce = fresh_nonce(&router, case, &enrollment).await;
        let response = post(
            &router,
            case,
            &sibling_url,
            &update(&nonce, &sibling_url, &sibling),
        )
        .await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case:?}: a still-valid account must go on updating, got {}",
            body_json(case, &response)
        );
        let sibling_account =
            account_detail(&router, case, &enrollment.endpoint_arn, &sibling_url).await;
        assert_eq!(
            reported_contacts(&sibling_account),
            updated.iter().map(|c| (*c).to_string()).collect::<Vec<_>>(),
            "case {case:?}: the sibling's own update must be visible, got {sibling_account}"
        );
    }
}

/// RULE 3, read half: a POST-as-GET to an account's own URL reports the account and
/// changes nothing, and a request signed under another account's key is refused.
#[tokio::test]
async fn an_account_resource_is_served_only_to_the_key_that_holds_it() {
    let (router, engine) = router_with_engine();
    let enrollment =
        enrol(&router, &engine, "the account-resource endpoint", &endpoint_body()).await;
    let key = AccountKey::ecdsa("ES256");
    let other = AccountKey::ecdsa("ES384");
    let (_, account_url, _) =
        register(&router, "holder", &enrollment, &key, &["mailto:holder@example.com"]).await;
    let (_, other_url, _) =
        register(&router, "other", &enrollment, &other, &["mailto:other@example.com"]).await;

    // POST-as-GET: the empty payload of RFC 8555 §6.3 reads the account.
    let before = account_detail(&router, "post-as-get", &enrollment.endpoint_arn, &account_url)
        .await;
    let nonce = fresh_nonce(&router, "post-as-get", &enrollment).await;
    let body = jws(
        &key,
        &Identification::KeyId(&account_url),
        &nonce,
        &account_url,
        None,
    );
    let response = post(&router, "post-as-get", &account_url, &body).await;
    let status = response.status().as_u16();
    let document = body_json("post-as-get", &response);
    assert_eq!(
        status, 200,
        "case \"post-as-get\": a POST-as-GET must report the account, got {status} {document}"
    );
    assert_eq!(
        document["contact"],
        json!(["mailto:holder@example.com"]),
        "case \"post-as-get\": the account resource must report the stored contacts, got {document}"
    );
    assert_eq!(
        account_detail(&router, "post-as-get", &enrollment.endpoint_arn, &account_url).await,
        before,
        "case \"post-as-get\": reading an account must change nothing a later caller sees"
    );

    // The table: every way a request to this account's URL is not this account speaking.
    let nonce = fresh_nonce(&router, "another account's key", &enrollment).await;
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "another account's key id",
            jws(
                &other,
                &Identification::KeyId(&other_url),
                &nonce,
                &account_url,
                Some(&json!({ "contact": ["mailto:hijack@example.com"] })),
            ),
            "urn:ietf:params:acme:error:unauthorized",
        ),
        (
            "this account's key id, signed by another key",
            jws(
                &other,
                &Identification::KeyId(&account_url),
                &nonce,
                &account_url,
                Some(&json!({ "contact": ["mailto:hijack@example.com"] })),
            ),
            "urn:ietf:params:acme:error:malformed",
        ),
        (
            "the key embedded instead of named",
            jws(
                &key,
                &Identification::Jwk,
                &nonce,
                &account_url,
                Some(&json!({ "contact": ["mailto:hijack@example.com"] })),
            ),
            "urn:ietf:params:acme:error:malformed",
        ),
        (
            "a status no client may set",
            jws(
                &key,
                &Identification::KeyId(&account_url),
                &nonce,
                &account_url,
                Some(&json!({ "status": "valid" })),
            ),
            "urn:ietf:params:acme:error:malformed",
        ),
    ];
    for (case, body, expected) in rows {
        let response = post(&router, case, &account_url, &body).await;
        let status = response.status().as_u16();
        let document = body_json(case, &response);
        assert_eq!(
            status, 400,
            "case {case:?}: the request must be refused, got {status} {document}"
        );
        assert_eq!(
            problem_type(&document),
            expected,
            "case {case:?}: expected {expected}, got {document}"
        );
        let after = account_detail(&router, case, &enrollment.endpoint_arn, &account_url).await;
        assert_eq!(
            reported_contacts(&after),
            vec!["mailto:holder@example.com".to_string()],
            "case {case:?}: a refused request must change nothing, got {after}"
        );
    }
}

/// RULE 4: the external account binding is what authorizes a registration — the rows
/// differ along how a presented binding fails to be one these credentials account for.
#[tokio::test]
async fn a_registration_is_authorized_by_the_endpoint_external_account_binding() {
    let (router, engine) = router_with_engine();
    let enrollment = enrol(&router, &engine, "the binding endpoint", &endpoint_body()).await;
    let url = enrollment.new_account();

    // A second binding on the same endpoint, whose credentials are NOT the ones a
    // registration may sign with for the first one's key id.
    let second = create_binding(
        &router,
        "second binding",
        &json!({
            "AcmeEndpointArn": enrollment.endpoint_arn,
            "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/OtherAcmeIssuer"),
        }),
    )
    .await;
    let second_arn = second["AcmeExternalAccountBindingArn"]
        .as_str()
        .expect("the second binding carried no ARN")
        .to_string();
    let (status, second_credentials) = call(
        &router,
        "GetAcmeExternalAccountBindingCredentials",
        &json!({ "AcmeExternalAccountBindingArn": second_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"second binding\": its credentials must be reported, got {second_credentials}"
    );
    let second_mac = second_credentials["MacKey"]
        .as_str()
        .expect("no MacKey")
        .to_string();

    let key = AccountKey::ecdsa("ES256");
    let forged = external_account_binding(
        &enrollment.key_id,
        // A MAC key of the same form that is not this binding's: a forged MAC.
        &base64url(&[9_u8; 33]),
        &url,
        &key.jwk(),
    );
    let wrong_binding = external_account_binding(&enrollment.key_id, &second_mac, &url, &key.jwk());
    let unknown_key_id = external_account_binding(
        "00000000-0000-4000-8000-000000000000",
        &enrollment.mac_key,
        &url,
        &key.jwk(),
    );
    let other_key = AccountKey::ed25519();
    let bound_to_another_key = external_account_binding(
        &enrollment.key_id,
        &enrollment.mac_key,
        &url,
        &other_key.jwk(),
    );
    let wrong_url = external_account_binding(
        &enrollment.key_id,
        &enrollment.mac_key,
        &enrollment.new_nonce(),
        &key.jwk(),
    );

    // The table: every way a presented binding fails to be one these credentials account
    // for, with the status each class of failure is refused at — a credential the server
    // rejected is `403`, a request whose binding is malformed is `400`. `None` is the
    // registration that presents none at all.
    let rows: Vec<(&str, Option<Value>, u16, &str)> = vec![
        (
            "no externalAccountBinding at all",
            None,
            400,
            "urn:ietf:params:acme:error:externalAccountRequired",
        ),
        (
            "a forged MAC under a key id this endpoint did issue",
            Some(forged),
            403,
            "urn:ietf:params:acme:error:unauthorized",
        ),
        (
            "another binding's MAC key under this binding's key id",
            Some(wrong_binding),
            403,
            "urn:ietf:params:acme:error:unauthorized",
        ),
        (
            "a key id this endpoint never issued",
            Some(unknown_key_id),
            403,
            "urn:ietf:params:acme:error:unauthorized",
        ),
        (
            "a binding bound to another account key",
            Some(bound_to_another_key),
            400,
            "urn:ietf:params:acme:error:malformed",
        ),
        (
            "a binding signed for another url",
            Some(wrong_url),
            400,
            "urn:ietf:params:acme:error:malformed",
        ),
    ];
    for (case, binding, expected_status, expected) in rows {
        let nonce = fresh_nonce(&router, case, &enrollment).await;
        let mut payload = registration(&enrollment, &key, &["mailto:bound@example.com"], None);
        match binding {
            Some(binding) => payload["externalAccountBinding"] = binding,
            None => {
                payload
                    .as_object_mut()
                    .expect("a registration payload is an object")
                    .remove("externalAccountBinding");
            }
        }
        let body = jws(&key, &Identification::Jwk, &nonce, &url, Some(&payload));
        let response = post(&router, case, &url, &body).await;
        let status = response.status().as_u16();
        let document = body_json(case, &response);
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {document}"
        );
        assert_eq!(
            problem_type(&document),
            expected,
            "case {case:?}: expected {expected}, got {document}"
        );
        assert!(
            header(&response, "location").is_empty(),
            "case {case:?}: a refused registration may mint no account URL"
        );
        assert!(
            list_accounts(&router, case, &enrollment.endpoint_arn, None)
                .await
                .is_empty(),
            "case {case:?}: a refused registration must register nothing"
        );
    }

    // The genuine credentials ARE accepted, and using them records the use the control
    // plane reports.
    let (status, account_url, document) = register(
        &router,
        "the binding's own credentials",
        &enrollment,
        &key,
        &["mailto:bound@example.com"],
    )
    .await;
    assert_eq!(
        status, 201,
        "case \"the binding's own credentials\": must register, got {status} {document}"
    );
    let (status, described) = call(
        &router,
        "DescribeAcmeExternalAccountBinding",
        &json!({ "AcmeExternalAccountBindingArn": enrollment.binding_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the binding's own credentials\": the binding must be described, got {described}"
    );
    assert!(
        described["ExternalAccountBinding"]["LastUsedAt"].is_number(),
        "case \"the binding's own credentials\": registering with a binding must record the use, got {described}"
    );
    assert_eq!(
        list_accounts(&router, "the binding's own credentials", &enrollment.endpoint_arn, None)
            .await,
        vec![account_url],
        "case \"the binding's own credentials\": the registered account must be the one listed"
    );

    // And a binding whose authority has been withdrawn authorizes nothing.
    let (status, revoked) = call(
        &router,
        "RevokeAcmeExternalAccountBinding",
        &json!({ "AcmeExternalAccountBindingArn": second_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a revoked binding\": the revocation must succeed, got {revoked}"
    );
    let fresh = AccountKey::ecdsa("ES256");
    let nonce = fresh_nonce(&router, "a revoked binding", &enrollment).await;
    let mut payload = registration(&enrollment, &fresh, &["mailto:late@example.com"], None);
    payload["externalAccountBinding"] = external_account_binding(
        second_credentials["KeyId"].as_str().expect("no KeyId"),
        &second_mac,
        &url,
        &fresh.jwk(),
    );
    let body = jws(&fresh, &Identification::Jwk, &nonce, &url, Some(&payload));
    let response = post(&router, "a revoked binding", &url, &body).await;
    let status = response.status().as_u16();
    let document = body_json("a revoked binding", &response);
    assert_eq!(
        status, 403,
        "case \"a revoked binding\": a withdrawn binding must authorize nothing, got {status} {document}"
    );
    assert_eq!(
        problem_type(&document),
        "urn:ietf:params:acme:error:unauthorized",
        "case \"a revoked binding\": expected the unauthorized problem type, got {document}"
    );
}

/// RULE 1, the contact policy half: an endpoint whose `Contact` is `REQUIRED` refuses a
/// registration that carries none, and a contact URI whose scheme an ACME server does not
/// support is refused whatever the endpoint's policy is.
#[tokio::test]
async fn the_endpoint_contact_policy_decides_a_contact_less_registration() {
    let (router, engine) = router_with_engine();
    let mut not_required = endpoint_body();
    not_required["Contact"] = json!("NOT_REQUIRED");
    let required = enrol(&router, &engine, "a Contact=REQUIRED endpoint", &endpoint_body()).await;
    let optional = enrol(&router, &engine, "a Contact=NOT_REQUIRED endpoint", &not_required).await;

    // The same contact-less registration, against both policies: the endpoint's own
    // configuration is what decides it.
    for (case, enrollment, accepted) in [
        ("Contact=REQUIRED", &required, false),
        ("Contact=NOT_REQUIRED", &optional, true),
    ] {
        let key = AccountKey::ecdsa("ES256");
        let url = enrollment.new_account();
        let nonce = fresh_nonce(&router, case, enrollment).await;
        let body = jws(
            &key,
            &Identification::Jwk,
            &nonce,
            &url,
            Some(&registration(enrollment, &key, &[], None)),
        );
        let response = post(&router, case, &url, &body).await;
        let status = response.status().as_u16();
        let document = body_json(case, &response);
        if accepted {
            assert_eq!(
                status, 201,
                "case {case:?}: a contact-less registration must be accepted, got {status} {document}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a contact-less registration must be refused, got {status} {document}"
            );
            assert_eq!(
                problem_type(&document),
                "urn:ietf:params:acme:error:invalidContact",
                "case {case:?}: expected the invalidContact problem type, got {document}"
            );
        }
    }

    // The scheme rule, which holds on either endpoint.
    let key = AccountKey::ecdsa("ES256");
    let url = optional.new_account();
    for (case, contacts, expected) in [
        (
            "a tel: contact URI",
            vec!["tel:+15555550123"],
            "urn:ietf:params:acme:error:unsupportedContact",
        ),
        (
            "a contact URI with no scheme at all",
            vec!["ops@example.com"],
            "urn:ietf:params:acme:error:unsupportedContact",
        ),
    ] {
        let nonce = fresh_nonce(&router, case, &optional).await;
        let body = jws(
            &key,
            &Identification::Jwk,
            &nonce,
            &url,
            Some(&registration(&optional, &key, &contacts, None)),
        );
        let response = post(&router, case, &url, &body).await;
        let status = response.status().as_u16();
        let document = body_json(case, &response);
        assert_eq!(
            status, 400,
            "case {case:?}: a contact URI this server cannot use must be refused, got {status} {document}"
        );
        assert_eq!(
            problem_type(&document),
            expected,
            "case {case:?}: expected {expected}, got {document}"
        );
    }
}

/// RULE 1, the envelope half: a body that is not a JWS this server can read is refused
/// with an ACME problem document, and nothing is registered — the rows differ along which
/// part of RFC 8555 §6.2's envelope is wrong.
#[tokio::test]
async fn a_body_that_is_not_an_acme_jws_registers_nothing() {
    let (router, engine) = router_with_engine();
    let enrollment = enrol(&router, &engine, "the envelope endpoint", &endpoint_body()).await;
    let url = enrollment.new_account();
    let key = AccountKey::ecdsa("ES256");
    let nonce = fresh_nonce(&router, "envelope", &enrollment).await;
    let payload = registration(&enrollment, &key, &["mailto:envelope@example.com"], None);

    // A correctly built JWS, with one part of it rewritten per row.
    let signed = jws(&key, &Identification::Jwk, &nonce, &url, Some(&payload));
    let rewritten = |mutate: &dyn Fn(&mut Value)| {
        let mut body = signed.clone();
        mutate(&mut body);
        body
    };
    let rows: Vec<(&str, Value, &str)> = vec![
        ("an empty JSON object", json!({}), "urn:ietf:params:acme:error:malformed"),
        (
            "a JSON body that is not a JWS",
            json!({ "termsOfServiceAgreed": true }),
            "urn:ietf:params:acme:error:malformed",
        ),
        (
            "a protected header that is not base64url",
            rewritten(&|body| body["protected"] = json!("@@@@")),
            "urn:ietf:params:acme:error:malformed",
        ),
        (
            "a payload rewritten after signing",
            rewritten(&|body| {
                body["payload"] = json!(base64url(
                    json!({ "termsOfServiceAgreed": true }).to_string().as_bytes()
                ));
            }),
            "urn:ietf:params:acme:error:malformed",
        ),
        (
            "a signature that is not the key's",
            rewritten(&|body| body["signature"] = json!(base64url(&[0_u8; 64]))),
            "urn:ietf:params:acme:error:malformed",
        ),
        (
            "an alg this server checks no signature under",
            jws_with_algorithm(&key, &nonce, &url, &payload, "HS256"),
            "urn:ietf:params:acme:error:badSignatureAlgorithm",
        ),
    ];
    for (case, body, expected) in rows {
        let response = post(&router, case, &url, &body).await;
        let status = response.status().as_u16();
        let document = body_json(case, &response);
        assert_eq!(
            status, 400,
            "case {case:?}: the body must be refused, got {status} {document}"
        );
        assert_eq!(
            problem_type(&document),
            expected,
            "case {case:?}: expected {expected}, got {document}"
        );
        assert!(
            list_accounts(&router, case, &enrollment.endpoint_arn, None)
                .await
                .is_empty(),
            "case {case:?}: a refused body must register nothing"
        );
    }

    // A `badSignatureAlgorithm` document names the algorithms this server does check,
    // which is what lets a client pick one (RFC 8555 §6.2).
    let response = post(
        &router,
        "the algorithms a refusal names",
        &url,
        &jws_with_algorithm(&key, &nonce, &url, &payload, "HS256"),
    )
    .await;
    let document = body_json("the algorithms a refusal names", &response);
    let named: Vec<String> = document["algorithms"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|algorithm| algorithm.as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        named,
        ::service_common::jwt::VERIFIABLE_SIGNATURE_ALGORITHMS
            .iter()
            .map(|algorithm| (*algorithm).to_string())
            .collect::<Vec<_>>(),
        "case \"the algorithms a refusal names\": expected the algorithms this server checks, got {document}"
    );

    // And a URL no account can be registered at: the resources this emulator performs no
    // interaction for stay their own gap rather than becoming a verdict on the JWS.
    for resource in ["new-order", "revoke-cert", "key-change"] {
        let case = format!("POST {resource}");
        let target = enrollment.directory[match resource {
            "new-order" => "newOrder",
            "revoke-cert" => "revokeCert",
            _ => "keyChange",
        }]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the directory names no {resource}"))
        .to_string();
        let response = post(&router, &case, &target, &signed).await;
        let status = response.status().as_u16();
        let document = body_json(&case, &response);
        assert_eq!(
            status, 501,
            "case {case:?}: an interaction nothing performs must not succeed, got {status} {document}"
        );
    }
}

/// The same JWS, with `alg` rewritten to `algorithm` — a header this server checks no
/// signature under.
fn jws_with_algorithm(
    key: &AccountKey,
    nonce: &str,
    url: &str,
    payload: &Value,
    algorithm: &str,
) -> Value {
    let protected = base64url(
        json!({ "alg": algorithm, "nonce": nonce, "url": url, "jwk": key.jwk() })
            .to_string()
            .as_bytes(),
    );
    let payload = base64url(payload.to_string().as_bytes());
    let signing_input = format!("{protected}.{payload}");
    json!({
        "protected": protected,
        "payload": payload,
        "signature": base64url(&key.sign(&signing_input)),
    })
}

/// RULE 1, the control-plane half: the account operations address an account by
/// (`AcmeEndpointArn`, `AccountUrl`), and every way that pair can name nothing is a
/// missing resource rather than an answer.
#[tokio::test]
async fn the_account_operations_report_only_accounts_the_pair_names() {
    let (router, engine) = router_with_engine();
    let enrollment = enrol(&router, &engine, "the lookup endpoint", &endpoint_body()).await;
    let elsewhere = enrol(&router, &engine, "another endpoint", &endpoint_body()).await;
    let key = AccountKey::ecdsa("ES256");
    let (_, account_url, _) =
        register(&router, "lookup", &enrollment, &key, &["mailto:lookup@example.com"]).await;

    let unknown_endpoint =
        state::acme_endpoint_arn("us-west-2", "11111111-2222-4333-8444-555555555555");
    let rows: Vec<(&str, String, String, &str, String)> = vec![
        (
            "an account URL this endpoint holds no account under",
            enrollment.endpoint_arn.clone(),
            format!("{account_url}-not-an-account"),
            "ResourceNotFoundException",
            state::acme_account_not_found_message(&format!("{account_url}-not-an-account")),
        ),
        (
            "an account of another endpoint",
            elsewhere.endpoint_arn.clone(),
            account_url.clone(),
            "ResourceNotFoundException",
            state::acme_account_not_found_message(&account_url),
        ),
        (
            "an endpoint no record answers to",
            unknown_endpoint.clone(),
            account_url.clone(),
            "ResourceNotFoundException",
            state::acme_endpoint_not_found_message(&unknown_endpoint),
        ),
    ];
    for (case, endpoint_arn, account_url, expected_code, expected_message) in rows {
        for operation in ["DescribeAcmeAccount", "RevokeAcmeAccount"] {
            let case = format!("{operation}: {case}");
            let (status, response) = call(
                &router,
                operation,
                &json!({ "AcmeEndpointArn": endpoint_arn, "AccountUrl": account_url }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: the pair names nothing, so it must be refused, got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                expected_code,
                "case {case:?}: expected {expected_code}, got {response}"
            );
            assert_eq!(
                error_message(&response),
                expected_message,
                "case {case:?}: expected this service's not-found sentence, got {response}"
            );
        }
    }

    // An ARN naming another account is a verdict on the REQUEST, before any lookup.
    let cross_account = state::acme_endpoint_arn("us-west-2", "66666666-7777-4888-8999-aaaaaaaaaaaa")
        .replace(ACCOUNT, "999999999999");
    for operation in ["DescribeAcmeAccount", "RevokeAcmeAccount"] {
        let case = format!("{operation}: another account's endpoint ARN");
        let (status, response) = call(
            &router,
            operation,
            &json!({ "AcmeEndpointArn": cross_account, "AccountUrl": account_url }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a cross-account ARN must be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
    }
}
