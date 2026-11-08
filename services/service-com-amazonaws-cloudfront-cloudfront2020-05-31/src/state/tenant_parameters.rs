// SPDX-License-Identifier: CC0-1.0

//! The tenant PARAMETER references a distribution configuration carries, and the rule they
//! are admitted by.
//!
//! A multi-tenant distribution is a template: the members of its configuration that address
//! the origin may name one of the distribution's own parameters, and CloudFront substitutes
//! the value each tenant supplies for it when it serves that tenant. The parameters a
//! configuration may name are exactly the ones its `TenantConfig$ParameterDefinitions`
//! declares — "a valid parameter value must exist for any parameter that is marked as
//! required in the multi-tenant distribution", and a tenant supplies values only for the
//! parameters that list defines ([`parameter_refusal`] is the same rule seen from the
//! tenant's side). A configuration declaring the STANDARD connection mode serves its own
//! traffic rather than a tenant's, so nothing is ever substituted into it and nothing in it
//! may reference a parameter.
//!
//! So a configuration carrying a reference no definition declares describes an origin
//! CloudFront could never resolve, and is refused before it is stored — on every path that
//! stores a `DistributionConfig`, so no stored configuration carries an unresolvable
//! reference.

use super::*;

/// What opens a tenant parameter reference inside a configuration member.
const REFERENCE_OPEN: &str = "{{";

/// What closes one.
const REFERENCE_CLOSE: &str = "}}";

/// The configuration members a tenant parameter reference may appear in, each named as the
/// service states it in the refusal below.
///
/// Both are members of `Origin`, the part of the configuration that says WHERE the content
/// comes from: `DomainName` is "the domain name for the origin" and `OriginPath` "an
/// optional path that CloudFront appends to the origin domain name when CloudFront requests
/// content from the origin" — the two values a tenant's own content is addressed by, and so
/// the two a multi-tenant distribution parameterises. The name each is stated by is the
/// model's member name with its first letter in lower case, which is how the service words
/// it on the wire.
const DOMAIN_NAME_MEMBER: &str = "domainName";

/// `Origin$OriginPath`, as [`DOMAIN_NAME_MEMBER`] explains.
const ORIGIN_PATH_MEMBER: &str = "originPath";

/// Every tenant parameter reference `value` carries, in the order it states them.
///
/// A reference is a parameter name between [`REFERENCE_OPEN`] and [`REFERENCE_CLOSE`]; text
/// that opens a reference and never closes it names nothing, so it carries none. Nothing is
/// indexed or sliced by a computed offset: the walk is `split_once` over the remainder.
pub fn parameter_references(value: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = value;
    while let Some((_, after_open)) = rest.split_once(REFERENCE_OPEN) {
        match after_open.split_once(REFERENCE_CLOSE) {
            Some((name, after_close)) => {
                found.push(name);
                rest = after_close;
            }
            None => break,
        }
    }
    found
}

/// The parameter names a configuration's `TenantConfig$ParameterDefinitions` declares.
fn declared_parameters(config: &DistributionConfig) -> BTreeSet<&str> {
    config
        .tenant_config
        .as_ref()
        .and_then(|tenant_config| tenant_config.parameter_definitions.as_deref())
        .unwrap_or_default()
        .iter()
        .map(|definition| definition.name.as_str())
        .collect()
}

/// Why a submitted `DistributionConfig` references a tenant parameter it cannot resolve, or
/// `None` when every reference it carries names a parameter the configuration declares.
///
/// One rule, read off the connection mode the configuration DECLARES
/// ([`connection_mode_declared`]) — which is what says whether the configuration is a
/// template at all:
///
/// * a configuration declaring `direct` describes a standard distribution, which serves its
///   own traffic rather than a tenant's and has no parameter values substituted into it, so
///   any reference in one of its templated members is invalid and the refusal names that
///   member ([`unresolvable_parameter_member_message`]) — whatever definitions the
///   configuration may also carry, since none of them is ever applied to it;
/// * a configuration declaring `tenant-only` may reference the parameters its
///   `TenantConfig$ParameterDefinitions` declares and no others, so a reference to a name
///   that list does not carry — including a configuration that declares no definitions at
///   all — is refused with the sentence that states which list the name is missing from
///   ([`PARAMETER_NOT_DECLARED_MESSAGE`]).
///
/// The members walked are the two templated ones ([`DOMAIN_NAME_MEMBER`],
/// [`ORIGIN_PATH_MEMBER`]), read in the order the configuration states its origins, so the
/// refusal names the FIRST member that breaks the rule. Checked on the way IN, on every path
/// that stores a `DistributionConfig`.
pub fn tenant_parameter_refusal(config: &DistributionConfig) -> Option<String> {
    let multi_tenant = connection_mode_declared(config) == CONNECTION_MODE_TENANT_ONLY;
    let declared = declared_parameters(config);
    for origin in &config.origins.items {
        let members = [
            (DOMAIN_NAME_MEMBER, origin.domain_name.as_str()),
            (
                ORIGIN_PATH_MEMBER,
                origin.origin_path.as_deref().unwrap_or_default(),
            ),
        ];
        for (member, value) in members {
            for name in parameter_references(value) {
                if !multi_tenant {
                    return Some(unresolvable_parameter_member_message(member));
                }
                if !declared.contains(name) {
                    return Some(PARAMETER_NOT_DECLARED_MESSAGE.to_string());
                }
            }
        }
    }
    None
}

/// The sentence CloudFront refuses a configuration whose member `member` references a
/// parameter that configuration cannot resolve with, on the wire.
///
/// The member is the one variable part of the text — it is what the caller has to change —
/// and the value that carried the reference is never echoed back.
pub fn unresolvable_parameter_member_message(member: &str) -> String {
    format!("The parameter {member} contains one or more parameters that are not valid.")
}

/// The sentence CloudFront refuses a MULTI-TENANT configuration that references a parameter
/// its `ParameterDefinitions` does not declare with, on the wire.
///
/// FIXED text: what the refusal states is the rule the configuration broke — a parameter it
/// references has to be declared — which is a property of the configuration as a whole, so
/// no name from the request belongs in it.
pub const PARAMETER_NOT_DECLARED_MESSAGE: &str =
    "A parameter in the DistributionConfig doesn't exist in the ParameterDefinitions list. \
     Remove the parameter and try again.";
