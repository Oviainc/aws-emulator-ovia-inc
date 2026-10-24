// SPDX-License-Identifier: CC0-1.0

//! The controls the Control Catalog publishes, as data: one row per published control.
//!
//! ## One row per published control, not per check
//!
//! The catalog publishes a control for every surface that governs a resource, and
//! several surfaces can govern the SAME thing: `ListControls` "[r]eturns a paginated
//! list of all available controls in the Control Catalog library", each as a
//! `ControlSummary` carrying its own `Arn`, `Aliases` and `Implementation`
//! (`smithy_query(query="operation", operation="ListControls",
//! model_path="services/service-com-amazonaws-controlcatalog-controlcatalog/smithy-model/com-amazonaws-controlcatalog-controlcatalog.json")`).
//! So the AWS Config rule that detects a misconfiguration, the Security Hub control
//! that reports it, the service control policy that prevents it and the CloudFormation
//! hook that rejects it are FOUR published controls, each with its own identity, not one
//! record wearing four designators — and two controls may legitimately share one
//! `Implementation` (a Control Tower detective control and the Config-rule control
//! behind it both report `AWS::Config::ConfigRule` / `CLOUD_TRAIL_ENABLED`), which is
//! why `Implementation` is not this table's key and the `Identifiers` filter can match
//! more than one row.
//!
//! ## Where the values come from
//!
//! * `implementation_type` / `implementation_identifier` are the implementing service's
//!   own published identifiers: an AWS Config managed rule identifier
//!   (`MULTI_REGION_CLOUD_TRAIL_ENABLED`), a Security Hub control id (`CloudTrail.1`),
//!   or — for a service control policy, which has no per-control identifier — `None`.
//!   Both members' character classes come from the model: `ImplementationType` is
//!   `^[A-Za-z0-9]+(::[A-Za-z0-9_]+){2,3}$` and `ImplementationIdentifier`
//!   `^[a-zA-Z0-9_\.-]+$` (`smithy_query(query="shape", shape="ImplementationType")`).
//! * `control_tower_identifier` is the `AWS-GR_…` identifier Amazon Web Services Control
//!   Tower publishes for the control, where it publishes one.
//! * `parameters` are the implementing rule's own parameters; the modeled
//!   `ParameterRequirementSummary` is DERIVED from them (see
//!   [`parameter_requirement_summary`]) rather than written per row, because it is
//!   defined as a summary of exactly that list.
//! * `aliases` are DERIVED for the two designators that are functions of an identifier
//!   (see [`aliases`]); only a designator that is neither — a `CONFIG.…` or `CT.…`
//!   catalog designator — is written in the row.
//!
//! The opaque 25-character identifier inside a catalog ARN is assigned by Amazon Web
//! Services and is derivable from nothing in the request, so — exactly as for the
//! ontology in [`super::published_ontology`] — this emulator mints its own
//! deterministically from each row's `slug` (see `super::catalog_id`). The slug never
//! appears on the wire.

/// `ControlParameterRequirement`: a parameter the caller must supply.
pub(super) const REQUIRED: &str = "REQUIRED";
/// `ControlParameterRequirement`: a parameter the caller may supply.
pub(super) const OPTIONAL: &str = "OPTIONAL";

/// `ControlBehavior`: the control reports on resources that already exist.
const DETECTIVE: &str = "DETECTIVE";
/// `ControlBehavior`: the control blocks a non-compliant action.
const PREVENTIVE: &str = "PREVENTIVE";
/// `ControlBehavior`: the control rejects a non-compliant deployment before it runs.
const PROACTIVE: &str = "PROACTIVE";

/// `ControlSeverity` values, as the modeled enum spells them.
const LOW: &str = "LOW";
const MEDIUM: &str = "MEDIUM";
const HIGH: &str = "HIGH";
const CRITICAL: &str = "CRITICAL";

/// `ControlScope`: effective in every Region, so it reports no deployable Regions.
const GLOBAL: &str = "GLOBAL";
/// `ControlScope`: deployed per Region, so it reports the Regions it deploys to.
const REGIONAL: &str = "REGIONAL";

/// Implementation types this catalog publishes controls for.
pub(super) const CONFIG_RULE: &str = "AWS::Config::ConfigRule";
pub(super) const SECURITY_HUB_CONTROL: &str = "AWS::SecurityHub::SecurityControl";
pub(super) const SERVICE_CONTROL_POLICY: &str =
    "AWS::Organizations::Policy::SERVICE_CONTROL_POLICY";
pub(super) const CLOUDFORMATION_HOOK: &str = "AWS::CloudFormation::Type::HOOK";

/// The `SH.` designator prefix a Security Hub control is published under. The
/// `Aliases` documentation gives `SH.S3.1` as its example of a "human-readable
/// designator", and `S3.1` is that control's own Security Hub id.
pub(super) const SECURITY_HUB_ALIAS_PREFIX: &str = "SH.";

/// `GovernedProviders` values: "a value of AWS indicates that the control governs
/// Amazon Web Services resources".
const AWS: &[&str] = &["AWS"];
const GCP: &[&str] = &["GCP"];

/// Region sets a REGIONAL control in this catalog deploys to. A control is available
/// in the Regions its implementing service is, which differs between controls, so the
/// table picks the set per row.
const BROAD_REGIONS: &[&str] = &[
    "ap-southeast-2",
    "eu-central-1",
    "eu-west-1",
    "us-east-1",
    "us-east-2",
    "us-west-2",
];
const CORE_REGIONS: &[&str] = &["eu-west-1", "us-east-1", "us-west-2"];
const US_REGIONS: &[&str] = &["us-east-1", "us-west-2"];
/// A GLOBAL control deploys to no Region of its own.
const NO_REGIONS: &[&str] = &[];

/// A control that supports no parameters.
const NO_PARAMETERS: &[(&str, &str)] = &[];

/// A control that is published under no catalog designator of its own (its aliases,
/// if any, are the derived ones).
const NO_DESIGNATORS: &[&str] = &[];

/// A resource set that no available resource type can represent: "If
/// GovernedResources cannot be represented by available resource types, it's returned
/// as an empty list."
const NO_GOVERNED_RESOURCES: &[&str] = &[];

// The compliance frameworks this catalog's FRAMEWORK mappings name, each written as
// the framework's own published title. The modeled `FrameworkName` admits 3..250
// characters, so the published title is what goes on the wire; the item beside it in
// a mapping row is that framework's own identifier for the requirement.
pub(super) const NIST_SP_800_53_R5: &str = "NIST Special Publication 800-53 Revision 5";
pub(super) const NIST_CSF_V1_1: &str = "NIST Cybersecurity Framework Version 1.1";
pub(super) const PCI_DSS_V4_0: &str = "Payment Card Industry Data Security Standard v4.0";
pub(super) const CIS_AWS_FOUNDATIONS_V3_0_0: &str =
    "CIS Amazon Web Services Foundations Benchmark v3.0.0";
pub(super) const CIS_CONTROLS_V8_0: &str = "CIS Critical Security Controls Version 8.0";
pub(super) const CIS_GCP_FOUNDATION_V2_0_0: &str =
    "CIS Google Cloud Platform Foundation Benchmark v2.0.0";

/// One control the catalog publishes.
#[derive(Debug)]
pub(super) struct ControlSeed {
    /// Internal, never on the wire: the hash input `super::catalog_id` mints this
    /// control's opaque catalog identifier from, and the key the mapping tables use.
    pub(super) slug: &'static str,
    pub(super) name: &'static str,
    pub(super) description: &'static str,
    /// `ControlBehavior`.
    pub(super) behavior: &'static str,
    /// `ControlSeverity`.
    pub(super) severity: &'static str,
    /// `ImplementationType`.
    pub(super) implementation_type: &'static str,
    /// `ImplementationIdentifier`, where the implementing surface publishes one.
    pub(super) implementation_identifier: Option<&'static str>,
    /// The Control Tower `AWS-GR_…` identifier, for a control Control Tower publishes.
    pub(super) control_tower_identifier: Option<&'static str>,
    /// Catalog designators that are not a function of an identifier (`CONFIG.…`,
    /// `CT.…`); the `SH.` and Control Tower designators are derived, not written here.
    pub(super) designators: &'static [&'static str],
    /// `ControlScope`.
    pub(super) scope: &'static str,
    /// Empty for a GLOBAL control: `DeployableRegions` does not apply to it.
    pub(super) deployable_regions: &'static [&'static str],
    /// `(name, ControlParameterRequirement)` for each parameter the control supports.
    pub(super) parameters: &'static [(&'static str, &'static str)],
    pub(super) governed_resources: &'static [&'static str],
    pub(super) governed_providers: &'static [&'static str],
    /// The slug of the common control this control implements, in
    /// [`super::published_ontology`]: the control's COMMON_CONTROL mapping.
    pub(super) implements: &'static str,
    /// `(framework title, that framework's identifier for the requirement)` for each
    /// published framework item the control satisfies: its FRAMEWORK mappings.
    pub(super) frameworks: &'static [(&'static str, &'static str)],
}

/// The catalog's controls, in one table.
pub(super) const PUBLISHED_CONTROLS: &[ControlSeed] = &[
    // --- AWS Config managed rules: detective controls -----------------------
    ControlSeed {
        slug: "account-part-of-organizations",
        name: "Checks whether an Amazon Web Services account is part of an organization",
        description: "Reports an account that is not a member of an Amazon Web Services \
                      organization, so organization-wide governance cannot reach it.",
        behavior: DETECTIVE,
        severity: HIGH,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("ACCOUNT_PART_OF_ORGANIZATIONS"),
        control_tower_identifier: None,
        designators: &["CONFIG.ACCOUNT.DT.1"],
        scope: GLOBAL,
        deployable_regions: NO_REGIONS,
        parameters: NO_PARAMETERS,
        // An account's membership of an organization has no resource type of its own.
        governed_resources: NO_GOVERNED_RESOURCES,
        governed_providers: AWS,
        implements: "maintain-an-asset-inventory",
        frameworks: &[
            // "System component inventory".
            (NIST_SP_800_53_R5, "CM-8"),
            // "Establish and maintain detailed enterprise asset inventory".
            (CIS_CONTROLS_V8_0, "1.1"),
        ],
    },
    ControlSeed {
        slug: "cloud-trail-enabled",
        name: "Checks whether CloudTrail is enabled in an Amazon Web Services account",
        description: "Reports an account that records no management events through a \
                      CloudTrail trail.",
        behavior: DETECTIVE,
        severity: HIGH,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("CLOUD_TRAIL_ENABLED"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: CORE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::CloudTrail::Trail"],
        governed_providers: AWS,
        implements: "enable-centralized-logging",
        frameworks: &[
            // "Event logging".
            (NIST_SP_800_53_R5, "AU-2"),
            // "Ensure CloudTrail is enabled in all Regions".
            (CIS_AWS_FOUNDATIONS_V3_0_0, "3.1"),
        ],
    },
    ControlSeed {
        slug: "detect-cloud-trail-enabled-on-member-accounts",
        name: "Detect whether CloudTrail is enabled in a member account",
        description: "Reports a member account of the organization that records no \
                      management events through a CloudTrail trail.",
        behavior: DETECTIVE,
        severity: HIGH,
        // The same Config rule backs this Control Tower control and the Config-rule
        // control above: two published controls, one implementation.
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("CLOUD_TRAIL_ENABLED"),
        control_tower_identifier: Some("AWS-GR_DETECT_CLOUDTRAIL_ENABLED_ON_MEMBER_ACCOUNTS"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: BROAD_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::CloudTrail::Trail"],
        governed_providers: AWS,
        implements: "enable-centralized-logging",
        frameworks: &[
            // "Event data are collected and correlated".
            (NIST_CSF_V1_1, "DE.AE-3"),
            // "Event logging".
            (NIST_SP_800_53_R5, "AU-2"),
        ],
    },
    ControlSeed {
        slug: "multi-region-cloud-trail-enabled",
        name: "Checks that there is at least one multi-region AWS CloudTrail",
        description: "Reports an account whose CloudTrail trails all record events in a \
                      single Region.",
        behavior: DETECTIVE,
        severity: HIGH,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("MULTI_REGION_CLOUD_TRAIL_ENABLED"),
        control_tower_identifier: None,
        designators: &["CONFIG.CLOUDTRAIL.DT.10"],
        scope: REGIONAL,
        deployable_regions: BROAD_REGIONS,
        // The Config rule's own parameters, all optional — which is what makes this
        // control's ParameterRequirementSummary OPTIONAL.
        parameters: &[
            ("s3BucketName", OPTIONAL),
            ("snsTopicArn", OPTIONAL),
            ("cloudWatchLogsLogGroupArn", OPTIONAL),
            ("includeManagementEvents", OPTIONAL),
            ("readWriteType", OPTIONAL),
        ],
        governed_resources: &["AWS::CloudTrail::Trail"],
        governed_providers: AWS,
        implements: "enable-centralized-logging",
        frameworks: &[
            // "Audit logs are enabled and active for all system components".
            (PCI_DSS_V4_0, "10.2.1"),
            // "Ensure CloudTrail is enabled in all Regions".
            (CIS_AWS_FOUNDATIONS_V3_0_0, "3.1"),
        ],
    },
    ControlSeed {
        slug: "cloud-trail-log-file-validation-enabled",
        name: "Checks whether CloudTrail log file validation is enabled",
        description: "Reports a CloudTrail trail that publishes no digest files, so log \
                      tampering cannot be detected.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("CLOUD_TRAIL_LOG_FILE_VALIDATION_ENABLED"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: CORE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::CloudTrail::Trail"],
        governed_providers: AWS,
        implements: "log-integrity-protection",
        frameworks: &[
            // "Protection of audit information".
            (NIST_SP_800_53_R5, "AU-9"),
            // "Ensure CloudTrail log file validation is enabled".
            (CIS_AWS_FOUNDATIONS_V3_0_0, "3.2"),
        ],
    },
    ControlSeed {
        slug: "s3-bucket-server-side-encryption-enabled",
        name: "Require server-side encryption on Amazon S3 buckets",
        description: "Checks whether an Amazon S3 bucket has a default server-side \
                      encryption configuration.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("S3_BUCKET_SERVER_SIDE_ENCRYPTION_ENABLED"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: CORE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::S3::Bucket"],
        governed_providers: AWS,
        implements: "encrypt-data-at-rest-in-storage-services",
        frameworks: &[
            // "Protection of information at rest".
            (NIST_SP_800_53_R5, "SC-28"),
            // "Data-at-rest is protected".
            (NIST_CSF_V1_1, "PR.DS-1"),
            // "Primary account number is rendered unreadable wherever it is stored".
            (PCI_DSS_V4_0, "3.5.1"),
            // "Encrypt sensitive data at rest".
            (CIS_CONTROLS_V8_0, "3.11"),
        ],
    },
    ControlSeed {
        slug: "s3-bucket-public-read-prohibited",
        name: "Checks whether an Amazon S3 bucket allows public read access",
        description: "Reports an Amazon S3 bucket whose policy or access control list \
                      grants read access to everyone.",
        behavior: DETECTIVE,
        severity: CRITICAL,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("S3_BUCKET_PUBLIC_READ_PROHIBITED"),
        control_tower_identifier: Some("AWS-GR_AUDIT_BUCKET_PUBLIC_READ_PROHIBITED"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: BROAD_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::S3::Bucket"],
        governed_providers: AWS,
        implements: "restrict-public-access",
        frameworks: &[
            // "Access enforcement".
            (NIST_SP_800_53_R5, "AC-3"),
            // "Ensure that S3 buckets are configured with 'Block public access'".
            (CIS_AWS_FOUNDATIONS_V3_0_0, "2.1.4"),
        ],
    },
    ControlSeed {
        slug: "s3-bucket-public-write-prohibited",
        name: "Checks whether an Amazon S3 bucket allows public write access",
        description: "Reports an Amazon S3 bucket whose policy or access control list \
                      grants write access to everyone.",
        behavior: DETECTIVE,
        severity: CRITICAL,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("S3_BUCKET_PUBLIC_WRITE_PROHIBITED"),
        control_tower_identifier: Some("AWS-GR_AUDIT_BUCKET_PUBLIC_WRITE_PROHIBITED"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: BROAD_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::S3::Bucket"],
        governed_providers: AWS,
        implements: "restrict-public-access",
        frameworks: &[
            // "Protections against data leaks are implemented".
            (NIST_CSF_V1_1, "PR.DS-5"),
            // "Access enforcement".
            (NIST_SP_800_53_R5, "AC-3"),
        ],
    },
    ControlSeed {
        slug: "s3-bucket-ssl-requests-only",
        name: "Checks whether an Amazon S3 bucket requests are made over a secure transport",
        description: "Reports an Amazon S3 bucket whose policy does not deny requests made \
                      over unencrypted HTTP.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("S3_BUCKET_SSL_REQUESTS_ONLY"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: CORE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::S3::Bucket"],
        governed_providers: AWS,
        implements: "enforce-encryption-in-transit",
        frameworks: &[
            // "Transmission confidentiality and integrity".
            (NIST_SP_800_53_R5, "SC-8"),
            // "Strong cryptography is implemented to safeguard data in transit".
            (PCI_DSS_V4_0, "4.2.1"),
        ],
    },
    ControlSeed {
        slug: "iam-root-access-key-check",
        name: "Checks whether the root user has an access key",
        description: "Reports an account whose root user holds an access key.",
        behavior: DETECTIVE,
        severity: CRITICAL,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("IAM_ROOT_ACCESS_KEY_CHECK"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: GLOBAL,
        deployable_regions: NO_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::IAM::AccessKey"],
        governed_providers: AWS,
        implements: "privileged-access-approval",
        frameworks: &[
            // "Ensure no 'root' user account access key exists".
            (CIS_AWS_FOUNDATIONS_V3_0_0, "1.4"),
            // "Least privilege".
            (NIST_SP_800_53_R5, "AC-6"),
        ],
    },
    ControlSeed {
        slug: "iam-user-mfa-enabled",
        name: "Checks whether every IAM user has multi-factor authentication enabled",
        description: "Reports an IAM user with no multi-factor authentication device.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("IAM_USER_MFA_ENABLED"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: GLOBAL,
        deployable_regions: NO_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::IAM::User"],
        governed_providers: AWS,
        implements: "require-multi-factor-authentication",
        frameworks: &[
            // "Ensure MFA is enabled for all IAM users that have a console password".
            (CIS_AWS_FOUNDATIONS_V3_0_0, "1.10"),
            // "Identification and authentication (organizational users)".
            (NIST_SP_800_53_R5, "IA-2"),
        ],
    },
    ControlSeed {
        slug: "access-keys-rotated",
        name: "Checks whether an IAM access key is rotated within the required number of days",
        description: "Reports an active IAM access key older than the age the control's \
                      parameter sets.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("ACCESS_KEYS_ROTATED"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: GLOBAL,
        deployable_regions: NO_REGIONS,
        // The Config rule's own parameter, which it has no default for: supplying it is
        // what makes this control's ParameterRequirementSummary REQUIRED.
        parameters: &[("maxAccessKeyAge", REQUIRED)],
        governed_resources: &["AWS::IAM::AccessKey"],
        governed_providers: AWS,
        implements: "credential-rotation",
        frameworks: &[
            // "Ensure access keys are rotated every 90 days or less".
            (CIS_AWS_FOUNDATIONS_V3_0_0, "1.14"),
            // "Authenticator management".
            (NIST_SP_800_53_R5, "IA-5"),
        ],
    },
    ControlSeed {
        slug: "iam-password-policy",
        name: "Checks whether the account password policy meets the required strength",
        description: "Reports an account whose IAM password policy is weaker than the \
                      control's parameters require.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("IAM_PASSWORD_POLICY"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: GLOBAL,
        deployable_regions: NO_REGIONS,
        parameters: &[
            ("MinimumPasswordLength", OPTIONAL),
            ("RequireUppercaseCharacters", OPTIONAL),
            ("RequireNumbers", OPTIONAL),
            ("MaxPasswordAge", OPTIONAL),
        ],
        // The account's password policy has no resource type of its own.
        governed_resources: NO_GOVERNED_RESOURCES,
        governed_providers: AWS,
        implements: "password-strength-enforcement",
        frameworks: &[
            // "Ensure IAM password policy requires a minimum length of 14 or greater".
            (CIS_AWS_FOUNDATIONS_V3_0_0, "1.8"),
            // "Authenticator management".
            (NIST_SP_800_53_R5, "IA-5"),
        ],
    },
    ControlSeed {
        slug: "encrypted-volumes",
        name: "Checks whether an attached Amazon EBS volume is encrypted",
        description: "Reports an Amazon EBS volume that is attached to an instance and not \
                      encrypted.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("ENCRYPTED_VOLUMES"),
        control_tower_identifier: Some("AWS-GR_ENCRYPTED_VOLUMES"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: BROAD_REGIONS,
        parameters: &[("kmsId", OPTIONAL)],
        governed_resources: &["AWS::EC2::Volume"],
        governed_providers: AWS,
        implements: "encrypt-data-at-rest-in-storage-services",
        frameworks: &[
            // "Ensure EBS volume encryption is enabled".
            (CIS_AWS_FOUNDATIONS_V3_0_0, "2.2.1"),
            // "Protection of information at rest".
            (NIST_SP_800_53_R5, "SC-28"),
        ],
    },
    ControlSeed {
        slug: "vpc-flow-logs-enabled",
        name: "Checks whether flow logs are enabled for every Amazon VPC",
        description: "Reports an Amazon VPC that publishes no flow logs, so network traffic \
                      cannot be reviewed.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("VPC_FLOW_LOGS_ENABLED"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: CORE_REGIONS,
        parameters: &[("trafficType", OPTIONAL)],
        governed_resources: &["AWS::EC2::VPC"],
        governed_providers: AWS,
        implements: "data-egress-monitoring",
        frameworks: &[
            // "Ensure VPC flow logging is enabled in all VPCs".
            (CIS_AWS_FOUNDATIONS_V3_0_0, "3.9"),
            // "Audit record generation".
            (NIST_SP_800_53_R5, "AU-12"),
        ],
    },
    ControlSeed {
        slug: "incoming-ssh-disabled",
        name: "Checks whether a security group allows unrestricted SSH ingress",
        description: "Reports a security group that allows inbound traffic to port 22 from \
                      any address.",
        behavior: DETECTIVE,
        severity: HIGH,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("INCOMING_SSH_DISABLED"),
        control_tower_identifier: Some("AWS-GR_RESTRICTED_SSH"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: BROAD_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::EC2::SecurityGroup"],
        governed_providers: AWS,
        implements: "restrict-administrative-port-exposure",
        frameworks: &[
            // "Ensure no security group allows ingress from 0.0.0.0/0 to port 22".
            (CIS_AWS_FOUNDATIONS_V3_0_0, "5.2"),
            // "Boundary protection".
            (NIST_SP_800_53_R5, "SC-7"),
        ],
    },
    ControlSeed {
        slug: "rds-snapshots-public-prohibited",
        name: "Checks whether an Amazon RDS snapshot is public",
        description: "Reports an Amazon RDS database snapshot that is shared with every \
                      account.",
        behavior: DETECTIVE,
        severity: HIGH,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("RDS_SNAPSHOTS_PUBLIC_PROHIBITED"),
        control_tower_identifier: Some("AWS-GR_RDS_SNAPSHOT_PUBLIC_PROHIBITED"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: CORE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::RDS::DBSnapshot"],
        governed_providers: AWS,
        implements: "restrict-public-access",
        frameworks: &[
            // "Access enforcement".
            (NIST_SP_800_53_R5, "AC-3"),
            // "Protections against data leaks are implemented".
            (NIST_CSF_V1_1, "PR.DS-5"),
        ],
    },
    ControlSeed {
        slug: "rds-storage-encrypted",
        name: "Checks whether storage encryption is enabled for an Amazon RDS instance",
        description: "Reports an Amazon RDS database instance whose storage is not \
                      encrypted.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("RDS_STORAGE_ENCRYPTED"),
        control_tower_identifier: Some("AWS-GR_RDS_STORAGE_ENCRYPTED"),
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: CORE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::RDS::DBInstance"],
        governed_providers: AWS,
        implements: "encrypt-data-at-rest-in-storage-services",
        frameworks: &[
            // "Data-at-rest is protected".
            (NIST_CSF_V1_1, "PR.DS-1"),
            // "Protection of information at rest".
            (NIST_SP_800_53_R5, "SC-28"),
        ],
    },
    ControlSeed {
        slug: "guardduty-enabled-centralized",
        name: "Checks whether Amazon GuardDuty is enabled and centrally managed",
        description: "Reports an account where Amazon GuardDuty is not enabled or is not \
                      administered from the organization's delegated account.",
        behavior: DETECTIVE,
        severity: HIGH,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("GUARDDUTY_ENABLED_CENTRALIZED"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: BROAD_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::GuardDuty::Detector"],
        governed_providers: AWS,
        implements: "intrusion-detection-deployment",
        frameworks: &[
            // "System monitoring".
            (NIST_SP_800_53_R5, "SI-4"),
            // "Ensure GuardDuty is enabled".
            (CIS_AWS_FOUNDATIONS_V3_0_0, "3.8"),
        ],
    },
    ControlSeed {
        slug: "cloudwatch-log-group-encrypted",
        name: "Checks whether a CloudWatch log group is encrypted with a KMS key",
        description: "Reports a CloudWatch Logs log group that is not encrypted with a KMS \
                      key.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: CONFIG_RULE,
        implementation_identifier: Some("CLOUDWATCH_LOG_GROUP_ENCRYPTED"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: CORE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::Logs::LogGroup"],
        governed_providers: AWS,
        implements: "log-integrity-protection",
        frameworks: &[
            // "Protection of audit information".
            (NIST_SP_800_53_R5, "AU-9"),
            // "Data-at-rest is protected".
            (NIST_CSF_V1_1, "PR.DS-1"),
        ],
    },
    // --- Security Hub controls ----------------------------------------------
    ControlSeed {
        slug: "securityhub-s3-4",
        name: "S3 general purpose buckets should have server-side encryption enabled",
        description: "Checks whether an Amazon S3 bucket has default server-side encryption \
                      enabled, and reports the finding in Security Hub.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: SECURITY_HUB_CONTROL,
        implementation_identifier: Some("S3.4"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: CORE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::S3::Bucket"],
        governed_providers: AWS,
        implements: "encrypt-data-at-rest-in-storage-services",
        frameworks: &[
            // "Protection of information at rest".
            (NIST_SP_800_53_R5, "SC-28"),
            // "Primary account number is rendered unreadable wherever it is stored".
            (PCI_DSS_V4_0, "3.5.1"),
        ],
    },
    ControlSeed {
        slug: "securityhub-cloudtrail-1",
        name: "CloudTrail should be enabled and configured with at least one multi-Region \
               trail that includes read and write management events",
        description: "Checks whether an account has a multi-Region CloudTrail trail \
                      recording read and write management events, and reports the finding \
                      in Security Hub.",
        behavior: DETECTIVE,
        severity: HIGH,
        implementation_type: SECURITY_HUB_CONTROL,
        implementation_identifier: Some("CloudTrail.1"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: BROAD_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::CloudTrail::Trail"],
        governed_providers: AWS,
        implements: "enable-centralized-logging",
        frameworks: &[
            // "Ensure CloudTrail is enabled in all Regions".
            (CIS_AWS_FOUNDATIONS_V3_0_0, "3.1"),
            // "Event logging".
            (NIST_SP_800_53_R5, "AU-2"),
        ],
    },
    ControlSeed {
        slug: "securityhub-iam-4",
        name: "IAM root user access key should not exist",
        description: "Checks whether the root user holds an access key, and reports the \
                      finding in Security Hub.",
        behavior: DETECTIVE,
        severity: CRITICAL,
        implementation_type: SECURITY_HUB_CONTROL,
        implementation_identifier: Some("IAM.4"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: GLOBAL,
        deployable_regions: NO_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::IAM::AccessKey"],
        governed_providers: AWS,
        implements: "privileged-access-approval",
        frameworks: &[
            // "Ensure no 'root' user account access key exists".
            (CIS_AWS_FOUNDATIONS_V3_0_0, "1.4"),
            // "Least privilege".
            (NIST_SP_800_53_R5, "AC-6"),
        ],
    },
    ControlSeed {
        slug: "securityhub-iam-5",
        name: "MFA should be enabled for all IAM users that have a console password",
        description: "Checks whether every identity with a console password also has \
                      multi-factor authentication enabled.",
        behavior: DETECTIVE,
        severity: LOW,
        implementation_type: SECURITY_HUB_CONTROL,
        implementation_identifier: Some("IAM.5"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: US_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::IAM::User"],
        governed_providers: AWS,
        implements: "require-multi-factor-authentication",
        frameworks: &[
            // "Identification and authentication (organizational users)".
            (NIST_SP_800_53_R5, "IA-2"),
            // "Multi-factor authentication is implemented for all access".
            (PCI_DSS_V4_0, "8.4.2"),
        ],
    },
    ControlSeed {
        slug: "gcp-storage-bucket-uniform-access",
        name: "Require uniform bucket-level access on Cloud Storage buckets",
        description: "Checks whether a Google Cloud Storage bucket enforces uniform \
                      bucket-level access.",
        behavior: DETECTIVE,
        severity: MEDIUM,
        implementation_type: SECURITY_HUB_CONTROL,
        implementation_identifier: Some("GCP.Storage.1"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: US_REGIONS,
        parameters: NO_PARAMETERS,
        // A non-Amazon Web Services control's resources are "represented in a
        // provider-specific format".
        governed_resources: &["storage.googleapis.com/Bucket"],
        governed_providers: GCP,
        implements: "restrict-public-access",
        frameworks: &[
            // "Ensure that uniform bucket-level access is enabled on Cloud Storage
            // buckets".
            (CIS_GCP_FOUNDATION_V2_0_0, "5.2"),
            // "Access enforcement".
            (NIST_SP_800_53_R5, "AC-3"),
        ],
    },
    // --- service control policies: preventive controls ----------------------
    ControlSeed {
        slug: "restrict-root-user-access-keys",
        name: "Disallow the creation of access keys for the root user",
        description: "Prevents the creation of access keys for the Amazon Web Services \
                      account root user.",
        behavior: PREVENTIVE,
        severity: CRITICAL,
        implementation_type: SERVICE_CONTROL_POLICY,
        // A service control policy is the control; it carries no identifier of its own.
        implementation_identifier: None,
        control_tower_identifier: Some("AWS-GR_RESTRICT_ROOT_USER_ACCESS_KEYS"),
        designators: &["CT.IAM.PV.1"],
        // A control implemented by a service control policy governs every Region.
        scope: GLOBAL,
        deployable_regions: NO_REGIONS,
        parameters: &[("ExemptedPrincipalArns", OPTIONAL)],
        governed_resources: &["AWS::IAM::AccessKey"],
        governed_providers: AWS,
        implements: "privileged-access-approval",
        frameworks: &[
            // "Ensure no 'root' user account access key exists".
            (CIS_AWS_FOUNDATIONS_V3_0_0, "1.4"),
            // "Least privilege".
            (NIST_SP_800_53_R5, "AC-6"),
            // "Group, shared and generic accounts are managed".
            (PCI_DSS_V4_0, "8.2.2"),
        ],
    },
    ControlSeed {
        slug: "deny-public-rds-snapshots",
        name: "Disallow public Amazon RDS database snapshots",
        description: "Prevents an Amazon RDS database snapshot from being shared publicly.",
        behavior: PREVENTIVE,
        severity: HIGH,
        implementation_type: SERVICE_CONTROL_POLICY,
        implementation_identifier: None,
        control_tower_identifier: None,
        designators: &["CT.RDS.PV.4"],
        scope: GLOBAL,
        deployable_regions: NO_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::RDS::DBSnapshot"],
        governed_providers: AWS,
        implements: "restrict-public-access",
        frameworks: &[
            // "Access enforcement".
            (NIST_SP_800_53_R5, "AC-3"),
            // "Configure data access control lists".
            (CIS_CONTROLS_V8_0, "3.3"),
        ],
    },
    ControlSeed {
        slug: "disallow-cloudtrail-changes",
        name: "Disallow changes to CloudTrail configuration",
        description: "Prevents an account from stopping, deleting or reconfiguring the \
                      CloudTrail trail the organization set up.",
        behavior: PREVENTIVE,
        severity: HIGH,
        implementation_type: SERVICE_CONTROL_POLICY,
        implementation_identifier: None,
        control_tower_identifier: Some("AWS-GR_CLOUDTRAIL_CHANGE_PROHIBITED"),
        designators: NO_DESIGNATORS,
        scope: GLOBAL,
        deployable_regions: NO_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::CloudTrail::Trail"],
        governed_providers: AWS,
        implements: "log-integrity-protection",
        frameworks: &[
            // "Protection of audit information".
            (NIST_SP_800_53_R5, "AU-9"),
            // "Audit/log records are determined, documented, implemented and
            // reviewed".
            (NIST_CSF_V1_1, "PR.PT-1"),
        ],
    },
    ControlSeed {
        slug: "disallow-log-archive-deletion",
        name: "Disallow deletion of the log archive bucket",
        description: "Prevents deletion of the Amazon S3 bucket the organization archives \
                      its audit logs in.",
        behavior: PREVENTIVE,
        severity: HIGH,
        implementation_type: SERVICE_CONTROL_POLICY,
        implementation_identifier: None,
        control_tower_identifier: Some("AWS-GR_AUDIT_BUCKET_DELETION_PROHIBITED"),
        designators: NO_DESIGNATORS,
        scope: GLOBAL,
        deployable_regions: NO_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::S3::Bucket"],
        governed_providers: AWS,
        implements: "log-retention-enforcement",
        frameworks: &[
            // "Audit record retention".
            (NIST_SP_800_53_R5, "AU-11"),
            // "Audit log history is retained for at least 12 months".
            (PCI_DSS_V4_0, "10.5.1"),
        ],
    },
    // --- CloudFormation hooks: proactive controls ---------------------------
    ControlSeed {
        slug: "require-encrypted-ebs-volumes",
        name: "Require encryption on Amazon EBS volumes at creation",
        description: "Rejects a CloudFormation deployment that would create an unencrypted \
                      Amazon EBS volume.",
        behavior: PROACTIVE,
        severity: MEDIUM,
        implementation_type: CLOUDFORMATION_HOOK,
        implementation_identifier: Some("AWS.EBS.EncryptionHook"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: &["eu-central-1", "us-east-1"],
        parameters: &[
            ("AllowedRegions", REQUIRED),
            ("ExemptedResourceArns", OPTIONAL),
        ],
        governed_resources: &["AWS::EC2::Volume", "AWS::EC2::Instance"],
        governed_providers: AWS,
        implements: "encrypt-data-at-rest-in-storage-services",
        frameworks: &[
            // "Protection of information at rest".
            (NIST_SP_800_53_R5, "SC-28"),
            // "Ensure EBS volume encryption is enabled".
            (CIS_AWS_FOUNDATIONS_V3_0_0, "2.2.1"),
        ],
    },
    ControlSeed {
        slug: "require-rds-storage-encryption-at-creation",
        name: "Require storage encryption on Amazon RDS instances at creation",
        description: "Rejects a CloudFormation deployment that would create an Amazon RDS \
                      database instance with unencrypted storage.",
        behavior: PROACTIVE,
        severity: MEDIUM,
        implementation_type: CLOUDFORMATION_HOOK,
        implementation_identifier: Some("AWS.RDS.StorageEncryptionHook"),
        control_tower_identifier: None,
        designators: NO_DESIGNATORS,
        scope: REGIONAL,
        deployable_regions: CORE_REGIONS,
        parameters: NO_PARAMETERS,
        governed_resources: &["AWS::RDS::DBInstance"],
        governed_providers: AWS,
        implements: "encrypt-data-at-rest-in-storage-services",
        frameworks: &[
            // "Data-at-rest is protected".
            (NIST_CSF_V1_1, "PR.DS-1"),
            // "Protection of information at rest".
            (NIST_SP_800_53_R5, "SC-28"),
        ],
    },
];

/// How two published controls relate: `(control slug, related control slug,
/// ControlRelationType)`.
///
/// ALTERNATIVE where either control satisfies the common control the pair implements —
/// the typical case for the same check published on two surfaces — and COMPLEMENTARY
/// where both can be enabled and reinforce one another, as a preventive control does
/// with the detective control that reports what slipped past it. This catalog publishes
/// no pair that conflicts, so it records no MUTUALLY_EXCLUSIVE relation. The seeder
/// records each row in BOTH directions, because a query filters on one control at a
/// time and each side has to report the relation when it is the one queried.
pub(super) const RELATED_CONTROLS: &[(&str, &str, &str)] = &[
    (
        "s3-bucket-server-side-encryption-enabled",
        "securityhub-s3-4",
        "ALTERNATIVE",
    ),
    (
        "s3-bucket-server-side-encryption-enabled",
        "require-encrypted-ebs-volumes",
        "COMPLEMENTARY",
    ),
    (
        "multi-region-cloud-trail-enabled",
        "securityhub-cloudtrail-1",
        "ALTERNATIVE",
    ),
    (
        "cloud-trail-enabled",
        "detect-cloud-trail-enabled-on-member-accounts",
        "COMPLEMENTARY",
    ),
    (
        "multi-region-cloud-trail-enabled",
        "disallow-cloudtrail-changes",
        "COMPLEMENTARY",
    ),
    (
        "iam-root-access-key-check",
        "securityhub-iam-4",
        "ALTERNATIVE",
    ),
    (
        "iam-root-access-key-check",
        "restrict-root-user-access-keys",
        "COMPLEMENTARY",
    ),
    ("iam-user-mfa-enabled", "securityhub-iam-5", "ALTERNATIVE"),
    (
        "encrypted-volumes",
        "require-encrypted-ebs-volumes",
        "COMPLEMENTARY",
    ),
    (
        "rds-storage-encrypted",
        "require-rds-storage-encryption-at-creation",
        "COMPLEMENTARY",
    ),
    (
        "rds-snapshots-public-prohibited",
        "deny-public-rds-snapshots",
        "COMPLEMENTARY",
    ),
    (
        "deny-public-rds-snapshots",
        "gcp-storage-bucket-uniform-access",
        "COMPLEMENTARY",
    ),
    (
        "s3-bucket-public-read-prohibited",
        "s3-bucket-public-write-prohibited",
        "COMPLEMENTARY",
    ),
];

/// The aliases a control is published under.
///
/// An alias is a "human-readable designator, such as SH.S3.1", and "[s]everal aliases
/// can refer to the same control across different Amazon Web Services services or
/// compliance frameworks" (`smithy_query(query="doc", shape="ControlSummary",
/// member="Aliases")`). A designator therefore names the control ON the surface that
/// issues it: the Security Hub designator of a Security Hub control is `SH.` followed
/// by that control's own Security Hub id, and a control Control Tower publishes is
/// designated by its Control Tower identifier. Both are DERIVED here from the
/// identifier they are a function of, which is what keeps one surface's designator off
/// another surface's control: a control implemented by a Config rule cannot wear a
/// Security Hub control's `SH.…` designator, because the Security Hub control is its
/// own row in [`PUBLISHED_CONTROLS`].
pub(super) fn aliases(seed: &ControlSeed) -> Vec<String> {
    let mut aliases: Vec<String> = seed
        .designators
        .iter()
        .map(|designator| (*designator).to_string())
        .collect();
    if seed.implementation_type == SECURITY_HUB_CONTROL {
        if let Some(identifier) = seed.implementation_identifier {
            aliases.push(format!("{SECURITY_HUB_ALIAS_PREFIX}{identifier}"));
        }
    }
    if let Some(identifier) = seed.control_tower_identifier {
        aliases.push(identifier.to_string());
    }
    aliases
}

/// The modeled `ParameterRequirementSummary` for a control that supports `parameters`.
///
/// It is "[a] summary that indicates whether the control requires parameters, accepts
/// optional parameters, or does not support parameters" (`smithy_query(query="doc",
/// shape="GetControlResponse", member="ParameterRequirementSummary")`), and the modeled
/// enum admits exactly `REQUIRED`, `OPTIONAL` and `NONE`. It is therefore a function of
/// the control's `Parameters` list, not an independent fact about the control: NONE when
/// the control supports no parameter, REQUIRED when at least one must be supplied, and
/// OPTIONAL when it accepts parameters and none is required.
pub(super) fn parameter_requirement_summary(parameters: &[(&str, &str)]) -> &'static str {
    if parameters.is_empty() {
        "NONE"
    } else if parameters
        .iter()
        .any(|(_, requirement)| *requirement == REQUIRED)
    {
        "REQUIRED"
    } else {
        "OPTIONAL"
    }
}
