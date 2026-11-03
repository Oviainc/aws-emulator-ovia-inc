// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetMicrovmImageVersion`: one version of one MicroVM image,
//! driven over the same router the emulator boots.
//!
//! RULE: the read reports the PERSISTED version record the `imageVersion` names on the
//! image the `imageIdentifier` names — resolved by either form
//! `MicrovmImageIdentifier` accepts — carrying that image's ARN, the record's own build
//! inputs, state, status, reason and instants, and changing nothing a later caller sees;
//! the verdict on the image is the one its own read path takes, decided before anything
//! is looked up, so an identifier this account holds no record of is HTTP 403
//! `AccessDeniedException` and a value that is no identifier of this API is HTTP 400
//! `ValidationException`, while an own-account image that is gone, or a version the
//! resolved image does not hold, is HTTP 404 `ResourceNotFoundException`.
//!
//! Each test states that rule and exercises it over a SET of inputs; the values any one
//! row happens to carry are examples, not the specification. The wire harness
//! (`emulator`, `call`, `image_body`, …) is the one `microvm_lifecycle` owns, and the
//! version listing is the one `image_version_listing` owns.

use serde_json::{json, Value};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::{
    image_not_found_message, image_version_not_found_message, invalid_arn_format_message,
    managed_base_image_arn, next_image_version, ACCESS_DENIED_MESSAGE, FIRST_IMAGE_VERSION,
    MANAGED_BASE_IMAGE_IDS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::image_version_listing::list_versions;
use crate::microvm_image_and_instance_paths::delete_image;
use crate::microvm_lifecycle::{
    as_uri_value, call, emulator, expected_image_arn, image_body, ACCOUNT, PAST_THE_SPAN, REGION,
};

/// Every member the version read shares with the `CreateMicrovmImage` response AND that
/// means the same thing in both: each response is the stored record, so each one must
/// agree. `state`, `updatedAt` and `status` are deliberately absent — on the create's
/// response `state` is the IMAGE's `MicrovmImageState`, while here it is the version's
/// own `MicrovmImageVersionState`.
const SHARED_WITH_THE_CREATE: &[&str] = &[
    "imageArn",
    "imageVersion",
    "baseImageArn",
    "baseImageVersion",
    "buildRoleArn",
    "description",
    "codeArtifact",
    "logging",
    "egressNetworkConnectors",
    "cpuConfigurations",
    "resources",
    "additionalOsCapabilities",
    "hooks",
    "environmentVariables",
    "tags",
    "createdAt",
];

/// Every member the version read shares with the `UpdateMicrovmImage` response AND that
/// means the same thing in both.
///
/// `createdAt` is absent as well as the three [`SHARED_WITH_THE_CREATE`] leaves out: on
/// the update's response it is the IMAGE's creation instant, while here it is the instant
/// the generation the update minted was created. `tags` is absent because
/// `UpdateMicrovmImageResponse` declares none
/// (`smithy_query(query="operation", operation="UpdateMicrovmImage")`).
const SHARED_WITH_THE_UPDATE: &[&str] = &[
    "imageArn",
    "imageVersion",
    "baseImageArn",
    "baseImageVersion",
    "buildRoleArn",
    "description",
    "codeArtifact",
    "logging",
    "egressNetworkConnectors",
    "cpuConfigurations",
    "resources",
    "additionalOsCapabilities",
    "hooks",
    "environmentVariables",
];

/// A build role ARN for an update: `RoleArn`, pattern
/// `^arn:aws[a-z\-]*:iam::[0-9]{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`.
const UPDATE_BUILD_ROLE_ARN: &str = "arn:aws:iam::123456789012:role/microvm-rebuild";

/// `GET /2025-09-09/microvm-images/{imageIdentifier}/versions/{imageVersion}`.
async fn get_version(
    router: &MultiServiceRouter,
    identifier: &str,
    image_version: &str,
) -> (u16, Value) {
    let uri = format!(
        "/2025-09-09/microvm-images/{}/versions/{}",
        as_uri_value(identifier),
        as_uri_value(image_version)
    );
    call(router, "GET", &uri, None).await
}

/// `POST /2025-09-09/microvm-images`, asserting the create, and its response.
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
        "case {name:?}: create expected 201, got {status} {body}"
    );
    body
}

/// `PUT /2025-09-09/microvm-images/{imageIdentifier}`, asserting the update, and its
/// response — the next generation of the image, which is a further version to read back.
async fn update_image(router: &MultiServiceRouter, identifier: &str, extra: Value) -> Value {
    let mut body = json!({
        "baseImageArn": managed_base_image_arn(REGION, MANAGED_BASE_IMAGE_IDS[0]),
        "buildRoleArn": UPDATE_BUILD_ROLE_ARN,
        "codeArtifact": {"uri": "s3://artifacts/app-v2.zip"},
    });
    if let Value::Object(members) = extra {
        let map = body.as_object_mut().expect("the body is an object");
        for (key, value) in members {
            map.insert(key, value);
        }
    }
    let (status, response) = call(
        router,
        "PUT",
        &format!("/2025-09-09/microvm-images/{}", as_uri_value(identifier)),
        Some(body),
    )
    .await;
    assert_eq!(
        status, 200,
        "update {identifier:?}: expected 200, got {status} {response}"
    );
    response
}

/// The listing's row for `image_version`, so a read can be held against the other
/// projection of the same stored record.
async fn listed_version(
    router: &MultiServiceRouter,
    identifier: &str,
    image_version: &str,
    case: &str,
) -> Value {
    let (status, body) = list_versions(router, identifier, "").await;
    assert_eq!(
        status, 200,
        "case {case}: the listing expected 200, got {status} {body}"
    );
    body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: items is a list, got {body}"))
        .iter()
        .find(|item| item["imageVersion"].as_str() == Some(image_version))
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: the listing carries {image_version}, got {body}"))
}

/// The refusal every denied row must carry: the modelled status, code and sentence.
fn assert_denied(status: u16, body: &Value, case: &str) {
    assert_eq!(status, 403, "case {case}: expected 403, got {status} {body}");
    assert_eq!(
        body["__type"].as_str(),
        Some("AccessDeniedException"),
        "case {case}: expected AccessDeniedException, got {body}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(ACCESS_DENIED_MESSAGE),
        "case {case}: expected the modelled denial sentence, got {:?}",
        body["message"]
    );
}

/// The refusal every not-found row must carry: the modelled status, code and message.
fn assert_not_found(status: u16, body: &Value, expected_message: &str, case: &str) {
    assert_eq!(status, 404, "case {case}: expected 404, got {status} {body}");
    assert_eq!(
        body["__type"].as_str(),
        Some("ResourceNotFoundException"),
        "case {case}: expected ResourceNotFoundException, got {body}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(expected_message),
        "case {case}: expected the lookup's own message, got {:?}",
        body["message"]
    );
}

/// RULE: the read is the stored version record, whichever form of the image's identifier
/// addresses it — so every member it shares with the create's own projection agrees with
/// it, the whole answer is the version listing's row for that same version, its state and
/// status are the ones the build clock has reached, and reading it changes nothing a
/// later caller sees.
#[tokio::test]
async fn a_version_read_is_the_stored_record_and_tracks_the_build_clock() {
    let router = emulator();

    // Rows differ in how much of the version the create specified: only the required
    // members, and every optional member a version carries.
    let rows: Vec<(&str, &str, Value)> = vec![
        ("only the required members", "minimal", json!({})),
        (
            "every optional member",
            "specified",
            json!({
                "description": "the service tier",
                "tags": {"team": "platform", "tier": "gold"},
                "environmentVariables": {"MODE": "batch"},
                // `Capability` publishes `ALL` and `Architecture` publishes `ARM_64`
                // (`smithy_query(query="shape", shape="Capability,Architecture")`).
                "additionalOsCapabilities": ["ALL"],
                "cpuConfigurations": [{"architecture": "ARM_64"}],
                "resources": [{"minimumMemoryInMiB": 4096}],
                "logging": {"disabled": {}},
            }),
        ),
    ];

    for (case, name, extra) in rows {
        let created = create_image(&router, name, extra).await;
        let version = created["imageVersion"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create reports its version, got {created}"))
            .to_string();
        assert_eq!(
            version, FIRST_IMAGE_VERSION,
            "case {case:?}: a created image holds the first generation, got {created}"
        );

        // Both forms of the identifier address the same stored record.
        let identifiers = [
            ("the image name", name.to_string()),
            ("the image ARN", expected_image_arn(name)),
        ];
        for (form, identifier) in &identifiers {
            let row = format!("{case:?} addressed by {form}");
            let (status, during) = get_version(&router, identifier, &version).await;
            assert_eq!(
                status, 200,
                "case {row}: expected 200, got {status} {during}"
            );
            for member in SHARED_WITH_THE_CREATE {
                assert_eq!(
                    during[member], created[member],
                    "case {row}: the read's {member} is the stored record's, got {:?} read and \
                     {:?} created",
                    during[member], created[member]
                );
            }
            assert_eq!(
                during["state"].as_str(),
                Some("IN_PROGRESS"),
                "case {row}: the build is still in flight, got {:?}",
                during["state"]
            );
            assert_eq!(
                during["status"].as_str(),
                Some("INACTIVE"),
                "case {row}: a version whose build is in flight launches nothing, got {:?}",
                during["status"]
            );
            assert!(
                during["stateReason"].is_null(),
                "case {row}: no build recorded a reason, so none is reported, got {:?}",
                during["stateReason"]
            );

            // The read and the listing project the SAME record, so the row the listing
            // carries for this version is the answer this read gives.
            assert_eq!(
                during,
                listed_version(&router, identifier, &version, &row).await,
                "case {row}: the read must be the listing's row for the same version"
            );

            // A reporting operation changes nothing a later caller sees.
            let (status, again) = get_version(&router, identifier, &version).await;
            assert_eq!(status, 200, "case {row}: re-read got {status} {again}");
            assert_eq!(
                again, during,
                "case {row}: the read reports state, so reading it twice reports the same, got \
                 {again}"
            );
        }

        // Once the clock has made the build due the SAME version is reported settled.
        tokio::time::sleep(PAST_THE_SPAN).await;
        let (status, settled) = get_version(&router, name, &version).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {settled}"
        );
        assert_eq!(
            settled["imageVersion"], created["imageVersion"],
            "case {case:?}: the build settling mints no further version, got {settled}"
        );
        assert_eq!(
            settled["state"].as_str(),
            Some("SUCCESSFUL"),
            "case {case:?}: the build the clock has made due has succeeded, got {:?}",
            settled["state"]
        );
        assert_eq!(
            settled["status"].as_str(),
            Some("ACTIVE"),
            "case {case:?}: a built version can launch MicroVMs, got {:?}",
            settled["status"]
        );
        assert_eq!(
            settled,
            listed_version(&router, name, &version, case).await,
            "case {case:?}: the settled read must be the listing's row for it"
        );
    }
}

/// RULE: the read answers for the version the request NAMES, so each generation an image
/// holds reads back its own stored configuration and its own status — the newest built
/// generation ACTIVE and the one it replaced INACTIVE.
#[tokio::test]
async fn each_generation_of_an_image_reads_back_its_own_record() {
    let router = emulator();

    let name = "generations";
    let first = create_image(
        &router,
        name,
        json!({"description": "the first generation", "environmentVariables": {"GEN": "1"}}),
    )
    .await;
    tokio::time::sleep(PAST_THE_SPAN).await;

    // PUT semantics: the update mints the next MAJOR generation of the image, holding
    // the configuration the update carried rather than the create's.
    let second = update_image(
        &router,
        name,
        json!({"description": "the second generation", "environmentVariables": {"GEN": "2"}}),
    )
    .await;
    tokio::time::sleep(PAST_THE_SPAN).await;

    let first_version = FIRST_IMAGE_VERSION.to_string();
    let second_version = next_image_version([first_version.as_str()]);
    assert_eq!(
        second["imageVersion"].as_str(),
        Some(second_version.as_str()),
        "the update mints the next generation, got {second}"
    );

    // Rows are the generations the image now holds, each with the response that minted
    // it, the members that response shares with this read, and the status the build clock
    // has left it in.
    let rows: Vec<(&str, &str, &Value, &[&str], &str)> = vec![
        (
            "the generation the create minted",
            &first_version,
            &first,
            SHARED_WITH_THE_CREATE,
            "INACTIVE",
        ),
        (
            "the generation the update minted",
            &second_version,
            &second,
            SHARED_WITH_THE_UPDATE,
            "ACTIVE",
        ),
    ];

    for (case, version, minted, members, expected_status) in rows {
        let (status, read) = get_version(&router, name, version).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {read}"
        );
        for member in members {
            assert_eq!(
                read[member], minted[member],
                "case {case:?}: the read's {member} is the record that generation was minted \
                 with, got {:?} read and {:?} minted",
                read[member], minted[member]
            );
        }
        assert_eq!(
            read["state"].as_str(),
            Some("SUCCESSFUL"),
            "case {case:?}: the clock has made this generation's build due, got {:?}",
            read["state"]
        );
        assert_eq!(
            read["status"].as_str(),
            Some(expected_status),
            "case {case:?}: exactly the newest built generation is ACTIVE, got {:?}",
            read["status"]
        );
    }
}

/// RULE: past the verdict on the image the lookup is an own-account one, so an
/// `imageVersion` the resolved image does not hold is HTTP 404
/// `ResourceNotFoundException` built from that image's ARN and the version the request
/// named, and an own-account image the clock has already removed answers the image
/// lookup's own 404.
#[tokio::test]
async fn a_version_the_image_does_not_hold_is_not_found() {
    let router = emulator();

    let name = "held";
    create_image(&router, name, json!({})).await;
    tokio::time::sleep(PAST_THE_SPAN).await;
    let arn = expected_image_arn(name);

    // Rows differ in HOW the version fails to be one the image holds: a generation never
    // built, the stored generation written another way, a lower generation, a
    // three-component version and a name that is no version of this service at all.
    // `imageVersion` is `@length(1..2048)` with pattern `^[^\s]+$`
    // (`smithy_query(query="operation", operation="GetMicrovmImageVersion")`), so each
    // row is a value the boundary validator admits and the lookup must answer for.
    let rows: Vec<(&str, &str)> = vec![
        ("a generation never built", "2.0"),
        ("the held generation written another way", "1.00"),
        ("a generation below the first", "0.9"),
        ("a three-component version", "1.0.1"),
        ("no version of this service at all", "latest"),
        ("a bare major with no minor", "9"),
    ];

    for (case, image_version) in rows {
        // Both identifier forms resolve the same image, so both answer the same 404 —
        // whose message names the image's ARN however the request addressed it.
        for identifier in [name, arn.as_str()] {
            let row = format!("{case:?} on {identifier:?}");
            let (status, body) = get_version(&router, identifier, image_version).await;
            assert_not_found(
                status,
                &body,
                &image_version_not_found_message(&arn, image_version),
                &row,
            );
        }
    }

    // The version IS served while the image holds it, so the rows above are refused by
    // the lookup and not by a dead path.
    let (status, served) = get_version(&router, name, FIRST_IMAGE_VERSION).await;
    assert_eq!(
        status, 200,
        "the held version is served, got {status} {served}"
    );

    // An image whose asynchronous deletion is in flight still holds its versions; once
    // the clock has removed the record the read reports the absence of the IMAGE, with
    // the identifier the request carried.
    let (status, deleted) = delete_image(&router, name).await;
    assert_eq!(status, 200, "the delete expected 200, got {status} {deleted}");
    let (status, during) = get_version(&router, name, FIRST_IMAGE_VERSION).await;
    assert_eq!(
        status, 200,
        "a delete in flight still holds the version, got {status} {during}"
    );
    assert_eq!(
        during["state"].as_str(),
        Some("SUCCESSFUL"),
        "a delete in flight does not rewrite the version's own state, got {during}"
    );

    tokio::time::sleep(PAST_THE_SPAN).await;
    let (status, gone) = get_version(&router, &arn, FIRST_IMAGE_VERSION).await;
    assert_not_found(
        status,
        &gone,
        &image_not_found_message(&arn),
        "the removed image holds no version",
    );
}

/// RULE: the verdict on the image is decided BEFORE anything is looked up, so a version
/// read on an `imageIdentifier` this account holds no record of — the AWS-owned managed
/// catalogue, another account's, region's, partition's or vendor's namespace, or a name
/// nothing was created under — is refused 403 `AccessDeniedException`, identically
/// whether or not the identifier names a resource; a value that is no identifier this API
/// can address is HTTP 400 `ValidationException`.
#[tokio::test]
async fn a_version_read_outside_this_accounts_own_images_is_denied() {
    let router = emulator();

    // An image this account DOES hold, so the denied rows are denied for the identifier
    // they name rather than for an empty store.
    let held = "held-image";
    create_image(&router, held, json!({})).await;
    tokio::time::sleep(PAST_THE_SPAN).await;

    // Rows differ in WHY the account holds no record of the identifier. The AWS-owned
    // rows name a catalogue entry that unambiguously EXISTS, so the refusal cannot be
    // read as a statement about existence.
    let mut rows: Vec<(String, String)> = MANAGED_BASE_IMAGE_IDS
        .iter()
        .map(|image_id| {
            (
                format!("an AWS-owned image the catalogue publishes {image_id:?}"),
                managed_base_image_arn(REGION, image_id),
            )
        })
        .collect();
    rows.extend([
        (
            "an AWS-owned ARN the catalogue does not publish".to_string(),
            managed_base_image_arn(REGION, "no-such-base-image"),
        ),
        (
            "this account's namespace, a name nothing was created under".to_string(),
            expected_image_arn("never-created"),
        ),
        (
            "this account's namespace in another region, a name it holds".to_string(),
            format!("arn:aws:lambda:eu-central-1:{ACCOUNT}:microvm-image:{held}"),
        ),
        (
            "another account's ARN for a name this one holds".to_string(),
            format!("arn:aws:lambda:{REGION}:210987654321:microvm-image:{held}"),
        ),
        (
            "another partition's ARN for a name this account holds".to_string(),
            format!("arn:aws-us-gov:lambda:{REGION}:{ACCOUNT}:microvm-image:{held}"),
        ),
        (
            "another vendor's ARN".to_string(),
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:microvm-image:{held}"),
        ),
        (
            "another resource type of this account".to_string(),
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{held}"),
        ),
    ]);

    let mut refusals: Vec<(String, Value)> = Vec::new();
    for (case, identifier) in &rows {
        // The version asked for cannot change the verdict: the version a held image
        // carries and one nothing carries are both refused the same way.
        for image_version in [FIRST_IMAGE_VERSION, "9.9"] {
            let row = format!("{case} ({identifier}) version {image_version}");
            let (status, body) = get_version(&router, identifier, image_version).await;
            assert_denied(status, &body, &row);
            refusals.push((row, body));
        }
    }

    // The refusal says NOTHING about existence: every row's answer is byte for byte the
    // same envelope.
    let (first_case, first) = refusals
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("the rows are not empty"));
    for (case, body) in &refusals {
        assert_eq!(
            body, &first,
            "case {case}: the denial must be indistinguishable from {first_case}'s, got {body}"
        );
    }

    // A value that is no identifier this API can address is a request that can never
    // succeed, whatever version accompanies it.
    for identifier in ["never-created", "not-an-identifier"] {
        let (status, body) = get_version(&router, identifier, FIRST_IMAGE_VERSION).await;
        assert_eq!(
            status, 400,
            "case {identifier:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            body["__type"].as_str(),
            Some("ValidationException"),
            "case {identifier:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(invalid_arn_format_message(identifier).as_str()),
            "case {identifier:?}: expected the refusal built from the identifier the request \
             carried, got {:?}",
            body["message"]
        );
    }

    // The positive control: this account's own version is still served through the same
    // path, and the refusals changed nothing a later caller sees.
    let (status, served) = get_version(&router, held, FIRST_IMAGE_VERSION).await;
    assert_eq!(
        status, 200,
        "this account's own version is served, got {status} {served}"
    );
    assert_eq!(
        served["imageArn"].as_str(),
        Some(expected_image_arn(held).as_str()),
        "the read must report the stored record, got {served}"
    );
}
