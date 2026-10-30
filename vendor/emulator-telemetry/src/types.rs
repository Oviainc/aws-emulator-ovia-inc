// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! Domain newtypes and closed wire enums for the telemetry client.
//!
//! Every value that reaches the wire is either a validated newtype (charset +
//! length constrained, so ARNs, paths, account ids, and other free-form text
//! can never ride along in a dimension) or a closed enum that serializes to a
//! fixed token from the backend's accepted vocabulary. Raw strings are parsed
//! ONCE at the input boundary; past that point only these types travel.

use crate::policy;
use std::fmt;
use std::sync::Arc;

/// Why a dimension string failed validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DimensionErrorReason {
    /// The value was empty.
    Empty,
    /// The value exceeded the field's maximum length.
    TooLong,
    /// The value carried bytes outside the closed dimension charset.
    Charset,
    /// An operation name was not shaped like an AWS operation name.
    OperationShape,
}

/// A dimension value failed validation and was rejected before it could reach
/// the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DimensionError {
    field: &'static str,
    reason: DimensionErrorReason,
}

impl DimensionError {
    fn new(field: &'static str, reason: DimensionErrorReason) -> Self {
        Self { field, reason }
    }

    /// The rejected field's name.
    pub fn field(&self) -> &'static str {
        self.field
    }

    /// Why the value was rejected.
    pub fn reason(&self) -> DimensionErrorReason {
        self.reason
    }
}

impl fmt::Display for DimensionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self.reason {
            DimensionErrorReason::Empty => "must not be empty",
            DimensionErrorReason::TooLong => "exceeds maximum length",
            DimensionErrorReason::Charset => {
                "may only contain ASCII alphanumerics, '.', '_', and '-'"
            }
            DimensionErrorReason::OperationShape => {
                "must be a PascalCase AWS operation name: ASCII letters and digits, \
                 starting uppercase, with at least one lowercase letter"
            }
        };
        write!(f, "invalid telemetry {}: {}", self.field, reason)
    }
}

impl std::error::Error for DimensionError {}

/// Validate a dimension string: 1..=`max_len` chars drawn from ASCII
/// alphanumerics plus [`policy::DIMENSION_EXTRA_ALLOWED_BYTES`]. The charset
/// deliberately excludes `:`, `/`, and whitespace so ARNs, URLs, file paths,
/// and prose cannot pass as a dimension.
fn validate_dimension(
    field: &'static str,
    value: &str,
    max_len: usize,
) -> Result<(), DimensionError> {
    if value.is_empty() {
        return Err(DimensionError::new(field, DimensionErrorReason::Empty));
    }
    if value.len() > max_len {
        return Err(DimensionError::new(field, DimensionErrorReason::TooLong));
    }
    if !value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || policy::DIMENSION_EXTRA_ALLOWED_BYTES.contains(&b))
    {
        return Err(DimensionError::new(field, DimensionErrorReason::Charset));
    }
    Ok(())
}

/// Validate an operation name: a dimension of at most `max_len` bytes that is
/// additionally shaped like an AWS Smithy operation name, the backend
/// contract's `operation.pattern` `^(?=.*[a-z])[A-Z][A-Za-z0-9]{0,63}$`. It
/// starts with an ASCII uppercase letter, holds only ASCII letters and digits,
/// and has at least one lowercase letter, so numbers, ARNs, access key ids,
/// emails, and resource names cannot pass.
fn validate_operation_name(
    field: &'static str,
    value: &str,
    max_len: usize,
) -> Result<(), DimensionError> {
    validate_dimension(field, value, max_len)?;
    let bytes = value.as_bytes();
    if !bytes[0].is_ascii_uppercase()
        || !bytes.iter().all(u8::is_ascii_alphanumeric)
        || !bytes.iter().any(u8::is_ascii_lowercase)
    {
        return Err(DimensionError::new(
            field,
            DimensionErrorReason::OperationShape,
        ));
    }
    Ok(())
}

macro_rules! dimension_newtype {
    ($(#[$doc:meta])* $name:ident, $field:literal, $max_len:expr, $validate:path) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(Arc<str>);

        impl $name {
            /// Validate and wrap a raw string. Crate-internal: the public
            /// recording API only accepts model-derived handles, which fold
            /// through [`Self::fold`] instead of rejecting.
            pub(crate) fn new(value: impl AsRef<str>) -> Result<Self, DimensionError> {
                let value = value.as_ref();
                $validate($field, value, $max_len)?;
                Ok(Self(Arc::from(value)))
            }

            /// Wrap a model-derived string, folding anything outside the
            /// field's grammar to [`policy::DIMENSION_UNKNOWN`] so a raw
            /// identifier-like value can never be stored.
            pub(crate) fn fold(value: &str) -> Self {
                Self::new(value)
                    .unwrap_or_else(|_| Self(Arc::from(policy::DIMENSION_UNKNOWN)))
            }

            /// The validated string value.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }
    };
}

dimension_newtype!(
    /// An emulated service namespace (e.g. `sqs`, `dynamodb`).
    Namespace,
    "namespace",
    policy::DIMENSION_MAX_LEN,
    validate_dimension
);

dimension_newtype!(
    /// An emulated operation name (e.g. `SendMessage`, `CreateDBInstance`,
    /// `ListObjectsV2`), shaped like an AWS Smithy operation name.
    OperationName,
    "operation name",
    policy::DIMENSION_MAX_LEN,
    validate_operation_name
);

/// Why a project id failed validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectIdError {
    /// The value does not start with the canonical `proj-` prefix.
    MissingPrefix,
    /// The hex tail is not exactly the canonical length.
    BadLength,
    /// The hex tail carries characters outside lowercase hex.
    BadCharset,
}

impl fmt::Display for ProjectIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self {
            ProjectIdError::MissingPrefix => "must start with the canonical prefix",
            ProjectIdError::BadLength => "must carry the exact canonical hex length",
            ProjectIdError::BadCharset => "must be lowercase hex after the prefix",
        };
        write!(f, "invalid telemetry project id: {reason}")
    }
}

impl std::error::Error for ProjectIdError {}

/// The canonical project identifier this emulator was generated for:
/// exactly [`policy::PROJECT_ID_PREFIX`] followed by
/// [`policy::PROJECT_ID_HEX_LEN`] lowercase hex characters. Constructed from
/// the service's canonical serialized project id — never from a project name
/// or environment guess.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProjectId(Arc<str>);

impl ProjectId {
    /// Validate and wrap a canonical serialized project id.
    pub fn new(value: impl AsRef<str>) -> Result<Self, ProjectIdError> {
        let value = value.as_ref();
        let hex = value
            .strip_prefix(policy::PROJECT_ID_PREFIX)
            .ok_or(ProjectIdError::MissingPrefix)?;
        if hex.len() != policy::PROJECT_ID_HEX_LEN {
            return Err(ProjectIdError::BadLength);
        }
        if !hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(ProjectIdError::BadCharset);
        }
        Ok(Self(Arc::from(value)))
    }

    /// The validated canonical project id.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl serde::Serialize for ProjectId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

/// A telemetry identifier that must be a UUID was malformed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidUuidId {
    field: &'static str,
}

impl InvalidUuidId {
    /// The rejected field's name.
    pub fn field(&self) -> &'static str {
        self.field
    }
}

impl fmt::Display for InvalidUuidId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid telemetry {}: must be a UUID", self.field)
    }
}

impl std::error::Error for InvalidUuidId {}

macro_rules! uuid_id_newtype {
    ($(#[$doc:meta])* $name:ident, $field:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(Arc<str>);

        impl $name {
            /// Parse a UUID string (any casing/format `uuid` accepts) into
            /// the canonical lowercase hyphenated form.
            pub fn new(value: impl AsRef<str>) -> Result<Self, InvalidUuidId> {
                let parsed = uuid::Uuid::parse_str(value.as_ref().trim())
                    .map_err(|_| InvalidUuidId { field: $field })?;
                Ok(Self(Arc::from(
                    parsed.as_hyphenated().to_string().as_str(),
                )))
            }

            /// A fresh random (v4) identifier.
            pub fn random() -> Self {
                Self(Arc::from(
                    uuid::Uuid::new_v4().as_hyphenated().to_string().as_str(),
                ))
            }

            /// The canonical hyphenated UUID string.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }
    };
}

uuid_id_newtype!(
    /// A stable, random, non-identifying installation identifier (UUID),
    /// persisted next to the telemetry outbox.
    InstallationId,
    "installation id"
);

uuid_id_newtype!(
    /// A per-process session identifier (UUID).
    SessionId,
    "session id"
);

uuid_id_newtype!(
    /// A per-batch identifier (UUID). The backend deduplicates on
    /// `installationId#batchId`, so a batch keeps its id across retries and is
    /// re-keyed only when it is split.
    BatchId,
    "batch id"
);

/// Why a product version failed validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductVersionError {
    /// The value was not a `major.minor[.patch][(-|+)suffix]` version.
    Malformed,
    /// The value exceeded [`policy::PRODUCT_VERSION_MAX_LEN`].
    TooLong,
    /// A component carried a leading zero, which the backend contract
    /// rejects.
    LeadingZero,
    /// The `major` or `minor` component exceeded
    /// [`policy::PRODUCT_VERSION_COMPONENT_MAX`].
    ComponentOutOfRange,
}

impl fmt::Display for ProductVersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self {
            ProductVersionError::Malformed => "must be a `major.minor[.patch][(-|+)suffix]` value",
            ProductVersionError::TooLong => "exceeds maximum length",
            ProductVersionError::LeadingZero => "has a component with a leading zero",
            ProductVersionError::ComponentOutOfRange => {
                "has a component above the backend's version-component cap"
            }
        };
        write!(f, "invalid telemetry product version: {reason}")
    }
}

impl std::error::Error for ProductVersionError {}

/// The product version dimension: the emulator's released version, in the
/// backend contract's `major.minor[.patch][(-|+)suffix]` shape. The backend
/// folds it to `major.minor` for its allowlisted dimension and keeps the full
/// value alongside.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProductVersion(Arc<str>);

impl ProductVersion {
    /// Validate and wrap a version string such as `1.2`, `1.2.3`, or
    /// `1.2.3-rc.1`: numeric components without leading zeros, `major` and
    /// `minor` at most [`policy::PRODUCT_VERSION_COMPONENT_MAX`], an optional
    /// `-`/`+` suffix of ASCII alphanumerics, `.`, and `-`, and at most
    /// [`policy::PRODUCT_VERSION_MAX_LEN`] bytes — the backend contract's
    /// shape.
    pub fn new(value: impl AsRef<str>) -> Result<Self, ProductVersionError> {
        let value = value.as_ref();
        if value.len() > policy::PRODUCT_VERSION_MAX_LEN {
            return Err(ProductVersionError::TooLong);
        }
        let (core, suffix) = match value.find(['-', '+']) {
            Some(at) => (&value[..at], Some(&value[at + 1..])),
            None => (value, None),
        };
        if suffix.is_some_and(|suffix| {
            suffix.is_empty()
                || !suffix
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
        }) {
            return Err(ProductVersionError::Malformed);
        }
        let mut parts = core.split('.');
        let (Some(major), Some(minor), patch, None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(ProductVersionError::Malformed);
        };
        for component in [major, minor].into_iter().chain(patch) {
            if component.is_empty() || !component.bytes().all(|b| b.is_ascii_digit()) {
                return Err(ProductVersionError::Malformed);
            }
            if component != "0" && component.starts_with('0') {
                return Err(ProductVersionError::LeadingZero);
            }
        }
        for component in [major, minor] {
            if component
                .parse::<u64>()
                .ok()
                .is_none_or(|n| n > policy::PRODUCT_VERSION_COMPONENT_MAX)
            {
                return Err(ProductVersionError::ComponentOutOfRange);
            }
        }
        Ok(Self(Arc::from(value)))
    }

    /// The validated version string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProductVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl serde::Serialize for ProductVersion {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

/// Why a region dimension failed validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionError {
    /// The value was empty.
    Empty,
    /// The value exceeded [`policy::REGION_MAX_LEN`].
    TooLong,
    /// The value did not follow the closed region grammar
    /// `^[a-z]{2}(-gov|-iso[a-z]?)?-[a-z]+-\d$`.
    Malformed,
    /// The value followed the grammar but is not in [`policy::KNOWN_REGIONS`].
    UnknownRegion,
}

impl fmt::Display for RegionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self {
            RegionError::Empty => "must not be empty",
            RegionError::TooLong => "exceeds maximum length",
            RegionError::Malformed => "must be a canonical region token",
            RegionError::UnknownRegion => "must be a known AWS region",
        };
        write!(f, "invalid telemetry region: {reason}")
    }
}

impl std::error::Error for RegionError {}

/// A canonical AWS region token (e.g. `us-west-2`), validated against the
/// closed region grammar AND the [`policy::KNOWN_REGIONS`] allowlist, or the
/// [`policy::REGION_UNKNOWN`] fold token. Observed-but-unrecognized regions
/// fold to `unknown` via [`Region::fold`]; free-form text never reaches the
/// wire.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Region(Arc<str>);

impl Region {
    /// Validate and wrap a region token.
    pub fn new(value: impl AsRef<str>) -> Result<Self, RegionError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(RegionError::Empty);
        }
        if value.len() > policy::REGION_MAX_LEN {
            return Err(RegionError::TooLong);
        }
        if !Self::matches_grammar(value) {
            return Err(RegionError::Malformed);
        }
        if policy::KNOWN_REGIONS.binary_search(&value).is_err() {
            return Err(RegionError::UnknownRegion);
        }
        Ok(Self(Arc::from(value)))
    }

    /// The closed-vocabulary fold: a valid known region passes through,
    /// anything else becomes the [`policy::REGION_UNKNOWN`] token.
    pub fn fold(value: impl AsRef<str>) -> Self {
        Self::new(value.as_ref()).unwrap_or_else(|_| Self::unknown())
    }

    /// The [`policy::REGION_UNKNOWN`] fold token.
    pub fn unknown() -> Self {
        Self(Arc::from(policy::REGION_UNKNOWN))
    }

    /// `^[a-z]{2}(-gov|-iso[a-z]?)?-[a-z]+-\d$`, parsed without a regex
    /// engine: try each optional-infix alternative and require the exact
    /// `-<letters>-<digit>` tail on any one of them.
    fn matches_grammar(value: &str) -> bool {
        let bytes = value.as_bytes();
        if bytes.len() < 2 || !bytes[..2].iter().all(u8::is_ascii_lowercase) {
            return false;
        }
        let after_partition = &value[2..];
        let mut candidates = vec![after_partition];
        if let Some(rest) = after_partition.strip_prefix("-gov") {
            candidates.push(rest);
        }
        if let Some(rest) = after_partition.strip_prefix("-iso") {
            candidates.push(rest);
            let mut chars = rest.chars();
            if chars.next().is_some_and(|c| c.is_ascii_lowercase()) {
                candidates.push(chars.as_str());
            }
        }
        candidates.into_iter().any(Self::tail_matches)
    }

    fn tail_matches(rest: &str) -> bool {
        let Some(rest) = rest.strip_prefix('-') else {
            return false;
        };
        let Some((letters, digit)) = rest.rsplit_once('-') else {
            return false;
        };
        !letters.is_empty()
            && letters.bytes().all(|b| b.is_ascii_lowercase())
            && digit.len() == 1
            && digit.bytes().all(|b| b.is_ascii_digit())
    }

    /// The validated region token.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl serde::Serialize for Region {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

/// Why a dotted version dimension failed validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VersionStringError {
    field: &'static str,
}

impl VersionStringError {
    /// The rejected field's name.
    pub fn field(&self) -> &'static str {
        self.field
    }
}

impl fmt::Display for VersionStringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid telemetry {}: must be a bounded dotted-digits version",
            self.field
        )
    }
}

impl std::error::Error for VersionStringError {}

fn validate_version_string(field: &'static str, value: &str) -> Result<(), VersionStringError> {
    if value.is_empty()
        || value.len() > policy::VERSION_DIMENSION_MAX_LEN
        || value.starts_with('.')
        || value.ends_with('.')
        || value.contains("..")
        || !value.bytes().all(|b| b.is_ascii_digit() || b == b'.')
    {
        return Err(VersionStringError { field });
    }
    Ok(())
}

macro_rules! version_string_newtype {
    ($(#[$doc:meta])* $name:ident, $field:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name(Arc<str>);

        impl $name {
            /// Validate and wrap a dotted-digits version fragment.
            pub fn new(value: impl AsRef<str>) -> Result<Self, VersionStringError> {
                let value = value.as_ref();
                validate_version_string($field, value)?;
                Ok(Self(Arc::from(value)))
            }

            /// The validated version fragment.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }
    };
}

version_string_newtype!(
    /// The calling SDK's bounded dotted version fragment (e.g. `1.82.0`).
    SdkVersion,
    "sdk version"
);

version_string_newtype!(
    /// The calling language runtime's bounded dotted version fragment.
    LanguageVersion,
    "language version"
);

/// The server-announced minimum supported client version. Lenient by design:
/// an unparseable minimum must never disable the client (fail-open), so this
/// only bounds length and charset; numeric comparison happens in
/// [`version_at_least`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinClientVersion(Arc<str>);

impl MinClientVersion {
    /// Wrap a trimmed, bounded, printable minimum-version string. Returns
    /// `None` (rather than an error) for values that cannot possibly gate —
    /// the caller treats those exactly like an absent minimum.
    pub fn new(value: impl AsRef<str>) -> Option<Self> {
        let value = value.as_ref().trim();
        if value.is_empty()
            || value.len() > policy::VERSION_DIMENSION_MAX_LEN
            || !value.bytes().all(|b| b.is_ascii_graphic())
        {
            return None;
        }
        Some(Self(Arc::from(value)))
    }

    /// The bounded minimum-version string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The config endpoint's `ETag`, kept for `If-None-Match` revalidation. The
/// only server-provided text the client persists, so it is bounded to
/// [`policy::CONFIG_ETAG_MAX_LEN`] visible ASCII bytes; anything else is
/// treated as no ETag (the next refresh simply fetches unconditionally).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigEtag(Arc<str>);

impl ConfigEtag {
    /// Wrap a bounded, visible-ASCII ETag value.
    pub fn new(value: impl AsRef<str>) -> Option<Self> {
        let value = value.as_ref().trim();
        if value.is_empty()
            || value.len() > policy::CONFIG_ETAG_MAX_LEN
            || !value.bytes().all(|b| b.is_ascii_graphic())
        {
            return None;
        }
        Some(Self(Arc::from(value)))
    }

    /// The validated ETag value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl serde::Serialize for ConfigEtag {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for ConfigEtag {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        ConfigEtag::new(&raw).ok_or_else(|| serde::de::Error::custom("invalid config etag"))
    }
}

macro_rules! token_enum {
    ($name:ident, $token_fn:path) => {
        impl $name {
            /// The exact wire token for this value.
            pub fn as_str(self) -> &'static str {
                $token_fn(self)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }
    };
}

/// The closed set of SDKs recognized from a `User-Agent` marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SdkName {
    AwsSdkRust,
    AwsSdkJava,
    Boto3,
    AwsSdkJsV3,
    AwsCli,
}

token_enum!(SdkName, policy::sdk_name_token);

/// The `environment.sdkName` dimension: unknown until a recognized SDK is
/// observed, a single recognized SDK, or `mixed` once more than one has been
/// observed in the session. Once mixed, always mixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SdkDimension {
    /// No recognized SDK observed yet.
    Unknown,
    /// More than one recognized SDK observed; sticky for the session.
    Mixed,
    /// Exactly one recognized SDK observed so far.
    Sdk(SdkName),
}

token_enum!(SdkDimension, policy::sdk_dimension_token);

/// The closed agent-client taxonomy accepted by the backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentClient {
    AmazonQ,
    ClaudeCode,
    Kiro,
    Cursor,
    Other,
}

token_enum!(AgentClient, policy::agent_client_token);

/// The closed operating-system taxonomy accepted by the backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OsFamily {
    Linux,
    Macos,
    Windows,
    Other,
}

token_enum!(OsFamily, policy::os_token);

/// The closed CPU-architecture taxonomy accepted by the backend
/// (`Amd64` serializes as the backend's `x86_64` token).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Arch {
    Amd64,
    Aarch64,
    Other,
}

token_enum!(Arch, policy::arch_token);

/// The closed container-runtime taxonomy accepted by the backend. Podman
/// detection folds to [`ContainerRuntime::Other`] before construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContainerRuntime {
    Docker,
    Kubernetes,
    Ecs,
    Other,
    None,
}

token_enum!(ContainerRuntime, policy::container_runtime_token);

/// The closed `environment.runMode` taxonomy accepted by the backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunMode {
    Standalone,
    Embedded,
}

token_enum!(RunMode, policy::run_mode_token);

/// The closed install-channel taxonomy accepted by the backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InstallChannel {
    Cargo,
    Npm,
    Pip,
    Docker,
    Binary,
    Source,
    Other,
    Unknown,
}

token_enum!(InstallChannel, policy::install_channel_token);

/// The closed memory-usage buckets; raw byte counts never reach the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemoryBucket {
    LessThan256Mib,
    From256To512Mib,
    From512MibTo1Gib,
    From1To2Gib,
    From2To4Gib,
    GreaterThan4Gib,
}

token_enum!(MemoryBucket, policy::memory_bucket_token);

/// The closed `performance.startupOutcome` taxonomy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StartupOutcome {
    Success,
    Failure,
}

token_enum!(StartupOutcome, policy::startup_outcome_token);

/// Closed, boolean-only emulator capabilities supplied by the scaffold.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetryFeatureFlags {
    pub persistence: bool,
    pub multi_region: bool,
    pub auth_emulation: bool,
    pub strict_mode: bool,
    pub request_logging: bool,
    pub fidelity_gating: bool,
}

/// The closed error taxonomy the backend accepts. Anything the folding rules
/// in [`ErrorType::from_code`] cannot classify becomes [`ErrorType::Other`] —
/// raw error codes and messages never reach the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ErrorType {
    ValidationError,
    ResourceNotFound,
    ResourceConflict,
    AccessDenied,
    AuthFailure,
    Throttling,
    LimitExceeded,
    UnsupportedOperation,
    SerializationError,
    Timeout,
    ServiceUnavailable,
    InternalError,
    Other,
}

impl ErrorType {
    /// Every taxonomy value ([`policy::ERROR_TAXONOMY`]), for tests and
    /// exhaustive folding checks.
    pub const ALL: [ErrorType; 13] = policy::ERROR_TAXONOMY;

    /// The exact wire token for this taxonomy value
    /// ([`policy::error_type_token`]).
    pub fn as_str(self) -> &'static str {
        policy::error_type_token(self)
    }

    /// Fold a raw error code (e.g. an AWS error code like
    /// `ThrottlingException`) into the closed taxonomy.
    ///
    /// Matching is case-insensitive and ignores `_`/`-`, mirroring the
    /// backend's own normalization: a canonical taxonomy token matches first,
    /// then common AWS error-code families fold by substring, and everything
    /// else becomes [`ErrorType::Other`]. The raw code itself is discarded.
    pub fn from_code(code: &str) -> ErrorType {
        let normalized: String = code
            .chars()
            .filter(|c| *c != '_' && *c != '-')
            .flat_map(char::to_lowercase)
            .collect();

        for taxonomy in ErrorType::ALL {
            if normalized == taxonomy.as_str().to_ascii_lowercase() {
                return taxonomy;
            }
        }

        for (needles, taxonomy) in policy::ERROR_CODE_FOLDS {
            if needles.iter().any(|needle| normalized.contains(needle)) {
                return *taxonomy;
            }
        }
        ErrorType::Other
    }
}

impl serde::Serialize for ErrorType {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

/// The closed crash taxonomy. Only the crash *type* is reported — never a
/// location, message, or backtrace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CrashType {
    Panic,
    Oom,
    Sigkill,
}

impl CrashType {
    /// The exact wire token for this crash type
    /// ([`policy::crash_type_token`]).
    pub fn as_str(self) -> &'static str {
        policy::crash_type_token(self)
    }
}

impl serde::Serialize for CrashType {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

/// The closed outcome vocabulary for metric rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Outcome {
    Success,
    ClientError,
    ServerError,
    NotEmulated,
    Error,
    Crashed,
}

impl Outcome {
    /// Classify an HTTP response status.
    pub fn from_http_status(status: u16) -> Outcome {
        match status {
            100..=399 => Outcome::Success,
            400..=499 => Outcome::ClientError,
            500..=599 => Outcome::ServerError,
            _ => Outcome::Error,
        }
    }

    /// The exact wire token for this outcome ([`policy::outcome_token`]).
    pub fn as_str(self) -> &'static str {
        policy::outcome_token(self)
    }
}

impl serde::Serialize for Outcome {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

/// The closed metric row type vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MetricType {
    Operation,
    CoverageGap,
    Error,
}

impl MetricType {
    /// The exact wire token for this metric row type
    /// ([`policy::metric_type_token`]).
    pub fn as_str(self) -> &'static str {
        policy::metric_type_token(self)
    }
}

impl serde::Serialize for MetricType {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

/// A piece of local telemetry state the client persists under the state
/// directory and deletes whenever telemetry becomes effectively off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum StateArtifact {
    /// The random installation id file.
    InstallationId,
    /// The SQLite outbox (including its journal side files).
    Outbox,
}

impl StateArtifact {
    fn describe(self) -> &'static str {
        match self {
            StateArtifact::InstallationId => "installation id",
            StateArtifact::Outbox => "outbox",
        }
    }
}

/// Deleting the local telemetry state failed. Carries only the artifact kind
/// and the I/O error kind — never a path — so it is safe to surface in the
/// startup notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatePurgeError {
    /// The state directory could not be listed.
    List(std::io::ErrorKind),
    /// Removing the artifact failed.
    Remove {
        /// Which artifact could not be removed.
        artifact: StateArtifact,
        /// The operating system's reason.
        kind: std::io::ErrorKind,
    },
    /// Removal reported success but the artifact is still present.
    Remains(StateArtifact),
}

impl fmt::Display for StatePurgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StatePurgeError::List(kind) => {
                write!(f, "could not list the telemetry state directory: {kind}")
            }
            StatePurgeError::Remove { artifact, kind } => {
                write!(f, "could not remove the {}: {kind}", artifact.describe())
            }
            StatePurgeError::Remains(artifact) => {
                write!(
                    f,
                    "the {} is still present after removal",
                    artifact.describe()
                )
            }
        }
    }
}

impl std::error::Error for StatePurgeError {}

/// Persisting the server's last-seen telemetry configuration failed. Carries
/// only the I/O error kind — never a path — so it is safe to surface through
/// the notice. While it stands, a server-side denial cannot be remembered
/// across an offline restart, so the failure is never silently ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigCacheError {
    /// The operating system's reason.
    pub kind: std::io::ErrorKind,
}

impl fmt::Display for ConfigCacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "could not write the server configuration cache: {}",
            self.kind
        )
    }
}

impl std::error::Error for ConfigCacheError {}

impl From<std::io::Error> for ConfigCacheError {
    fn from(error: std::io::Error) -> Self {
        Self { kind: error.kind() }
    }
}

/// Compare two lenient `major[.minor[.patch]]` version strings numerically.
/// Missing components count as zero; a component's numeric value is its
/// leading digits. Returns `None` when either string has no leading digits in
/// its first component (fail-open: an unparseable server minimum never
/// disables the client).
pub(crate) fn version_at_least(version: &str, minimum: &str) -> Option<bool> {
    fn parse(v: &str) -> Option<[u64; 3]> {
        let mut out = [0u64; 3];
        for (i, part) in v.trim().splitn(3, '.').enumerate() {
            let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
            if i == 0 && digits.is_empty() {
                return None;
            }
            out[i] = digits.parse().unwrap_or(0);
        }
        Some(out)
    }
    Some(parse(version)? >= parse(minimum)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimensions_reject_pii_shaped_values() {
        for bad in [
            "arn:aws:iam::123456789012:role/Admin",
            "/etc/passwd",
            "user@example.com",
            "two words",
            "",
            "a".repeat(65).as_str(),
        ] {
            assert!(
                Namespace::new(bad).is_err(),
                "namespace must reject {bad:?}"
            );
            assert!(
                OperationName::new(bad).is_err(),
                "operation must reject {bad:?}"
            );
        }
        assert_eq!(Namespace::new("dynamodb").unwrap().as_str(), "dynamodb");
        assert_eq!(
            Namespace::new("").unwrap_err().reason(),
            DimensionErrorReason::Empty
        );
        assert_eq!(
            Namespace::new("a".repeat(65)).unwrap_err().reason(),
            DimensionErrorReason::TooLong
        );
        assert_eq!(
            Namespace::new("two words").unwrap_err().reason(),
            DimensionErrorReason::Charset
        );
    }

    #[test]
    fn operation_names_accept_only_aws_operation_shaped_values() {
        let longest = format!("Describe{}", "X".repeat(64 - "Describe".len()));
        for good in [
            "PutObject",
            "GetQueueUrl",
            "CreateDBInstance",
            "GetSAMLProvider",
            "ListObjectsV2",
            longest.as_str(),
        ] {
            assert_eq!(OperationName::new(good).unwrap().as_str(), good);
            assert_eq!(OperationName::fold(good).as_str(), good);
        }
        let too_long = format!("Describe{}", "X".repeat(65 - "Describe".len()));
        for bad in [
            "12345678",
            "arn:aws:s3:::my-bucket",
            "AKIAIOSFODNN7EXAMPLE",
            "my-bucket-name",
            "john.doe.example.com",
            "secret.txt",
            "Put Object",
            "Send_Message-2.0",
            "putObject",
            "PUTOBJECT",
            "1PutObject",
            too_long.as_str(),
        ] {
            assert!(
                OperationName::new(bad).is_err(),
                "operation must reject {bad:?}"
            );
            assert_eq!(
                OperationName::fold(bad).as_str(),
                policy::DIMENSION_UNKNOWN,
                "operation must fold {bad:?}"
            );
        }
        assert_eq!(
            OperationName::new(too_long).unwrap_err().reason(),
            DimensionErrorReason::TooLong
        );
        assert_eq!(
            OperationName::new("Put Object").unwrap_err().reason(),
            DimensionErrorReason::Charset
        );
        assert_eq!(
            OperationName::new("AKIAIOSFODNN7EXAMPLE")
                .unwrap_err()
                .reason(),
            DimensionErrorReason::OperationShape
        );
        assert_eq!(
            OperationName::new("secret").unwrap_err().to_string(),
            "invalid telemetry operation name: must be a PascalCase AWS operation name: \
             ASCII letters and digits, starting uppercase, with at least one lowercase letter"
        );
    }

    #[test]
    fn project_ids_are_exactly_proj_dash_32_lowercase_hex() {
        let canonical = "proj-0123456789abcdef0123456789abcdef";
        assert_eq!(ProjectId::new(canonical).unwrap().as_str(), canonical);
        for (bad, expected) in [
            ("", ProjectIdError::MissingPrefix),
            (
                "project-0123456789abcdef0123456789abcdef",
                ProjectIdError::MissingPrefix,
            ),
            (
                "proj-0123456789abcdef0123456789abcde",
                ProjectIdError::BadLength,
            ),
            (
                "proj-0123456789abcdef0123456789abcdef0",
                ProjectIdError::BadLength,
            ),
            (
                "proj-0123456789ABCDEF0123456789abcdef",
                ProjectIdError::BadCharset,
            ),
            (
                "proj-0123456789abcdxf0123456789abcdef",
                ProjectIdError::BadCharset,
            ),
            ("proj-", ProjectIdError::BadLength),
        ] {
            assert_eq!(ProjectId::new(bad).unwrap_err(), expected, "for {bad:?}");
        }
    }

    #[test]
    fn uuid_ids_canonicalize_and_reject_non_uuids() {
        let canonical = "0f8fad5b-d9cb-469f-a165-70867728950e";
        assert_eq!(InstallationId::new(canonical).unwrap().as_str(), canonical);
        assert_eq!(
            SessionId::new("0F8FAD5B-D9CB-469F-A165-70867728950E")
                .unwrap()
                .as_str(),
            canonical,
            "uppercase input canonicalizes to lowercase hyphenated form"
        );
        for bad in ["", "b-1", "not/a/batch/id", "proj-1234", "id-0001"] {
            assert!(BatchId::new(bad).is_err(), "batch id must reject {bad:?}");
            assert!(
                InstallationId::new(bad).is_err(),
                "installation id must reject {bad:?}"
            );
        }
        let random = BatchId::random();
        assert_eq!(BatchId::new(random.as_str()).unwrap(), random);
        assert_ne!(BatchId::random(), BatchId::random());
    }

    #[test]
    fn product_version_follows_the_backend_contract_shape() {
        for good in [
            "1.0",
            "0.0",
            "12.34",
            "99.99",
            "0.1.0",
            "1.2.3",
            "99.99.123456",
            "1.2.3-rc.1",
            "1.0.0+build.5",
            "1.0-beta",
            "1.2.3-alpha-2",
        ] {
            assert_eq!(
                ProductVersion::new(good).map(|v| v.as_str().to_owned()),
                Ok(good.to_owned()),
                "product version must accept {good:?} verbatim"
            );
        }
        for bad in [
            "1",
            "1.x",
            "v1.0",
            "",
            "1.",
            ".1",
            "1.0.0.0",
            "1.2.",
            "1.2.x",
            "1.0-",
            "1.0+",
            "1.0.0-rc_1",
            "1.0.0-rc 1",
            "-1.0",
            "1..0",
        ] {
            assert_eq!(
                ProductVersion::new(bad).unwrap_err(),
                ProductVersionError::Malformed,
                "product version must reject {bad:?}"
            );
        }
        for bad in ["01.2", "1.00", "007.1", "1.2.03", "0.1.00-rc.1"] {
            assert_eq!(
                ProductVersion::new(bad).unwrap_err(),
                ProductVersionError::LeadingZero,
                "product version must reject the leading zero in {bad:?}"
            );
        }
        for bad in ["100.0", "1.100", "100.0.0", "1.100.0-rc.1"] {
            assert_eq!(
                ProductVersion::new(bad).unwrap_err(),
                ProductVersionError::ComponentOutOfRange,
                "product version must reject the oversized component in {bad:?}"
            );
        }
        let at_limit = format!("1.2.3-{}", "a".repeat(policy::PRODUCT_VERSION_MAX_LEN - 6));
        assert_eq!(at_limit.len(), policy::PRODUCT_VERSION_MAX_LEN);
        assert!(ProductVersion::new(&at_limit).is_ok());
        assert_eq!(
            ProductVersion::new(format!("{at_limit}a")).unwrap_err(),
            ProductVersionError::TooLong
        );
    }

    #[test]
    fn product_version_accepts_a_full_cargo_package_version() {
        let this_crate = env!("CARGO_PKG_VERSION");
        assert!(
            ProductVersion::new(this_crate).is_ok(),
            "this crate's own full Cargo version {this_crate:?} must be a valid product version"
        );
    }

    #[test]
    fn regions_follow_the_closed_backend_grammar() {
        for good in [
            "us-west-2",
            "us-east-1",
            "eu-central-1",
            "ap-southeast-3",
            "us-gov-west-1",
            "cn-north-1",
            "il-central-1",
        ] {
            assert_eq!(Region::new(good).unwrap().as_str(), good, "for {good:?}");
        }
        for bad in [
            "",
            "US-WEST-2",
            "us-west-",
            "us-west-22",
            "us-west",
            "us-west-2-lax-1",
            "us_west_2",
            "arn:aws:iam::1:role/x",
        ] {
            assert!(Region::new(bad).is_err(), "region must reject {bad:?}");
        }
        assert_eq!(
            Region::new("xx-west-1").unwrap_err(),
            RegionError::UnknownRegion,
            "grammar-shaped but unlisted regions are unknown, not accepted"
        );
    }

    #[test]
    fn regions_fold_to_the_unknown_token_never_to_raw_text() {
        assert_eq!(Region::fold("us-west-2").as_str(), "us-west-2");
        for folded in [
            "",
            "xx-west-1",
            "us-west-99",
            "US-WEST-2",
            "my region",
            "arn:aws:iam::1:role/x",
            policy::REGION_UNKNOWN,
        ] {
            assert_eq!(
                Region::fold(folded).as_str(),
                policy::REGION_UNKNOWN,
                "{folded:?} must fold"
            );
        }
        assert!(
            Region::new(policy::REGION_UNKNOWN).is_err(),
            "the fold token itself is not a parseable region"
        );
    }

    #[test]
    fn the_known_regions_list_is_sorted_and_grammar_consistent() {
        let mut sorted = policy::KNOWN_REGIONS.to_vec();
        sorted.sort_unstable();
        assert_eq!(
            sorted,
            policy::KNOWN_REGIONS,
            "KNOWN_REGIONS must stay sorted for binary search"
        );
        for region in policy::KNOWN_REGIONS {
            assert_eq!(
                Region::new(region).map(|r| r.as_str().to_string()),
                Ok((*region).to_string()),
                "every listed region must satisfy the closed grammar"
            );
        }
    }

    #[test]
    fn version_dimensions_are_bounded_dotted_digits() {
        assert_eq!(SdkVersion::new("1.82.0").unwrap().as_str(), "1.82.0");
        assert_eq!(LanguageVersion::new("22").unwrap().as_str(), "22");
        for bad in ["", ".", "1.", ".1", "1..2", "1.x", "v1", &"9".repeat(65)] {
            assert!(SdkVersion::new(bad).is_err(), "must reject {bad:?}");
            assert!(LanguageVersion::new(bad).is_err(), "must reject {bad:?}");
        }
    }

    #[test]
    fn min_client_version_is_lenient_but_bounded() {
        assert_eq!(MinClientVersion::new(" 1.0.0 ").unwrap().as_str(), "1.0.0");
        assert!(MinClientVersion::new("").is_none());
        assert!(MinClientVersion::new("   ").is_none());
        assert!(MinClientVersion::new("1.0\n0").is_none());
        assert!(MinClientVersion::new("9".repeat(65)).is_none());
    }

    #[test]
    fn error_folding_is_case_insensitive_and_ignores_separators() {
        assert_eq!(
            ErrorType::from_code("validation_error"),
            ErrorType::ValidationError
        );
        assert_eq!(
            ErrorType::from_code("RESOURCE-NOT-FOUND"),
            ErrorType::ResourceNotFound
        );
        assert_eq!(ErrorType::from_code("throttling"), ErrorType::Throttling);
    }

    #[test]
    fn error_folding_maps_common_aws_codes() {
        for (code, expected) in [
            ("ThrottlingException", ErrorType::Throttling),
            ("TooManyRequestsException", ErrorType::Throttling),
            ("ProvisionedThroughputExceededException", ErrorType::Other),
            ("LimitExceededException", ErrorType::LimitExceeded),
            ("ServiceQuotaExceededException", ErrorType::LimitExceeded),
            ("AccessDeniedException", ErrorType::AccessDenied),
            ("UnauthorizedOperation", ErrorType::AccessDenied),
            ("ExpiredTokenException", ErrorType::AuthFailure),
            ("InvalidSignatureException", ErrorType::AuthFailure),
            ("ResourceNotFoundException", ErrorType::ResourceNotFound),
            ("NoSuchBucket", ErrorType::ResourceNotFound),
            ("ConflictException", ErrorType::ResourceConflict),
            (
                "ResourceAlreadyExistsException",
                ErrorType::ResourceConflict,
            ),
            (
                "UnsupportedOperationException",
                ErrorType::UnsupportedOperation,
            ),
            ("NotImplemented", ErrorType::UnsupportedOperation),
            ("SerializationException", ErrorType::SerializationError),
            ("MalformedQueryString", ErrorType::SerializationError),
            ("RequestTimeout", ErrorType::Timeout),
            ("RequestTimedOut", ErrorType::Timeout),
            ("ServiceUnavailableException", ErrorType::ServiceUnavailable),
            ("InternalServerError", ErrorType::InternalError),
            ("InternalFailure", ErrorType::InternalError),
            ("ValidationException", ErrorType::ValidationError),
            ("InvalidParameterValue", ErrorType::ValidationError),
            ("SomethingNeverSeenBefore", ErrorType::Other),
            ("", ErrorType::Other),
        ] {
            assert_eq!(ErrorType::from_code(code), expected, "folding {code:?}");
        }
    }

    #[test]
    fn wire_tokens_match_serde_serialization() {
        for taxonomy in ErrorType::ALL {
            assert_eq!(
                serde_json::to_value(taxonomy).unwrap(),
                serde_json::Value::String(taxonomy.as_str().to_string())
            );
        }
        for (crash, token) in [
            (CrashType::Panic, "panic"),
            (CrashType::Oom, "oom"),
            (CrashType::Sigkill, "sigkill"),
        ] {
            assert_eq!(crash.as_str(), token);
            assert_eq!(
                serde_json::to_value(crash).unwrap(),
                serde_json::Value::String(token.to_string())
            );
        }
        for (outcome, token) in [
            (Outcome::Success, "success"),
            (Outcome::ClientError, "client_error"),
            (Outcome::ServerError, "server_error"),
            (Outcome::NotEmulated, "not_emulated"),
            (Outcome::Error, "error"),
            (Outcome::Crashed, "crashed"),
        ] {
            assert_eq!(outcome.as_str(), token);
            assert_eq!(
                serde_json::to_value(outcome).unwrap(),
                serde_json::Value::String(token.to_string())
            );
        }
    }

    #[test]
    fn closed_environment_enums_serialize_to_their_policy_tokens() {
        for (value, token) in [
            (SdkDimension::Unknown, "unknown"),
            (SdkDimension::Mixed, "mixed"),
            (SdkDimension::Sdk(SdkName::AwsSdkRust), "aws-sdk-rust"),
            (SdkDimension::Sdk(SdkName::AwsSdkJava), "aws-sdk-java"),
            (SdkDimension::Sdk(SdkName::Boto3), "boto3"),
            (SdkDimension::Sdk(SdkName::AwsSdkJsV3), "aws-sdk-js-v3"),
            (SdkDimension::Sdk(SdkName::AwsCli), "aws-cli"),
        ] {
            assert_eq!(value.as_str(), token);
            assert_eq!(
                serde_json::to_value(value).unwrap(),
                serde_json::Value::String(token.to_string())
            );
        }
        for (value, token) in [
            (AgentClient::AmazonQ, "amazon-q"),
            (AgentClient::ClaudeCode, "claude-code"),
            (AgentClient::Kiro, "kiro"),
            (AgentClient::Cursor, "cursor"),
            (AgentClient::Other, "other"),
        ] {
            assert_eq!(value.as_str(), token);
        }
        for (value, token) in [
            (OsFamily::Linux, "linux"),
            (OsFamily::Macos, "macos"),
            (OsFamily::Windows, "windows"),
            (OsFamily::Other, "other"),
        ] {
            assert_eq!(value.as_str(), token);
        }
        for (value, token) in [
            (Arch::Amd64, "x86_64"),
            (Arch::Aarch64, "aarch64"),
            (Arch::Other, "other"),
        ] {
            assert_eq!(value.as_str(), token);
        }
        for (value, token) in [
            (ContainerRuntime::Docker, "docker"),
            (ContainerRuntime::Kubernetes, "kubernetes"),
            (ContainerRuntime::Ecs, "ecs"),
            (ContainerRuntime::Other, "other"),
            (ContainerRuntime::None, "none"),
        ] {
            assert_eq!(value.as_str(), token);
        }
        for (value, token) in [
            (RunMode::Standalone, "standalone"),
            (RunMode::Embedded, "embedded"),
        ] {
            assert_eq!(value.as_str(), token);
        }
        for (value, token) in [
            (InstallChannel::Cargo, "cargo"),
            (InstallChannel::Npm, "npm"),
            (InstallChannel::Pip, "pip"),
            (InstallChannel::Docker, "docker"),
            (InstallChannel::Binary, "binary"),
            (InstallChannel::Source, "source"),
            (InstallChannel::Other, "other"),
            (InstallChannel::Unknown, "unknown"),
        ] {
            assert_eq!(value.as_str(), token);
        }
        for (value, token) in [
            (MemoryBucket::LessThan256Mib, "less_than_256_mib"),
            (MemoryBucket::From256To512Mib, "256_to_512_mib"),
            (MemoryBucket::From512MibTo1Gib, "512_mib_to_1_gib"),
            (MemoryBucket::From1To2Gib, "1_to_2_gib"),
            (MemoryBucket::From2To4Gib, "2_to_4_gib"),
            (MemoryBucket::GreaterThan4Gib, "greater_than_4_gib"),
        ] {
            assert_eq!(value.as_str(), token);
        }
        assert_eq!(StartupOutcome::Success.as_str(), "success");
        assert_eq!(StartupOutcome::Failure.as_str(), "failure");
    }

    #[test]
    fn outcome_classifies_http_status() {
        assert_eq!(Outcome::from_http_status(200), Outcome::Success);
        assert_eq!(Outcome::from_http_status(301), Outcome::Success);
        assert_eq!(Outcome::from_http_status(404), Outcome::ClientError);
        assert_eq!(Outcome::from_http_status(500), Outcome::ServerError);
        assert_eq!(Outcome::from_http_status(0), Outcome::Error);
    }

    #[test]
    fn version_comparison_is_numeric_and_lenient() {
        assert_eq!(version_at_least("1.0.0", "1.0.0"), Some(true));
        assert_eq!(version_at_least("1.0.0", "1.0.1"), Some(false));
        assert_eq!(version_at_least("1.10.0", "1.9.0"), Some(true));
        assert_eq!(version_at_least("2.0", "1.9.9"), Some(true));
        assert_eq!(version_at_least("1.0.0", "2.0.0"), Some(false));
        assert_eq!(version_at_least("1.0.0", "not-a-version"), None);
    }
}
