// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for DataZone user profiles.
//!
//! Every case drives the service the way a client does — a real `restJson1` request
//! through the router, with only the response body as evidence — and reuses the
//! `Emulator` harness from [`crate::harness`], so nothing here reaches into
//! `crate::state` and a handler answering from a hardcoded value would fail.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. `CreateUserProfile` stores the profile a domain holds for the identity the
//!    request names and reports it, so `GetUserProfile` answers from the stored record —
//!    addressed either by that identity or by the `UserProfileId` the create minted.
//! 2. What the profile reports is DERIVED from the request: `type` from `userType` (or,
//!    when the request names none, from the form of the identifier), `details` under the
//!    union member that type names, and `status` as the member of that type's status
//!    pair the created profile holds.
//! 3. The identity IS the profile, so a repeat of the request reports the profile the
//!    first one created rather than a second one (the model marks `CreateUserProfile`
//!    `@idempotent`) — including a repeat that names another `userType` — while a
//!    different session name is a different identity.
//! 4. A profile is scoped to its domain and disappears with it; a domain this emulator
//!    serves none of denies the call, and an identity it holds no profile for is not
//!    found.

use crate::harness::Emulator;
use serde_json::{json, Value};
// The enum members as the service stores them, so a drift between these constants and
// the values on the wire fails here.
use service_com_amazonaws_datazone_datazone::state::{
    USER_PROFILE_STATUS_ACTIVATED, USER_PROFILE_STATUS_ASSIGNED, USER_PROFILE_TYPE_IAM,
    USER_PROFILE_TYPE_SSO,
};

/// The account every ARN this emulator builds carries, so a derived IAM ARN is checked
/// against the emulator's own account rather than a literal from a recording.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// `UserProfileId`'s modelled pattern,
/// `^([0-9a-f]{10}-|)[A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{12}$`
/// (smithy_query shape="UserProfileId"), checked here independently of the service's own
/// helper: five groups of hex digits of 8-4-4-4-12, after an optional lowercase-hex
/// group of 10.
fn is_user_profile_id(id: &str) -> bool {
    let mut groups: Vec<&str> = id.split('-').collect();
    if groups.len() == 6 {
        let prefix = groups.remove(0);
        if prefix.len() != 10
            || !prefix
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        {
            return false;
        }
    }
    groups.len() == 5
        && groups
            .iter()
            .zip([8usize, 4, 4, 4, 12])
            .all(|(group, len)| group.len() == len && group.chars().all(|c| c.is_ascii_hexdigit()))
}

/// One `CreateUserProfile` request and the profile the rules say it produces.
struct ProfileCase {
    label: &'static str,
    /// The request body: `userIdentifier` plus the optional `userType`/`sessionName`.
    body: Value,
    /// The `UserProfileType` the profile must report.
    profile_type: &'static str,
    /// The `UserProfileStatus` the profile must report.
    status: &'static str,
    /// The `details` the profile must report, as the union is rendered on the wire.
    details: Value,
}

/// The cases span what the rules generalise over: every `UserType` member and a request
/// that names none, and all three alternatives of `UserIdentifier` — an IAM ARN, a
/// principal name, and an identity-store user id with and without its leading group.
///
/// SOURCE for the members and the identifier forms:
/// `smithy_query(query="shape", shape="UserType,UserProfileType,UserProfileStatus,UserIdentifier,UserProfileDetails")`.
fn profile_cases() -> Vec<ProfileCase> {
    let account = account();
    vec![
        ProfileCase {
            label: "IAM_USER named by principal name",
            body: json!({ "userIdentifier": "data-analyst", "userType": "IAM_USER" }),
            profile_type: USER_PROFILE_TYPE_IAM,
            status: USER_PROFILE_STATUS_ACTIVATED,
            details: json!({
                "iam": { "arn": format!("arn:aws:iam::{account}:user/data-analyst") }
            }),
        },
        ProfileCase {
            label: "IAM_USER named by ARN",
            body: json!({
                "userIdentifier": format!("arn:aws:iam::{account}:user/finance.reader"),
                "userType": "IAM_USER",
            }),
            profile_type: USER_PROFILE_TYPE_IAM,
            status: USER_PROFILE_STATUS_ACTIVATED,
            details: json!({
                "iam": { "arn": format!("arn:aws:iam::{account}:user/finance.reader") }
            }),
        },
        ProfileCase {
            label: "IAM_ROLE named by role name",
            body: json!({ "userIdentifier": "DataZoneConsumer", "userType": "IAM_ROLE" }),
            profile_type: USER_PROFILE_TYPE_IAM,
            status: USER_PROFILE_STATUS_ACTIVATED,
            details: json!({
                "iam": { "arn": format!("arn:aws:iam::{account}:role/DataZoneConsumer") }
            }),
        },
        ProfileCase {
            label: "IAM_ROLE named by ARN",
            body: json!({
                "userIdentifier": format!("arn:aws:iam::{account}:role/service-role/Producer"),
                "userType": "IAM_ROLE",
            }),
            profile_type: USER_PROFILE_TYPE_IAM,
            status: USER_PROFILE_STATUS_ACTIVATED,
            details: json!({
                "iam": { "arn": format!("arn:aws:iam::{account}:role/service-role/Producer") }
            }),
        },
        ProfileCase {
            label: "IAM_ROLE_SESSION carries the session name",
            body: json!({
                "userIdentifier": format!("arn:aws:iam::{account}:role/Analyst"),
                "userType": "IAM_ROLE_SESSION",
                // `sessionName` is 2..64 (smithy_query operation="CreateUserProfile").
                "sessionName": "nightly-load",
            }),
            profile_type: USER_PROFILE_TYPE_IAM,
            status: USER_PROFILE_STATUS_ACTIVATED,
            details: json!({
                "iam": {
                    "arn": format!("arn:aws:iam::{account}:role/Analyst"),
                    "sessionName": "nightly-load",
                }
            }),
        },
        ProfileCase {
            label: "IAM_ROLE_SESSION with the shortest session name",
            body: json!({
                "userIdentifier": "Analyst",
                "userType": "IAM_ROLE_SESSION",
                "sessionName": "ab",
            }),
            profile_type: USER_PROFILE_TYPE_IAM,
            status: USER_PROFILE_STATUS_ACTIVATED,
            details: json!({
                "iam": {
                    "arn": format!("arn:aws:iam::{account}:role/Analyst"),
                    "sessionName": "ab",
                }
            }),
        },
        ProfileCase {
            label: "SSO_USER named by username",
            body: json!({ "userIdentifier": "jane.doe@example.com", "userType": "SSO_USER" }),
            profile_type: USER_PROFILE_TYPE_SSO,
            status: USER_PROFILE_STATUS_ASSIGNED,
            details: json!({ "sso": { "username": "jane.doe@example.com" } }),
        },
        ProfileCase {
            label: "SSO_USER named by an identity-store user id",
            body: json!({
                "userIdentifier": "9067d1b6b0-4a2e8f31-7c0d-4b19-9f52-6e1d3a8b4c77",
                "userType": "SSO_USER",
            }),
            profile_type: USER_PROFILE_TYPE_SSO,
            // An id is not a username, and no identity store stands behind this
            // emulator to resolve one through, so nothing is reported under `sso`.
            details: json!({ "sso": {} }),
            status: USER_PROFILE_STATUS_ASSIGNED,
        },
        ProfileCase {
            label: "no userType, named by ARN",
            body: json!({
                "userIdentifier": format!("arn:aws:iam::{account}:role/Untyped"),
            }),
            profile_type: USER_PROFILE_TYPE_IAM,
            status: USER_PROFILE_STATUS_ACTIVATED,
            details: json!({
                "iam": { "arn": format!("arn:aws:iam::{account}:role/Untyped") }
            }),
        },
        ProfileCase {
            label: "no userType, named by principal name",
            body: json!({ "userIdentifier": "untyped_principal" }),
            profile_type: USER_PROFILE_TYPE_IAM,
            status: USER_PROFILE_STATUS_ACTIVATED,
            details: json!({
                "iam": { "arn": format!("arn:aws:iam::{account}:user/untyped_principal") }
            }),
        },
        ProfileCase {
            label: "no userType, named by a bare identity-store user id",
            body: json!({ "userIdentifier": "4a2e8f31-7c0d-4b19-9f52-6e1d3a8b4c77" }),
            // The identifier has the shape of an identity id, which is the identity
            // centre alternative of `UserIdentifier`.
            profile_type: USER_PROFILE_TYPE_SSO,
            status: USER_PROFILE_STATUS_ASSIGNED,
            details: json!({ "sso": {} }),
        },
    ]
}

/// The `userIdentifier` of a case, as a single URI path segment: a `/` inside an ARN is
/// percent-encoded so the whole identifier stays in the one `{userIdentifier}` label.
fn identifier_label(body: &Value) -> String {
    body.get("userIdentifier")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .replace('/', "%2F")
}

/// The `GetUserProfile` URI for one case: the identity in the path, plus the
/// `sessionName` query the profile was created with.
fn get_uri(domain: &str, case_body: &Value) -> String {
    let mut uri = format!(
        "/v2/domains/{domain}/user-profiles/{}",
        identifier_label(case_body)
    );
    if let Some(session) = case_body.get("sessionName").and_then(Value::as_str) {
        uri.push_str(&format!("?sessionName={session}"));
    }
    uri
}

/// Assert that one answer reports the profile the case's rules describe.
fn assert_profile(label: &str, source: &str, answer_body: &Value, case: &ProfileCase, domain: &str) {
    assert_eq!(
        answer_body.get("domainId").and_then(Value::as_str),
        Some(domain),
        "case {label:?}: {source} must report the domain the profile is in, got {:?}",
        answer_body.get("domainId")
    );
    assert_eq!(
        answer_body.get("type").and_then(Value::as_str),
        Some(case.profile_type),
        "case {label:?}: {source} must report type {:?}, got {:?}",
        case.profile_type,
        answer_body.get("type")
    );
    assert_eq!(
        answer_body.get("status").and_then(Value::as_str),
        Some(case.status),
        "case {label:?}: {source} must report status {:?}, got {:?}",
        case.status,
        answer_body.get("status")
    );
    assert_eq!(
        answer_body.get("details"),
        Some(&case.details),
        "case {label:?}: {source} must report the details derived from the request, got {:?}",
        answer_body.get("details")
    );
}

#[tokio::test]
async fn created_user_profiles_are_derived_persisted_and_reported_by_get() {
    let dz = Emulator::new();
    let domain = dz.available_domain("user-profiles-domain").await;
    let other_domain = dz.available_domain("user-profiles-elsewhere").await;
    let profiles_uri = format!("/v2/domains/{domain}/user-profiles");

    let mut created: Vec<(ProfileCase, String)> = Vec::new();
    for case in profile_cases() {
        let answer = dz.call("POST", &profiles_uri, Some(case.body.clone())).await;
        assert_eq!(
            answer.status, 201,
            "case {:?}: expected 201, got {} {:?}",
            case.label, answer.status, answer.body
        );
        let id = answer.s("id");
        assert!(
            is_user_profile_id(&id),
            "case {:?}: expected a modelled UserProfileId, got {id:?}",
            case.label
        );
        assert_profile(
            case.label,
            "CreateUserProfile",
            &answer.body,
            &case,
            &domain,
        );
        created.push((case, id));
    }

    // Every id is distinct: two profiles never share one.
    let mut ids: Vec<String> = created.iter().map(|(_, id)| id.clone()).collect();
    ids.sort();
    let distinct = ids.len();
    ids.dedup();
    assert_eq!(
        ids.len(),
        distinct,
        "each created user profile must have its own id, got {ids:?}"
    );

    for (case, id) in &created {
        // Read back by the IDENTITY the profile was created for...
        let by_identity = dz.call("GET", &get_uri(&domain, &case.body), None).await;
        assert_eq!(
            by_identity.status, 200,
            "case {:?}: expected 200 from GetUserProfile by identity, got {} {:?}",
            case.label, by_identity.status, by_identity.body
        );
        assert_eq!(
            by_identity.s("id"),
            *id,
            "case {:?}: GetUserProfile must report the stored id, got {:?}",
            case.label,
            by_identity.s("id")
        );
        assert_profile(
            case.label,
            "GetUserProfile by identity",
            &by_identity.body,
            case,
            &domain,
        );

        // ...and by the id the create minted, which addresses the same profile.
        let by_id = dz.call("GET", &format!("{profiles_uri}/{id}"), None).await;
        assert_eq!(
            by_id.status, 200,
            "case {:?}: expected 200 from GetUserProfile by id, got {} {:?}",
            case.label, by_id.status, by_id.body
        );
        assert_profile(
            case.label,
            "GetUserProfile by id",
            &by_id.body,
            case,
            &domain,
        );

        // A reader may narrow by `type`: the profile's own type finds it, the other
        // member of the enum does not.
        let narrowed = dz
            .call(
                "GET",
                &format!(
                    "/v2/domains/{domain}/user-profiles/{}?type={}",
                    identifier_label(&case.body),
                    case.profile_type
                ),
                None,
            )
            .await;
        assert_eq!(
            narrowed.status,
            if case.body.get("sessionName").is_some() {
                // The stored profile carries a session name, and this request names
                // none, so it addresses a different identity.
                404
            } else {
                200
            },
            "case {:?}: narrowing by the profile's own type, got {} {:?}",
            case.label,
            narrowed.status,
            narrowed.body
        );
        let other_type = if case.profile_type == USER_PROFILE_TYPE_IAM {
            USER_PROFILE_TYPE_SSO
        } else {
            USER_PROFILE_TYPE_IAM
        };
        let wrong_type = dz
            .call(
                "GET",
                &format!(
                    "/v2/domains/{domain}/user-profiles/{}?type={other_type}",
                    identifier_label(&case.body)
                ),
                None,
            )
            .await;
        assert_eq!(
            wrong_type.status, 404,
            "case {:?}: a profile of type {other_type} must not be found, got {} {:?}",
            case.label, wrong_type.status, wrong_type.body
        );

        // A profile is scoped to its domain: the same identity in another domain is as
        // absent as one nothing created.
        let cross = dz.call("GET", &get_uri(&other_domain, &case.body), None).await;
        assert_eq!(
            cross.status, 404,
            "case {:?}: a profile of another domain must not be readable here, got {} {:?}",
            case.label, cross.status, cross.body
        );
        assert_eq!(
            cross.error_code, "ResourceNotFoundException",
            "case {:?}: expected ResourceNotFoundException, got {:?}",
            case.label, cross.error_code
        );
    }

    // Deleting the domain takes its user profiles with it.
    let deleted = dz
        .call("DELETE", &format!("/v2/domains/{domain}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "deleting the domain: expected 202, got {} {:?}",
        deleted.status, deleted.body
    );
    for (case, id) in &created {
        let gone = dz.call("GET", &format!("{profiles_uri}/{id}"), None).await;
        // The delete has started, so the domain is still stored in its DELETING
        // status: the caller is still permitted on it, and what it no longer holds is
        // reported as existence.
        assert_eq!(
            gone.status, 404,
            "case {:?}: a profile must go with its domain, got {} {:?}",
            case.label, gone.status, gone.body
        );
    }
}

#[tokio::test]
async fn a_repeated_create_reports_the_same_profile() {
    let dz = Emulator::new();
    let domain = dz.available_domain("user-profiles-idempotent").await;
    let profiles_uri = format!("/v2/domains/{domain}/user-profiles");

    for case in profile_cases() {
        let first = dz.call("POST", &profiles_uri, Some(case.body.clone())).await;
        assert_eq!(
            first.status, 201,
            "case {:?}: expected 201, got {} {:?}",
            case.label, first.status, first.body
        );
        // Repeated bare, and repeated with a client token: neither stores a second
        // profile for the identity the request names.
        let mut with_token = case.body.clone();
        with_token["clientToken"] = json!("c8f2e0d4-9a71-4f2b-8c3d-0b5a6e7f1d20");
        for (repeat_label, body) in [("the same request", case.body.clone()), ("a clientToken", with_token)] {
            let again = dz.call("POST", &profiles_uri, Some(body)).await;
            assert_eq!(
                again.status, 201,
                "case {:?} repeated with {repeat_label}: expected 201, got {} {:?}",
                case.label, again.status, again.body
            );
            assert_eq!(
                again.s("id"),
                first.s("id"),
                "case {:?} repeated with {repeat_label}: expected the first profile's id {:?}, got {:?}",
                case.label,
                first.s("id"),
                again.s("id")
            );
            assert_eq!(
                again.body, first.body,
                "case {:?} repeated with {repeat_label}: expected the stored profile, got {:?}",
                case.label, again.body
            );
        }
    }

    // A different session name on the same role IS a different identity, so it gets a
    // profile of its own.
    let role = format!("arn:aws:iam::{}:role/Shared", account());
    let mut sessions: Vec<String> = Vec::new();
    for session in ["morning-load", "evening-load"] {
        let answer = dz
            .call(
                "POST",
                &profiles_uri,
                Some(json!({
                    "userIdentifier": role,
                    "userType": "IAM_ROLE_SESSION",
                    "sessionName": session,
                })),
            )
            .await;
        assert_eq!(
            answer.status, 201,
            "session {session:?}: expected 201, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body.get("details"),
            Some(&json!({ "iam": { "arn": role, "sessionName": session } })),
            "session {session:?}: expected the session's own details, got {:?}",
            answer.body.get("details")
        );
        sessions.push(answer.s("id"));
    }
    assert_ne!(
        sessions[0], sessions[1],
        "two sessions of one role must get their own profiles, got {sessions:?}"
    );

    // The identity is the profile, so a repeat that names ANOTHER userType for the same
    // identifier still reports the stored profile rather than a second one of that type.
    let retyped_uri = profiles_uri.clone();
    let first = dz
        .call(
            "POST",
            &retyped_uri,
            Some(json!({ "userIdentifier": "retyped.identity", "userType": "IAM_USER" })),
        )
        .await;
    assert_eq!(
        first.status, 201,
        "creating the profile to retype: expected 201, got {} {:?}",
        first.status, first.body
    );
    let retyped = dz
        .call(
            "POST",
            &retyped_uri,
            Some(json!({ "userIdentifier": "retyped.identity", "userType": "SSO_USER" })),
        )
        .await;
    assert_eq!(
        retyped.body, first.body,
        "a repeat naming another userType must report the stored profile, got {:?}",
        retyped.body
    );
}

#[tokio::test]
async fn a_user_profile_needs_a_domain_and_an_identity_the_store_holds() {
    let dz = Emulator::new();
    let domain = dz.available_domain("user-profiles-prerequisites").await;

    // A domain-scoped permission is held ON a domain, so a pattern-valid identifier
    // this emulator serves no domain for is denied rather than reported as a miss —
    // for the create and for the read alike.
    for case in profile_cases() {
        let created = dz
            .call(
                "POST",
                "/v2/domains/dzd_nosuchdomain000/user-profiles",
                Some(case.body.clone()),
            )
            .await;
        assert_eq!(
            created.status, 403,
            "case {:?}: expected 403 for an unknown domain, got {} {:?}",
            case.label, created.status, created.body
        );
        assert_eq!(
            created.error_code, "AccessDeniedException",
            "case {:?}: expected AccessDeniedException, got {:?}",
            case.label, created.error_code
        );
        let read = dz
            .call("GET", &get_uri("dzd_nosuchdomain000", &case.body), None)
            .await;
        assert_eq!(
            read.status, 403,
            "case {:?}: expected 403 reading in an unknown domain, got {} {:?}",
            case.label, read.status, read.body
        );
    }

    // An identity the domain holds no profile for is not found — one of each
    // `UserIdentifier` form, none of them created.
    for identifier in [
        "never-created",
        "arn:aws:iam::000000000000:role%2Fnever-created",
        "4a2e8f31-7c0d-4b19-9f52-000000000000",
    ] {
        let missing = dz
            .call(
                "GET",
                &format!("/v2/domains/{domain}/user-profiles/{identifier}"),
                None,
            )
            .await;
        assert_eq!(
            missing.status, 404,
            "reading profile {identifier:?}: expected 404, got {} {:?}",
            missing.status, missing.body
        );
        assert_eq!(
            missing.error_code, "ResourceNotFoundException",
            "reading profile {identifier:?}: expected ResourceNotFoundException, got {:?}",
            missing.error_code
        );
    }

    // Outside a modelled constraint the request is refused, and nothing is stored: a
    // `sessionName` shorter than the modelled minimum of 2, and a `userType` the enum
    // does not have (smithy_query operation="CreateUserProfile").
    let refused: Vec<(&str, Value)> = vec![
        (
            "a one-character sessionName, one short of the modelled minimum",
            json!({
                "userIdentifier": "session-too-short",
                "userType": "IAM_ROLE_SESSION",
                "sessionName": "a",
            }),
        ),
        (
            "a 65-character sessionName, one past the modelled maximum",
            json!({
                "userIdentifier": "session-too-long",
                "userType": "IAM_ROLE_SESSION",
                "sessionName": "s".repeat(65),
            }),
        ),
        (
            "a userType the enum does not have",
            json!({ "userIdentifier": "wrong-type", "userType": "IAM_GROUP" }),
        ),
        (
            "a userIdentifier outside the modelled pattern",
            json!({ "userIdentifier": "not a user identifier!" }),
        ),
    ];
    for (label, body) in refused {
        let answer = dz
            .call(
                "POST",
                &format!("/v2/domains/{domain}/user-profiles"),
                Some(body),
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {label:?}: expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {label:?}: expected ValidationException, got {:?}",
            answer.error_code
        );
    }
}
