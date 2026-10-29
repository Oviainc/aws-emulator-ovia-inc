// SPDX-License-Identifier: CC0-1.0

//! Whether the ARN a request names a PUBLIC extension by may pin a VERSION of it, end to end
//! over the awsQuery wire.
//!
//! RULE under test: **a public-extension ARN addresses the extension itself — its kind, its
//! publisher and its name, `type/<kind>/<publisher>/<Type-Name>` — so one carrying a further
//! segment pins a public version number the ARN cannot carry, and CloudFormation refuses it
//! with the registry's own modelled `CFNRegistryException` (HTTP 400) carrying "You cannot
//! specify public version number in the arn". The refusal is read off the ARN's shape alone,
//! so it is answered whatever else the request names and before every other rule of the
//! activation — before the extension the ARN names is looked up, and before the rule that a
//! public RESOURCE activation must name an `ExecutionRoleArn` — and the activation writes
//! nothing.**
//!
//! The errors the operation models are `CFNRegistryException` (400) and
//! `TypeNotFoundException` (404), and the ARN's own shape is the modelled `ThirdPartyTypeArn`
//! pattern `^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}::type/.+/[0-9a-zA-Z]{12,40}/.+$`
//! (`smithy_query(query="operation", operation="ActivateType")`), which admits a pinned
//! version in its trailing `.+` — so this refusal is the registry's own and not a constraint
//! violation.
//!
//! The rows differ along what the rule generalises over: the spelling of the pinned version
//! (a zero-padded version id, as the modelled `TypeVersionId` pattern `^[A-Za-z0-9-]+$`
//! admits; each bound of the modelled `PublicVersionNumber` pattern
//! `^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(.*)$`, `smithy_query(query="shape",
//! shape="PublicVersionNumber")`; and a segment in that position that is no version number at
//! all), each kind the modelled `ThirdPartyType` admits, with and without an
//! `ExecutionRoleArn`, with and without `Type` spelled out beside the ARN, with and without a
//! `TypeNameAlias`, and over a name segment that is no type name either. The version-free
//! rows are the controls the fix must leave alone — including an ARN whose trailing `/` is
//! followed by nothing, which pins no version and still activates.
//!
//! `public_extension_arns.rs` is the suite for which name an ARN resolves an extension by;
//! this one is whether it may pin a version of it.

use crate::stack_lifecycle::{call, member, router, xml_text, ACCOUNT_ID, REGION};
use service_com_amazonaws_cloudformation_cloudformation::state::registry;

/// A publisher id inside the modelled `PublisherId` pattern `^[0-9a-zA-Z]{12,40}$`, which is
/// also the segment the `ThirdPartyTypeArn` pattern expects in an ARN's third position.
const PUBLISHER_ID: &str = "abcdef0123456789abcd";

/// The execution role an activation of a public RESOURCE extension must name, inside the
/// modelled `RoleARN2` pattern `^arn:.+:iam::[0-9]{12}:role/.+$`.
const EXECUTION_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/extension-execution";

/// The refusal a public RESOURCE activation naming no execution role gets — the rule the
/// version refusal is ordered AHEAD of, and the control that shows it still fires for an ARN
/// pinning no version.
const NO_EXECUTION_ROLE: &str = "Please provide execution role to activate the Resource type";

/// What must come back for one row.
#[derive(Debug)]
enum Expected {
    /// HTTP 400 `CFNRegistryException` carrying exactly this message.
    Refused(&'static str),
    /// HTTP 200, with the activation held under this local name.
    Activated(&'static str),
}

/// One row: the ARN a request names the public extension by, what else the request names, and
/// what must come back.
#[derive(Debug)]
struct VersionCase {
    case: &'static str,
    kind: &'static str,
    /// The ARN's name segment, in the dashed spelling an ARN carries.
    name_segment: &'static str,
    /// The segment the ARN pins PAST that name, where the row carries one — `Some("")` for
    /// an ARN whose trailing `/` is followed by nothing, which pins no version.
    pinned: Option<&'static str>,
    /// Whether the request also names an execution role.
    execution_role: bool,
    /// Whether the request also spells `Type` out beside the ARN.
    spells_type: bool,
    /// The `TypeNameAlias` the request assigns, where it assigns one.
    alias: Option<&'static str>,
    expected: Expected,
}

fn version_cases() -> Vec<VersionCase> {
    vec![
        VersionCase {
            case: "a resource ARN pinning a zero-padded version id, naming no execution role",
            kind: registry::registry_type::RESOURCE,
            name_segment: "Example-Store-Bucket",
            pinned: Some("00000001"),
            execution_role: false,
            spells_type: false,
            alias: None,
            expected: Expected::Refused(registry::PUBLIC_VERSION_NUMBER_IN_ARN),
        },
        VersionCase {
            case: "the same ARN with an execution role named: the refusal is ordered ahead of \
                   the execution-role rule, so supplying one activates nothing",
            kind: registry::registry_type::RESOURCE,
            name_segment: "Example-Store-Bucket",
            pinned: Some("00000001"),
            execution_role: true,
            spells_type: false,
            alias: None,
            expected: Expected::Refused(registry::PUBLIC_VERSION_NUMBER_IN_ARN),
        },
        VersionCase {
            case: "the same ARN with Type spelled out beside it, agreeing with the ARN",
            kind: registry::registry_type::RESOURCE,
            name_segment: "Example-Store-Bucket",
            pinned: Some("00000001"),
            execution_role: true,
            spells_type: true,
            alias: None,
            expected: Expected::Refused(registry::PUBLIC_VERSION_NUMBER_IN_ARN),
        },
        VersionCase {
            case: "the same ARN under an alias: the refusal is ordered ahead of what the \
                   activation would be held under",
            kind: registry::registry_type::RESOURCE,
            name_segment: "Example-Store-Bucket",
            pinned: Some("00000001"),
            execution_role: true,
            spells_type: false,
            alias: Some("Local::Store::Bucket"),
            expected: Expected::Refused(registry::PUBLIC_VERSION_NUMBER_IN_ARN),
        },
        VersionCase {
            case: "a module ARN pinning the first version number CloudFormation assigns",
            kind: registry::registry_type::MODULE,
            name_segment: "Example-Network-Subnets-MODULE",
            pinned: Some("1.0.0"),
            execution_role: false,
            spells_type: false,
            alias: None,
            expected: Expected::Refused(registry::PUBLIC_VERSION_NUMBER_IN_ARN),
        },
        VersionCase {
            case: "a Hook ARN pinning a version number past its major and minor bounds",
            kind: registry::registry_type::HOOK,
            name_segment: "Example-Guard-Hook",
            pinned: Some("12.345.6"),
            execution_role: false,
            spells_type: true,
            alias: None,
            expected: Expected::Refused(registry::PUBLIC_VERSION_NUMBER_IN_ARN),
        },
        VersionCase {
            case: "a resource ARN pinning the lowest version number the modelled pattern \
                   admits",
            kind: registry::registry_type::RESOURCE,
            name_segment: "Example-Store-Bucket",
            pinned: Some("0.0.0"),
            execution_role: true,
            spells_type: false,
            alias: None,
            expected: Expected::Refused(registry::PUBLIC_VERSION_NUMBER_IN_ARN),
        },
        VersionCase {
            case: "a fourth segment that is no version number at all: the position is the \
                   version's, so the ARN pins one either way",
            kind: registry::registry_type::RESOURCE,
            name_segment: "Example-Store-Bucket",
            pinned: Some("latest"),
            execution_role: true,
            spells_type: false,
            alias: None,
            expected: Expected::Refused(registry::PUBLIC_VERSION_NUMBER_IN_ARN),
        },
        VersionCase {
            case: "a pinned version past a name segment that is no type name either: the \
                   ARN's shape is read before the name it carries is resolved",
            kind: registry::registry_type::RESOURCE,
            name_segment: "00000001",
            pinned: Some("1.2.3"),
            execution_role: true,
            spells_type: false,
            alias: None,
            expected: Expected::Refused(registry::PUBLIC_VERSION_NUMBER_IN_ARN),
        },
        // The controls: an ARN pinning NO version keeps answering exactly what it answered
        // before, for every kind.
        VersionCase {
            case: "control — a resource ARN pinning no version, naming no execution role",
            kind: registry::registry_type::RESOURCE,
            name_segment: "Example-Store-Bucket",
            pinned: None,
            execution_role: false,
            spells_type: false,
            alias: None,
            expected: Expected::Refused(NO_EXECUTION_ROLE),
        },
        VersionCase {
            case: "control — a resource ARN pinning no version, with an execution role",
            kind: registry::registry_type::RESOURCE,
            name_segment: "Example-Store-Bucket",
            pinned: None,
            execution_role: true,
            spells_type: true,
            alias: None,
            expected: Expected::Activated("Example::Store::Bucket"),
        },
        VersionCase {
            case: "control — the same under an alias, which the activation is held by",
            kind: registry::registry_type::RESOURCE,
            name_segment: "Example-Store-Bucket",
            pinned: None,
            execution_role: true,
            spells_type: false,
            alias: Some("Local::Store::Bucket"),
            expected: Expected::Activated("Local::Store::Bucket"),
        },
        VersionCase {
            case: "control — a trailing slash followed by nothing, which pins no version",
            kind: registry::registry_type::RESOURCE,
            name_segment: "Example-Store-Bucket",
            pinned: Some(""),
            execution_role: true,
            spells_type: false,
            alias: None,
            expected: Expected::Activated("Example::Store::Bucket"),
        },
        VersionCase {
            case: "control — a module ARN pinning no version, which needs no execution role",
            kind: registry::registry_type::MODULE,
            name_segment: "Example-Network-Subnets-MODULE",
            pinned: None,
            execution_role: false,
            spells_type: false,
            alias: None,
            expected: Expected::Activated("Example::Network::Subnets::MODULE"),
        },
        VersionCase {
            case: "control — a Hook ARN pinning no version",
            kind: registry::registry_type::HOOK,
            name_segment: "Example-Guard-Hook",
            pinned: None,
            execution_role: false,
            spells_type: true,
            alias: None,
            expected: Expected::Activated("Example::Guard::Hook"),
        },
    ]
}

impl VersionCase {
    /// The ARN this row names the public extension by.
    fn public_type_arn(&self) -> String {
        let mut arn = format!(
            "arn:aws:cloudformation:{REGION}::type/{}/{PUBLISHER_ID}/{}",
            self.kind.to_ascii_lowercase(),
            self.name_segment
        );
        if let Some(pinned) = self.pinned {
            arn.push('/');
            arn.push_str(pinned);
        }
        arn
    }

    /// The members the request carries beside the ARN.
    fn members(&self) -> Vec<(String, String)> {
        let mut members = vec![member("PublicTypeArn", &self.public_type_arn())];
        if self.execution_role {
            members.push(member("ExecutionRoleArn", EXECUTION_ROLE_ARN));
        }
        if self.spells_type {
            members.push(member("Type", self.kind));
        }
        if let Some(alias) = self.alias {
            members.push(member("TypeNameAlias", alias));
        }
        members
    }

    /// The local name an accepted activation of this row would be held under: the alias where
    /// the row assigns one, else the name the ARN carries.
    fn local_name(&self) -> String {
        self.alias
            .map(str::to_string)
            .unwrap_or_else(|| self.name_segment.replace('-', "::"))
    }
}

/// The ARN this account addresses an activation by.
fn activated_arn(kind: &str, local_name: &str) -> String {
    format!(
        "arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:type/{}/{}",
        kind.to_ascii_lowercase(),
        local_name.replace("::", "-")
    )
}

#[tokio::test]
async fn an_arn_pinning_a_public_version_is_the_registrys_own_refusal() {
    for row in version_cases() {
        // A fresh account per row, so each activation is the first one its registry saw.
        let router = router();
        let (status_code, body) = call(&router, "ActivateType", &row.members()).await;
        let local_arn = activated_arn(row.kind, &row.local_name());

        match row.expected {
            Expected::Refused(message) => {
                assert_eq!(
                    status_code, 400,
                    "case {:?}: expected the registry to refuse the ARN, got {status_code} \
                     {body}",
                    row.case
                );
                assert_eq!(
                    (
                        xml_text(&body, "Code").unwrap_or_default(),
                        xml_text(&body, "Message").unwrap_or_default()
                    ),
                    ("CFNRegistryException".to_string(), message.to_string()),
                    "case {:?}: expected the registry's own modelled refusal, got {body}",
                    row.case
                );
                // The refusal wrote nothing: no activation is readable in this account under
                // the name the request would have held one by.
                let (status_code, described) =
                    call(&router, "DescribeType", &[member("Arn", &local_arn)]).await;
                assert_eq!(
                    status_code, 404,
                    "case {:?}: expected the refused activation to write nothing, got \
                     {status_code} {described}",
                    row.case
                );
            }
            Expected::Activated(local_name) => {
                assert_eq!(
                    status_code, 200,
                    "case {:?}: expected the activation to succeed, got {status_code} {body}",
                    row.case
                );
                assert_eq!(
                    xml_text(&body, "Arn").as_deref(),
                    Some(local_arn.as_str()),
                    "case {:?}: expected the activation's own ARN, got {body}",
                    row.case
                );
                // Round trip: the activation is readable at the ARN it answered, under the
                // local name it is held by.
                let (status_code, described) =
                    call(&router, "DescribeType", &[member("Arn", &local_arn)]).await;
                assert_eq!(
                    status_code, 200,
                    "case {:?}: expected the activation readable, got {status_code} \
                     {described}",
                    row.case
                );
                assert_eq!(
                    xml_text(&described, "TypeName").as_deref(),
                    Some(local_name),
                    "case {:?}: expected it held under its local name, got {described}",
                    row.case
                );
            }
        }
    }
}
