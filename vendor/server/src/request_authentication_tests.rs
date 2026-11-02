// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
use super::*;
use std::time::{Duration, SystemTime};

const KEY: &str = "EMULATORAAAAAAAAAAAA";
const SECRET: &str = "emulator-test-secret";

fn emulator() -> SigV4Authenticator {
    SigV4Authenticator {
        access_key_id: KEY.into(),
        secret_access_key: SECRET.into(),
    }
}

fn store() -> CredentialStore {
    CredentialStore::default()
}

/// How a test client signs: the SDK signer, as an SDK would.
#[derive(Clone, Copy)]
struct Client {
    service: &'static str,
    region: &'static str,
    hashed_payload: bool,
    key: &'static str,
    secret: &'static str,
    presigned_for: Option<Duration>,
    signed_at: SystemTime,
}

const SQS: Client = Client {
    service: "sqs",
    region: "us-west-2",
    hashed_payload: false,
    key: KEY,
    secret: SECRET,
    presigned_for: None,
    signed_at: SystemTime::UNIX_EPOCH,
};

/// The SDKs sign IoT Secure Tunneling with its modeled `aws.auth#sigv4` name as written.
const IOT_SECURE_TUNNELING: Client = Client {
    service: "IoTSecuredTunneling",
    ..SQS
};

fn request(method: &str, uri: &str, body: &[u8]) -> http::Request<Vec<u8>> {
    let builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "127.0.0.1:8443");
    builder
        .header("content-type", "application/x-amz-json-1.0")
        .body(body.to_vec())
        .unwrap()
}

fn signed(client: &Client, mut request: http::Request<Vec<u8>>) -> (http::request::Parts, Vec<u8>) {
    let credentials =
        aws_credential_types::Credentials::new(client.key, client.secret, None, None, "test");
    let identity = credentials.into();
    let mut settings = SigningSettings::default();
    if signs_like_s3(client.service) {
        settings.percent_encoding_mode = PercentEncodingMode::Single;
        settings.uri_path_normalization_mode = UriPathNormalizationMode::Disabled;
    }
    if let Some(expires) = client.presigned_for {
        settings.signature_location = SignatureLocation::QueryParams;
        settings.expires_in = Some(expires);
    }
    let unset = client.signed_at == SystemTime::UNIX_EPOCH;
    let at = if unset {
        SystemTime::now()
    } else {
        client.signed_at
    };
    let s3_header_signed = client.presigned_for.is_none() && signs_like_s3(client.service);
    if s3_header_signed {
        settings.payload_checksum_kind = aws_sigv4::http_request::PayloadChecksumKind::XAmzSha256;
    }
    let body = match (client.presigned_for, s3_header_signed) {
        (None, true) if !client.hashed_payload => SignableBody::UnsignedPayload,
        (Some(_), _) => SignableBody::UnsignedPayload,
        _ => SignableBody::Bytes(request.body()),
    };
    let params = aws_sigv4::sign::v4::SigningParams::builder().identity(&identity);
    let params = params.region(client.region).name(client.service);
    let params = params.time(at).settings(settings).build().unwrap().into();
    let (method, uri) = (request.method().to_string(), request.uri().to_string());
    let headers = request.headers().iter();
    let headers = headers.map(|(name, value)| (name.as_str(), value.to_str().unwrap()));
    let signable = SignableRequest::new(&method, &uri, headers, body).unwrap();
    let (instructions, _) = sign(signable, &params).unwrap().into_parts();
    instructions.apply_to_request_http1x(&mut request);
    request.into_parts()
}

fn send_message() -> http::Request<Vec<u8>> {
    request("POST", "https://127.0.0.1:8443/", br#"{"QueueUrl":"q"}"#)
}

fn in_process() -> http::Request<Vec<u8>> {
    http::Request::builder()
        .method("POST")
        .uri("https://127.0.0.1:8443/")
        .body(vec![])
        .unwrap()
}

fn get_object() -> http::Request<Vec<u8>> {
    request("GET", "https://127.0.0.1:8443/bucket/key%20one", b"")
}

fn verdict(parts: &http::request::Parts, body: &[u8], serving: &str) -> Result<(), &'static str> {
    verdict_in(&store(), parts, body, serving)
}

fn verdict_in(
    store: &CredentialStore,
    parts: &http::request::Parts,
    body: &[u8],
    serving: &str,
) -> Result<(), &'static str> {
    let verified = emulator().verify(parts, body, serving, store);
    verified.map_err(|denied| denied.code)
}

#[test]
fn created_keys_verify_until_deleted_and_the_published_key_always() {
    let store = store();
    let created = store.create(&crate::aws_credentials::AWS_ACCESS_KEY);
    let leak = |text: &str| -> &'static str { Box::leak(text.into()) };
    let secret = leak(created.secret(SECRET_ACCESS_KEY).unwrap());
    let as_created = Client { key: leak(&created.id), secret, ..SQS };
    let sent = br#"{"QueueUrl":"q"}"#.to_vec();
    let verdict_of = |client: &Client| verdict_in(&store, &signed(client, send_message()).0, &sent, "sqs");
    assert_eq!(verdict_of(&as_created), Ok(()));
    assert_eq!(verdict_of(&Client { secret: "x", ..as_created }), Err("SignatureDoesNotMatch"));
    store.delete(&created.id).unwrap();
    assert_eq!(verdict_of(&as_created), Err("AccessDenied"));
    let refused = store.delete(KEY);
    assert_eq!(refused, Err(crate::credential_store::CredentialError::NotFound));
    assert_eq!(verdict_of(&SQS), Ok(()));
}

#[test]
fn requests_are_accepted_or_refused_as_their_signature_says() {
    #[rustfmt::skip]
    let (s3, s3_presigned) = (Client { service: "s3", ..SQS }, Client { service: "s3", presigned_for: Some(Duration::from_secs(600)), ..SQS });
    let hour_ago = SystemTime::now() - Duration::from_secs(3600);
    #[rustfmt::skip]
    let expired = Client { signed_at: hour_ago, presigned_for: Some(Duration::from_secs(60)), ..s3_presigned };
    #[rustfmt::skip]
    let (wrong_secret, hashed) = (Client { secret: "x", ..SQS }, Client { hashed_payload: true, ..s3 });
    let s3_put = || request("PUT", "https://127.0.0.1:8443/bucket/key", b"object");
    let sent = br#"{"QueueUrl":"q"}"#.to_vec();
    let wrong_secret_s3 = signed(&Client { secret: "x", ..s3 }, s3_put()).0;
    let trailer_marked = with_header(
        wrong_secret_s3,
        "x-amz-content-sha256",
        "STREAMING-UNSIGNED-PAYLOAD-TRAILER",
    );
    #[rustfmt::skip]
    let cases: Vec<(&str, http::request::Parts, Vec<u8>, &str, Result<(), &str>)> = vec![
        ("header-signed sqs", signed(&SQS, send_message()).0, sent.clone(), "sqs", Ok(())),
        ("signed in another region", signed(&Client { region: "eu-west-1", ..SQS }, send_message()).0, sent.clone(), "sqs", Ok(())),
        ("presigned s3 get", signed(&s3_presigned, get_object()).0, vec![], "s3", Ok(())),
        ("s3 put with unsigned payload", signed(&s3, s3_put()).0, b"object".to_vec(), "s3", Ok(())),
        ("s3 put with hashed payload", signed(&hashed, s3_put()).0, b"object".to_vec(), "s3", Ok(())),
        ("s3 put, body swapped under its hash", signed(&hashed, s3_put()).0, b"forged".to_vec(), "s3", Err("SignatureDoesNotMatch")),
        ("in-process, no host header", signed(&SQS, in_process()).0, vec![], "sqs", Ok(())),
        ("added unsigned header", with_header(signed(&SQS, send_message()).0, "x-amz-target", "AWSLambda.CreateFunction"), sent.clone(), "sqs", Ok(())),
        ("unsigned", send_message().into_parts().0, vec![], "sqs", Err("AccessDenied")),
        ("another key", signed(&Client { key: "EMULATORB", ..SQS }, send_message()).0, vec![], "sqs", Err("AccessDenied")),
        ("signed for another service", signed(&SQS, send_message()).0, vec![], "lambda", Err("AccessDenied")),
        ("shared s3 url sent to another service", signed(&s3_presigned, get_object()).0, vec![], "lambda", Err("AccessDenied")),
        ("expired presigned url", signed(&expired, get_object()).0, vec![], "s3", Err("AccessDenied")),
        ("presigned expiry past the clock's range", with_uri(signed(&s3_presigned, get_object()).0, "Expires=600", &format!("Expires={}", u64::MAX)), vec![], "s3", Err("AccessDenied")),
        ("wrong secret", signed(&wrong_secret, send_message()).0, sent.clone(), "sqs", Err("SignatureDoesNotMatch")),
        ("wrong secret, unsigned-trailer marker (MinIO CVE-2026-41145)", trailer_marked, vec![], "s3", Err("SignatureDoesNotMatch")),
        ("tampered body", signed(&SQS, send_message()).0, br#"{"QueueUrl":"x"}"#.to_vec(), "sqs", Err("SignatureDoesNotMatch")),
        ("tampered signed header", with_header(signed(&SQS, send_message()).0, "content-type", "application/x-amz-json-1.1"), sent.clone(), "sqs", Err("SignatureDoesNotMatch")),
        ("presigned validity over seven days", with_uri(signed(&s3_presigned, get_object()).0, "Expires=600", "Expires=604801"), vec![], "s3", Err("AccessDenied")),
        ("presigned scope with four parts", with_uri(signed(&s3_presigned, get_object()).0, "%2Faws4_request", ""), vec![], "s3", Err("AccessDenied")),
        ("header-signed ten minutes ago", signed(&Client { signed_at: SystemTime::now() - Duration::from_secs(600), ..SQS }, send_message()).0, sent.clone(), "sqs", Ok(())),
        ("header-signed an hour ago, replayed", signed(&Client { signed_at: hour_ago, ..SQS }, send_message()).0, sent.clone(), "sqs", Err("RequestTimeTooSkewed")),
        ("header-signed by a clock an hour fast", signed(&Client { signed_at: SystemTime::now() + Duration::from_secs(3600), ..SQS }, send_message()).0, sent.clone(), "sqs", Err("RequestTimeTooSkewed")),
        ("presigned url retargeted", with_uri(signed(&s3_presigned, get_object()).0, "/key%20one?", "/other?"), vec![], "s3", Err("SignatureDoesNotMatch")),
        ("mixed-case signing name, served lowercased", signed(&IOT_SECURE_TUNNELING, send_message()).0, sent.clone(), "iotsecuredtunneling", Ok(())),
        ("mixed-case signing name sent to another service", signed(&IOT_SECURE_TUNNELING, send_message()).0, sent.clone(), "iot", Err("AccessDenied")),
        ("mixed-case signing name, tampered body", signed(&IOT_SECURE_TUNNELING, send_message()).0, br#"{"QueueUrl":"x"}"#.to_vec(), "iotsecuredtunneling", Err("SignatureDoesNotMatch")),
        ("ec2 call claimed by a service serving another name", signed(&Client { service: "ec2", ..SQS }, send_message()).0, sent.clone(), "rds", Err("AccessDenied")),
        ("s3 key a//b sent as a/%2Fb, signed decoded (Java v2)", with_uri(signed(&s3, s3_get("a//b")).0, "/a//b", "/a/%2Fb"), vec![], "s3", Ok(())),
        ("s3express-signed call served by an s3 service (s3control)", signed(&Client { service: "s3express", ..s3 }, s3_get("k")).0, vec![], "s3", Ok(())),
        ("s3express-signed call sent to a non-S3 service", signed(&Client { service: "s3express", ..s3 }, s3_get("k")).0, vec![], "sqs", Err("AccessDenied")),
        ("s3-outposts key a//b sent as a/%2Fb, signed decoded", with_uri(signed(&Client { service: "s3-outposts", ..s3 }, s3_get("a//b")).0, "/a//b", "/a/%2Fb"), vec![], "s3-outposts", Ok(())),
        ("s3 key holding a literal %2F (boto3)", signed(&s3, s3_get("a%252Fb")).0, vec![], "s3", Ok(())),
        ("s3 key with ! + space ~ and UTF-8, encoded as SDKs send it", signed(&s3, s3_get("%21%2B%20~%D0%BA")).0, vec![], "s3", Ok(())),
        ("host left out of SignedHeaders", without_signed_host(signed(&SQS, send_message()).0), sent.clone(), "sqs", Err("SignatureDoesNotMatch")),
    ];
    for (name, parts, body, serving, expected) in cases {
        assert_eq!(verdict(&parts, &body, serving), expected, "{name}");
    }
}

fn with_header(
    mut parts: http::request::Parts,
    name: &'static str,
    value: &str,
) -> http::request::Parts {
    parts.headers.insert(name, value.parse().unwrap());
    parts
}

fn s3_get(key: &str) -> http::Request<Vec<u8>> {
    request("GET", &format!("https://127.0.0.1:8443/bucket/{key}"), b"")
}

fn without_signed_host(parts: http::request::Parts) -> http::request::Parts {
    let authorization = parts.headers["authorization"]
        .to_str()
        .unwrap()
        .replace(";host", "");
    with_header(parts, "authorization", &authorization)
}

fn with_uri(mut parts: http::request::Parts, from: &str, to: &str) -> http::request::Parts {
    parts.uri = parts.uri.to_string().replace(from, to).parse().unwrap();
    parts
}

#[test]
fn the_switch_is_off_by_default_and_the_credentials_file_is_private() {
    let home = std::env::temp_dir().join(format!("request-authentication-{}", std::process::id()));
    // SAFETY: no other test in this binary reads or writes these variables.
    unsafe { std::env::set_var("HOME", &home) };
    assert!(!authentication_enabled(), "off by default");
    for enabled in [true, false] {
        set_authentication(enabled);
        assert_eq!(authentication_enabled(), enabled);
    }
    let written = write_credentials_file(&emulator(), 4566);
    #[cfg(not(unix))]
    assert_eq!(written.unwrap_err().kind(), std::io::ErrorKind::Unsupported);
    #[cfg(unix)]
    {
        let file = written.unwrap();
        assert_eq!(file.0, home.join(".emulator/4566/emulator_credentials"));
        assert!(std::fs::read_to_string(&file.0).unwrap().contains(KEY));
        let mode = std::fs::metadata(&file.0).unwrap().permissions();
        assert_eq!(
            std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777,
            0o600
        );
        let path = file.0.clone();
        drop(file);
        assert!(!path.exists());
        let planted_target = home.join("planted");
        std::fs::write(&planted_target, "untouched").unwrap();
        std::os::unix::fs::symlink(&planted_target, &path).unwrap();
        let rewritten = write_credentials_file(&emulator(), 4566).unwrap();
        assert_eq!(
            std::fs::read_to_string(&planted_target).unwrap(),
            "untouched"
        );
        assert!(
            !std::fs::symlink_metadata(&rewritten.0)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let ca = write_emulator_file(4566, "ca.pem", "pem").unwrap();
        assert_eq!(std::fs::read_to_string(&ca.0).unwrap(), "pem");
        assert_eq!(ca.0, home.join(".emulator/4566/ca.pem"));
        created_secrets_stay_off_disk(&home.join(".emulator/4566"));
    }
}

/// Creates credentials, then finds only the emulator's own files in its port folder, none with
/// a created secret.
fn created_secrets_stay_off_disk(port_folder: &std::path::Path) {
    let created = store().create(&crate::aws_credentials::AWS_SESSION);
    let files: Vec<_> = std::fs::read_dir(port_folder).unwrap().map(|e| e.unwrap().path()).collect();
    let names: Vec<_> = files.iter().map(|path| path.file_name().unwrap()).collect();
    assert_eq!(names.len(), 2, "{files:?}");
    assert!(names.contains(&"emulator_credentials".as_ref()) && names.contains(&"ca.pem".as_ref()));
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap();
        assert!(created.secrets.values().all(|secret| !text.contains(secret.as_str())), "{file:?}");
    }
}

#[test]
fn vended_credentials_debug_redacts_the_secret() {
    let vended = crate::VendedCredentials::AwsAccessKey {
        access_key_id: "EMULATORKEY".to_string(),
        secret_access_key: "s3cr3t".to_string(),
    };
    let printed = format!("{vended:?}");
    assert!(
        printed.contains("EMULATORKEY") && !printed.contains("s3cr3t"),
        "{printed}"
    );
}
