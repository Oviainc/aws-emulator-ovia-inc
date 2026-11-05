// SPDX-License-Identifier: CC0-1.0

//! The role templates AWS services publish, and the parameter substitution a role created
//! from one goes through.
//!
//! One concern, held in one place: the catalog [`PUBLISHED_ROLE_TEMPLATES`], the row types
//! ([`PublishedRoleTemplate`], [`PublishedTemplateParameter`]), the lookups over them
//! ([`published_role_template`], [`published_role_template_version`]), the `@{parameter}`
//! substitution every templated member goes through ([`substitute_parameters`]), the FORM a
//! caller-supplied `TemplateArn` has to take ([`role_template_arn_refusal`]) and the wording
//! of a call addressing a template nothing resolves ([`role_template_not_found`] for
//! `AcquireRole`, [`role_template_version_not_found`] for `GetRoleTemplateVersion`).

#[allow(unused_imports)]
use super::*;

/// The message a call addressing a role template the account holds none of is refused with
/// — the `NoSuchEntityException` message, whose fixed text is `Role template not found: `
/// followed by the ARN the request supplied, with no trailing stop.
///
/// Measured wording: the `<Entity> not found: <identifier>` form the newer IAM API families
/// word an absence with, which is also how the delegation-request family words its own
/// ([`delegation_request_not_found_message`]) and not the `The <entity> with name <name>
/// cannot be found.` form IAM words its user, role and group reads with.
///
/// It names the TEMPLATE and nothing else: the resource such a call addresses is the role
/// template, which its ARN identifies — the minor version is a separate parameter of the
/// request rather than part of what identifies the resource — so a minor version the
/// account holds no such version of is described by this one sentence too.
///
/// This is the sentence `AcquireRole` refuses a template it cannot create from with.
/// `GetRoleTemplateVersion` answers its own, measured sentence
/// ([`role_template_version_not_found`]): both live here, so the wording of an absent role
/// template is stated in ONE module and a reader sees at a glance which operation answers
/// which.
pub fn role_template_not_found(template_arn: &str) -> String {
    format!("Role template not found: {template_arn}")
}

/// The message a READ of a role template version nothing resolves is refused with — the
/// `NoSuchEntityException` message, whose fixed text is `Role Template `, the ARN the request
/// supplied, and ` cannot be found.`
///
/// MEASURED wording, for `GetRoleTemplateVersion`: the `<Entity> … cannot be found.` form IAM
/// words an absent entity with, with the entity named by the ARN that identifies it and
/// nothing else quoted. The ARN comes from the REQUEST, so the fixed text is the sentence and
/// the ARN is the slot.
///
/// It names the TEMPLATE and nothing else, for the reason [`role_template_not_found`] gives,
/// so a minor version the resolved template does not carry is described by this one sentence
/// too.
pub fn role_template_version_not_found(template_arn: &str) -> String {
    format!("Role Template {template_arn} cannot be found.")
}

/// How IAM refuses a role-template ARN whose ACCOUNT field is not the publisher every role
/// template's ARN carries ([`AWS_MANAGED_POLICY_ACCOUNT`]).
///
/// MEASURED wording, entirely fixed: it quotes neither the member nor the value, and states
/// the field and the form the ARN has to take instead.
///
/// A stronger rule than [`TEMPLATE_ARN_WITH_ACCOUNT_NUMBER`], which refuses only an account
/// field of digits and so admits the account-less and other-publisher forms this one refuses.
/// That sentence stays what `CreateDelegationRequest` refuses a POLICY-template ARN with,
/// whose own wording this rule does not speak to.
pub const ROLE_TEMPLATE_ARN_WRONG_ACCOUNT: &str =
    "The specified role template ARN is not valid. The ARN must use 'aws' as the account \
     field (e.g., arn:aws:iam::aws:role-template/...).";

/// What `value` is not, as a role template's ARN — the sentence the reading that refused it
/// is stated with — or `None` when it IS one.
///
/// `arnType` bounds only the length (20..2048), so the FORM of a caller-supplied `TemplateArn`
/// is read here, in ONE place, before any template is resolved: a member documented as "the
/// Amazon Resource Name (ARN) of the role template" has to carry that ARN, and a value that
/// does not is one IAM cannot accept rather than a template that happens to be missing.
///
/// The three readings narrow one parse down in the order they exclude more — it has to be an
/// ARN ([`ARN_UNPARSEABLE`]), then the ARN of a role template in IAM's own namespace
/// ([`ARN_WRONG_RESOURCE_TYPE`]), then one whose account field names the publisher
/// ([`ROLE_TEMPLATE_ARN_WRONG_ACCOUNT`]) — so one value cannot answer two sentences. The six
/// fields come from the crate's ONE caller-supplied-ARN parse (`service_common::arn`).
pub fn role_template_arn_refusal(value: &str) -> Option<&'static str> {
    let Some(arn) = ::service_common::arn::parse(value) else {
        return Some(ARN_UNPARSEABLE);
    };
    if arn.vendor != IAM_ARN_VENDOR || arn.resource_type() != ROLE_TEMPLATE_ARN_RESOURCE {
        return Some(ARN_WRONG_RESOURCE_TYPE);
    }
    (arn.account != AWS_MANAGED_POLICY_ACCOUNT).then_some(ROLE_TEMPLATE_ARN_WRONG_ACCOUNT)
}

/// The ARN vendor field an IAM resource carries — the service namespace that says a value
/// names something of IAM's at all, which a role template's ARN has to spell
/// ([`role_template_arn_refusal`]).
const IAM_ARN_VENDOR: &str = "iam";

/// The `@{` ... `}` placeholder a role template's patterns carry, as
/// `RoleTemplateVersion$RoleNamePattern` states it: "the pattern can include
/// `@{parameter}` placeholders that are replaced with the values you supply in the
/// `ReplacementValues` parameter of AcquireRole".
const PARAMETER_PLACEHOLDER_OPEN: &str = "@{";

/// The character that closes a placeholder opened by [`PARAMETER_PLACEHOLDER_OPEN`].
const PARAMETER_PLACEHOLDER_CLOSE: char = '}';

/// `pattern` with every `@{parameter}` placeholder replaced by the value `resolved` holds
/// for that parameter.
///
/// The ONE substitution every templated member of a role template goes through — the name,
/// the path, the description, the trust policy, each inline policy's name and document and
/// each tag's key and value — so a role created from a template cannot have one member
/// substituted by one rule and another by another.
///
/// A placeholder naming a parameter `resolved` holds nothing for is replaced by NOTHING: a
/// parameter the template defines without a default and the request supplied no value for
/// has no value, so the pattern's slot for it is empty rather than carrying the
/// placeholder's own text. A `@{` that no `}` closes is not a placeholder and is left as
/// the pattern has it.
pub fn substitute_parameters(pattern: &str, resolved: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut rest = pattern;
    while let Some(open) = rest.find(PARAMETER_PLACEHOLDER_OPEN) {
        let after = &rest[open + PARAMETER_PLACEHOLDER_OPEN.len()..];
        let Some(close) = after.find(PARAMETER_PLACEHOLDER_CLOSE) else {
            break;
        };
        out.push_str(&rest[..open]);
        let name = &after[..close];
        if let Some(value) = resolved.get(name) {
            out.push_str(value);
        }
        rest = &after[close + PARAMETER_PLACEHOLDER_CLOSE.len_utf8()..];
    }
    out.push_str(rest);
    out
}

/// The `ManagedByType` a published role template reports: `managedByTypeType` declares
/// the single value `Service` (`smithy_query(query="shape", shape="managedByTypeType")`),
/// "indicates that the role template is managed by an Amazon Web Services service"
/// (`RoleTemplateVersion$ManagedByType`).
pub const ROLE_TEMPLATE_MANAGED_BY_SERVICE: &str = "Service";

/// The resource type a role template's ARN names, between the account segment and the
/// template's own identity: `arn:aws:iam::aws:role-template/<service>/<name>:<major>`.
const ROLE_TEMPLATE_ARN_RESOURCE: &str = "role-template";

/// The instant every entry of the published role-template catalog
/// ([`PUBLISHED_ROLE_TEMPLATES`]) reports as its `CreateTimestamp` and
/// `UpdateTimestamp`, in seconds since the epoch.
///
/// AWS publishes no per-template timestamp, so the catalog carries ONE for itself: the
/// values are of the modelled type (`dateType`) and stable across reads — the same for
/// every reader and every run — rather than derived from a request or moved by a read.
const PUBLISHED_ROLE_TEMPLATE_TIMESTAMP_SECS: i64 = 1_762_128_000;

/// One parameter a published role template version defines
/// ([`PublishedRoleTemplate::parameters`]), as `ParameterDefinition` carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublishedTemplateParameter {
    /// "The name of the parameter" — the name a request's `ReplacementValues` key has to
    /// spell and the name a `@{parameter}` placeholder of this template names.
    pub name: &'static str,
    /// One value of `parameterTypeType`
    /// (`smithy_query(query="shape", shape="parameterTypeType")`).
    pub parameter_type: &'static str,
    /// "The description of the parameter".
    pub description: &'static str,
    /// "Whether you must supply a value for the parameter when you create a role from the
    /// template" (`ParameterDefinition$IsRequired`).
    pub is_required: bool,
    /// "The value that the service uses for the parameter when you do not supply one"
    /// (`ParameterDefinition$DefaultValue`), or `None` where the template publishes none.
    pub default_value: Option<&'static str>,
}

/// One role template a SERVICE publishes, as every account can create roles from it.
///
/// A role template is "managed by an Amazon Web Services service" — `managedByTypeType`
/// declares the single value `Service` — and no operation of this API creates one, so the
/// templates a service publishes are NOT account state: they are the same in every
/// account, exactly as an AWS-managed policy is ([`AwsManagedPolicy`]), which is why they
/// live in this catalog rather than in
/// [`ComAmazonawsIamAwsidentitymanagementv20100508State::role_templates`]. A template an
/// account records itself takes precedence over a catalog entry under the same ARN
/// ([`ComAmazonawsIamAwsidentitymanagementv20100508State::role_template_version`]).
///
/// Each entry carries ONE minor version, the template's default: AWS publishes a template
/// at a major version and revises it with minor versions, and the versions of a template
/// this catalog has not seen are versions nothing here can describe — a minor version the
/// catalog does not carry names no version, exactly as an unknown ARN names no template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublishedRoleTemplate {
    /// "The identifier of the Amazon Web Services service that manages the role template"
    /// (`RoleTemplateVersion$ManagedByValue`): the publishing service's principal, and the
    /// first segment of the template's ARN ([`Self::arn`]).
    pub managed_by_value: &'static str,
    /// "The friendly name that identifies the role template"
    /// (`RoleTemplateVersion$TemplateName`).
    pub template_name: &'static str,
    /// "The major version number of the role template"
    /// (`RoleTemplateVersion$MajorVersion`), which the ARN ends with.
    pub major_version: i32,
    /// "The minor version number of this role template version"
    /// (`RoleTemplateVersion$MinorVersion`), which is also this template's
    /// `DefaultMinorVersion`.
    pub minor_version: i32,
    /// This entry's `TemplateVersionId`, carried as catalog data beside the template's
    /// name and version.
    ///
    /// AWS publishes no version identifier for a role template, so this is a value the
    /// EMULATOR assigns, of the modelled `idType` (len 16..128, `^[\w]+$`). It is fixed
    /// per entry, so every read in every run reports the same identifier for the same
    /// template version.
    pub template_version_id: &'static str,
    /// "The description of the role template" (`RoleTemplateVersion$Description`).
    pub description: &'static str,
    /// "The pattern that is used to generate the name of a role that is created from this
    /// template" (`RoleTemplateVersion$RoleNamePattern`).
    pub role_name_pattern: &'static str,
    /// "The pattern that is used to generate the path of a role that is created from this
    /// template" (`RoleTemplateVersion$RolePathPattern`).
    pub role_path_pattern: &'static str,
    /// "The pattern that is used to generate the description of a role that is created
    /// from this template" (`RoleTemplateVersion$RoleDescriptionPattern`), or `None` where
    /// the template publishes none and the roles it creates carry no description.
    pub role_description_pattern: Option<&'static str>,
    /// "The trust policy template that grants an entity permission to assume roles that
    /// you create from this template"
    /// (`RoleTemplateVersion$AssumeRolePolicyDocumentTemplate`).
    pub assume_role_policy_document_template: &'static str,
    /// "A list of the ARNs of the managed policies that the service attaches to roles that
    /// you create from this template" (`RoleTemplateVersion$ManagedPolicyArns`).
    pub managed_policy_arns: &'static [&'static str],
    /// "The maximum session duration (in seconds) for roles that are created from this
    /// template" (`RoleTemplateVersion$MaxSessionDuration`), inside the published range of
    /// `roleMaxSessionDurationType` (3600..43200).
    pub max_session_duration: i32,
    /// "A list of the parameters that are defined for this role template version"
    /// (`RoleTemplateVersion$ParametersDefinition`).
    pub parameters: &'static [PublishedTemplateParameter],
}

impl PublishedRoleTemplate {
    /// This entry's `TemplateArn`:
    /// `arn:aws:iam::aws:role-template/<managing service>/<name>:<major version>`.
    ///
    /// Built here rather than through `engine::emulator_arn` because that helper always
    /// writes the emulator's own account id into the account segment, and a published role
    /// template is owned by AWS: its account segment is the literal
    /// [`AWS_MANAGED_POLICY_ACCOUNT`], the same segment every AWS-owned IAM resource
    /// carries.
    pub fn arn(&self) -> String {
        format!(
            "arn:aws:iam::{AWS_MANAGED_POLICY_ACCOUNT}:{ROLE_TEMPLATE_ARN_RESOURCE}/{}/{}:{}",
            self.managed_by_value, self.template_name, self.major_version
        )
    }

    /// This entry as the role template version every reader of a template reports and
    /// `AcquireRole` creates a role from.
    ///
    /// Enabled, both as a template and at this minor version: a published template a
    /// service offers is one roles can be created from, and a disabled one is what
    /// `RoleTemplateDisabledException` refuses — a state a catalog entry is never in,
    /// since AWS publishes no disabled template.
    pub fn version(&self) -> RoleTemplateVersion {
        let stamped =
            ::aws_smithy_types::DateTime::from_secs(PUBLISHED_ROLE_TEMPLATE_TIMESTAMP_SECS);
        RoleTemplateVersion {
            template_arn: Some(self.arn()),
            template_name: Some(self.template_name.to_string()),
            template_version_id: Some(self.template_version_id.to_string()),
            description: Some(self.description.to_string()),
            major_version: Some(self.major_version),
            default_minor_version: Some(self.minor_version),
            managed_by_type: Some(ROLE_TEMPLATE_MANAGED_BY_SERVICE.to_string()),
            managed_by_value: Some(self.managed_by_value.to_string()),
            enabled: true,
            minor_version: Some(self.minor_version),
            role_name_pattern: Some(self.role_name_pattern.to_string()),
            role_path_pattern: Some(self.role_path_pattern.to_string()),
            role_description_pattern: self.role_description_pattern.map(str::to_string),
            assume_role_policy_document_template: Some(
                self.assume_role_policy_document_template.to_string(),
            ),
            // A published service role takes its permissions from the managed policies the
            // service maintains for it, so no catalog entry embeds an inline policy or
            // sets a permissions boundary: a template that carries neither reports neither.
            inline_policy_templates: None,
            managed_policy_arns: Some(
                self.managed_policy_arns
                    .iter()
                    .map(|arn| (*arn).to_string())
                    .collect(),
            ),
            permission_boundary_arn: None,
            parameters_definition: Some(
                self.parameters
                    .iter()
                    .map(|parameter| ParameterDefinition {
                        name: parameter.name.to_string(),
                        r#type: parameter.parameter_type.to_string(),
                        // `ParameterDefinition$SubType` is the element type of a LIST
                        // parameter, which no catalog entry defines.
                        sub_type: None,
                        description: Some(parameter.description.to_string()),
                        is_required: parameter.is_required,
                        default_value: parameter.default_value.map(str::to_string),
                        // "Specifies whether you can change the parameter value after you
                        // create the role" (`ParameterDefinition$Immutable`): a published
                        // template renders the role's name, path and trust policy from its
                        // parameters, and no operation of this API re-renders a role from
                        // the template it came from, so none of those values can change.
                        immutable: true,
                    })
                    .collect(),
            ),
            // No catalog entry tags the roles it creates: a tag is a record the account
            // keeps on its own resource.
            role_tags_template: None,
            max_session_duration: Some(self.max_session_duration),
            version_enabled: true,
            create_timestamp: Some(stamped),
            update_timestamp: Some(stamped),
        }
    }
}

/// The role templates AWS services publish, ordered by ARN.
///
/// SOURCE: the role each publishing service documents for the purpose the template serves —
/// its path, the principal its trust policy admits, the managed policies the service
/// attaches to it and the description it carries. For `backup.amazonaws.com` that is the
/// default service role AWS Backup's own user guide describes, filed under the path AWS
/// reserves for service roles and carrying the service's backup and restore policies, with
/// the trust policy's source-account condition rendered for the account the service acts
/// for. For `iam.amazonaws.com` it is the power-user role: `PowerUserAccess` at the account
/// root, trusted by the service principal the request names.
///
/// A service publishes a template so that every account can create that role without
/// writing its trust policy by hand, so the entries here are the templates a client can
/// name in `AcquireRole` without anything in the account having recorded one. Every value a
/// created role carries that is NOT catalog data — the rendered name, the rendered trust
/// policy, the role id, the creation instant and the ARN — is computed per call from the
/// request and the account's state.
///
/// An `arn:aws:iam::aws:role-template/` ARN this catalog does not carry resolves to no
/// template, and a call addressing one is refused as the missing entity
/// `NoSuchEntityException` names ([`role_template_not_found`]).
pub const PUBLISHED_ROLE_TEMPLATES: &[PublishedRoleTemplate] = &[
    PublishedRoleTemplate {
        managed_by_value: "backup.amazonaws.com",
        template_name: "AWSBackupDefaultServiceRoleTemplate",
        major_version: 1,
        minor_version: 0,
        template_version_id: "roletemplateversion_backup_default_service_role_1_0",
        description: "Standard service role the provides AWS Backup service permission to create and restore backups on your behalf across AWS services",
        role_name_pattern: "@{RoleName}",
        // AWS Backup's default service role is one AWS files under the path it reserves for
        // service roles, which is the path the role a client creates from this template
        // carries.
        role_path_pattern: "/service-role/",
        role_description_pattern: Some("Standard service role the provides AWS Backup service permission to create and restore backups on your behalf across AWS services"),
        assume_role_policy_document_template: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"backup.amazonaws.com"},"Action":"sts:AssumeRole","Condition":{"StringEquals":{"aws:SourceAccount":"@{accountId}"}}}]}"#,
        managed_policy_arns: &[
            "arn:aws:iam::aws:policy/service-role/AWSBackupServiceRolePolicyForBackup",
            "arn:aws:iam::aws:policy/service-role/AWSBackupServiceRolePolicyForRestores",
        ],
        max_session_duration: DEFAULT_MAX_SESSION_DURATION_SECONDS,
        parameters: &[
            PublishedTemplateParameter {
                name: "RoleName",
                parameter_type: "String",
                description: "The name of the role to create from this template",
                is_required: true,
                default_value: None,
            },
            PublishedTemplateParameter {
                name: "accountId",
                parameter_type: "String",
                description: "The account that the service acts on behalf of when it assumes the role",
                is_required: true,
                default_value: None,
            },
        ],
    },
    PublishedRoleTemplate {
        managed_by_value: "iam.amazonaws.com",
        template_name: "PowerUserRoleTemplate",
        major_version: 1,
        minor_version: 0,
        template_version_id: "roletemplateversion_iam_power_user_1_0",
        description: "Power user role for the service principal that you specify",
        role_name_pattern: "@{RoleName}",
        // A power-user role is a role of the account's own rather than a service role, so it
        // is created where every entity whose create names no path goes: the account root.
        role_path_pattern: "/",
        role_description_pattern: Some("Power user role for @{AWSServiceName}"),
        assume_role_policy_document_template: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"@{AWSServiceName}"},"Action":"sts:AssumeRole"}]}"#,
        managed_policy_arns: &["arn:aws:iam::aws:policy/PowerUserAccess"],
        max_session_duration: DEFAULT_MAX_SESSION_DURATION_SECONDS,
        parameters: &[
            PublishedTemplateParameter {
                name: "AWSServiceName",
                parameter_type: "String",
                description: "The service principal that may assume the role",
                is_required: true,
                default_value: None,
            },
            PublishedTemplateParameter {
                name: "RoleName",
                parameter_type: "String",
                description: "The name of the role to create from this template",
                is_required: true,
                default_value: None,
            },
        ],
    },
];

/// The published role template `template_arn` names, or `None` where
/// [`PUBLISHED_ROLE_TEMPLATES`] carries none under that ARN.
///
/// The ARN is matched VERBATIM: it is the handle the publishing service minted rather than
/// an IAM entity name, so there is no case to fold.
pub fn published_role_template(template_arn: &str) -> Option<&'static PublishedRoleTemplate> {
    PUBLISHED_ROLE_TEMPLATES
        .iter()
        .find(|template| template.arn() == template_arn)
}

/// The minor version of the published role template `template_arn` that a request naming
/// `minor_version` addresses, or `None` where the catalog carries no such template or no
/// such version of it.
///
/// A request that names no minor version addresses the template's default — "if you do not
/// specify a minor version, the service uses the template's default minor version"
/// (`AcquireRole$TemplateMinorVersion`) — which for a catalog entry is the one version it
/// carries ([`PublishedRoleTemplate`]).
pub fn published_role_template_version(
    template_arn: &str,
    minor_version: Option<i32>,
) -> Option<RoleTemplateVersion> {
    let template = published_role_template(template_arn)?;
    match minor_version {
        Some(named) if named != template.minor_version => None,
        _ => Some(template.version()),
    }
}
