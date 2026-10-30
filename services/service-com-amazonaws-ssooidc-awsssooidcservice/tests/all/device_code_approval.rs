// SPDX-License-Identifier: CC0-1.0

//! What makes a device authorization redeemable, end to end over the wire.
//!
//! RULE: `CreateToken`'s device-code grant answers from the approval recorded on
//! the stored authorization and from nothing else — a user's approval at the
//! verification URI is what moves it out of `Pending`, so an authorization nobody
//! approved stays pending for the whole life of its device code and every poll of
//! it is refused with `AuthorizationPendingException`, however much time has
//! passed and however many times it is polled; and a device code is redeemable
//! only by the client it was issued to, any other presenter being refused exactly
//! as an unknown device code is, with the authorization left as it stood.
//!
//! Every case drives the service through the router the production emulator
//! serves, using the harness the device-flow suite beside this file builds.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_ssooidc_awsssooidcservice as ssooidc;
use ssooidc::state::{DeviceAuthStatus, DEVICE_CODE_EXPIRES_IN, DEVICE_CODE_INTERVAL,
    GRANT_TYPE_DEVICE_CODE, GRANT_TYPE_REFRESH_TOKEN};

use crate::device_authorization_flow::{assert_failure, start_url, text, Emulator};

/// The fixed `error_description` of the pending refusal, as the service words it
/// for this error class.
const PENDING_DESCRIPTION: &str = "Authorization is still pending";

/// The fixed `error_description` of the device-code refusal — the same sentence
/// for an unknown code and for another client's, so one tells a presenter nothing
/// the other does not.
const INVALID_DEVICE_CODE_DESCRIPTION: &str = "Invalid device code provided";

/// Start a device authorization for `client` and answer its `deviceCode` and
/// `userCode`.
async fn started_authorization(
    emulator: &Emulator,
    client_id: &str,
    client_secret: &str,
) -> (String, String) {
    let (status, started) = emulator
        .start_device_authorization(&json!({
            "clientId": client_id,
            "clientSecret": client_secret,
            "startUrl": start_url(),
        }))
        .await;
    assert_eq!(status, 200, "StartDeviceAuthorization answered {started}");
    (text(&started, "deviceCode"), text(&started, "userCode"))
}

/// Move the stored authorization's `createdAt` back by `elapsed`, which is how a
/// case spans time the wall clock cannot be moved for.
fn age_authorization(emulator: &Emulator, device_code: &str, elapsed: Duration) {
    let state = emulator.state();
    let mut store = state.lock();
    let record = store
        .device_auths
        .get_mut(device_code)
        .expect("the device authorization is persisted");
    record.created_at -= elapsed;
}

/// The status the store holds for `device_code`.
fn stored_status(emulator: &Emulator, device_code: &str) -> Option<DeviceAuthStatus> {
    emulator
        .state()
        .lock()
        .device_auths
        .get(device_code)
        .map(|record| record.status)
}

/// Assert that `body` is the pending refusal, whole: status, shape, `error` and the
/// fixed `error_description`.
fn assert_pending(case: &str, status: u16, body: &Value) {
    assert_failure(
        case,
        status,
        body,
        400,
        "AuthorizationPendingException",
        "authorization_pending",
    );
    assert_eq!(
        body["error_description"].as_str(),
        Some(PENDING_DESCRIPTION),
        "case {case:?}: expected the pending text {PENDING_DESCRIPTION:?}, got {body}"
    );
}

/// Assert that `body` is the device-code refusal, whole.
fn assert_invalid_device_code(case: &str, status: u16, body: &Value) {
    assert_failure(case, status, body, 400, "InvalidGrantException", "invalid_grant");
    assert_eq!(
        body["error_description"].as_str(),
        Some(INVALID_DEVICE_CODE_DESCRIPTION),
        "case {case:?}: expected the device-code text {INVALID_DEVICE_CODE_DESCRIPTION:?}, got {body}"
    );
}

/// RULE: an authorization no user approved is pending for the life of its device
/// code — elapsed time approves nothing, so every poll is refused and no token is
/// minted. The table varies how much time has passed since the authorization was
/// started (the member the advertised `interval` and `expiresIn` are measured in)
/// and how many times the client polls.
#[tokio::test]
async fn a_device_authorization_nobody_approved_stays_pending_however_long_it_is_polled() {
    let emulator = Emulator::new();
    let (client_id, client_secret) = emulator.registered_client(json!({})).await;

    let interval = u64::try_from(DEVICE_CODE_INTERVAL).expect("a modeled interval is positive");
    let lifetime = u64::try_from(DEVICE_CODE_EXPIRES_IN).expect("a modeled lifetime is positive");
    // (case, seconds elapsed since StartDeviceAuthorization, polls made at that point)
    let rows: Vec<(&str, u64, u32)> = vec![
        ("the first poll, straight after starting", 0, 1),
        ("one advertised interval later", interval, 1),
        ("one interval and a second later", interval + 1, 1),
        ("ten intervals later, polled at each one", 10 * interval, 10),
        ("one second before the device code expires", lifetime - 1, 3),
    ];

    for (case, elapsed, polls) in rows {
        let (device_code, _user_code) =
            started_authorization(&emulator, &client_id, &client_secret).await;
        if elapsed > 0 {
            age_authorization(&emulator, &device_code, Duration::from_secs(elapsed));
        }
        let poll = json!({
            "clientId": client_id,
            "clientSecret": client_secret,
            "grantType": GRANT_TYPE_DEVICE_CODE,
            "deviceCode": device_code,
        });
        for attempt in 1..=polls {
            let (status, body) = emulator.create_token(&poll).await;
            assert_pending(&format!("{case}, poll {attempt} of {polls}"), status, &body);
            assert_eq!(
                stored_status(&emulator, &device_code),
                Some(DeviceAuthStatus::Pending),
                "case {case:?}: the authorization must still be pending after poll {attempt}"
            );
        }
    }

    // The same holds for time that really passes rather than time aged off a
    // stored record: a poll after a wall-clock wait is refused too.
    let (device_code, _user_code) =
        started_authorization(&emulator, &client_id, &client_secret).await;
    tokio::time::sleep(Duration::from_millis(750)).await;
    let (status, body) = emulator
        .create_token(&json!({
            "clientId": client_id,
            "clientSecret": client_secret,
            "grantType": GRANT_TYPE_DEVICE_CODE,
            "deviceCode": device_code,
        }))
        .await;
    assert_pending("a poll after waiting on the wall clock", status, &body);

    // Nothing above minted a token, so no credential exists for an authorization
    // no user approved.
    let state = emulator.state();
    let store = state.lock();
    assert!(
        store.access_tokens.is_empty() && store.refresh_tokens.is_empty(),
        "no token may be issued for an unapproved authorization, got {store:?}"
    );
}

/// RULE: a device code is redeemable only by the client it was issued to, and any
/// other presenter is refused exactly as an unknown device code is, with the
/// authorization left as it stood. The table varies the presenting registration —
/// what it scopes itself to, what grants it names, and whether it registered
/// before or after the authorization was started.
#[tokio::test]
async fn another_clients_device_code_is_refused_as_an_unknown_one_and_left_pending() {
    let emulator = Emulator::new();
    let (owner_id, owner_secret) = emulator
        .registered_client(json!({"clientName": "device-flow-owner"}))
        .await;
    let (early_id, early_secret) = emulator
        .registered_client(json!({"clientName": "registered-before", "scopes": ["openid"]}))
        .await;

    let (device_code, user_code) =
        started_authorization(&emulator, &owner_id, &owner_secret).await;
    let owner_poll = json!({
        "clientId": owner_id,
        "clientSecret": owner_secret,
        "grantType": GRANT_TYPE_DEVICE_CODE,
        "deviceCode": device_code,
    });

    // A device code this service never issued, for the refusal every other
    // presenter's must be indistinguishable from.
    let (status, unknown) = emulator
        .create_token(&json!({
            "clientId": early_id,
            "clientSecret": early_secret,
            "grantType": GRANT_TYPE_DEVICE_CODE,
            "deviceCode": "a1b2c3d4e5f60718293a4b5c6d7e8f90",
        }))
        .await;
    assert_invalid_device_code("a device code this service never issued", status, &unknown);

    let mut presenters: Vec<(&str, String, String)> = vec![(
        "a client registered before the authorization was started",
        early_id,
        early_secret,
    )];
    for (case, registration) in [
        ("a client naming no scopes or grants", json!({})),
        (
            "a client naming the device-code grant",
            json!({"grantTypes": [GRANT_TYPE_DEVICE_CODE]}),
        ),
        (
            "a client naming the device-code and refresh-token grants",
            json!({"grantTypes": [GRANT_TYPE_DEVICE_CODE, GRANT_TYPE_REFRESH_TOKEN]}),
        ),
        (
            "a client scoped to the same scope the portal serves",
            json!({"scopes": ["sso:account:access"]}),
        ),
    ] {
        let (id, secret) = emulator.registered_client(registration).await;
        presenters.push((case, id, secret));
    }

    for (case, presenter_id, presenter_secret) in &presenters {
        let (status, refused) = emulator
            .create_token(&json!({
                "clientId": presenter_id,
                "clientSecret": presenter_secret,
                "grantType": GRANT_TYPE_DEVICE_CODE,
                "deviceCode": device_code,
            }))
            .await;
        assert_invalid_device_code(case, status, &refused);
        assert_eq!(
            refused["error_description"].as_str(),
            unknown["error_description"].as_str(),
            "case {case:?}: the refusal must read exactly as an unknown device code's, got {refused}"
        );

        // The refused attempt neither consumed nor invalidated the code: the
        // issuing client's next poll is still the pending refusal.
        assert_eq!(
            stored_status(&emulator, &device_code),
            Some(DeviceAuthStatus::Pending),
            "case {case:?}: a refused attempt must leave the authorization pending"
        );
        let (status, body) = emulator.create_token(&owner_poll).await;
        assert_pending(&format!("the owner's poll after {case}"), status, &body);
    }

    // And the code is still redeemable by its own client once a user approves it,
    // which is what proves the refused attempts spent nothing.
    emulator.approve(&user_code);
    let (status, token) = emulator.create_token(&owner_poll).await;
    assert_eq!(status, 200, "the approved poll answered {token}");
    let access_token = text(&token, "accessToken");
    let state = emulator.state();
    let store = state.lock();
    assert_eq!(
        store
            .access_tokens
            .get(&access_token)
            .map(|issued| issued.client_id.clone()),
        Some(owner_id.clone()),
        "the minted token must be persisted for the issuing client, got {store:?}"
    );
    assert_eq!(
        store.device_auths.get(&device_code).map(|r| r.status),
        Some(DeviceAuthStatus::Consumed),
        "the redeemed device code must be recorded as consumed"
    );
}

/// RULE: the approval is recorded on ONE authorization, so only the authorization
/// a user approved becomes redeemable while every other authorization of the same
/// client stays pending. The table varies which of three concurrent
/// authorizations of one client is approved.
#[tokio::test]
async fn only_the_authorization_a_user_approved_becomes_redeemable() {
    for approved_index in 0..3_usize {
        let emulator = Emulator::new();
        let (client_id, client_secret) = emulator.registered_client(json!({})).await;

        let mut authorizations = Vec::new();
        for _ in 0..3 {
            authorizations
                .push(started_authorization(&emulator, &client_id, &client_secret).await);
        }

        let (approved_code, approved_user_code) = authorizations[approved_index].clone();
        emulator.approve(&approved_user_code);

        for (index, (device_code, _)) in authorizations.iter().enumerate() {
            let case = format!("authorization {index} with {approved_index} approved");
            let poll = json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": GRANT_TYPE_DEVICE_CODE,
                "deviceCode": device_code,
            });
            let (status, body) = emulator.create_token(&poll).await;
            if index == approved_index {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the approved authorization must be redeemable, got {body}"
                );
                // Approved and redeemed: the device code is spent, so a second
                // poll of it is an invalid grant.
                let (status, replayed) = emulator.create_token(&poll).await;
                assert_invalid_device_code(&case, status, &replayed);
            } else {
                assert_pending(&case, status, &body);
            }
        }

        assert_eq!(
            stored_status(&emulator, &approved_code),
            Some(DeviceAuthStatus::Consumed),
            "case {approved_index}: the redeemed authorization must be recorded as consumed"
        );
    }
}
