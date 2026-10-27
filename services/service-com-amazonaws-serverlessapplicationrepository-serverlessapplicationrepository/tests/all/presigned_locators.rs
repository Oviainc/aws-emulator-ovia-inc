// SPDX-License-Identifier: CC0-1.0

//! The locators the Serverless Application Repository announces for the bodies it stores,
//! end to end over HTTP: what makes one followable, and what the document plane does with
//! one it did not mint.
//!
//! THE RULE under test, asserted over a SET of publishes rather than one recorded request:
//! every `*Url` this service reports for a body a publisher supplied INLINE is a PRESIGNED
//! download address — the address of the stored document plus the SigV4 query-string
//! fields (`X-Amz-Algorithm`, `X-Amz-Credential`, `X-Amz-Date`, `X-Amz-Expires`,
//! `X-Amz-SignedHeaders`, `X-Amz-Signature`) that authorise a `GET` of exactly that
//! address, so whoever holds the link can fetch the document with no credential of their
//! own. A link the PUBLISHER supplied is reported verbatim, unsigned: it is not this
//! service's to sign.
//!
//! What follows from that, and is asserted here:
//!
//! 1. every locator this service mints — a version's template, an application's license,
//!    its readme, a prepared template's — carries the six signing fields over its own
//!    address, with the algorithm, scope, lifetime and signed-header list the signature is
//!    computed under;
//! 2. the document plane honours exactly what it signed: a locator with its signature
//!    removed, rewritten, re-scoped or presented over another address is refused, and so
//!    is one past the expiry it states;
//! 3. a refused locator is refused BEFORE the store is read, so it reveals nothing about
//!    what is published, while a locator this service signed over an address nothing is
//!    published at is the absence it has always been.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_serverlessapplicationrepository_serverlessapplicationrepository::state;

use super::applications::{created, path_label, Answer, Repo};

/// The region every call of this suite is signed for, so the region a locator's scope
/// names and the one its path carries are both this.
const REGION: &str = "us-west-2";

/// The endpoint the publishes of this suite arrive on, which is the authority the locators
/// they announce are addressed to.
const HOST: &str = "127.0.0.1:9297";

/// A packaged SAM template as the JSON document a publisher sends inline.
fn template(resource: &str) -> String {
    json!({
        "Transform": "AWS::Serverless-2016-10-31",
        "Resources": { resource: { "Type": "AWS::SQS::Queue" } }
    })
    .to_string()
}

/// `url` split into the address a signature covers and the raw query it carries.
fn split_query(url: &str) -> (&str, &str) {
    url.split_once('?').unwrap_or((url, ""))
}

/// The value of the query field `name` of `url`, or `""` when it carries none.
fn field<'u>(url: &'u str, name: &str) -> &'u str {
    split_query(url)
        .1
        .split('&')
        .filter_map(|field| field.split_once('='))
        .find_map(|(field_name, value)| (field_name == name).then_some(value))
        .unwrap_or_default()
}

/// `url` with the query field `name` replaced by `value`, or appended when it carries none
/// — how a client rewrites one field of a locator it was handed.
fn with_field(url: &str, name: &str, value: &str) -> String {
    let (address, query) = split_query(url);
    let mut fields: Vec<String> = Vec::new();
    let mut replaced = false;
    for existing in query.split('&').filter(|field| !field.is_empty()) {
        let (field_name, _) = existing.split_once('=').unwrap_or((existing, ""));
        if field_name == name {
            replaced = true;
            if !value.is_empty() {
                fields.push(format!("{name}={value}"));
            }
        } else {
            fields.push(existing.to_string());
        }
    }
    if !replaced && !value.is_empty() {
        fields.push(format!("{name}={value}"));
    }
    if fields.is_empty() {
        address.to_string()
    } else {
        format!("{address}?{}", fields.join("&"))
    }
}

/// A member of an answer, as a string.
fn member(answer: &Answer, name: &str) -> String {
    answer.at(name).as_str().unwrap_or_default().to_string()
}

/// The `templateUrl` of the version an answer carries under `version`.
fn version_template_url(answer: &Answer) -> String {
    answer
        .at("version")
        .get("templateUrl")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `CreateApplication` from a client that reached the emulator at [`HOST`].
async fn publish(repo: &Repo, body: Value) -> Answer {
    let answer = repo
        .call_from_host("POST", "/applications", Some(body), Some(HOST))
        .await;
    assert_eq!(
        answer.status, 201,
        "publishing: expected 201, got {} with {}",
        answer.status, answer.body
    );
    answer
}

/// RULE 1: every locator this service mints carries the signature that authorises a `GET`
/// of its own address, under the algorithm, scope, lifetime and signed-header list that
/// signature is computed with.
///
/// The rows differ along what the rule generalises over: which artefact the locator
/// addresses (a version's template, an application's license, its readme), which call
/// announced it (`CreateApplication` with a `SemanticVersion`, `CreateApplicationVersion`,
/// `UpdateApplication`, `CreateCloudFormationTemplate`), and whether the artefact is
/// application-scoped or version-scoped. The last row is the control: a link the publisher
/// supplied is not this service's to sign.
#[tokio::test]
async fn every_minted_locator_is_a_presigned_download_address() {
    let repo = Repo::new();
    let license = "Apache License, Version 2.0\n";
    let readme = "# demo\n";
    let first = template("First");

    let created_app = publish(
        &repo,
        json!({
            "name": "presign-create",
            "author": "example-author",
            "description": "a demo app",
            "semanticVersion": "1.0.0",
            "templateBody": first,
            "licenseBody": license,
            "readmeBody": readme
        }),
    )
    .await;
    let application_id = member(&created_app, "applicationId");

    let second = template("Second");
    let published = repo
        .call_from_host(
            "PUT",
            &format!(
                "/applications/{}/versions/2.0.0",
                path_label(&application_id)
            ),
            Some(json!({ "templateBody": second })),
            Some(HOST),
        )
        .await;
    assert_eq!(
        published.status, 201,
        "publishing a second version: got {} with {}",
        published.status, published.body
    );

    let patched_readme = "# patched\n";
    // Its own application: a readme is application-scoped, so patching the one above would
    // replace the document the create's own locator addresses.
    let patch_target = created(&repo, "presign-patch").await;
    let patched = repo
        .call_from_host(
            "PATCH",
            &format!("/applications/{}", path_label(&patch_target)),
            Some(json!({ "readmeBody": patched_readme })),
            Some(HOST),
        )
        .await;
    assert_eq!(
        patched.status, 200,
        "patching the readme: got {} with {}",
        patched.status, patched.body
    );

    let prepared = repo
        .call_from_host(
            "POST",
            &format!("/applications/{}/templates", path_label(&application_id)),
            Some(json!({ "semanticVersion": "2.0.0" })),
            Some(HOST),
        )
        .await;
    assert_eq!(
        prepared.status, 201,
        "preparing a template: got {} with {}",
        prepared.status, prepared.body
    );

    let by_url = publish(
        &repo,
        json!({
            "name": "presign-by-url",
            "author": "example-author",
            "description": "a demo app",
            "semanticVersion": "1.0.0",
            "templateUrl": "https://example.com/packaged/template.yaml"
        }),
    )
    .await;

    // (case, the locator announced, the address it must be signed over, the bytes it
    //  serves — `None` for a link this service did not mint)
    let cases: Vec<(&str, String, String, Option<String>)> = vec![
        (
            "the template of the version the create published",
            version_template_url(&created_app),
            artifact_address("presign-create", Some("1.0.0"), "template.yaml"),
            Some(first.clone()),
        ),
        (
            "the license the create published",
            member(&created_app, "licenseUrl"),
            artifact_address("presign-create", None, "license.txt"),
            Some(license.to_string()),
        ),
        (
            "the readme the create published",
            member(&created_app, "readmeUrl"),
            artifact_address("presign-create", None, "readme.md"),
            Some(readme.to_string()),
        ),
        (
            "the template CreateApplicationVersion published",
            member(&published, "templateUrl"),
            artifact_address("presign-create", Some("2.0.0"), "template.yaml"),
            Some(second.clone()),
        ),
        (
            "the readme UpdateApplication wrote",
            member(&patched, "readmeUrl"),
            artifact_address("presign-patch", None, "readme.md"),
            Some(patched_readme.to_string()),
        ),
        (
            "the template CreateCloudFormationTemplate prepared",
            member(&prepared, "templateUrl"),
            artifact_address("presign-create", Some("2.0.0"), "template.yaml"),
            Some(second.clone()),
        ),
        (
            "a link the publisher supplied, which this service does not sign",
            version_template_url(&by_url),
            "https://example.com/packaged/template.yaml".to_string(),
            None,
        ),
    ];

    for (case, locator, address, bytes) in &cases {
        let (announced_address, query) = split_query(locator);
        assert_eq!(
            announced_address, address,
            "case {case:?}: the locator addresses the stored document, got {locator:?}"
        );
        let Some(bytes) = bytes else {
            assert_eq!(
                query, "",
                "case {case:?}: a publisher's own link is reported verbatim, got {locator:?}"
            );
            continue;
        };
        assert_signing_fields(case, locator);
        // And the signature is honoured: the address announced is the address served.
        let fetched = repo.follow("GET", locator, false).await;
        assert_eq!(
            (fetched.status, fetched.body.as_str()),
            (200, bytes.as_str()),
            "case {case:?}: following {locator:?} must serve the published body, got {} with \
             {:?}",
            fetched.status,
            fetched.body
        );
    }
}

/// The address a locator of this suite's application is announced at: the published path,
/// without the signature.
fn artifact_address(name: &str, semantic_version: Option<&str>, file_name: &str) -> String {
    let account = "000000000000";
    match semantic_version {
        Some(semantic_version) => format!(
            "https://{HOST}/serverlessrepo-artifacts/{REGION}/{account}/{name}/{semantic_version}/\
             {file_name}"
        ),
        None => format!(
            "https://{HOST}/serverlessrepo-artifacts/{REGION}/{account}/{name}/{file_name}"
        ),
    }
}

/// The six query-string fields a presigned download address carries, each held to what the
/// signature was computed under: SigV4's algorithm name, the credential scope of this
/// request's region and this service's signing name, the lifetime the locator states, the
/// host it is signed over, and a SHA-256 signature.
fn assert_signing_fields(case: &str, locator: &str) {
    assert_eq!(
        field(locator, "X-Amz-Algorithm"),
        "AWS4-HMAC-SHA256",
        "case {case:?}: the locator states the signing algorithm, got {locator:?}"
    );
    let date = field(locator, "X-Amz-Date");
    assert!(
        date.len() == 16
            && date.as_bytes()[8] == b'T'
            && date.ends_with('Z')
            && date[0..8].bytes().all(|byte| byte.is_ascii_digit())
            && date[9..15].bytes().all(|byte| byte.is_ascii_digit()),
        "case {case:?}: X-Amz-Date is the basic SigV4 timestamp YYYYMMDDTHHMMSSZ, got {date:?}"
    );
    // The credential names the signing key and the scope the signature is bound to; the
    // field's `/` separators travel percent-encoded in a query string.
    let credential = field(locator, "X-Amz-Credential").replace("%2F", "/");
    let scope: Vec<&str> = credential.split('/').collect();
    assert_eq!(
        scope.len(),
        5,
        "case {case:?}: X-Amz-Credential is <key>/<date>/<region>/<service>/aws4_request, got \
         {credential:?}"
    );
    assert!(
        scope[0].len() == 20
            && scope[0]
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit()),
        "case {case:?}: the credential names a 20-character signing key, got {:?}",
        scope[0]
    );
    assert_eq!(
        (scope[1], scope[2], scope[3], scope[4]),
        (&date[0..8], REGION, "serverlessrepo", "aws4_request"),
        "case {case:?}: the credential scope is this request's day, region and service, got \
         {credential:?}"
    );
    assert_eq!(
        field(locator, "X-Amz-Expires"),
        state::ARTIFACT_URL_LIFETIME.as_secs().to_string(),
        "case {case:?}: the locator states its lifetime in seconds, got {locator:?}"
    );
    assert_eq!(
        field(locator, "X-Amz-SignedHeaders"),
        "host",
        "case {case:?}: the signature covers the host the locator is addressed to, got \
         {locator:?}"
    );
    let signature = field(locator, "X-Amz-Signature");
    assert!(
        signature.len() == 64 && signature.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "case {case:?}: the locator carries a SHA-256 signature, got {signature:?}"
    );
}

/// RULE 2 and 3: the document plane serves exactly the locators it signed, and only until
/// the expiry each one states — every other presentation of one is refused, and refused
/// without reading the store.
///
/// The rows span the ways a locator can fail to be one this service minted and honours: no
/// signature at all, each signing field removed or rewritten, a lifetime longer than any
/// this service mints, an address the signature does not cover (another application's
/// document, another host, an extra query field of the client's own), and a locator that is
/// genuinely this service's but whose stated lifetime has run out. The two rows that must
/// NOT be refused as unsigned are the controls: the locator as announced, and one this
/// service signed over an address nothing is published at — the absence a `404` reports.
#[tokio::test]
async fn the_document_plane_honours_only_the_locators_it_signed() {
    let repo = Repo::new();
    let body = template("Queue");
    let other = template("Other");

    let mine = publish(
        &repo,
        json!({
            "name": "presign-mine",
            "author": "example-author",
            "description": "a demo app",
            "semanticVersion": "1.0.0",
            "templateBody": body
        }),
    )
    .await;
    let locator = version_template_url(&mine);

    let theirs = publish(
        &repo,
        json!({
            "name": "presign-theirs",
            "author": "example-author",
            "description": "a demo app",
            "semanticVersion": "1.0.0",
            "templateBody": other
        }),
    )
    .await;
    let other_locator = version_template_url(&theirs);

    // One hex digit of the signature rewritten: still 64 hex characters, still this
    // service's shape, and not the signature of this address.
    let signature = field(&locator, "X-Amz-Signature").to_string();
    let flipped: String = signature
        .char_indices()
        .map(|(index, digit)| {
            if index == 0 {
                if digit == '0' {
                    '1'
                } else {
                    '0'
                }
            } else {
                digit
            }
        })
        .collect();

    // A locator this service signed, with the lifetime it mints, but signed long enough
    // ago that the lifetime has run out. Minted through the service's own signing path, so
    // what is under test is the expiry and not a mangled field.
    let lifetime = state::ARTIFACT_URL_LIFETIME;
    let expired = state::presigned_artifact_url(
        &artifact_address("presign-mine", Some("1.0.0"), "template.yaml"),
        REGION,
        SystemTime::now() - (lifetime + Duration::from_secs(60)),
        lifetime,
    );
    // The same signing path over an address nothing is published at: the signature is this
    // service's, so the answer is about the document, not about the signature.
    let unpublished = state::presigned_artifact_url(
        &artifact_address("presign-mine", Some("9.9.9"), "template.yaml"),
        REGION,
        SystemTime::now(),
        lifetime,
    );

    // (case, the locator as presented, the status expected)
    let cases: Vec<(String, String, u16)> = vec![
        ("the locator as announced".to_string(), locator.clone(), 200),
        (
            "the address with no signature at all".to_string(),
            split_query(&locator).0.to_string(),
            403,
        ),
        (
            "the signature field removed".to_string(),
            with_field(&locator, "X-Amz-Signature", ""),
            403,
        ),
        (
            "one digit of the signature rewritten".to_string(),
            with_field(&locator, "X-Amz-Signature", &flipped),
            403,
        ),
        (
            "the signing timestamp moved".to_string(),
            with_field(&locator, "X-Amz-Date", "20240101T000000Z"),
            403,
        ),
        (
            "the timestamp replaced by something that is not one".to_string(),
            with_field(&locator, "X-Amz-Date", "yesterday"),
            403,
        ),
        (
            "the stated lifetime raised".to_string(),
            with_field(&locator, "X-Amz-Expires", "900000"),
            403,
        ),
        (
            "the stated lifetime shortened".to_string(),
            with_field(&locator, "X-Amz-Expires", "60"),
            403,
        ),
        (
            "the credential scope rewritten to another region".to_string(),
            with_field(
                &locator,
                "X-Amz-Credential",
                &field(&locator, "X-Amz-Credential").replace(REGION, "eu-west-1"),
            ),
            403,
        ),
        (
            "the signed-header list emptied".to_string(),
            with_field(&locator, "X-Amz-SignedHeaders", ""),
            403,
        ),
        (
            "a query field of the client's own appended".to_string(),
            format!("{locator}&download=1"),
            403,
        ),
        (
            "more query fields than any locator this service mints".to_string(),
            format!(
                "{locator}{}",
                (0..64)
                    .map(|field| format!("&field{field}=x"))
                    .collect::<String>()
            ),
            403,
        ),
        (
            "this signature over another application's document".to_string(),
            format!(
                "{}?{}",
                split_query(&other_locator).0,
                split_query(&locator).1
            ),
            403,
        ),
        (
            "the locator followed through another endpoint host".to_string(),
            locator.replacen(HOST, "elsewhere.example:443", 1),
            403,
        ),
        (
            "a locator whose stated lifetime has run out".to_string(),
            expired,
            403,
        ),
        (
            "a locator signed over an address nothing is published at".to_string(),
            unpublished,
            404,
        ),
        (
            // A path space whose name merely starts with the published prefix is not this
            // service's document plane at all: it names no modelled operation either, so
            // the framework's own routing answer stands and nothing is served from it.
            "a sibling path space whose name starts with the published prefix".to_string(),
            locator.replacen(
                "/serverlessrepo-artifacts/",
                "/serverlessrepo-artifacts-elsewhere/",
                1,
            ),
            501,
        ),
    ];

    for (case, presented, expected) in &cases {
        for method in ["GET", "HEAD"] {
            let fetched = repo.follow(method, presented, false).await;
            assert_eq!(
                fetched.status, *expected,
                "case {case:?} ({method}): expected {expected}, got {} with {:?}",
                fetched.status, fetched.body
            );
            assert!(
                *expected == 200 || !fetched.body.contains(&body),
                "case {case:?} ({method}): a refused locator serves no document, got {:?}",
                fetched.body
            );
        }
    }

    // The refusals changed nothing: the announced locator still serves its document, and
    // the other application's still serves its own.
    for (locator, expected) in [(&locator, &body), (&other_locator, &other)] {
        let fetched = repo.follow("GET", locator, false).await;
        assert_eq!(
            (fetched.status, fetched.body.as_str()),
            (200, expected.as_str()),
            "after the refusals: {locator:?} must still serve its document, got {} with {:?}",
            fetched.status,
            fetched.body
        );
    }
}
