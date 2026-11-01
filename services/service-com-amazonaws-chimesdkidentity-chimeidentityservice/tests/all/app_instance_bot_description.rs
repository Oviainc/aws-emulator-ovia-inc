// SPDX-License-Identifier: CC0-1.0

//! `DescribeAppInstanceBot`, end to end over HTTP.
//!
//! The RULE pinned here: `DescribeAppInstanceBot` REPORTS the bot persisted under the
//! `AppInstanceBotArn` it is asked about — that ARN, the `Name`, `Metadata` and
//! `Configuration` the bot was created with, and the instants it was created and last
//! written at — and changes nothing; an `AppInstanceBotArn` this service holds no bot for
//! is an ARN the caller may not read, so it is refused with the modelled 403
//! `ForbiddenException` rather than answered or told that the resource is absent.
//!
//! The tables span what the rule generalises over: which optional members a bot carries
//! and at which sizes (absent, empty, the model's length maxima, non-ASCII), the shape of
//! its `Configuration` (the deprecated `RespondsTo` against each `InvokedBy` combination,
//! several locales, with and without a welcome intent), and every way an ARN can fail to
//! resolve to a bot (never minted, under an absent parent, deleted, cascaded away, or
//! naming another resource kind entirely).

use serde_json::{json, Value};

use crate::app_instance_lifecycle::{uri_encode, Emulator};

/// The one sentence this service refuses an ARN it will not resolve with.
const FORBIDDEN_MESSAGE: &str = "You are not allowed to make this request.";

/// A well-formed Amazon Lex V2 bot-alias ARN for `bot_id`/`alias_id`.
///
/// Source: `smithy_query(query="shape",shape="LexConfiguration")` — `LexBotAliasArn` is
/// `@required`, len 15..2048 and
/// `^arn:aws:lex:[a-z]{2}-[a-z]+-\d{1}:\d{12}:bot-alias/[A-Z0-9]{10}/[A-Z0-9]{10}$`.
fn lex_bot_alias_arn(bot_id: &str, alias_id: &str) -> String {
    format!("arn:aws:lex:us-west-2:123456789012:bot-alias/{bot_id}/{alias_id}")
}

/// A `Configuration` driven by the deprecated `RespondsTo` member.
///
/// Source: `smithy_query(query="shape",shape="RespondsTo")` — the enum's only value is
/// `STANDARD_MESSAGES`. `LocaleId` is `@required` and modelled as a free string the Lex V2
/// service resolves, so a locale is a value this table varies rather than a catalog.
fn responds_to_configuration(bot_id: &str, locale_id: &str) -> Value {
    json!({
        "Lex": {
            "LexBotAliasArn": lex_bot_alias_arn(bot_id, "ALIASAAAAA"),
            "LocaleId": locale_id,
            "RespondsTo": "STANDARD_MESSAGES",
        }
    })
}

/// A `Configuration` driven by `InvokedBy`, the member that replaces `RespondsTo`.
///
/// Source: `smithy_query(query="shape",shape="InvokedBy")` — both members are
/// `@required`; `StandardMessages` is one of `AUTO`/`ALL`/`MENTIONS`/`NONE` and
/// `TargetedMessages` one of `ALL`/`NONE`. `WelcomeIntent` is optional, len 1..100,
/// pattern `^([A-Za-z]_?)+$`.
fn invoked_by_configuration(
    bot_id: &str,
    locale_id: &str,
    standard: &str,
    targeted: &str,
    welcome_intent: Option<&str>,
) -> Value {
    let mut lex = json!({
        "LexBotAliasArn": lex_bot_alias_arn(bot_id, "ALIASBBBBB"),
        "LocaleId": locale_id,
        "InvokedBy": { "StandardMessages": standard, "TargetedMessages": targeted },
    });
    if let Some(intent) = welcome_intent {
        lex["WelcomeIntent"] = json!(intent);
    }
    json!({ "Lex": lex })
}

/// `POST /app-instance-bots`, with only the members the row carries, returning the ARN
/// the create minted.
async fn create_bot(
    emulator: &Emulator,
    app_instance_arn: &str,
    token: &str,
    name: Option<&str>,
    metadata: Option<&str>,
    configuration: &Value,
) -> String {
    let mut body = json!({
        "AppInstanceArn": app_instance_arn,
        "ClientRequestToken": token,
        "Configuration": configuration,
    });
    if let Some(name) = name {
        body["Name"] = json!(name);
    }
    if let Some(metadata) = metadata {
        body["Metadata"] = json!(metadata);
    }
    let (status, body) = emulator.call("POST", "/app-instance-bots", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {token:?}: expected CreateAppInstanceBot to answer 201, got {status} {body:?}"
    );
    body["AppInstanceBotArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {token:?}: CreateAppInstanceBot returned no ARN: {body:?}"))
        .to_string()
}

/// `GET /app-instance-bots/{AppInstanceBotArn}`.
async fn describe_bot(emulator: &Emulator, app_instance_bot_arn: &str) -> (u16, Value) {
    emulator
        .call(
            "GET",
            &format!(
                "/app-instance-bots/{}",
                uri_encode(app_instance_bot_arn)
            ),
            None,
        )
        .await
}

/// `DELETE /app-instance-bots/{AppInstanceBotArn}`, failing the test if it was refused.
async fn delete_bot(emulator: &Emulator, app_instance_bot_arn: &str) {
    let (status, body) = emulator
        .call(
            "DELETE",
            &format!(
                "/app-instance-bots/{}",
                uri_encode(app_instance_bot_arn)
            ),
            None,
        )
        .await;
    assert_eq!(
        status, 204,
        "expected DeleteAppInstanceBot to answer 204, got {status} {body:?}"
    );
}

/// `DELETE /app-instances/{AppInstanceArn}`, failing the test if it was refused.
async fn delete_app_instance(emulator: &Emulator, app_instance_arn: &str) {
    let (status, body) = emulator
        .call(
            "DELETE",
            &format!("/app-instances/{}", uri_encode(app_instance_arn)),
            None,
        )
        .await;
    assert_eq!(
        status, 204,
        "expected DeleteAppInstance to answer 204, got {status} {body:?}"
    );
}

/// `arn` with its ACCOUNT segment (the 5th) replaced, so a row can name the same
/// resource in another account without hardcoding the emulator's account id.
fn in_another_account(arn: &str) -> String {
    let mut segments: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(segments.len(), 6, "expected a six-segment ARN, got {arn:?}");
    let other = if segments[4] == "210987654321" {
        "123456789012"
    } else {
        "210987654321"
    };
    segments[4] = other;
    segments.join(":")
}

/// The reported member, or `Value::Null` when the response omits it.
fn member(described: &Value, name: &str) -> Value {
    described["AppInstanceBot"]
        .get(name)
        .cloned()
        .unwrap_or(Value::Null)
}

/// RULE: a describe reports the bot PERSISTED under the ARN it names — that ARN and the
/// `Name`, `Metadata` and `Configuration` the create stored, member for member, with an
/// absent optional member still absent — plus the instants it was created and last
/// written at.
#[tokio::test]
async fn a_describe_reports_the_bot_that_was_created() {
    let emulator = Emulator::new();
    let instance = emulator.app_instance("described-bots", "tok-describe").await;

    // Rows differ in which optional members the bot carries and at which size (absent,
    // empty, the modelled maxima of `Name` len 0..256 and `Metadata` len 0..1024,
    // non-ASCII), and in the shape of the configuration: the deprecated `RespondsTo`
    // against each documented `InvokedBy` combination, a different locale each time, and
    // a welcome intent present or not.
    let max_name = "n".repeat(256);
    let max_metadata = "m".repeat(1024);
    let cases: Vec<(&str, Option<&str>, Option<&str>, Value)> = vec![
        (
            "no Name, no Metadata, RespondsTo",
            None,
            None,
            responds_to_configuration("BOTIDAAAAA", "en_US"),
        ),
        (
            "empty Name and Metadata",
            Some(""),
            Some(""),
            responds_to_configuration("BOTIDBBBBB", "de_DE"),
        ),
        (
            "Name at the modelled maximum",
            Some(max_name.as_str()),
            None,
            invoked_by_configuration("BOTIDCCCCC", "ja_JP", "ALL", "NONE", None),
        ),
        (
            "Metadata at the modelled maximum",
            Some("sized"),
            Some(max_metadata.as_str()),
            invoked_by_configuration("BOTIDDDDDD", "fr_FR", "AUTO", "ALL", Some("Welcome")),
        ),
        (
            "non-ASCII Name and Metadata",
            Some("ボット ⚙"),
            Some("メタデータ — ©"),
            invoked_by_configuration(
                "BOTIDEEEEE",
                "es_US",
                "MENTIONS",
                "NONE",
                Some("Greet_User"),
            ),
        ),
        (
            "a bot nothing triggers",
            Some("silent"),
            Some("triggers nothing"),
            invoked_by_configuration("BOTIDFFFFF", "it_IT", "NONE", "NONE", None),
        ),
    ];

    let mut minted: Vec<(&str, String)> = Vec::new();
    for (index, (label, name, metadata, configuration)) in cases.iter().enumerate() {
        let arn = create_bot(
            &emulator,
            &instance,
            &format!("tok-described-{index}"),
            *name,
            *metadata,
            configuration,
        )
        .await;

        let (status, body) = describe_bot(&emulator, &arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected DescribeAppInstanceBot to answer 200, got {status} {body:?}"
        );
        assert_eq!(
            member(&body, "AppInstanceBotArn").as_str(),
            Some(arn.as_str()),
            "case {label:?}: expected the described ARN {arn:?}, got {body:?}"
        );
        assert_eq!(
            member(&body, "Name"),
            name.map(|name| json!(name)).unwrap_or(Value::Null),
            "case {label:?}: expected the Name the create stored ({name:?}), got {body:?}"
        );
        assert_eq!(
            member(&body, "Metadata"),
            metadata.map(|value| json!(value)).unwrap_or(Value::Null),
            "case {label:?}: expected the Metadata the create stored ({metadata:?}), got {body:?}"
        );
        assert_eq!(
            &member(&body, "Configuration"),
            configuration,
            "case {label:?}: expected the Configuration the create stored, got {body:?}"
        );

        let created = member(&body, "CreatedTimestamp").as_f64().unwrap_or_else(|| {
            panic!("case {label:?}: expected a numeric CreatedTimestamp, got {body:?}")
        });
        let updated = member(&body, "LastUpdatedTimestamp")
            .as_f64()
            .unwrap_or_else(|| {
                panic!("case {label:?}: expected a numeric LastUpdatedTimestamp, got {body:?}")
            });
        assert!(
            created > 0.0,
            "case {label:?}: expected a CreatedTimestamp after the epoch, got {created}"
        );
        assert!(
            updated >= created,
            "case {label:?}: expected LastUpdatedTimestamp {updated} at or after CreatedTimestamp {created}"
        );

        minted.push((label, arn));
    }

    // Every ARN still describes ITS OWN bot once all of them exist: a describe resolves
    // the ARN it was given rather than the last bot written.
    for (index, (label, arn)) in minted.iter().enumerate() {
        let (_, _, expected_metadata, expected_configuration) = &cases[index];
        let (status, body) = describe_bot(&emulator, arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the describe to still answer 200, got {status} {body:?}"
        );
        assert_eq!(
            member(&body, "AppInstanceBotArn").as_str(),
            Some(arn.as_str()),
            "case {label:?}: expected the describe to report {arn:?}, got {body:?}"
        );
        assert_eq!(
            member(&body, "Metadata"),
            expected_metadata
                .map(|value| json!(value))
                .unwrap_or(Value::Null),
            "case {label:?}: expected its own Metadata, got {body:?}"
        );
        assert_eq!(
            &member(&body, "Configuration"),
            expected_configuration,
            "case {label:?}: expected its own Configuration, got {body:?}"
        );
    }
}

/// RULE: a describe REPORTS, so repeating it answers exactly the same bot and leaves
/// every later caller — another describe, the listing — seeing what it saw before.
#[tokio::test]
async fn a_describe_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let instance = emulator.app_instance("stable-bots", "tok-stable").await;

    // Rows differ in how many times the same bot is read before the state is checked.
    for (label, reads) in [("read once", 1_usize), ("read twice", 2), ("read ten times", 10)] {
        let arn = create_bot(
            &emulator,
            &instance,
            &format!("tok-stable-{reads}"),
            Some(label),
            Some("unchanged by reading"),
            &responds_to_configuration("BOTIDSTABL", "en_GB"),
        )
        .await;

        let (status, first) = describe_bot(&emulator, &arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the first describe to answer 200, got {status} {first:?}"
        );
        for read in 1..reads {
            let (status, again) = describe_bot(&emulator, &arn).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected read {read} to answer 200, got {status} {again:?}"
            );
            assert_eq!(
                again, first,
                "case {label:?}: expected read {read} to answer exactly what the first did, got {again:?}"
            );
        }

        // The bot is still listed, and still described, after the reads.
        let (status, listed) = emulator
            .call(
                "GET",
                &format!(
                    "/app-instance-bots?app-instance-arn={}",
                    uri_encode(&instance)
                ),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected ListAppInstanceBots to answer 200, got {status} {listed:?}"
        );
        let holds = listed["AppInstanceBots"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: expected a bot list, got {listed:?}"))
            .iter()
            .any(|bot| bot["AppInstanceBotArn"].as_str() == Some(arn.as_str()));
        assert!(
            holds,
            "case {label:?}: expected the read bot {arn:?} to still be listed, got {listed:?}"
        );
    }
}

/// RULE: an `AppInstanceBotArn` this service holds no bot for is refused with the modelled
/// 403 `ForbiddenException` — `Code` `Forbidden` and the one forbidden-request sentence —
/// reporting no bot and disclosing nothing about whether such a resource exists.
#[tokio::test]
async fn an_unresolvable_bot_arn_is_refused() {
    let emulator = Emulator::new();
    let instance = emulator.app_instance("refused-bots", "tok-refused").await;
    let user = emulator.user(&instance, "refused-user", "tok-refused-user").await;
    let kept = create_bot(
        &emulator,
        &instance,
        "tok-refused-kept",
        Some("kept"),
        None,
        &responds_to_configuration("BOTIDKEPTA", "en_US"),
    )
    .await;

    // A bot created and then deleted, and a bot whose parent instance is deleted under
    // it: both ARNs were once real, so they test the refusal and not just a malformed id.
    let deleted = create_bot(
        &emulator,
        &instance,
        "tok-refused-deleted",
        Some("deleted"),
        None,
        &responds_to_configuration("BOTIDDELET", "en_US"),
    )
    .await;
    delete_bot(&emulator, &deleted).await;

    let doomed_instance = emulator.app_instance("doomed", "tok-refused-doomed").await;
    let cascaded = create_bot(
        &emulator,
        &doomed_instance,
        "tok-refused-cascaded",
        Some("cascaded"),
        None,
        &responds_to_configuration("BOTIDCASCA", "en_US"),
    )
    .await;
    delete_app_instance(&emulator, &doomed_instance).await;

    // Rows span every way a well-formed ChimeArn can fail to resolve to a bot.
    for (label, arn) in [
        ("an ARN under a live parent that was never minted", format!("{instance}/bot/never-minted")),
        ("a bot ARN under an AppInstance that does not exist", format!("{instance}x/bot/absent-parent")),
        ("a bot that was deleted", deleted.clone()),
        ("a bot whose parent AppInstance was deleted", cascaded.clone()),
        ("the parent AppInstance's own ARN", instance.clone()),
        ("an AppInstanceUser ARN", user.clone()),
        ("the same bot ARN in another account", in_another_account(&kept)),
    ] {
        let (status, body) = describe_bot(&emulator, &arn).await;
        assert_eq!(
            status, 403,
            "case {label:?}: expected {arn:?} to be refused with 403, got {status} {body:?}"
        );
        assert_eq!(
            body["Code"].as_str(),
            Some("Forbidden"),
            "case {label:?}: expected the refusal to carry the Forbidden code, got {body:?}"
        );
        assert_eq!(
            body["Message"].as_str(),
            Some(FORBIDDEN_MESSAGE),
            "case {label:?}: expected the forbidden-request sentence, got {body:?}"
        );
        assert!(
            body.get("AppInstanceBot").is_none(),
            "case {label:?}: expected the refusal to report no bot, got {body:?}"
        );
    }

    // None of the refusals disturbed the bot that does exist.
    let (status, body) = describe_bot(&emulator, &kept).await;
    assert_eq!(
        status, 200,
        "expected the surviving bot to still be described, got {status} {body:?}"
    );
    assert_eq!(
        member(&body, "Name").as_str(),
        Some("kept"),
        "expected the surviving bot to report the Name it was created with, got {body:?}"
    );
}
