// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ssm-amazonssm service.
//!
//! Besides the state struct itself this module holds the Parameter Store rules the
//! parameter handlers share — the naming constraints, the tier and content-size
//! rules, ARN construction, version retention and the record-to-modelled-shape
//! conversion — so every operation that touches a parameter agrees on one
//! implementation of each instead of carrying copies that can drift apart.
//!
//! Every constant below cites the model sentence it comes from; ask the model for
//! the full text with
//! `smithy_query(query="doc",shape="PutParameterRequest",member="<Member>",model_path="services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json")`.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::Parameter;

/// The region an emulated request is treated as having been made in when its
/// SigV4 credential scope does not name one (an unsigned or anonymous request).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `arnNamespace` / `endpointPrefix` this service's ARNs carry, from the
/// model's `aws.api#service` trait (`smithy_query(query="service_info")`:
/// `arnNamespace: ssm`).
pub const ARN_NAMESPACE: &str = "ssm";

/// `ParameterTier` values, from the model's enum (`Standard`, `Advanced`,
/// `Intelligent-Tiering`).
pub const TIER_STANDARD: &str = "Standard";
/// See [`TIER_STANDARD`].
pub const TIER_ADVANCED: &str = "Advanced";
/// See [`TIER_STANDARD`].
pub const TIER_INTELLIGENT: &str = "Intelligent-Tiering";

/// The `ParameterType` value whose values Parameter Store encrypts with a KMS key.
pub const TYPE_SECURE_STRING: &str = "SecureString";

/// A standard parameter's content size limit (`PutParameterRequest$Tier`:
/// "Standard parameters have a content size limit of 4 KB").
pub const STANDARD_VALUE_LIMIT: usize = 4 * 1024;

/// An advanced parameter's content size limit (`PutParameterRequest$Tier`:
/// "Advanced parameters have a content size limit of 8 KB").
pub const ADVANCED_VALUE_LIMIT: usize = 8 * 1024;

/// The number of parameters past which `Intelligent-Tiering` creates advanced
/// parameters (`PutParameterRequest$Tier`, the options that require an advanced
/// parameter: "More than 10,000 parameters already exist in your Amazon Web
/// Services account in the current Amazon Web Services Region").
pub const STANDARD_PARAMETER_COUNT_LIMIT: usize = 10_000;

/// The deepest hierarchy a parameter name may describe
/// (`PutParameterRequest$Name`: "Parameter hierarchies are limited to a maximum
/// depth of fifteen levels", and `HierarchyLevelLimitExceededException`: "A
/// hierarchy can have a maximum of 15 levels").
pub const MAX_HIERARCHY_LEVELS: usize = 15;

/// How many versions of one parameter Parameter Store keeps
/// (`ParameterMaxVersionLimitExceeded`: "Parameter Store retains the 100 most
/// recently created versions of a parameter … Parameter Store deletes the oldest
/// version when a new one is created").
pub const MAX_VERSIONS_RETAINED: usize = 100;

/// The `DataType` values Parameter Store supports
/// (`PutParameterRequest$DataType`: "The following data type values are
/// supported. text aws:ec2:image aws:ssm:integration").
pub const DATA_TYPES: &[&str] = &["text", "aws:ec2:image", "aws:ssm:integration"];

/// The data type of a parameter whose request names none (`Parameter$DataType`:
/// "The data type of the parameter, such as text or aws:ec2:image. The default is
/// text").
pub const DEFAULT_DATA_TYPE: &str = "text";

/// The key a `SecureString` parameter is encrypted with when the request names
/// none (`PutParameterRequest$KeyId`: "If you don't specify a key ID, the system
/// uses the default key associated with your Amazon Web Services account").
pub const DEFAULT_SECURE_STRING_KEY_ID: &str = "alias/aws/ssm";

/// The most parameters this store holds: the larger of the two account quotas the
/// model documents (`PutParameterRequest$Tier`: "You can create a maximum of
/// 100,000 advanced parameters for each Region in an Amazon Web Services
/// account"). It is also what keeps the map bounded, so a caller cannot grow the
/// process's memory without limit: a create past it is refused with the modelled
/// `ParameterLimitExceeded`.
pub const MAX_PARAMETERS: usize = 100_000;

/// The longest `AllowedPattern` this service compiles. The model constrains
/// `AllowedPattern` to `len 0..1024`, so the boundary validator already refuses a
/// longer one; the bound is restated here so the one the compile relies on is its
/// own rather than a constraint elsewhere.
pub const MAX_ALLOWED_PATTERN_LEN: usize = 1024;

/// The compiled-program budget an `AllowedPattern` has to fit, so the work one
/// request's pattern can cost is bounded by this code rather than by the regex
/// crate's much larger default.
pub const ALLOWED_PATTERN_SIZE_LIMIT: usize = 64 * 1024;

/// The lazy-DFA cache budget an `AllowedPattern` match runs under, bounding the
/// memory one match can occupy. See [`ALLOWED_PATTERN_SIZE_LIMIT`].
pub const ALLOWED_PATTERN_DFA_SIZE_LIMIT: usize = 256 * 1024;

/// Compile an `AllowedPattern` under the explicit length and program-size budgets
/// above, or say why it was refused.
///
/// The refusal text is the message of the `InvalidAllowedPatternException` the
/// caller gets: it names the member and the bound it broke, so the request can be
/// fixed from that text alone.
pub fn compile_allowed_pattern(pattern: &str) -> Result<::regex::Regex, String> {
    if pattern.len() > MAX_ALLOWED_PATTERN_LEN {
        return Err(format!(
            "AllowedPattern is {} characters long, which exceeds the maximum of \
             {MAX_ALLOWED_PATTERN_LEN}.",
            pattern.len()
        ));
    }
    ::regex::RegexBuilder::new(pattern)
        .size_limit(ALLOWED_PATTERN_SIZE_LIMIT)
        .dfa_size_limit(ALLOWED_PATTERN_DFA_SIZE_LIMIT)
        .build()
        .map_err(|error| {
            format!("AllowedPattern {pattern:?} isn't a usable regular expression: {error}")
        })
}

/// One version of a parameter: the definition a single `PutParameter` call wrote.
///
/// A parameter's value, type and metadata belong to the version rather than to the
/// parameter, because `GetParameter` can ask for an earlier one by selector and
/// must answer with what that call stored.
#[derive(Debug, Clone)]
pub struct ParameterVersion {
    /// The version number this call was assigned: 1 for a created parameter, one
    /// more than the previous version for every overwrite.
    pub version: i64,
    pub value: String,
    /// The `ParameterType` of this version (`String`, `StringList` or
    /// `SecureString`).
    pub r#type: String,
    pub description: Option<String>,
    /// The KMS key id a `SecureString` version is encrypted with, defaulted to
    /// [`DEFAULT_SECURE_STRING_KEY_ID`]; whatever the request named otherwise.
    pub key_id: Option<String>,
    pub allowed_pattern: Option<String>,
    pub policies: Option<String>,
    pub data_type: String,
    /// The tier Parameter Store assigned this version, which is what
    /// `PutParameter` reports back.
    pub tier: String,
    pub last_modified_date: SystemTime,
}

/// One stored parameter: its identity (name, ARN and tags) plus its versions.
#[derive(Debug, Clone)]
pub struct ParameterRecord {
    /// The name the parameter was created under — the key it is stored by.
    pub name: String,
    /// The ARN the service minted for it, so every reader reports the identical
    /// value instead of recomputing it.
    pub arn: String,
    pub tags: BTreeMap<String, String>,
    /// The version a read with no selector answers with: the most recent one.
    pub current: ParameterVersion,
    /// The retained earlier versions, oldest first and at most
    /// [`MAX_VERSIONS_RETAINED`] - 1 of them.
    pub superseded: Vec<ParameterVersion>,
}

impl ParameterRecord {
    /// The named version, whether it is the current one or a retained earlier one.
    pub fn version(&self, version: i64) -> Option<&ParameterVersion> {
        if self.current.version == version {
            return Some(&self.current);
        }
        self.superseded.iter().find(|v| v.version == version)
    }

    /// Make `next` the current version, retaining the one it replaces and dropping
    /// the oldest once [`MAX_VERSIONS_RETAINED`] versions are held.
    pub fn push_version(&mut self, next: ParameterVersion) {
        let replaced = std::mem::replace(&mut self.current, next);
        self.superseded.push(replaced);
        while self.superseded.len() + 1 > MAX_VERSIONS_RETAINED {
            self.superseded.remove(0);
        }
    }

    /// This record's `version` as the modelled `Parameter` shape. `selector` is the
    /// text the caller addressed the version with, which `Parameter$Selector`
    /// reports back ("Either the version number or the label used to retrieve the
    /// parameter value"), and is absent when the caller named no selector.
    pub fn to_modeled(&self, version: &ParameterVersion, selector: Option<String>) -> Parameter {
        Parameter {
            name: Some(self.name.clone()),
            r#type: Some(version.r#type.clone()),
            value: Some(version.value.clone()),
            version: version.version,
            selector,
            // Only a parameter that references another service's data carries a
            // source result; a Parameter Store parameter holds its own value.
            source_result: None,
            last_modified_date: Some(::aws_smithy_types::DateTime::from(
                version.last_modified_date,
            )),
            arn: Some(self.arn.clone()),
            data_type: Some(version.data_type.clone()),
        }
    }
}

/// Per-service state for com-amazonaws-ssm-amazonssm. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSsmAmazonssmState {
    /// Every stored parameter, keyed by the name it was created under — a
    /// parameter's identity in this API. A `BTreeMap` so hierarchy walks and list
    /// order are deterministic.
    pub parameters: BTreeMap<String, ParameterRecord>,
}

impl ComAmazonawsSsmAmazonssmState {
    /// The parameter a `Name` addresses, which may be the parameter's name or its
    /// full ARN (`GetParameterRequest$Name`: "The name or Amazon Resource Name
    /// (ARN) of the parameter that you want to query").
    pub fn parameter(&self, name: &str) -> Option<&ParameterRecord> {
        for candidate in name_candidates(name) {
            if let Some(record) = self.parameters.get(&candidate) {
                return Some(record);
            }
        }
        None
    }
}

/// The stored names a `Name` can address: itself, plus — when it is a parameter
/// ARN — the name the ARN's resource part spells, with and without the leading
/// `/` that a hierarchical name carries.
pub fn name_candidates(name: &str) -> Vec<String> {
    if !name.starts_with("arn:") {
        return vec![name.to_string()];
    }
    match name.split_once(":parameter/") {
        Some((_, resource)) => vec![resource.to_string(), format!("/{resource}")],
        None => vec![name.to_string()],
    }
}

/// Split a read's `Name` into the parameter name and the version-or-label
/// selector after it (`GetParameterRequest$Name`: "To query by parameter label,
/// use "Name": "name:label". To query by parameter version, use "Name":
/// "name:version"").
///
/// A parameter name cannot contain `:` ([`check_parameter_name`]), so the first
/// one separates the selector — except in a full ARN, whose own fields are
/// colon-separated and which therefore carries no selector.
pub fn split_selector(name: &str) -> (&str, Option<&str>) {
    if name.starts_with("arn:") {
        return (name, None);
    }
    match name.split_once(':') {
        Some((base, selector)) => (base, Some(selector)),
        None => (name, None),
    }
}

/// Why a parameter name cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameProblem {
    /// A space between characters (`PutParameterRequest$Name`: "If the specified
    /// name for a parameter contains spaces between characters, the request fails
    /// with a `ValidationException` error").
    ContainsSpace,
    /// Prefixed with `aws` or `ssm` (`PutParameterRequest$Name`: "A parameter name
    /// can't be prefixed with "aws" or "ssm" (case-insensitive)").
    ///
    /// Read literally, as the model words it: the letters at the start of the name,
    /// with no segment or word boundary after them, so `awsome-thing` is refused
    /// along with `/aws/thing`. The model states no boundary, and the narrower
    /// reading would accept names the reserved prefix is meant to keep free.
    ReservedPrefix,
    /// A character outside `a-zA-Z0-9_.-` and the hierarchy separator `/`
    /// (`PutParameterRequest$Name`: "Parameter names can include only the
    /// following symbols and letters: a-zA-Z0-9_.-").
    InvalidCharacter(char),
    /// Deeper than [`MAX_HIERARCHY_LEVELS`], carrying the depth asked for.
    TooManyLevels(usize),
}

/// The name Parameter Store stores a requested one under: the same name without
/// the spaces around it (`PutParameterRequest$Name`: "The service removes any
/// spaces specified for the beginning or end of a parameter name").
pub fn canonical_name(name: &str) -> &str {
    name.trim()
}

/// How many hierarchy levels a name describes: its `/`-separated segments, so a
/// name with no separator at all is one level.
pub fn hierarchy_levels(name: &str) -> usize {
    name.split('/').filter(|segment| !segment.is_empty()).count().max(1)
}

/// Check a [`canonical_name`] against Parameter Store's naming constraints.
pub fn check_parameter_name(name: &str) -> Result<(), NameProblem> {
    if name.chars().any(char::is_whitespace) {
        return Err(NameProblem::ContainsSpace);
    }
    if let Some(bad) = name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '/')))
    {
        return Err(NameProblem::InvalidCharacter(bad));
    }
    let bare = name.trim_start_matches('/').to_ascii_lowercase();
    if bare.starts_with("aws") || bare.starts_with("ssm") {
        return Err(NameProblem::ReservedPrefix);
    }
    let levels = hierarchy_levels(name);
    if levels > MAX_HIERARCHY_LEVELS {
        return Err(NameProblem::TooManyLevels(levels));
    }
    Ok(())
}

/// Whether a request carries an option that only an advanced parameter supports
/// (`PutParameterRequest$Tier`, "Options that require an advanced parameter":
/// content larger than 4 KB, a parameter policy, or more than 10,000 parameters
/// already in this account and Region).
pub fn requires_advanced_tier(value_len: usize, has_policies: bool, existing: usize) -> bool {
    value_len > STANDARD_VALUE_LIMIT || has_policies || existing > STANDARD_PARAMETER_COUNT_LIMIT
}

/// The tier Parameter Store assigns a request.
///
/// A request that names a tier gets that tier ("Whenever you specify a tier in the
/// request, Parameter Store creates or updates the parameter according to that
/// request"), except `Intelligent-Tiering`, which "evaluates each request to
/// determine if the parameter is standard or advanced". A request that names none
/// gets the account's default tier, which is the standard tier until
/// `UpdateServiceSetting` changes it ("The default tier when you begin using
/// Parameter Store is the standard-parameter tier").
pub fn resolve_tier(
    requested: Option<&str>,
    value_len: usize,
    has_policies: bool,
    existing: usize,
) -> String {
    match requested {
        Some(TIER_ADVANCED) => TIER_ADVANCED.to_string(),
        Some(TIER_INTELLIGENT) => {
            if requires_advanced_tier(value_len, has_policies, existing) {
                TIER_ADVANCED.to_string()
            } else {
                TIER_STANDARD.to_string()
            }
        }
        _ => TIER_STANDARD.to_string(),
    }
}

/// The content size limit of `tier`.
pub fn value_limit(tier: &str) -> usize {
    if tier == TIER_ADVANCED {
        ADVANCED_VALUE_LIMIT
    } else {
        STANDARD_VALUE_LIMIT
    }
}

/// The region of the request being served: the third field of its SigV4
/// credential scope, falling back to [`DEFAULT_REGION`] for a request that is
/// not signed.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of a parameter in `region`:
/// `arn:aws:ssm:<region>:<account>:parameter/<name>`, where the name's leading
/// hierarchy slash is the one the ARN's `parameter/` already carries.
pub fn parameter_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("parameter/{}", name.trim_start_matches('/')),
    )
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSsmAmazonssmState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSsmAmazonssmState>)
}
