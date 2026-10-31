// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the requirement a `@pattern` violation STATES.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the
//! `MultiServiceRouter`, reusing the harness of `gateway_lifecycle`, so what is asserted
//! is the wire behaviour.
//!
//! The rule under test: a value outside a member's `@pattern` is refused with 400
//! `ValidationException` whose clause states the modelled expression without the
//! whole-value `^` / `$` pair the model writes around it — but only where what that pair
//! encloses carries no anchor of its OWN; an expression that anchors its own branches is
//! already a whole-value expression before any wrapping, and is stated exactly as the model
//! declares it.
//!
//! The table below spans both halves of that rule over the forms this model's patterns
//! take, rather than the one member that exposed the defect:
//!
//! * no anchor of its own, so the pair is dropped — a plain character class, an "any
//!   character" expression, a class wrapped in a group, a NEGATED class (whose `^` is no
//!   anchor), a class carrying escaped brackets, and a branch carrying an alternation inside
//!   a group;
//! * branches that anchor themselves, stated as modelled — an alternation of two separately
//!   anchored branches (one of them the anchored EMPTY string), and that same shape wrapped
//!   in a further anchored pair.
//!
//! Six operations and both `@sensitive` members are covered, and a violation inside a list
//! element as well as at a top-level member.
//!
//! The second test is the other half of the rule: a value each branch of those alternations
//! ACCEPTS is not refused, and reads back — including the empty string the `LogGroupArn`
//! alternation's first branch accepts and the bare `alias/…` form the `KmsKeyArn`
//! alternation's second branch accepts, the two branches a rewritten expression loses.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::gateway_lifecycle::{build_engine, build_router, call, create, Reply, REGION};

// --- Patterns carrying no anchor of their own: the clause drops the model's pair. -------
// Each expression below is its member's modelled `@pattern` minus that pair.

/// `Name`: modelled `^[a-zA-Z0-9-]*$` (`smithy_query(query="shape",shape="Name")`), reached
/// by `ImportHypervisorConfiguration$Name`, `UpdateHypervisor$Name` and
/// `CreateGateway$GatewayDisplayName`.
const NAME_REQUIREMENT: &str = "[a-zA-Z0-9-]*";

/// `ActivationKey`: modelled `^[0-9a-zA-Z\-]+$`
/// (`smithy_query(query="shape",shape="ActivationKey")`).
const ACTIVATION_KEY_REQUIREMENT: &str = r"[0-9a-zA-Z\-]+";

/// `Host`: modelled `^.+$` (`smithy_query(query="shape",shape="Host")`), reached by the
/// `Host` member of `ImportHypervisorConfiguration`, `TestHypervisorConfiguration` and
/// `UpdateHypervisor`.
const HOST_REQUIREMENT: &str = ".+";

/// `Password`: modelled `^[ -~]+$` (`smithy_query(query="shape",shape="Password")`).
const PASSWORD_REQUIREMENT: &str = "[ -~]+";

/// `Username`: modelled `^[ -\.0-\[\]-~]*[!-\.0-\[\]-~][ -\.0-\[\]-~]*$`
/// (`smithy_query(query="shape",shape="Username")`) — a class carrying escaped brackets,
/// which are members of the class and not its delimiters.
const USERNAME_REQUIREMENT: &str = r"[ -\.0-\[\]-~]*[!-\.0-\[\]-~][ -\.0-\[\]-~]*";

/// `TagKey`: modelled `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`
/// (`smithy_query(query="shape",shape="TagKey")`) — a class wrapped in a group, which the
/// clause keeps because the group is the expression, not its anchoring.
const TAG_KEY_REQUIREMENT: &str = r"([\p{L}\p{Z}\p{N}_.:/=+\-@]*)";

/// `TagValue`: modelled `^[^\x00]*$` (`smithy_query(query="shape",shape="TagValue")`) — the
/// second `^` NEGATES the character class and is no anchor, so it stays.
const TAG_VALUE_REQUIREMENT: &str = r"[^\x00]*";

/// `ServerArn`: modelled
/// `^arn:(aws|aws-cn|aws-us-gov):backup-gateway(:[a-zA-Z-0-9]+){3}\/[a-zA-Z-0-9]+$`
/// (`smithy_query(query="shape",shape="ServerArn")`, which `GatewayArn` and `ResourceArn`
/// spell identically) — it carries no anchor of its own, its `|` being an alternation of a
/// group's branches, so the model's pair is dropped.
const SERVER_ARN_REQUIREMENT: &str =
    r"arn:(aws|aws-cn|aws-us-gov):backup-gateway(:[a-zA-Z-0-9]+){3}\/[a-zA-Z-0-9]+";

// --- Patterns whose branches anchor themselves: stated as the model declares them. ------

/// `LogGroupArn`, verbatim (`smithy_query(query="shape",shape="LogGroupArn")`): an
/// alternation whose first branch is the anchored EMPTY string — the only way a caller
/// states "no log group" — and whose second is an anchored ARN. Dropping the first branch's
/// `^` and the second's `$` would leave an alternative that is a bare end-of-input anchor,
/// which every value satisfies.
const LOG_GROUP_ARN_REQUIREMENT: &str =
    r"^$|^arn:(aws|aws-cn|aws-us-gov):logs:([a-zA-Z0-9-]+):([0-9]+):log-group:[a-zA-Z0-9_\-\/\.]+:\*$";

/// `KmsKeyArn`, verbatim (`smithy_query(query="shape",shape="KmsKeyArn")`): the same shape,
/// with the alternation of two separately anchored branches wrapped in a further anchored
/// pair.
const KMS_KEY_ARN_REQUIREMENT: &str =
    r"^(^arn:(aws|aws-cn|aws-us-gov):kms:([a-zA-Z0-9-]+):([0-9]+):(key|alias)/(\S+)$)|(^alias/(\S+)$)$";

/// The published minimum length of `KmsKeyArn` and `ServerArn`
/// (`smithy_query(query="shape",shape="KmsKeyArn,ServerArn")` -> `@length` min 50): a row
/// refused for its PATTERN has to satisfy the length bound, or the refusal would state two
/// clauses.
const ARN_MIN_LENGTH: usize = 50;

/// The refusal message, wherever the envelope carries it.
fn message_of(reply: &Reply) -> String {
    reply
        .str_at("/message")
        .or_else(|| reply.str_at("/Message"))
        .unwrap_or_else(|| panic!("no message in {:?}", reply.body))
        .to_string()
}

/// The whole message a single refused `@pattern` must state: the envelope, the subject, the
/// member's validation path, and the expression the member requires.
fn expected_message(subject: &str, path: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: {subject} at '{path}' failed to satisfy constraint: \
         Member must satisfy regular expression pattern: {requirement}"
    )
}

/// A value of at least [`ARN_MIN_LENGTH`] characters that no ARN-shaped pattern in this
/// model accepts: it satisfies the member's `@length` so the PATTERN is the only constraint
/// that can fire.
fn long_enough_non_arn(prefix: &str) -> String {
    let padding = ARN_MIN_LENGTH.saturating_sub(prefix.len());
    format!("{prefix}{}", "x".repeat(padding))
}

/// One refused request and the member whose pattern it violated.
struct Row {
    case: &'static str,
    operation: &'static str,
    body: Value,
    /// The member's validation path, as the refusal names it.
    path: &'static str,
    /// The rejected value as the clause reproduces it, or `None` for a `@sensitive` member,
    /// whose clause names the member alone.
    shown: Option<String>,
    /// The expression the clause has to state.
    requirement: &'static str,
}

#[tokio::test]
async fn a_pattern_refusal_states_the_expression_the_member_requires() {
    let engine = build_engine();
    let router = build_router(&engine);
    let gateway_arn = create(&router, "gw-patterns").await;
    let hypervisor_arn = import(&router, "hv-patterns", "198.51.100.10", None).await;
    // Valid values for the members a row is not about, so each refusal states ONE clause.
    let host = "198.51.100.10";
    let name = "hv-one";
    let unopened_log_group = log_group_arn("/aws/backup-gateway/one")
        .trim_end_matches(":*")
        .to_string();
    let not_a_kms_key = long_enough_non_arn("not-a-kms-key-");
    let spaced_alias = long_enough_non_arn("alias/a key with spaces ");
    let not_a_hypervisor_arn = long_enough_non_arn("arn:aws-dark:backup-gateway:");

    let rows: Vec<Row> = vec![
        Row {
            case: "CreateGateway activation key with a space",
            operation: "CreateGateway",
            body: json!({
                "ActivationKey": "bad key",
                "GatewayDisplayName": "gw-one",
                "GatewayType": "BACKUP_VM",
            }),
            path: "activationKey",
            shown: Some("bad key".to_string()),
            requirement: ACTIVATION_KEY_REQUIREMENT,
        },
        Row {
            case: "CreateGateway display name with an underscore",
            operation: "CreateGateway",
            body: json!({
                "ActivationKey": "KEY1",
                "GatewayDisplayName": "my_gateway",
                "GatewayType": "BACKUP_VM",
            }),
            path: "gatewayDisplayName",
            shown: Some("my_gateway".to_string()),
            requirement: NAME_REQUIREMENT,
        },
        Row {
            case: "ImportHypervisorConfiguration name with an underscore",
            operation: "ImportHypervisorConfiguration",
            body: json!({ "Name": "bad_name", "Host": host }),
            path: "name",
            shown: Some("bad_name".to_string()),
            requirement: NAME_REQUIREMENT,
        },
        Row {
            case: "ImportHypervisorConfiguration host of three newlines",
            operation: "ImportHypervisorConfiguration",
            body: json!({ "Name": name, "Host": "\n\n\n" }),
            path: "host",
            shown: Some("\n\n\n".to_string()),
            requirement: HOST_REQUIREMENT,
        },
        Row {
            case: "ImportHypervisorConfiguration password with a tab",
            operation: "ImportHypervisorConfiguration",
            body: json!({ "Name": name, "Host": host, "Password": "bad\tpassword" }),
            path: "password",
            shown: None,
            requirement: PASSWORD_REQUIREMENT,
        },
        Row {
            case: "TestHypervisorConfiguration username with a tab",
            operation: "TestHypervisorConfiguration",
            body: json!({
                "GatewayArn": gateway_arn,
                "Host": host,
                "Username": "bad\tusername",
            }),
            path: "username",
            shown: None,
            requirement: USERNAME_REQUIREMENT,
        },
        Row {
            case: "TagResource tag key with an exclamation mark",
            operation: "TagResource",
            body: json!({
                "ResourceARN": gateway_arn,
                "Tags": [{ "Key": "bad!key", "Value": "test" }],
            }),
            path: "tags.1.member.key",
            shown: Some("bad!key".to_string()),
            requirement: TAG_KEY_REQUIREMENT,
        },
        Row {
            case: "TagResource tag value carrying a NUL",
            operation: "TagResource",
            body: json!({
                "ResourceARN": gateway_arn,
                "Tags": [{ "Key": "env", "Value": "bad\u{0}value" }],
            }),
            path: "tags.1.member.value",
            shown: Some("bad\u{0}value".to_string()),
            requirement: TAG_VALUE_REQUIREMENT,
        },
        Row {
            case: "UpdateHypervisor hypervisor ARN of a partition the pattern does not name",
            operation: "UpdateHypervisor",
            body: json!({ "HypervisorArn": not_a_hypervisor_arn }),
            path: "hypervisorArn",
            shown: Some(not_a_hypervisor_arn.clone()),
            requirement: SERVER_ARN_REQUIREMENT,
        },
        Row {
            // The member the alternation defect showed on: a word that is neither the
            // empty string the first branch accepts nor the ARN the second one does.
            case: "UpdateHypervisor log group that is a bare word",
            operation: "UpdateHypervisor",
            body: json!({ "HypervisorArn": hypervisor_arn, "LogGroupArn": "nope" }),
            path: "logGroupArn",
            shown: Some("nope".to_string()),
            requirement: LOG_GROUP_ARN_REQUIREMENT,
        },
        Row {
            case: "UpdateHypervisor log group ARN without the trailing stream wildcard",
            operation: "UpdateHypervisor",
            body: json!({
                "HypervisorArn": hypervisor_arn,
                "LogGroupArn": unopened_log_group,
            }),
            path: "logGroupArn",
            shown: Some(unopened_log_group.clone()),
            requirement: LOG_GROUP_ARN_REQUIREMENT,
        },
        Row {
            case: "ImportHypervisorConfiguration kms key that is neither an ARN nor an alias",
            operation: "ImportHypervisorConfiguration",
            body: json!({ "Name": name, "Host": host, "KmsKeyArn": not_a_kms_key }),
            path: "kmsKeyArn",
            shown: Some(not_a_kms_key.clone()),
            requirement: KMS_KEY_ARN_REQUIREMENT,
        },
        Row {
            case: "ImportHypervisorConfiguration kms key alias carrying a space",
            operation: "ImportHypervisorConfiguration",
            body: json!({ "Name": name, "Host": host, "KmsKeyArn": spaced_alias }),
            path: "kmsKeyArn",
            shown: Some(spaced_alias.clone()),
            requirement: KMS_KEY_ARN_REQUIREMENT,
        },
    ];

    for row in &rows {
        let refused = call(&router, row.operation, &row.body).await;
        assert_eq!(
            (refused.status, refused.code.as_deref()),
            (400, Some("ValidationException")),
            "case {:?}: a value outside a modelled @pattern must be refused with 400 \
             ValidationException, got {} {:?}",
            row.case,
            refused.status,
            refused.body
        );
        let subject = match &row.shown {
            Some(shown) => format!("Value '{shown}'"),
            None => "Value".to_string(),
        };
        let stated = message_of(&refused);
        assert_eq!(
            stated,
            expected_message(&subject, row.path, row.requirement),
            "case {:?}: the clause must state the expression the member requires, got \
             {stated:?}",
            row.case
        );
    }
}

/// The other half of the rule: every branch of the two alternations above is a value the
/// member really accepts, so the clause states a requirement that tells accepted from
/// refused.
///
/// The rows span the branches a rewritten expression would lose — the empty `LogGroupArn`
/// and the bare `alias/…` `KmsKeyArn` — beside the ARN branch of each, and every accepted
/// value is read back through `GetHypervisor`.
#[tokio::test]
async fn a_value_each_branch_of_an_alternation_accepts_is_stored_and_read_back() {
    let engine = build_engine();
    let router = build_router(&engine);

    // The second branch of the `KmsKeyArn` alternation, at the published minimum length.
    let bare_alias = "alias/backup-gateway-emulator-hypervisor-key-00001";
    assert_eq!(
        bare_alias.len(),
        ARN_MIN_LENGTH,
        "the bare-alias row must sit on the published minimum length, got {}",
        bare_alias.len()
    );
    let account = engine::EMULATOR_ACCOUNT_ID;

    // (case, the accepted `KmsKeyArn`)
    let kms_rows: [(&str, String); 3] = [
        (
            "a key ARN, the first branch of the alternation",
            format!("arn:aws:kms:{REGION}:{account}:key/1234abcd-12ab-34cd-56ef-1234567890ab"),
        ),
        (
            "an alias ARN, the same branch's other form",
            format!("arn:aws:kms:{REGION}:{account}:alias/aws/backup-gateway-emulator"),
        ),
        (
            "a bare alias, the second branch of the alternation",
            bare_alias.to_string(),
        ),
    ];

    for (case, kms_key_arn) in &kms_rows {
        let arn = import(&router, "hv-kms", "198.51.100.10", Some(kms_key_arn)).await;
        let read = get_hypervisor(&router, &arn).await;
        assert_eq!(
            (read.status, read.str_at("/Hypervisor/KmsKeyArn")),
            (200, Some(kms_key_arn.as_str())),
            "case {case:?}: a KmsKeyArn the modelled pattern accepts must be stored and \
             read back, got {} {:?}",
            read.status,
            read.body
        );
    }

    // (case, the accepted `LogGroupArn`, what a read must report for it)
    let attached = log_group_arn("/aws/backup-gateway/attached");
    let log_group_rows: [(&str, &str, Option<&str>); 3] = [
        (
            "a log group ARN, the second branch of the alternation",
            attached.as_str(),
            Some(attached.as_str()),
        ),
        (
            "the empty string, the first branch of the alternation",
            "",
            None,
        ),
        (
            "a log group ARN again, so the empty row detached rather than broke the record",
            attached.as_str(),
            Some(attached.as_str()),
        ),
    ];

    let arn = import(&router, "hv-logs", "198.51.100.11", None).await;
    for (case, log_group, reported) in &log_group_rows {
        let updated = call(
            &router,
            "UpdateHypervisor",
            &json!({ "HypervisorArn": arn, "LogGroupArn": log_group }),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: a LogGroupArn the modelled pattern accepts must be written, \
             got {} {:?}",
            updated.status, updated.body
        );
        let read = get_hypervisor(&router, &arn).await;
        assert_eq!(
            (read.status, read.str_at("/Hypervisor/LogGroupArn")),
            (200, *reported),
            "case {case:?}: the written LogGroupArn must read back, got {} {:?}",
            read.status,
            read.body
        );
    }
}

/// A `LogGroupArn` of the form the member's pattern accepts
/// (`smithy_query(query="shape",shape="LogGroupArn")`).
fn log_group_arn(name: &str) -> String {
    format!(
        "arn:aws:logs:{REGION}:{}:log-group:{name}:*",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// Import one hypervisor over the wire and hand back the ARN the store holds it under.
async fn import(
    router: &MultiServiceRouter,
    name: &str,
    host: &str,
    kms_key_arn: Option<&str>,
) -> String {
    let mut body = json!({ "Name": name, "Host": host });
    if let Some(kms_key_arn) = kms_key_arn {
        body["KmsKeyArn"] = json!(kms_key_arn);
    }
    let reply = call(router, "ImportHypervisorConfiguration", &body).await;
    assert_eq!(
        reply.status, 200,
        "case {name:?}: ImportHypervisorConfiguration must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .str_at("/HypervisorArn")
        .unwrap_or_else(|| panic!("no HypervisorArn in {:?}", reply.body))
        .to_string()
}

async fn get_hypervisor(router: &MultiServiceRouter, arn: &str) -> Reply {
    call(router, "GetHypervisor", &json!({ "HypervisorArn": arn })).await
}
