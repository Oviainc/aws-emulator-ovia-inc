// SPDX-License-Identifier: CC0-1.0

//! How the Support App WORDS the failures it answers with.
//!
//! Three rules are under test, and each is service-wide rather than tied to one
//! operation or one recorded request:
//!
//! 1. A request a HANDLER refuses — one the boundary validator passed, because the rule
//!    it broke is not a modelled constraint — is answered 400 `ValidationException`
//!    whose message is service prose naming the OPERATION that was called, restating
//!    the rule with the request members spelled the way the API spells them, and
//!    closing with "Try your request again.". That holds for every such rule this
//!    service enforces: the `notifyOnCaseSeverity` / notification-flag combination
//!    documented on `CreateSlackChannelConfigurationRequest$notifyOnCaseSeverity`, a
//!    `nextToken` no listing of this service returned, and a registration past the
//!    documented Slack workspace quota.
//! 2. A request refused for a constraint the model DOES declare is answered in the
//!    standard validation envelope (`N validation error(s) detected: ` over one
//!    clause per violation, each naming the rejected value and the member path), with
//!    a `@pattern` requirement stated WITHOUT the `^` / `$` anchors the model
//!    brackets it with.
//! 3. A Slack channel configuration this account does not hold is reported 404
//!    `ResourceNotFoundException` with one template, built from the request's
//!    identifying members in the order the request carries them (`teamId`, then
//!    `channelId`), shared by every operation that looks one up.
//!
//! So the tests are TABLES spanning each class: both operations that enforce the
//! severity rule, every severity level and flag combination on each side of it, both
//! operations that look a configuration up, id pairs at both published `@length`
//! bounds and in several forms, and a `@pattern` row for every patterned member this
//! service declares whose form the BOUNDARY judges — not only the `accountAlias` the issue
//! cited. `channelRoleArn` is the one patterned member outside that class, because the role
//! it names is one the service is asked to use: its own row asserts the 403 the pass-role
//! step answers instead. Each row's
//! expected message is written out in full AND checked against the invariants of its
//! rule, so a message that drifts back to an internal paraphrase fails twice.
//!
//! Every constraint quoted here comes from the model, via
//! `smithy_query(query="operation", operation="CreateSlackChannelConfiguration,UpdateSlackChannelConfiguration,DeleteSlackChannelConfiguration,PutAccountAlias",
//! model_path="services/service-com-amazonaws-supportapp-supportapp/smithy-model/com-amazonaws-supportapp-supportapp.json")`.

use serde_json::{json, Value};
use service_com_amazonaws_supportapp_supportapp::state::{
    CROSS_ACCOUNT_PASS_ROLE_MESSAGE, MAX_SLACK_WORKSPACE_CONFIGURATIONS,
};

use crate::slack_channel_configurations::{role_arn, Emulator, Reply};
use crate::slack_workspace_configurations::list_workspaces;

/// The clause stating the documented `all`/`high` rule: "If you specify high or all,
/// you must specify true for at least one of the following parameters:
/// notifyOnAddCorrespondenceToCase notifyOnCreateOrReopenCase notifyOnResolveCase"
/// (`smithy_query(query="doc", shape="CreateSlackChannelConfigurationRequest", member="notifyOnCaseSeverity")`),
/// in the service's own prose.
const ALL_OR_HIGH_RULE: &str = "If NotifyOnCaseSeverity is all or high, you must enter true for at \
                                least one of the following parameters: \
                                NotifyOnCreateOrReopenCase, NotifyOnAddCorrespondenceToCase or \
                                NotifyOnResolveCase.";

/// The clause stating the other half of the same documented rule: "If you specify
/// none, the following parameters must be null or false".
const NONE_RULE: &str = "If NotifyOnCaseSeverity is none, the following parameters must be null or \
                         false: NotifyOnCreateOrReopenCase, NotifyOnAddCorrespondenceToCase or \
                         NotifyOnResolveCase.";

/// `awsAccountAlias`'s `@pattern`, with the anchors the model spells it with stripped
/// — which is what a clause reporting it must state. Model value: `^[\w\- ]+$`.
const ACCOUNT_ALIAS_PATTERN: &str = r"[\w\- ]+";

/// `teamId`'s and `channelId`'s `@pattern`, unanchored. Model value: `^\S+$`.
const SLACK_ID_PATTERN: &str = r"\S+";

/// `channelName`'s `@pattern`, unanchored. Model value: `^.+$`.
const CHANNEL_NAME_PATTERN: &str = r".+";

/// `roleArn`'s `@pattern`. Model value: `^arn:aws:iam::[0-9]{12}:role/(.+)$`. A value outside
/// it is NOT a boundary refusal on this service — `channelRoleArn` names a role the service
/// is asked to use, so the pass-role step judges it — which is what
/// [`a_value_outside_the_role_arn_pattern_is_the_pass_role_refusal`] pins.
const ROLE_ARN_PATTERN: &str = r"arn:aws:iam::[0-9]{12}:role/(.+)";

/// `teamId` / `channelId` are `@length min 1 max 256`, so a 256-character id is the
/// published upper bound rather than a value past it.
const SLACK_ID_MAX_LEN: usize = 256;

/// The envelope a Support App request-validation message is stated in.
fn invalid_request(operation: &str, rule: &str) -> String {
    format!("Your {operation} request isn't valid. {rule} Try your request again.")
}

/// The template a miss on a Slack channel configuration is reported with.
///
/// `pub(crate)` so the sibling suites in this test target assert the same template
/// instead of each spelling it out.
pub(crate) fn not_found(team_id: &str, channel_id: &str) -> String {
    format!("Slack channel configuration for team {team_id} and channel {channel_id} not found")
}

/// The single-violation validation envelope for a `@pattern` violation.
fn pattern_refusal(value: &str, member: &str, pattern: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member}' failed to satisfy constraint: \
         Member must satisfy regular expression pattern: {pattern}"
    )
}

/// The `message` the reply carried, or a panic naming the whole reply: a refusal that
/// says nothing is as bad as one that says the wrong thing.
fn message_of(case: &str, reply: &Reply) -> String {
    reply
        .string("message")
        .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no message: {}", reply.body))
}

/// Require `reply` to be `status` / `error_type` and return the message it carried.
///
/// `pub(crate)` so the sibling suites in this test target check a refusal the same way.
pub(crate) fn refusal(case: &str, reply: &Reply, status: u16, error_type: &str) -> String {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {} / {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type, error_type,
        "case {case:?}: expected {error_type}, got {:?}",
        reply.error_type
    );
    message_of(case, reply)
}

/// The invariants of the envelope itself, checked on every handler-raised
/// `ValidationException` whatever rule it states: it names the OPERATION whose request
/// was refused, and closes with the retry sentence. A message that is an internal
/// paraphrase has neither.
fn assert_prose_envelope(case: &str, operation: &str, message: &str) {
    let prefix = format!("Your {operation} request isn't valid. ");
    assert!(
        message.starts_with(&prefix),
        "case {case:?}: expected the message to open with {prefix:?}, got {message:?}"
    );
    assert!(
        message.ends_with("Try your request again."),
        "case {case:?}: expected the message to close with the retry sentence, got {message:?}"
    );
    assert!(
        message.len() > prefix.len() + "Try your request again.".len(),
        "case {case:?}: expected a rule clause between the two sentences, got {message:?}"
    );
}

/// A request member this service's prose names must be spelled the way the API spells
/// it (PascalCase), never in the lowerCamel wire spelling an internal paraphrase uses.
fn assert_names_member(case: &str, message: &str, member_in_prose: &str) {
    assert!(
        message.contains(member_in_prose),
        "case {case:?}: expected the message to name {member_in_prose:?}, got {message:?}"
    );
    let mut wire_spelling = member_in_prose.to_string();
    wire_spelling.replace_range(..1, &member_in_prose[..1].to_lowercase());
    assert!(
        !message.contains(&wire_spelling),
        "case {case:?}: a member must be named as the API names it, not {wire_spelling:?}: got \
         {message:?}"
    );
}

/// The invariants of rule 1, checked on every row: the operation-named prose
/// envelope, and no lowerCamel wire spelling of a parameter, which is what an
/// internal paraphrase reads like.
fn assert_service_prose(case: &str, operation: &str, message: &str) {
    assert_prose_envelope(case, operation, message);
    for wire_spelling in [
        "notifyOnCaseSeverity",
        "notifyOnCreateOrReopenCase",
        "notifyOnAddCorrespondenceToCase",
        "notifyOnResolveCase",
    ] {
        assert!(
            !message.contains(wire_spelling),
            "case {case:?}: a parameter must be named as the API names it, not {wire_spelling:?}: \
             got {message:?}"
        );
    }
}

/// The invariants of rule 2, checked on every refusal of a declared constraint: the
/// standard envelope, and no anchor on any reported pattern.
fn assert_validation_envelope(case: &str, message: &str) {
    assert!(
        message.starts_with("1 validation error detected: ")
            || message
                .split_once(" validation errors detected: ")
                .is_some_and(|(count, _)| count.parse::<u32>().is_ok_and(|n| n > 1)),
        "case {case:?}: expected the AWS validation envelope, got {message:?}"
    );
    for clause in message
        .split_once(": ")
        .map(|(_, clauses)| clauses)
        .unwrap_or_default()
        .split("; ")
    {
        if let Some((_, pattern)) =
            clause.split_once("Member must satisfy regular expression pattern: ")
        {
            assert!(
                !pattern.starts_with('^') && !pattern.ends_with('$'),
                "case {case:?}: a reported pattern must carry no anchors, got {pattern:?}"
            );
        }
    }
}

/// A create body every row starts from: valid in every member, so only what a row
/// changes is refused.
fn valid_create() -> Value {
    json!({
        "teamId": "T012ABCDEFG",
        "channelId": "C01234A5BCD",
        "notifyOnCaseSeverity": "high",
        "notifyOnCreateOrReopenCase": true,
        "channelRoleArn": role_arn("SupportAppRole"),
    })
}

/// A create body carrying `severity` and the three notification flags as `flags`
/// gives them, an absent flag being left out of the request entirely.
fn create_with(severity: &str, flags: [Option<bool>; 3]) -> Value {
    let mut body = json!({
        "teamId": "T012ABCDEFG",
        "channelId": "C01234A5BCD",
        "notifyOnCaseSeverity": severity,
        "channelRoleArn": role_arn("SupportAppRole"),
    });
    for (member, flag) in [
        "notifyOnCreateOrReopenCase",
        "notifyOnAddCorrespondenceToCase",
        "notifyOnResolveCase",
    ]
    .into_iter()
    .zip(flags)
    {
        if let Some(value) = flag {
            body[member] = json!(value);
        }
    }
    body
}

#[tokio::test]
async fn the_all_or_high_severity_rule_is_stated_as_service_prose_naming_the_operation() {
    // Rows differ in the severity that needs a flag and in HOW no flag is true:
    // every flag omitted, every flag explicitly false, and mixtures of the two.
    let create_rows: [(&str, &str, [Option<bool>; 3]); 6] = [
        ("high, every flag omitted", "high", [None, None, None]),
        (
            "high, every flag false",
            "high",
            [Some(false), Some(false), Some(false)],
        ),
        (
            "high, one flag false and two omitted",
            "high",
            [None, Some(false), None],
        ),
        ("all, every flag omitted", "all", [None, None, None]),
        (
            "all, every flag false",
            "all",
            [Some(false), Some(false), Some(false)],
        ),
        (
            "all, two flags false and one omitted",
            "all",
            [Some(false), None, Some(false)],
        ),
    ];
    let expected = invalid_request("CreateSlackChannelConfiguration", ALL_OR_HIGH_RULE);
    for (label, severity, flags) in create_rows {
        let emulator = Emulator::new();
        let reply = emulator.create(create_with(severity, flags)).await;
        let message = refusal(label, &reply, 400, "ValidationException");
        assert_eq!(
            message, expected,
            "case {label:?}: expected {expected:?}, got {message:?}"
        );
        assert_service_prose(label, "CreateSlackChannelConfiguration", &message);
        assert!(
            emulator.listed("T012ABCDEFG", "C01234A5BCD").await.is_none(),
            "case {label:?}: a refused create must store nothing"
        );
    }

    // The same rule, the same prose, on the other operation that enforces it — which
    // names ITSELF, because the message states which request was not valid.
    let update_rows: [(&str, Value); 3] = [
        (
            "severity stays high, the stored flag turned off",
            json!({ "notifyOnCaseSeverity": "high", "notifyOnCreateOrReopenCase": false }),
        ),
        (
            "severity to all, the stored flag turned off",
            json!({ "notifyOnCaseSeverity": "all", "notifyOnCreateOrReopenCase": false }),
        ),
        (
            "only the stored flag turned off, severity left alone",
            json!({ "notifyOnCreateOrReopenCase": false }),
        ),
    ];
    let expected = invalid_request("UpdateSlackChannelConfiguration", ALL_OR_HIGH_RULE);
    for (label, patch) in update_rows {
        let emulator = Emulator::new();
        assert_eq!(
            emulator
                .create_in_registered_workspace(valid_create())
                .await
                .status,
            200,
            "case {label:?}: the fixture configuration must be created"
        );
        let mut body = patch;
        body["teamId"] = json!("T012ABCDEFG");
        body["channelId"] = json!("C01234A5BCD");
        let reply = emulator.update(body).await;
        let message = refusal(label, &reply, 400, "ValidationException");
        assert_eq!(
            message, expected,
            "case {label:?}: expected {expected:?}, got {message:?}"
        );
        assert_service_prose(label, "UpdateSlackChannelConfiguration", &message);
    }
}

#[tokio::test]
async fn the_none_severity_rule_is_stated_in_the_same_service_prose() {
    // The other side of the documented rule, over which flag is the true one.
    let create_rows: [(&str, [Option<bool>; 3]); 4] = [
        ("none with create-or-reopen true", [Some(true), None, None]),
        ("none with add-correspondence true", [None, Some(true), None]),
        ("none with resolve true", [None, None, Some(true)]),
        (
            "none with every flag true",
            [Some(true), Some(true), Some(true)],
        ),
    ];
    let expected = invalid_request("CreateSlackChannelConfiguration", NONE_RULE);
    for (label, flags) in create_rows {
        let emulator = Emulator::new();
        let reply = emulator.create(create_with("none", flags)).await;
        let message = refusal(label, &reply, 400, "ValidationException");
        assert_eq!(
            message, expected,
            "case {label:?}: expected {expected:?}, got {message:?}"
        );
        assert_service_prose(label, "CreateSlackChannelConfiguration", &message);
    }

    // And on update, where the offending flag is one the REQUEST carries true beside
    // the `none` severity — the scope that request's wording gives the clause. Rows
    // differ in which flag it is: the one the record already holds true, one the
    // record holds false, and all three at once.
    let update_rows: [(&str, Value); 3] = [
        (
            "severity to none with the stored flag restated true",
            json!({ "notifyOnCaseSeverity": "none", "notifyOnCreateOrReopenCase": true }),
        ),
        (
            "severity to none with another flag turned on",
            json!({ "notifyOnCaseSeverity": "none", "notifyOnResolveCase": true }),
        ),
        (
            "severity to none with every flag turned on",
            json!({
                "notifyOnCaseSeverity": "none",
                "notifyOnCreateOrReopenCase": true,
                "notifyOnAddCorrespondenceToCase": true,
                "notifyOnResolveCase": true,
            }),
        ),
    ];
    let expected = invalid_request("UpdateSlackChannelConfiguration", NONE_RULE);
    for (label, patch) in update_rows {
        let emulator = Emulator::new();
        assert_eq!(
            emulator
                .create_in_registered_workspace(valid_create())
                .await
                .status,
            200,
            "case {label:?}: the fixture configuration must be created"
        );
        let mut body = patch;
        body["teamId"] = json!("T012ABCDEFG");
        body["channelId"] = json!("C01234A5BCD");
        let reply = emulator.update(body).await;
        let message = refusal(label, &reply, 400, "ValidationException");
        assert_eq!(
            message, expected,
            "case {label:?}: expected {expected:?}, got {message:?}"
        );
        assert_service_prose(label, "UpdateSlackChannelConfiguration", &message);
    }
}

#[tokio::test]
async fn a_configuration_this_account_does_not_hold_is_reported_with_one_template() {
    let emulator = Emulator::new();
    // A stored neighbour, so each 404 is about the named pair and not an empty store.
    assert_eq!(
        emulator
            .create_in_registered_workspace(valid_create())
            .await
            .status,
        200,
        "the stored neighbour must be created"
    );

    let longest_id = "T".repeat(SLACK_ID_MAX_LEN);
    // Rows differ in the FORM and LENGTH of the ids the template interpolates: the
    // recorded Slack shapes, both published `@length` bounds, punctuation, and a pair
    // that reuses the stored team with an unconfigured channel.
    let rows: [(&str, &str, &str); 5] = [
        ("Slack-shaped ids", "T999ZZZZZZZ", "C999ZZZZZZZ"),
        ("single-character ids", "t", "c"),
        ("ids at the length maximum", &longest_id, &longest_id),
        ("punctuated ids", "T-0_1.2", "C#9/x"),
        ("the stored team, another channel", "T012ABCDEFG", "C-other"),
    ];
    // A configuration can only be MISSING from a workspace the account holds: an
    // unregistered `teamId` is the documented conflict instead (covered in
    // [`crate::channel_configuration_workspace_precondition`]), so each row's workspace
    // is registered first.
    for (label, team_id, _) in rows {
        assert_eq!(
            emulator.register_workspace(team_id).await.status,
            200,
            "case {label:?}: the workspace the row looks a channel up in must be registered"
        );
    }
    for (label, team_id, channel_id) in rows {
        let expected = not_found(team_id, channel_id);

        let case = format!("{label} / delete");
        let reply = emulator.delete(team_id, channel_id).await;
        let message = refusal(&case, &reply, 404, "ResourceNotFoundException");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );

        let case = format!("{label} / update");
        let reply = emulator
            .update(json!({ "teamId": team_id, "channelId": channel_id }))
            .await;
        let message = refusal(&case, &reply, 404, "ResourceNotFoundException");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // None of those refusals touched the stored neighbour.
    assert!(
        emulator.listed("T012ABCDEFG", "C01234A5BCD").await.is_some(),
        "a refused lookup must leave the stored configuration alone"
    );
}

#[tokio::test]
async fn a_pattern_violation_states_the_model_pattern_without_its_anchors() {
    let emulator = Emulator::new();

    // One row per patterned member this service declares, each with a value that
    // violates its `@pattern` and nothing else: the cited `accountAlias` is one row.
    let mut team_space = valid_create();
    team_space["teamId"] = json!("T012 ABCDEFG");
    let mut channel_space = valid_create();
    channel_space["channelId"] = json!("C0123 45");
    let mut newline_name = valid_create();
    newline_name["channelName"] = json!("first\nsecond");

    let rows: [(&str, &str, Value, &str, &str, &str); 5] = [
        (
            "accountAlias outside its charset",
            "put-account-alias",
            json!({ "accountAlias": "bad alias!" }),
            "bad alias!",
            "accountAlias",
            ACCOUNT_ALIAS_PATTERN,
        ),
        (
            "teamId with a space",
            "create-slack-channel-configuration",
            team_space,
            "T012 ABCDEFG",
            "teamId",
            SLACK_ID_PATTERN,
        ),
        (
            "channelId with a space",
            "create-slack-channel-configuration",
            channel_space,
            "C0123 45",
            "channelId",
            SLACK_ID_PATTERN,
        ),
        (
            "channelName spanning a line break",
            "create-slack-channel-configuration",
            newline_name,
            "first\nsecond",
            "channelName",
            CHANNEL_NAME_PATTERN,
        ),
        (
            "channelId with a space on a delete",
            "delete-slack-channel-configuration",
            json!({ "teamId": "T012ABCDEFG", "channelId": "C0123 45" }),
            "C0123 45",
            "channelId",
            SLACK_ID_PATTERN,
        ),
    ];

    for (label, path, body, value, member, pattern) in rows {
        let reply = emulator.call(path, body).await;
        let message = refusal(label, &reply, 400, "ValidationException");
        let expected = pattern_refusal(value, member, pattern);
        assert_eq!(
            message, expected,
            "case {label:?}: expected {expected:?}, got {message:?}"
        );
        assert_validation_envelope(label, &message);
    }
}

/// The row the table above no longer carries: a `channelRoleArn` outside `roleArn`'s
/// `@pattern` is NOT a request-validation refusal on this service.
///
/// The member names a role the service is asked to USE, so its value goes into the
/// pass-role/authorization step, which refuses anything that is not a role of the calling
/// account with the modelled `AccessDeniedException` (HTTP 403) and AWS's own wording for
/// that refusal — never the 400 `ValidationException` envelope the table above pins for a
/// caller-input mistake. The rows differ in HOW the value leaves the member's form.
#[tokio::test]
async fn a_value_outside_the_role_arn_pattern_is_the_pass_role_refusal() {
    let emulator = Emulator::new();
    for (label, submitted) in [
        (
            "an account segment that is not twelve digits",
            "arn:aws:iam::ABCDEFGHIJKL:role/SupportAppRole".to_string(),
        ),
        (
            "no ARN at all",
            "not-an-iam-role-arn-000000000000".to_string(),
        ),
        (
            "an IAM entity that is not a role",
            format!(
                "arn:aws:iam::{}:user/SupportAppUser",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
    ] {
        let mut body = valid_create();
        body["channelRoleArn"] = json!(submitted);
        let reply = emulator
            .call("create-slack-channel-configuration", body)
            .await;
        let message = refusal(label, &reply, 403, "AccessDeniedException");
        assert_eq!(
            message, CROSS_ACCOUNT_PASS_ROLE_MESSAGE,
            "case {label:?} ({submitted:?}): expected {CROSS_ACCOUNT_PASS_ROLE_MESSAGE:?}, got \
             {message:?}"
        );
        // And the pattern clause the table above states for a caller-input mistake is NOT
        // what such a value is answered with.
        assert!(
            !message.contains(ROLE_ARN_PATTERN),
            "case {label:?}: expected no request-validation clause, got {message:?}"
        );
    }
}

#[tokio::test]
async fn every_other_constraint_class_keeps_the_standard_clause() {
    let emulator = Emulator::new();

    // Only the pattern clause changes: a length bound, an absent required member and
    // an enumerated value set are reported exactly as the standard envelope states
    // them. `accountAlias` is `@length min 1 max 30`, so 31 characters is one past
    // the published maximum.
    let too_long = "a".repeat(31);
    let reply = emulator.put_alias(&too_long).await;
    let message = refusal("alias one past the length maximum", &reply, 400, "ValidationException");
    let expected = format!(
        "1 validation error detected: Value '{too_long}' at 'accountAlias' failed to satisfy \
         constraint: Member must have length less than or equal to 30"
    );
    assert_eq!(
        message, expected,
        "case \"alias one past the length maximum\": expected {expected:?}, got {message:?}"
    );
    assert_validation_envelope("alias one past the length maximum", &message);

    // The published minimum, from the other side: an empty alias is short AND outside
    // the charset, so both clauses are reported, in one counted envelope.
    let reply = emulator.put_alias("").await;
    let message = refusal("empty alias", &reply, 400, "ValidationException");
    let expected = format!(
        "2 validation errors detected: Value '' at 'accountAlias' failed to satisfy constraint: \
         Member must have length greater than or equal to 1; Value '' at 'accountAlias' failed to \
         satisfy constraint: Member must satisfy regular expression pattern: \
         {ACCOUNT_ALIAS_PATTERN}"
    );
    assert_eq!(
        message, expected,
        "case \"empty alias\": expected {expected:?}, got {message:?}"
    );
    assert_validation_envelope("empty alias", &message);

    // An absent `@required` member names the null it was.
    let mut without_team = valid_create();
    without_team
        .as_object_mut()
        .expect("object")
        .remove("teamId");
    let reply = emulator.create(without_team).await;
    let message = refusal("teamId absent", &reply, 400, "ValidationException");
    assert_eq!(
        message,
        "1 validation error detected: Value null at 'teamId' failed to satisfy constraint: \
         Member must not be null",
        "case \"teamId absent\": got {message:?}"
    );
    assert_validation_envelope("teamId absent", &message);

    // A severity outside `NotificationSeverityLevel`'s value set.
    let mut bad_severity = valid_create();
    bad_severity["notifyOnCaseSeverity"] = json!("critical");
    let reply = emulator.create(bad_severity).await;
    let message = refusal("severity outside the value set", &reply, 400, "ValidationException");
    assert!(
        message.contains("at 'notifyOnCaseSeverity' failed to satisfy constraint: Member must \
                          satisfy enum value set: ["),
        "case \"severity outside the value set\": expected the enum clause, got {message:?}"
    );
    assert_validation_envelope("severity outside the value set", &message);

    // Two violations in one request still render one envelope, counted.
    let reply = emulator
        .call(
            "delete-slack-channel-configuration",
            json!({ "teamId": "T012 ABCDEFG", "channelId": "C0123 45" }),
        )
        .await;
    let message = refusal("two patterned members at once", &reply, 400, "ValidationException");
    assert!(
        message.starts_with("2 validation errors detected: "),
        "case \"two patterned members at once\": expected a counted envelope, got {message:?}"
    );
    for member in ["teamId", "channelId"] {
        assert!(
            message.contains(&format!(
                "at '{member}' failed to satisfy constraint: Member must satisfy regular \
                 expression pattern: {SLACK_ID_PATTERN}"
            )),
            "case \"two patterned members at once\": expected a clause for {member:?}, got \
             {message:?}"
        );
    }
    assert_validation_envelope("two patterned members at once", &message);
}

/// The clause a refused pagination token is stated with, for the operation that was
/// called.
///
/// It restates what the member documents — "To retrieve the next batch of results,
/// reissue the search request and include the returned token"
/// (`smithy_query(query="doc", shape="ListSlackChannelConfigurationsRequest", member="nextToken")`)
/// — and names the member in the API's PascalCase spelling, as this service's prose
/// names a request parameter.
fn invalid_token_rule(operation: &str) -> String {
    format!("NextToken must be a pagination token that an earlier {operation} response returned.")
}

/// The clause a registration past the workspace quota is stated with: the documented
/// quota itself ("You can add up to 5 Slack workspaces for your account" —
/// `smithy_query(query="doc", shape="CreateSlackChannelConfiguration")`), read from the
/// constant that enforces it so the test cannot pin a bound the store does not apply.
fn workspace_quota_rule() -> String {
    format!(
        "You can add up to {MAX_SLACK_WORKSPACE_CONFIGURATIONS} Slack workspaces for your account."
    )
}

/// Tokens no `nextToken` of this service can be — neither listing returns one, because
/// each reports a collection a documented account quota bounds and is served whole.
/// Each row is a different form a client could send: not hex at all, hex that is not a
/// whole number of bytes, hex that decodes to bytes that are not text, hex of
/// whitespace, hex of an id, hex of a position, hex of a count no `usize` holds, and a
/// token at the published `paginationToken` length maximum. Every row satisfies the
/// member's own `@length` / `@pattern` constraints, so each is refused by the HANDLER
/// rather than by the boundary validator.
fn bogus_tokens() -> Vec<(&'static str, String)> {
    vec![
        ("not hex", "not-a-cursor".to_string()),
        ("odd-length hex", "abc".to_string()),
        ("hex of bytes that are not text", "fffe".to_string()),
        ("hex of a space", hex::encode(" ")),
        ("hex of a team id", hex::encode("T012ABCDEFG")),
        ("hex of a negative position", hex::encode("-1")),
        ("hex of a padded position", hex::encode("007")),
        ("hex of a fractional position", hex::encode("1.5")),
        ("hex of a position past any count", hex::encode("9".repeat(40))),
        // `paginationToken` is `len 1..256`, so 256 characters is the published upper
        // bound rather than a value past it.
        ("hex-shaped and at the length maximum", "a".repeat(256)),
    ]
}

#[tokio::test]
async fn a_pagination_token_no_listing_returned_is_refused_in_the_service_prose_envelope() {
    let emulator = Emulator::new();
    // Stored state on both resources, so each refusal is about the token rather than
    // about an empty store.
    assert_eq!(
        emulator
            .create_in_registered_workspace(valid_create())
            .await
            .status,
        200,
        "the fixture configuration must be created"
    );

    // Rows span both paginated operations — each names ITSELF, because the message
    // states which request was refused — and every form of a token no listing of this
    // service returned.
    for (label, token) in bogus_tokens() {
        for operation in [
            "ListSlackChannelConfigurations",
            "ListSlackWorkspaceConfigurations",
        ] {
            let case = format!("{operation} / {label}");
            let reply = if operation == "ListSlackChannelConfigurations" {
                emulator.list(Some(&token)).await
            } else {
                list_workspaces(&emulator, Some(&token)).await
            };
            let message = refusal(&case, &reply, 400, "ValidationException");
            let expected = invalid_request(operation, &invalid_token_rule(operation));
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            assert_prose_envelope(&case, operation, &message);
            assert_names_member(&case, &message, "NextToken");
        }
    }

    // A listing with no token is still served, so the refusals above are about the
    // token and not about pagination being refused outright.
    for (operation, reply) in [
        (
            "ListSlackChannelConfigurations",
            emulator.list(None).await,
        ),
        (
            "ListSlackWorkspaceConfigurations",
            list_workspaces(&emulator, None).await,
        ),
    ] {
        assert_eq!(
            reply.status, 200,
            "case {operation:?}: expected an untokened listing to be served, got {} / {}",
            reply.status, reply.body
        );
    }
}

#[tokio::test]
async fn a_registration_past_the_documented_workspace_quota_is_stated_in_the_same_envelope() {
    let emulator = Emulator::new();
    for index in 0..MAX_SLACK_WORKSPACE_CONFIGURATIONS {
        let team_id = format!("T{index:03}FILLER");
        assert_eq!(
            emulator.register_workspace(&team_id).await.status,
            200,
            "case {team_id:?}: the account must fill to its documented workspace quota"
        );
    }

    // Rows differ in the form and length of the team id that cannot be registered,
    // including both published `teamId` `@length` bounds.
    let longest_id = "T".repeat(SLACK_ID_MAX_LEN);
    let rows: [(&str, &str); 4] = [
        ("Slack-shaped id", "T999ZZZZZZZ"),
        ("single-character id", "t"),
        ("id at the length maximum", &longest_id),
        ("punctuated id", "T-0_1.2"),
    ];
    let expected = invalid_request(
        "RegisterSlackWorkspaceForOrganization",
        &workspace_quota_rule(),
    );
    for (label, team_id) in rows {
        let reply = emulator.register_workspace(team_id).await;
        let message = refusal(label, &reply, 400, "ValidationException");
        assert_eq!(
            message, expected,
            "case {label:?}: expected {expected:?}, got {message:?}"
        );
        assert_prose_envelope(label, "RegisterSlackWorkspaceForOrganization", &message);
    }

    // A workspace the account already holds is not a new registration, so it is not
    // past the quota: the refusals above are about the quota and not about the call.
    assert_eq!(
        emulator.register_workspace("T000FILLER").await.status,
        200,
        "expected re-registering a held workspace to be served"
    );
}
