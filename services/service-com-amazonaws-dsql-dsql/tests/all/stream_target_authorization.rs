// SPDX-License-Identifier: CC0-1.0

//! WHERE the pass-role authorization decision on a `CreateStream` target sits
//! relative to the model-constraint validation of the rest of the request.
//!
//! RULE: being allowed to pass the role named in `targetDefinition` is a decision
//! about the caller and that ONE member — the operation's documented
//! `iam:PassRole` requirement — so it is reached before the request's other
//! members are checked against the model: a target naming a role in another
//! account is refused 403 `AccessDeniedException`, whichever other modelled
//! constraint the request breaks, and nothing is created. The one input the
//! decision itself depends on is the role: a `roleArn` outside `RoleArn`'s own
//! constraints names no role to authorize, so such a request stays a 400
//! `ValidationException`.
//!
//! Every case drives the real request path through `MultiServiceRouter::dispatch`
//! with a signed-looking restJson1 request, so the boundary constraint checks —
//! the layer this suite is about — run exactly as they do for a client. The
//! tables span the class: the broken member varies over its location (a URI
//! label, a body member, a member nested in the target's union) and over the
//! constraint class it breaks (`@pattern`, `@length`, an enum's value set), and
//! the role varies over several foreign accounts and several ways of being
//! malformed.

use serde_json::{json, Value};

use crate::harness::{Answer, Dsql};

/// An IAM role ARN in `account`, as `RoleArn`'s pattern requires
/// (`^arn:aws(-[^:]+)?:iam::[0-9]{12}:role(/[a-zA-Z0-9+=,.@_-]+)+$`). Source:
/// `smithy_query(query="shape", shape="KinesisTargetDefinition")`.
fn role_arn(account: &str, path_and_name: &str) -> String {
    format!("arn:aws:iam::{account}:role/{path_and_name}")
}

/// A Kinesis stream ARN, as `KinesisStreamArn`'s pattern requires
/// (`^arn:aws[a-zA-Z-]*:kinesis:[a-z0-9-]*:[0-9]{12}:stream/[a-zA-Z0-9+=,.@_/\-]+$`).
fn kinesis_arn(name: &str) -> String {
    format!(
        "arn:aws:kinesis:us-east-1:{}:stream/{name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// Accounts that are NOT this emulator's (which is `engine::EMULATOR_ACCOUNT_ID`,
/// all zeroes), each a well-formed 12-digit account id.
const FOREIGN_ACCOUNTS: &[&str] = &["111122223333", "210987654321", "999999999999"];

/// One way a `CreateStream` request breaks a modelled constraint on a member
/// OTHER than the role its target names.
struct Broken {
    name: &'static str,
    /// Whether this row breaks a constraint at all: the control row does not, so
    /// a role in this account is accepted rather than refused. A field rather
    /// than a reading of [`Self::name`], so rewording a case cannot change what
    /// it expects.
    breaks_a_constraint: bool,
    /// The `clusterIdentifier` the request is addressed to (an `@httpLabel`), or
    /// `None` for the cluster the test created.
    cluster: Option<&'static str>,
    /// The target's `streamArn`, or `None` for a well-formed one.
    stream_arn: Option<&'static str>,
    /// Body members written over the valid request, as (member, value).
    members: &'static [(&'static str, &'static str)],
}

/// The broken-member rows, plus the control row that breaks nothing. Each cites
/// the constraint it breaks; the constraints come from
/// `smithy_query(query="operation", operation="CreateStream")` and
/// `smithy_query(query="shape", shape="KinesisTargetDefinition")`.
const BROKEN: &[Broken] = &[
    Broken {
        // The row the issue recorded: a sibling of the role inside the target.
        name: "a Kinesis streamArn outside KinesisStreamArn's pattern",
        breaks_a_constraint: true,
        cluster: None,
        stream_arn: Some("not-a-kinesis-arn-at-all"),
        members: &[],
    },
    Broken {
        name: "an empty Kinesis streamArn, under its @length minimum of 1",
        breaks_a_constraint: true,
        cluster: None,
        stream_arn: Some(""),
        members: &[],
    },
    Broken {
        name: "a clusterIdentifier outside ClusterId's pattern (a URI label)",
        breaks_a_constraint: true,
        cluster: Some("NOT-A-CLUSTER-ID"),
        stream_arn: None,
        members: &[],
    },
    Broken {
        name: "an ordering outside StreamOrdering's value set",
        breaks_a_constraint: true,
        cluster: None,
        stream_arn: None,
        members: &[("ordering", "SEQUENTIAL")],
    },
    Broken {
        name: "a format outside StreamFormat's value set",
        breaks_a_constraint: true,
        cluster: None,
        stream_arn: None,
        members: &[("format", "CSV")],
    },
    Broken {
        name: "a clientToken outside ClientToken's pattern ^[!-~]+$",
        breaks_a_constraint: true,
        cluster: None,
        stream_arn: None,
        members: &[("clientToken", "has a space")],
    },
    Broken {
        name: "nothing broken at all",
        breaks_a_constraint: false,
        cluster: None,
        stream_arn: None,
        members: &[],
    },
];

/// The request `broken` describes, with its target naming a role in `account`.
fn request(broken: &Broken, account: &str) -> Value {
    let stream_arn = broken
        .stream_arn
        .map(str::to_string)
        .unwrap_or_else(|| kinesis_arn("changes"));
    let mut body = json!({
        "format": "JSON",
        "ordering": "UNORDERED",
        "targetDefinition": {"kinesis": {
            "streamArn": stream_arn,
            "roleArn": role_arn(account, "dsql-cdc"),
        }},
    });
    for (member, value) in broken.members {
        body[*member] = Value::String((*value).to_string());
    }
    body
}

/// `CreateStream` for `broken` against `cluster`, with a role in `account`.
async fn create_stream(dsql: &Dsql, cluster: &str, broken: &Broken, account: &str) -> Answer {
    dsql.create_stream(
        broken.cluster.unwrap_or(cluster),
        request(broken, account),
    )
    .await
}

/// RULE: a target naming a role in another account is refused 403
/// `AccessDeniedException` whichever OTHER modelled constraint the request
/// breaks, while the same request with a role in this account is refused 400
/// `ValidationException` for that constraint (and accepted when it breaks none).
/// The role is what decides which refusal the caller gets.
#[tokio::test]
async fn a_foreign_role_is_denied_ahead_of_every_other_constraint() {
    assert!(
        !FOREIGN_ACCOUNTS.contains(&engine::EMULATOR_ACCOUNT_ID),
        "case the table itself: every account here must be one this emulator does NOT own \
         ({}), got {FOREIGN_ACCOUNTS:?}",
        engine::EMULATOR_ACCOUNT_ID
    );
    let dsql = Dsql::new();
    let cluster = dsql.create_settled().await;

    for broken in BROKEN {
        let name = broken.name;

        // A role in THIS account: the request's own constraints decide.
        let own = create_stream(&dsql, &cluster, broken, engine::EMULATOR_ACCOUNT_ID).await;
        let (expected_status, expected_code) = if broken.breaks_a_constraint {
            (400, Some("ValidationException"))
        } else {
            (200, None)
        };
        assert_eq!(
            own.status, expected_status,
            "case {name:?} with a role in this account: expected {expected_status}, got {} {:?}",
            own.status, own.body
        );
        assert_eq!(
            own.error.as_deref(),
            expected_code,
            "case {name:?} with a role in this account: expected error {expected_code:?}, got {:?}",
            own.error
        );

        // The same request, with the role in some other account: the pass-role
        // denial is reached first, so the classification is the authorization one.
        for account in FOREIGN_ACCOUNTS {
            let answer = create_stream(&dsql, &cluster, broken, account).await;
            assert_eq!(
                answer.status, 403,
                "case {name:?} with a role in account {account}: a role this account \
                 cannot pass must be refused 403, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("AccessDeniedException"),
                "case {name:?} with a role in account {account}: the refusal must be an \
                 AccessDeniedException, got {:?}",
                answer.error
            );
            // `AccessDeniedException` models `message` as `@required`.
            assert!(
                !answer.message().is_empty(),
                "case {name:?} with a role in account {account}: the refusal must carry a \
                 message, got {:?}",
                answer.body
            );
            assert!(
                answer.get("streamIdentifier").is_null(),
                "case {name:?} with a role in account {account}: a refused CreateStream must \
                 mint no stream, got {:?}",
                answer.body
            );
        }
    }
}

/// RULE: the pass-role decision depends on the role itself, so a `roleArn`
/// outside `RoleArn`'s own constraints names no role to authorize — such a
/// request is the 400 `ValidationException` the model's constraint makes it, even
/// when the account digits inside it are not this account's and even when a
/// sibling member is malformed too.
#[tokio::test]
async fn a_role_arn_outside_its_own_constraints_stays_a_validation_refusal() {
    let dsql = Dsql::new();
    let cluster = dsql.create_settled().await;

    // Values outside `RoleArn`'s `@length(20, 2048)` /
    // `^arn:aws(-[^:]+)?:iam::[0-9]{12}:role(/[a-zA-Z0-9+=,.@_-]+)+$`.
    let malformed_roles = [
        ("not an ARN at all", "not-an-iam-arn-at-all-but-long-enough"),
        ("an account that is not 12 digits", "arn:aws:iam::12345:role/dsql-cdc"),
        ("no role name after the type", "arn:aws:iam::111122223333:role"),
        (
            "another service's resource type",
            "arn:aws:iam::111122223333:user/not-a-role",
        ),
        ("shorter than the @length minimum of 20", "arn:aws:iam::1:r"),
        (
            "a trailing empty name segment",
            "arn:aws:iam::111122223333:role/",
        ),
    ];

    for (name, role) in malformed_roles {
        let answer = dsql
            .create_stream(
                &cluster,
                json!({
                    "format": "JSON",
                    "ordering": "UNORDERED",
                    "targetDefinition": {"kinesis": {
                        // A sibling that is malformed too: the role decides,
                        // and this role cannot be authorized at all.
                        "streamArn": "not-a-kinesis-arn-at-all",
                        "roleArn": role,
                    }},
                }),
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {name:?}: a roleArn outside its own constraints must be refused 400, \
             got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("ValidationException"),
            "case {name:?}: the refusal must be a ValidationException, got {:?}",
            answer.error
        );
    }
}

/// RULE: a denied pass-role has NO effect — no stream, and nothing recorded
/// against the `clientToken` the denied call carried, so that token is still free
/// to serve a request this account may make.
#[tokio::test]
async fn a_denied_pass_role_leaves_the_request_unserved() {
    let dsql = Dsql::new();
    let cluster = dsql.create_settled().await;
    let token = "token-of-a-denied-request";

    for account in FOREIGN_ACCOUNTS {
        let denied = dsql
            .create_stream(
                &cluster,
                json!({
                    "format": "JSON",
                    "ordering": "UNORDERED",
                    "targetDefinition": {"kinesis": {
                        "streamArn": kinesis_arn("changes"),
                        "roleArn": role_arn(account, "dsql-cdc"),
                    }},
                    "clientToken": token,
                }),
            )
            .await;
        assert_eq!(
            denied.status, 403,
            "case account={account}: the denied call must answer 403, got {} {:?}",
            denied.status, denied.body
        );
    }

    // The token the denied calls carried names no request, so this one is served
    // rather than replayed or refused as a conflict.
    let served = dsql
        .create_stream(
            &cluster,
            json!({
                "format": "JSON",
                "ordering": "UNORDERED",
                "targetDefinition": {"kinesis": {
                    "streamArn": kinesis_arn("changes"),
                    "roleArn": role_arn(engine::EMULATOR_ACCOUNT_ID, "dsql-cdc"),
                }},
                "clientToken": token,
            }),
        )
        .await;
    assert_eq!(
        served.status, 200,
        "a request this account may make must be served under the token the denied \
         calls carried, got {} {:?}",
        served.status, served.body
    );

    // Round trip: the stream the served call minted is the only one there is to
    // read, and it reads back through GetStream.
    let stream = served.str("streamIdentifier");
    let fetched = dsql.get_stream(&cluster, &stream).await;
    assert_eq!(
        fetched.status, 200,
        "GetStream must report the stream the served call minted, got {} {:?}",
        fetched.status, fetched.body
    );
    assert_eq!(
        fetched.get("targetDefinition"),
        json!({"kinesis": {
            "streamArn": kinesis_arn("changes"),
            "roleArn": role_arn(engine::EMULATOR_ACCOUNT_ID, "dsql-cdc"),
        }}),
        "GetStream must report the target the served call persisted, got {:?}",
        fetched.get("targetDefinition")
    );
}
