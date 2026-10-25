// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IoT SiteWise Monitor portal slice (`CreatePortal` ->
//! `DescribePortal`), driven over HTTP through the same `MultiServiceRouter` the
//! emulator boots with, so every assertion is about the wire behaviour a real
//! client sees.
//!
//! The rules these suites pin:
//!
//! * a created portal is persisted and reads back exactly as it was created — every
//!   optional member it carried and none it did not — with the ARN form
//!   `CreatePortal` documents;
//! * `portalAuthMode` defaults to SSO, and the IAM Identity Center identifiers
//!   (`ssoApplicationId`, `portalClientId`) are reported only for a portal that
//!   authenticates through IAM Identity Center;
//! * a portal answers `CREATING` straight after the create and `ACTIVE` once the
//!   transition the clock owes it falls due, and a read never changes what the next
//!   caller sees;
//! * an id that resolves to no stored portal is a modelled 404 rather than a
//!   fabricated answer;
//! * a tag key outside the modelled `TagKey` bounds is the handler's own 400.
//!
//! The one refusal these suites do NOT exercise is the `MAX_PORTALS` ceiling: the
//! bound is 1000 portals and nothing evicts one, so covering it would mean a
//! thousand creates in a single test. The same ceiling on the sibling resource
//! maps is left uncovered for the same reason.
//!
//! The HTTP harness (`call`, `router`, `error_code`, `is_modelled_id`) is the one
//! the asset suite already has; this suite reuses it rather than growing a second.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    service_state, ACTIVATION_DELAY, TAG_KEY_MAX_LENGTH,
};

use crate::asset_lifecycle::{
    call, error_code, is_modelled_id, router, router_with_engine, REGION,
};

/// Where every portal request goes, as `OPERATIONS.md` reports the route:
/// `POST /portals`, `GET /portals/{portalId}`.
const PORTALS: &str = "/portals";

/// A service-role ARN within the modelled `IamArn` pattern.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::123456789012:role/{name}")
}

/// One row of the create-and-read-back table: the body the create sends, beside
/// what the read must then report.
#[derive(Debug)]
struct CreateCase {
    label: &'static str,
    /// The request members beyond the required `portalName`, `portalContactEmail`
    /// and `roleArn`, which every row sends.
    extra: Value,
    /// What `DescribePortal` must report for `portalAuthMode` — the value the
    /// request sent, or the documented default when it sent none.
    expected_auth_mode: &'static str,
    /// Whether the portal authenticates through IAM Identity Center, and so names
    /// an `ssoApplicationId` / `portalClientId` at all.
    identity_center: bool,
}

/// The rows. They differ along every dimension `CreatePortal`'s input has:
/// the `AuthMode` value (absent / `SSO` / `IAM`, the enum's whole value list —
/// smithy_query query="shape" shape="AuthMode"), the `PortalType` value
/// (`SITEWISE_PORTAL_V1` | `SITEWISE_PORTAL_V2`, likewise the whole list), a
/// description or none, alarms with and without the optional notification Lambda,
/// a notification sender email, a logo image file, a portal-type configuration and
/// tags of several sizes.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            label: "bare-required-members-only",
            extra: json!({}),
            // "Default: SSO" — `portalAuthMode`'s own documentation.
            expected_auth_mode: "SSO",
            identity_center: true,
        },
        CreateCase {
            label: "explicit-sso-with-description-and-tags",
            extra: json!({
                "portalAuthMode": "SSO",
                "portalDescription": "the floor-one monitor portal",
                "tags": { "stage": "test" },
            }),
            expected_auth_mode: "SSO",
            identity_center: true,
        },
        CreateCase {
            label: "iam-auth-mode",
            extra: json!({ "portalAuthMode": "IAM" }),
            expected_auth_mode: "IAM",
            identity_center: false,
        },
        CreateCase {
            label: "iam-with-alarms-and-sender-email",
            extra: json!({
                "portalAuthMode": "IAM",
                "alarms": {
                    "alarmRoleArn": role_arn("alarm-role"),
                    "notificationLambdaArn": "arn:aws:lambda:us-west-2:123456789012:function:alarms",
                },
                "notificationSenderEmail": "alarms@example.com",
                "tags": { "team": "ops", "env": "edge", "owner": "sitewise" },
            }),
            expected_auth_mode: "IAM",
            identity_center: false,
        },
        CreateCase {
            label: "alarms-without-notification-lambda",
            extra: json!({
                "alarms": { "alarmRoleArn": role_arn("alarm-role-only") },
            }),
            expected_auth_mode: "SSO",
            identity_center: true,
        },
        CreateCase {
            label: "classic-portal-type-with-configuration",
            extra: json!({
                "portalType": "SITEWISE_PORTAL_V1",
                "portalTypeConfiguration": {
                    "SITEWISE_PORTAL_V1": { "portalTools": ["DASHBOARD"] },
                },
            }),
            expected_auth_mode: "SSO",
            identity_center: true,
        },
        CreateCase {
            label: "ai-aware-portal-type-with-two-tools",
            extra: json!({
                "portalType": "SITEWISE_PORTAL_V2",
                "portalTypeConfiguration": {
                    "SITEWISE_PORTAL_V2": { "portalTools": ["ASSISTANT", "DASHBOARD"] },
                },
                "portalDescription": "the AI-aware portal",
            }),
            expected_auth_mode: "SSO",
            identity_center: true,
        },
        CreateCase {
            label: "with-logo-image-file",
            extra: json!({
                // `ImageFileType` admits exactly one value, `PNG`
                // (smithy_query query="shape" shape="ImageFileType"); the data is a
                // base64-encoded blob, as `ImageFile$data` models it.
                "portalLogoImageFile": { "data": "aVZCT1J3MEtHZ29BQUE=", "type": "PNG" },
            }),
            expected_auth_mode: "SSO",
            identity_center: true,
        },
        CreateCase {
            label: "client-token-and-every-optional-member",
            extra: json!({
                "clientToken": "0c2a4b8e-6d1f-4a3b-9c7e-5f1a2b3c4d5e-token",
                "portalAuthMode": "SSO",
                "portalDescription": "everything at once",
                "notificationSenderEmail": "sender@example.com",
                "alarms": { "alarmRoleArn": role_arn("everything-alarm-role") },
                "portalType": "SITEWISE_PORTAL_V2",
                "portalTypeConfiguration": {
                    "SITEWISE_PORTAL_V2": { "portalTools": ["ASSISTANT"] },
                },
                "portalLogoImageFile": { "data": "aVZCT1J3MEtHZ29BQUE=", "type": "PNG" },
                "tags": { "everything": "yes" },
            }),
            expected_auth_mode: "SSO",
            identity_center: true,
        },
    ]
}

/// The body one row sends.
fn create_body(case: &CreateCase, name: &str) -> Value {
    let mut body = json!({
        "portalName": name,
        "portalContactEmail": "admin@example.com",
        "roleArn": role_arn(&format!("service-role-{}", case.label)),
    });
    let object = body.as_object_mut().expect("an object body");
    for (member, value) in case.extra.as_object().expect("an object of extras") {
        object.insert(member.clone(), value.clone());
    }
    body
}

#[tokio::test]
async fn created_portal_reads_back_through_describe_for_every_input_shape() {
    let (router, engine) = router_with_engine();
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();
    let mut seen_identity_center_ids: BTreeSet<String> = BTreeSet::new();

    for case in create_cases() {
        let name = format!("portal-{}", case.label);
        let body = create_body(&case, &name);
        let (status, created) = call(&router, "POST", PORTALS, Some(body.clone())).await;
        assert_eq!(
            status, 202,
            "case {:?}: expected 202, got {status} {created}",
            case.label
        );

        let id = created["portalId"].as_str().unwrap_or("").to_string();
        assert!(
            is_modelled_id(&id),
            "case {:?}: portalId must be a modelled UUID, got {created}",
            case.label
        );
        assert!(
            seen_ids.insert(id.clone()),
            "case {:?}: portalId {id} was already minted for another portal",
            case.label
        );
        assert_eq!(
            created["portalArn"].as_str().unwrap_or(""),
            format!(
                "arn:aws:iotsitewise:{REGION}:{}:portal/{id}",
                engine::EMULATOR_ACCOUNT_ID
            ),
            "case {:?}: unexpected portalArn in {created}",
            case.label
        );
        // `portalStartUrl` is the portal's own URL, so it carries the portal's id
        // and stays within the modelled `Url` pattern.
        let start_url = created["portalStartUrl"].as_str().unwrap_or("").to_string();
        assert!(
            start_url.starts_with("https://") && start_url.contains(&id) && start_url.len() <= 256,
            "case {:?}: unexpected portalStartUrl in {created}",
            case.label
        );
        // "a state (CREATING after successfully calling this operation)".
        assert_eq!(
            created["portalStatus"]["state"].as_str().unwrap_or(""),
            "CREATING",
            "case {:?}: a fresh portal answers CREATING, got {created}",
            case.label
        );

        let sso_application_id = created["ssoApplicationId"].as_str().unwrap_or("<absent>");
        if case.identity_center {
            assert!(
                !sso_application_id.is_empty() && sso_application_id.len() <= 64,
                "case {:?}: an IAM Identity Center portal names an ssoApplicationId, got {created}",
                case.label
            );
            assert!(
                seen_identity_center_ids.insert(sso_application_id.to_string()),
                "case {:?}: ssoApplicationId {sso_application_id} was already minted",
                case.label
            );
        } else {
            // "The associated IAM Identity Center application ID, IF the portal uses
            // IAM Identity Center" — a portal authenticating through IAM has no such
            // application, so it names none.
            assert_eq!(
                sso_application_id, "",
                "case {:?}: an IAM portal names no ssoApplicationId, got {created}",
                case.label
            );
        }

        let (status, described) = call(&router, "GET", &format!("{PORTALS}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribePortal expected 200, got {status} {described}",
            case.label
        );
        assert_eq!(
            described["portalId"].as_str().unwrap_or(""),
            id,
            "case {:?}: DescribePortal reported another id in {described}",
            case.label
        );
        assert_eq!(
            described["portalName"].as_str().unwrap_or(""),
            name,
            "case {:?}: DescribePortal reported another name in {described}",
            case.label
        );
        assert_eq!(
            described["portalArn"], created["portalArn"],
            "case {:?}: DescribePortal reported another ARN in {described}",
            case.label
        );
        assert_eq!(
            described["portalStartUrl"], created["portalStartUrl"],
            "case {:?}: DescribePortal reported another start URL in {described}",
            case.label
        );
        assert_eq!(
            described["portalContactEmail"], body["portalContactEmail"],
            "case {:?}: DescribePortal reported another contact email in {described}",
            case.label
        );
        assert_eq!(
            described["roleArn"], body["roleArn"],
            "case {:?}: DescribePortal reported another roleArn in {described}",
            case.label
        );
        assert_eq!(
            described["portalAuthMode"].as_str().unwrap_or(""),
            case.expected_auth_mode,
            "case {:?}: unexpected portalAuthMode in {described}",
            case.label
        );

        // Every optional member the create carried reads back as it was sent, and a
        // member it did not carry is reported by neither the create nor the read.
        for member in [
            "portalDescription",
            "notificationSenderEmail",
            "alarms",
            "portalType",
            "portalTypeConfiguration",
        ] {
            match body.get(member) {
                Some(sent) => assert_eq!(
                    &described[member], sent,
                    "case {:?}: {member} must read back as it was sent, got {described}",
                    case.label
                ),
                None => assert!(
                    described.get(member).is_none_or(Value::is_null),
                    "case {:?}: {member} was never sent, so nothing reports it: {described}",
                    case.label
                ),
            }
        }

        // A logo image file uploaded by the create becomes a location with a
        // modelled image id and a URL under the portal's own origin; a portal given
        // no logo reports no location at all.
        match body.get("portalLogoImageFile") {
            Some(_) => {
                let location = &described["portalLogoImageLocation"];
                assert!(
                    is_modelled_id(location["id"].as_str().unwrap_or("")),
                    "case {:?}: the logo location needs a modelled image id, got {described}",
                    case.label
                );
                let url = location["url"].as_str().unwrap_or("");
                assert!(
                    url.starts_with(&start_url) && url.len() <= 256,
                    "case {:?}: unexpected logo image URL in {described}",
                    case.label
                );
            }
            None => assert!(
                described
                    .get("portalLogoImageLocation")
                    .is_none_or(Value::is_null),
                "case {:?}: a portal with no logo reports no location: {described}",
                case.label
            ),
        }

        let client_id = described["portalClientId"].as_str().unwrap_or("<absent>");
        if case.identity_center {
            assert!(
                !client_id.is_empty() && client_id.len() <= 256,
                "case {:?}: an IAM Identity Center portal names a portalClientId, got {described}",
                case.label
            );
        } else {
            // "IoT SiteWise includes portalClientId for ONLY portals that use IAM
            // Identity Center to authenticate users."
            assert_eq!(
                client_id, "",
                "case {:?}: an IAM portal names no portalClientId, got {described}",
                case.label
            );
        }

        let creation = described["portalCreationDate"].as_f64().unwrap_or(0.0);
        let last_update = described["portalLastUpdateDate"].as_f64().unwrap_or(0.0);
        assert!(
            creation > 0.0,
            "case {:?}: portalCreationDate must be a real instant, got {described}",
            case.label
        );
        assert_eq!(
            creation, last_update,
            "case {:?}: an untouched portal's lastUpdateDate is its creationDate, got {described}",
            case.label
        );

        // Reading does not change what the next caller sees.
        let (status, again) = call(&router, "GET", &format!("{PORTALS}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: second DescribePortal expected 200, got {status} {again}",
            case.label
        );
        assert_eq!(
            again, described,
            "case {:?}: DescribePortal answered differently on the second read: {again}",
            case.label
        );

        // The tags the create carried are persisted on the portal. Asserted against
        // the store because this service's tagging operations are still scaffolds,
        // so there is no reader for them on the wire yet.
        let expected_tags: Vec<(String, String)> = body
            .get("tags")
            .and_then(Value::as_object)
            .map(|tags| {
                let mut pairs: Vec<(String, String)> = tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                    .collect();
                pairs.sort();
                pairs
            })
            .unwrap_or_default();
        let state = service_state(&*engine).expect("the iotsitewise store");
        let mut stored_tags: Vec<(String, String)> = state
            .lock()
            .portals
            .get(&id)
            .unwrap_or_else(|| panic!("case {:?}: portal {id} was not persisted", case.label))
            .tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        stored_tags.sort();
        assert_eq!(
            stored_tags, expected_tags,
            "case {:?}: unexpected stored tags for portal {id}",
            case.label
        );
    }

    assert_eq!(
        seen_ids.len(),
        create_cases().len(),
        "every row must have created a portal, got {seen_ids:?}"
    );
}

#[tokio::test]
async fn a_portal_settles_from_creating_to_active_once_the_owed_transition_falls_due() {
    let router = router();

    let (status, created) = call(
        &router,
        "POST",
        PORTALS,
        Some(json!({
            "portalName": "the-settling-portal",
            "portalContactEmail": "admin@example.com",
            "roleArn": role_arn("settling-role"),
        })),
    )
    .await;
    assert_eq!(status, 202, "the settling portal: got {status} {created}");
    let id = created["portalId"].as_str().unwrap_or("").to_string();

    // The span is non-zero, so a caller reading straight after the create still
    // observes the transient state the real service reports.
    let (status, described) = call(&router, "GET", &format!("{PORTALS}/{id}"), None).await;
    assert_eq!(status, 200, "the immediate read: got {status} {described}");
    assert_eq!(
        described["portalStatus"]["state"].as_str().unwrap_or(""),
        "CREATING",
        "immediately after the create the portal is CREATING, got {described}"
    );
    assert!(
        described["portalStatus"].get("error").is_none_or(Value::is_null),
        "a portal that failed nothing carries no error, got {described}"
    );

    tokio::time::sleep(ACTIVATION_DELAY + std::time::Duration::from_millis(50)).await;

    let (status, settled) = call(&router, "GET", &format!("{PORTALS}/{id}"), None).await;
    assert_eq!(status, 200, "the settled read: got {status} {settled}");
    assert_eq!(
        settled["portalStatus"]["state"].as_str().unwrap_or(""),
        "ACTIVE",
        "once the owed transition falls due the portal is ACTIVE, got {settled}"
    );
    // Settling is a transition the CLOCK owed: nothing else about the portal moved.
    assert_eq!(
        settled["portalLastUpdateDate"], described["portalLastUpdateDate"],
        "settling must not touch portalLastUpdateDate, got {settled}"
    );
}

#[tokio::test]
async fn describing_a_portal_that_was_never_created_is_refused() {
    let router = router();

    let (status, created) = call(
        &router,
        "POST",
        PORTALS,
        Some(json!({
            "portalName": "the-only-portal",
            "portalContactEmail": "admin@example.com",
            "roleArn": role_arn("the-only-role"),
        })),
    )
    .await;
    assert_eq!(status, 202, "the only portal: expected 202, got {status} {created}");
    let id = created["portalId"].as_str().unwrap_or("").to_string();

    // Well-formed ids no create ever minted: one all-f, one that differs from the
    // stored id in a single nibble.
    let mut neighbour: Vec<char> = id.chars().collect();
    let at = neighbour
        .iter()
        .position(|ch| *ch != '-' && *ch != '0')
        .expect("a non-zero character in the id");
    neighbour[at] = '0';
    let neighbour: String = neighbour.into_iter().collect();

    for case in ["ffffffff-ffff-ffff-ffff-ffffffffffff", neighbour.as_str()] {
        assert!(
            is_modelled_id(case),
            "case {case:?}: the probe id must itself be modelled"
        );
        let (status, body) = call(&router, "GET", &format!("{PORTALS}/{case}"), None).await;
        assert_eq!(status, 404, "case {case:?}: expected 404, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
        // The refusal names the identifier the REQUEST supplied, not the one portal
        // that does exist.
        let message = body["message"].as_str().unwrap_or("");
        assert!(
            message.contains(case) && !message.contains(&id),
            "case {case:?}: the refusal must name the requested id only, got {body}"
        );
    }

    // The refusals left the stored portal readable.
    let (status, described) = call(&router, "GET", &format!("{PORTALS}/{id}"), None).await;
    assert_eq!(
        status, 200,
        "the stored portal after the refusals: got {status} {described}"
    );
}

#[tokio::test]
async fn two_portals_may_share_a_name_and_stay_separate_resources() {
    let router = router();

    // `portalName` is "A friendly name for the portal" — not a unique one, unlike
    // `gatewayName` — so a second portal under the same name is a second resource
    // with an identity of its own rather than a conflict.
    let name = "the-shared-portal-name";
    let mut ids: Vec<String> = Vec::new();
    for case in ["first", "second"] {
        let (status, created) = call(
            &router,
            "POST",
            PORTALS,
            Some(json!({
                "portalName": name,
                "portalContactEmail": format!("{case}@example.com"),
                "roleArn": role_arn(&format!("shared-name-{case}")),
            })),
        )
        .await;
        assert_eq!(status, 202, "case {case:?}: expected 202, got {status} {created}");
        ids.push(created["portalId"].as_str().unwrap_or("").to_string());
    }
    assert_ne!(
        ids[0], ids[1],
        "two portals sharing a name are still two resources, got {ids:?}"
    );

    for (case, id) in ["first", "second"].iter().zip(ids.iter()) {
        let (status, described) = call(&router, "GET", &format!("{PORTALS}/{id}"), None).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {described}");
        assert_eq!(
            described["portalContactEmail"].as_str().unwrap_or(""),
            format!("{case}@example.com"),
            "case {case:?}: each portal keeps its own contact email, got {described}"
        );
    }
}

#[tokio::test]
async fn a_portal_request_outside_the_modelled_constraints_is_refused() {
    let router = router();

    // (case, body, expected status). Every row is a member the MODEL constrains:
    // an absent required member, an `AuthMode` outside the enum's value list, an
    // `ImageFileType` outside its single value, an email outside the modelled
    // pattern, and a portal name past the modelled 256-character length.
    let cases: Vec<(&str, Value)> = vec![
        (
            "no portalName",
            json!({
                "portalContactEmail": "admin@example.com",
                "roleArn": role_arn("no-name"),
            }),
        ),
        (
            "no roleArn",
            json!({
                "portalName": "no-role",
                "portalContactEmail": "admin@example.com",
            }),
        ),
        (
            "no portalContactEmail",
            json!({ "portalName": "no-email", "roleArn": role_arn("no-email") }),
        ),
        (
            "portalAuthMode outside the enum",
            json!({
                "portalName": "bad-auth-mode",
                "portalContactEmail": "admin@example.com",
                "roleArn": role_arn("bad-auth-mode"),
                "portalAuthMode": "SAML",
            }),
        ),
        (
            "portalType outside the enum",
            json!({
                "portalName": "bad-portal-type",
                "portalContactEmail": "admin@example.com",
                "roleArn": role_arn("bad-portal-type"),
                "portalType": "SITEWISE_PORTAL_V3",
            }),
        ),
        (
            "logo image of an unmodelled type",
            json!({
                "portalName": "bad-image-type",
                "portalContactEmail": "admin@example.com",
                "roleArn": role_arn("bad-image-type"),
                "portalLogoImageFile": { "data": "aVZCT1J3MEtHZ29BQUE=", "type": "JPEG" },
            }),
        ),
        (
            "portalContactEmail outside the pattern",
            json!({
                "portalName": "bad-email",
                "portalContactEmail": "not-an-email",
                "roleArn": role_arn("bad-email"),
            }),
        ),
        (
            "portalName past the modelled length",
            json!({
                "portalName": "n".repeat(257),
                "portalContactEmail": "admin@example.com",
                "roleArn": role_arn("long-name"),
            }),
        ),
    ];

    for (case, body) in cases {
        let (status, answer) = call(&router, "POST", PORTALS, Some(body)).await;
        // Only the STATUS is asserted for these rows: every one of them is refused
        // by the boundary validator before the handler runs, and the error CODE that
        // walk renders is the subject of its own issue (the same reason
        // `resource_tags.rs` records for a `TagMap` length violation). The
        // HANDLER's own refusals are code-asserted below.
        assert_eq!(
            status, 400,
            "case {case:?}: a request outside the model expected 400, got {status} {answer}"
        );
    }

    // Nothing was persisted by any of the refusals.
    let (status, body) = call(
        &router,
        "GET",
        &format!("{PORTALS}/ffffffff-ffff-ffff-ffff-ffffffffffff"),
        None,
    )
    .await;
    assert_eq!(status, 404, "no portal may exist after the refusals: {body}");
}

#[tokio::test]
async fn a_tag_key_outside_the_modelled_bounds_is_refused_by_the_handler() {
    let (router, engine) = router_with_engine();

    // `TagMap`'s KEYS target `TagKey` (@length 1..128), a bound the framework's
    // constraint walk does not reach — it validates a map's values only — so these
    // are the HANDLER's own refusals, answered with the client-input error this
    // operation models, and their error code is pinned. The rows walk the bound:
    // both published ends, and one character past each.
    let cases: Vec<(&str, String, bool)> = vec![
        ("the empty key, one below the minimum", String::new(), false),
        ("a key at the minimum length", "k".to_string(), true),
        (
            "a key at the maximum length",
            "k".repeat(TAG_KEY_MAX_LENGTH),
            true,
        ),
        (
            "a key one past the maximum length",
            "k".repeat(TAG_KEY_MAX_LENGTH + 1),
            false,
        ),
    ];

    for (case, key, accepted) in cases {
        let (status, answer) = call(
            &router,
            "POST",
            PORTALS,
            Some(json!({
                "portalName": format!("tag-key-{}", key.len()),
                "portalContactEmail": "admin@example.com",
                "roleArn": role_arn("tag-key-bound"),
                "tags": { key.clone(): "value" },
            })),
        )
        .await;

        if accepted {
            assert_eq!(
                status, 202,
                "case {case:?}: a key inside the bound is accepted, got {status} {answer}"
            );
            let id = answer["portalId"].as_str().unwrap_or("").to_string();
            let state = service_state(&*engine).expect("the iotsitewise store");
            let stored = state
                .lock()
                .portals
                .get(&id)
                .map(|portal| portal.tags.clone())
                .unwrap_or_else(|| panic!("case {case:?}: portal {id} was not persisted"));
            assert_eq!(
                stored.get(&key).map(String::as_str),
                Some("value"),
                "case {case:?}: the accepted key must be persisted, got {stored:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&answer)),
                (400, "InvalidRequestException"),
                "case {case:?}: a key outside the bound must be refused, got {status} {answer}"
            );
            // A refused create stores nothing.
            let state = service_state(&*engine).expect("the iotsitewise store");
            let stored_names: Vec<String> = state
                .lock()
                .portals
                .values()
                .map(|portal| portal.name.clone())
                .collect();
            assert!(
                !stored_names.contains(&format!("tag-key-{}", key.len())),
                "case {case:?}: a refused create must persist nothing, got {stored_names:?}"
            );
        }
    }
}
