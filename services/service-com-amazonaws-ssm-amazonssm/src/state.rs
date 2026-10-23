// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ssm-amazonssm service.
//!
//! Two slices are modelled here. The first is **Parameter Store**: `PutParameter`
//! creates or updates a parameter and `GetParameter` reads one back. Both reach the one
//! store below, so whatever a put persisted is what a later get reports.
//!
//! A parameter lives at a name that is unique *within a Region* (the `PutParameter`
//! `Name` doc: "A parameter name must be unique within an Amazon Web Services Region"),
//! so the store is keyed by `(region, canonical name)`. Each parameter holds its
//! **versions**, oldest first: a put that overwrites an existing name appends a new
//! version rather than replacing the old one, which is what lets a `name:version`
//! selector resolve an earlier value.
//!
//! The second is the **managed nodes** the console-facing describes report
//! ([`ManagedNodeRecord`]), keyed by `(region, node id)` and filtered by the keys
//! [`is_node_filter_key`] accepts.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use crate::ops::types::{
    InstanceProperty, Parameter, Tag, ASSOCIATION_STATUS_NAME_VALUES,
    INSTANCE_PROPERTY_FILTER_KEY_VALUES, PING_STATUS_VALUES, PLATFORM_TYPE_VALUES,
    RESOURCE_TYPE_VALUES, SOURCE_TYPE_VALUES,
};

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

    /// Every managed node, keyed by `(region, node id)`: a node is registered with the
    /// Systems Manager endpoint of one Region and is identified there by its node id. A
    /// `BTreeMap` so a describe walks a stable, total order and can resume a page from
    /// the id it stopped at.
    ///
    /// Nothing registers a node yet: registration is the SSM Agent's own call, which
    /// this service's model does not expose, and every operation that would admit one
    /// another way (`CreateActivation`, `CreateAssociation`, `PutInventory`) is still a
    /// scaffold. A describe therefore reports the empty set for a fresh emulator — what
    /// the store holds — rather than nodes nobody registered.
    pub managed_nodes: BTreeMap<(String, String), ManagedNodeRecord>,
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

    /// Every managed node `region` holds, in ascending node-id order, as
    /// `(node id, record)`.
    pub fn managed_nodes_in_region<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = (&'a str, &'a ManagedNodeRecord)> + 'a {
        self.managed_nodes
            .iter()
            .filter(move |((held, _), _)| held == region)
            .map(|((_, id), node)| (id.as_str(), node))
    }

    /// Store `node` as a managed node of `region`, keyed by the node id it reports.
    ///
    /// Answers `false` for a record reporting no node id: a managed node is identified
    /// by that id, so there is no key to store one without it.
    pub fn insert_managed_node(&mut self, region: &str, node: ManagedNodeRecord) -> bool {
        let Some(id) = node.node_id().map(str::to_string) else {
            return false;
        };
        self.managed_nodes.insert((region.to_string(), id), node);
        true
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

// ---------------------------------------------------------------------------
// Managed nodes
// ---------------------------------------------------------------------------

/// The filter keys a managed-node describe accepts beside the modelled
/// `InstancePropertyFilterKey` enum (`ActivationIds`, `AgentVersion`,
/// `AssociationStatus`, `DocumentName`, `IamRole`, `InstanceIds`, `PingStatus`,
/// `PlatformTypes`, `ResourceType`).
///
/// Both come from this service's own model: the enum is what
/// `InstancePropertyFilter$key` targets, and these two are the remaining keys the
/// sibling string filter over the same managed nodes publishes — "Valid filter key
/// values: ActivationIds | AgentVersion | AssociationStatus | IamRole | InstanceIds |
/// PingStatus | PlatformTypes | ResourceType | SourceIds | SourceTypes | "tag-key" |
/// "tag:{keyname}"" (`InstanceInformationStringFilter$Key`). `InstanceProperty`
/// reports a `SourceId` and a `SourceType` for every node, so both are properties this
/// store can answer a filter on.
pub const EXTRA_NODE_FILTER_KEYS: &[&str] = &["SourceIds", "SourceTypes"];

/// The filter key that matches a node carrying a tag of a given KEY, whatever its
/// value: "Valid tag examples: `Key=tag-key,Values=Purpose`" (same doc).
pub const TAG_KEY_FILTER_KEY: &str = "tag-key";

/// The prefix of the filter key that matches a node whose tag of that name has a given
/// value: "`Key=tag:Purpose,Values=Test`" (same doc).
pub const TAG_FILTER_KEY_PREFIX: &str = "tag:";

/// The filter operator a filter that names none is served with: the modelled
/// `InstancePropertyFilterOperator` values are `Equal`, `NotEqual`, `BeginWith`,
/// `LessThan` and `GreaterThan`, and `InstancePropertyFilter` — which carries no
/// operator at all — selects the nodes whose property EQUALS one of its values.
pub const OPERATOR_EQUAL: &str = "Equal";

/// See [`OPERATOR_EQUAL`].
pub const OPERATOR_NOT_EQUAL: &str = "NotEqual";

/// See [`OPERATOR_EQUAL`].
pub const OPERATOR_BEGIN_WITH: &str = "BeginWith";

/// See [`OPERATOR_EQUAL`].
pub const OPERATOR_LESS_THAN: &str = "LessThan";

/// See [`OPERATOR_EQUAL`].
pub const OPERATOR_GREATER_THAN: &str = "GreaterThan";

/// Whether `key` is a filter key a managed-node describe knows, which is what tells an
/// accepted filter from a refused one. The comparison is exact: a filter key is a name,
/// not a prose label, so neither a different spelling nor a different case names a
/// property of a node.
pub fn is_node_filter_key(key: &str) -> bool {
    INSTANCE_PROPERTY_FILTER_KEY_VALUES.contains(&key)
        || EXTRA_NODE_FILTER_KEYS.contains(&key)
        || key == TAG_KEY_FILTER_KEY
        || key
            .strip_prefix(TAG_FILTER_KEY_PREFIX)
            .is_some_and(|name| !name.is_empty())
}

/// Why `key` is not a filter key a managed-node describe accepts, as the service words
/// it: the key the caller submitted, then the member it filled — "`<Key>` filter Key is
/// invalid." The text is built from the request, so a caller reading it knows which
/// filter to fix.
pub fn filter_key_rejection(key: &str) -> String {
    format!("{key} filter Key is invalid.")
}

/// Every filter key a managed-node describe accepts, for a refusal or a report that
/// lists them. The tag form is listed as the doc writes it, with the tag name as a
/// placeholder.
pub fn node_filter_keys() -> Vec<&'static str> {
    INSTANCE_PROPERTY_FILTER_KEY_VALUES
        .iter()
        .chain(EXTRA_NODE_FILTER_KEYS.iter())
        .copied()
        .chain([TAG_KEY_FILTER_KEY, "tag:{keyname}"])
        .collect()
}

/// The values `key` accepts, for the filter keys whose value set this service's model
/// publishes, or `None` for a key whose values are free text (a node id, an agent
/// version, a role name, a tag value).
///
/// Each set is the modelled enum the property targets, which is also what
/// `InstanceInformationStringFilter$Key` lists for the same keys ("Valid values for the
/// `PingStatus` filter key: Online | ConnectionLost | Inactive", and so on for
/// `AssociationStatus`, `PlatformTypes`, `ResourceType` and `SourceType`).
pub fn node_filter_key_value_set(key: &str) -> Option<&'static [&'static str]> {
    match key {
        "AssociationStatus" => Some(ASSOCIATION_STATUS_NAME_VALUES),
        "PingStatus" => Some(PING_STATUS_VALUES),
        "PlatformTypes" => Some(PLATFORM_TYPE_VALUES),
        "ResourceType" => Some(RESOURCE_TYPE_VALUES),
        "SourceTypes" => Some(SOURCE_TYPE_VALUES),
        _ => None,
    }
}

/// Whether `id` is shaped like a managed-node id, from the modelled `InstanceId`
/// pattern `^(^i-(\w{8}|\w{17})$)|(^mi-\w{17}$)$`: an EC2 instance id with an 8- or
/// 17-character suffix, or a 17-character hybrid-activation (`mi-`) id.
pub fn managed_node_id_is_well_formed(id: &str) -> bool {
    let is_word = |rest: &str| rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if let Some(rest) = id.strip_prefix("i-") {
        return (rest.len() == 8 || rest.len() == 17) && is_word(rest);
    }
    if let Some(rest) = id.strip_prefix("mi-") {
        return rest.len() == 17 && is_word(rest);
    }
    false
}

/// How `LessThan` and `GreaterThan` order two filter values.
///
/// Both operators are declared for a filter over properties that are mostly dotted
/// numbers (`AgentVersion` is modelled as `^[0-9]{1,6}(\.[0-9]{1,6}){2,3}$`), so two
/// values whose every dot-separated segment is a number are ordered segment by segment
/// as numbers — `3.0.1` is above `3.0.10` nowhere, unlike a text comparison. Anything
/// else is ordered as text.
pub fn compare_filter_values(left: &str, right: &str) -> Ordering {
    let segments = |value: &str| {
        value
            .split('.')
            .map(str::parse::<u64>)
            .collect::<Result<Vec<u64>, _>>()
            .ok()
    };
    match (segments(left), segments(right)) {
        (Some(left), Some(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

/// Whether a node's `held` value satisfies `operator` against a filter's `wanted`
/// value.
pub fn operator_matches(operator: &str, held: &str, wanted: &str) -> bool {
    match operator {
        OPERATOR_NOT_EQUAL => held != wanted,
        OPERATOR_BEGIN_WITH => held.starts_with(wanted),
        OPERATOR_LESS_THAN => compare_filter_values(held, wanted) == Ordering::Less,
        OPERATOR_GREATER_THAN => compare_filter_values(held, wanted) == Ordering::Greater,
        // `Equal`, and the operator an `InstancePropertyFilter` does not carry.
        _ => held == wanted,
    }
}

/// One managed node, as the console-facing describes report it.
///
/// `properties` is the modelled `InstanceProperty` itself, so a describe reports what
/// was persisted rather than a second copy of it; `tags` is what the `tag-key` and
/// `tag:<name>` filter keys match against, which no response member carries.
#[derive(Debug, Clone, Default)]
pub struct ManagedNodeRecord {
    /// Everything a describe reports about this node.
    pub properties: InstanceProperty,
    /// The node's tags, keyed by tag key.
    pub tags: BTreeMap<String, String>,
}

impl ManagedNodeRecord {
    /// The node id this record is stored under, from the properties it reports.
    pub fn node_id(&self) -> Option<&str> {
        self.properties.instance_id.as_deref()
    }

    /// The values of this node that a filter on `key` is matched against, or `None`
    /// when `key` is not a filter key at all ([`is_node_filter_key`]).
    ///
    /// A node the store holds no value of that property for answers with an EMPTY list
    /// and so is reported by no filter on it, whatever the operator: a filter selects
    /// the nodes whose property satisfies it, and a node without the property has
    /// nothing to satisfy it with.
    pub fn filter_values(&self, key: &str) -> Option<Vec<&str>> {
        fn one(value: &Option<String>) -> Option<Vec<&str>> {
            Some(value.as_deref().into_iter().collect())
        }
        match key {
            "ActivationIds" => one(&self.properties.activation_id),
            "AgentVersion" => one(&self.properties.agent_version),
            "AssociationStatus" => one(&self.properties.association_status),
            "IamRole" => one(&self.properties.iam_role),
            "InstanceIds" => one(&self.properties.instance_id),
            "PingStatus" => one(&self.properties.ping_status),
            "PlatformTypes" => one(&self.properties.platform_type),
            "ResourceType" => one(&self.properties.resource_type),
            "SourceIds" => one(&self.properties.source_id),
            "SourceTypes" => one(&self.properties.source_type),
            // `DocumentName` selects the nodes an association of that SSM document
            // targets. Associations are a separate resource of this service and no
            // operation that creates one is implemented yet, so no node in this store
            // is the target of any document and a filter on it selects nothing — which
            // is the store's answer, not a refusal.
            "DocumentName" => Some(Vec::new()),
            TAG_KEY_FILTER_KEY => Some(self.tags.keys().map(String::as_str).collect()),
            _ => match key.strip_prefix(TAG_FILTER_KEY_PREFIX) {
                Some(name) if !name.is_empty() => {
                    Some(self.tags.get(name).map(String::as_str).into_iter().collect())
                }
                _ => None,
            },
        }
    }

    /// Whether this node satisfies one filter: any value it holds for `key` against any
    /// of the filter's `values`, under `operator`.
    pub fn matches_filter(&self, key: &str, values: &[String], operator: &str) -> bool {
        let Some(held) = self.filter_values(key) else {
            return false;
        };
        held.iter().any(|held| {
            values
                .iter()
                .any(|wanted| operator_matches(operator, held, wanted))
        })
    }
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
