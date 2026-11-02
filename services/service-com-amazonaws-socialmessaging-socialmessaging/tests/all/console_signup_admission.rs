// SPDX-License-Identifier: CC0-1.0

//! Who is admitted to the console-only door, and what the refusal says.
//!
//! RULE: an operation this service serves only through the console's sign-up flow admits
//! just the requests that arrive through that flow — the ones carrying the console CSRF
//! header — and answers every other one with the access-denied envelope the console-facing
//! front end frames: HTTP 403, the Coral-located error code in `x-amzn-errortype` and in
//! the body's `__type`, and the fixed text under a CAPITALISED `Message`. The refusal is
//! decided from the request alone, before anything is read or written, so a refused call
//! records no signup, mints no token and leaves the store as it found it.
//!
//! The refused rows span HOW the header is missing — absent, empty, whitespace, a
//! near-miss name, an unrelated header — and WHICH form of the input the body carries (a
//! signup callback, a setup finalization, both), because neither is what the admission
//! turns on. The admitted rows span the token VALUE, which is not read either: any
//! non-empty one is a request that came through the flow.

use serde_json::{json, Value};
use service_com_amazonaws_socialmessaging_socialmessaging::state::ComAmazonawsSocialmessagingSocialmessagingState as Store;

use crate::harness::{
    call, call_with_headers, console_call, emulator, emulator_with_engine, CONSOLE_CSRF_HEADER,
    CONSOLE_CSRF_TOKEN,
};

/// The error code this refusal carries, in `x-amzn-errortype` and in the body's `__type`
/// (`CONSOLE_ACCESS_DENIED_ERROR_TYPE` in `src/scaffolded.rs`): the access-denied shape
/// with the Coral locator the console-facing front end appends.
const CORAL_ACCESS_DENIED: &str =
    "AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/";

/// What the refusal says (`CSRF_HEADER_MISSING_MESSAGE` in `src/state.rs`): fixed text,
/// with nothing of the request in it.
const CSRF_MISSING: &str = "CSRF header does not exist";

/// A signup-callback body: the form that starts a sign-up.
fn callback_body() -> Value {
    json!({ "signupCallback": { "accessToken": "meta-access-token" } })
}

/// A setup-finalization body: the form that finishes one. The token names no signup, which
/// is beside the point — admission is decided before any lookup, so this body is refused
/// for the same reason the callback is.
fn finalization_body() -> Value {
    json!({
        "setupFinalization": {
            "associateInProgressToken": "signup-00000000000000000000000000000000",
            "phoneNumbers": [{ "id": "phone-number-id-aaaa", "twoFactorPin": "123456" }],
            "waba": { "id": "waba-11111111111111111111111111111111" },
        }
    })
}

/// Both members at once: still one request, still decided the same way.
fn both_body() -> Value {
    json!({
        "signupCallback": { "accessToken": "meta-access-token" },
        "setupFinalization": {
            "associateInProgressToken": "signup-00000000000000000000000000000000",
            "phoneNumbers": [{ "id": "phone-number-id-bbbb", "twoFactorPin": "123456" }],
        }
    })
}

#[tokio::test]
async fn a_signup_that_did_not_come_through_the_console_is_refused_untouched() {
    let (router, engine) = emulator_with_engine();

    // How the console header can fail to be presented. A header spelled differently is
    // not this header, and neither is this header with nothing in it.
    let absences: [(&str, Vec<(&str, &str)>); 5] = [
        ("no headers beyond the signed ones", vec![]),
        ("the console header, empty", vec![(CONSOLE_CSRF_HEADER, "")]),
        (
            "the console header, whitespace only",
            vec![(CONSOLE_CSRF_HEADER, "   ")],
        ),
        (
            "a near-miss header name",
            vec![("x-amz-csrf-token", CONSOLE_CSRF_TOKEN)],
        ),
        (
            "an unrelated header",
            vec![("x-requested-with", "XMLHttpRequest")],
        ),
    ];
    let bodies = [
        ("a signup callback", callback_body()),
        ("a setup finalization", finalization_body()),
        ("both members", both_body()),
    ];

    for (absence, headers) in &absences {
        for (form, body) in &bodies {
            let case = format!("{absence}, {form}");
            let answer = call_with_headers(
                &router,
                "POST",
                "/v1/whatsapp/signup",
                Some(body.clone()),
                headers,
            )
            .await;
            assert_eq!(
                answer.status, 403,
                "case {case:?}: expected 403, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_type, CORAL_ACCESS_DENIED,
                "case {case:?}: expected the error type {CORAL_ACCESS_DENIED:?}, got {:?}",
                answer.error_type
            );
            assert_eq!(
                answer.at("/__type"),
                &json!(CORAL_ACCESS_DENIED),
                "case {case:?}: expected the body's __type to carry {CORAL_ACCESS_DENIED:?}, \
                 got {:?}",
                answer.at("/__type")
            );
            assert_eq!(
                answer.at("/Message"),
                &json!(CSRF_MISSING),
                "case {case:?}: expected Message {CSRF_MISSING:?}, got {:?}",
                answer.at("/Message")
            );
            assert_eq!(
                answer.body.get("message"),
                None,
                "case {case:?}: this refusal carries its text under Message alone, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.at("/signupCallbackResult"),
                &Value::Null,
                "case {case:?}: a refused signup mints no token, got {:?}",
                answer.body
            );
        }
    }

    // Nothing the refusals touched: the store holds no signup and no account, and the
    // account's own listing still answers. A signup the console DOES make then works,
    // which is what shows the refused ones consumed no capacity.
    {
        let store = engine.state::<Store>();
        let store = store.lock();
        assert!(
            store.pending_signups.is_empty() && store.linked_accounts.is_empty(),
            "after the refusals: expected an untouched store, got {} signups and {} accounts",
            store.pending_signups.len(),
            store.linked_accounts.len()
        );
    }
    let listed = call(&router, "GET", "/v1/whatsapp/waba/list", None).await;
    assert_eq!(
        (listed.status, listed.at("/linkedAccounts")),
        (200, &json!([])),
        "after the refusals: expected an empty listing, got {} {:?}",
        listed.status,
        listed.body
    );

    let started = console_call(
        &router,
        "POST",
        "/v1/whatsapp/signup",
        Some(callback_body()),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "the console's own callback: expected 200, got {} {:?}",
        started.status, started.body
    );
    let token = started.string_at("/signupCallbackResult/associateInProgressToken");
    assert!(
        !token.is_empty(),
        "the console's own callback: expected a minted token, got {:?}",
        started.body
    );
    let recorded = engine.state::<Store>().lock().pending_signups.len();
    assert_eq!(
        recorded, 1,
        "the console's own callback: expected the one signup it started, got {recorded}"
    );
}

#[tokio::test]
async fn any_console_token_is_admitted_and_the_signup_runs() {
    // The admission reads only whether the header is there, so these rows differ in the
    // token's length and alphabet and all run the same sign-up.
    let tokens = [
        CONSOLE_CSRF_TOKEN,
        "0",
        "a",
        "A1b2-C3d4_E5f6.G7h8",
        &"t".repeat(512),
    ];
    for token in tokens {
        let router = emulator();
        let started = call_with_headers(
            &router,
            "POST",
            "/v1/whatsapp/signup",
            Some(callback_body()),
            &[(CONSOLE_CSRF_HEADER, token)],
        )
        .await;
        let case = format!("a console token of {} characters", token.len());
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            started.status, started.body
        );
        let in_progress = started.string_at("/signupCallbackResult/associateInProgressToken");
        assert!(
            !in_progress.is_empty() && in_progress.len() <= 50,
            "case {case:?}: expected a 1..50 character associateInProgressToken, got {:?}",
            started.body
        );

        // ROUND TRIP: the admitted signup finalizes, and the account it linked reads back.
        let finalized = call_with_headers(
            &router,
            "POST",
            "/v1/whatsapp/signup",
            Some(json!({
                "setupFinalization": {
                    "associateInProgressToken": in_progress,
                    "phoneNumbers": [{ "id": "phone-number-id-cccc", "twoFactorPin": "123456" }],
                    "waba": { "id": "waba-22222222222222222222222222222222" },
                }
            })),
            &[(CONSOLE_CSRF_HEADER, token)],
        )
        .await;
        assert_eq!(
            finalized.status, 200,
            "case {case:?}: expected 200 from the finalization, got {} {:?}",
            finalized.status, finalized.body
        );
        let id = finalized.string_at("/linkedWhatsAppBusinessAccountId");
        let read = call(
            &router,
            "GET",
            &format!("/v1/whatsapp/waba/details?id={id}"),
            None,
        )
        .await;
        assert_eq!(
            (read.status, read.string_at("/account/id")),
            (200, id.clone()),
            "case {case:?}: expected the linked account {id:?} to read back, got {} {:?}",
            read.status,
            read.body
        );
    }
}
