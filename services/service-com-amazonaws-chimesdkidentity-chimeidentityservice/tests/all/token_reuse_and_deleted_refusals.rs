// SPDX-License-Identifier: CC0-1.0

//! What a create does with a `ClientRequestToken` it has seen before, and what every
//! operation answers once the `AppInstance` it addresses has been deleted — end to end
//! over HTTP.
//!
//! Three RULEs are pinned here:
//!
//! * a create's `@idempotencyToken` replays only the request the token already made: the
//!   same token with ANY different parameter is refused `ConflictException` (409) and
//!   mints nothing;
//! * a create whose resource already exists is refused `ConflictException` (409) with
//!   AWS's fixed already-exists sentence, whatever the resource's id looks like;
//! * a request addressed to a deleted `AppInstance` — its own ARN, or the ARN of a
//!   resource the delete cascaded away — is refused `ForbiddenException` (403), while an
//!   ARN that never named a resource keeps the service's unknown-ARN refusal.
//!
//! The tables span what those rules generalise over: which member a reused token differs
//! in, the form of the id a duplicate create names, and which operation (and which kind
//! of ARN under the instance) the request is addressed to. A fourth test pins the bound on
//! the deletions the emulator remembers, and the order in which it forgets them.

use serde_json::{json, Value};

use service_com_amazonaws_chimesdkidentity_chimeidentityservice::state::MAX_DELETED_APP_INSTANCES;

use crate::app_instance_lifecycle::{uri_encode, Emulator};

/// One row of a reused-token table: the case's label and the single change it makes to
/// the request the token already carried.
type MismatchRow = (&'static str, Box<dyn Fn(&mut Value)>);

/// The `Message` AWS words the permanent refusal with, on every operation that serves it.
const FORBIDDEN_MESSAGE: &str = "You are not allowed to make this request.";

/// The `Message` AWS words a token already associated with a resource with.
const TOKEN_ALREADY_ASSOCIATED_MESSAGE: &str =
    "The provided ClientRequestToken is already associated with an existing resource.";

/// The `Message` AWS words an already-existing resource with.
const RESOURCE_ALREADY_EXISTS_MESSAGE: &str = "The resource that you are trying to create already exists.";

/// A well-formed Amazon Lex V2 bot-alias ARN, which `Configuration.Lex.LexBotAliasArn`
/// is constrained to
/// (`smithy_query(query="shape",shape="LexConfiguration")`: len 15..2048, pattern
/// `^arn:aws:lex:[a-z]{2}-[a-z]+-\d{1}:\d{12}:bot-alias/[A-Z0-9]{10}/[A-Z0-9]{10}$`).
fn lex_configuration() -> Value {
    json!({
        "Lex": {
            "LexBotAliasArn": "arn:aws:lex:us-west-2:123456789012:bot-alias/BOTIDAAAAA/ALIASAAAAA",
            "LocaleId": "en_US",
            "RespondsTo": "STANDARD_MESSAGES",
        }
    })
}

/// A well-formed Amazon Pinpoint application ARN, which an endpoint's `ResourceArn` names
/// (`smithy_query(query="operation",operation="RegisterAppInstanceUserEndpoint")`); the
/// application id is the 32 hexadecimal characters Pinpoint mints.
fn pinpoint_arn() -> String {
    "arn:aws:mobiletargeting:us-east-1:123456789012:apps/0123456789abcdef0123456789abcdef"
        .to_string()
}

/// `POST /app-instances` with whatever members the row carries.
async fn create_instance(emulator: &Emulator, body: Value) -> (u16, Value) {
    emulator.call("POST", "/app-instances", Some(body)).await
}

/// The request one `ClientRequestToken` first made: every member `CreateAppInstance`
/// takes, so a row can change exactly one of them.
fn instance_body(token: &str) -> Value {
    json!({
        "Name": "original-name",
        "Metadata": "original-metadata",
        "ClientRequestToken": token,
        "Tags": [{ "Key": "env", "Value": "prod" }],
    })
}

/// `POST /app-instance-users` with whatever members the row carries.
async fn create_user(emulator: &Emulator, body: Value) -> (u16, Value) {
    emulator
        .call("POST", "/app-instance-users", Some(body))
        .await
}

/// The request one `ClientRequestToken` first made for a user, with every member
/// `CreateAppInstanceUser` takes.
fn user_body(app_instance_arn: &str, token: &str) -> Value {
    json!({
        "AppInstanceArn": app_instance_arn,
        "AppInstanceUserId": "original-user",
        "Name": "original-owner",
        "Metadata": "original-metadata",
        "ClientRequestToken": token,
        "ExpirationSettings": { "ExpirationDays": 7, "ExpirationCriterion": "CREATED_TIMESTAMP" },
        "Tags": [{ "Key": "env", "Value": "prod" }],
    })
}

/// The ARN a 201 create answered with, under `member`.
fn minted(body: &Value, member: &str, label: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: the create returned no {member}: {body:?}"))
        .to_string()
}

/// Assert that `answer` is the modelled 409 conflict carrying `message`.
fn assert_conflict(label: &str, status: u16, answer: &Value, message: &str) {
    assert_eq!(
        (status, answer["Code"].as_str(), answer["Message"].as_str()),
        (409, Some("Conflict"), Some(message)),
        "case {label:?}: expected the modelled conflict carrying {message:?}, got \
         {status} {answer:?}"
    );
}

/// How many `AppInstance`s the account holds right now.
async fn instance_count(emulator: &Emulator) -> usize {
    let (status, body) = emulator.call("GET", "/app-instances", None).await;
    assert_eq!(status, 200, "expected ListAppInstances to answer 200, got {body:?}");
    body["AppInstances"].as_array().map_or(0, Vec::len)
}

/// RULE: a create's `ClientRequestToken` stands for ONE request — the same token
/// presented with the same request replays the ARN it minted, and the same token
/// presented with any different parameter is refused the modelled conflict and mints
/// nothing.
#[tokio::test]
async fn a_reused_token_replays_only_an_identical_request() {
    let emulator = Emulator::new();

    // The request the token makes first, replayed twice unchanged: a retrying client.
    let token = "tok-reuse-instance";
    let (status, answer) = create_instance(&emulator, instance_body(token)).await;
    assert_eq!(status, 201, "expected the first create to answer 201, got {answer:?}");
    let first = minted(&answer, "AppInstanceArn", "the first create");
    for repeat in 1..=2 {
        let (status, answer) = create_instance(&emulator, instance_body(token)).await;
        assert_eq!(
            (status, answer["AppInstanceArn"].as_str()),
            (201, Some(first.as_str())),
            "case \"retry {repeat}\": expected the identical request to replay {first:?}, \
             got {status} {answer:?}"
        );
    }

    // Rows: the same token with exactly ONE member of the request changed — one row per
    // member `CreateAppInstance` takes, plus each way a tag set can differ.
    let instance_rows: Vec<MismatchRow> = vec![
        (
            "a different Name",
            Box::new(|body: &mut Value| body["Name"] = json!("conflict-name")),
        ),
        (
            "a different Metadata",
            Box::new(|body: &mut Value| body["Metadata"] = json!("different-metadata")),
        ),
        (
            "Metadata left out",
            Box::new(|body: &mut Value| {
                body.as_object_mut().expect("an object body").remove("Metadata");
            }),
        ),
        (
            "a tag value changed",
            Box::new(|body: &mut Value| body["Tags"] = json!([{ "Key": "env", "Value": "dev" }])),
        ),
        (
            "a tag added",
            Box::new(|body: &mut Value| {
                body["Tags"] = json!([
                    { "Key": "env", "Value": "prod" },
                    { "Key": "tier", "Value": "1" },
                ]);
            }),
        ),
        (
            "the tags left out",
            Box::new(|body: &mut Value| {
                body.as_object_mut().expect("an object body").remove("Tags");
            }),
        ),
    ];

    let before = instance_count(&emulator).await;
    for (label, change) in instance_rows {
        let mut body = instance_body(token);
        change(&mut body);
        let (status, answer) = create_instance(&emulator, body).await;
        assert_conflict(label, status, &answer, TOKEN_ALREADY_ASSOCIATED_MESSAGE);
        assert_eq!(
            instance_count(&emulator).await,
            before,
            "case {label:?}: a refused create must mint no AppInstance"
        );
        // The instance the token DID mint is untouched by the refusal.
        let (status, described) = emulator
            .call("GET", &format!("/app-instances/{}", uri_encode(&first)), None)
            .await;
        assert_eq!(
            (
                status,
                described["AppInstance"]["Name"].as_str(),
                described["AppInstance"]["Metadata"].as_str()
            ),
            (200, Some("original-name"), Some("original-metadata")),
            "case {label:?}: the refused create must leave {first:?} exactly as it was, \
             got {described:?}"
        );
    }

    // The token stands for the REQUEST it made, not for what the resource holds NOW: an
    // update and a tagging call after the fact must not turn an honest retry of the
    // original create into a conflict.
    let (status, answer) = emulator
        .call(
            "PUT",
            &format!("/app-instances/{}", uri_encode(&first)),
            Some(json!({ "Name": "renamed", "Metadata": "changed-metadata" })),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected UpdateAppInstance to answer 200, got {status} {answer:?}"
    );
    let (status, answer) = emulator
        .call(
            "POST",
            "/tags?operation=tag-resource",
            Some(json!({
                "ResourceARN": first,
                "Tags": [{ "Key": "tier", "Value": "1" }],
            })),
        )
        .await;
    assert_eq!(
        status, 204,
        "expected TagResource to answer 204, got {status} {answer:?}"
    );
    let (status, answer) = create_instance(&emulator, instance_body(token)).await;
    assert_eq!(
        (status, answer["AppInstanceArn"].as_str()),
        (201, Some(first.as_str())),
        "case \"a retry after the instance was updated and tagged\": expected the replay \
         of {first:?}, got {status} {answer:?}"
    );

    // The same rule on a child resource, whose ARN is derived from the request: an
    // identical request replays, and each differing member is a conflict.
    let other_instance = emulator
        .app_instance("reuse-other-parent", "tok-reuse-other")
        .await;
    let user_token = "tok-reuse-user";
    let (status, answer) = create_user(&emulator, user_body(&first, user_token)).await;
    assert_eq!(status, 201, "expected the first user create to answer 201, got {answer:?}");
    let user = minted(&answer, "AppInstanceUserArn", "the first user create");
    let (status, answer) = create_user(&emulator, user_body(&first, user_token)).await;
    assert_eq!(
        (status, answer["AppInstanceUserArn"].as_str()),
        (201, Some(user.as_str())),
        "case \"an identical user retry\": expected the replay of {user:?}, got {status} {answer:?}"
    );

    let parent = other_instance.clone();
    let user_rows: Vec<MismatchRow> = vec![
        (
            "a different Name",
            Box::new(|body: &mut Value| body["Name"] = json!("impostor-owner")),
        ),
        (
            "a different Metadata",
            Box::new(|body: &mut Value| body["Metadata"] = json!("different-metadata")),
        ),
        (
            "different ExpirationSettings",
            Box::new(|body: &mut Value| body["ExpirationSettings"]["ExpirationDays"] = json!(30)),
        ),
        (
            "the ExpirationSettings left out",
            Box::new(|body: &mut Value| {
                body.as_object_mut()
                    .expect("an object body")
                    .remove("ExpirationSettings");
            }),
        ),
        (
            "a different tag set",
            Box::new(|body: &mut Value| body["Tags"] = json!([{ "Key": "env", "Value": "dev" }])),
        ),
        (
            "a different AppInstanceUserId",
            Box::new(|body: &mut Value| body["AppInstanceUserId"] = json!("other-user")),
        ),
        (
            "a different parent AppInstance",
            Box::new(move |body: &mut Value| body["AppInstanceArn"] = json!(parent)),
        ),
    ];

    for (label, change) in user_rows {
        let mut body = user_body(&first, user_token);
        change(&mut body);
        let (status, answer) = create_user(&emulator, body).await;
        assert_conflict(label, status, &answer, TOKEN_ALREADY_ASSOCIATED_MESSAGE);
        let (status, described) = emulator
            .call(
                "GET",
                &format!("/app-instance-users/{}", uri_encode(&user)),
                None,
            )
            .await;
        assert_eq!(
            (
                status,
                described["AppInstanceUser"]["Name"].as_str(),
                described["AppInstanceUser"]["Metadata"].as_str()
            ),
            (200, Some("original-owner"), Some("original-metadata")),
            "case {label:?}: the refused create must leave {user:?} exactly as it was, \
             got {described:?}"
        );
    }
}

/// RULE: a create whose resource already exists is refused the modelled conflict
/// carrying AWS's fixed already-exists sentence — whatever the id names the resource —
/// and leaves the stored resource exactly as it was.
#[tokio::test]
async fn a_duplicate_resource_is_refused_with_the_fixed_already_exists_sentence() {
    let emulator = Emulator::new();
    let instance = emulator.app_instance("dup-parent", "tok-dup-parent").await;

    // Rows span the form an `AppInstanceUserId` may take: `UserId` is len 1..64, pattern
    // `^[A-Za-z0-9]([A-Za-z0-9:\-_.@]{0,62}[A-Za-z0-9])?$`
    // (`smithy_query(query="operation",operation="CreateAppInstanceUser")`), so both
    // length bounds and every punctuation character it admits are exercised.
    let ids: Vec<(&str, String)> = vec![
        ("the shortest id the model admits", "a".to_string()),
        ("a plain id", "dup-user".to_string()),
        ("every punctuation character the id admits", "a.b-c_d@e:1".to_string()),
        ("a numeric id", "0123456789".to_string()),
        (
            "the longest id the model admits",
            format!("a{}z", "x".repeat(62)),
        ),
    ];

    for (index, (label, user_id)) in ids.into_iter().enumerate() {
        let body = json!({
            "AppInstanceArn": instance,
            "AppInstanceUserId": user_id,
            "Name": "original-owner",
            "ClientRequestToken": format!("tok-dup-first-{index}"),
        });
        let (status, answer) = create_user(&emulator, body).await;
        assert_eq!(
            status, 201,
            "case {label:?}: expected the first create to answer 201, got {answer:?}"
        );
        let user = minted(&answer, "AppInstanceUserArn", label);

        // A FRESH token, so this is not a token reuse: the id itself already exists.
        let second = json!({
            "AppInstanceArn": instance,
            "AppInstanceUserId": user_id,
            "Name": "impostor-owner",
            "ClientRequestToken": format!("tok-dup-again-{index}"),
        });
        let (status, answer) = create_user(&emulator, second).await;
        assert_conflict(label, status, &answer, RESOURCE_ALREADY_EXISTS_MESSAGE);

        let (status, described) = emulator
            .call(
                "GET",
                &format!("/app-instance-users/{}", uri_encode(&user)),
                None,
            )
            .await;
        assert_eq!(
            (status, described["AppInstanceUser"]["Name"].as_str()),
            (200, Some("original-owner")),
            "case {label:?}: the refused create must leave {user:?} as it was, got {described:?}"
        );
    }
}

/// One addressed request of the table: what it is called, and how it is sent.
struct AddressedCall {
    label: String,
    method: &'static str,
    path: String,
    body: Option<Value>,
}

/// Every operation of this service that is ADDRESSED to an app instance, to one of its
/// users or to one of its bots, built over the three ARNs given.
///
/// `DescribeAppInstanceBot` is not here: it answers the permanent refusal for ANY bot ARN
/// it holds no record for, so it cannot tell the two cases below apart.
/// `DescribeAppInstanceAdmin` is out for the same reason: the grant it addresses is the
/// resource it resolves, so it answers the permanent refusal for any pair of ARNs it
/// holds no promotion for.
fn calls_addressed_to(instance: &str, user: &str, bot: &str) -> Vec<AddressedCall> {
    let call = |label: &str, method: &'static str, path: String, body: Option<Value>| {
        AddressedCall {
            label: label.to_string(),
            method,
            path,
            body,
        }
    };
    vec![
        call(
            "DescribeAppInstance",
            "GET",
            format!("/app-instances/{}", uri_encode(instance)),
            None,
        ),
        call(
            "UpdateAppInstance",
            "PUT",
            format!("/app-instances/{}", uri_encode(instance)),
            Some(json!({ "Name": "ghost", "Metadata": "" })),
        ),
        call(
            "DeleteAppInstance",
            "DELETE",
            format!("/app-instances/{}", uri_encode(instance)),
            None,
        ),
        call(
            "ListAppInstanceUsers",
            "GET",
            format!("/app-instance-users?app-instance-arn={}", uri_encode(instance)),
            None,
        ),
        call(
            "ListAppInstanceBots",
            "GET",
            format!("/app-instance-bots?app-instance-arn={}", uri_encode(instance)),
            None,
        ),
        call(
            "CreateAppInstanceUser",
            "POST",
            "/app-instance-users".to_string(),
            Some(json!({
                "AppInstanceArn": instance,
                "AppInstanceUserId": "ghost-user",
                "Name": "ghost",
                "ClientRequestToken": "tok-ghost-user",
            })),
        ),
        call(
            "CreateAppInstanceBot",
            "POST",
            "/app-instance-bots".to_string(),
            Some(json!({
                "AppInstanceArn": instance,
                "ClientRequestToken": "tok-ghost-bot",
                "Configuration": lex_configuration(),
            })),
        ),
        call(
            "CreateAppInstanceAdmin",
            "POST",
            format!("/app-instances/{}/admins", uri_encode(instance)),
            Some(json!({ "AppInstanceAdminArn": user })),
        ),
        call(
            "DescribeAppInstanceUser",
            "GET",
            format!("/app-instance-users/{}", uri_encode(user)),
            None,
        ),
        call(
            "UpdateAppInstanceUser",
            "PUT",
            format!("/app-instance-users/{}", uri_encode(user)),
            Some(json!({ "Name": "ghost", "Metadata": "" })),
        ),
        call(
            "DeleteAppInstanceUser",
            "DELETE",
            format!("/app-instance-users/{}", uri_encode(user)),
            None,
        ),
        call(
            "PutAppInstanceUserExpirationSettings",
            "PUT",
            format!("/app-instance-users/{}/expiration-settings", uri_encode(user)),
            Some(json!({
                "ExpirationSettings": {
                    "ExpirationDays": 1,
                    "ExpirationCriterion": "CREATED_TIMESTAMP",
                }
            })),
        ),
        call(
            "RegisterAppInstanceUserEndpoint",
            "POST",
            format!("/app-instance-users/{}/endpoints", uri_encode(user)),
            Some(json!({
                "ClientRequestToken": "tok-ghost-endpoint",
                "Type": "APNS",
                "ResourceArn": pinpoint_arn(),
                "EndpointAttributes": { "DeviceToken": "device-token-ghost" },
            })),
        ),
        call(
            "ListAppInstanceUserEndpoints",
            "GET",
            format!("/app-instance-users/{}/endpoints", uri_encode(user)),
            None,
        ),
        call(
            "DeleteAppInstanceBot",
            "DELETE",
            format!("/app-instance-bots/{}", uri_encode(bot)),
            None,
        ),
        call(
            "ListTagsForResource",
            "GET",
            format!("/tags?arn={}", uri_encode(instance)),
            None,
        ),
        call(
            "TagResource",
            "POST",
            "/tags?operation=tag-resource".to_string(),
            Some(json!({
                "ResourceARN": user,
                "Tags": [{ "Key": "env", "Value": "prod" }],
            })),
        ),
        call(
            "UntagResource",
            "POST",
            "/tags?operation=untag-resource".to_string(),
            Some(json!({ "ResourceARN": user, "TagKeys": ["env"] })),
        ),
    ]
}

/// RULE: a request addressed to a deleted `AppInstance` — its own ARN, or the ARN of a
/// resource the delete cascaded away with it — is refused the modelled
/// `ForbiddenException` (403) carrying AWS's fixed forbidden sentence, while an ARN that
/// never named a resource of this service keeps the unknown-ARN refusal (400 `NotFound`).
#[tokio::test]
async fn a_request_addressed_to_a_deleted_app_instance_is_forbidden() {
    let emulator = Emulator::new();

    // An instance holding one of each kind of child the delete cascades to.
    let instance = emulator.app_instance("doomed-app", "tok-dead-parent").await;
    let user = emulator.user(&instance, "cascade-user", "tok-dead-user").await;
    let (status, answer) = emulator
        .call(
            "POST",
            "/app-instance-bots",
            Some(json!({
                "AppInstanceArn": instance,
                "ClientRequestToken": "tok-dead-bot",
                "Name": "cascade-bot",
                "Configuration": lex_configuration(),
            })),
        )
        .await;
    assert_eq!(
        status, 201,
        "expected CreateAppInstanceBot to answer 201, got {status} {answer:?}"
    );
    let bot = minted(&answer, "AppInstanceBotArn", "the cascaded bot");

    // Every addressed call is served while the instance is alive, so the table below
    // tests the DELETE and not a request the service never served.
    let (status, answer) = emulator
        .call("GET", &format!("/app-instances/{}", uri_encode(&instance)), None)
        .await;
    assert_eq!(
        status, 200,
        "expected the live instance to describe, got {status} {answer:?}"
    );
    let (status, answer) = emulator
        .call(
            "GET",
            &format!("/app-instance-users/{}", uri_encode(&user)),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "expected the live user to describe, got {status} {answer:?}"
    );

    let (status, answer) = emulator
        .call(
            "DELETE",
            &format!("/app-instances/{}", uri_encode(&instance)),
            None,
        )
        .await;
    assert_eq!(
        status, 204,
        "expected DeleteAppInstance to answer 204, got {status} {answer:?}"
    );

    for call in calls_addressed_to(&instance, &user, &bot) {
        let AddressedCall {
            label,
            method,
            path,
            body,
        } = call;
        let (status, answer) = emulator.call(method, &path, body).await;
        assert_eq!(
            (
                status,
                answer["Code"].as_str(),
                answer["Message"].as_str()
            ),
            (403, Some("Forbidden"), Some(FORBIDDEN_MESSAGE)),
            "case {label:?} on the deleted AppInstance: expected the modelled forbidden \
             refusal, got {status} {answer:?}"
        );
    }

    // The control: the SAME calls against ARNs of the same shapes that never named a
    // resource keep the unknown-ARN refusal, so the deleted case is a class of its own.
    let live = emulator.app_instance("live-app", "tok-live-parent").await;
    let absent_instance = format!("{live}x");
    let absent_user = format!("{absent_instance}/user/nobody");
    let absent_bot = format!("{absent_instance}/bot/nobody");
    for call in calls_addressed_to(&absent_instance, &absent_user, &absent_bot) {
        let AddressedCall {
            label,
            method,
            path,
            body,
        } = call;
        let (status, answer) = emulator.call(method, &path, body).await;
        assert_eq!(
            (status, answer["Code"].as_str()),
            (400, Some("NotFound")),
            "case {label:?} on an ARN that never existed: expected the unknown-ARN \
             refusal, got {status} {answer:?}"
        );
    }

    // And the distinction is the INSTANCE's: a user deleted on its own, under an
    // instance that is still alive, is still an unknown ARN rather than a forbidden one.
    let orphan = emulator.user(&live, "deleted-alone", "tok-live-user").await;
    let (status, answer) = emulator
        .call(
            "DELETE",
            &format!("/app-instance-users/{}", uri_encode(&orphan)),
            None,
        )
        .await;
    assert_eq!(
        status, 204,
        "expected DeleteAppInstanceUser to answer 204, got {status} {answer:?}"
    );
    let (status, answer) = emulator
        .call(
            "GET",
            &format!("/app-instance-users/{}", uri_encode(&orphan)),
            None,
        )
        .await;
    assert_eq!(
        (status, answer["Code"].as_str()),
        (400, Some("NotFound")),
        "case \"a user deleted under a live AppInstance\": expected the unknown-ARN \
         refusal, got {status} {answer:?}"
    );
}

/// RULE: the set of deleted `AppInstance`s this emulator remembers is BOUNDED at
/// [`MAX_DELETED_APP_INSTANCES`] deletions, oldest forgotten first — so a delete loop
/// cannot grow the store without limit, and once a deletion is forgotten its ARN reads as
/// an unknown one again rather than as a deleted one.
#[tokio::test]
async fn the_remembered_deletions_are_bounded_and_forget_the_oldest_first() {
    let emulator = Emulator::new();

    // One deletion per row, in order, up to the bound: every ARN deleted is still
    // remembered as deleted while the set holds it.
    let mut deleted = Vec::with_capacity(MAX_DELETED_APP_INSTANCES);
    for n in 0..MAX_DELETED_APP_INSTANCES {
        let arn = emulator
            .app_instance(&format!("evicted-{n}"), &format!("tok-evict-{n}"))
            .await;
        let (status, answer) = emulator
            .call("DELETE", &format!("/app-instances/{}", uri_encode(&arn)), None)
            .await;
        assert_eq!(
            status, 204,
            "case \"deletion {n}\": expected DeleteAppInstance to answer 204, got {answer:?}"
        );
        deleted.push(arn);
    }
    for (n, arn) in deleted.iter().enumerate() {
        let (status, answer) = emulator
            .call("GET", &format!("/app-instances/{}", uri_encode(arn)), None)
            .await;
        assert_eq!(
            (status, answer["Code"].as_str()),
            (403, Some("Forbidden")),
            "case \"deletion {n} of {MAX_DELETED_APP_INSTANCES}\": expected a remembered \
             deletion to be forbidden, got {status} {answer:?}"
        );
    }

    // One deletion past the bound: the set still holds exactly the bound, so the OLDEST
    // deletion is the one forgotten and the rest are untouched.
    let overflow = emulator
        .app_instance("evicted-overflow", "tok-evict-overflow")
        .await;
    let (status, answer) = emulator
        .call(
            "DELETE",
            &format!("/app-instances/{}", uri_encode(&overflow)),
            None,
        )
        .await;
    assert_eq!(
        status, 204,
        "expected the deletion past the bound to answer 204, got {status} {answer:?}"
    );

    let oldest = deleted.first().expect("a first deletion").clone();
    let (status, answer) = emulator
        .call("GET", &format!("/app-instances/{}", uri_encode(&oldest)), None)
        .await;
    assert_eq!(
        (status, answer["Code"].as_str()),
        (400, Some("NotFound")),
        "case \"the oldest deletion, one past the bound\": expected it to be forgotten, \
         got {status} {answer:?}"
    );
    for (n, arn) in deleted.iter().enumerate().skip(1) {
        let (status, answer) = emulator
            .call("GET", &format!("/app-instances/{}", uri_encode(arn)), None)
            .await;
        assert_eq!(
            (status, answer["Code"].as_str()),
            (403, Some("Forbidden")),
            "case \"deletion {n} after one eviction\": expected it to be remembered, got \
             {status} {answer:?}"
        );
    }
    let (status, answer) = emulator
        .call(
            "GET",
            &format!("/app-instances/{}", uri_encode(&overflow)),
            None,
        )
        .await;
    assert_eq!(
        (status, answer["Code"].as_str()),
        (403, Some("Forbidden")),
        "case \"the newest deletion\": expected it to be remembered, got {status} {answer:?}"
    );
}
