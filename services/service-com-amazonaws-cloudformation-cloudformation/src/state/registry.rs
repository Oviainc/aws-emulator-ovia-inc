// SPDX-License-Identifier: CC0-1.0

//! The CloudFormation extension REGISTRY: the extensions (resource types, modules and
//! Hooks) this account and Region holds, and the versions of each.
//!
//! An extension is a registry record, separate from the stacks a template deploys:
//! `DescribeType` "returns detailed information about an extension from the CloudFormation
//! registry in your current account and Region", at "that specific extension version" when
//! the request names one and at "the default extension version" otherwise. Everything that
//! read reports therefore lives on ONE [`StoredExtension`] record and the
//! [`StoredTypeVersion`] rows under it, so a read computes its answer from the record
//! rather than from the request.
//!
//! ## What writes it
//!
//! Two things put a record in this registry.
//!
//! The first is Amazon Web Services: the public extensions it publishes are in every
//! account's registry without anybody registering them, which [`aws_published`] is — the
//! resource types this emulator provisions, reported as the public extensions they are.
//!
//! The second is this account's own registry WRITERS — `RegisterType` and `ActivateType`
//! (which add an extension version), `SetTypeConfiguration` (`ConfigurationSchema`),
//! `SetTypeDefaultVersion` (`DefaultVersionId`), `DeprecateType`/`DeregisterType`
//! (`DeprecatedStatus`) and `TestType` (`TypeTestsStatus`). Each is still a scaffold stub
//! with its own issue, so nothing in `src/` grows the stored side of this registry yet:
//! that is why it carries no growth bound of its own (the writer that adds a version is
//! where RegisterType's documented "maximum of 50 resource extension versions registered
//! at a time" belongs). The record shape below is what those writers fill in and what
//! every registry READ is served from.
//!
//! ## Visibility is what gates a reported member
//!
//! A registry record is either `PRIVATE` (registered in this account) or `PUBLIC` (provided
//! by Amazon Web Services, or published by a third party and activated in this account),
//! and `DescribeType` documents member by member which of those it reports: the execution
//! role, the logging configuration, the default version and the deprecation status are
//! "only private extensions", while the publisher id, the public version number and
//! `IsActivated` are "only public third-party extensions". [`StoredExtension::is_private`]
//! and [`StoredExtension::is_public_third_party`] are the two predicates that gating reads.

use std::time::SystemTime;

/// The extension kinds the model's `RegistryType` enum admits
/// (`smithy_query(query="shape", shape="RegistryType")`: `HOOK | MODULE | RESOURCE`).
pub mod registry_type {
    pub const RESOURCE: &str = "RESOURCE";
    pub const MODULE: &str = "MODULE";
    pub const HOOK: &str = "HOOK";
}

/// The model's `Visibility` enum: "PRIVATE: The extension is only visible and usable
/// within the account in which it is registered. PUBLIC: The extension is publicly visible
/// and usable within any Amazon Web Services account."
pub mod visibility {
    pub const PRIVATE: &str = "PRIVATE";
    pub const PUBLIC: &str = "PUBLIC";
}

/// The model's `DeprecatedStatus` enum: "LIVE: The extension is activated or registered and
/// can be used in CloudFormation operations... DEPRECATED: The extension has been
/// deactivated or deregistered and can no longer be used."
pub mod deprecated_status {
    pub const LIVE: &str = "LIVE";
    pub const DEPRECATED: &str = "DEPRECATED";
}

/// The model's `ProvisioningType` enum, the provisioning behaviour CloudFormation
/// "determines during registration, based on the types of handlers in the schema handler
/// package submitted": `FULLY_MUTABLE | IMMUTABLE | NON_PROVISIONABLE`.
pub mod provisioning_type {
    pub const FULLY_MUTABLE: &str = "FULLY_MUTABLE";
    pub const IMMUTABLE: &str = "IMMUTABLE";
    pub const NON_PROVISIONABLE: &str = "NON_PROVISIONABLE";
}

/// The model's `TypeTestsStatus` enum, the contract-test status of a registered private
/// extension version: `FAILED | IN_PROGRESS | NOT_TESTED | PASSED`.
pub mod type_tests_status {
    pub const PASSED: &str = "PASSED";
    pub const FAILED: &str = "FAILED";
    pub const IN_PROGRESS: &str = "IN_PROGRESS";
    pub const NOT_TESTED: &str = "NOT_TESTED";
}

/// The segment an extension kind occupies in a registry ARN: the lowercase form of the
/// `RegistryType` value, as in `type/resource/My-Sample-Resource`.
pub fn arn_kind_segment(type_kind: &str) -> String {
    type_kind.to_ascii_lowercase()
}

/// The name an extension's ARN carries: the type name with each `::` separator written as
/// one `-` (`My::Sample::Resource` becomes `My-Sample-Resource`), which is the form the
/// modelled `TypeArn` pattern (`...:type/.+$`) carries.
pub fn arn_type_name(type_name: &str) -> String {
    type_name.replace("::", "-")
}

/// Mint the ARN of an extension held in THIS account and Region:
/// `arn:aws:cloudformation:<region>:<account>:type/<kind>/<Type-Name>`. The account segment
/// comes from the emulator-wide [`::emulator_server::emulator_arn`] helper.
pub fn type_arn(region: &str, type_kind: &str, type_name: &str) -> String {
    ::emulator_server::emulator_arn(
        crate::state::ARN_NAMESPACE,
        region,
        &format!(
            "type/{}/{}",
            arn_kind_segment(type_kind),
            arn_type_name(type_name)
        ),
    )
}

/// The ARN of ONE version of an extension: the extension's ARN with the version id
/// appended — "the version ID is the value at the end of the Amazon Resource Name (ARN)
/// assigned to the extension version when it is registered"
/// (`DescribeTypeInput$VersionId`).
pub fn type_version_arn(extension_arn: &str, version_id: &str) -> String {
    format!("{extension_arn}/{version_id}")
}

/// The ARN of a PUBLIC extension: `arn:aws:cloudformation:<region>::type/<kind>/<Name>`.
///
/// A public extension belongs to no account, so its ARN carries an EMPTY account segment —
/// which the modelled `TypeArn` pattern admits (`:([0-9]{12})?:type/.+$`) and which
/// [`::emulator_server::emulator_arn`] cannot express, since that helper always stamps the
/// emulator's canonical account id. The partition is the one every ARN this emulator mints
/// belongs to ([`crate::state::template::PARTITION`]).
pub fn public_type_arn(region: &str, type_kind: &str, type_name: &str) -> String {
    format!(
        "arn:{}:{}:{region}::type/{}/{}",
        crate::state::template::PARTITION,
        crate::state::ARN_NAMESPACE,
        arn_kind_segment(type_kind),
        arn_type_name(type_name)
    )
}

/// Split an ARN that MAY name one version of an extension into the extension's own ARN and
/// that version id.
///
/// A registry ARN's resource portion is `type/<kind>/<Type-Name>`, so an ARN with a fourth
/// segment names a version (what `TestType` returns as its `TypeVersionArn` and documents
/// passing "into DescribeType to monitor the current test status") and one with three names
/// the extension itself. A dashed type name carries no `/` of its own, so the count is
/// unambiguous.
pub fn split_version_arn(arn: &str) -> (&str, Option<&str>) {
    if arn.matches('/').count() < 3 {
        return (arn, None);
    }
    match arn.rsplit_once('/') {
        Some((extension, version_id)) => (extension, Some(version_id)),
        None => (arn, None),
    }
}

/// `LoggingConfig` as the registry persists it: "logging configuration information for
/// private extensions".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredLoggingConfig {
    pub log_role_arn: String,
    pub log_group_name: String,
}

/// One entry of `RequiredActivatedTypes`: "for extensions that are modules, the public
/// third-party extensions that must be activated in your account in order for the module
/// itself to be activated".
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StoredRequiredActivatedType {
    pub type_name_alias: Option<String>,
    pub original_type_name: Option<String>,
    pub publisher_id: Option<String>,
    pub supported_major_versions: Vec<i32>,
}

/// One VERSION of one extension — what a `DescribeType` request selects with `VersionId`,
/// with `PublicVersionNumber`, or by naming no version at all (the default version).
///
/// Everything a read reports about the version it describes is held here, so two versions
/// of one extension can differ in exactly the ways CloudFormation lets them: their schema,
/// their description, their provisioning behaviour, their deprecation status and their
/// contract-test status.
#[derive(Debug, Clone)]
pub struct StoredTypeVersion {
    /// `TypeVersionId`, the id at the end of this version's ARN.
    pub version_id: String,
    /// This version's own ARN ([`type_version_arn`]).
    pub arn: String,
    pub description: Option<String>,
    /// `Schema`: "the schema that defines the extension". `None` where the registry holds
    /// no schema for the version rather than an invented one.
    pub schema: Option<String>,
    /// [`provisioning_type`], which registration determines from the handlers the schema
    /// handler package carries.
    pub provisioning_type: Option<String>,
    /// [`deprecated_status`].
    pub deprecated_status: String,
    /// [`type_tests_status`], the contract-test status of this version.
    pub type_tests_status: Option<String>,
    pub type_tests_status_description: Option<String>,
    pub source_url: Option<String>,
    pub documentation_url: Option<String>,
    /// `PublicVersionNumber`, the publisher's version number for a public extension.
    pub public_version_number: Option<String>,
    /// The IAM role registration was given, "the ARN of the IAM execution role used to
    /// register the extension".
    pub execution_role_arn: Option<String>,
    pub logging_config: Option<StoredLoggingConfig>,
    pub required_activated_types: Vec<StoredRequiredActivatedType>,
    /// `TimeCreated`: "when the specified private extension version was registered or
    /// activated in your account" — the instant the version entered this registry. `None`
    /// only for a record that carries no such instant at all; the Amazon-Web-Services-
    /// published extensions carry the instant the registry itself came into being (see
    /// [`aws_published`]), since they have been in it for as long as it has existed.
    pub time_created: Option<SystemTime>,
    /// `LastUpdated`: "when the specified extension version was registered". Filled in on
    /// the same records, and from the same instant, as [`Self::time_created`].
    pub last_updated: Option<SystemTime>,
}

impl StoredTypeVersion {
    /// A freshly registered version: `LIVE` and `NOT_TESTED`, which is what a version that
    /// has just entered the registry and had no contract test run against it is.
    pub fn new(extension_arn: &str, version_id: &str, registered_at: SystemTime) -> Self {
        Self {
            version_id: version_id.to_string(),
            arn: type_version_arn(extension_arn, version_id),
            description: None,
            schema: None,
            provisioning_type: None,
            deprecated_status: deprecated_status::LIVE.to_string(),
            type_tests_status: Some(type_tests_status::NOT_TESTED.to_string()),
            type_tests_status_description: None,
            source_url: None,
            documentation_url: None,
            public_version_number: None,
            execution_role_arn: None,
            logging_config: None,
            required_activated_types: Vec::new(),
            time_created: Some(registered_at),
            last_updated: Some(registered_at),
        }
    }
}

/// One extension in the registry, with every version of it the registry holds.
#[derive(Debug, Clone)]
pub struct StoredExtension {
    /// [`registry_type`]: the kind of extension.
    pub type_kind: String,
    /// The name this account addresses the extension by — for a public third-party
    /// extension activated under an alias, the alias ("CloudFormation treats that alias as
    /// the extension's type name within the account and Region").
    pub type_name: String,
    /// The extension's own ARN in this account and Region ([`type_arn`]).
    pub arn: String,
    /// The Region whose registry holds the record: `DescribeType` reads "the CloudFormation
    /// registry in your current account and Region", so a record is addressable only from
    /// the Region it was registered in.
    pub region: String,
    /// [`visibility`].
    pub visibility: String,
    /// `DefaultVersionId`, "used when the extension version isn't specified". Set for the
    /// private extensions whose versions this account registered; `None` for a public
    /// extension, which carries none.
    pub default_version_id: Option<String>,
    /// `ConfigurationSchema`: "a JSON string that represent the current configuration data
    /// for the extension in this account and Region", written by `SetTypeConfiguration`.
    pub configuration_schema: Option<String>,
    /// `PublisherId`, present exactly for a public THIRD-PARTY extension: "extensions
    /// provided by Amazon Web Services are not assigned a publisher ID".
    pub publisher_id: Option<String>,
    /// `OriginalTypeName` / `OriginalTypeArn`: for a public extension activated in this
    /// account, the type name and ARN of the public extension behind the local name.
    pub original_type_name: Option<String>,
    pub original_type_arn: Option<String>,
    /// `LatestPublicVersion`: "the latest version of a public extension that is available
    /// for use".
    pub latest_public_version: Option<String>,
    /// `IsActivated`: whether a public third-party extension is activated in this account
    /// and Region.
    pub is_activated: Option<bool>,
    /// `AutoUpdate`: whether CloudFormation updates an activated public extension when the
    /// publisher releases a new minor version.
    pub auto_update: Option<bool>,
    /// Every version the registry holds, in the order they entered it.
    pub versions: Vec<StoredTypeVersion>,
}

impl StoredExtension {
    /// An extension record for a PRIVATE extension of this account and Region, with no
    /// versions yet: "CloudFormation marks any extensions you register as PRIVATE".
    pub fn new(region: &str, type_kind: &str, type_name: &str) -> Self {
        Self {
            type_kind: type_kind.to_string(),
            type_name: type_name.to_string(),
            arn: type_arn(region, type_kind, type_name),
            region: region.to_string(),
            visibility: visibility::PRIVATE.to_string(),
            default_version_id: None,
            configuration_schema: None,
            publisher_id: None,
            original_type_name: None,
            original_type_arn: None,
            latest_public_version: None,
            is_activated: None,
            auto_update: None,
            versions: Vec::new(),
        }
    }

    /// Whether this is a private extension — the ones the members documented as applying
    /// "only to private extensions you have registered in your account" are reported for.
    pub fn is_private(&self) -> bool {
        self.visibility == visibility::PRIVATE
    }

    /// Whether this is a public THIRD-PARTY extension: public and carrying a publisher id,
    /// which "extensions provided by Amazon Web Services are not assigned". The members
    /// documented as applying "only to public third-party extensions" are reported for
    /// these, and the ones documented as `null` "for public third-party extensions" are
    /// withheld from them.
    pub fn is_public_third_party(&self) -> bool {
        !self.is_private() && self.publisher_id.is_some()
    }

    /// The version with this `TypeVersionId`, if the registry holds it.
    pub fn version(&self, version_id: &str) -> Option<&StoredTypeVersion> {
        self.versions
            .iter()
            .find(|version| version.version_id == version_id)
    }

    /// The version a publisher published under this `PublicVersionNumber`, if the registry
    /// holds it.
    pub fn version_of_public_number(&self, public_version_number: &str) -> Option<&StoredTypeVersion> {
        self.versions.iter().find(|version| {
            version.public_version_number.as_deref() == Some(public_version_number)
        })
    }

    /// The version a request that names none describes: "it returns information about the
    /// default extension version".
    ///
    /// That is the version `DefaultVersionId` names where the record carries one. A record
    /// that carries none is a public extension — `DefaultVersionId` is `null` for those —
    /// and the version the registry holds for it is the one its activation installed, the
    /// newest in the record.
    pub fn default_version(&self) -> Option<&StoredTypeVersion> {
        match &self.default_version_id {
            Some(version_id) => self.version(version_id),
            None => self.versions.last(),
        }
    }

    /// Whether this version is the record's default version (`IsDefaultVersion`).
    pub fn is_default_version(&self, version_id: &str) -> bool {
        match &self.default_version_id {
            Some(default) => default == version_id,
            // No `DefaultVersionId` on the record: the one version it holds is the one a
            // request naming no version gets.
            None => self
                .default_version()
                .is_some_and(|version| version.version_id == version_id),
        }
    }
}

/// The version id the one version of an Amazon-Web-Services-published public extension
/// carries, in the `TypeVersionId` form (`@pattern("^[A-Za-z0-9-]+$")`) the registry uses.
///
/// The emulator holds exactly one version of each such extension — the behaviour it
/// implements for that resource type — so there is one id, and it is the default version.
pub const AWS_PUBLISHED_VERSION_ID: &str = "00000001";

/// The Amazon-Web-Services-published public extension the registry of this Region holds
/// for a resource type, if this is one of the types the emulator implements.
///
/// The public extensions Amazon Web Services publishes are in every account's registry
/// without anybody registering them — `DescribeType` reads "an extension from the
/// CloudFormation registry in your current account and Region", and `RegisterType` reserves
/// the `AWS` namespace, so no account's own registration can own one of these names. The
/// set the emulator holds is the set of resource types it actually provisions
/// ([`crate::state::template::IMPLEMENTED_RESOURCE_TYPES`]); a name outside that set is not
/// in this registry, which is what makes the answer for it a `TypeNotFoundException`
/// instead of a record nothing stands behind.
///
/// The record carries only what is known about such an extension: it is `PUBLIC`, it is
/// `LIVE`, it is `FULLY_MUTABLE` (the emulator's own stack operations create, update and
/// delete these resource types, which is what that value states: "the resource type
/// includes an update handler to process updates to the type during stack update
/// operations"), it has no publisher id — "extensions provided by Amazon Web Services are
/// not assigned a publisher ID" — its `SourceUrl` is the open-source repository AWS
/// publishes that type's resource provider in ([`aws_published_source_url`]), and its
/// registration instants are the instant this registry came into being
/// ([`REGISTRY_GENESIS`]), which is how long it has held them. Its `Schema` is "the schema
/// that defines the extension": the resource schema this registry holds for the type
/// ([`crate::state::resource_schema`]), derived from the properties the type publishes. The
/// description
/// and the documentation URL are NOT known here, so they stay absent rather than invented.
pub fn aws_published(region: &str, type_kind: &str, type_name: &str) -> Option<StoredExtension> {
    if type_kind != registry_type::RESOURCE
        || !crate::state::template::IMPLEMENTED_RESOURCE_TYPES.contains(&type_name)
    {
        return None;
    }
    Some(aws_published_record(region, type_name))
}

/// [`aws_published`], for a request that named the extension by ARN.
///
/// A public extension's ARN carries no account id and spells its name with `-` separators,
/// so the ARN is matched back to the catalogued type name rather than parsed into one. The
/// ARN must name the Region the request was served in: the registry is regional, so an ARN
/// minted for another Region names nothing here.
pub fn aws_published_by_arn(region: &str, arn: &str) -> Option<StoredExtension> {
    if public_arn_region(arn) != Some(region) {
        return None;
    }
    let type_name = crate::state::template::IMPLEMENTED_RESOURCE_TYPES
        .iter()
        .find(|type_name| public_type_arn(region, registry_type::RESOURCE, type_name) == arn)?;
    Some(aws_published_record(region, type_name))
}

/// The Region segment of an ARN whose account segment is EMPTY, which is the form a public
/// extension's ARN carries. `None` for anything else, so an account-scoped ARN never
/// matches a public extension.
fn public_arn_region(arn: &str) -> Option<&str> {
    let mut segments = arn.split(':');
    let (prefix, partition, service) = (segments.next()?, segments.next()?, segments.next()?);
    let region = segments.next()?;
    let account = segments.next()?;
    let names_a_public_extension = prefix == "arn"
        && partition == crate::state::template::PARTITION
        && service == crate::state::ARN_NAMESPACE
        && account.is_empty()
        && !region.is_empty();
    names_a_public_extension.then_some(region)
}

/// The instant this emulator's registry came into being, which is what the Amazon-Web-
/// Services-published records report as `TimeCreated` and `LastUpdated`.
///
/// Those records are not registered by anybody — they are in every account's registry
/// already — so there is no registration call whose instant could be stored on them. What
/// is true of them here is that they have been in this registry for as long as it has
/// existed, and that instant is this one: resolved once, on the first registry read, so
/// every record reports the same instant and a second read of one reports the instant the
/// first did. A registry WRITER (`RegisterType`, `ActivateType`) stores the instant of its
/// own call instead ([`StoredTypeVersion::new`]).
static REGISTRY_GENESIS: ::std::sync::LazyLock<SystemTime> =
    ::std::sync::LazyLock::new(SystemTime::now);

/// The source the extensions Amazon Web Services publishes are built from: AWS's
/// CloudFormation resource providers are open source, one repository per service, named
/// `aws-cloudformation-resource-providers-<service>` after the type's own service segment
/// (`AWS::S3::Bucket` -> `.../aws-cloudformation-resource-providers-s3`). `SourceUrl` is
/// "the URL of the source code for the extension", and that is the URL for one of these,
/// derived from the type name rather than held as a per-type constant.
///
/// It is also the `sourceUrl` of the resource schema this registry derives for the type
/// ([`crate::state::resource_schema`]): a published resource schema names that same
/// repository.
pub fn aws_published_source_url(type_name: &str) -> Option<String> {
    let service = type_name.split("::").nth(1)?;
    if service.is_empty() {
        return None;
    }
    Some(format!(
        "https://github.com/aws-cloudformation/aws-cloudformation-resource-providers-{}",
        service.to_ascii_lowercase()
    ))
}

/// The record [`aws_published`] and [`aws_published_by_arn`] both answer with.
fn aws_published_record(region: &str, type_name: &str) -> StoredExtension {
    let arn = public_type_arn(region, registry_type::RESOURCE, type_name);
    let version = StoredTypeVersion {
        version_id: AWS_PUBLISHED_VERSION_ID.to_string(),
        // A public extension is addressed by the extension's own ARN — that is the ARN
        // CloudFormation reports for one — so the version carries it unsuffixed.
        arn: arn.clone(),
        description: None,
        // "The schema that defines the extension": the resource schema this registry holds
        // for the type, derived from the properties the type publishes rather than read
        // out of a handler package — nobody registered this extension, so there is none.
        schema: crate::state::resource_schema::resource_type_schema(type_name),
        provisioning_type: Some(provisioning_type::FULLY_MUTABLE.to_string()),
        deprecated_status: deprecated_status::LIVE.to_string(),
        // "CloudFormation doesn't return this information for public extensions": the
        // contract-test status belongs to registered private extension versions.
        type_tests_status: None,
        type_tests_status_description: None,
        source_url: aws_published_source_url(type_name),
        documentation_url: None,
        public_version_number: None,
        execution_role_arn: None,
        logging_config: None,
        required_activated_types: Vec::new(),
        // Every extension version in the registry carries the instant it entered it; for
        // one nobody registered that is [`REGISTRY_GENESIS`].
        time_created: Some(*REGISTRY_GENESIS),
        last_updated: Some(*REGISTRY_GENESIS),
    };
    StoredExtension {
        type_kind: registry_type::RESOURCE.to_string(),
        type_name: type_name.to_string(),
        arn,
        region: region.to_string(),
        visibility: visibility::PUBLIC.to_string(),
        // "For public extensions, both those provided by Amazon Web Services and published
        // by third parties, CloudFormation returns null."
        default_version_id: None,
        configuration_schema: None,
        // "Extensions provided by Amazon Web Services are not assigned a publisher ID."
        publisher_id: None,
        // Nothing was activated under an alias: the local name IS the public name.
        original_type_name: None,
        original_type_arn: None,
        latest_public_version: None,
        is_activated: None,
        auto_update: None,
        versions: vec![version],
    }
}
