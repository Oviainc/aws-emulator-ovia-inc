// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ssm-amazonssm service.
//!
//! Three slices are modelled here. The first is **Parameter Store**: `PutParameter`
//! creates or updates a parameter, `GetParameter` reads one back and
//! `GetParametersByPath` reads the parameters stored under one level of a hierarchy
//! ([`parameter_path`], [`is_under_path`]). All of them reach the one store below, so
//! whatever a put persisted is what a later read reports.
//!
//! A parameter lives at a name that is unique *within a Region* (the `PutParameter`
//! `Name` doc: "A parameter name must be unique within an Amazon Web Services Region"),
//! so the store is keyed by `(region, canonical name)`. Each parameter holds its
//! **versions**, oldest first: a put that overwrites an existing name appends a new
//! version rather than replacing the old one, which is what lets a `name:version`
//! selector resolve an earlier value.
//!
//! The second is **SSM documents**: `CreateDocument` registers a document and
//! `DescribeDocument` reports it. A document is keyed by `(region, name)` the same way,
//! and everything a read reports beyond the request's own members — the schema version,
//! the description, the parameters, the platform types, the hash — is derived from the
//! content that was persisted (see [`parse_document_content`]).
//!
//! The third is the **managed nodes** the node describes report
//! ([`ManagedNodeRecord`]), keyed by `(region, node id)`: `DescribeInstanceProperties`
//! projects a node's `properties` and `DescribeInstanceInformation` projects the same
//! record through [`ManagedNodeRecord::to_instance_information`], so both report the one
//! stored node. A node enters the store only by registering — nothing here invents one —
//! so an account that has registered none is reported as holding none. The filter keys a
//! describe accepts are [`is_node_filter_key`] (the property describe) and
//! [`is_instance_information_filter_key`] (the string-filter describe).

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AttachmentInformation, DocumentDescription, DocumentParameter, DocumentRequires,
    InstanceInformation, InstanceProperty, Parameter, Tag, ASSOCIATION_STATUS_NAME_VALUES,
    INSTANCE_INFORMATION_FILTER_KEY_VALUES, INSTANCE_PROPERTY_FILTER_KEY_VALUES,
    PING_STATUS_VALUES, PLATFORM_TYPE_VALUES, RESOURCE_TYPE_VALUES, SOURCE_TYPE_VALUES,
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

impl ParameterVersionRecord {
    /// The values of `key` this version holds, for a `ParameterStringFilter` to compare
    /// against, or `None` when the key names no attribute of a parameter version.
    ///
    /// A version has exactly one type, at most one key id (only a `SecureString` names
    /// one) and any number of labels, so a key that names an attribute this version
    /// does not carry yields an EMPTY list — which matches no filter value, rather than
    /// refusing the filter.
    pub fn filter_values(&self, key: &str) -> Option<Vec<&str>> {
        match key {
            FILTER_KEY_TYPE => Some(vec![self.type_.as_str()]),
            FILTER_KEY_KEY_ID => Some(self.key_id.as_deref().into_iter().collect()),
            FILTER_KEY_LABEL => Some(self.labels.iter().map(String::as_str).collect()),
            _ => None,
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

// ---------------------------------------------------------------------------
// SSM documents
// ---------------------------------------------------------------------------

/// The resource portion a document ARN carries before the document's name, e.g.
/// `arn:aws:ssm:us-east-2:111122223333:document/MyDocument`.
pub const ARN_DOCUMENT_RESOURCE_PREFIX: &str = "document/";

/// `DocumentStatus` values, from the modelled enum
/// (`crate::ops::types::DOCUMENT_STATUS_VALUES`): a document Systems Manager is still
/// registering is `Creating`, and a registered one is `Active`.
pub const DOCUMENT_STATUS_CREATING: &str = "Creating";

/// See [`DOCUMENT_STATUS_CREATING`].
pub const DOCUMENT_STATUS_ACTIVE: &str = "Active";

/// How long a document stays `Creating` before the clock owes its move to `Active`.
///
/// AWS publishes no span for it, so this is the emulator's CHOSEN transient window.
/// It is deliberately never zero: a caller that reads a document straight after
/// creating it observes `Creating`, the way it would against the real service, and the
/// move is materialised by [`ComAmazonawsSsmAmazonssmState::materialize_due`] when the
/// clock makes it due rather than by any call.
pub const DOCUMENT_TRANSITION_DELAY: Duration = Duration::from_millis(500);

/// The version a newly created document carries: "The document version" counts from 1,
/// and `UpdateDocument` — still a scaffold — is what mints a second one.
pub const FIRST_DOCUMENT_VERSION: &str = "1";

/// The `DocumentVersion` selectors that name a version by role rather than by number
/// (the modelled `DocumentVersion` pattern `^([$]LATEST|[$]DEFAULT|^[1-9][0-9]*$)$`).
pub const DOCUMENT_VERSION_LATEST: &str = "$LATEST";

/// See [`DOCUMENT_VERSION_LATEST`].
pub const DOCUMENT_VERSION_DEFAULT: &str = "$DEFAULT";

/// "The content of the document must not exceed 64KB" (`CreateDocument`'s `Content`
/// doc), which `MaxDocumentSizeExceeded` ("The size limit of a document is 64 KB")
/// reports.
pub const MAX_DOCUMENT_CONTENT: usize = 64 * 1024;

/// "You can have at most 500 active SSM documents" (the `DocumentLimitExceeded` doc):
/// the published per-Region quota.
pub const MAX_DOCUMENTS: usize = 500;

/// How many documents this store holds in TOTAL, across every Region.
///
/// Emulator resource guard, NOT a published AWS quota, and the bound that actually
/// keeps this process finite: the Region a document is stored under comes from the
/// request's own credential scope ([`request_region`]), so the per-Region quota alone
/// would be one quota per caller-chosen partition of the key space and the store could
/// grow without bound. Set ABOVE the per-Region quota so that quota stays exact, and
/// low enough that the worst case a create can reach — this many documents each holding
/// [`MAX_DOCUMENT_CONTENT`] of content — is a size a test process can hold (about 64
/// MB). A create past it is refused with the same modelled `DocumentLimitExceeded` as
/// one past the per-Region quota.
pub const MAX_DOCUMENTS_TOTAL: usize = 2 * MAX_DOCUMENTS;

/// "JSON is the default format" (`CreateDocument`'s `DocumentFormat` doc).
pub const DEFAULT_DOCUMENT_FORMAT: &str = "JSON";

/// The `DocumentFormat` whose content is free text rather than a document tree — a
/// Change Calendar entry, say — so nothing is derived from it.
pub const DOCUMENT_FORMAT_TEXT: &str = "TEXT";

/// The document type a create that names none produces: `Command`, Systems Manager's
/// default document type (`DocumentType` is optional on `CreateDocument` and a
/// run-command document is what it produces).
pub const DEFAULT_DOCUMENT_TYPE: &str = "Command";

/// The document type whose versions go through the review workflow, and so the only
/// one that reports a `ReviewStatus` ("The current status of the review",
/// `DocumentDescription$ReviewStatus`).
pub const CHANGE_TEMPLATE_DOCUMENT_TYPE: &str = "Automation.ChangeTemplate";

/// The review status a document version carries before anybody reviews it, from the
/// modelled `ReviewStatus` enum (`crate::ops::types::REVIEW_STATUS_VALUES`).
pub const DOCUMENT_REVIEW_STATUS_NOT_REVIEWED: &str = "NOT_REVIEWED";

/// The hash type a document carries: "The Sha256 or Sha1 hash created by the system
/// when the document was created" with "Sha1 hashes have been deprecated"
/// (`DocumentDescription$Hash` / `$HashType`), so the system hash is Sha256 and the
/// deprecated `Sha1` member is left unset.
pub const DOCUMENT_HASH_TYPE: &str = "Sha256";

/// "You can't use the following strings as document name prefixes. These are reserved
/// by Amazon Web Services for use as document name prefixes: aws amazon amzn AWSEC2
/// AWSConfigRemediation AWSSupport" (`CreateDocument`'s `Name` doc). Compared
/// case-insensitively, the way Systems Manager reserves the `aws` family.
pub const RESERVED_DOCUMENT_NAME_PREFIXES: &[&str] = &[
    "aws",
    "amazon",
    "amzn",
    "AWSEC2",
    "AWSConfigRemediation",
    "AWSSupport",
];

/// How deep a document's content may nest, and how many flow openers (`[`, `{`) and
/// compact indicators (`-`, `?`) the untrusted bytes may carry.
///
/// The YAML parser descends recursively, so nesting has to be bounded BEFORE a tree
/// exists to measure (see [`guard_document_content`]). Every bound is far above what a
/// real SSM document needs.
const MAX_DOCUMENT_DEPTH: usize = 64;

/// See [`MAX_DOCUMENT_DEPTH`].
const MAX_DOCUMENT_FLOW_OPENERS: usize = 1024;

/// See [`MAX_DOCUMENT_DEPTH`].
const MAX_DOCUMENT_COMPACT_PER_LINE: usize = 256;

/// See [`MAX_DOCUMENT_DEPTH`].
const MAX_DOCUMENT_INDENT: usize = 256;

/// The schema versions a document of `document_type` may declare, or an EMPTY slice
/// for a type whose content carries no Systems Manager schema at all (an AppConfig
/// configuration, a CloudFormation template, a Change Calendar entry).
///
/// From the schema-version rules Systems Manager publishes per document type
/// ("Systems Manager document schemas and features" in the User Guide): Command
/// documents support 1.2, 2.0 and 2.2, Policy documents 1.0 and 2.0, Session documents
/// 1.0, Package documents 2.0, and Automation documents and change templates 0.3.
/// A content declaring anything else for one of those types is what
/// `InvalidDocumentSchemaVersion` ("The version of the document schema isn't
/// supported") reports.
pub fn supported_schema_versions(document_type: &str) -> &'static [&'static str] {
    match document_type {
        "Command" => &["1.2", "2.0", "2.2"],
        "Policy" => &["1.0", "2.0"],
        "Session" => &["1.0"],
        "Package" => &["2.0"],
        "Automation" | "Automation.ChangeTemplate" => &["0.3"],
        _ => &[],
    }
}

/// Why `name` is not a document name `CreateDocument` accepts, or `None` when it is.
///
/// The modelled `DocumentName` pattern (`^[a-zA-Z0-9_\-.]{3,128}$`) is already refused
/// by the boundary walk, so the only rule left is the reserved-prefix one the `Name`
/// doc states ([`RESERVED_DOCUMENT_NAME_PREFIXES`]).
pub fn document_name_rejection(name: &str) -> Option<String> {
    let reserved = RESERVED_DOCUMENT_NAME_PREFIXES.iter().find(|prefix| {
        name.get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
    })?;
    Some(format!(
        "Name '{name}' is prefixed with '{reserved}': the prefixes {} are reserved by \
         Amazon Web Services for use as document name prefixes",
        RESERVED_DOCUMENT_NAME_PREFIXES.join(" ")
    ))
}

/// What a `DocumentARN` value names: which document, and — when the value is a full
/// ARN — in which Region and account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentQuery<'a> {
    /// The document name to look the document up by.
    pub name: &'a str,
    /// The Region a full ARN named, or `None` when the request named a bare name.
    pub region: Option<&'a str>,
    /// The account a full ARN named.
    pub account: Option<&'a str>,
}

/// What `value` names, or `None` when it is an ARN that names no `ssm` document.
///
/// A `DocumentARN` is "the name of the required SSM document. The name can be an
/// Amazon Resource Name (ARN)" (`DocumentRequires$Name`), so either spelling resolves
/// to the same document; the ARN is read through the one parse every service reads a
/// caller-supplied ARN with.
pub fn parse_document_query(value: &str) -> Option<DocumentQuery<'_>> {
    if !value.starts_with(ARN_PREFIX) {
        return Some(DocumentQuery {
            name: value,
            region: None,
            account: None,
        });
    }
    let arn = ::service_common::arn::parse_of(value, ARN_SERVICE)?;
    Some(DocumentQuery {
        name: arn.resource.strip_prefix(ARN_DOCUMENT_RESOURCE_PREFIX)?,
        region: Some(arn.region),
        account: Some(arn.account),
    })
}

/// Why a document's content is not one this service accepts. Each variant maps to the
/// modelled error that reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentRejection {
    /// The content isn't a document in the declared format: `InvalidDocumentContent`.
    Content(String),
    /// The content declares a schema version the document type doesn't support:
    /// `InvalidDocumentSchemaVersion`.
    SchemaVersion(String),
}

/// What a document's content declares: everything a read reports that is derived from
/// the content rather than echoed from the create's own members.
#[derive(Debug, Clone, Default)]
pub struct ParsedDocumentContent {
    /// The content's `schemaVersion`, which is what `SchemaVersion` reports.
    pub schema_version: Option<String>,
    /// The content's `description`.
    pub description: Option<String>,
    /// One entry per `parameters` entry, in the content's own order.
    pub parameters: Vec<DocumentParameter>,
    /// The platforms the content's step preconditions restrict the document to, or
    /// every modelled platform when it restricts it to none.
    pub platform_types: Vec<String>,
}

/// The documented size bound, applied to content of EVERY format: "The content of the
/// document must not exceed 64KB" ([`MAX_DOCUMENT_CONTENT`]).
fn content_within_size(content: &str) -> Result<(), ContentRejection> {
    if content.len() > MAX_DOCUMENT_CONTENT {
        return Err(ContentRejection::Content(format!(
            "Content is {} bytes; a document's content must not exceed {MAX_DOCUMENT_CONTENT} bytes",
            content.len()
        )));
    }
    Ok(())
}

/// Pre-flight bounds on untrusted content, applied BEFORE any parser builds a tree
/// from it: the YAML parser descends recursively, so the nesting the bytes can ask for
/// has to be bounded while there is still no tree to measure.
///
/// This is a GUARD, not a parser: it knows nothing about YAML context and measures only
/// quantities a hiding trick can push UP but never DOWN — the content's length, the
/// number of flow openers (whose count IS their depth bound, since flow nesting costs
/// neither a line nor a column), the compact indicators on any one line, and a line's
/// leading indentation. Closers are never subtracted, so a `]` inside a scalar cannot
/// buy depth back. The same guard the Greengrass recipe parser
/// (`service-com-amazonaws-greengrassv2-greengrassv2/src/state.rs`) and the
/// CloudFormation template parser apply, for the same reason and with the same bounds;
/// the three copies are to be migrated onto one shared implementation rather than
/// copied a fourth time.
fn guard_document_content(content: &str) -> Result<(), ContentRejection> {
    content_within_size(content)?;
    let mut flow_openers = 0usize;
    let mut compact_on_line = 0usize;
    let mut indent = 0usize;
    let mut counting_indent = true;
    for byte in content.bytes() {
        match byte {
            b'\n' | b'\r' => {
                indent = 0;
                compact_on_line = 0;
                counting_indent = true;
            }
            b' ' | b'\t' if counting_indent => {
                indent += 1;
                if indent > MAX_DOCUMENT_INDENT {
                    return Err(ContentRejection::Content(format!(
                        "a line of Content must not be indented past {MAX_DOCUMENT_INDENT} columns"
                    )));
                }
            }
            b'[' | b'{' => {
                counting_indent = false;
                flow_openers += 1;
                if flow_openers > MAX_DOCUMENT_FLOW_OPENERS {
                    return Err(ContentRejection::Content(format!(
                        "Content must not carry more than {MAX_DOCUMENT_FLOW_OPENERS} '[' or '{{' \
                         openers"
                    )));
                }
            }
            b'-' | b'?' => {
                counting_indent = false;
                compact_on_line += 1;
                if compact_on_line > MAX_DOCUMENT_COMPACT_PER_LINE {
                    return Err(ContentRejection::Content(format!(
                        "a line of Content must not carry more than \
                         {MAX_DOCUMENT_COMPACT_PER_LINE} '-' or '?' indicators"
                    )));
                }
            }
            _ => counting_indent = false,
        }
    }
    Ok(())
}

/// Refuses content that uses a YAML alias, BEFORE the loader can expand one.
///
/// `yaml-rust2`'s loader RESOLVES an alias by copying the anchored node, so by the
/// time a tree exists the expansion has already happened — a short alias chain can
/// expand to gigabytes. The parser's own event stream reports an alias as
/// `Event::Alias` and copies nothing, so the content is parsed once for this check and
/// only alias-free content is handed to the loader. An anchor no alias refers to
/// expands nothing and is accepted.
fn reject_document_aliases(content: &str) -> Result<(), ContentRejection> {
    #[derive(Default)]
    struct AliasDetector {
        saw_alias: bool,
    }
    impl ::yaml_rust2::parser::EventReceiver for AliasDetector {
        fn on_event(&mut self, event: ::yaml_rust2::parser::Event) {
            if matches!(event, ::yaml_rust2::parser::Event::Alias(_)) {
                self.saw_alias = true;
            }
        }
    }

    let mut detector = AliasDetector::default();
    ::yaml_rust2::parser::Parser::new_from_str(content)
        .load(&mut detector, true)
        .map_err(|why| {
            ContentRejection::Content(format!("Content could not be parsed as a document: {why}"))
        })?;
    if detector.saw_alias {
        return Err(ContentRejection::Content(
            "Content must not use YAML aliases".to_string(),
        ))
    }
    Ok(())
}

/// Look a key up in a document mapping.
fn mapping_get<'a>(
    map: &'a ::yaml_rust2::yaml::Hash,
    key: &str,
) -> Option<&'a ::yaml_rust2::Yaml> {
    map.iter()
        .find(|(held, _)| held.as_str() == Some(key))
        .map(|(_, value)| value)
}

/// A document scalar as the string the API reports it as.
fn scalar_string(value: &::yaml_rust2::Yaml) -> Option<String> {
    match value {
        ::yaml_rust2::Yaml::String(text) | ::yaml_rust2::Yaml::Real(text) => Some(text.clone()),
        ::yaml_rust2::Yaml::Boolean(flag) => Some(flag.to_string()),
        ::yaml_rust2::Yaml::Integer(number) => Some(number.to_string()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Reading a hierarchy of parameters
// ---------------------------------------------------------------------------

/// A `ParameterStringFilter` key that filters on a parameter's `ParameterType`.
pub const FILTER_KEY_TYPE: &str = "Type";

/// A `ParameterStringFilter` key that filters on the KMS key a `SecureString` names.
pub const FILTER_KEY_KEY_ID: &str = "KeyId";

/// A `ParameterStringFilter` key that filters on a version's labels.
pub const FILTER_KEY_LABEL: &str = "Label";

/// The `Key` values a `ParameterStringFilter` may carry on a hierarchy read.
///
/// The modelled key pattern is `^(tag:.+|Name|Type|KeyId|Path|Label|Tier|DataType)$`,
/// and "For `GetParametersByPath`, the following patterns listed for `Key` aren't valid:
/// `tag`, `DataType`, `Name`, `Path`, and `Tier`" (the `ParameterStringFilter$Key`
/// doc) — which leaves these three.
pub const PARAMETERS_BY_PATH_FILTER_KEYS: &[&str] =
    &[FILTER_KEY_TYPE, FILTER_KEY_KEY_ID, FILTER_KEY_LABEL];

/// Compare a filter's value against the whole held value.
pub const FILTER_OPTION_EQUALS: &str = "Equals";

/// Compare a filter's value against the start of the held value.
pub const FILTER_OPTION_BEGINS_WITH: &str = "BeginsWith";

/// The comparison options a hierarchy read's filters accept: "For filters used with
/// `GetParametersByPath`, valid options include `Equals` and `BeginsWith`" (the
/// `ParameterStringFilter$Option` doc).
pub const PARAMETERS_BY_PATH_FILTER_OPTIONS: &[&str] =
    &[FILTER_OPTION_EQUALS, FILTER_OPTION_BEGINS_WITH];

/// The options a `Label` filter accepts: "(Exception: For filters using `Label` as the
/// Key name, the only valid option is `Equals`.)" (the same doc).
pub const LABEL_FILTER_OPTIONS: &[&str] = &[FILTER_OPTION_EQUALS];

/// Whether `key` is a filter key a hierarchy read accepts (see
/// [`PARAMETERS_BY_PATH_FILTER_KEYS`]).
pub fn is_parameters_by_path_filter_key(key: &str) -> bool {
    PARAMETERS_BY_PATH_FILTER_KEYS.contains(&key)
}

/// The comparison options a hierarchy read accepts for `key`.
pub fn parameters_by_path_filter_options(key: &str) -> &'static [&'static str] {
    if key == FILTER_KEY_LABEL {
        LABEL_FILTER_OPTIONS
    } else {
        PARAMETERS_BY_PATH_FILTER_OPTIONS
    }
}

/// Whether `held` carries a value matching one of `wanted` under `option`.
///
/// `held` is every value the stored record carries for the filter's key (a parameter
/// has one type, at most one key id, and any number of labels), and a filter matches
/// when any held value matches any wanted one.
pub fn string_filter_matches(held: &[&str], option: &str, wanted: &[String]) -> bool {
    held.iter().any(|value| {
        wanted.iter().any(|want| {
            if option == FILTER_OPTION_BEGINS_WITH {
                value.starts_with(want.as_str())
            } else {
                *value == want.as_str()
            }
        })
    })
}

/// The bounds the modelled `GetParametersByPathMaxResults` carries (range 1..10).
pub const MIN_PARAMETERS_BY_PATH_PAGE_SIZE: i32 = 1;

/// See [`MIN_PARAMETERS_BY_PATH_PAGE_SIZE`]. It is also the page size a request that
/// names no `MaxResults` gets: "If you specify `MaxResults` in the request, the response
/// includes information up to the limit specified" (`GetParametersByPath`'s doc), so a
/// request naming none is served up to the modelled maximum.
pub const MAX_PARAMETERS_BY_PATH_PAGE_SIZE: i32 = 10;

/// The page size a hierarchy read serves, or the refusal message for a `MaxResults`
/// outside the modelled range — which the protocol's boundary validator already
/// refuses, so this is a backstop rather than the first line of defence.
pub fn parameters_by_path_page_size(max_results: Option<i32>) -> Result<usize, String> {
    let requested = max_results.unwrap_or(MAX_PARAMETERS_BY_PATH_PAGE_SIZE);
    if !(MIN_PARAMETERS_BY_PATH_PAGE_SIZE..=MAX_PARAMETERS_BY_PATH_PAGE_SIZE).contains(&requested) {
        return Err(format!(
            "Value '{requested}' at 'maxResults' failed to satisfy constraint: Member must be \
             between {MIN_PARAMETERS_BY_PATH_PAGE_SIZE} and {MAX_PARAMETERS_BY_PATH_PAGE_SIZE}"
        ));
    }
    Ok(requested as usize)
}

/// The hierarchy a read's `Path` names, canonical (no leading and no trailing `/`), or
/// the reason the request is refused.
///
/// "The hierarchy for the parameter. Hierarchies start with a forward slash (/)"
/// (`GetParametersByPath`'s `Path` doc), so a path that carries no leading separator
/// names no hierarchy and is refused. The spaces rule is the name rule
/// ([`trimmed_name`]): the path is a parameter-name prefix.
pub fn parameter_path(path: &str) -> Result<&str, String> {
    let trimmed = trimmed_name(path)?;
    if !trimmed.starts_with('/') {
        return Err(format!(
            "Path '{path}' doesn't start with a forward slash (/): a parameter hierarchy starts \
             with a forward slash (/)"
        ));
    }
    Ok(canonical_name(trimmed).trim_end_matches('/'))
}

/// Whether the parameter stored under canonical `name` lies in the hierarchy under
/// canonical `path` (both as [`canonical_name`] and [`parameter_path`] render them).
///
/// A parameter is under a path when the path is a whole prefix of its name's levels —
/// "the hierarchy is the parameter name except the last part of the parameter" — so the
/// parameter AT the path is not under it and a path is never half a level (`/Dev` does
/// not hold `/DevOps/x`). A non-recursive read reports only the level immediately below
/// the path; a recursive one reports every level under it ("Retrieve all parameters
/// within a hierarchy", the `Recursive` doc).
pub fn is_under_path(name: &str, path: &str, recursive: bool) -> bool {
    let rest = if path.is_empty() {
        name
    } else {
        match name.strip_prefix(path).and_then(|rest| rest.strip_prefix('/')) {
            Some(rest) => rest,
            None => return false,
        }
    };
    !rest.is_empty() && (recursive || !rest.contains('/'))
}

// ---------------------------------------------------------------------------
// The string filters `DescribeInstanceInformation` accepts
// ---------------------------------------------------------------------------
//
// The same managed nodes as the property describe reaches, through the filter vocabulary
// `DescribeInstanceInformation`'s own `Filters` member publishes.

/// The filter keys `DescribeInstanceInformation`'s `Filters` accepts beside the two tag
/// forms: "Valid filter key values: ActivationIds | AgentVersion | AssociationStatus |
/// IamRole | InstanceIds | PingStatus | PlatformTypes | ResourceType | SourceIds |
/// SourceTypes | "tag-key" | "tag:{keyname}"" (the `InstanceInformationStringFilter$Key`
/// doc). It is the property describe's set ([`is_node_filter_key`]) without
/// `DocumentName`, which that operation's filter publishes and this one's does not.
pub const INSTANCE_INFORMATION_FILTER_KEYS: &[&str] = &[
    "ActivationIds",
    "AgentVersion",
    "AssociationStatus",
    "IamRole",
    "InstanceIds",
    "PingStatus",
    "PlatformTypes",
    "ResourceType",
    "SourceIds",
    "SourceTypes",
];

/// Whether `key` names a tag rather than a property of a node ([`TAG_KEY_FILTER_KEY`] or
/// a [`TAG_FILTER_KEY_PREFIX`] key).
pub fn is_tag_filter_key(key: &str) -> bool {
    key == TAG_KEY_FILTER_KEY || key.starts_with(TAG_FILTER_KEY_PREFIX)
}

/// Whether `key` is a filter key `DescribeInstanceInformation`'s `Filters` documents.
///
/// `tag:` must name a tag key after the colon, so `tag:` alone is not one. The
/// comparison is exact, as it is for the property describe: a filter key is a name, so
/// neither a different spelling nor a different case names a property of a node.
pub fn is_instance_information_filter_key(key: &str) -> bool {
    if let Some(tag_key) = key.strip_prefix(TAG_FILTER_KEY_PREFIX) {
        return !tag_key.is_empty();
    }
    key == TAG_KEY_FILTER_KEY || INSTANCE_INFORMATION_FILTER_KEYS.contains(&key)
}

/// Whether `key` is a key the legacy `InstanceInformationFilterList` accepts — the
/// modelled `InstanceInformationFilterKey` enum, which carries no tag form ("The
/// `InstanceInformationFilterList` method is a legacy method and doesn't support tags").
pub fn is_legacy_node_filter_key(key: &str) -> bool {
    INSTANCE_INFORMATION_FILTER_KEY_VALUES.contains(&key)
}

/// Whether `id` is an activation id — the modelled `ActivationId` pattern
/// `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`.
pub fn is_activation_id(id: &str) -> bool {
    let mut groups = id.split('-');
    for width in [8usize, 4, 4, 4, 12] {
        let Some(group) = groups.next() else {
            return false;
        };
        if group.len() != width || !group.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return false;
        }
    }
    groups.next().is_none()
}

/// Whether `version` is an SSM Agent version — the modelled `Version` pattern
/// `^[0-9]{1,6}(\.[0-9]{1,6}){2,3}$`: three or four dot-separated runs of 1..6 digits.
pub fn is_agent_version(version: &str) -> bool {
    let groups: Vec<&str> = version.split('.').collect();
    (3..=4).contains(&groups.len())
        && groups
            .iter()
            .all(|group| (1..=6).contains(&group.len()) && group.bytes().all(|b| b.is_ascii_digit()))
}

/// Why `value` is not a value the node attribute `key` filters could hold, or `None`
/// when it could be one.
///
/// The rule is one sentence: a filter value has to be a legal value of the member it
/// filters, as the model constrains that member — one of its enum's values
/// ([`node_filter_key_value_set`]), or a string matching its pattern. A key whose member
/// the model leaves unconstrained (`IamRole`, `SourceIds`, a tag) constrains nothing
/// here, and a node id is checked by its caller, which answers it with the operation's
/// own `InvalidInstanceId` rather than with a filter-value error.
pub fn node_filter_value_rejection(key: &str, value: &str) -> Option<String> {
    if let Some(catalog) = node_filter_key_value_set(key) {
        if !catalog.contains(&value) {
            return Some(format!(
                "Value '{value}' for filter key '{key}' is not valid: valid values are {}",
                catalog.join(" | ")
            ));
        }
        return None;
    }
    match key {
        "ActivationIds" if !is_activation_id(value) => Some(format!(
            "Value '{value}' for filter key '{key}' is not an activation id"
        )),
        "AgentVersion" if !is_agent_version(value) => Some(format!(
            "Value '{value}' for filter key '{key}' is not an SSM Agent version"
        )),
        _ => None,
    }
}

/// A parameter's `default` as the single string `DefaultValue` reports: the scalar
/// itself, or the JSON rendering of a list or mapping default.
///
/// `budget` is the recursion left, so a deeply nested default cannot drive this into
/// the stack even though [`guard_document_content`] already bounded the nesting; a
/// default that exhausts it is REFUSED rather than reported with the member missing.
fn default_value_string(
    name: &str,
    value: &::yaml_rust2::Yaml,
    budget: usize,
) -> Result<String, ContentRejection> {
    if let Some(scalar) = scalar_string(value) {
        return Ok(scalar);
    }
    let rendered = document_as_json(value, budget).ok_or_else(|| {
        ContentRejection::Content(format!(
            "the default of parameter '{name}' is not a value this service can read: it nests \
             past {MAX_DOCUMENT_DEPTH} levels or is keyed by a non-string"
        ))
    })?;
    Ok(rendered.to_string())
}

/// A document node as the JSON value that renders it.
fn document_as_json(node: &::yaml_rust2::Yaml, budget: usize) -> Option<::serde_json::Value> {
    use ::serde_json::Value;
    use ::yaml_rust2::Yaml;
    let budget = budget.checked_sub(1)?;
    Some(match node {
        Yaml::Null | Yaml::BadValue => Value::Null,
        Yaml::Boolean(flag) => Value::Bool(*flag),
        Yaml::Integer(number) => Value::Number((*number).into()),
        Yaml::Real(real) => real
            .parse::<f64>()
            .ok()
            .and_then(::serde_json::Number::from_f64)
            .map_or_else(|| Value::String(real.clone()), Value::Number),
        Yaml::String(text) => Value::String(text.clone()),
        Yaml::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| document_as_json(item, budget))
                .collect::<Option<Vec<_>>>()?,
        ),
        Yaml::Hash(map) => {
            let mut object = ::serde_json::Map::new();
            for (key, value) in map {
                object.insert(key.as_str()?.to_string(), document_as_json(value, budget)?);
            }
            Value::Object(object)
        }
        // An alias is refused before the load by `reject_document_aliases`, so a
        // resolved alias node can never reach here.
        Yaml::Alias(_) => return None,
    })
}

/// The parameters a document declares: one `DocumentParameter` per `parameters` entry,
/// in the content's own order.
///
/// "A description of the parameters for a document"
/// (`DocumentDescription$Parameters`): each entry's `type`, `description` and
/// `default` are reported as the content declares them — the declared type is reported
/// verbatim rather than narrowed, because an Automation document's parameters carry
/// types (`Integer`, `StringMap`, …) beyond the two a Command document's do.
fn declared_parameters(
    map: &::yaml_rust2::yaml::Hash,
) -> Result<Vec<DocumentParameter>, ContentRejection> {
    let Some(declared) = mapping_get(map, "parameters") else {
        return Ok(Vec::new());
    };
    if matches!(declared, ::yaml_rust2::Yaml::Null) {
        return Ok(Vec::new());
    }
    let Some(declared) = declared.as_hash() else {
        return Err(ContentRejection::Content(
            "the parameters of a document must be a mapping of parameter name to its definition"
                .to_string(),
        ));
    };
    let mut parameters = Vec::with_capacity(declared.len());
    for (name, definition) in declared {
        let Some(name) = name.as_str() else {
            return Err(ContentRejection::Content(
                "every parameter of a document must be named by a string".to_string(),
            ));
        };
        let Some(definition) = definition.as_hash() else {
            return Err(ContentRejection::Content(format!(
                "the definition of parameter '{name}' must be a mapping"
            )));
        };
        let default_value = match mapping_get(definition, "default") {
            None => None,
            Some(default) => Some(default_value_string(name, default, MAX_DOCUMENT_DEPTH)?),
        };
        parameters.push(DocumentParameter {
            name: Some(name.to_string()),
            r#type: mapping_get(definition, "type").and_then(scalar_string),
            description: mapping_get(definition, "description").and_then(scalar_string),
            default_value,
        });
    }
    Ok(parameters)
}

/// The platforms the content restricts the document to.
///
/// "The list of operating system (OS) platforms compatible with this SSM document"
/// (`DocumentDescription$PlatformTypes`). A document declares a platform through a step
/// precondition on `platformType` (`precondition: {StringEquals: [platformType,
/// Linux]}`), so the compatible platforms are the ones its steps name; content that
/// names none restricts nothing and so is compatible with every platform the model
/// lists. Reported in the modelled enum's order, so two reads of one document agree.
fn declared_platform_types(map: &::yaml_rust2::yaml::Hash) -> Vec<String> {
    let mut declared: Vec<&str> = Vec::new();
    if let Some(steps) = mapping_get(map, "mainSteps").and_then(::yaml_rust2::Yaml::as_vec) {
        for step in steps {
            let Some(precondition) = step
                .as_hash()
                .and_then(|step| mapping_get(step, "precondition"))
                .and_then(::yaml_rust2::Yaml::as_hash)
            else {
                continue;
            };
            let Some(equals) =
                mapping_get(precondition, "StringEquals").and_then(::yaml_rust2::Yaml::as_vec)
            else {
                continue;
            };
            if equals.first().and_then(::yaml_rust2::Yaml::as_str) != Some("platformType") {
                continue;
            }
            for value in equals.iter().skip(1) {
                if let Some(platform) = value.as_str() {
                    if PLATFORM_TYPE_VALUES.contains(&platform) && !declared.contains(&platform) {
                        declared.push(platform);
                    }
                }
            }
        }
    }
    PLATFORM_TYPE_VALUES
        .iter()
        .filter(|platform| declared.is_empty() || declared.contains(*platform))
        .map(|platform| (*platform).to_string())
        .collect()
}

/// What `content` declares, or why it is not a document this service accepts.
///
/// "The content for the new SSM document in JSON or YAML format"
/// (`CreateDocument`'s `Content` doc), so the content is parsed in the format the
/// request declared — YAML 1.2 is a superset of JSON, so one parser reads either, and
/// `JSON` is additionally required to BE JSON rather than only YAML that resembles it.
/// `TEXT` content (a Change Calendar entry) is free text with no document tree, so
/// nothing is derived from it and nothing in it is refused.
pub fn parse_document_content(
    content: &str,
    format: &str,
    document_type: &str,
) -> Result<ParsedDocumentContent, ContentRejection> {
    // The documented size bound is measured for content of every format, including the
    // one with no document tree to parse.
    content_within_size(content)?;
    if format == DOCUMENT_FORMAT_TEXT {
        return Ok(ParsedDocumentContent::default());
    }
    guard_document_content(content)?;
    if format == DEFAULT_DOCUMENT_FORMAT
        && ::serde_json::from_str::<::serde_json::Value>(content).is_err()
    {
        return Err(ContentRejection::Content(
            "Content isn't a valid JSON document".to_string(),
        ));
    }
    reject_document_aliases(content)?;
    let mut documents = ::yaml_rust2::YamlLoader::load_from_str(content).map_err(|why| {
        ContentRejection::Content(format!("Content could not be parsed as a document: {why}"))
    })?;
    if documents.len() != 1 {
        return Err(ContentRejection::Content(format!(
            "Content must be exactly one document; found {}",
            documents.len()
        )));
    }
    let document = documents.remove(0);
    let Some(map) = document.as_hash() else {
        return Err(ContentRejection::Content(
            "Content must be a mapping of document keys to values".to_string(),
        ));
    };

    let schema_version = mapping_get(map, "schemaVersion").and_then(scalar_string);
    let supported = supported_schema_versions(document_type);
    if !supported.is_empty() {
        match schema_version.as_deref() {
            None => {
                return Err(ContentRejection::Content(format!(
                    "Content declares no schemaVersion; a {document_type} document must declare \
                     one of {}",
                    supported.join(", ")
                )))
            }
            Some(declared) if !supported.contains(&declared) => {
                return Err(ContentRejection::SchemaVersion(format!(
                    "Document schema version '{declared}' isn't supported for a {document_type} \
                     document; the supported schema versions are {}",
                    supported.join(", ")
                )))
            }
            Some(_) => {}
        }
    }

    Ok(ParsedDocumentContent {
        schema_version,
        description: mapping_get(map, "description").and_then(scalar_string),
        parameters: declared_parameters(map)?,
        platform_types: declared_platform_types(map),
    })
}

/// One stored SSM document: what the create supplied, what the service derived from its
/// content, and the status move the clock still owes.
///
/// A document holds exactly one version here ([`FIRST_DOCUMENT_VERSION`]):
/// `UpdateDocument`, the operation that mints a second one, is still a scaffold, so
/// `LatestVersion` and `DefaultVersion` are that one version.
#[derive(Debug, Clone)]
pub struct DocumentRecord {
    /// The name the create supplied, which is what a read reports and part of the
    /// store key.
    pub name: String,
    /// The Region the document was created in; part of its store key and of its ARN.
    pub region: String,
    pub display_name: Option<String>,
    pub version_name: Option<String>,
    /// A `DocumentType` value.
    pub document_type: String,
    /// A `DocumentFormat` value.
    pub document_format: String,
    pub target_type: Option<String>,
    /// The content exactly as the create supplied it: what [`DocumentRecord::hash`] is
    /// taken over, and what `GetDocument` reports.
    pub content: String,
    /// Hex-rendered Sha256 of [`DocumentRecord::content`].
    pub hash: String,
    /// Everything derived from the content (see [`parse_document_content`]).
    pub parsed: ParsedDocumentContent,
    pub requires: Option<Vec<DocumentRequires>>,
    /// The attachment names the create named, which is what `AttachmentsInformation`
    /// reports.
    pub attachment_names: Vec<String>,
    pub tags: BTreeMap<String, String>,
    pub created_date: SystemTime,
    /// A `DocumentStatus` value.
    pub status: String,
    /// When the clock owes this document's move out of `Creating`, or `None` once that
    /// move has been materialised.
    pub active_due: Option<SystemTime>,
}

impl DocumentRecord {
    /// This document's ARN: `arn:aws:ssm:<region>:<account>:document/<name>`.
    pub fn arn(&self) -> String {
        ::engine::emulator_arn(
            ARN_SERVICE,
            &self.region,
            &format!("{ARN_DOCUMENT_RESOURCE_PREFIX}{}", self.name),
        )
    }

    /// Materialise the status move the clock already owes at `now` — the move to
    /// `Active` that was due whether or not anybody called.
    pub fn materialize_due(&mut self, now: SystemTime) {
        if self.active_due.is_some_and(|due| now >= due) {
            self.active_due = None;
            self.status = DOCUMENT_STATUS_ACTIVE.to_string();
        }
    }

    /// The modelled `DocumentDescription` for this document.
    pub fn to_description(&self) -> DocumentDescription {
        DocumentDescription {
            // "Sha1 hashes have been deprecated", so the system hash is the Sha256 in
            // `Hash` and the deprecated member is left unset.
            sha1: None,
            hash: Some(self.hash.clone()),
            hash_type: Some(DOCUMENT_HASH_TYPE.to_string()),
            name: Some(self.name.clone()),
            display_name: self.display_name.clone(),
            version_name: self.version_name.clone(),
            // "The Amazon Web Services user that created the document" — every
            // document this emulator holds was created in its one account.
            owner: Some(::engine::emulator_account_id().to_string()),
            created_date: Some(::aws_smithy_types::DateTime::from(self.created_date)),
            status: Some(self.status.clone()),
            // "A message returned by Amazon Web Services Systems Manager that explains
            // the Status value", which it sets for a failure; nothing here failed.
            status_information: None,
            document_version: Some(FIRST_DOCUMENT_VERSION.to_string()),
            description: self.parsed.description.clone(),
            parameters: (!self.parsed.parameters.is_empty())
                .then(|| self.parsed.parameters.clone()),
            platform_types: (!self.parsed.platform_types.is_empty())
                .then(|| self.parsed.platform_types.clone()),
            document_type: Some(self.document_type.clone()),
            schema_version: self.parsed.schema_version.clone(),
            latest_version: Some(FIRST_DOCUMENT_VERSION.to_string()),
            default_version: Some(FIRST_DOCUMENT_VERSION.to_string()),
            document_format: Some(self.document_format.clone()),
            target_type: self.target_type.clone(),
            tags: (!self.tags.is_empty()).then(|| {
                self.tags
                    .iter()
                    .map(|(key, value)| Tag {
                        key: key.clone(),
                        value: value.clone(),
                    })
                    .collect()
            }),
            attachments_information: (!self.attachment_names.is_empty()).then(|| {
                self.attachment_names
                    .iter()
                    .map(|name| AttachmentInformation {
                        name: Some(name.clone()),
                    })
                    .collect()
            }),
            requires: self.requires.clone(),
            // "The user in your organization who created the document": the emulator
            // authenticates no human author, so there is none to report.
            author: None,
            // A review is the change-template approval workflow, and `UpdateDocument`
            // metadata — the operation that records one — is still a scaffold, so a
            // change template created here has not been reviewed and no other document
            // type carries a review at all.
            review_information: None,
            approved_version: None,
            pending_review_version: None,
            review_status: (self.document_type == CHANGE_TEMPLATE_DOCUMENT_TYPE)
                .then(|| DOCUMENT_REVIEW_STATUS_NOT_REVIEWED.to_string()),
            // "The classification of a document to help you identify and categorize
            // its use", which Systems Manager assigns to the documents it publishes;
            // nothing classifies a document created through this API.
            category: None,
            category_enum: None,
        }
    }
}

/// The `PingStatus` a node that has just pinged the service carries (from the modelled
/// enum, `crate::ops::types::PING_STATUS_VALUES`).
pub const PING_STATUS_ONLINE: &str = "Online";

/// The `ResourceType` of a node that is an EC2 instance (from the modelled enum,
/// `crate::ops::types::RESOURCE_TYPE_VALUES`).
pub const RESOURCE_TYPE_EC2_INSTANCE: &str = "EC2Instance";

/// The `ResourceType` of a node registered through a hybrid activation (same enum).
pub const RESOURCE_TYPE_MANAGED_INSTANCE: &str = "ManagedInstance";

// ---------------------------------------------------------------------------
// Paging a node report
// ---------------------------------------------------------------------------

/// How many nodes a page holds when the request names no `MaxResults`: "The default
/// value is 10 items" (`DescribeInstanceInformationRequest$MaxResults`).
pub const DEFAULT_NODE_PAGE_SIZE: i32 = 10;

/// The bounds the modelled `MaxResultsEC2Compatible` carries (range 5..50).
pub const MIN_NODE_PAGE_SIZE: i32 = 5;

/// See [`MIN_NODE_PAGE_SIZE`].
pub const MAX_NODE_PAGE_SIZE: i32 = 50;

/// The page size a node report serves, or the refusal message for a `MaxResults` outside
/// the modelled range — which the protocol's boundary validator already refuses, so this
/// is a backstop rather than the first line of defence.
pub fn node_page_size(max_results: Option<i32>) -> Result<usize, String> {
    let requested = max_results.unwrap_or(DEFAULT_NODE_PAGE_SIZE);
    if !(MIN_NODE_PAGE_SIZE..=MAX_NODE_PAGE_SIZE).contains(&requested) {
        return Err(format!(
            "Value '{requested}' at 'maxResults' failed to satisfy constraint: Member must be \
             between {MIN_NODE_PAGE_SIZE} and {MAX_NODE_PAGE_SIZE}"
        ));
    }
    Ok(requested as usize)
}

/// How this service states a `NextToken` it did not issue. `InvalidNextToken`'s own
/// documentation is "The specified token isn't valid".
pub const INVALID_NEXT_TOKEN: &str = "The NextToken value is not valid";

/// Per-service state for com-amazonaws-ssm-amazonssm. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSsmAmazonssmState {
    /// Every parameter, keyed by `(region, canonical name)`: "A parameter name must be
    /// unique within an Amazon Web Services Region". A `BTreeMap` so a listing walks a
    /// stable, total order.
    pub parameters: BTreeMap<(String, String), ParameterRecord>,
    /// Every SSM document, keyed by `(region, name)`: a document name is unique within
    /// a Region, which is what `DocumentAlreadyExists` reports a collision on. A
    /// `BTreeMap` so a listing walks a stable, total order.
    pub documents: BTreeMap<(String, String), DocumentRecord>,

    /// Every managed node, keyed by `(region, node id)`: a node is registered with the
    /// Systems Manager endpoint of one Region and is identified there by its node id. A
    /// `BTreeMap` so a describe walks a stable, total order — ascending by node id
    /// within one Region, whatever order its nodes registered in — and can resume a page
    /// from the id it stopped at rather than from an offset.
    ///
    /// Nothing registers a node yet: registration is the SSM Agent's own call, which
    /// this service's model does not expose, and every operation that would admit one
    /// another way (`CreateActivation`, `CreateAssociation`, `PutInventory`) is still a
    /// scaffold. A describe therefore reports the empty set for a fresh emulator — what
    /// the store holds — rather than nodes nobody registered.
    pub managed_nodes: BTreeMap<(String, String), ManagedNodeRecord>,
}

impl ComAmazonawsSsmAmazonssmState {
    /// Materialise every transition the clock already owes at `now`, so a read reports
    /// what was due whether or not anybody called (see [`DOCUMENT_TRANSITION_DELAY`]).
    pub fn materialize_due(&mut self, now: SystemTime) {
        for document in self.documents.values_mut() {
            document.materialize_due(now);
        }
    }

    /// The document `name` names in `region`, or `None` when that Region holds none.
    pub fn document(&self, region: &str, name: &str) -> Option<&DocumentRecord> {
        self.documents.get(&(region.to_string(), name.to_string()))
    }

    /// Store `record` as `region`'s document of that name.
    pub fn insert_document(&mut self, record: DocumentRecord) {
        let key = (record.region.clone(), record.name.clone());
        self.documents.insert(key, record);
    }

    /// How many documents `region` holds, which is what the documented document quota
    /// is measured against.
    pub fn documents_in_region(&self, region: &str) -> usize {
        self.documents
            .keys()
            .filter(|(held, _)| held == region)
            .count()
    }

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

    /// Every parameter `region` holds, in ascending canonical-name order, as
    /// `(canonical name, record)`. The order is what makes a hierarchy read's resume
    /// cursor total: it is a parameter name, not an offset into a list that may have
    /// changed between two pages.
    pub fn parameters_of_region<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = (&'a str, &'a ParameterRecord)> + 'a {
        self.parameters
            .iter()
            .filter(move |((held, _), _)| held == region)
            .map(|((_, name), record)| (name.as_str(), record))
    }

    /// Every managed node `region` holds, in ascending node-id order, as
    /// `(node id, record)`. The order is what makes a describe's resume cursor total: it
    /// is a node id, not an offset into a list that may have changed.
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

/// One managed node, as the node describes report it.
///
/// `properties` is the modelled `InstanceProperty` itself, so a describe reports what
/// was persisted rather than a second copy of it; `tags` is what the `tag-key` and
/// `tag:<name>` filter keys match against, which no response member carries;
/// `is_latest_version` is the one member `InstanceInformation` reports that
/// `InstanceProperty` has no place for, so the record holds it beside them rather than
/// in a second node store.
#[derive(Debug, Clone, Default)]
pub struct ManagedNodeRecord {
    /// Everything a describe reports about this node.
    pub properties: InstanceProperty,
    /// The node's tags, keyed by tag key.
    pub tags: BTreeMap<String, String>,
    /// Whether the node runs the latest SSM Agent, as the agent last reported
    /// (`InstanceInformation$IsLatestVersion`). `None` for a node that has reported no
    /// agent version, rather than a guess either way.
    pub is_latest_version: Option<bool>,
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

    /// A node registering now: the id it registered under and which kind of node it is
    /// (a `ResourceType` value).
    ///
    /// A node that has just completed registration pinged the service to do it, so it
    /// starts `Online` with that ping and that registration date recorded; every other
    /// property is left for whatever registered it to fill in.
    pub fn registering(
        instance_id: impl Into<String>,
        resource_type: impl Into<String>,
        at: SystemTime,
    ) -> Self {
        let at = ::aws_smithy_types::DateTime::from(at);
        Self {
            properties: InstanceProperty {
                instance_id: Some(instance_id.into()),
                resource_type: Some(resource_type.into()),
                ping_status: Some(PING_STATUS_ONLINE.to_string()),
                last_ping_date_time: Some(at),
                registration_date: Some(at),
                ..InstanceProperty::default()
            },
            tags: BTreeMap::new(),
            is_latest_version: None,
        }
    }

    /// The modelled `InstanceInformation` for this node — the same stored node
    /// `properties` reports, through the members `DescribeInstanceInformation` answers
    /// with.
    ///
    /// Every member is copied from what was persisted; the ones `InstanceProperty` does
    /// not carry (`IsLatestVersion`) come from the record's own field, and the ones
    /// neither carries are left unset rather than invented.
    pub fn to_instance_information(&self) -> InstanceInformation {
        let held = &self.properties;
        InstanceInformation {
            instance_id: held.instance_id.clone(),
            ping_status: held.ping_status.clone(),
            last_ping_date_time: held.last_ping_date_time,
            agent_version: held.agent_version.clone(),
            is_latest_version: self.is_latest_version,
            platform_type: held.platform_type.clone(),
            platform_name: held.platform_name.clone(),
            platform_version: held.platform_version.clone(),
            activation_id: held.activation_id.clone(),
            iam_role: held.iam_role.clone(),
            registration_date: held.registration_date,
            resource_type: held.resource_type.clone(),
            name: held.name.clone(),
            ip_address: held.ip_address.clone(),
            computer_name: held.computer_name.clone(),
            association_status: held.association_status.clone(),
            last_association_execution_date: held.last_association_execution_date,
            last_successful_association_execution_date: held
                .last_successful_association_execution_date,
            association_overview: held.association_overview.clone(),
            source_id: held.source_id.clone(),
            source_type: held.source_type.clone(),
            source_location: held.source_location.clone(),
        }
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
