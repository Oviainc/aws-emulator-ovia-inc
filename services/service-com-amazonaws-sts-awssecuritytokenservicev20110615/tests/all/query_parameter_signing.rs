// SPDX-License-Identifier: CC0-1.0

//! A call whose SigV4 fields live in the QUERY STRING is served, exactly as one whose
//! fields live in the `Authorization` header is.
//!
//! RULE: SigV4 has two schemes — "Authenticating Requests: Using Authorization Header"
//! and "Authenticating Requests: Using Query Parameters", the one a presigned URL uses —
//! and an `awsQuery` service accepts either, so the WHOLE call is read from the query
//! string when that is where it was signed: the `Action` that names the operation, the
//! input members, and the `X-Amz-Credential` scope that says who is calling. A presigned
//! `GET` therefore answers what the same call made as a signed `POST` with the same
//! credential answers, and acts on the parameters it carried. A query field never
//! out-ranks a body that names an action, and a query `Action` naming no modelled
//! operation is still the protocol's `InvalidAction`.
//!
//! The rows differ along what the rule generalises over: which operation the query names,
//! whether it carries input members or omits a required one, whether the credential is a
//! long-term key or a session this service minted, and whether the name resolves at all.

use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;

use super::sts_sessions::{
    call, element, emulator, emulator_account, member, seed_demo_role, LONG_TERM_KEY,
    STS_API_VERSION, STS_SIGNING_NAME,
};
use smithy_server_runtime::MultiServiceRouter;

/// The SigV4 algorithm a presigned URL declares, which is what marks the query-parameter
/// scheme (`X-Amz-Algorithm`).
const SIGV4_ALGORITHM: &str = "AWS4-HMAC-SHA256";

/// One call made the way a presigned URL is fetched: a `GET` of the root path whose
/// whole call — `Action`, `Version`, the input members `fields` names, and the SigV4
/// fields — is in the query string, and which carries NO `Authorization` header and no
/// body at all.
///
/// The signature value is a placeholder: these suites run with request authentication
/// off (`emulator()`), so what is under test is that the request is CLAIMED, ROUTED,
/// DESERIALIZED and attributed to the credential its scope names — not the signing
/// arithmetic, which the framework owns for both schemes alike.
async fn presigned_get(
    router: &MultiServiceRouter,
    access_key_id: &str,
    action: &str,
    fields: &[(&str, String)],
) -> (u16, String) {
    let credential =
        format!("{access_key_id}%2F20260101%2Fus-east-1%2F{STS_SIGNING_NAME}%2Faws4_request");
    let mut query = format!(
        "Action={action}&Version={STS_API_VERSION}&X-Amz-Algorithm={SIGV4_ALGORITHM}\
         &X-Amz-Credential={credential}&X-Amz-Date=20260101T000000Z&X-Amz-Expires=900\
         &X-Amz-SignedHeaders=host&X-Amz-Signature=0123456789abcdef"
    );
    for (name, value) in fields {
        query.push('&');
        query.push_str(name);
        query.push('=');
        query.push_str(value);
    }
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("https://{STS_SIGNING_NAME}.amazonaws.com/?{query}"))
        .header("host", format!("{STS_SIGNING_NAME}.amazonaws.com"))
        .body(bytes::Bytes::new())
        .expect("a well-formed presigned request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// A presigned `GetCallerIdentity` answers the SAME identity the signed POST answers for
/// the same credential — for the long-term key and for a session this service minted.
#[tokio::test]
async fn a_presigned_call_is_served_as_the_credential_its_scope_names() {
    let (router, engine) = emulator();
    let (role_arn, role_id) = seed_demo_role(&engine);
    let account = emulator_account();

    // A session credential of this emulator's own minting, so one row's caller is a
    // temporary credential rather than the long-term key.
    let (status, body) = call(
        &router,
        LONG_TERM_KEY,
        "AssumeRole",
        &[
            ("RoleArn", role_arn),
            ("RoleSessionName", "presigned".to_string()),
        ],
    )
    .await;
    assert_eq!(status, 200, "the session under test: {status} {body}");
    let session_key = member("the session under test", &body, "AccessKeyId");

    // (case, the key id the presigned URL is scoped to, the identity it must report)
    let rows: Vec<(&str, String, String)> = vec![
        (
            "the long-term key the emulator publishes",
            LONG_TERM_KEY.to_string(),
            format!("arn:aws:iam::{account}:user/emulator"),
        ),
        (
            "a session this service minted",
            session_key.clone(),
            format!("arn:aws:sts::{account}:assumed-role/demo/presigned"),
        ),
    ];

    for (case, key_id, expected_arn) in rows {
        let (status, body) = presigned_get(&router, &key_id, "GetCallerIdentity", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from a presigned GetCallerIdentity, got {status} \
             {body}"
        );
        let presigned_arn = member(case, &body, "Arn");
        assert_eq!(
            presigned_arn, expected_arn,
            "case {case:?}: expected the presigned call to speak for {expected_arn}, got \
             {presigned_arn}"
        );

        // The signed POST with the SAME credential is the control: the two schemes are
        // two ways of presenting one credential, so they report one identity.
        let (status, posted) = call(&router, &key_id, "GetCallerIdentity", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the signed POST control: {status} {posted}"
        );
        for field in ["Arn", "Account", "UserId"] {
            assert_eq!(
                element(&body, field),
                element(&posted, field),
                "case {case:?}: expected the presigned call and the signed POST to report the \
                 same {field}, got {body} and {posted}"
            );
        }
    }

    // The session's own UserId is the role id IAM holds, which is what makes the
    // presigned answer correlatable back to the role rather than merely well-formed.
    let (_, body) = presigned_get(&router, &session_key, "GetCallerIdentity", &[]).await;
    assert_eq!(
        element(&body, "UserId"),
        Some(format!("{role_id}:presigned")),
        "expected the presigned session call to report the role id IAM issued, got {body}"
    );
}

/// Which operation a presigned `GET` reaches, over the actions a query string can name.
#[tokio::test]
async fn a_query_string_action_routes_to_the_operation_it_names() {
    let (router, engine) = emulator();
    seed_demo_role(&engine);

    // (case, the Action the query names, the status it must answer, an element the answer
    // must carry). `GetAccessKeyInfo`'s `AccessKeyId` is `required: true`
    // (`smithy_query(query="operation", operation="GetAccessKeyInfo")`), so the row that
    // names it pins that a routed presigned call is walked and validated like any other
    // rather than served as an empty one.
    type Row = (&'static str, &'static str, u16, &'static str);
    let rows: Vec<Row> = vec![
        (
            "an operation with no input members",
            "GetCallerIdentity",
            200,
            "Arn",
        ),
        (
            "an operation that mints a session",
            "GetSessionToken",
            200,
            "SessionToken",
        ),
        (
            "an operation whose required member the query does not carry",
            "GetAccessKeyInfo",
            400,
            "Code",
        ),
        (
            "an action no modelled operation answers",
            "NotAnStsAction",
            400,
            "Code",
        ),
    ];

    for (case, action, expected_status, expected_element) in rows {
        let (status, body) = presigned_get(&router, LONG_TERM_KEY, action, &[]).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for Action={action}, got {status} \
             {body}"
        );
        assert!(
            element(&body, expected_element).is_some(),
            "case {case:?}: expected a <{expected_element}> element in the answer, got {body}"
        );
        assert!(
            body.starts_with("<?xml") || body.starts_with('<'),
            "case {case:?}: expected this service's XML envelope, got {body}"
        );
    }

    // An unresolvable action is the protocol's own routing failure, unchanged.
    let (status, body) = presigned_get(&router, LONG_TERM_KEY, "NotAnStsAction", &[]).await;
    assert_eq!(
        (status, element(&body, "Code").as_deref()),
        (400, Some("InvalidAction")),
        "expected 400 InvalidAction for an action no operation answers, got {status} {body}"
    );
}

/// A member the request does not carry is refused the same way under both schemes: once
/// routed, a presigned call is walked and validated exactly as the POST path is.
#[tokio::test]
async fn a_presigned_call_is_refused_for_what_it_omits_exactly_as_the_post_path_is() {
    let (router, _engine) = emulator();

    let (presigned_status, presigned) =
        presigned_get(&router, LONG_TERM_KEY, "GetAccessKeyInfo", &[]).await;
    let (posted_status, posted) = call(&router, LONG_TERM_KEY, "GetAccessKeyInfo", &[]).await;
    assert_eq!(
        (presigned_status, element(&presigned, "Code")),
        (posted_status, element(&posted, "Code")),
        "expected the two schemes to refuse an omitted required member alike, got {presigned} \
         and {posted}"
    );
}

/// The input members a presigned call carries in its query string are the ones the
/// handler acts on — a call is never served with its parameters dropped.
///
/// The rows differ in the KIND of member the query carries: a required string the answer
/// is computed from, an integer the minted session's own length is set by, and a member
/// whose value the model constrains and the request violates.
#[tokio::test]
async fn a_presigned_call_acts_on_the_members_its_query_string_carries() {
    let (router, engine) = emulator();
    let account = emulator_account();

    // A required STRING member: `GetAccessKeyInfo` answers the account of the key id the
    // REQUEST names, so an answer that ignored the query would have nothing to report.
    let (status, body) = presigned_get(
        &router,
        LONG_TERM_KEY,
        "GetAccessKeyInfo",
        &[("AccessKeyId", LONG_TERM_KEY.to_string())],
    )
    .await;
    assert_eq!(
        (status, element(&body, "Account")),
        (200, Some(account.clone())),
        "expected the presigned GetAccessKeyInfo to resolve the key id its query named, got \
         {status} {body}"
    );

    // An INTEGER member that changes stored state: the session the call mints must carry
    // the length the query asked for, read back out of the registry rather than off the
    // response, so a dropped parameter cannot pass.
    let asked_for = 900;
    let (status, body) = presigned_get(
        &router,
        LONG_TERM_KEY,
        "GetSessionToken",
        &[("DurationSeconds", asked_for.to_string())],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from a presigned GetSessionToken, got {status} {body}"
    );
    let minted = member("the presigned mint", &body, "AccessKeyId");
    let recorded = engine
        .state::<sts::ComAmazonawsStsAwssecuritytokenservicev20110615State>()
        .lock()
        .session(&minted)
        .map(|session| session.duration_seconds);
    assert_eq!(
        recorded,
        Some(asked_for),
        "expected the minted session to carry the DurationSeconds the query named, got \
         {recorded:?}"
    );

    // A member whose value the MODEL constrains: `DurationSeconds` is `range 900..129600`
    // (`smithy_query(query="operation", operation="GetSessionToken")`), so a query value
    // below it is refused — which it could not be if the query were never read.
    let (status, body) = presigned_get(
        &router,
        LONG_TERM_KEY,
        "GetSessionToken",
        &[("DurationSeconds", "60".to_string())],
    )
    .await;
    assert_eq!(
        (status, element(&body, "Code").as_deref()),
        (400, Some("ValidationError")),
        "expected a query value outside the modelled range to be refused, got {status} {body}"
    );

    // And the BODY still wins where a request carries one: the same action named in a
    // form-encoded POST body is read from the body, so nothing is routed or deserialized
    // from a query field in preference to the bytes the signature covered.
    let (status, body) = call(
        &router,
        LONG_TERM_KEY,
        "GetAccessKeyInfo",
        &[("AccessKeyId", LONG_TERM_KEY.to_string())],
    )
    .await;
    assert_eq!(
        (status, element(&body, "Account")),
        (200, Some(account)),
        "expected the signed POST path to be unchanged, got {status} {body}"
    );
}
