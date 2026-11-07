// SPDX-License-Identifier: CC0-1.0

//! The action types this service PROVIDES ITSELF — the catalog no caller created —
//! with the per-provider detail an `ActionType` reports for each one: its artifact
//! counts, the configuration properties an action declaration of it must carry, and the
//! deep links the console follows to the resource the action acts on.
//!
//! Split out of [`super::action_types`] so one file holds one concern (and so neither
//! file grows past the project's file-length limit); every name stays re-exported from
//! `crate::state`, which is where the handlers reach it.
//!
//! # Where the entries come from
//!
//! The page `ActionTypeId$provider` and `ActionTypeId$owner` both link to in their model
//! documentation, "Valid action types and providers in CodePipeline"
//! (<https://docs.aws.amazon.com/codepipeline/latest/userguide/reference-pipeline-structure.html#actions-valid-providers>),
//! which lists each category's providers and their owners, together with that page's
//! per-action structure reference, which documents for each provider:
//!
//! * the number of input and output artifacts the action takes and produces
//!   ([`BuiltInActionType::input_artifacts`] / [`BuiltInActionType::output_artifacts`]);
//! * its configuration parameters and which of them are required
//!   ([`BuiltInActionType::configuration_properties`]).
//!
//! Three choices this file makes, stated rather than hidden, because the model does not
//! state them:
//!
//! * a provider whose documented artifact count exceeds `MaximumArtifactCount`'s modelled
//!   `@range(max: 5)` (the CloudFormation deploy action's ten input artifacts, the
//!   `Commands` action's) is left OUT rather than reported with a count the model cannot
//!   carry;
//! * `key` marks a property the action's deep link is built FROM — exactly the ones
//!   [`BuiltInActionType::entity_url_template`] substitutes, since
//!   `ActionTypeSettings$entityUrlTemplate` is documented as "a deep link to the resources
//!   of the external system" and a property that identifies that resource is what such a
//!   link needs;
//! * `queryable` is false on every entry, because `ActionConfigurationProperty$queryable`
//!   documents it as the member that "is used with `PollForJobs`" "[w]hen creating a custom
//!   action" — none of these action types is run by a job worker of the caller's account;
//!   and `description` is left unset, because the display text of a provider's property is
//!   the provider's own and neither the model nor the reference page states it.
//!
//! The URL templates below are REPORTED, never followed: each is the text of a modelled
//! member (`ActionTypeSettings$entityUrlTemplate` is documented as "[t]he URL returned to
//! the CodePipeline console that provides a deep link to the resources of the external
//! system"), so this module is a table of response strings and nothing here opens a
//! connection, resolves a name or reads a file.

use super::*;

/// One configuration property of an action type the service provides: the members
/// `ActionConfigurationProperty` carries that the provider's reference page states.
///
/// `queryable` and `description` are not here — see this module's note on both.
#[derive(Debug, Clone, Copy)]
pub struct BuiltInConfigurationProperty {
    /// `ActionConfigurationProperty$name`: the parameter's name, as an action's
    /// `configuration` map keys it.
    pub name: &'static str,
    /// Whether the provider's reference page documents the parameter as required.
    pub required: bool,
    /// Whether the action's deep link is built from this property (see the module note).
    pub key: bool,
    /// Whether the value is a secret the API hides from a read of the pipeline — a
    /// credential the caller hands the provider.
    pub secret: bool,
}

/// Declare one configuration property, in the order
/// `ActionConfigurationProperty`'s members are declared.
const fn property(
    name: &'static str,
    required: bool,
    key: bool,
    secret: bool,
) -> BuiltInConfigurationProperty {
    BuiltInConfigurationProperty {
        name,
        required,
        key,
        secret,
    }
}

/// One entry of the catalog of action types this service provides itself: the four
/// `ActionTypeId` members plus everything else `ActionType` reports for it.
#[derive(Debug, Clone, Copy)]
pub struct BuiltInActionType {
    /// An `ActionCategory` value.
    pub category: &'static str,
    /// An `ActionOwner` value — `AWS` for an action the service runs itself,
    /// `ThirdParty` for one a partner integration runs.
    pub owner: &'static str,
    /// The provider of the service the action calls.
    pub provider: &'static str,
    /// The action type's version.
    pub version: &'static str,
    /// `(minimumCount, maximumCount)` of the action's input artifacts.
    pub input_artifacts: (i32, i32),
    /// `(minimumCount, maximumCount)` of the action's output artifacts.
    pub output_artifacts: (i32, i32),
    /// `ActionTypeSettings$entityUrlTemplate`: the deep link to the resource the action
    /// acts on, with `{Config:<property>}` standing for the value the action's
    /// configuration gives that property.
    pub entity_url_template: Option<&'static str>,
    /// `ActionTypeSettings$executionUrlTemplate`: the link to the external system's own
    /// view of the work this action ran.
    pub execution_url_template: Option<&'static str>,
    /// `ActionTypeSettings$revisionUrlTemplate`: the link to the revision the action
    /// consumed.
    pub revision_url_template: Option<&'static str>,
    /// `ActionTypeSettings$thirdPartyConfigurationUrl`: where a user signs up for the
    /// external service and configures it. Only a `ThirdParty` provider has one.
    pub third_party_configuration_url: Option<&'static str>,
    /// The action's documented configuration parameters, in the order its reference page
    /// lists them, bounded by `ActionConfigurationPropertyList`'s `@length(max: 10)`.
    pub configuration_properties: &'static [BuiltInConfigurationProperty],
}

impl BuiltInActionType {
    /// This entry's identity.
    pub fn key(&self) -> ActionTypeKey {
        ActionTypeKey {
            category: self.category.to_string(),
            owner: self.owner.to_string(),
            provider: self.provider.to_string(),
            version: self.version.to_string(),
        }
    }

    /// The URLs this entry reports, or `None` when it names none at all.
    ///
    /// `None` and an empty `ActionTypeSettings` state the same fact, and the member is
    /// reported either way ([`ActionTypeRecord::to_action_type`] always carries it), so
    /// this is the record's own shape rather than the wire's.
    fn settings(&self) -> Option<ActionTypeSettings> {
        let settings = ActionTypeSettings {
            third_party_configuration_url: self.third_party_configuration_url.map(str::to_string),
            entity_url_template: self.entity_url_template.map(str::to_string),
            execution_url_template: self.execution_url_template.map(str::to_string),
            revision_url_template: self.revision_url_template.map(str::to_string),
        };
        [
            &settings.third_party_configuration_url,
            &settings.entity_url_template,
            &settings.execution_url_template,
            &settings.revision_url_template,
        ]
        .iter()
        .any(|url| url.is_some())
        .then_some(settings)
    }

    /// This entry as a stored record: available in every Region, carrying the URLs and
    /// the configuration properties its provider documents, and tagged by nobody.
    pub fn record(&self) -> ActionTypeRecord {
        ActionTypeRecord {
            settings: self.settings(),
            configuration_properties: self
                .configuration_properties
                .iter()
                .map(|property| ActionConfigurationProperty {
                    name: property.name.to_string(),
                    required: property.required,
                    key: property.key,
                    secret: property.secret,
                    // See the module note on both members.
                    queryable: false,
                    description: None,
                    r#type: None,
                })
                .collect(),
            input_artifact_details: ArtifactDetails {
                minimum_count: self.input_artifacts.0,
                maximum_count: self.input_artifacts.1,
            },
            output_artifact_details: ArtifactDetails {
                minimum_count: self.output_artifacts.0,
                maximum_count: self.output_artifacts.1,
            },
            region: None,
            // Nobody tagged an action type the service provides itself.
            tags: Vec::new(),
        }
    }
}

/// Declare one catalog entry that names no URL and no configuration property, so an
/// entry below states only what it has.
const fn entry(
    category: &'static str,
    owner: &'static str,
    provider: &'static str,
    version: &'static str,
    input_artifacts: (i32, i32),
    output_artifacts: (i32, i32),
) -> BuiltInActionType {
    BuiltInActionType {
        category,
        owner,
        provider,
        version,
        input_artifacts,
        output_artifacts,
        entity_url_template: None,
        execution_url_template: None,
        revision_url_template: None,
        third_party_configuration_url: None,
        configuration_properties: &[],
    }
}

/// The action types this service provides on its own — the ones no caller created —
/// as `ListActionTypes` ("a summary of all CodePipeline action types associated with
/// your account") reports them beside the account's own registrations.
///
/// See this module's note for where every member of every entry comes from. Every version
/// is `1`, the only version these actions publish.
pub const BUILT_IN_ACTION_TYPES: [BuiltInActionType; 17] = [
    // ---- Approval -------------------------------------------------------------------
    BuiltInActionType {
        // The manual-approval action takes and produces no artifact, and its three
        // parameters are all optional: it carries the message a reviewer reads.
        configuration_properties: &[
            property("NotificationArn", false, false, false),
            property("CustomData", false, false, false),
            property("ExternalEntityLink", false, false, false),
        ],
        ..entry("Approval", "AWS", "Manual", "1", (0, 0), (0, 0))
    },
    // ---- Build ----------------------------------------------------------------------
    BuiltInActionType {
        entity_url_template: Some(
            "https://console.aws.amazon.com/codesuite/codebuild/projects/{Config:ProjectName}",
        ),
        execution_url_template: Some(
            "https://console.aws.amazon.com/codesuite/codebuild/projects/{Config:ProjectName}/history",
        ),
        configuration_properties: CODEBUILD_PROPERTIES,
        ..entry("Build", "AWS", "CodeBuild", "1", (1, 5), (0, 5))
    },
    // ---- Deploy ---------------------------------------------------------------------
    BuiltInActionType {
        entity_url_template: Some(
            "https://console.aws.amazon.com/codesuite/codedeploy/applications/{Config:ApplicationName}/deployment-groups/{Config:DeploymentGroupName}",
        ),
        configuration_properties: &[
            property("ApplicationName", true, true, false),
            property("DeploymentGroupName", true, true, false),
        ],
        ..entry("Deploy", "AWS", "CodeDeploy", "1", (1, 1), (0, 0))
    },
    BuiltInActionType {
        entity_url_template: Some(
            "https://console.aws.amazon.com/ecs/home#/clusters/{Config:ClusterName}/services/{Config:ServiceName}/details",
        ),
        configuration_properties: &[
            property("ClusterName", true, true, false),
            property("ServiceName", true, true, false),
            property("FileName", false, false, false),
            property("DeploymentTimeout", false, false, false),
        ],
        ..entry("Deploy", "AWS", "ECS", "1", (1, 1), (0, 0))
    },
    BuiltInActionType {
        entity_url_template: Some(
            "https://console.aws.amazon.com/elasticbeanstalk/home#/environment/dashboard?applicationName={Config:ApplicationName}&environmentName={Config:EnvironmentName}",
        ),
        configuration_properties: &[
            property("ApplicationName", true, true, false),
            property("EnvironmentName", true, true, false),
        ],
        ..entry("Deploy", "AWS", "ElasticBeanstalk", "1", (1, 1), (0, 0))
    },
    BuiltInActionType {
        entity_url_template: Some(
            "https://console.aws.amazon.com/codesuite/opsworks/stacks/{Config:Stack}",
        ),
        configuration_properties: &[
            property("Stack", true, true, false),
            property("App", true, false, false),
            property("Layer", false, false, false),
        ],
        ..entry("Deploy", "AWS", "OpsWorks", "1", (1, 1), (0, 0))
    },
    BuiltInActionType {
        entity_url_template: Some(
            "https://console.aws.amazon.com/s3/buckets/{Config:BucketName}",
        ),
        configuration_properties: &[
            property("BucketName", true, true, false),
            property("Extract", true, false, false),
            property("ObjectKey", false, false, false),
            property("KMSEncryptionKeyARN", false, false, false),
            property("CannedACL", false, false, false),
            property("CacheControl", false, false, false),
        ],
        ..entry("Deploy", "AWS", "S3", "1", (1, 1), (0, 0))
    },
    // ---- Invoke ---------------------------------------------------------------------
    BuiltInActionType {
        entity_url_template: Some(
            "https://console.aws.amazon.com/lambda/home#/functions/{Config:FunctionName}",
        ),
        configuration_properties: &[
            property("FunctionName", true, true, false),
            property("UserParameters", false, false, false),
        ],
        ..entry("Invoke", "AWS", "Lambda", "1", (0, 5), (0, 5))
    },
    BuiltInActionType {
        configuration_properties: &[
            property("StateMachineArn", true, true, false),
            property("ExecutionNamePrefix", false, false, false),
            property("InputType", false, false, false),
            property("Input", false, false, false),
        ],
        ..entry("Invoke", "AWS", "StepFunctions", "1", (0, 1), (0, 1))
    },
    // ---- Source ---------------------------------------------------------------------
    BuiltInActionType {
        entity_url_template: Some(
            "https://console.aws.amazon.com/codesuite/codecommit/repositories/{Config:RepositoryName}/browse/refs/heads/{Config:BranchName}",
        ),
        configuration_properties: &[
            property("RepositoryName", true, true, false),
            property("BranchName", true, true, false),
            property("PollForSourceChanges", false, false, false),
            property("OutputArtifactFormat", false, false, false),
        ],
        ..entry("Source", "AWS", "CodeCommit", "1", (0, 0), (1, 1))
    },
    BuiltInActionType {
        // The repository lives behind a connection to the external provider, so the
        // console links the connection rather than a resource of this account.
        configuration_properties: &[
            property("ConnectionArn", true, true, false),
            property("FullRepositoryId", true, false, false),
            property("BranchName", true, false, false),
            property("OutputArtifactFormat", false, false, false),
            property("DetectChanges", false, false, false),
        ],
        ..entry(
            "Source",
            "AWS",
            "CodeStarSourceConnection",
            "1",
            (0, 0),
            (1, 1),
        )
    },
    BuiltInActionType {
        entity_url_template: Some(
            "https://console.aws.amazon.com/ecr/repositories/{Config:RepositoryName}",
        ),
        configuration_properties: &[
            property("RepositoryName", true, true, false),
            property("ImageTag", false, false, false),
        ],
        ..entry("Source", "AWS", "ECR", "1", (0, 0), (1, 1))
    },
    BuiltInActionType {
        entity_url_template: Some(
            "https://console.aws.amazon.com/s3/buckets/{Config:S3Bucket}?prefix={Config:S3ObjectKey}",
        ),
        configuration_properties: &[
            property("S3Bucket", true, true, false),
            property("S3ObjectKey", true, true, false),
            property("PollForSourceChanges", false, false, false),
            property("AllowOverrideForS3ObjectKey", false, false, false),
        ],
        ..entry("Source", "AWS", "S3", "1", (0, 0), (1, 1))
    },
    BuiltInActionType {
        // The GitHub (OAuth) source action: the repository is the external system, so
        // every link is a github.com one and the token the caller hands over is secret.
        entity_url_template: Some("https://github.com/{Config:Owner}/{Config:Repo}/tree/{Config:Branch}"),
        revision_url_template: Some("https://github.com/{Config:Owner}/{Config:Repo}/commits"),
        third_party_configuration_url: Some("https://github.com/join"),
        configuration_properties: &[
            property("Owner", true, true, false),
            property("Repo", true, true, false),
            property("Branch", true, true, false),
            property("OAuthToken", true, false, true),
            property("PollForSourceChanges", false, false, false),
        ],
        ..entry("Source", "ThirdParty", "GitHub", "1", (0, 0), (1, 1))
    },
    // ---- Test -----------------------------------------------------------------------
    BuiltInActionType {
        entity_url_template: Some(
            "https://console.aws.amazon.com/codesuite/codebuild/projects/{Config:ProjectName}",
        ),
        execution_url_template: Some(
            "https://console.aws.amazon.com/codesuite/codebuild/projects/{Config:ProjectName}/history",
        ),
        configuration_properties: CODEBUILD_PROPERTIES,
        ..entry("Test", "AWS", "CodeBuild", "1", (1, 5), (0, 5))
    },
    BuiltInActionType {
        entity_url_template: Some(
            "https://console.aws.amazon.com/devicefarm/home#/projects/{Config:ProjectId}/runs",
        ),
        configuration_properties: &[
            property("ProjectId", true, true, false),
            property("DevicePoolArn", true, false, false),
            property("AppType", true, false, false),
            property("App", true, false, false),
            property("TestType", true, false, false),
            property("Test", false, false, false),
            property("TestSpec", false, false, false),
        ],
        ..entry("Test", "AWS", "DeviceFarm", "1", (1, 1), (0, 0))
    },
    BuiltInActionType {
        // A partner test integration: the account's pipeline calls the partner's service
        // with the caller's own credential for it, which is why the token is secret.
        third_party_configuration_url: Some("https://ghostinspector.com/docs/integration/aws-codepipeline/"),
        configuration_properties: &[
            property("ClientId", true, true, false),
            property("ClientToken", true, false, true),
            property("TestId", true, false, false),
            property("ExtraParams", false, false, false),
        ],
        ..entry("Test", "ThirdParty", "GhostInspector", "1", (1, 1), (0, 0))
    },
];

/// The CodeBuild action's configuration parameters, shared by the two categories it is
/// published in (`Build` and `Test`): one provider, one documented parameter list.
const CODEBUILD_PROPERTIES: &[BuiltInConfigurationProperty] = &[
    property("ProjectName", true, true, false),
    property("PrimarySource", false, false, false),
    property("BatchEnabled", false, false, false),
    property("CombineArtifacts", false, false, false),
    property("EnvironmentVariables", false, false, false),
];
