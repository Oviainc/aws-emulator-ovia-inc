// SPDX-License-Identifier: CC0-1.0

//! One entry per resource type this emulator really creates: which service owns the type,
//! and the create, update and delete calls that make its resources exist, change and stop
//! existing.
//!
//! This is the half of provisioning that is PER RESOURCE TYPE. The half that is not —
//! reconciling what a deployment declares against what the stack already has, deleting
//! what it replaced, and the in-process transport every one of these calls rides on —
//! is [`super::provisioning`], which owns the algorithm and reaches this module only
//! through [`provisioner_for`]. Adding a type is therefore a change to THIS file alone:
//! one arm in [`provisioner_for`] and the create/delete pair it names.
//!
//! A type with no entry here is left declared-only, which
//! [`super::provisioning`](crate::state::provisioning) documents: nothing is created in
//! any service, and the resource still carries the physical id and attributes
//! CloudFormation's documented naming rules give it.
//!
//! A deployment that re-declares a resource with a DIFFERENT configuration is applied
//! through the owning service's own update API ([`Provisioner::update`]), which is per
//! type for the same reason the create is: `AWS::SQS::Queue`'s configuration is its
//! attributes and its tags, so the change is a `SetQueueAttributes` plus a
//! `TagQueue`/`UntagQueue` ([`update_queue`]). A type with no arm there has nothing asked
//! of its service and the deployment reports the change as not applied, exactly as a type
//! with no create arm is left declared-only — never a stack that completed while the
//! caller's change went nowhere.
//!
//! How a create call is BUILT depends on what the type's properties are:
//!
//! * `AWS::KinesisFirehose::DeliveryStream`'s properties ARE `CreateDeliveryStream`'s own
//!   members, so the evaluated property document is the request, passed through verbatim
//!   with the one member CloudFormation decides (the name) set on the way out;
//! * `AWS::CloudWatch::MetricStream`'s are `PutMetricStream`'s own members the same way,
//!   and that one call is documented "Creates or updates a metric stream" — so it is also
//!   this type's UPDATE, the one arm here that can apply a changed declaration to a
//!   resource the stack already has;
//! * `AWS::CodePipeline::Pipeline`'s are the same document with one transformation: they
//!   are `PipelineDeclaration`'s members spelled with an upper-case initial, so
//!   [`camel_cased`] lower-cases each key's first letter and the document is the
//!   declaration;
//! * `AWS::ECR::PublicRepository`'s are NOT (`RepositoryName` and
//!   `RepositoryCatalogData.RepositoryDescription` against the API's `repositoryName` and
//!   `catalogData.description`), so its arm maps the members AWS's resource reference
//!   publishes one by one, and sends nothing for a property the template leaves out;
//! * the `AWS::OpenSearchServerless::*` types are the same case (`Name`/`Type`/`Policy`
//!   against `name`/`type`/`policy`), and a collection's identifier is one the OWNING
//!   service mints rather than a name the template asked for, so its arm answers that
//!   identifier and [`super::template::identity_is_service_assigned`] names the type it
//!   belongs to;
//! * `AWS::S3::Bucket`'s and `AWS::IAM::Role`'s reach services whose wire is not JSON at
//!   all, so those arms go through the official `aws-sdk-s3` / `aws-sdk-iam` clients over
//!   the same in-process callback ([`s3_client`], [`iam_client`]) and name the members
//!   they send one by one;
//! * `AWS::ECS::Cluster`'s are the same members under a different CASE — the Amazon ECS
//!   model spells them in lower camel case, and renames one (`ClusterSettings` sets
//!   `settings`) — so its arm names the properties `CreateCluster` takes and re-cases each
//!   value;
//! * `AWS::SQS::Queue`'s properties are the queue's ATTRIBUTES, named identically on both
//!   sides, so its arm passes them through as `CreateQueue`'s `Attributes` map, lifting
//!   out the two that are not attributes (the name CloudFormation decides, and the tag
//!   list the create takes as its own member);
//! * `AWS::SageMaker::ModelPackageGroup`'s are its create's members EXCEPT ONE — a model
//!   group's resource policy is attached through SageMaker's own
//!   `PutModelPackageGroupPolicy` — so its arm passes the properties through and then makes
//!   that second call, the same pair a client makes;
//! * `AWS::SNS::Topic`'s are the topic's attributes in the same way, with the three that
//!   are not attributes lifted out (the name, the tag list and the data-protection policy,
//!   each a `CreateTopic` member of its own), the inline `Subscription` list made by
//!   `Subscribe` calls on the created topic, and `DeliveryStatusLogging` written as the
//!   per-protocol feedback attributes SNS names;
//! * `AWS::FIS::ExperimentTemplate`'s are the same members in lower camel case, but its
//!   `Targets` and `Actions` are MAPS keyed by names the template chose (as are a target's
//!   `ResourceTags` and `Parameters` and an action's `Parameters` and `Targets`), so its
//!   arm maps each modelled structure member by member and passes every caller-keyed map
//!   through as written. Its physical id is one the owning service ASSIGNS: FIS mints the
//!   experiment template's id, so that id — not a computed name — is what the stack reports
//!   for the resource;
//! * `AWS::WAFRegional::IPSet`'s two properties reach AWS WAF Classic through TWO calls,
//!   because that is how its API takes them: `CreateIPSet` takes the `Name`, each declared
//!   `IPSetDescriptors` entry is INSERTed by an `UpdateIPSet`, and both carry a change
//!   token of their own. Its identity is its owning service's too ("Ref returns the IPSet
//!   ID"), so its arm answers the `IPSetId` AWS WAF returned as the resource's physical id.
//! * `AWS::Grafana::Workspace`'s properties are `CreateWorkspace`'s own members under
//!   DIFFERENT names — the type spells the workspace's own fields without the `workspace`
//!   prefix that API carries (`Name`/`Description`/`RoleArn`/`DataSources` against
//!   `workspaceName`/`workspaceDescription`/`workspaceRoleArn`/`workspaceDataSources`) —
//!   so its arm names the members the resource reference publishes one by one and sends
//!   nothing for a property the template leaves out. Its identity is that service's as
//!   well: Amazon Managed Grafana mints the workspace id its `Ref` returns.
//! * `AWS::EMR::Cluster`'s are mostly `RunJobFlow`'s own members (`ReleaseLabel`,
//!   `Applications`, `BootstrapActions`, `Tags`, the roles and the scalars), so they pass
//!   through verbatim; the two that are shaped differently are mapped — the per-role
//!   capacity properties of `Instances` onto the one `InstanceGroups` or `InstanceFleets`
//!   list the API takes, and a `Configurations` entry's `ConfigurationProperties` onto the
//!   API's `Properties`. Its identity is its owning service's as well ("Ref returns the
//!   cluster ID"), so its arm answers the `j-` job-flow id Amazon EMR minted as the
//!   resource's physical id;
//! * `AWS::GuardDuty::Detector`'s are the `CreateDetector` request's own members spelled
//!   with an initial capital (`Enable` / `enable`), all the way down, so its arm carries the
//!   whole evaluated document through the same re-casing [`lower_camel_members`] the ECS
//!   arm needs and lifts out the tag list, which the create takes as a map. A detector's id
//!   is GuardDuty's OWN to mint — `CreateDetector` takes no identifier and answers with
//!   `detectorId` — so, as for the FIS, AWS WAF Classic and Amazon EMR types above, what
//!   the stack reports for the resource is the id that create answered with.
//! * `AWS::RDS::DBInstance`'s and `AWS::RDS::DBSubnetGroup`'s are the members
//!   `CreateDBInstance` / `CreateDBSubnetGroup` take, named identically on both sides
//!   except the two Amazon RDS spells differently (`VPCSecurityGroups` is
//!   `VpcSecurityGroupIds`, `CustomIAMInstanceProfile` is `CustomIamInstanceProfile`), so
//!   each arm walks the declaration against its create's member catalogue
//!   ([`CREATE_DB_INSTANCE_MEMBERS`], [`CREATE_DB_SUBNET_GROUP_MEMBERS`], taken from that
//!   service's own model) and writes each member as the awsQuery form carries it — a list
//!   under the element name its list member's `@xmlName` fixes
//!   (`SubnetIds.SubnetIdentifier.1`). A property the catalogue does not carry is REFUSED,
//!   except the few that are members of a DIFFERENT call and are documented as such
//!   ([`DB_INSTANCE_PROPERTIES_NOT_SENT`]). Both identities are CloudFormation's — "Ref
//!   returns the DBInstanceIdentifier" / "the DBSubnetGroupName" — so each arm sets that
//!   member from the resource's physical id, and the DB instance's published endpoint,
//!   ARN and resource id are replaced by the ones Amazon RDS answered with, since those
//!   are the values a client reaches the database through.
//! * `AWS::RDS::DBParameterGroup`'s and `AWS::RDS::DBClusterParameterGroup`'s are their
//!   creates' own members read the same way ([`ParameterGroupCalls`]), with the one property
//!   Amazon RDS spells differently renamed (`Family` is `DBParameterGroupFamily`) and the
//!   name CloudFormation decides set on the way out — each type's `Ref` returns its group's
//!   name. The two declare the same document under different member and action names, so one
//!   arm serves both. Their `Parameters` MAP is no member of either create: a group's
//!   parameter values are written by the modify a client makes next, so the arm makes that
//!   second call ([`modify_parameters`]) in the batches that request's own bound admits.
//! * `AWS::Events::Rule`'s are `PutRule`'s own members, spelled identically, with two
//!   exceptions: `EventPattern` is a template DOCUMENT where the request takes the JSON
//!   TEXT of one, and `Targets` is no `PutRule` member at all — a rule's targets are
//!   installed by the `PutTargets` a client makes next, so the arm makes that second call
//!   ([`install_targets`]), maps each declared target against `Target`'s own members and
//!   fails the resource on a target EventBridge reports as a failed entry. `PutRule` is
//!   documented "Creates or updates the specified rule", so it is also this type's UPDATE,
//!   with the declared target list reconciled against the installed one.
//! * `AWS::SSM::Parameter`'s are `PutParameter`'s own members the same way, with the name
//!   CloudFormation decides set on the way out and the declared tag MAP sent as the
//!   `[{Key, Value}]` list that request takes. That one call is "Create or update a
//!   parameter in Parameter Store", so `Overwrite` makes it this type's update too.
//! * `AWS::Cognito::UserPoolGroup`'s are `CreateGroup`'s own members, spelled identically on
//!   both sides (`UserPoolId`, `GroupName`, `Description`, `Precedence`, `RoleArn`), so the
//!   evaluated properties are the request with the one member CloudFormation decides (the
//!   group's name, which is this resource's physical id) set on the way out. A group's
//!   identity is a PAIR — the pool that holds it and its name — so every call of its arm
//!   carries the declared `UserPoolId` beside that name, and a declaration that moves the
//!   group to another pool is no in-place change: `UpdateGroup` changes only the three
//!   configuration members, so such a deployment reports the change as not applied.
//! * the two `AWS::S3Vectors::*` types' properties are their creates' own members with a
//!   lower-case initial, and each type's IDENTITY is its own ARN rather than a name: `Ref`
//!   returns the vector bucket's `bucket/<name>` ARN and the index's
//!   `bucket/<bucket>/index/<name>` one, so the names each create carries are read back off
//!   that ARN ([`vector_bucket_name`], [`vector_index_identity`]) and the delete and the
//!   drift read address the resource by the ARN itself.
//! * the two `AWS::Scheduler::*` types' properties ARE their creates' own members, spelled
//!   identically all the way down, with the name CloudFormation decides carried in the URI
//!   those `restJson1` operations bind it to — so each arm passes the evaluated property
//!   document through, a group's declared tags beside the ones CloudFormation gives it. A
//!   schedule is keyed by its name AND the group holding it
//!   ([`scheduled_group_of`]), so its delete and its drift read carry both.
//! * the two `AWS::MSK::*` cluster types' properties are their creates' own members in
//!   lower camel case, so each arm walks the evaluated document ([`msk_members`]) with the
//!   one member whose API spelling is not a case change renamed
//!   ([`MSK_RENAMED_MEMBERS`]); a serverless cluster's two configuration properties go
//!   under the `serverless` body `CreateClusterV2` takes, whose `VpcConfigs` entries are
//!   mapped by name. Each type's identity is the `ClusterArn` Amazon MSK mints ("Ref
//!   returns the ARN of the cluster"), so that ARN — not a computed name — is what the
//!   stack reports for the resource, and the delete and the drift read address the cluster
//!   by it.
//!
//! ## The tags every created resource carries
//!
//! Beside what its declaration asks for, a resource CloudFormation creates carries the tags
//! CloudFormation gives it: the stack-level `Tags` plus `aws:cloudformation:stack-name`,
//! `aws:cloudformation:stack-id` and `aws:cloudformation:logical-id`
//! ([`super::provisioning::StackProvenance`]). That SET is the same for every type, so it is
//! computed once there; APPLYING it is per type, because each owning API takes a tag set its
//! own way — a `Tags` member of the create whose `Tag` shape spells `Key`/`Value` (Amazon
//! SNS, Amazon Data Firehose, Amazon ECR Public, Amazon CloudWatch, Amazon EMR, Amazon
//! SageMaker, IAM), the
//! same list spelled `key`/`value`
//! (Amazon ECS, CodePipeline), a map of key to value (Amazon SQS, AWS FIS), or a tagging
//! call of its own where the create takes no tags at all (Amazon S3's `PutBucketTagging`).
//! [`tags_of`] is the one place the declared and the propagated halves are merged, and a key
//! the declaration itself states keeps the declaration's value. A type that is not taggable
//! — `AWS::WAFRegional::IPSet`, which publishes no `Tags` property — is tagged with nothing,
//! because CloudFormation propagates no tags to one, and a type whose owning API refuses the
//! reserved `aws:` namespace to its callers carries the stack's own tags without the three
//! ([`Provisioner::applicable_tags`]).
//!
//! No arm invents a value: a property the template does not declare is absent from the
//! request, and what the owning service answers is what the stack records. A property an
//! arm does not map onto the create's request at all is REFUSED rather than dropped, so a
//! stack never reports a resource configured differently from its declaration — the FIS
//! arm's `ExperimentReportConfiguration` is the one such property today. Inside a nested
//! structure the member tables are the type's published members, so a member not in one is
//! not a member of the type.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::provisioning::{
    call_json_rpc, call_query, call_rest_json, percent_encoded, xml_element, xml_entry_map,
    xml_scalar_children, Created, Refusal, AWS_JSON_1_0, AWS_JSON_1_1,
};
use super::template::{
    ACM_ACME_ENDPOINT, AOSS_COLLECTION, AOSS_SECURITY_POLICY, CLOUDWATCH_METRIC_STREAM,
    COGNITO_USER_POOL_GROUP, DEFAULT_SCHEDULE_GROUP, ECR_PUBLIC_REPOSITORY, ECS_CLUSTER,
    EMR_CLUSTER, EVENTS_RULE, FIREHOSE_DELIVERY_STREAM, FIS_EXPERIMENT_TEMPLATE,
    GRAFANA_WORKSPACE, GUARDDUTY_DETECTOR, MSK_CLUSTER, MSK_SERVERLESS_CLUSTER,
    S3VECTORS_BUCKET_SEGMENT, S3VECTORS_INDEX,
    S3VECTORS_VECTOR_BUCKET, SAGEMAKER_MODEL_PACKAGE_GROUP, SCHEDULER_SCHEDULE,
    SCHEDULER_SCHEDULE_GROUP, SNS_TOPIC, SQS_QUEUE, SSM_PARAMETER, STEP_FUNCTIONS_ACTIVITY,
    WAF_REGIONAL_IP_SET,
};
use super::StoredResource;

/// `AWS::S3::Bucket`: "Ref returns the bucket name", so a resource of this type is a
/// bucket of Amazon S3 addressed by its physical resource id.
const S3_BUCKET: &str = "AWS::S3::Bucket";

/// `AWS::IAM::Role`: "Ref returns the role name".
const IAM_ROLE: &str = "AWS::IAM::Role";

/// `AWS::CodePipeline::Pipeline`: "Ref returns the pipeline name".
const CODEPIPELINE_PIPELINE: &str = "AWS::CodePipeline::Pipeline";

/// `AWS::RDS::DBInstance`: "Ref returns the DBInstanceIdentifier", so a resource of this
/// type is a DB instance of Amazon RDS addressed by its physical resource id.
const RDS_DB_INSTANCE: &str = "AWS::RDS::DBInstance";

/// `AWS::RDS::DBSubnetGroup`: "Ref returns the DBSubnetGroupName" — the group a DB
/// instance's `DBSubnetGroupName` names, which Amazon RDS resolves before it will create
/// an instance in it.
const RDS_DB_SUBNET_GROUP: &str = "AWS::RDS::DBSubnetGroup";

/// `AWS::RDS::DBParameterGroup`: "Ref returns the DB parameter group name" — the group a DB
/// instance's `DBParameterGroupName` names, and the one `DescribeDBParameters` reports the
/// parameters of.
const RDS_DB_PARAMETER_GROUP: &str = "AWS::RDS::DBParameterGroup";

/// `AWS::RDS::DBClusterParameterGroup`: "Ref returns the DB cluster parameter group name" —
/// the cluster-scoped sibling of the type above, which a DB cluster's
/// `DBClusterParameterGroupName` names.
const RDS_DB_CLUSTER_PARAMETER_GROUP: &str = "AWS::RDS::DBClusterParameterGroup";

/// The code a refusal carries where the change a deployment asks for is one this emulator
/// has not written the owning API call for. It is the same code the scaffold's own 501
/// answers under (`smithy.framework#NotImplemented`), and not a code any AWS service
/// answers, so it is never mistaken for one of theirs.
const NOT_IMPLEMENTED: &str = "NotImplemented";

/// The services whose resources this emulator creates. One variant per resource type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Provisioner {
    FirehoseDeliveryStream,
    CloudWatchMetricStream,
    EcrPublicRepository,
    OpenSearchServerlessCollection,
    OpenSearchServerlessSecurityPolicy,
    S3Bucket,
    S3VectorsVectorBucket,
    S3VectorsIndex,
    EmrCluster,
    IamRole,
    CodePipelinePipeline,
    EcsCluster,
    SqsQueue,
    SageMakerModelPackageGroup,
    SnsTopic,
    FisExperimentTemplate,
    WafRegionalIpSet,
    GrafanaWorkspace,
    GuardDutyDetector,
    MskCluster,
    MskServerlessCluster,
    RdsDbInstance,
    RdsDbSubnetGroup,
    RdsDbParameterGroup,
    RdsDbClusterParameterGroup,
    EventsRule,
    SsmParameter,
    StepFunctionsActivity,
    SchedulerScheduleGroup,
    SchedulerSchedule,
    CognitoUserPoolGroup,
    AcmAcmeEndpoint,
}

/// The provisioner that owns a template resource type, or `None` for a type this emulator
/// leaves declared-only.
pub(super) fn provisioner_for(resource_type: &str) -> Option<Provisioner> {
    match resource_type {
        FIREHOSE_DELIVERY_STREAM => Some(Provisioner::FirehoseDeliveryStream),
        CLOUDWATCH_METRIC_STREAM => Some(Provisioner::CloudWatchMetricStream),
        ECR_PUBLIC_REPOSITORY => Some(Provisioner::EcrPublicRepository),
        AOSS_COLLECTION => Some(Provisioner::OpenSearchServerlessCollection),
        AOSS_SECURITY_POLICY => Some(Provisioner::OpenSearchServerlessSecurityPolicy),
        S3_BUCKET => Some(Provisioner::S3Bucket),
        S3VECTORS_VECTOR_BUCKET => Some(Provisioner::S3VectorsVectorBucket),
        S3VECTORS_INDEX => Some(Provisioner::S3VectorsIndex),
        EMR_CLUSTER => Some(Provisioner::EmrCluster),
        IAM_ROLE => Some(Provisioner::IamRole),
        CODEPIPELINE_PIPELINE => Some(Provisioner::CodePipelinePipeline),
        ECS_CLUSTER => Some(Provisioner::EcsCluster),
        SQS_QUEUE => Some(Provisioner::SqsQueue),
        SAGEMAKER_MODEL_PACKAGE_GROUP => Some(Provisioner::SageMakerModelPackageGroup),
        SNS_TOPIC => Some(Provisioner::SnsTopic),
        FIS_EXPERIMENT_TEMPLATE => Some(Provisioner::FisExperimentTemplate),
        WAF_REGIONAL_IP_SET => Some(Provisioner::WafRegionalIpSet),
        GRAFANA_WORKSPACE => Some(Provisioner::GrafanaWorkspace),
        GUARDDUTY_DETECTOR => Some(Provisioner::GuardDutyDetector),
        MSK_CLUSTER => Some(Provisioner::MskCluster),
        MSK_SERVERLESS_CLUSTER => Some(Provisioner::MskServerlessCluster),
        RDS_DB_INSTANCE => Some(Provisioner::RdsDbInstance),
        RDS_DB_SUBNET_GROUP => Some(Provisioner::RdsDbSubnetGroup),
        RDS_DB_PARAMETER_GROUP => Some(Provisioner::RdsDbParameterGroup),
        RDS_DB_CLUSTER_PARAMETER_GROUP => Some(Provisioner::RdsDbClusterParameterGroup),
        EVENTS_RULE => Some(Provisioner::EventsRule),
        SSM_PARAMETER => Some(Provisioner::SsmParameter),
        STEP_FUNCTIONS_ACTIVITY => Some(Provisioner::StepFunctionsActivity),
        SCHEDULER_SCHEDULE_GROUP => Some(Provisioner::SchedulerScheduleGroup),
        SCHEDULER_SCHEDULE => Some(Provisioner::SchedulerSchedule),
        COGNITO_USER_POOL_GROUP => Some(Provisioner::CognitoUserPoolGroup),
        ACM_ACME_ENDPOINT => Some(Provisioner::AcmAcmeEndpoint),
        _ => None,
    }
}

impl Provisioner {
    /// Create one resource in the service that owns it, and report back what that service
    /// decided about it ([`Created`]): the attributes it reported (an ARN it minted, say),
    /// which replace the ones CloudFormation's naming rules computed, and — for a type
    /// whose identity is that service's
    /// ([`super::template::identity_is_service_assigned`]) — the physical id it assigned.
    /// `Err` is that service's own refusal.
    ///
    /// `propagated` are the tags CloudFormation gives the resource — the stack's own plus
    /// the three that name the stack and this resource's place in it
    /// ([`super::provisioning::StackProvenance::tags_for`]) — and every arm whose type is
    /// taggable applies them ([`tags_of`]) beside the ones the declaration itself asks for,
    /// so the owning service's tag read on the handle the stack publishes answers with
    /// them. The one arm that applies none is `AWS::WAFRegional::IPSet`, whose type
    /// publishes no `Tags` property at all: CloudFormation propagates no tags to a resource
    /// type that is not taggable.
    pub(super) async fn create(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        resource: &StoredResource,
        propagated: &[(String, String)],
    ) -> Result<Created, Refusal> {
        let propagated = &self.applicable_tags(propagated);
        match self {
            Self::FirehoseDeliveryStream => {
                create_delivery_stream(ctx, region, resource, propagated).await
            }
            Self::CloudWatchMetricStream => {
                put_metric_stream(ctx, region, resource, propagated).await
            }
            Self::EmrCluster => run_job_flow(ctx, region, resource, propagated).await,
            Self::EcrPublicRepository => {
                create_public_repository(ctx, region, resource, propagated).await
            }
            Self::S3Bucket => create_bucket(ctx, region, resource, propagated).await,
            Self::S3VectorsVectorBucket => {
                create_vector_bucket(ctx, region, resource, propagated).await
            }
            Self::S3VectorsIndex => create_index(ctx, region, resource, propagated).await,
            Self::IamRole => create_role(ctx, region, resource, propagated).await,
            Self::CodePipelinePipeline => {
                create_pipeline(ctx, region, resource, propagated).await
            }
            Self::EcsCluster => create_cluster(ctx, region, resource, propagated).await,
            Self::SqsQueue => create_queue(ctx, region, resource, propagated).await,
            Self::SageMakerModelPackageGroup => {
                create_model_package_group(ctx, region, resource, propagated).await
            }
            Self::SnsTopic => create_topic(ctx, region, resource, propagated).await,
            Self::FisExperimentTemplate => {
                create_experiment_template(ctx, region, resource, propagated).await
            }
            // `AWS::WAFRegional::IPSet` publishes `Name` and `IPSetDescriptors` and no
            // `Tags`: the type is not taggable, so no tags are propagated to it.
            Self::WafRegionalIpSet => create_ip_set(ctx, region, resource).await,
            // `AWS::Grafana::Workspace` publishes no `Tags` property either, so the type
            // is not taggable and no stack tags are propagated to it.
            Self::GrafanaWorkspace => create_workspace(ctx, region, resource).await,
            // `AWS::OpenSearchServerless::SecurityPolicy` publishes no `Tags` property and
            // `CreateSecurityPolicy` takes none, so a policy carries no propagated tag.
            Self::OpenSearchServerlessSecurityPolicy => {
                create_security_policy(ctx, region, resource).await
            }
            Self::OpenSearchServerlessCollection => {
                create_collection(ctx, region, resource, propagated).await
            }
            Self::GuardDutyDetector => {
                create_detector(ctx, region, resource, propagated).await
            }
            Self::MskCluster => create_msk_cluster(ctx, region, resource, propagated).await,
            Self::MskServerlessCluster => {
                create_msk_serverless_cluster(ctx, region, resource, propagated).await
            }
            Self::RdsDbInstance => {
                create_db_instance(ctx, region, resource, propagated).await
            }
            Self::RdsDbSubnetGroup => {
                create_db_subnet_group(ctx, region, resource, propagated).await
            }
            Self::RdsDbParameterGroup => {
                create_parameter_group(
                    ctx,
                    region,
                    resource,
                    propagated,
                    DB_PARAMETER_GROUP_CALLS,
                )
                .await
            }
            Self::RdsDbClusterParameterGroup => {
                create_parameter_group(
                    ctx,
                    region,
                    resource,
                    propagated,
                    DB_CLUSTER_PARAMETER_GROUP_CALLS,
                )
                .await
            }
            // `AWS::Events::Rule` publishes `Description`, `EventBusName`, `EventPattern`,
            // `Name`, `RoleArn`, `ScheduleExpression`, `State` and `Targets` and no
            // `Tags`: the type is not taggable, so no tags are propagated to it.
            Self::EventsRule => put_rule(ctx, region, resource).await,
            Self::SsmParameter => put_parameter(ctx, region, resource, propagated).await,
            Self::StepFunctionsActivity => {
                create_activity(ctx, region, resource, propagated).await
            }
            Self::SchedulerScheduleGroup => {
                create_schedule_group(ctx, region, resource, propagated).await
            }
            // `AWS::Scheduler::Schedule` publishes no `Tags` property — EventBridge
            // Scheduler tags a schedule GROUP, not a schedule — so the type is not
            // taggable and no tags are propagated to it.
            Self::SchedulerSchedule => create_schedule(ctx, region, resource).await,
            // `AWS::Cognito::UserPoolGroup` publishes `Description`, `GroupName`,
            // `Precedence`, `RoleArn` and `UserPoolId` and no `Tags`: the type is not
            // taggable, so no tags are propagated to a group.
            Self::CognitoUserPoolGroup => create_group(ctx, region, resource).await,
            Self::AcmAcmeEndpoint => create_acme_endpoint(ctx, region, resource, propagated).await,
        }
    }

    /// Those of the tags CloudFormation gives a resource that the owning service's own API
    /// will take.
    ///
    /// Every create here is made through the owning service's PUBLIC API — the same path a
    /// client's own call takes, which is what makes a stack's resource indistinguishable
    /// from one a client created. Some of those APIs refuse a tag key in AWS's reserved
    /// `aws:` namespace, which is AWS's rule for a CALLER's request ("don't use the `aws:`
    /// prefix ... it is reserved for AWS use"): IAM's `CreateRole`, CodePipeline's
    /// `CreatePipeline` and EventBridge Scheduler's `CreateScheduleGroup` all do, in their
    /// own crates' tag validation. AWS sets the
    /// `aws:cloudformation:*` tags from inside itself, which no public API models and this
    /// emulator has no privileged channel for, so for such a type those three are left off
    /// rather than sent to be refused — a stack must not fail over tags CloudFormation adds
    /// of its own accord. The stack's own tags, which are ordinary keys, are applied either
    /// way. ACM is the same case: its `CreateAcmeEndpoint` refuses a reserved key on either
    /// half of a pair, and so do Amazon ECS's `CreateCluster` and Amazon MSK's two cluster
    /// creates.
    fn applicable_tags(self, propagated: &[(String, String)]) -> Vec<(String, String)> {
        let refuses_reserved_keys = matches!(
            self,
            Self::IamRole
                | Self::CodePipelinePipeline
                | Self::EcsCluster
                | Self::S3VectorsVectorBucket
                | Self::S3VectorsIndex
                | Self::SchedulerScheduleGroup
                | Self::AcmAcmeEndpoint
                | Self::MskCluster
                | Self::MskServerlessCluster
        );
        propagated
            .iter()
            .filter(|(key, _)| !(refuses_reserved_keys && is_reserved_tag_key(key)))
            .cloned()
            .collect()
    }

    /// Apply a CHANGED configuration to a resource that already exists, through the owning
    /// service's own update API, and report what that service decided about it (the same
    /// [`Created`] an initial create answers with: the attributes it reported now).
    ///
    /// `resource` is the record of the resource as it stands — the identity its service
    /// knows it by, and the properties it was last deployed with — and `declared` is the
    /// property document this deployment declares for it. The difference between the two is
    /// what the call carries, so a property neither side changed is not re-sent.
    ///
    /// `Ok(None)` is a type this emulator has no update path for YET: nothing is asked of
    /// the owning service, and [`super::provisioning::reconcile`] reports the change as not
    /// applied rather than as a success that dropped it. `Err` is that service's own
    /// refusal of the change — including a change its API cannot make to an existing
    /// resource, which is the refusal a client would get.
    pub(super) async fn update(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        resource: &StoredResource,
        declared: &Value,
    ) -> Result<Option<Created>, Refusal> {
        match self {
            Self::SqsQueue => update_queue(ctx, region, resource, declared).await.map(Some),
            // CloudWatch's own `PutMetricStream` is documented "Creates or updates a metric
            // stream", so the call that created the stream is the call that applies a
            // changed declaration to it — under the name the stack gave it, with the
            // configuration the template now declares.
            Self::CloudWatchMetricStream => {
                put_metric_stream_configuration(
                    ctx,
                    region,
                    &resource.physical_resource_id,
                    declared,
                )
                .await
                .map(Some)
            }
            // `PutRule` is documented "Creates or updates the specified rule", so the call
            // that created the rule is the call that applies a changed declaration to it,
            // with the declared target list reconciled against the one it carries.
            Self::EventsRule => update_rule(ctx, region, resource, declared).await,
            // `PutParameter` is documented "Create or update a parameter in Parameter
            // Store", and `Overwrite` is what makes the second of the two: the changed
            // declaration reaches Systems Manager as a new version of the same parameter.
            Self::SsmParameter => update_parameter(ctx, region, resource, declared)
                .await
                .map(Some),
            // `UpdateGroup` "updates any of the properties for precedence, IAM role, or
            // description" of a group the pool already holds, so a changed declaration of
            // those reaches Amazon Cognito through it — while the two members that make the
            // group's identity are not members of that call at all.
            Self::CognitoUserPoolGroup => update_group(ctx, region, resource, declared).await,
            // Every other type's in-place update is its own entry here, exactly as its
            // create and delete are: until one is written, nothing is asked of the owning
            // service and the deployment says so.
            _ => Ok(None),
        }
    }

    /// Delete one resource from the service that owns it. A resource that service no
    /// longer holds is already in the state the delete asks for, so each arm decides that
    /// on the answer's own terms: on its not-found CODE where the service models one —
    /// never on a message, which carries the request's own name — and, where it models
    /// none (SageMaker's model group), by READING the resource back.
    ///
    /// The whole record is passed, not just its physical id: a type whose identity is a
    /// PAIR (a security policy's `(type, name)`) needs the property that carries the other
    /// half.
    pub(super) async fn delete(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        resource: &StoredResource,
    ) -> Result<(), Refusal> {
        let physical_resource_id = &resource.physical_resource_id;
        match self {
            Self::FirehoseDeliveryStream => {
                delete_delivery_stream(ctx, region, physical_resource_id).await
            }
            Self::CloudWatchMetricStream => {
                delete_metric_stream(ctx, region, physical_resource_id).await
            }
            Self::EcrPublicRepository => {
                delete_public_repository(ctx, region, physical_resource_id).await
            }
            Self::OpenSearchServerlessSecurityPolicy => {
                delete_security_policy(ctx, region, resource).await
            }
            Self::OpenSearchServerlessCollection => {
                delete_collection(ctx, region, physical_resource_id).await
            }
            Self::S3Bucket => delete_bucket(ctx, region, physical_resource_id).await,
            Self::S3VectorsVectorBucket => {
                delete_vector_bucket(ctx, region, physical_resource_id).await
            }
            Self::S3VectorsIndex => delete_index(ctx, region, physical_resource_id).await,
            Self::EmrCluster => terminate_job_flow(ctx, region, physical_resource_id).await,
            // The role's policies have to go first, and only the resource's own record
            // says which ones there are.
            Self::IamRole => delete_role(ctx, region, resource).await,
            Self::CodePipelinePipeline => {
                delete_pipeline(ctx, region, physical_resource_id).await
            }
            Self::EcsCluster => delete_cluster(ctx, region, physical_resource_id).await,
            Self::SqsQueue => delete_queue(ctx, region, physical_resource_id).await,
            Self::SageMakerModelPackageGroup => {
                delete_model_package_group(ctx, region, physical_resource_id).await
            }
            Self::SnsTopic => delete_topic(ctx, region, physical_resource_id).await,
            Self::FisExperimentTemplate => {
                delete_experiment_template(ctx, region, physical_resource_id).await
            }
            Self::WafRegionalIpSet => delete_ip_set(ctx, region, physical_resource_id).await,
            Self::GrafanaWorkspace => {
                delete_workspace(ctx, region, physical_resource_id).await
            }
            Self::GuardDutyDetector => delete_detector(ctx, region, physical_resource_id).await,
            // One `DeleteCluster` deletes a cluster of either kind, addressed by the ARN
            // Amazon MSK minted for it.
            Self::MskCluster | Self::MskServerlessCluster => {
                delete_msk_cluster(ctx, region, physical_resource_id).await
            }
            // The delete carries one member off the resource's own declaration
            // (`DeleteAutomatedBackups`), so it takes the record rather than the id.
            Self::RdsDbInstance => delete_db_instance(ctx, region, resource).await,
            Self::RdsDbSubnetGroup => {
                delete_db_subnet_group(ctx, region, physical_resource_id).await
            }
            Self::RdsDbParameterGroup => {
                delete_parameter_group(
                    ctx,
                    region,
                    physical_resource_id,
                    DB_PARAMETER_GROUP_CALLS,
                )
                .await
            }
            Self::RdsDbClusterParameterGroup => {
                delete_parameter_group(
                    ctx,
                    region,
                    physical_resource_id,
                    DB_CLUSTER_PARAMETER_GROUP_CALLS,
                )
                .await
            }
            // A rule still carrying targets cannot be deleted, and only the rule's own
            // record says which bus it is on, so the delete takes the record.
            Self::EventsRule => delete_rule(ctx, region, resource).await,
            Self::SsmParameter => delete_parameter(ctx, region, physical_resource_id).await,
            Self::StepFunctionsActivity => {
                delete_activity(ctx, region, physical_resource_id).await
            }
            Self::SchedulerScheduleGroup => {
                delete_schedule_group(ctx, region, physical_resource_id).await
            }
            // A schedule is addressed by its name AND the group that holds it, which only
            // the resource's own record carries, so the delete takes the record.
            Self::SchedulerSchedule => delete_schedule(ctx, region, resource).await,
            // The delete addresses the group by the PAIR that identifies it, and the pool
            // half of that pair is on the resource's own declaration.
            Self::CognitoUserPoolGroup => delete_group(ctx, region, resource).await,
            Self::AcmAcmeEndpoint => delete_acme_endpoint(ctx, region, physical_resource_id).await,
        }
    }

    /// What the service that OWNS a resource reports the resource's configuration to be
    /// NOW: the document its own read answers with. This is the ACTUAL configuration
    /// [`super::drift`] compares a resource's expected template configuration against, so
    /// drift is read off the owning service rather than off CloudFormation's own record of
    /// what it once applied.
    ///
    /// `Ok(None)` is that service answering that it no longer holds the resource — judged
    /// on its own not-found CODE, never on a message, exactly as [`Self::delete`] does —
    /// which is the `DELETED` drift status. `Err(refusal)` is any other refusal: the check
    /// did not run, and the detection reports that rather than a resource as checked.
    pub(super) async fn actual_configuration(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        resource: &StoredResource,
    ) -> Result<Option<Value>, Refusal> {
        let physical_resource_id = &resource.physical_resource_id;
        match self {
            Self::FirehoseDeliveryStream => {
                describe_delivery_stream(ctx, region, physical_resource_id).await
            }
            Self::CloudWatchMetricStream => {
                describe_metric_stream(ctx, region, physical_resource_id).await
            }
            Self::EcrPublicRepository => {
                describe_public_repository(ctx, region, physical_resource_id).await
            }
            Self::S3Bucket => describe_bucket(ctx, region, physical_resource_id).await,
            Self::S3VectorsVectorBucket => {
                describe_vector_bucket(ctx, region, physical_resource_id).await
            }
            Self::S3VectorsIndex => describe_index(ctx, region, physical_resource_id).await,
            Self::EmrCluster => describe_job_flow(ctx, region, physical_resource_id).await,
            Self::IamRole => describe_role(ctx, region, physical_resource_id).await,
            Self::CodePipelinePipeline => {
                describe_pipeline(ctx, region, physical_resource_id).await
            }
            Self::EcsCluster => describe_cluster(ctx, region, physical_resource_id).await,
            Self::SqsQueue => describe_queue(ctx, region, physical_resource_id).await,
            Self::OpenSearchServerlessCollection => {
                describe_collection(ctx, region, physical_resource_id).await
            }
            Self::OpenSearchServerlessSecurityPolicy => {
                describe_security_policy(ctx, region, resource).await
            }
            Self::SageMakerModelPackageGroup => {
                describe_model_package_group(ctx, region, physical_resource_id).await
            }
            Self::SnsTopic => describe_topic(ctx, region, physical_resource_id).await,
            Self::FisExperimentTemplate => {
                describe_experiment_template(ctx, region, physical_resource_id).await
            }
            Self::WafRegionalIpSet => describe_ip_set(ctx, region, physical_resource_id).await,
            Self::GrafanaWorkspace => {
                describe_workspace(ctx, region, physical_resource_id).await
            }
            Self::GuardDutyDetector => describe_detector(ctx, region, physical_resource_id).await,
            // One `DescribeClusterV2` reads a cluster of either kind back.
            Self::MskCluster | Self::MskServerlessCluster => {
                describe_msk_cluster(ctx, region, physical_resource_id).await
            }
            Self::RdsDbInstance => {
                describe_db_instance(ctx, region, physical_resource_id).await
            }
            Self::RdsDbSubnetGroup => {
                describe_db_subnet_group(ctx, region, physical_resource_id).await
            }
            Self::RdsDbParameterGroup => {
                describe_parameter_group(
                    ctx,
                    region,
                    physical_resource_id,
                    DB_PARAMETER_GROUP_CALLS,
                )
                .await
            }
            Self::RdsDbClusterParameterGroup => {
                describe_parameter_group(
                    ctx,
                    region,
                    physical_resource_id,
                    DB_CLUSTER_PARAMETER_GROUP_CALLS,
                )
                .await
            }
            // The read is addressed by the rule's name AND the bus it is bound to, which
            // only the resource's own record carries.
            Self::EventsRule => describe_rule(ctx, region, resource).await,
            Self::SsmParameter => describe_parameter(ctx, region, physical_resource_id).await,
            Self::StepFunctionsActivity => {
                describe_activity(ctx, region, physical_resource_id).await
            }
            Self::SchedulerScheduleGroup => {
                describe_schedule_group(ctx, region, physical_resource_id).await
            }
            // The read is addressed by the schedule's name AND its group, which only the
            // resource's own record carries.
            Self::SchedulerSchedule => describe_schedule(ctx, region, resource).await,
            // The read is addressed by the group's name AND the pool that holds it, which
            // only the resource's own record carries.
            Self::CognitoUserPoolGroup => describe_group(ctx, region, resource).await,
            Self::AcmAcmeEndpoint => {
                describe_acme_endpoint(ctx, region, physical_resource_id).await
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Amazon Data Firehose: AWS::KinesisFirehose::DeliveryStream
// ---------------------------------------------------------------------------

/// Amazon Data Firehose's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: firehose`, service shape `Firehose_20150804`), which speaks
/// `awsJson1_1`.
const FIREHOSE_SIGNING_NAME: &str = "firehose";
const FIREHOSE_TARGET_PREFIX: &str = "Firehose_20150804";

/// The error code Firehose answers for a stream it does not have.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// Create the stream an `AWS::KinesisFirehose::DeliveryStream` declares.
///
/// The type's properties are the `CreateDeliveryStream` request's own members — the
/// destination configurations, the source configuration, `DeliveryStreamType`,
/// `DeliveryStreamEncryptionConfigurationInput` and `Tags` are named identically on both
/// sides — so the resolved properties ARE the request, with the one member CloudFormation
/// decides (the stream's name, which is this resource's physical id) set to it.
///
/// `Tags` is the one member the request does not carry verbatim: the stream carries the
/// tags CloudFormation gives it beside the declared ones ([`tags_of`]), as the `[{Key,
/// Value}]` list Firehose's own `Tag` shape spells.
async fn create_delivery_stream(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    request.insert(
        "DeliveryStreamName".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("Tags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
    let reported = call_json_rpc(
        ctx,
        FIREHOSE_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{FIREHOSE_TARGET_PREFIX}.CreateDeliveryStream"),
        &Value::Object(request),
    )
    .await?;
    let mut attributes = BTreeMap::new();
    if let Some(arn) = reported.get("DeliveryStreamARN").and_then(Value::as_str) {
        attributes.insert("Arn".to_string(), arn.to_string());
    }
    Ok(Created::attributes(attributes))
}

/// Delete the stream a stack created. A stream Firehose no longer has
/// (`ResourceNotFoundException`) is already in the state the delete asks for.
async fn delete_delivery_stream(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "DeliveryStreamName": physical_resource_id });
    match call_json_rpc(
        ctx,
        FIREHOSE_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{FIREHOSE_TARGET_PREFIX}.DeleteDeliveryStream"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        // The stream is already in the state the delete asks for. Judged on the error
        // CODE, never on the message, which carries the request's own resource name.
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The stream's configuration as Amazon Data Firehose reports it, for drift detection.
/// A stream it no longer has (`ResourceNotFoundException`) was deleted.
async fn describe_delivery_stream(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "DeliveryStreamName": physical_resource_id });
    match call_json_rpc(
        ctx,
        FIREHOSE_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{FIREHOSE_TARGET_PREFIX}.DescribeDeliveryStream"),
        &request,
    )
    .await
    {
        Ok(answer) => Ok(Some(reported_member(&answer, "DeliveryStreamDescription"))),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

// ---------------------------------------------------------------------------
// Amazon CloudWatch: AWS::CloudWatch::MetricStream
// ---------------------------------------------------------------------------

/// Amazon CloudWatch's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: monitoring`, service shape `GraniteServiceVersion20100801`), whose
/// protocol list starts with `awsJson1_0` — the wire this call is written on, as the
/// service's own suites drive it.
const CLOUDWATCH_SIGNING_NAME: &str = "monitoring";
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";

/// Create — or update — the metric stream an `AWS::CloudWatch::MetricStream` declares.
///
/// The type's properties ARE `PutMetricStream`'s own members: `FirehoseArn`, `RoleArn`,
/// `OutputFormat`, `IncludeFilters`, `ExcludeFilters`, `StatisticsConfigurations`,
/// `IncludeLinkedAccountsMetrics` and `Tags` are named identically on both sides (AWS's
/// resource reference for the type against `PutMetricStreamInput`), so the resolved
/// property document IS the request, with the one member CloudFormation decides — the
/// stream's name, which is this resource's physical id — set on the way out.
///
/// The same call serves an UPDATE of the stream: "Creates or updates a metric stream …
/// When you use PutMetricStream to create a new metric stream, the stream is created in
/// the running state. If you use it to update an existing stream, the state of the stream
/// is not changed" (`PutMetricStream`). That is why [`put_metric_stream_configuration`] is
/// also the type's [`Provisioner::update`]: a deployment that changes a declared filter
/// reaches CloudWatch as the configuration the template now declares, under the same name.
async fn put_metric_stream(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut properties = match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    // The stream carries the tags the declaration asks for plus the ones CloudFormation
    // gives the resource ([`tags_of`]), as the `[{Key, Value}]` list CloudWatch's own `Tag`
    // shape spells. Only on the CREATE: "If you're using PutMetricStream to update an
    // existing stream, you can't change the tags of the existing stream"
    // (`PutMetricStream`), which is why the update path below sends the declaration as it
    // stands.
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        properties.insert("Tags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
    put_metric_stream_configuration(
        ctx,
        region,
        &resource.physical_resource_id,
        &Value::Object(properties),
    )
    .await
}

/// `PutMetricStream` with `properties` as the request and `name` as the one member
/// CloudFormation decides — the call behind both this type's create and its update, so the
/// two cannot send a different request for the same declaration. `properties` is the
/// configuration to put: the resource's own on a create, and the document the deployment
/// DECLARES on an update.
async fn put_metric_stream_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    properties: &Value,
) -> Result<Created, Refusal> {
    let mut request = match properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    request.insert("Name".to_string(), Value::String(name.to_string()));
    let reported = call_json_rpc(
        ctx,
        CLOUDWATCH_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{CLOUDWATCH_TARGET_PREFIX}.PutMetricStream"),
        &Value::Object(request),
    )
    .await?;
    let mut attributes = BTreeMap::new();
    if let Some(arn) = reported.get("Arn").and_then(Value::as_str) {
        attributes.insert("Arn".to_string(), arn.to_string());
    }
    Ok(Created::attributes(attributes))
}

/// Delete the metric stream a stack created: "Permanently deletes the metric stream that
/// you specify."
///
/// `DeleteMetricStream` models no not-found error at all (`InternalServiceFault`,
/// `InvalidParameterValueException` and `MissingRequiredParameterException` are its whole
/// error list), unlike `GetMetricStream`, so a stream CloudWatch no longer holds is not
/// refused and there is no not-found code to treat as success here.
async fn delete_metric_stream(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "Name": physical_resource_id });
    call_json_rpc(
        ctx,
        CLOUDWATCH_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{CLOUDWATCH_TARGET_PREFIX}.DeleteMetricStream"),
        &request,
    )
    .await
    .map(|_| ())
}

/// The metric stream's configuration as CloudWatch reports it, for drift detection: the
/// `GetMetricStream` answer, whose members are the type's own property names. A stream
/// CloudWatch no longer holds (`ResourceNotFoundException`) was deleted.
async fn describe_metric_stream(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "Name": physical_resource_id });
    match call_json_rpc(
        ctx,
        CLOUDWATCH_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{CLOUDWATCH_TARGET_PREFIX}.GetMetricStream"),
        &request,
    )
    .await
    {
        Ok(answer) => Ok(Some(answer)),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

// ---------------------------------------------------------------------------
// Amazon ECR Public: AWS::ECR::PublicRepository
// ---------------------------------------------------------------------------

/// Amazon ECR Public's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: ecr-public`, service shape `SpencerFrontendService`), which speaks
/// `awsJson1_1`.
const ECR_PUBLIC_SIGNING_NAME: &str = "ecr-public";
const ECR_PUBLIC_TARGET_PREFIX: &str = "SpencerFrontendService";

/// The error code ECR Public answers for a repository it does not hold
/// (`RepositoryNotFoundException`: "The specified repository can't be found").
const REPOSITORY_NOT_FOUND: &str = "RepositoryNotFoundException";

/// What `AWS::ECR::PublicRepository`'s `RepositoryCatalogData` property declares, as
/// `CreateRepository`'s `catalogData` member spells it.
///
/// Source: AWS's CloudFormation resource reference for the type, member by member against
/// the `RepositoryCatalogDataInput` shape. `LogoImageBlob` is base64 on both sides (the
/// property is "the base64-encoded repository logo payload", and `awsJson1_1` carries a
/// blob as base64), so it goes through as the template spells it.
const CATALOG_DATA_MEMBERS: &[(&str, &str)] = &[
    ("RepositoryDescription", "description"),
    ("AboutText", "aboutText"),
    ("UsageText", "usageText"),
    ("LogoImageBlob", "logoImageBlob"),
    ("Architectures", "architectures"),
    ("OperatingSystems", "operatingSystems"),
];

/// Create the repository an `AWS::ECR::PublicRepository` declares.
///
/// The repository's own ARN is read back off the answer: it is the resource's real
/// identity, so it replaces the one the naming rules computed.
async fn create_public_repository(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let properties = match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    let mut request = Map::new();
    // The repository's name is this resource's physical id — "Ref returns the repository
    // name" — already resolved against the stack's parameters.
    request.insert(
        "repositoryName".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    if let Some(Value::Object(declared)) = properties.get("RepositoryCatalogData") {
        let mut catalog_data = Map::new();
        for (property, member) in CATALOG_DATA_MEMBERS {
            if let Some(value) = declared.get(*property) {
                catalog_data.insert((*member).to_string(), value.clone());
            }
        }
        request.insert("catalogData".to_string(), Value::Object(catalog_data));
    }
    // `Tags` is a list of `{Key, Value}` on both sides: ECR Public's own `Tag` shape
    // spells its members `Key` and `Value` too, so the declared list goes through as
    // declared, carrying the tags CloudFormation gives the repository beside them.
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("tags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
    let reported = call_json_rpc(
        ctx,
        ECR_PUBLIC_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ECR_PUBLIC_TARGET_PREFIX}.CreateRepository"),
        &Value::Object(request),
    )
    .await?;
    let mut attributes = BTreeMap::new();
    if let Some(arn) = reported
        .get("repository")
        .and_then(|repository| repository.get("repositoryArn"))
        .and_then(Value::as_str)
    {
        attributes.insert("Arn".to_string(), arn.to_string());
    }
    // The repository was created under the name CloudFormation decided.
    Ok(Created::attributes(attributes))
}

/// Delete the repository a stack created. A repository ECR Public no longer holds
/// (`RepositoryNotFoundException`) is already in the state the delete asks for; one that
/// still holds images is NOT deleted (`RepositoryNotEmptyException`), and that refusal is
/// what the stack reports.
async fn delete_public_repository(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "repositoryName": physical_resource_id });
    match call_json_rpc(
        ctx,
        ECR_PUBLIC_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ECR_PUBLIC_TARGET_PREFIX}.DeleteRepository"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == REPOSITORY_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The repository's configuration as Amazon ECR Public reports it, for drift detection. A
/// repository it no longer holds (`RepositoryNotFoundException`, and an answer carrying no
/// repository at all) was deleted.
async fn describe_public_repository(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "repositoryNames": [physical_resource_id] });
    match call_json_rpc(
        ctx,
        ECR_PUBLIC_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ECR_PUBLIC_TARGET_PREFIX}.DescribeRepositories"),
        &request,
    )
    .await
    {
        Ok(answer) => Ok(first_listed(&answer, "repositories")),
        Err(refusal) if refusal.code == REPOSITORY_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

// ---------------------------------------------------------------------------
// OpenSearch Serverless: AWS::OpenSearchServerless::Collection / ::SecurityPolicy
// ---------------------------------------------------------------------------

/// OpenSearch Serverless's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: aoss`, service shape `OpenSearchServerless`).
const AOSS_SIGNING_NAME: &str = "aoss";
const AOSS_TARGET_PREFIX: &str = "OpenSearchServerless";

/// Create the policy an `AWS::OpenSearchServerless::SecurityPolicy` declares.
///
/// The resource declares `Name`, `Type`, `Policy` and `Description` against an API that
/// takes `name`, `type`, `policy` and `description`, so those four are mapped here. `Ref`
/// on the resource returns the policy's NAME, which is the physical id the deployment
/// already computed from the `Name` property, so the service assigns nothing this record
/// has to take over.
async fn create_security_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let mut request = Map::new();
    request.insert(
        "name".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    request.insert("type".to_string(), Value::String(declared_type(resource)?));
    request.insert(
        "policy".to_string(),
        Value::String(policy_document(resource)?),
    );
    if let Some(description) = property(resource, "Description") {
        request.insert("description".to_string(), Value::String(description));
    }
    call_json_rpc(
        ctx,
        AOSS_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{AOSS_TARGET_PREFIX}.CreateSecurityPolicy"),
        &Value::Object(request),
    )
    .await?;
    // `AWS::OpenSearchServerless::SecurityPolicy` publishes no `Fn::GetAtt` attributes, so
    // an attribute read against it yields no value rather than a guessed one.
    Ok(Created::default())
}

/// Delete the policy a stack created, by the `(type, name)` pair that identifies one. A
/// policy the service no longer has is already in the state the delete asks for.
async fn delete_security_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let kind = declared_type(resource)?;
    let request = serde_json::json!({
        "type": kind,
        "name": resource.physical_resource_id,
    });
    match call_json_rpc(
        ctx,
        AOSS_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{AOSS_TARGET_PREFIX}.DeleteSecurityPolicy"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// Create the collection an `AWS::OpenSearchServerless::Collection` declares.
///
/// `Ref` on this resource returns the collection's **Id** — the server-minted
/// `CollectionId`, which is what `BatchGetCollection` and `CreateIndex` address a
/// collection by — so the create's own answer replaces the computed physical id, and
/// `Fn::GetAtt` reads `Id`, `Arn`, `CollectionEndpoint` and `DashboardEndpoint` off what
/// the owning service reports rather than off anything rebuilt here. The resource's
/// `Name`, `Type`, `Description`, `StandbyReplicas` and `Tags` are mapped onto the API's
/// own members, and its `tags` carry the stack's propagated tags beside the declared ones
/// ([`tags_of`]).
async fn create_collection(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = Map::new();
    request.insert(
        "name".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    for (declared, member) in [
        ("Type", "type"),
        ("Description", "description"),
        ("StandbyReplicas", "standbyReplicas"),
    ] {
        if let Some(value) = property(resource, declared) {
            request.insert(member.to_string(), Value::String(value));
        }
    }
    let tags: Vec<Value> = tags_of(&resource.properties, propagated)
        .into_iter()
        .map(|(key, value)| serde_json::json!({ "key": key, "value": value }))
        .collect();
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Array(tags));
    }
    let reported = call_json_rpc(
        ctx,
        AOSS_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{AOSS_TARGET_PREFIX}.CreateCollection"),
        &Value::Object(request),
    )
    .await?;
    let detail = reported
        .get("createCollectionDetail")
        .cloned()
        .unwrap_or(Value::Null);
    let id = detail
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "the created collection carries no id".to_string(),
        })?
        .to_string();
    let mut attributes = BTreeMap::new();
    attributes.insert("Id".to_string(), id.clone());
    if let Some(arn) = detail.get("arn").and_then(Value::as_str) {
        attributes.insert("Arn".to_string(), arn.to_string());
    }
    // The endpoints are derived from the collection's id BY THE OWNING SERVICE, so they
    // are read back from it rather than rebuilt here. A read that does not answer them
    // leaves those attributes unset rather than guessed.
    let read = call_json_rpc(
        ctx,
        AOSS_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{AOSS_TARGET_PREFIX}.BatchGetCollection"),
        &serde_json::json!({ "ids": [id] }),
    )
    .await
    .ok();
    let detail = read
        .as_ref()
        .and_then(|answer| answer.get("collectionDetails"))
        .and_then(Value::as_array)
        .and_then(|details| details.first());
    if let Some(detail) = detail {
        for (member, attribute) in [
            ("collectionEndpoint", "CollectionEndpoint"),
            ("dashboardEndpoint", "DashboardEndpoint"),
        ] {
            if let Some(endpoint) = detail.get(member).and_then(Value::as_str) {
                attributes.insert(attribute.to_string(), endpoint.to_string());
            }
        }
    }
    Ok(Created {
        physical_resource_id: Some(id),
        attributes,
    })
}

/// Delete the collection a stack created, by the id the create reported. A collection the
/// service no longer has is already in the state the delete asks for.
async fn delete_collection(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "id": physical_resource_id });
    match call_json_rpc(
        ctx,
        AOSS_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{AOSS_TARGET_PREFIX}.DeleteCollection"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

// ---------------------------------------------------------------------------
// Amazon S3: AWS::S3::Bucket
// ---------------------------------------------------------------------------

/// The error code Amazon S3 answers for a bucket it does not hold.
const NO_SUCH_BUCKET: &str = "NoSuchBucket";

/// Create the bucket an `AWS::S3::Bucket` declares, and apply the
/// `VersioningConfiguration` and the tags it carries.
///
/// "Ref returns the bucket name" for this type, so the physical resource id the stack
/// reports IS the bucket a client addresses, and it is the bucket created here.
/// `CreateBucketConfiguration$LocationConstraint` is what decides a bucket's Region and
/// "buckets in Region us-east-1 have a LocationConstraint of null", so the stack's own
/// Region is sent as that constraint for every other Region and as nothing for that one.
///
/// `CreateBucket` takes no tag set at all, so the bucket's tags are a `PutBucketTagging` of
/// its own — "sets the tags for a bucket", the one call S3 publishes for them — carrying
/// the tags the declaration asks for plus the ones CloudFormation gives the resource
/// ([`tags_of`]). It is sent only where there are tags to set: an empty tag set is no
/// tagging rather than a call that would clear one.
async fn create_bucket(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    use ::aws_sdk_s3::types::{
        BucketLocationConstraint, BucketVersioningStatus, CreateBucketConfiguration, Tag, Tagging,
        VersioningConfiguration,
    };

    let client = s3_client(ctx, region);
    let bucket = &resource.physical_resource_id;
    let configuration = (region != super::DEFAULT_REGION).then(|| {
        CreateBucketConfiguration::builder()
            .location_constraint(BucketLocationConstraint::from(region))
            .build()
    });
    if let Err(error) = client
        .create_bucket()
        .bucket(bucket)
        .set_create_bucket_configuration(configuration)
        .send()
        .await
    {
        // Whatever Amazon S3 answers short of success is a bucket this deployment did not
        // create, including a name that is already in use: "for legacy compatibility, if
        // you re-create an existing bucket that you already own in the North Virginia
        // Region, Amazon S3 returns 200 OK" is a SUCCESS S3 itself answers where it holds
        // — nothing here has to stand in for it — and everywhere else a name in use is a
        // refusal, which fails the resource like any other.
        return Err(refusal_of(&error));
    }
    // "VersioningConfiguration — enables multiple versions of all objects in this bucket",
    // whose one member is `Status`. The value is the template's: S3's own deserializer is
    // what judges it against `BucketVersioningStatus`.
    if let Some(status) = resource
        .properties
        .get("VersioningConfiguration")
        .and_then(|configuration| configuration.get("Status"))
        .and_then(Value::as_str)
    {
        client
            .put_bucket_versioning()
            .bucket(bucket)
            .versioning_configuration(
                VersioningConfiguration::builder()
                    .status(BucketVersioningStatus::from(status))
                    .build(),
            )
            .send()
            .await
            .map_err(|error| refusal_of(&error))?;
    }
    // The bucket's whole tag set, in the one call Amazon S3 takes it through.
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        let mut tag_set = Vec::with_capacity(tags.len());
        for (key, value) in &tags {
            tag_set.push(
                Tag::builder()
                    .key(key)
                    .value(value)
                    .build()
                    .map_err(|error| Refusal {
                        code: "InternalFailure".to_string(),
                        message: format!("the tag {key} cannot be sent to Amazon S3: {error}"),
                    })?,
            );
        }
        let tagging = Tagging::builder()
            .set_tag_set(Some(tag_set))
            .build()
            .map_err(|error| Refusal {
                code: "InternalFailure".to_string(),
                message: format!("the tag set for {bucket} cannot be sent: {error}"),
            })?;
        client
            .put_bucket_tagging()
            .bucket(bucket)
            .tagging(tagging)
            .send()
            .await
            .map_err(|error| refusal_of(&error))?;
    }
    Ok(Created::default())
}

/// Delete the bucket a stack created. A bucket S3 no longer has is already in the state
/// the delete asks for; one that still holds objects is NOT deleted, and that refusal is
/// what the stack reports.
async fn delete_bucket(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    match s3_client(ctx, region)
        .delete_bucket()
        .bucket(physical_resource_id)
        .send()
        .await
    {
        Ok(_) => Ok(()),
        Err(error) => {
            let refusal = refusal_of(&error);
            if refusal.code == NO_SUCH_BUCKET {
                return Ok(());
            }
            Err(refusal)
        }
    }
}

/// The error code Amazon S3 answers for a bucket a `HeadBucket` does not find
/// (`HeadBucketOutput`'s one modelled error is `NotFound`).
const BUCKET_NOT_FOUND: &str = "NotFound";

/// The bucket's configuration as Amazon S3 reports it, for drift detection.
///
/// A bucket S3 no longer holds was deleted. For one it still holds, the configuration
/// reported is the `VersioningConfiguration` the create applies — the one property of this
/// type this emulator sets in S3 — under the property name the template spells it with, so
/// a bucket whose versioning was changed outside the stack is MODIFIED. A versioning read
/// the owning service refuses leaves the bucket reported as existing with no comparable
/// property rather than failing the whole detection.
async fn describe_bucket(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let client = s3_client(ctx, region);
    if let Err(error) = client
        .head_bucket()
        .bucket(physical_resource_id)
        .send()
        .await
    {
        let refusal = refusal_of(&error);
        if refusal.code == BUCKET_NOT_FOUND || refusal.code == NO_SUCH_BUCKET {
            return Ok(None);
        }
        return Err(refusal);
    }
    let mut actual = Map::new();
    if let Ok(versioning) = client
        .get_bucket_versioning()
        .bucket(physical_resource_id)
        .send()
        .await
    {
        if let Some(status) = versioning.status() {
            actual.insert(
                "VersioningConfiguration".to_string(),
                serde_json::json!({ "Status": status.as_str() }),
            );
        }
    }
    Ok(Some(Value::Object(actual)))
}

// ---------------------------------------------------------------------------
// Amazon EMR: AWS::EMR::Cluster
// ---------------------------------------------------------------------------

/// Amazon EMR's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: elasticmapreduce`, service shape `ElasticMapReduce`).
const EMR_SIGNING_NAME: &str = "elasticmapreduce";
const EMR_TARGET_PREFIX: &str = "ElasticMapReduce";

/// The error code Amazon EMR answers for a cluster id it does not hold.
const INVALID_REQUEST: &str = "InvalidRequestException";

/// The deepest nesting of `Configurations` inside `Configurations` this arm descends into.
///
/// The declared properties come from a caller-supplied template body, so the nesting is the
/// REQUESTER's choice and a recursive walk over it must be bounded by a constant rather
/// than by the input: exhausting the native stack aborts the process instead of unwinding.
/// Chosen, and deliberately shallow, because EMR's own documented nesting is one level of
/// application configuration inside another — a deeper chain is a template built to explode
/// the walk, and it is REFUSED rather than quietly truncated.
const MAX_CONFIGURATION_NESTING: usize = 8;

/// `InstanceGroupConfig$InstanceRole` for the three groups an `AWS::EMR::Cluster` declares
/// one property at a time, and which `RunJobFlow` takes as ONE list whose entries carry the
/// role. The one shape difference between the template's `Instances` and the API's.
const INSTANCE_GROUP_ROLES: &[(&str, &str)] = &[
    ("MasterInstanceGroup", "MASTER"),
    ("CoreInstanceGroup", "CORE"),
    ("TaskInstanceGroups", "TASK"),
];

/// The same fold for the FLEET form of the same three node types: the template declares
/// `MasterInstanceFleet`, `CoreInstanceFleet` and `TaskInstanceFleets` one property at a
/// time where `RunJobFlow` takes one `InstanceFleets` list whose entries carry
/// `InstanceFleetConfig$InstanceFleetType` ("The node type that the instance fleet hosts.
/// Valid values are MASTER, CORE, and TASK").
///
/// A cluster may declare groups or fleets but not both ("A valid JobFlowInstancesConfig
/// must contain either InstanceGroups or InstanceFleets. They cannot be used together"),
/// which Amazon EMR's own `RunJobFlow` refuses — so a template declaring both is carried to
/// EMR as declared and left to that refusal rather than judged here.
const INSTANCE_FLEET_ROLES: &[(&str, &str)] = &[
    ("MasterInstanceFleet", "MASTER"),
    ("CoreInstanceFleet", "CORE"),
    ("TaskInstanceFleets", "TASK"),
];

/// Run the job flow an `AWS::EMR::Cluster` declares, and report the `j-` job-flow id EMR
/// ASSIGNED it.
///
/// Unlike a stream, a repository or a bucket, this identifier is not CloudFormation's to
/// decide: "Ref returns the cluster ID", which exists only once EMR has run the flow — so
/// it is answered as the resource's own physical id ([`Created::physical_resource_id`]),
/// which [`super::provisioning::reconcile`] adopts, and the `Id`/`ClusterId` attributes are
/// re-pointed at it ([`super::template::identity_attributes`]).
///
/// Most of the type's properties ARE `RunJobFlow`'s own members (`ReleaseLabel`,
/// `Applications`, `BootstrapActions`, `Tags`, the roles and the scalars), so they are
/// passed through verbatim. Two are shaped differently and are mapped: `Instances`, whose
/// per-role capacity properties the API takes as one `InstanceGroups` or `InstanceFleets`
/// list, and `Configurations`, whose property map the template spells
/// `ConfigurationProperties` where the API spells it `Properties`. `Name` is the one member
/// CloudFormation decides when the template declares none: EMR requires a cluster name, so
/// the generated resource name is used.
///
/// The cluster carries the tags the declaration asks for plus the ones CloudFormation gives
/// the resource ([`tags_of`]), as `RunJobFlow`'s own `Tags` list — the `[{Key, Value}]`
/// spelling Amazon EMR's `Tag` shape uses, which is the type's own spelling too.
async fn run_job_flow(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let declared = match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    let mut request = Map::new();
    for (member, value) in &declared {
        match member.as_str() {
            "Instances" => {
                request.insert("Instances".to_string(), instances(value)?);
            }
            "Configurations" => {
                request.insert("Configurations".to_string(), configurations(value, 0)?);
            }
            _ => {
                request.insert(member.clone(), value.clone());
            }
        }
    }
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("Tags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
    let name = declared
        .get("Name")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| resource.physical_resource_id.clone());
    request.insert("Name".to_string(), Value::String(name));

    let reported = call_json_rpc(
        ctx,
        EMR_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{EMR_TARGET_PREFIX}.RunJobFlow"),
        &Value::Object(request),
    )
    .await?;
    let Some(job_flow_id) = reported.get("JobFlowId").and_then(Value::as_str) else {
        // EMR answered a success carrying no job-flow id: there is no identifier to report,
        // and nothing here invents one.
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: "Amazon EMR reported no JobFlowId for the job flow it ran".to_string(),
        });
    };
    Ok(Created {
        // The job-flow id is the cluster's identity, not an attribute beside it: "Ref
        // returns the cluster ID".
        physical_resource_id: Some(job_flow_id.to_string()),
        attributes: BTreeMap::new(),
    })
}

/// The cluster's configuration as Amazon EMR reports it, for drift detection:
/// `DescribeCluster`'s own record, read by the `j-` job-flow id EMR minted.
///
/// A cluster id EMR does not hold is refused with its own `InvalidRequestException` — "the
/// cluster id is not valid or does not exist" — which is the resource having been deleted.
/// The record's members are EMR's own (`Name`, `ReleaseLabel`, `Applications`,
/// `Configurations`), so a declared property spelled identically on both sides takes part in
/// the comparison and one spelled differently leaves the cluster reported as existing rather
/// than as modified, exactly as [`super::drift`] documents for every arm here.
async fn describe_job_flow(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "ClusterId": physical_resource_id });
    match call_json_rpc(
        ctx,
        EMR_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{EMR_TARGET_PREFIX}.DescribeCluster"),
        &request,
    )
    .await
    {
        Ok(answer) => Ok(answer.get("Cluster").cloned()),
        Err(refusal) if refusal.code == INVALID_REQUEST => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Terminate the job flow a stack created.
///
/// `TerminateJobFlows` "shuts a list of clusters down" and answers nothing; a cluster EMR
/// does not have is refused with its own `InvalidRequestException`, which is the state the
/// delete asks for.
async fn terminate_job_flow(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "JobFlowIds": [physical_resource_id] });
    match call_json_rpc(
        ctx,
        EMR_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{EMR_TARGET_PREFIX}.TerminateJobFlows"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == INVALID_REQUEST => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The `Instances` an `AWS::EMR::Cluster` declares, as the `JobFlowInstancesConfig`
/// `RunJobFlow` takes: every member passed through, and the per-role capacity properties
/// folded onto the one list the API takes for that form — the three GROUP properties onto
/// `InstanceGroups`, keyed by `InstanceRole`, and the three FLEET properties onto
/// `InstanceFleets`, keyed by `InstanceFleetType`. Nothing a template declares is dropped:
/// a cluster created with less capacity than its template asked for is not the cluster the
/// template described.
fn instances(declared: &Value) -> Result<Value, Refusal> {
    let Value::Object(members) = declared else {
        return Ok(declared.clone());
    };
    let mut config = Map::new();
    let mut groups: Vec<Value> = Vec::new();
    let mut fleets: Vec<Value> = Vec::new();
    for (member, value) in members {
        if let Some((_, role)) = INSTANCE_GROUP_ROLES
            .iter()
            .find(|(property, _)| property == member)
        {
            fold_per_role(member, value, "InstanceRole", role, &mut groups)?;
            continue;
        }
        if let Some((_, role)) = INSTANCE_FLEET_ROLES
            .iter()
            .find(|(property, _)| property == member)
        {
            fold_per_role(member, value, "InstanceFleetType", role, &mut fleets)?;
            continue;
        }
        config.insert(member.clone(), value.clone());
    }
    if !groups.is_empty() {
        config.insert("InstanceGroups".to_string(), Value::Array(groups));
    }
    if !fleets.is_empty() {
        config.insert("InstanceFleets".to_string(), Value::Array(fleets));
    }
    Ok(Value::Object(config))
}

/// Fold one per-role capacity property onto the list the API takes, stamping the role onto
/// each entry under `role_member`.
///
/// The task-node property is a LIST (`TaskInstanceGroups`, `TaskInstanceFleets`) where the
/// master and core ones are single structures, so both spellings are accepted. An entry
/// that is not a structure at all is REFUSED rather than dropped: it is capacity the
/// template asked for and this arm cannot carry.
fn fold_per_role(
    property: &str,
    declared: &Value,
    role_member: &str,
    role: &str,
    onto: &mut Vec<Value>,
) -> Result<(), Refusal> {
    let entries: Vec<&Value> = match declared {
        Value::Array(items) => items.iter().collect(),
        single => vec![single],
    };
    for entry in entries {
        let Value::Object(entry) = entry else {
            return Err(malformed_property(property));
        };
        let mut built = entry.clone();
        built.insert(role_member.to_string(), Value::String(role.to_string()));
        onto.push(Value::Object(built));
    }
    Ok(())
}

/// The `Configurations` an `AWS::EMR::Cluster` declares, as the `Configuration` list
/// `RunJobFlow` takes — the application configurations (`hive-site`, `spark-hive-site`, …)
/// a client reads back off the cluster.
///
/// One member is spelled differently on the two sides: the template's property map is
/// `ConfigurationProperties` where the API's is `Properties`. Nested `Configurations` are
/// mapped the same way, to the depth the template declares them; a chain of more than
/// [`MAX_CONFIGURATION_NESTING`] levels is REFUSED, because a cluster created with part of
/// the configuration its template declared is not the cluster the template described.
///
/// `depth` is how many levels are already above this one, so the outermost list is `0` and
/// the deepest list this carries is `MAX_CONFIGURATION_NESTING - 1`: exactly
/// [`MAX_CONFIGURATION_NESTING`] levels, which is what that constant names.
fn configurations(declared: &Value, depth: usize) -> Result<Value, Refusal> {
    if depth >= MAX_CONFIGURATION_NESTING {
        return Err(Refusal {
            code: "ValidationException".to_string(),
            message: format!(
                "Configurations are nested deeper than the {MAX_CONFIGURATION_NESTING} levels \
                 this emulator carries, so the cluster was not created"
            ),
        });
    }
    let Value::Array(items) = declared else {
        return Ok(declared.clone());
    };
    let mut mapped = Vec::with_capacity(items.len());
    for configuration in items {
        let Value::Object(members) = configuration else {
            return Err(malformed_property("Configurations"));
        };
        let mut built = Map::new();
        for (member, value) in members {
            match member.as_str() {
                "ConfigurationProperties" => {
                    built.insert("Properties".to_string(), value.clone());
                }
                "Configurations" => {
                    built.insert(
                        "Configurations".to_string(),
                        configurations(value, depth + 1)?,
                    );
                }
                _ => {
                    built.insert(member.clone(), value.clone());
                }
            }
        }
        mapped.push(Value::Object(built));
    }
    Ok(Value::Array(mapped))
}

/// The refusal for a declared property this arm cannot map onto the owning API's member,
/// named so a caller can tell WHICH property it was. Refusing is what keeps a resource from
/// being created with less than the template declared.
fn malformed_property(property: &str) -> Refusal {
    Refusal {
        code: "ValidationException".to_string(),
        message: format!("Property {property} is not shaped as the resource type declares it"),
    }
}

// ---------------------------------------------------------------------------
// IAM: AWS::IAM::Role
// ---------------------------------------------------------------------------

/// The error code IAM answers for an entity it does not hold.
const NO_SUCH_ENTITY: &str = "NoSuchEntity";

/// Create the role an `AWS::IAM::Role` declares, with its trust policy and its inline
/// policies.
///
/// `AssumeRolePolicyDocument` is REQUIRED on this resource type and on IAM's own
/// `CreateRole`, so a declaration carrying none describes no role that could be created:
/// the resource is refused here, naming the property, rather than a role being created
/// with a trust policy this module invented or a stack completing over a role nothing
/// created.
///
/// A name IAM will not create a role under — `EntityAlreadyExists`, which is what an IAM
/// name the ACCOUNT already holds answers, since role names are account-global — fails the
/// resource like any other refusal: the role that is there is not this stack's, and a
/// stack never reports a resource it did not create.
///
/// `ManagedPolicyArns` is applied too, with `AttachRolePolicy` per entry: IAM is what
/// judges whether a policy ARN is attachable, so a policy it refuses fails the resource
/// like any other refusal rather than leaving the stack complete over a role without the
/// permissions its template declared.
///
/// The role carries the tags the declaration asks for plus the ones CloudFormation gives
/// the resource ([`tags_of`]), as `CreateRole`'s own `Tags` member.
async fn create_role(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let Some(trust_policy) = resource
        .properties
        .get("AssumeRolePolicyDocument")
        .and_then(document_text)
    else {
        return Err(Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!(
                "Property AssumeRolePolicyDocument cannot be empty for resource of type {}",
                resource.resource_type
            ),
        });
    };
    let client = iam_client(ctx, region);
    let role_name = &resource.physical_resource_id;
    let mut role_tags = Vec::new();
    for (key, value) in tags_of(&resource.properties, propagated) {
        role_tags.push(
            ::aws_sdk_iam::types::Tag::builder()
                .key(&key)
                .value(&value)
                .build()
                .map_err(|error| Refusal {
                    code: "InternalFailure".to_string(),
                    message: format!("the tag {key} cannot be sent to IAM: {error}"),
                })?,
        );
    }
    let created = client
        .create_role()
        .role_name(role_name)
        .assume_role_policy_document(trust_policy)
        // `Path`, `Description` and `MaxSessionDuration` are spelled the same on the
        // resource type and on `CreateRole`; a property the template leaves out is not
        // sent.
        .set_path(resource.properties.get("Path").and_then(scalar_text))
        .set_description(resource.properties.get("Description").and_then(scalar_text))
        .set_max_session_duration(whole_number(resource.properties.get("MaxSessionDuration")))
        .set_tags((!role_tags.is_empty()).then_some(role_tags))
        .send()
        .await;
    let mut attributes = BTreeMap::new();
    match created {
        Ok(answer) => {
            // What IAM answered wins over the computed ARN: it is the role's real
            // identity.
            if let Some(role) = answer.role() {
                attributes.insert("Arn".to_string(), role.arn().to_string());
                // `RoleId` is the UNIQUE ID IAM mints for a role ("Returns the stable and
                // unique string identifying the role"), which only IAM can know: without
                // it the attribute keeps the value the uniform convention read off the
                // role's NAME, and a client comparing the stack's output with IAM's own
                // record sees two different ids for one role.
                attributes.insert("RoleId".to_string(), role.role_id().to_string());
            }
        }
        Err(error) => {
            // What IAM answered is what the resource is: a name it will not create a role
            // under is a role this deployment did not create, and CloudFormation reports
            // no resource it did not create. IAM role names are unique across the whole
            // ACCOUNT, so this is also what a template whose role name is not
            // Region-qualified gets when it is deployed a second time — the refusal
            // belongs to the stack, not to this module.
            return Err(refusal_of(&error));
        }
    }
    // "Policies — adds or updates an inline policy document that is embedded in the
    // specified IAM role", each entry a `{PolicyName, PolicyDocument}` pair.
    for policy in inline_policies(resource) {
        let (Some(name), Some(body)) = (
            policy.get("PolicyName").and_then(scalar_text),
            policy.get("PolicyDocument").and_then(document_text),
        ) else {
            continue;
        };
        client
            .put_role_policy()
            .role_name(role_name)
            .policy_name(name)
            .policy_document(body)
            .send()
            .await
            .map_err(|error| refusal_of(&error))?;
    }
    // "ManagedPolicyArns — a list of Amazon Resource Names (ARNs) of the IAM managed
    // policies that you want to attach to the role", each applied with the call that
    // attaches one.
    for policy_arn in managed_policy_arns(resource) {
        client
            .attach_role_policy()
            .role_name(role_name)
            .policy_arn(policy_arn)
            .send()
            .await
            .map_err(|error| refusal_of(&error))?;
    }
    Ok(Created::attributes(attributes))
}

/// Delete the role a stack created. "You must delete the policies that are associated
/// with the role" first — an attached managed policy is DETACHED and an inline policy
/// deleted — so each one the template declared goes before the role does; a policy or a
/// role IAM no longer has is already in the state the delete asks for.
async fn delete_role(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let client = iam_client(ctx, region);
    let role_name = &resource.physical_resource_id;
    for policy_arn in managed_policy_arns(resource) {
        if let Err(error) = client
            .detach_role_policy()
            .role_name(role_name)
            .policy_arn(policy_arn)
            .send()
            .await
        {
            let refusal = refusal_of(&error);
            if refusal.code != NO_SUCH_ENTITY {
                return Err(refusal);
            }
        }
    }
    for policy in inline_policies(resource) {
        let Some(name) = policy.get("PolicyName").and_then(scalar_text) else {
            continue;
        };
        if let Err(error) = client
            .delete_role_policy()
            .role_name(role_name)
            .policy_name(name)
            .send()
            .await
        {
            let refusal = refusal_of(&error);
            if refusal.code != NO_SUCH_ENTITY {
                return Err(refusal);
            }
        }
    }
    match client.delete_role().role_name(role_name).send().await {
        Ok(_) => Ok(()),
        Err(error) => {
            let refusal = refusal_of(&error);
            if refusal.code == NO_SUCH_ENTITY {
                return Ok(());
            }
            Err(refusal)
        }
    }
}

/// The role's configuration as IAM reports it, for drift detection.
///
/// A role IAM no longer holds (`NoSuchEntity`) was deleted. The members compared are the
/// ones the resource type and `GetRole` spell identically and the create sends — `Path`,
/// `Description` and `MaxSessionDuration` — so a role whose description or session length
/// was changed outside the stack is MODIFIED.
async fn describe_role(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let answer = match iam_client(ctx, region)
        .get_role()
        .role_name(physical_resource_id)
        .send()
        .await
    {
        Ok(answer) => answer,
        Err(error) => {
            let refusal = refusal_of(&error);
            if refusal.code == NO_SUCH_ENTITY {
                return Ok(None);
            }
            return Err(refusal);
        }
    };
    let mut actual = Map::new();
    if let Some(role) = answer.role() {
        actual.insert("Path".to_string(), Value::String(role.path().to_string()));
        if let Some(description) = role.description() {
            actual.insert(
                "Description".to_string(),
                Value::String(description.to_string()),
            );
        }
        if let Some(duration) = role.max_session_duration() {
            actual.insert("MaxSessionDuration".to_string(), Value::from(duration));
        }
    }
    Ok(Some(Value::Object(actual)))
}

/// The `Policies` entries a role declaration carries, as a slice — empty for a role that
/// declares none, so the create and the delete walk them the same way.
fn inline_policies(resource: &StoredResource) -> &[Value] {
    declared_list(resource, "Policies")
}

/// The `ManagedPolicyArns` a role declaration carries, each as the ARN text
/// `AttachRolePolicy` takes.
fn managed_policy_arns(resource: &StoredResource) -> Vec<String> {
    declared_list(resource, "ManagedPolicyArns")
        .iter()
        .filter_map(scalar_text)
        .collect()
}

/// One list-valued property of a resource, as a slice — empty for a property the template
/// does not declare or declares as something other than a list.
fn declared_list<'a>(resource: &'a StoredResource, property: &str) -> &'a [Value] {
    resource
        .properties
        .get(property)
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
}

// ---------------------------------------------------------------------------
// CodePipeline: AWS::CodePipeline::Pipeline
// ---------------------------------------------------------------------------

/// CodePipeline's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: codepipeline`, service shape `CodePipeline_20150709`).
const CODEPIPELINE_SIGNING_NAME: &str = "codepipeline";
const CODEPIPELINE_TARGET_PREFIX: &str = "CodePipeline_20150709";

/// The error code CodePipeline answers when a pipeline of that name already exists, which
/// is the resource this deployment wanted.
const PIPELINE_NAME_IN_USE: &str = "PipelineNameInUseException";

/// The error code CodePipeline answers for a pipeline it does not hold.
const PIPELINE_NOT_FOUND: &str = "PipelineNotFoundException";

/// Create the pipeline an `AWS::CodePipeline::Pipeline` declares.
///
/// This type's CloudFormation properties ARE `CreatePipeline`'s `PipelineDeclaration`,
/// member for member — `RoleArn`/`roleArn`, `ArtifactStore`/`artifactStore`,
/// `Stages`/`stages`, `ActionTypeId`/`actionTypeId` and the rest
/// (`smithy_query query="shape" shape="PipelineDeclaration"`) — differing only in the case
/// of each member's initial letter, so the evaluated property document is carried through
/// with that one transformation ([`camel_cased`]) rather than mapped member by member.
///
/// Three properties are not members of the declaration and are taken out of it: `Tags` is
/// a member of the `CreatePipeline` REQUEST beside the pipeline, and
/// `DisableInboundStageTransitions` and `RestartExecutionOnUpdate` are CloudFormation's
/// own (a held transition is `DisableStageTransition`'s business, and restart-on-update is
/// a deployment instruction) — so the request carries only what the model declares.
///
/// The pipeline carries the tags the declaration asks for plus the ones CloudFormation
/// gives the resource ([`tags_of`]), as the list CodePipeline's own `Tag` shape spells in
/// lower camel case.
async fn create_pipeline(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut declaration = match camel_cased(&resource.properties, 0)? {
        Value::Object(object) => object,
        _ => Map::new(),
    };
    // The one member CloudFormation itself decides.
    declaration.insert(
        "name".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    // `Tags` is the request's own member, not the declaration's, so it comes out of the
    // document the pipeline is.
    declaration.remove("tags");
    declaration.remove("disableInboundStageTransitions");
    declaration.remove("restartExecutionOnUpdate");
    // `ArtifactStores` is a LIST of `{Region, ArtifactStore}` in a template and
    // `ArtifactStoreMap`, keyed by Region, in the model: the same information either way.
    if let Some(stores) = declaration.remove("artifactStores") {
        let rewritten = match stores.as_array() {
            Some(entries) => {
                let mut by_region = Map::new();
                for entry in entries {
                    let (Some(entry_region), Some(store)) = (
                        entry.get("region").and_then(Value::as_str),
                        entry.get("artifactStore"),
                    ) else {
                        continue;
                    };
                    by_region.insert(entry_region.to_string(), store.clone());
                }
                Value::Object(by_region)
            }
            None => stores,
        };
        declaration.insert("artifactStores".to_string(), rewritten);
    }
    let mut request = Map::new();
    request.insert("pipeline".to_string(), Value::Object(declaration));
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("tags".to_string(), tags_as_list(&tags, LOWER_CASED_TAG));
    }
    match call_json_rpc(
        ctx,
        CODEPIPELINE_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{CODEPIPELINE_TARGET_PREFIX}.CreatePipeline"),
        &Value::Object(request),
    )
    .await
    {
        Ok(_) => Ok(Created::default()),
        Err(refusal) if refusal.code == PIPELINE_NAME_IN_USE => Ok(Created::default()),
        Err(refusal) => Err(refusal),
    }
}

/// Delete the pipeline a stack created. One CodePipeline no longer has is already in the
/// state the delete asks for.
async fn delete_pipeline(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    match call_json_rpc(
        ctx,
        CODEPIPELINE_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{CODEPIPELINE_TARGET_PREFIX}.DeletePipeline"),
        &serde_json::json!({ "name": physical_resource_id }),
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == PIPELINE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The pipeline's declaration as CodePipeline reports it, for drift detection. One it no
/// longer holds (`PipelineNotFoundException`) was deleted.
async fn describe_pipeline(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    match call_json_rpc(
        ctx,
        CODEPIPELINE_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{CODEPIPELINE_TARGET_PREFIX}.GetPipeline"),
        &serde_json::json!({ "name": physical_resource_id }),
    )
    .await
    {
        Ok(answer) => Ok(Some(reported_member(&answer, "pipeline"))),
        Err(refusal) if refusal.code == PIPELINE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

// ---------------------------------------------------------------------------
// Reading an evaluated property document
// ---------------------------------------------------------------------------

/// The deepest a re-casing walk descends into an evaluated property document.
///
/// The same rule, for the same reason, as [`MAX_CONFIGURATION_NESTING`]: the properties
/// come from a caller-supplied template body, so their nesting is the REQUESTER's choice
/// and a recursive walk over them has to be bounded by a constant rather than by the
/// input — exhausting the native stack ABORTS the process instead of unwinding, which
/// would take the emulator down for every caller rather than failing one stack. A document
/// nested deeper than this is REFUSED, never quietly truncated: a resource created from
/// part of the document its template declared is not the resource the template described.
///
/// Set to the template reader's own [`super::template::MAX_TEMPLATE_DEPTH`], the deepest
/// body that reader admits at all, so this bound refuses nothing a template could carry
/// and still holds when a property document reaches a walk from somewhere else.
const MAX_PROPERTY_NESTING: usize = super::template::MAX_TEMPLATE_DEPTH;

/// The same document with every object KEY's initial letter lower-cased, which is the one
/// difference between a CloudFormation resource type's property names and the API members
/// they are (`RoleArn` -> `roleArn`).
///
/// An `ActionDeclaration`'s `configuration` is the exception: `ActionConfigurationMap`'s
/// keys are the action PROVIDER's own, case-sensitive settings (`S3Bucket`,
/// `PollForSourceChanges`), not modelled member names, so the entries of that one map are
/// carried through verbatim.
///
/// `depth` is how many levels are already above this one, so the outermost call passes
/// `0`; a document deeper than [`MAX_PROPERTY_NESTING`] is refused rather than walked.
fn camel_cased(value: &Value, depth: usize) -> Result<Value, Refusal> {
    /// The one map whose KEYS are data rather than member names.
    const CONFIGURATION: &str = "configuration";

    if depth >= MAX_PROPERTY_NESTING {
        return Err(too_deeply_nested());
    }
    match value {
        Value::Object(members) => {
            let mut converted = Map::new();
            for (key, member) in members {
                let name = lower_initial(key);
                let member = if name == CONFIGURATION {
                    member.clone()
                } else {
                    camel_cased(member, depth + 1)?
                };
                converted.insert(name, member);
            }
            Ok(Value::Object(converted))
        }
        Value::Array(items) => {
            let mut converted = Vec::with_capacity(items.len());
            for item in items {
                converted.push(camel_cased(item, depth + 1)?);
            }
            Ok(Value::Array(converted))
        }
        scalar => Ok(scalar.clone()),
    }
}

/// The refusal a property document nested past [`MAX_PROPERTY_NESTING`] answers: the
/// resource is not created, which is what [`configurations`] answers for the same case.
fn too_deeply_nested() -> Refusal {
    Refusal {
        code: "ValidationException".to_string(),
        message: format!(
            "Resource properties are nested deeper than the {MAX_PROPERTY_NESTING} levels \
             this emulator carries, so the resource was not created"
        ),
    }
}

/// One name with its FIRST character lower-cased and nothing else touched, so an
/// initialism inside a member name keeps its case (`RoleARN` -> `roleARN`).
fn lower_initial(name: &str) -> String {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) => first.to_lowercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

/// One member of an answer document, or an empty object where the answer carried none: a
/// read that succeeded reported the resource, and a member it did not carry is no
/// configuration to compare rather than a missing resource.
fn reported_member(answer: &Value, member: &str) -> Value {
    answer
        .get(member)
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()))
}

/// The FIRST entry of a list an answer carries, or `None` for an answer that lists none —
/// which is how a describe-many call reports a resource the service does not hold.
fn first_listed(answer: &Value, member: &str) -> Option<Value> {
    answer
        .get(member)
        .and_then(Value::as_array)
        .and_then(|entries| entries.first())
        .cloned()
}

/// One JSON scalar as the text an API member takes, or `None` for a composite.
///
/// The one spelling rule, held in [`super::scalar_text`] so the create side and the drift
/// comparison that reads the resource back cannot disagree about it.
fn scalar_text(value: &Value) -> Option<String> {
    super::scalar_text(value)
}

/// A policy-document property as the JSON text the IAM member takes.
///
/// CloudFormation accepts a policy document as an inline object (JSON or YAML) or as a
/// string, and IAM takes it as text, so an object is rendered and a string passes through.
fn document_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Object(_) | Value::Array(_) => serde_json::to_string(value).ok(),
        _ => None,
    }
}

/// A property that carries a whole number, in either spelling a template gives one: JSON's
/// number, or the string a quoted YAML scalar leaves behind.
fn whole_number(value: Option<&Value>) -> Option<i32> {
    let value = value?;
    let whole = match value {
        Value::Number(number) => number.as_i64()?,
        Value::String(text) => text.parse().ok()?,
        _ => return None,
    };
    i32::try_from(whole).ok()
}

// ---------------------------------------------------------------------------
// The SDK clients an arm calls its owning service through
// ---------------------------------------------------------------------------

/// The error code this module reports for a declaration that is missing a property its
/// resource type requires, which is the code CloudFormation's own template validation
/// answers with.
const MISSING_PROPERTY_CODE: &str = "ValidationError";

/// The error code that stands in for one an answer did not carry: the call did not
/// succeed, and nothing on the other end identified the problem.
const UNIDENTIFIED_FAILURE: &str = "InternalFailure";

/// An SDK error as the [`Refusal`] the stack reports: the error code the owning service
/// NAMED and the message it worded, kept apart so the caller branches on the code.
///
/// An error carrying no code is still a refusal — the create did not succeed — reported
/// under [`UNIDENTIFIED_FAILURE`] with whatever the transport said.
fn refusal_of<E>(error: &E) -> Refusal
where
    E: ::aws_smithy_types::error::metadata::ProvideErrorMetadata + ::std::fmt::Display,
{
    Refusal {
        code: error.code().unwrap_or(UNIDENTIFIED_FAILURE).to_string(),
        message: error
            .message()
            .map_or_else(|| error.to_string(), ToString::to_string),
    }
}

/// The endpoint URL an in-process call to another emulated service is addressed at.
///
/// The host is built from `service` and the FIXED [`super::DEFAULT_REGION`] literal, never
/// from template-derived text: the framework's callback routes such a call in process on
/// the service label, and the client carries the emulator's own credentials, so no value a
/// caller sends may reach the host position. The stack's own Region is what the call is
/// ABOUT, and is the client's signing scope; it is not part of the destination.
fn in_process_endpoint(service: &str) -> String {
    format!(
        "https://{service}.{}.amazonaws.com",
        super::DEFAULT_REGION
    )
}

/// An Amazon S3 client over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated service:
/// it routes in process and signs with the emulator's own credentials, so no network
/// client is configured here. Path-style addressing is what the emulated S3 service routes
/// on (its operations are bound under a `{Bucket}` path label).
fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(in_process_endpoint("s3"))
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        // The transport is in-process: nothing to retry, time out or watch for a stalled
        // stream, and each would need a sleep implementation this client is not given.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// An IAM client over the same in-process callback, configured the same way.
fn iam_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_iam::Client {
    let config = ::aws_sdk_iam::Config::builder()
        .behavior_version(::aws_sdk_iam::config::BehaviorVersion::latest())
        .region(::aws_sdk_iam::config::Region::new(region.to_string()))
        .endpoint_url(in_process_endpoint("iam"))
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        .retry_config(::aws_sdk_iam::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_iam::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_iam::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_iam::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_iam::Client::from_conf(config)
}

// ---------------------------------------------------------------------------
// Amazon ECS: AWS::ECS::Cluster
// ---------------------------------------------------------------------------

/// Amazon ECS's signing name and `X-Amz-Target` prefix, from its model (`sigv4Name: ecs`,
/// service shape `AmazonEC2ContainerServiceV20141113`), which speaks `awsJson1_1`.
const ECS_SIGNING_NAME: &str = "ecs";
const ECS_TARGET_PREFIX: &str = "AmazonEC2ContainerServiceV20141113";

/// The error code Amazon ECS answers for a cluster it does not hold
/// (`ClusterNotFoundException`: "The specified cluster wasn't found").
const CLUSTER_NOT_FOUND: &str = "ClusterNotFoundException";

/// The `AWS::ECS::Cluster` properties `CreateCluster` takes, as (property, member) pairs.
///
/// Source: AWS's CloudFormation resource reference for the type, member by member against
/// the `CreateClusterRequest` shape. The Amazon ECS model spells its members in LOWER
/// CAMEL case, so each value's own members are re-cased by [`lower_camel_members`], and
/// one property's documented name is not its member's at all — a cluster's
/// `ClusterSettings` sets `settings` ("The setting to use when creating a cluster").
/// A property these pairs do not name is one `CreateCluster` has no member for; the
/// cluster's `ClusterName` is set from the physical id instead, and its `tags` by
/// [`tags_of`], which carries the stack's propagated tags beside the declared ones.
const ECS_CLUSTER_MEMBERS: &[(&str, &str)] = &[
    ("CapacityProviders", "capacityProviders"),
    ("ClusterSettings", "settings"),
    ("Configuration", "configuration"),
    (
        "DefaultCapacityProviderStrategy",
        "defaultCapacityProviderStrategy",
    ),
    ("ServiceConnectDefaults", "serviceConnectDefaults"),
];

/// Create the cluster an `AWS::ECS::Cluster` declares, so a client reading the stack's
/// outputs and then calling Amazon ECS against that cluster (`ListTasks`,
/// `DescribeClusters`) finds it there rather than a `ClusterNotFoundException`.
///
/// `Ref` of this type returns the cluster NAME, which is this resource's physical id, so
/// the cluster is created under exactly the name the stack reports for it; the ARN it
/// reports back is the resource's real identity and replaces the computed one.
///
/// The cluster carries the tags the declaration asks for plus the ones CloudFormation gives
/// the resource ([`tags_of`]), as the list Amazon ECS's own `Tag` shape spells in lower
/// camel case — less the `aws:cloudformation:*` ones, which `CreateCluster` refuses to a
/// caller like the other reserved-namespace APIs ([`Provisioner::applicable_tags`]).
async fn create_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = Map::new();
    if let Value::Object(properties) = &resource.properties {
        for (property, member) in ECS_CLUSTER_MEMBERS {
            if let Some(value) = properties.get(*property) {
                request.insert((*member).to_string(), lower_camel_members(value, 0)?);
            }
        }
    }
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("tags".to_string(), tags_as_list(&tags, LOWER_CASED_TAG));
    }
    request.insert(
        "clusterName".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    let reported = call_json_rpc(
        ctx,
        ECS_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ECS_TARGET_PREFIX}.CreateCluster"),
        &Value::Object(request),
    )
    .await?;
    let mut attributes = BTreeMap::new();
    if let Some(arn) = reported
        .get("cluster")
        .and_then(|cluster| cluster.get("clusterArn"))
        .and_then(Value::as_str)
    {
        attributes.insert("Arn".to_string(), arn.to_string());
    }
    // The cluster was created under the name CloudFormation decided.
    Ok(Created::attributes(attributes))
}

/// Delete the cluster a stack created. A cluster Amazon ECS no longer holds
/// (`ClusterNotFoundException`) is already in the state the delete asks for; one that
/// still holds active services or tasks is NOT deleted, and that refusal is what the stack
/// reports.
async fn delete_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "cluster": physical_resource_id });
    match call_json_rpc(
        ctx,
        ECS_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ECS_TARGET_PREFIX}.DeleteCluster"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == CLUSTER_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// One evaluated scalar property of a resource as the text an API member takes.
///
/// Read through [`scalar_text`], this module's single spelling rule, so a property a
/// template writes as a number or a boolean reaches the owning service the same way here
/// as in every other arm, and an empty string is treated as a property the template did
/// not really declare.
fn property(resource: &StoredResource, name: &str) -> Option<String> {
    scalar_text(resource.properties.get(name)?).filter(|text| !text.is_empty())
}

/// The `Type` property both OpenSearch Serverless resource types require (a policy's
/// `encryption`/`network`, a collection's `SEARCH`/`TIMESERIES`/`VECTORSEARCH`).
///
/// A resource that declares none is refused HERE rather than with a request the owning
/// service would have to reject: the property is required by the resource type.
fn declared_type(resource: &StoredResource) -> Result<String, Refusal> {
    property(resource, "Type").ok_or_else(|| Refusal {
        code: MISSING_PROPERTY_CODE.to_string(),
        message: format!("{} declares no Type", resource.resource_type),
    })
}

/// The `Policy` property as the policy document text the owning service takes.
///
/// A template may spell it as the JSON text or as the document itself (an inline mapping
/// in a YAML template), which is exactly [`document_text`]'s rule, shared with the IAM
/// arm. A declaration carrying none describes no policy that could be created, so it is
/// refused here.
fn policy_document(resource: &StoredResource) -> Result<String, Refusal> {
    resource
        .properties
        .get("Policy")
        .and_then(document_text)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!("{} declares no Policy", resource.resource_type),
        })
}

/// The cluster's configuration as Amazon ECS reports it, for drift detection.
///
/// A cluster Amazon ECS no longer holds (`ClusterNotFoundException`, an answer listing no
/// cluster, or one it reports `INACTIVE` — the status a deleted cluster keeps, which
/// `DeleteCluster` documents) was deleted.
async fn describe_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    /// The status Amazon ECS reports for a cluster whose deletion has gone through.
    const INACTIVE: &str = "INACTIVE";

    let request = serde_json::json!({ "clusters": [physical_resource_id] });
    match call_json_rpc(
        ctx,
        ECS_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ECS_TARGET_PREFIX}.DescribeClusters"),
        &request,
    )
    .await
    {
        Ok(answer) => Ok(first_listed(&answer, "clusters").filter(|cluster| {
            cluster.get("status").and_then(Value::as_str) != Some(INACTIVE)
        })),
        Err(refusal) if refusal.code == CLUSTER_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The collection's configuration as OpenSearch Serverless reports it, for drift
/// detection, in the TYPE's own property vocabulary — the same correspondence the create
/// maps the other way, so an actual configuration is comparable with the template's.
///
/// `BatchGetCollection` reports a collection it no longer holds as a
/// `collectionErrorDetails` entry rather than with a refusal, so "no `collectionDetails`
/// entry" is what says the collection was deleted. A property the read does not answer is
/// absent from the document rather than guessed.
async fn describe_collection(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let answer = call_json_rpc(
        ctx,
        AOSS_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{AOSS_TARGET_PREFIX}.BatchGetCollection"),
        &serde_json::json!({ "ids": [physical_resource_id] }),
    )
    .await?;
    let Some(detail) = first_listed(&answer, "collectionDetails") else {
        return Ok(None);
    };
    Ok(Some(reported_as_properties(
        &detail,
        &[
            ("Name", "name"),
            ("Type", "type"),
            ("Description", "description"),
            ("StandbyReplicas", "standbyReplicas"),
        ],
    )))
}

/// The policy's configuration as OpenSearch Serverless reports it, for drift detection, in
/// the type's own property vocabulary. A policy the service no longer holds is its own
/// `ResourceNotFoundException`.
///
/// `Policy` is deliberately NOT reported: the service answers the policy as the parsed
/// document, while a template may spell it as that document or as its JSON text, so
/// comparing the two spellings would report drift where there is none.
async fn describe_security_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({
        "type": declared_type(resource)?,
        "name": resource.physical_resource_id,
    });
    match call_json_rpc(
        ctx,
        AOSS_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{AOSS_TARGET_PREFIX}.GetSecurityPolicy"),
        &request,
    )
    .await
    {
        Ok(answer) => {
            let Some(detail) = answer.get("securityPolicyDetail") else {
                return Ok(None);
            };
            Ok(Some(reported_as_properties(
                detail,
                &[
                    ("Name", "name"),
                    ("Type", "type"),
                    ("Description", "description"),
                ],
            )))
        }
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// One answered detail document under the resource type's own property names, as
/// `(property, member)` pairs. A member the answer does not carry contributes nothing, so
/// the result reports only what the owning service really said.
fn reported_as_properties(detail: &Value, members: &[(&str, &str)]) -> Value {
    let mut reported = Map::new();
    for (property, member) in members {
        if let Some(value) = detail.get(*member) {
            reported.insert((*property).to_string(), value.clone());
        }
    }
    Value::Object(reported)
}

/// One property value with every member name inside it in LOWER CAMEL case, which is how
/// a service whose model spells its members that way names the very same members a
/// CloudFormation property names in upper camel case (`ClusterSettings: [{Name, Value}]`
/// is `settings: [{name, value}]`).
///
/// The walk is bounded by [`MAX_PROPERTY_NESTING`] for the reason that constant records:
/// the document is caller-supplied, so a deeper one is refused rather than walked. It is
/// applied only where every member inside the property is a modelled name: a property
/// carrying a MAP of caller data (a container definition's `DockerLabels`) would have its
/// keys re-cased too, and no property of the type above carries one.
///
/// `depth` is how many levels are already above this one, as in [`camel_cased`].
fn lower_camel_members(value: &Value, depth: usize) -> Result<Value, Refusal> {
    // No member of these types is spelled anything but the property with its initial
    // letter lower-cased, so the walk carries no exceptions.
    renamed_members(value, depth, &[])
}

/// The same walk where some of the members are NOT the property with its initial letter
/// lower-cased: `renamed` is the table of those, as (property, member) pairs, and every
/// other key is re-cased exactly as [`lower_camel_members`] does it.
///
/// That is the shape a type whose API spells one member differently needs
/// (`AWS::MSK::Cluster`'s `StorageInfo.EBSStorageInfo` against `storageInfo.\
/// ebsStorageInfo`), and the same exception-carrying arrangement [`camel_cased`] has for
/// the one map whose keys are data. A table small enough to read is what keeps this a
/// re-casing walk rather than a per-member mapping ([`mapped_members`]).
fn renamed_members(
    value: &Value,
    depth: usize,
    renamed: &[(&str, &str)],
) -> Result<Value, Refusal> {
    if depth >= MAX_PROPERTY_NESTING {
        return Err(too_deeply_nested());
    }
    match value {
        Value::Object(members) => {
            let mut converted = Map::new();
            for (name, member) in members {
                converted.insert(
                    renamed_member(name, renamed),
                    renamed_members(member, depth + 1, renamed)?,
                );
            }
            Ok(Value::Object(converted))
        }
        Value::Array(items) => {
            let mut converted = Vec::with_capacity(items.len());
            for item in items {
                converted.push(renamed_members(item, depth + 1, renamed)?);
            }
            Ok(Value::Array(converted))
        }
        scalar => Ok(scalar.clone()),
    }
}

/// The API member one property name is: the one `renamed` names it, and otherwise the
/// property with its initial letter lower-cased.
fn renamed_member(property: &str, renamed: &[(&str, &str)]) -> String {
    renamed
        .iter()
        .find(|(declared, _)| *declared == property)
        .map_or_else(
            || lower_initial(property),
            |(_, member)| (*member).to_string(),
        )
}

// ---------------------------------------------------------------------------
// Amazon SQS: AWS::SQS::Queue
// ---------------------------------------------------------------------------

/// Amazon SQS's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: sqs`, service shape `AmazonSQS`), which speaks `awsJson1_0`.
const SQS_SIGNING_NAME: &str = "sqs";
const SQS_TARGET_PREFIX: &str = "AmazonSQS";

/// The error code SQS answers for a queue url no queue answers to (`QueueDoesNotExist`,
/// the modelled error of `DeleteQueue`).
const QUEUE_DOES_NOT_EXIST: &str = "QueueDoesNotExist";

/// Create the queue an `AWS::SQS::Queue` declares.
///
/// A queue's configuration is its ATTRIBUTES, so the call is `CreateQueue` with the
/// queue's name — the one thing CloudFormation decides, which is this resource's name and
/// the last segment of the queue url its `Ref` resolves to — and the attribute map
/// [`queue_attributes`] reads off the declared properties.
///
/// The url the create answers with is reported back as the resource's `QueueUrl`
/// attribute. It is the SAME url the type's published return values computed
/// (`https://sqs.<region>.amazonaws.com/<account>/<name>`), because both are built from
/// the one account id this emulator mints and the queue's own name — and SQS resolves a
/// `QueueUrl` by exactly that pair (`queue_name_from_url` in its own state module), so
/// the handle the stack publishes addresses the queue this call created however a client
/// reaches SQS.
///
/// The queue carries the tags the declaration asks for plus the ones CloudFormation gives
/// the resource ([`tags_of`]), as `CreateQueue`'s own `tags` map.
async fn create_queue(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = Map::new();
    request.insert("QueueName".to_string(), Value::String(queue_name(resource)));
    let attributes = queue_attributes(&resource.properties);
    if !attributes.is_empty() {
        request.insert("Attributes".to_string(), Value::Object(attributes));
    }
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let reported = call_json_rpc(
        ctx,
        SQS_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{SQS_TARGET_PREFIX}.CreateQueue"),
        &Value::Object(request),
    )
    .await?;
    let mut attributes = BTreeMap::new();
    if let Some(url) = reported.get("QueueUrl").and_then(Value::as_str) {
        attributes.insert("QueueUrl".to_string(), url.to_string());
    }
    // The queue was created under the name CloudFormation decided, and its url is built
    // from that name.
    Ok(Created::attributes(attributes))
}

/// Apply a changed `AWS::SQS::Queue` declaration to the queue that already exists.
///
/// A queue's configuration is its ATTRIBUTES and its TAGS, and Amazon SQS has an API for
/// changing each of them on an existing queue, so that is what this calls: one
/// `SetQueueAttributes` carrying the attributes this deployment states differently (added
/// or given another value), one `TagQueue` carrying the tags it adds or re-values, and one
/// `UntagQueue` naming the tag keys it no longer declares. A property neither revision
/// changed is not sent at all, and nothing is called where nothing changed.
///
/// The queue keeps its name, so it keeps the url its `Ref` resolves to and everything it
/// holds: the change is applied to the queue the stack created rather than replacing it.
/// The create reported the `QueueUrl` attribute and the queue still answers to it, so this
/// reports no attribute of its own.
///
/// Two changes Amazon SQS will not make to an existing queue are REFUSED rather than
/// quietly dropped, each with the refusal a client calling that API would get, so the stack
/// never reports a queue configured differently from its declaration:
///
/// * an attribute `SetQueueAttributes` does not take — `FifoQueue` is "an attribute [you
///   can] provide only during queue creation", so SQS answers `InvalidAttributeName`;
/// * a property this deployment no longer declares, which AWS resets to that attribute's
///   documented default. `SetQueueAttributes` has no way to ask for a default and the
///   defaults are Amazon SQS's own (restating them here would be a second copy of its
///   documentation), so the removal is answered with `NotImplemented` naming the property.
async fn update_queue(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    let url = resource.physical_resource_id.clone();
    let before = queue_attributes(&resource.properties);
    let after = queue_attributes(declared);

    // A property the new declaration drops: nothing is called at all, so the queue is left
    // exactly as it is and the deployment reports why.
    if let Some(dropped) = before.keys().find(|name| !after.contains_key(*name)) {
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not reset the {dropped} property of an existing \
                 {SQS_QUEUE} that an update no longer declares to Amazon SQS's documented \
                 default; the resource was left as it is."
            ),
        });
    }

    let changed: Map<String, Value> = after
        .into_iter()
        .filter(|(name, value)| before.get(name) != Some(value))
        .collect();
    if !changed.is_empty() {
        call_json_rpc(
            ctx,
            SQS_SIGNING_NAME,
            region,
            AWS_JSON_1_0,
            &format!("{SQS_TARGET_PREFIX}.SetQueueAttributes"),
            &serde_json::json!({ "QueueUrl": url, "Attributes": Value::Object(changed) }),
        )
        .await?;
    }

    let before_tags = queue_tags(&resource.properties);
    let after_tags = queue_tags(declared);
    let added: Map<String, Value> = after_tags
        .iter()
        .filter(|(key, value)| before_tags.get(*key) != Some(value))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if !added.is_empty() {
        call_json_rpc(
            ctx,
            SQS_SIGNING_NAME,
            region,
            AWS_JSON_1_0,
            &format!("{SQS_TARGET_PREFIX}.TagQueue"),
            &serde_json::json!({ "QueueUrl": url, "Tags": Value::Object(added) }),
        )
        .await?;
    }
    let removed: Vec<Value> = before_tags
        .keys()
        .filter(|key| !after_tags.contains_key(*key))
        .map(|key| Value::String(key.clone()))
        .collect();
    if !removed.is_empty() {
        call_json_rpc(
            ctx,
            SQS_SIGNING_NAME,
            region,
            AWS_JSON_1_0,
            &format!("{SQS_TARGET_PREFIX}.UntagQueue"),
            &serde_json::json!({ "QueueUrl": url, "TagKeys": Value::Array(removed) }),
        )
        .await?;
    }

    Ok(Created::default())
}

/// Delete the queue a stack created. A queue SQS no longer has (`QueueDoesNotExist`) is
/// already in the state the delete asks for.
///
/// The queue is addressed by its url, which is this resource's physical id — "Ref returns
/// the queue URL" for this type.
async fn delete_queue(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "QueueUrl": physical_resource_id });
    match call_json_rpc(
        ctx,
        SQS_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{SQS_TARGET_PREFIX}.DeleteQueue"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == QUEUE_DOES_NOT_EXIST => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The queue's configuration as Amazon SQS reports it, for drift detection.
///
/// A queue SQS no longer has (`QueueDoesNotExist`) was deleted. A queue's configuration IS
/// its attribute map, named identically to this type's properties ([`queue_attributes`]),
/// so the attributes are the actual configuration compared against the declared one and a
/// queue whose visibility timeout was changed outside the stack is MODIFIED.
async fn describe_queue(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({
        "QueueUrl": physical_resource_id,
        "AttributeNames": ["All"],
    });
    match call_json_rpc(
        ctx,
        SQS_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{SQS_TARGET_PREFIX}.GetQueueAttributes"),
        &request,
    )
    .await
    {
        Ok(answer) => Ok(Some(reported_member(&answer, "Attributes"))),
        Err(refusal) if refusal.code == QUEUE_DOES_NOT_EXIST => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The NAME of the queue a resource record describes: the `QueueName` its type's
/// published return values carry, which is the name the template asked for or the
/// `<stack>-<logical id>-<suffix>` name CloudFormation generated — never the physical id,
/// which for this type is the queue's url.
///
/// A record with no such attribute (one whose attribute budget was spent) falls back to
/// the last segment of that url, which is the queue's name.
fn queue_name(resource: &StoredResource) -> String {
    if let Some(name) = resource.attributes.get("QueueName") {
        return name.clone();
    }
    resource
        .physical_resource_id
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `Attributes` map an `AWS::SQS::Queue`'s declared properties are, as `CreateQueue`
/// takes them.
///
/// This type's properties ARE the queue's own attributes, named identically on both sides
/// (`VisibilityTimeout`, `MessageRetentionPeriod`, `DelaySeconds`, `FifoQueue`,
/// `RedrivePolicy`, …): CloudFormation's property names for the type are the attribute
/// names `CreateQueue`/`SetQueueAttributes` document. So every property is passed through
/// under its own name except the two that are not attributes — `QueueName`, which is the
/// create's own member and the one value CloudFormation decides, and `Tags`, which the
/// create takes as its own `tags` member ([`queue_tags`]).
///
/// An attribute value is a STRING on the wire, so a number or a boolean the template
/// wrote as such is rendered as the text it is, and the policy attributes — documented
/// "as a JSON object" — as the compact JSON of the object declared. A property SQS does
/// not know is not filtered out here: the owning service's own validation is what judges
/// an attribute, and its refusal leaves the stack `CREATE_FAILED` rather than a queue
/// configured differently from the template.
fn queue_attributes(properties: &Value) -> Map<String, Value> {
    let Some(properties) = properties.as_object() else {
        return Map::new();
    };
    properties
        .iter()
        .filter(|(property, _)| property.as_str() != "QueueName" && property.as_str() != "Tags")
        .map(|(property, value)| (property.clone(), Value::String(attribute_text(value))))
        .collect()
}

/// One attribute value as the wire carries it: a string stays itself, every other JSON
/// value is its own compact JSON (`30`, `true`, `{"maxReceiveCount":3}`).
fn attribute_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The `tags` map an `AWS::SQS::Queue`'s `Tags` property is.
///
/// CloudFormation's tag property is a LIST of `{Key, Value}` pairs and `CreateQueue`
/// takes a map, which is the conversion [`declared_tags`] and [`tags_as_map`] do.
fn queue_tags(properties: &Value) -> Map<String, Value> {
    tags_as_map(&declared_tags(properties))
}

// ---------------------------------------------------------------------------
// The tags a provisioned resource carries
// ---------------------------------------------------------------------------

/// The tags ONE provisioned resource carries: the ones its own declaration asks for, then
/// the ones CloudFormation propagates to it under every key that declaration does not use.
///
/// The rule: a resource CloudFormation creates carries the stack-level tags plus the three
/// `aws:cloudformation:*` tags ([`super::provisioning::StackProvenance::tags_for`]), and a
/// key the resource's own `Tags` property states keeps the value that property gives it —
/// the declaration is the more specific statement about that one resource. The three
/// CloudFormation records itself are the exception, exactly as they are in `tags_for`: they
/// are CloudFormation's to set, so a declaration spelling one of them does not displace it.
/// The result is ORDERED (declared first, in declaration order), so the same declaration
/// builds the same request however the maps behind it happen to iterate.
fn tags_of(properties: &Value, propagated: &[(String, String)]) -> Vec<(String, String)> {
    let mut tags = declared_tags(properties);
    for (key, value) in propagated {
        if key.is_empty() {
            continue;
        }
        match tags.iter().position(|(held, _)| held == key) {
            Some(index) if is_cloudformation_tag(key) => tags[index].1 = value.clone(),
            Some(_) => {}
            None => tags.push((key.clone(), value.clone())),
        }
    }
    tags
}

/// Whether `key` is one of the three tags CloudFormation records on every resource it
/// provisions, which no declaration of its own may displace.
fn is_cloudformation_tag(key: &str) -> bool {
    [
        super::provisioning::STACK_NAME_TAG,
        super::provisioning::STACK_ID_TAG,
        super::provisioning::LOGICAL_ID_TAG,
    ]
    .contains(&key)
}

/// The tags a resource's own declaration asks for, in the order it writes them.
///
/// CloudFormation spells a type's `Tags` property either as the `[{Key, Value}]` list most
/// types publish or as the MAP of tag key to value a few do
/// (`AWS::FIS::ExperimentTemplate`), and both are read here, so a declaration reaches the
/// owning service whichever spelling the type uses. A tag with no key is not a tag and is
/// dropped rather than sent as an empty one; a value that is not a string is sent as the
/// text it is, since a tag value is a string on every wire that takes one.
///
/// A key the LIST form states twice is one tag, not two — a tag set is a map from key to
/// value, so a repeated key keeps its first position with the later value, which is the
/// same rule [`super::provisioning::StackProvenance::tags_for`] applies to a stack's own
/// tags. (The map form cannot state a key twice.)
fn declared_tags(properties: &Value) -> Vec<(String, String)> {
    let declared: Vec<(String, String)> = match properties.get("Tags") {
        Some(Value::Array(tags)) => tags
            .iter()
            .filter_map(|tag| {
                let key = tag.get("Key").and_then(Value::as_str)?;
                if key.is_empty() {
                    return None;
                }
                Some((
                    key.to_string(),
                    tag.get("Value").map(attribute_text).unwrap_or_default(),
                ))
            })
            .collect(),
        Some(Value::Object(tags)) => tags
            .iter()
            .filter(|(key, _)| !key.is_empty())
            .map(|(key, value)| (key.clone(), attribute_text(value)))
            .collect(),
        _ => Vec::new(),
    };
    let mut tags: Vec<(String, String)> = Vec::with_capacity(declared.len());
    for (key, value) in declared {
        match tags.iter().position(|(held, _)| *held == key) {
            Some(index) => tags[index].1 = value,
            None => tags.push((key, value)),
        }
    }
    tags
}

/// AWS's reserved tag-key namespace: "the `aws:` prefix is reserved for AWS use", so it is
/// the namespace a caller's own request may not write into. Matched case-insensitively, as
/// the services that refuse it match it.
const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` is in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace.
fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// The member names a service's own `Tag` shape spells a tag's key and value with: upper
/// camel on Amazon Data Firehose and Amazon ECR Public (`Key`/`Value`), lower camel on
/// Amazon ECS and CodePipeline (`key`/`value`), which is the one difference between the
/// list each of them takes.
const UPPER_CASED_TAG: (&str, &str) = ("Key", "Value");
const LOWER_CASED_TAG: (&str, &str) = ("key", "value");

/// A tag set as the list of `Tag` structures a creating API takes, with each structure's
/// members spelled the way that service's model spells them ([`UPPER_CASED_TAG`],
/// [`LOWER_CASED_TAG`]).
fn tags_as_list(tags: &[(String, String)], members: (&str, &str)) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| {
                let mut tag = Map::new();
                tag.insert(members.0.to_string(), Value::String(key.clone()));
                tag.insert(members.1.to_string(), Value::String(value.clone()));
                Value::Object(tag)
            })
            .collect(),
    )
}

/// A tag set as the MAP of tag key to value a creating API that takes one takes (Amazon
/// SQS's `tags`, AWS FIS's `tags`).
fn tags_as_map(tags: &[(String, String)]) -> Map<String, Value> {
    tags.iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect()
}

// ---------------------------------------------------------------------------
// Amazon SNS: AWS::SNS::Topic
// ---------------------------------------------------------------------------

/// Amazon SNS's signing name and API version, from its model (`sigv4Name: sns`, service
/// shape `AmazonSimpleNotificationService`, `version: 2010-03-31`), which speaks
/// `awsQuery` — so its calls ride on [`call_query`] rather than on [`call_json_rpc`].
const SNS_SIGNING_NAME: &str = "sns";
const SNS_API_VERSION: &str = "2010-03-31";

/// The error code SNS answers for a topic ARN no topic answers to: `NotFoundException`'s
/// own `awsQueryError` code, which is what an awsQuery client branches on.
const NOT_FOUND: &str = "NotFound";

/// The properties of an `AWS::SNS::Topic` that are NOT topic attributes, each a member of
/// `CreateTopic` in its own right or a call of its own:
///
/// * `TopicName` is the one value CloudFormation decides, and the create's `Name` member;
/// * `Tags` is the create's own tag list;
/// * `DataProtectionPolicy` is the create's own member ("The body of the policy document
///   you want to use for this topic");
/// * `Subscription` is a list of subscriptions, which are `Subscribe` calls on the topic
///   once it exists.
const NON_ATTRIBUTE_PROPERTIES: &[&str] =
    &["TopicName", "Tags", "DataProtectionPolicy", "Subscription"];

/// The property whose entries are the per-protocol delivery-status attributes.
const DELIVERY_STATUS_LOGGING: &str = "DeliveryStatusLogging";

/// How a `DeliveryStatusLogging` entry's `Protocol` is spelled in the delivery-status
/// attribute names SNS documents: a `sqs` entry's role ARN is the topic's
/// `SQSSuccessFeedbackRoleArn`, a `http/s` entry's its `HTTPSuccessFeedbackRoleArn`.
///
/// Source: the `Protocol` value set AWS's resource reference publishes for
/// `AWS::SNS::Topic`'s `LoggingConfig`, against the attribute names
/// `SetTopicAttributes` documents. A protocol outside this set is passed through as the
/// template spells it, so SNS's own validation is what refuses it rather than a name
/// invented here.
const LOGGING_PROTOCOL_PREFIXES: &[(&str, &str)] = &[
    ("http/s", "HTTP"),
    ("sqs", "SQS"),
    ("lambda", "Lambda"),
    ("firehose", "Firehose"),
    ("application", "Application"),
];

/// The members of one `LoggingConfig`, named identically on both sides once the protocol's
/// prefix is in front of them. `Protocol` itself is the prefix, not a member.
const LOGGING_CONFIG_MEMBERS: &[&str] = &[
    "SuccessFeedbackRoleArn",
    "SuccessFeedbackSampleRate",
    "FailureFeedbackRoleArn",
];

/// Create the topic an `AWS::SNS::Topic` declares, and the subscriptions it declares on it.
///
/// A topic's configuration is its ATTRIBUTES, so the call is `CreateTopic` with the topic's
/// name — the one thing CloudFormation decides, which is the last segment of the ARN its
/// `Ref` resolves to — and the attribute map [`topic_attributes`] reads off the declared
/// properties.
///
/// The ARN the create answers with is reported back as the resource's `TopicArn`
/// attribute. It is the SAME ARN the type's published return values computed
/// (`arn:aws:sns:<region>:<account>:<name>`), because both are built from the one account
/// id this emulator mints and the topic's own name — and SNS keys a topic by exactly that
/// ARN, so the handle the stack publishes addresses the topic this call created however a
/// client reaches SNS.
///
/// The topic carries the tags the declaration asks for plus the ones CloudFormation gives
/// the resource ([`tags_of`]), as `CreateTopic`'s own `Tags` list — so `ListTagsForResource`
/// on the ARN the stack publishes answers with the stack's tags and the
/// `aws:cloudformation:*` ones.
async fn create_topic(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut parameters = vec![("Name".to_string(), topic_name(resource))];
    for (index, (name, value)) in topic_attributes(&resource.properties).iter().enumerate() {
        let entry = index + 1;
        parameters.push((format!("Attributes.entry.{entry}.key"), name.clone()));
        parameters.push((format!("Attributes.entry.{entry}.value"), value.clone()));
    }
    // Amazon SNS's own `Tag` shape carries no `@xmlName` on its list member, so its tag
    // list is indexed under the Smithy default element ([`push_tag_list`]).
    push_tag_list(
        &mut parameters,
        "Tags",
        "member",
        &tags_of(&resource.properties, propagated),
    );
    if let Some(policy) = resource.properties.get("DataProtectionPolicy") {
        parameters.push(("DataProtectionPolicy".to_string(), attribute_text(policy)));
    }
    let answered = call_query(
        ctx,
        SNS_SIGNING_NAME,
        region,
        SNS_API_VERSION,
        "CreateTopic",
        &parameters,
    )
    .await?;
    let arn = xml_element(&answered, "TopicArn")
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| Refusal {
            code: "InternalFailure".to_string(),
            message: "Amazon SNS answered CreateTopic without a TopicArn".to_string(),
        })?;

    // "The SNS subscriptions (endpoints) for this topic" — declared inline on the topic,
    // so they are subscriptions of the topic this call just created.
    for subscription in declared_subscriptions(&resource.properties) {
        let mut parameters = vec![("TopicArn".to_string(), arn.clone())];
        parameters.extend(subscription);
        let subscribed = call_query(
            ctx,
            SNS_SIGNING_NAME,
            region,
            SNS_API_VERSION,
            "Subscribe",
            &parameters,
        )
        .await;
        if let Err(refusal) = subscribed {
            // The resource the template declares is the topic AND the subscriptions on
            // it, so a subscription SNS refuses fails the resource. The stack records no
            // physical id for a resource whose create failed, so nothing would ever
            // delete the topic this call already made: it is deleted here, before the
            // refusal propagates, and what the stack reports is SNS's own refusal.
            delete_topic(ctx, region, &arn)
                .await
                .map_err(|failure| Refusal {
                    code: refusal.code.clone(),
                    message: format!(
                        "{refusal}; the topic it created was left behind: {failure}"
                    ),
                })?;
            return Err(refusal);
        }
    }

    let mut attributes = BTreeMap::new();
    attributes.insert("TopicArn".to_string(), arn);
    Ok(Created::attributes(attributes))
}

/// Delete the topic a stack created. A topic SNS no longer has (`NotFound`) is already in
/// the state the delete asks for.
///
/// The topic is addressed by its ARN, which is this resource's physical id — "Ref returns
/// the topic ARN" for this type.
async fn delete_topic(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let parameters = vec![("TopicArn".to_string(), physical_resource_id.to_string())];
    match call_query(
        ctx,
        SNS_SIGNING_NAME,
        region,
        SNS_API_VERSION,
        "DeleteTopic",
        &parameters,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The topic's configuration as Amazon SNS reports it, for drift detection.
///
/// A topic SNS no longer has (`NotFound`) was deleted. A topic's configuration IS its
/// attribute map, named identically to this type's properties ([`topic_attributes`]), so
/// the attributes are the actual configuration compared against the declared one and a
/// topic whose display name was changed outside the stack is MODIFIED. The map the query
/// protocol serializes as `<entry><key/><value/></entry>` is read back as the JSON object
/// the comparison takes.
async fn describe_topic(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![("TopicArn".to_string(), physical_resource_id.to_string())];
    match call_query(
        ctx,
        SNS_SIGNING_NAME,
        region,
        SNS_API_VERSION,
        "GetTopicAttributes",
        &parameters,
    )
    .await
    {
        Ok(answered) => Ok(Some(Value::Object(xml_entry_map(&answered, "Attributes")))),
        Err(refusal) if refusal.code == NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The NAME of the topic a resource record describes: the `TopicName` its type's published
/// return values carry, which is the name the template asked for or the
/// `<stack>-<logical id>-<suffix>` name CloudFormation generated — never the physical id,
/// which for this type is the topic's ARN.
///
/// A record with no such attribute (one whose attribute budget was spent) falls back to the
/// last segment of that ARN, which is the topic's name.
fn topic_name(resource: &StoredResource) -> String {
    if let Some(name) = resource.attributes.get("TopicName") {
        return name.clone();
    }
    resource
        .physical_resource_id
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `Attributes` map an `AWS::SNS::Topic`'s declared properties are, as `CreateTopic`
/// takes them.
///
/// This type's properties ARE the topic's own attributes, named identically on both sides
/// (`DisplayName`, `FifoTopic`, `KmsMasterKeyId`, `SignatureVersion`, `TracingConfig`,
/// `ArchivePolicy`, `ContentBasedDeduplication`, `FifoThroughputScope`): CloudFormation's
/// property names for the type are the attribute names
/// `CreateTopic`/`SetTopicAttributes` document. So every property is passed through under
/// its own name except the ones that are not attributes ([`NON_ATTRIBUTE_PROPERTIES`]) and
/// `DeliveryStatusLogging`, whose entries are the per-protocol attributes
/// [`delivery_status_attributes`] names.
///
/// An attribute value is a STRING on the wire, so a number or a boolean the template wrote
/// as such is rendered as the text it is, and the policy attributes — documented as JSON
/// objects — as the compact JSON of the object declared. A property SNS does not know is
/// not filtered out here: the owning service's own validation is what judges an attribute,
/// and its refusal leaves the stack `CREATE_FAILED` rather than a topic configured
/// differently from the template.
fn topic_attributes(properties: &Value) -> BTreeMap<String, String> {
    let mut attributes = BTreeMap::new();
    let Some(properties) = properties.as_object() else {
        return attributes;
    };
    for (property, value) in properties {
        if NON_ATTRIBUTE_PROPERTIES.contains(&property.as_str()) {
            continue;
        }
        if property == DELIVERY_STATUS_LOGGING {
            attributes.extend(delivery_status_attributes(value));
            continue;
        }
        attributes.insert(property.clone(), attribute_text(value));
    }
    attributes
}

/// The topic attributes a `DeliveryStatusLogging` list declares: one entry per member each
/// of its `LoggingConfig`s carries, under that config's protocol prefix
/// ([`LOGGING_PROTOCOL_PREFIXES`]).
///
/// A member the config does not declare is absent. `Protocol` is required on a
/// `LoggingConfig`, so an entry carrying none is a template error: its members go through
/// under their BARE names, which are not topic attribute names, so SNS refuses them — the
/// same way an unknown protocol spelling goes through as the template wrote it. Neither is
/// dropped, which would configure the topic differently from what the template declared
/// while the stack still completed.
fn delivery_status_attributes(declared: &Value) -> BTreeMap<String, String> {
    let mut attributes = BTreeMap::new();
    for config in declared.as_array().map(Vec::as_slice).unwrap_or_default() {
        let prefix = config
            .get("Protocol")
            .and_then(Value::as_str)
            .map_or("", |protocol| {
                LOGGING_PROTOCOL_PREFIXES
                    .iter()
                    .find(|(declared, _)| declared.eq_ignore_ascii_case(protocol))
                    .map_or(protocol, |(_, prefix)| prefix)
            });
        for member in LOGGING_CONFIG_MEMBERS {
            if let Some(value) = config.get(*member) {
                attributes.insert(format!("{prefix}{member}"), attribute_text(value));
            }
        }
    }
    attributes
}

/// The `Subscribe` parameters each entry of a `Subscription` property declares: its
/// `Protocol` and its `Endpoint`, named identically on both sides. A member the entry does
/// not declare is absent from the call, so SNS's own validation is what refuses an
/// incomplete subscription.
fn declared_subscriptions(properties: &Value) -> Vec<Vec<(String, String)>> {
    properties
        .get("Subscription")
        .and_then(Value::as_array)
        .map(|declared| {
            declared
                .iter()
                .map(|subscription| {
                    ["Protocol", "Endpoint"]
                        .iter()
                        .filter_map(|member| {
                            let value = subscription.get(*member)?;
                            Some(((*member).to_string(), attribute_text(value)))
                        })
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Amazon SageMaker: AWS::SageMaker::ModelPackageGroup
// ---------------------------------------------------------------------------

/// SageMaker's signing name and `X-Amz-Target` prefix, from its model (`sigv4Name:
/// sagemaker`, service shape `SageMaker`), which speaks `awsJson1_1`.
const SAGEMAKER_SIGNING_NAME: &str = "sagemaker";
const SAGEMAKER_TARGET_PREFIX: &str = "SageMaker";

/// The property `AWS::SageMaker::ModelPackageGroup` carries the model group's resource
/// policy under.
///
/// It is the ONE property of that type which is not a `CreateModelPackageGroup` member:
/// SageMaker attaches a model group's resource policy through its own
/// `PutModelPackageGroupPolicy`, so the property is taken out of the create request and
/// applied with that call once the group exists — the same two calls a client makes.
const MODEL_PACKAGE_GROUP_POLICY: &str = "ModelPackageGroupPolicy";

/// Create the model group an `AWS::SageMaker::ModelPackageGroup` declares, with the
/// resource policy it declares for it.
///
/// `ModelPackageGroupName`, `ModelPackageGroupDescription` and `Tags` are named identically
/// on the type and on `CreateModelPackageGroup`, so the resolved properties ARE the
/// request, with the one member CloudFormation decides (the group's name, which is this
/// resource's physical id) set on the way out, and the `Tags` list carrying the tags
/// CloudFormation propagates beside the declared ones ([`tags_of`]) — `CreateModelPackageGroup$Tags`
/// is the same `Tag{Key, Value}` list the type's property is. [`MODEL_PACKAGE_GROUP_POLICY`]
/// is the exception, and is attached afterwards through the operation that owns it; a group
/// whose policy SageMaker refuses is not left behind half-configured — it is deleted again,
/// and the refusal is what the deployment reports, with a cleanup that itself failed named
/// in it rather than swallowed.
async fn create_model_package_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    let policy = request.remove(MODEL_PACKAGE_GROUP_POLICY);
    let name = resource.physical_resource_id.clone();
    request.insert(
        "ModelPackageGroupName".to_string(),
        Value::String(name.clone()),
    );
    let tags = tags_of(&resource.properties, propagated);
    if tags.is_empty() {
        request.remove("Tags");
    } else {
        request.insert("Tags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
    let reported = call_json_rpc(
        ctx,
        SAGEMAKER_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{SAGEMAKER_TARGET_PREFIX}.CreateModelPackageGroup"),
        &Value::Object(request),
    )
    .await?;
    let mut attributes = BTreeMap::new();
    if let Some(arn) = reported.get("ModelPackageGroupArn").and_then(Value::as_str) {
        // The group's real ARN, which is the ARN its `Fn::GetAtt` reports.
        attributes.insert("Arn".to_string(), arn.to_string());
        attributes.insert("ModelPackageGroupArn".to_string(), arn.to_string());
    }

    let Some(policy) = policy.filter(|policy| !policy.is_null()) else {
        return Ok(Created::attributes(attributes));
    };
    // The type's property is a `Json` document, which a template may spell as the document
    // itself or as the text of one; `PutModelPackageGroupPolicy$ResourcePolicy` is modelled
    // a string either way, so the one JSON-to-text rendering this module has is what
    // renders it: text stays itself, a document becomes its own compact JSON.
    let document = attribute_text(&policy);
    let attached = call_json_rpc(
        ctx,
        SAGEMAKER_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{SAGEMAKER_TARGET_PREFIX}.PutModelPackageGroupPolicy"),
        &serde_json::json!({
            "ModelPackageGroupName": name,
            "ResourcePolicy": document,
        }),
    )
    .await;
    let Err(refusal) = attached else {
        return Ok(Created::attributes(attributes));
    };
    // The group exists but carries none of the policy the template declared, and the
    // deployment will not record it as created: delete it rather than leave a group behind
    // that no stack would ever clean up. A cleanup SageMaker refuses leaves one there, so
    // that refusal is reported alongside the one that caused it.
    match delete_model_package_group(ctx, region, &name).await {
        Ok(()) => Err(refusal),
        Err(cleanup) => Err(Refusal {
            code: refusal.code,
            message: format!(
                "{}; the model package group could not be deleted again: {cleanup}",
                refusal.message
            ),
        }),
    }
}

/// The error code SageMaker answers for a model group it does not have: it validates the
/// group's existence before the call rather than modelling a not-found error, so this is
/// what `DescribeModelPackageGroup` answers for a group that is absent.
const SAGEMAKER_VALIDATION_EXCEPTION: &str = "ValidationException";

/// Delete the model group a stack created. SageMaker releases a group's resource policy
/// with the group, so the one call removes both.
///
/// A group SageMaker no longer has is already in the state the delete asks for. This
/// operation models no not-found error — the service validates the group's existence and
/// answers `ValidationException`, which is also what it answers for a group it refuses to
/// delete (one that still contains model package versions) — so "already gone" is decided
/// by READING the group back rather than by matching the refusal's text. Only the read's
/// OWN `ValidationException` means absence: any other answer (a transport fault, an
/// internal failure) leaves the delete's refusal standing rather than reporting a deletion
/// nothing carried out.
async fn delete_model_package_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "ModelPackageGroupName": physical_resource_id });
    let refusal = match call_json_rpc(
        ctx,
        SAGEMAKER_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{SAGEMAKER_TARGET_PREFIX}.DeleteModelPackageGroup"),
        &request,
    )
    .await
    {
        Ok(_) => return Ok(()),
        Err(refusal) => refusal,
    };
    let described = call_json_rpc(
        ctx,
        SAGEMAKER_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{SAGEMAKER_TARGET_PREFIX}.DescribeModelPackageGroup"),
        &request,
    )
    .await;
    if matches!(&described, Err(absence) if absence.code == SAGEMAKER_VALIDATION_EXCEPTION) {
        // SageMaker does not have the group, which is the state the delete asked for.
        return Ok(());
    }
    Err(refusal)
}

/// The model group's configuration as SageMaker reports it, for drift detection.
///
/// A group SageMaker no longer holds answers `ValidationException` (it validates the
/// group's existence rather than modelling a not-found error), which is the `DELETED`
/// drift status. The group's own resource policy is not part of that answer, so it is read
/// through the operation that owns it and reported under the property the template declares
/// it as; a group carrying none simply has no such property, and a property the owning
/// service does not report is not evidence of drift.
async fn describe_model_package_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "ModelPackageGroupName": physical_resource_id });
    let described = match call_json_rpc(
        ctx,
        SAGEMAKER_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{SAGEMAKER_TARGET_PREFIX}.DescribeModelPackageGroup"),
        &request,
    )
    .await
    {
        Ok(described) => described,
        Err(refusal) if refusal.code == SAGEMAKER_VALIDATION_EXCEPTION => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    let Some(mut configuration) = described.as_object().cloned() else {
        return Ok(Some(described));
    };
    // The policy read's own absence refusal means the group carries no policy, which is a
    // configuration without that property. Any OTHER refusal is a read that did not
    // happen, and is propagated rather than reported as a property the service does not
    // have.
    match call_json_rpc(
        ctx,
        SAGEMAKER_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{SAGEMAKER_TARGET_PREFIX}.GetModelPackageGroupPolicy"),
        &request,
    )
    .await
    {
        Ok(policy) => {
            if let Some(document) = policy.get("ResourcePolicy") {
                configuration.insert(MODEL_PACKAGE_GROUP_POLICY.to_string(), document.clone());
            }
        }
        Err(refusal) if refusal.code == SAGEMAKER_VALIDATION_EXCEPTION => {}
        Err(refusal) => return Err(refusal),
    }
    Ok(Some(Value::Object(configuration)))
}

// ---------------------------------------------------------------------------
// AWS Fault Injection Service: AWS::FIS::ExperimentTemplate
// ---------------------------------------------------------------------------

/// AWS FIS's signing name, from its model (`sigv4Name: fis`). FIS speaks `restJson1`, so
/// its operations are bound to a METHOD and a URI rather than to an `X-Amz-Target` header:
/// `CreateExperimentTemplate` is `POST /experimentTemplates` and
/// `DeleteExperimentTemplate` is `DELETE /experimentTemplates/{id}`
/// (`smithy_query(query="operation", operation="CreateExperimentTemplate,\
/// DeleteExperimentTemplate")` on that service's model).
const FIS_SIGNING_NAME: &str = "fis";

/// The URI `CreateExperimentTemplate` is bound to.
const FIS_EXPERIMENT_TEMPLATES_URI: &str = "/experimentTemplates";

/// The error code FIS answers for an experiment template it does not hold
/// (`ResourceNotFoundException`, a modelled error of `DeleteExperimentTemplate`).
const FIS_RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The `AWS::FIS::ExperimentTemplate` properties `CreateExperimentTemplate` takes whose
/// value goes through unchanged under its lower-camel member name, as (property, member)
/// pairs.
///
/// Source: AWS's CloudFormation resource reference for the type against the
/// `CreateExperimentTemplateRequest` shape. The FIS model spells its members in lower
/// camel case and renames nothing, so the only difference is the first character's case;
/// `StopConditions`, `Targets`, `Actions`, `LogConfiguration` and `ExperimentOptions` are
/// NOT here because their own members have to be re-cased one structure at a time (a map
/// of targets or of actions is keyed by names the TEMPLATE chose, which must not be
/// re-cased), and `Tags` is not because the type spells it as a map of tag keys the
/// template chose.
const FIS_SCALAR_MEMBERS: &[(&str, &str)] =
    &[("Description", "description"), ("RoleArn", "roleArn")];

/// The members of one `ExperimentTemplateStopCondition` the type declares.
const FIS_STOP_CONDITION_MEMBERS: &[(&str, &str)] = &[("Source", "source"), ("Value", "value")];

/// The members of one `ExperimentTemplateTarget` the type declares. `ResourceTags` and
/// `Parameters` are maps keyed by DATA (tag keys, and the target parameter names the
/// resource type publishes), so their values go through as the template wrote them.
const FIS_TARGET_MEMBERS: &[(&str, &str)] = &[
    ("ResourceType", "resourceType"),
    ("SelectionMode", "selectionMode"),
    ("ResourceArns", "resourceArns"),
    ("ResourceTags", "resourceTags"),
    ("Parameters", "parameters"),
];

/// The members of one `ExperimentTemplateTargetFilter`, which a target's `Filters` list
/// carries.
const FIS_FILTER_MEMBERS: &[(&str, &str)] = &[("Path", "path"), ("Values", "values")];

/// The members of one `ExperimentTemplateAction` the type declares. `Parameters` is keyed
/// by the action's published parameter names and `Targets` by the action's published target
/// names, so both go through as the template wrote them.
const FIS_ACTION_MEMBERS: &[(&str, &str)] = &[
    ("ActionId", "actionId"),
    ("Description", "description"),
    ("Parameters", "parameters"),
    ("Targets", "targets"),
    ("StartAfter", "startAfter"),
];

/// The members of the type's `LogConfiguration` property, and of the two configurations it
/// nests.
const FIS_LOG_CONFIGURATION_MEMBERS: &[(&str, &str)] = &[("LogSchemaVersion", "logSchemaVersion")];
const FIS_CLOUDWATCH_LOGS_MEMBERS: &[(&str, &str)] = &[("LogGroupArn", "logGroupArn")];
const FIS_S3_LOG_MEMBERS: &[(&str, &str)] = &[("BucketName", "bucketName"), ("Prefix", "prefix")];

/// The members of the type's `ExperimentOptions` property.
const FIS_EXPERIMENT_OPTIONS_MEMBERS: &[(&str, &str)] = &[
    ("AccountTargeting", "accountTargeting"),
    ("EmptyTargetResolutionMode", "emptyTargetResolutionMode"),
];

/// The one property of the type this provisioner does not map onto the create's request.
const FIS_REPORT_CONFIGURATION: &str = "ExperimentReportConfiguration";

/// Create the experiment template an `AWS::FIS::ExperimentTemplate` declares, so a client
/// that reads the id out of the stack's outputs can call FIS with it
/// (`GetExperimentTemplate`, `StartExperiment`) rather than being handed an id FIS never
/// minted.
///
/// FIS ASSIGNS the id — `Ref` of this type returns it, and the type has no name property —
/// so the id this call answers with becomes the resource's physical id, replacing the
/// stand-in the naming rules computed, and is reported back as the `Id` attribute
/// `Fn::GetAtt` reads.
///
/// `clientToken` is the create's `@idempotencyToken`, required by the model: it is the
/// computed physical id, which is a function of the stack and the resource's logical id, so
/// the same declaration of the same stack asks for the same template rather than a second
/// one.
async fn create_experiment_template(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let properties = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    if properties.contains_key(FIS_REPORT_CONFIGURATION) {
        // The property is not mapped onto `CreateExperimentTemplate`'s own
        // `experimentReportConfiguration`, so creating the template would create one
        // configured differently from the declaration. Refused rather than silently
        // dropped; mapping it is its own fix.
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not implement the {FIS_REPORT_CONFIGURATION} property of \
                 {}",
                resource.resource_type
            ),
        });
    }
    let mut request = Map::new();
    request.insert(
        "clientToken".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    copy_members(&properties, FIS_SCALAR_MEMBERS, &mut request);
    if let Some(Value::Array(conditions)) = properties.get("StopConditions") {
        let conditions: Vec<Value> = conditions
            .iter()
            .map(|condition| mapped_members(condition, FIS_STOP_CONDITION_MEMBERS))
            .collect();
        request.insert("stopConditions".to_string(), Value::Array(conditions));
    }
    if let Some(Value::Object(targets)) = properties.get("Targets") {
        let targets: Map<String, Value> = targets
            .iter()
            .map(|(name, target)| (name.clone(), experiment_target(target)))
            .collect();
        request.insert("targets".to_string(), Value::Object(targets));
    }
    if let Some(Value::Object(actions)) = properties.get("Actions") {
        let actions: Map<String, Value> = actions
            .iter()
            .map(|(name, action)| (name.clone(), mapped_members(action, FIS_ACTION_MEMBERS)))
            .collect();
        request.insert("actions".to_string(), Value::Object(actions));
    }
    if let Some(configuration) = properties.get("LogConfiguration") {
        request.insert(
            "logConfiguration".to_string(),
            log_configuration(configuration),
        );
    }
    if let Some(options) = properties.get("ExperimentOptions") {
        request.insert(
            "experimentOptions".to_string(),
            mapped_members(options, FIS_EXPERIMENT_OPTIONS_MEMBERS),
        );
    }
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let reported = call_rest_json(
        ctx,
        FIS_SIGNING_NAME,
        region,
        "POST",
        FIS_EXPERIMENT_TEMPLATES_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let Some(id) = reported
        .get("experimentTemplate")
        .and_then(|template| template.get("id"))
        .and_then(Value::as_str)
    else {
        // FIS answered a success carrying no id, so there is nothing a client could call
        // it with: reported as a refusal rather than recorded as a created resource.
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: "CreateExperimentTemplate answered no experiment template id".to_string(),
        });
    };
    let mut attributes = BTreeMap::new();
    attributes.insert("Id".to_string(), id.to_string());
    Ok(Created {
        physical_resource_id: Some(id.to_string()),
        attributes,
    })
}

/// Delete the experiment template a stack created, by the id FIS minted for it — this
/// resource's physical id. A template FIS no longer holds
/// (`ResourceNotFoundException`) is already in the state the delete asks for.
async fn delete_experiment_template(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    match call_rest_json(
        ctx,
        FIS_SIGNING_NAME,
        region,
        "DELETE",
        &format!("{FIS_EXPERIMENT_TEMPLATES_URI}/{physical_resource_id}"),
        None,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == FIS_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The experiment template's configuration as FIS reports it, for drift detection:
/// `GetExperimentTemplate`'s own record, read by the id FIS minted.
///
/// A template FIS no longer holds (`ResourceNotFoundException`) was deleted. The record's
/// members are FIS's own (lower camel), so only a declared property spelled identically on
/// both sides takes part in the comparison — the position the Amazon ECS arm above is in
/// too, and what [`super::drift`] documents: a name mismatch reports the resource as
/// existing rather than as modified.
async fn describe_experiment_template(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    match call_rest_json(
        ctx,
        FIS_SIGNING_NAME,
        region,
        "GET",
        &format!("{FIS_EXPERIMENT_TEMPLATES_URI}/{physical_resource_id}"),
        None,
    )
    .await
    {
        Ok(answer) => Ok(answer.get("experimentTemplate").cloned()),
        Err(refusal) if refusal.code == FIS_RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// One `ExperimentTemplateTarget` as `CreateExperimentTemplate` takes it: its own members
/// re-cased, and its `Filters` list — whose entries are modelled structures of their own —
/// mapped entry by entry.
fn experiment_target(target: &Value) -> Value {
    let mut mapped = mapped_members(target, FIS_TARGET_MEMBERS);
    if let Some(Value::Array(filters)) = target.get("Filters") {
        let filters: Vec<Value> = filters
            .iter()
            .map(|filter| mapped_members(filter, FIS_FILTER_MEMBERS))
            .collect();
        if let Value::Object(members) = &mut mapped {
            members.insert("filters".to_string(), Value::Array(filters));
        }
    }
    mapped
}

/// The type's `LogConfiguration` property as `CreateExperimentTemplate` takes it: its own
/// member plus the two nested configurations, each mapped member by member.
fn log_configuration(configuration: &Value) -> Value {
    let mut mapped = mapped_members(configuration, FIS_LOG_CONFIGURATION_MEMBERS);
    let Value::Object(members) = &mut mapped else {
        return mapped;
    };
    if let Some(logs) = configuration.get("CloudWatchLogsConfiguration") {
        members.insert(
            "cloudWatchLogsConfiguration".to_string(),
            mapped_members(logs, FIS_CLOUDWATCH_LOGS_MEMBERS),
        );
    }
    if let Some(s3) = configuration.get("S3Configuration") {
        members.insert(
            "s3Configuration".to_string(),
            mapped_members(s3, FIS_S3_LOG_MEMBERS),
        );
    }
    mapped
}

/// One declared structure with the members `pairs` names mapped onto the member names the
/// owning service's model spells, and every value passed through as the template wrote it.
///
/// A property `pairs` does not name is one the create has no member for, so it is not
/// sent: the request carries what the declaration said and nothing invented.
fn mapped_members(declared: &Value, pairs: &[(&str, &str)]) -> Value {
    let mut mapped = Map::new();
    if let Some(members) = declared.as_object() {
        copy_members(members, pairs, &mut mapped);
    }
    Value::Object(mapped)
}

/// Copy the members `pairs` names out of `declared` into `request`, under the member name
/// each pair gives.
fn copy_members(
    declared: &Map<String, Value>,
    pairs: &[(&str, &str)],
    request: &mut Map<String, Value>,
) {
    for (property, member) in pairs {
        if let Some(value) = declared.get(*property) {
            request.insert((*member).to_string(), value.clone());
        }
    }
}

// ---------------------------------------------------------------------------
// AWS WAF Classic, regional: AWS::WAFRegional::IPSet
// ---------------------------------------------------------------------------

/// AWS WAF Classic Regional's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: waf-regional`, service shape `AWSWAF_Regional_20161128`), which speaks
/// `awsJson1_1`.
const WAF_REGIONAL_SIGNING_NAME: &str = "waf-regional";
const WAF_REGIONAL_TARGET_PREFIX: &str = "AWSWAF_Regional_20161128";

/// The error code AWS WAF Classic answers for an object it does not hold
/// (`WAFNonexistentItemException`: "the referenced object doesn't exist"), and the one
/// `UpdateIPSet` answers when the IPSet the updates are for is not there at all
/// (`WAFNonexistentContainerException`: "you tried to add an IP address to or delete an IP
/// address from an IPSet that doesn't exist").
const WAF_NONEXISTENT_ITEM: &str = "WAFNonexistentItemException";
const WAF_NONEXISTENT_CONTAINER: &str = "WAFNonexistentContainerException";

/// The two `ChangeAction` values an `UpdateIPSet` update carries: "whether to insert or
/// delete the IP address".
const WAF_INSERT: &str = "INSERT";
const WAF_DELETE: &str = "DELETE";

/// Create the IPSet an `AWS::WAFRegional::IPSet` declares, and answer the `IPSetId` AWS WAF
/// Classic minted as the resource's physical id — "Ref returns the IPSet ID", which is a
/// value only that service produces.
///
/// The type's properties are `Name` and `IPSetDescriptors`, and AWS WAF Classic takes them
/// through the two calls its own documentation sequences: "Submit a CreateIPSet request …
/// Submit an UpdateIPSet request to specify the IP addresses that you want AWS WAF to watch
/// for". So the create carries the name, and each declared descriptor is INSERTed by one
/// update. Every mutating call carries its own change token ("Use GetChangeToken to get the
/// change token that you provide in the ChangeToken parameter"), so a resource whose
/// descriptors the template declares is created with them rather than with the name alone.
async fn create_ip_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let request = serde_json::json!({
        "Name": declared_name(resource),
        "ChangeToken": waf_regional_change_token(ctx, region).await?,
    });
    let reported = call_waf_regional(ctx, region, "CreateIPSet", &request).await?;
    let Some(ip_set_id) = reported
        .pointer("/IPSet/IPSetId")
        .and_then(Value::as_str)
        .map(ToString::to_string)
    else {
        // The create answered success and identified nothing, so there is no id to address
        // the IPSet by: not something this stack can report as a created resource.
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: "CreateIPSet answered no IPSet.IPSetId".to_string(),
        });
    };
    let descriptors = declared_ip_set_descriptors(&resource.properties);
    if !descriptors.is_empty() {
        let updates: Vec<Value> = descriptors
            .into_iter()
            .map(|descriptor| {
                serde_json::json!({ "Action": WAF_INSERT, "IPSetDescriptor": descriptor })
            })
            .collect();
        if let Err(refusal) = update_ip_set(ctx, region, &ip_set_id, updates).await {
            // The resource the template declares is the IPSet WITH its addresses, and this
            // create could not make it. The empty IPSet the first call left is deleted
            // again, so a create the stack reports as failed leaves nothing behind in the
            // owning service; the refusal reported is the one that failed.
            let _ = delete_ip_set(ctx, region, &ip_set_id).await;
            return Err(refusal);
        }
    }
    Ok(Created {
        physical_resource_id: Some(ip_set_id),
        attributes: BTreeMap::new(),
    })
}

/// Delete the IPSet a stack created.
///
/// "You can't delete an IPSet if it still includes any IP addresses", so the addresses go
/// first, which is the sequence AWS WAF Classic documents for permanently deleting one:
/// "Update the IPSet to remove IP address ranges, if any. Submit a DeleteIPSet request".
/// The IPSet is read back, every descriptor it still holds is DELETEd by one update, and
/// the IPSet itself is then deleted.
///
/// An IPSet AWS WAF Classic no longer holds is already in the state the delete asks for,
/// judged on its own not-found CODES (`WAFNonexistentItemException` from the read and the
/// delete, `WAFNonexistentContainerException` from the update that empties it) and never on
/// a message, which carries the request's own id. One a Rule still references is NOT
/// deleted (`WAFReferencedItemException`), and that refusal is what the stack reports.
async fn delete_ip_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let Some(ip_set) = describe_ip_set(ctx, region, physical_resource_id).await? else {
        return Ok(());
    };
    let descriptors = ip_set
        .get("IPSetDescriptors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !descriptors.is_empty() {
        let updates: Vec<Value> = descriptors
            .into_iter()
            .map(|descriptor| {
                serde_json::json!({ "Action": WAF_DELETE, "IPSetDescriptor": descriptor })
            })
            .collect();
        match update_ip_set(ctx, region, physical_resource_id, updates).await {
            Ok(()) => {}
            Err(refusal) if refusal.code == WAF_NONEXISTENT_CONTAINER => return Ok(()),
            Err(refusal) => return Err(refusal),
        }
    }
    let request = serde_json::json!({
        "IPSetId": physical_resource_id,
        "ChangeToken": waf_regional_change_token(ctx, region).await?,
    });
    match call_waf_regional(ctx, region, "DeleteIPSet", &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == WAF_NONEXISTENT_ITEM => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The IPSet as AWS WAF Classic reports it, for drift detection and for the delete that has
/// to empty it first. An IPSet it no longer holds (`WAFNonexistentItemException`) was
/// deleted.
///
/// The `IPSet` the read answers carries `Name` and `IPSetDescriptors` under exactly the
/// names the type's properties have, so it is the actual configuration the declared one is
/// compared against.
async fn describe_ip_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "IPSetId": physical_resource_id });
    match call_waf_regional(ctx, region, "GetIPSet", &request).await {
        Ok(answer) => Ok(Some(reported_member(&answer, "IPSet"))),
        Err(refusal) if refusal.code == WAF_NONEXISTENT_ITEM => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Apply one batch of `Updates` to an IPSet, under a change token of its own.
async fn update_ip_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    ip_set_id: &str,
    updates: Vec<Value>,
) -> Result<(), Refusal> {
    let request = serde_json::json!({
        "IPSetId": ip_set_id,
        "ChangeToken": waf_regional_change_token(ctx, region).await?,
        "Updates": updates,
    });
    call_waf_regional(ctx, region, "UpdateIPSet", &request)
        .await
        .map(|_| ())
}

/// The `IPSetDescriptors` an `AWS::WAFRegional::IPSet` declares, as `UpdateIPSet` takes
/// them.
///
/// An `IPSetDescriptor` is `{Type, Value}` on both sides — the property's members are the
/// API shape's own — so each entry goes through as the template wrote it. A declaration
/// AWS WAF Classic will not take is not filtered out here: that service's own validation is
/// what judges a descriptor, and its refusal leaves the stack `CREATE_FAILED` rather than
/// an IPSet configured differently from the template.
fn declared_ip_set_descriptors(properties: &Value) -> Vec<Value> {
    properties
        .get("IPSetDescriptors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The NAME a resource record declares, for a type whose PHYSICAL id is the identifier the
/// owning service minted: the `Name` the record's attribute carries, which is the name the
/// template asked for or the `<stack>-<logical id>-<suffix>` name CloudFormation generated
/// for it — never the physical id, which for such a type is the service's own identifier
/// once the create went through.
///
/// A record with no such attribute (one whose attribute budget was spent) falls back to the
/// physical id, which before the create IS that computed name. Shared by every arm in that
/// position (an AWS WAF Classic regional IPSet, an Amazon Managed Grafana workspace), so
/// the create of one cannot read the declared name differently from the create of another.
fn declared_name(resource: &StoredResource) -> String {
    resource
        .attributes
        .get("Name")
        .cloned()
        .unwrap_or_else(|| resource.physical_resource_id.clone())
}

/// A change token for one mutating AWS WAF Classic call. "Each create, update, or delete
/// request must use a unique change token", so one is fetched per call rather than reused.
async fn waf_regional_change_token(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Result<String, Refusal> {
    let answer = call_waf_regional(ctx, region, "GetChangeToken", &serde_json::json!({})).await?;
    answer
        .get("ChangeToken")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| Refusal {
            code: "InternalFailure".to_string(),
            message: "GetChangeToken answered no ChangeToken".to_string(),
        })
}

/// Call one AWS WAF Classic Regional operation on the awsJson1_1 wire its model declares.
async fn call_waf_regional(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        WAF_REGIONAL_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{WAF_REGIONAL_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

// ---------------------------------------------------------------------------
// Amazon GuardDuty: AWS::GuardDuty::Detector
// ---------------------------------------------------------------------------

/// Amazon GuardDuty's signing name, from its model (`sigv4Name: guardduty`,
/// `endpointPrefix: guardduty`). GuardDuty speaks `restJson1`, so its operations are
/// addressed by their `@http` METHOD and URI rather than by an `X-Amz-Target`.
const GUARDDUTY_SIGNING_NAME: &str = "guardduty";

/// The URI `CreateDetector` is bound to, and the collection a detector's own URI hangs
/// off: `POST /detector` creates one, `GET /detector/{DetectorId}` reads it and
/// `DELETE /detector/{DetectorId}` deletes it
/// (`smithy_query(operation="CreateDetector,GetDetector,DeleteDetector")`).
const GUARDDUTY_DETECTOR_URI: &str = "/detector";

/// The error classes GuardDuty answers a client rejection under.
///
/// GuardDuty models ONE client error (`BadRequestException`, httpError 400) and answers
/// with the specific class in the body's `__type`
/// (`services/service-com-amazonaws-guardduty-guarddutyapiservice/src/scaffolded.rs`), so
/// the code alone does not distinguish a detector id that names no detector of this
/// account from any other rejection — which is why [`delete_detector`] confirms one by
/// reading the detector back rather than reading "already deleted" off the code.
const GUARDDUTY_INVALID_INPUT: &str = "InvalidInputException";
const GUARDDUTY_BAD_REQUEST: &str = "BadRequestException";

/// Create the detector an `AWS::GuardDuty::Detector` declares.
///
/// The type's properties ARE `CreateDetector`'s own members, spelled with an initial
/// capital — `Enable`, `FindingPublishingFrequency`, `DataSources` and `Features` against
/// `enable`, `findingPublishingFrequency`, `dataSources` and `features`, every one of them
/// a `jsonName` of `smithy_query(operation="CreateDetector")` — and the correspondence
/// holds all the way down (`DataSources.Kubernetes.AuditLogs.Enable` is
/// `dataSources.kubernetes.auditLogs.enable`;
/// `DataSources.MalwareProtection.ScanEc2InstanceWithFindings.EbsVolumes` is
/// `dataSources.malwareProtection.scanEc2InstanceWithFindings.ebsVolumes`; a `Features`
/// entry's `Name`, `Status` and `AdditionalConfiguration[].Status` are `name`, `status` and
/// `additionalConfiguration[].status` — `smithy_query(shape="DataSourceConfigurations,
/// KubernetesConfiguration,MalwareProtectionConfiguration,ScanEc2InstanceWithFindings,
/// S3LogsConfiguration,DetectorFeatureConfiguration,DetectorAdditionalConfiguration")`).
/// So the whole evaluated document is carried through [`lower_camel_members`] rather than
/// mapped member by member, which is the same re-casing `AWS::ECS::Cluster` needs and is
/// safe for the same reason: no property of this type carries a MAP of caller data. The
/// one exception is `Tags`, a list of `{Key, Value}` pairs where the create takes a map
/// ([`tags_as_map`]), lifted out before the walk — carrying the tags CloudFormation gives
/// the resource beside the declared ones ([`tags_of`]).
///
/// VALUES are untouched: an enum value, a boolean and a number are what the template
/// declared, and GuardDuty's own validation is what judges them — its refusal leaves the
/// stack `CREATE_FAILED` rather than a detector configured differently from the template.
///
/// A detector's id is GuardDuty's OWN to mint — `CreateDetector` takes no identifier and
/// answers with `detectorId` — so that answer is reported back as the resource's physical
/// id, which is what `Ref` resolves to and what a client then passes as `DetectorId`. It is
/// also the type's one published `Fn::GetAtt` attribute (`Id`), so that is re-reported off
/// the identity GuardDuty assigned rather than the one the naming rules computed.
async fn create_detector(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = Map::new();
    if let Some(properties) = resource.properties.as_object() {
        for (property, value) in properties {
            if property == "Tags" {
                continue;
            }
            request.insert(lower_initial(property), lower_camel_members(value, 0)?);
        }
    }
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let reported = call_rest_json(
        ctx,
        GUARDDUTY_SIGNING_NAME,
        region,
        "POST",
        GUARDDUTY_DETECTOR_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let Some(detector_id) = reported.get("detectorId").and_then(Value::as_str) else {
        // The create answered 2xx without the id its output models. Nothing addresses a
        // detector without one, so this is reported as a failure rather than recorded as
        // a resource with a physical id nothing minted.
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: "CreateDetector answered no detectorId".to_string(),
        });
    };
    let mut attributes = BTreeMap::new();
    attributes.insert("Id".to_string(), detector_id.to_string());
    Ok(Created {
        physical_resource_id: Some(detector_id.to_string()),
        attributes,
    })
}

/// Delete the detector a stack created, addressed by its id — this resource's physical id,
/// which is what `Ref` returns for the type.
///
/// A detector GuardDuty no longer resolves the id to is already in the state the delete
/// asks for. GuardDuty answers every client rejection under one class, so the code alone
/// does not say WHICH rejection it was: a refusal is confirmed by reading the detector
/// back, and only a read that refuses it too — the id resolves to no detector — is treated
/// as already deleted. A refusal of a detector GuardDuty still holds is reported, so
/// nothing passes off a deletion that did not happen.
async fn delete_detector(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let uri = format!("{GUARDDUTY_DETECTOR_URI}/{physical_resource_id}");
    let refusal =
        match call_rest_json(ctx, GUARDDUTY_SIGNING_NAME, region, "DELETE", &uri, None).await {
            Ok(_) => return Ok(()),
            Err(refusal) => refusal,
        };
    if !guardduty_rejected(&refusal) {
        return Err(refusal);
    }
    match call_rest_json(ctx, GUARDDUTY_SIGNING_NAME, region, "GET", &uri, None).await {
        // The detector is still there, so the delete really was refused.
        Ok(_) => Err(refusal),
        // GuardDuty does not resolve the id: the detector is already in the state the
        // delete asks for. Judged on the client-rejection class, so a transport or
        // internal failure of the confirming read is reported rather than mistaken for a
        // deletion — that failure says nothing about whether the detector is there.
        Err(confirmation) if guardduty_rejected(&confirmation) => Ok(()),
        Err(confirmation) => Err(confirmation),
    }
}

/// The detector's configuration as GuardDuty reports it, for drift detection.
///
/// A detector GuardDuty no longer resolves the id to was deleted: a READ has only that one
/// reading of the client-rejection class this service answers with (unlike
/// [`delete_detector`], where the same class could also be a refusal of the delete
/// itself), so that class is `Ok(None)`.
///
/// Only the two properties `GetDetector` reports in the SAME shape the type declares them
/// in are reported, because a property the actual document does not carry is not evidence
/// of anything (`super::drift::differs`): `Enable`, which the detector's required `Status`
/// is — "a Boolean value that specifies whether the detector is to be enabled", and
/// `DetectorStatus` is `ENABLED`/`DISABLED` — and `FindingPublishingFrequency`, the same
/// enum on both sides. `DataSources` and `Features` are reported as RESULT shapes, with a
/// per-switch status and the instant it last changed rather than the enable flags the
/// properties are, and `Tags` as a map where the property is a list of pairs; comparing
/// either would need a mapping this module would have to invent, so neither is reported
/// and neither is ever read as drift.
async fn describe_detector(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    /// The `DetectorStatus` of a detector that is enabled (`smithy_query(shape=
    /// "DetectorStatus")`: `DISABLED | ENABLED`).
    const ENABLED: &str = "ENABLED";

    let uri = format!("{GUARDDUTY_DETECTOR_URI}/{physical_resource_id}");
    let reported =
        match call_rest_json(ctx, GUARDDUTY_SIGNING_NAME, region, "GET", &uri, None).await {
            Ok(reported) => reported,
            Err(refusal) if guardduty_rejected(&refusal) => return Ok(None),
            Err(refusal) => return Err(refusal),
        };
    let mut actual = Map::new();
    if let Some(status) = reported.get("status").and_then(Value::as_str) {
        actual.insert("Enable".to_string(), Value::Bool(status == ENABLED));
    }
    if let Some(frequency) = reported.get("findingPublishingFrequency") {
        actual.insert("FindingPublishingFrequency".to_string(), frequency.clone());
    }
    Ok(Some(Value::Object(actual)))
}

/// Whether a refusal is GuardDuty REJECTING the request — the class a detector id that
/// resolves to no detector of this account comes back under — rather than a transport or
/// internal failure, which says nothing about whether the detector is there.
fn guardduty_rejected(refusal: &Refusal) -> bool {
    refusal.code == GUARDDUTY_INVALID_INPUT || refusal.code == GUARDDUTY_BAD_REQUEST
}

// ---------------------------------------------------------------------------
// Amazon MSK: AWS::MSK::Cluster and AWS::MSK::ServerlessCluster
// ---------------------------------------------------------------------------

/// Amazon MSK's signing name, from its model (`sigv4Name: kafka`, `endpointPrefix: kafka`).
/// That service speaks `restJson1`, so its operations are bound to a METHOD and a URI
/// rather than to an `X-Amz-Target` header.
const MSK_SIGNING_NAME: &str = "kafka";

/// The URI `CreateCluster` is bound to, and the collection a cluster's own URI hangs off:
/// `POST /v1/clusters` creates a provisioned cluster, `GET /v1/clusters/{ClusterArn}` reads
/// one and `DELETE /v1/clusters/{ClusterArn}` deletes a cluster of EITHER kind
/// (`smithy_query(query="operation", operation="CreateCluster,DescribeCluster,\
/// DeleteCluster")` on that service's model).
const MSK_CLUSTERS_URI: &str = "/v1/clusters";

/// The v2 collection, which is the one that serves a SERVERLESS cluster: `CreateClusterV2`
/// is `POST /api/v2/clusters` and `DescribeClusterV2` is
/// `GET /api/v2/clusters/{ClusterArn}`. The v1 `DescribeCluster` answers only for a
/// provisioned cluster — its `ClusterInfo` describes broker nodes, which a serverless
/// cluster has none of — while the v2 `Cluster` shape models a member for each kind.
const MSK_CLUSTERS_V2_URI: &str = "/api/v2/clusters";

/// The error code Amazon MSK answers for a cluster ARN it holds no cluster for
/// (`NotFoundException`, a modelled error of `DescribeCluster`, `DescribeClusterV2` and
/// `DeleteCluster`).
const MSK_NOT_FOUND: &str = "NotFoundException";

/// The `AWS::MSK::Cluster` properties `CreateCluster` has no member for.
///
/// `CurrentVersion` is the version token an UPDATE of a cluster that already exists must
/// match ("The current version of the MSK cluster"), and a create has no version to match
/// — so it is not carried out, and nothing about the cluster the create makes differs for
/// it. Every other property of the type IS a `CreateCluster` member ([`msk_members`]).
const MSK_UPDATE_ONLY_PROPERTIES: &[&str] = &["CurrentVersion"];

/// The one `AWS::MSK::Cluster` property name that is not its API member's name with the
/// initial letter lower-cased: the type spells the initialism of
/// `StorageInfo.EBSStorageInfo` in full capitals where the member is
/// `storageInfo.ebsStorageInfo` (`smithy_query(query="shape", shape="StorageInfo")`).
const MSK_RENAMED_MEMBERS: &[(&str, &str)] = &[("EBSStorageInfo", "ebsStorageInfo")];

/// The members one `AWS::MSK::ServerlessCluster` `VpcConfigs` entry carries, as
/// (property, member) pairs.
///
/// The entry is Amazon MSK's `VpcConfig` (`smithy_query(query="shape", shape="VpcConfig")`:
/// `subnetIds`, `securityGroupIds`), and the type publishes the security-group list as
/// `SecurityGroups` — a per-member correspondence rather than a case change, which is why
/// this one structure is mapped by name instead of walked.
const MSK_SERVERLESS_VPC_CONFIG_MEMBERS: &[(&str, &str)] =
    &[("SubnetIds", "subnetIds"), ("SecurityGroups", "securityGroupIds")];

/// Create the provisioned cluster an `AWS::MSK::Cluster` declares, and report the
/// `ClusterArn` Amazon MSK ASSIGNED it.
///
/// The type's properties ARE `CreateCluster`'s own members spelled with an initial capital
/// — `BrokerNodeGroupInfo`, `ClientAuthentication`, `ConfigurationInfo`, `EncryptionInfo`,
/// `EnhancedMonitoring`, `KafkaVersion`, `LoggingInfo`, `NumberOfBrokerNodes`,
/// `OpenMonitoring` and `StorageMode` against `brokerNodeGroupInfo`,
/// `clientAuthentication`, `configurationInfo`, `encryptionInfo`, `enhancedMonitoring`,
/// `kafkaVersion`, `loggingInfo`, `numberOfBrokerNodes`, `openMonitoring` and
/// `storageMode`, every one of them a `jsonName` of
/// `smithy_query(query="closure", operation="CreateCluster")` — and the correspondence
/// holds all the way down (`BrokerNodeGroupInfo.ClientSubnets` is
/// `brokerNodeGroupInfo.clientSubnets`; `ClientAuthentication.Sasl.Iam.Enabled` is
/// `clientAuthentication.sasl.iam.enabled`;
/// `LoggingInfo.BrokerLogs.CloudWatchLogs.LogGroup` is
/// `loggingInfo.brokerLogs.cloudWatchLogs.logGroup`;
/// `OpenMonitoring.Prometheus.JmxExporter.EnabledInBroker` is
/// `openMonitoring.prometheus.jmxExporter.enabledInBroker`). So the whole evaluated
/// document is carried through [`msk_members`] rather than mapped member by member, which
/// is safe for the same reason the Amazon ECS and GuardDuty walks are: no property of this
/// type carries a MAP of caller data. The exceptions are the one member whose API spelling
/// is not a case change ([`MSK_RENAMED_MEMBERS`]), the property the create has no member
/// for ([`MSK_UPDATE_ONLY_PROPERTIES`]) and `Tags`, which the type publishes as a MAP of
/// tag key to value exactly as the create takes one ([`tags_as_map`]) — lifted out before
/// the walk so it carries the tags CloudFormation gives the resource beside the declared
/// ones ([`tags_of`]).
///
/// VALUES are untouched: an enum value, a number and a boolean are what the template
/// declared, and Amazon MSK's own validation is what judges them — its refusal leaves the
/// stack `CREATE_FAILED` rather than a cluster configured differently from the template.
///
/// The cluster's NAME is the one value CloudFormation decides
/// ([`msk_cluster_name`]), so it is set on the way out; its identity is not
/// CloudFormation's at all ("Ref returns the ARN of the cluster", whose resource path
/// carries an identifier only Amazon MSK mints), so the ARN the create answers with is
/// reported as the resource's physical id ([`msk_created`]).
async fn create_msk_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = Map::new();
    if let Some(properties) = resource.properties.as_object() {
        for (property, value) in properties {
            if property == "Tags" || MSK_UPDATE_ONLY_PROPERTIES.contains(&property.as_str()) {
                continue;
            }
            request.insert(
                renamed_member(property, MSK_RENAMED_MEMBERS),
                msk_members(value)?,
            );
        }
    }
    request.insert(
        "clusterName".to_string(),
        Value::String(msk_cluster_name(resource)?),
    );
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let reported = call_rest_json(
        ctx,
        MSK_SIGNING_NAME,
        region,
        "POST",
        MSK_CLUSTERS_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    msk_created(resource, &reported)
}

/// Create the serverless cluster an `AWS::MSK::ServerlessCluster` declares, and report the
/// `ClusterArn` Amazon MSK ASSIGNED it.
///
/// A serverless cluster is created by `CreateClusterV2`, whose request carries the cluster
/// name and the tags beside ONE of its two alternative bodies — the `Serverless` one here.
/// That body's members are this type's `VpcConfigs` and `ClientAuthentication`
/// (`smithy_query(query="shape", shape="ServerlessRequest")`: `vpcConfigs`,
/// `clientAuthentication`), so the declaration's two configuration properties go under
/// `serverless` rather than at the top of the request. `ClientAuthentication` is the same
/// members with a lower-case initial all the way down (`Sasl.Iam.Enabled` is
/// `sasl.iam.enabled`) and is walked; a `VpcConfigs` entry is not, because the type
/// publishes its security-group list under a different name
/// ([`MSK_SERVERLESS_VPC_CONFIG_MEMBERS`]).
async fn create_msk_serverless_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut serverless = Map::new();
    if let Some(Value::Array(configs)) = resource.properties.get("VpcConfigs") {
        serverless.insert(
            "vpcConfigs".to_string(),
            Value::Array(
                configs
                    .iter()
                    .map(|config| mapped_members(config, MSK_SERVERLESS_VPC_CONFIG_MEMBERS))
                    .collect(),
            ),
        );
    }
    if let Some(authentication) = resource.properties.get("ClientAuthentication") {
        serverless.insert(
            "clientAuthentication".to_string(),
            msk_members(authentication)?,
        );
    }
    let mut request = Map::new();
    request.insert(
        "clusterName".to_string(),
        Value::String(msk_cluster_name(resource)?),
    );
    request.insert("serverless".to_string(), Value::Object(serverless));
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let reported = call_rest_json(
        ctx,
        MSK_SIGNING_NAME,
        region,
        "POST",
        MSK_CLUSTERS_V2_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    msk_created(resource, &reported)
}

/// Delete the cluster a stack created, addressed by the ARN Amazon MSK minted for it —
/// this resource's physical id, which is what `Ref` returns for either type.
///
/// `DeleteCluster` deletes a cluster of either kind, and a cluster Amazon MSK holds no
/// cluster for under that ARN (`NotFoundException`) is already in the state the delete asks
/// for. Judged on the error CODE, never on the message, which carries the request's own
/// ARN. The ARN goes into the URI percent-encoded, as a non-greedy `@httpLabel` is sent,
/// so its `:` and `/` cannot address a different operation.
async fn delete_msk_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let uri = format!(
        "{MSK_CLUSTERS_URI}/{}",
        percent_encoded(physical_resource_id)
    );
    match call_rest_json(ctx, MSK_SIGNING_NAME, region, "DELETE", &uri, None).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == MSK_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The cluster's configuration as Amazon MSK reports it, for drift detection: the record
/// `DescribeClusterV2` answers with, read by the ARN that service minted.
///
/// The v2 read serves both kinds of cluster, which is why it is the one this uses. An ARN
/// Amazon MSK holds no cluster for (`NotFoundException`) is the resource having been
/// deleted. The record's members are Amazon MSK's own (lower camel), so a declared property
/// spelled identically on both sides takes part in the comparison and one spelled
/// differently leaves the cluster reported as existing rather than as modified — exactly
/// what [`super::drift`] documents for every arm here.
async fn describe_msk_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let uri = format!(
        "{MSK_CLUSTERS_V2_URI}/{}",
        percent_encoded(physical_resource_id)
    );
    match call_rest_json(ctx, MSK_SIGNING_NAME, region, "GET", &uri, None).await {
        Ok(answer) => Ok(answer.get("clusterInfo").cloned()),
        Err(refusal) if refusal.code == MSK_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// What a cluster create answered, as the resource's identity: the `ClusterArn` Amazon MSK
/// minted.
///
/// "Ref returns the ARN of the cluster" for both types, and that ARN is also each type's
/// one published `Fn::GetAtt` attribute — so it is reported as the physical id
/// ([`Created::physical_resource_id`]), which
/// [`super::provisioning::reconcile`] adopts, and as `Arn`. The conventional
/// `<Type>Arn` name every resource also answers
/// ([`super::template::conventional_attributes`]) was computed from the stand-in identifier
/// the naming rules derived, so it is re-pointed at the minted ARN too: one resource must
/// not report two different ARNs.
fn msk_created(resource: &StoredResource, reported: &Value) -> Result<Created, Refusal> {
    let Some(arn) = reported.get("clusterArn").and_then(Value::as_str) else {
        // The create answered 2xx without the ARN its output models. Nothing addresses an
        // MSK cluster without one, so this is reported as a failure rather than recorded
        // as a resource with a physical id nothing minted.
        return Err(Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "Amazon MSK answered no clusterArn for the cluster it created".to_string(),
        });
    };
    let mut attributes = BTreeMap::new();
    attributes.insert("Arn".to_string(), arn.to_string());
    if let Some(segment) = resource.resource_type.rsplit("::").next() {
        attributes.insert(format!("{segment}Arn"), arn.to_string());
    }
    Ok(Created {
        physical_resource_id: Some(arn.to_string()),
        attributes,
    })
}

/// The name the cluster is created under: the `ClusterName` the declaration asks for —
/// which both types REQUIRE — and otherwise the name CloudFormation generated for the
/// resource, which [`super::template`] records as its `Name`.
///
/// A declaration carrying neither describes no cluster that could be created, so it is
/// refused here rather than with a request Amazon MSK would have to reject.
fn msk_cluster_name(resource: &StoredResource) -> Result<String, Refusal> {
    property(resource, "ClusterName")
        .or_else(|| {
            resource
                .attributes
                .get("Name")
                .filter(|name| !name.is_empty())
                .cloned()
        })
        .ok_or_else(|| Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!("{} declares no ClusterName", resource.resource_type),
        })
}

/// One Amazon MSK property value with every member name inside it spelled as that
/// service's model spells the very same member: the shared re-casing walk
/// ([`renamed_members`]) carrying the one member whose API name is not the property with
/// its initial letter lower-cased ([`MSK_RENAMED_MEMBERS`]).
fn msk_members(value: &Value) -> Result<Value, Refusal> {
    renamed_members(value, 0, MSK_RENAMED_MEMBERS)
}

// ---------------------------------------------------------------------------
// Amazon Managed Grafana: AWS::Grafana::Workspace
// ---------------------------------------------------------------------------

/// Amazon Managed Grafana's signing name, from its model (`sigv4Name: grafana`). That
/// service speaks `restJson1`, so its operations are bound to a METHOD and a URI rather
/// than to an `X-Amz-Target` header: `CreateWorkspace` is `POST /workspaces`,
/// `DeleteWorkspace` is `DELETE /workspaces/{workspaceId}` and `DescribeWorkspace` is
/// `GET /workspaces/{workspaceId}` (`smithy_query(query="operation",
/// operation="CreateWorkspace,DeleteWorkspace,DescribeWorkspace")` on that service's
/// model).
const GRAFANA_SIGNING_NAME: &str = "grafana";

/// The URI `CreateWorkspace` is bound to, and the collection the per-workspace URIs hang
/// off.
const GRAFANA_WORKSPACES_URI: &str = "/workspaces";

/// The error code Amazon Managed Grafana answers for a workspace it does not hold
/// (`ResourceNotFoundException`, a modelled error of `DeleteWorkspace` and
/// `DescribeWorkspace`).
const GRAFANA_RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The `AWS::Grafana::Workspace` properties `CreateWorkspace` takes whose value goes
/// through unchanged under the member named beside it, as (property, member) pairs.
///
/// Source: AWS's CloudFormation resource reference for the type against the
/// `CreateWorkspaceRequest` shape. The type spells the workspace's own fields without the
/// `workspace` prefix that API carries, so the correspondence is a per-member one rather
/// than a case change: `Description` is `workspaceDescription`, `RoleArn` is
/// `workspaceRoleArn`, `DataSources` is `workspaceDataSources`,
/// `NotificationDestinations` is `workspaceNotificationDestinations` and
/// `OrganizationalUnits` is `workspaceOrganizationalUnits`, while
/// `AccountAccessType`, `AuthenticationProviders`, `ClientToken`, `GrafanaVersion`,
/// `OrganizationRoleName`, `PermissionType` and `StackSetName` differ only in the first
/// character's case.
///
/// `Name` is NOT here: it is the one value CloudFormation decides (the name the template
/// declared, or the one it generated), so [`workspace_name`] is what the create carries
/// out. `VpcConfiguration` and `NetworkAccessControl` are not here either, because their
/// own members have to be mapped one structure at a time.
const GRAFANA_WORKSPACE_MEMBERS: &[(&str, &str)] = &[
    ("AccountAccessType", "accountAccessType"),
    ("AuthenticationProviders", "authenticationProviders"),
    ("ClientToken", "clientToken"),
    ("DataSources", "workspaceDataSources"),
    ("Description", "workspaceDescription"),
    ("GrafanaVersion", "grafanaVersion"),
    ("NotificationDestinations", "workspaceNotificationDestinations"),
    ("OrganizationRoleName", "organizationRoleName"),
    ("OrganizationalUnits", "workspaceOrganizationalUnits"),
    ("PermissionType", "permissionType"),
    ("RoleArn", "workspaceRoleArn"),
    ("StackSetName", "stackSetName"),
];

/// The members of the type's `VpcConfiguration` property, as `VpcConfiguration` declares
/// them.
const GRAFANA_VPC_MEMBERS: &[(&str, &str)] = &[
    ("SecurityGroupIds", "securityGroupIds"),
    ("SubnetIds", "subnetIds"),
];

/// The members of the type's `NetworkAccessControl` property, as
/// `NetworkAccessConfiguration` declares them.
const GRAFANA_NETWORK_ACCESS_MEMBERS: &[(&str, &str)] =
    &[("PrefixListIds", "prefixListIds"), ("VpceIds", "vpceIds")];

/// The properties of the type this provisioner does not map onto `CreateWorkspace`'s
/// request, because that operation takes neither: a workspace's SAML configuration is
/// written by `UpdateWorkspaceAuthentication` and its plugin administration by
/// `UpdateWorkspaceConfiguration`.
const GRAFANA_UNMAPPED_PROPERTIES: &[&str] = &["SamlConfiguration", "PluginAdminEnabled"];

/// Create the workspace an `AWS::Grafana::Workspace` declares, so a client that reads the
/// id out of the stack's outputs can call Amazon Managed Grafana with it
/// (`DescribeWorkspace`, `UpdateWorkspace`) rather than being handed an id that service
/// never minted.
///
/// Amazon Managed Grafana ASSIGNS the id — `Ref` of the type returns the workspace id,
/// a `^g-[0-9a-f]{10}$` value, while the type's `Name` is only what the create asks for —
/// so the id this call answers with becomes the resource's physical id, replacing the
/// stand-in the naming rules computed.
async fn create_workspace(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let properties = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    if let Some(property) = GRAFANA_UNMAPPED_PROPERTIES
        .iter()
        .find(|property| properties.contains_key(**property))
    {
        // The property is not mapped onto `CreateWorkspace`, so creating the workspace
        // would create one configured differently from the declaration. Refused rather
        // than silently dropped; mapping it is its own fix.
        return Err(Refusal {
            code: "NotImplemented".to_string(),
            message: format!(
                "this emulator does not implement the {property} property of {}",
                resource.resource_type
            ),
        });
    }
    let mut request = Map::new();
    copy_members(&properties, GRAFANA_WORKSPACE_MEMBERS, &mut request);
    request.insert(
        "workspaceName".to_string(),
        Value::String(declared_name(resource)),
    );
    if let Some(configuration) = properties.get("VpcConfiguration") {
        request.insert(
            "vpcConfiguration".to_string(),
            mapped_members(configuration, GRAFANA_VPC_MEMBERS),
        );
    }
    if let Some(configuration) = properties.get("NetworkAccessControl") {
        request.insert(
            "networkAccessControl".to_string(),
            mapped_members(configuration, GRAFANA_NETWORK_ACCESS_MEMBERS),
        );
    }
    let reported = call_rest_json(
        ctx,
        GRAFANA_SIGNING_NAME,
        region,
        "POST",
        GRAFANA_WORKSPACES_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let Some(id) = reported
        .get("workspace")
        .and_then(|workspace| workspace.get("id"))
        .and_then(Value::as_str)
    else {
        // The create answered a success identifying no workspace, so there is nothing a
        // client could call Amazon Managed Grafana with: reported as a refusal rather
        // than recorded as a created resource.
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: "CreateWorkspace answered no workspace id".to_string(),
        });
    };
    Ok(Created {
        physical_resource_id: Some(id.to_string()),
        attributes: BTreeMap::new(),
    })
}

/// Delete the workspace a stack created, by the id Amazon Managed Grafana minted for it —
/// this resource's physical id. A workspace that service no longer holds
/// (`ResourceNotFoundException`) is already in the state the delete asks for.
async fn delete_workspace(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    match call_rest_json(
        ctx,
        GRAFANA_SIGNING_NAME,
        region,
        "DELETE",
        &format!("{GRAFANA_WORKSPACES_URI}/{physical_resource_id}"),
        None,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == GRAFANA_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

// ---------------------------------------------------------------------------
// Amazon RDS: AWS::RDS::DBInstance, AWS::RDS::DBSubnetGroup,
// AWS::RDS::DBParameterGroup, AWS::RDS::DBClusterParameterGroup
// ---------------------------------------------------------------------------

/// Amazon RDS's signing name and API version, from its model (`sigv4Name: rds`, service
/// shape `AmazonRDSv19`, `version: 2014-10-31`), which speaks `awsQuery` — so its calls
/// ride on [`call_query`] rather than on [`call_json_rpc`].
const RDS_SIGNING_NAME: &str = "rds";
const RDS_API_VERSION: &str = "2014-10-31";

/// The error codes Amazon RDS answers for resources it does not hold:
/// `DBInstanceNotFoundFault`'s and `DBSubnetGroupNotFoundFault`'s own `awsQueryError`
/// codes, which is what an awsQuery client branches on.
const DB_INSTANCE_NOT_FOUND: &str = "DBInstanceNotFound";
const DB_SUBNET_GROUP_NOT_FOUND: &str = "DBSubnetGroupNotFoundFault";

/// `DBParameterGroupNotFoundFault`'s own `awsQueryError` code
/// (`smithy_query(query="operation", operation="DeleteDBParameterGroup")`), which is what
/// an awsQuery client branches on for a parameter group Amazon RDS does not hold — one code
/// for both group kinds, since both levels' operations model that same error shape.
const DB_PARAMETER_GROUP_NOT_FOUND: &str = "DBParameterGroupNotFound";

/// How one member of a create reaches an awsQuery form body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueryMember {
    /// One value under the member's own name (`Engine=mysql`).
    Scalar,
    /// A list, whose entries are indexed from 1 under the element name the list's own
    /// member `@xmlName` fixes (`SubnetIds.SubnetIdentifier.1`); the Smithy default for a
    /// list member that carries none is `member`. A STRUCTURED entry's own members follow
    /// the index (`Tags.Tag.1.Key`).
    List(&'static str),
}

/// The members `CreateDBInstance` takes, from Amazon RDS's own model
/// (`smithy_query(query="operation", operation="CreateDBInstance")`), with each list's
/// element name from its list shape's member `@xmlName`
/// (`smithy_query(query="shape", shape="VpcSecurityGroupIdList")`).
///
/// This is the catalogue the declaration of an `AWS::RDS::DBInstance` is walked against:
/// the type's properties ARE these members, so a property whose name is one of them (or
/// renames to one, [`DB_INSTANCE_RENAMED_PROPERTIES`]) is passed through under it.
const CREATE_DB_INSTANCE_MEMBERS: &[(&str, QueryMember)] = &[
    ("AllocatedStorage", QueryMember::Scalar),
    ("AutoMinorVersionUpgrade", QueryMember::Scalar),
    ("AvailabilityZone", QueryMember::Scalar),
    ("BackupRetentionPeriod", QueryMember::Scalar),
    ("BackupTarget", QueryMember::Scalar),
    ("CACertificateIdentifier", QueryMember::Scalar),
    ("CharacterSetName", QueryMember::Scalar),
    ("CopyTagsToSnapshot", QueryMember::Scalar),
    ("CustomIamInstanceProfile", QueryMember::Scalar),
    ("DBCapacityReservationIdentifier", QueryMember::Scalar),
    ("DBClusterIdentifier", QueryMember::Scalar),
    ("DBInstanceClass", QueryMember::Scalar),
    ("DBInstanceIdentifier", QueryMember::Scalar),
    ("DBName", QueryMember::Scalar),
    ("DBParameterGroupName", QueryMember::Scalar),
    ("DBSecurityGroups", QueryMember::List("DBSecurityGroupName")),
    ("DBSubnetGroupName", QueryMember::Scalar),
    ("DBSystemId", QueryMember::Scalar),
    ("DatabaseInsightsMode", QueryMember::Scalar),
    ("DedicatedLogVolume", QueryMember::Scalar),
    ("DeletionProtection", QueryMember::Scalar),
    ("Domain", QueryMember::Scalar),
    ("DomainAuthSecretArn", QueryMember::Scalar),
    ("DomainDnsIps", QueryMember::List("member")),
    ("DomainFqdn", QueryMember::Scalar),
    ("DomainIAMRoleName", QueryMember::Scalar),
    ("DomainOu", QueryMember::Scalar),
    ("EnableCloudwatchLogsExports", QueryMember::List("member")),
    ("EnableCustomerOwnedIp", QueryMember::Scalar),
    ("EnableIAMDatabaseAuthentication", QueryMember::Scalar),
    ("EnablePerformanceInsights", QueryMember::Scalar),
    ("Engine", QueryMember::Scalar),
    ("EngineLifecycleSupport", QueryMember::Scalar),
    ("EngineVersion", QueryMember::Scalar),
    ("Iops", QueryMember::Scalar),
    ("KmsKeyId", QueryMember::Scalar),
    ("LicenseModel", QueryMember::Scalar),
    ("ManageMasterUserPassword", QueryMember::Scalar),
    ("MasterUserAuthenticationType", QueryMember::Scalar),
    ("MasterUserPassword", QueryMember::Scalar),
    ("MasterUserSecretKmsKeyId", QueryMember::Scalar),
    ("MasterUsername", QueryMember::Scalar),
    ("MaxAllocatedStorage", QueryMember::Scalar),
    ("MonitoringInterval", QueryMember::Scalar),
    ("MonitoringRoleArn", QueryMember::Scalar),
    ("MultiAZ", QueryMember::Scalar),
    ("MultiTenant", QueryMember::Scalar),
    ("NcharCharacterSetName", QueryMember::Scalar),
    ("NetworkType", QueryMember::Scalar),
    ("OptionGroupName", QueryMember::Scalar),
    ("PerformanceInsightsKMSKeyId", QueryMember::Scalar),
    ("PerformanceInsightsRetentionPeriod", QueryMember::Scalar),
    ("Port", QueryMember::Scalar),
    ("PreferredBackupWindow", QueryMember::Scalar),
    ("PreferredMaintenanceWindow", QueryMember::Scalar),
    ("ProcessorFeatures", QueryMember::List("ProcessorFeature")),
    ("PromotionTier", QueryMember::Scalar),
    ("PubliclyAccessible", QueryMember::Scalar),
    ("StorageEncrypted", QueryMember::Scalar),
    ("StorageThroughput", QueryMember::Scalar),
    ("StorageType", QueryMember::Scalar),
    ("Tags", QueryMember::List("Tag")),
    ("TdeCredentialArn", QueryMember::Scalar),
    ("TdeCredentialPassword", QueryMember::Scalar),
    ("Timezone", QueryMember::Scalar),
    ("VpcSecurityGroupIds", QueryMember::List("VpcSecurityGroupId")),
];

/// The two properties of `AWS::RDS::DBInstance` whose `CreateDBInstance` member is spelled
/// differently: the resource type's `VPCSecurityGroups` is the API's
/// `VpcSecurityGroupIds` ("a list of the VPC security group IDs to assign to the DB
/// instance"), and its `CustomIAMInstanceProfile` is the API's
/// `CustomIamInstanceProfile`. Every other property is its member's own name.
const DB_INSTANCE_RENAMED_PROPERTIES: &[(&str, &str)] = &[
    ("VPCSecurityGroups", "VpcSecurityGroupIds"),
    ("CustomIAMInstanceProfile", "CustomIamInstanceProfile"),
];

/// The properties of `AWS::RDS::DBInstance` the walk over the declaration does NOT turn
/// into a `CreateDBInstance` member, each for a reason of its own. Every other property
/// the catalogue does not carry is refused rather than dropped ([`query_members`]).
const DB_INSTANCE_PROPERTIES_NOT_SENT: &[&str] = &[
    // The identifier CloudFormation decides, which is this resource's physical id ("Ref
    // returns the DBInstanceIdentifier"): the arm sets the member from that id, so the
    // walk must not also push the declared spelling of it.
    "DBInstanceIdentifier",
    // The tag list carries the declared tags AND the ones CloudFormation propagates to the
    // resource, so the arm builds it out of both ([`tags_of`]).
    "Tags",
    // Members of a DIFFERENT call, which say nothing about the instance a create makes:
    // `ModifyDBInstance` takes `AllowMajorVersionUpgrade`, `ApplyImmediately`,
    // `CertificateRotationRestart` and `UseDefaultProcessorFeatures` (each governs how a
    // LATER modification is applied), and `DeleteDBInstance` takes
    // `DeleteAutomatedBackups` — which is why [`delete_db_instance`] reads that one off
    // the resource's own record when it deletes the instance.
    "AllowMajorVersionUpgrade",
    "ApplyImmediately",
    "CertificateRotationRestart",
    "UseDefaultProcessorFeatures",
    "DeleteAutomatedBackups",
    // Read-only members CloudFormation publishes as this type's RETURN values: a create
    // neither takes them nor could be told them.
    "CertificateDetails",
    "DbiResourceId",
    "Endpoint",
    "MasterUserSecret",
];

/// The members `CreateDBSubnetGroup` takes, from the same model, read the same way.
const CREATE_DB_SUBNET_GROUP_MEMBERS: &[(&str, QueryMember)] = &[
    ("DBSubnetGroupDescription", QueryMember::Scalar),
    ("DBSubnetGroupName", QueryMember::Scalar),
    ("SubnetIds", QueryMember::List("SubnetIdentifier")),
    ("Tags", QueryMember::List("Tag")),
];

/// `AWS::RDS::DBSubnetGroup`'s two properties the arm itself pushes, for the same reasons
/// the DB instance's are ([`DB_INSTANCE_PROPERTIES_NOT_SENT`]): the name CloudFormation
/// decides, and the tag list it merges.
const DB_SUBNET_GROUP_PROPERTIES_NOT_SENT: &[&str] = &["DBSubnetGroupName", "Tags"];

/// One Amazon RDS parameter-group TYPE, as the calls that make a group of it exist, carry a
/// caller's settings, stop existing and be read.
///
/// `AWS::RDS::DBParameterGroup` and `AWS::RDS::DBClusterParameterGroup` declare the same
/// document — `Description`, `Family`, the `Parameters` map, the name and the tag list — and
/// their owning APIs differ only in the member each addresses a group by and in the action
/// names, so ONE arm serves both and the difference between them is this record.
#[derive(Debug, Clone, Copy)]
struct ParameterGroupCalls {
    /// The resource type, which a property outside the create's catalogue is refused by
    /// name ([`query_members`]).
    resource_type: &'static str,
    /// The member every one of these calls addresses the group by, which is also what this
    /// type's `Ref` returns and so this resource's physical id.
    name_member: &'static str,
    /// The members the create takes, from Amazon RDS's own model, with each list's element
    /// name from its list shape's member `@xmlName`
    /// (`smithy_query(query="shape", shape="TagList")`).
    create_members: &'static [(&'static str, QueryMember)],
    create: &'static str,
    modify: &'static str,
    delete: &'static str,
    describe: &'static str,
    /// The element the describe reports one group as.
    record: &'static str,
}

impl ParameterGroupCalls {
    /// The declared properties the walk does NOT turn into a create member: the name
    /// CloudFormation decides and the tag list it merges, as for the two types above, plus
    /// the `Parameters` MAP, which is no member of the create at all — a group's parameter
    /// values are written by the modify [`modify_parameters`] makes next.
    fn properties_not_sent(&self) -> [&'static str; 3] {
        [self.name_member, "Parameters", "Tags"]
    }
}

/// `AWS::RDS::DBParameterGroup`, whose group is the one a DB instance's
/// `DBParameterGroupName` names and `DescribeDBParameters` reports the parameters of
/// (`smithy_query(query="operation", operation="CreateDBParameterGroup")`).
const DB_PARAMETER_GROUP_CALLS: ParameterGroupCalls = ParameterGroupCalls {
    resource_type: RDS_DB_PARAMETER_GROUP,
    name_member: "DBParameterGroupName",
    create_members: &[
        ("DBParameterGroupFamily", QueryMember::Scalar),
        ("DBParameterGroupName", QueryMember::Scalar),
        ("Description", QueryMember::Scalar),
        ("Tags", QueryMember::List("Tag")),
    ],
    create: "CreateDBParameterGroup",
    modify: "ModifyDBParameterGroup",
    delete: "DeleteDBParameterGroup",
    describe: "DescribeDBParameterGroups",
    record: "DBParameterGroup",
};

/// `AWS::RDS::DBClusterParameterGroup`, the cluster-scoped sibling: the group a DB cluster's
/// `DBClusterParameterGroupName` names and `DescribeDBClusterParameters` reports the
/// parameters of. Its create takes the family under the SAME member name as the
/// instance-scoped one (`DBParameterGroupFamily`) and the group under its own
/// (`smithy_query(query="operation", operation="CreateDBClusterParameterGroup")`).
const DB_CLUSTER_PARAMETER_GROUP_CALLS: ParameterGroupCalls = ParameterGroupCalls {
    resource_type: RDS_DB_CLUSTER_PARAMETER_GROUP,
    name_member: "DBClusterParameterGroupName",
    create_members: &[
        ("DBClusterParameterGroupName", QueryMember::Scalar),
        ("DBParameterGroupFamily", QueryMember::Scalar),
        ("Description", QueryMember::Scalar),
        ("Tags", QueryMember::List("Tag")),
    ],
    create: "CreateDBClusterParameterGroup",
    modify: "ModifyDBClusterParameterGroup",
    delete: "DeleteDBClusterParameterGroup",
    describe: "DescribeDBClusterParameterGroups",
    record: "DBClusterParameterGroup",
};

/// The one property of either parameter-group type whose create member is spelled
/// differently: the types' `Family` is the API's `DBParameterGroupFamily` ("the DB parameter
/// group family name"). Every other property is its member's own name.
const PARAMETER_GROUP_RENAMED_PROPERTIES: &[(&str, &str)] =
    &[("Family", "DBParameterGroupFamily")];

/// How many parameters one modify carries: "A maximum of 20 parameters can be modified in a
/// single request" (`ModifyDBParameterGroupMessage$Parameters`, and the cluster-level
/// request's own `Parameters`), so a declaration carrying more reaches Amazon RDS as several
/// calls rather than as one request that service would refuse.
const PARAMETERS_PER_MODIFY: usize = 20;

/// Create the DB instance an `AWS::RDS::DBInstance` declares.
///
/// The type's properties are `CreateDBInstance`'s own members
/// ([`CREATE_DB_INSTANCE_MEMBERS`]), so the evaluated declaration IS the request, with the
/// one value CloudFormation decides — the `DBInstanceIdentifier`, which is this resource's
/// physical id — set on the way out, and the tag list built from the declared tags plus the
/// ones CloudFormation propagates.
///
/// What the create ANSWERS replaces the identifiers CloudFormation's naming rules had
/// computed for the resource: the ARN, the `DbiResourceId` and the endpoint are Amazon
/// RDS's to assign, and the whole point of provisioning the instance is that the handle the
/// stack publishes addresses the database RDS really holds. The identifier itself is not
/// replaced — it is the one the instance was created under.
async fn create_db_instance(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut parameters = query_members(
        &resource.properties,
        CREATE_DB_INSTANCE_MEMBERS,
        DB_INSTANCE_RENAMED_PROPERTIES,
        DB_INSTANCE_PROPERTIES_NOT_SENT,
        RDS_DB_INSTANCE,
    )?;
    parameters.push((
        "DBInstanceIdentifier".to_string(),
        resource.physical_resource_id.clone(),
    ));
    push_tag_list(
        &mut parameters,
        "Tags",
        "Tag",
        &tags_of(&resource.properties, propagated),
    );
    let answered = call_query(
        ctx,
        RDS_SIGNING_NAME,
        region,
        RDS_API_VERSION,
        "CreateDBInstance",
        &parameters,
    )
    .await?;

    let mut attributes = BTreeMap::new();
    if let Some(arn) = xml_element(&answered, "DBInstanceArn").filter(|arn| !arn.is_empty()) {
        // The type publishes one ARN, so the conventional `Arn` attribute every type
        // answers is that same value ([`super::template::conventional_attributes`]): both
        // are re-pointed at the ARN the instance really carries.
        attributes.insert("Arn".to_string(), arn.clone());
        attributes.insert("DBInstanceArn".to_string(), arn);
    }
    if let Some(resource_id) =
        xml_element(&answered, "DbiResourceId").filter(|id| !id.is_empty())
    {
        attributes.insert("DbiResourceId".to_string(), resource_id);
    }
    // `Endpoint` is a structure (`<Endpoint><Address/><Port/>…`), and the two members the
    // type publishes are read off it by name.
    let endpoint = xml_scalar_children(&answered, "Endpoint");
    for member in ["Address", "Port"] {
        if let Some(value) = endpoint.get(member).and_then(scalar_text) {
            attributes.insert(format!("Endpoint.{member}"), value);
        }
    }
    Ok(Created::attributes(attributes))
}

/// Delete the DB instance a stack created, addressed by its identifier — this resource's
/// physical id. An instance Amazon RDS no longer has (`DBInstanceNotFound`) is already in
/// the state the delete asks for.
///
/// `DeleteDBInstance` requires the caller to say what becomes of the data: "you must
/// specify the `FinalDBSnapshotIdentifier` parameter" unless the snapshot is skipped. This
/// emulator's CloudFormation implements no `DeletionPolicy` — nothing in this crate reads
/// that resource attribute, and a stack's deletion deletes every resource it created — so
/// the call is the form CloudFormation makes for the `Delete` policy, which skips the final
/// snapshot. The one member the type's own declaration carries for the delete,
/// `DeleteAutomatedBackups`, is read off the record and sent as declared.
///
/// Amazon RDS answers the delete by putting the instance in `deleting` and removing it a
/// moment later, so this arm then WAITS for it to be gone
/// ([`confirm_db_instance_deleted`]): CloudFormation deletes a stack's resources in
/// dependency order and waits for each deletion to complete before it deletes what that
/// resource sat in, and a DB subnet group whose instance is still there is still in use.
async fn delete_db_instance(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let mut parameters = vec![
        (
            "DBInstanceIdentifier".to_string(),
            resource.physical_resource_id.clone(),
        ),
        ("SkipFinalSnapshot".to_string(), "true".to_string()),
    ];
    if let Some(declared) = resource
        .properties
        .get("DeleteAutomatedBackups")
        .and_then(scalar_text)
    {
        parameters.push(("DeleteAutomatedBackups".to_string(), declared));
    }
    match call_query(
        ctx,
        RDS_SIGNING_NAME,
        region,
        RDS_API_VERSION,
        "DeleteDBInstance",
        &parameters,
    )
    .await
    {
        Ok(_) => {
            confirm_db_instance_deleted(ctx, region, &resource.physical_resource_id).await;
            Ok(())
        }
        Err(refusal) if refusal.code == DB_INSTANCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The workspace's configuration as Amazon Managed Grafana reports it, for drift
/// detection: `DescribeWorkspace`'s own record, read by the id that service minted.
///
/// A workspace it no longer holds (`ResourceNotFoundException`) was deleted. The record's
/// members are that service's own (lower camel, `workspace`-prefixed for several), so only
/// a declared property spelled identically on both sides takes part in the comparison —
/// the position the Amazon ECS and AWS FIS arms above are in too, and what [`super::drift`]
/// documents: a name mismatch reports the resource as existing rather than as modified.
async fn describe_workspace(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    match call_rest_json(
        ctx,
        GRAFANA_SIGNING_NAME,
        region,
        "GET",
        &format!("{GRAFANA_WORKSPACES_URI}/{physical_resource_id}"),
        None,
    )
    .await
    {
        Ok(answer) => Ok(answer.get("workspace").cloned()),
        Err(refusal) if refusal.code == GRAFANA_RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// How often this module checks whether an accepted DB instance deletion has finished, and
/// how many times.
///
/// The product is the longest a stack's deletion waits for ONE instance to go: an upper
/// bound on the wait, not a duration anything is owed. Amazon RDS reports a deleted
/// instance as `deleting` and removes it a moment later (its own transition span), so the
/// check is far shorter than that moment and the whole wait is a small multiple of it —
/// enough that a loaded machine still gets there, and bounded so a `DeleteStack` of several
/// instances cannot hold its request open.
const DELETION_CHECK_INTERVAL: ::std::time::Duration = ::std::time::Duration::from_millis(25);
const DELETION_CHECKS: usize = 24;

/// Wait until Amazon RDS no longer holds the instance whose deletion it has ACCEPTED.
///
/// Returns as soon as the instance is gone, and gives up after [`DELETION_CHECKS`] checks
/// — the delete itself succeeded, so a wait that runs out is not a failure of it; what
/// fails then is whatever the next delete of the same stack needs the instance gone for,
/// reported as that service's own refusal. Any other refusal of the READ ends the wait the
/// same way: it says nothing about whether the instance is there, and the delete was
/// accepted either way.
async fn confirm_db_instance_deleted(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) {
    for _ in 0..DELETION_CHECKS {
        match describe_db_instance(ctx, region, physical_resource_id).await {
            // The instance is gone: the deletion RDS accepted has finished.
            Ok(None) | Err(_) => return,
            Ok(Some(_)) => {}
        }
        ::tokio::time::sleep(DELETION_CHECK_INTERVAL).await;
    }
}

/// The DB instance's configuration as Amazon RDS reports it, for drift detection. An
/// instance it no longer has (`DBInstanceNotFound`) was deleted.
///
/// A DB instance is reported as a STRUCTURE whose scalar members are named exactly as this
/// type's properties are (`Engine`, `DBInstanceClass`, `AllocatedStorage`, `MultiAZ`), so
/// those members are the actual configuration the declared one is compared against. Its
/// composite members are not: `VpcSecurityGroups`, `DBParameterGroups` and `Subnets` are
/// reported as MEMBERSHIP structures carrying a status, where the properties are plain id
/// and name lists, so comparing either would need a mapping this module would have to
/// invent — and a property the actual document does not carry is not evidence of anything
/// ([`super::drift`]).
async fn describe_db_instance(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![(
        "DBInstanceIdentifier".to_string(),
        physical_resource_id.to_string(),
    )];
    match call_query(
        ctx,
        RDS_SIGNING_NAME,
        region,
        RDS_API_VERSION,
        "DescribeDBInstances",
        &parameters,
    )
    .await
    {
        Ok(answered) => Ok(Some(Value::Object(xml_scalar_children(
            &answered,
            "DBInstance",
        )))),
        Err(refusal) if refusal.code == DB_INSTANCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Create the DB subnet group an `AWS::RDS::DBSubnetGroup` declares.
///
/// The type's four properties are `CreateDBSubnetGroup`'s own members, named identically on
/// both sides, with the name CloudFormation decides and the merged tag list pushed by this
/// arm the way the DB instance's are. The group is what a DB instance's
/// `DBSubnetGroupName` names, and Amazon RDS resolves that name before it will create an
/// instance (`DBSubnetGroupNotFoundFault`), so a stack that declares both has to create
/// this one for real too.
async fn create_db_subnet_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut parameters = query_members(
        &resource.properties,
        CREATE_DB_SUBNET_GROUP_MEMBERS,
        &[],
        DB_SUBNET_GROUP_PROPERTIES_NOT_SENT,
        RDS_DB_SUBNET_GROUP,
    )?;
    parameters.push((
        "DBSubnetGroupName".to_string(),
        resource.physical_resource_id.clone(),
    ));
    push_tag_list(
        &mut parameters,
        "Tags",
        "Tag",
        &tags_of(&resource.properties, propagated),
    );
    let answered = call_query(
        ctx,
        RDS_SIGNING_NAME,
        region,
        RDS_API_VERSION,
        "CreateDBSubnetGroup",
        &parameters,
    )
    .await?;
    let mut attributes = BTreeMap::new();
    if let Some(arn) = xml_element(&answered, "DBSubnetGroupArn").filter(|arn| !arn.is_empty())
    {
        // The group's own ARN, which is the resource's: the conventional `Arn` attribute
        // the type answers is re-pointed at the one Amazon RDS reports.
        attributes.insert("Arn".to_string(), arn);
    }
    Ok(Created::attributes(attributes))
}

/// Delete the DB subnet group a stack created, addressed by its name — this resource's
/// physical id. A group Amazon RDS no longer has (`DBSubnetGroupNotFoundFault`) is already
/// in the state the delete asks for.
async fn delete_db_subnet_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let parameters = vec![(
        "DBSubnetGroupName".to_string(),
        physical_resource_id.to_string(),
    )];
    match call_query(
        ctx,
        RDS_SIGNING_NAME,
        region,
        RDS_API_VERSION,
        "DeleteDBSubnetGroup",
        &parameters,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == DB_SUBNET_GROUP_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The DB subnet group's configuration as Amazon RDS reports it, for drift detection. A
/// group it no longer has (`DBSubnetGroupNotFoundFault`) was deleted.
///
/// Its scalar members are read the same way the DB instance's are, which covers the one
/// property a declaration can drift in under the same name
/// (`DBSubnetGroupDescription`); `SubnetIds` is reported as the `Subnets` structure list,
/// which is not that property's form, so it is not compared.
async fn describe_db_subnet_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![(
        "DBSubnetGroupName".to_string(),
        physical_resource_id.to_string(),
    )];
    match call_query(
        ctx,
        RDS_SIGNING_NAME,
        region,
        RDS_API_VERSION,
        "DescribeDBSubnetGroups",
        &parameters,
    )
    .await
    {
        Ok(answered) => Ok(Some(Value::Object(xml_scalar_children(
            &answered,
            "DBSubnetGroup",
        )))),
        Err(refusal) if refusal.code == DB_SUBNET_GROUP_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Create the parameter group a declaration of either RDS parameter-group type asks for.
///
/// The type's properties are its create's own members ([`ParameterGroupCalls`]) but for the
/// one Amazon RDS spells differently ([`PARAMETER_GROUP_RENAMED_PROPERTIES`]), with the name
/// CloudFormation decides — what this type's `Ref` returns, this resource's physical id —
/// and the merged tag list pushed by this arm the way the two types above push theirs.
///
/// The declared `Parameters` are a SECOND call ([`modify_parameters`]), the same pair a
/// client makes: a group is created carrying none of its own parameters, and the values a
/// template declares are what the describe of the group's parameters then reports for it.
async fn create_parameter_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
    calls: ParameterGroupCalls,
) -> Result<Created, Refusal> {
    let mut parameters = query_members(
        &resource.properties,
        calls.create_members,
        PARAMETER_GROUP_RENAMED_PROPERTIES,
        &calls.properties_not_sent(),
        calls.resource_type,
    )?;
    parameters.push((
        calls.name_member.to_string(),
        resource.physical_resource_id.clone(),
    ));
    push_tag_list(
        &mut parameters,
        "Tags",
        "Tag",
        &tags_of(&resource.properties, propagated),
    );
    let answered = call_query(
        ctx,
        RDS_SIGNING_NAME,
        region,
        RDS_API_VERSION,
        calls.create,
        &parameters,
    )
    .await?;
    modify_parameters(
        ctx,
        region,
        &resource.physical_resource_id,
        &resource.properties,
        calls,
    )
    .await?;

    let mut attributes = BTreeMap::new();
    if let Some(arn) =
        xml_element(&answered, &format!("{}Arn", calls.record)).filter(|arn| !arn.is_empty())
    {
        // The group's own ARN, which is the resource's: the conventional `Arn` attribute the
        // type answers is re-pointed at the one Amazon RDS reports.
        attributes.insert("Arn".to_string(), arn);
    }
    Ok(Created::attributes(attributes))
}

/// Write the parameter settings a declaration carries into the group named
/// `physical_resource_id`, through the modify a client makes for them.
///
/// Either type's `Parameters` is a MAP of parameter name to value, where the modify takes
/// the `Parameter` STRUCTURE list its `ParametersList` spells, so each entry is written as
/// one element of that list. A declaration carrying more than one request admits
/// ([`PARAMETERS_PER_MODIFY`]) is sent as several calls, since one request carrying them all
/// is a request Amazon RDS refuses; a declaration carrying none makes no call at all.
///
/// How many calls that is is bounded by the declared map, which is bounded by the template
/// body the stack was created from, and Amazon RDS's own cap on how many parameters one
/// group carries refuses the rest — so neither this loop nor the group it writes grows with
/// anything a caller can choose freely.
async fn modify_parameters(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
    properties: &Value,
    calls: ParameterGroupCalls,
) -> Result<(), Refusal> {
    let Some(declared) = properties.get("Parameters").and_then(Value::as_object) else {
        return Ok(());
    };
    let settings: Vec<(&String, &Value)> = declared.iter().collect();
    for batch in settings.chunks(PARAMETERS_PER_MODIFY) {
        let mut parameters = vec![(
            calls.name_member.to_string(),
            physical_resource_id.to_string(),
        )];
        for (index, (name, value)) in batch.iter().enumerate() {
            let position = index + 1;
            parameters.push((
                format!("Parameters.Parameter.{position}.ParameterName"),
                (*name).clone(),
            ));
            parameters.push((
                format!("Parameters.Parameter.{position}.ParameterValue"),
                attribute_text(value),
            ));
        }
        call_query(
            ctx,
            RDS_SIGNING_NAME,
            region,
            RDS_API_VERSION,
            calls.modify,
            &parameters,
        )
        .await?;
    }
    Ok(())
}

/// Delete the parameter group a stack created, addressed by its name — this resource's
/// physical id. A group Amazon RDS no longer has (`DBParameterGroupNotFound`) is already in
/// the state the delete asks for.
///
/// Amazon RDS refuses the delete of a DB parameter group an instance is still a member of,
/// so that deletion comes first: CloudFormation deletes a stack's resources in dependency
/// order and waits for each deletion to finish, which is what
/// [`confirm_db_instance_deleted`] is for.
async fn delete_parameter_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
    calls: ParameterGroupCalls,
) -> Result<(), Refusal> {
    let parameters = vec![(
        calls.name_member.to_string(),
        physical_resource_id.to_string(),
    )];
    match call_query(
        ctx,
        RDS_SIGNING_NAME,
        region,
        RDS_API_VERSION,
        calls.delete,
        &parameters,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == DB_PARAMETER_GROUP_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The parameter group's configuration as Amazon RDS reports it, for drift detection. A
/// group it no longer has (`DBParameterGroupNotFound`) was deleted.
///
/// Its scalar members are read the way the two types above read theirs, which covers the one
/// property a declaration can drift in under the same name (`Description`); `Family` is
/// reported under the API's own `DBParameterGroupFamily` and `Parameters` is not a member of
/// this record at all, so neither takes part in the comparison — the position every arm
/// whose property is spelled differently from its member is in, and what [`super::drift`]
/// documents.
async fn describe_parameter_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
    calls: ParameterGroupCalls,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![(
        calls.name_member.to_string(),
        physical_resource_id.to_string(),
    )];
    match call_query(
        ctx,
        RDS_SIGNING_NAME,
        region,
        RDS_API_VERSION,
        calls.describe,
        &parameters,
    )
    .await
    {
        Ok(answered) => Ok(Some(Value::Object(xml_scalar_children(
            &answered,
            calls.record,
        )))),
        Err(refusal) if refusal.code == DB_PARAMETER_GROUP_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The awsQuery parameters an evaluated property document is, against the member catalogue
/// of the create that takes it.
///
/// `renamed` is the properties whose member is spelled differently, and `not_sent` the ones
/// the ARM itself pushes or that belong to another call. Every other property has to be a
/// member of the catalogue: one that is not is REFUSED, because a resource created without
/// a property its declaration asked for is configured differently from what the template
/// declared — and the stack would have reported it as created.
fn query_members(
    properties: &Value,
    members: &[(&str, QueryMember)],
    renamed: &[(&str, &str)],
    not_sent: &[&str],
    resource_type: &str,
) -> Result<Vec<(String, String)>, Refusal> {
    let mut parameters: Vec<(String, String)> = Vec::new();
    let Some(properties) = properties.as_object() else {
        return Ok(parameters);
    };
    for (property, value) in properties {
        if not_sent.contains(&property.as_str()) {
            continue;
        }
        let member = renamed
            .iter()
            .find(|(declared, _)| *declared == property.as_str())
            .map_or(property.as_str(), |(_, member)| *member);
        let Some((_, shape)) = members.iter().find(|(name, _)| *name == member) else {
            return Err(Refusal {
                code: NOT_IMPLEMENTED.to_string(),
                message: format!(
                    "this emulator does not send {resource_type}'s {property} property to \
                     the service that owns the type, so the resource was not created"
                ),
            });
        };
        match shape {
            QueryMember::Scalar => {
                parameters.push((member.to_string(), attribute_text(value)));
            }
            QueryMember::List(element) => push_list(&mut parameters, member, element, value),
        }
    }
    Ok(parameters)
}

/// Write one LIST member into an awsQuery form body: each entry indexed from 1 under
/// `element`, with a structured entry's own members following the index.
///
/// A value the template did not write as a list is the one-entry list of itself, so a
/// single value a template spells without the sequence syntax is not dropped.
fn push_list(
    parameters: &mut Vec<(String, String)>,
    member: &str,
    element: &str,
    declared: &Value,
) {
    let entries: Vec<&Value> = match declared {
        Value::Array(values) => values.iter().collect(),
        single => vec![single],
    };
    for (index, value) in entries.iter().enumerate() {
        let position = index + 1;
        match value {
            Value::Object(entry) => {
                for (name, inner) in entry {
                    parameters.push((
                        format!("{member}.{element}.{position}.{name}"),
                        attribute_text(inner),
                    ));
                }
            }
            scalar => parameters.push((
                format!("{member}.{element}.{position}"),
                attribute_text(scalar),
            )),
        }
    }
}

/// Write a tag set as the `Tag` structure list an awsQuery create takes, with the
/// `Key`/`Value` members Amazon RDS's own `Tag` shape spells.
fn push_tag_list(
    parameters: &mut Vec<(String, String)>,
    member: &str,
    element: &str,
    tags: &[(String, String)],
) {
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        parameters.push((format!("{member}.{element}.{position}.Key"), key.clone()));
        parameters.push((
            format!("{member}.{element}.{position}.Value"),
            value.clone(),
        ));
    }
}

// ---------------------------------------------------------------------------
// Amazon EventBridge: AWS::Events::Rule
// ---------------------------------------------------------------------------

/// Amazon EventBridge's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: events`, service shape `AWSEvents`), which speaks `awsJson1_1`.
const EVENTS_SIGNING_NAME: &str = "events";
const EVENTS_TARGET_PREFIX: &str = "AWSEvents";

/// The error code CloudFormation's own resource handlers answer for a create whose
/// identifier is already in use (`AlreadyExists`), for a type whose owning API takes the
/// create as an UPSERT and so refuses nothing itself.
const ALREADY_EXISTS: &str = "AlreadyExists";

/// The `AWS::Events::Rule` properties `PutRule` takes under the SAME member name.
///
/// Source: AWS's CloudFormation resource reference for the type against
/// `smithy_query(operation="PutRule")` — `Description`, `EventBusName`, `RoleArn`,
/// `ScheduleExpression` and `State` are spelled identically on both sides. The three
/// properties NOT here are the three that are not passed straight through: `Name`, which
/// CloudFormation decides and which is this resource's physical id, `EventPattern`, which
/// the type declares as a template DOCUMENT while `PutRule` takes the JSON TEXT of one,
/// and `Targets`, which is not a `PutRule` member at all — a rule's targets are installed
/// by `PutTargets`, the same second call a client makes.
const EVENTS_RULE_MEMBERS: &[&str] = &[
    "Description",
    "EventBusName",
    "RoleArn",
    "ScheduleExpression",
    "State",
];

/// The `Target` property type's members, named identically to `Target`'s own
/// (`smithy_query(shape="Target")`): a target's id, the ARN it invokes, the role it
/// assumes, the input it carries and the per-service parameter structures. The one member
/// absent here is `EcsParameters`, whose two renamed members [`ecs_parameters`] maps.
const EVENTS_TARGET_MEMBERS: &[&str] = &[
    "AppSyncParameters",
    "Arn",
    "BatchParameters",
    "DeadLetterConfig",
    "HttpParameters",
    "Id",
    "Input",
    "InputPath",
    "InputTransformer",
    "KinesisParameters",
    "RedshiftDataParameters",
    "RetryPolicy",
    "RoleArn",
    "RunCommandParameters",
    "SageMakerPipelineParameters",
    "SqsParameters",
];

/// The `EcsParameters` members named identically on both sides
/// (`smithy_query(shape="EcsParameters")`). The two that are NOT are mapped by
/// [`ecs_parameters`].
const EVENTS_ECS_PARAMETER_MEMBERS: &[&str] = &[
    "CapacityProviderStrategy",
    "EnableECSManagedTags",
    "EnableExecuteCommand",
    "Group",
    "LaunchType",
    "PlacementConstraints",
    "PlacementStrategy",
    "PlatformVersion",
    "PropagateTags",
    "ReferenceId",
    "TaskCount",
    "TaskDefinitionArn",
];

/// Create — or update — the rule an `AWS::Events::Rule` declares, so a client reading the
/// stack's outputs and then calling `events DescribeRule` with that name finds the rule
/// there rather than a `ResourceNotFoundException`.
///
/// `Ref` of this type returns the rule NAME, which is this resource's physical id, so the
/// rule is created under exactly the name the stack reports for it; the ARN `PutRule`
/// answers with is the resource's real identity and replaces the computed one.
///
/// The declared `Targets` are installed by a `PutTargets` on the created rule — the same
/// pair of calls a client makes, since `PutRule` takes no targets.
///
/// `PutRule` is an UPSERT ("creates or updates the specified rule"), so the name is probed
/// first: a rule the bus ALREADY holds is not this stack's to reconfigure, and a create
/// that put the declaration over it would report a resource the stack took over rather than
/// created. That is the refusal CloudFormation itself answers for a resource whose
/// identifier is already in use, and it is the same thing the Systems Manager arm gets for
/// free from `PutParameter` refusing a put without `Overwrite`.
///
/// A rule whose TARGETS could not be installed is not a created resource either, so the
/// rule this call just made is removed before that refusal is reported: nothing is left
/// behind under a name the stack does not record.
async fn put_rule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    if describe_rule(ctx, region, resource).await?.is_some() {
        return Err(Refusal {
            code: ALREADY_EXISTS.to_string(),
            message: format!(
                "Resource of type '{}' with identifier '{}' already exists.",
                resource.resource_type, resource.physical_resource_id
            ),
        });
    }
    let reported = put_rule_configuration(
        ctx,
        region,
        &resource.physical_resource_id,
        &resource.properties,
    )
    .await?;
    if let Err(refusal) = install_targets(
        ctx,
        region,
        &resource.physical_resource_id,
        &resource.properties,
    )
    .await
    {
        // Best effort: the refusal that stopped the create is what the stack reports, so a
        // failure to undo it must not displace that reason.
        let _ = delete_rule(ctx, region, resource).await;
        return Err(refusal);
    }
    Ok(reported)
}

/// Apply a CHANGED `AWS::Events::Rule` declaration to the rule the stack already has.
///
/// `PutRule` is "Creates or updates the specified rule", so the declared configuration is
/// put under the same name, and the declared target list is reconciled against the one the
/// rule carries: a target the declaration no longer names is removed, and the ones it
/// names are put (`PutTargets` "adds the specified targets to the specified rule, or
/// updates the targets if they are already associated with the rule").
///
/// A declaration that moves the rule to a DIFFERENT event bus is not an in-place change:
/// a rule belongs to one bus, so nothing here can carry it across and the deployment
/// reports the change as not applied (`Ok(None)`) rather than creating a second rule and
/// leaving the first behind.
async fn update_rule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    if declared_bus(declared) != declared_bus(&resource.properties) {
        return Ok(None);
    }
    let name = &resource.physical_resource_id;
    let reported = put_rule_configuration(ctx, region, name, declared).await?;
    let installed = installed_target_ids(ctx, region, name, declared).await?;
    let retained: Vec<String> = declared_targets(declared)
        .iter()
        .filter_map(|target| target.get("Id").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect();
    let removed: Vec<String> = installed
        .into_iter()
        .filter(|id| !retained.contains(id))
        .collect();
    remove_targets(ctx, region, name, declared, &removed).await?;
    install_targets(ctx, region, name, declared).await?;
    Ok(Some(reported))
}

/// `PutRule` with `properties` as the rule's configuration and `name` as the one member
/// CloudFormation decides — the call behind both this type's create and its update, so the
/// two cannot send a different request for the same declaration.
///
/// No tag is sent: the type publishes no `Tags` property, so CloudFormation propagates
/// none to a rule, exactly as it propagates none to an `AWS::WAFRegional::IPSet`.
async fn put_rule_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    properties: &Value,
) -> Result<Created, Refusal> {
    let mut request = Map::new();
    if let Some(declared) = properties.as_object() {
        for member in EVENTS_RULE_MEMBERS {
            if let Some(value) = declared.get(*member) {
                request.insert((*member).to_string(), value.clone());
            }
        }
    }
    // The type declares `EventPattern` as a document ("Json"), while `PutRule` takes the
    // JSON text of one — the same spelling rule a policy document reaches IAM under.
    if let Some(pattern) = properties.get("EventPattern").and_then(document_text) {
        request.insert("EventPattern".to_string(), Value::String(pattern));
    }
    request.insert("Name".to_string(), Value::String(name.to_string()));
    let reported = call_events(ctx, region, "PutRule", &Value::Object(request)).await?;
    let mut attributes = BTreeMap::new();
    if let Some(arn) = reported.get("RuleArn").and_then(Value::as_str) {
        attributes.insert("Arn".to_string(), arn.to_string());
    }
    Ok(Created::attributes(attributes))
}

/// Install the targets a declaration names on the rule `name`, through EventBridge's own
/// `PutTargets`. A declaration naming none asks for no call at all, since `Targets` is a
/// `@length(min: 1)` list of that request.
///
/// `PutTargets` reports per-target failures in its answer rather than refusing the call,
/// so a reported failed entry is read as the refusal it is: the resource failed, and the
/// stack carries the code and message EventBridge gave for that target.
async fn install_targets(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    properties: &Value,
) -> Result<(), Refusal> {
    let declared = declared_targets(properties);
    if declared.is_empty() {
        return Ok(());
    }
    let targets: Vec<Value> = declared.iter().map(event_target).collect();
    let mut request = Map::new();
    request.insert("Rule".to_string(), Value::String(name.to_string()));
    request.insert("Targets".to_string(), Value::Array(targets));
    if let Some(bus) = declared_bus(properties) {
        request.insert("EventBusName".to_string(), Value::String(bus));
    }
    let reported = call_events(ctx, region, "PutTargets", &Value::Object(request)).await?;
    match first_listed(&reported, "FailedEntries") {
        Some(failed) => Err(Refusal {
            code: failed
                .get("ErrorCode")
                .and_then(Value::as_str)
                .unwrap_or(UNIDENTIFIED_FAILURE)
                .to_string(),
            message: failed
                .get("ErrorMessage")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }),
        None => Ok(()),
    }
}

/// Remove the targets `ids` names from the rule `name`. An empty set asks for no call:
/// `RemoveTargets$Ids` is a `@length(min: 1)` list.
async fn remove_targets(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    properties: &Value,
    ids: &[String],
) -> Result<(), Refusal> {
    if ids.is_empty() {
        return Ok(());
    }
    let mut request = Map::new();
    request.insert("Rule".to_string(), Value::String(name.to_string()));
    request.insert(
        "Ids".to_string(),
        Value::Array(ids.iter().map(|id| Value::String(id.clone())).collect()),
    );
    if let Some(bus) = declared_bus(properties) {
        request.insert("EventBusName".to_string(), Value::String(bus));
    }
    let reported = call_events(ctx, region, "RemoveTargets", &Value::Object(request)).await?;
    match first_listed(&reported, "FailedEntries") {
        Some(failed) => Err(Refusal {
            code: failed
                .get("ErrorCode")
                .and_then(Value::as_str)
                .unwrap_or(UNIDENTIFIED_FAILURE)
                .to_string(),
            message: failed
                .get("ErrorMessage")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }),
        None => Ok(()),
    }
}

/// The ids of the targets the rule `name` CARRIES, as EventBridge reports them.
///
/// One page is the whole set: a rule holds at most as many targets as `Target`'s own list
/// bound admits, which is inside `ListTargetsByRule`'s maximum page.
async fn installed_target_ids(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    properties: &Value,
) -> Result<Vec<String>, Refusal> {
    let mut request = Map::new();
    request.insert("Rule".to_string(), Value::String(name.to_string()));
    if let Some(bus) = declared_bus(properties) {
        request.insert("EventBusName".to_string(), Value::String(bus));
    }
    let reported = call_events(ctx, region, "ListTargetsByRule", &Value::Object(request)).await?;
    Ok(reported
        .get("Targets")
        .and_then(Value::as_array)
        .map(|targets| {
            targets
                .iter()
                .filter_map(|target| target.get("Id").and_then(Value::as_str))
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default())
}

/// Delete the rule a stack created: its targets first, because "before you can delete the
/// rule, you must remove all associated targets" (`DeleteRule`), then the rule itself.
///
/// A rule EventBridge no longer holds is already in the state the delete asks for: the
/// read that lists its targets answers `ResourceNotFoundException` for it, which is the
/// one code that says the rule is gone, and `DeleteRule` is itself idempotent ("if you
/// call delete rule multiple times for the same rule, all calls will succeed").
async fn delete_rule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let name = &resource.physical_resource_id;
    let installed = match installed_target_ids(ctx, region, name, &resource.properties).await {
        Ok(installed) => installed,
        // Judged on the error CODE, never on the message, which carries the rule's name.
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => return Ok(()),
        Err(refusal) => return Err(refusal),
    };
    remove_targets(ctx, region, name, &resource.properties, &installed).await?;
    let mut request = Map::new();
    request.insert("Name".to_string(), Value::String(name.to_string()));
    if let Some(bus) = declared_bus(&resource.properties) {
        request.insert("EventBusName".to_string(), Value::String(bus));
    }
    match call_events(ctx, region, "DeleteRule", &Value::Object(request)).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The rule's configuration as EventBridge reports it, for drift detection. A rule it no
/// longer holds (`ResourceNotFoundException`) was deleted.
///
/// `DescribeRule` answers the rule's members under the very names the type declares them
/// with (`Description`, `EventBusName`, `EventPattern`, `ScheduleExpression`, `State`,
/// `RoleArn`), so its answer IS the actual configuration document; the declared
/// `EventPattern` document is compared against the text EventBridge holds by
/// [`super::drift`]'s own rule for a composite a service reports as a string.
async fn describe_rule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let mut request = Map::new();
    request.insert(
        "Name".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    if let Some(bus) = declared_bus(&resource.properties) {
        request.insert("EventBusName".to_string(), Value::String(bus));
    }
    match call_events(ctx, region, "DescribeRule", &Value::Object(request)).await {
        Ok(answer) => Ok(Some(answer)),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The bus a declaration binds its rule to, or `None` for one that names none — which is
/// the account's `default` bus, the member's own documented default, so nothing is sent
/// for it.
fn declared_bus(properties: &Value) -> Option<String> {
    properties
        .get("EventBusName")
        .and_then(scalar_text)
        .filter(|bus| !bus.is_empty())
}

/// The `Targets` a declaration names, in the order it writes them.
fn declared_targets(properties: &Value) -> Vec<Value> {
    properties
        .get("Targets")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// One declared `Target` as `PutTargets` takes it: the members named identically on both
/// sides ([`EVENTS_TARGET_MEMBERS`]) passed through as the template wrote them, plus
/// `EcsParameters` with its two renamed members mapped ([`ecs_parameters`]).
fn event_target(declared: &Value) -> Value {
    let mut target = Map::new();
    if let Some(members) = declared.as_object() {
        for member in EVENTS_TARGET_MEMBERS {
            if let Some(value) = members.get(*member) {
                target.insert((*member).to_string(), value.clone());
            }
        }
        if let Some(ecs) = members.get("EcsParameters") {
            target.insert("EcsParameters".to_string(), ecs_parameters(ecs));
        }
    }
    Value::Object(target)
}

/// A declared `EcsParameters` as `Target$EcsParameters` takes it.
///
/// Its members are named identically on both sides ([`EVENTS_ECS_PARAMETER_MEMBERS`])
/// except two: the task's tag list, which the property type publishes as `TagList` while
/// the shape's member is `Tags`, and the awsvpc configuration, which the property type
/// publishes as `NetworkConfiguration.AwsVpcConfiguration` while the
/// `NetworkConfiguration` shape's one member is `awsvpcConfiguration`
/// (`smithy_query(shape="EcsParameters,NetworkConfiguration")`).
fn ecs_parameters(declared: &Value) -> Value {
    let mut mapped = Map::new();
    if let Some(members) = declared.as_object() {
        for member in EVENTS_ECS_PARAMETER_MEMBERS {
            if let Some(value) = members.get(*member) {
                mapped.insert((*member).to_string(), value.clone());
            }
        }
        if let Some(tags) = members.get("TagList") {
            mapped.insert("Tags".to_string(), tags.clone());
        }
        if let Some(awsvpc) = members
            .get("NetworkConfiguration")
            .and_then(|network| network.get("AwsVpcConfiguration"))
        {
            let mut network = Map::new();
            network.insert("awsvpcConfiguration".to_string(), awsvpc.clone());
            mapped.insert("NetworkConfiguration".to_string(), Value::Object(network));
        }
    }
    Value::Object(mapped)
}

/// Call one Amazon EventBridge operation on the awsJson1_1 wire its model declares.
async fn call_events(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        EVENTS_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{EVENTS_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

// ---------------------------------------------------------------------------
// AWS Systems Manager: AWS::SSM::Parameter
// ---------------------------------------------------------------------------

/// AWS Systems Manager's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: ssm`, service shape `AmazonSSM`), which speaks `awsJson1_1`.
const SSM_SIGNING_NAME: &str = "ssm";
const SSM_TARGET_PREFIX: &str = "AmazonSSM";

/// The error code Systems Manager answers for a parameter it does not hold
/// (`ParameterNotFound`: "The parameter couldn't be found. Verify the name and try
/// again").
const PARAMETER_NOT_FOUND: &str = "ParameterNotFound";

/// The `AWS::SSM::Parameter` properties `PutParameter` takes under the SAME member name.
///
/// Source: AWS's CloudFormation resource reference for the type against
/// `smithy_query(operation="PutParameter")` — every property of the type is a member of
/// that request, spelled identically. The two not here are the two that are not passed
/// straight through: `Name`, which CloudFormation decides and which is this resource's
/// physical id, and `Tags`, which the type publishes as a MAP while the request takes the
/// `[{Key, Value}]` list its own `Tag` shape spells.
const SSM_PARAMETER_MEMBERS: &[&str] = &[
    "AllowedPattern",
    "DataType",
    "Description",
    "Policies",
    "Tier",
    "Type",
    "Value",
];

/// Create the parameter an `AWS::SSM::Parameter` declares, so a client that deployed the
/// stack and then calls `ssm GetParameter` with that name reads the value back rather than
/// a `ParameterNotFound`.
///
/// `Ref` of this type returns the parameter NAME, which is this resource's physical id, so
/// the parameter is created under exactly the name the stack reports for it. Nothing
/// `PutParameter` answers (the version it created, the tier it resolved) is a published
/// return value of the type, so no attribute is re-reported off it: the type's `Type` and
/// `Value` are the declaration's own.
///
/// The parameter carries the tags the declaration asks for plus the ones CloudFormation
/// gives the resource ([`tags_of`]), as the list Systems Manager's own `Tag` shape spells.
async fn put_parameter(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = parameter_request(&resource.properties, &resource.physical_resource_id);
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("Tags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
    call_ssm(ctx, region, "PutParameter", &Value::Object(request)).await?;
    Ok(Created::default())
}

/// Apply a CHANGED `AWS::SSM::Parameter` declaration to the parameter the stack already
/// has, through the `Overwrite` half of the one call that "create[s] or update[s] a
/// parameter in Parameter Store".
///
/// No tag is sent with the overwrite: "to add tags to an existing Systems Manager
/// parameter, use the AddTagsToResource operation" (`PutParameter$Tags`), so the set the
/// parameter carries is left exactly as the create left it rather than replaced.
async fn update_parameter(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    let mut request = parameter_request(declared, &resource.physical_resource_id);
    request.insert("Overwrite".to_string(), Value::Bool(true));
    call_ssm(ctx, region, "PutParameter", &Value::Object(request)).await?;
    Ok(Created::default())
}

/// Delete the parameter a stack created, addressed by its name — this resource's physical
/// id, which is what `Ref` returns for the type. A parameter Systems Manager no longer
/// holds (`ParameterNotFound`) is already in the state the delete asks for.
async fn delete_parameter(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "Name": physical_resource_id });
    match call_ssm(ctx, region, "DeleteParameter", &request).await {
        Ok(_) => Ok(()),
        // Judged on the error CODE, never on the message, which carries the request's own
        // parameter name.
        Err(refusal) if refusal.code == PARAMETER_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The parameter's configuration as Systems Manager reports it, for drift detection: the
/// `Parameter` its own read answers with, whose `Type`, `Value` and `DataType` are the
/// properties the type declares under those very names. One it no longer holds
/// (`ParameterNotFound`) was deleted.
async fn describe_parameter(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "Name": physical_resource_id });
    match call_ssm(ctx, region, "GetParameter", &request).await {
        Ok(answer) => Ok(Some(reported_member(&answer, "Parameter"))),
        Err(refusal) if refusal.code == PARAMETER_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The `PutParameter` request one declaration describes, under the name CloudFormation
/// decided: the members named identically on both sides ([`SSM_PARAMETER_MEMBERS`]), each
/// as the text its API member takes.
fn parameter_request(properties: &Value, name: &str) -> Map<String, Value> {
    let mut request = Map::new();
    if let Some(declared) = properties.as_object() {
        for member in SSM_PARAMETER_MEMBERS {
            if let Some(value) = declared.get(*member) {
                request.insert((*member).to_string(), parameter_text(value));
            }
        }
    }
    request.insert("Name".to_string(), Value::String(name.to_string()));
    request
}

/// One declared parameter property as the TEXT its `PutParameter` member takes: every one
/// of them is a string member, and a template may write a value as a number, a boolean, or
/// — for the `Policies` array — as an inline document.
fn parameter_text(value: &Value) -> Value {
    match document_text(value).or_else(|| scalar_text(value)) {
        Some(text) => Value::String(text),
        None => value.clone(),
    }
}

/// Call one AWS Systems Manager operation on the awsJson1_1 wire its model declares.
async fn call_ssm(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        SSM_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{SSM_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

// ---------------------------------------------------------------------------
// AWS Step Functions: AWS::StepFunctions::Activity
// ---------------------------------------------------------------------------

/// AWS Step Functions' signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: states`, service shape `AWSStepFunctions`), which speaks `awsJson1_0`.
const SFN_SIGNING_NAME: &str = "states";
const SFN_TARGET_PREFIX: &str = "AWSStepFunctions";

/// The error code Step Functions answers for an activity ARN no activity answers to
/// (`ActivityDoesNotExist`: "The specified activity does not exist").
const ACTIVITY_DOES_NOT_EXIST: &str = "ActivityDoesNotExist";

/// Create the activity an `AWS::StepFunctions::Activity` declares, so a client that
/// deployed the stack and then calls `sfn DescribeActivity` with the ARN the stack
/// publishes reads the activity back rather than an `ActivityDoesNotExist`.
///
/// `Ref` of this type returns the activity's ARN, which is this resource's physical id, and
/// an activity ARN is a total function of the Region and the NAME — so the activity is
/// created under the name CloudFormation decided and the ARN Step Functions answers with is
/// the one the stack already computed. It is reported back as the resource's `Arn`
/// attribute all the same, so the handle the stack publishes is the one that service really
/// minted.
///
/// The type's two remaining properties are `CreateActivity`'s own members: the
/// `EncryptionConfiguration`, whose members Step Functions spells in lower camel case, and
/// the `Tags` list — carrying the tags the declaration asks for plus the ones
/// CloudFormation gives the resource ([`tags_of`]), as the list that model's own `Tag` shape
/// spells, so `sfn ListTagsForResource` on the ARN the stack publishes answers with the
/// stack's tags and the `aws:cloudformation:*` ones.
async fn create_activity(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = Map::new();
    request.insert(
        "name".to_string(),
        Value::String(activity_name(resource)),
    );
    if let Some(configuration) = resource.properties.get("EncryptionConfiguration") {
        request.insert(
            "encryptionConfiguration".to_string(),
            lower_camel_members(configuration, 0)?,
        );
    }
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("tags".to_string(), tags_as_list(&tags, LOWER_CASED_TAG));
    }
    let answered = call_sfn(ctx, region, "CreateActivity", &Value::Object(request)).await?;
    let arn = answered
        .get("activityArn")
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| Refusal {
            code: "InternalFailure".to_string(),
            message: "AWS Step Functions answered CreateActivity without an activityArn"
                .to_string(),
        })?;

    let mut attributes = BTreeMap::new();
    attributes.insert("Arn".to_string(), arn.to_string());
    Ok(Created::attributes(attributes))
}

/// Delete the activity a stack created, addressed by its ARN — this resource's physical id,
/// which is what `Ref` returns for the type.
///
/// `DeleteActivity` models no does-not-exist error at all: an activity Step Functions no
/// longer holds is already in the state the delete asks for, and the call says so itself.
async fn delete_activity(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "activityArn": physical_resource_id });
    call_sfn(ctx, region, "DeleteActivity", &request).await?;
    Ok(())
}

/// The activity's configuration as Step Functions reports it, for drift detection. One it
/// no longer holds (`ActivityDoesNotExist`, judged on the CODE and never on a message that
/// carries the request's own ARN) was deleted.
///
/// The one property its read reports is the activity's `Name`; an activity holds no other
/// configuration that `DescribeActivity` answers under a name this type declares — the
/// encryption settings it reports are spelled in that service's own member names, and a
/// property the read does not report is not evidence of anything ([`super::drift`]).
async fn describe_activity(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "activityArn": physical_resource_id });
    match call_sfn(ctx, region, "DescribeActivity", &request).await {
        Ok(answer) => Ok(Some(reported_as_properties(&answer, &[("Name", "name")]))),
        Err(refusal) if refusal.code == ACTIVITY_DOES_NOT_EXIST => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The NAME of the activity a resource record describes: the `Name` its type's published
/// return values carry, which is the name the template asked for or the
/// `<stack>-<logical id>-<suffix>` name CloudFormation generated — never the physical id,
/// which for this type is the activity's ARN.
///
/// A record with no such attribute (one whose attribute budget was spent) falls back to the
/// last segment of that ARN, which is the activity's name.
fn activity_name(resource: &StoredResource) -> String {
    if let Some(name) = resource.attributes.get("Name") {
        return name.clone();
    }
    resource
        .physical_resource_id
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Call one AWS Step Functions operation on the awsJson1_0 wire its model declares.
async fn call_sfn(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        SFN_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{SFN_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

// ---------------------------------------------------------------------------
// Amazon Cognito user pools: AWS::Cognito::UserPoolGroup
// ---------------------------------------------------------------------------

/// Amazon Cognito user pools' signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: cognito-idp`, service shape `AWSCognitoIdentityProviderService`), which
/// speaks `awsJson1_1`.
const COGNITO_IDP_SIGNING_NAME: &str = "cognito-idp";
const COGNITO_IDP_TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// The members a group's CONFIGURATION is, as `CreateGroup` and `UpdateGroup` both name
/// them — the type's properties beside the two that make the group's identity, spelled
/// identically on both sides.
const GROUP_CONFIGURATION_MEMBERS: &[&str] = &["Description", "Precedence", "RoleArn"];

/// Create the group an `AWS::Cognito::UserPoolGroup` declares, so a client that deployed
/// the stack and then calls `cognito-idp GetGroup` with the pool it declared and the name
/// the stack publishes reads the group back rather than a `ResourceNotFoundException`.
///
/// The type's properties ARE `CreateGroup`'s own members, so they are passed through with
/// the one member CloudFormation decides — the group's NAME, which is this resource's
/// physical id — set to it. A group is addressed by the pair `(UserPoolId, GroupName)`, and
/// the pool half is a required property of the type: a declaration carrying none describes
/// no group that could be created, so it is refused here rather than with a request Amazon
/// Cognito would have to reject.
async fn create_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let mut request = group_configuration(&resource.properties);
    request.insert("UserPoolId".to_string(), Value::String(group_pool(resource)?));
    request.insert(
        "GroupName".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    call_cognito_idp(ctx, region, "CreateGroup", &Value::Object(request)).await?;
    // The type publishes no `Fn::GetAtt` return value of its own: what identifies the
    // group is the pair the stack already records (its physical id and its declared
    // `UserPoolId`), so there is no attribute to adopt from the answer.
    Ok(Created::attributes(BTreeMap::new()))
}

/// Apply a CHANGED `AWS::Cognito::UserPoolGroup` declaration to the group the stack already
/// has, through Amazon Cognito's own `UpdateGroup` — which "updates any of the properties
/// for precedence, IAM role, or description" of the named group.
///
/// `Ok(None)` is a change that call cannot make, so the deployment reports it as not
/// applied rather than as a success that dropped it:
///
/// * a declaration that moves the group to a DIFFERENT user pool — a group belongs to one
///   pool, and `UpdateGroup` takes the pool only to address the group;
/// * a declaration that no longer states a `Precedence` or a `RoleArn` the group was
///   created with: a member `UpdateGroup` leaves out keeps its stored value, and neither
///   can be sent empty (`RoleArn` is `@length(min: 20)`, `Precedence` an integer), so
///   nothing in that call unsets them. A dropped `Description` IS sent, as the empty string
///   its own `@length(min: 0)` admits.
async fn update_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    let pool = group_pool(resource)?;
    if declared.get("UserPoolId").and_then(scalar_text) != Some(pool.clone()) {
        return Ok(None);
    }
    let mut request = group_configuration(declared);
    for member in GROUP_CONFIGURATION_MEMBERS {
        if request.contains_key(*member) || resource.properties.get(*member).is_none() {
            continue;
        }
        // A member the declaration DROPPED. Only a description can be emptied through
        // this call.
        if *member == "Description" {
            request.insert((*member).to_string(), Value::String(String::new()));
        } else {
            return Ok(None);
        }
    }
    request.insert("UserPoolId".to_string(), Value::String(pool));
    request.insert(
        "GroupName".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    call_cognito_idp(ctx, region, "UpdateGroup", &Value::Object(request)).await?;
    Ok(Some(Created::attributes(BTreeMap::new())))
}

/// Delete the group a stack created, addressed by the pair that identifies it: the pool its
/// declaration names and its name, which is this resource's physical id.
///
/// A group Amazon Cognito no longer holds — and a pool it no longer holds, which cannot
/// hold the group either — is already in the state the delete asks for; both are that
/// service's `ResourceNotFoundException`, judged on the CODE and never on a message.
async fn delete_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let request = serde_json::json!({
        "UserPoolId": group_pool(resource)?,
        "GroupName": resource.physical_resource_id.clone(),
    });
    match call_cognito_idp(ctx, region, "DeleteGroup", &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The group's configuration as Amazon Cognito reports it, for drift detection. One it no
/// longer holds (`ResourceNotFoundException`) was deleted.
///
/// `GetGroup` answers a `GroupType` whose members carry the very names this type declares
/// its properties with, so the answer maps onto the property vocabulary one for one.
async fn describe_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({
        "UserPoolId": group_pool(resource)?,
        "GroupName": resource.physical_resource_id.clone(),
    });
    match call_cognito_idp(ctx, region, "GetGroup", &request).await {
        Ok(answer) => {
            let Some(group) = answer.get("Group") else {
                return Ok(None);
            };
            Ok(Some(reported_as_properties(
                group,
                &[
                    ("GroupName", "GroupName"),
                    ("UserPoolId", "UserPoolId"),
                    ("Description", "Description"),
                    ("Precedence", "Precedence"),
                    ("RoleArn", "RoleArn"),
                ],
            )))
        }
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The three configuration members a declaration states, as the request carries them. The
/// type and the API spell them identically, so each value goes through as the template wrote
/// it — except `Precedence`, which `PrecedenceType` models as an INTEGER while a template may
/// write it as a quoted YAML scalar or feed it through a `Ref` to a `String` parameter: it is
/// read through [`whole_number`], this module's one spelling rule for a property carrying a
/// whole number. A property the declaration leaves out is absent from the request; a
/// `Precedence` that is no whole number at all is left as written, so the owning service
/// refuses the value the template really declared rather than a silently changed one.
fn group_configuration(properties: &Value) -> Map<String, Value> {
    let mut request = Map::new();
    for member in GROUP_CONFIGURATION_MEMBERS {
        let Some(value) = properties.get(*member) else {
            continue;
        };
        let carried = match *member {
            "Precedence" => {
                whole_number(Some(value)).map_or_else(|| value.clone(), Value::from)
            }
            _ => value.clone(),
        };
        request.insert((*member).to_string(), carried);
    }
    request
}

/// The user pool a group's declaration names — the other half of the group's identity.
///
/// `UserPoolId` is a required property of the type, so a declaration carrying none is
/// refused here, naming that property, rather than sent as a request the owning service
/// would reject.
fn group_pool(resource: &StoredResource) -> Result<String, Refusal> {
    property(resource, "UserPoolId").ok_or_else(|| Refusal {
        code: MISSING_PROPERTY_CODE.to_string(),
        message: format!("{} declares no UserPoolId", resource.resource_type),
    })
}

/// Call one Amazon Cognito user-pools operation on the awsJson1_1 wire its model declares.
async fn call_cognito_idp(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        COGNITO_IDP_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{COGNITO_IDP_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

// ---------------------------------------------------------------------------
// Amazon S3 Vectors: AWS::S3Vectors::VectorBucket / ::Index
// ---------------------------------------------------------------------------

/// Amazon S3 Vectors' signing name, from its model (`sigv4Name: s3vectors`, service shape
/// `S3Vectors`), which speaks `restJson1` — so its calls ride on [`call_rest_json`], each
/// on the URI its own operation's `@http` binding declares.
const S3VECTORS_SIGNING_NAME: &str = "s3vectors";

/// The URIs the S3 Vectors operations a stack makes are bound to, as that model's `@http`
/// traits declare them (`POST /CreateVectorBucket` and so on: every operation of this
/// service is a POST to a URI named after it).
const S3VECTORS_CREATE_VECTOR_BUCKET_URI: &str = "/CreateVectorBucket";
const S3VECTORS_GET_VECTOR_BUCKET_URI: &str = "/GetVectorBucket";
const S3VECTORS_DELETE_VECTOR_BUCKET_URI: &str = "/DeleteVectorBucket";
const S3VECTORS_CREATE_INDEX_URI: &str = "/CreateIndex";
const S3VECTORS_GET_INDEX_URI: &str = "/GetIndex";
const S3VECTORS_DELETE_INDEX_URI: &str = "/DeleteIndex";

/// The error code S3 Vectors answers for a vector bucket or a vector index it does not
/// hold (`NotFoundException`, the modelled 404 of its reads and deletes).
const S3VECTORS_NOT_FOUND: &str = "NotFoundException";

/// The members of an `AWS::S3Vectors::Index`'s own declaration that are `CreateIndex`'s own
/// members under the same name with a lower-case initial, and whose values are scalars that
/// go through as the template spells them.
///
/// Source: `CreateIndex`'s input members in that service's model (`indexName`, `dataType`,
/// `dimension`, `distanceMetric`), against the upper-camel property names a CloudFormation
/// template spells. The parent vector bucket is NOT one of them: it is resolved from the
/// resource's own ARN ([`vector_index_identity`]).
const S3VECTORS_INDEX_SCALAR_MEMBERS: &[(&str, &str)] = &[
    ("DataType", "dataType"),
    ("Dimension", "dimension"),
    ("DistanceMetric", "distanceMetric"),
];

/// Create the vector bucket an `AWS::S3Vectors::VectorBucket` declares, so a client that
/// deployed the stack and then calls `s3vectors GetVectorBucket` with the ARN the stack
/// publishes reads the bucket back rather than a `NotFoundException`.
///
/// `Ref` of this type returns the bucket's ARN, which is this resource's physical id, and
/// such an ARN is a total function of the Region and the bucket's NAME — so the bucket is
/// created under the name CloudFormation decided ([`vector_bucket_name`]) and the ARN S3
/// Vectors answers with is the one the stack already computed. It is reported back as the
/// resource's `VectorBucketArn` attribute all the same, so the handle the stack publishes
/// is the one that service really minted.
///
/// The type's remaining properties are `CreateVectorBucket`'s own members: the
/// `EncryptionConfiguration`, whose members S3 Vectors spells in lower camel case, and the
/// tags — the ones the declaration asks for plus the ones CloudFormation gives the resource
/// ([`tags_of`]), as the MAP that request takes, so `s3vectors ListTagsForResource` on the
/// ARN the stack publishes answers with the stack's own tags.
async fn create_vector_bucket(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = Map::new();
    request.insert(
        "vectorBucketName".to_string(),
        Value::String(vector_bucket_name(resource)),
    );
    if let Some(configuration) = resource.properties.get("EncryptionConfiguration") {
        request.insert(
            "encryptionConfiguration".to_string(),
            lower_camel_members(configuration, 0)?,
        );
    }
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let answered = call_s3vectors(
        ctx,
        region,
        S3VECTORS_CREATE_VECTOR_BUCKET_URI,
        &Value::Object(request),
    )
    .await?;
    let arn = answered
        .get("vectorBucketArn")
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| Refusal {
            code: "InternalFailure".to_string(),
            message: "Amazon S3 Vectors answered CreateVectorBucket without a vectorBucketArn"
                .to_string(),
        })?;
    let mut attributes = BTreeMap::new();
    attributes.insert("VectorBucketArn".to_string(), arn.to_string());
    Ok(Created::attributes(attributes))
}

/// Delete the vector bucket a stack created, addressed by its ARN — this resource's
/// physical id, which is what `Ref` returns for the type. A bucket S3 Vectors no longer
/// holds (`NotFoundException`, judged on the CODE and never on a message that carries the
/// request's own name) is already in the state the delete asks for; one that still holds
/// vector indexes is NOT deleted (`ConflictException`), and that refusal is what the stack
/// reports.
async fn delete_vector_bucket(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "vectorBucketArn": physical_resource_id });
    match call_s3vectors(ctx, region, S3VECTORS_DELETE_VECTOR_BUCKET_URI, &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == S3VECTORS_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The vector bucket's configuration as Amazon S3 Vectors reports it, for drift detection:
/// `GetVectorBucket`'s own record, read by the ARN that is this resource's physical id. One
/// it no longer holds (`NotFoundException`) was deleted.
///
/// The one property its read reports under the name this type declares is the bucket's
/// `VectorBucketName`; the encryption settings it reports are spelled in that service's own
/// member names, and a property the read does not report is not evidence of anything
/// ([`super::drift`]).
async fn describe_vector_bucket(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "vectorBucketArn": physical_resource_id });
    match call_s3vectors(ctx, region, S3VECTORS_GET_VECTOR_BUCKET_URI, &request).await {
        Ok(answer) => Ok(answer.get("vectorBucket").map(|bucket| {
            reported_as_properties(bucket, &[("VectorBucketName", "vectorBucketName")])
        })),
        Err(refusal) if refusal.code == S3VECTORS_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Create the vector index an `AWS::S3Vectors::Index` declares, in the vector bucket its
/// own ARN names.
///
/// `Ref` of this type returns the index's ARN, which is this resource's physical id and
/// which carries both names S3 Vectors keys an index by
/// (`bucket/<bucket>/index/<index>`) — so [`vector_index_identity`] reads the pair back off
/// it and the index is created under exactly the names the stack already published.
///
/// The type's remaining properties are `CreateIndex`'s own members: the three scalars
/// [`S3VECTORS_INDEX_SCALAR_MEMBERS`] names, and the `MetadataConfiguration` and
/// `EncryptionConfiguration` structures, whose members S3 Vectors spells in lower camel
/// case.
async fn create_index(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let properties = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    let (bucket, index) = vector_index_identity(resource);
    let mut request = Map::new();
    if let Some(bucket) = bucket {
        request.insert("vectorBucketName".to_string(), Value::String(bucket));
    }
    request.insert("indexName".to_string(), Value::String(index));
    copy_members(&properties, S3VECTORS_INDEX_SCALAR_MEMBERS, &mut request);
    for (property, member) in [
        ("MetadataConfiguration", "metadataConfiguration"),
        ("EncryptionConfiguration", "encryptionConfiguration"),
    ] {
        if let Some(configuration) = properties.get(property) {
            request.insert(member.to_string(), lower_camel_members(configuration, 0)?);
        }
    }
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let answered =
        call_s3vectors(ctx, region, S3VECTORS_CREATE_INDEX_URI, &Value::Object(request))
            .await?;
    let arn = answered
        .get("indexArn")
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| Refusal {
            code: "InternalFailure".to_string(),
            message: "Amazon S3 Vectors answered CreateIndex without an indexArn".to_string(),
        })?;
    let mut attributes = BTreeMap::new();
    attributes.insert("IndexArn".to_string(), arn.to_string());
    Ok(Created::attributes(attributes))
}

/// Delete the vector index a stack created, addressed by its ARN — this resource's physical
/// id. One S3 Vectors no longer holds (`NotFoundException`) is already in the state the
/// delete asks for.
async fn delete_index(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "indexArn": physical_resource_id });
    match call_s3vectors(ctx, region, S3VECTORS_DELETE_INDEX_URI, &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == S3VECTORS_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The vector index's configuration as Amazon S3 Vectors reports it, for drift detection:
/// `GetIndex`'s own record, read by the ARN that is this resource's physical id. One it no
/// longer holds (`NotFoundException`) was deleted.
///
/// The properties its read reports under the names this type declares are the ones named
/// identically on both sides up to their initial letter; a property the read does not
/// report is not evidence of anything ([`super::drift`]).
async fn describe_index(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "indexArn": physical_resource_id });
    match call_s3vectors(ctx, region, S3VECTORS_GET_INDEX_URI, &request).await {
        Ok(answer) => Ok(answer.get("index").map(|index| {
            let mut members = vec![
                ("VectorBucketName", "vectorBucketName"),
                ("IndexName", "indexName"),
            ];
            members.extend_from_slice(S3VECTORS_INDEX_SCALAR_MEMBERS);
            reported_as_properties(index, &members)
        })),
        Err(refusal) if refusal.code == S3VECTORS_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The NAME of the vector bucket a resource record describes: the `VectorBucketName` the
/// deployment gave it, which is the name the template asked for or the
/// `<stack>-<logical id>-<suffix>` name CloudFormation generated — never the physical id,
/// which for this type is the bucket's ARN.
///
/// A record with no such attribute (one whose attribute budget was spent) falls back to the
/// bucket segment of that ARN, which is the bucket's name.
fn vector_bucket_name(resource: &StoredResource) -> String {
    if let Some(name) = resource.attributes.get("VectorBucketName") {
        return name.clone();
    }
    vector_bucket_of(&resource.physical_resource_id).unwrap_or_default()
}

/// The vector bucket and index a provisioned `AWS::S3Vectors::Index` record names: the two
/// segments of the index ARN that is this type's physical id
/// (`bucket/<bucket>/index/<index>`), which is how S3 Vectors keys an index.
///
/// A record whose physical id is NOT such an ARN names no bucket here either — the
/// declaration named none, so the deployment left the index with CloudFormation's own
/// generated name ([`super::template`]) — and the create then carries the index name alone,
/// so what the stack reports is S3 Vectors' own refusal of a request that addresses no
/// vector bucket rather than one invented here.
fn vector_index_identity(resource: &StoredResource) -> (Option<String>, String) {
    let parsed = arn_resource(&resource.physical_resource_id)
        .and_then(|resource| resource.strip_prefix(S3VECTORS_BUCKET_SEGMENT))
        .and_then(|rest| rest.split_once("/index/"))
        .filter(|(bucket, index)| {
            !bucket.is_empty()
                && !bucket.contains('/')
                && !index.is_empty()
                && !index.contains('/')
        });
    match parsed {
        Some((bucket, index)) => (Some(bucket.to_string()), index.to_string()),
        None => (
            None,
            resource
                .attributes
                .get("IndexName")
                .cloned()
                .unwrap_or_else(|| resource.physical_resource_id.clone()),
        ),
    }
}

/// The bucket name an Amazon S3 Vectors vector-bucket ARN carries, or `None` for anything
/// that is not one.
fn vector_bucket_of(arn: &str) -> Option<String> {
    let bucket = arn_resource(arn)?.strip_prefix(S3VECTORS_BUCKET_SEGMENT)?;
    (!bucket.is_empty() && !bucket.contains('/')).then(|| bucket.to_string())
}

/// The RESOURCE portion of an ARN: everything past the fifth `:`, which is where every ARN
/// puts the resource it names.
fn arn_resource(arn: &str) -> Option<&str> {
    let mut fields = arn.splitn(6, ':');
    let (Some("arn"), Some(_partition), Some(_service), Some(_region), Some(_account)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    fields.next()
}

/// Call one Amazon S3 Vectors operation on the `restJson1` wire its model declares: a POST
/// to the operation's own URI, carrying the request document.
async fn call_s3vectors(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    uri: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_rest_json(
        ctx,
        S3VECTORS_SIGNING_NAME,
        region,
        "POST",
        uri,
        Some(request),
    )
    .await
}

// ---------------------------------------------------------------------------
// Amazon EventBridge Scheduler: AWS::Scheduler::ScheduleGroup / ::Schedule
// ---------------------------------------------------------------------------

/// EventBridge Scheduler's signing name, from its model (`sigv4Name: scheduler`, service
/// shape `AWSChronosService`), which speaks `restJson1` — so its calls ride on
/// [`call_rest_json`], each on the URI its own operation's `@http` binding declares.
const SCHEDULER_SIGNING_NAME: &str = "scheduler";

/// The URIs the EventBridge Scheduler operations a stack makes are bound to, as that
/// model's `@http` traits declare them: `POST|GET|DELETE /schedule-groups/{Name}` and
/// `POST|GET|DELETE /schedules/{Name}`, the latter naming the group in the `groupName`
/// query parameter its read and its delete bind it to.
const SCHEDULER_SCHEDULE_GROUPS_URI: &str = "/schedule-groups";
const SCHEDULER_SCHEDULES_URI: &str = "/schedules";

/// The error code EventBridge Scheduler answers for a schedule group or a schedule it does
/// not hold (`ResourceNotFoundException`, the modelled 404 of its reads and its deletes).
const SCHEDULER_RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The members of a `CreateSchedule` request that are TIMESTAMPS (`StartDate` and
/// `EndDate`, each `@timestamp` in that model), which a template spells as an ISO-8601
/// instant and the wire carries as a number ([`epoch_seconds`]).
const SCHEDULER_INSTANT_MEMBERS: &[&str] = &["StartDate", "EndDate"];

/// Create the schedule group an `AWS::Scheduler::ScheduleGroup` declares, so a client that
/// deployed the stack and then calls `scheduler GetScheduleGroup` with the name the stack
/// publishes reads the group back rather than a `ResourceNotFoundException`.
///
/// "Ref returns the schedule group name", which is this resource's physical id and the one
/// member `CreateScheduleGroup` carries in its URI (`POST /schedule-groups/{Name}`), so the
/// group is created under the name CloudFormation decided. The type's one other property is
/// that create's own `Tags` member — the tags the declaration asks for plus the ones
/// CloudFormation gives the resource ([`tags_of`]), as the `[{Key, Value}]` list that
/// model's own `Tag` shape spells, so `scheduler ListTagsForResource` on the ARN the stack
/// publishes answers with the stack's tags and the `aws:cloudformation:*` ones.
///
/// The group's ARN is reported back as its `Arn` attribute, so the handle the stack
/// publishes is the one that service really minted.
async fn create_schedule_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = Map::new();
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("Tags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
    let answered = call_rest_json(
        ctx,
        SCHEDULER_SIGNING_NAME,
        region,
        "POST",
        &format!(
            "{SCHEDULER_SCHEDULE_GROUPS_URI}/{}",
            percent_encoded(&resource.physical_resource_id)
        ),
        Some(&Value::Object(request)),
    )
    .await?;
    let arn = answered
        .get("ScheduleGroupArn")
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| Refusal {
            code: "InternalFailure".to_string(),
            message: "EventBridge Scheduler answered CreateScheduleGroup without a \
                      ScheduleGroupArn"
                .to_string(),
        })?;
    let mut attributes = BTreeMap::new();
    attributes.insert("Arn".to_string(), arn.to_string());
    Ok(Created::attributes(attributes))
}

/// Delete the schedule group a stack created, addressed by its name — this resource's
/// physical id. A group EventBridge Scheduler no longer holds
/// (`ResourceNotFoundException`, judged on the CODE and never on a message that carries
/// the request's own name) is already in the state the delete asks for.
async fn delete_schedule_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let uri = format!(
        "{SCHEDULER_SCHEDULE_GROUPS_URI}/{}",
        percent_encoded(physical_resource_id)
    );
    match call_rest_json(ctx, SCHEDULER_SIGNING_NAME, region, "DELETE", &uri, None).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == SCHEDULER_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The schedule group's configuration as EventBridge Scheduler reports it, for drift
/// detection: `GetScheduleGroup`'s own record. One it no longer holds
/// (`ResourceNotFoundException`) was deleted.
///
/// The one property its read reports under the name this type declares is the group's
/// `Name`; the tags it holds are answered by a tagging call of its own, and a property the
/// read does not report is not evidence of anything ([`super::drift`]).
async fn describe_schedule_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let uri = format!(
        "{SCHEDULER_SCHEDULE_GROUPS_URI}/{}",
        percent_encoded(physical_resource_id)
    );
    match call_rest_json(ctx, SCHEDULER_SIGNING_NAME, region, "GET", &uri, None).await {
        Ok(answer) => Ok(Some(reported_as_properties(&answer, &[("Name", "Name")]))),
        Err(refusal) if refusal.code == SCHEDULER_RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Create the schedule an `AWS::Scheduler::Schedule` declares, in the group its
/// declaration names.
///
/// The type's properties ARE `CreateSchedule`'s own members — `Description`, `EndDate`,
/// `FlexibleTimeWindow`, `GroupName`, `KmsKeyArn`, `ScheduleExpression`,
/// `ScheduleExpressionTimezone`, `StartDate`, `State` and `Target` are named identically on
/// both sides, and the correspondence holds all the way down (a `Target`'s `Arn`, `RoleArn`,
/// `Input`, `DeadLetterConfig`, `RetryPolicy` and the five templated parameter structures;
/// a `FlexibleTimeWindow`'s `Mode` and `MaximumWindowInMinutes` —
/// `smithy_query(shape="Target,FlexibleTimeWindow")`) — so the evaluated property document
/// IS the request. `Name` is the one member CloudFormation decides, and the one that create
/// binds to its URI (`POST /schedules/{Name}`), so it is lifted out of the body; the two
/// timestamp members are carried as the wire carries a timestamp ([`epoch_seconds`]).
///
/// "Ref returns the schedule name", so the physical id stays the name the stack computed,
/// and the schedule's ARN is reported back as its `Arn` attribute.
async fn create_schedule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let mut request = match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    request.remove("Name");
    for member in SCHEDULER_INSTANT_MEMBERS {
        let Some(declared) = request.get(*member).cloned() else {
            continue;
        };
        // A template that already wrote the number goes through as written; only the
        // ISO-8601 TEXT a template spells a timestamp property with is converted, and text
        // that is not an instant is refused here rather than sent to be rejected.
        let Some(text) = declared.as_str() else {
            continue;
        };
        let instant = epoch_seconds(text).ok_or_else(|| Refusal {
            code: "ValidationException".to_string(),
            message: format!("{member} must be an ISO-8601 instant"),
        })?;
        request.insert((*member).to_string(), instant);
    }
    let answered = call_rest_json(
        ctx,
        SCHEDULER_SIGNING_NAME,
        region,
        "POST",
        &format!(
            "{SCHEDULER_SCHEDULES_URI}/{}",
            percent_encoded(&resource.physical_resource_id)
        ),
        Some(&Value::Object(request)),
    )
    .await?;
    let arn = answered
        .get("ScheduleArn")
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| Refusal {
            code: "InternalFailure".to_string(),
            message: "EventBridge Scheduler answered CreateSchedule without a ScheduleArn"
                .to_string(),
        })?;
    let mut attributes = BTreeMap::new();
    attributes.insert("Arn".to_string(), arn.to_string());
    Ok(Created::attributes(attributes))
}

/// Delete the schedule a stack created, addressed by the pair EventBridge Scheduler keys
/// one by: its name — this resource's physical id — and the group holding it
/// ([`scheduled_group_of`]). One that service no longer holds
/// (`ResourceNotFoundException`, judged on the CODE) is already in the state the delete
/// asks for, which is also the state a deleted group leaves its schedules in ("deleting a
/// schedule group results in EventBridge Scheduler deleting all schedules associated with
/// the group").
async fn delete_schedule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let uri = schedule_uri(resource);
    match call_rest_json(ctx, SCHEDULER_SIGNING_NAME, region, "DELETE", &uri, None).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == SCHEDULER_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The schedule's configuration as EventBridge Scheduler reports it, for drift detection:
/// `GetSchedule`'s own record, read by the same name-and-group pair the delete uses. One it
/// no longer holds (`ResourceNotFoundException`) was deleted.
///
/// Every member that read reports is spelled exactly as this type's property is, so the
/// comparison is the declaration against what the service really holds; a property the read
/// does not report is not evidence of anything ([`super::drift`]).
async fn describe_schedule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let uri = schedule_uri(resource);
    match call_rest_json(ctx, SCHEDULER_SIGNING_NAME, region, "GET", &uri, None).await {
        Ok(answer) => Ok(Some(reported_as_properties(
            &answer,
            &[
                ("Description", "Description"),
                ("FlexibleTimeWindow", "FlexibleTimeWindow"),
                ("GroupName", "GroupName"),
                ("KmsKeyArn", "KmsKeyArn"),
                ("Name", "Name"),
                ("ScheduleExpression", "ScheduleExpression"),
                ("ScheduleExpressionTimezone", "ScheduleExpressionTimezone"),
                ("State", "State"),
                ("Target", "Target"),
            ],
        ))),
        Err(refusal) if refusal.code == SCHEDULER_RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The URI that addresses ONE schedule: its name in the path and its group in the
/// `groupName` query parameter `GetSchedule` and `DeleteSchedule` bind it to.
fn schedule_uri(resource: &StoredResource) -> String {
    format!(
        "{SCHEDULER_SCHEDULES_URI}/{}?groupName={}",
        percent_encoded(&resource.physical_resource_id),
        percent_encoded(&scheduled_group_of(resource))
    )
}

/// The schedule group a provisioned `AWS::Scheduler::Schedule` record belongs to: the
/// `GroupName` its declaration names, else the account's `default` group — "if you omit
/// this, the default schedule group is used" — which is the group the create landed it in
/// and half of the pair EventBridge Scheduler keys a schedule by.
fn scheduled_group_of(resource: &StoredResource) -> String {
    resource
        .properties
        .get("GroupName")
        .and_then(Value::as_str)
        .filter(|group| !group.is_empty())
        .unwrap_or(DEFAULT_SCHEDULE_GROUP)
        .to_string()
}

/// One declared instant as the seconds since the epoch a `restJson1` timestamp member is
/// carried as (that protocol's default timestamp format, which the member declares no
/// `@timestampFormat` over), or `None` for text that is not an ISO-8601 instant.
fn epoch_seconds(declared: &str) -> Option<Value> {
    let instant = ::aws_smithy_types::DateTime::from_str(
        declared,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    ::serde_json::Number::from_f64(instant.as_secs_f64()).map(Value::Number)
}

// ---------------------------------------------------------------------------
// AWS Certificate Manager: AWS::CertificateManager::AcmeEndpoint
// ---------------------------------------------------------------------------

/// ACM's signing name and `X-Amz-Target` prefix, from its model (`sigv4Name: acm`,
/// service shape `CertificateManager`), which speaks `awsJson1_1`.
const ACM_SIGNING_NAME: &str = "acm";
const ACM_TARGET_PREFIX: &str = "CertificateManager";

/// Create the ACME endpoint an `AWS::CertificateManager::AcmeEndpoint` declares.
///
/// The type's properties ARE `CreateAcmeEndpoint`'s own members, spelled identically on
/// both sides — `AuthorizationBehavior`, `Contact`, `CertificateAuthority`,
/// `CertificateTags` and `Tags` (`smithy_query(operation="CreateAcmeEndpoint")`) — so the
/// evaluated property document IS the request. There is no member for a name or an
/// identifier, so nothing CloudFormation computed is sent with it.
///
/// `Tags` is the one member not carried verbatim: the endpoint carries the tags
/// CloudFormation gives it beside the declared ones ([`tags_of`]), as the `[{Key, Value}]`
/// list ACM's own `Tag` shape spells.
///
/// The endpoint's identifier is ACM's OWN to mint — the create takes no identifier and
/// answers with `AcmeEndpointArn` — so that answer is reported back as the resource's
/// physical id, which is what a client then passes to `DescribeAcmeEndpoint`, and as the
/// ARN attributes read off the resource, so neither reports a value the naming rules
/// computed instead.
async fn create_acme_endpoint(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("Tags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
    let reported = call_json_rpc(
        ctx,
        ACM_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ACM_TARGET_PREFIX}.CreateAcmeEndpoint"),
        &Value::Object(request),
    )
    .await?;
    let Some(arn) = reported.get("AcmeEndpointArn").and_then(Value::as_str) else {
        // The create answered 2xx without the ARN its output models. Nothing addresses an
        // ACME endpoint without one, so this is reported as a failure rather than recorded
        // as a resource with an identifier nothing minted.
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: "CreateAcmeEndpoint answered no AcmeEndpointArn".to_string(),
        });
    };
    let mut attributes = BTreeMap::new();
    attributes.insert("Arn".to_string(), arn.to_string());
    attributes.insert("AcmeEndpointArn".to_string(), arn.to_string());
    Ok(Created {
        physical_resource_id: Some(arn.to_string()),
        attributes,
    })
}

/// Delete the ACME endpoint a stack created.
///
/// `DeleteAcmeEndpoint` is `@idempotent` and its model declares no not-found error: an ARN
/// ACM holds no endpoint for is already in the state the delete asks for, and ACM answers
/// that call successfully, so there is no not-found code to read here.
async fn delete_acme_endpoint(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "AcmeEndpointArn": physical_resource_id });
    call_json_rpc(
        ctx,
        ACM_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ACM_TARGET_PREFIX}.DeleteAcmeEndpoint"),
        &request,
    )
    .await
    .map(|_| ())
}

/// The endpoint's configuration as ACM reports it, for drift detection. An ARN ACM no
/// longer holds an endpoint for (`ResourceNotFoundException`) was deleted.
///
/// Only the members `DescribeAcmeEndpoint` reports in the SAME shape the type declares
/// them in are reported, because a property the actual document does not carry is not
/// evidence of anything (`super::drift::differs`): `AuthorizationBehavior`, `Contact` and
/// `CertificateAuthority`, each the identical member on both sides. `Tags` is not reported
/// at all — the describe answers none, and `CertificateTags` comes back as a tag LIST
/// whose order ACM decides, so comparing it would read an ordering as drift.
async fn describe_acme_endpoint(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "AcmeEndpointArn": physical_resource_id });
    let reported = match call_json_rpc(
        ctx,
        ACM_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ACM_TARGET_PREFIX}.DescribeAcmeEndpoint"),
        &request,
    )
    .await
    {
        Ok(reported) => reported,
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    let Some(endpoint) = reported.get("AcmeEndpoint") else {
        return Ok(Some(Value::Object(Map::new())));
    };
    Ok(Some(reported_as_properties(
        endpoint,
        &[
            ("AuthorizationBehavior", "AuthorizationBehavior"),
            ("Contact", "Contact"),
            ("CertificateAuthority", "CertificateAuthority"),
        ],
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every property name becomes the API member it is, and only the first letter moves.
    #[test]
    fn property_names_become_their_api_members() {
        for (property, expected) in [
            ("RoleArn", "roleArn"),
            ("ActionTypeId", "actionTypeId"),
            ("Name", "name"),
            ("RoleARN", "roleARN"),
            ("", ""),
        ] {
            let actual = lower_initial(property);
            assert_eq!(
                actual, expected,
                "case {property:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// An action's `Configuration` keys are the provider's own and are NOT converted,
    /// while every member name around them is.
    #[test]
    fn action_configuration_keys_are_carried_verbatim() {
        let declared = serde_json::json!({
            "Stages": [{
                "Name": "Source",
                "Actions": [{
                    "Name": "Fetch",
                    "Configuration": {"S3Bucket": "b", "PollForSourceChanges": "false"}
                }]
            }]
        });
        let converted = camel_cased(&declared, 0).expect("a shallow document is carried");
        let action = &converted["stages"][0]["actions"][0];
        assert_eq!(
            action["name"].as_str(),
            Some("Fetch"),
            "expected the member names converted, got {converted}"
        );
        assert_eq!(
            action["configuration"]["S3Bucket"].as_str(),
            Some("b"),
            "expected the provider's own configuration keys verbatim, got {converted}"
        );
        assert!(
            action["configuration"].get("s3Bucket").is_none(),
            "a configuration key must not be converted, got {converted}"
        );
    }

    /// A property document deeper than [`MAX_PROPERTY_NESTING`] is REFUSED by both
    /// re-casing walks, and one at the bound is carried.
    ///
    /// The nesting of an evaluated property document is the REQUESTER's choice, and a
    /// recursive walk over it that is bounded by the input exhausts the native stack —
    /// which aborts the process rather than failing the one stack. The rows differ along
    /// the shape the nesting is built from (objects, arrays, and the two alternating) and
    /// cover the deepest document each walk carries, one level past it, and far past it.
    #[test]
    fn a_property_document_nested_past_the_bound_is_refused() {
        /// A document `levels` deep, built from `shape`.
        fn nested(shape: &str, levels: usize) -> Value {
            let mut value = Value::String("leaf".to_string());
            for level in 0..levels {
                value = match shape {
                    "objects" => serde_json::json!({ "Member": value }),
                    "arrays" => Value::Array(vec![value]),
                    // Alternating, so neither arm of the walk is the only one exercised.
                    _ if level % 2 == 0 => serde_json::json!({ "Member": value }),
                    _ => Value::Array(vec![value]),
                };
            }
            value
        }

        for shape in ["objects", "arrays", "alternating"] {
            // The deepest document the walk carries: the leaf sits at depth
            // MAX_PROPERTY_NESTING - 1, so the walk never reaches the bound.
            let carried = nested(shape, MAX_PROPERTY_NESTING - 1);
            for (walk, converted) in [
                ("camel_cased", camel_cased(&carried, 0)),
                ("lower_camel_members", lower_camel_members(&carried, 0)),
            ] {
                assert!(
                    converted.is_ok(),
                    "case {walk}({shape} at the bound): expected the document to be carried, \
                     got {:?}",
                    converted.err()
                );
            }

            for levels in [MAX_PROPERTY_NESTING + 1, MAX_PROPERTY_NESTING * 4] {
                let too_deep = nested(shape, levels);
                for (walk, refused) in [
                    ("camel_cased", camel_cased(&too_deep, 0)),
                    ("lower_camel_members", lower_camel_members(&too_deep, 0)),
                ] {
                    let refusal = refused.err().unwrap_or_else(|| {
                        panic!("case {walk}({shape}, {levels} levels): expected a refusal")
                    });
                    assert_eq!(
                        refusal.code, "ValidationException",
                        "case {walk}({shape}, {levels} levels): expected ValidationException, got \
                         {refusal:?}"
                    );
                }
            }
        }
    }

    /// A policy document reaches IAM as text whichever way the template spells it.
    #[test]
    fn policy_documents_render_as_text() {
        for declared in [
            serde_json::json!({"Version": "2012-10-17"}),
            serde_json::json!("{\"Version\":\"2012-10-17\"}"),
        ] {
            let actual = document_text(&declared);
            assert_eq!(
                actual.as_deref(),
                Some(r#"{"Version":"2012-10-17"}"#),
                "case {declared:?}: expected the document as text, got {actual:?}"
            );
        }
    }
}
