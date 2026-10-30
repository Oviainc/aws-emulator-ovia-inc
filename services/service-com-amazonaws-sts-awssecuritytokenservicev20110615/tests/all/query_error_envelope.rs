// SPDX-License-Identifier: CC0-1.0

//! The awsQuery error envelope this service answers a REFUSAL in, over the class of
//! requests that provoke one.
//!
//! RULE: an `awsQuery` refusal is `<ErrorResponse><Error>…</Error><RequestId>…</RequestId>
//! </ErrorResponse>` — the `<RequestId>` is a child of the ENVELOPE, beside `<Error>`, it
//! is there on every refusal, and it carries a per-response id (the 8-4-4-4-12 form every
//! AWS request id has), so two refusals are distinguishable and a client can correlate
//! either of them. It is a property of the RESPONSE, so it does not depend on which
//! operation was named, which layer refused (the request's values, the caller's
//! authorization, an `Action` the model does not declare), or which status the refusal
//! carries. A SUCCESS is unaffected: it names the request in `ResponseMetadata` already
//! and carries no `<ErrorResponse>` at all.
//!
//! The rows span what the rule generalises over: the operation, the HTTP status (400 and
//! 403), which layer answered (the boundary walk's constraint refusal, a handler's own
//! modelled error, the caller-authorization refusal, the routing failure for an unmodelled
//! `Action`), and whether the request was refused at all.
//!
//! Every call goes over the wire as the `application/x-www-form-urlencoded` awsQuery POST
//! a client makes, through the same router the emulator serves with.

use std::collections::BTreeSet;

use crate::sts_sessions::{call, element, emulator, seed_demo_role, LONG_TERM_KEY};

/// A role this account holds, seeded by the harness: the rows that must NOT be refused for
/// their role name this one.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/demo";

/// Whether `id` has the form every AWS request id carries: five lowercase-hex groups of
/// 8-4-4-4-12 characters.
fn is_request_id(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8usize, 4, 4, 4, 12])
            .all(|(group, width)| {
                group.len() == width
                    && group
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            })
}

/// The `<RequestId>` of the ENVELOPE: the one between `</Error>` and `</ErrorResponse>`,
/// which is the only position a child of the envelope can sit in — text inside `<Error>`
/// is the modelled error's, not the response's.
fn envelope_request_id(case: &str, body: &str) -> String {
    assert!(
        body.contains("<ErrorResponse"),
        "case {case:?}: expected an <ErrorResponse> envelope, got {body}"
    );
    let close = body
        .rfind("</ErrorResponse>")
        .unwrap_or_else(|| panic!("case {case:?}: expected a closed envelope, got {body}"));
    let siblings = match body[..close].rfind("</Error>") {
        Some(error_end) => &body[error_end..close],
        None => &body[..close],
    };
    let open = "<RequestId>";
    let start = siblings
        .find(open)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a <RequestId> between </Error> and </ErrorResponse>, \
                 got {body}"
            )
        })
        + open.len();
    let end = siblings[start..]
        .find("</RequestId>")
        .unwrap_or_else(|| panic!("case {case:?}: expected a closed <RequestId>, got {body}"));
    siblings[start..start + end].to_string()
}

/// One row: the case, the action, the form fields, and the status the refusal carries —
/// `None` for the request that is served rather than refused.
struct Row {
    case: &'static str,
    action: &'static str,
    fields: Vec<(&'static str, String)>,
    refused: Option<u16>,
}

/// The rows: every operation this service refuses a request on, each refused by a
/// different layer, plus an unmodelled `Action` and a request that succeeds.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "AssumeRole, a session name outside its modelled length bounds",
            action: "AssumeRole",
            fields: vec![
                ("RoleArn", ROLE_ARN.to_string()),
                ("RoleSessionName", "a".to_string()),
            ],
            refused: Some(400),
        },
        Row {
            case: "AssumeRole, a session policy no parser can read",
            action: "AssumeRole",
            fields: vec![
                ("RoleArn", ROLE_ARN.to_string()),
                ("RoleSessionName", "probe".to_string()),
                ("Policy", "{not valid json".to_string()),
            ],
            refused: Some(400),
        },
        Row {
            case: "AssumeRole, a role this account does not hold",
            action: "AssumeRole",
            fields: vec![
                (
                    "RoleArn",
                    "arn:aws:iam::000000000000:role/nothing-here".to_string(),
                ),
                ("RoleSessionName", "probe".to_string()),
            ],
            refused: Some(403),
        },
        Row {
            case: "AssumeRoleWithWebIdentity, a token that is not a web identity token",
            action: "AssumeRoleWithWebIdentity",
            fields: vec![
                ("RoleArn", ROLE_ARN.to_string()),
                ("RoleSessionName", "probe".to_string()),
                ("WebIdentityToken", "not-a-web-identity-token".to_string()),
            ],
            refused: Some(400),
        },
        Row {
            case: "GetDelegatedAccessToken, a trade-in token the account holds no record of",
            action: "GetDelegatedAccessToken",
            fields: vec![(
                "TradeInToken",
                "AQoDYXdzEUnknownTradeInTokenValue0123456789".to_string(),
            )],
            refused: Some(400),
        },
        Row {
            case: "GetFederationToken, a name outside its modelled length bounds",
            action: "GetFederationToken",
            fields: vec![("Name", "a".to_string())],
            refused: Some(400),
        },
        Row {
            case: "GetFederationToken, a session policy no parser can read",
            action: "GetFederationToken",
            fields: vec![
                ("Name", "bob".to_string()),
                ("Policy", r#"{"Statement":[{"#.to_string()),
            ],
            refused: Some(400),
        },
        Row {
            case: "DecodeAuthorizationMessage, a message this account did not issue",
            action: "DecodeAuthorizationMessage",
            fields: vec![("EncodedMessage", "bm90LWFuLWF1dGhvcml6YXRpb24tbWVzc2FnZQ".to_string())],
            refused: Some(400),
        },
        Row {
            case: "an Action this service's model does not declare",
            action: "NoSuchOperation",
            fields: vec![],
            refused: Some(400),
        },
        Row {
            case: "a request that is served",
            action: "GetCallerIdentity",
            fields: vec![],
            refused: None,
        },
    ]
}

/// RULE, over the table: every refusal carries the envelope's own `<RequestId>`, it has
/// the form an AWS request id has, no two responses report the same one, and a success
/// carries no error envelope at all.
#[tokio::test]
async fn every_refusal_names_the_request_it_answers_in_the_envelope() {
    let (router, engine) = emulator();
    // `AssumeRole` resolves its role in IAM, so the role the rows name is one this account
    // holds: a row must be refused for the reason it describes and nothing else.
    let (_role_arn, _role_id) = seed_demo_role(&engine);

    let mut reported: BTreeSet<String> = BTreeSet::new();
    for row in rows() {
        let case = row.case;
        let (status, body) = call(&router, LONG_TERM_KEY, row.action, &row.fields).await;

        let Some(expected_status) = row.refused else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the request to be served, got {status} {body}"
            );
            assert!(
                !body.contains("<ErrorResponse"),
                "case {case:?}: a served request carries no error envelope, got {body}"
            );
            let served = element(&body, "RequestId")
                .unwrap_or_else(|| panic!("case {case:?}: no <RequestId> in {body}"));
            assert!(
                is_request_id(&served),
                "case {case:?}: expected a request id, got {served:?}"
            );
            assert!(
                reported.insert(served.clone()),
                "case {case:?}: {served:?} was reported by an earlier response too"
            );
            continue;
        };

        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        let request_id = envelope_request_id(case, &body);
        assert!(
            is_request_id(&request_id),
            "case {case:?}: expected the envelope to carry a request id, got {request_id:?} in \
             {body}"
        );
        assert!(
            reported.insert(request_id.clone()),
            "case {case:?}: {request_id:?} was reported by an earlier response too"
        );
    }
}

/// The id is per RESPONSE, not per request shape: the same refusal asked for twice reports
/// two different ids, and each is still a well-formed one.
#[tokio::test]
async fn two_identical_refusals_report_two_ids() {
    let (router, engine) = emulator();
    let (_role_arn, _role_id) = seed_demo_role(&engine);
    let case = "the same refusal twice";
    let fields = vec![
        ("RoleArn", ROLE_ARN.to_string()),
        ("RoleSessionName", "a".to_string()),
    ];

    let mut seen: Vec<String> = Vec::new();
    for _ in 0..3 {
        let (status, body) = call(&router, LONG_TERM_KEY, "AssumeRole", &fields).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        let request_id = envelope_request_id(case, &body);
        assert!(
            is_request_id(&request_id),
            "case {case:?}: expected a request id, got {request_id:?}"
        );
        assert!(
            !seen.contains(&request_id),
            "case {case:?}: {request_id:?} repeats, already reported in {seen:?}"
        );
        seen.push(request_id);
    }
}
