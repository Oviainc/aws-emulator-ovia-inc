// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-appconfig-amazonappconfig service.
//!
//! AppConfig's control plane is a tree, and this module holds all of it: an
//! application owns environments and configuration profiles, a profile owns the
//! hosted configuration versions written into it, and an environment owns the
//! deployments started into it. Extensions sit beside that tree, owned by the account
//! rather than by any application, each one holding its own version history. Every
//! value an operation answers is read back out of
//! this tree rather than computed from the request that asked for it.
//!
//! A deployment is the one resource here with a lifetime of its own: AppConfig rolls
//! a configuration out over the strategy's deployment duration, then monitors it for
//! the strategy's final bake time, and only then is the deployment `COMPLETE`. That
//! progression is owed by the CLOCK, not by anybody calling — so it is stored as the
//! instant it started plus the strategy's documented durations, and
//! [`Deployment::materialize`] brings the record up to whatever the clock already
//! made due. Reporting it never invents a transition that was not owed.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Action, AppliedExtension, DeletionProtectionSettings, DeploymentEvent, Monitor, Parameter,
    Treatment, Validator, VendedMetricsSettings,
};

pub mod feature_flags;

/// `DeploymentState` values this service moves a deployment through. Each is a member
/// of `DEPLOYMENT_STATE_VALUES` in `crate::ops::types` (the model's `DeploymentState`
/// enum).
pub const DEPLOYMENT_DEPLOYING: &str = "DEPLOYING";
/// See [`DEPLOYMENT_DEPLOYING`].
pub const DEPLOYMENT_BAKING: &str = "BAKING";
/// See [`DEPLOYMENT_DEPLOYING`].
pub const DEPLOYMENT_COMPLETE: &str = "COMPLETE";

/// `EnvironmentState` values this service moves an environment through — the WIRE
/// values, which for this one enum are NOT the model's member names.
///
/// `ENVIRONMENT_STATE_VALUES` in `crate::ops::types` lists the member names
/// (`READY_FOR_DEPLOYMENT`, `DEPLOYING`, …), but AppConfig serves `Environment$State`
/// in the CamelCase form its API Reference lists as that member's valid values —
/// `ReadyForDeployment | Deploying | RollingBack | RolledBack | Reverted` — and that
/// is what every recorded response carries. The shipped model JSON in
/// `smithy-model/` carries no `@enumValue` trait on the enum's members, so the
/// generated value list fell back to the member names; the recorded wire form is
/// authoritative and these constants are the single place the service spells it.
///
/// Every other enum this module names a value of (`DeploymentState`, `GrowthType`,
/// `DeploymentEventType`, `TriggeredBy`) is served as its member name, exactly as the
/// AppConfig API Reference lists those members' valid values, so their constants are
/// unchanged.
pub const ENVIRONMENT_READY_FOR_DEPLOYMENT: &str = "ReadyForDeployment";
/// See [`ENVIRONMENT_READY_FOR_DEPLOYMENT`].
pub const ENVIRONMENT_DEPLOYING: &str = "Deploying";
/// See [`ENVIRONMENT_READY_FOR_DEPLOYMENT`]. No operation moves an environment here
/// yet — AppConfig enters it only while rolling a deployment back — so the value is
/// carried for completeness of the wire catalog.
pub const ENVIRONMENT_ROLLING_BACK: &str = "RollingBack";
/// See [`ENVIRONMENT_ROLLING_BACK`].
pub const ENVIRONMENT_ROLLED_BACK: &str = "RolledBack";
/// See [`ENVIRONMENT_ROLLING_BACK`].
pub const ENVIRONMENT_REVERTED: &str = "Reverted";

/// `ExperimentDefinitionStatus` values this service reports, from
/// `EXPERIMENT_DEFINITION_STATUS_VALUES` in `crate::ops::types` (the model's
/// `ExperimentDefinitionStatus` enum: `ACTIVE` | `IDLE` | `ARCHIVED`).
///
/// A definition that was just created has no run behind it, so it is `IDLE`; `ACTIVE`
/// is what a started experiment run makes it, and `ARCHIVED` what an archiving delete
/// leaves behind.
pub const EXPERIMENT_DEFINITION_IDLE: &str = "IDLE";
/// See [`EXPERIMENT_DEFINITION_IDLE`].
pub const EXPERIMENT_DEFINITION_ACTIVE: &str = "ACTIVE";
/// See [`EXPERIMENT_DEFINITION_IDLE`].
pub const EXPERIMENT_DEFINITION_ARCHIVED: &str = "ARCHIVED";

/// `GrowthType` values, from `GROWTH_TYPE_VALUES` in `crate::ops::types`.
pub const GROWTH_LINEAR: &str = "LINEAR";
/// See [`GROWTH_LINEAR`].
pub const GROWTH_EXPONENTIAL: &str = "EXPONENTIAL";

/// The `ReplicateTo` value meaning the strategy is saved nowhere but AppConfig, from
/// `REPLICATE_TO_VALUES` in `crate::ops::types`.
pub const REPLICATE_TO_NONE: &str = "NONE";

/// `DeploymentEventType` values this service records, from
/// `DEPLOYMENT_EVENT_TYPE_VALUES` in `crate::ops::types`.
pub const EVENT_DEPLOYMENT_STARTED: &str = "DEPLOYMENT_STARTED";
/// See [`EVENT_DEPLOYMENT_STARTED`].
pub const EVENT_PERCENTAGE_UPDATED: &str = "PERCENTAGE_UPDATED";
/// See [`EVENT_DEPLOYMENT_STARTED`].
pub const EVENT_BAKE_TIME_STARTED: &str = "BAKE_TIME_STARTED";
/// See [`EVENT_DEPLOYMENT_STARTED`].
pub const EVENT_DEPLOYMENT_COMPLETED: &str = "DEPLOYMENT_COMPLETED";

/// The `DeploymentType` value of a deployment a caller started with `StartDeployment`,
/// from `DEPLOYMENT_TYPE_VALUES` in `crate::ops::types` (the model's `DeploymentType`
/// enum, `USER | MANAGED`). The other value is for a deployment AppConfig starts on its
/// own behalf, which this emulator never does: every deployment it holds was asked for by
/// a caller.
pub const DEPLOYMENT_TYPE_USER: &str = "USER";

/// `TriggeredBy` values, from `TRIGGERED_BY_VALUES` in `crate::ops::types`.
pub const TRIGGERED_BY_USER: &str = "USER";
/// See [`TRIGGERED_BY_USER`].
pub const TRIGGERED_BY_APPCONFIG: &str = "APPCONFIG";

/// The documented application quota: `ServiceQuotaExceededException`'s own
/// documentation lists "Applications: 100 max".
pub const MAX_APPLICATIONS: usize = 100;

/// Bounds on the stores this service grows one request at a time, so a loop of create
/// calls cannot grow the emulator's heap without limit. AppConfig's own per-account
/// quotas for these resources are adjustable and are not carried in the model, so these
/// numbers are the EMULATOR's bounds; a write past one is refused with the quota error
/// the operation already declares (`ServiceQuotaExceededException`, HTTP 402) rather
/// than accepted silently.
pub const MAX_ENVIRONMENTS_PER_APPLICATION: usize = 100;
/// See [`MAX_ENVIRONMENTS_PER_APPLICATION`].
pub const MAX_CONFIGURATION_PROFILES_PER_APPLICATION: usize = 100;
/// See [`MAX_ENVIRONMENTS_PER_APPLICATION`].
pub const MAX_HOSTED_CONFIGURATION_VERSIONS_PER_PROFILE: usize = 100;
/// See [`MAX_ENVIRONMENTS_PER_APPLICATION`].
pub const MAX_EXPERIMENT_DEFINITIONS_PER_APPLICATION: usize = 100;
/// See [`MAX_ENVIRONMENTS_PER_APPLICATION`]. Enforced by
/// [`ComAmazonawsAppconfigAmazonappconfigState::try_insert_strategy`], which is the only
/// way into the created-strategy map.
pub const MAX_DEPLOYMENT_STRATEGIES: usize = 100;
/// See [`MAX_ENVIRONMENTS_PER_APPLICATION`]. Enforced by
/// [`ComAmazonawsAppconfigAmazonappconfigState::try_insert_extension`], the only way a
/// new extension reaches the extension map.
pub const MAX_EXTENSIONS: usize = 100;
/// See [`MAX_ENVIRONMENTS_PER_APPLICATION`]. Every `CreateExtension` against a name that
/// already exists adds one more version to that extension, so the per-extension version
/// history is bounded too.
pub const MAX_EXTENSION_VERSIONS: usize = 100;

/// The largest single hosted configuration this emulator stores, and the total it holds
/// across every configuration profile.
///
/// A hosted configuration version is the one place this service stores caller-supplied
/// bytes, so both the per-write size and the cumulative total are bounded here. AppConfig's
/// own configuration-size quota is not in the model, so these are the emulator's
/// bounds: an over-sized write is refused with the modelled `PayloadTooLargeException`
/// (HTTP 413), which reports the limit and the size in [`BYTES_MEASURE_KILOBYTES`] so a
/// client can read both, and a write that would exceed the total budget is refused with
/// `ServiceQuotaExceededException` (HTTP 402).
pub const MAX_HOSTED_CONFIGURATION_BYTES: usize = 2 * 1024 * 1024;
/// See [`MAX_HOSTED_CONFIGURATION_BYTES`].
pub const MAX_TOTAL_HOSTED_CONFIGURATION_BYTES: usize = 64 * 1024 * 1024;
/// Bytes per kilobyte, the unit `PayloadTooLargeException` reports its limit in.
pub const BYTES_PER_KILOBYTE: f32 = 1024.0;
/// The `BytesMeasure` value `PayloadTooLargeException` reports, from
/// `BYTES_MEASURE_VALUES` in `crate::ops::types`.
pub const BYTES_MEASURE_KILOBYTES: &str = "KILOBYTES";

/// The message AppConfig refuses an over-sized hosted configuration with.
///
/// A FIXED service-owned constant, not a sentence built from the request: the recorded
/// refusal carries exactly this text whatever the payload's size, and
/// `PayloadTooLargeException`'s own `Measure` / `Limit` / `Size` members are what
/// report the numbers. The emulator must not substitute prose of its own here — a
/// client that surfaces the service's message would then show emulator internals.
pub const CONTENT_SIZE_LIMIT_EXCEEDED_MESSAGE: &str = "Content size limit exceeded.";

/// How many deployment records an environment keeps.
///
/// AppConfig retains an environment's deployment history; this emulator retains the most
/// recent of it and drops the oldest, already-terminal records beyond this bound, so an
/// environment deployed into in a loop cannot grow without limit. The bound is the
/// emulator's, not an AppConfig quota.
pub const MAX_RETAINED_DEPLOYMENTS_PER_ENVIRONMENT: usize = 200;

/// The `LocationUri` that selects AppConfig's own hosted configuration store: "For the
/// AppConfig hosted configuration store and for feature flags, specify `hosted`", as the
/// model documents on `CreateConfigurationProfile`'s `LocationUri` member.
pub const HOSTED_LOCATION_URI: &str = "hosted";

/// The `LocationUri` prefix that selects a CodePipeline pipeline: "For an Amazon Web
/// Services CodePipeline pipeline, specify the URI in the following format:
/// `codepipeline`://<pipeline name>", as the model documents on
/// `CreateConfigurationProfile`'s `LocationUri` member.
pub const CODEPIPELINE_LOCATION_PREFIX: &str = "codepipeline://";

/// Whether a configuration profile stored at `location_uri` needs a `RetrievalRoleArn`.
///
/// The model's own `RetrievalRoleArn` documentation states the rule: "A retrieval role
/// ARN is not required for configurations stored in CodePipeline or the AppConfig hosted
/// configuration store. It is required for all other sources that store your
/// configuration." So a role is required for every location except those two — the
/// member is optional in the model, which makes this a server-side cross-field rule
/// rather than one the schema walk can refuse.
pub fn location_uri_requires_retrieval_role(location_uri: &str) -> bool {
    location_uri != HOSTED_LOCATION_URI
        && !location_uri.starts_with(CODEPIPELINE_LOCATION_PREFIX)
}

/// How AppConfig refuses a configuration profile whose `LocationUri` needs a
/// `RetrievalRoleArn` the request did not carry.
///
/// A cross-field rule, stated in the same `N validation error detected: …` envelope the
/// boundary validator uses for a modelled constraint, naming the member that was null
/// and the uri that made it required — so the message is derived from the request, not
/// from a stored sentence.
pub fn retrieval_role_required_message(location_uri: &str) -> String {
    format!(
        "1 validation error detected: Value null at 'retrievalRoleArn' failed to satisfy \
         constraint: Member must not be null if the request has the following uri: \
         {location_uri}"
    )
}

/// The `Type` values a configuration profile accepts. The model's own
/// `CreateConfigurationProfile` `Type` documentation says: "When calling this API, enter
/// one of the following values for `Type`: `AWS.AppConfig.FeatureFlags`,
/// `AWS.Freeform`."
pub const CONFIGURATION_PROFILE_TYPES: &[&str] = &[FEATURE_FLAGS_PROFILE_TYPE, "AWS.Freeform"];

/// The configuration profile `Type` whose hosted configuration content AppConfig runs
/// through the feature-flags helper extension before storing it; see
/// [`feature_flags`].
pub const FEATURE_FLAGS_PROFILE_TYPE: &str = "AWS.AppConfig.FeatureFlags";

/// This service's namespace in an ARN: the third colon-separated section of every ARN
/// that addresses an AppConfig resource. An ARN naming any other service addresses no
/// resource this service holds.
pub const SERVICE_NAMESPACE: &str = "appconfig";

/// The Region an ARN this service mints carries when the request names none.
/// `::engine::emulator_arn`'s own documentation gives this as the emulator's default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// Whether `region` is shaped like an AWS Region code (`us-east-1`, `ap-southeast-4`):
/// lower-case letters, digits and hyphens only, and short enough that an ARN carrying it
/// stays inside the modelled `Arn len 20..2048`.
fn is_region_shaped(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= 32
        && region
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The Region this request was signed for, for the ARNs its response carries.
///
/// Read from the caller's own SigV4 credential scope, and checked against the Region
/// grammar before it is interpolated into an ARN: a scope naming something that is not a
/// Region (or no scope at all) falls back to [`DEFAULT_REGION`] rather than letting
/// caller-chosen text reshape an ARN's colon-delimited fields.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_shaped(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of one version of one extension: `extension/<extension id>/<version number>`,
/// the resource portion [`parse_resource_arn`] reads back.
pub fn extension_arn(region: &str, extension_id: &str, version_number: i32) -> String {
    ::engine::emulator_arn(
        SERVICE_NAMESPACE,
        region,
        &format!("extension/{extension_id}/{version_number}"),
    )
}

/// The most tags ONE resource keeps, however many tag calls fill it: "You can specify a
/// maximum of 50 tags for a resource" (`TagResource`'s own documentation,
/// `smithy_query(query="operation",operation="TagResource")`).
///
/// The model bounds a single request's `TagMap` at 50 entries, which the boundary
/// validator already enforces; this is the bound on the MERGED map the resource carries,
/// so a sequence of tag calls cannot grow one past what a single call could set. A write
/// past it is refused with `BadRequestException` — "the input fails to satisfy the
/// constraints specified by an Amazon Web Services service".
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The longest tag key this service stores: "The tag key can be up to 128 characters"
/// (`smithy_query(query="doc",shape="TagResourceRequest",member="Tags")`), which is also
/// the length the model's own `TagKey` shape declares (`length 1..128`).
pub const MAX_TAG_KEY_LENGTH: usize = 128;

/// The longest tag value this service stores: "The tag value can be up to 256
/// characters" (same member doc as [`MAX_TAG_KEY_LENGTH`]), which the model's `TagValue`
/// shape declares as `length 0..256`.
///
/// Named here for the readers that state the bound; nothing enforces it in a handler,
/// because the schema walk refuses a longer value with `ValidationException` BEFORE the
/// handler runs.
pub const MAX_TAG_VALUE_LENGTH: usize = 256;

/// The key prefix AWS reserves for its own tags: a tag key "must not start with `aws:`"
/// (same member doc as [`MAX_TAG_KEY_LENGTH`]).
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Why `key` is not a tag key this service stores, or `None` when it is.
///
/// This is only what the schema walk does NOT already refuse. `TagMap`'s VALUE is a
/// constrained `TagValue` (`length 0..256`), so a value longer than
/// [`MAX_TAG_VALUE_LENGTH`] never reaches a handler; a map KEY's constraints are not part
/// of that walk, so `TagKey`'s own `length 1..128` is enforced here, together with the
/// reserved prefix the member's documentation states and the model carries no trait for.
/// The length counts CHARACTERS, as the documentation does.
///
/// The character set the same sentence lists (`[a-zA-Z+-=._:/]`) is NOT enforced: it omits
/// the digits and spaces AWS's tag-naming rules allow on every taggable resource, so
/// refusing on it would reject keys the service accepts. Every operation that writes tags
/// — `TagResource` and the `Tags` member of each create — refuses through this one
/// function, so the create side and the tagging side cannot drift apart.
pub fn tag_key_rejection(key: &str) -> Option<String> {
    if key.is_empty() {
        return Some("A tag key must not be empty.".to_string());
    }
    if key.starts_with(RESERVED_TAG_KEY_PREFIX) {
        return Some(format!(
            "Tag key {key:?} starts with {RESERVED_TAG_KEY_PREFIX:?}, which Amazon Web \
             Services reserves for its own tags."
        ));
    }
    let key_length = key.chars().count();
    if key_length > MAX_TAG_KEY_LENGTH {
        return Some(format!(
            "Tag key {key:?} is {key_length} characters long; a tag key may have at most \
             {MAX_TAG_KEY_LENGTH} characters."
        ));
    }
    None
}

/// The first key of `tags` this service will not store, as [`tag_key_rejection`] words it.
pub fn first_tag_key_rejection(tags: &HashMap<String, String>) -> Option<String> {
    // Sorted, so a request carrying two unacceptable keys is refused with the same
    // wording whichever order the map happened to iterate in.
    let mut keys: Vec<&String> = tags.keys().collect();
    keys.sort();
    keys.into_iter().find_map(|key| tag_key_rejection(key))
}

/// Why `parameters` is not an extension parameter map this service stores, or `None`
/// when it is.
///
/// The one rule the model states but cannot express as a constraint: "Dynamic parameters
/// can't be marked `Required`"
/// (`smithy_query(query="doc",shape="Parameter",member="Dynamic")`). Every operation that
/// writes a `ParameterMap` refuses through this one function, so the create side and the
/// update side cannot drift apart. The names are sorted, so a request carrying two
/// unacceptable parameters is refused with the same wording whichever order the map
/// happened to iterate in.
pub fn first_parameter_rejection(parameters: &HashMap<String, Parameter>) -> Option<String> {
    let mut names: Vec<&String> = parameters.keys().collect();
    names.sort();
    names.into_iter().find_map(|name| {
        let parameter = parameters.get(name)?;
        (parameter.dynamic && parameter.required).then(|| {
            format!("Parameter {name} is Dynamic, so it cannot also be marked Required.")
        })
    })
}

/// A resource one of this service's tag operations can address, as the resource portion
/// of its ARN names it.
///
/// One variant per resource whose tag map this service PERSISTS. AppConfig's ARN resource
/// portions are `/`-separated type/id pairs rooted at the owning application
/// (`application/<id>`, `application/<id>/environment/<id>`,
/// `application/<id>/configurationprofile/<id>`,
/// `application/<id>/environment/<id>/deployment/<number>`) or, for a resource that has no
/// owning application, at the resource type itself (`deploymentstrategy/<id>`,
/// `extension/<id>/<version>`), which is what [`parse_resource_arn`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedResource<'arn> {
    /// `application/<application id>`.
    Application { application_id: &'arn str },
    /// `application/<application id>/environment/<environment id>`.
    Environment {
        application_id: &'arn str,
        environment_id: &'arn str,
    },
    /// `application/<application id>/configurationprofile/<profile id>`.
    ConfigurationProfile {
        application_id: &'arn str,
        configuration_profile_id: &'arn str,
    },
    /// `application/<application id>/environment/<environment id>/deployment/<number>`.
    Deployment {
        application_id: &'arn str,
        environment_id: &'arn str,
        deployment_number: i32,
    },
    /// `deploymentstrategy/<deployment strategy id>`. A deployment strategy belongs to
    /// the account rather than to an application, so its resource portion is rooted at
    /// the type.
    DeploymentStrategy {
        deployment_strategy_id: &'arn str,
    },
    /// `extension/<extension id>/<version number>`. An extension belongs to the account
    /// rather than to an application, and each of its versions is addressed separately —
    /// which is the ARN `CreateExtension` answers with.
    Extension {
        extension_id: &'arn str,
        version_number: i32,
    },
}

/// The resource `resource_arn` addresses, or `None` when it addresses nothing whose tags
/// this service holds.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account>:<resource>`; the partition,
/// region and account sections are not discriminating here (this emulator presents ONE
/// account, [`::engine::EMULATOR_ACCOUNT_ID`], and its store is not partitioned by
/// region), so what is read is the service namespace — which must be
/// [`SERVICE_NAMESPACE`] — and the resource portion. An ARN naming an AppConfig resource
/// type whose tags this service does not persist (a hosted configuration version or an
/// extension association: no record this service stores for one carries a tag map)
/// answers `None`, exactly as an ARN naming a resource that does not exist does —
/// which includes a predefined `AppConfig.*` deployment strategy, since this service stores
/// no record for one.
pub fn parse_resource_arn(resource_arn: &str) -> Option<TaggedResource<'_>> {
    let mut sections = resource_arn.splitn(6, ':');
    let prefix = sections.next()?;
    let partition = sections.next()?;
    let service = sections.next()?;
    let _region = sections.next()?;
    let _account = sections.next()?;
    let resource = sections.next()?;
    // The model's `Arn` pattern allows an empty partition, so only a non-empty one is
    // checked against the `aws` family it constrains partitions to.
    if prefix != "arn" || service != SERVICE_NAMESPACE {
        return None;
    }
    if !partition.is_empty() && !partition.starts_with("aws") {
        return None;
    }

    let segments: Vec<&str> = resource.split('/').collect();
    let named = |id: &str| !id.is_empty();
    match segments.as_slice() {
        ["application", application_id] if named(application_id) => {
            Some(TaggedResource::Application { application_id })
        }
        ["application", application_id, "environment", environment_id]
            if named(application_id) && named(environment_id) =>
        {
            Some(TaggedResource::Environment {
                application_id,
                environment_id,
            })
        }
        ["application", application_id, "configurationprofile", configuration_profile_id]
            if named(application_id) && named(configuration_profile_id) =>
        {
            Some(TaggedResource::ConfigurationProfile {
                application_id,
                configuration_profile_id,
            })
        }
        ["application", application_id, "environment", environment_id, "deployment", number]
            if named(application_id) && named(environment_id) =>
        {
            Some(TaggedResource::Deployment {
                application_id,
                environment_id,
                // AppConfig numbers a deployment, so a resource portion whose last
                // segment is not a deployment number addresses no deployment.
                deployment_number: number.parse().ok()?,
            })
        }
        ["deploymentstrategy", deployment_strategy_id] if named(deployment_strategy_id) => {
            Some(TaggedResource::DeploymentStrategy {
                deployment_strategy_id,
            })
        }
        ["extension", extension_id, version] if named(extension_id) => {
            Some(TaggedResource::Extension {
                extension_id,
                // Every extension ARN names one version, so a resource portion whose
                // last segment is not a version number addresses no extension.
                version_number: version.parse().ok()?,
            })
        }
        _ => None,
    }
}


/// The description of each `DeploymentEvent` this service records.
///
/// AppConfig's own wording, which the model documents only by example ("Descriptions
/// include, but are not limited to, the following",
/// `smithy_query(query="doc",shape="DeploymentEvent",member="Description")`): the start and
/// completion texts are the recorded service's, which carry no trailing stop, and the
/// bake and percentage texts follow the same form.
const DESCRIPTION_DEPLOYMENT_STARTED: &str = "Deployment started";
/// See [`DESCRIPTION_DEPLOYMENT_STARTED`].
const DESCRIPTION_DEPLOYMENT_COMPLETED: &str = "Deployment completed";
/// See [`DESCRIPTION_DEPLOYMENT_STARTED`]. A deployment whose completion falls due at the
/// instant it was asked for took no time at all, which is what its event says.
const DESCRIPTION_DEPLOYMENT_COMPLETED_INSTANTLY: &str = "Deployment completed instantly";

/// The percentage of targets a finished rollout has reached, and the ceiling every growth
/// step is capped at: the maximum of the model's `Percentage` shape (`range 1..100`).
const FULL_PERCENTAGE: f32 = 100.0;

/// The most growth steps a rollout is split into.
///
/// The smallest growth factor the model's `Percentage` shape allows is 1%, which needs
/// 100 linear steps (and 7 exponential ones) to reach every target, so no strategy the
/// model can express is cut short by this bound — and the schedule's loop is bounded by
/// it rather than by a float comparison, so it terminates for any stored value.
const MAX_ROLLOUT_STEPS: usize = 100;

/// Lowercase alphanumerics, the alphabet the model's `Id` shape allows
/// (`^[a-z0-9]{4,7}$`).
const ID_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// Length of a minted id. Within `Id`'s `{4,7}` bound, and the width AppConfig's own
/// ids use.
const ID_LENGTH: usize = 7;

/// One row of AppConfig's predefined deployment-strategy catalog: a strategy every
/// account can deploy with, without first calling `CreateDeploymentStrategy`.
#[derive(Debug, Clone, Copy)]
pub struct PredefinedStrategy {
    /// The strategy id, which for a predefined strategy is also its name.
    pub id: &'static str,
    /// What the published table says the strategy is for.
    pub description: &'static str,
    /// Total time the rollout lasts.
    pub deployment_duration_in_minutes: i32,
    /// How the percentage of targets grows over that time.
    pub growth_type: &'static str,
    /// The percentage of targets each growth step adds.
    pub growth_factor: f32,
    /// Time AppConfig monitors the deployment after the rollout before completing it.
    pub final_bake_time_in_minutes: i32,
}

/// AppConfig's predefined deployment strategies, from the AWS AppConfig User Guide's
/// "Predefined deployment strategies" table — in the order that table publishes them,
/// which is the order a listing of the catalogue reports them in. Every column of that
/// table is a published AWS fact rather than a value this emulator may choose: the
/// `description` is the table's "Description" ("Quick", "Test/Demo", "AWS Recommended"),
/// and the deployment duration, growth type, growth factor and final bake time are the
/// table's own — `Linear20Percent` `Every6Minutes` adds 20% of targets every 6 minutes,
/// i.e. five steps across its 30-minute deployment, and then bakes for 30 minutes, while
/// `AppConfig.AllAtOnce` reaches every target at once and bakes for 10.
pub const PREDEFINED_DEPLOYMENT_STRATEGIES: &[PredefinedStrategy] = &[
    PredefinedStrategy {
        id: "AppConfig.AllAtOnce",
        description: "Quick",
        deployment_duration_in_minutes: 0,
        growth_type: GROWTH_LINEAR,
        growth_factor: 100.0,
        final_bake_time_in_minutes: 10,
    },
    PredefinedStrategy {
        id: "AppConfig.Linear50PercentEvery30Seconds",
        description: "Test/Demo",
        deployment_duration_in_minutes: 1,
        growth_type: GROWTH_LINEAR,
        growth_factor: 50.0,
        final_bake_time_in_minutes: 1,
    },
    PredefinedStrategy {
        id: "AppConfig.Canary10Percent20Minutes",
        description: "AWS Recommended",
        deployment_duration_in_minutes: 20,
        growth_type: GROWTH_EXPONENTIAL,
        growth_factor: 10.0,
        final_bake_time_in_minutes: 10,
    },
    PredefinedStrategy {
        id: "AppConfig.Linear20PercentEvery6Minutes",
        description: "AWS Recommended",
        deployment_duration_in_minutes: 30,
        growth_type: GROWTH_LINEAR,
        growth_factor: 20.0,
        final_bake_time_in_minutes: 30,
    },
];

/// The prefix a catalogue cursor key carries for a predefined strategy, and the one it
/// carries for a strategy this account created. `'0'` sorts before `'1'`, which is what
/// puts the published table ahead of the account's own strategies. See
/// [`ComAmazonawsAppconfigAmazonappconfigState::all_strategies`].
const PREDEFINED_STRATEGY_KEY_PREFIX: char = '0';
/// See [`PREDEFINED_STRATEGY_KEY_PREFIX`].
const CREATED_STRATEGY_KEY_PREFIX: char = '1';

/// A deployment strategy: the rollout schedule a deployment follows.
#[derive(Debug, Clone)]
pub struct DeploymentStrategy {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub deployment_duration_in_minutes: i32,
    pub growth_type: String,
    pub growth_factor: f32,
    pub final_bake_time_in_minutes: i32,
    pub replicate_to: String,
    /// The metadata `CreateDeploymentStrategy` was asked to assign to this strategy.
    /// Held like an application's or an environment's tags: the `DeploymentStrategy`
    /// shape carries none, so they are reported by the tagging operations rather than
    /// by the strategy readers.
    pub tags: HashMap<String, String>,
}

impl From<&PredefinedStrategy> for DeploymentStrategy {
    fn from(predefined: &PredefinedStrategy) -> Self {
        Self {
            id: predefined.id.to_string(),
            // A predefined strategy's name is its id; it is not replicated anywhere.
            name: predefined.id.to_string(),
            description: Some(predefined.description.to_string()),
            deployment_duration_in_minutes: predefined.deployment_duration_in_minutes,
            growth_type: predefined.growth_type.to_string(),
            growth_factor: predefined.growth_factor,
            final_bake_time_in_minutes: predefined.final_bake_time_in_minutes,
            replicate_to: REPLICATE_TO_NONE.to_string(),
            tags: HashMap::new(),
        }
    }
}

/// An AppConfig application: the organisational root that owns environments,
/// configuration profiles and the experiment definitions written under it.
#[derive(Debug, Clone)]
pub struct Application {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub tags: HashMap<String, String>,
    pub environments: HashMap<String, Environment>,
    pub profiles: HashMap<String, ConfigurationProfile>,
    /// Experiment definitions created under this application, by definition id.
    pub experiment_definitions: HashMap<String, ExperimentDefinition>,
}

impl Application {
    pub fn new(
        id: String,
        name: String,
        description: Option<String>,
        tags: HashMap<String, String>,
    ) -> Self {
        Self {
            id,
            name,
            description,
            tags,
            environments: HashMap::new(),
            profiles: HashMap::new(),
            experiment_definitions: HashMap::new(),
        }
    }

    /// The environment `identifier` names, by id or by name.
    ///
    /// The model's `Identifier` members documented as "the environment ID or name"
    /// accept either, so an id lookup is tried first and the name scan only answers
    /// when no environment holds that id. See [`resolve_by_name`] for how a name
    /// several resources share resolves.
    pub fn resolve_environment(&self, identifier: &str) -> Option<&Environment> {
        if let Some(environment) = self.environments.get(identifier) {
            return Some(environment);
        }
        resolve_by_name(self.environments.values(), identifier, |environment| {
            (&environment.name, &environment.id)
        })
    }

    /// The configuration profile `identifier` names, by id or by name. See
    /// [`Application::resolve_environment`].
    pub fn resolve_profile(&self, identifier: &str) -> Option<&ConfigurationProfile> {
        if let Some(profile) = self.profiles.get(identifier) {
            return Some(profile);
        }
        resolve_by_name(self.profiles.values(), identifier, |profile| {
            (&profile.name, &profile.id)
        })
    }

    /// The experiment definition `identifier` names, by id or by name. See
    /// [`Application::resolve_environment`].
    pub fn resolve_experiment_definition(&self, identifier: &str) -> Option<&ExperimentDefinition> {
        self.resolve_experiment_definition_id(identifier)
            .and_then(|id| self.experiment_definitions.get(&id))
    }

    /// The id of the experiment definition `identifier` names, so a caller that must
    /// MUTATE the record can look it up again mutably without re-running the name scan
    /// against a borrowed record.
    pub fn resolve_experiment_definition_id(&self, identifier: &str) -> Option<String> {
        if self.experiment_definitions.contains_key(identifier) {
            return Some(identifier.to_string());
        }
        resolve_by_name(
            self.experiment_definitions.values(),
            identifier,
            |definition| (&definition.name, &definition.id),
        )
        .map(|definition| definition.id.clone())
    }
}

/// The one resource of `candidates` whose name is `name`.
///
/// AWS documents no behaviour for a name several resources of one kind share — nothing
/// in this service makes a name unique except an experiment definition's, which
/// `CreateExperimentDefinition` refuses to duplicate — so the match with the
/// lexicographically smallest id is answered, which makes resolution deterministic
/// rather than dependent on the order a `HashMap` happens to iterate in.
fn resolve_by_name<'store, T: 'store>(
    candidates: impl Iterator<Item = &'store T>,
    name: &str,
    name_and_id: impl Fn(&T) -> (&String, &String),
) -> Option<&'store T> {
    candidates
        .filter(|candidate| name_and_id(candidate).0 == name)
        .min_by(|left, right| name_and_id(left).1.cmp(name_and_id(right).1))
}

/// The key the control treatment carries. See [`treatment_key`].
pub const CONTROL_TREATMENT_KEY: &str = "C";

/// The key this service gives the `index`-th treatment of a definition, counting from
/// `0`.
///
/// The model's `TreatmentKey` is `^[a-zA-Z0-9]{1,8}$` and the `Treatment` shape carries
/// it while `TreatmentInput` does not, so the key is the SERVICE's to mint; AWS
/// documents no composition for it. It is derived from the treatment's place in the
/// definition — the control is [`CONTROL_TREATMENT_KEY`] and the treatments compared
/// against it are `T1`, `T2`, … in the order the request listed them — so it is unique
/// within a definition, stays within the modelled pattern for every list the model
/// allows (`TreatmentInputList` holds at most 5), and is stable across every read.
pub fn treatment_key(index: usize) -> String {
    format!("T{}", index + 1)
}

/// An experiment definition: the purpose, scope and treatments of an experiment over
/// one feature flag in one environment.
///
/// `created_at` and `updated_at` are the instants this service wrote the record, and
/// `status` is the definition's own lifecycle: a definition nothing has run yet is
/// [`EXPERIMENT_DEFINITION_IDLE`], a started run makes it
/// [`EXPERIMENT_DEFINITION_ACTIVE`] and an archiving delete makes it
/// [`EXPERIMENT_DEFINITION_ARCHIVED`]. No part of it is owed by the clock, so there is
/// nothing to materialize: a definition sits in its status until a call moves it.
#[derive(Debug, Clone)]
pub struct ExperimentDefinition {
    pub id: String,
    pub application_id: String,
    pub configuration_profile_id: String,
    pub environment_id: String,
    pub name: String,
    pub flag_key: String,
    pub audience_rule: String,
    pub audience_description: Option<String>,
    pub hypothesis: Option<String>,
    pub launch_criteria: Option<String>,
    pub status: String,
    /// The baseline treatment, keyed like every other treatment.
    pub control: Treatment,
    /// The treatments compared against the control, in the order the request listed
    /// them: the model's `TreatmentList` is ordered, so the order a caller wrote is the
    /// order every reader reports.
    pub treatments: Vec<Treatment>,
    /// The KMS key an experiment RUN of this definition encrypts its data with; set by
    /// the run, not by the definition's own create, which carries no such member.
    pub kms_key_identifier: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub tags: HashMap<String, String>,
}

/// A deployment group within an application, and the deployments made into it.
#[derive(Debug, Clone)]
pub struct Environment {
    pub id: String,
    pub application_id: String,
    pub name: String,
    pub description: Option<String>,
    pub state: String,
    pub monitors: Vec<Monitor>,
    pub tags: HashMap<String, String>,
    pub deployments: HashMap<i32, Deployment>,
    pub next_deployment_number: i32,
}

impl Environment {
    pub fn new(
        id: String,
        application_id: String,
        name: String,
        description: Option<String>,
        monitors: Vec<Monitor>,
        tags: HashMap<String, String>,
    ) -> Self {
        Self {
            id,
            application_id,
            name,
            description,
            // A new environment has nothing deploying into it yet.
            state: ENVIRONMENT_READY_FOR_DEPLOYMENT.to_string(),
            monitors,
            tags,
            deployments: HashMap::new(),
            // AppConfig numbers an environment's deployments from 1.
            next_deployment_number: 1,
        }
    }

    /// Bring this environment, and every deployment in it, up to the state the clock
    /// already owes at `now`: an environment is `DEPLOYING` exactly while one of its
    /// deployments is still rolling out or baking, and `READY_FOR_DEPLOYMENT` once
    /// none is.
    pub fn materialize(&mut self, now: SystemTime) {
        for deployment in self.deployments.values_mut() {
            deployment.materialize(now);
        }
        self.state = if self.deployments.values().any(Deployment::is_in_flight) {
            ENVIRONMENT_DEPLOYING.to_string()
        } else {
            ENVIRONMENT_READY_FOR_DEPLOYMENT.to_string()
        };
    }

    /// The deployment still rolling out or baking in this environment, if any. A
    /// second deployment into the same environment conflicts with it.
    pub fn deployment_in_flight(&self) -> Option<&Deployment> {
        self.deployments.values().find(|d| d.is_in_flight())
    }

    /// Store a new deployment and answer the stored record.
    ///
    /// Only [`MAX_RETAINED_DEPLOYMENTS_PER_ENVIRONMENT`] records are kept: the oldest
    /// beyond that bound are dropped first, and only ones the deployment has already
    /// finished with — a deployment still rolling out or baking is the one a caller is
    /// watching, so it is never evicted, and an environment holding nothing but
    /// in-flight deployments keeps them all (at most one, since `StartDeployment`
    /// refuses a second). The record being stored is never dropped either: pruning
    /// drops lower deployment numbers and this is the highest, so the entry it is
    /// written into is always vacant.
    pub fn record_deployment(&mut self, deployment: Deployment) -> &Deployment {
        let number = deployment.deployment_number;
        while self.deployments.len() >= MAX_RETAINED_DEPLOYMENTS_PER_ENVIRONMENT {
            let Some(oldest_finished) = self
                .deployments
                .values()
                .filter(|held| !held.is_in_flight())
                .map(|held| held.deployment_number)
                .min()
            else {
                break;
            };
            self.deployments.remove(&oldest_finished);
        }
        self.deployments.entry(number).or_insert(deployment)
    }

    /// The highest deployment number this environment has issued; `0` before the
    /// first deployment.
    pub fn latest_deployment_number(&self) -> i32 {
        self.next_deployment_number - 1
    }
}

/// A configuration profile: where a configuration lives, and the versions of it this
/// service hosts when that location is AppConfig's own hosted store.
#[derive(Debug, Clone)]
pub struct ConfigurationProfile {
    pub id: String,
    pub application_id: String,
    pub name: String,
    pub description: Option<String>,
    pub location_uri: String,
    pub retrieval_role_arn: Option<String>,
    pub validators: Vec<Validator>,
    /// The `Type` the caller declared (`AWS.Freeform`, `AWS.AppConfig.FeatureFlags`),
    /// or `None` when it declared none: the model lists the values it accepts but
    /// states no default, so none is substituted.
    pub profile_type: Option<String>,
    pub kms_key_identifier: Option<String>,
    pub tags: HashMap<String, String>,
    pub hosted_versions: BTreeMap<i32, HostedConfigurationVersion>,
    pub next_version_number: i32,
}

impl ConfigurationProfile {
    /// The highest hosted version number written to this profile; `0` before the
    /// first write.
    pub fn latest_version_number(&self) -> i32 {
        self.next_version_number - 1
    }
}

/// An extension: the logic AppConfig injects at an action point of its own workflow.
///
/// An extension is VERSIONED. Its name and its system-generated id are properties of the
/// extension as a whole ("Extension versions use the same name", as the model documents
/// on `CreateExtension`'s `Name` member); the description, the actions, the parameters
/// and the tags belong to one version, because `CreateExtension` against a name that
/// already exists writes a new version carrying its own copy of all four.
#[derive(Debug, Clone)]
pub struct Extension {
    pub id: String,
    pub name: String,
    /// Every version of this extension, by version number. Ordered, so the latest
    /// version is the last key rather than a separately maintained counter that could
    /// disagree with the map.
    pub versions: BTreeMap<i32, ExtensionVersion>,
}

impl Extension {
    /// A brand-new extension holding only `version`.
    pub fn new(id: String, name: String, version: ExtensionVersion) -> Self {
        Self {
            id,
            name,
            versions: BTreeMap::from([(version.version_number, version)]),
        }
    }

    /// The highest version number this extension holds; `0` for an extension with no
    /// version at all, which is also what a name nobody has created reports, so the two
    /// compare alike against `CreateExtension`'s `LatestVersionNumber`.
    pub fn latest_version_number(&self) -> i32 {
        self.versions.keys().next_back().copied().unwrap_or(0)
    }

    /// The highest-numbered version, which is the one a reader naming no version gets:
    /// "If no version number was defined, AppConfig uses the highest version"
    /// (`smithy_query(query="doc",shape="GetExtensionRequest",member="VersionNumber")`).
    pub fn latest_version(&self) -> Option<&ExtensionVersion> {
        self.versions.values().next_back()
    }
}

/// One version of an extension, exactly as the `CreateExtension` call that wrote it
/// described it.
#[derive(Debug, Clone)]
pub struct ExtensionVersion {
    pub version_number: i32,
    /// The ARN of THIS version, minted when it was created and answered unchanged on
    /// every later read.
    pub arn: String,
    pub description: Option<String>,
    /// The action list bound to each action point, keyed by the `ActionPoint` value.
    pub actions: HashMap<String, Vec<Action>>,
    pub parameters: HashMap<String, Parameter>,
    /// The metadata `CreateExtension` was asked to assign to this extension version.
    /// Held on the record like an application's tags; the `Extension` shape carries no
    /// `Tags` member, so they are reported by the tagging operations.
    pub tags: HashMap<String, String>,
}

/// One version of a configuration held in AppConfig's hosted store, byte for byte as
/// the caller wrote it.
#[derive(Debug, Clone)]
pub struct HostedConfigurationVersion {
    pub version_number: i32,
    pub description: Option<String>,
    pub content: Vec<u8>,
    pub content_type: String,
    pub version_label: Option<String>,
}

/// A deployment of one configuration version into one environment.
#[derive(Debug, Clone)]
pub struct Deployment {
    pub deployment_number: i32,
    pub application_id: String,
    pub environment_id: String,
    pub deployment_strategy_id: String,
    pub configuration_profile_id: String,
    pub configuration_name: String,
    pub configuration_location_uri: String,
    pub configuration_version: String,
    pub description: Option<String>,
    pub deployment_duration_in_minutes: i32,
    pub growth_type: String,
    pub growth_factor: f32,
    pub final_bake_time_in_minutes: i32,
    pub state: String,
    pub percentage_complete: f32,
    /// The `DeploymentType` this deployment reports: [`DEPLOYMENT_TYPE_USER`], because
    /// every deployment this service holds was started by a caller's own
    /// `StartDeployment` call.
    pub deployment_type: String,
    /// The extensions this deployment invoked, which the `Deployment` document reports as
    /// `AppliedExtensions`: "the extensions that were previously associated to the
    /// configuration profile, environment, or the application when `StartDeployment` was
    /// called".
    ///
    /// Recorded when the deployment starts, so a later read reports what was applied THEN
    /// rather than what is associated now. It is empty for every deployment this emulator
    /// starts, and that is a computed fact rather than a placeholder: an extension reaches
    /// a deployment only through an extension ASSOCIATION, and every operation that writes
    /// one (`CreateExtensionAssociation`, `UpdateExtensionAssociation`) still answers 501,
    /// so no association exists for a deployment to invoke.
    pub applied_extensions: Vec<AppliedExtension>,
    pub started_at: SystemTime,
    pub completed_at: Option<SystemTime>,
    pub event_log: Vec<DeploymentEvent>,
    pub kms_key_identifier: Option<String>,
    pub version_label: Option<String>,
    pub tags: HashMap<String, String>,
}

/// What `StartDeployment` persists about a new deployment, beside the strategy that
/// paces it. Grouped so [`Deployment::start`] takes one argument per concern instead
/// of a dozen positional strings.
#[derive(Debug, Clone)]
pub struct NewDeployment {
    pub deployment_number: i32,
    pub application_id: String,
    pub environment_id: String,
    pub configuration_profile_id: String,
    pub configuration_name: String,
    pub configuration_location_uri: String,
    pub configuration_version: String,
    pub description: Option<String>,
    pub kms_key_identifier: Option<String>,
    pub version_label: Option<String>,
    pub tags: HashMap<String, String>,
}

impl Deployment {
    /// Start a deployment at `started_at`, paced by `strategy`.
    ///
    /// The deployment begins at 0% of its targets with one `DEPLOYMENT_STARTED` event,
    /// because nothing has rolled out yet at the instant it was asked for; every later
    /// state, percentage and event is owed by the clock and materialised by
    /// [`Deployment::materialize`], which is called once here so a strategy whose rollout
    /// window and bake time are both zero — which has nothing left to wait for — is
    /// already `COMPLETE` when `StartDeployment` answers.
    pub fn start(new: NewDeployment, strategy: &DeploymentStrategy, started_at: SystemTime) -> Self {
        let mut deployment = Self {
            deployment_number: new.deployment_number,
            application_id: new.application_id,
            environment_id: new.environment_id,
            deployment_strategy_id: strategy.id.clone(),
            configuration_profile_id: new.configuration_profile_id,
            configuration_name: new.configuration_name,
            configuration_location_uri: new.configuration_location_uri,
            configuration_version: new.configuration_version,
            description: new.description,
            deployment_duration_in_minutes: strategy.deployment_duration_in_minutes,
            growth_type: strategy.growth_type.clone(),
            growth_factor: strategy.growth_factor,
            final_bake_time_in_minutes: strategy.final_bake_time_in_minutes,
            state: DEPLOYMENT_DEPLOYING.to_string(),
            percentage_complete: 0.0,
            deployment_type: DEPLOYMENT_TYPE_USER.to_string(),
            // Nothing to apply: see the field's own note.
            applied_extensions: Vec::new(),
            started_at,
            completed_at: None,
            event_log: vec![deployment_event(
                EVENT_DEPLOYMENT_STARTED,
                TRIGGERED_BY_USER,
                DESCRIPTION_DEPLOYMENT_STARTED.to_string(),
                started_at,
            )],
            kms_key_identifier: new.kms_key_identifier,
            version_label: new.version_label,
            tags: new.tags,
        };
        // Whatever the clock already owes this deployment at the instant it started: for a
        // strategy with a rollout window that is nothing at all, and for one with none it
        // is the whole deployment.
        deployment.materialize(started_at);
        deployment
    }

    /// Whether the deployment is still rolling out or baking, i.e. is in one of the
    /// `DeploymentState` values it can still leave on its own.
    pub fn is_in_flight(&self) -> bool {
        matches!(
            self.state.as_str(),
            DEPLOYMENT_DEPLOYING | DEPLOYMENT_BAKING
        )
    }

    /// How long the rollout itself lasts: the strategy's deployment duration.
    ///
    /// Not floored: a strategy declaring a duration of 0 minutes (`AppConfig.AllAtOnce`,
    /// or any created strategy with `DeploymentDurationInMinutes` 0) reaches every target
    /// at the instant the deployment starts, which is what AppConfig answers for one —
    /// with no bake time to follow, `StartDeployment` reports it `COMPLETE`.
    fn rollout_duration(&self) -> Duration {
        Duration::from_secs(60 * self.deployment_duration_in_minutes.max(0) as u64)
    }

    /// How long AppConfig monitors the deployment after the rollout before completing
    /// it: the strategy's final bake time.
    fn bake_duration(&self) -> Duration {
        Duration::from_secs(60 * self.final_bake_time_in_minutes.max(0) as u64)
    }

    /// The instant the rollout finishes and the final bake begins.
    fn rollout_end(&self) -> SystemTime {
        self.started_at + self.rollout_duration()
    }

    /// The instant the deployment becomes `COMPLETE`.
    fn completion_due(&self) -> SystemTime {
        self.rollout_end() + self.bake_duration()
    }

    /// The rollout schedule: for each growth step the strategy performs, the instant
    /// it falls due and the cumulative percentage of targets it reaches.
    ///
    /// `LINEAR` adds `GrowthFactor` percent of targets per step; `EXPONENTIAL` reaches
    /// `GrowthFactor * 2^N` percent at step `N`, the formula the model's `GrowthType`
    /// documentation gives. A step is one INTERVAL of the rollout window, so the steps
    /// are spread evenly across that window with the first one due at the END of the
    /// first interval and the last exactly at the window's end:
    /// `AppConfig.Linear50PercentEvery30Seconds` reaches 50% thirty seconds in and 100% at
    /// sixty, and a deployment just started has reached nobody yet.
    ///
    /// The step count is bounded by [`MAX_ROLLOUT_STEPS`] and the loop runs over a
    /// fixed range, so this terminates whatever the stored growth factor is — a growth
    /// factor is request-derived for a caller-created strategy, and a non-finite one
    /// survives `f32::clamp`, which propagates NaN instead of mapping it into range.
    fn rollout_schedule(&self) -> Vec<(SystemTime, f32)> {
        // A growth factor that is not a finite percentage cannot pace anything, so it
        // is normalised to a single-step rollout before it reaches the loop below.
        let growth_factor = if self.growth_factor.is_finite() {
            self.growth_factor.clamp(1.0, FULL_PERCENTAGE)
        } else {
            FULL_PERCENTAGE
        };

        let mut percentages: Vec<f32> = Vec::with_capacity(MAX_ROLLOUT_STEPS);
        for step in 0..MAX_ROLLOUT_STEPS {
            let multiplier = if self.growth_type == GROWTH_EXPONENTIAL {
                // `2^step`, saturating at infinity, which `min(100.0)` then clamps.
                2f32.powi(i32::try_from(step).unwrap_or(i32::MAX))
            } else {
                // Step `n` has added the growth factor `n + 1` times.
                step as f32 + 1.0
            };
            let reached = (growth_factor * multiplier).min(FULL_PERCENTAGE);
            percentages.push(reached);
            if reached >= FULL_PERCENTAGE {
                break;
            }
        }

        let rollout = self.rollout_duration();
        let steps = percentages.len() as f64;
        percentages
            .into_iter()
            .enumerate()
            .map(|(index, reached)| {
                let due = self.started_at + rollout.mul_f64((index as f64 + 1.0) / steps);
                (due, reached)
            })
            .collect()
    }

    /// Bring the deployment up to the state the clock already owes it at `now`.
    ///
    /// Every transition this applies was due at an instant that has passed, whether or
    /// not anybody asked, so a reporting operation may call it: it cannot make a later
    /// caller see anything other than what the clock had already made true.
    pub fn materialize(&mut self, now: SystemTime) {
        if !self.is_in_flight() {
            return;
        }

        // A `PERCENTAGE_UPDATED` event records a growth interval that ELAPSED, so a step
        // falling due at the start instant itself — every step of a strategy with no
        // rollout window at all — records none.
        for (due, reached) in self.rollout_schedule() {
            if due > self.started_at && due <= now && reached > self.percentage_complete {
                self.percentage_complete = reached;
                self.event_log.push(deployment_event(
                    EVENT_PERCENTAGE_UPDATED,
                    TRIGGERED_BY_APPCONFIG,
                    format!("Configuration available to {reached}% of clients"),
                    due,
                ));
            }
        }

        let rollout_end = self.rollout_end();
        if now >= rollout_end {
            // The rollout window is over, so every target holds the configuration —
            // including when the window was empty and no interval elapsed on the way.
            self.percentage_complete = FULL_PERCENTAGE;
        }
        if self.state == DEPLOYMENT_DEPLOYING
            && now >= rollout_end
            && !self.bake_duration().is_zero()
        {
            self.state = DEPLOYMENT_BAKING.to_string();
            self.event_log.push(deployment_event(
                EVENT_BAKE_TIME_STARTED,
                TRIGGERED_BY_APPCONFIG,
                format!(
                    "Deployment bake time started for {} minute(s)",
                    self.final_bake_time_in_minutes
                ),
                rollout_end,
            ));
        }

        let completion_due = self.completion_due();
        if now >= completion_due {
            self.state = DEPLOYMENT_COMPLETE.to_string();
            self.completed_at = Some(completion_due);
            // A deployment whose completion falls due at the instant it was asked for
            // took no time at all, which is what its event says.
            let description = if completion_due == self.started_at {
                DESCRIPTION_DEPLOYMENT_COMPLETED_INSTANTLY
            } else {
                DESCRIPTION_DEPLOYMENT_COMPLETED
            };
            self.event_log.push(deployment_event(
                EVENT_DEPLOYMENT_COMPLETED,
                TRIGGERED_BY_APPCONFIG,
                description.to_string(),
                completion_due,
            ));
        }
    }

    /// This deployment's events as the model describes `EventLog`: "The most recent
    /// events are displayed first."
    pub fn event_log_newest_first(&self) -> Vec<DeploymentEvent> {
        self.event_log.iter().rev().cloned().collect()
    }
}

/// One deployment event, as the model's `DeploymentEvent` shape.
fn deployment_event(
    event_type: &str,
    triggered_by: &str,
    description: String,
    occurred_at: SystemTime,
) -> DeploymentEvent {
    DeploymentEvent {
        event_type: Some(event_type.to_string()),
        triggered_by: Some(triggered_by.to_string()),
        description: Some(description),
        action_invocations: None,
        occurred_at: Some(to_date_time(occurred_at)),
    }
}

/// An instant as the `Iso8601DateTime` the model binds these timestamps to.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

// =============================================================================
// `ResourceNotFoundException` messages
// =============================================================================

/// How AppConfig words a `ResourceNotFoundException`, as the recorded refusals show it:
///
/// ```text
/// <Resource kind> with <Label> <value>[, <Label> <value>]… and <Label> <value> \
///     could not be found for account <account id>
/// ```
///
/// The sentence is DERIVED from the request — the kind of resource that was looked up,
/// every identifier the request used to scope that lookup under AppConfig's own label
/// for it, and the account the lookup was scoped to — so each builder below takes the
/// request's values and nothing is stored. The account is always
/// [`::engine::emulator_account_id`], the single account a generated emulator presents.
///
/// `terminator` is the sentence's final punctuation: the recorded messages end with a
/// full stop for every resource except the hosted configuration version, whose recorded
/// message ends without one, so it is passed in per resource rather than appended here.
fn not_found_message(kind: &str, identifiers: &[(&str, &str)], terminator: &str) -> String {
    format!(
        "{kind} with {} could not be found for account {}{terminator}",
        identifier_phrase(identifiers),
        ::engine::emulator_account_id()
    )
}

/// The identifier clause of a [`not_found_message`]: each `<Label> <value>` pair in the
/// order the resource's path nests them, comma-separated with `and` before the last and
/// the Oxford comma kept from three identifiers on — the form the recorded hosted
/// configuration version message uses.
fn identifier_phrase(identifiers: &[(&str, &str)]) -> String {
    let parts: Vec<String> = identifiers
        .iter()
        .map(|(label, value)| format!("{label} {value}"))
        .collect();
    match parts.as_slice() {
        [] => String::new(),
        [only] => only.clone(),
        [first, second] => format!("{first} and {second}"),
        [leading @ .., last] => format!("{}, and {last}", leading.join(", ")),
    }
}

/// AppConfig ends a not-found message with a full stop …
const NOT_FOUND_FULL_STOP: &str = ".";
/// … except the hosted configuration version's, which the recorded refusal ends without
/// one.
const NOT_FOUND_NO_FULL_STOP: &str = "";

/// `Message` for an application the account does not hold.
pub fn application_not_found(application_id: &str) -> String {
    not_found_message(
        "Application",
        &[("Id", application_id)],
        NOT_FOUND_FULL_STOP,
    )
}

/// `Message` for an environment the named application does not hold.
pub fn environment_not_found(application_id: &str, environment_id: &str) -> String {
    not_found_message(
        "Environment",
        &[
            ("Application Id", application_id),
            ("Environment Id", environment_id),
        ],
        NOT_FOUND_FULL_STOP,
    )
}

/// `Message` for a configuration profile the named application does not hold.
pub fn configuration_profile_not_found(
    application_id: &str,
    configuration_profile_id: &str,
) -> String {
    not_found_message(
        "Configuration Profile",
        &[
            ("Application Id", application_id),
            ("Configuration Profile Id", configuration_profile_id),
        ],
        NOT_FOUND_FULL_STOP,
    )
}

/// `Message` for a hosted configuration version the named profile does not hold.
pub fn hosted_configuration_version_not_found(
    application_id: &str,
    configuration_profile_id: &str,
    version_number: i32,
) -> String {
    not_found_message(
        "Hosted Configuration Version",
        &[
            ("Application Id", application_id),
            ("Configuration Profile Id", configuration_profile_id),
            ("Version", &version_number.to_string()),
        ],
        NOT_FOUND_NO_FULL_STOP,
    )
}

/// `Message` for a deployment number the named environment never issued.
pub fn deployment_not_found(
    application_id: &str,
    environment_id: &str,
    deployment_number: i32,
) -> String {
    not_found_message(
        "Deployment",
        &[
            ("Application Id", application_id),
            ("Environment Id", environment_id),
            ("Deployment Number", &deployment_number.to_string()),
        ],
        NOT_FOUND_FULL_STOP,
    )
}

/// `Message` for a deployment strategy that is neither created nor predefined.
pub fn deployment_strategy_not_found(deployment_strategy_id: &str) -> String {
    not_found_message(
        "Deployment Strategy",
        &[("Id", deployment_strategy_id)],
        NOT_FOUND_FULL_STOP,
    )
}

/// `Message` for an extension no `ExtensionIdentifier` resolves to.
pub fn extension_not_found(extension_identifier: &str) -> String {
    not_found_message(
        "Extension",
        &[("Id", extension_identifier)],
        NOT_FOUND_FULL_STOP,
    )
}

/// `Message` for a `ResourceArn` that addresses no resource this service holds — the
/// identifier the tagging operations scope their lookup with.
pub fn resource_arn_not_found(resource_arn: &str) -> String {
    not_found_message("Resource", &[("Arn", resource_arn)], NOT_FOUND_FULL_STOP)
}

/// `Message` for a version an extension does not hold.
pub fn extension_version_not_found(extension_identifier: &str, version_number: i32) -> String {
    not_found_message(
        "Extension",
        &[
            ("Id", extension_identifier),
            ("Version Number", &version_number.to_string()),
        ],
        NOT_FOUND_FULL_STOP,
    )
}

// =============================================================================
// Account-level settings
// =============================================================================

/// AppConfig's documented default protection period: "The default interval specified by
/// `ProtectionPeriodInMinutes` is 60", from the model's `DeletionProtectionSettings`
/// documentation.
pub const DEFAULT_PROTECTION_PERIOD_IN_MINUTES: i32 = 60;

/// The account's AppConfig settings: what `GetAccountSettings` reports and what
/// `UpdateAccountSettings` writes.
///
/// An account that has never called `UpdateAccountSettings` is not unconfigured — it is
/// running on AppConfig's documented defaults — so [`Default`] is those defaults rather
/// than absent values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSettingsRecord {
    /// Whether deletion protection is on. Off until a caller turns it on: AppConfig
    /// documents the protection period's default but describes deletion protection
    /// itself as something a caller configures.
    pub deletion_protection_enabled: bool,
    /// The interval deletion protection watches for configuration reads over, in
    /// minutes. Defaults to [`DEFAULT_PROTECTION_PERIOD_IN_MINUTES`]; the model's
    /// `DeletionProtectionDuration` bounds a written value to 15..=1440, which the
    /// schema walk enforces before any handler runs.
    pub protection_period_in_minutes: i32,
    /// Whether vended metrics are enabled, or `None` while nobody has set them: the
    /// model states no default for `VendedMetricsSettings$Enabled`, so none is
    /// substituted and the member is not reported until it has been written.
    pub vended_metrics_enabled: Option<bool>,
}

impl Default for AccountSettingsRecord {
    fn default() -> Self {
        Self {
            deletion_protection_enabled: false,
            protection_period_in_minutes: DEFAULT_PROTECTION_PERIOD_IN_MINUTES,
            vended_metrics_enabled: None,
        }
    }
}

impl AccountSettingsRecord {
    /// This record's deletion protection as the model's `DeletionProtectionSettings`.
    ///
    /// Both members always have a value to report: `Enabled` is a flag, and
    /// `ProtectionPeriodInMinutes` carries AppConfig's documented default until a write
    /// replaces it.
    pub fn deletion_protection(&self) -> DeletionProtectionSettings {
        DeletionProtectionSettings {
            enabled: Some(self.deletion_protection_enabled),
            protection_period_in_minutes: Some(self.protection_period_in_minutes),
        }
    }

    /// This record's vended metrics as the model's `VendedMetricsSettings`, or `None`
    /// while nobody has set them: the model states no default for
    /// `VendedMetricsSettings$Enabled`, so reporting one would be fabricating it.
    pub fn vended_metrics(&self) -> Option<VendedMetricsSettings> {
        self.vended_metrics_enabled
            .map(|enabled| VendedMetricsSettings {
                enabled: Some(enabled),
            })
    }

    /// Apply one `UpdateAccountSettings` request to this record.
    ///
    /// `UpdateAccountSettings` is a `PATCH`: a member the request omits keeps the value
    /// the account already holds, and that applies inside `DeletionProtection` too —
    /// naming only `Enabled` leaves the stored protection period alone. The written
    /// values are already within the model's bounds, because the schema walk refuses a
    /// `ProtectionPeriodInMinutes` outside `DeletionProtectionDuration`'s 15..=1440
    /// range before this handler runs.
    pub fn apply(
        &mut self,
        deletion_protection: Option<&DeletionProtectionSettings>,
        vended_metrics: Option<&VendedMetricsSettings>,
    ) {
        if let Some(deletion_protection) = deletion_protection {
            if let Some(enabled) = deletion_protection.enabled {
                self.deletion_protection_enabled = enabled;
            }
            if let Some(period) = deletion_protection.protection_period_in_minutes {
                self.protection_period_in_minutes = period;
            }
        }
        if let Some(vended_metrics) = vended_metrics {
            if let Some(enabled) = vended_metrics.enabled {
                self.vended_metrics_enabled = Some(enabled);
            }
        }
    }
}

/// Per-service state for com-amazonaws-appconfig-amazonappconfig. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAppconfigAmazonappconfigState {
    /// Every application, by application id.
    pub applications: HashMap<String, Application>,
    /// This account's AppConfig settings, as `UpdateAccountSettings` last wrote them —
    /// [`AccountSettingsRecord::default`] (AppConfig's documented defaults) until it
    /// does. Account-scoped rather than per-application: the settings surface is
    /// `/settings`, with no resource in its path.
    pub account_settings: AccountSettingsRecord,
    /// Deployment strategies this account created, by strategy id.
    ///
    /// PRIVATE on purpose: [`Self::try_insert_strategy`] is the only way in, so the
    /// [`MAX_DEPLOYMENT_STRATEGIES`] bound cannot be bypassed by a handler mutating the
    /// map directly. AppConfig's predefined `AppConfig.*` strategies are not in here;
    /// [`Self::resolve_strategy`] answers for those too.
    deployment_strategies: HashMap<String, DeploymentStrategy>,
    /// Counter behind [`Self::mint_id`]; never answered to a caller.
    id_sequence: u64,
    /// Every extension this account created, by extension id.
    ///
    /// PRIVATE on purpose, for the same reason the strategy map is:
    /// [`Self::try_insert_extension`] and [`Self::try_insert_extension_version`] are the
    /// only ways in, so the [`MAX_EXTENSIONS`] and [`MAX_EXTENSION_VERSIONS`] bounds
    /// cannot be bypassed by a handler mutating the map directly. AppConfig's own
    /// AWS-authored `AWS.*` extensions are not in here: this service stores no record
    /// for one.
    extensions: HashMap<String, Extension>,
    /// Bytes of hosted configuration content this service is holding, across every
    /// configuration profile. Maintained by
    /// [`Self::reserve_hosted_configuration_bytes`] and bounded by
    /// [`MAX_TOTAL_HOSTED_CONFIGURATION_BYTES`].
    hosted_configuration_bytes: usize,
}

impl ComAmazonawsAppconfigAmazonappconfigState {
    /// Mint a fresh resource id matching the model's `Id` shape (`^[a-z0-9]{4,7}$`),
    /// unique across every resource this service holds.
    pub fn mint_id(&mut self) -> String {
        loop {
            self.id_sequence = self.id_sequence.wrapping_add(1);
            let candidate = encode_id(self.id_sequence);
            if !self.id_taken(&candidate) {
                return candidate;
            }
        }
    }

    /// Whether any resource already answers to `id`.
    fn id_taken(&self, id: &str) -> bool {
        self.deployment_strategies.contains_key(id)
            || self.applications.contains_key(id)
            || self.extensions.contains_key(id)
            || self.applications.values().any(|application| {
                application.environments.contains_key(id)
                    || application.profiles.contains_key(id)
                    || application.experiment_definitions.contains_key(id)
            })
    }

    /// The application `identifier` names, by id or by name: the model's
    /// `ApplicationIdentifier` members are documented as "the application ID or name",
    /// so either resolves, an id first. See [`resolve_by_name`] for how a name several
    /// applications share resolves.
    pub fn resolve_application(&self, identifier: &str) -> Option<&Application> {
        self.resolve_application_id(identifier)
            .and_then(|id| self.applications.get(&id))
    }

    /// The id of the application `identifier` names, for a caller that goes on to
    /// mutate the record. See [`Self::resolve_application`].
    pub fn resolve_application_id(&self, identifier: &str) -> Option<String> {
        if self.applications.contains_key(identifier) {
            return Some(identifier.to_string());
        }
        resolve_by_name(self.applications.values(), identifier, |application| {
            (&application.name, &application.id)
        })
        .map(|application| application.id.clone())
    }

    /// The extension `identifier` names, and the version number the identifier itself
    /// pins, or `None` when this service holds no such extension.
    ///
    /// `identifier` is "the name, the ID, or the Amazon Resource Name (ARN) of the
    /// extension"
    /// (`smithy_query(query="doc",shape="GetExtensionRequest",member="ExtensionIdentifier")`),
    /// so all three forms resolve here and every operation that takes an
    /// `ExtensionIdentifier` resolves it the same way. An ARN addresses ONE version, so
    /// it pins that version number; an id or a name addresses the extension as a whole
    /// and pins none.
    pub fn resolve_extension(&self, identifier: &str) -> Option<(&Extension, Option<i32>)> {
        if let Some(by_id) = self.extensions.get(identifier) {
            return Some((by_id, None));
        }
        if let Some(TaggedResource::Extension {
            extension_id,
            version_number,
        }) = parse_resource_arn(identifier)
        {
            return self
                .extensions
                .get(extension_id)
                .map(|extension| (extension, Some(version_number)));
        }
        self.extensions
            .values()
            .find(|extension| extension.name == identifier)
            .map(|extension| (extension, None))
    }

    /// The extension stored under `extension_id`, for a caller that already holds the
    /// minted id rather than a caller-supplied identifier.
    pub fn extension(&self, extension_id: &str) -> Option<&Extension> {
        self.extensions.get(extension_id)
    }

    /// The extension carrying `name`, or `None` when no extension does. Extension names
    /// are unique per account ("Each extension name in your account must be unique", as
    /// the model documents on `CreateExtension`'s `Name` member), so at most one answers.
    pub fn extension_named(&self, name: &str) -> Option<&Extension> {
        self.extensions
            .values()
            .find(|extension| extension.name == name)
    }

    /// Every extension this account created, for a reader that must report them without
    /// reaching into the map.
    pub fn created_extensions(&self) -> impl Iterator<Item = &Extension> {
        self.extensions.values()
    }

    /// Store a brand-new extension, holding its first version.
    ///
    /// Answers `false`, having stored nothing, when this service already holds
    /// [`MAX_EXTENSIONS`] of them — the bound that keeps a loop of create calls from
    /// growing the emulator's heap without limit. `CreateExtension` is the caller this
    /// exists for, and answers that `false` with the `ServiceQuotaExceededException` it
    /// models.
    pub fn try_insert_extension(&mut self, extension: Extension) -> bool {
        if self.extensions.len() >= MAX_EXTENSIONS {
            return false;
        }
        self.extensions.insert(extension.id.clone(), extension);
        true
    }

    /// Add one more version to the extension `extension_id` names.
    ///
    /// Answers `false`, having stored nothing, when that extension already holds
    /// [`MAX_EXTENSION_VERSIONS`] versions, or when no extension answers to
    /// `extension_id`.
    pub fn try_insert_extension_version(
        &mut self,
        extension_id: &str,
        version: ExtensionVersion,
    ) -> bool {
        let Some(extension) = self.extensions.get_mut(extension_id) else {
            return false;
        };
        if extension.versions.len() >= MAX_EXTENSION_VERSIONS {
            return false;
        }
        extension.versions.insert(version.version_number, version);
        true
    }

    /// Forget an extension this account created, with every version it holds.
    /// `DeleteExtension` is the caller this exists for, and removing the record is what
    /// frees its slot.
    pub fn remove_extension(&mut self, extension_id: &str) -> Option<Extension> {
        self.extensions.remove(extension_id)
    }

    /// Forget ONE version of an extension this account created, answering the version
    /// that was removed, or `None` when no extension answers to `extension_id` or it
    /// holds no such version.
    ///
    /// An extension exists only as its versions — the id and the name are properties of
    /// the set, and the description, actions, parameters and tags belong to a version —
    /// so an extension whose last version is removed is itself gone, and
    /// [`Self::resolve_extension`] stops answering for it. That is the same bookkeeping
    /// [`Self::remove_extension`] does, reached one version at a time, so a deleted
    /// version never leaves an extension that answers with nothing.
    pub fn remove_extension_version(
        &mut self,
        extension_id: &str,
        version_number: i32,
    ) -> Option<ExtensionVersion> {
        let extension = self.extensions.get_mut(extension_id)?;
        let removed = extension.versions.remove(&version_number)?;
        if extension.versions.is_empty() {
            self.extensions.remove(extension_id);
        }
        Some(removed)
    }

    /// Resolve a deployment strategy id to the strategy it names: one this account
    /// created, or one of AppConfig's predefined `AppConfig.*` strategies, which are
    /// deployable without a `CreateDeploymentStrategy` call.
    pub fn resolve_strategy(&self, id: &str) -> Option<DeploymentStrategy> {
        if let Some(created) = self.deployment_strategies.get(id) {
            return Some(created.clone());
        }
        PREDEFINED_DEPLOYMENT_STRATEGIES
            .iter()
            .find(|predefined| predefined.id == id)
            .map(DeploymentStrategy::from)
    }

    /// Store a deployment strategy this account created.
    ///
    /// Answers `false`, having stored nothing, when this service already holds
    /// [`MAX_DEPLOYMENT_STRATEGIES`] of them — the bound that keeps a loop of create
    /// calls from growing the emulator's heap without limit. `CreateDeploymentStrategy`
    /// is the caller this exists for, and answers that `false` with the
    /// `ServiceQuotaExceededException` it models; replacing a strategy that already
    /// exists under the same id is not growth and is always allowed.
    pub fn try_insert_strategy(&mut self, strategy: DeploymentStrategy) -> bool {
        let replacing = self.deployment_strategies.contains_key(&strategy.id);
        if !replacing && self.deployment_strategies.len() >= MAX_DEPLOYMENT_STRATEGIES {
            return false;
        }
        self.deployment_strategies
            .insert(strategy.id.clone(), strategy);
        true
    }

    /// Forget a deployment strategy this account created, answering the record that was
    /// removed, or `None` when this account created no strategy under `id`.
    /// `DeleteDeploymentStrategy` is the caller this exists for, and removing the record
    /// frees its slot.
    pub fn remove_strategy(&mut self, id: &str) -> Option<DeploymentStrategy> {
        self.deployment_strategies.remove(id)
    }

    /// The deployment strategies this account created, for a reader that must report
    /// them without reaching into the map.
    pub fn created_strategies(&self) -> impl Iterator<Item = &DeploymentStrategy> {
        self.deployment_strategies.values()
    }

    /// The whole deployment-strategy catalogue this account can deploy with, in the order
    /// a listing reports it, each entry beside the key that order is a TOTAL order on.
    ///
    /// The catalogue is AppConfig's predefined `AppConfig.*` strategies in the order the
    /// published table carries them ([`PREDEFINED_DEPLOYMENT_STRATEGIES`]), followed by
    /// every strategy this account created, in ascending id order. A key is derived from
    /// the entry itself — its position in the published table, or its id — and never from
    /// how many entries precede it, so a strategy created between two pages of a walk
    /// shifts no later page.
    ///
    /// This is [`resolve_strategy`] read over the whole catalogue rather than for one id —
    /// including its precedence: a created strategy holding an id a predefined one also
    /// holds takes that slot, so the catalogue reports each id exactly once and never with
    /// a record `resolve_strategy` would not answer for it. `ListDeploymentStrategies` is
    /// the caller this exists for.
    ///
    /// [`resolve_strategy`]: ComAmazonawsAppconfigAmazonappconfigState::resolve_strategy
    pub fn all_strategies(&self) -> Vec<(String, DeploymentStrategy)> {
        let mut catalogue: Vec<(String, DeploymentStrategy)> = PREDEFINED_DEPLOYMENT_STRATEGIES
            .iter()
            .enumerate()
            .map(|(position, predefined)| {
                let strategy = self
                    .deployment_strategies
                    .get(predefined.id)
                    .cloned()
                    .unwrap_or_else(|| DeploymentStrategy::from(predefined));
                (
                    format!("{PREDEFINED_STRATEGY_KEY_PREFIX}{position:04}"),
                    strategy,
                )
            })
            .collect();
        // Ascending by id, and without the ids already reported in a predefined slot.
        let created: BTreeMap<&str, &DeploymentStrategy> = self
            .deployment_strategies
            .iter()
            .filter(|(id, _)| {
                !PREDEFINED_DEPLOYMENT_STRATEGIES
                    .iter()
                    .any(|predefined| predefined.id == id.as_str())
            })
            .map(|(id, strategy)| (id.as_str(), strategy))
            .collect();
        catalogue.extend(created.into_iter().map(|(id, strategy)| {
            (
                format!("{CREATED_STRATEGY_KEY_PREFIX}{id}"),
                strategy.clone(),
            )
        }));
        catalogue
    }

    /// Account for `bytes` about to be stored as a hosted configuration version.
    /// Answers `false`, having changed nothing, when storing them would take this
    /// service past [`MAX_TOTAL_HOSTED_CONFIGURATION_BYTES`]; the caller then refuses
    /// the write rather than growing the heap.
    pub fn reserve_hosted_configuration_bytes(&mut self, bytes: usize) -> bool {
        let total = self.hosted_configuration_bytes.saturating_add(bytes);
        if total > MAX_TOTAL_HOSTED_CONFIGURATION_BYTES {
            return false;
        }
        self.hosted_configuration_bytes = total;
        true
    }

    /// Give back the accounting for a hosted configuration version that is no longer
    /// stored. `DeleteHostedConfigurationVersion` calls this with the length of the
    /// content it removed, so deleting a version frees its budget.
    pub fn release_hosted_configuration_bytes(&mut self, bytes: usize) {
        self.hosted_configuration_bytes = self.hosted_configuration_bytes.saturating_sub(bytes);
    }

    /// Bytes of hosted configuration content this service is currently holding.
    pub fn hosted_configuration_bytes(&self) -> usize {
        self.hosted_configuration_bytes
    }

    /// The tags the resource `resource_arn` addresses carries, or `None` when this
    /// service holds no such resource.
    ///
    /// Every resource whose tag map this service persists already owns it — a tag is part
    /// of the resource record a create wrote, not a separate store — so a tag call
    /// resolves its `ResourceArn` ([`parse_resource_arn`]) and reads the map in place.
    /// There is no second copy of a resource's tags anywhere.
    pub fn tags_of(&self, resource_arn: &str) -> Option<&HashMap<String, String>> {
        match parse_resource_arn(resource_arn)? {
            TaggedResource::Application { application_id } => {
                Some(&self.applications.get(application_id)?.tags)
            }
            TaggedResource::Environment {
                application_id,
                environment_id,
            } => Some(
                &self
                    .applications
                    .get(application_id)?
                    .environments
                    .get(environment_id)?
                    .tags,
            ),
            TaggedResource::ConfigurationProfile {
                application_id,
                configuration_profile_id,
            } => Some(
                &self
                    .applications
                    .get(application_id)?
                    .profiles
                    .get(configuration_profile_id)?
                    .tags,
            ),
            TaggedResource::Deployment {
                application_id,
                environment_id,
                deployment_number,
            } => Some(
                &self
                    .applications
                    .get(application_id)?
                    .environments
                    .get(environment_id)?
                    .deployments
                    .get(&deployment_number)?
                    .tags,
            ),
            // Only a strategy this account CREATED has a record here; a predefined
            // `AppConfig.*` one is not stored, so it carries no tag map.
            TaggedResource::DeploymentStrategy {
                deployment_strategy_id,
            } => Some(&self.deployment_strategies.get(deployment_strategy_id)?.tags),
            // An extension ARN names one VERSION, and each version carries the tags the
            // `CreateExtension` call that wrote it was asked to assign.
            TaggedResource::Extension {
                extension_id,
                version_number,
            } => Some(
                &self
                    .extensions
                    .get(extension_id)?
                    .versions
                    .get(&version_number)?
                    .tags,
            ),
        }
    }

    /// The tags of the resource `resource_arn` addresses, for a tag write. Resolves
    /// exactly the ARNs [`ComAmazonawsAppconfigAmazonappconfigState::tags_of`] resolves.
    pub fn tags_of_mut(&mut self, resource_arn: &str) -> Option<&mut HashMap<String, String>> {
        match parse_resource_arn(resource_arn)? {
            TaggedResource::Application { application_id } => {
                Some(&mut self.applications.get_mut(application_id)?.tags)
            }
            TaggedResource::Environment {
                application_id,
                environment_id,
            } => Some(
                &mut self
                    .applications
                    .get_mut(application_id)?
                    .environments
                    .get_mut(environment_id)?
                    .tags,
            ),
            TaggedResource::ConfigurationProfile {
                application_id,
                configuration_profile_id,
            } => Some(
                &mut self
                    .applications
                    .get_mut(application_id)?
                    .profiles
                    .get_mut(configuration_profile_id)?
                    .tags,
            ),
            TaggedResource::Deployment {
                application_id,
                environment_id,
                deployment_number,
            } => Some(
                &mut self
                    .applications
                    .get_mut(application_id)?
                    .environments
                    .get_mut(environment_id)?
                    .deployments
                    .get_mut(&deployment_number)?
                    .tags,
            ),
            TaggedResource::DeploymentStrategy {
                deployment_strategy_id,
            } => Some(
                &mut self
                    .deployment_strategies
                    .get_mut(deployment_strategy_id)?
                    .tags,
            ),
            TaggedResource::Extension {
                extension_id,
                version_number,
            } => Some(
                &mut self
                    .extensions
                    .get_mut(extension_id)?
                    .versions
                    .get_mut(&version_number)?
                    .tags,
            ),
        }
    }
}

/// Render a counter value as an id over [`ID_ALPHABET`].
///
/// The counter is run through SplitMix64's finalizer first, a bijection on `u64`, so
/// consecutive resources do not get adjacent ids — AppConfig's ids carry no ordering
/// a caller could depend on. Uniqueness is not left to the mixing: [`mint_id`] rejects
/// a candidate any resource already holds.
///
/// [`mint_id`]: ComAmazonawsAppconfigAmazonappconfigState::mint_id
fn encode_id(sequence: u64) -> String {
    let mut mixed = sequence.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    mixed ^= mixed >> 31;

    let mut id = String::with_capacity(ID_LENGTH);
    for slot in 0..ID_LENGTH {
        let index = (mixed >> (slot * 6)) % ID_ALPHABET.len() as u64;
        id.push(char::from(ID_ALPHABET[index as usize]));
    }
    id
}

/// The largest page a list operation answers, and the page it answers when the caller
/// names no `MaxResults`.
///
/// Every `MaxResults` member this service models is bound at `range 1..50`
/// (`smithy_query(query="operation",operation="ListConfigurationProfiles")`), so 50
/// items is the most one call can carry and the most a caller can ask for.
pub const MAX_LIST_PAGE_SIZE: usize = 50;

/// The `kind` a `ListConfigurationProfiles` cursor is stamped with, so a token minted
/// by another listing cannot resume this one. See [`paginate`].
pub const LISTING_CONFIGURATION_PROFILES: &str = "ListConfigurationProfiles";

/// The `kind` a `ListEnvironments` cursor is stamped with, so a token minted by another
/// listing cannot resume this one. See [`paginate`].
pub const LISTING_ENVIRONMENTS: &str = "ListEnvironments";

/// The `kind` a `ListDeploymentStrategies` cursor is stamped with, so a token minted by
/// another listing cannot resume this one. See [`paginate`].
pub const LISTING_DEPLOYMENT_STRATEGIES: &str = "ListDeploymentStrategies";

/// The `kind` a `ListExtensions` cursor is stamped with, so a token minted by another
/// listing cannot resume this one. See [`paginate`].
pub const LISTING_EXTENSIONS: &str = "ListExtensions";

/// One page of a list operation: the items on it, and the token that resumes the walk
/// after them (`None` on the last page).
#[derive(Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

/// Encode an opaque pagination cursor: `kind` names the listing the token belongs to,
/// and `key` is the key the NEXT page starts at.
///
/// An AppConfig `NextToken` is opaque, so the cursor is hex-encoded rather than handed
/// to the caller in the clear: a client that treats it as anything but a token to hand
/// back is reading structure the service does not publish. The encoding is
/// OBFUSCATION, not authentication — it carries no integrity tag, so a caller that
/// decodes it can mint another token for the same listing, which can only resume that
/// caller's own listing at a key of its choosing. `hex` is the maintained crate for the
/// codec.
fn encode_cursor(kind: &str, key: &str) -> String {
    hex::encode(format!("{kind}\u{1f}{key}"))
}

/// The inverse of [`encode_cursor`]: the key the page resumes at, or `None` for a token
/// this service did not mint for `kind`.
fn decode_cursor(kind: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let (token_kind, key) = decoded.split_once('\u{1f}')?;
    (token_kind == kind).then(|| key.to_string())
}

/// Cut `ordered` — a listing's items, each beside the unique key it is ordered by, in
/// ascending key order — into the page that resumes at `next_token` and carries at most
/// `max_results` items.
///
/// `None` means `next_token` is not a token this service minted for `kind`, which the
/// calling operation refuses with the error it models for a bad request. Keying the
/// cursor on an item's own key rather than on an offset is what makes a walk stable:
/// an item created or deleted between two calls shifts no later page.
pub fn paginate<T>(
    ordered: Vec<(String, T)>,
    kind: &str,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Option<Page<T>> {
    let resume_at = match next_token {
        Some(token) => Some(decode_cursor(kind, token)?),
        None => None,
    };
    // `MaxResults` is a maximum, not a demand. The modelled range (1..50) is enforced
    // by the schema walk BEFORE any handler runs, so the clamp is unreachable on the
    // wire path and is here only so that an in-process caller cannot ask for an
    // unbounded page; the assertion is what makes a value outside the range loud in a
    // debug build rather than quietly answered as a different page size.
    debug_assert!(
        max_results.is_none_or(|requested| (1..=MAX_LIST_PAGE_SIZE as i32).contains(&requested)),
        "MaxResults {max_results:?} is outside the modelled range 1..{MAX_LIST_PAGE_SIZE}"
    );
    let page_size = max_results.map_or(MAX_LIST_PAGE_SIZE, |requested| {
        requested.clamp(1, MAX_LIST_PAGE_SIZE as i32) as usize
    });
    let mut remaining = ordered
        .into_iter()
        .filter(|(key, _)| resume_at.as_deref().is_none_or(|at| key.as_str() >= at));
    let items: Vec<T> = remaining
        .by_ref()
        .take(page_size)
        .map(|(_, item)| item)
        .collect();
    Some(Page {
        next_token: remaining.next().map(|(key, _)| encode_cursor(kind, &key)),
        items,
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAppconfigAmazonappconfigState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAppconfigAmazonappconfigState>)
}
