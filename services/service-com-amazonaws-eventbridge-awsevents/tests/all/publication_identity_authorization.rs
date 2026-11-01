// SPDX-License-Identifier: CC0-1.0

//! A client's `PutEvents` is authorized against the CALLING PRINCIPAL's own
//! identity-based policies as well as the bus's resource policy, end to end over the wire
//! through the real `MultiServiceRouter` with IAM and SQS mounted beside EventBridge.
//!
//! The RULE asserted here: publishing an event is the action `events:PutEvents` on the
//! event bus the entry names, so it is allowed only when something grants it — an `Allow`
//! about it in the identity-based policies in force on the principal the request's access
//! key speaks for, or one in that bus's own resource policy naming that principal — and an
//! explicit `Deny` in either document refuses it whatever grants it. A principal with
//! neither grant is refused with `AccessDeniedException`, the whole request is refused
//! (nothing of it is published, not even an entry no statement names), and the account's
//! own published credential — which holds no identity-based policies and owns its buses —
//! keeps publishing.
//!
//! The rows span the class rather than the one recorded case: they differ along WHO
//! publishes (the account's own credential, a key the account holds no record of, an IAM
//! user), along WHICH document grants or refuses (nothing at all, the caller's
//! identity-based policy, the bus's resource policy, both), along the shape of that
//! statement (this action or a wildcard over the service, this bus or another, a
//! `Principal` naming the user or only its account, a statement scoped by an event-content
//! condition key), along the EFFECT (an allow, an explicit deny in each of the two
//! documents) and along the BATCH the request carries (one entry, and two of which only one
//! is granted).
//!
//! A twin control runs on every row: a rule of the same bus whose target is a queue, so
//! what the request published is read where a client reads it — an accepted request puts
//! one copy per entry on that queue, a refused one puts none.
//!
//! The harness (the router with EventBridge, SQS, IAM and CloudWatch on one engine) is the
//! one `target_dispatch` owns, and the access key each row presents is what selects its
//! principal: `iam:CreateAccessKey` mints a key that speaks for the user it was minted for.

use serde_json::{json, Value};

use crate::target_dispatch::{
    drain, emulator, events, events_as, given_queue, iam, xml_text, Emulator, AMBIENT_KEY, REGION,
};

/// The bus every row publishes onto, and a second bus named only as another statement's
/// `Resource` — the publication never goes near it.
const BUS: &str = "authorized-bus";
const OTHER_BUS: &str = "another-bus";

/// The rule that carries the control queue, and its target.
const RULE: &str = "receives";
const TARGET_ID: &str = "t1";

/// The sources the rows publish under: two, so a statement can be scoped to one of them
/// and a batch can mix a named one with an unnamed one.
const NAMED_SOURCE: &str = "com.demo.tenant-a";
const OTHER_SOURCE: &str = "com.demo.tenant-b";

/// The action a publication onto a bus performs, as a statement names it and as a refusal
/// reports it.
const PUT_EVENTS_ACTION: &str = "events:PutEvents";

/// The `Sid` of the statement a row installs on the bus.
const STATEMENT_ID: &str = "PublisherStatement";

/// The name of the inline policy a row attaches to its user.
const USER_POLICY_NAME: &str = "publish";

/// The ARN of the bus `name` in the Region every call here is signed for.
fn bus_arn(name: &str) -> String {
    format!(
        "arn:aws:events:{REGION}:{}:event-bus/{name}",
        engine::emulator_account_id()
    )
}

/// The ARN of the IAM user `name` of this account, the spelling a statement names it by
/// and the one a refusal blames.
fn user_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:user/{name}", engine::emulator_account_id())
}

/// The sentence AWS refuses a call with when no policy in force allows it.
fn implicit_deny_message(principal: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {PUT_EVENTS_ACTION} on resource: {} \
         because no identity-based policy allows the {PUT_EVENTS_ACTION} action",
        bus_arn(BUS)
    )
}

/// The sentence AWS refuses a call with when a document in force explicitly denies it.
fn explicit_deny_message(principal: &str, document: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {PUT_EVENTS_ACTION} on resource: {} \
         with an explicit deny in {document}",
        bus_arn(BUS)
    )
}

/// Who a row publishes as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Publisher {
    /// The account's own published credential: a key the account holds no record of, which
    /// is what every administrative call here presents.
    AccountCredential,
    /// An IAM user of the account, publishing with a key `iam:CreateAccessKey` minted for
    /// it. The identity-based policy it holds, if any, is the row's.
    User,
}

/// One row of the table.
struct Row {
    case: &'static str,
    publisher: Publisher,
    /// The inline policy document the row's user holds, or `None` for a user holding none.
    user_policy: Option<Value>,
    /// The statement the row installs on the bus as its whole resource policy, or `None`
    /// for a bus nobody has installed one on.
    bus_statement: Option<Value>,
    /// The sources the request publishes, one entry each, in order.
    sources: &'static [&'static str],
    /// The refusal the publication earns, or `None` when it is accepted. The principal is
    /// filled in per row, because a user's ARN embeds the name this row created.
    refusal: Option<Refusal>,
}

/// Which document refused the publication, which is what the refusal's closing clause
/// names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// Nothing in force allowed it.
    NoGrant,
    /// An explicit deny in the caller's identity-based policy.
    IdentityDeny,
    /// An explicit deny in the bus's resource policy.
    ResourceDeny,
}

impl Refusal {
    /// The message this refusal carries for `principal`.
    fn message(self, principal: &str) -> String {
        match self {
            Self::NoGrant => implicit_deny_message(principal),
            Self::IdentityDeny => {
                explicit_deny_message(principal, "an identity-based policy")
            }
            Self::ResourceDeny => {
                explicit_deny_message(principal, "a resource-based policy")
            }
        }
    }
}

/// An identity-based statement: `effect` on `action` over `resource`, scoped by
/// `condition`.
fn identity_statement(
    effect: &str,
    action: &str,
    resource: &str,
    condition: Option<Value>,
) -> Value {
    let mut statement = json!({
        "Effect": effect,
        "Action": action,
        "Resource": resource,
    });
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    json!({ "Version": "2012-10-17", "Statement": [statement] })
}

/// An allow of the publication onto [`BUS`] in an identity-based policy.
fn identity_allow() -> Value {
    identity_statement("Allow", PUT_EVENTS_ACTION, &bus_arn(BUS), None)
}

/// A resource-policy statement, as the rows write one.
fn bus_statement(effect: &str, principal: Value, action: &str, resource: &str) -> Value {
    json!({
        "Sid": STATEMENT_ID,
        "Effect": effect,
        "Principal": principal,
        "Action": action,
        "Resource": bus_arn(resource),
    })
}

/// A clause over the published event's `Source`, the condition key AWS publishes for this
/// action (`events:source`).
fn on_source(source: &str) -> Value {
    json!({ "StringEquals": { "events:source": source } })
}

/// The rows. The user name is derived from the row's index, so each row's principal is its
/// own and a grant never leaks from one row to the next.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the account's own credential onto a bus carrying no policy",
            publisher: Publisher::AccountCredential,
            user_policy: None,
            bus_statement: None,
            sources: &[NAMED_SOURCE],
            refusal: None,
        },
        Row {
            case: "an IAM user holding no policy at all, onto a bus carrying none",
            publisher: Publisher::User,
            user_policy: None,
            bus_statement: None,
            sources: &[NAMED_SOURCE],
            refusal: Some(Refusal::NoGrant),
        },
        Row {
            case: "an IAM user whose identity policy allows the publication onto this bus",
            publisher: Publisher::User,
            user_policy: Some(identity_allow()),
            bus_statement: None,
            sources: &[NAMED_SOURCE],
            refusal: None,
        },
        Row {
            case: "an IAM user whose identity policy allows it onto another bus",
            publisher: Publisher::User,
            user_policy: Some(identity_statement(
                "Allow",
                PUT_EVENTS_ACTION,
                &bus_arn(OTHER_BUS),
                None,
            )),
            bus_statement: None,
            sources: &[NAMED_SOURCE],
            refusal: Some(Refusal::NoGrant),
        },
        Row {
            case: "an IAM user whose identity policy allows every action of the service",
            publisher: Publisher::User,
            user_policy: Some(identity_statement("Allow", "events:*", "*", None)),
            bus_statement: None,
            sources: &[NAMED_SOURCE],
            refusal: None,
        },
        Row {
            case: "an IAM user whose identity policy allows another action of the service",
            publisher: Publisher::User,
            user_policy: Some(identity_statement("Allow", "events:PutRule", "*", None)),
            bus_statement: None,
            sources: &[NAMED_SOURCE],
            refusal: Some(Refusal::NoGrant),
        },
        Row {
            case: "an IAM user whose identity policy denies the publication explicitly",
            publisher: Publisher::User,
            user_policy: Some(identity_statement(
                "Deny",
                PUT_EVENTS_ACTION,
                &bus_arn(BUS),
                None,
            )),
            bus_statement: Some(bus_statement(
                "Allow",
                json!("*"),
                PUT_EVENTS_ACTION,
                BUS,
            )),
            sources: &[NAMED_SOURCE],
            refusal: Some(Refusal::IdentityDeny),
        },
        Row {
            case: "an IAM user granted by the bus's resource policy alone",
            publisher: Publisher::User,
            user_policy: None,
            bus_statement: Some(bus_statement(
                "Allow",
                json!("*"),
                PUT_EVENTS_ACTION,
                BUS,
            )),
            sources: &[NAMED_SOURCE],
            refusal: None,
        },
        Row {
            case: "an IAM user whose grant in the bus policy names another principal",
            publisher: Publisher::User,
            user_policy: None,
            bus_statement: Some(bus_statement(
                "Allow",
                json!({ "AWS": format!("arn:aws:iam::{}:user/someone-else", engine::emulator_account_id()) }),
                PUT_EVENTS_ACTION,
                BUS,
            )),
            sources: &[NAMED_SOURCE],
            refusal: Some(Refusal::NoGrant),
        },
        Row {
            // "You must also attach an identity-based policy": a statement naming the
            // ACCOUNT delegates to it rather than granting one of its principals.
            case: "an IAM user whose grant in the bus policy names only its account",
            publisher: Publisher::User,
            user_policy: None,
            bus_statement: Some(bus_statement(
                "Allow",
                json!({ "AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id()) }),
                PUT_EVENTS_ACTION,
                BUS,
            )),
            sources: &[NAMED_SOURCE],
            refusal: Some(Refusal::NoGrant),
        },
        Row {
            case: "an IAM user with an identity grant, onto a bus whose policy denies \
                   every principal",
            publisher: Publisher::User,
            user_policy: Some(identity_allow()),
            bus_statement: Some(bus_statement("Deny", json!("*"), PUT_EVENTS_ACTION, BUS)),
            sources: &[NAMED_SOURCE],
            refusal: Some(Refusal::ResourceDeny),
        },
        Row {
            case: "an IAM user whose identity grant is scoped to the source it publishes",
            publisher: Publisher::User,
            user_policy: Some(identity_statement(
                "Allow",
                PUT_EVENTS_ACTION,
                &bus_arn(BUS),
                Some(on_source(NAMED_SOURCE)),
            )),
            sources: &[NAMED_SOURCE],
            bus_statement: None,
            refusal: None,
        },
        Row {
            case: "an IAM user whose identity grant is scoped to another source",
            publisher: Publisher::User,
            user_policy: Some(identity_statement(
                "Allow",
                PUT_EVENTS_ACTION,
                &bus_arn(BUS),
                Some(on_source(OTHER_SOURCE)),
            )),
            bus_statement: None,
            sources: &[NAMED_SOURCE],
            refusal: Some(Refusal::NoGrant),
        },
        Row {
            // The verdict is about the REQUEST: one entry the grant does not cover
            // refuses the batch, and the covered entry is not published either.
            case: "a batch whose second entry the scoped identity grant does not cover",
            publisher: Publisher::User,
            user_policy: Some(identity_statement(
                "Allow",
                PUT_EVENTS_ACTION,
                &bus_arn(BUS),
                Some(on_source(NAMED_SOURCE)),
            )),
            bus_statement: None,
            sources: &[NAMED_SOURCE, OTHER_SOURCE],
            refusal: Some(Refusal::NoGrant),
        },
        Row {
            case: "a batch every entry of which the scoped identity grant covers",
            publisher: Publisher::User,
            user_policy: Some(identity_statement(
                "Allow",
                PUT_EVENTS_ACTION,
                &bus_arn(BUS),
                Some(json!({ "StringLike": { "events:source": "com.demo.tenant-*" } })),
            )),
            bus_statement: None,
            sources: &[NAMED_SOURCE, OTHER_SOURCE],
            refusal: None,
        },
    ]
}

/// Create the bus `name`.
async fn given_bus(emulator: &Emulator, case: &str, name: &str) {
    let (status, body) = events(emulator, "CreateEventBus", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateEventBus {name} answered {status} {body}"
    );
}

/// A rule on [`BUS`] matching every source published below, delivering to the queue
/// `target_arn` names.
async fn given_rule(emulator: &Emulator, case: &str, target_arn: &str) {
    let pattern = json!({ "source": [NAMED_SOURCE, OTHER_SOURCE] }).to_string();
    let (status, body) = events(
        emulator,
        "PutRule",
        &json!({
            "Name": RULE,
            "EventBusName": BUS,
            "EventPattern": pattern,
            "State": "ENABLED",
        }),
    )
    .await;
    assert_eq!(status, 200, "case {case:?}: PutRule answered {status} {body}");
    let (status, body) = events(
        emulator,
        "PutTargets",
        &json!({
            "Rule": RULE,
            "EventBusName": BUS,
            "Targets": [{ "Id": TARGET_ID, "Arn": target_arn }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutTargets answered {status} {body}"
    );
}

/// Install `statement` on [`BUS`] as its whole resource policy.
async fn given_bus_policy(emulator: &Emulator, case: &str, statement: &Value) {
    let document = json!({ "Version": "2012-10-17", "Statement": [statement] }).to_string();
    let (status, body) = events(
        emulator,
        "PutPermission",
        &json!({ "EventBusName": BUS, "Policy": document }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutPermission answered {status} {body}"
    );
}

/// Create the IAM user `name`, attach `policy` to it when the row holds one, and answer
/// the access key id a request publishes as that user with.
async fn given_user(
    emulator: &Emulator,
    case: &str,
    name: &str,
    policy: Option<&Value>,
) -> String {
    let (status, body) = iam(emulator, "CreateUser", &[("UserName", name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateUser {name} answered {status} {body}"
    );
    if let Some(policy) = policy {
        let (status, body) = iam(
            emulator,
            "PutUserPolicy",
            &[
                ("UserName", name),
                ("PolicyName", USER_POLICY_NAME),
                ("PolicyDocument", &policy.to_string()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutUserPolicy for {name} answered {status} {body}"
        );
    }
    let (status, body) = iam(emulator, "CreateAccessKey", &[("UserName", name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateAccessKey for {name} answered status {status}"
    );
    xml_text(&body, "AccessKeyId").unwrap_or_else(|| {
        panic!("case {case:?}: the minted-credential answer carries no AccessKeyId element")
    })
}

/// Publish one entry per source onto [`BUS`], presenting `access_key_id`.
async fn put_events(
    emulator: &Emulator,
    access_key_id: &str,
    sources: &[&str],
) -> (u16, Value) {
    let entries: Vec<Value> = sources
        .iter()
        .map(|source| {
            json!({
                "EventBusName": BUS,
                "Source": source,
                "DetailType": "Order Created",
                "Detail": r#"{"orderId":"A-1"}"#,
            })
        })
        .collect();
    events_as(
        emulator,
        access_key_id,
        "PutEvents",
        &json!({ "Entries": entries }),
    )
    .await
}

/// The wire error code of a failure body (`__type` on awsJson), without its namespace.
fn error_code(body: &Value) -> &str {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
}

/// The `message` a failure body carries.
fn message(body: &Value) -> &str {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

#[tokio::test]
async fn a_publication_needs_a_grant_from_the_caller_or_from_the_bus() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let emulator = emulator();
        given_bus(&emulator, case, BUS).await;
        let (queue_url, queue_arn) = given_queue(&emulator, &format!("published-{index}")).await;
        given_rule(&emulator, case, &queue_arn).await;
        if let Some(statement) = &row.bus_statement {
            given_bus_policy(&emulator, case, statement).await;
        }

        let user_name = format!("publisher-{index}");
        let (access_key_id, principal) = match row.publisher {
            Publisher::AccountCredential => (
                AMBIENT_KEY.to_string(),
                ::service_common::caller_principal::ambient_user_arn(),
            ),
            Publisher::User => (
                given_user(&emulator, case, &user_name, row.user_policy.as_ref()).await,
                user_arn(&user_name),
            ),
        };

        let (status, body) = put_events(&emulator, &access_key_id, row.sources).await;

        match row.refusal {
            Some(refusal) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the publication to be refused, got {status} {body}"
                );
                let code = error_code(&body);
                assert_eq!(
                    code, "AccessDeniedException",
                    "case {case:?}: expected the code AccessDeniedException, got {code:?} \
                     in {body}"
                );
                let expected = refusal.message(&principal);
                let reported = message(&body);
                assert_eq!(
                    reported, expected,
                    "case {case:?}: expected the refusal {expected:?}, got {reported:?}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the publication to be accepted, got {status} {body}"
                );
                assert_eq!(
                    body["FailedEntryCount"],
                    json!(0),
                    "case {case:?}: expected no failed entry, got {body}"
                );
                let results = body["Entries"].as_array().cloned().unwrap_or_default();
                assert_eq!(
                    results.len(),
                    row.sources.len(),
                    "case {case:?}: expected one result per entry, got {body}"
                );
                for (at, entry) in results.iter().enumerate() {
                    assert!(
                        entry["EventId"].as_str().is_some_and(|id| !id.is_empty()),
                        "case {case:?}: expected an EventId on entry {at}, got {entry}"
                    );
                }
            }
        }

        // What the request actually published, read where a client reads it: an accepted
        // request puts one copy per entry on the rule's queue, a refused one puts none.
        let delivered = drain(&emulator, &queue_url).await;
        let expected = if row.refusal.is_some() {
            0
        } else {
            row.sources.len()
        };
        assert_eq!(
            delivered.len(),
            expected,
            "case {case:?}: expected {expected} published event(s) on the rule's queue, \
             got {delivered:?}"
        );
    }
}

/// The grant is read as it stands NOW: the user refused while it held nothing publishes as
/// soon as its identity policy grants the action, and is refused again once that policy is
/// deleted — so the absence in between is the policy's doing rather than the delivery
/// path's.
#[tokio::test]
async fn a_grant_takes_effect_and_stops_when_it_is_withdrawn() {
    let case = "the grant written, then deleted";
    let emulator = emulator();
    given_bus(&emulator, case, BUS).await;
    let (queue_url, queue_arn) = given_queue(&emulator, "published").await;
    given_rule(&emulator, case, &queue_arn).await;

    let user_name = "late-publisher";
    let access_key_id = given_user(&emulator, case, user_name, None).await;
    let principal = user_arn(user_name);

    let (status, body) = put_events(&emulator, &access_key_id, &[NAMED_SOURCE]).await;
    assert_eq!(
        status, 400,
        "case {case:?}: a user holding nothing must be refused, got {status} {body}"
    );
    assert_eq!(
        message(&body),
        implicit_deny_message(&principal),
        "case {case:?}: expected the implicit-deny sentence, got {body}"
    );
    let suppressed = drain(&emulator, &queue_url).await;
    assert!(
        suppressed.is_empty(),
        "case {case:?}: a refused request publishes nothing, got {suppressed:?}"
    );

    // The account's own credential publishes onto the same bus and rule throughout, so the
    // absence above is the refusal and not a broken delivery path.
    let (status, body) = put_events(&emulator, AMBIENT_KEY, &[NAMED_SOURCE]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the control publication answered {status} {body}"
    );
    let arrived = drain(&emulator, &queue_url).await;
    assert_eq!(
        arrived.len(),
        1,
        "case {case:?}: expected the control event on the queue, got {arrived:?}"
    );

    let document = identity_allow().to_string();
    let (status, body) = iam(
        &emulator,
        "PutUserPolicy",
        &[
            ("UserName", user_name),
            ("PolicyName", USER_POLICY_NAME),
            ("PolicyDocument", &document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutUserPolicy answered {status} {body}"
    );

    let (status, body) = put_events(&emulator, &access_key_id, &[NAMED_SOURCE]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the granted user must publish, got {status} {body}"
    );
    let granted = drain(&emulator, &queue_url).await;
    assert_eq!(
        granted.len(),
        1,
        "case {case:?}: expected the granted publication on the queue, got {granted:?}"
    );

    let (status, body) = iam(
        &emulator,
        "DeleteUserPolicy",
        &[("UserName", user_name), ("PolicyName", USER_POLICY_NAME)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteUserPolicy answered {status} {body}"
    );

    let (status, body) = put_events(&emulator, &access_key_id, &[NAMED_SOURCE]).await;
    assert_eq!(
        status, 400,
        "case {case:?}: the withdrawn grant must stop the publication, got {status} {body}"
    );
    assert_eq!(
        message(&body),
        implicit_deny_message(&principal),
        "case {case:?}: expected the implicit-deny sentence again, got {body}"
    );
    let withdrawn = drain(&emulator, &queue_url).await;
    assert!(
        withdrawn.is_empty(),
        "case {case:?}: nothing is published once the grant is gone, got {withdrawn:?}"
    );
}
