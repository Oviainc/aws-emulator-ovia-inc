// SPDX-License-Identifier: CC0-1.0

//! The compute authentication-token contract, over the wire: `GetComputeAuthToken` issues
//! the one temporary token the compute a request names holds, and reports it beside the
//! stored identifiers of that compute and its fleet.
//!
//! The rule the rows below span: a request for a compute's authentication token is answered
//! from the compute the fleet holds under the name the request gave — by either of the
//! forms `FleetIdOrArn` and `ComputeNameOrArn` accept — with the ONE token that compute
//! holds while it is valid, a freshly minted one once it has expired, and the token's
//! expiration; a fleet or a compute that is not registered is refused with
//! `NotFoundException` and nothing is issued.
//!
//! The harness (`GameLift`, `REGION`, `gamelift_arn`, `anywhere_fleet`, `create_fleet`,
//! `error_code`, `error_message`) is the one `hosting_loop.rs` defines and drives the
//! production dispatch path with.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_gamelift_gamelift::state::COMPUTE_AUTH_TOKEN_VALIDITY;

use crate::hosting_loop::{
    anywhere_fleet, create_fleet, error_code, error_message, gamelift_arn, GameLift, REGION,
};

/// The longest `ComputeAuthToken` the model allows (`smithy_query
/// operation=GetComputeAuthToken`: len 1..64).
const MAX_AUTH_TOKEN_LENGTH: usize = 64;

/// The two custom locations the fleets below deploy to.
const RACK_ONE: &str = "custom-rack-1";
/// See [`RACK_ONE`].
const RACK_TWO: &str = "custom-rack-2";

/// One row of the table: a compute to request a token for, and the forms the request
/// names it and its fleet by.
struct Case {
    label: &'static str,
    /// The `ComputeName` the compute is registered under.
    name: String,
    location: &'static str,
    /// Whether the request names the fleet by its ARN rather than its id.
    fleet_by_arn: bool,
    /// Whether the request names the compute by its ARN rather than its name.
    compute_by_arn: bool,
}

/// The computes the rule is asserted over. The rows differ along what the rule generalises
/// over: which of the two forms `FleetIdOrArn` and `ComputeNameOrArn` accept the request
/// names the fleet and the compute by, the form and LENGTH of the compute's own name
/// (`ComputeName`, len 1..128, pattern `^[a-zA-Z0-9\-]+(\/[a-zA-Z0-9\-]+)?$`: a single
/// character, digits, upper case, hyphens, the optional second segment), and which of its
/// fleet's locations the compute resides in.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "the fleet by id and the compute by name",
            name: "rack-1-host-a".to_string(),
            location: RACK_ONE,
            fleet_by_arn: false,
            compute_by_arn: false,
        },
        Case {
            label: "the fleet by ARN and the compute by name",
            name: "HOST-2".to_string(),
            location: RACK_ONE,
            fleet_by_arn: true,
            compute_by_arn: false,
        },
        Case {
            label: "the fleet by id and the compute by ARN",
            name: "rack-2-host/process-1".to_string(),
            location: RACK_TWO,
            fleet_by_arn: false,
            compute_by_arn: true,
        },
        Case {
            label: "both named by ARN",
            name: "9".to_string(),
            location: RACK_TWO,
            fleet_by_arn: true,
            compute_by_arn: true,
        },
        Case {
            label: "the longest compute name the model allows",
            name: format!("host-{}", "a".repeat(128 - "host-".len())),
            location: RACK_ONE,
            fleet_by_arn: true,
            compute_by_arn: false,
        },
    ]
}

/// The ARN assigned to a compute named `name`. `Compute$ComputeArn`'s modelled pattern is
/// `^arn:.*:compute\/[a-zA-Z0-9\-]+(\/[a-zA-Z0-9\-]+)?$`, so the resource is the compute's
/// own name.
fn compute_arn(name: &str) -> String {
    gamelift_arn(&format!("compute/{name}"))
}

/// The ARN assigned to the fleet `fleet_id`.
fn fleet_arn(fleet_id: &str) -> String {
    gamelift_arn(&format!("fleet/{fleet_id}"))
}

/// An instant in Unix seconds — the awsJson wire form of a modelled `Timestamp`.
fn unix_seconds(instant: SystemTime) -> f64 {
    instant
        .duration_since(UNIX_EPOCH)
        .expect("an instant after the Unix epoch")
        .as_secs_f64()
}

/// A timestamp member of a response, in Unix seconds.
fn instant(response: &Value, member: &str) -> f64 {
    response[member]
        .as_f64()
        .unwrap_or_else(|| panic!("{member} must be a Unix-time number, got {response}"))
}

/// Register the compute `name` in `location` on `fleet_id`.
async fn register(gamelift: &GameLift, fleet_id: &str, name: &str, location: &str) {
    gamelift
        .ok(
            "RegisterCompute",
            json!({
                "FleetId": fleet_id,
                "ComputeName": name,
                "Location": location,
                "IpAddress": "10.0.3.17",
            }),
        )
        .await;
}

/// Request the authentication token of `compute_selector` on `fleet_selector`.
async fn auth_token(gamelift: &GameLift, fleet_selector: &str, compute_selector: &str) -> Value {
    gamelift
        .ok(
            "GetComputeAuthToken",
            json!({"FleetId": fleet_selector, "ComputeName": compute_selector}),
        )
        .await
}

/// The token the store holds for the compute `name` on `fleet_id`, and the instant it
/// stops being valid, read straight out of the service's own state.
fn stored_token(gamelift: &GameLift, fleet_id: &str, name: &str) -> Option<(String, f64)> {
    let state = gamelift.state();
    let state = state.lock();
    state
        .computes
        .get(&(fleet_id.to_string(), name.to_string()))
        .and_then(|compute| compute.auth_token.as_ref())
        .map(|token| (token.token.clone(), unix_seconds(token.expires_at)))
}

/// Assert the compute `name` on `fleet_id` holds exactly the token a response reported.
///
/// The wire form of a modelled `Timestamp` is a double, which the stored instant rounds to
/// when it is serialized, so the expiration is compared to within a millisecond rather than
/// bit for bit.
fn assert_holds_token(gamelift: &GameLift, fleet_id: &str, name: &str, reported: &Value, case: &str) {
    let token = reported["AuthToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a token must be reported, got {reported}"));
    let expires_at = instant(reported, "ExpirationTimestamp");
    let stored = stored_token(gamelift, fleet_id, name);
    let holds = stored
        .as_ref()
        .is_some_and(|(held, held_expiry)| held == token && (held_expiry - expires_at).abs() < 1e-3);
    assert!(
        holds,
        "case {case:?}: the compute must hold the reported token {token:?} expiring at \
         {expires_at}, got {stored:?}"
    );
}

/// Move the stored token of the compute `name` on `fleet_id` past its expiration, so the
/// next request for it is the first one made after it stopped being valid. No test can
/// wait that span out, so the clock is moved under the record instead.
fn expire_stored_token(gamelift: &GameLift, fleet_id: &str, name: &str) {
    let state = gamelift.state();
    let mut state = state.lock();
    let compute = state
        .computes
        .get_mut(&(fleet_id.to_string(), name.to_string()))
        .unwrap_or_else(|| panic!("compute {name} must be registered on fleet {fleet_id}"));
    let token = compute
        .auth_token
        .as_mut()
        .unwrap_or_else(|| panic!("compute {name} must hold a token to expire"));
    token.expires_at = SystemTime::now() - Duration::from_secs(1);
}

/// Assert `request` is refused at HTTP 400 with `code` and exactly `expected` as its
/// `Message`.
async fn assert_refused(gamelift: &GameLift, request: Value, code: &str, expected: &str, case: &str) {
    let (status, body) = gamelift.call("GetComputeAuthToken", &request).await;
    let got = format!("{}|{}", error_code(&body), error_message(&body));
    let wanted = format!("{code}|{expected}");
    assert_eq!(
        (status, got.as_str()),
        (400, wanted.as_str()),
        "case {case:?}: expected HTTP 400 with {wanted:?}, got HTTP {status} with {got:?}"
    );
}

/// An issued token names the stored compute and its fleet, carries a value of the modelled
/// form, expires the documented span after it was issued, and is the token the store holds
/// for that compute — whichever of the accepted identifier forms the request used.
#[tokio::test]
async fn an_issued_token_is_reported_as_stored_for_every_identifier_form() {
    let gamelift = GameLift::new();
    let fleet_id = create_fleet(
        &gamelift,
        anywhere_fleet("token-forms", &[RACK_ONE, RACK_TWO]),
    )
    .await;

    let mut issued_tokens: Vec<String> = Vec::new();
    for case in cases() {
        register(&gamelift, &fleet_id, &case.name, case.location).await;

        let fleet_selector = if case.fleet_by_arn {
            fleet_arn(&fleet_id)
        } else {
            fleet_id.clone()
        };
        let compute_selector = if case.compute_by_arn {
            compute_arn(&case.name)
        } else {
            case.name.clone()
        };

        let before = unix_seconds(SystemTime::now());
        let reported = auth_token(&gamelift, &fleet_selector, &compute_selector).await;
        let after = unix_seconds(SystemTime::now());

        // The identifiers are the STORED ones, whichever form the request named them by.
        for (member, expected) in [
            ("FleetId", fleet_id.clone()),
            ("FleetArn", fleet_arn(&fleet_id)),
            ("ComputeName", case.name.clone()),
            ("ComputeArn", compute_arn(&case.name)),
        ] {
            assert_eq!(
                reported[member],
                json!(expected),
                "case {:?}: {member} must be {expected:?}, got {}",
                case.label,
                reported[member]
            );
        }

        // `ComputeAuthToken` is modelled len 1..64, pattern `^[a-zA-Z0-9\-]+$`.
        let token = reported["AuthToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: a token must be reported, got {reported}", case.label))
            .to_string();
        assert!(
            (1..=MAX_AUTH_TOKEN_LENGTH).contains(&token.len())
                && token
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-'),
            "case {:?}: the token must satisfy the modelled form, got {token:?}",
            case.label
        );

        // "Authentication tokens are valid for a limited time span": the expiration is
        // that span after the instant the token was issued.
        let expires_at = instant(&reported, "ExpirationTimestamp");
        let validity = COMPUTE_AUTH_TOKEN_VALIDITY.as_secs_f64();
        assert!(
            expires_at >= before + validity && expires_at <= after + validity,
            "case {:?}: the token must expire {validity} seconds after it was issued, got \
             {expires_at} for a call between {before} and {after}",
            case.label
        );

        // The round trip: the response is the record the compute now holds.
        assert_holds_token(&gamelift, &fleet_id, &case.name, &reported, case.label);

        // Issuing a token leaves the compute itself exactly as it was registered.
        let described = gamelift
            .ok(
                "DescribeCompute",
                json!({"FleetId": fleet_id, "ComputeName": case.name}),
            )
            .await;
        assert_eq!(
            described["Compute"]["ComputeStatus"],
            json!("ACTIVE"),
            "case {:?}: the compute must still be reported as registered, got {described}",
            case.label
        );

        issued_tokens.push(token);
    }

    // A token is issued per compute, so no two computes were handed the same one.
    let mut distinct = issued_tokens.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        issued_tokens.len(),
        "each compute must hold its own token, got {issued_tokens:?}"
    );
}

/// While the token a compute holds is valid every request for it is answered with that
/// token; the first request after it has expired is answered with a fresh one, and only
/// that compute's token changes.
#[tokio::test]
async fn the_token_a_compute_holds_is_reported_until_it_expires_and_replaced_after() {
    let gamelift = GameLift::new();
    let fleet_id = create_fleet(
        &gamelift,
        anywhere_fleet("token-refresh", &[RACK_ONE, RACK_TWO]),
    )
    .await;
    // A second compute, whose own token must be untouched by anything the rows below do.
    let bystander = "bystander-host";
    register(&gamelift, &fleet_id, bystander, RACK_TWO).await;
    let bystander_token = auth_token(&gamelift, &fleet_id, bystander).await;

    for case in cases() {
        register(&gamelift, &fleet_id, &case.name, case.location).await;
        let by_arn = compute_arn(&case.name);

        let first = auth_token(&gamelift, &fleet_id, &case.name).await;
        // The same compute, named by the other accepted form: still the same token.
        let again = auth_token(&gamelift, &fleet_arn(&fleet_id), &by_arn).await;
        assert_eq!(
            (&again["AuthToken"], &again["ExpirationTimestamp"]),
            (&first["AuthToken"], &first["ExpirationTimestamp"]),
            "case {:?}: a valid token must be reported again unchanged, got {again} after \
             {first}",
            case.label
        );

        expire_stored_token(&gamelift, &fleet_id, &case.name);

        let before = unix_seconds(SystemTime::now());
        let refreshed = auth_token(&gamelift, &fleet_id, &case.name).await;
        let after = unix_seconds(SystemTime::now());
        assert_ne!(
            refreshed["AuthToken"], first["AuthToken"],
            "case {:?}: an expired token must be replaced, got {refreshed} after {first}",
            case.label
        );
        let expires_at = instant(&refreshed, "ExpirationTimestamp");
        let validity = COMPUTE_AUTH_TOKEN_VALIDITY.as_secs_f64();
        assert!(
            expires_at >= before + validity && expires_at <= after + validity,
            "case {:?}: the fresh token must expire {validity} seconds after it was issued, \
             got {expires_at} for a call between {before} and {after}",
            case.label
        );
        assert_holds_token(&gamelift, &fleet_id, &case.name, &refreshed, case.label);

        // The other compute's token is its own: nothing above touched it.
        let bystander_now = auth_token(&gamelift, &fleet_id, bystander).await;
        assert_eq!(
            (
                &bystander_now["AuthToken"],
                &bystander_now["ExpirationTimestamp"]
            ),
            (
                &bystander_token["AuthToken"],
                &bystander_token["ExpirationTimestamp"]
            ),
            "case {:?}: another compute's token must be unchanged, got {bystander_now} after \
             {bystander_token}",
            case.label
        );
    }
}

/// A fleet or a compute that is not registered is refused with `NotFoundException`, and
/// the refusal issues nothing.
#[tokio::test]
async fn a_token_is_refused_for_a_fleet_or_compute_that_is_not_registered() {
    let gamelift = GameLift::new();
    let fleet_id = create_fleet(&gamelift, anywhere_fleet("token-refusals", &[RACK_ONE])).await;
    let other_fleet = create_fleet(&gamelift, anywhere_fleet("token-elsewhere", &[RACK_TWO])).await;
    register(&gamelift, &fleet_id, "registered-host", RACK_ONE).await;
    register(&gamelift, &other_fleet, "host-on-the-other-fleet", RACK_TWO).await;

    let absent_fleet = "fleet-00000000-0000-4000-8000-000000000000";
    // The rows differ along which of the two resources the request names is missing, and
    // the form it names it by.
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a fleet id no fleet holds",
            json!({"FleetId": absent_fleet, "ComputeName": "registered-host"}),
            format!("Cannot find fleet with ID {absent_fleet}"),
        ),
        (
            "a fleet ARN no fleet holds",
            json!({"FleetId": fleet_arn(absent_fleet), "ComputeName": "registered-host"}),
            format!("Cannot find fleet with ID {}", fleet_arn(absent_fleet)),
        ),
        (
            "a compute name the fleet holds none under",
            json!({"FleetId": fleet_id, "ComputeName": "no-such-host"}),
            "Host (no-such-host) not found.".to_string(),
        ),
        (
            "a compute ARN the fleet holds none under",
            json!({"FleetId": fleet_id, "ComputeName": compute_arn("no-such-host")}),
            format!("Host ({}) not found.", compute_arn("no-such-host")),
        ),
        (
            "a compute of another fleet",
            json!({"FleetId": fleet_id, "ComputeName": "host-on-the-other-fleet"}),
            "Host (host-on-the-other-fleet) not found.".to_string(),
        ),
    ];

    for (case, request, expected) in rows {
        assert_refused(&gamelift, request, "NotFoundException", &expected, case).await;
        assert_eq!(
            stored_token(&gamelift, &fleet_id, "registered-host"),
            None,
            "case {case:?}: a refusal must issue no token, yet the fleet's compute holds one"
        );
        assert_eq!(
            stored_token(&gamelift, &other_fleet, "host-on-the-other-fleet"),
            None,
            "case {case:?}: a refusal must issue no token for the compute it named"
        );
    }

    // A deregistered compute holds no token either: its record, and the token with it, is
    // gone.
    let token = auth_token(&gamelift, &fleet_id, "registered-host").await;
    assert!(
        token["AuthToken"].is_string(),
        "the registered compute must be issued a token, got {token}"
    );
    gamelift
        .ok(
            "DeregisterCompute",
            json!({"FleetId": fleet_id, "ComputeName": "registered-host"}),
        )
        .await;
    assert_eq!(
        stored_token(&gamelift, &fleet_id, "registered-host"),
        None,
        "a deregistered compute must hold no token"
    );
    assert_refused(
        &gamelift,
        json!({"FleetId": fleet_id, "ComputeName": "registered-host"}),
        "NotFoundException",
        "Host (registered-host) not found.",
        "a compute that has been deregistered",
    )
    .await;
}

/// The region every ARN above is minted in is the one the harness signs for, so a token
/// request served in another Region still reports the stored ARNs.
#[tokio::test]
async fn a_token_reports_the_stored_arns_whatever_region_the_request_is_served_in() {
    let gamelift = GameLift::new();
    let fleet_id = create_fleet(&gamelift, anywhere_fleet("token-regions", &[RACK_ONE])).await;
    register(&gamelift, &fleet_id, "cross-region-host", RACK_ONE).await;

    for region in ["eu-central-1", "ap-southeast-2", REGION] {
        let reported = gamelift
            .ok_in(
                region,
                "GetComputeAuthToken",
                json!({"FleetId": fleet_id, "ComputeName": "cross-region-host"}),
            )
            .await;
        assert_eq!(
            (&reported["FleetArn"], &reported["ComputeArn"]),
            (
                &json!(fleet_arn(&fleet_id)),
                &json!(compute_arn("cross-region-host"))
            ),
            "case {region:?}: the ARNs must be the ones the resources were stored with, got \
             {reported}"
        );
    }
}
