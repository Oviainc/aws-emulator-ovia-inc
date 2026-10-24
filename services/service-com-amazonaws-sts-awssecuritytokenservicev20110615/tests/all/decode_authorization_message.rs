// SPDX-License-Identifier: CC0-1.0

//! `DecodeAuthorizationMessage` end-to-end: the opaque blob an authorization failure
//! hands a caller is decoded back into the decision the account recorded, and a blob
//! this account did not issue is refused.
//!
//! The rules under test:
//!
//! * a message this account issued decodes, for as long as its validity window lasts,
//!   to the document of the decision recorded when it was issued — and decoding is a
//!   read, so the same message decodes again to the same document;
//! * the blob itself carries none of the decision's detail, which is the whole reason
//!   the message is encoded;
//! * anything else — bytes that are not base64 at all, base64 of something this
//!   account never issued, a message whose validity window has run out — is refused
//!   with `InvalidAuthorizationMessageException` (HTTP 400), reporting the bytes of
//!   the message it could not read.
//!
//! Every call goes over the wire as the `application/x-www-form-urlencoded` awsQuery
//! POST a client makes, through the same router the emulator serves with.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;
use smithy_server_runtime::MultiServiceRouter;
use sts::state::{
    AuthorizationDecision, ComAmazonawsStsAwssecuritytokenservicev20110615State as StsState,
    ENCODED_MESSAGE_VALIDITY, MAX_AUTHORIZATION_MESSAGES,
};

use crate::sts_sessions::{call, element, emulator, member, LONG_TERM_KEY};

/// One `DecodeAuthorizationMessage` call carrying `encoded_message`.
async fn decode(router: &MultiServiceRouter, encoded_message: &str) -> (u16, String) {
    call(
        router,
        LONG_TERM_KEY,
        "DecodeAuthorizationMessage",
        &[("EncodedMessage", encoded_message.to_string())],
    )
    .await
}

/// The bytes a refusal reports, rendered the way the refusal renders them: signed
/// 8-bit values, comma-separated, in brackets.
///
/// Computed here with widening arithmetic rather than by calling the service's own
/// renderer, so the test pins the wording instead of echoing it.
fn expected_refusal(bytes: &[u8]) -> String {
    let rendered: Vec<String> = bytes
        .iter()
        .map(|byte| {
            let signed = i16::from(*byte) - if *byte > 127 { 256 } else { 0 };
            signed.to_string()
        })
        .collect();
    format!("Message is not valid: [{}]", rendered.join(", "))
}

/// The text of an XML element with the five predefined entities resolved, so a JSON
/// document carried in element content reads back exactly as it was serialized.
fn unescape_xml(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// `bytes` in the standard base64 alphabet, the codec the wire serializer uses.
fn base64(bytes: &[u8]) -> String {
    aws_smithy_types::base64::encode(bytes)
}

/// The bytes a blob carries, read the way the service reads one: the URL-safe
/// alphabet mapped back onto the standard one, with the padding restored.
fn blob_bytes(case: &str, blob: &str) -> Vec<u8> {
    let standard = blob.replace('-', "+").replace('_', "/");
    let padded = format!("{standard}{}", "=".repeat((4 - standard.len() % 4) % 4));
    aws_smithy_types::base64::decode(padded)
        .unwrap_or_else(|error| panic!("case {case:?}: the blob must be base64 ({error}): {blob}"))
}

/// `count` bytes from a seeded linear congruential generator (the constants are
/// Numerical Recipes'), so a blob of "random" bytes is reproducible from its seed and
/// a failing case can be replayed from the seed the assertion prints.
fn pseudo_random_bytes(seed: u64, count: usize) -> Vec<u8> {
    let mut state = seed;
    (0..count)
        .map(|_| {
            state = state
                .wrapping_mul(1_664_525)
                .wrapping_add(1_013_904_223);
            u8::try_from((state >> 24) & 0xff).expect("a masked byte fits in a u8")
        })
        .collect()
}

/// The decision a denied request recorded, with everything the document carries.
fn decision(
    allowed: bool,
    explicit_deny: bool,
    action: &str,
    resource: &str,
    principal: (&str, &str),
    matched: &[(&str, &str)],
    conditions: &[(&str, &[&str])],
) -> AuthorizationDecision {
    AuthorizationDecision {
        allowed,
        explicit_deny,
        principal_id: principal.0.to_string(),
        principal_arn: principal.1.to_string(),
        action: action.to_string(),
        resource: resource.to_string(),
        conditions: conditions
            .iter()
            .map(|(key, values)| {
                (
                    (*key).to_string(),
                    values.iter().map(|value| (*value).to_string()).collect(),
                )
            })
            .collect(),
        matched_statements: matched
            .iter()
            .map(|(id, effect)| ((*id).to_string(), (*effect).to_string()))
            .collect(),
    }
}

/// Record `decision` in the account's registry and return the encoded message a denied
/// request would have handed its caller.
fn issue(
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    issued_at: SystemTime,
    decision: &AuthorizationDecision,
) -> String {
    let state = engine.state::<StsState>();
    let mut state = state.lock();
    sts::state::encode_authorization_message(&mut state, issued_at, decision)
}

/// How many records the registry holds right now.
fn held(engine: &std::sync::Arc<engine::InMemoryEngine>) -> usize {
    engine
        .state::<StsState>()
        .lock()
        .authorization_messages
        .len()
}

// ---------------------------------------------------------------------------
// The decode a message this account issued
// ---------------------------------------------------------------------------

/// A message this account issued decodes to the decision it recorded: whether the
/// request was allowed, whether a deny was explicit, the statements the decision
/// matched, and the context — principal, action, resource and condition keys — it was
/// decided in.
///
/// The rows differ along what the rule generalises over: the decision itself (denied
/// implicitly, denied explicitly, allowed), the number of matched statements (none,
/// one, two), the number of condition keys and of values under one key (none, one,
/// two), the kind of principal (an IAM user, an assumed role), and the service the
/// action and resource belong to.
#[tokio::test]
async fn an_issued_message_decodes_to_the_decision_the_account_recorded() {
    let (router, engine) = emulator();
    let account = engine::emulator_account_id().to_string();
    let user = (
        "AIDAEXAMPLEUSERID0001",
        format!("arn:aws:iam::{account}:user/alice"),
    );
    let role = (
        "AROAEXAMPLEROLEID0001:s1",
        format!("arn:aws:sts::{account}:assumed-role/demo/s1"),
    );
    let cases = [
        (
            "denied with no statement matched",
            decision(
                false,
                false,
                "ec2:RunInstances",
                &format!("arn:aws:ec2:us-east-1:{account}:instance/*"),
                (user.0, &user.1),
                &[],
                &[],
            ),
        ),
        (
            "denied by an explicit deny, one statement, one condition key",
            decision(
                false,
                true,
                "s3:GetObject",
                "arn:aws:s3:::demo-bucket/reports/q1.csv",
                (user.0, &user.1),
                &[("DenyUnencrypted", "Deny")],
                &[("aws:SecureTransport", &["false"])],
            ),
        ),
        (
            "allowed, two statements, two condition keys one of which has two values",
            decision(
                true,
                false,
                "dynamodb:PutItem",
                &format!("arn:aws:dynamodb:eu-west-1:{account}:table/orders"),
                (role.0, &role.1),
                &[("AllowWrites", "Allow"), ("RequireTag", "Allow")],
                &[
                    (
                        "aws:PrincipalTag/team",
                        &["platform", "platform-oncall"] as &[&str],
                    ),
                    ("aws:RequestedRegion", &["eu-west-1"] as &[&str]),
                ],
            ),
        ),
        (
            "an assumed-role principal and an action of another service",
            decision(
                false,
                true,
                "lambda:InvokeFunction",
                &format!("arn:aws:lambda:ap-south-1:{account}:function:nightly"),
                (role.0, &role.1),
                &[("DenyProd", "Deny")],
                &[("lambda:FunctionArn", &["arn:aws:lambda:*:*:function:*"])],
            ),
        ),
    ];

    let now = SystemTime::now();
    let mut blobs: BTreeSet<String> = BTreeSet::new();
    for (case, decided) in &cases {
        let blob = issue(&engine, now, decided);
        assert!(
            blobs.insert(blob.clone()),
            "case {case:?}: every issued message must be its own blob, {blob} repeats"
        );
        assert!(
            !blob.contains(['+', '/', '=']),
            "case {case:?}: an issued message travels in the URL-safe alphabet with no padding, got {blob}"
        );

        // The blob carries none of the decision: that is why the message is encoded.
        let carried = blob_bytes(case, &blob);
        let carried = String::from_utf8_lossy(&carried).to_string();
        for secret in [&decided.action, &decided.resource, &decided.principal_arn] {
            assert!(
                !carried.contains(secret.as_str()),
                "case {case:?}: the blob must not carry {secret:?}, it reads {carried:?}"
            );
        }

        let (status, body) = decode(&router, &blob).await;
        assert_eq!(status, 200, "case {case:?}: {body}");
        let rendered = unescape_xml(&member(case, &body, "DecodedMessage"));
        let document: Value = serde_json::from_str(&rendered).unwrap_or_else(|error| {
            panic!("case {case:?}: the decoded message must be a JSON document ({error}): {rendered}")
        });

        assert_eq!(
            document["allowed"],
            json!(decided.allowed),
            "case {case:?}: allowed, got {document}"
        );
        assert_eq!(
            document["explicitDeny"],
            json!(decided.explicit_deny),
            "case {case:?}: explicitDeny, got {document}"
        );
        assert_eq!(
            document["context"]["principal"]["id"],
            json!(decided.principal_id),
            "case {case:?}: the principal id, got {document}"
        );
        assert_eq!(
            document["context"]["principal"]["arn"],
            json!(decided.principal_arn),
            "case {case:?}: the principal ARN, got {document}"
        );
        assert_eq!(
            document["context"]["action"],
            json!(decided.action),
            "case {case:?}: the action, got {document}"
        );
        assert_eq!(
            document["context"]["resource"],
            json!(decided.resource),
            "case {case:?}: the resource, got {document}"
        );

        let matched = document["matchedStatements"]["items"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: matchedStatements.items must be a list, got {document}"));
        assert_eq!(
            matched.len(),
            decided.matched_statements.len(),
            "case {case:?}: matched statement count, got {document}"
        );
        for (index, (id, effect)) in decided.matched_statements.iter().enumerate() {
            assert_eq!(
                (&matched[index]["statementId"], &matched[index]["effect"]),
                (&json!(id), &json!(effect)),
                "case {case:?}: matched statement {index}, got {document}"
            );
        }

        let conditions = document["context"]["conditions"]["items"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: conditions.items must be a list, got {document}"));
        assert_eq!(
            conditions.len(),
            decided.conditions.len(),
            "case {case:?}: condition key count, got {document}"
        );
        for (index, (key, values)) in decided.conditions.iter().enumerate() {
            assert_eq!(
                conditions[index]["key"],
                json!(key),
                "case {case:?}: condition key {index}, got {document}"
            );
            let reported: Vec<String> = conditions[index]["values"]["items"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: condition {index} values, got {document}"))
                .iter()
                .map(|value| value["value"].as_str().unwrap_or_default().to_string())
                .collect();
            assert_eq!(
                &reported, values,
                "case {case:?}: condition {index} values, got {document}"
            );
        }
        assert_eq!(
            document["failures"]["items"],
            json!([]),
            "case {case:?}: nothing here fails to read a policy, got {document}"
        );

        // Decoding is a read: the same message decodes again, to the same document,
        // and the record is still held.
        let (again_status, again_body) = decode(&router, &blob).await;
        assert_eq!(
            (again_status, unescape_xml(&member(case, &again_body, "DecodedMessage"))),
            (200, rendered.clone()),
            "case {case:?}: decoding twice must answer the same document, got {again_body}"
        );
    }
    assert_eq!(
        held(&engine),
        cases.len(),
        "every issued message must still be held after being decoded"
    );
}

// ---------------------------------------------------------------------------
// The refusal of a message this account did not issue
// ---------------------------------------------------------------------------

/// A message this account did not issue is refused with
/// `InvalidAuthorizationMessageException` (HTTP 400), reporting the bytes of the
/// message it could not read: the decoded bytes when the blob is base64, and the
/// blob's own bytes when it is not base64 at all.
///
/// The rows differ along what the rule generalises over: the content of the blob
/// (random bytes, text, a near-miss of a message this account issued), its length
/// (the modelled one-character floor, the 10,240-character ceiling), the alphabet it
/// is written in (standard, URL-safe) and whether it is base64 at all (padded,
/// unpadded, carrying a line break, carrying a character outside both alphabets).
#[tokio::test]
async fn a_message_this_account_did_not_issue_is_refused_with_the_bytes_it_carried() {
    let (router, engine) = emulator();

    // A near miss: a message this account DID issue, with the last byte of the token
    // it names changed, so the envelope is this account's and the record is not.
    let issued = issue(
        &engine,
        SystemTime::now(),
        &decision(
            false,
            false,
            "ec2:StopInstances",
            "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0",
            ("AIDAEXAMPLEUSERID0001", "arn:aws:iam::000000000000:user/alice"),
            &[],
            &[],
        ),
    );
    let mut tampered = blob_bytes("a near miss", &issued);
    let last = tampered.len() - 1;
    tampered[last] ^= 0x01;
    let recorded_before = held(&engine);

    // 7,680 bytes encode to exactly 10,240 base64 characters, the modelled ceiling.
    let longest = pseudo_random_bytes(0x5eed_0004, 7_680);
    // Bytes whose base64 uses `+` and `/`, so swapping in the URL-safe alphabet's
    // `-` and `_` really changes every one of them.
    let url_safe_source = [0xfb_u8, 0xff, 0xbf];
    let unpadded_source = pseudo_random_bytes(0x5eed_0005, 10);
    // The class the corpus records: a blob in the URL-safe alphabet with its padding
    // stripped, which is how an encoded authorization message travels.
    let url_safe_realistic = pseudo_random_bytes(0x5eed_0006, 64);
    let outside_the_alphabet = {
        let encoded = base64(&pseudo_random_bytes(0x5eed_0007, 24));
        format!("{}*{}", &encoded[..8], &encoded[8..])
    };
    let line_broken = {
        let encoded = base64(&pseudo_random_bytes(0x5eed_0002, 24));
        format!("{}\n{}", &encoded[..16], &encoded[16..])
    };

    let cases: [(&str, String, Vec<u8>); 11] = [
        (
            "a realistic blob of random bytes",
            base64(&pseudo_random_bytes(0x5eed_0001, 256)),
            pseudo_random_bytes(0x5eed_0001, 256),
        ),
        (
            "base64 of plain text",
            base64(b"not an encoded authorization message"),
            b"not an encoded authorization message".to_vec(),
        ),
        (
            "this account's envelope naming a token it never issued",
            base64(&tampered),
            tampered.clone(),
        ),
        (
            "base64 of a single zero byte",
            base64(&[0]),
            vec![0],
        ),
        (
            "the shortest message the model allows",
            "A".to_string(),
            b"A".to_vec(),
        ),
        (
            "the longest message the model allows",
            base64(&longest),
            longest.clone(),
        ),
        (
            "valid base64 with its padding stripped",
            base64(&unpadded_source).trim_end_matches('=').to_string(),
            unpadded_source.clone(),
        ),
        (
            "a blob carrying a line break",
            line_broken.clone(),
            line_broken.clone().into_bytes(),
        ),
        (
            "a blob carrying a character outside both alphabets",
            outside_the_alphabet.clone(),
            outside_the_alphabet.clone().into_bytes(),
        ),
        (
            "a blob in the URL-safe alphabet",
            base64(&url_safe_source).replace('+', "-").replace('/', "_"),
            url_safe_source.to_vec(),
        ),
        (
            "a realistic blob: the URL-safe alphabet with no padding",
            base64(&url_safe_realistic)
                .replace('+', "-")
                .replace('/', "_")
                .trim_end_matches('=')
                .to_string(),
            url_safe_realistic.clone(),
        ),
    ];

    for (case, blob, carried) in &cases {
        let (status, body) = decode(&router, blob).await;
        assert_eq!(
            (status, element(&body, "Code").as_deref()),
            (400, Some("InvalidAuthorizationMessageException")),
            "case {case:?}: expected a 400 InvalidAuthorizationMessageException, got {status} {body}"
        );
        assert_eq!(
            member(case, &body, "Message"),
            expected_refusal(carried),
            "case {case:?}: the refusal must report the bytes of the message it could not read"
        );
    }

    assert_eq!(
        held(&engine),
        recorded_before,
        "a refused message must persist nothing"
    );
}

/// The same rule over a bounded sweep of pseudo-random blobs: none of them is a
/// message this account issued, so each is refused reporting its own decoded bytes,
/// whichever length it has and whichever of the two alphabets it is written in. The
/// seed is printed by every assertion, so a failing case replays.
#[tokio::test]
async fn random_blobs_are_refused_reporting_their_own_bytes() {
    let (router, engine) = emulator();
    let seed: u64 = 0x5eed_0a11;
    for case in 0..24_u64 {
        // Lengths 1..=47 bytes, so the blob length varies with the case.
        let length = usize::try_from(case * 2 + 1).expect("a small length");
        let bytes = pseudo_random_bytes(seed.wrapping_add(case), length);
        // Half the rows travel in the URL-safe alphabet with the padding stripped, the
        // way an encoded authorization message does.
        let url_safe = case % 2 == 1;
        let blob = if url_safe {
            base64(&bytes)
                .replace('+', "-")
                .replace('/', "_")
                .trim_end_matches('=')
                .to_string()
        } else {
            base64(&bytes)
        };
        let (status, body) = decode(&router, &blob).await;
        assert_eq!(
            (status, element(&body, "Code").as_deref()),
            (400, Some("InvalidAuthorizationMessageException")),
            "case seed {seed:#x}+{case} ({length} bytes, url-safe {url_safe}): expected a 400 refusal, got {status} {body}"
        );
        assert_eq!(
            member("random", &body, "Message"),
            expected_refusal(&bytes),
            "case seed {seed:#x}+{case} ({length} bytes, url-safe {url_safe}): the refusal must report the blob's own bytes"
        );
    }
    assert_eq!(
        held(&engine),
        0,
        "a refused message must persist nothing"
    );
}

// ---------------------------------------------------------------------------
// Expiry and the registry's bound
// ---------------------------------------------------------------------------

/// A message decodes until its validity window runs out and not a moment longer: one
/// issued inside the window is served, one issued a window ago or longer is refused,
/// and the record of an expired message is pruned by the next call.
///
/// The rows put the same window on either side of the clock, including the exact
/// boundary.
#[tokio::test]
async fn a_message_stops_decoding_when_its_validity_window_runs_out() {
    let (router, engine) = emulator();
    let now = SystemTime::now();
    let minute = Duration::from_secs(60);
    let cases: [(&str, Duration, u16); 5] = [
        ("issued now", Duration::ZERO, 200),
        ("issued halfway through the window", ENCODED_MESSAGE_VALIDITY / 2, 200),
        (
            "issued a minute before the window runs out",
            ENCODED_MESSAGE_VALIDITY - minute,
            200,
        ),
        ("issued exactly a window ago", ENCODED_MESSAGE_VALIDITY, 400),
        (
            "issued a minute past the window",
            ENCODED_MESSAGE_VALIDITY + minute,
            400,
        ),
    ];

    for (case, age, expected_status) in cases {
        let decided = decision(
            false,
            false,
            "ec2:DescribeInstances",
            "arn:aws:ec2:us-east-1:000000000000:instance/*",
            ("AIDAEXAMPLEUSERID0001", "arn:aws:iam::000000000000:user/alice"),
            &[],
            &[],
        );
        let blob = issue(&engine, now - age, &decided);
        let (status, body) = decode(&router, &blob).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                element(&body, "Code").as_deref(),
                Some("InvalidAuthorizationMessageException"),
                "case {case:?}: expected an InvalidAuthorizationMessageException, got {body}"
            );
        }
    }

    // The record of an expired message is pruned: it stopped being decodable when the
    // clock passed its window, whether or not anybody called.
    let (router, engine) = emulator();
    let long_gone = SystemTime::now() - ENCODED_MESSAGE_VALIDITY - minute;
    let stale = issue(
        &engine,
        long_gone,
        &decision(
            false,
            false,
            "sts:AssumeRole",
            "arn:aws:iam::000000000000:role/demo",
            ("AIDAEXAMPLEUSERID0001", "arn:aws:iam::000000000000:user/alice"),
            &[],
            &[],
        ),
    );
    assert_eq!(held(&engine), 1, "case 'pruning': the record was recorded");
    let (status, body) = decode(&router, &stale).await;
    assert_eq!(status, 400, "case 'pruning': {body}");
    assert_eq!(
        held(&engine),
        0,
        "case 'pruning': a message past its window must be pruned"
    );
}

/// The registry is bounded: it never holds more than `MAX_AUTHORIZATION_MESSAGES`
/// records, and at the cap the earliest-issued message gives way while the newest
/// still decodes.
#[tokio::test]
async fn the_authorization_message_registry_is_bounded() {
    let (router, engine) = emulator();
    let now = SystemTime::now();
    let over_the_cap = MAX_AUTHORIZATION_MESSAGES + 5;
    let mut blobs: Vec<String> = Vec::with_capacity(over_the_cap);
    for index in 0..over_the_cap {
        // Each message is issued a second after the one before, so "earliest issued"
        // is unambiguous — and every one of them is well inside its validity window.
        let age = Duration::from_secs(u64::try_from(over_the_cap - index).expect("a small age"));
        let decided = decision(
            false,
            false,
            &format!("ec2:Action{index}"),
            "arn:aws:ec2:us-east-1:000000000000:instance/*",
            ("AIDAEXAMPLEUSERID0001", "arn:aws:iam::000000000000:user/alice"),
            &[],
            &[],
        );
        blobs.push(issue(&engine, now - age, &decided));
    }

    let recorded = held(&engine);
    assert!(
        recorded <= MAX_AUTHORIZATION_MESSAGES,
        "the registry holds {recorded} messages, past the {MAX_AUTHORIZATION_MESSAGES} cap"
    );

    let newest = blobs.last().expect("a newest message");
    let (status, body) = decode(&router, newest).await;
    assert_eq!(
        status, 200,
        "case 'the newest message': it must still decode, got {status} {body}"
    );

    let (status, body) = decode(&router, &blobs[0]).await;
    assert_eq!(
        (status, element(&body, "Code").as_deref()),
        (400, Some("InvalidAuthorizationMessageException")),
        "case 'the earliest message': it gave way at the cap, so it must be refused, got {status} {body}"
    );
}
