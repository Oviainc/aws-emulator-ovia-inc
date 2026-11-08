// SPDX-License-Identifier: CC0-1.0

//! What MEDIA TYPE this service declares on a successful answer.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with and reads the RAW response, because what is under test
//! is a response HEADER rather than a body member.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one example:
//!
//! 1. Every successful answer this service puts on the wire declares `application/json` for
//!    the whole response — including `GetPackageVersionAsset`, whose body is a package
//!    version's raw asset bytes rather than a JSON document. Rows cover the asset read over
//!    assets of different sizes and byte shapes (binary, text, bytes that are themselves
//!    JSON) beside the JSON-bodied reads of the same records, so the rule is shown to be the
//!    SERVICE's and not one operation's.
//! 2. The media type does not change the BODY: the asset read still answers the published
//!    bytes byte for byte, with the asset name, the version and the version's revision in
//!    their own headers.
//! 3. A refusal is a JSON document too, so the media type of a refused read is the same
//!    `application/json` the successful one declares.

use smithy_server_runtime::MultiServiceRouter;

// The HTTP harness the crate's suites share and the publish helpers from the suite that
// owns `PublishPackageVersion`.
use crate::harness::{auth_for, REGION};
use crate::package_version_publish::{
    binary_content, emulator_with_repository, publish, Publish,
};

/// The media type every successful answer of this service declares.
const JSON_MEDIA_TYPE: &str = "application/json";

/// One raw answer: the status, the declared media type and the body bytes, plus the three
/// metadata headers an asset read binds.
#[derive(Debug)]
struct Raw {
    status: u16,
    content_type: Option<String>,
    asset_name: Option<String>,
    package_version: Option<String>,
    revision: Option<String>,
    body: Vec<u8>,
}

/// Send one request and keep the whole response, undecoded.
async fn send(router: &MultiServiceRouter, method: &str, uri: &str, body: &[u8]) -> Raw {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("codeartifact.{REGION}.amazonaws.com"))
        .header("authorization", auth_for(REGION))
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("build request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };
    let content_type = header("content-type");
    let asset_name = header("X-AssetName");
    let package_version = header("X-PackageVersion");
    let revision = header("X-PackageVersionRevision");
    Raw {
        status,
        content_type,
        asset_name,
        package_version,
        revision,
        body: response.into_body().to_vec(),
    }
}

/// The asset read of exactly what a [`Publish`] stored.
fn asset_uri(published: &Publish) -> String {
    let mut uri = format!(
        "/v1/package/version/asset?domain={}&repository={}&format={}&package={}&version={}&asset={}",
        published.domain,
        published.repository,
        published.format,
        published.package,
        published.version,
        published.asset_name
    );
    if let Some(namespace) = &published.namespace {
        uri.push_str(&format!("&namespace={namespace}"));
    }
    uri
}

/// Publish `case`, asserting it landed.
async fn given_published(router: &MultiServiceRouter, case: &Publish, label: &str) {
    let answer = publish(router, case).await;
    assert_eq!(
        (answer.status, answer.code()),
        (200, String::new()),
        "case {label:?}: the publish must succeed, got {answer:?}"
    );
}

// ---------------------------------------------------------------------------
// Rules 1 and 2: the asset read declares the service's media type and answers
// the published bytes.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_asset_read_declares_the_services_media_type_and_the_published_bytes() {
    let router = emulator_with_repository().await;

    // Rows differ along what the rule generalises over: the asset's size and byte shape —
    // including bytes that are themselves a JSON document and bytes that are plain text,
    // neither of which may change the header or the body.
    let cases: Vec<(&str, Publish)> = vec![
        (
            "one byte",
            Publish::new("media-one", "1.0.0", "payload.bin", vec![0x00]),
        ),
        (
            "4 KiB of cycling binary",
            Publish::new("media-binary", "1.0.0", "bundle.tar.gz", binary_content(4096, 7)),
        ),
        (
            "plain text",
            Publish::new("media-text", "2.0.0", "notes.txt", b"hello generic\n".to_vec()),
        ),
        (
            "bytes that are themselves JSON",
            Publish::new(
                "media-json",
                "3.0.0",
                "manifest.json",
                br#"{"name":"not-the-envelope"}"#.to_vec(),
            ),
        ),
        (
            "a single high byte",
            Publish::new("media-high", "0.0.1", "one.bin", vec![0xFF]),
        ),
    ];

    for (case, published) in &cases {
        given_published(&router, published, case).await;
        let answer = send(&router, "GET", &asset_uri(published), &[]).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the asset must download, got {answer:?}"
        );
        assert_eq!(
            answer.content_type.as_deref(),
            Some(JSON_MEDIA_TYPE),
            "case {case:?}: expected the answer to declare {JSON_MEDIA_TYPE}, got {:?}",
            answer.content_type
        );
        // RULE 2: the declared media type changed nothing about the answer itself.
        assert_eq!(
            answer.body, published.content,
            "case {case:?}: expected the published bytes, got {} bytes",
            answer.body.len()
        );
        assert_eq!(
            answer.asset_name.as_deref(),
            Some(published.asset_name.as_str()),
            "case {case:?}: expected the stored asset name, got {:?}",
            answer.asset_name
        );
        assert_eq!(
            answer.package_version.as_deref(),
            Some(published.version.as_str()),
            "case {case:?}: expected the version header, got {:?}",
            answer.package_version
        );
        assert!(
            answer
                .revision
                .as_deref()
                .is_some_and(|revision| !revision.is_empty()),
            "case {case:?}: expected the version's revision header, got {:?}",
            answer.revision
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 1 across the service: the JSON-bodied reads of the same records declare
// the same media type, so the rule is the service's and not the asset read's.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_successful_answer_of_this_service_declares_one_media_type() {
    let router = emulator_with_repository().await;

    let published = Publish::new("media-wide", "1.0.0", "payload.bin", binary_content(64, 3));
    given_published(&router, &published, "the record every read below is about").await;

    let coordinate = format!(
        "domain={}&repository={}&format={}&namespace={}&package={}",
        published.domain,
        published.repository,
        published.format,
        published.namespace.clone().unwrap_or_default(),
        published.package
    );

    // One row per read: the asset download whose body is raw bytes, and the JSON-bodied
    // answers of the same records — a version, a listing, a package and the domain.
    let reads: Vec<(&str, &str, String)> = vec![
        ("the asset download", "GET", asset_uri(&published)),
        (
            "the version description",
            "GET",
            format!("/v1/package/version?{coordinate}&version={}", published.version),
        ),
        (
            "the version listing",
            "POST",
            format!("/v1/package/versions?{coordinate}"),
        ),
        (
            "the asset listing",
            "POST",
            format!(
                "/v1/package/version/assets?{coordinate}&version={}",
                published.version
            ),
        ),
        (
            "the package description",
            "GET",
            format!("/v1/package?{coordinate}"),
        ),
        (
            "the domain description",
            "GET",
            format!("/v1/domain?domain={}", published.domain),
        ),
    ];

    for (case, method, uri) in &reads {
        let body: &[u8] = if *method == "POST" { b"{}" } else { b"" };
        let answer = send(&router, method, uri, body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the read must succeed, got {answer:?}"
        );
        assert_eq!(
            answer.content_type.as_deref(),
            Some(JSON_MEDIA_TYPE),
            "case {case:?}: expected the answer to declare {JSON_MEDIA_TYPE}, got {:?}",
            answer.content_type
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a refusal declares the same media type.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_refused_asset_read_declares_the_same_media_type() {
    let router = emulator_with_repository().await;

    let published = Publish::new("media-refusal", "1.0.0", "payload.bin", binary_content(32, 11));
    given_published(&router, &published, "the version the refusals are addressed around").await;

    // Rows differ along WHICH step of the coordinate does not resolve, so the refusal is a
    // different modelled failure each time.
    let mut other_version = published.clone();
    other_version.version = "9.9.9".to_string();
    let mut other_asset = published.clone();
    other_asset.asset_name = "absent.bin".to_string();
    let mut other_package = published.clone();
    other_package.package = "media-absent".to_string();

    for (case, request) in [
        ("a version the package does not hold", &other_version),
        ("an asset the version does not hold", &other_asset),
        ("a package the repository does not hold", &other_package),
    ] {
        let answer = send(&router, "GET", &asset_uri(request), &[]).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected the modelled 404, got {answer:?}"
        );
        assert_eq!(
            answer.content_type.as_deref(),
            Some(JSON_MEDIA_TYPE),
            "case {case:?}: expected the refusal to declare {JSON_MEDIA_TYPE}, got {:?}",
            answer.content_type
        );
        let document: serde_json::Value =
            serde_json::from_slice(&answer.body).unwrap_or(serde_json::Value::Null);
        assert!(
            document["message"].as_str().is_some(),
            "case {case:?}: expected the modelled error document, got {answer:?}"
        );
    }
}
