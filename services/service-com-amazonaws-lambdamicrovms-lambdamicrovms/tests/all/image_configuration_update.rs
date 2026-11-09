// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateMicrovmImage`: re-configuring a MicroVM image this
//! account holds.
//!
//! RULE: the update carries the WHOLE configuration (PUT semantics), so it mints a
//! further version of the image the `imageIdentifier` names — the next MAJOR, `<n>.0`,
//! after the generations it holds — holding exactly the members the request carried plus
//! the values the service supplies for the ones it left out; the image is reported in the
//! progressive `UPDATING` state with that version's build in flight and reaches
//! `UPDATED`, with the new version as its active one, once the clock has made the build
//! due. An image whose own lifecycle work is still in flight has no settled record to
//! build the next version from, so the update is refused with HTTP 400
//! `ValidationException`; a request whose build inputs cannot resolve is refused the
//! same way; a well-formed identifier naming no image of this account is HTTP 404, a
//! value that is no identifier of this API is HTTP 400, and an identifier of the
//! AWS-owned namespace is HTTP 403 before any lookup.
//!
//! Each test states that rule and exercises it over a SET of inputs; the values any one
//! row happens to carry are examples, not the specification. The wire harness
//! (`emulator`, `call`, `image_body`, …) is the one `microvm_lifecycle` owns, and the
//! version listing is the read-back path `image_version_listing` owns.

use serde_json::{json, Value};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::{
    base_image_not_found_message, default_log_group, image_not_found_message,
    image_not_updatable_message, invalid_arn_format_message, managed_base_image_arn,
    managed_network_connector_arn, next_image_version, service_state,
    too_many_image_versions_message, access_denied_message, CODE_ARTIFACT_URI_REQUIREMENT,
    DEFAULT_CPU_ARCHITECTURE, DEFAULT_EGRESS_NETWORK_CONNECTOR, DEFAULT_MINIMUM_MEMORY_IN_MIB,
    FIRST_IMAGE_VERSION, MANAGED_BASE_IMAGE_IDS, MAX_MICROVM_IMAGE_VERSIONS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::image_version_listing::list_versions;
use crate::microvm_image_and_instance_paths::{delete_image, get_image};
use crate::microvm_lifecycle::{
    as_uri_value, call, emulator, emulator_with_engine, expected_image_arn, image_body, ACCOUNT,
    PAST_THE_SPAN, REGION,
};

/// A build role ARN: `RoleArn`, pattern
/// `^arn:aws[a-z\-]*:iam::[0-9]{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`.
const BUILD_ROLE_ARN: &str = "arn:aws:iam::123456789012:role/microvm-rebuild";

/// A second build role ARN of the same `RoleArn` shape, so a row can change the role
/// the update names.
const OTHER_BUILD_ROLE_ARN: &str = "arn:aws:iam::123456789012:role/microvm-rebuild-2";

/// The code artifact an update points at: `CodeArtifact$uri` is "The URI of the code
/// artifact in Amazon S3", so it is an S3 object URI.
const CODE_ARTIFACT_URI: &str = "s3://artifacts/app-v2.zip";

/// Every member of an `UpdateMicrovmImage` request that is also a member of its
/// response. PUT semantics: each one the request carried comes back unchanged, and each
/// one it left out comes back carrying the value the service supplies
/// ([`service_supplied`]).
const CONFIGURATION_MEMBERS: &[&str] = &[
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

/// The AWS-managed base image every build here builds on: the one this service
/// publishes (`state::MANAGED_BASE_IMAGE_IDS`), in the region the harness signs for.
fn base_image_arn() -> String {
    managed_base_image_arn(REGION, MANAGED_BASE_IMAGE_IDS[0])
}

/// `PUT /2025-09-09/microvm-images/{imageIdentifier}`.
async fn update_image(
    router: &MultiServiceRouter,
    identifier: &str,
    body: Value,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/2025-09-09/microvm-images/{}", as_uri_value(identifier)),
        Some(body),
    )
    .await
}

/// Merge the members of `extra` into `body`, overriding what is already there.
fn merge(body: &mut Value, extra: Value) {
    if let Value::Object(members) = extra {
        let map = body.as_object_mut().expect("the body is an object");
        for (key, value) in members {
            map.insert(key, value);
        }
    }
}

/// An `UpdateMicrovmImage` body: the three members the operation requires
/// (`codeArtifact`, `baseImageArn`, `buildRoleArn`), plus whatever `extra` adds.
fn update_body(extra: Value) -> Value {
    let mut body = json!({
        "baseImageArn": base_image_arn(),
        "buildRoleArn": BUILD_ROLE_ARN,
        "codeArtifact": {"uri": CODE_ARTIFACT_URI},
    });
    merge(&mut body, extra);
    body
}

/// Create the image `name` (with whatever `extra` adds to the create body) and wait out
/// its asynchronous build, so an update addressed at it starts from a settled record.
/// Hands back the create's own response.
async fn built_image_with(router: &MultiServiceRouter, name: &str, extra: Value) -> Value {
    let created = create_image(router, name, extra).await;
    tokio::time::sleep(PAST_THE_SPAN).await;
    created
}

/// Create the image `name`, asserting the create, and hand back its response.
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

/// The value the service supplies for the configuration member `member` of an image
/// named `name` when the request leaves it out.
///
/// The members with a service-supplied value are the ones whose documentation says the
/// accepted configuration carries one (`baseImageVersion`, `egressNetworkConnectors`,
/// `cpuConfigurations`, `resources`, `logging`); every other member is simply absent
/// from a record that never named it.
fn service_supplied(name: &str, member: &str) -> Value {
    match member {
        // A build that pins no `baseImageVersion` records the `major.minor` INITIAL
        // version, whatever generation the AWS-managed catalogue publishes for the base
        // image (`state::resolve_base_image_version`).
        "baseImageVersion" => json!(FIRST_IMAGE_VERSION),
        "egressNetworkConnectors" => json!([managed_network_connector_arn(
            REGION,
            DEFAULT_EGRESS_NETWORK_CONNECTOR
        )]),
        "cpuConfigurations" => json!([{"architecture": DEFAULT_CPU_ARCHITECTURE}]),
        "resources" => json!([{"minimumMemoryInMiB": DEFAULT_MINIMUM_MEMORY_IN_MIB}]),
        "logging" => json!({"cloudWatch": {"logGroup": default_log_group(name)}}),
        _ => Value::Null,
    }
}

/// The listed entry for the version `image_version` of the image `identifier` holds.
async fn version_entry(
    router: &MultiServiceRouter,
    identifier: &str,
    image_version: &str,
    case: &str,
) -> Value {
    let (status, body) = list_versions(router, identifier, "").await;
    assert_eq!(
        status, 200,
        "case {case}: listing the versions of {identifier:?} got {status} {body}"
    );
    body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: items is a list, got {body}"))
        .iter()
        .find(|item| item["imageVersion"] == json!(image_version))
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: no version {image_version} listed, got {body}"))
}

/// How many versions the image `identifier` holds.
async fn version_count(router: &MultiServiceRouter, identifier: &str, case: &str) -> usize {
    let (status, body) = list_versions(router, identifier, "").await;
    assert_eq!(
        status, 200,
        "case {case}: listing the versions of {identifier:?} got {status} {body}"
    );
    body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: items is a list, got {body}"))
        .len()
}

/// RULE: the update mints a FURTHER version of the named image holding exactly the
/// configuration the request carried — PUT semantics, so a member the request leaves
/// out is absent from the new version (or carries the value the service supplies for
/// it) rather than inherited from the version before — the image is reported `UPDATING`
/// with the version it was already serving still active, and the response is the
/// persisted record, so the listing and the image read back report the same.
#[tokio::test]
async fn an_update_mints_a_further_version_holding_the_configuration_it_carried() {
    let router = emulator();

    // Rows differ in how much of the configuration the update names (none of the
    // optional members, every one of them), in which members the version BEFORE it
    // carried — so an inherited value would show — and in which form
    // `MicrovmImageIdentifier` addresses the image.
    let rows: Vec<(&str, &str, bool, Value, Value)> = vec![
        (
            "only the required members, over a fully specified version, addressed by name",
            "updated-minimal",
            false,
            json!({
                "description": "the version before",
                "environmentVariables": {"MODE": "batch"},
                "additionalOsCapabilities": ["ALL"],
                "resources": [{"minimumMemoryInMiB": 4096}],
                "logging": {"disabled": {}},
            }),
            json!({}),
        ),
        (
            "every optional member, over a minimal version, addressed by ARN",
            "updated-full",
            true,
            json!({}),
            json!({
                "buildRoleArn": OTHER_BUILD_ROLE_ARN,
                "baseImageVersion": "1.0",
                "description": "the updated tier",
                // `Logging` is a union of `cloudWatch` and `disabled`.
                "logging": {"cloudWatch": {"logGroup": "/aws/microvm/updated"}},
                "egressNetworkConnectors": [managed_network_connector_arn(
                    REGION,
                    DEFAULT_EGRESS_NETWORK_CONNECTOR,
                )],
                // `Architecture` publishes `ARM_64` and `Capability` publishes `ALL`
                // (`smithy_query(query="shape",shape="Architecture,Capability")`).
                "cpuConfigurations": [{"architecture": "ARM_64"}],
                "resources": [{"minimumMemoryInMiB": 8192}],
                "additionalOsCapabilities": ["ALL"],
                // `Hooks$port` is `@range(min: 1, max: 65535)`.
                "hooks": {"port": 8080},
                "environmentVariables": {"MODE": "interactive", "TIER": "gold"},
            }),
        ),
        (
            "the logging union's other arm, addressed by name",
            "updated-logging-off",
            false,
            json!({"logging": {"cloudWatch": {"logGroup": "/aws/microvm/before"}}}),
            json!({"logging": {"disabled": {}}}),
        ),
    ];

    for (case, name, by_arn, create_extra, update_extra) in rows {
        let created = built_image_with(&router, name, create_extra).await;
        let identifier = match by_arn {
            true => expected_image_arn(name),
            false => name.to_string(),
        };

        let request = update_body(update_extra);
        let (status, updated) = update_image(&router, &identifier, request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {updated}"
        );

        // The SAME image record: its ARN, its name and its creation instant are the
        // ones the create minted, whichever form addressed it.
        assert_eq!(
            updated["imageArn"].as_str(),
            Some(expected_image_arn(name).as_str()),
            "case {case:?}: the update answers for the image the identifier names, got {updated}"
        );
        assert_eq!(
            updated["name"].as_str(),
            Some(name),
            "case {case:?}: the image keeps its name, got {updated}"
        );
        assert_eq!(
            updated["createdAt"], created["createdAt"],
            "case {case:?}: an update does not re-create the image, got {:?}",
            updated["createdAt"]
        );

        // A FURTHER version, with the image's lifecycle in flight and the version it
        // was already serving still the active one.
        assert_eq!(
            updated["imageVersion"].as_str(),
            Some("2.0"),
            "case {case:?}: the update mints the next major after {:?}, got {:?}",
            created["imageVersion"],
            updated["imageVersion"]
        );
        assert_eq!(
            updated["state"].as_str(),
            Some("UPDATING"),
            "case {case:?}: the update's build is in flight, got {:?}",
            updated["state"]
        );
        assert_eq!(
            updated["latestActiveImageVersion"], created["imageVersion"],
            "case {case:?}: the version the image was serving stays active while the update's \
             build is in flight, got {:?}",
            updated["latestActiveImageVersion"]
        );
        assert!(
            updated["updatedAt"].is_number(),
            "case {case:?}: the update is a write, so the record carries its instant, got {:?}",
            updated["updatedAt"]
        );

        // PUT semantics: every configuration member is the request's own, or the value
        // the service supplies for a member the request left out — never the previous
        // version's.
        for member in CONFIGURATION_MEMBERS {
            let expected = request
                .get(*member)
                .cloned()
                .unwrap_or_else(|| service_supplied(name, member));
            assert_eq!(
                updated[*member], expected,
                "case {case:?}: member {member} expected {expected}, got {:?}",
                updated[*member]
            );
        }

        // The response is the PERSISTED record: the image reads back the same, and the
        // new version is listed in flight beside the one it will replace.
        let (status, image) = get_image(&router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading the image got {status} {image}"
        );
        assert_eq!(
            image["state"], updated["state"],
            "case {case:?}: the image reads back in the state the update reported, got {image}"
        );
        assert_eq!(
            image["updatedAt"], updated["updatedAt"],
            "case {case:?}: the image reads back the update's own instant, got {image}"
        );
        assert_eq!(
            image["latestActiveImageVersion"], created["imageVersion"],
            "case {case:?}: the active version is unchanged until the build settles, got {image}"
        );

        let entry = version_entry(&router, &identifier, "2.0", case).await;
        assert_eq!(
            entry["state"].as_str(),
            Some("IN_PROGRESS"),
            "case {case:?}: the new version's build is in flight, got {entry}"
        );
        assert_eq!(
            entry["status"].as_str(),
            Some("INACTIVE"),
            "case {case:?}: a version whose build is in flight launches nothing, got {entry}"
        );
        for member in CONFIGURATION_MEMBERS {
            assert_eq!(
                entry[*member], updated[*member],
                "case {case:?}: the listed version is the stored record, so its {member} is the \
                 update's, got {:?}",
                entry[*member]
            );
        }

        // Once the clock has made the build due the image settles into the progressive
        // state's own terminal value and the new version becomes the active one.
        tokio::time::sleep(PAST_THE_SPAN).await;
        let (status, settled) = get_image(&router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading the settled image got {status} {settled}"
        );
        assert_eq!(
            settled["state"].as_str(),
            Some("UPDATED"),
            "case {case:?}: the update's build has been made due, got {settled}"
        );
        assert_eq!(
            settled["latestActiveImageVersion"].as_str(),
            Some("2.0"),
            "case {case:?}: the version the update built is the active one, got {settled}"
        );

        let entry = version_entry(&router, &identifier, "2.0", case).await;
        assert_eq!(
            entry["state"].as_str(),
            Some("SUCCESSFUL"),
            "case {case:?}: the build the clock has made due has succeeded, got {entry}"
        );
        assert_eq!(
            entry["status"].as_str(),
            Some("ACTIVE"),
            "case {case:?}: the built version can launch MicroVMs, got {entry}"
        );
        let previous = version_entry(&router, &identifier, "1.0", case).await;
        assert_eq!(
            previous["status"].as_str(),
            Some("INACTIVE"),
            "case {case:?}: exactly one version is active, so the previous one steps down, got \
             {previous}"
        );
    }
}

/// RULE: each update mints the next MAJOR version after the generations the image
/// already holds, so repeated updates accumulate versions on the one image record rather
/// than replacing one.
#[tokio::test]
async fn successive_updates_mint_successive_versions() {
    let router = emulator();
    let name = "versioned";
    built_image_with(&router, name, json!({})).await;

    // Rows differ in how many updates the image has already taken, which is what the
    // minting generalises over.
    for (round, expected) in ["2.0", "3.0", "4.0"].iter().enumerate() {
        let case = format!("update {} of the image", round + 1);
        let (status, updated) = update_image(
            &router,
            name,
            update_body(json!({"description": format!("round {round}")})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} {updated}"
        );
        assert_eq!(
            updated["imageVersion"].as_str(),
            Some(*expected),
            "case {case}: expected version {expected}, got {:?}",
            updated["imageVersion"]
        );

        tokio::time::sleep(PAST_THE_SPAN).await;
        let (status, image) = get_image(&router, name).await;
        assert_eq!(status, 200, "case {case}: reading got {status} {image}");
        assert_eq!(
            image["latestActiveImageVersion"].as_str(),
            Some(*expected),
            "case {case}: the newest built version is the active one, got {image}"
        );
        assert_eq!(
            version_count(&router, name, &case).await,
            round + 2,
            "case {case}: every version the image has built is still held"
        );
    }
}

/// RULE: an image whose own lifecycle work is still in flight — every progressive
/// `MicrovmImageState` — is no settled record to build the next version from, so the
/// update is refused with HTTP 400 `ValidationException` naming the image's own ARN,
/// whichever form `MicrovmImageIdentifier` addressed it by.
#[tokio::test]
async fn an_update_of_an_image_whose_lifecycle_work_is_in_flight_is_refused() {
    let router = emulator();

    // The create's own build is in flight.
    let creating = "in-flight-creating";
    let created = create_image(&router, creating, json!({})).await;
    assert_eq!(
        created["state"].as_str(),
        Some("CREATING"),
        "the create's build is in flight, got {created}"
    );
    assert_update_refused_in_flight(&router, creating, "CREATING").await;

    // An update's build is in flight: the image took one update and is UPDATING.
    let updating = "in-flight-updating";
    built_image_with(&router, updating, json!({})).await;
    let (status, accepted) = update_image(&router, updating, update_body(json!({}))).await;
    assert_eq!(
        status, 200,
        "the first update must be accepted, got {status} {accepted}"
    );
    assert_eq!(
        accepted["state"].as_str(),
        Some("UPDATING"),
        "the accepted update's build is in flight, got {accepted}"
    );
    assert_update_refused_in_flight(&router, updating, "UPDATING").await;

    // A deletion is in flight: the record is still there, and still not updatable.
    let deleting = "in-flight-deleting";
    built_image_with(&router, deleting, json!({})).await;
    let (status, deleted) = delete_image(&router, deleting).await;
    assert_eq!(status, 200, "the delete got {status} {deleted}");
    assert_eq!(
        deleted["state"].as_str(),
        Some("DELETING"),
        "the deletion is in flight, got {deleted}"
    );
    assert_update_refused_in_flight(&router, deleting, "DELETING").await;
}

/// Assert that both forms of the image `name`'s identifier are refused the update,
/// because the image is in the progressive state `state`.
async fn assert_update_refused_in_flight(
    router: &MultiServiceRouter,
    name: &str,
    state: &str,
) {
    let image_arn = expected_image_arn(name);
    for identifier in [name.to_string(), image_arn.clone()] {
        let case = format!("{state} addressed by {identifier:?}");
        let (status, body) = update_image(router, &identifier, update_body(json!({}))).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {body}"
        );
        assert_eq!(
            body["__type"].as_str(),
            Some("ValidationException"),
            "case {case}: expected ValidationException, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(image_not_updatable_message(&image_arn).as_str()),
            "case {case}: the refusal names the image's own ARN, got {:?}",
            body["message"]
        );
    }
}

/// RULE: the identifier is resolved against the images THIS ACCOUNT holds, by either
/// form `MicrovmImageIdentifier` accepts: an identifier of the AWS-OWNED namespace is
/// HTTP 403 before any lookup, a well-formed identifier of this account's namespace
/// naming no image of it — one the clock has already removed included — is HTTP 404
/// `ResourceNotFoundException`, and a value that is no identifier of this API at all is
/// HTTP 400 `ValidationException`.
#[tokio::test]
async fn an_identifier_naming_no_image_of_this_account_is_refused() {
    let router = emulator();
    built_image_with(&router, "held", json!({})).await;

    let never_created = expected_image_arn("never-created");
    let other_region = format!("arn:aws:lambda:eu-central-1:{ACCOUNT}:microvm-image:held");
    let other_account = format!("arn:aws:lambda:{REGION}:111122223333:microvm-image:held");
    let managed = base_image_arn();

    // Rows differ in HOW the identifier fails to name an image this account holds.
    let rows: Vec<(&str, String, u16, &str, String)> = vec![
        (
            "the AWS-owned namespace",
            managed.clone(),
            403,
            "AccessDeniedException",
            access_denied_message(),
        ),
        (
            "an ARN for a name never created",
            never_created.clone(),
            404,
            "ResourceNotFoundException",
            image_not_found_message(&never_created),
        ),
        (
            "another region's ARN for a held name",
            other_region.clone(),
            404,
            "ResourceNotFoundException",
            image_not_found_message(&other_region),
        ),
        (
            "another account's ARN for a held name",
            other_account.clone(),
            404,
            "ResourceNotFoundException",
            image_not_found_message(&other_account),
        ),
        (
            "a name no image of this account carries",
            "never-created".to_string(),
            400,
            "ValidationException",
            invalid_arn_format_message("never-created"),
        ),
        (
            "no identifier of this API at all",
            "not-an-identifier".to_string(),
            400,
            "ValidationException",
            invalid_arn_format_message("not-an-identifier"),
        ),
    ];

    for (case, identifier, expected_status, expected_type, expected_message) in rows {
        let (status, body) = update_image(&router, &identifier, update_body(json!({}))).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: {identifier:?} expected {expected_status}, got {status} {body}"
        );
        assert_eq!(
            body["__type"].as_str(),
            Some(expected_type),
            "case {case:?}: expected {expected_type}, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(expected_message.as_str()),
            "case {case:?}: expected the refusal built from the identifier the request carried, \
             got {:?}",
            body["message"]
        );
    }

    // An image the clock has removed answers the lookup's own 404, not the refusal a
    // deletion in flight answers.
    let (status, deleted) = delete_image(&router, "held").await;
    assert_eq!(status, 200, "the delete got {status} {deleted}");
    tokio::time::sleep(PAST_THE_SPAN).await;
    let held_arn = expected_image_arn("held");
    let (status, body) = update_image(&router, &held_arn, update_body(json!({}))).await;
    assert_eq!(
        status, 404,
        "a removed image is no record to update, got {status} {body}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(image_not_found_message(&held_arn).as_str()),
        "the removed image answers the lookup's own 404, got {body}"
    );
}

/// RULE: the build inputs the request carries are resolved before anything is written,
/// so a `codeArtifact` that addresses no S3 object and a `baseImageArn` that names
/// nothing this emulator holds are refused with HTTP 400 `ValidationException` — while a
/// reference into neither namespace this emulator can resolve is taken as given.
#[tokio::test]
async fn a_request_whose_build_inputs_cannot_resolve_is_refused() {
    let router = emulator();

    let unpublished = managed_base_image_arn(REGION, "al2023-99");
    let own_absent = expected_image_arn("never-created");
    let foreign = format!("arn:aws:lambda:{REGION}:111122223333:microvm-image:base");

    // Rows differ in WHICH build input fails to resolve and how: the code artifact's
    // URI in each way it can fail to address an S3 object, and the base image in each
    // namespace a reference can name.
    let rows: Vec<(&str, Value, u16, String)> = vec![
        (
            "a code artifact URI that is no S3 URI",
            json!({"codeArtifact": {"uri": "https://example.com/app.zip"}}),
            400,
            CODE_ARTIFACT_URI_REQUIREMENT.to_string(),
        ),
        (
            "an S3 URI naming a bucket and no object",
            json!({"codeArtifact": {"uri": "s3://artifacts"}}),
            400,
            CODE_ARTIFACT_URI_REQUIREMENT.to_string(),
        ),
        (
            "an S3 URI naming no bucket",
            json!({"codeArtifact": {"uri": "s3:///app.zip"}}),
            400,
            CODE_ARTIFACT_URI_REQUIREMENT.to_string(),
        ),
        (
            "a managed base image the catalogue does not publish",
            json!({"baseImageArn": unpublished.clone()}),
            400,
            base_image_not_found_message(&unpublished),
        ),
        (
            "an own-namespace base image this account does not hold",
            json!({"baseImageArn": own_absent.clone()}),
            400,
            base_image_not_found_message(&own_absent),
        ),
        (
            "a base image in neither namespace, taken as given",
            json!({"baseImageArn": foreign.clone()}),
            200,
            String::new(),
        ),
    ];

    // Each row gets its own image, so a row that is accepted cannot change what the
    // next one is answered; the images are built first and waited out together.
    for (row, _) in rows.iter().enumerate() {
        create_image(&router, &format!("inputs-{row}"), json!({})).await;
    }
    tokio::time::sleep(PAST_THE_SPAN).await;

    for (row, (case, extra, expected_status, expected_message)) in rows.into_iter().enumerate() {
        let name = format!("inputs-{row}");
        let (status, body) = update_image(&router, &name, update_body(extra)).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {body}"
        );
        if expected_status == 200 {
            assert_eq!(
                body["baseImageArn"].as_str(),
                Some(foreign.as_str()),
                "case {case:?}: the accepted reference is the stored one, got {body}"
            );
            continue;
        }
        assert_eq!(
            body["__type"].as_str(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(expected_message.as_str()),
            "case {case:?}: expected the refusal to name the member it refused on, got {:?}",
            body["message"]
        );
        assert_eq!(
            version_count(&router, &name, case).await,
            1,
            "case {case:?}: a refused update writes no version"
        );
    }
}

/// Give the image `name` `count` further settled versions, each a copy of the record
/// it already holds under a version of its own.
///
/// Writing the records the handlers read is what lets a test bring an image to its
/// version ceiling: every update's build has to settle before the next update is
/// accepted, so driving the ceiling over the wire would take one build span per
/// version.
fn seed_versions(engine: &engine::InMemoryEngine, name: &str, count: usize) {
    let state = service_state(engine).expect("the service's own store");
    let mut store = state.lock();
    let image = store
        .image_mut(REGION, name)
        .unwrap_or_else(|| panic!("the created image {name:?} is held"));
    let template = image
        .versions
        .values()
        .next()
        .cloned()
        .unwrap_or_else(|| panic!("the created image {name:?} holds its first version"));
    for _ in 0..count {
        let version = next_image_version(image.versions.keys().map(String::as_str));
        let mut record = template.clone();
        record.image_version = version.clone();
        record.build_due_at = None;
        image.versions.insert(version, record);
    }
}

/// RULE: an image's version history is bounded, so an update of an image already
/// holding the versions the service allows it is refused with the modelled HTTP 402
/// `ServiceQuotaExceededException` rather than growing the record without bound.
#[tokio::test]
async fn an_update_past_the_images_version_ceiling_is_refused() {
    let (router, engine) = emulator_with_engine();
    let name = "ceiling";
    built_image_with(&router, name, json!({})).await;

    // One version short of the ceiling: the next update is the last one that fits, and
    // the one after it is refused.
    seed_versions(&engine, name, MAX_MICROVM_IMAGE_VERSIONS - 2);

    let (status, accepted) = update_image(&router, name, update_body(json!({}))).await;
    assert_eq!(
        status, 200,
        "the update that fills the last version must be accepted, got {status} {accepted}"
    );
    tokio::time::sleep(PAST_THE_SPAN).await;

    let (status, refused) = update_image(&router, name, update_body(json!({}))).await;
    assert_eq!(
        status, 402,
        "an image at its version ceiling holds no further version, got {status} {refused}"
    );
    assert_eq!(
        refused["__type"].as_str(),
        Some("ServiceQuotaExceededException"),
        "expected the modelled quota error, got {refused}"
    );
    assert_eq!(
        refused["message"].as_str(),
        Some(too_many_image_versions_message().as_str()),
        "expected the refusal to state the limit it was refused on, got {:?}",
        refused["message"]
    );
    assert_eq!(
        refused["resourceId"].as_str(),
        Some(expected_image_arn(name).as_str()),
        "expected the refusal to name the image it was refused for, got {refused}"
    );

    // The refusal is the image's own ceiling, not the account's: another image still
    // takes an update.
    built_image_with(&router, "roomy", json!({})).await;
    let (status, other) = update_image(&router, "roomy", update_body(json!({}))).await;
    assert_eq!(
        status, 200,
        "an image below the ceiling still updates, got {status} {other}"
    );
}

/// RULE: `clientToken` is this operation's `@idempotencyToken`, so a retry carrying a
/// token that already minted a version reports that version — the persisted record as
/// it stands — rather than building a second version of the same configuration; a
/// different token mints the next version.
#[tokio::test]
async fn a_retried_update_replays_the_version_its_client_token_minted() {
    let router = emulator();
    let name = "retried";
    built_image_with(&router, name, json!({})).await;

    let request = update_body(json!({"clientToken": "token-1", "description": "the first body"}));
    let (status, first) = update_image(&router, name, request.clone()).await;
    assert_eq!(
        status, 200,
        "the first update must be accepted, got {status} {first}"
    );
    assert_eq!(
        first["imageVersion"].as_str(),
        Some("2.0"),
        "the first update mints the next version, got {first}"
    );

    // Rows differ in what the RETRY carries beside the token: the same body, and a
    // different one — this service models no idempotent-parameter-mismatch error, so
    // both report what the token minted.
    let rows: Vec<(&str, Value)> = vec![
        ("the same body", request),
        (
            "a different body",
            update_body(json!({
                "clientToken": "token-1",
                "buildRoleArn": OTHER_BUILD_ROLE_ARN,
                "description": "a different body",
            })),
        ),
    ];
    for (case, retry) in rows {
        let (status, replayed) = update_image(&router, name, retry).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the replay must succeed, got {status} {replayed}"
        );
        assert_eq!(
            replayed, first,
            "case {case:?}: the replay reports the version the token minted"
        );
        assert_eq!(
            version_count(&router, name, case).await,
            2,
            "case {case:?}: the replay mints no further version"
        );
    }

    // The replay reports the PERSISTED record, so once the clock has settled the build
    // it reports the same version in the state the record has reached.
    tokio::time::sleep(PAST_THE_SPAN).await;
    let (status, settled) = update_image(
        &router,
        name,
        update_body(json!({"clientToken": "token-1"})),
    )
    .await;
    assert_eq!(status, 200, "the replay got {status} {settled}");
    assert_eq!(
        settled["imageVersion"].as_str(),
        Some("2.0"),
        "the replay reports the version the token minted, got {settled}"
    );
    assert_eq!(
        settled["state"].as_str(),
        Some("UPDATED"),
        "the replay reports the record as it stands, got {settled}"
    );

    // A token that minted nothing is a new update, and mints the next version.
    let (status, next) = update_image(
        &router,
        name,
        update_body(json!({"clientToken": "token-2"})),
    )
    .await;
    assert_eq!(status, 200, "a new token's update got {status} {next}");
    assert_eq!(
        next["imageVersion"].as_str(),
        Some("3.0"),
        "a different token mints the next version, got {next}"
    );
}
