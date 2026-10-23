// SPDX-License-Identifier: CC0-1.0

//! The artifact document plane of the Serverless Application Repository, end to end over
//! HTTP: the locators a publish announces for a body supplied INLINE, and the fetch of
//! them.
//!
//! The rule under test, asserted over a SET of publishes rather than one recorded
//! request: a `*Url` this service announces for a body the publisher supplied inline is
//! an address the caller can FOLLOW — `Version$TemplateUrl` is "A link to the template
//! that can be used to deploy the application using AWS CloudFormation"
//! (`smithy_query(query="doc", shape="Version", member="TemplateUrl")`), and the
//! `TemplateUrl` of a prepared template is the same link — so a plain, unsigned `GET` of
//! it answers HTTP 200 carrying exactly the bytes that publish retained, and a locator
//! with nothing published behind it answers 404 rather than inventing any.
//!
//! What follows from that, and is asserted here:
//!
//! 1. every inline artefact — a version's template, an application's license, its
//!    readme — is served at the address its own publish announced, byte for byte;
//! 2. the announced address is on the endpoint the publishing request arrived at, so a
//!    client is never sent out of the emulator to fetch it, and it is served whether the
//!    fetch is signed or not;
//! 3. a fetch is a pure READ: two fetches answer the same bytes, and a `HEAD` reports the
//!    same metadata without them;
//! 4. an address with nothing published behind it — a body the publisher named by `*Url`
//!    instead of supplying inline, an application that has been deleted, a version never
//!    published, another account's catalogue, a crossed artefact path — is 404, and a
//!    method that is not a download is 405.
//!
//! The harness (`Repo`, `Answer`, `Fetched`) is the `applications` suite's, so the
//! control-plane calls here drive the same wire path the other suites do and the data
//! plane reads the one store those calls wrote.

use serde_json::{json, Value};

use super::applications::{created, minimal, path_label, Answer, Fetched, Repo};
use super::version_templates::SAM_TRANSFORM;

/// A packaged SAM template as the JSON document a publisher sends inline.
fn json_template(resource: &str) -> String {
    json!({
        "Transform": SAM_TRANSFORM,
        "Resources": { resource: { "Type": "AWS::SQS::Queue" } }
    })
    .to_string()
}

/// The same packaged SAM template as the YAML document a publisher may send instead —
/// retained as supplied, because this emulator reads only the JSON form and never
/// rewrites what it cannot read.
fn yaml_template(resource: &str) -> String {
    format!(
        "Transform: {SAM_TRANSFORM}\nResources:\n  {resource}:\n    Type: AWS::SQS::Queue\n"
    )
}

/// `CreateApplication`, from a client that reached the emulator at `host`.
async fn publish(repo: &Repo, host: Option<&str>, body: Value) -> Answer {
    let answer = repo
        .call_from_host("POST", "/applications", Some(body), host)
        .await;
    assert_eq!(
        answer.status, 201,
        "publishing: expected 201, got {} with {}",
        answer.status, answer.body
    );
    answer
}

/// The `templateUrl` of the version a create reported.
fn version_template_url(answer: &Answer) -> String {
    answer
        .at("version")
        .get("templateUrl")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A member of a create's answer, as a string.
fn member(answer: &Answer, name: &str) -> String {
    answer.at(name).as_str().unwrap_or_default().to_string()
}

/// Prepare a CloudFormation template of `application_id` and answer with the record.
async fn prepare(repo: &Repo, application_id: &str, semantic_version: &str) -> Answer {
    let path = format!("/applications/{}/templates", path_label(application_id));
    let answer = repo
        .call(
            "POST",
            &path,
            Some(json!({ "semanticVersion": semantic_version })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "preparing a template: expected 201, got {} with {}",
        answer.status, answer.body
    );
    answer
}

/// RULE 1 and 3: `url` serves exactly `expected`, signed or unsigned, as often as it is
/// followed; a `HEAD` of it reports the same metadata and no bytes.
async fn assert_serves(repo: &Repo, case: &str, url: &str, expected: &str, content_type: &str) {
    assert!(
        url.contains("/serverlessrepo-artifacts/"),
        "case {case:?}: an inline body is announced at an address this emulator serves, \
         got {url:?}"
    );
    for signed in [false, true] {
        let got: Fetched = repo.follow("GET", url, signed).await;
        assert_eq!(
            got.status, 200,
            "case {case:?}: following {url:?} (signed={signed}) must serve the published \
             body, got {} with {:?}",
            got.status, got.body
        );
        assert_eq!(
            got.body, expected,
            "case {case:?}: (signed={signed}) expected the retained body, got {:?}",
            got.body
        );
        assert_eq!(
            got.content_type, content_type,
            "case {case:?}: (signed={signed}) expected content-type {content_type:?}, got {:?}",
            got.content_type
        );
        assert_eq!(
            got.content_length,
            Some(expected.len()),
            "case {case:?}: (signed={signed}) expected a content-length of {}, got {:?}",
            expected.len(),
            got.content_length
        );
    }
    // A HEAD carries what a GET of the same address would answer ABOUT the document —
    // its type and its length — without the document itself.
    let head = repo.follow("HEAD", url, false).await;
    assert_eq!(
        head.status, 200,
        "case {case:?}: a HEAD of {url:?} answers 200, got {} with {:?}",
        head.status, head.body
    );
    assert!(
        head.body.is_empty(),
        "case {case:?}: a HEAD carries no body, got {:?}",
        head.body
    );
    assert_eq!(
        head.content_length,
        Some(expected.len()),
        "case {case:?}: a HEAD reports the document's length, got {:?}",
        head.content_length
    );
}

/// RULE 1: every artefact a publisher supplies INLINE is served at the address that
/// publish announced, byte for byte.
///
/// The rows differ along what the rule generalises over: which artefact it is (a
/// version's template, an application's license, its readme), which publish announced it
/// (`CreateApplication` with a `SemanticVersion`, `CreateApplicationVersion`,
/// `UpdateApplication`), the form of the retained bytes (a JSON template, a YAML
/// template, plain text, Markdown, non-ASCII text), and how many of the three one
/// application publishes at once.
#[tokio::test]
async fn an_inline_body_is_served_at_the_announced_locator() {
    let repo = Repo::new();

    // (case, the name to publish under, the create request, the locators to follow:
    //  member of the answer, the bytes expected, the media type expected)
    let license = "Apache License, Version 2.0\n\nTERMS AND CONDITIONS\n";
    let readme = "# demo\n\nA *serverless* application.\n";
    let unicode_readme = "# ünïcödé\n\nSchrödinger's \u{1f4e6}\n";
    let json_body = json_template("Queue");
    let yaml_body = yaml_template("Queue");

    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, &str, Value, Vec<(&str, String, &str)>)> = vec![
        (
            "a license supplied inline",
            "artifact-license",
            json!({
                "name": "artifact-license",
                "author": "example-author",
                "description": "a demo app",
                "licenseBody": license
            }),
            vec![("licenseUrl", license.to_string(), "text/plain; charset=utf-8")],
        ),
        (
            "a readme supplied inline",
            "artifact-readme",
            json!({
                "name": "artifact-readme",
                "author": "example-author",
                "description": "a demo app",
                "readmeBody": readme
            }),
            vec![(
                "readmeUrl",
                readme.to_string(),
                "text/markdown; charset=utf-8",
            )],
        ),
        (
            "a readme of non-ASCII text",
            "artifact-unicode",
            json!({
                "name": "artifact-unicode",
                "author": "example-author",
                "description": "a demo app",
                "readmeBody": unicode_readme
            }),
            vec![(
                "readmeUrl",
                unicode_readme.to_string(),
                "text/markdown; charset=utf-8",
            )],
        ),
        (
            "a JSON SAM template published with the first version",
            "artifact-json-template",
            json!({
                "name": "artifact-json-template",
                "author": "example-author",
                "description": "a demo app",
                "semanticVersion": "1.0.0",
                "templateBody": json_body
            }),
            vec![("version.templateUrl", json_body.clone(), "application/json")],
        ),
        (
            "a YAML SAM template published with the first version",
            "artifact-yaml-template",
            json!({
                "name": "artifact-yaml-template",
                "author": "example-author",
                "description": "a demo app",
                "semanticVersion": "1.0.0",
                "templateBody": yaml_body
            }),
            vec![(
                "version.templateUrl",
                yaml_body.clone(),
                "application/x-yaml",
            )],
        ),
        (
            "all three artefacts of one application, published in one call",
            "artifact-all-three",
            json!({
                "name": "artifact-all-three",
                "author": "example-author",
                "description": "a demo app",
                "semanticVersion": "2.1.3",
                "templateBody": json_body,
                "licenseBody": license,
                "readmeBody": readme
            }),
            vec![
                ("version.templateUrl", json_body.clone(), "application/json"),
                ("licenseUrl", license.to_string(), "text/plain; charset=utf-8"),
                (
                    "readmeUrl",
                    readme.to_string(),
                    "text/markdown; charset=utf-8",
                ),
            ],
        ),
    ];

    for (case, _name, request, locators) in &cases {
        let created = publish(&repo, Some("127.0.0.1:9297"), request.clone()).await;
        for (locator, expected, content_type) in locators {
            let url = if *locator == "version.templateUrl" {
                version_template_url(&created)
            } else {
                member(&created, locator)
            };
            assert_serves(&repo, case, &url, expected, content_type).await;
        }
    }

    // A version published by `CreateApplicationVersion` announces its own locator, and a
    // readme written by `UpdateApplication` replaces the one the create announced — both
    // are served the same way.
    let application_id = created(&repo, "artifact-later-publishes").await;
    let second = json_template("Second");
    let published = repo
        .create_version(&application_id, "2.0.0", json!({ "templateBody": second }))
        .await;
    assert_eq!(
        published.status, 201,
        "publishing a second version: got {} with {}",
        published.status, published.body
    );
    assert_serves(
        &repo,
        "a version published on its own",
        &member(&published, "templateUrl"),
        &second,
        "application/json",
    )
    .await;

    let patched_readme = "# patched\n\nRewritten by UpdateApplication.\n";
    let patched = repo
        .call(
            "PATCH",
            &format!("/applications/{}", path_label(&application_id)),
            Some(json!({ "readmeBody": patched_readme })),
        )
        .await;
    assert_eq!(
        patched.status, 200,
        "patching the readme: got {} with {}",
        patched.status, patched.body
    );
    assert_serves(
        &repo,
        "a readme written by UpdateApplication",
        &member(&patched, "readmeUrl"),
        patched_readme,
        "text/markdown; charset=utf-8",
    )
    .await;

    // The TemplateUrl of a PREPARED template is the location of the body published with
    // the version it was prepared from, so it is the very same served address: the
    // artifact a CloudFormation client deploys from.
    let prepared = prepare(&repo, &application_id, "2.0.0").await;
    assert_eq!(
        member(&prepared, "templateUrl"),
        member(&published, "templateUrl"),
        "a prepared template serves the body published with its version, got {}",
        prepared.body
    );
    assert_serves(
        &repo,
        "the TemplateUrl of a prepared template",
        &member(&prepared, "templateUrl"),
        &second,
        "application/json",
    )
    .await;
}

/// RULE 2: the announced address is on the endpoint the publishing request arrived at, so
/// following it lands back on this emulator rather than on a public AWS hostname.
///
/// The rows are the endpoint forms a client reaches an emulator through: loopback by
/// address, by name, a port-less host, and a deployment's own DNS name. A request that
/// names no host at all has no emulator endpoint to announce, and is the one row that
/// falls back to the real service's own endpoint.
#[tokio::test]
async fn the_announced_locator_is_on_the_endpoint_the_publish_arrived_at() {
    let repo = Repo::new();
    let body = json_template("Queue");

    let cases: Vec<(&str, Option<&str>, &str)> = vec![
        ("loopback by address", Some("127.0.0.1:9297"), "127.0.0.1:9297"),
        ("loopback by name", Some("localhost:9297"), "localhost:9297"),
        ("a host with no port", Some("emulator"), "emulator"),
        (
            "a deployment's own DNS name",
            Some("aws-emulator.svc.cluster.local:8443"),
            "aws-emulator.svc.cluster.local:8443",
        ),
        // No `Host` at all: nothing of the emulator is named, so the only honest address
        // is the real service's own endpoint for the request's region.
        (
            "a request that names no host",
            None,
            "serverlessrepo.us-west-2.amazonaws.com",
        ),
    ];

    for (index, (case, host, expected_authority)) in cases.iter().enumerate() {
        let name = format!("artifact-endpoint-{index}");
        let created = publish(
            &repo,
            *host,
            json!({
                "name": name,
                "author": "example-author",
                "description": "a demo app",
                "semanticVersion": "1.0.0",
                "templateBody": body
            }),
        )
        .await;
        let url = version_template_url(&created);
        assert_eq!(
            url,
            format!(
                "https://{expected_authority}/serverlessrepo-artifacts/us-west-2/\
                 000000000000/{name}/1.0.0/template.yaml"
            ),
            "case {case:?}: the locator is announced on the endpoint the publish arrived \
             at, got {url:?}"
        );
        // And it is served there: the address announced and the address served are one.
        assert_serves(&repo, case, &url, &body, "application/json").await;
    }
}

/// RULE 4: an address with nothing published behind it is 404, and a request that is not
/// a download is 405.
///
/// The rows span every way a well-formed locator can address nothing: a body the
/// publisher named by `*Url` rather than supplying inline, an application that has since
/// been deleted, a version that was never published, an application of another region or
/// another account, an artefact crossed with the wrong scope (a template without its
/// version, a license under one), and a file name this service never mints.
#[tokio::test]
async fn an_address_with_nothing_published_behind_it_is_not_found() {
    let repo = Repo::new();
    let host = "127.0.0.1:9297";
    let body = json_template("Queue");

    // One application publishing its template inline, one naming it by URL, and one that
    // is published and then deleted.
    let inline = publish(
        &repo,
        Some(host),
        json!({
            "name": "artifact-present",
            "author": "example-author",
            "description": "a demo app",
            "semanticVersion": "1.0.0",
            "templateBody": body,
            "readmeBody": "# demo\n"
        }),
    )
    .await;
    let served = version_template_url(&inline);
    assert_serves(&repo, "the control", &served, &body, "application/json").await;

    let by_url = publish(
        &repo,
        Some(host),
        json!({
            "name": "artifact-by-url",
            "author": "example-author",
            "description": "a demo app",
            "semanticVersion": "1.0.0",
            "templateUrl": "https://example.com/packaged/template.yaml"
        }),
    )
    .await;
    assert_eq!(
        version_template_url(&by_url),
        "https://example.com/packaged/template.yaml",
        "a publisher's own link is reported verbatim, got {}",
        by_url.body
    );

    let doomed = publish(
        &repo,
        Some(host),
        json!({
            "name": "artifact-doomed",
            "author": "example-author",
            "description": "a demo app",
            "semanticVersion": "1.0.0",
            "templateBody": body
        }),
    )
    .await;
    let doomed_url = version_template_url(&doomed);
    assert_serves(
        &repo,
        "before the delete",
        &doomed_url,
        &body,
        "application/json",
    )
    .await;
    let deleted = repo
        .delete_application(&member(&doomed, "applicationId"))
        .await;
    assert_eq!(
        deleted.status, 204,
        "deleting the application: got {} with {}",
        deleted.status, deleted.body
    );

    let prefix = format!("https://{host}/serverlessrepo-artifacts");
    let cases: Vec<(&str, String)> = vec![
        (
            "a template whose publisher named it by TemplateUrl, so no body was retained",
            format!("{prefix}/us-west-2/000000000000/artifact-by-url/1.0.0/template.yaml"),
        ),
        (
            "a license the publisher never supplied",
            format!("{prefix}/us-west-2/000000000000/artifact-present/license.txt"),
        ),
        (
            "an application the catalogue never held",
            format!("{prefix}/us-west-2/000000000000/never-published/1.0.0/template.yaml"),
        ),
        (
            "an application that has since been deleted",
            doomed_url.clone(),
        ),
        (
            "a version the application never published",
            format!("{prefix}/us-west-2/000000000000/artifact-present/9.9.9/template.yaml"),
        ),
        (
            "the catalogue of another region",
            format!("{prefix}/eu-west-1/000000000000/artifact-present/1.0.0/template.yaml"),
        ),
        (
            "the catalogue of another account",
            format!("{prefix}/us-west-2/123456789012/artifact-present/1.0.0/template.yaml"),
        ),
        (
            "a template addressed without the version that published it",
            format!("{prefix}/us-west-2/000000000000/artifact-present/template.yaml"),
        ),
        (
            "a readme addressed under a version, which no publish announces",
            format!("{prefix}/us-west-2/000000000000/artifact-present/1.0.0/readme.md"),
        ),
        (
            "a file name this service never mints",
            format!("{prefix}/us-west-2/000000000000/artifact-present/1.0.0/secrets.env"),
        ),
        (
            "an empty segment",
            format!("{prefix}/us-west-2/000000000000//1.0.0/template.yaml"),
        ),
        (
            "more segments than a locator carries",
            format!("{prefix}/us-west-2/000000000000/artifact-present/1.0.0/deep/template.yaml"),
        ),
    ];

    for (case, url) in &cases {
        let got = repo.follow("GET", url, false).await;
        assert_eq!(
            got.status, 404,
            "case {case:?}: expected 404 for {url:?}, got {} with {:?}",
            got.status, got.body
        );
        assert!(
            !got.body.contains(&body),
            "case {case:?}: a 404 serves no document, got {:?}",
            got.body
        );
    }

    // The refusals changed nothing: the control address still serves its document.
    assert_serves(
        &repo,
        "after the refusals",
        &served,
        &body,
        "application/json",
    )
    .await;

    // A published artifact is a document to download, so only the download methods reach
    // it; anything else is refused without touching what is stored.
    for method in ["PUT", "POST", "DELETE", "PATCH"] {
        let refused = repo.follow(method, &served, false).await;
        assert_eq!(
            refused.status, 405,
            "case {method:?}: a published artifact is served by GET, got {} with {:?}",
            refused.status, refused.body
        );
    }
    assert_serves(
        &repo,
        "after the refused methods",
        &served,
        &body,
        "application/json",
    )
    .await;
}

/// A locator addresses the artefact it NAMES, so one application's document never answers
/// for another's — including when the two are published under names that share a prefix,
/// and when an application is deleted and re-published under the same name.
#[tokio::test]
async fn one_applications_document_is_not_anothers() {
    let repo = Repo::new();
    let host = "127.0.0.1:9297";

    let mut urls: Vec<(String, String)> = Vec::new();
    for name in ["share", "shared", "share-too"] {
        let body = json_template(&format!("Resource{}", name.replace('-', "")));
        let created = publish(
            &repo,
            Some(host),
            json!({
                "name": name,
                "author": "example-author",
                "description": "a demo app",
                "semanticVersion": "1.0.0",
                "templateBody": body
            }),
        )
        .await;
        urls.push((version_template_url(&created), body));
    }
    for (url, body) in &urls {
        assert_serves(&repo, url, url, body, "application/json").await;
    }

    // A re-published application of the same name starts with nothing published at its
    // old address, and then serves its OWN body there.
    let application_id = member(
        &publish(
            &repo,
            Some(host),
            json!({
                "name": "republished",
                "author": "example-author",
                "description": "a demo app",
                "semanticVersion": "1.0.0",
                "templateBody": json_template("First")
            }),
        )
        .await,
        "applicationId",
    );
    let url = format!(
        "https://{host}/serverlessrepo-artifacts/us-west-2/000000000000/republished/1.0.0/\
         template.yaml"
    );
    let deleted = repo.delete_application(&application_id).await;
    assert_eq!(deleted.status, 204, "deleting: got {}", deleted.body);
    let gone = repo.follow("GET", &url, false).await;
    assert_eq!(
        gone.status, 404,
        "a deleted application publishes nothing, got {} with {:?}",
        gone.status, gone.body
    );

    let recreated = repo.create_application(minimal("republished")).await;
    assert_eq!(recreated.status, 201, "re-publishing: got {}", recreated.body);
    let second = json_template("Second");
    let version = repo
        .create_version(
            &member(&recreated, "applicationId"),
            "1.0.0",
            json!({ "templateBody": second }),
        )
        .await;
    assert_eq!(version.status, 201, "publishing: got {}", version.body);
    assert_serves(
        &repo,
        "the re-published application's own body",
        &member(&version, "templateUrl"),
        &second,
        "application/json",
    )
    .await;
}
