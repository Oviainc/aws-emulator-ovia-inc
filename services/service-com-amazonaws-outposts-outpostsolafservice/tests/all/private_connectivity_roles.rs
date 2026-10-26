// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IAM ROLE members an Outpost's private connectivity
//! configuration reports, driven over the wire through the same front door as
//! [`crate::site_and_outpost_lifecycle`] (whose harness this suite reuses), and over
//! the request/route helpers of [`crate::private_connectivity`], which covers the
//! configuration's other members.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. A created private connectivity configuration reports `RoleArn` — the
//!    service-linked role AWS Outposts provisions the private connectivity network
//!    interfaces with — in the modelled `RoleArn` form, for the partition of the
//!    Region the request was addressed to and the account every emulated ARN
//!    carries. Every configuration has one, and it is the SAME value every later
//!    read of that configuration answers.
//! 2. `ProvisioningRoleArn` is reported only for a configuration that is VPC
//!    ENDPOINT-based — the single condition the member's own documentation attaches
//!    to it (`smithy_query(query="doc",shape="PrivateConnectivityConfig",member="ProvisioningRoleArn")`)
//!    — i.e. exactly when the configuration's VPC information names an interface
//!    VPC endpoint, and never for one that does not; when reported, it is in the
//!    same modelled form and is not the service-linked role.
//! 3. The roles are derived from the Outpost they were created for — the member's
//!    documentation scopes the service-linked role's permissions to the specific
//!    Outpost and VPC — so two Outposts never report the same role ARN, and an
//!    Outpost nobody configured reports no configuration (and therefore no role)
//!    at all.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::private_connectivity::{config_of, request, route, vpc_information, MAX_SUBNET_IDS};
use crate::site_and_outpost_lifecycle::{text, Outposts};

/// The modelled `RoleArn` maximum length (`@length(min: 1, max: 150)`), from
/// `smithy_query(query="shape",shape="PrivateConnectivityConfig")`.
const MAX_ROLE_ARN_LENGTH: usize = 150;

/// The exact `(RoleArn, ProvisioningRoleArn)` a configuration created in
/// `partition` for `outpost_id` reports, so a change to the derivation is caught
/// rather than only a change to its shape. Both are per-Outpost, for the reason
/// rule 3 states.
fn expected_role_arns(partition: &str, outpost_id: &str) -> (String, String) {
    let account = engine::emulator_account_id();
    (
        format!(
            "arn:{partition}:iam::{account}:role/aws-service-role/outposts.amazonaws.com/\
             AWSServiceRoleForOutposts_{outpost_id}"
        ),
        format!("arn:{partition}:iam::{account}:role/AWSOutpostsProvisioning_{outpost_id}"),
    )
}

/// Whether `value` is of the modelled `RoleArn` form for `partition`:
/// `arn:<partition>:iam::<account>:role/<name>` with a non-empty name drawn from the
/// character set the member's pattern declares
/// (`^arn:aws([a-z-]+)?:iam::\d{12}:role\/[a-zA-Z_0-9+=,.@\-_\/]+$`), within the
/// modelled maximum length. IAM is a global service, so the Region segment of the
/// ARN is empty. The account is the one every generated emulator presents, taken
/// from the same accessor the handler mints it through.
fn is_modelled_role_arn(value: &str, partition: &str) -> bool {
    let prefix = format!(
        "arn:{partition}:iam::{}:role/",
        engine::emulator_account_id()
    );
    let Some(name) = value.strip_prefix(&prefix) else {
        return false;
    };
    !name.is_empty()
        && value.len() <= MAX_ROLE_ARN_LENGTH
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_+=,.@-/".contains(&byte))
}

/// One row of the table: the Region the configuration is created in, the partition
/// AWS places that Region in, the request that creates it, and whether that request
/// makes the configuration VPC endpoint-based.
///
/// The Regions and their partitions are AWS's published partition prefixes, the same
/// source [`crate::arn_partition`] names: `aws` for the commercial Regions, `aws-cn`
/// for `cn-*`, `aws-us-gov` for `us-gov-*`, and the ISO partitions for `us-isob-*`
/// and `us-isof-*`. The rows differ along everything the rules generalise over:
/// whether the VPC information names an interface VPC endpoint, which other members
/// it carries, how many subnet ids it names (one, several, and the modelled
/// maximum), and which partition the request's Region belongs to.
struct Case {
    label: &'static str,
    region: &'static str,
    partition: &'static str,
    payload: Value,
    endpoint_based: bool,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "vpc, one subnet and an endpoint, in a commercial Region",
            region: "us-east-1",
            partition: "aws",
            payload: request(vpc_information(
                Some("vpc-0a1b2c3d"),
                Some(1),
                Some("vpce-0a1b2c3d4e5f"),
            )),
            endpoint_based: true,
        },
        Case {
            label: "vpc and three subnets, no endpoint, in another commercial Region",
            region: "eu-west-3",
            partition: "aws",
            payload: request(vpc_information(Some("vpc-11112222"), Some(3), None)),
            endpoint_based: false,
        },
        Case {
            label: "a vpc and nothing else, in GovCloud",
            region: "us-gov-west-1",
            partition: "aws-us-gov",
            payload: request(vpc_information(Some("vpc-33334444"), None, None)),
            endpoint_based: false,
        },
        Case {
            label: "an endpoint and nothing else, in China",
            region: "cn-north-1",
            partition: "aws-cn",
            payload: request(vpc_information(None, None, Some("vpce-aaaabbbbcccc"))),
            endpoint_based: true,
        },
        Case {
            label: "every subnet the modelled list takes plus an endpoint, in an ISO Region",
            region: "us-isob-east-1",
            partition: "aws-iso-b",
            payload: request(vpc_information(
                Some("vpc-55556666"),
                Some(MAX_SUBNET_IDS),
                Some("vpce-0000111122223333"),
            )),
            endpoint_based: true,
        },
        Case {
            label: "subnets and nothing else, in another ISO Region",
            region: "us-isof-south-1",
            partition: "aws-iso-f",
            payload: request(vpc_information(None, Some(4), None)),
            endpoint_based: false,
        },
    ]
}

/// A fresh Outpost on a fresh site, both in `region`, as its `(OutpostId, OutpostArn)`.
async fn outpost_in(outposts: &Outposts, region: &str, name: &str) -> (String, String) {
    let site_id = outposts.site_id_in(region, &format!("{name} site")).await;
    let created = outposts
        .create_outpost_in(region, &json!({ "Name": name, "SiteId": site_id }))
        .await;
    (text(&created, "OutpostId"), text(&created, "OutpostArn"))
}

/// Rules 1 + 2 + 3, over the whole table: every created configuration reports the
/// service-linked role in the modelled form for its Region's partition, reports the
/// provisioning role exactly when it is VPC endpoint-based, answers the same values
/// from every later read through either identifier form, and shares neither role
/// with another Outpost.
#[tokio::test]
async fn a_created_configuration_reports_its_role_arns_and_reads_them_back() {
    let outposts = Outposts::new();
    // Every role ARN the table has seen, so no two Outposts can report one.
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for case in cases() {
        let Case {
            label,
            region,
            partition,
            payload,
            endpoint_based,
        } = case;
        let (outpost_id, outpost_arn) = outpost_in(&outposts, region, label).await;
        let (expected_role, expected_provisioning) = expected_role_arns(partition, &outpost_id);

        let created = outposts
            .ok_in(region, "POST", &route(&outpost_id), Some(&payload))
            .await;
        let config = config_of(&created);

        // Rule 1: the service-linked role, present for every configuration, in the
        // modelled form for this Region's partition.
        let role_arn = text(&config, "RoleArn");
        assert!(
            is_modelled_role_arn(&role_arn, partition),
            "case {label:?}: expected a RoleArn of the modelled form \
             arn:{partition}:iam::<account>:role/<name>, got {role_arn:?}"
        );
        assert_eq!(
            role_arn, expected_role,
            "case {label:?}: expected the service-linked role of this Outpost, got {role_arn:?}"
        );

        // Rule 2: the provisioning role, reported only for a VPC endpoint-based
        // configuration.
        let provisioning = config.get("ProvisioningRoleArn").cloned();
        match endpoint_based {
            true => {
                let provisioning_arn = provisioning
                    .as_ref()
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    is_modelled_role_arn(&provisioning_arn, partition),
                    "case {label:?}: a VPC endpoint-based configuration must report a \
                     ProvisioningRoleArn of the modelled form \
                     arn:{partition}:iam::<account>:role/<name>, got {provisioning_arn:?}"
                );
                assert_eq!(
                    provisioning_arn, expected_provisioning,
                    "case {label:?}: expected the provisioning role of this Outpost, got \
                     {provisioning_arn:?}"
                );
                assert_ne!(
                    provisioning_arn, role_arn,
                    "case {label:?}: the provisioning role is not the service-linked role, got \
                     {provisioning_arn:?}"
                );
                assert!(
                    seen.insert(provisioning_arn.clone()),
                    "case {label:?}: the provisioning role belongs to this Outpost alone, but \
                     {provisioning_arn:?} was already reported for another"
                );
            }
            false => assert_eq!(
                provisioning, None,
                "case {label:?}: a configuration that names no VPC endpoint must report no \
                 ProvisioningRoleArn, got {provisioning:?}"
            ),
        }

        // Rule 3: one Outpost's roles are its own.
        assert!(
            seen.insert(role_arn.clone()),
            "case {label:?}: the service-linked role belongs to this Outpost alone, but \
             {role_arn:?} was already reported for another"
        );

        // Rule 1's second half: the create reported what was stored, so every read —
        // by bare id or by Outpost ARN, however often — answers exactly the same
        // configuration, role members included.
        for reader in [outpost_id.clone(), outpost_arn.clone(), outpost_id.clone()] {
            let read = config_of(&outposts.ok_in(region, "GET", &route(&reader), None).await);
            assert_eq!(
                read, config,
                "case {label:?}: GetPrivateConnectivityConfig on {reader:?} must answer the \
                 stored configuration, role members included, got {read:?}"
            );
        }
    }
}

/// Rule 3's other half: an Outpost nobody configured reports no configuration, so no
/// role ARN is minted for one that does not exist — and configuring one Outpost does
/// not give another Outpost's configuration its roles.
#[tokio::test]
async fn an_unconfigured_outpost_reports_no_role_arns() {
    let outposts = Outposts::new();
    let region = "us-east-1";
    let (configured, _) = outpost_in(&outposts, region, "configured for roles").await;
    let (untouched, _) = outpost_in(&outposts, region, "untouched by roles").await;

    let payload = request(vpc_information(
        Some("vpc-77778888"),
        Some(2),
        Some("vpce-99990000"),
    ));
    let created = config_of(
        &outposts
            .ok_in(region, "POST", &route(&configured), Some(&payload))
            .await,
    );

    let read = outposts
        .ok_in(region, "GET", &route(&untouched), None)
        .await;
    assert_eq!(
        config_of(&read),
        Value::Null,
        "an Outpost nobody configured must report no PrivateConnectivityConfig at all, got {read:?}"
    );

    let configured_read = config_of(&outposts.ok_in(region, "GET", &route(&configured), None).await);
    assert_eq!(
        configured_read, created,
        "the configured Outpost keeps its own roles, got {configured_read:?}"
    );
}
