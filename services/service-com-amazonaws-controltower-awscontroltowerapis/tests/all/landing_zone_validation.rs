// SPDX-License-Identifier: CC0-1.0

//! What a landing zone configuration must be, end to end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter)
//! the production emulator dispatches with, so what these tests exercise is the wire
//! behaviour a client sees — status codes, error codes and JSON bodies.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * a landing zone is created or updated only at a version the service OFFERS — the
//!   modelled `LandingZoneVersion` (len 3..10, pattern `^\d+.\d+$`) bounds the shape of
//!   the string, not the set of versions that exist — and a version outside that set is
//!   refused with `ValidationException` (400), with nothing persisted;
//! * the `manifest` is the landing zone's CONFIGURATION document: a create or update
//!   whose manifest is not a JSON object, or which omits or mistypes a parameter the
//!   configuration requires, is refused the same way and persists nothing;
//! * everything the manifest carries BEYOND those parameters is accepted and stored
//!   unchanged, because the manifest's full schema "is not published, by design"
//!   (`UpdateLandingZoneInput$manifest`).
//!
//! The catalog of offered versions and the set of required manifest parameters are the
//! service's own (`state::CATALOG_LANDING_ZONE_VERSIONS`,
//! `state::REQUIRED_MANIFEST_MEMBERS`, sourced there from the AWS Control Tower user
//! guide's landing zone version table and its example manifest), and these tables are
//! driven from them so EVERY entry is covered rather than the ones the issue cited.

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    ManifestMemberKind, CATALOG_LANDING_ZONE_VERSION, CATALOG_LANDING_ZONE_VERSIONS,
    OPTIONAL_MANIFEST_MEMBERS, REQUIRED_MANIFEST_MEMBERS,
};

use crate::landing_zone_lifecycle::{
    call, emulator, manifest_governing, past_the_settle_span, simple_manifest, Answer,
};

/// A `CreateLandingZone` request body.
fn create_body(version: &str, manifest: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "version": version, "manifest": manifest })
}

/// JSON-pointer form of a dotted manifest path (`a.b` -> `/a/b`).
fn pointer(path: &str) -> String {
    format!("/{}", path.replace('.', "/"))
}

/// `document` with the member `path` names dropped. Panics when it is not there, so a
/// row can never silently assert nothing.
fn without(document: &serde_json::Value, path: &str) -> serde_json::Value {
    let mut copy = document.clone();
    let (parent_path, leaf) = match path.rsplit_once('.') {
        Some((parent, leaf)) => (pointer(parent), leaf),
        None => (String::new(), path),
    };
    let parent = if parent_path.is_empty() {
        &mut copy
    } else {
        copy.pointer_mut(&parent_path)
            .unwrap_or_else(|| panic!("the manifest holds the container of {path:?}"))
    };
    parent
        .as_object_mut()
        .unwrap_or_else(|| panic!("the container of {path:?} is an object"))
        .remove(leaf)
        .unwrap_or_else(|| panic!("the manifest holds {path:?}, so it can be removed"));
    copy
}

/// `document` with the member `path` names replaced by `value`.
///
/// REPLACES rather than inserts: `path` must already be there (it goes through
/// [`without`], which panics otherwise), so a row cannot quietly assert on a manifest
/// that never held the member it meant to change.
fn with(document: &serde_json::Value, path: &str, value: serde_json::Value) -> serde_json::Value {
    let mut copy = without(document, path);
    let (parent_path, leaf) = match path.rsplit_once('.') {
        Some((parent, leaf)) => (pointer(parent), leaf),
        None => (String::new(), path),
    };
    let parent = if parent_path.is_empty() {
        &mut copy
    } else {
        copy.pointer_mut(&parent_path)
            .unwrap_or_else(|| panic!("the manifest holds the container of {path:?}"))
    };
    parent
        .as_object_mut()
        .unwrap_or_else(|| panic!("the container of {path:?} is an object"))
        .insert(leaf.to_string(), value);
    copy
}

/// Assert `answer` is the modelled 400 `ValidationException`, and that `router` holds no
/// landing zone: a refused create persists nothing.
async fn refused(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    answer: &Answer,
) {
    assert_eq!(
        (answer.status, answer.error_code.as_deref()),
        (400, Some("ValidationException")),
        "case {case:?}: expected 400 ValidationException, got {:?} body {:?}",
        (answer.status, answer.error_code.as_deref()),
        answer.body
    );
    assert!(
        answer
            .body
            .get("message")
            .and_then(|value| value.as_str())
            .is_some_and(|message| !message.is_empty()),
        "case {case:?}: expected the refusal to carry a message, got {:?}",
        answer.body
    );
    let listed = call(router, "/list-landingzones", serde_json::json!({})).await;
    assert_eq!(
        listed.get("landingZones"),
        &serde_json::json!([]),
        "case {case:?}: a refused create must persist nothing, got {:?}",
        listed.get("landingZones")
    );
}

/// RULE (the catalog this service publishes): every version it offers satisfies the
/// modelled `LandingZoneVersion` (len 3..10, pattern `^\d+.\d+$`), the list is in
/// strictly increasing NUMERIC order, and its newest entry is the one reported as
/// `latestAvailableVersion`.
#[test]
fn the_catalog_offers_well_formed_versions_in_numeric_order() {
    assert!(
        !CATALOG_LANDING_ZONE_VERSIONS.is_empty(),
        "expected the catalog to offer at least one version, got {CATALOG_LANDING_ZONE_VERSIONS:?}"
    );

    let mut previous: Option<(u64, u64)> = None;
    for version in CATALOG_LANDING_ZONE_VERSIONS {
        assert!(
            (3..=10).contains(&version.len()),
            "version {version:?}: expected LandingZoneVersion's length 3..10, got {}",
            version.len()
        );
        let Some((major, minor)) = version.split_once('.') else {
            panic!("version {version:?}: expected the pattern ^\\d+.\\d+$, got no separator")
        };
        assert!(
            !major.is_empty()
                && !minor.is_empty()
                && major.bytes().all(|b| b.is_ascii_digit())
                && minor.bytes().all(|b| b.is_ascii_digit()),
            "version {version:?}: expected the pattern ^\\d+.\\d+$"
        );
        let order = (
            major.parse::<u64>().expect("the major component parses"),
            minor.parse::<u64>().expect("the minor component parses"),
        );
        assert!(
            previous.is_none_or(|earlier| earlier < order),
            "version {version:?}: expected the catalog in increasing numeric order, \
             {previous:?} precedes {order:?}"
        );
        previous = Some(order);
    }

    assert_eq!(
        CATALOG_LANDING_ZONE_VERSIONS.last().copied(),
        Some(CATALOG_LANDING_ZONE_VERSION),
        "expected the catalog's newest entry to be {CATALOG_LANDING_ZONE_VERSION:?}, got {:?}",
        CATALOG_LANDING_ZONE_VERSIONS.last()
    );
}

/// RULE: a landing zone is created at a version the service OFFERS; a `version` the
/// catalog does not offer is refused with `ValidationException` (400) and no landing zone
/// is minted.
///
/// The accepted rows are EVERY entry of the catalog. The refused rows all satisfy
/// `LandingZoneVersion`'s length and pattern, so the front door passes them, and sit one
/// step outside each end of the published line as well as inside gaps it does not
/// contain.
#[tokio::test]
async fn only_a_version_the_catalog_offers_is_accepted() {
    for version in CATALOG_LANDING_ZONE_VERSIONS {
        let router = emulator();
        let created = call(
            &router,
            "/create-landingzone",
            create_body(version, &simple_manifest()),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "version {version:?}: expected the catalog's own version to be accepted, got \
             {} body {:?}",
            created.status, created.body
        );
        let detail = call(
            &router,
            "/get-landingzone",
            serde_json::json!({ "landingZoneIdentifier": created.string("arn") }),
        )
        .await;
        assert_eq!(
            detail.get("landingZone").get("version").cloned(),
            Some(serde_json::json!(version)),
            "version {version:?}: expected it reported back as the deployed version, got {:?}",
            detail.get("landingZone")
        );
    }

    // (case, a version the catalog does not offer)
    let unsupported = [
        ("one minor past the newest offered version", "3.4"),
        ("one major past the newest offered version", "4.0"),
        ("one minor before the oldest offered version", "1.9"),
        ("a major the line does not contain", "0.0"),
        ("a minor the 2.x line does not contain", "2.10"),
        ("a double-digit major", "10.0"),
        ("the issue's own unsupported version", "9.9"),
        ("the longest version the model admits", "1234.56789"),
    ];
    for (case, version) in unsupported {
        let router = emulator();
        let answer = call(
            &router,
            "/create-landingzone",
            create_body(version, &simple_manifest()),
        )
        .await;
        refused(&router, &format!("{case} ({version:?})"), &answer).await;
    }
}

/// RULE: the `manifest` is the landing zone's configuration document, so a manifest that
/// is not a JSON object describes no configuration and is refused.
///
/// Rows span every other JSON kind a `Manifest` document can be — array, string, number,
/// boolean, null — plus the object that declares none of the required parameters.
#[tokio::test]
async fn a_manifest_that_is_not_a_configuration_document_is_refused() {
    let cases: Vec<(&str, serde_json::Value)> = vec![
        ("an empty object", serde_json::json!({})),
        (
            "an object of unrelated members",
            serde_json::json!({ "hello": "world" }),
        ),
        ("an array at the root", serde_json::json!([1, "two"])),
        ("a string at the root", serde_json::json!("not-an-object")),
        ("a number at the root", serde_json::json!(3.3)),
        ("a boolean at the root", serde_json::json!(true)),
        ("null at the root", serde_json::Value::Null),
    ];

    for (case, manifest) in cases {
        let router = emulator();
        let answer = call(
            &router,
            "/create-landingzone",
            create_body(CATALOG_LANDING_ZONE_VERSION, &manifest),
        )
        .await;
        refused(&router, case, &answer).await;
    }
}

/// RULE: a create whose manifest omits, or mistypes, a parameter the landing zone
/// configuration requires is refused with `ValidationException` (400) and persists
/// nothing — for EVERY such parameter, not only the ones an example names.
///
/// The rows are generated from `state::REQUIRED_MANIFEST_MEMBERS` (the required
/// parameters of the user guide's example manifest): each one dropped from an otherwise
/// well-formed manifest, and each one replaced by a value of a JSON kind no parameter
/// accepts.
#[tokio::test]
async fn a_manifest_missing_a_required_parameter_is_refused() {
    let well_formed = simple_manifest();

    for (path, _) in REQUIRED_MANIFEST_MEMBERS {
        for (shape, manifest) in [
            ("dropped", without(&well_formed, path)),
            // A number is of no kind any required parameter accepts, so one value shows
            // the type check for every path.
            ("mistyped", with(&well_formed, path, serde_json::json!(17))),
        ] {
            let case = format!("{path} {shape}");
            let router = emulator();
            let answer = call(
                &router,
                "/create-landingzone",
                create_body(CATALOG_LANDING_ZONE_VERSION, &manifest),
            )
            .await;
            refused(&router, &case, &answer).await;
            assert!(
                answer
                    .body
                    .get("message")
                    .and_then(|value| value.as_str())
                    .is_some_and(|message| message.contains(path)),
                "case {case:?}: expected the refusal to name {path:?}, got {:?}",
                answer.body
            );
        }
    }
}

/// RULE: a required parameter's VALUE must be of the documented kind — a Region list of
/// strings, a 12-digit account id, a boolean, an organizational unit carrying its `name`
/// — and an optional parameter the manifest does carry is held to the same rule.
///
/// Rows span each kind with a value that is of the right JSON type but still outside the
/// kind, which the one-number row of the previous table cannot reach.
#[tokio::test]
async fn a_required_parameter_of_the_wrong_kind_is_refused() {
    let well_formed = simple_manifest();

    // (case, the manifest, whether the service must refuse it)
    let cases: Vec<(&str, serde_json::Value, bool)> = vec![
        (
            "governedRegions holding a non-string entry",
            with(
                &well_formed,
                "governedRegions",
                serde_json::json!(["us-west-2", 7]),
            ),
            true,
        ),
        (
            "governedRegions as an object",
            with(&well_formed, "governedRegions", serde_json::json!({})),
            true,
        ),
        (
            "an account id of the wrong length",
            with(
                &well_formed,
                "centralizedLogging.accountId",
                serde_json::json!("12345"),
            ),
            true,
        ),
        (
            "an account id that is not digits",
            with(
                &well_formed,
                "securityRoles.accountId",
                serde_json::json!("abcdefghijkl"),
            ),
            true,
        ),
        (
            "an enabled flag as the string \"true\"",
            with(
                &well_formed,
                "accessManagement.enabled",
                serde_json::json!("true"),
            ),
            true,
        ),
        (
            "a security OU with no name",
            with(
                &well_formed,
                "organizationStructure.security",
                serde_json::json!({ "note": "no name here" }),
            ),
            true,
        ),
        (
            "a security OU whose name is not a string",
            with(
                &well_formed,
                "organizationStructure.security",
                serde_json::json!({ "name": 7 }),
            ),
            true,
        ),
        (
            "the optional sandbox OU, mistyped",
            with(
                &well_formed,
                "organizationStructure.sandbox",
                serde_json::json!("Sandbox"),
            ),
            true,
        ),
        // Accepted rows: the optional parameter absent, an empty Region list, and
        // the enabled flags turned off — all well-formed configurations.
        (
            "the optional sandbox OU absent",
            without(&well_formed, "organizationStructure.sandbox"),
            false,
        ),
        ("no governed regions", manifest_governing(&[]), false),
        (
            "centralizedLogging disabled",
            with(
                &well_formed,
                "centralizedLogging.enabled",
                serde_json::json!(false),
            ),
            false,
        ),
        (
            "an empty configurations object",
            with(
                &well_formed,
                "centralizedLogging.configurations",
                serde_json::json!({}),
            ),
            false,
        ),
    ];

    // The optional table must really name the member the rows above mistype, so this
    // table cannot drift away from the service's own set.
    assert!(
        OPTIONAL_MANIFEST_MEMBERS.contains(&(
            "organizationStructure.sandbox",
            ManifestMemberKind::NamedOrganizationalUnit
        )),
        "expected the sandbox OU among the optional manifest members, got \
         {OPTIONAL_MANIFEST_MEMBERS:?}"
    );

    for (case, manifest, must_refuse) in cases {
        let router = emulator();
        let answer = call(
            &router,
            "/create-landingzone",
            create_body(CATALOG_LANDING_ZONE_VERSION, &manifest),
        )
        .await;
        if must_refuse {
            refused(&router, case, &answer).await;
        } else {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected a well-formed manifest to be accepted, got {} \
                 body {:?}",
                answer.status, answer.body
            );
            let detail = call(
                &router,
                "/get-landingzone",
                serde_json::json!({ "landingZoneIdentifier": answer.string("arn") }),
            )
            .await;
            assert_eq!(
                detail.get("landingZone").get("manifest"),
                Some(&manifest),
                "case {case:?}: expected the accepted manifest reported back unchanged, \
                 got {:?}",
                detail.get("landingZone")
            );
        }
    }
}

/// RULE: the same two rules hold for `UpdateLandingZone`, because they are properties of
/// a landing zone configuration rather than of one call — and a refused update changes
/// nothing: the landing zone keeps the version and manifest it has deployed.
///
/// Rows span an unsupported version, a manifest of the wrong JSON kind, a manifest
/// missing a required parameter, and (as the control) a request that is valid on both
/// counts.
#[tokio::test]
async fn an_update_is_held_to_the_same_version_and_manifest_rules() {
    let router = emulator();
    let deployed_version = CATALOG_LANDING_ZONE_VERSIONS[0];
    let deployed_manifest = manifest_governing(&["us-west-2"]);
    let created = call(
        &router,
        "/create-landingzone",
        create_body(deployed_version, &deployed_manifest),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the create the update rows start from: expected 200, got {} body {:?}",
        created.status, created.body
    );
    let arn = created.string("arn");
    tokio::time::sleep(past_the_settle_span()).await;

    // (case, the update body)
    let refusals: Vec<(&str, serde_json::Value)> = vec![
        (
            "a version the catalog does not offer",
            serde_json::json!({ "landingZoneIdentifier": arn, "version": "9.9" }),
        ),
        (
            "a manifest that is not an object",
            serde_json::json!({
                "landingZoneIdentifier": arn,
                "version": CATALOG_LANDING_ZONE_VERSION,
                "manifest": "not-an-object"
            }),
        ),
        (
            "an empty manifest",
            serde_json::json!({
                "landingZoneIdentifier": arn,
                "version": CATALOG_LANDING_ZONE_VERSION,
                "manifest": {}
            }),
        ),
        (
            "a manifest missing its securityRoles",
            serde_json::json!({
                "landingZoneIdentifier": arn,
                "version": CATALOG_LANDING_ZONE_VERSION,
                "manifest": without(&deployed_manifest, "securityRoles")
            }),
        ),
    ];

    for (case, body) in refusals {
        let answer = call(&router, "/update-landingzone", body).await;
        assert_eq!(
            (answer.status, answer.error_code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected 400 ValidationException, got {:?} body {:?}",
            (answer.status, answer.error_code.as_deref()),
            answer.body
        );

        // A refused update started no operation, so the landing zone is still ACTIVE on
        // the configuration it had deployed.
        let detail = call(
            &router,
            "/get-landingzone",
            serde_json::json!({ "landingZoneIdentifier": arn }),
        )
        .await;
        let landing_zone = detail.get("landingZone").clone();
        assert_eq!(
            (
                landing_zone.get("version").cloned(),
                landing_zone.get("manifest").cloned(),
                landing_zone.get("status").cloned(),
            ),
            (
                Some(serde_json::json!(deployed_version)),
                Some(deployed_manifest.clone()),
                Some(serde_json::json!("ACTIVE")),
            ),
            "case {case:?}: expected the deployed configuration untouched and ACTIVE, got \
             {landing_zone:?}"
        );
    }

    // The control: a version the catalog offers with a well-formed manifest is accepted,
    // and once the clock makes the operation due it is what is deployed.
    let updated_manifest = manifest_governing(&["us-west-2", "eu-west-1"]);
    let accepted = call(
        &router,
        "/update-landingzone",
        serde_json::json!({
            "landingZoneIdentifier": arn,
            "version": CATALOG_LANDING_ZONE_VERSION,
            "manifest": updated_manifest
        }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "a valid update: expected 200, got {} body {:?}",
        accepted.status, accepted.body
    );
    tokio::time::sleep(past_the_settle_span()).await;
    let detail = call(
        &router,
        "/get-landingzone",
        serde_json::json!({ "landingZoneIdentifier": arn }),
    )
    .await;
    let landing_zone = detail.get("landingZone").clone();
    assert_eq!(
        (
            landing_zone.get("version").cloned(),
            landing_zone.get("manifest").cloned(),
        ),
        (
            Some(serde_json::json!(CATALOG_LANDING_ZONE_VERSION)),
            Some(updated_manifest),
        ),
        "a valid update: expected the requested configuration deployed, got {landing_zone:?}"
    );
}
