// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UpdatePortal`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * an accepted update WRITES the portal the request addresses from the members
//!   that request carries — every caller-owned member is replaced by what was sent
//!   and one the request omits is cleared — while the portal's identity and
//!   provenance (its id, ARN, start URL, authentication mode and the identifiers
//!   that go with it, its tags and its creation date) stay where they were, and
//!   `DescribePortal` reads the new state back;
//! * `portalLogoImage` is a choice: a file uploads a NEW image, an id keeps the one
//!   the portal already holds, an id naming no image of that portal is a modelled
//!   404, and naming both alternatives or neither is a modelled 400;
//! * the portal answers `UPDATING` until the transition the call owes falls due and
//!   `ACTIVE` from then on, and settling moves nothing else about it;
//! * an id that resolves to no stored portal is a modelled 404 and writes nothing;
//! * a `clientToken` makes the call idempotent: the same request under the same
//!   token is a retry answered from what the first call left, and a different
//!   request reusing that token is the modelled 409.
//!
//! The HTTP harness (`call`, `router`, `error_code`, `error_message`,
//! `is_modelled_id`) is the one the asset and refusal suites already define; this
//! suite reuses it rather than growing a second.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{service_state, ACTIVATION_DELAY};

use crate::asset_lifecycle::{call, error_code, is_modelled_id, router, router_with_engine};
use crate::hierarchies_etags_and_refusals::error_message;

/// Where every portal request goes, as `OPERATIONS.md` reports the route:
/// `POST /portals`, `GET /portals/{portalId}`, `PUT /portals/{portalId}`.
const PORTALS: &str = "/portals";

/// A `portalId`-shaped value no portal in these suites is ever created under, and a
/// second one for the image rows. Both are within the modelled `ID` pattern.
const UNKNOWN_ID: &str = "11111111-2222-4333-8444-555555555555";
const OTHER_UNKNOWN_ID: &str = "66666666-7777-4888-8999-aaaaaaaaaaaa";

/// A base64-encoded `ImageFile$data` blob, and a second, DIFFERENT one, so an
/// upload can be told from a re-upload. `ImageFileType` admits exactly one value,
/// `PNG` (smithy_query query="shape" shape="ImageFileType").
const LOGO_DATA: &str = "aVZCT1J3MEtHZ29BQUE=";
const OTHER_LOGO_DATA: &str = "aVZCT1J3MEtHZ29CQkI=";

/// Comfortably past [`ACTIVATION_DELAY`], so the transition an accepted call owes
/// has fallen due by the time the next call lands.
fn settled() -> Duration {
    ACTIVATION_DELAY + Duration::from_millis(200)
}

/// A service-role ARN within the modelled `IamArn` pattern.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::123456789012:role/{name}")
}

/// A `ClientToken`-shaped value built from `seed`.
///
/// Source: `UpdatePortal`'s own `clientToken` member — `smithy_query(query =
/// "operation", operation = "UpdatePortal")` reports target `ClientToken`,
/// `length 36..64`, `pattern ^\S{36,64}$` — so a short, readable seed is padded with
/// a non-whitespace filler up to the minimum.
fn client_token(seed: &str) -> String {
    let token = format!("{seed:-<36}");
    assert!(
        (36..=64).contains(&token.len()),
        "case {seed:?}: a ClientToken is 36..64 characters, built {token:?}"
    );
    token
}

/// An upload of `data` as the `portalLogoImage` of an update.
fn uploaded_logo(data: &str) -> Value {
    json!({ "file": { "data": data, "type": "PNG" } })
}

/// Create a portal carrying EVERY caller-owned member, so each row of the table
/// below can show a member being replaced or cleared, and return
/// `(portalId, the create's answer, the read that followed it)`.
async fn create_full_portal(router: &MultiServiceRouter, label: &str) -> (String, Value, Value) {
    let (status, created) = call(
        router,
        "POST",
        PORTALS,
        Some(json!({
            "portalName": format!("portal-{label}"),
            "portalContactEmail": "created@example.com",
            "roleArn": role_arn("created-service-role"),
            "portalDescription": "as created",
            "notificationSenderEmail": "created-sender@example.com",
            "alarms": {
                "alarmRoleArn": role_arn("created-alarm-role"),
                "notificationLambdaArn":
                    "arn:aws:lambda:us-west-2:123456789012:function:created-alarms",
            },
            "portalType": "SITEWISE_PORTAL_V1",
            "portalTypeConfiguration": {
                "SITEWISE_PORTAL_V1": { "portalTools": ["DASHBOARD"] },
            },
            "portalLogoImageFile": { "data": LOGO_DATA, "type": "PNG" },
            "tags": { "stage": "test" },
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {label:?}: CreatePortal expected 202, got {status} {created}"
    );
    let id = created["portalId"].as_str().unwrap_or("").to_string();
    let (status, described) = call(router, "GET", &format!("{PORTALS}/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {label:?}: DescribePortal expected 200, got {status} {described}"
    );
    (id, created, described)
}

/// What the row expects of the portal's logo image after its update.
#[derive(Debug)]
enum Logo {
    /// The update carries no `portalLogoImage`, so the portal is left holding no
    /// image: the request states the portal's new state in full.
    Omitted,
    /// The update uploads a file, so the portal carries a NEW image — a different
    /// one from the image the create gave it.
    Uploaded,
}

/// One row of the update table: the members an update sends beyond the three
/// `UpdatePortal` requires, and what it expects of the logo image.
#[derive(Debug)]
struct UpdateCase {
    label: &'static str,
    extra: Value,
    logo: Logo,
}

/// The rows. They differ along every dimension `UpdatePortal`'s own input has: the
/// optional members carried (none at all, one, all of them), the `PortalType` value
/// (`SITEWISE_PORTAL_V1` | `SITEWISE_PORTAL_V2`, the whole modelled value list —
/// smithy_query query="shape" shape="PortalType"), the number of tools a
/// portal-type entry names, alarms with and without the optional notification
/// Lambda, and whether a logo image is uploaded or left behind.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            label: "required-members-only",
            extra: json!({}),
            logo: Logo::Omitted,
        },
        UpdateCase {
            label: "description-only",
            extra: json!({ "portalDescription": "the floor-two monitor portal" }),
            logo: Logo::Omitted,
        },
        UpdateCase {
            label: "alarms-without-notification-lambda",
            extra: json!({
                "alarms": { "alarmRoleArn": role_arn("updated-alarm-role") },
            }),
            logo: Logo::Omitted,
        },
        UpdateCase {
            label: "classic-type-with-one-tool",
            extra: json!({
                "portalType": "SITEWISE_PORTAL_V1",
                "portalTypeConfiguration": {
                    "SITEWISE_PORTAL_V1": { "portalTools": ["DASHBOARD"] },
                },
            }),
            logo: Logo::Omitted,
        },
        UpdateCase {
            label: "ai-aware-type-with-two-tools-and-sender-email",
            extra: json!({
                "portalType": "SITEWISE_PORTAL_V2",
                "portalTypeConfiguration": {
                    "SITEWISE_PORTAL_V2": { "portalTools": ["ASSISTANT", "DASHBOARD"] },
                },
                "notificationSenderEmail": "updated-sender@example.com",
            }),
            logo: Logo::Omitted,
        },
        UpdateCase {
            label: "new-logo-image-file",
            extra: json!({ "portalLogoImage": uploaded_logo(OTHER_LOGO_DATA) }),
            logo: Logo::Uploaded,
        },
        UpdateCase {
            label: "every-optional-member-at-once",
            extra: json!({
                "portalDescription": "everything at once",
                "notificationSenderEmail": "everything@example.com",
                "alarms": {
                    "alarmRoleArn": role_arn("everything-alarm-role"),
                    "notificationLambdaArn":
                        "arn:aws:lambda:us-west-2:123456789012:function:everything",
                },
                "portalType": "SITEWISE_PORTAL_V2",
                "portalTypeConfiguration": {
                    "SITEWISE_PORTAL_V2": { "portalTools": ["ASSISTANT"] },
                },
                "portalLogoImage": uploaded_logo(LOGO_DATA),
            }),
            logo: Logo::Uploaded,
        },
    ]
}

/// The body one row sends: the three members `UpdatePortal` requires, each a value
/// the create did NOT use so the write can be told from the state it replaced, plus
/// the row's own extras.
fn update_body(case: &UpdateCase) -> Value {
    let mut body = json!({
        "portalName": format!("updated-{}", case.label),
        "portalContactEmail": "updated@example.com",
        "roleArn": role_arn(&format!("updated-role-{}", case.label)),
    });
    let object = body.as_object_mut().expect("an object body");
    for (member, value) in case.extra.as_object().expect("an object of extras") {
        object.insert(member.clone(), value.clone());
    }
    body
}

/// The optional members of `UpdatePortal`'s input that `DescribePortal` reports back
/// under the same name. Source: `smithy_query(query = "operation", operation =
/// "UpdatePortal")` beside `DescribePortal`'s output.
const MIRRORED_OPTIONAL_MEMBERS: [&str; 5] = [
    "portalDescription",
    "notificationSenderEmail",
    "alarms",
    "portalType",
    "portalTypeConfiguration",
];

/// Whatever an update may not touch: a portal's identity and provenance.
fn assert_identity_held(label: &str, created: &Value, described: &Value, after: &Value) {
    for member in [
        "portalId",
        "portalArn",
        "portalStartUrl",
        "portalAuthMode",
        "portalClientId",
        "portalCreationDate",
    ] {
        assert_eq!(
            after[member], described[member],
            "case {label:?}: {member} is not an update's to change, got {after}"
        );
    }
    assert_eq!(
        after["portalArn"], created["portalArn"],
        "case {label:?}: the create's ARN must survive the update, got {after}"
    );
}

#[tokio::test]
async fn an_update_writes_every_caller_owned_member_and_clears_the_ones_it_omits() {
    let (router, engine) = router_with_engine();

    for case in update_cases() {
        let (id, created, described) = create_full_portal(&router, case.label).await;
        let created_logo_id = described["portalLogoImageLocation"]["id"]
            .as_str()
            .unwrap_or("")
            .to_string();
        assert!(
            is_modelled_id(&created_logo_id),
            "case {:?}: the created portal must carry a logo image, got {described}",
            case.label
        );

        let body = update_body(&case);
        let (status, answered) = call(
            &router,
            "PUT",
            &format!("{PORTALS}/{id}"),
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {:?}: UpdatePortal expected 202, got {status} {answered}",
            case.label
        );
        // "The status of the portal, which contains a state (UPDATING after
        // successfully calling this operation) and any error message."
        assert_eq!(
            answered["portalStatus"]["state"].as_str().unwrap_or(""),
            "UPDATING",
            "case {:?}: an accepted update answers UPDATING, got {answered}",
            case.label
        );
        assert!(
            answered["portalStatus"]
                .get("error")
                .is_none_or(Value::is_null),
            "case {:?}: an update that failed nothing carries no error, got {answered}",
            case.label
        );

        let (status, after) = call(&router, "GET", &format!("{PORTALS}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribePortal expected 200, got {status} {after}",
            case.label
        );

        // The three required members are what the request sent, not what the create
        // left behind.
        for member in ["portalName", "portalContactEmail", "roleArn"] {
            assert_eq!(
                &after[member], &body[member],
                "case {:?}: {member} must read back as the update sent it, got {after}",
                case.label
            );
            assert_ne!(
                after[member], described[member],
                "case {:?}: {member} still reports the created value, got {after}",
                case.label
            );
        }

        // Every optional member the update carried reads back as it was sent; one it
        // omitted is CLEARED, even where the create had set it.
        for member in MIRRORED_OPTIONAL_MEMBERS {
            match body.get(member) {
                Some(sent) => assert_eq!(
                    &after[member], sent,
                    "case {:?}: {member} must read back as the update sent it, got {after}",
                    case.label
                ),
                None => assert!(
                    after.get(member).is_none_or(Value::is_null),
                    "case {:?}: {member} was not sent, so the update cleared it: {after}",
                    case.label
                ),
            }
        }

        // The logo image follows the same rule, through the `Image` choice.
        match case.logo {
            Logo::Omitted => assert!(
                after
                    .get("portalLogoImageLocation")
                    .is_none_or(Value::is_null),
                "case {:?}: an update naming no image leaves the portal none: {after}",
                case.label
            ),
            Logo::Uploaded => {
                let location = &after["portalLogoImageLocation"];
                let uploaded = location["id"].as_str().unwrap_or("");
                assert!(
                    is_modelled_id(uploaded),
                    "case {:?}: an uploaded image needs a modelled id, got {after}",
                    case.label
                );
                assert_ne!(
                    uploaded, created_logo_id,
                    "case {:?}: an uploaded image is a NEW image, got {after}",
                    case.label
                );
                let url = location["url"].as_str().unwrap_or("");
                let start_url = after["portalStartUrl"].as_str().unwrap_or("");
                assert!(
                    url.starts_with(start_url) && url.contains(uploaded) && url.len() <= 256,
                    "case {:?}: unexpected logo image URL in {after}",
                    case.label
                );
            }
        }

        assert_identity_held(case.label, &created, &described, &after);

        // The write moved the portal's last-update instant forward from where the
        // create left it, and left the creation instant where it was.
        let creation = after["portalCreationDate"].as_f64().unwrap_or(0.0);
        let last_update = after["portalLastUpdateDate"].as_f64().unwrap_or(0.0);
        let created_last_update = described["portalLastUpdateDate"].as_f64().unwrap_or(0.0);
        assert_eq!(
            after["portalCreationDate"], described["portalCreationDate"],
            "case {:?}: an update does not move portalCreationDate, got {after}",
            case.label
        );
        assert!(
            creation > 0.0 && last_update > created_last_update,
            "case {:?}: the update must move portalLastUpdateDate past the create's \
             {created_last_update}, got {after}",
            case.label
        );

        // Reading the updated portal does not change what the next caller sees.
        let (status, again) = call(&router, "GET", &format!("{PORTALS}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: second DescribePortal expected 200, got {status} {again}",
            case.label
        );
        assert_eq!(
            again["portalLastUpdateDate"], after["portalLastUpdateDate"],
            "case {:?}: a read must not move the portal on, got {again}",
            case.label
        );

        // The tags the create carried are provenance, not something this call
        // writes. Asserted against the store, the way the create suite does it.
        let state = service_state(&*engine).expect("the iotsitewise store");
        let tags: Vec<(String, String)> = state
            .lock()
            .portals
            .get(&id)
            .unwrap_or_else(|| panic!("case {:?}: portal {id} was not persisted", case.label))
            .tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        assert_eq!(
            tags,
            vec![("stage".to_string(), "test".to_string())],
            "case {:?}: an update is not the call that writes a portal's tags",
            case.label
        );
    }
}

#[tokio::test]
async fn an_updated_portal_settles_from_updating_to_active_once_the_owed_transition_falls_due() {
    let router = router();
    let (id, _, _) = create_full_portal(&router, "settling").await;

    // Past the create's own owed transition, so the state the read reports is the
    // one the UPDATE owes rather than the one the create did.
    tokio::time::sleep(settled()).await;

    let (status, answered) = call(
        &router,
        "PUT",
        &format!("{PORTALS}/{id}"),
        Some(json!({
            "portalName": "the-settling-portal",
            "portalContactEmail": "settling@example.com",
            "roleArn": role_arn("settling-role"),
        })),
    )
    .await;
    assert_eq!(status, 202, "the update: got {status} {answered}");
    assert_eq!(
        answered["portalStatus"]["state"].as_str().unwrap_or(""),
        "UPDATING",
        "an accepted update answers UPDATING, got {answered}"
    );

    // The span is non-zero, so a caller reading straight after the update still
    // observes the transient state the real service reports.
    let (status, updating) = call(&router, "GET", &format!("{PORTALS}/{id}"), None).await;
    assert_eq!(status, 200, "the immediate read: got {status} {updating}");
    assert_eq!(
        updating["portalStatus"]["state"].as_str().unwrap_or(""),
        "UPDATING",
        "immediately after the update the portal is UPDATING, got {updating}"
    );

    tokio::time::sleep(settled()).await;

    let (status, active) = call(&router, "GET", &format!("{PORTALS}/{id}"), None).await;
    assert_eq!(status, 200, "the settled read: got {status} {active}");
    assert_eq!(
        active["portalStatus"]["state"].as_str().unwrap_or(""),
        "ACTIVE",
        "once the owed transition falls due the portal is ACTIVE, got {active}"
    );
    // Settling is a transition the CLOCK owed: nothing else about the portal moved.
    assert_eq!(
        active["portalLastUpdateDate"], updating["portalLastUpdateDate"],
        "settling must not touch portalLastUpdateDate, got {active}"
    );
    assert_eq!(
        active["portalName"], updating["portalName"],
        "settling must not touch the members the update wrote, got {active}"
    );
}

#[tokio::test]
async fn an_update_keeps_the_portals_own_image_and_refuses_any_other_choice() {
    let router = router();
    let (id, _, described) = create_full_portal(&router, "images").await;
    let location = described["portalLogoImageLocation"].clone();
    let image_id = location["id"].as_str().unwrap_or("").to_string();

    // "The ID of an existing image. Choose this option to keep an existing image."
    let (status, answered) = call(
        &router,
        "PUT",
        &format!("{PORTALS}/{id}"),
        Some(json!({
            "portalName": "the-portal-that-keeps-its-image",
            "portalContactEmail": "images@example.com",
            "roleArn": role_arn("images-role"),
            "portalLogoImage": { "id": image_id },
        })),
    )
    .await;
    assert_eq!(status, 202, "keeping the image: got {status} {answered}");

    let (status, kept) = call(&router, "GET", &format!("{PORTALS}/{id}"), None).await;
    assert_eq!(status, 200, "the read after keeping: got {status} {kept}");
    assert_eq!(
        kept["portalLogoImageLocation"], location,
        "a kept image is the same image, reported identically, got {kept}"
    );

    // Each row is a `portalLogoImage` that names no single image of this portal:
    // the id of an image nothing holds, both alternatives at once, and neither.
    let rows: Vec<(&str, Value, u16, &str, String)> = vec![
        (
            "an image id this portal does not hold",
            json!({ "id": OTHER_UNKNOWN_ID }),
            404,
            "ResourceNotFoundException",
            format!("Image with ID {OTHER_UNKNOWN_ID} not found"),
        ),
        (
            "both alternatives at once",
            json!({ "id": image_id, "file": { "data": LOGO_DATA, "type": "PNG" } }),
            400,
            "InvalidRequestException",
            "portalLogoImage must name exactly one of id, file; this request named 2.".to_string(),
        ),
        (
            "neither alternative",
            json!({}),
            400,
            "InvalidRequestException",
            "portalLogoImage must name exactly one of id, file; this request named 0.".to_string(),
        ),
    ];

    for (case, image, expected_status, expected_code, expected_message) in rows {
        let (status, answered) = call(
            &router,
            "PUT",
            &format!("{PORTALS}/{id}"),
            Some(json!({
                "portalName": "the-refused-update",
                "portalContactEmail": "refused@example.com",
                "roleArn": role_arn("refused-role"),
                "portalLogoImage": image,
            })),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            expected_code,
            "case {case:?}: expected {expected_code}, got {answered}"
        );
        assert_eq!(
            error_message(&answered),
            expected_message,
            "case {case:?}: expected {expected_message:?}, got {:?}",
            error_message(&answered)
        );

        // A refused update writes NOTHING: the portal still reads as the accepted
        // update left it.
        let (status, unchanged) = call(&router, "GET", &format!("{PORTALS}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the read after the refusal: got {status} {unchanged}"
        );
        assert_eq!(
            unchanged["portalName"], kept["portalName"],
            "case {case:?}: a refused update must leave the portal alone, got {unchanged}"
        );
        assert_eq!(
            unchanged["portalLogoImageLocation"], location,
            "case {case:?}: a refused update must leave the image alone, got {unchanged}"
        );
        assert_eq!(
            unchanged["portalLastUpdateDate"], kept["portalLastUpdateDate"],
            "case {case:?}: a refused update must not move the portal on, got {unchanged}"
        );
    }
}

#[tokio::test]
async fn updating_a_portal_that_was_never_created_is_refused() {
    let (router, engine) = router_with_engine();
    let (id, _, described) = create_full_portal(&router, "the-only-portal").await;

    let (status, answered) = call(
        &router,
        "PUT",
        &format!("{PORTALS}/{UNKNOWN_ID}"),
        Some(json!({
            "portalName": "a-portal-that-is-not-there",
            "portalContactEmail": "absent@example.com",
            "roleArn": role_arn("absent-role"),
        })),
    )
    .await;
    assert_eq!(status, 404, "an absent portal is a 404, got {status} {answered}");
    assert_eq!(
        error_code(&answered),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {answered}"
    );
    assert_eq!(
        error_message(&answered),
        format!("Portal with ID {UNKNOWN_ID} not found"),
        "unexpected refusal message in {answered}"
    );

    // Nothing was created for the id the refused request named, and the portal that
    // does exist is untouched.
    let state = service_state(&*engine).expect("the iotsitewise store");
    let ids: Vec<String> = state.lock().portals.keys().cloned().collect();
    assert_eq!(
        ids,
        vec![id.clone()],
        "a refused update must not add a portal, got {ids:?}"
    );
    let (status, after) = call(&router, "GET", &format!("{PORTALS}/{id}"), None).await;
    assert_eq!(status, 200, "the read after the refusal: got {status} {after}");
    assert_eq!(
        after["portalName"], described["portalName"],
        "a refused update must leave the stored portal alone, got {after}"
    );
}

#[tokio::test]
async fn a_retried_update_replays_the_answer_and_a_reused_token_is_refused() {
    let router = router();
    let (id, _, _) = create_full_portal(&router, "idempotency").await;
    tokio::time::sleep(settled()).await;

    let token = client_token("portal-update-retry");
    let body = json!({
        "portalName": "the-retried-portal",
        "portalContactEmail": "retry@example.com",
        "roleArn": role_arn("retry-role"),
        "portalDescription": "written once",
        "clientToken": token,
    });

    let (status, first) = call(
        &router,
        "PUT",
        &format!("{PORTALS}/{id}"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, 202, "the first update: got {status} {first}");
    assert_eq!(
        first["portalStatus"]["state"].as_str().unwrap_or(""),
        "UPDATING",
        "the first update answers UPDATING, got {first}"
    );

    let (status, written) = call(&router, "GET", &format!("{PORTALS}/{id}"), None).await;
    assert_eq!(status, 200, "the read after the write: got {status} {written}");

    // The retry is answered from what the first call left behind and restarts no
    // transition: once the first call's own transition has fallen due, the retry
    // reports the portal ACTIVE rather than UPDATING again.
    tokio::time::sleep(settled()).await;
    let (status, retried) = call(
        &router,
        "PUT",
        &format!("{PORTALS}/{id}"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, 202, "the retry: got {status} {retried}");
    assert_eq!(
        retried["portalStatus"]["state"].as_str().unwrap_or(""),
        "ACTIVE",
        "a retry restarts no transition, got {retried}"
    );

    let (status, after_retry) = call(&router, "GET", &format!("{PORTALS}/{id}"), None).await;
    assert_eq!(
        status, 200,
        "the read after the retry: got {status} {after_retry}"
    );
    assert_eq!(
        after_retry["portalLastUpdateDate"], written["portalLastUpdateDate"],
        "a retry writes nothing further, got {after_retry}"
    );
    assert_eq!(
        after_retry["portalName"], written["portalName"],
        "a retry writes nothing further, got {after_retry}"
    );

    // A DIFFERENT request reusing that token belongs to another call: the modelled
    // conflict, naming the portal the first call acted on, and it writes nothing.
    let mut changed = body.clone();
    changed["portalDescription"] = json!("written twice");
    let (status, refused) = call(
        &router,
        "PUT",
        &format!("{PORTALS}/{id}"),
        Some(changed),
    )
    .await;
    assert_eq!(status, 409, "the reused token: got {status} {refused}");
    assert_eq!(
        error_code(&refused),
        "ConflictingOperationException",
        "expected ConflictingOperationException, got {refused}"
    );
    assert_eq!(
        refused["resourceId"].as_str().unwrap_or(""),
        id,
        "the conflict names the portal the first call acted on, got {refused}"
    );

    let (status, unchanged) = call(&router, "GET", &format!("{PORTALS}/{id}"), None).await;
    assert_eq!(
        status, 200,
        "the read after the conflict: got {status} {unchanged}"
    );
    assert_eq!(
        unchanged["portalDescription"], written["portalDescription"],
        "a refused update must leave the portal alone, got {unchanged}"
    );

    // A request carrying NO token is a new call every time, so it writes again.
    let mut untokened = body.clone();
    untokened
        .as_object_mut()
        .expect("an object body")
        .remove("clientToken");
    untokened["portalDescription"] = json!("written without a token");
    let (status, served) = call(
        &router,
        "PUT",
        &format!("{PORTALS}/{id}"),
        Some(untokened),
    )
    .await;
    assert_eq!(status, 202, "the untokened update: got {status} {served}");
    let (status, rewritten) = call(&router, "GET", &format!("{PORTALS}/{id}"), None).await;
    assert_eq!(
        status, 200,
        "the read after the untokened update: got {status} {rewritten}"
    );
    assert_eq!(
        rewritten["portalDescription"].as_str().unwrap_or(""),
        "written without a token",
        "a request with no token is a new call, got {rewritten}"
    );
}

#[tokio::test]
async fn a_retry_is_recognised_by_what_it_asks_for_not_by_how_it_was_written() {
    let router = router();
    let (id, _, _) = create_full_portal(&router, "fingerprint").await;
    tokio::time::sleep(settled()).await;

    // The two members a request is recognised by that have no single textual form:
    // a MAP, which has no order of its own, and an uploaded image, whose bytes are
    // what two uploads differ in.
    let token = client_token("portal-update-fingerprint");
    let first = json!({
        "portalName": "the-fingerprinted-portal",
        "portalContactEmail": "fingerprint@example.com",
        "roleArn": role_arn("fingerprint-role"),
        "portalType": "SITEWISE_PORTAL_V2",
        "portalTypeConfiguration": {
            "SITEWISE_PORTAL_V1": { "portalTools": ["DASHBOARD"] },
            "SITEWISE_PORTAL_V2": { "portalTools": ["ASSISTANT", "DASHBOARD"] },
        },
        "portalLogoImage": uploaded_logo(LOGO_DATA),
        "clientToken": token,
    });
    let (status, answered) = call(
        &router,
        "PUT",
        &format!("{PORTALS}/{id}"),
        Some(first.clone()),
    )
    .await;
    assert_eq!(status, 202, "the first update: got {status} {answered}");
    let (status, written) = call(&router, "GET", &format!("{PORTALS}/{id}"), None).await;
    assert_eq!(status, 200, "the read after the write: got {status} {written}");
    let image = written["portalLogoImageLocation"].clone();

    // The SAME request, with the configuration entries written in the other order
    // and the same image uploaded again: it asks for exactly what the first call
    // did, so it is a retry and writes nothing further.
    let reordered = json!({
        "portalName": "the-fingerprinted-portal",
        "portalContactEmail": "fingerprint@example.com",
        "roleArn": role_arn("fingerprint-role"),
        "portalType": "SITEWISE_PORTAL_V2",
        "portalTypeConfiguration": {
            "SITEWISE_PORTAL_V2": { "portalTools": ["ASSISTANT", "DASHBOARD"] },
            "SITEWISE_PORTAL_V1": { "portalTools": ["DASHBOARD"] },
        },
        "portalLogoImage": uploaded_logo(LOGO_DATA),
        "clientToken": token,
    });
    let (status, replayed) = call(
        &router,
        "PUT",
        &format!("{PORTALS}/{id}"),
        Some(reordered),
    )
    .await;
    assert_eq!(status, 202, "the reordered retry: got {status} {replayed}");
    let (status, after_replay) = call(&router, "GET", &format!("{PORTALS}/{id}"), None).await;
    assert_eq!(
        status, 200,
        "the read after the reordered retry: got {status} {after_replay}"
    );
    assert_eq!(
        after_replay["portalLastUpdateDate"], written["portalLastUpdateDate"],
        "a map written in another order is the same request, got {after_replay}"
    );
    assert_eq!(
        after_replay["portalLogoImageLocation"], image,
        "a retry must not mint a second image, got {after_replay}"
    );

    // A request uploading DIFFERENT bytes under that token asks for something else,
    // so it is the modelled conflict rather than a retry — and writes nothing.
    let other_image = json!({
        "portalName": "the-fingerprinted-portal",
        "portalContactEmail": "fingerprint@example.com",
        "roleArn": role_arn("fingerprint-role"),
        "portalType": "SITEWISE_PORTAL_V2",
        "portalTypeConfiguration": {
            "SITEWISE_PORTAL_V1": { "portalTools": ["DASHBOARD"] },
            "SITEWISE_PORTAL_V2": { "portalTools": ["ASSISTANT", "DASHBOARD"] },
        },
        "portalLogoImage": uploaded_logo(OTHER_LOGO_DATA),
        "clientToken": token,
    });
    let (status, refused) = call(
        &router,
        "PUT",
        &format!("{PORTALS}/{id}"),
        Some(other_image),
    )
    .await;
    assert_eq!(
        status, 409,
        "a different image under a spent token: got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ConflictingOperationException",
        "expected ConflictingOperationException, got {refused}"
    );
    // The conflict names the portal the first call acted on, by the ARN that portal
    // actually carries.
    assert_eq!(
        refused["resourceId"].as_str().unwrap_or(""),
        id,
        "the conflict names the portal the first call acted on, got {refused}"
    );
    assert_eq!(
        refused["resourceArn"].as_str().unwrap_or(""),
        written["portalArn"].as_str().unwrap_or(""),
        "the conflict names the stored portal's own ARN, got {refused}"
    );

    let (status, unchanged) = call(&router, "GET", &format!("{PORTALS}/{id}"), None).await;
    assert_eq!(
        status, 200,
        "the read after the conflict: got {status} {unchanged}"
    );
    assert_eq!(
        unchanged["portalLogoImageLocation"], image,
        "a refused update must leave the image alone, got {unchanged}"
    );
}
