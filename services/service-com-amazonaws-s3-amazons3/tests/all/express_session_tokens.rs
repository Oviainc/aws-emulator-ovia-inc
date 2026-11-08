// SPDX-License-Identifier: CC0-1.0

//! S3 Express One Zone: what the SESSION TOKEN a Zonal endpoint request presents authorizes.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle and object suites in this crate already own), so what is pinned is the
//! wire behaviour — status, error code and message.
//!
//! The rule under test: a client that has taken a session on a directory bucket "appl[ies]
//! the session token to the `x-amz-s3session-token` request header for authorization", and
//! "temporary security credentials are scoped to the bucket and expire after 5 minutes"
//! (`CreateSession`'s own documentation) — so a presented token authorizes the Zonal endpoint
//! calls on the bucket ITS OWN session was created for, for as long as that session lives,
//! and nothing else. A token no session carries is `InvalidToken`, one whose session the
//! clock has expired is `ExpiredToken`, and one whose live session was created for another
//! bucket is `AccessDenied` — the three codes Amazon S3 publishes for these conditions.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_s3_amazons3::service::express_session::SESSION_TOKEN_HEADER;
use service_com_amazonaws_s3_amazons3::state::ComAmazonawsS3Amazons3State;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router_with_engine, send, send_bytes_signed, Answer};
use super::directory_bucket_sessions_and_appends::directory_bucket;
use super::object_and_tagging_roundtrip::{put_object, text};

/// `GET /<bucket>?session` — `CreateSession`, and the `(AccessKeyId, SessionToken)` it
/// vended.
async fn session_credentials(router: &MultiServiceRouter, bucket_name: &str) -> (String, String) {
    let vended = send(router, "GET", &format!("/{bucket_name}?session"), &[], "").await;
    assert_eq!(
        vended.status, 200,
        "a session on {bucket_name:?} must be vended first, got {} with body {:?}",
        vended.status,
        text(&vended)
    );
    let key_id = vended.element("AccessKeyId").unwrap_or_default();
    let token = vended.element("SessionToken").unwrap_or_default();
    for (member, value) in [("AccessKeyId", &key_id), ("SessionToken", &token)] {
        assert!(
            !value.is_empty(),
            "the session on {bucket_name:?} must carry a {member}, got {:?}",
            text(&vended)
        );
    }
    (key_id, token)
}

/// One Zonal endpoint call SIGNED with a session's own key pair and presenting no session
/// token, which is the other way a request names the session it acts under.
///
/// Driven through the one place this crate's harness puts a request on the wire; the key id
/// in the credential scope is what this case varies, and neither it nor the signature is
/// verified here (this suite's router verifies none, as the other suites' do).
async fn signed_with_session_key(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    key_id: &str,
) -> Answer {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={key_id}/20260101/us-east-1/s3express/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    );
    send_bytes_signed(
        router,
        &authorization,
        "GET",
        "s3express-use1-az4.us-east-1.amazonaws.com",
        &format!("/{bucket_name}/{key}"),
        &[],
        b"",
    )
    .await
}

/// One Zonal endpoint call, as `(case, method, the path and query under the bucket, the body)`
/// — the calls a client makes under a session.
type ZonalCall = (&'static str, &'static str, &'static str, &'static [u8]);

/// The Zonal endpoint calls this suite drives under a token, one per kind of data-plane call
/// a session is used for: a read, a write, a listing and a rename.
const ZONAL_CALLS: [ZonalCall; 4] = [
    ("a read", "GET", "/session-object.txt", b""),
    ("a write", "PUT", "/session-written.txt", b"written under a session"),
    ("a listing", "GET", "?list-type=2", b""),
    ("a rename", "PUT", "/session-renamed.txt?renameObject", b""),
];

/// Drive `call` against `bucket_name` presenting `token`.
async fn under_token(
    router: &MultiServiceRouter,
    bucket_name: &str,
    call: ZonalCall,
    token: &str,
) -> Answer {
    let (_, method, path, body) = call;
    let mut headers: Vec<(&str, &str)> = vec![(SESSION_TOKEN_HEADER, token)];
    // The rename names its source, which is the object this suite stores in every bucket.
    if path.contains("renameObject") {
        headers.push(("x-amz-rename-source", "session-object.txt"));
    }
    let uri = format!("/{bucket_name}{path}");
    super::bucket_lifecycle::send_bytes(router, method, &uri, &headers, body).await
}

/// RULE: a presented session token authorizes the Zonal endpoint calls on the bucket its own
/// session was created for, and refuses every other request — with the code S3 publishes for
/// the condition that refused it.
///
/// The rows differ along WHAT the token is (the bucket's own live session, another bucket's
/// live session, a session the clock has expired, and a token no session carries) and the
/// assertions run over a set of Zonal calls, so the rule is pinned for the data plane rather
/// than for one operation.
#[tokio::test]
async fn a_session_token_authorizes_only_its_own_bucket() {
    let (router, engine) = router_with_engine();
    let own = "sessiontoken-own--use1-az4--x-s3";
    let other = "sessiontoken-other--use1-az5--x-s3";
    directory_bucket(&router, own, "use1-az4").await;
    directory_bucket(&router, other, "use1-az5").await;
    for name in [own, other] {
        let stored = put_object(&router, name, "session-object.txt", b"stored bytes", &[]).await;
        assert_eq!(
            stored.status, 200,
            "the object must be stored in {name:?} first, got {} with body {:?}",
            stored.status,
            text(&stored)
        );
    }

    let (own_key_id, own_token) = session_credentials(&router, own).await;
    let (_, other_token) = session_credentials(&router, other).await;

    // A session the CLOCK has already expired, recorded directly in the store: no sequence of
    // requests can reach that state, because the documented lifetime is five minutes.
    let expired_token = "an-expired-sessions-token";
    let expired_key_id = "EMULATORAKEXPIREDSESSION";
    {
        let state = engine.state::<ComAmazonawsS3Amazons3State>();
        let mut store = state.lock();
        assert!(
            store.record_vended_session_on(
                expired_key_id,
                SystemTime::now() - Duration::from_secs(60),
                own,
                expired_token,
            ),
            "the store must take the expired session this case is about"
        );
    }

    // (case, the token presented, the status and code expected, or `None` for a call served)
    type Refused = Option<(u16, &'static str)>;
    let rows: [(&str, &str, Refused); 4] = [
        ("the bucket's own live session", own_token.as_str(), None),
        (
            "another bucket's live session",
            other_token.as_str(),
            Some((403, "AccessDenied")),
        ),
        (
            "a session the clock has expired",
            expired_token,
            Some((400, "ExpiredToken")),
        ),
        (
            "a token no session carries",
            "not-a-token-this-emulator-vended",
            Some((400, "InvalidToken")),
        ),
    ];
    for (case, token, refused) in rows {
        for call in ZONAL_CALLS {
            let (which, _, _, _) = call;
            let answered = under_token(&router, own, call, token).await;
            match refused {
                None => assert_eq!(
                    answered.status, 200,
                    "case {case:?} ({which}): expected the call to be served, got {} with body \
                     {:?}",
                    answered.status,
                    text(&answered)
                ),
                Some((status, code)) => assert_eq!(
                    (answered.status, answered.error_code().as_str()),
                    (status, code),
                    "case {case:?} ({which}): expected {status} {code:?}, got {} {:?} with body \
                     {:?}",
                    answered.status,
                    answered.error_code(),
                    text(&answered)
                ),
            }
            assert!(
                !answered.error_message().contains(token),
                "case {case:?} ({which}): a refusal must not echo the token, got {:?}",
                answered.error_message()
            );
        }
    }

    // The other half of the rule: the token of the OTHER bucket's session authorizes that
    // bucket's own calls, so what refused the rows above is the pairing and not the token.
    let served = under_token(&router, other, ZONAL_CALLS[0], other_token.as_str()).await;
    assert_eq!(
        served.status, 200,
        "case \"the other bucket's own session\": expected the call to be served, got {} with \
         body {:?}",
        served.status,
        text(&served)
    );

    // The SCOPE and the EXPIRY belong to the credentials too, not only to the token they are
    // presented with: a call signed with a session's own key pair reaches that session's
    // bucket while that session lives, token or no token.
    //
    // (case, the key id signed with, the bucket addressed, the key read there, the status and
    // code expected)
    for (case, key_id, addressed, key, status, code) in [
        (
            "the bucket the session was created for",
            own_key_id.as_str(),
            own,
            "session-renamed.txt",
            200,
            "",
        ),
        (
            "a bucket the session was not created for",
            own_key_id.as_str(),
            other,
            "session-object.txt",
            403,
            "AccessDenied",
        ),
        (
            "a session the clock has expired",
            expired_key_id,
            own,
            "session-renamed.txt",
            400,
            "ExpiredToken",
        ),
    ] {
        let answered = signed_with_session_key(&router, addressed, key, key_id).await;
        match code.is_empty() {
            true => assert_eq!(
                answered.status, status,
                "case {case:?}: expected the call to be served, got {} with body {:?}",
                answered.status,
                text(&answered)
            ),
            false => assert_eq!(
                (answered.status, answered.error_code().as_str()),
                (status, code),
                "case {case:?}: expected {status} {code:?}, got {} {:?} with body {:?}",
                answered.status,
                answered.error_code(),
                text(&answered)
            ),
        }
    }
    // A credential that is not a live session's says nothing about the bucket it addresses,
    // which is what leaves every other request of this service exactly as it was.
    let unscoped = signed_with_session_key(
        &router,
        other,
        "session-object.txt",
        "EMULATORAKNOTASESSION",
    )
    .await;
    assert_eq!(
        unscoped.status, 200,
        "case \"a credential that is not a session's\": expected the call to be served, got {} \
         with body {:?}",
        unscoped.status,
        text(&unscoped)
    );

    // A request that presents NO session token presents no session, so this rule has nothing
    // to say about it: it is served as every other request of this emulator is. The key it
    // reads is the one the served rename above moved the object to.
    let unsessioned = super::bucket_lifecycle::send_bytes(
        &router,
        "GET",
        &format!("/{own}/session-renamed.txt"),
        &[],
        b"",
    )
    .await;
    assert_eq!(
        unsessioned.status, 200,
        "case \"no session token at all\": expected the call to be served, got {} with body \
         {:?}",
        unsessioned.status,
        text(&unsessioned)
    );
}
