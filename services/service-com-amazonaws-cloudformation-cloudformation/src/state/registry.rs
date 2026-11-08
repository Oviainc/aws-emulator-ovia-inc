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
//! resource types this emulator provisions, reported as the public extensions they are, the
//! public resource types no resource provider stands behind
//! ([`aws_published_resource_types`], which the registry reports in the provisioning class
//! that says so), and the extensions it has PUBLISHED to the registry
//! ([`PUBLIC_EXTENSION_CATALOG`]), which are Hooks rather than resource types and which
//! belong to the publisher that published them. So the public half of this registry is no
//! more one KIND of extension, or one provisioning class, than the private half is.
//!
//! The second is this account's own registry WRITERS — `RegisterType` and `ActivateType`
//! (which add an extension version), `SetTypeConfiguration` (`ConfigurationSchema`),
//! `SetTypeDefaultVersion` (`DefaultVersionId`), `DeregisterType` (`DeprecatedStatus`, see
//! [`StoredTypeVersion::deprecate`] and [`StoredExtension::deprecate_every_version`]),
//! `DeactivateType` (which deactivates an activation, see
//! [`StoredExtension::deactivate`]), `TestType` (`TypeTestsStatus`) and `PublishType` (which
//! publishes one version of a private record as the PUBLIC extension record
//! [`published_record`] is).
//! Every one of those writers is implemented, so the records `src/` adds to this registry
//! are a registration's private one, an activation's and a publication's public one. A
//! publication adds at most one RECORD per private record it publishes, and
//! the versions it then adds to that record are bounded at [`MAX_PUBLISHED_VERSIONS`],
//! oldest reclaimed first; a registration adds at most one version per call, bounded at
//! [`MAX_REGISTERED_VERSIONS`] — RegisterType's documented "maximum of 50 resource extension
//! versions registered at a time", which is also what it refuses a registration past. The
//! record shape below is what those writers fill in and what every registry READ is served
//! from.
//!
//! ## The status transitions the clock owes here
//!
//! Contract testing is the registry's own asynchronous operation: `TestType` answers while
//! the tests are still running, and "an extension must have a test status of PASSED before
//! it can be published", so a tested version carries `IN_PROGRESS` and the conclusion
//! [`type_tests_outcome`] computed over what the registry holds for it
//! ([`PendingTypeTests`]) until the due instant passes. [`StoredExtension::settle`] is what
//! installs it, from the same `settle` walk every operation makes first, so a caller that
//! reads straight after the call still sees `IN_PROGRESS` and nothing sleeps.
//!
//! Registering a version is the other one: `RegisterType` is answered with a registration
//! token rather than with a registered version, so the REQUEST is a record of its own
//! ([`StoredTypeRegistration`]) holding `IN_PROGRESS` until its own conclusion is due, which
//! [`StoredTypeRegistration::settle`] installs from that same walk.
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

use std::time::{Duration, SystemTime};

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

/// The model's `RegistrationStatus` enum, "the current status of the extension registration
/// request" (`smithy_query(query="shape", shape="RegistrationStatus")`: `COMPLETE | FAILED |
/// IN_PROGRESS`).
pub mod registration_status {
    /// The status a registration request still running holds, and the one a listing of
    /// registration requests narrows to when the request names no other
    /// (`ListTypeRegistrationsInput$RegistrationStatusFilter`: "the default is IN_PROGRESS").
    pub const IN_PROGRESS: &str = "IN_PROGRESS";
    /// The status a registration request that finished registering the version holds.
    pub const COMPLETE: &str = "COMPLETE";
    /// The status a registration request that could not register the version holds.
    pub const FAILED: &str = "FAILED";
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

/// Mint the ARN of one CONFIGURATION of an extension held in this account and Region:
/// `arn:aws:cloudformation:<region>:<account>:type-configuration/<kind>/<Type-Name>/<alias>`,
/// the `type-configuration/` form the modelled `TypeConfigurationArn` pattern carries
/// (`^arn:aws…:cloudformation:…:([0-9]{12})?:type-configuration/.+$`).
///
/// Built from the extension the configuration belongs to and the alias it was set under —
/// the two things that identify it — so the ARN `SetTypeConfiguration` answers is the one a
/// later read of the same extension and alias resolves.
pub fn type_configuration_arn(
    region: &str,
    type_kind: &str,
    type_name: &str,
    configuration_alias: &str,
) -> String {
    ::emulator_server::emulator_arn(
        crate::state::ARN_NAMESPACE,
        region,
        &format!(
            "type-configuration/{}/{}/{configuration_alias}",
            arn_kind_segment(type_kind),
            arn_type_name(type_name)
        ),
    )
}

/// The alias a configuration set under no alias of its own is held under.
///
/// `SetTypeConfiguration`'s `ConfigurationAlias` is "an alias by which to refer to this
/// extension configuration data", required only "when setting a configuration for a resource
/// type extension"; the configuration an extension has when nothing named an alias is its
/// DEFAULT one, which is what `TypeConfigurationDetails$IsDefaultConfiguration` reports.
pub const DEFAULT_CONFIGURATION_ALIAS: &str = "default";

/// How many configurations the registry holds for ONE extension before the oldest is
/// reclaimed.///
/// A configuration is keyed by a CALLER-CHOSEN alias, so the set would otherwise grow with
/// the aliases a caller invents; this is the bound that makes it finite, the same shape
/// [`MAX_PUBLISHED_VERSIONS`] gives a record's versions. Chosen: AWS publishes no ceiling on
/// an extension's configuration aliases, and this is well past the one alias a Hook and the
/// handful a resource type are configured under.
pub const MAX_TYPE_CONFIGURATIONS: usize = 50;

/// How many extension RECORDS this account and Region's registry holds before the oldest is
/// reclaimed.
///
/// A registry writer adds a record under a name the caller chose (`ActivateType`'s
/// `TypeNameAlias`, `RegisterType`'s `TypeName`), so the set of records is a caller-counted
/// growth site and needs a ceiling of its own, like every other store this service keeps.
/// Chosen: AWS publishes a quota of 50 registered resource extension versions and no ceiling
/// on activations, and this is well past the extensions one account and Region works with
/// while keeping the registry's size bounded.
pub const MAX_EXTENSIONS: usize = 200;

/// Add one record to the registry, keeping it bounded at [`MAX_EXTENSIONS`] with the oldest
/// records reclaimed first.
///
/// The ONE bounded push every registry writer that adds a RECORD makes — a registration
/// (`RegisterType`), an activation (`ActivateType`) and a publication — since each adds a
/// record under a name its caller chose, so none of them spells the bound itself.
pub fn record_extension(extensions: &mut Vec<StoredExtension>, record: StoredExtension) {
    let over_the_bound = (extensions.len() + 1).saturating_sub(MAX_EXTENSIONS);
    if over_the_bound > 0 {
        extensions.drain(..over_the_bound);
    }
    extensions.push(record);
}

/// One account-level CONFIGURATION of an extension: the configuration data
/// `SetTypeConfiguration` persisted, the alias it was set under, the ARN that write
/// answered, and when it was written.
///
/// `BatchDescribeTypeConfigurations` reports exactly these
/// (`TypeConfigurationDetails$Configuration`/`Alias`/`Arn`/`LastUpdated`), so a read is
/// served from the record the write left rather than from the request that read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredTypeConfiguration {
    /// `Alias`: "the alias specified for this configuration, if one was specified when the
    /// configuration was set" ([`DEFAULT_CONFIGURATION_ALIAS`] otherwise).
    pub alias: String,
    /// `Arn`: "the ARN for the configuration data, in this account and Region"
    /// ([`type_configuration_arn`]).
    pub arn: String,
    /// `Configuration`: "a JSON string specifying the configuration data for the extension,
    /// in this account and Region", as the write submitted it.
    pub configuration: String,
    /// `LastUpdated`: "when the configuration data was last updated for this extension".
    pub last_updated: SystemTime,
}


/// How many REGISTRATION REQUESTS this account and Region's registry keeps, oldest reclaimed
/// first.
///
/// A caller chooses how many registrations it starts, so the set of requests is a
/// caller-counted growth site of its own: a request is a record beside the extension version
/// it registers, addressable by its own registration token for as long as the registry keeps
/// it. The model documents no retention for one, so this is a COUNT rather than an age — a
/// bound on the store, not a claim about how long the real service keeps a token
/// addressable — and it is well past the 50 resource extension versions one account and
/// Region may have registered at a time.
pub const MAX_TYPE_REGISTRATIONS: usize = 500;

/// ONE registration request of this account and Region: the record `RegisterType` writes for
/// the version it registers, addressable by the registration token that call answers with.
///
/// "Once you have initiated a registration request using RegisterType, you can use
/// DescribeTypeRegistration to monitor the progress of the registration request" and
/// `ListTypeRegistrations` "returns a list of registration tokens for the specified
/// extension(s)" — so both of those reads are served from these records, and the request's
/// own status is what they report rather than the extension's.
#[derive(Debug, Clone)]
pub struct StoredTypeRegistration {
    /// The Region this registration belongs to. The registry is regional, so a token issued
    /// in one Region names no registration in another — the same gate every other registry
    /// read goes through.
    pub region: String,
    /// `RegistrationToken`, "the identifier for this registration request"
    /// (`@length(1..128)`, `@pattern("^[a-zA-Z0-9][-a-zA-Z0-9]*$")`), which is the only thing
    /// a read names the request by.
    pub registration_token: String,
    /// [`registry_type`]: "the kind of extension" being registered.
    pub type_kind: String,
    /// `TypeName`, "the name of the extension being registered".
    pub type_name: String,
    /// The `TypeVersionId` of the version this request registers, which is what makes the
    /// request's own ARN of "this specific version of the extension being registered"
    /// ([`Self::type_version_arn`]) readable from the record. `None` for a request that
    /// concludes `FAILED`: a registration that cannot register a version does not reserve
    /// one.
    pub version_id: Option<String>,
    /// `ClientRequestToken`, the idempotency key the registration was submitted under, where
    /// it carried one: "specifying a client request token prevents CloudFormation from
    /// generating more than one version of an extension from the same registration request,
    /// even if the request is submitted multiple times", so a resubmission under a token this
    /// Region already holds a request for IS that request.
    pub client_request_token: Option<String>,
    /// The ARN of the extension being registered ([`type_arn`]): what a request narrowing a
    /// listing by `TypeArn` resolves against.
    pub type_arn: String,
    /// [`registration_status`], "the current status of the extension registration request".
    pub status: String,
    /// The status this request CONCLUDES in once the clock has made the conclusion due:
    /// `COMPLETE` for a registration that registered its version, `FAILED` for one that
    /// could not. The conclusion is computed by the call that STARTS the request — it follows
    /// from the handler package that call read — and [`Self::settle`] only installs it, so no
    /// read invents a conclusion and nothing sleeps for one.
    pub conclusion: String,
    /// Why the registration could not register a version, for a request whose conclusion is
    /// `FAILED`: `DescribeTypeRegistration`'s `Description`, "the description of the
    /// extension registration request".
    pub failure_reason: Option<String>,
    /// The instant this request's conclusion becomes due, while it is still running: a
    /// registration is asynchronous — `RegisterType` is answered with a token to monitor it
    /// by — so the request holds `IN_PROGRESS` until the clock has made the conclusion due
    /// and [`Self::settle`] is what installs it. Nothing sleeps for it and no read invents a
    /// conclusion the clock had not already made due.
    pub concluded_at: SystemTime,
}

impl StoredTypeRegistration {
    /// A registration request that has just started and REGISTERS the version `version_id`
    /// when the clock makes its conclusion due: `IN_PROGRESS` until `concluded_at`, then
    /// `COMPLETE`.
    pub fn registering(
        region: impl Into<String>,
        type_kind: impl Into<String>,
        type_name: impl Into<String>,
        version_id: impl Into<String>,
        registration_token: impl Into<String>,
        concluded_at: SystemTime,
    ) -> Self {
        Self {
            version_id: Some(version_id.into()),
            conclusion: registration_status::COMPLETE.to_string(),
            ..Self::started(region, type_kind, type_name, registration_token, concluded_at)
        }
    }

    /// A registration request that has just started and FAILS when the clock makes its
    /// conclusion due, for `reason`: registering an extension "includes validating the
    /// extension schema", so a request whose handler package the registration can read no
    /// schema of the extension out of registers no version at all — the request is still a
    /// record, and the reason is what a read of it reports.
    pub fn failing(
        region: impl Into<String>,
        type_kind: impl Into<String>,
        type_name: impl Into<String>,
        registration_token: impl Into<String>,
        reason: impl Into<String>,
        concluded_at: SystemTime,
    ) -> Self {
        Self {
            conclusion: registration_status::FAILED.to_string(),
            failure_reason: Some(reason.into()),
            ..Self::started(region, type_kind, type_name, registration_token, concluded_at)
        }
    }

    /// The record both constructors above fill in: a request still running, concluding at
    /// `concluded_at` in whichever status its conclusion is.
    fn started(
        region: impl Into<String>,
        type_kind: impl Into<String>,
        type_name: impl Into<String>,
        registration_token: impl Into<String>,
        concluded_at: SystemTime,
    ) -> Self {
        let region = region.into();
        let type_kind = type_kind.into();
        let type_name = type_name.into();
        Self {
            type_arn: type_arn(&region, &type_kind, &type_name),
            region,
            type_kind,
            type_name,
            version_id: None,
            client_request_token: None,
            registration_token: registration_token.into(),
            status: registration_status::IN_PROGRESS.to_string(),
            conclusion: registration_status::COMPLETE.to_string(),
            failure_reason: None,
            concluded_at,
        }
    }

    /// The ARN of "this specific version of the extension being registered", which is the
    /// version id this request registers under the extension's own ARN
    /// ([`type_version_arn`]); `None` for a request that registers no version.
    pub fn type_version_arn(&self) -> Option<String> {
        self.version_id
            .as_deref()
            .map(|version_id| type_version_arn(&self.type_arn, version_id))
    }

    /// Materialise the conclusion the clock has already made due: a request still running
    /// past [`Self::concluded_at`] has reached [`Self::conclusion`]. A request that already
    /// concluded is left exactly as it is — a conclusion is a request's last status.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == registration_status::IN_PROGRESS && now >= self.concluded_at {
            self.status = self.conclusion.clone();
        }
    }

    /// Whether this is the registration a read signed for `region` and carrying
    /// `registration_token` names.
    pub fn is_addressed_by(&self, region: &str, registration_token: &str) -> bool {
        self.region == region && self.registration_token == registration_token
    }

    /// Whether this registration is OF the extension `type_arn` names. An extension-version
    /// ARN names the extension it is a version of ([`split_version_arn`]), since a
    /// registration belongs to the extension rather than to one version of it.
    pub fn is_of_extension_arn(&self, arn: &str) -> bool {
        self.type_arn == split_version_arn(arn).0
    }

    /// Whether this registration is OF the extension a `Type`-and-`TypeName` request names.
    pub fn is_of_extension_named(&self, type_kind: &str, type_name: &str) -> bool {
        self.type_kind == type_kind && self.type_name == type_name
    }
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

/// What a PUBLIC extension ARN names: the extension's kind, the publisher that published
/// it, its name — and the public VERSION NUMBER the ARN pins past that name, where it pins
/// one ([`split_public_type_arn`] reads all four out of one ARN).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicTypeArnParts {
    /// The kind of extension, in the spelling [`registry_type`] keys a record by.
    pub type_kind: String,
    /// The id of the publisher that published the extension.
    pub publisher_id: String,
    /// The extension's own name, in the `::`-separated spelling the registry keys a record
    /// by.
    pub type_name: String,
    /// What the ARN pins PAST the extension's name, which in the public-extension ARN form
    /// `…::type/<kind>/<publisher id>/<Type-Name>/<version>` is a public version number.
    /// `None` for an ARN that names the extension itself, which is the only form an
    /// extension is addressable by ([`PUBLIC_VERSION_NUMBER_IN_ARN`]).
    pub public_version_number: Option<String>,
}

/// The extension NAME an account-scoped registry ARN carries, as the registry keys a record
/// by it.
///
/// CloudFormation addresses an extension of one account and Region by
/// `…:<account>:type/<kind>/<Type-Name>` ([`type_arn`] mints exactly that), spelling the
/// `::` separators of the name as `-` ([`arn_type_name`]), so the second segment after
/// `type/` is read back here into the spelling the registry keys the record by — a type
/// name's own segments carry no `-` of their own (the modelled `TypeName` pattern admits only
/// letters and digits in them), so that reading is unambiguous.
///
/// `None` for anything that is not that form: an ARN of another resource kind, and one
/// carrying further segments — a public extension's ARN names its publisher between the kind
/// and the name ([`public_type_arn`], read back by [`split_public_type_arn`]) and a version's
/// ARN carries the version id after the name ([`type_version_arn`]), neither of which this
/// reads.
///
/// What this is for: a refusal that names the extension a request identified names it by this
/// name, whether the request named the extension by name or by its ARN.
pub fn arn_extension_name(arn: &str) -> Option<String> {
    let (_, path) = arn.split_once(":type/")?;
    let mut segments = path.split('/');
    let type_kind = segments.next()?;
    let dashed_name = segments.next()?;
    if type_kind.is_empty() || dashed_name.is_empty() || segments.next().is_some() {
        return None;
    }
    Some(dashed_name.replace('-', "::"))
}

/// The kind, the publisher id, the extension name and any pinned public version a PUBLIC
/// extension ARN carries.
///
/// CloudFormation addresses a public extension by `…::type/<kind>/<publisher id>/<Type-Name>`
/// where a PUBLISHER published it, and by `…::type/<kind>/<Type-Name>` where AMAZON WEB
/// SERVICES did — "extensions published by Amazon aren't assigned a publisher ID"
/// (`ListTypeVersionsInput$PublisherId`), so such an ARN carries no publisher segment at all,
/// which is also the form this registry PUBLISHES one under ([`public_type_arn`]). Both are
/// read here, told apart by whether the segment after the kind is an extension TYPE NAME
/// ([`is_type_name`]): a publisher id is "^[0-9a-zA-Z]{12,40}$", which no type name is, and a
/// dashed type name is three alphanumeric segments, which no publisher id is — so the
/// reading is unambiguous either way, and an ARN this registry handed out can be used to
/// activate the extension it names.
///
/// A registry ARN spells the kind in lower case and the `::` separators of a type name as `-`
/// ([`arn_kind_segment`], [`arn_type_name`]), so both are read back into the spellings the
/// registry keys a record by, and a kind the registry does not name is read as the extension
/// kind it spells. Anything PAST the name segment is the public version number the ARN pins,
/// reported in [`PublicTypeArnParts::public_version_number`]. `None` for an ARN carrying no
/// such segments at all.
///
/// The registry PARSES this ARN itself rather than leaving it to a syntax gate, which is why
/// this reading is shared: the refusal an unresolvable one gets is the registry's own
/// ([`INVALID_TYPE_NAME`]), as is the refusal of one pinning a version
/// ([`PUBLIC_VERSION_NUMBER_IN_ARN`]).
pub fn split_public_type_arn(arn: &str) -> Option<PublicTypeArnParts> {
    let (_, path) = arn.split_once(":type/")?;
    let mut segments = path.split('/');
    let type_kind = segments.next()?;
    let after_kind = segments.next()?;
    if type_kind.is_empty() || after_kind.is_empty() {
        return None;
    }
    // Whether the segment after the kind is the extension's own NAME — the form an extension
    // Amazon Web Services published carries, which names no publisher — or the publisher that
    // published it, with the name in the segment after that.
    let publishers_own = is_type_name(&after_kind.replace('-', "::"));
    let (publisher_id, type_name, pinned_at) = if publishers_own {
        ("", after_kind, 2)
    } else {
        (after_kind, segments.next()?, 3)
    };
    if type_name.is_empty() {
        return None;
    }
    let type_kind = [
        registry_type::RESOURCE,
        registry_type::MODULE,
        registry_type::HOOK,
    ]
    .into_iter()
    .find(|kind| arn_kind_segment(kind).eq_ignore_ascii_case(type_kind))
    .map_or_else(|| type_kind.to_ascii_uppercase(), str::to_string);
    // Everything past the name segment, kept whole: an extension's name carries no `/` of
    // its own, so whatever follows it is the version the ARN pins rather than part of the
    // name.
    let pinned = path
        .splitn(pinned_at + 1, '/')
        .nth(pinned_at)
        .unwrap_or_default();
    Some(PublicTypeArnParts {
        type_kind,
        publisher_id: publisher_id.to_string(),
        type_name: type_name.replace('-', "::"),
        public_version_number: (!pinned.is_empty()).then(|| pinned.to_string()),
    })
}

/// Whether `type_name` is a well-formed extension type name: the form the modelled `TypeName`
/// states — two to sixty-four letters or digits per segment, three segments separated by
/// `::`, optionally followed by `::MODULE`, within the member's declared length
/// (`smithy_query(query="shape", shape="TypeName")`: `@length(10..204)`, `@pattern
/// "^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}(::MODULE){0,1}$"`).
///
/// The registry resolves an extension BY this name, including the name it reads out of an
/// ARN ([`split_public_type_arn`]), so a name outside this form names no extension at all and
/// is refused with [`INVALID_TYPE_NAME`].
pub fn is_type_name(type_name: &str) -> bool {
    if type_name.len() < TYPE_NAME_MIN_LEN || type_name.len() > TYPE_NAME_MAX_LEN {
        return false;
    }
    let mut segments: Vec<&str> = type_name.split("::").collect();
    // "(::MODULE){0,1}": a module's name carries one further segment, which is that word.
    if segments.len() == 4 && segments.last() == Some(&registry_type::MODULE) {
        segments.pop();
    }
    segments.len() == 3
        && segments.iter().all(|segment| {
            (TYPE_NAME_SEGMENT_MIN_LEN..=TYPE_NAME_SEGMENT_MAX_LEN).contains(&segment.len())
                && segment.chars().all(|character| character.is_ascii_alphanumeric())
        })
}

/// The bounds `TypeName`'s own `@length` states.
const TYPE_NAME_MIN_LEN: usize = 10;
const TYPE_NAME_MAX_LEN: usize = 204;

/// The bounds `TypeName`'s pattern states for ONE `::`-separated segment.
const TYPE_NAME_SEGMENT_MIN_LEN: usize = 2;
const TYPE_NAME_SEGMENT_MAX_LEN: usize = 64;

/// The organization namespaces an extension name a caller registers may NOT open with: "the
/// following organization namespaces are reserved and can't be used in your extension names:
/// Alexa AMZN Amazon AWS Custom Dev"
/// (`smithy_query(query="doc", shape="RegisterTypeInput", member="TypeName")`), which is why
/// no registration of this account can own a name the registry's own published records carry.
pub const RESERVED_NAMESPACES: &[&str] = &["Alexa", "AMZN", "Amazon", "AWS", "Custom", "Dev"];

/// The reserved namespace `type_name` belongs to, where it belongs to one.
///
/// An extension name's FIRST `::`-separated segment is its organization namespace ("for
/// resource types, `company_or_organization::service::type`"), so only that whole segment is
/// reserved: a name whose namespace merely starts with one of them is another organization's.
/// Compared without regard to case, since the namespace a name claims does not depend on how
/// it is spelled.
pub fn reserved_namespace(type_name: &str) -> Option<&'static str> {
    let namespace = type_name.split("::").next().unwrap_or_default();
    RESERVED_NAMESPACES
        .iter()
        .copied()
        .find(|reserved| reserved.eq_ignore_ascii_case(namespace))
}

/// The message a registration in one of those namespaces is refused with: the name the
/// request spelled, and the namespace it claimed, which is the only other variable part.
pub fn restricted_namespace(type_name: &str, reserved: &str) -> String {
    format!(
        "Type name {type_name} is using a restricted namespace {}",
        reserved.to_ascii_lowercase()
    )
}

/// How many extension versions this account and Region may have REGISTERED at one time:
/// "you can have a maximum of 50 resource extension versions registered at a time. This
/// maximum is per account and per Region" (`RegisterType`), which is both the quota a
/// registration past it is refused by and the bound on how many versions one record keeps.
pub const MAX_REGISTERED_VERSIONS: usize = 50;

/// How long one registration request takes to conclude.
///
/// Chosen (CloudFormation publishes no duration for a registration): registering an
/// extension validates its schema and readies its handlers, which is why the call is
/// answered with a token to monitor instead of with the registered version, so the span is
/// longer than the status transitions of this service's own records
/// ([`crate::state::STATUS_TRANSITION_DELAY`]) — long enough that a caller that starts a
/// registration and then reads or lists its request still sees it running, and short enough
/// that nothing waits long for the conclusion. The instant is stored and nothing sleeps for
/// it.
pub const REGISTRATION_SPAN: Duration = Duration::from_secs(3);

/// Mint the token one registration request is addressed by: "the identifier for this
/// registration request. Use this registration token when calling DescribeTypeRegistration"
/// (`RegisterTypeOutput$RegistrationToken`).
///
/// The same minted identifier every other one this service hands out is built from
/// ([`crate::state::new_identifier_digits`] behind it), in the 8-4-4-4-12 form — 36
/// characters of hexadecimal and `-`, inside the modelled `RegistrationToken` length (1..128)
/// and pattern (`^[a-zA-Z0-9][-a-zA-Z0-9]*$`).
pub fn new_registration_token() -> String {
    super::new_identifier("type-registration")
}

/// The message the registry refuses a request naming an extension by a name that is no
/// extension type name at all with — including the name an ARN carries, which CloudFormation
/// reads out of it itself. Entirely fixed text: no part of it comes from the request or from
/// the store.
pub const INVALID_TYPE_NAME: &str = "TypeName is invalid";

/// The message the registry refuses a public-extension ARN that pins a public VERSION NUMBER
/// past the extension's name with.
///
/// A public extension ARN addresses the EXTENSION — its kind, its publisher and its name — and
/// which version of it a caller gets is said by the request's own version members, never by
/// the ARN, so an ARN carrying a further segment addresses nothing the registry can act on.
/// Entirely fixed text: no part of it comes from the request or from the store.
pub const PUBLIC_VERSION_NUMBER_IN_ARN: &str = "You cannot specify public version number in the arn";

/// How many versions of ONE public extension this registry holds, oldest reclaimed first.
///
/// Chosen (CloudFormation documents no ceiling on how many versions of a public extension a
/// publisher may publish): a publisher chooses how many publications it makes, and each one
/// adds a version to the record, so the one growth site a caller counts up has a bound.
pub const MAX_PUBLISHED_VERSIONS: usize = 50;

/// The `TypeVersionId` the next version of a record carries: one past the highest id the
/// record already holds, in the zero-padded form the registry's version ids take
/// (`@pattern("^[A-Za-z0-9-]+$")`).
///
/// The registry's ONE version-id minting rule, for a record of either visibility: a
/// publication adds a version to a public record and a registration adds one to a private
/// record, and both ids are read back the same way ("the version ID is the value at the end
/// of the ARN assigned to the extension version when it is registered"), so there is one
/// rule rather than one per record kind. (The name says `public` because publication was the
/// first writer to need it; `next_version_id` would read better, and renaming it means
/// touching `src/state.rs`, which the file-length rule puts out of reach.)
///
/// Counted from the ids rather than from the number of versions, so the id of a version the
/// record's own bound reclaimed ([`MAX_PUBLISHED_VERSIONS`],
/// [`MAX_REGISTERED_VERSIONS`]) is never handed to a second version.
pub fn next_public_version_id(versions: &[StoredTypeVersion]) -> String {
    let highest = versions
        .iter()
        .filter_map(|version| version.version_id.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    format!("{:08}", highest.saturating_add(1))
}

/// The version number CloudFormation assigns the FIRST publication of an extension:
/// "You cannot specify a version number the first time you publish a type. CloudFormation
/// automatically sets the first version number to be 1.0.0" (`PublishTypeInput$PublicVersionNumber`).
pub const FIRST_PUBLIC_VERSION_NUMBER: &str = "1.0.0";

/// The version number the publication after `latest` is assigned when the request names
/// none: "if you don't specify a version number, CloudFormation increments the version
/// number by one minor version release" — so the major version stays as it is, the minor
/// version advances by one, and the patch starts that new minor release at zero.
///
/// `None` for a number this cannot be computed from, which is one whose `MAJOR.MINOR`
/// prefix the `PublicVersionNumber` pattern (`^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(.*)$`) would
/// not admit, or one whose minor version has no successor.
pub fn next_minor_version_number(latest: &str) -> Option<String> {
    let mut segments = latest.split('.');
    let major: u64 = segments.next()?.parse().ok()?;
    let minor: u64 = segments.next()?.parse().ok()?;
    Some(format!("{major}.{}.0", minor.checked_add(1)?))
}

/// Whether an ARN's ACCOUNT segment is this account's — the account every ARN this emulator
/// mints carries ([`::emulator_server::emulator_account_id`]).
///
/// An ARN spells the account that owns what it addresses in its fifth colon-separated segment
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`), and the modelled
/// `PrivateTypeArn` requires twelve digits there. An ARN carrying another account's id
/// addresses nothing of THIS account, so it is not an ARN this registry reads any further —
/// what it names cannot be resolved "in your account" at all. `false` for a public
/// extension's ARN too, whose account segment is empty ([`public_type_arn`]).
///
/// What this is for: an account-scoped ARN of this account is one CloudFormation parses
/// itself, so the rules about the segments it carries — the version a registry ARN pins past
/// the extension's name ([`split_version_arn`]) — are read from the ARN alone, before the
/// registry is consulted, while an ARN of another account only ever reaches the
/// account-scoped existence refusal.
pub fn arn_names_this_account(arn: &str) -> bool {
    arn.split(':').nth(4) == Some(::emulator_server::emulator_account_id())
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

/// What contract testing concluded about one extension version, held until the clock makes
/// reporting the conclusion due.
///
/// `TestType` is answered while the tests are still running — the status it leaves on the
/// version is `IN_PROGRESS` — so the conclusion it computed waits here until [`Self::due_at`]
/// and [`StoredTypeVersion::settle`] is what installs it. No read invents a conclusion the
/// clock had not already made due, and nothing sleeps for one.
#[derive(Debug, Clone)]
pub struct PendingTypeTests {
    /// [`type_tests_status`]: the `PASSED` or `FAILED` the tests reached.
    pub status: String,
    /// `TypeTestsStatusDescription`, "the description of the test status", where the
    /// conclusion has something to describe.
    pub status_description: Option<String>,
    /// The instant reporting the conclusion becomes due.
    pub due_at: SystemTime,
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
    /// The contract-test conclusion the clock owes this version, once `TestType` has
    /// started testing it and until the conclusion is due (see [`PendingTypeTests`]).
    pub pending_tests: Option<PendingTypeTests>,
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
            pending_tests: None,
        }
    }

    /// Start contract testing this version of the extension `type_name`, which is what
    /// `TestType` does: the version's test status becomes `IN_PROGRESS` now, and the
    /// conclusion the tests reach over what the registry holds for the version
    /// ([`type_tests_outcome`]) becomes due `after` from `now`.
    ///
    /// Testing again restarts it: the operation is `@idempotent`, and "once you've
    /// initiated testing on an extension using TestType, you can pass the returned
    /// TypeVersionArn into DescribeType to monitor the current test status" — so what a
    /// second call leaves behind is the same in-progress test, concluding from the instant
    /// of that call.
    pub fn start_type_tests(&mut self, type_name: &str, now: SystemTime, after: Duration) {
        let (status, status_description) = type_tests_outcome(type_name, self);
        self.type_tests_status = Some(type_tests_status::IN_PROGRESS.to_string());
        // "The description of the test status" is the description of the status being
        // reported, so the conclusion's own arrives with the conclusion.
        self.type_tests_status_description = None;
        self.pending_tests = Some(PendingTypeTests {
            status,
            status_description,
            due_at: now + after,
        });
    }

    /// Whether this version is still in ACTIVE USE: `LIVE`, "the extension is activated or
    /// registered and can be used in CloudFormation operations" — as opposed to `DEPRECATED`,
    /// which "can no longer be used".
    pub fn is_live(&self) -> bool {
        self.deprecated_status == deprecated_status::LIVE
    }

    /// Mark this version `DEPRECATED`, "removing it from active use".
    pub fn deprecate(&mut self) {
        self.deprecated_status = deprecated_status::DEPRECATED.to_string();
    }

    /// Materialise the contract-test conclusion the clock has already made due, if any.
    pub fn settle(&mut self, now: SystemTime) {
        if self
            .pending_tests
            .as_ref()
            .is_none_or(|pending| pending.due_at > now)
        {
            return;
        }
        if let Some(concluded) = self.pending_tests.take() {
            self.type_tests_status = Some(concluded.status);
            self.type_tests_status_description = concluded.status_description;
        }
    }
}

/// What contract testing concludes about one version of the extension `type_name`, computed
/// from what the registry holds for that version.
///
/// Testing an extension is "determining if the ... model meets all necessary requirements",
/// and the model is the version's `Schema`, "the schema that defines the extension". What
/// the registry requires of that document is that it is a JSON object declaring the
/// extension it defines, under the `typeName` member the provider definition meta-schema
/// fixes (see [`crate::state::resource_schema`]). A version the registry holds no schema
/// for, one whose schema is not such a document, and one whose schema declares a DIFFERENT
/// extension each fail that requirement, and `FAILED` says which; anything else is
/// `PASSED`, the status "an extension must have ... before it can be published".
///
/// `PASSED` here therefore states exactly that: the model this registry HOLDS for the
/// version meets the requirements that can be checked against a stored schema. It is not
/// the verdict of a run of the extension's handlers against the contract-test suite — this
/// registry holds no handler package to run one over, so the conclusion is computed from
/// the record rather than from an execution.
pub fn type_tests_outcome(
    type_name: &str,
    version: &StoredTypeVersion,
) -> (String, Option<String>) {
    let version_id = &version.version_id;
    let failed = |reason: String| (type_tests_status::FAILED.to_string(), Some(reason));

    let Some(schema) = version.schema.as_deref() else {
        return failed(format!(
            "No schema is registered for version {version_id} of {type_name}, so its model \
             could not be tested"
        ));
    };
    let document = match ::serde_json::from_str::<::serde_json::Value>(schema) {
        Ok(::serde_json::Value::Object(document)) => document,
        _ => {
            return failed(format!(
                "The schema registered for version {version_id} of {type_name} is not a JSON \
                 object"
            ))
        }
    };
    match document
        .get("typeName")
        .and_then(::serde_json::Value::as_str)
    {
        Some(declared) if declared == type_name => (type_tests_status::PASSED.to_string(), None),
        Some(declared) => failed(format!(
            "The schema registered for version {version_id} of {type_name} declares typeName \
             {declared}"
        )),
        None => failed(format!(
            "The schema registered for version {version_id} of {type_name} declares no typeName"
        )),
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
    /// The account-level CONFIGURATIONS this Region's registry holds for the extension, one
    /// per alias `SetTypeConfiguration` has written under, in the order they were first
    /// written. Bounded at [`MAX_TYPE_CONFIGURATIONS`], oldest reclaimed first.
    pub configurations: Vec<StoredTypeConfiguration>,
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
            configurations: Vec::new(),
            versions: Vec::new(),
        }
    }

    /// Persist the account-level configuration of this extension under `alias`, and answer
    /// the ARN of the configuration data — what `SetTypeConfiguration` hands back.
    ///
    /// Setting a configuration under an alias the record already holds REPLACES that
    /// configuration (the call "specifies the configuration data for… an extension in the
    /// given account and Region", so there is one configuration per alias, not a history),
    /// keeping its place and its ARN. A new alias is added, and the oldest is reclaimed past
    /// [`MAX_TYPE_CONFIGURATIONS`] so the record cannot grow without limit.
    ///
    /// What is stored is the document as the registry can hold it
    /// ([`ascii_configuration`]), which is what a later read reports.
    pub fn set_configuration(
        &mut self,
        alias: &str,
        configuration: &str,
        now: SystemTime,
    ) -> String {
        let configuration = ascii_configuration(configuration);
        let arn = type_configuration_arn(&self.region, &self.type_kind, &self.type_name, alias);
        if let Some(held) = self
            .configurations
            .iter_mut()
            .find(|held| held.alias == alias)
        {
            held.configuration = configuration;
            held.last_updated = now;
            return held.arn.clone();
        }
        let over_the_bound =
            (self.configurations.len() + 1).saturating_sub(MAX_TYPE_CONFIGURATIONS);
        if over_the_bound > 0 {
            self.configurations.drain(..over_the_bound);
        }
        self.configurations.push(StoredTypeConfiguration {
            alias: alias.to_string(),
            arn: arn.clone(),
            configuration,
            last_updated: now,
        });
        arn
    }

    /// The configuration this record holds under `alias`, if any.
    pub fn configuration_with_alias(&self, alias: &str) -> Option<&StoredTypeConfiguration> {
        self.configurations
            .iter()
            .find(|held| held.alias == alias)
    }

    /// The configuration this record holds under one configuration ARN, if any.
    pub fn configuration_by_arn(&self, arn: &str) -> Option<&StoredTypeConfiguration> {
        self.configurations.iter().find(|held| held.arn == arn)
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

    /// Whether this record is a public third-party extension that is ACTIVATED in this
    /// account and Region right now — `IsActivated`, "whether the extension is activated in
    /// the account and Region".
    ///
    /// That is the one kind of record `DeactivateType` ("deactivates a public third-party
    /// extension ... when you no longer use it") can name: a private registration, an
    /// extension Amazon Web Services published (those carry no publisher id and nobody
    /// activated them) and an activation already deactivated are all outside it.
    pub fn is_activated_third_party(&self) -> bool {
        self.is_public_third_party() && self.is_activated == Some(true)
    }

    /// Deactivate this activation: what `DeactivateType` leaves behind.
    ///
    /// "Deactivating an extension deletes the configuration details that are associated with
    /// it", so the configuration data this account held for the extension goes; `IsActivated`
    /// becomes false, since the extension is no longer activated in this account and Region;
    /// "deactivated extensions aren't automatically updated if a new version of the extension
    /// is released", so `AutoUpdate` is false; and "once deactivated, an extension can't be
    /// used in any CloudFormation operation", which is what `DEPRECATED` states of every
    /// version of it ("the extension has been deactivated or deregistered and can no longer
    /// be used in CloudFormation operations").
    ///
    /// The record itself stays in the registry, deprecated rather than dropped — that is what
    /// a deprecation status meaning "has been deactivated" is for — so a later read still
    /// reports what this account and Region hold for the extension.
    pub fn deactivate(&mut self) {
        self.is_activated = Some(false);
        self.auto_update = Some(false);
        self.configuration_schema = None;
        self.deprecate_every_version();
    }

    /// Mark every version of this extension `DEPRECATED`: what the extension ITSELF being
    /// "marked as deprecated in the registry" is, since a deprecation status is carried per
    /// version and a record with no version left in active use is one nothing can be used
    /// from.
    ///
    /// The record itself stays in the registry, deprecated rather than dropped, so a later
    /// read still reports what this account and Region hold for the extension — "to view the
    /// deprecation status of an extension or extension version, use DescribeType".
    pub fn deprecate_every_version(&mut self) {
        for version in &mut self.versions {
            version.deprecate();
        }
    }

    /// The versions of this extension that are still in ACTIVE USE, in the order the record
    /// carries them.
    pub fn live_versions(&self) -> impl Iterator<Item = &StoredTypeVersion> {
        self.versions.iter().filter(|version| version.is_live())
    }

    /// Whether this extension is still in ACTIVE USE: it holds at least one `LIVE` version.
    ///
    /// A record every version of which is `DEPRECATED` is a deregistered or deactivated
    /// extension: "deprecated extensions or extension versions cannot be used in
    /// CloudFormation operations".
    pub fn is_live(&self) -> bool {
        self.live_versions().next().is_some()
    }

    /// The version with this `TypeVersionId`, if the registry holds it.
    pub fn version(&self, version_id: &str) -> Option<&StoredTypeVersion> {
        self.versions
            .iter()
            .find(|version| version.version_id == version_id)
    }

    /// [`Self::version`], for a writer that records something on the version.
    pub fn version_mut(&mut self, version_id: &str) -> Option<&mut StoredTypeVersion> {
        self.versions
            .iter_mut()
            .find(|version| version.version_id == version_id)
    }

    /// The version a request selects: the one its `VersionId` names, or — where it names
    /// none — the record's default version, "the default version of the extension in your
    /// account and Region". `None` where the record holds no such version.
    pub fn selected_version(&self, version_id: Option<&str>) -> Option<&StoredTypeVersion> {
        match version_id {
            Some(version_id) => self.version(version_id),
            None => self.default_version(),
        }
    }

    /// Materialise every contract-test conclusion the clock has made due, over every
    /// version this record holds.
    pub fn settle(&mut self, now: SystemTime) {
        for version in &mut self.versions {
            version.settle(now);
        }
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

    /// Make `version_id` the version a request that names none resolves to — "the default
    /// version of an extension will be used in CloudFormation operations" — which is what
    /// `SetTypeDefaultVersion` writes.
    ///
    /// A record carries ONE default version, so this replaces whichever version was the
    /// default before; the versions themselves are untouched, since which one is the default
    /// is a property of the record rather than of a version.
    pub fn set_default_version(&mut self, version_id: &str) {
        self.default_version_id = Some(version_id.to_string());
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

    /// Record one freshly registered version on this record, keeping the versions it holds
    /// bounded at [`MAX_REGISTERED_VERSIONS`].
    ///
    /// The oldest version is reclaimed past that bound — except the record's DEFAULT version,
    /// which is the one a request naming no version resolves to
    /// ([`Self::default_version`]), so reclaiming it would leave the record readable by
    /// version id and unreadable without one. A record every version of which is the default
    /// cannot happen (there is one), so the walk always has a version to reclaim while it is
    /// past the bound.
    pub fn record_version(&mut self, version: StoredTypeVersion) {
        self.versions.push(version);
        while self.versions.len() > MAX_REGISTERED_VERSIONS {
            let oldest_reclaimable = self
                .versions
                .iter()
                .position(|held| !self.is_default_version(&held.version_id));
            match oldest_reclaimable {
                Some(at) => {
                    self.versions.remove(at);
                }
                None => break,
            }
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
/// set the emulator holds is [`aws_published_resource_types`]: the resource types it
/// actually provisions, and the ones no resource provider stands behind; a name outside
/// that set is not in this registry, which is what makes the answer for it a
/// `TypeNotFoundException` instead of a record nothing stands behind.
///
/// The record carries only what is known about such an extension: it is `PUBLIC`, it is
/// `LIVE`, it carries the provisioning behaviour its resource provider's handlers give it
/// ([`published_provisioning_type`]), it has no publisher id — "extensions provided by
/// Amazon Web Services are not assigned a publisher ID" — its `SourceUrl` is the
/// open-source repository AWS publishes that type's resource provider in
/// ([`aws_published_source_url`]), and its
/// registration instants are the instant this registry came into being
/// ([`REGISTRY_GENESIS`]), which is how long it has held them. Its `Schema` is "the schema
/// that defines the extension": the resource schema this registry holds for the type
/// ([`crate::state::resource_schema`]), derived from the properties the type publishes. The
/// description
/// and the documentation URL are NOT known here, so they stay absent rather than invented.
/// The record a single read of the Amazon-Web-Services-published registry answers with: the
/// resource type this emulator provisions, or the extension this registry publishes
/// ([`PUBLIC_EXTENSION_CATALOG`]) — both of which a listing reports too
/// ([`aws_published_all`]), so the two reads cannot disagree about one extension.
pub fn aws_published(region: &str, type_kind: &str, type_name: &str) -> Option<StoredExtension> {
    if let Some(published) = published_catalog_entry(type_kind, type_name) {
        return Some(published_catalog_record(region, published));
    }
    if type_kind != registry_type::RESOURCE
        || !aws_published_resource_types().any(|published| published == type_name)
    {
        return None;
    }
    Some(aws_published_record(region, type_name))
}

/// Every RESOURCE type the registry of an account and Region holds an
/// Amazon-Web-Services-published record for.
///
/// Two catalogues, because a public resource type is in this registry whether or not
/// anything provisions one: the types this emulator provisions
/// ([`crate::state::template::IMPLEMENTED_RESOURCE_TYPES`]), and the types no resource
/// provider stands behind at all
/// ([`::service_common::cfn_provisioning::NON_PROVISIONABLE_RESOURCE_TYPES`]) — which
/// CloudFormation serves itself inside a stack and which the registry therefore reports in
/// the provisioning class that says so ([`published_provisioning_type`]). A read of ONE
/// type and a listing of all of them both resolve the set here, so they cannot disagree
/// about which types this registry holds.
pub fn aws_published_resource_types() -> impl Iterator<Item = &'static str> {
    crate::state::template::IMPLEMENTED_RESOURCE_TYPES
        .iter()
        .copied()
        .chain(
            ::service_common::cfn_provisioning::NON_PROVISIONABLE_RESOURCE_TYPES
                .iter()
                .copied(),
        )
}

/// The provisioning behaviour the registry reports for the Amazon-Web-Services-published
/// resource type `type_name`: "for resource types, the provisioning behavior of the
/// resource type", which the handlers that type's resource provider publishes decide.
///
/// A type CloudFormation serves ITSELF inside a stack publishes no handler package at all,
/// so it has no create, read or delete handler and is `NON_PROVISIONABLE`: "the resource
/// type doesn't include create, read, and delete handlers, and therefore can't actually be
/// provisioned". Every other published type here is provisioned by a provider that creates,
/// reads, updates and deletes it — the emulator's own stack operations do all four — which
/// is `FULLY_MUTABLE`: "the resource type includes an update handler to process updates to
/// the type during stack update operations".
pub fn published_provisioning_type(type_name: &str) -> &'static str {
    if ::service_common::cfn_provisioning::is_non_provisionable(type_name) {
        provisioning_type::NON_PROVISIONABLE
    } else {
        provisioning_type::FULLY_MUTABLE
    }
}

/// Every Amazon-Web-Services-published public extension the registry of this Region holds,
/// in the order the catalogues carry them.
///
/// The same records [`aws_published`] answers one at a time, for a read that reports what
/// the registry HOLDS rather than one extension a request named: both are built by
/// [`aws_published_record`] and [`published_catalog_record`], so the listing and the single
/// read cannot disagree about one of them.
///
/// The registry's public catalogue is not one KIND of extension: the resource types Amazon
/// Web Services provides, and the extensions it PUBLISHES
/// ([`PUBLIC_EXTENSION_CATALOG`]), are both in it.
pub fn aws_published_all(region: &str) -> Vec<StoredExtension> {
    aws_published_resource_types()
        .map(|type_name| aws_published_record(region, type_name))
        .chain(
            PUBLIC_EXTENSION_CATALOG
                .iter()
                .map(|published| published_catalog_record(region, published)),
        )
        .collect()
}

/// One PUBLIC extension the registry of every account and Region holds because Amazon Web
/// Services PUBLISHED it, rather than because it provides the resource type
/// ([`aws_published_record`]).
///
/// A published extension belongs to the publisher that published it, which is what makes it
/// a public THIRD-PARTY record here ([`StoredExtension::is_public_third_party`]): its
/// publisher id, publisher name and verified identity are reported for it, while the
/// resource types Amazon Web Services provides carry no publisher id at all ("extensions
/// provided by Amazon Web Services are not assigned a publisher ID").
#[derive(Debug, Clone, Copy)]
pub struct PublishedExtension {
    /// [`registry_type`]: the kind of extension.
    pub type_kind: &'static str,
    /// The extension's own name, in the form the modelled `TypeName` states.
    pub type_name: &'static str,
    /// The public name of the publisher that published it, which is what
    /// [`published_publisher`] reads back and what its `PublisherId` is derived from.
    pub publisher_name: &'static str,
    /// `Description`, "the description of the extension".
    pub description: &'static str,
}

/// The public extensions this registry holds that are NOT resource types: the HOOKS Amazon
/// Web Services publishes to the CloudFormation registry, which a client enumerating the
/// public registry sees beside the resource types.
///
/// Source: the Hook types the CloudFormation Hooks documentation publishes by name — a Guard
/// Hook, which evaluates Guard rules against a stack operation's targets, and a Lambda Hook,
/// which hands those targets to a Lambda function. This service's own model names the first
/// of them (`GetHookResultOutput$TypeName`: "if you activate AWS::Hooks::GuardHook with a
/// type name alias"), which is why the registry can report a record for it rather than
/// inventing one.
///
/// It carries no MODULE: a module is a package of template fragments its PUBLISHER authors
/// ("for modules, `company_or_organization::service::type::MODULE`",
/// `RegisterTypeInput$TypeName`), so a MODULE record enters this registry when a
/// registration or an activation writes one — and such a record is listed and described
/// through the very same code path these are.
pub const PUBLIC_EXTENSION_CATALOG: &[PublishedExtension] = &[
    PublishedExtension {
        type_kind: registry_type::HOOK,
        type_name: "AWS::Hooks::GuardHook",
        publisher_name: AWS_PUBLISHER_NAME,
        description: "Evaluates the targets of a stack operation against a set of \
                      CloudFormation Guard rules.",
    },
    PublishedExtension {
        type_kind: registry_type::HOOK,
        type_name: "AWS::Hooks::LambdaHook",
        publisher_name: AWS_PUBLISHER_NAME,
        description: "Invokes a Lambda function to evaluate the targets of a stack \
                      operation.",
    },
];

/// The public name of the publisher Amazon Web Services publishes its own extensions under.
const AWS_PUBLISHER_NAME: &str = "Amazon Web Services";

/// What this registry knows about the publisher an extension of
/// [`PUBLIC_EXTENSION_CATALOG`] was published by, beyond the id the record carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogPublisher {
    /// `PublisherName`, "the publisher name, as defined in the public profile for that
    /// publisher in the service used to verify the publisher identity".
    pub publisher_name: &'static str,
    /// `PublisherIdentity`, "the type of account used as the identity provider when
    /// registering this publisher with CloudFormation" ([`crate::state::publisher`]).
    pub identity_provider: &'static str,
}

/// The publisher of a catalogued public extension, read back by the `PublisherId` its record
/// carries; `None` for any other id.
///
/// A publisher is a record of its OWN, so the name and the verified identity a summary
/// reports are read back from the publisher rather than held a second time on every
/// extension it published. These publishers registered before any account read this registry
/// — their extensions are in it without anybody activating them — so the registration is the
/// catalogue's rather than one `RegisterPublisher` wrote, which
/// [`crate::state::ComAmazonawsCloudformationCloudformationState::publisher`] answers for.
/// Each is verified through its Amazon Web Services Marketplace account, which is the
/// identity provider a registration that supplied no source-control connection reports
/// ([`crate::state::publisher::StoredPublisher::identity_provider`]).
pub fn published_publisher(publisher_id: &str) -> Option<CatalogPublisher> {
    PUBLIC_EXTENSION_CATALOG
        .iter()
        .map(|published| published.publisher_name)
        .find(|publisher_name| published_publisher_id(publisher_name) == publisher_id)
        .map(|publisher_name| CatalogPublisher {
            publisher_name,
            identity_provider: crate::state::publisher::identity_provider::AWS_MARKETPLACE,
        })
}

/// The `PublisherId` this registry holds the publisher `publisher_name` under.
///
/// DERIVED from the publisher's name rather than minted per call: a publisher id is stable
/// ("this publisher ID applies to your account in all Amazon Web Services Regions") and no
/// call registered these publishers, so the same name resolves the same id in every Region
/// and on every read — which is what lets [`published_publisher`] read the publisher back
/// out of an id a response handed a client. The 32 hexadecimal digits of the digest satisfy
/// the modelled `PublisherId` (`@length(1..40)`, `@pattern("^[0-9a-zA-Z]{12,40}$")`), the
/// same form [`crate::state::new_publisher_id`] mints for a registration.
pub fn published_publisher_id(publisher_name: &str) -> String {
    crate::state::stable_identifier_digits(publisher_name, 32)
}

/// The record a catalogued public extension ([`PUBLIC_EXTENSION_CATALOG`]) is reported as.
///
/// It carries what is known about a published extension: it is `PUBLIC` and `LIVE`, it
/// belongs to the publisher that published it, its description is the catalogue's, and the
/// public version this registry holds it at is the first one a publication is assigned
/// ([`FIRST_PUBLIC_VERSION_NUMBER`]) — which is also the latest available version of it,
/// since the registry holds that one publication. `IsActivated` is false: an activation is
/// what puts a public extension to use in an account and Region, and nothing here has
/// activated it. Its registration instants are the instant this registry came into being
/// ([`REGISTRY_GENESIS`]), the way the provided resource types' are.
///
/// What it deliberately does NOT carry: a `Schema`, because the document that defines a
/// published extension comes from the handler package its publisher registered and no
/// package was uploaded here; a provisioning behaviour, which is a resource type's
/// ("for resource types, the provisioning behavior of the resource type"); and a source or
/// documentation URL, which the catalogue does not record.
fn published_catalog_record(region: &str, published: &PublishedExtension) -> StoredExtension {
    let arn = public_type_arn(region, published.type_kind, published.type_name);
    let version = StoredTypeVersion {
        version_id: AWS_PUBLISHED_VERSION_ID.to_string(),
        arn: arn.clone(),
        description: Some(published.description.to_string()),
        schema: None,
        provisioning_type: None,
        deprecated_status: deprecated_status::LIVE.to_string(),
        type_tests_status: None,
        type_tests_status_description: None,
        source_url: None,
        documentation_url: None,
        public_version_number: Some(FIRST_PUBLIC_VERSION_NUMBER.to_string()),
        execution_role_arn: None,
        logging_config: None,
        required_activated_types: Vec::new(),
        time_created: Some(*REGISTRY_GENESIS),
        last_updated: Some(*REGISTRY_GENESIS),
        // Nobody can test a public extension, so no conclusion is ever owed on one.
        pending_tests: None,
    };
    StoredExtension {
        type_kind: published.type_kind.to_string(),
        type_name: published.type_name.to_string(),
        arn,
        region: region.to_string(),
        visibility: visibility::PUBLIC.to_string(),
        // "For public extensions, both those provided by Amazon Web Services and published
        // by third parties, CloudFormation returns null."
        default_version_id: None,
        configuration_schema: None,
        publisher_id: Some(published_publisher_id(published.publisher_name)),
        // Nothing was activated under an alias: the local name IS the public name.
        original_type_name: None,
        original_type_arn: None,
        latest_public_version: Some(FIRST_PUBLIC_VERSION_NUMBER.to_string()),
        is_activated: Some(false),
        // `AutoUpdate` is a property of an ACTIVATION, and nothing activated this record.
        auto_update: None,
        // Nothing has configured it: a configuration is what `SetTypeConfiguration` writes.
        configurations: Vec::new(),
        versions: vec![version],
    }
}

/// The catalogued public extension of `type_kind` named `type_name`, if this registry
/// publishes one ([`PUBLIC_EXTENSION_CATALOG`]).
fn published_catalog_entry(type_kind: &str, type_name: &str) -> Option<&'static PublishedExtension> {
    PUBLIC_EXTENSION_CATALOG
        .iter()
        .find(|published| published.type_kind == type_kind && published.type_name == type_name)
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
    if let Some(published) = PUBLIC_EXTENSION_CATALOG.iter().find(|published| {
        public_type_arn(region, published.type_kind, published.type_name) == arn
    }) {
        return Some(published_catalog_record(region, published));
    }
    let type_name = aws_published_resource_types()
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
/// (`AWS::S3::Bucket` -> `.../aws-cloudformation-resource-providers-s3.git`). `SourceUrl` is
/// "the URL of the source code for the extension", and that is the URL for one of these,
/// derived from the type name rather than held as a per-type constant.
///
/// The URL is the repository's CLONE url, so it carries the [`GIT_REPOSITORY_SUFFIX`] a
/// caller resolving the extension's source hands straight to a client: a source URL without
/// it names a web page rather than a repository.
///
/// It is also the `sourceUrl` of the resource schema this registry derives for the type
/// ([`crate::state::resource_schema`]): a published resource schema names that same
/// repository, in the same form.
pub fn aws_published_source_url(type_name: &str) -> Option<String> {
    let service = type_name.split("::").nth(1)?;
    if service.is_empty() {
        return None;
    }
    Some(format!(
        "https://github.com/aws-cloudformation/aws-cloudformation-resource-providers-{}{GIT_REPOSITORY_SUFFIX}",
        service.to_ascii_lowercase()
    ))
}

/// What a git repository's clone url ends in, which is what makes
/// [`aws_published_source_url`] a URL something can be cloned from.
pub const GIT_REPOSITORY_SUFFIX: &str = ".git";

/// The version row a PUBLICATION writes on a public extension record: the model the
/// published version of the private extension carries, addressed by the public version
/// number it was published under.
///
/// A public extension version carries what is public about the version it was published
/// from — its description, its `Schema` ("the schema that defines the extension"), its
/// provisioning behaviour and the URLs the registration supplied — and none of the members
/// the model reports for registered PRIVATE extension versions alone: the execution role
/// and the logging configuration belong to the registration in the publisher's own account,
/// and the contract-test status is one "CloudFormation doesn't return ... for public
/// extensions, whether they are activated in your account". Its `TypeVersionId` is the
/// record's own ([`next_public_version_id`]): the version ids of a public extension count up
/// in the registry that holds it, independently of the private registration's.
pub fn published_version(
    public_arn: &str,
    version_id: &str,
    publication: &Publication<'_>,
) -> StoredTypeVersion {
    StoredTypeVersion {
        version_id: version_id.to_string(),
        // A public extension is addressed by the extension's own ARN — that is the ARN
        // CloudFormation reports for one — so the version carries it unsuffixed, as the
        // Amazon-Web-Services-published records do.
        arn: public_arn.to_string(),
        public_version_number: Some(publication.public_version_number.to_string()),
        // Private-extension members: they describe the registration this version was
        // published FROM, not the public extension the publication created.
        execution_role_arn: None,
        logging_config: None,
        type_tests_status: None,
        type_tests_status_description: None,
        // Nobody can test a public extension, so no conclusion is ever owed on one.
        pending_tests: None,
        // This version entered the public registry when it was published.
        time_created: Some(publication.published_at),
        last_updated: Some(publication.published_at),
        ..publication.version.clone()
    }
}

/// A PUBLIC extension record this Region's registry holds because a publisher published one
/// version of their private extension to it.
///
/// `PublishType` "publishes the specified extension to the CloudFormation registry as a
/// public extension in this Region. Public extensions are available for use by all
/// CloudFormation users", so the publication is a record of its own rather than a change to
/// the private registration it was published from: a public extension with the publisher id
/// the publishing account registered under, addressed by the account-less ARN every public
/// extension carries ([`public_type_arn`]).
pub fn published_record(
    publication: &Publication<'_>,
    version: StoredTypeVersion,
) -> StoredExtension {
    StoredExtension {
        type_kind: publication.type_kind.to_string(),
        type_name: publication.type_name.to_string(),
        arn: publication.public_arn(),
        region: publication.region.to_string(),
        visibility: visibility::PUBLIC.to_string(),
        // "For public extensions, both those provided by Amazon Web Services and published
        // by third parties, CloudFormation returns null."
        default_version_id: None,
        configuration_schema: None,
        publisher_id: Some(publication.publisher_id.to_string()),
        // Nothing was activated under an alias: this record IS the public extension, not a
        // local activation of somebody else's.
        original_type_name: None,
        original_type_arn: None,
        latest_public_version: version.public_version_number.clone(),
        // `IsActivated` is "whether the extension is activated in the account and Region",
        // and publishing an extension activates it nowhere — `ActivateType` is what does.
        is_activated: Some(false),
        // `AutoUpdate` is a property of an ACTIVATION ("whether CloudFormation
        // automatically updates the extension in this account and Region when a new minor
        // version is published"), so a record nobody has activated carries none.
        auto_update: None,
        // A configuration is what `SetTypeConfiguration` writes against an extension in an
        // account and Region; publishing one configures it nowhere.
        configurations: Vec::new(),
        versions: vec![version],
    }
}

/// What ONE publication states: the extension of this Region being published, the publisher
/// publishing it, the version of it being published and the public version number that
/// version is published under.
///
/// `PublishType` resolves all of it before any of it is written —
/// [`crate::state::ComAmazonawsCloudformationCloudformationState::publish_extension_version`]
/// is what writes it — so the record and the version a publication produces are built from
/// one value rather than from members that could disagree.
#[derive(Debug, Clone, Copy)]
pub struct Publication<'a> {
    /// The Region whose registry the public extension is published into: "publishes the
    /// specified extension to the CloudFormation registry as a public extension in this
    /// Region".
    pub region: &'a str,
    /// [`registry_type`]: the kind of extension being published.
    pub type_kind: &'a str,
    /// The name of the extension being published.
    pub type_name: &'a str,
    /// `PublisherId`, "the ID assigned this account by CloudFormation for publishing
    /// extensions": the publisher the public extension belongs to.
    pub publisher_id: &'a str,
    /// The version of the private extension being published, which is its default version:
    /// what the public extension's version is built from.
    pub version: &'a StoredTypeVersion,
    /// `PublicVersionNumber`, the number this publication is assigned.
    pub public_version_number: &'a str,
    /// The instant of the publishing call, which the published version entered the registry
    /// at.
    pub published_at: SystemTime,
}

impl Publication<'_> {
    /// The ARN the public extension this publication produces is addressed by: the
    /// account-less ARN every public extension carries ([`public_type_arn`]).
    pub fn public_arn(&self) -> String {
        public_type_arn(self.region, self.type_kind, self.type_name)
    }
}

/// The record [`aws_published`] and [`aws_published_by_arn`] both answer with.
fn aws_published_record(region: &str, type_name: &str) -> StoredExtension {
    let arn = public_type_arn(region, registry_type::RESOURCE, type_name);
    let provisionable =
        published_provisioning_type(type_name) != provisioning_type::NON_PROVISIONABLE;
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
        // The provisioning behaviour this type's own resource provider gives it, which for
        // a type no provider stands behind is the class that states exactly that.
        provisioning_type: Some(published_provisioning_type(type_name).to_string()),
        deprecated_status: deprecated_status::LIVE.to_string(),
        // "CloudFormation doesn't return this information for public extensions": the
        // contract-test status belongs to registered private extension versions.
        type_tests_status: None,
        type_tests_status_description: None,
        // "The URL of the source code for the extension": the repository the type's
        // resource provider is published in — which a type no resource provider stands
        // behind has none of, so it names none rather than a repository holding nothing.
        source_url: provisionable
            .then(|| aws_published_source_url(type_name))
            .flatten(),
        documentation_url: None,
        public_version_number: None,
        execution_role_arn: None,
        logging_config: None,
        required_activated_types: Vec::new(),
        // Every extension version in the registry carries the instant it entered it; for
        // one nobody registered that is [`REGISTRY_GENESIS`].
        time_created: Some(*REGISTRY_GENESIS),
        last_updated: Some(*REGISTRY_GENESIS),
        // Nobody can test a public extension, so no conclusion is ever owed on one.
        pending_tests: None,
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
        // Nothing has configured it: a configuration is what `SetTypeConfiguration` writes.
        configurations: Vec::new(),
        versions: vec![version],
    }
}

/// The configuration document the registry holds for what one `SetTypeConfiguration` call
/// submitted: the submitted text with every codepoint outside ASCII replaced by one `?`.
///
/// CloudFormation stores an extension's configuration data as ASCII text
/// ([`super::ascii_text`], the same rule a stack's tags are held by), so a document that
/// carries a codepoint outside it reads back with that codepoint replaced one-for-one — the
/// document's STRUCTURE is untouched (every delimiter, name and nesting level survives,
/// because each of those is ASCII already) and only the characters it cannot hold change.
/// Applied at the WRITE, so what a read reports is what the registry holds rather than a
/// second transformation a read invented.
pub fn ascii_configuration(configuration: &str) -> String {
    super::ascii_text(configuration)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every ARN this registry PUBLISHES for a public extension is one it reads back: the
    /// form an extension a publisher published carries (with its publisher id) and the form
    /// one Amazon Web Services published carries (with none), each with and without a public
    /// version number pinned past the name.
    ///
    /// The rows differ along what the two forms generalise over: whether the ARN names a
    /// publisher, the kind of extension, whether the name is a module's (which carries one
    /// further segment), and whether a version is pinned.
    #[test]
    fn every_published_public_extension_arn_is_read_back() {
        const REGION: &str = "us-east-1";
        const PUBLISHER: &str = "abcdef0123456789abcd";
        // One row: the ARN, and the four things reading it back must answer.
        struct Row {
            case: &'static str,
            arn: String,
            type_kind: &'static str,
            publisher_id: &'static str,
            type_name: &'static str,
            pinned: Option<&'static str>,
        }
        let rows: &[Row] = &[
            Row {
                case: "a Hook Amazon Web Services published, as this registry mints its ARN",
                arn: public_type_arn(REGION, registry_type::HOOK, "AWS::Hooks::GuardHook"),
                type_kind: registry_type::HOOK,
                publisher_id: "",
                type_name: "AWS::Hooks::GuardHook",
                pinned: None,
            },
            Row {
                case: "a resource type Amazon Web Services published",
                arn: public_type_arn(REGION, registry_type::RESOURCE, "AWS::SQS::Queue"),
                type_kind: registry_type::RESOURCE,
                publisher_id: "",
                type_name: "AWS::SQS::Queue",
                pinned: None,
            },
            Row {
                case: "a module Amazon Web Services published, whose name carries MODULE",
                arn: public_type_arn(REGION, registry_type::MODULE, "AWS::Network::Vpc::MODULE"),
                type_kind: registry_type::MODULE,
                publisher_id: "",
                type_name: "AWS::Network::Vpc::MODULE",
                pinned: None,
            },
            Row {
                case: "a Hook a publisher published",
                arn: format!(
                    "arn:aws:cloudformation:{REGION}::type/hook/{PUBLISHER}/Example-Guard-Hook"
                ),
                type_kind: registry_type::HOOK,
                publisher_id: PUBLISHER,
                type_name: "Example::Guard::Hook",
                pinned: None,
            },
            Row {
                case: "a publisher's extension with a public version pinned past the name",
                arn: format!(
                    "arn:aws:cloudformation:{REGION}::type/resource/{PUBLISHER}/Example-Store-Bucket/1.0.0"
                ),
                type_kind: registry_type::RESOURCE,
                publisher_id: PUBLISHER,
                type_name: "Example::Store::Bucket",
                pinned: Some("1.0.0"),
            },
            Row {
                case: "an AWS-published extension with a public version pinned past the name",
                arn: format!(
                    "arn:aws:cloudformation:{REGION}::type/hook/AWS-Hooks-LambdaHook/2.1.0"
                ),
                type_kind: registry_type::HOOK,
                publisher_id: "",
                type_name: "AWS::Hooks::LambdaHook",
                pinned: Some("2.1.0"),
            },
        ];
        for row in rows {
            let Row {
                case,
                arn,
                type_kind,
                publisher_id,
                type_name,
                pinned,
            } = row;
            let read = split_public_type_arn(arn);
            assert_eq!(
                read,
                Some(PublicTypeArnParts {
                    type_kind: (*type_kind).to_string(),
                    publisher_id: (*publisher_id).to_string(),
                    type_name: (*type_name).to_string(),
                    public_version_number: pinned.map(str::to_string),
                }),
                "case {case:?}: expected {arn} to be read back whole, got {read:?}"
            );
            assert!(
                is_type_name(type_name),
                "case {case:?}: the name read out of the ARN must be an extension type name, \
                 got {type_name:?}"
            );
        }
    }

    /// An ARN that names no public extension is read as naming none.
    #[test]
    fn an_arn_that_names_no_public_extension_reads_as_none() {
        let rows: &[(&str, &str)] = &[
            ("no type path at all", "arn:aws:cloudformation:us-east-1::stack/a/0000"),
            ("nothing after the kind", "arn:aws:cloudformation:us-east-1::type/hook"),
            ("an empty kind", "arn:aws:cloudformation:us-east-1::type//Example-Guard-Hook"),
            (
                "a publisher with no name after it",
                "arn:aws:cloudformation:us-east-1::type/hook/abcdef0123456789abcd",
            ),
            (
                "a segment that is neither a name nor a publisher with a name",
                "arn:aws:cloudformation:us-east-1::type/hook/garbage",
            ),
        ];
        for (case, arn) in rows {
            let read = split_public_type_arn(arn);
            assert!(
                read.is_none(),
                "case {case:?}: expected {arn} to name no public extension, got {read:?}"
            );
        }
    }
}
