// SPDX-License-Identifier: CC0-1.0

//! The attributes of the ACCOUNT itself — what `DescribeAccountAttributes` reports.
//!
//! A submodule of [`crate::state`] rather than more lines in `state.rs`, for the reason
//! `serial_console_access.rs` gives: one subject in one file, and out of the end of
//! `state.rs`, where every concurrently landing EC2 fix inserts.
//!
//! An account attribute is a FACT OF THE ACCOUNT, not a resource any EC2 operation
//! creates: nothing in this API makes, renames or deletes one, so the attributes are
//! DERIVED here the way [`availability_zones_in_region`] derives a Region's zones, and
//! `DescribeAccountAttributes` stores nothing of its own. Each attribute's value comes from
//! the one place that already owns it:
//!
//! * `default-vpc` is the id of the default VPC the account holds IN THE REGION the request
//!   was signed for — [`ComAmazonawsEc2Amazonec2State::default_vpc_id`], the same index
//!   `DescribeVpcs` and `CreateDefaultVpc` read — or [`NO_DEFAULT_VPC`] when it has none,
//!   so the attribute and a describe of that VPC can never disagree about its id.
//! * `supported-platforms` is the network platform an instance of this account launches
//!   into: [`SUPPORTED_PLATFORM_VPC`] alone, EC2-Classic being retired.
//! * a numeric attribute reports the limit THIS service enforces where it enforces one
//!   ([`MAX_SECURITY_GROUPS_PER_INSTANCE`] for the groups an interface may carry), and the
//!   default AWS publishes for the account where it does not — so no value reported here
//!   contradicts what a request is actually refused for.
//!
//! The attribute NAMES are the ones the operation's own doc lists as "the supported account
//! attributes" (`smithy_query(query="doc",shape="DescribeAccountAttributes")`), which is a
//! longer list than the model's stale `AccountAttributeName` enum
//! (`default-vpc | supported-platforms`); the response member is a plain `String`
//! (`AccountAttribute$AttributeName`), so every documented attribute is reportable.

use super::*;
use crate::ops::types::{AccountAttribute, AccountAttributeValue};

/// `default-vpc` — "The ID of the default VPC for your account, or none"
/// (`DescribeAccountAttributes`).
pub const ACCOUNT_ATTRIBUTE_DEFAULT_VPC: &str = "default-vpc";

/// `max-instances` — "This attribute is no longer supported. The returned value does not
/// reflect your actual vCPU limit for running On-Demand Instances" (same doc).
pub const ACCOUNT_ATTRIBUTE_MAX_INSTANCES: &str = "max-instances";

/// `max-elastic-ips` — "The maximum number of Elastic IP addresses that you can allocate"
/// (same doc).
pub const ACCOUNT_ATTRIBUTE_MAX_ELASTIC_IPS: &str = "max-elastic-ips";

/// `supported-platforms` — "This attribute is deprecated" (same doc); it reports the network
/// platforms the account can launch into.
pub const ACCOUNT_ATTRIBUTE_SUPPORTED_PLATFORMS: &str = "supported-platforms";

/// `vpc-max-elastic-ips` — "The maximum number of Elastic IP addresses that you can
/// allocate" (same doc), for the account's VPCs.
pub const ACCOUNT_ATTRIBUTE_VPC_MAX_ELASTIC_IPS: &str = "vpc-max-elastic-ips";

/// `vpc-max-security-groups-per-interface` — "The maximum number of security groups that you
/// can assign to a network interface" (same doc).
pub const ACCOUNT_ATTRIBUTE_VPC_MAX_SECURITY_GROUPS_PER_INTERFACE: &str =
    "vpc-max-security-groups-per-interface";

/// Every account attribute this service reports, in the order the operation's doc lists
/// them.
///
/// The ONE catalog: it is both what a request's `AttributeNames` is checked against and the
/// order a response reports in ("The order of the elements in the response … might vary.
/// Applications should not assume the elements appear in a particular order"), so a request
/// that names none and one that names them all are answered from the same list.
pub const ACCOUNT_ATTRIBUTE_NAMES: [&str; 6] = [
    ACCOUNT_ATTRIBUTE_DEFAULT_VPC,
    ACCOUNT_ATTRIBUTE_MAX_INSTANCES,
    ACCOUNT_ATTRIBUTE_MAX_ELASTIC_IPS,
    ACCOUNT_ATTRIBUTE_SUPPORTED_PLATFORMS,
    ACCOUNT_ATTRIBUTE_VPC_MAX_ELASTIC_IPS,
    ACCOUNT_ATTRIBUTE_VPC_MAX_SECURITY_GROUPS_PER_INTERFACE,
];

/// What `default-vpc` reports for an account that has no default VPC in the Region asked in:
/// "The ID of the default VPC for your account, or none" — the word itself, not an absent
/// value, because the attribute always carries one value.
pub const NO_DEFAULT_VPC: &str = "none";

/// The one network platform this account launches into: a VPC.
///
/// `supported-platforms` reports `EC2` for an account that can still launch outside a VPC;
/// EC2-Classic is retired and nothing in this emulator launches an instance outside a VPC
/// (every launch resolves a subnet of one), so `VPC` is the whole of the account's platform
/// set.
pub const SUPPORTED_PLATFORM_VPC: &str = "VPC";

/// What `max-instances` reports: the legacy On-Demand Instance limit the attribute has
/// carried since it stopped meaning anything.
///
/// Its own doc says the value "does not reflect your actual vCPU limit for running On-Demand
/// Instances", so it is NOT read off anything this emulator enforces — a value derived from
/// the instance store would claim exactly the meaning AWS says the attribute has lost.
pub const LEGACY_MAX_INSTANCES: u32 = 20;

/// What `max-elastic-ips` and `vpc-max-elastic-ips` report: the Elastic IP addresses an
/// account may allocate per Region, as AWS publishes the account's default for that quota.
///
/// This service enforces no Elastic IP quota of its own — `AllocateAddress`'s only ceilings
/// are the emulator's storage bounds, which are not quotas — so the attribute reports the
/// published default rather than a storage bound a caller cannot read as a quota.
pub const ELASTIC_IPS_PER_REGION_QUOTA: u32 = 5;

impl ComAmazonawsEc2Amazonec2State {
    /// The values this account holds for the attribute `name`, asked in `region` — or `None`
    /// when `name` is no account attribute at all.
    ///
    /// A pure READ: every value is derived from what the store already holds (the Region's
    /// default VPC) or from a fact of the account, so two calls report the same attributes
    /// and reading one changes nothing a later caller sees.
    pub fn account_attribute_values(&self, name: &str, region: &str) -> Option<Vec<String>> {
        let values = match name {
            ACCOUNT_ATTRIBUTE_DEFAULT_VPC => vec![self
                .default_vpc_id(region)
                .unwrap_or_else(|| NO_DEFAULT_VPC.to_string())],
            ACCOUNT_ATTRIBUTE_MAX_INSTANCES => vec![LEGACY_MAX_INSTANCES.to_string()],
            ACCOUNT_ATTRIBUTE_MAX_ELASTIC_IPS | ACCOUNT_ATTRIBUTE_VPC_MAX_ELASTIC_IPS => {
                vec![ELASTIC_IPS_PER_REGION_QUOTA.to_string()]
            }
            ACCOUNT_ATTRIBUTE_SUPPORTED_PLATFORMS => vec![SUPPORTED_PLATFORM_VPC.to_string()],
            ACCOUNT_ATTRIBUTE_VPC_MAX_SECURITY_GROUPS_PER_INTERFACE => {
                vec![MAX_SECURITY_GROUPS_PER_INSTANCE.to_string()]
            }
            _ => return None,
        };
        Some(values)
    }

    /// One account attribute as the modelled shape a report carries — "Describes an account
    /// attribute" — or `None` when `name` is no account attribute at all.
    ///
    /// The projection every report of an attribute reads, so the name a response carries is
    /// the catalog's own spelling and the values are [`Self::account_attribute_values`]'s.
    pub fn account_attribute(&self, name: &str, region: &str) -> Option<AccountAttribute> {
        let values = self.account_attribute_values(name, region)?;
        Some(AccountAttribute {
            attribute_name: Some(name.to_string()),
            attribute_values: Some(
                values
                    .into_iter()
                    .map(|value| AccountAttributeValue {
                        attribute_value: Some(value),
                    })
                    .collect(),
            ),
        })
    }
}

/// How EC2 refuses an `AttributeName` that names no account attribute: HTTP 400
/// [`INVALID_PARAMETER`], the recorded sentence `The property '<value>' is not valid` —
/// fixed text with the value the REQUEST sent quoted inside it, and nothing of the catalog,
/// which a caller reads from the API reference.
///
/// The value is echoed through [`echoed_resource_id`], the one place a refused value the
/// caller chose is cut down to length, so naming it cannot amplify a request into an
/// arbitrarily long response.
pub fn invalid_account_attribute_name<E>(name: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_PARAMETER,
        400,
        format!("The property '{}' is not valid", echoed_resource_id(name)),
    )
}
