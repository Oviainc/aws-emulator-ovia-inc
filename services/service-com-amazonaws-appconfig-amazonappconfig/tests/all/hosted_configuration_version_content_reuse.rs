// SPDX-License-Identifier: CC0-1.0

//! What `CreateHostedConfigurationVersion` answers for content a configuration profile
//! already holds.
//!
//! The rule under test, in one sentence: a write whose STORED bytes are byte-for-byte equal
//! to those of the profile's newest hosted configuration version is answered with THAT
//! version — its number, its bytes and its metadata — and stores nothing, while content
//! differing by even one byte is a version of its own with the next number.
//!
//! Every case drives real HTTP requests through the router the emulator's front door uses,
//! so what is asserted is what an AppConfig client sees on the wire: the `Version-Number`
//! header the write answers, the listing of the profile's versions afterwards, and the
//! bytes each stored version reads back as.
//!
//! The table spans what the rule generalises over rather than the one recorded repeat: the
//! FORM of the content (JSON, bytes that are not text, empty content), whether the repeated
//! content equals the NEWEST version or an older one the profile still holds, how far the
//! content differs (one byte changed, one byte longer), and which of the write's other
//! members change while the bytes do not (`Content-Type`, `Description`) — the bytes that
//! get stored are what identifies a version, so those members do not make a version of
//! their own.
//!
//! A feature-flags profile is asserted separately, because the bytes that get stored there
//! are the feature-flags helper's stamped document rather than the caller's: re-posting an
//! unchanged authoring document keeps the stamps the profile already recorded, so the
//! stored bytes are identical and the version is reused.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{
    create_application, create_environment, create_flags_profile, create_profile, json_call,
    router, send, Answer,
};

/// One predefined deployment strategy id, so the reused version can be deployed without
/// creating a strategy first.
const ALL_AT_ONCE: &str = "AppConfig.AllAtOnce";

/// `…/hostedconfigurationversions`, this write's modelled URI
/// (`smithy_query(query="operation",operation="CreateHostedConfigurationVersion")`).
fn hosted_uri(application: &str, profile: &str) -> String {
    format!("/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions")
}

/// `POST` one hosted configuration version. `ContentType` travels as `Content-Type` and
/// `Description` as its own header, both `@httpHeader` members of this write.
async fn write_version(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    content_type: &str,
    content: &[u8],
    description: Option<&str>,
) -> Answer {
    let headers: Vec<(&str, String)> = description
        .map(|text| vec![("Description", text.to_string())])
        .unwrap_or_default();
    send(
        router,
        "POST",
        &hosted_uri(application, profile),
        content_type,
        content.to_vec(),
        &headers,
    )
    .await
}

/// `GET` one hosted configuration version back.
async fn read_version(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    version: i32,
) -> Answer {
    send(
        router,
        "GET",
        &format!("{}/{version}", hosted_uri(application, profile)),
        "application/json",
        Vec::new(),
        &[],
    )
    .await
}

/// The version numbers `ListHostedConfigurationVersions` reports for the profile.
async fn listed_versions(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
) -> Vec<i64> {
    let answer = json_call(
        router,
        "GET",
        &hosted_uri(application, profile),
        &Value::Null,
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "ListHostedConfigurationVersions must answer 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("VersionNumber").and_then(Value::as_i64))
                .collect()
        })
        .unwrap_or_default()
}

/// The `Version-Number` the write answered.
fn version_number(answer: &Answer, case: &str) -> i32 {
    assert_eq!(
        answer.status,
        201,
        "case {case:?}: CreateHostedConfigurationVersion must answer 201, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .header("Version-Number")
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or_else(|| panic!("case {case:?}: the write answered no Version-Number header"))
}

/// One row: what the write carries, and the version number AppConfig must answer for it.
struct Row {
    case: &'static str,
    content_type: &'static str,
    content: Vec<u8>,
    description: Option<&'static str>,
    /// The number the write must answer: one already held when the bytes are the newest
    /// version's, otherwise the next one allocated.
    expected_version: i32,
    /// Whether this write must have stored a version of its own.
    stores_a_version: bool,
}

/// Byte-identical content reuses the profile's newest version; content that differs by a
/// single byte allocates the next one.
#[tokio::test]
async fn identical_content_reuses_the_newest_version_and_different_content_allocates() {
    let router = router();
    let application = create_application(&router, "hosted-reuse").await;
    let profile = create_profile(&router, &application, "reused").await;

    let first = br#"{"enabled":true}"#.to_vec();
    let second = br#"{"enabled":false}"#.to_vec();
    let binary = vec![0u8, 1, 2, 250, 251, 255];

    let rows = vec![
        Row {
            case: "the profile's first content",
            content_type: "application/json",
            content: first.clone(),
            description: None,
            expected_version: 1,
            stores_a_version: true,
        },
        Row {
            case: "the same bytes again",
            content_type: "application/json",
            content: first.clone(),
            description: None,
            expected_version: 1,
            stores_a_version: false,
        },
        Row {
            case: "the same bytes a third time",
            content_type: "application/json",
            content: first.clone(),
            description: None,
            expected_version: 1,
            stores_a_version: false,
        },
        Row {
            case: "the same bytes under a different ContentType",
            content_type: "application/octet-stream",
            content: first.clone(),
            description: None,
            expected_version: 1,
            stores_a_version: false,
        },
        Row {
            case: "the same bytes under a Description",
            content_type: "application/json",
            content: first.clone(),
            description: Some("a re-post of bytes already held"),
            expected_version: 1,
            stores_a_version: false,
        },
        Row {
            case: "one byte changed",
            content_type: "application/json",
            content: second.clone(),
            description: None,
            expected_version: 2,
            stores_a_version: true,
        },
        Row {
            case: "the new newest bytes again",
            content_type: "application/json",
            content: second.clone(),
            description: None,
            expected_version: 2,
            stores_a_version: false,
        },
        Row {
            case: "bytes equal to an older version, no longer the newest",
            content_type: "application/json",
            content: first.clone(),
            description: None,
            expected_version: 3,
            stores_a_version: true,
        },
        Row {
            case: "one byte longer than the newest",
            content_type: "application/json",
            content: br#"{"enabled":true} "#.to_vec(),
            description: None,
            expected_version: 4,
            stores_a_version: true,
        },
        Row {
            case: "empty content",
            content_type: "application/octet-stream",
            content: Vec::new(),
            description: None,
            expected_version: 5,
            stores_a_version: true,
        },
        Row {
            case: "empty content again",
            content_type: "application/octet-stream",
            content: Vec::new(),
            description: None,
            expected_version: 5,
            stores_a_version: false,
        },
        Row {
            case: "bytes that are not text",
            content_type: "application/octet-stream",
            content: binary.clone(),
            description: None,
            expected_version: 6,
            stores_a_version: true,
        },
        Row {
            case: "the same bytes that are not text",
            content_type: "application/octet-stream",
            content: binary.clone(),
            description: None,
            expected_version: 6,
            stores_a_version: false,
        },
    ];

    // The version numbers the rows so far have stored, which the listing must report and
    // nothing else.
    let mut stored: Vec<i32> = Vec::new();
    for row in &rows {
        let answer = write_version(
            &router,
            &application,
            &profile,
            row.content_type,
            &row.content,
            row.description,
        )
        .await;
        let case = row.case;
        assert_eq!(
            version_number(&answer, case),
            row.expected_version,
            "case {case:?}: expected version {}, got {:?}",
            row.expected_version,
            answer.header("Version-Number")
        );
        assert_eq!(
            answer.body.as_ref(),
            row.content.as_slice(),
            "case {case:?}: the write must answer the version's stored bytes, got {:?}",
            String::from_utf8_lossy(&answer.body)
        );
        if row.stores_a_version {
            stored.push(row.expected_version);
        }
        // The listing is the record of what was stored: a reused version adds no entry.
        let mut expected_listing = stored.clone();
        expected_listing.sort_unstable_by(|left, right| right.cmp(left));
        let listed = listed_versions(&router, &application, &profile).await;
        assert_eq!(
            listed,
            expected_listing
                .iter()
                .map(|version| i64::from(*version))
                .collect::<Vec<i64>>(),
            "case {case:?}: expected the profile to hold {expected_listing:?}, got {listed:?}"
        );
    }

    // Every stored version still reads back as the bytes it was written with, so a reuse
    // did not overwrite one.
    for (case, version, content) in [
        ("version 1", 1, first.clone()),
        ("version 2", 2, second.clone()),
        ("version 3", 3, first.clone()),
        ("version 4", 4, br#"{"enabled":true} "#.to_vec()),
        ("version 5", 5, Vec::new()),
        ("version 6", 6, binary.clone()),
    ] {
        let answer = read_version(&router, &application, &profile, version).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: GetHostedConfigurationVersion must answer 200, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.body.as_ref(),
            content.as_slice(),
            "case {case:?}: expected the stored bytes back, got {:?}",
            String::from_utf8_lossy(&answer.body)
        );
    }

    // The first reused version carries the metadata it was STORED with: the re-posts above
    // carried another `ContentType` and a `Description`, and neither rewrote the version.
    let held = read_version(&router, &application, &profile, 1).await;
    assert_eq!(
        held.header("Content-Type").as_deref(),
        Some("application/json"),
        "the reused version must keep the ContentType it was stored with, got {:?}",
        held.header("Content-Type")
    );
    assert_eq!(
        held.header("Description"),
        None,
        "the reused version must keep the Description it was stored with, got {:?}",
        held.header("Description")
    );
}

/// The version a repeat reuses is a real, deployable version: a deployment of it is
/// accepted and records that version number.
#[tokio::test]
async fn the_reused_version_is_deployable() {
    let router = router();
    let application = create_application(&router, "hosted-reuse-deploy").await;
    let environment = create_environment(&router, &application, "prod").await;
    let profile = create_profile(&router, &application, "deployed").await;
    let content = br#"{"enabled":true,"limit":5}"#;

    let first = write_version(
        &router,
        &application,
        &profile,
        "application/json",
        content,
        None,
    )
    .await;
    assert_eq!(
        version_number(&first, "the first write"),
        1,
        "the first write must answer version 1"
    );
    let again = write_version(
        &router,
        &application,
        &profile,
        "application/json",
        content,
        None,
    )
    .await;
    assert_eq!(
        version_number(&again, "the repeat"),
        1,
        "the repeat must reuse version 1, got {:?}",
        again.header("Version-Number")
    );

    let started = json_call(
        &router,
        "POST",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        &json!({
            "DeploymentStrategyId": ALL_AT_ONCE,
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": "1",
        }),
    )
    .await;
    assert_eq!(
        started.status,
        201,
        "StartDeployment of the reused version must answer 201, got {} / {}",
        started.status,
        String::from_utf8_lossy(&started.body)
    );
    assert_eq!(
        started.string("ConfigurationVersion"),
        Some("1".to_string()),
        "the deployment must record the reused version, got {}",
        started.json()
    );
}

/// On a feature-flags profile the bytes that get STORED are the helper's stamped document,
/// so an unchanged authoring document re-posted reuses the version while a changed flag
/// value allocates the next one.
#[tokio::test]
async fn an_unchanged_feature_flag_document_reuses_its_version() {
    let router = router();
    let application = create_application(&router, "hosted-reuse-flags").await;
    let profile = create_flags_profile(&router, &application, "flagged").await;

    let authored = json!({
        "flags": { "runflag": { "name": "runflag" } },
        "values": { "runflag": { "enabled": true } },
        "version": "1",
    });
    let changed = json!({
        "flags": { "runflag": { "name": "runflag" } },
        "values": { "runflag": { "enabled": false } },
        "version": "1",
    });

    for (case, document, expected, held) in [
        ("the first authoring document", &authored, 1, vec![1]),
        ("the same authoring document again", &authored, 1, vec![1]),
        ("a changed flag value", &changed, 2, vec![2, 1]),
        ("the changed document again", &changed, 2, vec![2, 1]),
    ] {
        let content = serde_json::to_vec(document).expect("the document must serialize");
        let answer = write_version(
            &router,
            &application,
            &profile,
            "application/json",
            &content,
            None,
        )
        .await;
        assert_eq!(
            version_number(&answer, case),
            expected,
            "case {case:?}: expected version {expected}, got {:?}",
            answer.header("Version-Number")
        );
        let listed = listed_versions(&router, &application, &profile).await;
        assert_eq!(
            listed,
            held.iter().map(|version| i64::from(*version)).collect::<Vec<i64>>(),
            "case {case:?}: expected the profile to hold {held:?}, got {listed:?}"
        );
    }
}
