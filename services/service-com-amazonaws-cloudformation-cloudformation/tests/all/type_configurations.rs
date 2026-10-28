// SPDX-License-Identifier: CC0-1.0

//! Activating a public third-party extension and configuring it, end to end over the
//! awsQuery wire.
//!
//! The rule every case here holds the service to: **`ActivateType` puts the activation of a
//! public extension in this account and Region's registry and answers the ARN this account
//! addresses it by — for a RESOURCE extension only when the request names an execution role;
//! `SetTypeConfiguration` persists the account-level configuration of the
//! extension THAT ARN names and answers the ARN of the configuration data; and
//! `BatchDescribeTypeConfigurations` reads that same configuration, alias and ARN back,
//! reporting an identifier that resolves to no extension as this batch's own error entry
//! rather than refusing the whole read.**
//!
//! The rows differ along what the rule generalises over: the kind of extension (a Hook, a
//! resource, a module), how the request names the public extension (`PublicTypeArn`, or
//! `Type` + `TypeName` + `PublisherId`), whether the activation takes an alias, whether the
//! configuration is set under an alias of its own or the default one, and how the read
//! identifies the configuration (by the type's ARN, by the configuration's own ARN, by kind
//! and name).
//!
//! The recorded traces this suite was written for are one row of it: a Guard Hook activated
//! with an execution role, configured under the alias `default`, and read back.

use crate::harness::xml_blocks;
use crate::stack_lifecycle::{call, member, router, xml_text};
use service_com_amazonaws_cloudformation_cloudformation::state::registry;
use smithy_server_runtime::MultiServiceRouter;

/// The account and Region every ARN this suite asserts belongs to.
const ACCOUNT_ID: &str = "000000000000";
const REGION: &str = "us-east-1";

/// The publisher id a public third-party extension carries, within the modelled
/// `PublisherId` pattern `^[0-9a-zA-Z]{12,40}$`.
const PUBLISHER_ID: &str = "abcdef0123456789abcd";

/// The configuration a Hook is turned on by, as a client writes it.
const HOOK_CONFIGURATION: &str = r#"{"CloudFormationConfiguration":{"HookConfiguration":{"HookInvocationStatus":"ENABLED","TargetOperations":["STACK"],"FailureMode":"FAIL","Properties":{"ruleLocation":"s3://rules/guard.rules"}}}}"#;

/// The ARN of a public extension, as the modelled `ThirdPartyTypeArn` pattern spells one:
/// `arn:aws:cloudformation:<region>::type/<kind>/<publisher id>/<Type-Name>`.
fn public_type_arn(kind: &str, type_name: &str) -> String {
    format!(
        "arn:aws:cloudformation:{REGION}::type/{}/{PUBLISHER_ID}/{}",
        kind.to_ascii_lowercase(),
        type_name.replace("::", "-")
    )
}

/// One row of the activation-and-configuration table.
struct ExtensionCase {
    case: &'static str,
    /// The `Type` of extension: the kinds the modelled `ThirdPartyType` admits.
    kind: &'static str,
    /// The public extension's own name.
    type_name: &'static str,
    /// The alias the activation gives it, if any — which becomes the name this account
    /// addresses it by.
    alias: Option<&'static str>,
    /// Whether the activation names the public extension by its ARN (else by kind, name and
    /// publisher).
    by_public_arn: bool,
    /// The alias the configuration is set under, if any (else the default one).
    configuration_alias: Option<&'static str>,
    configuration: &'static str,
}

fn extension_cases() -> Vec<ExtensionCase> {
    vec![
        ExtensionCase {
            case: "a Hook activated by kind, name and publisher, configured under default",
            kind: "HOOK",
            type_name: "Example::Guard::Hook",
            alias: None,
            by_public_arn: false,
            configuration_alias: None,
            configuration: HOOK_CONFIGURATION,
        },
        ExtensionCase {
            case: "a Hook activated by its public ARN under an alias",
            kind: "HOOK",
            type_name: "Example::Other::Hook",
            alias: Some("Local::Guard::Hook"),
            by_public_arn: true,
            configuration_alias: Some("default"),
            configuration: HOOK_CONFIGURATION,
        },
        ExtensionCase {
            case: "a resource type configured under an alias of its own",
            kind: "RESOURCE",
            type_name: "Example::Store::Bucket",
            alias: None,
            by_public_arn: false,
            configuration_alias: Some("production"),
            configuration: r#"{"Credentials":{"ApiKey":"{{resolve:secretsmanager:key}}"}}"#,
        },
        ExtensionCase {
            case: "a module activated by its public ARN, configuration naming one member",
            kind: "MODULE",
            type_name: "Example::Network::Subnets::MODULE",
            alias: None,
            by_public_arn: true,
            configuration_alias: Some("Alias2"),
            configuration: r#"{"Properties":{"Cidr":"10.0.0.0/16"}}"#,
        },
    ]
}

/// `ActivateType`, with the members a row varies.
async fn activate(
    router: &MultiServiceRouter,
    case: &ExtensionCase,
) -> (u16, String) {
    let mut members = vec![member(
        "ExecutionRoleArn",
        &format!("arn:aws:iam::{ACCOUNT_ID}:role/extension-role"),
    )];
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
    call(router, "ActivateType", &members).await
}

/// The `TypeConfigurationDetails` block one read answered with, as `(member, text)` pairs.
fn details(body: &str) -> Vec<(String, String)> {
    let block = xml_blocks(body, "TypeConfigurations")
        .into_iter()
        .next()
        .unwrap_or_default();
    ["Arn", "Alias", "Configuration", "TypeArn", "TypeName", "IsDefaultConfiguration"]
        .iter()
        .filter_map(|name| {
            Some(((*name).to_string(), xml_text(&block, name)?))
        })
        .collect()
}

/// What one read reports for `member`, or the empty string.
fn reported(read: &[(String, String)], member: &str) -> String {
    read.iter()
        .find(|(name, _)| name == member)
        .map(|(_, text)| text.clone())
        .unwrap_or_default()
}

#[tokio::test]
async fn an_activated_extension_takes_a_configuration_and_reads_it_back() {
    for case in extension_cases() {
        let router = router();
        let (code, body) = activate(&router, &case).await;
        assert_eq!(
            code, 200,
            "case {:?}: expected ActivateType to succeed, got {code} {body}",
            case.case
        );
        let activated_arn = xml_text(&body, "Arn").unwrap_or_default();
        let local_name = case.alias.unwrap_or(case.type_name);
        let expected_arn = format!(
            "arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:type/{}/{}",
            case.kind.to_ascii_lowercase(),
            local_name.replace("::", "-")
        );
        assert_eq!(
            activated_arn, expected_arn,
            "case {:?}: expected the activation's ARN to name this account, Region, kind and \
             local name, got {activated_arn:?}",
            case.case
        );

        // The activated extension is in this account and Region's registry, under the name
        // this account addresses it by.
        let (code, described) = call(
            &router,
            "DescribeType",
            &[member("Arn", &activated_arn)],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {:?}: expected DescribeType to answer for the activation, got {code} \
             {described}",
            case.case
        );
        assert_eq!(
            xml_text(&described, "TypeName").as_deref(),
            Some(local_name),
            "case {:?}: expected the activation to be held under its local name, got \
             {described}",
            case.case
        );
        assert_eq!(
            xml_text(&described, "IsActivated").as_deref(),
            Some("true"),
            "case {:?}: expected the extension to be reported activated, got {described}",
            case.case
        );

        // Its configuration is persisted against that ARN, and the write answers the ARN of
        // the configuration data.
        let mut members = vec![
            member("TypeArn", &activated_arn),
            member("Configuration", case.configuration),
        ];
        if let Some(alias) = case.configuration_alias {
            members.push(member("ConfigurationAlias", alias));
        }
        let (code, body) = call(&router, "SetTypeConfiguration", &members).await;
        assert_eq!(
            code, 200,
            "case {:?}: expected SetTypeConfiguration to succeed for the ARN ActivateType \
             answered, got {code} {body}",
            case.case
        );
        let configuration_arn = xml_text(&body, "ConfigurationArn").unwrap_or_default();
        let alias = case
            .configuration_alias
            .unwrap_or(registry::DEFAULT_CONFIGURATION_ALIAS);
        let expected_configuration_arn = format!(
            "arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:type-configuration/{}/{}/{alias}",
            case.kind.to_ascii_lowercase(),
            local_name.replace("::", "-")
        );
        assert_eq!(
            configuration_arn, expected_configuration_arn,
            "case {:?}: expected the configuration's own ARN, got {configuration_arn:?}",
            case.case
        );

        // ...and the read reports exactly what the write wrote, however the identifier names
        // it: by the type's ARN and alias, by the configuration's own ARN, and by kind and
        // name.
        let identifiers: Vec<Vec<(String, String)>> = vec![
            vec![
                member("TypeConfigurationIdentifiers.member.1.TypeArn", &activated_arn),
                member(
                    "TypeConfigurationIdentifiers.member.1.TypeConfigurationAlias",
                    alias,
                ),
            ],
            vec![member(
                "TypeConfigurationIdentifiers.member.1.TypeConfigurationArn",
                &configuration_arn,
            )],
            vec![
                member("TypeConfigurationIdentifiers.member.1.Type", case.kind),
                member("TypeConfigurationIdentifiers.member.1.TypeName", local_name),
                member(
                    "TypeConfigurationIdentifiers.member.1.TypeConfigurationAlias",
                    alias,
                ),
            ],
        ];
        for identifier in identifiers {
            let (code, body) =
                call(&router, "BatchDescribeTypeConfigurations", &identifier).await;
            assert_eq!(
                code, 200,
                "case {:?}: expected BatchDescribeTypeConfigurations to answer, got {code} \
                 {body}",
                case.case
            );
            let read = details(&body);
            assert_eq!(
                reported(&read, "Configuration"),
                case.configuration,
                "case {:?} ({identifier:?}): expected the configuration that was set, got \
                 {read:?}",
                case.case
            );
            assert_eq!(
                reported(&read, "Alias"),
                alias,
                "case {:?}: expected the alias the configuration was set under, got {read:?}",
                case.case
            );
            assert_eq!(
                reported(&read, "Arn"),
                configuration_arn,
                "case {:?}: expected the configuration's own ARN, got {read:?}",
                case.case
            );
            assert_eq!(
                reported(&read, "TypeArn"),
                activated_arn,
                "case {:?}: expected the extension's ARN, got {read:?}",
                case.case
            );
            assert_eq!(
                reported(&read, "TypeName"),
                local_name,
                "case {:?}: expected the extension's local name, got {read:?}",
                case.case
            );
            assert_eq!(
                reported(&read, "IsDefaultConfiguration"),
                (alias == registry::DEFAULT_CONFIGURATION_ALIAS).to_string(),
                "case {:?}: expected the default-configuration flag to follow the alias, got \
                 {read:?}",
                case.case
            );
        }

        // Setting the configuration again under the same alias REPLACES it, under the same
        // configuration ARN.
        let revised = r#"{"Properties":{"revised":"true"}}"#;
        let mut members = vec![
            member("TypeArn", &activated_arn),
            member("Configuration", revised),
        ];
        if let Some(alias) = case.configuration_alias {
            members.push(member("ConfigurationAlias", alias));
        }
        let (code, body) = call(&router, "SetTypeConfiguration", &members).await;
        assert_eq!(
            code, 200,
            "case {:?}: expected a second SetTypeConfiguration to succeed, got {code} {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "ConfigurationArn").unwrap_or_default(),
            configuration_arn,
            "case {:?}: expected the replacement to keep the configuration's ARN, got {body}",
            case.case
        );
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
            reported(&details(&body), "Configuration"),
            revised,
            "case {:?}: expected the read to report the configuration last written, got {body}",
            case.case
        );
    }
}

#[tokio::test]
async fn an_identifier_that_resolves_to_nothing_is_this_batch_s_own_error_entry() {
    let router = router();
    let case = &extension_cases()[0];
    let (code, body) = activate(&router, case).await;
    assert_eq!(code, 200, "expected ActivateType to succeed, got {code} {body}");
    let activated_arn = xml_text(&body, "Arn").unwrap_or_default();
    let (code, body) = call(
        &router,
        "SetTypeConfiguration",
        &[
            member("TypeArn", &activated_arn),
            member("Configuration", HOOK_CONFIGURATION),
        ],
    )
    .await;
    assert_eq!(code, 200, "expected SetTypeConfiguration to succeed, got {code} {body}");

    // One identifier the registry answers to, one it does not: the read reports the first
    // and names the second under `Errors`, with the status of the modelled not-found
    // refusal as its `ErrorCode`.
    let unknown = format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:type/hook/No-Such-Hook");
    let (code, body) = call(
        &router,
        "BatchDescribeTypeConfigurations",
        &[
            member("TypeConfigurationIdentifiers.member.1.TypeArn", &activated_arn),
            member("TypeConfigurationIdentifiers.member.2.TypeArn", &unknown),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "expected the batch to succeed with an error entry, got {code} {body}"
    );
    assert_eq!(
        reported(&details(&body), "TypeArn"),
        activated_arn,
        "expected the resolvable identifier to be reported, got {body}"
    );
    let errors = xml_blocks(&body, "Errors").into_iter().next().unwrap_or_default();
    assert_eq!(
        xml_text(&errors, "ErrorCode").as_deref(),
        Some("404"),
        "expected the unresolvable identifier to be reported with the not-found code, got \
         {body}"
    );
    assert_eq!(
        xml_text(&errors, "TypeArn").as_deref(),
        Some(unknown.as_str()),
        "expected the error entry to carry the identifier it came from, got {body}"
    );
}

#[tokio::test]
async fn an_extension_no_configuration_was_set_for_reports_an_empty_configuration() {
    let router = router();
    let case = &extension_cases()[0];
    let (code, body) = activate(&router, case).await;
    assert_eq!(code, 200, "expected ActivateType to succeed, got {code} {body}");
    let activated_arn = xml_text(&body, "Arn").unwrap_or_default();

    let (code, body) = call(
        &router,
        "BatchDescribeTypeConfigurations",
        &[member(
            "TypeConfigurationIdentifiers.member.1.TypeArn",
            &activated_arn,
        )],
    )
    .await;
    assert_eq!(code, 200, "expected the batch to succeed, got {code} {body}");
    let read = details(&body);
    assert_eq!(
        reported(&read, "Configuration"),
        "{}",
        "expected an extension with no configuration to report an empty one, got {read:?}"
    );
    assert_eq!(
        reported(&read, "Alias"),
        "",
        "expected no alias for a configuration that was never set, got {read:?}"
    );
}

#[tokio::test]
async fn a_type_arn_no_extension_answers_to_is_refused_as_not_found() {
    let router = router();
    let unknown = format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:type/hook/Never-Activated");
    let (code, body) = call(
        &router,
        "SetTypeConfiguration",
        &[
            member("TypeArn", &unknown),
            member("Configuration", HOOK_CONFIGURATION),
        ],
    )
    .await;
    assert_eq!(
        code, 404,
        "expected a configuration of an extension the registry does not hold to be refused, \
         got {code} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("TypeNotFoundException"),
        "expected the modelled not-found error, got {body}"
    );
}
