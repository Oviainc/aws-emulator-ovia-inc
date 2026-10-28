// SPDX-License-Identifier: CC0-1.0

//! Two rules the registry holds every ACTIVATED public extension to, end to end over the
//! awsQuery wire.
//!
//! 1. **Activating a public RESOURCE extension requires an execution role: a request that
//!    names no `ExecutionRoleArn` is refused with the registry's own modelled
//!    `CFNRegistryException` (400) carrying the execution-role sentence and writes nothing,
//!    while a MODULE and a Hook — which declare no handlers CloudFormation invokes on the
//!    activating account's behalf — activate without one.**
//!
//! 2. **The registry holds an extension's configuration document as ASCII: every codepoint
//!    outside ASCII reads back replaced by one `?`, and nothing else about the document
//!    changes — every delimiter, member name and nesting level survives, so the document a
//!    `BatchDescribeTypeConfigurations` reports parses to the same tree the write submitted
//!    modulo that substitution.**
//!
//! The tables span what the rules generalise over: for the first, each kind the modelled
//! `ThirdPartyType` admits, both ways a request can name the public extension
//! (`PublicTypeArn`, or `Type` + `TypeName` + `PublisherId`), and with and without an alias;
//! for the second, nesting depth, every JSON value kind, and codepoints on both sides of the
//! ASCII boundary — the last ASCII codepoint, a Latin-1 letter, a two-byte, a three-byte and
//! a four-byte encoding — each inside a value, inside a member NAME and inside a nested
//! array.
//!
//! `type_configurations.rs` is the suite for what an activation and a configuration WRITE and
//! read back; this one is the two rules above.

use crate::stack_lifecycle::{call, member, router, xml_blocks, xml_text, ACCOUNT_ID, REGION};
use service_com_amazonaws_cloudformation_cloudformation::state::registry;
use smithy_server_runtime::MultiServiceRouter;

/// The publisher id a public third-party extension is named by, within the modelled
/// `PublisherId` pattern `^[0-9a-zA-Z]{12,40}$`.
const PUBLISHER_ID: &str = "abcdef0123456789abcd";

/// The execution role an activation names, inside the modelled `RoleARN2` pattern
/// `^arn:.+:iam::[0-9]{12}:role/.+$`.
const EXECUTION_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/extension-execution";

/// The refusal AWS answers an activation of a public RESOURCE extension that names no
/// execution role. Entirely fixed text: no part of it comes from the request or the store.
const NO_EXECUTION_ROLE: &str = "Please provide execution role to activate the Resource type";

/// The ARN of a public extension, as the modelled `ThirdPartyTypeArn` pattern spells one:
/// `arn:aws:cloudformation:<region>::type/<kind>/<publisher id>/<Type-Name>`.
fn public_type_arn(kind: &str, type_name: &str) -> String {
    format!(
        "arn:aws:cloudformation:{REGION}::type/{}/{PUBLISHER_ID}/{}",
        kind.to_ascii_lowercase(),
        type_name.replace("::", "-")
    )
}

/// The ARN this account addresses an activation by: `type/<kind>/<Local-Name>` in this
/// account and Region.
fn activated_arn(kind: &str, local_name: &str) -> String {
    format!(
        "arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:type/{}/{}",
        kind.to_ascii_lowercase(),
        local_name.replace("::", "-")
    )
}

/// One row of the execution-role table.
struct ActivationCase {
    case: &'static str,
    /// The kinds the modelled `ThirdPartyType` admits: `HOOK | MODULE | RESOURCE`
    /// (`smithy_query(query="shape", shape="ThirdPartyType")`).
    kind: &'static str,
    type_name: &'static str,
    /// The alias the activation gives it, if any.
    alias: Option<&'static str>,
    /// Whether the request names the public extension by its ARN.
    by_public_arn: bool,
    /// Whether an activation of this kind needs an execution role.
    needs_execution_role: bool,
}

fn activation_cases() -> Vec<ActivationCase> {
    vec![
        ActivationCase {
            case: "a resource type named by kind, name and publisher",
            kind: registry::registry_type::RESOURCE,
            type_name: "Example::Store::Bucket",
            alias: None,
            by_public_arn: false,
            needs_execution_role: true,
        },
        ActivationCase {
            case: "a resource type named by its public ARN, under an alias",
            kind: registry::registry_type::RESOURCE,
            type_name: "Example::Other::Table",
            alias: Some("Local::Other::Table"),
            by_public_arn: true,
            needs_execution_role: true,
        },
        ActivationCase {
            case: "a module, which declares no handlers",
            kind: registry::registry_type::MODULE,
            type_name: "Example::Network::Subnets::MODULE",
            alias: None,
            by_public_arn: true,
            needs_execution_role: false,
        },
        ActivationCase {
            case: "a Hook, invoked on the stack operation's own credentials",
            kind: registry::registry_type::HOOK,
            type_name: "Example::Guard::Hook",
            alias: Some("Local::Guard::Hook"),
            by_public_arn: false,
            needs_execution_role: false,
        },
    ]
}

/// The members one row's `ActivateType` request carries, without the execution role.
fn activation_members(case: &ActivationCase) -> Vec<(String, String)> {
    let mut members = Vec::new();
    if case.by_public_arn {
        members.push(member(
            "PublicTypeArn",
            &public_type_arn(case.kind, case.type_name),
        ));
    } else {
        members.push(member("Type", case.kind));
        members.push(member("TypeName", case.type_name));
        members.push(member("PublisherId", PUBLISHER_ID));
    }
    if let Some(alias) = case.alias {
        members.push(member("TypeNameAlias", alias));
    }
    members
}

#[tokio::test]
async fn an_activation_needs_an_execution_role_exactly_where_handlers_are_invoked() {
    for case in activation_cases() {
        // A fresh account per row, so each activation is the first one its registry saw.
        let router = router();
        let local_name = case.alias.unwrap_or(case.type_name);
        let expected_arn = activated_arn(case.kind, local_name);

        // Without the role.
        let members = activation_members(&case);
        let (status_code, body) = call(&router, "ActivateType", &members).await;
        if case.needs_execution_role {
            assert_eq!(
                status_code, 400,
                "case {:?}: expected the activation refused with 400, got {status_code} {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "Code").as_deref(),
                Some("CFNRegistryException"),
                "case {:?}: expected CFNRegistryException, got {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "Message").as_deref(),
                Some(NO_EXECUTION_ROLE),
                "case {:?}: expected {NO_EXECUTION_ROLE:?}, got {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "Arn"),
                None,
                "case {:?}: expected no ARN in the refusal, got {body}",
                case.case
            );
            // Nothing was written: the extension is not in this account's registry.
            let (status_code, described) =
                call(&router, "DescribeType", &[member("Arn", &expected_arn)]).await;
            assert_eq!(
                status_code, 404,
                "case {:?}: expected the refused activation to write nothing, got \
                 {status_code} {described}",
                case.case
            );
        } else {
            assert_eq!(
                status_code, 200,
                "case {:?}: expected a kind that needs no execution role to activate, got \
                 {status_code} {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "Arn").as_deref(),
                Some(expected_arn.as_str()),
                "case {:?}: expected the activation's own ARN, got {body}",
                case.case
            );
        }

        // With the role, every kind activates, and the activation is readable at the ARN it
        // answered.
        let mut with_role = activation_members(&case);
        with_role.push(member("ExecutionRoleArn", EXECUTION_ROLE_ARN));
        let (status_code, body) = call(&router, "ActivateType", &with_role).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: expected the activation to succeed, got {status_code} {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "Arn").as_deref(),
            Some(expected_arn.as_str()),
            "case {:?}: expected the activation's own ARN, got {body}",
            case.case
        );
        let (status_code, described) =
            call(&router, "DescribeType", &[member("Arn", &expected_arn)]).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: expected the activation readable, got {status_code} {described}",
            case.case
        );
        assert_eq!(
            xml_text(&described, "IsActivated").as_deref(),
            Some("true"),
            "case {:?}: expected it reported activated, got {described}",
            case.case
        );
        assert_eq!(
            xml_text(&described, "TypeName").as_deref(),
            Some(local_name),
            "case {:?}: expected it held under its local name, got {described}",
            case.case
        );
    }
}

/// One row of the configuration round-trip table: a document to set, and what the registry
/// must report back for it.
struct ConfigurationCase {
    case: &'static str,
    /// What the client sends.
    sent: &'static str,
    /// What a read must report: the same document with every codepoint outside ASCII
    /// replaced by one `?`.
    expected: &'static str,
}

fn configuration_cases() -> Vec<ConfigurationCase> {
    vec![
        ConfigurationCase {
            case: "a flat, wholly ASCII document",
            sent: r#"{"ApiKey":"abc-123"}"#,
            expected: r#"{"ApiKey":"abc-123"}"#,
        },
        ConfigurationCase {
            case: "a nested document carrying every JSON value kind",
            sent: r#"{"Credentials":{"ApiKey":"k","Retries":3,"Ratio":0.5,"Enabled":true,"Absent":null,"Tags":["a","b"],"Nested":{"Deep":{"Deeper":"end"}}}}"#,
            expected: r#"{"Credentials":{"ApiKey":"k","Retries":3,"Ratio":0.5,"Enabled":true,"Absent":null,"Tags":["a","b"],"Nested":{"Deep":{"Deeper":"end"}}}}"#,
        },
        ConfigurationCase {
            case: "the last ASCII codepoint, which is held as it is",
            sent: "{\"Separator\":\"a\u{7f}b\"}",
            expected: "{\"Separator\":\"a\u{7f}b\"}",
        },
        ConfigurationCase {
            case: "a Latin-1 letter, the first codepoint past ASCII",
            sent: r#"{"Owner":"José"}"#,
            expected: r#"{"Owner":"Jos?"}"#,
        },
        ConfigurationCase {
            case: "a two-byte encoding outside Latin-1",
            sent: r#"{"Label":"Привет"}"#,
            expected: r#"{"Label":"??????"}"#,
        },
        ConfigurationCase {
            case: "a three-byte encoding inside a nested array",
            sent: r#"{"Labels":{"Names":["東京","Tokyo"]}}"#,
            expected: r#"{"Labels":{"Names":["??","Tokyo"]}}"#,
        },
        ConfigurationCase {
            case: "a four-byte encoding, one codepoint and so one question mark",
            sent: r#"{"Mood":"🙂"}"#,
            expected: r#"{"Mood":"?"}"#,
        },
        ConfigurationCase {
            case: "a non-ASCII member NAME, so the structure survives too",
            sent: r#"{"Région":{"Nom":"eu-ouest"}}"#,
            expected: r#"{"R?gion":{"Nom":"eu-ouest"}}"#,
        },
    ]
}

/// The `TypeConfigurationDetails` member `name` one read reported.
fn detail(body: &str, name: &str) -> Option<String> {
    let block = xml_blocks(body, "TypeConfigurations").into_iter().next()?;
    xml_text(&block, name)
}

/// Activate the Hook every configuration row is set on, and answer the ARN this account
/// addresses it by.
async fn activate_a_hook(router: &MultiServiceRouter) -> String {
    let (status_code, body) = call(
        router,
        "ActivateType",
        &[
            member("Type", registry::registry_type::HOOK),
            member("TypeName", "Example::Guard::Hook"),
            member("PublisherId", PUBLISHER_ID),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case \"activating the extension to configure\": expected 200, got {status_code} {body}"
    );
    xml_text(&body, "Arn").expect("the activation answers an ARN")
}

#[tokio::test]
async fn a_stored_configuration_is_held_as_ascii_with_its_structure_intact() {
    for ConfigurationCase {
        case,
        sent,
        expected,
    } in configuration_cases()
    {
        // A fresh account per row, so each document is the first one its extension was
        // configured with.
        let router = router();
        let activated = activate_a_hook(&router).await;

        let (status_code, body) = call(
            &router,
            "SetTypeConfiguration",
            &[
                member("TypeArn", &activated),
                member("Configuration", sent),
            ],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected SetTypeConfiguration to succeed, got {status_code} {body}"
        );
        let configuration_arn = xml_text(&body, "ConfigurationArn").unwrap_or_default();

        let (status_code, body) = call(
            &router,
            "BatchDescribeTypeConfigurations",
            &[member(
                "TypeConfigurationIdentifiers.member.1.TypeConfigurationArn",
                &configuration_arn,
            )],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected the batch read to answer, got {status_code} {body}"
        );
        let reported = detail(&body, "Configuration");
        assert_eq!(
            reported.as_deref(),
            Some(expected),
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
        let reported = reported.unwrap_or_default();
        assert!(
            reported.is_ascii(),
            "case {case:?}: expected the stored document to be ASCII, got {reported:?}"
        );
        // Structure intact: the document still parses, and to the same tree the expected
        // text does.
        let read: ::serde_json::Value =
            ::serde_json::from_str(&reported).unwrap_or_else(|error| {
                panic!("case {case:?}: expected JSON, got {reported:?} ({error})")
            });
        let wanted: ::serde_json::Value = ::serde_json::from_str(expected)
            .unwrap_or_else(|error| panic!("case {case:?}: {expected:?} is not JSON ({error})"));
        assert_eq!(
            read, wanted,
            "case {case:?}: expected the document's structure intact, got {read:?}"
        );
        // One `?` per CODEPOINT outside ASCII, not per byte: the document keeps its length
        // in characters.
        assert_eq!(
            reported.chars().count(),
            sent.chars().count(),
            "case {case:?}: expected one character per submitted codepoint, got {reported:?}"
        );
        assert_eq!(
            detail(&body, "Alias").as_deref(),
            Some(registry::DEFAULT_CONFIGURATION_ALIAS),
            "case {case:?}: expected the default alias, got {body}"
        );
        assert_eq!(
            detail(&body, "TypeArn").as_deref(),
            Some(activated.as_str()),
            "case {case:?}: expected the activation's ARN, got {body}"
        );
    }
}

/// Setting a configuration again under the same alias replaces it, and the replacement is
/// held as ASCII too: the coercion is a property of the write, not of the first one.
#[tokio::test]
async fn a_replacement_configuration_is_coerced_as_well() {
    let router = router();
    let activated = activate_a_hook(&router).await;

    for (case, sent, expected) in [
        ("the first write", r#"{"Owner":"ascii"}"#, r#"{"Owner":"ascii"}"#),
        (
            "the replacement",
            r#"{"Owner":"naïve","Extra":"東"}"#,
            r#"{"Owner":"na?ve","Extra":"?"}"#,
        ),
    ] {
        let (status_code, body) = call(
            &router,
            "SetTypeConfiguration",
            &[
                member("TypeArn", &activated),
                member("Configuration", sent),
            ],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected the write to succeed, got {status_code} {body}"
        );
        let configuration_arn = xml_text(&body, "ConfigurationArn").unwrap_or_default();
        let (_, body) = call(
            &router,
            "BatchDescribeTypeConfigurations",
            &[member(
                "TypeConfigurationIdentifiers.member.1.TypeConfigurationArn",
                &configuration_arn,
            )],
        )
        .await;
        assert_eq!(
            detail(&body, "Configuration").as_deref(),
            Some(expected),
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }
}
