// SPDX-License-Identifier: CC0-1.0

//! What STS ADMITS, over the two classes of request it refuses before it answers.
//!
//! Two rules, each asserted over a table rather than one recorded request.
//!
//! **A key id that is not an access key id has no account.** `GetAccessKeyInfo` "returns
//! the account identifier for the specified access key ID" (the operation's
//! documentation), so the question is only asked of an access key id: twenty characters,
//! each an upper-case letter or a digit — the shape of AWS's own
//! (`AKIAIOSFODNN7EXAMPLE`, the example the same documentation gives) and of every key id
//! this emulator's credential store mints. The modelled `accessKeyIdType` is wider
//! (`@length` 16..128, `@pattern ^[\w]*$`,
//! `smithy_query(query="shape", shape="accessKeyIdType")`), so a value inside those bounds
//! that is not a key id reaches the handler and is refused there: HTTP 400 `ValidationError`
//! with the service's own fixed sentence, which carries nothing of the request.
//!
//! **An MFA challenge is decided against the device the account holds.** "MFA-enabled IAM
//! users must call `GetSessionToken` and submit an MFA code that is associated with their
//! MFA device … An incorrect MFA code causes the API to return an access denied error"
//! (`GetSessionToken`'s documentation), and "if the role being assumed requires MFA and if
//! the `TokenCode` value is missing or expired, the `AssumeRole` call returns an 'access
//! denied' error" (`AssumeRoleRequest$TokenCode`) — so the two operations whose input
//! carries `SerialNumber` and `TokenCode`
//! (`smithy_query(query="operation", operation="AssumeRole,GetSessionToken")`) refuse a
//! challenge the account cannot validate with HTTP 403 `AccessDenied`, before any
//! credential is minted. A request that presents no challenge is unaffected.
//!
//! No row below SUCCEEDS a challenge, deliberately: the only MFA device kind this
//! emulator's IAM holds is a FIDO security key, which authenticates by challenge and
//! response and answers no one-time pass code, and the operation that would register a
//! device answering one (`iam:CreateVirtualMFADevice`) is still a 501 stub. So the rows
//! cover the two refusals and the served no-challenge case, and the success leg becomes
//! testable when a code-answering device kind exists.
//!
//! Every call goes over the awsQuery wire through the same `MultiServiceRouter` the
//! emulator serves with.

use service_com_amazonaws_iam_awsidentitymanagementv20100508 as iam;

use super::sts_sessions::{
    call, element, emulator, emulator_account, member, now_secs, seed_demo_role, LONG_TERM_KEY,
};

/// The error code STS answers a request it refused over the values it carried.
const VALIDATION_ERROR: &str = "ValidationError";

/// The error code STS answers a refusal of the caller with.
const ACCESS_DENIED: &str = "AccessDenied";

/// The `<Type>` an `awsQuery` error envelope carries for a refusal of the SENDER, which
/// every 4xx of these two rules is.
const SENDER: &str = "Sender";

/// The sentence AWS refuses a value that is not an access key id with.
const NOT_AN_ACCESS_KEY_ID: &str = "Access key ID is not valid.";

/// The sentence AWS refuses an MFA challenge naming no device of the caller with. Written
/// in two pieces so the two spaces after the first sentence stay visible: they are part of
/// the fixed text.
const MFA_SERIAL_NOT_ASSOCIATED: &str = concat!(
    "MultiFactorAuthentication failed, unable to validate MFA code.",
    "  Please verify your MFA serial number is valid and associated with this user."
);

/// The sentence AWS refuses a code the caller's own device does not answer with.
const MFA_INVALID_PASS_CODE: &str =
    "MultiFactorAuthentication failed with invalid MFA one time pass code.";

/// The serial number a virtual MFA device of the IAM user `user` is addressed by: "an
/// Amazon Resource Name (ARN) for a virtual device (such as
/// arn:aws:iam::123456789012:mfa/user)" (`GetSessionTokenRequest$SerialNumber`).
fn mfa_serial(user: &str) -> String {
    format!("arn:aws:iam::{}:mfa/{user}", emulator_account())
}

/// Enrol an MFA device for the IAM user `user` in the IAM store of the SAME shared engine
/// — the registry STS reads a challenge against — and answer the serial it is addressed
/// by.
fn enrol_mfa_device(engine: &engine::InMemoryEngine, user: &str) -> String {
    let serial_number = mfa_serial(user);
    engine
        .state::<iam::ComAmazonawsIamAwsidentitymanagementv20100508State>()
        .lock()
        .fido_security_keys
        .insert(
            serial_number.clone(),
            iam::state::FidoSecurityKey {
                serial_number: serial_number.clone(),
                user_key: iam::state::name_key(user),
                enable_date: aws_smithy_types::DateTime::from_secs(now_secs()),
                certifications: std::collections::BTreeMap::new(),
            },
        );
    serial_number
}

// ---------------------------------------------------------------------------
// GetAccessKeyInfo: what counts as an access key id
// ---------------------------------------------------------------------------

/// A value the model admits but AWS does not recognise as an access key id is refused,
/// and the refusal states the fixed sentence without echoing the value.
///
/// The rows differ along every dimension the rule generalises over: the LENGTH (the
/// model's own minimum of 16, one under and one over twenty, and the model's maximum of
/// 128), the CHARACTER SET (lower case, mixed case, and the underscore `^[\w]*$` admits),
/// the four-character PREFIX (one AWS assigns to another kind of entity, and one it
/// assigns to nothing), and the BODY (digits outside the base32 alphabet the owning
/// account is encoded in).
#[tokio::test]
async fn get_access_key_info_refuses_a_value_that_is_not_an_access_key_id() {
    let (router, _engine) = emulator();
    let rejected: Vec<(&str, String)> = vec![
        ("16 characters, the modelled minimum", "NOTANACCESSKEYID".to_string()),
        ("19 characters, one under", "EMULATORAKTESTKEY01".to_string()),
        ("21 characters, one over", "EMULATORAKTESTKEY0012".to_string()),
        ("128 characters, the modelled maximum", "A".repeat(128)),
        ("twenty, all lower case", "emulatorakalllower01".to_string()),
        ("twenty, mixed case", "emulatorakTestKey001".to_string()),
        ("twenty, carrying an underscore", "EMULATOR_AKTESTKEY01".to_string()),
        // The prefix names no kind of access key: `AROA` is a role's unique-id prefix and
        // `NOTA` is nothing at all. Both bodies are inside the base32 alphabet, so these
        // two rows turn on the prefix alone.
        ("a role's unique-id prefix", "AROAEXAMPLEROLEID234".to_string()),
        ("a prefix AWS assigns to nothing", "NOTAREALACCESSKEYID2".to_string()),
        // The body carries digits the base32 alphabet the owning account is encoded in
        // does not have (`0`, `1`, `8`, `9`).
        ("a long-term prefix with a body of digits", "AKIA1234567890ABCDEF".to_string()),
        ("a temporary prefix with a body of digits", "ASIAEXAMPLEKEY123456".to_string()),
    ];

    for (case, value) in rejected {
        let (status, body) = call(
            &router,
            LONG_TERM_KEY,
            "GetAccessKeyInfo",
            &[("AccessKeyId", value.clone())],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {value:?}, got {status} {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some(VALIDATION_ERROR),
            "case {case:?}: expected the {VALIDATION_ERROR} code, got {body}"
        );
        assert_eq!(
            element(&body, "Type").as_deref(),
            Some(SENDER),
            "case {case:?}: expected a {SENDER} error, got {body}"
        );
        let message = member(case, &body, "Message");
        assert_eq!(
            message, NOT_AN_ACCESS_KEY_ID,
            "case {case:?}: expected the fixed refusal sentence, got {message:?}"
        );
        assert!(
            !message.contains(&value) && !message.contains("accessKeyId"),
            "case {case:?}: the refusal must carry nothing of the request, got {message:?}"
        );
        assert!(
            element(&body, "Account").is_none(),
            "case {case:?}: a refused request must report no account, got {body}"
        );
        assert!(
            element(&body, "RequestId").is_some(),
            "case {case:?}: the awsQuery error envelope must carry a RequestId: {body}"
        );
    }
}

/// A value that IS an access key id is still answered from the registry: AWS's own
/// published shapes, this emulator's long-term key, and a key id this service minted in
/// the same run — read back through the operation that reports it.
#[tokio::test]
async fn get_access_key_info_answers_every_well_formed_access_key_id() {
    let (router, _engine) = emulator();

    // A key this service minted in this run, so one row is a key id the emulator issued
    // rather than one the test wrote.
    let (status, body) = call(&router, LONG_TERM_KEY, "GetSessionToken", &[]).await;
    assert_eq!(status, 200, "minting a session token: {body}");
    let minted = member("a minted session", &body, "AccessKeyId");

    let admitted: Vec<(&str, String)> = vec![
        ("the emulator's own long-term key", LONG_TERM_KEY.to_string()),
        // The example the operation's own documentation gives for a long-term key.
        ("a long-term AWS key id", "AKIAIOSFODNN7EXAMPLE".to_string()),
        // "Access key IDs beginning with ASIA are temporary credentials that are created
        // using STS operations" (the same documentation).
        ("a temporary AWS key id", "ASIAEXAMPLEKEY234567".to_string()),
        ("a key id this service minted", minted),
    ];

    for (case, value) in admitted {
        let (status, body) = call(
            &router,
            LONG_TERM_KEY,
            "GetAccessKeyInfo",
            &[("AccessKeyId", value.clone())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for {value:?}, got {status} {body}"
        );
        assert_eq!(
            member(case, &body, "Account"),
            emulator_account(),
            "case {case:?}: expected the account the key belongs to, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The MFA challenge
// ---------------------------------------------------------------------------

/// One challenge: the `SerialNumber` and `TokenCode` a request carries, and the sentence
/// the refusal must state — or `None` for a request that presents no challenge and is
/// served.
struct Challenge {
    case: &'static str,
    serial_number: Option<String>,
    token_code: Option<String>,
    refusal: Option<&'static str>,
}

/// Neither operation vends credentials on a challenge the account cannot validate, and
/// each refuses it the way AWS words that refusal.
///
/// The rows differ along what the rule generalises over: whether the serial names a device
/// the account holds at all, whether the device it names is the CALLER's, whether the
/// challenge is complete, and the code presented — plus the row that presents no challenge,
/// which is served, so the rule refuses the challenge rather than the operation.
#[tokio::test]
async fn an_mfa_challenge_is_decided_against_the_device_the_account_holds() {
    for action in ["GetSessionToken", "AssumeRole"] {
        let (router, engine) = emulator();
        let (role_arn, _role_id) = seed_demo_role(&engine);
        // Two devices: one enrolled for the IAM user the caller's long-term key speaks
        // for, one enrolled for a different user of the same account.
        let own_device = enrol_mfa_device(
            &engine,
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::state::AMBIENT_USER_NAME,
        );
        let other_device = enrol_mfa_device(&engine, "someone-else");

        let challenges = vec![
            Challenge {
                case: "no challenge at all",
                serial_number: None,
                token_code: None,
                refusal: None,
            },
            Challenge {
                case: "a serial the account holds no device under",
                serial_number: Some("GAHT12345678".to_string()),
                token_code: Some("123456".to_string()),
                refusal: Some(MFA_SERIAL_NOT_ASSOCIATED),
            },
            Challenge {
                case: "a device of another user of the account",
                serial_number: Some(other_device.clone()),
                token_code: Some("123456".to_string()),
                refusal: Some(MFA_SERIAL_NOT_ASSOCIATED),
            },
            Challenge {
                case: "a serial with no code to validate",
                serial_number: Some(own_device.clone()),
                token_code: None,
                refusal: Some(MFA_SERIAL_NOT_ASSOCIATED),
            },
            Challenge {
                case: "a code with no device named",
                serial_number: None,
                token_code: Some("123456".to_string()),
                refusal: Some(MFA_SERIAL_NOT_ASSOCIATED),
            },
            Challenge {
                case: "the caller's own device, with a code it does not answer",
                serial_number: Some(own_device.clone()),
                token_code: Some("000000".to_string()),
                refusal: Some(MFA_INVALID_PASS_CODE),
            },
            Challenge {
                case: "the caller's own device, with another code it does not answer",
                serial_number: Some(own_device.clone()),
                token_code: Some("999999".to_string()),
                refusal: Some(MFA_INVALID_PASS_CODE),
            },
        ];

        for challenge in challenges {
            let case = format!("{action}: {}", challenge.case);
            let mut params: Vec<(&str, String)> = Vec::new();
            if action == "AssumeRole" {
                params.push(("RoleArn", role_arn.clone()));
                params.push(("RoleSessionName", "mfa".to_string()));
            }
            if let Some(serial_number) = &challenge.serial_number {
                params.push(("SerialNumber", serial_number.clone()));
            }
            if let Some(token_code) = &challenge.token_code {
                params.push(("TokenCode", token_code.clone()));
            }

            let (status, body) = call(&router, LONG_TERM_KEY, action, &params).await;
            let Some(expected) = challenge.refusal else {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a request presenting no challenge must be served, got \
                     {status} {body}"
                );
                assert!(
                    element(&body, "AccessKeyId").is_some(),
                    "case {case:?}: expected minted credentials, got {body}"
                );
                continue;
            };
            assert_eq!(
                status, 403,
                "case {case:?}: expected HTTP 403, got {status} {body}"
            );
            assert_eq!(
                element(&body, "Code").as_deref(),
                Some(ACCESS_DENIED),
                "case {case:?}: expected the {ACCESS_DENIED} code, got {body}"
            );
            assert_eq!(
                element(&body, "Type").as_deref(),
                Some(SENDER),
                "case {case:?}: expected a {SENDER} error, got {body}"
            );
            let message = member(&case, &body, "Message");
            assert_eq!(
                message, expected,
                "case {case:?}: expected AWS's own MFA refusal sentence, got {message:?}"
            );
            // The refusal echoes neither half of the challenge.
            if let Some(serial_number) = &challenge.serial_number {
                assert!(
                    !message.contains(serial_number.as_str()),
                    "case {case:?}: the refusal must not echo the serial number, got {message:?}"
                );
            }
            if let Some(token_code) = &challenge.token_code {
                assert!(
                    !message.contains(token_code.as_str()),
                    "case {case:?}: the refusal must not echo the code, got {message:?}"
                );
            }
            assert!(
                element(&body, "AccessKeyId").is_none(),
                "case {case:?}: a refused challenge must mint no credentials, got {body}"
            );
            assert!(
                element(&body, "RequestId").is_some(),
                "case {case:?}: the awsQuery error envelope must carry a RequestId: {body}"
            );
        }
    }
}

/// A refused challenge mints nothing: the session registry is as empty after it as before,
/// so no credential the caller never received is left behind for a later call to resolve.
#[tokio::test]
async fn a_refused_mfa_challenge_records_no_session() {
    let (router, engine) = emulator();
    let own_device = enrol_mfa_device(
        &engine,
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::state::AMBIENT_USER_NAME,
    );

    for (case, serial_number, token_code) in [
        ("an unheld serial", "GAHT12345678".to_string(), "123456"),
        ("the caller's own device", own_device, "123456"),
    ] {
        let (status, body) = call(
            &router,
            LONG_TERM_KEY,
            "GetSessionToken",
            &[
                ("SerialNumber", serial_number),
                ("TokenCode", token_code.to_string()),
            ],
        )
        .await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected HTTP 403, got {status} {body}"
        );
        let sessions = engine
            .state::<service_com_amazonaws_sts_awssecuritytokenservicev20110615::state::ComAmazonawsStsAwssecuritytokenservicev20110615State>()
            .lock()
            .sessions
            .len();
        assert_eq!(
            sessions, 0,
            "case {case:?}: a refused challenge must record no session, found {sessions}"
        );
    }
}
