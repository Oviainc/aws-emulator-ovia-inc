// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a MicroVM image's IDENTITY and its VERSION GENERATIONS.
//!
//! RULES:
//!
//! * every projection of a MicroVM image carries that image's own `id` — a lowercase
//!   8-4-4-4-12 UUID minted once when the image is created, byte-identical in the create
//!   response, in a later read and in the answer to an update, and distinct between two
//!   images;
//! * a build of an image mints the NEXT MAJOR version of it, `<n>.0`: the first build is
//!   `1.0` and each later build takes the next major, never a point revision of the
//!   generation before it; and
//! * the version listing serves the versions NEWEST FIRST, so the row a build just
//!   produced — carrying the configuration that build was given — is the first one,
//!   however the pages are cut.
//!
//! Each test states that rule and exercises it over a SET of inputs; the values any one
//! row happens to carry are examples, not the specification. The wire harness
//! (`emulator`, `call`, `image_body`, …) is the one `microvm_lifecycle` owns, the image
//! read is the one `microvm_image_and_instance_paths` owns, and the version listing is
//! the one `image_version_listing` owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::image_version_listing::list_versions;
use crate::microvm_image_and_instance_paths::get_image;
use crate::microvm_lifecycle::{
    as_uri_value, call, emulator, expected_image_arn, image_body, PAST_THE_SPAN,
};

/// A build role ARN: `RoleArn`, pattern
/// `^arn:aws[a-z\-]*:iam::[0-9]{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`.
const BUILD_ROLE_ARN: &str = "arn:aws:iam::123456789012:role/microvm-generation";

/// The AWS-managed base image every build here builds on, in the ARN form an AWS-owned
/// resource of this service carries — the same one `microvm_lifecycle`'s creates use.
const BASE_IMAGE_ARN: &str = "arn:aws:lambda:us-west-2:aws:microvm-image:al2023-1";

/// The group lengths of the UUID form `id` reports: 8-4-4-4-12.
const UUID_GROUP_LENGTHS: &[usize] = &[8, 4, 4, 4, 12];

/// Whether `value` is a lowercase 8-4-4-4-12 UUID, which is the form a MicroVM image's
/// `id` carries.
fn is_uuid_shaped(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == UUID_GROUP_LENGTHS.len()
        && groups
            .iter()
            .zip(UUID_GROUP_LENGTHS)
            .all(|(group, length)| {
                group.len() == *length
                    && group
                        .chars()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            })
}

/// `POST /2025-09-09/microvm-images`, asserting the create and handing back its body.
async fn create_image(router: &MultiServiceRouter, name: &str, extra: Value) -> Value {
    let (status, body) = call(
        router,
        "POST",
        "/2025-09-09/microvm-images",
        Some(image_body(name, extra)),
    )
    .await;
    assert_eq!(
        status, 201,
        "create {name:?}: expected 201, got {status} {body}"
    );
    body
}

/// `PUT /2025-09-09/microvm-images/{imageIdentifier}`: the three members
/// `UpdateMicrovmImage` requires plus whatever `extra` adds.
async fn update_image(
    router: &MultiServiceRouter,
    identifier: &str,
    extra: Value,
) -> (u16, Value) {
    let mut body = json!({
        "baseImageArn": BASE_IMAGE_ARN,
        "buildRoleArn": BUILD_ROLE_ARN,
        "codeArtifact": {"uri": "s3://artifacts/app.zip"},
    });
    if let Value::Object(members) = extra {
        let map = body.as_object_mut().expect("the body is an object");
        for (key, value) in members {
            map.insert(key, value);
        }
    }
    call(
        router,
        "PUT",
        &format!("/2025-09-09/microvm-images/{}", as_uri_value(identifier)),
        Some(body),
    )
    .await
}

/// The `imageVersion`s one page of the version listing carries, in the order it carries
/// them.
fn listed_versions(body: &Value) -> Vec<String> {
    body["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            item["imageVersion"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// RULE: every projection of a MicroVM image reports that image's own `id` — a lowercase
/// 8-4-4-4-12 UUID — and the value is the image's identity: the create response, a later
/// read and the answer to an update all report the one value, and two images never share
/// one.
#[tokio::test]
async fn every_image_projection_reports_the_images_own_uuid_id() {
    let router = emulator();

    // Rows differ in the name's form and in how much of the configuration the create
    // carried, which is what the identity generalises over: nothing of the request
    // decides it.
    let rows: Vec<(&str, &str, Value)> = vec![
        ("only the required members", "identity-a", json!({})),
        (
            "underscores and digits in the name",
            "identity_b-2",
            json!({"description": "the second image"}),
        ),
        (
            "every optional member the create takes",
            "identity-c",
            json!({
                "description": "fully specified",
                "tags": {"team": "platform"},
                "environmentVariables": {"MODE": "batch"},
                "additionalOsCapabilities": ["ALL"],
                "cpuConfigurations": [{"architecture": "ARM_64"}],
                "resources": [{"minimumMemoryInMiB": 4096}],
            }),
        ),
    ];

    let mut minted: Vec<(&str, String)> = Vec::new();
    for (case, name, extra) in rows {
        let created = create_image(&router, name, extra).await;
        let id = created["id"].as_str().unwrap_or_default().to_string();
        assert!(
            is_uuid_shaped(&id),
            "case {case:?}: the create reports a lowercase 8-4-4-4-12 UUID id, got {:?}",
            created["id"]
        );

        // The same record, read back by each form `MicrovmImageIdentifier` accepts.
        for (form, identifier) in [
            ("the image name", name.to_string()),
            ("the image ARN", expected_image_arn(name)),
        ] {
            let (status, read) = get_image(&router, &identifier).await;
            assert_eq!(
                status, 200,
                "case {case:?}: read by {form} got {status} {read}"
            );
            assert_eq!(
                read["id"].as_str(),
                Some(id.as_str()),
                "case {case:?}: the read by {form} reports the created image's id, got {:?}",
                read["id"]
            );
        }

        // An update mints a further version of the SAME image, so the identity is
        // unchanged by it — before and after the build the update triggered settles.
        tokio::time::sleep(PAST_THE_SPAN).await;
        let (status, updated) = update_image(&router, name, json!({"description": "rev 2"})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the update got {status} {updated}"
        );
        assert_eq!(
            updated["id"].as_str(),
            Some(id.as_str()),
            "case {case:?}: the update reports the same id as the create, got {:?}",
            updated["id"]
        );
        tokio::time::sleep(PAST_THE_SPAN).await;
        let (status, settled) = get_image(&router, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the settled read got {status} {settled}"
        );
        assert_eq!(
            settled["id"].as_str(),
            Some(id.as_str()),
            "case {case:?}: the id survives the build the update triggered, got {:?}",
            settled["id"]
        );

        minted.push((case, id));
    }

    for (index, (case, id)) in minted.iter().enumerate() {
        for (other_case, other) in minted.iter().skip(index + 1) {
            assert_ne!(
                id, other,
                "case {case:?} and case {other_case:?}: each image carries an identity of its \
                 own, got {id} twice"
            );
        }
    }
}

/// RULE: `baseImageVersion` is the caller-chosen version of the base image, so a create
/// that names one carries THAT value into the version record it persists — the create
/// response and the stored version both report it unchanged, whatever form the value
/// takes — and only a create that names none is answered with the version the named base
/// image publishes.
#[tokio::test]
async fn a_requested_base_image_version_is_carried_into_the_stored_version() {
    let router = emulator();

    // `baseImageVersion` targets `Version` (`@length(1..2048)`, `@pattern ^[^\s]+$` —
    // `smithy_query(query="shape",shape="CreateMicrovmImageResponse")`), so the rows span
    // the forms that constraint admits: this service's own `major.minor` form, a bare
    // generation counter, a long dotted version, and the length bounds the member
    // publishes.
    let rows: Vec<(&str, &str, String)> = vec![
        ("the next major of the base image", "chosen-major", "2.0".to_string()),
        ("a bare generation counter", "chosen-generation", "1".to_string()),
        ("a three-part version", "chosen-dotted", "4.12.9".to_string()),
        ("one character, the published minimum", "chosen-shortest", "7".to_string()),
        (
            "the published maximum length",
            "chosen-longest",
            "v".repeat(2048),
        ),
    ];

    for (case, name, requested) in rows {
        let created = create_image(
            &router,
            name,
            json!({"baseImageVersion": requested.clone()}),
        )
        .await;
        assert_eq!(
            created["baseImageVersion"].as_str(),
            Some(requested.as_str()),
            "case {case:?}: the create reports the version the request selected, got {:?}",
            created["baseImageVersion"]
        );

        // The response is the stored record, so the version the build was given reports
        // the same selection.
        let (status, listed) = list_versions(&router, name, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: the version listing got {status} {listed}"
        );
        let items = listed["items"].as_array().cloned().unwrap_or_default();
        let first = items.first().cloned().unwrap_or(Value::Null);
        assert_eq!(
            first["baseImageVersion"].as_str(),
            Some(requested.as_str()),
            "case {case:?}: the stored version carries the selection, got {first}"
        );
    }

    // A create that names NONE still reports the version the service resolved for it —
    // the member is never left unset on an accepted create — and the stored version
    // record reports that same value. WHICH version an omitted member resolves to is the
    // AWS-managed catalogue's own content, so it is not pinned here.
    let defaulted = create_image(&router, "unchosen", json!({})).await;
    let resolved = defaulted["baseImageVersion"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !resolved.is_empty(),
        "a create naming no base version is still answered with the version the service \
         resolved, got {:?}",
        defaulted["baseImageVersion"]
    );
    let (status, listed) = list_versions(&router, "unchosen", "").await;
    assert_eq!(
        status, 200,
        "the version listing of the defaulted image got {status} {listed}"
    );
    let items = listed["items"].as_array().cloned().unwrap_or_default();
    let first = items.first().cloned().unwrap_or(Value::Null);
    assert_eq!(
        first["baseImageVersion"].as_str(),
        Some(resolved.as_str()),
        "the stored version carries the resolved selection, got {first}"
    );
}

/// RULE: a build of a MicroVM image mints the NEXT MAJOR version of it in `<n>.0` form —
/// the first build is `1.0` and each later build takes the next major, never a point
/// revision — and that version is the one the image reports as active once the build
/// settles.
#[tokio::test]
async fn each_build_of_an_image_mints_the_next_major_version() {
    let router = emulator();
    let name = "generations";
    let created = create_image(&router, name, json!({"description": "rev 1"})).await;
    assert_eq!(
        created["imageVersion"].as_str(),
        Some("1.0"),
        "the first build is the image's first generation, got {:?}",
        created["imageVersion"]
    );

    // Rows differ in how many builds the image has already taken, which is what the
    // minting generalises over.
    let expected_versions = ["2.0", "3.0", "4.0"];
    for (round, expected) in expected_versions.iter().enumerate() {
        let case = format!("build {} of the image", round + 2);
        tokio::time::sleep(PAST_THE_SPAN).await;
        let (status, updated) =
            update_image(&router, name, json!({"description": format!("rev {}", round + 2)})).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} {updated}"
        );
        assert_eq!(
            updated["imageVersion"].as_str(),
            Some(*expected),
            "case {case}: expected the next major {expected}, got {:?}",
            updated["imageVersion"]
        );

        tokio::time::sleep(PAST_THE_SPAN).await;
        let (status, settled) = get_image(&router, name).await;
        assert_eq!(
            status, 200,
            "case {case}: reading the settled image got {status} {settled}"
        );
        assert_eq!(
            settled["latestActiveImageVersion"].as_str(),
            Some(*expected),
            "case {case}: the version the build produced is the active one, got {settled}"
        );
    }

    // Every generation stays addressable, so nothing a build minted was replaced.
    let (status, body) = list_versions(&router, name, "").await;
    assert_eq!(status, 200, "listing the versions got {status} {body}");
    let mut held = listed_versions(&body);
    held.sort();
    let mut all: Vec<String> = expected_versions.iter().map(|v| (*v).to_string()).collect();
    all.push("1.0".to_string());
    all.sort();
    assert_eq!(
        held, all,
        "every generation the builds minted is still held, got {body}"
    );
}

/// RULE: the version listing serves the versions NEWEST FIRST, so the first row is the
/// version the last build minted and it carries the configuration THAT build was given;
/// the order holds however the pages are cut.
#[tokio::test]
async fn the_version_listing_serves_the_newest_generation_first() {
    let router = emulator();
    let name = "ordered";
    create_image(
        &router,
        name,
        json!({"description": "rev 1", "codeArtifact": {"uri": "s3://artifacts/code.zip"}}),
    )
    .await;

    // Rows differ in the configuration each build carries, so the newest row can be told
    // from the one before it by its own members and not only by its version.
    let builds: Vec<(&str, &str, &str)> = vec![
        ("the first build", "2.0", "s3://artifacts/code2.zip"),
        ("the second build", "3.0", "s3://artifacts/code3.zip"),
    ];

    let mut newest_first: Vec<String> = vec!["1.0".to_string()];
    for (case, version, uri) in &builds {
        tokio::time::sleep(PAST_THE_SPAN).await;
        let (status, updated) = update_image(
            &router,
            name,
            json!({"description": format!("rev {version}"), "codeArtifact": {"uri": uri}}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the update got {status} {updated}"
        );
        newest_first.insert(0, (*version).to_string());

        let (status, body) = list_versions(&router, name, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: the listing got {status} {body}"
        );
        assert_eq!(
            listed_versions(&body),
            newest_first,
            "case {case:?}: expected the versions newest first, got {body}"
        );

        let items = body["items"].as_array().cloned().unwrap_or_default();
        let first = items.first().cloned().unwrap_or(Value::Null);
        assert_eq!(
            first["imageVersion"].as_str(),
            Some(*version),
            "case {case:?}: the first row is the version this build minted, got {first}"
        );
        assert_eq!(
            first["description"].as_str(),
            Some(format!("rev {version}").as_str()),
            "case {case:?}: the first row carries this build's description, got {first}"
        );
        assert_eq!(
            first["codeArtifact"],
            json!({"uri": uri}),
            "case {case:?}: the first row carries this build's code artifact, got {first}"
        );
    }

    // However the pages are cut, the walk serves the same order: rows differ in the page
    // size, from one row per page up to more than the image holds.
    for size in [1usize, 2, 3, 50] {
        let case = format!("maxResults={size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..=newest_first.len() {
            let mut query = format!("maxResults={size}");
            if let Some(token) = &token {
                query.push_str(&format!("&nextToken={token}"));
            }
            let (status, body) = list_versions(&router, name, &query).await;
            assert_eq!(
                status, 200,
                "case {case}: the page got {status} {body}"
            );
            seen.extend(listed_versions(&body));
            token = body["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, newest_first,
            "case {case}: every page keeps the newest-first order, got {seen:?}"
        );
    }
}
