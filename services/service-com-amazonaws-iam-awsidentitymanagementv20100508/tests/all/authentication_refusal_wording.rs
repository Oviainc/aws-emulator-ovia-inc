// SPDX-License-Identifier: CC0-1.0

//! How this service words a request the emulator's AUTHENTICATION layer refused, driven end
//! to end over the awsQuery wire with request authentication ON — the configuration the
//! emulator serves traffic in.
//!
//! RULE: a request whose SigV4 access key id resolves to no credential this emulator holds
//! failed AUTHENTICATION — no principal was found, so nothing could be authorized — and is
//! refused with this service's unknown-credential code (`InvalidClientTokenId`, HTTP 403)
//! carrying AWS's fixed sentence; a request whose key id DOES resolve, whose signature does
//! not verify under that credential's secret, failed the SIGNATURE and is refused
//! `SignatureDoesNotMatch` with the SigV4 layer's own sentence. Neither is `AccessDenied`,
//! which is what a KNOWN principal is answered for an action its policies do not permit: an
//! SDK re-resolves its credential chain on an authentication failure and reports a policy
//! problem on an authorization one, so collapsing them hides a stale or mistyped key behind
//! a permissions diagnosis.
//!
//! The rows differ along what the rule generalises over: the SHAPE of the key id the request
//! presented (the long-term `AKIA` class, the temporary `ASIA` class, one of the emulator's
//! own published form, and one a single character off a key the emulator really holds),
//! which of the two credential failures it is, and the operation the refused request named —
//! a read that resolves its caller from the signing key, an account-wide read, and a
//! MUTATION — so what is answered follows from the credential rather than from the call.
//!
//! The wording itself belongs to the whole tree rather than to this suite, so the sentences
//! are read from the module that owns them
//! (`service_common::authentication_refusal`) and only the CODES — which are this
//! service's own — are stated here.

use std::sync::Arc;

use service_common::authentication_refusal::{
    SIGNATURE_MISMATCH_MESSAGE, UNRESOLVED_CREDENTIAL_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{call, element, elements, HARNESS_REGION};

/// The code AWS answers a request whose signing credential resolves to nothing with. An
/// authentication outcome, so it is NOT `AccessDenied`.
const UNRESOLVED_CREDENTIAL_CODE: &str = "InvalidClientTokenId";

/// The code AWS answers a request whose credential DID resolve and whose signature did not
/// verify with — the SigV4 code every `awsQuery` service names for that one condition.
const SIGNATURE_MISMATCH_CODE: &str = "SignatureDoesNotMatch";

/// The authorization denial, which no row below may be answered: it is what a resolved
/// principal gets for an action its policies do not permit.
const AUTHORIZATION_DENIAL: &str = "AccessDenied";

/// A signature value that is not the one the emulator computes for any of these requests:
/// 64 hex digits, the shape a SigV4 signature has, so what fails is the comparison and not
/// the parse.
const WRONG_SIGNATURE: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// The instant every request below signs at, in SigV4's own `YYYYMMDDTHHMMSSZ` form, and the
/// scope date it falls on — read from the CLOCK rather than written down, because a request
/// signed at a fixed instant is refused for the difference between that instant and the
/// current time once the clock has moved past the window SigV4 admits, and what these rows
/// are about is the CREDENTIAL. Both halves come from one reading, so the scope date and the
/// signing instant can never name different days.
fn signed_at() -> (String, String) {
    let rendered = ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now())
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("the current instant renders in RFC 3339");
    let (date, time) = rendered
        .split_once('T')
        .expect("an RFC 3339 instant separates its date and time with 'T'");
    let date: String = date.chars().filter(|held| held.is_ascii_digit()).collect();
    let time: String = time
        .chars()
        .filter(|held| held.is_ascii_digit())
        .take(6)
        .collect();
    (format!("{date}T{time}Z"), date)
}

/// The user a MUTATION row names, so the store can be read afterwards to show the refused
/// write never happened.
const REFUSED_USER: &str = "authentication-refusal-wording";

/// Which credential a row presents.
#[derive(Debug, Clone, Copy)]
enum Presented {
    /// An access key id this emulator never issued, written out as the row names it.
    NeverIssued(&'static str),
    /// The key id the emulator PUBLISHED for this run, one character off: a credential of
    /// exactly the issued form that still resolves to nothing.
    OneCharacterOffThePublishedKey,
    /// The key id the emulator published, which resolves — so what the row is about is the
    /// signature computed under its secret.
    PublishedKey,
}

/// One operation a row presents its credential to.
///
/// The set spans how an IAM operation reaches state: a read that resolves its caller FROM
/// the signing key (`GetUser` with no `UserName`), an account-wide read that names nothing
/// (`ListRoles`), and a MUTATION (`CreateUser`) — so a refusal is shown to land ahead of
/// each of them rather than only ahead of a read.
const CALLS: &[(&str, &[(&str, &str)])] = &[
    ("GetUser", &[]),
    ("ListRoles", &[]),
    ("CreateUser", &[("UserName", REFUSED_USER)]),
];

/// A router serving only this service with request authentication ON, plus the access key id
/// the emulator published for this run — the one credential a refusal has RESOLVED.
///
/// Shared with the suite about WHICH user that published credential belongs to
/// (`tests/all/caller_identity_resolution.rs`), which turns the authenticator back off
/// before it drives its calls: the credential-reading block is the same either way, and
/// `vended_credentials` is the only place that id can be read.
pub(crate) fn authenticating_router() -> (MultiServiceRouter, String) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(true);
    let access_key_id = match router.vended_credentials().into_iter().next() {
        Some(emulator_server::VendedCredentials::AwsAccessKey { access_key_id, .. }) => {
            access_key_id
        }
        other => panic!(
            "with authentication on the emulator must vend a SigV4 key pair, got {other:?}"
        ),
    };
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    (router, access_key_id)
}

/// The published key id with its last character replaced, which is a key id of the issued
/// form that the emulator holds no credential for.
fn one_character_off(access_key_id: &str) -> String {
    let mut characters: Vec<char> = access_key_id.chars().collect();
    let last = characters
        .pop()
        .expect("the emulator's published key id is not empty");
    characters.push(if last == 'A' { 'B' } else { 'A' });
    characters.into_iter().collect()
}

/// Dispatch one awsQuery call signed with `access_key_id` and a signature that is not the
/// one the emulator computes, answering the HTTP status and the XML body.
async fn signed_call(
    router: &MultiServiceRouter,
    access_key_id: &str,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push_str(&format!("&{name}={value}"));
    }
    let (signing_instant, signing_date) = signed_at();
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/{signing_date}/{HARNESS_REGION}/iam/\
         aws4_request, SignedHeaders=host;x-amz-date, Signature={WRONG_SIGNATURE}"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("https://iam.amazonaws.com/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("x-amz-date", &signing_instant)
        .header("authorization", authorization)
        .body(bytes::Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8(response.body().to_vec()).expect("a UTF-8 response body");
    (status, body)
}

/// One row: the credential the request presented, and the code and sentence AWS answers for
/// that credential's failure.
struct Row {
    case: &'static str,
    presented: Presented,
    code: &'static str,
    message: &'static str,
}

/// Every credential failure is answered with the code AWS names for THAT failure, at HTTP
/// 403, with AWS's fixed sentence — and never with the authorization denial.
#[tokio::test]
async fn a_credential_failure_is_answered_with_the_code_aws_names_for_it() {
    let (mut router, published) = authenticating_router();
    let off_by_one = one_character_off(&published);

    let rows = vec![
        Row {
            case: "a never-issued long-term key id",
            presented: Presented::NeverIssued("AKIAIOSFODNN7EXAMPLE"),
            code: UNRESOLVED_CREDENTIAL_CODE,
            message: UNRESOLVED_CREDENTIAL_MESSAGE,
        },
        Row {
            case: "a never-issued temporary key id",
            presented: Presented::NeverIssued("ASIAIOSFODNN7EXAMPLE"),
            code: UNRESOLVED_CREDENTIAL_CODE,
            message: UNRESOLVED_CREDENTIAL_MESSAGE,
        },
        Row {
            case: "a never-issued key id of the emulator's own form",
            presented: Presented::NeverIssued("EMULATORDEADBEEF0123"),
            code: UNRESOLVED_CREDENTIAL_CODE,
            message: UNRESOLVED_CREDENTIAL_MESSAGE,
        },
        Row {
            case: "a key id one character off one the emulator holds",
            presented: Presented::OneCharacterOffThePublishedKey,
            code: UNRESOLVED_CREDENTIAL_CODE,
            message: UNRESOLVED_CREDENTIAL_MESSAGE,
        },
        Row {
            case: "a key id the emulator holds, with a signature that does not verify",
            presented: Presented::PublishedKey,
            code: SIGNATURE_MISMATCH_CODE,
            message: SIGNATURE_MISMATCH_MESSAGE,
        },
    ];

    for Row {
        case,
        presented,
        code: expected_code,
        message: expected_message,
    } in rows
    {
        let access_key_id = match presented {
            Presented::NeverIssued(key) => key.to_string(),
            Presented::OneCharacterOffThePublishedKey => off_by_one.clone(),
            Presented::PublishedKey => published.clone(),
        };
        for (action, params) in CALLS {
            let (status, body) = signed_call(&router, &access_key_id, action, params).await;
            assert_eq!(
                status, 403,
                "case {case:?} on {action}: expected HTTP 403 for a credential failure, got \
                 {status}: {body}"
            );
            let (code, message) = (element(&body, "Code"), element(&body, "Message"));
            assert_eq!(
                (code.as_str(), message.as_str()),
                (expected_code, expected_message),
                "case {case:?} on {action}: expected {expected_code} with AWS's own wording, \
                 got {code:?}: {message:?}"
            );
            assert_ne!(
                code.as_str(), AUTHORIZATION_DENIAL,
                "case {case:?} on {action}: an authentication failure must not be answered as \
                 an authorization denial, got {body}"
            );
        }
    }

    // The refused MUTATION never reached a handler: with authentication off again, the
    // account holds no user of the name every refused `CreateUser` above named.
    router.set_request_authentication(false);
    let (status, listed) = call(&router, "ListUsers", &[]).await;
    assert_eq!(
        status, 200,
        "case the store after the refusals: ListUsers must be served, got {status}: {listed}"
    );
    let names = elements(&listed, "UserName");
    assert!(
        !names.iter().any(|name| name == REFUSED_USER),
        "case the store after the refusals: a refused CreateUser must write nothing, got \
         {names:?}"
    );
}
