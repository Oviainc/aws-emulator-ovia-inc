// SPDX-License-Identifier: CC0-1.0

//! The catalog of AWS managed permissions this service ships.
//!
//! RAM maintains managed permissions of its own for the resource types it shares, and
//! those permissions belong to Amazon Web Services rather than to an account: their
//! ARNs name no Region and carry `aws` where an account id would be
//! (`arn:aws:ram::aws:permission/<PermissionName>`), their `permissionType` is
//! `AWS_MANAGED` — "Amazon Web Services created and manages this managed permission.
//! You can associate it with your resource shares, but you can't modify it"
//! (`ResourceSharePermissionDetail$permissionType`) — and they exist before any caller
//! creates anything. So the store holds them from the moment it is first touched
//! (`ComAmazonawsRamAmazonresourcesharingState::default`), beside the customer managed
//! permissions `CreatePermission` mints, and every read that resolves a permission ARN
//! resolves these too.
//!
//! Source of the table below: the managed permissions AWS Resource Access Manager
//! publishes for its shareable resource types — the RAM Permissions library, which
//! `ListPermissions` with `permissionType` `AWS` reports and which
//! `GetPermission$permissionArn` points a caller at to discover them ("To find the ARN
//! for a permission, use either the ListPermissions operation or go to the Permissions
//! library page in the RAM console") — one entry per published permission, with the
//! resource type it applies to, the actions its policy allows and whether it is that
//! resource type's default. The model enumerates none of them, so the catalog is
//! written out here, exactly as the resource-type catalog in [`super::RESOURCE_TYPES`]
//! is; every entry's resource type is one of that catalog's.

use std::collections::BTreeMap;

use super::{
    PermissionRecord, PermissionVersionRecord, ARN_SERVICE, FIRST_PERMISSION_VERSION,
    PERMISSION_ARN_RESOURCE_PREFIX, PERMISSION_FEATURE_SET_STANDARD, PERMISSION_STATUS_ATTACHABLE,
    POLICY_EFFECT_ALLOW, POLICY_TEMPLATE_ACTION_ELEMENT, POLICY_TEMPLATE_EFFECT_ELEMENT,
};

/// The `PermissionType` of a permission Amazon Web Services created and manages, the
/// other value of the modelled `PermissionType` enum beside
/// [`super::PERMISSION_TYPE_CUSTOMER_MANAGED`].
pub const PERMISSION_TYPE_AWS_MANAGED: &str = "AWS_MANAGED";

/// The partition field of every ARN this emulator reports.
pub const ARN_PARTITION: &str = "aws";

/// What an AWS managed permission ARN carries where an account id would be: the
/// permission is Amazon Web Services' own rather than any account's.
pub const AWS_MANAGED_PERMISSION_ARN_ACCOUNT: &str = "aws";

/// One entry of the AWS managed permission catalog.
#[derive(Debug, Clone, Copy)]
pub struct AwsManagedPermission {
    /// The permission's name, which is also the resource id of its ARN.
    pub name: &'static str,
    /// The resource type it applies to, `<service-code>:<resource-code>`; one of
    /// [`super::RESOURCE_TYPES`].
    pub resource_type: &'static str,
    /// The actions its policy allows, which is what the `permission` document reports.
    pub actions: &'static [&'static str],
    /// Whether this is "the default managed permission for all resources of this
    /// resource type" (`ResourceSharePermissionDetail$isResourceTypeDefault`) — the
    /// one RAM attaches when a share names no permission for the type.
    pub is_resource_type_default: bool,
}

/// The AWS managed permissions this service serves, one entry per published
/// permission. See the module documentation for the source.
pub const AWS_MANAGED_PERMISSIONS: &[AwsManagedPermission] = &[
    entry(
        "APIGatewayPrivateDomainNameManagementPolicyDefaultPermission",
        "apigateway:Domainnames",
        &["apigateway:CreateAccessAssociation"],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionAppMesh",
        "appmesh:Mesh",
        &[
            "appmesh:CreateVirtualNode",
            "appmesh:DescribeMesh",
            "appmesh:DescribeVirtualNode",
            "appmesh:ListVirtualNodes",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionCodeBuildProject",
        "codebuild:Project",
        &[
            "codebuild:BatchGetProjects",
            "codebuild:ListBuildsForProject",
            "codebuild:StartBuild",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionCodeBuildReportGroup",
        "codebuild:ReportGroup",
        &[
            "codebuild:BatchGetReportGroups",
            "codebuild:BatchGetReports",
            "codebuild:ListReportsForReportGroup",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionCapacityReservation",
        "ec2:CapacityReservation",
        &["ec2:DescribeCapacityReservations", "ec2:RunInstances"],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionDedicatedHost",
        "ec2:DedicatedHost",
        &["ec2:DescribeHosts", "ec2:RunInstances"],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionIpamPool",
        "ec2:IpamPool",
        &[
            "ec2:AllocateIpamPoolCidr",
            "ec2:DescribeIpamPools",
            "ec2:GetIpamPoolAllocations",
            "ec2:GetIpamPoolCidrs",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionPrefixList",
        "ec2:PrefixList",
        &[
            "ec2:DescribeManagedPrefixLists",
            "ec2:GetManagedPrefixListEntries",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionSubnet",
        "ec2:Subnet",
        &[
            "ec2:CreateNetworkInterface",
            "ec2:DescribeSubnets",
            "ec2:RunInstances",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionTransitGateway",
        "ec2:TransitGateway",
        &[
            "ec2:CreateTransitGatewayVpcAttachment",
            "ec2:DeleteTransitGatewayVpcAttachment",
            "ec2:DescribeTransitGatewayVpcAttachments",
            "ec2:DescribeTransitGateways",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionGlueDatabase",
        "glue:Database",
        &[
            "glue:GetDatabase",
            "glue:GetDatabases",
            "glue:GetTable",
            "glue:GetTables",
        ],
        true,
    ),
    entry(
        "AWSRAMPermissionGlueDatabaseReadWrite",
        "glue:Database",
        &[
            "glue:CreateTable",
            "glue:DeleteTable",
            "glue:GetDatabase",
            "glue:GetTable",
            "glue:UpdateTable",
        ],
        false,
    ),
    entry(
        "AWSRAMDefaultPermissionGlueTable",
        "glue:Table",
        &[
            "glue:GetPartition",
            "glue:GetPartitions",
            "glue:GetTable",
            "glue:GetTables",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionImageBuilderImage",
        "imagebuilder:Image",
        &[
            "imagebuilder:GetImage",
            "imagebuilder:ListImageBuildVersions",
            "imagebuilder:ListImages",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionLicenseConfiguration",
        "license-manager:LicenseConfiguration",
        &[
            "license-manager:GetLicenseConfiguration",
            "license-manager:ListLicenseSpecificationsForResource",
            "license-manager:ListUsageForLicenseConfiguration",
            "license-manager:UpdateLicenseSpecificationsForResource",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionNetworkFirewallFirewallPolicy",
        "network-firewall:FirewallPolicy",
        &[
            "network-firewall:AssociateFirewallPolicy",
            "network-firewall:DescribeFirewallPolicy",
            "network-firewall:ListFirewallPolicies",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionStatefulRuleGroup",
        "network-firewall:StatefulRuleGroup",
        &[
            "network-firewall:DescribeRuleGroup",
            "network-firewall:ListRuleGroups",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionOutpost",
        "outposts:Outpost",
        &[
            "outposts:GetOutpost",
            "outposts:GetOutpostInstanceTypes",
            "outposts:ListOutposts",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionSite",
        "outposts:Site",
        &["outposts:GetSite", "outposts:ListSites"],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionResourceGroup",
        "resource-groups:Group",
        &[
            "resource-groups:GetGroup",
            "resource-groups:GetGroupConfiguration",
            "resource-groups:ListGroupResources",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionResolverRule",
        "route53resolver:ResolverRule",
        &[
            "route53resolver:AssociateResolverRule",
            "route53resolver:DisassociateResolverRule",
            "route53resolver:GetResolverRule",
            "route53resolver:ListResolverRuleAssociations",
        ],
        true,
    ),
    entry(
        "AWSRAMDefaultPermissionVpcLatticeServiceNetwork",
        "vpc-lattice:ServiceNetwork",
        &[
            "vpc-lattice:CreateServiceNetworkVpcAssociation",
            "vpc-lattice:GetServiceNetwork",
            "vpc-lattice:ListServiceNetworks",
        ],
        true,
    ),
];

/// One catalog entry, so [`AWS_MANAGED_PERMISSIONS`] reads as the table it is.
const fn entry(
    name: &'static str,
    resource_type: &'static str,
    actions: &'static [&'static str],
    is_resource_type_default: bool,
) -> AwsManagedPermission {
    AwsManagedPermission {
        name,
        resource_type,
        actions,
        is_resource_type_default,
    }
}

/// The ARN of the AWS managed permission `name`.
///
/// An AWS managed permission is Amazon Web Services' own and exists in every Region,
/// so — unlike [`super::permission_arn`], which names a customer managed permission in
/// one account and Region — its ARN carries no Region and
/// [`AWS_MANAGED_PERMISSION_ARN_ACCOUNT`] in place of an account id.
pub fn aws_managed_permission_arn(name: &str) -> String {
    format!(
        "arn:{ARN_PARTITION}:{ARN_SERVICE}::{AWS_MANAGED_PERMISSION_ARN_ACCOUNT}:\
         {PERMISSION_ARN_RESOURCE_PREFIX}{name}"
    )
}

impl AwsManagedPermission {
    /// This entry's ARN.
    pub fn arn(&self) -> String {
        aws_managed_permission_arn(self.name)
    }

    /// The policy document this entry's version reports: the effect and the actions,
    /// the two elements `ResourceSharePermissionDetail$permission` describes ("The
    /// permission's effect and actions in JSON format").
    pub fn policy_document(&self) -> String {
        ::serde_json::json!({
            POLICY_TEMPLATE_EFFECT_ELEMENT: POLICY_EFFECT_ALLOW,
            POLICY_TEMPLATE_ACTION_ELEMENT: self.actions,
        })
        .to_string()
    }

    /// This entry as the record the store holds, created at `now`.
    ///
    /// Amazon Web Services publishes each of these permissions at one version, which
    /// is therefore its default version, and that version is `ATTACHABLE`: it "can be
    /// associated with resource shares" (`ResourceSharePermissionDetail$status`),
    /// which is the whole point of the catalog. A permission nobody can modify carries
    /// no tags.
    fn record(&self, now: i64) -> PermissionRecord {
        let version = FIRST_PERMISSION_VERSION;
        PermissionRecord {
            arn: self.arn(),
            name: self.name.to_string(),
            resource_type: self.resource_type.to_string(),
            permission_type: PERMISSION_TYPE_AWS_MANAGED.to_string(),
            feature_set: PERMISSION_FEATURE_SET_STANDARD.to_string(),
            tags: BTreeMap::new(),
            creation_time: now,
            last_updated_time: now,
            default_version: version,
            is_resource_type_default: self.is_resource_type_default,
            versions: [(
                version,
                PermissionVersionRecord {
                    version,
                    policy_template: self.policy_document(),
                    status: PERMISSION_STATUS_ATTACHABLE.to_string(),
                    creation_time: now,
                    last_updated_time: now,
                },
            )]
            .into_iter()
            .collect(),
        }
    }
}

/// The whole catalog as the records the store starts from, keyed by ARN exactly as the
/// customer managed permissions beside them are, so one lookup resolves either kind.
pub fn catalog_records(now: i64) -> BTreeMap<String, PermissionRecord> {
    AWS_MANAGED_PERMISSIONS
        .iter()
        .map(|entry| (entry.arn(), entry.record(now)))
        .collect()
}
