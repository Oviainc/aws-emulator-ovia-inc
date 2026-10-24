// SPDX-License-Identifier: CC0-1.0

//! The Amazon EVS Custom Addon depot URL, end to end over awsJson1_0.
//!
//! The rule these tests assert over a SET of inputs: `GetDepotUrl` reports the depot URL
//! and the authentication token OF the environment the request names, the token is the
//! environment's own and is issued once — every call that does not rotate reports the
//! same token — and a call carrying `rotate` true replaces it with a different token,
//! while the URL always carries the token it reports as a path component.
//!
//! Where each clause comes from (`smithy_query(query="operation",
//! operation="GetDepotUrl")` and the docs it carries):
//!
//! * "Returns a URL and authentication token for accessing the Amazon EVS Custom Addon
//!   depot. … The depot URL remains active until you rotate the authentication token by
//!   calling this action with rotate set to true." (`GetDepotUrl`.)
//! * "Revokes the current authentication token and returns a new depot URL with a new
//!   token." (`GetDepotUrlRequest$rotate`.)
//! * "The URL for accessing the Amazon EVS Custom Addon depot. This URL includes the
//!   authentication token as a path component." (`GetDepotUrlResponse$depotUrl`.)
//! * "The authentication token for depot access. This token is included in the depot URL
//!   and is used to authenticate requests." (`GetDepotUrlResponse$token`.)
//! * `ResourceNotFoundException` — "The resource might not be specified correctly, or it
//!   may have a `state` of `DELETED`" — for an environment this service holds no record
//!   of at the endpoint the caller addressed.
//!
//! The rows differ along what the rule generalises over: whether `rotate` is absent,
//! false or true, how many calls are made, which environment is addressed, and the Region
//! the caller addressed.
//!
//! The harness (`emulator`, `call`, `call_in_region`, `create`, `settle`) is the one
//! `environment_lifecycle` already owns, so this suite drives the service the way a
//! client does: `POST /` with `X-Amz-Target: AmazonElasticVMwareService.<Operation>`.

use serde_json::{json, Value};

use crate::environment_lifecycle::{
    call, call_in_region, create, emulator, error_type, settle, Emulator,
};

/// How many characters a depot authentication token this service issues carries.
///
/// Written out here rather than read from the service, so a change to the length the
/// handler issues shows up as a failure of this suite instead of agreeing with itself.
const TOKEN_LENGTH: usize = 40;

/// A `GetDepotUrl` request for `environment_id`, with `rotate` set as the row asks:
/// `None` leaves the member out altogether, which is the request a client that never
/// rotates sends.
fn request(environment_id: &str, rotate: Option<bool>) -> Value {
    let mut body = json!({ "environmentId": environment_id });
    if let Some(rotate) = rotate {
        body["rotate"] = json!(rotate);
    }
    body
}

/// The `(depotUrl, token)` one accepted `GetDepotUrl` call reports, with the whole rule
/// that holds of EVERY answer checked on the way out: both members are present and
/// non-empty, the URL is an HTTPS URL, it names the environment asked about, and it
/// carries the token as a PATH COMPONENT — a whole path segment, not a query parameter
/// and not a substring of some longer segment.
async fn depot(
    router: &Emulator,
    case: &str,
    environment_id: &str,
    rotate: Option<bool>,
) -> (String, String) {
    let (status, body) = call(router, "GetDepotUrl", &request(environment_id, rotate)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetDepotUrl to answer 200, got {status} {body}"
    );
    let url = body["depotUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no depotUrl in {body}"))
        .to_string();
    let token = body["token"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no token in {body}"))
        .to_string();

    assert!(
        !token.is_empty(),
        "case {case:?}: the reported token must not be empty, got {token:?}"
    );
    assert_eq!(
        token.len(),
        TOKEN_LENGTH,
        "case {case:?}: expected a token of {TOKEN_LENGTH} characters, got {token:?}"
    );
    assert!(
        token.chars().all(|character| character.is_ascii_alphanumeric()),
        "case {case:?}: a token carried in a URL path must be path-safe, got {token:?}"
    );
    let path = url.strip_prefix("https://").unwrap_or_else(|| {
        panic!("case {case:?}: the depot URL must be an HTTPS URL, got {url:?}")
    });
    let segments: Vec<&str> = path.split('/').skip(1).collect();
    assert!(
        segments.contains(&token.as_str()),
        "case {case:?}: the URL must carry the token as a path component, got {url:?} \
         for token {token:?}"
    );
    assert!(
        segments.contains(&environment_id),
        "case {case:?}: the URL must name the environment it is the depot of, got \
         {url:?} for {environment_id}"
    );
    assert!(
        !url.contains('?') && !url.contains('#'),
        "case {case:?}: the token is a path component, not a query or a fragment, got \
         {url:?}"
    );
    (url, token)
}

/// One call of the stability table: how it asks, and what it must report.
struct Call {
    /// What this row is a case of.
    case: &'static str,
    /// The `rotate` the call carries, or `None` for a request that omits the member.
    rotate: Option<bool>,
    /// Whether the token this call reports must DIFFER from the one before it.
    rotates: bool,
}

#[tokio::test]
async fn a_depot_token_is_issued_once_and_only_a_rotating_call_replaces_it() {
    let router = emulator();
    let environment = create(&router, Some("depot-env")).await;

    // The rows differ along how `rotate` is spelled (absent, false, true), and each is
    // checked against the token the call before it reported: only a rotating call may
    // hand out a different one.
    let rows = [
        Call {
            case: "the first call issues the environment's token",
            rotate: None,
            rotates: false,
        },
        Call {
            case: "a second call with rotate absent reports the same token",
            rotate: None,
            rotates: false,
        },
        Call {
            case: "rotate false is not a rotation either",
            rotate: Some(false),
            rotates: false,
        },
        Call {
            case: "rotate true replaces the token",
            rotate: Some(true),
            rotates: true,
        },
        Call {
            case: "the rotated token is now the stored one",
            rotate: None,
            rotates: false,
        },
        Call {
            case: "rotate false after a rotation still reports the rotated token",
            rotate: Some(false),
            rotates: false,
        },
        Call {
            case: "rotating twice hands out a third token",
            rotate: Some(true),
            rotates: true,
        },
        Call {
            case: "and that token is what a later caller reads",
            rotate: None,
            rotates: false,
        },
    ];

    let mut previous: Option<(String, String)> = None;
    let mut seen: Vec<String> = Vec::new();
    for Call {
        case,
        rotate,
        rotates,
    } in rows
    {
        let (url, token) = depot(&router, case, &environment, rotate).await;
        if let Some((previous_url, previous_token)) = &previous {
            if rotates {
                assert_ne!(
                    &token, previous_token,
                    "case {case:?}: a rotating call must report a new token, got the \
                     previous one {token:?}"
                );
                assert_ne!(
                    &url, previous_url,
                    "case {case:?}: a rotating call must report a new depot URL, got \
                     the previous one {url:?}"
                );
            } else {
                assert_eq!(
                    &token, previous_token,
                    "case {case:?}: expected the stored token {previous_token:?}, got \
                     {token:?}"
                );
                assert_eq!(
                    &url, previous_url,
                    "case {case:?}: expected the stored depot URL {previous_url:?}, got \
                     {url:?}"
                );
            }
        }
        if rotates {
            assert!(
                !seen.contains(&token),
                "case {case:?}: a rotation must mint a token no earlier call reported, \
                 got {token:?} among {seen:?}"
            );
        }
        seen.push(token.clone());
        previous = Some((url, token));
    }
}

#[tokio::test]
async fn every_environment_has_its_own_depot_token() {
    let router = emulator();

    // Three environments in one account and Region, each asked about in turn: the depot
    // is an environment's own, so no two share a token or a URL, and a rotation on one
    // leaves the others exactly as a later caller finds them.
    let names = ["depot-a", "depot-b", "depot-c"];
    let mut environments = Vec::new();
    for name in names {
        environments.push(create(&router, Some(name)).await);
    }

    let mut issued = Vec::new();
    for (name, environment) in names.iter().zip(&environments) {
        let (url, token) = depot(&router, name, environment, None).await;
        issued.push((url, token));
    }
    for (left, right) in [(0, 1), (0, 2), (1, 2)] {
        assert_ne!(
            issued[left].1, issued[right].1,
            "case {:?} vs {:?}: two environments must not share a depot token, got {:?}",
            names[left], names[right], issued[left].1
        );
        assert_ne!(
            issued[left].0, issued[right].0,
            "case {:?} vs {:?}: two environments must not share a depot URL, got {:?}",
            names[left], names[right], issued[left].0
        );
    }

    // Rotate the first environment's token only.
    let (rotated_url, rotated_token) =
        depot(&router, "depot-a rotated", &environments[0], Some(true)).await;
    assert_ne!(
        rotated_token, issued[0].1,
        "case \"depot-a rotated\": expected a new token, got {rotated_token:?}"
    );
    for (index, name) in names.iter().enumerate().skip(1) {
        let (url, token) = depot(&router, name, &environments[index], None).await;
        assert_eq!(
            (url.as_str(), token.as_str()),
            (issued[index].0.as_str(), issued[index].1.as_str()),
            "case {name:?}: rotating another environment's token must leave this one \
             alone, got ({url:?}, {token:?})"
        );
        assert_ne!(
            token, rotated_token,
            "case {name:?}: must not be handed the token rotated for another \
             environment, got {token:?}"
        );
        assert_ne!(
            url, rotated_url,
            "case {name:?}: must not be handed the URL rotated for another environment, \
             got {url:?}"
        );
    }
}

#[tokio::test]
async fn an_environment_this_region_does_not_hold_has_no_depot_url() {
    let router = emulator();
    let home = create(&router, Some("depot-home")).await;
    // One token issued, so a miss below is the lookup's answer and not an empty store.
    let (_, home_token) = depot(&router, "the environment that exists", &home, None).await;

    // Each row: the case, the environment id asked about, the Region the caller
    // addressed, and the `rotate` it carried — a rotation must not create a depot for an
    // environment that is not there either.
    let rows: Vec<(&str, String, &str, Option<bool>)> = vec![
        (
            "a well-formed id no environment holds",
            "env-0000000000".to_string(),
            "us-west-2",
            None,
        ),
        (
            "a well-formed id no environment holds, rotating",
            "env-ZZZZZZZZZZ".to_string(),
            "us-west-2",
            Some(true),
        ),
        (
            "the home environment from another Region",
            home.clone(),
            "us-east-1",
            None,
        ),
        (
            "the home environment from another Region, rotating",
            home.clone(),
            "eu-central-1",
            Some(true),
        ),
        (
            "the home environment from a four-segment Region",
            home.clone(),
            "us-gov-west-1",
            None,
        ),
    ];
    for (case, environment_id, region, rotate) in rows {
        let (status, body) = call_in_region(
            &router,
            region,
            "GetDepotUrl",
            &request(&environment_id, rotate),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_type(&body).rsplit(['#', '/']).next().unwrap_or(""),
            "ResourceNotFoundException",
            "case {case:?}: expected a ResourceNotFoundException, got {}",
            error_type(&body)
        );
    }

    // None of those refusals disturbed the environment that does exist: it still reports
    // the token it was issued.
    let (_, token) = depot(&router, "the home environment again", &home, None).await;
    assert_eq!(
        token, home_token,
        "case \"the home environment again\": a refused call changes nothing, expected \
         {home_token:?}, got {token:?}"
    );
}

#[tokio::test]
async fn a_deleted_environment_has_no_depot_url() {
    let router = emulator();
    let environment = create(&router, Some("depot-deleted")).await;
    let (_, token) = depot(&router, "before the delete", &environment, None).await;
    assert!(
        !token.is_empty(),
        "case \"before the delete\": a token must have been issued, got {token:?}"
    );

    let (status, body) = call(
        &router,
        "DeleteEnvironment",
        &json!({ "environmentId": environment }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the delete\": expected 200, got {status} {body}"
    );
    // Past the deletion transition the clock owes the environment, so its state has
    // settled on DELETED.
    settle(&router);

    for rotate in [None, Some(false), Some(true)] {
        let case = format!("after the delete, rotate {rotate:?}");
        let (status, body) = call(&router, "GetDepotUrl", &request(&environment, rotate)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_type(&body).rsplit(['#', '/']).next().unwrap_or(""),
            "ResourceNotFoundException",
            "case {case:?}: expected a ResourceNotFoundException, got {}",
            error_type(&body)
        );
    }
}
