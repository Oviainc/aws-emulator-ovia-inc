// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ssm-amazonssm service.
//!
//! The slice modelled here is **Parameter Store**: `PutParameter` creates or updates a
//! parameter and `GetParameter` reads one back. Both reach the one store below, so
//! whatever a put persisted is what a later get reports.
//!
//! A parameter lives at a name that is unique *within a Region* (the `PutParameter`
//! `Name` doc: "A parameter name must be unique within an Amazon Web Services Region"),
//! so the store is keyed by `(region, canonical name)`. Each parameter holds its
//! **versions**, oldest first: a put that overwrites an existing name appends a new
//! version rather than replacing the old one, which is what lets a `name:version`
//! selector resolve an earlier value.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use crate::ops::types::{Parameter, Tag};

/// The signing name, endpoint prefix and ARN namespace of this service, all three
/// `ssm` (the model's `aws.auth#sigv4` / `aws.api#service` traits).
pub const ARN_SERVICE: &str = "ssm";

/// Region used when a request carries no SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The resource portion a parameter ARN carries before the parameter's path, e.g.
/// `arn:aws:ssm:us-east-2:111122223333:parameter/Dev/DBServer` (the `PutParameter`
/// `Name` doc quotes the same prefix).
pub const ARN_RESOURCE_PREFIX: &str = "parameter/";

/// What every ARN starts with, and therefore how `GetParameter` tells a full ARN from
/// a bare parameter name.
pub const ARN_PREFIX: &str = "arn:";

/// `ParameterType` values, from the modelled enum
/// (`crate::ops::types::PARAMETER_TYPE_VALUES`).
pub const TYPE_SECURE_STRING: &str = "SecureString";

/// `ParameterTier` values, from the modelled enum
/// (`crate::ops::types::PARAMETER_TIER_VALUES`).
pub const TIER_STANDARD: &str = "Standard";

/// See [`TIER_STANDARD`].
pub const TIER_ADVANCED: &str = "Advanced";

/// See [`TIER_STANDARD`].
pub const TIER_INTELLIGENT_TIERING: &str = "Intelligent-Tiering";

/// Content size a standard parameter holds: "Standard parameters have a content size
/// limit of 4 KB" (`PutParameter`'s `Tier` doc).
pub const STANDARD_VALUE_LIMIT: usize = 4 * 1024;

/// Content size an advanced parameter holds: "Advanced parameters have a content size
/// limit of 8 KB" (same doc).
pub const ADVANCED_VALUE_LIMIT: usize = 8 * 1024;

/// How deep a parameter hierarchy may go: "Parameter hierarchies are limited to a
/// maximum depth of fifteen levels" (`PutParameter`'s `Name` doc), which is what
/// `HierarchyLevelLimitExceededException` reports.
pub const MAX_HIERARCHY_LEVELS: usize = 15;

/// How many versions of one parameter are retained: "Parameter Store retains the 100
/// most recently created versions of a parameter. After this number of versions has
/// been created, Parameter Store deletes the oldest version when a new one is
/// created" (the `ParameterMaxVersionLimitExceeded` doc).
pub const MAX_VERSIONS_RETAINED: usize = 100;

/// How many parameters one Region holds: "You can create a maximum of 10,000 standard
/// parameters for each Region in an Amazon Web Services account" (`PutParameter`'s
/// `Tier` doc). The advanced tier's published quota is ten times this; this emulator
/// applies the standard quota to both and reports a create past it as
/// `ParameterLimitExceeded`.
pub const MAX_PARAMETERS_PER_REGION: usize = 10_000;

/// How many parameters this store holds in TOTAL, across every Region.
///
/// Emulator resource guard, NOT a published AWS quota, and the bound that actually
/// keeps this process finite: the Region a parameter is stored under comes from the
/// request's own credential scope ([`request_region`]), so the per-Region quota alone
/// would be one quota per caller-chosen partition of the key space and the store could
/// grow without bound. A create past this total is refused with the same modelled
/// `ParameterLimitExceeded` as one past the per-Region quota.
pub const MAX_PARAMETERS_TOTAL: usize = MAX_PARAMETERS_PER_REGION;

/// The policy types a parameter may carry: "Parameter Store supports the following
/// policy types: Expiration, ExpirationNotification, and NoChangeNotification" (the
/// `InvalidPolicyTypeException` doc).
pub const POLICY_TYPES: &[&str] = &[
    "Expiration",
    "ExpirationNotification",
    "NoChangeNotification",
];

/// How many policies one parameter may carry: "You specified more than the maximum
/// number of allowed policies for the parameter. The maximum is 10" (the
/// `PoliciesLimitExceededException` doc).
pub const MAX_POLICIES: usize = 10;

/// The `DataType` values Parameter Store supports: "The following data type values are
/// supported. text aws:ec2:image aws:ssm:integration" (`PutParameter`'s `DataType`
/// doc).
pub const PARAMETER_DATA_TYPES: &[&str] = &["text", "aws:ec2:image", "aws:ssm:integration"];

/// The `DataType` a parameter carries when the request names none: "The data type of
/// the parameter, such as text or aws:ec2:image. The default is text" (the
/// `Parameter$DataType` doc).
pub const DEFAULT_DATA_TYPE: &str = "text";

/// The key a `SecureString` is encrypted under when the request names no `KeyId`: "If
/// you don't specify a key ID, the system uses the default key associated with your
/// Amazon Web Services account" (`PutParameter`'s `KeyId` doc), which Parameter Store
/// reaches through this alias.
pub const DEFAULT_SECURE_STRING_KEY_ID: &str = "alias/aws/ssm";

/// The characters a parameter name may carry: "Parameter names can include only the
/// following symbols and letters: a-zA-Z0-9_.-" plus the `/` hierarchy separator
/// (`PutParameter`'s `Name` doc).
fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '/')
}

/// The prefixes a parameter name may not start with: "A parameter name can't be
/// prefixed with "aws" or "ssm" (case-insensitive)" (`PutParameter`'s `Name` doc).
pub const RESERVED_NAME_PREFIXES: &[&str] = &["aws", "ssm"];

/// The store key for a parameter name: the name without its leading `/`.
///
/// A hierarchical name must carry the leading separator and a top-level one may
/// ("For parameters in a hierarchy, you must include a leading forward slash
/// character (/) when you create or reference a parameter"), so dropping it is what
/// makes `MyParameter` and `/MyParameter` the one parameter they are, while
/// `/Dev/DBServer` keeps its whole path.
pub fn canonical_name(name: &str) -> &str {
    name.strip_prefix('/').unwrap_or(name)
}

/// The name a request carries, with the spaces rule applied, or the reason it is
/// refused.
///
/// "Parameter names can't contain spaces. The service removes any spaces specified for
/// the beginning or end of a parameter name. If the specified name for a parameter
/// contains spaces between characters, the request fails with a `ValidationException`
/// error" (`PutParameter`'s `Name` doc; `GetParameter`'s own doc repeats it). So the
/// name is trimmed, and a trimmed name that still carries whitespace is refused with a
/// message naming the member and the constraint.
pub fn trimmed_name(name: &str) -> Result<&str, String> {
    let trimmed = name.trim();
    if trimmed.chars().any(char::is_whitespace) {
        return Err(format!(
            "Name '{name}' contains spaces between characters: a parameter name can't contain spaces"
        ));
    }
    Ok(trimmed)
}

/// Why a name is not one `PutParameter` accepts. Each variant maps to the modelled
/// error that reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameRejection {
    /// The name is not shaped like a parameter name:
    /// `ParameterPatternMismatchException`.
    Pattern(String),
    /// The name is nested deeper than Parameter Store allows:
    /// `HierarchyLevelLimitExceededException`.
    Depth(String),
}

/// Why `name` is not a parameter name this service accepts, or `None` when it is.
///
/// Every rule here is one of the naming constraints `PutParameter`'s `Name` doc lists;
/// the modelled `PSParameterName` carries only a length, so none of them is refused by
/// the boundary walk.
pub fn name_rejection(name: &str) -> Option<NameRejection> {
    if name.starts_with(ARN_PREFIX) {
        return Some(NameRejection::Pattern(format!(
            "Name '{name}' is an ARN: you can't enter the Amazon Resource Name (ARN) for a \
             parameter, only the parameter name itself"
        )));
    }
    let path = canonical_name(name);
    if path.is_empty() {
        return Some(NameRejection::Pattern(
            "Name carries no parameter name at all".to_string(),
        ));
    }
    if let Some(bad) = path.chars().find(|c| !is_name_char(*c)) {
        return Some(NameRejection::Pattern(format!(
            "Name '{name}' carries '{bad}': parameter names can include only the symbols and \
             letters a-zA-Z0-9_.- and the / hierarchy separator"
        )));
    }
    if path.split('/').any(str::is_empty) {
        return Some(NameRejection::Pattern(format!(
            "Name '{name}' carries an empty hierarchy level"
        )));
    }
    // The hierarchy rule: anything below the top level must carry the leading
    // separator, so `Dev/DBServer` is not a name Parameter Store accepts.
    if path.contains('/') && !name.starts_with('/') {
        return Some(NameRejection::Pattern(format!(
            "Name '{name}' is hierarchical and must include a leading forward slash character (/)"
        )));
    }
    if let Some(reserved) = RESERVED_NAME_PREFIXES.iter().find(|prefix| {
        // Case-insensitive, compared against the head of the path rather than a
        // lowercased copy of it, so matching allocates nothing. Every character of the
        // path has already been checked to be ASCII above, so the slice is on a
        // character boundary.
        path.len() >= prefix.len() && path[..prefix.len()].eq_ignore_ascii_case(prefix)
    }) {
        return Some(NameRejection::Pattern(format!(
            "Name '{name}' is prefixed with '{reserved}': a parameter name can't be prefixed with \
             {} (case-insensitive)",
            RESERVED_NAME_PREFIXES.join(" or ")
        )));
    }
    let levels = path.split('/').count();
    if levels > MAX_HIERARCHY_LEVELS {
        return Some(NameRejection::Depth(format!(
            "Name '{name}' is {levels} levels deep: a parameter hierarchy is limited to a maximum \
             depth of {MAX_HIERARCHY_LEVELS} levels"
        )));
    }
    None
}

/// The content size limit a parameter of `tier` holds (see [`STANDARD_VALUE_LIMIT`]
/// and [`ADVANCED_VALUE_LIMIT`]).
pub fn value_limit(tier: &str) -> usize {
    if tier == TIER_ADVANCED {
        ADVANCED_VALUE_LIMIT
    } else {
        STANDARD_VALUE_LIMIT
    }
}

/// The tier a put serves the parameter in.
///
/// Straight from `PutParameter`'s `Tier` doc: a request that names a tier is served in
/// that tier; one that names none keeps the tier the parameter already has, or — for a
/// parameter being created — gets the default tier configuration, which is the standard
/// tier ("The default tier when you begin using Parameter Store is the
/// standard-parameter tier", and `UpdateServiceSetting` — the operation that changes
/// it — is still a scaffold, so this emulator never leaves that default). Under
/// `Intelligent-Tiering` the request is evaluated against the three options the same
/// doc says require an advanced parameter: content over 4 KB, a parameter policy, or
/// more than 10,000 parameters already in the Region.
///
/// `stored` is the tier the parameter is in today, so an update that only changes a
/// value stays in its own tier: the no-revert rule the caller then faces fires only
/// when the request ASKS for a lower tier.
pub fn resolve_tier(
    requested: Option<&str>,
    stored: Option<&str>,
    value_len: usize,
    has_policies: bool,
    parameters_in_region: usize,
) -> String {
    match requested.or(stored) {
        None => TIER_STANDARD.to_string(),
        Some(TIER_INTELLIGENT_TIERING) => {
            // An advanced parameter can never be reverted, so evaluating a request
            // against the options keeps it advanced whatever this request carries. The
            // parameter-count option is stated for completeness and cannot fire while
            // the quota guard stands: `PutParameter` refuses a create once a Region
            // holds `MAX_PARAMETERS_PER_REGION` parameters, so the store never gets
            // past it.
            let needs_advanced = value_len > STANDARD_VALUE_LIMIT
                || has_policies
                || stored == Some(TIER_ADVANCED)
                || parameters_in_region > MAX_PARAMETERS_PER_REGION;
            if needs_advanced {
                TIER_ADVANCED.to_string()
            } else {
                TIER_STANDARD.to_string()
            }
        }
        Some(tier) => tier.to_string(),
    }
}

/// One stored version of one parameter: the value a put persisted, and everything the
/// service recorded alongside it.
#[derive(Debug, Clone)]
pub struct ParameterVersionRecord {
    /// This version's number, counting from 1 for a parameter's first version.
    pub version: i64,
    pub value: String,
    /// A `ParameterType` value.
    pub type_: String,
    /// A `ParameterDataType` value (see [`PARAMETER_DATA_TYPES`]).
    pub data_type: String,
    /// A `ParameterTier` value, already resolved (see [`resolve_tier`]).
    pub tier: String,
    pub description: Option<String>,
    pub allowed_pattern: Option<String>,
    pub key_id: Option<String>,
    pub policies: Option<String>,
    pub last_modified_date: SystemTime,
    /// The labels that point at THIS version. `LabelParameterVersion` is what writes
    /// them and is still a scaffold, so the set is empty today — but a `name:label`
    /// selector resolves against it rather than against anything invented, so a label
    /// nothing created resolves to nothing.
    pub labels: BTreeSet<String>,
}

impl ParameterVersionRecord {
    /// The modelled `Parameter` for this version of `record`.
    ///
    /// `selector` is the `name:selector` suffix the read carried, which is what the
    /// `Parameter$Selector` doc describes: "Either the version number or the label used
    /// to retrieve the parameter value".
    pub fn to_parameter(&self, record: &ParameterRecord, selector: Option<&str>) -> Parameter {
        Parameter {
            name: Some(record.name.clone()),
            r#type: Some(self.type_.clone()),
            value: Some(self.value.clone()),
            version: self.version,
            selector: selector.map(str::to_string),
            // `SourceResult` is "the raw result or response from the source" for a
            // parameter that references another Amazon Web Services service. Nothing
            // in this store references one, so there is no source result to report.
            source_result: None,
            last_modified_date: Some(::aws_smithy_types::DateTime::from(self.last_modified_date)),
            arn: Some(record.arn()),
            data_type: Some(self.data_type.clone()),
        }
    }
}

/// One stored parameter: its name, where it lives, its tags, and every version of it
/// the store still retains.
#[derive(Debug, Clone)]
pub struct ParameterRecord {
    /// The name as the create supplied it (trimmed), which is what a read reports.
    pub name: String,
    /// The Region the parameter was created in; part of its store key and of its ARN.
    pub region: String,
    pub tags: BTreeMap<String, String>,
    /// Every retained version, oldest first and the newest last. Never empty for a
    /// record that is in the store.
    pub versions: Vec<ParameterVersionRecord>,
}

impl ParameterRecord {
    /// The newest version, which is what a read with no selector answers with:
    /// "By default, if you don't specify a specific version, the system returns the
    /// latest parameter value when a parameter is called" (`PutParameter`'s `Version`
    /// doc).
    pub fn latest(&self) -> Option<&ParameterVersionRecord> {
        self.versions.last()
    }

    /// The oldest retained version, the one a new version displaces once the store
    /// holds [`MAX_VERSIONS_RETAINED`] of them.
    pub fn oldest(&self) -> Option<&ParameterVersionRecord> {
        self.versions.first()
    }

    /// The version a `name:selector` read resolves to, or `None` when this parameter
    /// has no such version or label.
    ///
    /// A selector is "either the version number or the label used to retrieve the
    /// parameter value" (`Parameter$Selector`), so an all-digit selector names a
    /// version and anything else names a label.
    pub fn select(&self, selector: Option<&str>) -> Option<&ParameterVersionRecord> {
        match selector {
            None => self.latest(),
            Some(selector) => match selector.parse::<i64>() {
                Ok(version) => self.versions.iter().find(|held| held.version == version),
                Err(_) => self
                    .versions
                    .iter()
                    .find(|held| held.labels.contains(selector)),
            },
        }
    }

    /// The number the next version of this parameter carries.
    pub fn next_version(&self) -> i64 {
        self.latest()
            .map_or(1, |latest| latest.version.saturating_add(1))
    }

    /// Append `version`, dropping the oldest retained version once more than
    /// [`MAX_VERSIONS_RETAINED`] are held — "Parameter Store deletes the oldest
    /// version when a new one is created".
    pub fn push_version(&mut self, version: ParameterVersionRecord) {
        self.versions.push(version);
        while self.versions.len() > MAX_VERSIONS_RETAINED {
            self.versions.remove(0);
        }
    }

    /// This parameter's ARN. A hierarchical name keeps its whole path after the
    /// `parameter/` prefix: `arn:aws:ssm:<region>:<account>:parameter/Dev/DBServer`.
    pub fn arn(&self) -> String {
        ::engine::emulator_arn(
            ARN_SERVICE,
            &self.region,
            &format!("{ARN_RESOURCE_PREFIX}{}", canonical_name(&self.name)),
        )
    }
}

/// What a read's `Name` names: which parameter, in which Region and account, and the
/// selector that chose a version or a label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterQuery<'a> {
    /// The canonical name (no leading `/`) to look the parameter up by.
    pub name: &'a str,
    /// The `name:selector` suffix the request carried, without the colon, or `None`
    /// when it named none.
    pub selector: Option<&'a str>,
    /// The Region a full ARN named, or `None` when the request named a bare name.
    pub region: Option<&'a str>,
    /// The account a full ARN named.
    pub account: Option<&'a str>,
}

/// What `name` names, or `None` when it is an ARN that names no `ssm` parameter.
///
/// `GetParameter` takes "the name or Amazon Resource Name (ARN) of the parameter that
/// you want to query", and either form may carry a selector ("To query by parameter
/// label, use "Name": "name:label". To query by parameter version, use "Name":
/// "name:version""). An ARN is split on its six fields first, so the colons inside it
/// are never mistaken for a selector; a parameter name itself can carry no colon.
pub fn parse_parameter_query(name: &str) -> Option<ParameterQuery<'_>> {
    let (region, account, rest) = if name.starts_with(ARN_PREFIX) {
        let mut fields = name.splitn(6, ':');
        // arn : partition : service : region : account : resource
        let (_arn, _partition) = (fields.next()?, fields.next()?);
        let service = fields.next()?;
        let region = fields.next()?;
        let account = fields.next()?;
        let resource = fields.next()?;
        if service != ARN_SERVICE {
            return None;
        }
        (
            Some(region),
            Some(account),
            resource.strip_prefix(ARN_RESOURCE_PREFIX)?,
        )
    } else {
        (None, None, name)
    };
    let (path, selector) = match rest.rsplit_once(':') {
        Some((before, after)) if !before.is_empty() => (before, Some(after)),
        _ => (rest, None),
    };
    Some(ParameterQuery {
        name: canonical_name(path),
        selector,
        region,
        account,
    })
}

/// Per-service state for com-amazonaws-ssm-amazonssm. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSsmAmazonssmState {
    /// Every parameter, keyed by `(region, canonical name)`: "A parameter name must be
    /// unique within an Amazon Web Services Region". A `BTreeMap` so a listing walks a
    /// stable, total order.
    pub parameters: BTreeMap<(String, String), ParameterRecord>,
}

impl ComAmazonawsSsmAmazonssmState {
    /// The parameter `name` names in `region`, or `None` when that Region holds none.
    pub fn parameter(&self, region: &str, name: &str) -> Option<&ParameterRecord> {
        self.parameters
            .get(&(region.to_string(), canonical_name(name).to_string()))
    }

    /// The same record, to append a version or replace its tags.
    pub fn parameter_mut(&mut self, region: &str, name: &str) -> Option<&mut ParameterRecord> {
        self.parameters
            .get_mut(&(region.to_string(), canonical_name(name).to_string()))
    }

    /// Store `record` as `region`'s parameter of that name.
    pub fn insert_parameter(&mut self, record: ParameterRecord) {
        let key = (
            record.region.clone(),
            canonical_name(&record.name).to_string(),
        );
        self.parameters.insert(key, record);
    }

    /// How many parameters `region` holds, which is what the per-Region quota and the
    /// `Intelligent-Tiering` evaluation are measured against.
    pub fn parameters_in_region(&self, region: &str) -> usize {
        self.parameters
            .keys()
            .filter(|(held, _)| held == region)
            .count()
    }
}

/// A modelled `TagList` as the store holds it: ordered, so two reads of one record
/// report its tags in the same order.
pub fn to_btree_map(tags: Option<Vec<Tag>>) -> BTreeMap<String, String> {
    tags.unwrap_or_default()
        .into_iter()
        .map(|tag| (tag.key, tag.value))
        .collect()
}

/// The region this request was signed for, from its SigV4 credential scope; the
/// emulator's default region when it carries none.
///
/// The scope is caller-controlled text and the value lands inside an ARN and in a
/// store key, so anything outside `[A-Za-z0-9-]` is refused here and the default used
/// instead: a generated ARN can never carry text an ARN may not hold.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 64
                && region
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
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
