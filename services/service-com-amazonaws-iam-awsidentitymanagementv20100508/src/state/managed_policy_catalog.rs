// SPDX-License-Identifier: CC0-1.0

//! AWS's own catalog of managed policies — the `arn:aws:iam::aws:policy/` namespace
//! every account reads and attaches from, and the resolution of an ARN in it.
//!
//! A policy AWS owns is not account state (nothing creates or deletes one), which is
//! why the catalog is a constant here rather than a map in
//! [`ComAmazonawsIamAwsidentitymanagementv20100508State`].

use super::*;

/// One entry of AWS's own catalog of managed policies — a policy AWS owns and
/// every account can read and attach, addressed under the
/// `arn:aws:iam::aws:policy/` namespace ([`AWS_MANAGED_POLICY_ACCOUNT`]).
///
/// An AWS-managed policy is not account state: no call creates or deletes one, so
/// an entry carries no counter and no minted id. Its identity is its path and
/// friendly name (which is what its ARN spells, [`Self::arn`]), its id is catalog
/// data fixed per entry ([`Self::policy_id`]), and its content is the ONE version
/// this emulator carries — the default version, reported under
/// [`FIRST_POLICY_VERSION_ID`].
///
/// AWS publishes a managed policy's own default version number (`v3`, `v12`, …) and
/// bumps it as it updates the policy; the number it is at right now is not something
/// this emulator can know, so the catalog promises only the invariant the API has:
/// `GetPolicy`'s `DefaultVersionId` names the version `GetPolicyVersion` serves.
/// AWS's friendly `Description` prose is not carried either — the catalog holds a
/// policy's identity and document, so `Description` is reported only for a policy
/// this ACCOUNT created with one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AwsManagedPolicy {
    /// The policy's path, beginning and ending with `/`: `/` for a policy at the
    /// root, `/service-role/` for one AWS files under that path.
    pub path: &'static str,
    /// The policy's friendly name, as the AWS Managed Policy Reference spells it.
    pub policy_name: &'static str,
    /// This entry's `PolicyId`, carried as catalog data beside the path, name and
    /// document.
    ///
    /// Stable and unique, the two properties `Policy$PolicyId` documents ("the stable
    /// and unique string identifying the policy"): it is fixed per entry, so every
    /// read in every run reports the same id for the same policy, and the suite
    /// asserts the catalog's ids are distinct. AWS's own `PolicyId` for a managed
    /// policy is not published in the reference the catalog copies from, so this is a
    /// value the EMULATOR assigns, of the modelled `idType` (len 16..128, `^[\w]+$`).
    /// Every one carries `_`, which the base32 alphabet a MINTED id is drawn from does
    /// not ([`unique_id`]), so a catalog id can never collide with the id the account's
    /// own `CreatePolicy` mints.
    pub policy_id: &'static str,
    /// The policy's default version document, an IAM JSON policy.
    pub document: &'static str,
}

impl AwsManagedPolicy {
    /// This entry's ARN: `arn:aws:iam::aws:policy<path><name>`.
    ///
    /// Built here rather than through `engine::emulator_arn` because that helper
    /// always writes the emulator's own account id into the account segment, and an
    /// AWS-managed policy is owned by AWS: its account segment is the literal
    /// [`AWS_MANAGED_POLICY_ACCOUNT`]. The rest of the ARN is the same grammar
    /// [`policy_arn`] builds and [`parse_policy_arn`] reads.
    pub fn arn(&self) -> String {
        format!(
            "arn:aws:iam::{AWS_MANAGED_POLICY_ACCOUNT}:policy{}{}",
            self.path, self.policy_name
        )
    }

    /// Whether this policy may be attached to an IAM user, group or role —
    /// `Policy$IsAttachable`, "specifies whether the policy can be attached to an IAM
    /// user, group, or role".
    ///
    /// AWS's catalog holds two kinds of policy. Most are published to be attached, and
    /// report `true`. The ones AWS files under the path it reserves for service-linked
    /// roles ([`SERVICE_LINKED_ROLE_PATH_PREFIX`]) belong to the service-linked role
    /// they were written for — "Amazon Web Services service role policies can only be
    /// attached to the service-linked role for that service"
    /// (`PolicyNotAttachableException`) — so no identity may take one as a permissions
    /// policy or as a permissions boundary, and they report `false`.
    ///
    /// DERIVED from the path rather than carried per entry, so the flag a reader reports
    /// and the refusal an attach or a boundary put answers
    /// ([`ComAmazonawsIamAwsidentitymanagementv20100508State::policy_is_attachable`])
    /// cannot disagree about one policy.
    pub fn is_attachable(&self) -> bool {
        !is_service_linked_role_path(self.path)
    }

    /// This entry as a managed policy record: the `Policy` metadata every reader
    /// reports, plus the single version `GetPolicyVersion` serves.
    ///
    /// The two counts are the caller's, not the catalog's — they are what the
    /// account's own attachments and permissions boundaries make them, which is why
    /// they are passed in (see
    /// [`ComAmazonawsIamAwsidentitymanagementv20100508State::resolve_policy`]).
    pub(super) fn record(&self, attachment_count: i32, permissions_boundary_usage_count: i32) -> ManagedPolicy {
        let created = ::aws_smithy_types::DateTime::from_secs(AWS_MANAGED_POLICY_CREATE_DATE_SECS);
        let version = PolicyVersion {
            document: Some(self.document.to_string()),
            version_id: Some(FIRST_POLICY_VERSION_ID.to_string()),
            is_default_version: true,
            create_date: Some(created),
        };
        ManagedPolicy {
            policy: Policy {
                arn: Some(self.arn()),
                policy_id: Some(self.policy_id.to_string()),
                policy_name: Some(self.policy_name.to_string()),
                path: Some(self.path.to_string()),
                default_version_id: Some(FIRST_POLICY_VERSION_ID.to_string()),
                attachment_count: Some(attachment_count),
                permissions_boundary_usage_count: Some(permissions_boundary_usage_count),
                // "Specifies whether the policy can be attached to an IAM user,
                // group, or role" (`Policy$IsAttachable`): see [`Self::is_attachable`]
                // for the two kinds of entry AWS's catalog holds.
                is_attachable: self.is_attachable(),
                // See the type's documentation: AWS's prose is not carried.
                description: None,
                create_date: Some(created),
                update_date: Some(created),
                // An AWS-managed policy carries no tags: tagging is an account
                // operation on an account's own resource.
                tags: None,
            },
            versions: [(FIRST_POLICY_VERSION_ID.to_string(), version)]
                .into_iter()
                .collect(),
        }
    }
}

/// The AWS-managed policies this emulator carries, ordered by ARN.
///
/// AWS-managed policies "are created and managed by AWS" and exist in every
/// account, so a read of one is NOT a read of account state — which is why they
/// live in this constant rather than in
/// [`ComAmazonawsIamAwsidentitymanagementv20100508State::policies`], the account's
/// own policy store.
///
/// SOURCE: the AWS Managed Policy Reference ("AWS managed policies", IAM User
/// Guide), which publishes each policy's path, friendly name and default-version
/// document. AWS's catalog runs to well over a thousand policies and this is a
/// SUBSET of it — the policies the emulator's own suites and fixtures attach, plus
/// the account-wide and job-function ones a client is most likely to read, and the
/// service-linked-role policies that are published but NOT attachable
/// ([`AwsManagedPolicy::is_attachable`]), so both kinds of entry a client can meet
/// are discoverable here.
///
/// It also carries every AWS-managed policy ARN another service of this emulator
/// hands a caller as a recommendation: a control plane that names a policy ARN in a
/// response must be able to describe that ARN, so an identifier this emulator
/// publishes is never one its own API reports as missing. The EKS add-on catalog's
/// `podIdentityConfiguration[].recommendedManagedPolicies` is the one place that
/// does so today, and the five policies it names are entries here.
///
/// An `arn:aws:iam::aws:policy/` ARN this catalog does not carry resolves to no
/// policy, and a read addressed by such an ARN is refused as the missing entity
/// `NoSuchEntityException` names — the operations' modelled refusal — rather than
/// with a code none of them declares.
///
/// Where the catalog carries a policy, it is authoritative about WHERE that policy
/// lives: an entry's path is the one AWS publishes it at, and a friendly name
/// identifies one policy ([`aws_managed_policy_named`]), so the same name under any
/// other path names nothing — for an attachment and a permissions boundary as well as
/// for a read ([`is_published_aws_managed_policy_arn`]). Adding an entry therefore adds
/// both facts: that AWS publishes the policy, and that it publishes it there.
pub const AWS_MANAGED_POLICIES: &[AwsManagedPolicy] = &[
    AwsManagedPolicy {
        path: "/",
        policy_name: "AWSLakeFormationDataAdmin",
        policy_id: "ANPA_AWSLAKEFORMATIONDATAADMIN",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["lakeformation:*","cloudtrail:DescribeTrails","cloudtrail:LookupEvents","glue:GetDatabase","glue:GetDatabases","glue:CreateDatabase","glue:UpdateDatabase","glue:DeleteDatabase","glue:GetConnections","glue:SearchTables","glue:GetTable","glue:CreateTable","glue:UpdateTable","glue:DeleteTable","glue:GetTableVersions","glue:GetPartitions","glue:GetTables","glue:ListWorkflows","glue:BatchGetWorkflows","glue:DeleteWorkflow","glue:GetWorkflows","glue:GetTags","glue:TagResource","glue:UnTagResource","iam:ListUsers","iam:ListRoles","iam:GetRole","iam:GetRolePolicy","s3:ListAllMyBuckets","s3:GetBucketLocation"],"Resource":"*"},{"Effect":"Deny","Action":["lakeformation:PutDataLakeSettings"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "AWSLambdaExecute",
        policy_id: "ANPA_AWSLAMBDAEXECUTE",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["logs:*"],"Resource":"arn:aws:logs:*:*:*"},{"Effect":"Allow","Action":["s3:GetObject","s3:PutObject"],"Resource":"arn:aws:s3:::*"}]}"#,
    },
    // Recommended by the EKS add-on catalog for the `amazon-cloudwatch-observability`
    // add-on's `cloudwatch-agent` service account, beside
    // `CloudWatchAgentServerPolicy`.
    AwsManagedPolicy {
        path: "/",
        policy_name: "AWSXrayWriteOnlyAccess",
        policy_id: "ANPA_AWSXRAYWRITEONLYACCESS",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["xray:PutTraceSegments","xray:PutTelemetryRecords","xray:GetSamplingRules","xray:GetSamplingTargets","xray:GetSamplingStatisticSummaries"],"Resource":["*"]}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "AdministratorAccess",
        policy_id: "ANPA_ADMINISTRATORACCESS",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"*","Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonEC2ReadOnlyAccess",
        policy_id: "ANPA_AMAZONEC2READONLYACCESS",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"ec2:Describe*","Resource":"*"},{"Effect":"Allow","Action":"elasticloadbalancing:Describe*","Resource":"*"},{"Effect":"Allow","Action":["cloudwatch:ListMetrics","cloudwatch:GetMetricStatistics","cloudwatch:Describe*"],"Resource":"*"},{"Effect":"Allow","Action":"autoscaling:Describe*","Resource":"*"}]}"#,
    },
    // The policy an EKS cluster's own service role carries, published at the account
    // root: the grant the control plane manages a cluster's EC2 network interfaces,
    // load balancers and volumes through.
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonEKSClusterPolicy",
        policy_id: "ANPA_AMAZONEKSCLUSTERPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["autoscaling:DescribeAutoScalingGroups","autoscaling:UpdateAutoScalingGroup","ec2:AttachVolume","ec2:AuthorizeSecurityGroupIngress","ec2:CreateRoute","ec2:CreateSecurityGroup","ec2:CreateTags","ec2:CreateVolume","ec2:DeleteRoute","ec2:DeleteSecurityGroup","ec2:DeleteVolume","ec2:Describe*","ec2:DetachVolume","ec2:ModifyInstanceAttribute","ec2:ModifyVolume","ec2:RevokeSecurityGroupIngress","elasticloadbalancing:Add*","elasticloadbalancing:Apply*","elasticloadbalancing:Attach*","elasticloadbalancing:Configure*","elasticloadbalancing:Create*","elasticloadbalancing:Delete*","elasticloadbalancing:Deregister*","elasticloadbalancing:Describe*","elasticloadbalancing:Detach*","elasticloadbalancing:Modify*","elasticloadbalancing:Register*","elasticloadbalancing:Set*","kms:DescribeKey"],"Resource":"*"},{"Effect":"Allow","Action":"iam:CreateServiceLinkedRole","Resource":"*","Condition":{"StringEquals":{"iam:AWSServiceName":"elasticloadbalancing.amazonaws.com"}}}]}"#,
    },
    // Recommended by the EKS add-on catalog for the `vpc-cni` add-on's `aws-node`
    // service account.
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonEKS_CNI_Policy",
        policy_id: "ANPA_AMAZONEKS_CNI_POLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:AssignPrivateIpAddresses","ec2:AttachNetworkInterface","ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeInstances","ec2:DescribeTags","ec2:DescribeNetworkInterfaces","ec2:DescribeInstanceTypes","ec2:DescribeSubnets","ec2:DetachNetworkInterface","ec2:ModifyNetworkInterfaceAttribute","ec2:UnassignPrivateIpAddresses"],"Resource":"*"},{"Effect":"Allow","Action":["ec2:CreateTags"],"Resource":["arn:aws:ec2:*:*:network-interface/*"]}]}"#,
    },
    // The placement-group grant an EMR cluster's role carries, published at the
    // account root — NOT under `/service-role/`, where the legacy EMR service-role
    // policy below lives.
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonElasticMapReducePlacementGroupPolicy",
        policy_id: "ANPA_AMAZONELASTICMAPREDUCEPLACEMENTGROUPPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreatePlacementGroup","ec2:DeletePlacementGroup","ec2:DescribePlacementGroups"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonS3FullAccess",
        policy_id: "ANPA_AMAZONS3FULLACCESS",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:*","s3-object-lambda:*"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonS3ReadOnlyAccess",
        policy_id: "ANPA_AMAZONS3READONLYACCESS",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:Get*","s3:List*","s3:Describe*","s3-object-lambda:Get*","s3-object-lambda:List*"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonSNSFullAccess",
        policy_id: "ANPA_AMAZONSNSFULLACCESS",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["sns:*"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonSQSFullAccess",
        policy_id: "ANPA_AMAZONSQSFULLACCESS",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["sqs:*"],"Resource":"*"}]}"#,
    },
    // Recommended by the EKS add-on catalog for the
    // `amazon-cloudwatch-observability` add-on's `cloudwatch-agent` service account.
    AwsManagedPolicy {
        path: "/",
        policy_name: "CloudWatchAgentServerPolicy",
        policy_id: "ANPA_CLOUDWATCHAGENTSERVERPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:PutMetricData","ec2:DescribeVolumes","ec2:DescribeTags","logs:PutLogEvents","logs:PutRetentionPolicy","logs:DescribeLogStreams","logs:DescribeLogGroups","logs:CreateLogStream","logs:CreateLogGroup","xray:PutTraceSegments","xray:PutTelemetryRecords","xray:GetSamplingRules","xray:GetSamplingTargets","xray:GetSamplingStatisticSummaries"],"Resource":"*"},{"Effect":"Allow","Action":["ssm:GetParameter"],"Resource":"arn:aws:ssm:*:*:parameter/AmazonCloudWatch-*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "CloudWatchLogsFullAccess",
        policy_id: "ANPA_CLOUDWATCHLOGSFULLACCESS",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["logs:*"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "IAMFullAccess",
        policy_id: "ANPA_IAMFULLACCESS",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iam:*","organizations:DescribeAccount","organizations:DescribeOrganization","organizations:DescribeOrganizationalUnit","organizations:DescribePolicy","organizations:ListChildren","organizations:ListParents","organizations:ListPoliciesForTarget","organizations:ListRoots","organizations:ListPolicies","organizations:ListTargetsForPolicy"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "IAMReadOnlyAccess",
        policy_id: "ANPA_IAMREADONLYACCESS",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iam:GenerateCredentialReport","iam:GenerateServiceLastAccessedDetails","iam:Get*","iam:List*","iam:SimulateCustomPolicy","iam:SimulatePrincipalPolicy"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "PowerUserAccess",
        policy_id: "ANPA_POWERUSERACCESS",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","NotAction":["iam:*","organizations:*","account:*"],"Resource":"*"},{"Effect":"Allow","Action":["iam:CreateServiceLinkedRole","iam:DeleteServiceLinkedRole","iam:ListRoles","organizations:DescribeOrganization","account:ListRegions","account:GetAccountInformation"],"Resource":"*"}]}"#,
    },
    // The policy a client most often resolves by name: "ReadOnlyAccess" grants the read
    // verbs of EVERY service. AWS's own default version enumerates them service by
    // service and runs far past the character quota a managed policy's document is held
    // to here ([`MAX_MANAGED_POLICY_CHARACTERS`]), so the document carried for it states
    // the same grant in the form the IAM action grammar already has for "an action of
    // every service": the read verb families AWS's document is built out of, each over
    // the whole namespace position, beside the two policy-simulation actions AWS's names
    // outside those families. A reader of this document therefore sees the services a
    // read-only principal can reach as AWS's document puts them in reach — every one —
    // rather than the handful a hand-written namespace list named. The policy's identity
    // — its path, name, ARN and attachability — is AWS's.
    AwsManagedPolicy {
        path: "/",
        policy_name: "ReadOnlyAccess",
        policy_id: "ANPA_READONLYACCESS",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["*:BatchGet*","*:Describe*","*:Get*","*:List*","*:Lookup*","*:Query*","*:Scan*","*:Search*","*:View*"],"Resource":"*"},{"Effect":"Allow","Action":["iam:SimulateCustomPolicy","iam:SimulatePrincipalPolicy"],"Resource":"*"}]}"#,
    },
    // The policies AWS files under the path it reserves for service-linked roles.
    // Each belongs to the service-linked role it was written for, so every one of
    // them reports `IsAttachable` false ([`AwsManagedPolicy::is_attachable`]) and no
    // identity can attach it or be bounded by it. A client provisioning a
    // service-linked role resolves the policy of the service it is provisioning FOR by
    // listing this family, so the family is carried rather than the one or two entries
    // a single fixture happens to name.
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSBackupServiceLinkedRolePolicyForBackup",
        policy_id: "ANPA_AWSSERVICEROLE_AWSBACKUPSERVICELINKEDROLEPOLICYFORBACKUP",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["backup:DescribeBackupVault","backup:ListBackupVaults","dynamodb:DescribeTable","ec2:CopySnapshot","ec2:CreateSnapshot","ec2:DeleteSnapshot","ec2:DescribeSnapshots","ec2:DescribeVolumes","elasticfilesystem:DescribeFilesystems","rds:DescribeDBInstances","rds:DescribeDBSnapshots","storagegateway:DescribeCachediSCSIVolumes","tag:GetResources"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSConfigServiceRolePolicy",
        policy_id: "ANPA_AWSSERVICEROLE_AWSCONFIGSERVICEROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudtrail:DescribeTrails","cloudtrail:GetTrailStatus","cloudwatch:DescribeAlarms","config:Put*","dynamodb:DescribeTable","ec2:Describe*","elasticloadbalancing:DescribeLoadBalancers","iam:GetRole","iam:ListRoles","lambda:GetFunction","lambda:ListFunctions","organizations:DescribeOrganization","rds:DescribeDBInstances","s3:GetBucketAcl","s3:GetBucketLocation","s3:ListAllMyBuckets","tag:GetResources"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSEC2SpotServiceRolePolicy",
        policy_id: "ANPA_AWSSERVICEROLE_AWSEC2SPOTSERVICEROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateTags","ec2:DescribeInstances","ec2:DescribeInstanceStatus","ec2:RunInstances","ec2:StartInstances","ec2:StopInstances","ec2:TerminateInstances"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSElasticBeanstalkServiceRolePolicy",
        policy_id: "ANPA_AWSSERVICEROLE_AWSELASTICBEANSTALKSERVICEROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["autoscaling:Describe*","cloudformation:Describe*","cloudformation:Get*","cloudformation:List*","cloudwatch:DescribeAlarms","cloudwatch:PutMetricAlarm","ec2:Describe*","elasticloadbalancing:Describe*","logs:DescribeLogGroups","logs:DescribeLogStreams","s3:GetObject","s3:ListBucket","sns:ListSubscriptionsByTopic","sqs:GetQueueAttributes"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSLambdaReplicator",
        policy_id: "ANPA_AWSSERVICEROLE_AWSLAMBDAREPLICATOR",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["lambda:CreateFunction","lambda:DeleteFunction","lambda:DisableReplication","lambda:EnableReplication","lambda:GetFunction","lambda:UpdateFunctionCode","lambda:UpdateFunctionConfiguration"],"Resource":"arn:aws:lambda:*:*:function:*"},{"Effect":"Allow","Action":["iam:PassRole"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSOrganizationsServiceTrustPolicy",
        policy_id: "ANPA_AWSSERVICEROLE_AWSORGANIZATIONSSERVICETRUSTPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["organizations:DescribeAccount","organizations:DescribeOrganization","organizations:DescribeOrganizationalUnit","organizations:ListAccounts","organizations:ListAccountsForParent","organizations:ListChildren","organizations:ListParents","organizations:ListRoots"],"Resource":"*"},{"Effect":"Allow","Action":["iam:DeleteRole"],"Resource":"arn:aws:iam::*:role/aws-service-role/organizations.amazonaws.com/*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSSSOServiceRolePolicy",
        policy_id: "ANPA_AWSSERVICEROLE_AWSSSOSERVICEROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iam:AttachRolePolicy","iam:CreateRole","iam:CreateSAMLProvider","iam:DeleteRole","iam:DeleteRolePolicy","iam:DeleteSAMLProvider","iam:DetachRolePolicy","iam:GetRole","iam:GetSAMLProvider","iam:ListAttachedRolePolicies","iam:ListRolePolicies","iam:PutRolePolicy","iam:UpdateSAMLProvider","organizations:DescribeAccount","organizations:DescribeOrganization","organizations:ListAccounts"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSSupportServiceRolePolicy",
        policy_id: "ANPA_AWSSERVICEROLE_AWSSUPPORTSERVICEROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["support:*","trustedadvisor:Describe*","trustedadvisor:Get*","trustedadvisor:List*","cloudformation:DescribeStacks","cloudwatch:DescribeAlarms","ec2:DescribeInstances","iam:GetRole","iam:ListRoles","s3:ListAllMyBuckets"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSTrustedAdvisorServiceRolePolicy",
        policy_id: "ANPA_AWSSERVICEROLE_AWSTRUSTEDADVISORSERVICEROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["autoscaling:Describe*","cloudformation:DescribeStacks","cloudtrail:DescribeTrails","cloudwatch:GetMetricStatistics","ec2:Describe*","elasticloadbalancing:Describe*","iam:GenerateCredentialReport","iam:GetAccountSummary","iam:ListAccessKeys","kms:DescribeKey","rds:DescribeDBInstances","route53:ListHostedZones","s3:GetBucketAcl","s3:GetBucketPolicy","s3:ListAllMyBuckets","trustedadvisor:Describe*"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSVPCTransitGatewayServiceRolePolicy",
        policy_id: "ANPA_AWSSERVICEROLE_AWSVPCTRANSITGATEWAYSERVICEROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeNetworkInterfaces","ec2:DescribeSubnets","ec2:DescribeVpcs","ec2:ModifyNetworkInterfaceAttribute"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonECSServiceRolePolicy",
        policy_id: "ANPA_AWSSERVICEROLE_AMAZONECSSERVICEROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:AttachNetworkInterface","ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeInstances","ec2:DescribeNetworkInterfaces","ec2:DescribeSubnets","ec2:DescribeVpcs","ec2:DetachNetworkInterface","elasticloadbalancing:DeregisterTargets","elasticloadbalancing:DescribeTargetGroups","elasticloadbalancing:RegisterTargets","logs:CreateLogStream","logs:PutLogEvents","route53:ChangeResourceRecordSets","route53:ListHostedZonesByName"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonEKSServiceRolePolicy",
        policy_id: "ANPA_AWSSERVICEROLE_AMAZONEKSSERVICEROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateNetworkInterface","ec2:CreateNetworkInterfacePermission","ec2:DeleteNetworkInterface","ec2:DescribeInstances","ec2:DescribeNetworkInterfaces","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeVpcs","ec2:ModifyNetworkInterfaceAttribute","elasticloadbalancing:DescribeLoadBalancers","elasticloadbalancing:DescribeTargetGroups","iam:ListAttachedRolePolicies","kms:CreateGrant","kms:DescribeKey"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonElasticFileSystemServiceRolePolicy",
        policy_id: "ANPA_AWSSERVICEROLE_AMAZONELASTICFILESYSTEMSERVICEROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["backup:DescribeBackupVault","ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeNetworkInterfaceAttribute","ec2:DescribeNetworkInterfaces","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:ModifyNetworkInterfaceAttribute","tag:GetResources"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonElasticsearchServiceRolePolicy",
        policy_id: "ANPA_AWSSERVICEROLE_AMAZONELASTICSEARCHSERVICEROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeNetworkInterfaces","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeVpcs","ec2:ModifyNetworkInterfaceAttribute","elasticloadbalancing:DescribeLoadBalancers"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonGuardDutyServiceRolePolicy",
        policy_id: "ANPA_AWSSERVICEROLE_AMAZONGUARDDUTYSERVICEROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudtrail:DescribeTrails","cloudtrail:GetTrailStatus","ec2:DescribeInstances","ec2:DescribeNetworkInterfaces","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeVpcs","iam:ListAccountAliases","organizations:DescribeAccount","organizations:DescribeOrganization","organizations:ListAccounts","s3:GetBucketPublicAccessBlock","s3:GetEncryptionConfiguration"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonInspectorServiceRolePolicy",
        policy_id: "ANPA_AWSSERVICEROLE_AMAZONINSPECTORSERVICEROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["directconnect:DescribeConnections","directconnect:DescribeVirtualGateways","ec2:DescribeInstances","ec2:DescribeNetworkAcls","ec2:DescribeNetworkInterfaces","ec2:DescribeRouteTables","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeTags","ec2:DescribeVpcs","elasticloadbalancing:DescribeLoadBalancers","elasticloadbalancing:DescribeTags"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonMacieServiceRolePolicy",
        policy_id: "ANPA_AWSSERVICEROLE_AMAZONMACIESERVICEROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudtrail:DescribeTrails","cloudtrail:GetTrailStatus","organizations:DescribeAccount","organizations:DescribeOrganization","s3:GetBucketAcl","s3:GetBucketLocation","s3:GetBucketPolicy","s3:GetBucketPublicAccessBlock","s3:GetEncryptionConfiguration","s3:GetObject","s3:ListAllMyBuckets","s3:ListBucket"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonRDSServiceRolePolicy",
        policy_id: "ANPA_AWSSERVICEROLE_AMAZONRDSSERVICEROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:PutMetricData","ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeNetworkInterfaces","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeVpcs","logs:CreateLogGroup","logs:CreateLogStream","logs:PutLogEvents","logs:PutRetentionPolicy","sns:Publish"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonRedshiftServiceLinkedRolePolicy",
        policy_id: "ANPA_AWSSERVICEROLE_AMAZONREDSHIFTSERVICELINKEDROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:AssignIpv6Addresses","ec2:AssignPrivateIpAddresses","ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeAddresses","ec2:DescribeNetworkInterfaces","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeVpcs","ec2:ModifyNetworkInterfaceAttribute","ec2:UnassignIpv6Addresses","ec2:UnassignPrivateIpAddresses"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonSSMServiceRolePolicy",
        policy_id: "ANPA_AWSSERVICEROLE_AMAZONSSMSERVICEROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudformation:DescribeStacks","ec2:DescribeInstances","ec2:DescribeInstanceStatus","resource-groups:GetGroup","resource-groups:ListGroupResources","ssm:DescribeInstanceInformation","ssm:GetAutomationExecution","ssm:ListCommands","ssm:SendCommand","ssm:StartAutomationExecution","states:DescribeExecution","tag:GetResources"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AutoScalingServiceRolePolicy",
        policy_id: "ANPA_AWSSERVICEROLE_AUTOSCALINGSERVICEROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:DeleteAlarms","cloudwatch:DescribeAlarms","cloudwatch:PutMetricAlarm","ec2:AttachClassicLinkVpc","ec2:CreateTags","ec2:DescribeInstances","ec2:DescribeInstanceStatus","ec2:DescribeSubnets","ec2:RunInstances","ec2:TerminateInstances","elasticloadbalancing:DeregisterTargets","elasticloadbalancing:RegisterTargets"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "ConfigConformsServiceRolePolicy",
        policy_id: "ANPA_AWSSERVICEROLE_CONFIGCONFORMSSERVICEROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudformation:CreateStack","cloudformation:DeleteStack","cloudformation:DescribeStackEvents","cloudformation:DescribeStacks","config:DescribeConfigRules","config:PutConfigRule","config:DeleteConfigRule","s3:GetObject","s3:ListBucket"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "DynamoDBReplicationServiceRolePolicy",
        policy_id: "ANPA_AWSSERVICEROLE_DYNAMODBREPLICATIONSERVICEROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["dynamodb:DeleteItem","dynamodb:DescribeTable","dynamodb:GetItem","dynamodb:PutItem","dynamodb:Query","dynamodb:Scan","dynamodb:UpdateItem","dynamodb:DescribeStream","dynamodb:GetRecords","dynamodb:GetShardIterator","dynamodb:ListStreams"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "ElasticLoadBalancingServiceRolePolicy",
        policy_id: "ANPA_AWSSERVICEROLE_ELASTICLOADBALANCINGSERVICEROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:AssignIpv6Addresses","ec2:AssignPrivateIpAddresses","ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeAddresses","ec2:DescribeInstances","ec2:DescribeInternetGateways","ec2:DescribeNetworkInterfaces","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeVpcs","ec2:ModifyNetworkInterfaceAttribute","ec2:UnassignIpv6Addresses","ec2:UnassignPrivateIpAddresses","logs:CreateLogDelivery","logs:DeleteLogDelivery","logs:GetLogDelivery","logs:ListLogDeliveries","logs:UpdateLogDelivery"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "LakeFormationDataAccessServiceRolePolicy",
        policy_id: "ANPA_AWSSERVICEROLE_LAKEFORMATIONDATAACCESSSERVICEROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["glue:GetDatabase","glue:GetDatabases","glue:GetPartitions","glue:GetTable","glue:GetTables","s3:GetBucketLocation","s3:GetObject","s3:ListBucket"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "WAFRegionalLoggingServiceRolePolicy",
        policy_id: "ANPA_AWSSERVICEROLE_WAFREGIONALLOGGINGSERVICEROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["firehose:DescribeDeliveryStream","firehose:PutRecord","firehose:PutRecordBatch"],"Resource":"arn:aws:firehose:*:*:deliverystream/aws-waf-logs-*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "WAFV2LoggingServiceRolePolicy",
        policy_id: "ANPA_AWSSERVICEROLE_WAFV2LOGGINGSERVICEROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["firehose:DescribeDeliveryStream","firehose:PutRecord","firehose:PutRecordBatch"],"Resource":"arn:aws:firehose:*:*:deliverystream/aws-waf-logs-*"}]}"#,
    },
    // The policies AWS files under the path it reserves for JOB FUNCTIONS — the family
    // a client provisioning a human role attaches, resolved by listing the catalog
    // under that path. They are published to be attached, so each reports
    // `IsAttachable` true like every other policy outside the service-linked-role path.
    //
    // AWS publishes the family as a whole, and this is all of it: one entry per job
    // function the AWS Managed Policy Reference files under `/job-function/`. Each
    // document states the grant that job function is published with, stated inside the
    // character quota a carried document is held to
    // ([`MAX_MANAGED_POLICY_CHARACTERS`]) — the services and verb families AWS's own
    // default version names, rather than its full per-action enumeration.
    AwsManagedPolicy {
        path: "/job-function/",
        policy_name: "Billing",
        policy_id: "ANPA_JOBFUNCTION_BILLING",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["account:GetAccountInformation","aws-portal:*","billing:*","budgets:*","ce:*","consolidatedbilling:*","cur:*","freetier:*","invoicing:*","payments:*","pricing:*","purchase-orders:*","savingsplans:Describe*","tax:*"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/job-function/",
        policy_name: "DataScientist",
        policy_id: "ANPA_JOBFUNCTION_DATASCIENTIST",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["athena:*","cloudwatch:GetMetricData","cloudwatch:GetMetricStatistics","datapipeline:Describe*","datapipeline:GetPipelineDefinition","dynamodb:Describe*","dynamodb:List*","ec2:Describe*","elasticmapreduce:*","glue:Get*","glue:List*","iam:GetRole","iam:ListRoles","kinesis:Describe*","kinesis:List*","kms:ListAliases","lambda:ListFunctions","logs:Describe*","logs:Get*","machinelearning:*","rds:Describe*","redshift:Describe*","s3:Get*","s3:List*","sagemaker:*","sdb:*","sns:ListSubscriptions","sns:ListTopics","sqs:ListQueues","tag:GetResources"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/job-function/",
        policy_name: "DatabaseAdministrator",
        policy_id: "ANPA_JOBFUNCTION_DATABASEADMINISTRATOR",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:Describe*","cloudwatch:Get*","cloudwatch:List*","cloudwatch:PutMetricAlarm","datapipeline:*","dax:*","dynamodb:*","ec2:DescribeAccountAttributes","ec2:DescribeAvailabilityZones","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeVpcs","elasticache:*","iam:GetRole","iam:ListRoles","kms:ListAliases","logs:Describe*","logs:Get*","rds:*","redshift:*","s3:GetBucketLocation","s3:ListAllMyBuckets","sns:ListSubscriptions","sns:ListTopics","sqs:ListQueues"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/job-function/",
        policy_name: "NetworkAdministrator",
        policy_id: "ANPA_JOBFUNCTION_NETWORKADMINISTRATOR",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["autoscaling:Describe*","cloudfront:ListDistributions","cloudwatch:DeleteAlarms","cloudwatch:DescribeAlarms","cloudwatch:GetMetricStatistics","cloudwatch:PutMetricAlarm","directconnect:*","ec2:AllocateAddress","ec2:AssociateAddress","ec2:AttachInternetGateway","ec2:AttachVpnGateway","ec2:AuthorizeSecurityGroupEgress","ec2:AuthorizeSecurityGroupIngress","ec2:CreateSecurityGroup","ec2:Describe*","elasticbeanstalk:Describe*","elasticloadbalancing:*","logs:Describe*","logs:Get*","route53:*","route53domains:*","sns:ListSubscriptions","sns:ListTopics"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/job-function/",
        policy_name: "SupportUser",
        policy_id: "ANPA_JOBFUNCTION_SUPPORTUSER",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["acm:Describe*","acm:Get*","acm:List*","autoscaling:Describe*","cloudformation:Describe*","cloudformation:List*","cloudwatch:Describe*","cloudwatch:Get*","cloudwatch:List*","ec2:Describe*","ecs:Describe*","ecs:List*","iam:Get*","iam:List*","logs:Describe*","logs:Get*","rds:Describe*","s3:Get*","s3:List*","support:*","trustedadvisor:Describe*"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/job-function/",
        policy_name: "SystemAdministrator",
        policy_id: "ANPA_JOBFUNCTION_SYSTEMADMINISTRATOR",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["autoscaling:*","cloudtrail:Describe*","cloudtrail:Get*","cloudtrail:List*","cloudwatch:*","ec2:*","ecs:*","elasticloadbalancing:*","events:*","iam:GetRole","iam:ListRoles","iam:PassRole","kms:ListAliases","logs:*","rds:Describe*","route53:Get*","route53:List*","s3:Get*","s3:List*","sns:*","ssm:*"],"Resource":"*"}]}"#,
    },
    // "This policy grants permissions to view resources and basic metadata across all
    // Amazon Web Services services": the same whole-catalog reach as `ReadOnlyAccess`
    // above, narrowed to the listing and describing verbs, which is what separates the
    // two policies.
    AwsManagedPolicy {
        path: "/job-function/",
        policy_name: "ViewOnlyAccess",
        policy_id: "ANPA_JOBFUNCTION_VIEWONLYACCESS",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["*:Describe*","*:List*","*:View*"],"Resource":"*"},{"Effect":"Allow","Action":["s3:GetBucketLocation","s3:GetAccountPublicAccessBlock","tag:GetResources"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSGlueServiceRole",
        policy_id: "ANPA_SERVICEROLE_AWSGLUESERVICEROLE",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["glue:*","s3:GetBucketLocation","s3:ListBucket","s3:ListAllMyBuckets","s3:GetBucketAcl","ec2:DescribeVpcEndpoints","ec2:DescribeRouteTables","ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeNetworkInterfaces","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeVpcAttribute","iam:ListRolePolicies","iam:GetRole","iam:GetRolePolicy","cloudwatch:PutMetricData"],"Resource":["*"]},{"Effect":"Allow","Action":["s3:CreateBucket"],"Resource":["arn:aws:s3:::aws-glue-*"]},{"Effect":"Allow","Action":["s3:GetObject","s3:PutObject","s3:DeleteObject"],"Resource":["arn:aws:s3:::aws-glue-*/*","arn:aws:s3:::*/*aws-glue-*/*"]},{"Effect":"Allow","Action":["s3:GetObject"],"Resource":["arn:aws:s3:::crawler-public*","arn:aws:s3:::aws-glue-*"]},{"Effect":"Allow","Action":["logs:CreateLogGroup","logs:CreateLogStream","logs:PutLogEvents"],"Resource":["arn:aws:logs:*:*:/aws-glue/*"]}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSLambdaBasicExecutionRole",
        policy_id: "ANPA_SERVICEROLE_AWSLAMBDABASICEXECUTIONROLE",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["logs:CreateLogGroup","logs:CreateLogStream","logs:PutLogEvents"],"Resource":"*"}]}"#,
    },
    // Recommended by the EKS add-on catalog for the `aws-ebs-csi-driver` add-on's
    // `ebs-csi-controller-sa` service account. The conditions are the published
    // policy's: the driver may create and delete only the volumes and snapshots it
    // tagged for itself.
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonEBSCSIDriverPolicy",
        policy_id: "ANPA_SERVICEROLE_AMAZONEBSCSIDRIVERPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateSnapshot","ec2:AttachVolume","ec2:DetachVolume","ec2:ModifyVolume","ec2:DescribeAvailabilityZones","ec2:DescribeInstances","ec2:DescribeSnapshots","ec2:DescribeTags","ec2:DescribeVolumes","ec2:DescribeVolumesModifications"],"Resource":"*"},{"Effect":"Allow","Action":["ec2:CreateTags"],"Resource":["arn:aws:ec2:*:*:volume/*","arn:aws:ec2:*:*:snapshot/*"],"Condition":{"StringEquals":{"ec2:CreateAction":["CreateVolume","CreateSnapshot"]}}},{"Effect":"Allow","Action":["ec2:DeleteTags"],"Resource":["arn:aws:ec2:*:*:volume/*","arn:aws:ec2:*:*:snapshot/*"]},{"Effect":"Allow","Action":["ec2:CreateVolume"],"Resource":"*","Condition":{"StringLike":{"aws:RequestTag/ebs.csi.aws.com/cluster":"true"}}},{"Effect":"Allow","Action":["ec2:CreateVolume"],"Resource":"*","Condition":{"StringLike":{"aws:RequestTag/CSIVolumeName":"*"}}},{"Effect":"Allow","Action":["ec2:DeleteVolume"],"Resource":"*","Condition":{"StringLike":{"ec2:ResourceTag/ebs.csi.aws.com/cluster":"true"}}},{"Effect":"Allow","Action":["ec2:DeleteVolume"],"Resource":"*","Condition":{"StringLike":{"ec2:ResourceTag/CSIVolumeName":"*"}}},{"Effect":"Allow","Action":["ec2:DeleteSnapshot"],"Resource":"*","Condition":{"StringLike":{"ec2:ResourceTag/CSIVolumeSnapshotName":"*"}}},{"Effect":"Allow","Action":["ec2:DeleteSnapshot"],"Resource":"*","Condition":{"StringLike":{"ec2:ResourceTag/ebs.csi.aws.com/cluster":"true"}}}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonECSTaskExecutionRolePolicy",
        policy_id: "ANPA_SERVICEROLE_AMAZONECSTASKEXECUTIONROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ecr:GetAuthorizationToken","ecr:BatchCheckLayerAvailability","ecr:GetDownloadUrlForLayer","ecr:BatchGetImage","logs:CreateLogStream","logs:PutLogEvents"],"Resource":"*"}]}"#,
    },
    // Recommended by the EKS add-on catalog for the `aws-efs-csi-driver` add-on's
    // `efs-csi-controller-sa` and `efs-csi-node-sa` service accounts. The conditions
    // are the published
    // policy's: the driver may create an access point only while tagging it for its
    // cluster, and may tag or delete only an access point already carrying that tag.
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonEFSCSIDriverPolicy",
        policy_id: "ANPA_SERVICEROLE_AMAZONEFSCSIDRIVERPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["elasticfilesystem:DescribeAccessPoints","elasticfilesystem:DescribeFileSystems","elasticfilesystem:DescribeMountTargets","ec2:DescribeAvailabilityZones"],"Resource":"*"},{"Effect":"Allow","Action":["elasticfilesystem:CreateAccessPoint"],"Resource":"*","Condition":{"StringLike":{"aws:RequestTag/efs.csi.aws.com/cluster":"true"}}},{"Effect":"Allow","Action":"elasticfilesystem:TagResource","Resource":"*","Condition":{"StringLike":{"aws:ResourceTag/efs.csi.aws.com/cluster":"true"}}},{"Effect":"Allow","Action":"elasticfilesystem:DeleteAccessPoint","Resource":"*","Condition":{"StringEquals":{"aws:ResourceTag/efs.csi.aws.com/cluster":"true"}}}]}"#,
    },
    // The EMR service role's own policy, which AWS files under `/service-role/` —
    // the counterpart of the placement-group policy above, published at the root. The
    // two are the pair a client provisioning EMR attaches, and they sit under
    // DIFFERENT paths, so the catalog carries each under the one AWS publishes it at.
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonElasticMapReduceRole",
        policy_id: "ANPA_SERVICEROLE_AMAZONELASTICMAPREDUCEROLE",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:AuthorizeSecurityGroupEgress","ec2:AuthorizeSecurityGroupIngress","ec2:CancelSpotInstanceRequests","ec2:CreateNetworkInterface","ec2:CreateSecurityGroup","ec2:CreateTags","ec2:DeleteNetworkInterface","ec2:DeleteSecurityGroup","ec2:DeleteTags","ec2:Describe*","ec2:DetachNetworkInterface","ec2:ModifyImageAttribute","ec2:ModifyInstanceAttribute","ec2:RequestSpotInstances","ec2:RevokeSecurityGroupEgress","ec2:RunInstances","ec2:TerminateInstances","ec2:CreateVolume","ec2:DeleteVolume","ec2:AttachVolume","ec2:DetachVolume","iam:GetRole","iam:GetRolePolicy","iam:ListInstanceProfiles","iam:ListRolePolicies","iam:PassRole","s3:CreateBucket","s3:Get*","s3:List*","sdb:BatchPutAttributes","sdb:Select","sqs:CreateQueue","sqs:Delete*","sqs:GetQueue*","sqs:PurgeQueue","sqs:ReceiveMessage","cloudwatch:PutMetricAlarm","cloudwatch:DescribeAlarms","cloudwatch:DeleteAlarms","application-autoscaling:RegisterScalableTarget","application-autoscaling:DeregisterScalableTarget","application-autoscaling:PutScalingPolicy","application-autoscaling:DeleteScalingPolicy","application-autoscaling:Describe*"],"Resource":"*"}]}"#,
    },
];

/// The catalog entry `arn` names, or `None` when `arn` names no AWS-managed policy
/// this emulator carries.
///
/// The ARN is read through the one grammar this crate holds
/// ([`parse_policy_arn`]), so a value that is not a managed-policy ARN at all names
/// nothing here either; the account segment is what says the ARN is in AWS's
/// namespace rather than an account's, and the entry answers only when its own ARN
/// is exactly the one asked for — the same identity test
/// [`ComAmazonawsIamAwsidentitymanagementv20100508State::policy_by_arn`] applies to
/// the account's policies.
pub fn aws_managed_policy(arn: &str) -> Option<&'static AwsManagedPolicy> {
    let parsed = parse_policy_arn(arn)?;
    if parsed.account != AWS_MANAGED_POLICY_ACCOUNT {
        return None;
    }
    AWS_MANAGED_POLICIES
        .iter()
        .find(|entry| entry.path == parsed.path && entry.policy_name == parsed.policy_name)
        .filter(|entry| entry.arn() == arn)
}

/// Whether `arn` is an ARN in AWS's own managed-policy namespace — well formed and
/// carrying [`AWS_MANAGED_POLICY_ACCOUNT`] as its account — whether or not
/// [`AWS_MANAGED_POLICIES`] carries that policy.
///
/// This is what separates "AWS owns this policy" from "this account owns it": the
/// write operations refuse an AWS-owned ARN as an out-of-range value for their
/// parameter rather than as a missing entity, because no account can create, change
/// or delete a policy AWS owns.
pub fn is_aws_managed_policy_arn(arn: &str) -> bool {
    parse_policy_arn(arn).is_some_and(|parsed| parsed.account == AWS_MANAGED_POLICY_ACCOUNT)
}

/// The catalog entry published under the friendly name `policy_name`, whatever path
/// AWS files it under, or `None` when the catalog carries no policy of that name.
///
/// A managed policy's friendly name identifies ONE policy in the namespace that owns
/// it — IAM holds a single policy per name whichever path it is filed under, which is
/// why the account's own policies are keyed by name alone
/// ([`ComAmazonawsIamAwsidentitymanagementv20100508State::policies`]) — so the name is
/// enough to say WHICH policy an ARN means to name, and the entry it answers with is
/// the one whose path that ARN has to carry to name it.
///
/// Matched without regard to ASCII case, the folding every IAM name is keyed under
/// ([`name_key`]).
pub fn aws_managed_policy_named(policy_name: &str) -> Option<&'static AwsManagedPolicy> {
    AWS_MANAGED_POLICIES
        .iter()
        .find(|entry| entry.policy_name.eq_ignore_ascii_case(policy_name))
}

/// Whether `arn`, an ARN in AWS's own managed-policy namespace, names a policy AWS
/// publishes.
///
/// An AWS-managed policy is not account state, so what the account holds says nothing
/// about it; the catalog does. A policy's identity is its path AND its friendly name
/// (the two its ARN spells), and a name names one policy
/// ([`aws_managed_policy_named`]), so an ARN that spells a carried policy's name under
/// any other path names NO policy at all — the path is not a hint AWS ignores, it is
/// part of the published identifier.
///
/// [`AWS_MANAGED_POLICIES`] is a SUBSET of AWS's catalog, which runs to well over a
/// thousand policies, so a name the catalog does not carry cannot be judged: such an
/// ARN is taken as published rather than refusing an attachment AWS accepts. What the
/// catalog knows, it is authoritative about — including where the policy it knows
/// lives.
pub fn is_published_aws_managed_policy_arn(arn: &str) -> bool {
    let Some(parsed) = parse_policy_arn(arn) else {
        return false;
    };
    if parsed.account != AWS_MANAGED_POLICY_ACCOUNT {
        return false;
    }
    match aws_managed_policy_named(&parsed.policy_name) {
        Some(entry) => entry.arn() == arn,
        None => true,
    }
}
