// SPDX-License-Identifier: CC0-1.0

//! `AppInstanceBot` creation and listing, end to end over HTTP.
//!
//! The RULE pinned here: `ListAppInstanceBots` reports the bots PERSISTED under the one
//! `AppInstance` it was asked about — each bot's ARN, `Name` and `Metadata` exactly as
//! created, in a stable key order, paged at `MaxResults` with a `NextToken` that resumes
//! the walk — and nothing else: not another instance's bots, not a bot a refused create
//! never stored.
//!
//! The tables span what the rule generalises over: the `Name`/`Metadata` values and
//! sizes a bot may carry (absent, empty, the model's length bounds, non-ASCII), the bot
//! configuration's shape (the deprecated `RespondsTo` vs `InvokedBy`, each locale, with
//! and without a welcome intent), the number of bots in a collection against the page
//! size (both ends of the model's `MaxResults` range), and how many instances hold bots
//! at once.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::app_instance_lifecycle::{uri_encode, Emulator};

/// A well-formed Amazon Lex V2 bot-alias ARN for `bot_id`/`alias_id`.
///
/// Source: `smithy_query(query="shape",shape="LexConfiguration")` — `LexBotAliasArn` is
/// `@required`, len 15..2048 and
/// `^arn:aws:lex:[a-z]{2}-[a-z]+-\d{1}:\d{12}:bot-alias/[A-Z0-9]{10}/[A-Z0-9]{10}$`, i.e.
/// `arn:aws:lex:REGION:ACCOUNT:bot-alias/MYBOTID/MYBOTALIAS` with ten upper-case
/// alphanumerics in each id.
fn lex_bot_alias_arn(bot_id: &str, alias_id: &str) -> String {
    format!("arn:aws:lex:us-west-2:123456789012:bot-alias/{bot_id}/{alias_id}")
}

/// A `Configuration` driven by the deprecated `RespondsTo` member.
///
/// Source: `smithy_query(query="shape",shape="RespondsTo")` — the enum's only value is
/// `STANDARD_MESSAGES`.
fn responds_to_configuration(lex_bot_alias_arn: &str, locale_id: &str) -> Value {
    json!({
        "Lex": {
            "LexBotAliasArn": lex_bot_alias_arn,
            "LocaleId": locale_id,
            "RespondsTo": "STANDARD_MESSAGES",
        }
    })
}

/// A `Configuration` driven by `InvokedBy`, the member that replaces `RespondsTo`.
///
/// Source: `smithy_query(query="shape",shape="InvokedBy,StandardMessages,TargetedMessages")`
/// — both members are `@required`; `StandardMessages` is one of `AUTO`/`ALL`/`MENTIONS`/
/// `NONE` and `TargetedMessages` one of `ALL`/`NONE`.
fn invoked_by_configuration(
    lex_bot_alias_arn: &str,
    locale_id: &str,
    standard: &str,
    targeted: &str,
    welcome_intent: Option<&str>,
) -> Value {
    let mut lex = json!({
        "LexBotAliasArn": lex_bot_alias_arn,
        "LocaleId": locale_id,
        "InvokedBy": { "StandardMessages": standard, "TargetedMessages": targeted },
    });
    if let Some(intent) = welcome_intent {
        lex["WelcomeIntent"] = json!(intent);
    }
    json!({ "Lex": lex })
}

/// `POST /app-instance-bots`, with only the members the row carries.
async fn create_bot(
    emulator: &Emulator,
    app_instance_arn: &str,
    token: &str,
    name: Option<&str>,
    metadata: Option<&str>,
    configuration: Value,
) -> (u16, Value) {
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
    emulator.call("POST", "/app-instance-bots", Some(body)).await
}

/// Create a bot and return its ARN, failing the test if it was refused.
/// `pub(crate)` so the suites beside this one in the same test binary mint a stored
/// `AppInstanceBot` through this one helper rather than each carrying a copy.
pub(crate) async fn bot(
    emulator: &Emulator,
    app_instance_arn: &str,
    token: &str,
    name: &str,
) -> String {
    let (status, body) = create_bot(
        emulator,
        app_instance_arn,
        token,
        Some(name),
        None,
        responds_to_configuration(&lex_bot_alias_arn("BOTIDAAAAA", "ALIASAAAAA"), "en_US"),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {name:?}: expected CreateAppInstanceBot to answer 201, got {status} {body:?}"
    );
    body["AppInstanceBotArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: CreateAppInstanceBot returned no ARN: {body:?}"))
        .to_string()
}

/// `DELETE /app-instance-bots/{AppInstanceBotArn}`.
async fn delete_bot(emulator: &Emulator, app_instance_bot_arn: &str) -> (u16, Value) {
    emulator
        .call(
            "DELETE",
            &format!(
                "/app-instance-bots/{}",
                uri_encode(app_instance_bot_arn)
            ),
            None,
        )
        .await
}

/// `GET /app-instance-bots?app-instance-arn=…`, plus any extra query parameters.
async fn list_bots_raw(
    emulator: &Emulator,
    app_instance_arn: &str,
    extra_query: &str,
) -> (u16, Value) {
    emulator
        .call(
            "GET",
            &format!(
                "/app-instance-bots?app-instance-arn={}{extra_query}",
                uri_encode(app_instance_arn)
            ),
            None,
        )
        .await
}

/// The whole `AppInstanceBots` list of one instance, failing the test if it was refused.
async fn list_bots(emulator: &Emulator, app_instance_arn: &str) -> Vec<Value> {
    let (status, body) = list_bots_raw(emulator, app_instance_arn, "").await;
    assert_eq!(
        status, 200,
        "expected ListAppInstanceBots to answer 200, got {status} {body:?}"
    );
    assert_eq!(
        body["AppInstanceArn"].as_str(),
        Some(app_instance_arn),
        "expected the listing to echo the AppInstanceArn it was asked about, got {body:?}"
    );
    body["AppInstanceBots"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// One row of the persistence table: what the create carries, and so what the listing
/// must report back.
#[derive(Debug)]
struct BotCase {
    label: &'static str,
    name: Option<String>,
    metadata: Option<String>,
    configuration: Value,
}

/// RULE: `ListAppInstanceBots` reports every bot persisted under the named `AppInstance`
/// with the `Name` and `Metadata` that created it — for any value either may take, and
/// for any shape the bot's `Configuration` takes.
#[tokio::test]
async fn listing_reports_what_create_persisted() {
    // Rows span the model's own constraints: `Name` is optional `ResourceName`
    // (len 0..256, printable + unicode), `Metadata` optional `Metadata` (len 0..1024),
    // and `Configuration.Lex` is driven either by the deprecated `RespondsTo` or by
    // `InvokedBy` over its enum values.
    let cases: Vec<BotCase> = vec![
        BotCase {
            label: "no Name and no Metadata",
            name: None,
            metadata: None,
            configuration: responds_to_configuration(
                &lex_bot_alias_arn("AAAAAAAAAA", "BBBBBBBBBB"),
                "en_US",
            ),
        },
        BotCase {
            label: "an empty Name and an empty Metadata (both lower length bounds)",
            name: Some(String::new()),
            metadata: Some(String::new()),
            configuration: invoked_by_configuration(
                &lex_bot_alias_arn("CCCCCCCCCC", "DDDDDDDDDD"),
                "en_GB",
                "ALL",
                "ALL",
                None,
            ),
        },
        BotCase {
            label: "Name and Metadata at their upper length bounds",
            name: Some("n".repeat(256)),
            metadata: Some("m".repeat(1024)),
            configuration: invoked_by_configuration(
                &lex_bot_alias_arn("EEEEEEEEEE", "FFFFFFFFFF"),
                "fr_FR",
                "MENTIONS",
                "NONE",
                Some("WelcomeIntent"),
            ),
        },
        BotCase {
            label: "a non-ASCII Name with a welcome intent",
            name: Some("ボット・助手 ☃".to_string()),
            metadata: Some("{\"locale\":\"ja_JP\"}".to_string()),
            configuration: invoked_by_configuration(
                &lex_bot_alias_arn("GGGGGGGGGG", "HHHHHHHHHH"),
                "ja_JP",
                "AUTO",
                "NONE",
                Some("Greet"),
            ),
        },
        BotCase {
            label: "a plain Name with no Metadata and no message trigger",
            name: Some("support-desk".to_string()),
            metadata: None,
            configuration: json!({
                "Lex": {
                    "LexBotAliasArn": lex_bot_alias_arn("IIIIIIIIII", "JJJJJJJJJJ"),
                    "LocaleId": "es_ES",
                }
            }),
        },
    ];

    let emulator = Emulator::new();
    let instance = emulator.app_instance("bot-host", "tok-bot-host").await;

    // Every row is created under ONE instance, so the listing has to report all of them
    // together rather than only the last one written.
    let mut expected: Vec<(&str, String, Option<String>, Option<String>)> = Vec::new();
    for (n, case) in cases.iter().enumerate() {
        let (status, body) = create_bot(
            &emulator,
            &instance,
            &format!("tok-bot-{n}"),
            case.name.as_deref(),
            case.metadata.as_deref(),
            case.configuration.clone(),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201, got {status} {body:?}",
            case.label
        );
        let arn = body["AppInstanceBotArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no ARN in {body:?}", case.label))
            .to_string();
        // An AppInstanceBotArn hangs off its parent under the `bot` segment.
        assert!(
            arn.starts_with(&format!("{instance}/bot/")),
            "case {:?}: expected the bot ARN under {instance}/bot/, got {arn:?}",
            case.label
        );
        expected.push((
            case.label,
            arn,
            case.name.clone(),
            case.metadata.clone(),
        ));
    }

    let listed = list_bots(&emulator, &instance).await;
    assert_eq!(
        listed.len(),
        expected.len(),
        "expected {} listed bots, got {listed:?}",
        expected.len()
    );

    for (label, arn, name, metadata) in &expected {
        let summary = listed
            .iter()
            .find(|entry| entry["AppInstanceBotArn"].as_str() == Some(arn.as_str()))
            .unwrap_or_else(|| panic!("case {label:?}: {arn} is not in the listing: {listed:?}"));
        assert_eq!(
            summary["Name"].as_str().map(str::to_string),
            *name,
            "case {label:?}: expected Name {name:?}, got {summary:?}"
        );
        assert_eq!(
            summary["Metadata"].as_str().map(str::to_string),
            *metadata,
            "case {label:?}: expected Metadata {metadata:?}, got {summary:?}"
        );
    }

    // The listing is in a stable key order, so the same request twice reports the same
    // sequence.
    let again = list_bots(&emulator, &instance).await;
    assert_eq!(
        listed, again,
        "expected ListAppInstanceBots to be stable across calls, got {listed:?} then {again:?}"
    );
}

/// RULE: a listing is scoped to the one `AppInstance` it names — a bot is reported by
/// its own parent's listing and by no other.
#[tokio::test]
async fn a_listing_reports_only_its_own_instances_bots() {
    // Rows differ in how many bots each instance holds, including an instance with none.
    let plan: &[(&str, usize)] = &[
        ("an instance with no bots", 0),
        ("an instance with one bot", 1),
        ("an instance with several bots", 4),
    ];

    let emulator = Emulator::new();
    let mut per_instance: Vec<(&str, String, BTreeSet<String>)> = Vec::new();
    for (label, count) in plan {
        let instance = emulator
            .app_instance(label, &format!("tok-scope-{count}"))
            .await;
        let mut arns = BTreeSet::new();
        for n in 0..*count {
            arns.insert(
                bot(
                    &emulator,
                    &instance,
                    &format!("tok-scope-{count}-{n}"),
                    &format!("scoped-{count}-{n}"),
                )
                .await,
            );
        }
        per_instance.push((label, instance, arns));
    }

    for (label, instance, arns) in &per_instance {
        let listed: BTreeSet<String> = list_bots(&emulator, instance)
            .await
            .iter()
            .filter_map(|entry| entry["AppInstanceBotArn"].as_str().map(str::to_string))
            .collect();
        assert_eq!(
            &listed, arns,
            "case {label:?}: expected exactly this instance's bots {arns:?}, got {listed:?}"
        );
    }
}

/// One page of the bot listing: the ARNs it carried and the token that resumes after it.
async fn page(
    emulator: &Emulator,
    app_instance_arn: &str,
    max_results: i32,
    next_token: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let mut query = format!("&max-results={max_results}");
    if let Some(token) = next_token {
        query.push_str(&format!("&next-token={}", uri_encode(token)));
    }
    let (status, body) = list_bots_raw(emulator, app_instance_arn, &query).await;
    assert_eq!(
        status, 200,
        "expected a page of {max_results} to answer 200, got {status} {body:?}"
    );
    let arns = body["AppInstanceBots"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|entry| entry["AppInstanceBotArn"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    (
        arns,
        body["NextToken"].as_str().map(str::to_string),
    )
}

/// RULE: a paginated listing returns at most `MaxResults` entries per page in a stable
/// key order, emits a `NextToken` only while entries remain, and resuming from that
/// token yields every remaining entry exactly once.
#[tokio::test]
async fn pagination_partitions_the_whole_collection() {
    // Rows vary the collection size against the page size, including a page size equal
    // to the collection (so the first page is also the last) and both ends of the
    // model's `MaxResults` range (min 1, max 50).
    let cases: &[(&str, usize, i32)] = &[
        ("one bot, smallest page", 1, 1),
        ("page size equal to the collection", 3, 3),
        ("collection split into uneven pages", 7, 3),
        ("collection smaller than the largest page", 5, 50),
        ("every bot on its own page", 4, 1),
    ];

    for (label, count, max_results) in cases {
        let emulator = Emulator::new();
        let instance = emulator
            .app_instance("paged-bots", &format!("tok-page-{count}-{max_results}"))
            .await;
        let mut created = BTreeSet::new();
        for n in 0..*count {
            created.insert(
                bot(
                    &emulator,
                    &instance,
                    &format!("tok-page-{count}-{max_results}-{n}"),
                    &format!("paged-{n:03}"),
                )
                .await,
            );
        }

        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let (arns, next) = page(&emulator, &instance, *max_results, token.as_deref()).await;
            pages += 1;
            assert!(
                arns.len() <= *max_results as usize,
                "case {label:?}: page {pages} carried {} entries, more than MaxResults {max_results}",
                arns.len()
            );
            walked.extend(arns);
            match next {
                Some(next) => token = Some(next),
                None => break,
            }
            assert!(
                pages <= count + 1,
                "case {label:?}: the walk did not terminate after {pages} page(s), got {walked:?}"
            );
        }

        assert_eq!(
            walked.len(),
            *count,
            "case {label:?}: expected the walk to yield {count} bot(s) exactly once, got {walked:?}"
        );
        let walked_set: BTreeSet<String> = walked.iter().cloned().collect();
        assert_eq!(
            walked_set, created,
            "case {label:?}: expected the walk to cover every created bot {created:?}, got {walked_set:?}"
        );
        let mut sorted = walked.clone();
        sorted.sort();
        assert_eq!(
            walked, sorted,
            "case {label:?}: expected the pages to walk the collection in key order, got {walked:?}"
        );
    }
}

/// RULE: a page request the listing cannot honour — a `MaxResults` outside the modelled
/// range, or a `NextToken` this listing never issued — is refused with a 4xx client
/// error rather than served, and a listing of an `AppInstance` that does not exist is
/// refused with the modelled `NotFound` code.
#[tokio::test]
async fn a_listing_refuses_what_it_cannot_honour() {
    let emulator = Emulator::new();
    let instance = emulator.app_instance("bot-bounds", "tok-bot-bounds").await;
    let _first = bot(&emulator, &instance, "tok-bounds-bot", "bounds-bot").await;

    // `MaxResults` is `range(min: 1, max: 50)`: each published bound is served and one
    // past it is refused.
    let cases: &[(&str, i32, bool)] = &[
        ("the lower bound", 1, true),
        ("the upper bound", 50, true),
        ("one below the lower bound", 0, false),
        ("one past the upper bound", 51, false),
    ];
    for (label, max_results, served) in cases {
        let (status, body) =
            list_bots_raw(&emulator, &instance, &format!("&max-results={max_results}")).await;
        if *served {
            assert_eq!(
                status, 200,
                "case {label:?}: MaxResults {max_results} is inside the modelled range, got {status} {body:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {label:?}: MaxResults {max_results} is outside the modelled range, got {status} {body:?}"
            );
        }
    }

    // A token this listing never issued: a foreign string, and a well-formed token
    // issued by the SIBLING user listing of the same instance.
    let (_users, foreign_token) = {
        // Fill the user listing so it issues a token of its own.
        for n in 0..2 {
            emulator
                .user(&instance, &format!("u{n}"), &format!("tok-bounds-u{n}"))
                .await;
        }
        let (status, body) = emulator
            .call(
                "GET",
                &format!(
                    "/app-instance-users?app-instance-arn={}&max-results=1",
                    uri_encode(&instance)
                ),
                None,
            )
            .await;
        assert_eq!(status, 200, "ListAppInstanceUsers answered: {body:?}");
        (
            body["AppInstanceUsers"].clone(),
            body["NextToken"]
                .as_str()
                .expect("the user listing issued a NextToken")
                .to_string(),
        )
    };

    for (label, token) in [
        ("a token that is not hex at all", "not-a-token".to_string()),
        ("hex that decodes to something else", "6162636465".to_string()),
        ("a token issued by the sibling user listing", foreign_token),
    ] {
        let (status, body) = list_bots_raw(
            &emulator,
            &instance,
            &format!("&next-token={}", uri_encode(&token)),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected a NextToken this listing never issued to be refused, got {status} {body:?}"
        );
    }

    // An AppInstance this service does not hold.
    let absent = format!("{instance}x");
    let (status, body) = list_bots_raw(&emulator, &absent, "").await;
    assert_eq!(
        status, 400,
        "expected listing the bots of an absent AppInstance to be refused, got {status} {body:?}"
    );
    assert_eq!(
        body["Code"].as_str(),
        Some("NotFound"),
        "expected the absent-AppInstance refusal to carry the NotFound code, got {body:?}"
    );
}

/// RULE: a create the service refuses persists nothing, so the listing never reports a
/// bot a refused create named — whatever made the request invalid.
#[tokio::test]
async fn a_refused_create_is_never_listed() {
    let emulator = Emulator::new();
    let instance = emulator.app_instance("bot-refusals", "tok-bot-refuse").await;
    let kept = bot(&emulator, &instance, "tok-refuse-kept", "kept-bot").await;

    // Rows span the ways a create of a bot is invalid: the Lex bot-alias ARN outside its
    // modelled pattern (truncated, lower-case ids, a wholly different service's ARN, an
    // empty string), a Configuration missing a required member, and a parent instance
    // that does not exist.
    let malformed_alias_arns = [
        ("an alias ARN with too-short ids", lex_bot_alias_arn("SHORT", "ALSOSHORT")),
        ("an alias ARN with lower-case ids", lex_bot_alias_arn("botidaaaaa", "aliasaaaaa")),
        (
            "an ARN of another service entirely",
            "arn:aws:iam::123456789012:role/not-a-lex-alias".to_string(),
        ),
        ("an empty alias ARN", String::new()),
        (
            "an alias ARN missing its alias segment",
            "arn:aws:lex:us-west-2:123456789012:bot-alias/BOTIDAAAAA".to_string(),
        ),
    ];

    for (label, alias_arn) in &malformed_alias_arns {
        let (status, body) = create_bot(
            &emulator,
            &instance,
            "tok-refuse-lex",
            Some("rejected-bot"),
            None,
            responds_to_configuration(alias_arn, "en_US"),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected a malformed LexBotAliasArn to be refused, got {status} {body:?}"
        );
    }

    // A Configuration whose required members are absent.
    for (label, configuration) in [
        ("no Lex member at all", json!({})),
        (
            "a Lex member with no LexBotAliasArn",
            json!({ "Lex": { "LocaleId": "en_US" } }),
        ),
        (
            "a Lex member with no LocaleId",
            json!({ "Lex": { "LexBotAliasArn": lex_bot_alias_arn("KKKKKKKKKK", "LLLLLLLLLL") } }),
        ),
    ] {
        let (status, body) = create_bot(
            &emulator,
            &instance,
            "tok-refuse-config",
            Some("rejected-bot"),
            None,
            configuration,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected an incomplete Configuration to be refused, got {status} {body:?}"
        );
    }

    // A parent the service does not hold.
    let absent_parent = format!("{instance}x");
    let (status, body) = create_bot(
        &emulator,
        &absent_parent,
        "tok-refuse-parent",
        Some("orphan-bot"),
        None,
        responds_to_configuration(&lex_bot_alias_arn("MMMMMMMMMM", "NNNNNNNNNN"), "en_US"),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected a create under an absent AppInstance to be refused, got {status} {body:?}"
    );
    assert_eq!(
        body["Code"].as_str(),
        Some("NotFound"),
        "expected the absent-parent refusal to carry the NotFound code, got {body:?}"
    );

    // Nothing any refused create named was stored: the listing still reports only the
    // one bot that was actually created.
    let listed: Vec<String> = list_bots(&emulator, &instance)
        .await
        .iter()
        .filter_map(|entry| entry["AppInstanceBotArn"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        listed,
        vec![kept.clone()],
        "expected only the accepted bot {kept} to be listed, got {listed:?}"
    );
}

/// RULE: `ClientRequestToken` is an idempotency token — repeating one token with the
/// same request answers with the ARN its first call minted and creates nothing new,
/// repeating it with a DIFFERENT request is refused as a conflict, and a token never
/// used before always mints a new bot.
#[tokio::test]
async fn a_repeated_client_request_token_mints_nothing_new() {
    // Rows differ in how many times one token is replayed, and in what the mismatched
    // reuse changes (the Name, the Metadata, the configuration).
    let emulator = Emulator::new();
    let instance = emulator.app_instance("bot-idem", "tok-bot-idem").await;
    let configuration =
        responds_to_configuration(&lex_bot_alias_arn("OOOOOOOOOO", "PPPPPPPPPP"), "en_US");

    for (label, replays) in [("one retry", 1usize), ("three retries", 3), ("no retry", 0)] {
        let token = format!("tok-idem-{}", label.replace(' ', "-"));
        let (status, body) = create_bot(
            &emulator,
            &instance,
            &token,
            Some("idempotent-bot"),
            Some("first"),
            configuration.clone(),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {label:?}: expected the first create to answer 201, got {status} {body:?}"
        );
        let minted = body["AppInstanceBotArn"]
            .as_str()
            .expect("the first create returned an ARN")
            .to_string();

        for replay in 0..replays {
            let (status, body) = create_bot(
                &emulator,
                &instance,
                &token,
                Some("idempotent-bot"),
                Some("first"),
                configuration.clone(),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {label:?}: replay {replay} expected 201, got {status} {body:?}"
            );
            assert_eq!(
                body["AppInstanceBotArn"].as_str(),
                Some(minted.as_str()),
                "case {label:?}: replay {replay} expected the first ARN {minted}, got {body:?}"
            );
        }

        // Each mismatched reuse of the SAME token is refused, and still nothing new is
        // created.
        for (mismatch, name, metadata, config) in [
            (
                "a different Name",
                Some("other-bot"),
                Some("first"),
                configuration.clone(),
            ),
            (
                "a different Metadata",
                Some("idempotent-bot"),
                Some("second"),
                configuration.clone(),
            ),
            (
                "no Metadata at all",
                Some("idempotent-bot"),
                None,
                configuration.clone(),
            ),
            (
                "a different Configuration",
                Some("idempotent-bot"),
                Some("first"),
                invoked_by_configuration(
                    &lex_bot_alias_arn("QQQQQQQQQQ", "RRRRRRRRRR"),
                    "en_US",
                    "NONE",
                    "ALL",
                    None,
                ),
            ),
        ] {
            let (status, body) =
                create_bot(&emulator, &instance, &token, name, metadata, config).await;
            assert_eq!(
                status, 409,
                "case {label:?}/{mismatch:?}: expected a mismatched token reuse to conflict, got {status} {body:?}"
            );
            assert_eq!(
                body["Code"].as_str(),
                Some("Conflict"),
                "case {label:?}/{mismatch:?}: expected the Conflict code, got {body:?}"
            );
            assert_eq!(
                body["Message"].as_str(),
                Some("The provided ClientRequestToken is already associated with an existing resource."),
                "case {label:?}/{mismatch:?}: expected the fixed already-associated message, got {body:?}"
            );
        }
    }

    // Three tokens were used above; each minted exactly one bot, and the mismatched
    // reuses minted none.
    let listed = list_bots(&emulator, &instance).await;
    assert_eq!(
        listed.len(),
        3,
        "expected one bot per distinct token, got {listed:?}"
    );

    // The request a token replays includes the parent it names and the tags it carries:
    // a tag set in another ORDER is the same request, a different set or a different
    // parent is not.
    let other_instance = emulator.app_instance("bot-idem-other", "tok-bot-idem-2").await;
    let tagged = |parent: &str, tags: Value| {
        json!({
            "AppInstanceArn": parent,
            "ClientRequestToken": "tok-idem-tagged",
            "Name": "tagged-bot",
            "Configuration": configuration.clone(),
            "Tags": tags,
        })
    };
    let (status, body) = emulator
        .call(
            "POST",
            "/app-instance-bots",
            Some(tagged(
                &instance,
                json!([{"Key": "env", "Value": "test"}, {"Key": "team", "Value": "chat"}]),
            )),
        )
        .await;
    assert_eq!(
        status, 201,
        "expected the tagged create to answer 201, got {status} {body:?}"
    );
    let tagged_arn = body["AppInstanceBotArn"]
        .as_str()
        .expect("the tagged create returned an ARN")
        .to_string();

    for (label, parent, tags, expected_status) in [
        (
            "the same tags in the other order",
            instance.clone(),
            json!([{"Key": "team", "Value": "chat"}, {"Key": "env", "Value": "test"}]),
            201,
        ),
        (
            "a different tag value",
            instance.clone(),
            json!([{"Key": "env", "Value": "prod"}, {"Key": "team", "Value": "chat"}]),
            409,
        ),
        (
            "one tag fewer",
            instance.clone(),
            json!([{"Key": "env", "Value": "test"}]),
            409,
        ),
        (
            "the same request under another AppInstance",
            other_instance.clone(),
            json!([{"Key": "env", "Value": "test"}, {"Key": "team", "Value": "chat"}]),
            409,
        ),
    ] {
        let (status, body) = emulator
            .call("POST", "/app-instance-bots", Some(tagged(&parent, tags)))
            .await;
        assert_eq!(
            status, expected_status,
            "case {label:?}: expected HTTP {expected_status}, got {status} {body:?}"
        );
        if expected_status == 201 {
            assert_eq!(
                body["AppInstanceBotArn"].as_str(),
                Some(tagged_arn.as_str()),
                "case {label:?}: expected the replayed ARN {tagged_arn}, got {body:?}"
            );
        }
    }

    // The mismatched reuses created nothing, under either parent.
    assert_eq!(
        list_bots(&emulator, &instance).await.len(),
        4,
        "expected the tagged bot to be the only addition to this instance"
    );
    assert!(
        list_bots(&emulator, &other_instance).await.is_empty(),
        "expected the refused reuse to store nothing under the other AppInstance"
    );
}

/// RULE: deleting an `AppInstanceBot` removes it from its parent's listing and leaves
/// every other bot in place, while a bot ARN the service does not hold — including one
/// already deleted — is refused with the modelled `NotFound` code rather than answered.
#[tokio::test]
async fn deleting_a_bot_removes_it_from_the_listing() {
    // Rows differ in WHICH bot of a collection is deleted (the first in key order, one
    // in the middle, the last) and in how many are left behind.
    let emulator = Emulator::new();
    let instance = emulator.app_instance("bot-deletes", "tok-bot-delete").await;

    // Each bot is remembered with the token that minted it, then ordered by ARN — the
    // key order the listing walks — so a row can name "the first bot in key order"
    // without assuming which token minted it.
    let mut created: Vec<(String, String)> = Vec::new();
    for n in 0..4 {
        let token = format!("tok-delete-{n}");
        let arn = bot(&emulator, &instance, &token, &format!("deletable-{n}")).await;
        created.push((token, arn));
    }
    created.sort_by(|left, right| left.1.cmp(&right.1));

    for (label, index) in [
        ("the first bot in key order", 0usize),
        ("a bot in the middle", 1),
        ("the last remaining bot", 2),
    ] {
        let doomed = created[index].1.clone();
        let (status, body) = delete_bot(&emulator, &doomed).await;
        assert_eq!(
            status, 204,
            "case {label:?}: expected DeleteAppInstanceBot to answer 204, got {status} {body:?}"
        );

        let listed: BTreeSet<String> = list_bots(&emulator, &instance)
            .await
            .iter()
            .filter_map(|entry| entry["AppInstanceBotArn"].as_str().map(str::to_string))
            .collect();
        assert!(
            !listed.contains(&doomed),
            "case {label:?}: expected {doomed} to stop being listed, got {listed:?}"
        );
        let expected: BTreeSet<String> = created[(index + 1)..]
            .iter()
            .map(|(_token, arn)| arn.clone())
            .collect();
        assert_eq!(
            listed, expected,
            "case {label:?}: expected the remaining bots {expected:?}, got {listed:?}"
        );

        // The second delete of the same ARN has nothing to delete.
        let (status, body) = delete_bot(&emulator, &doomed).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected a repeated delete to be refused, got {status} {body:?}"
        );
        assert_eq!(
            body["Code"].as_str(),
            Some("NotFound"),
            "case {label:?}: expected the repeated delete to carry the NotFound code, got {body:?}"
        );
    }

    // An ARN under the right parent that was never minted.
    let (status, body) = delete_bot(&emulator, &format!("{instance}/bot/never-minted")).await;
    assert_eq!(
        status, 400,
        "expected deleting a bot ARN the service never minted to be refused, got {status} {body:?}"
    );
    assert_eq!(
        body["Code"].as_str(),
        Some("NotFound"),
        "expected the unknown-bot refusal to carry the NotFound code, got {body:?}"
    );

    // A deleted bot's idempotency token no longer replays it: the token that minted the
    // first deleted bot mints a NEW one, which the listing then reports.
    let (released_token, deleted_arn) = created[0].clone();
    let remade = bot(&emulator, &instance, &released_token, "remade-bot").await;
    assert_ne!(
        remade, deleted_arn,
        "expected the reused token of a deleted bot to mint a new ARN, got {remade}"
    );
    let listed: BTreeSet<String> = list_bots(&emulator, &instance)
        .await
        .iter()
        .filter_map(|entry| entry["AppInstanceBotArn"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        listed,
        BTreeSet::from([created[3].1.clone(), remade.clone()]),
        "expected the surviving bot and the remade one, got {listed:?}"
    );
}

/// RULE: deleting an `AppInstance` cascades to the bots it owns, so they stop being
/// listed with it while another instance's bots are untouched.
#[tokio::test]
async fn deleting_an_app_instance_cascades_to_its_bots() {
    let emulator = Emulator::new();
    let doomed = emulator.app_instance("doomed-app", "tok-cascade-1").await;
    let kept = emulator.app_instance("kept-app", "tok-cascade-2").await;

    // Rows: an instance holding several bots, and a bystander holding one.
    for n in 0..3 {
        bot(
            &emulator,
            &doomed,
            &format!("tok-cascade-doomed-{n}"),
            &format!("doomed-{n}"),
        )
        .await;
    }
    let survivor = bot(&emulator, &kept, "tok-cascade-kept", "kept-bot").await;

    let (status, body) = emulator
        .call(
            "DELETE",
            &format!("/app-instances/{}", uri_encode(&doomed)),
            None,
        )
        .await;
    assert_eq!(
        status, 204,
        "expected DeleteAppInstance to answer 204, got {status} {body:?}"
    );

    // The deleted AppInstance can no longer serve a listing of the bots it held, so the
    // refusal is the modelled permanent one rather than the unknown-ARN report.
    let (status, body) = list_bots_raw(&emulator, &doomed, "").await;
    assert_eq!(
        (status, body["Code"].as_str()),
        (403, Some("Forbidden")),
        "expected the deleted instance's bot listing to be refused, got {status} {body:?}"
    );

    let listed: Vec<String> = list_bots(&emulator, &kept)
        .await
        .iter()
        .filter_map(|entry| entry["AppInstanceBotArn"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        listed,
        vec![survivor.clone()],
        "expected the bystander's bot {survivor} to be untouched, got {listed:?}"
    );
}
