// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-controlcatalog-controlcatalog service.
//!
//! ControlCatalog is a read-only catalog: every one of its six operations reports
//! what Amazon Web Services publishes, and none of them creates, updates or deletes
//! anything. The catalog therefore lives in this service's state, seeded once (the
//! engine builds the state through `Default` on first touch, atomically) with a
//! deterministic set of domains, objectives, common controls, controls and control
//! mappings. Every handler projects its response out of these records, so two
//! processes talking to one emulator — and two runs of the same emulator — see the
//! same catalog.
//!
//! The collections are `BTreeMap`s keyed by ARN: that key order is the stable total
//! order the paginated list operations walk, and it is what a `NextToken` resumes
//! after (see [`page`]).

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

/// A catalog domain — the top of the catalog hierarchy.
#[derive(Debug, Clone)]
pub struct Domain {
    pub arn: String,
    pub name: String,
    pub description: String,
    /// Seconds since the Unix epoch; rendered as the modeled `Timestamp`.
    pub create_time: i64,
    pub last_update_time: i64,
}

/// A catalog objective, which always belongs to exactly one [`Domain`].
#[derive(Debug, Clone)]
pub struct Objective {
    pub arn: String,
    pub name: String,
    pub description: String,
    pub domain_arn: String,
    pub create_time: i64,
    pub last_update_time: i64,
}

/// A common control, which belongs to one [`Objective`] (and so to that
/// objective's domain).
#[derive(Debug, Clone)]
pub struct CommonControl {
    pub arn: String,
    pub name: String,
    pub description: String,
    pub objective_arn: String,
    pub domain_arn: String,
    pub create_time: i64,
    pub last_update_time: i64,
}

/// Where a control can be deployed: the modeled `RegionConfiguration`.
#[derive(Debug, Clone)]
pub struct RegionConfig {
    /// `ControlScope`: `GLOBAL` or `REGIONAL`.
    pub scope: String,
    /// Regions the control can be deployed in. Empty for a `GLOBAL` control: the
    /// GetControl documentation says a control whose `Scope` is `GLOBAL` does not
    /// show `DeployableRegions`, because the field does not apply.
    pub deployable_regions: Vec<String>,
}

/// How a control is implemented: the modeled `ImplementationDetails` /
/// `ImplementationSummary` (the two shapes carry the same two members).
#[derive(Debug, Clone)]
pub struct Implementation {
    /// `ImplementationType`, e.g. `AWS::Config::ConfigRule`.
    pub implementation_type: String,
    /// `ImplementationIdentifier`, the implementing service's own id for the control.
    pub identifier: Option<String>,
}

/// One parameter a control accepts: the modeled `ControlParameter`.
#[derive(Debug, Clone)]
pub struct ControlParam {
    pub name: String,
    /// `ControlParameterRequirement`: `REQUIRED` or `OPTIONAL`.
    pub requirement: Option<String>,
}

/// A catalog control, as both GetControl and ListControls report it — the list
/// summary is a projection of this same record.
#[derive(Debug, Clone)]
pub struct Control {
    /// The `controlcatalog` ARN. GetControl always answers with this form, whichever
    /// accepted form the caller asked with.
    pub arn: String,
    /// The Control Tower control identifier (the segment after `control/` in an
    /// `arn:aws:controltower:{REGION}::control/{ID}` ARN), when this control is also
    /// a Control Tower control. GetControl accepts that ARN form as input.
    pub control_tower_identifier: Option<String>,
    pub aliases: Vec<String>,
    pub name: String,
    pub description: String,
    /// `ControlBehavior`: `PREVENTIVE`, `PROACTIVE` or `DETECTIVE`.
    pub behavior: String,
    /// `ControlSeverity`: `LOW`, `MEDIUM`, `HIGH` or `CRITICAL`.
    pub severity: Option<String>,
    pub region_configuration: RegionConfig,
    pub implementation: Option<Implementation>,
    /// `ParameterRequirementSummary`: `REQUIRED`, `OPTIONAL` or `NONE`.
    pub parameter_requirement_summary: Option<String>,
    pub parameters: Vec<ControlParam>,
    pub create_time: i64,
    pub governed_resources: Vec<String>,
    pub governed_providers: Vec<String>,
}

/// The target of a control mapping: the modeled `Mapping` union.
#[derive(Debug, Clone)]
pub enum MappingDetails {
    Framework { name: String, item: String },
    CommonControl { common_control_arn: String },
    RelatedControl {
        control_arn: Option<String>,
        /// `ControlRelationType`: `COMPLEMENTARY`, `ALTERNATIVE` or `MUTUALLY_EXCLUSIVE`.
        relation_type: String,
    },
}

/// A control mapping: a control, the kind of mapping, and what it maps to.
#[derive(Debug, Clone)]
pub struct ControlMappingRecord {
    pub control_arn: String,
    /// `MappingType`: `FRAMEWORK`, `COMMON_CONTROL` or `RELATED_CONTROL`.
    pub mapping_type: String,
    pub mapping: MappingDetails,
}

/// Per-service state for com-amazonaws-controlcatalog-controlcatalog. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsControlcatalogControlcatalogState {
    /// Keyed by domain ARN.
    pub domains: BTreeMap<String, Domain>,
    /// Keyed by objective ARN.
    pub objectives: BTreeMap<String, Objective>,
    /// Keyed by common control ARN.
    pub common_controls: BTreeMap<String, CommonControl>,
    /// Keyed by the control's `controlcatalog` ARN.
    pub controls: BTreeMap<String, Control>,
    /// Keyed by a synthesized mapping key (`<control arn>|<mapping type>|<target>`),
    /// because a mapping has no ARN of its own yet still needs the stable total
    /// order ListControlMappings' `NextToken` resumes in.
    pub control_mappings: BTreeMap<String, ControlMappingRecord>,
}

impl Default for ComAmazonawsControlcatalogControlcatalogState {
    fn default() -> Self {
        seed_catalog()
    }
}

impl ComAmazonawsControlcatalogControlcatalogState {
    /// Resolve an accepted control ARN to the stored control.
    ///
    /// GetControl's documentation: it "accepts controltower or controlcatalog control
    /// ARNs as input" and "returns a controlcatalog ARN format", and gives the two
    /// accepted forms as `arn:{PARTITION}:controlcatalog:::control/
    /// {CONTROL_CATALOG_OPAQUE_ID}` and `arn:{PARTITION}:controltower:{REGION}::control/
    /// {CONTROL_TOWER_OPAQUE_ID}`. What names the control is therefore the identifier
    /// after `control/`, read in the namespace the service segment selects; `{PARTITION}`
    /// says which partition the caller writes the ARN in and `{REGION}` where the control
    /// was enabled, so neither segment is part of a control's identity. An ARN of either
    /// form whose identifier the catalog publishes resolves, whatever partition it is
    /// written in — the catalog is global, and publishes one control per identifier.
    pub fn resolve_control(&self, requested_arn: &str) -> Option<&Control> {
        match ControlArnRef::parse(requested_arn)? {
            ControlArnRef::Catalog(identifier) => {
                self.controls.get(&catalog_arn("control", identifier))
            }
            ControlArnRef::ControlTower(identifier) => self.controls.values().find(|control| {
                control
                    .control_tower_identifier
                    .as_deref()
                    .is_some_and(|stored| stored == identifier)
            }),
        }
    }
}

/// What an accepted control ARN names: the identifier it carries, in the namespace its
/// service segment selects. Any other ARN names no control at all.
#[derive(Debug)]
enum ControlArnRef<'a> {
    /// A `controlcatalog` ARN's `{CONTROL_CATALOG_OPAQUE_ID}` — the opaque identifier
    /// this catalog publishes a control under.
    Catalog(&'a str),
    /// A `controltower` ARN's `{CONTROL_TOWER_OPAQUE_ID}` — the Control Tower identifier
    /// of a control that is also a Control Tower control.
    ControlTower(&'a str),
}

impl<'a> ControlArnRef<'a> {
    /// Read an ARN as the control it names, or `None` when it names none.
    fn parse(arn: &'a str) -> Option<Self> {
        let mut fields = arn.splitn(6, ':');
        let (
            Some("arn"),
            Some(partition),
            Some(service),
            Some(region),
            // Neither accepted form carries an account id.
            Some(""),
            Some(resource),
        ) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        )
        else {
            return None;
        };
        // The modeled `ControlArn` pattern admits the `aws` partition family only:
        // ^arn:(aws(?:[-a-z]*)?):(controlcatalog|controltower):[a-zA-Z0-9-]*::control/…
        if !partition.starts_with("aws") {
            return None;
        }
        let identifier = resource.strip_prefix("control/")?;
        if identifier.is_empty() {
            return None;
        }
        match service {
            // The global catalog form carries no Region segment.
            "controlcatalog" if region.is_empty() => Some(Self::Catalog(identifier)),
            "controltower" => Some(Self::ControlTower(identifier)),
            _ => None,
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsControlcatalogControlcatalogState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsControlcatalogControlcatalogState>)
}

// ---------------------------------------------------------------------------
// ARNs and identifiers
// ---------------------------------------------------------------------------

/// Build a ControlCatalog ARN for `kind`/`id`.
///
/// ControlCatalog ARNs carry neither a Region nor an account id — the modeled
/// `DomainArn` pattern is `^arn:(aws(?:[-a-z]*)?):controlcatalog:::domain/[0-9a-z]+$`,
/// and `ControlArn`, `ObjectiveArn` and `CommonControlArn` have the same empty
/// Region and account segments — so `engine::emulator_arn`, which always writes the
/// emulator's account id into the ARN, cannot produce one. The format below is the
/// model's own.
fn catalog_arn(kind: &str, id: &str) -> String {
    format!("arn:aws:controlcatalog:::{kind}/{id}")
}

/// Length of the opaque identifier at the end of a catalog ARN. The modeled ARN
/// patterns admit `[0-9a-z]+`; AWS's published catalog ARNs (e.g.
/// `arn:aws:controlcatalog:::control/5kvme4m5d2b4d7if2fs5yg2ui`, quoted in the
/// `ControlParameter` documentation) use 25 characters.
const CATALOG_ID_LEN: usize = 25;

/// A deterministic opaque catalog identifier for `slug`.
///
/// Derived by hashing, so the identifier — and therefore every ARN in the catalog —
/// is identical in every process and on every run, which is what lets a client hold
/// an ARN across sessions. Hex digits are a subset of the `[0-9a-z]` the ARN
/// patterns allow.
fn catalog_id(slug: &str) -> String {
    let digest = Sha256::digest(slug.as_bytes());
    hex::encode(digest)[..CATALOG_ID_LEN].to_string()
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// One page of a list operation's results.
#[derive(Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// Set only when items remain after this page.
    pub next_token: Option<String>,
}

/// Why a page could not be taken. Both variants are client-input failures a handler
/// answers with the service's modeled `ValidationException`.
#[derive(Debug)]
pub enum PageError {
    /// A `NextToken` this collection and filter cannot resume from.
    InvalidToken,
    /// A `MaxResults` outside the operation's modeled range. The modeled `@range`
    /// refuses such a value before a handler runs, so it is unreachable from the
    /// wire; refusing it here too means an in-process caller gets the same answer as
    /// a client, in every build profile, rather than a page quietly sized differently
    /// from the one it asked for.
    MaxResultsOutOfRange { requested: i32, modeled_max: i32 },
}

/// A short, stable fingerprint of the filter a page was produced under.
///
/// A `NextToken` is only meaningful for the collection and filter that produced it:
/// replaying it against a different filter would resume in a sequence the token's key
/// may not even belong to. The fingerprint travels inside the token so such a replay
/// is refused instead of silently answering from the wrong sequence.
///
/// Every list operation builds it the same way — one `(dimension, values)` pair per
/// dimension its filter declares, in the order the model declares them, with the
/// values the caller set (none for a dimension the caller omitted) — so the input to
/// the hash is canonical across the service rather than hand-rolled per operation.
pub fn filter_fingerprint(dimensions: &[(&str, Vec<String>)]) -> String {
    let canonical = dimensions
        .iter()
        .map(|(dimension, values)| format!("{dimension}=[{}]", values.join(",")))
        .collect::<Vec<_>>()
        .join(";");
    let digest = Sha256::digest(canonical.as_bytes());
    hex::encode(digest)[..16].to_string()
}

/// Field separator inside a decoded token. A unit separator cannot appear in an ARN,
/// a collection name or a fingerprint, so the three fields always split cleanly.
const TOKEN_SEPARATOR: char = '\u{1f}';

/// Encode an opaque `NextToken` for the given collection, filter and last-returned key.
fn encode_page_token(collection: &str, fingerprint: &str, last_key: &str) -> String {
    hex::encode(format!(
        "{collection}{TOKEN_SEPARATOR}{fingerprint}{TOKEN_SEPARATOR}{last_key}"
    ))
}

/// Decode a `NextToken` back to the key it resumes after, refusing one that was not
/// issued for this collection and filter.
fn decode_page_token(
    token: &str,
    collection: &str,
    fingerprint: &str,
) -> Result<String, PageError> {
    let decoded = hex::decode(token).map_err(|_| PageError::InvalidToken)?;
    let decoded = String::from_utf8(decoded).map_err(|_| PageError::InvalidToken)?;
    let mut fields = decoded.split(TOKEN_SEPARATOR);
    let (Some(token_collection), Some(token_fingerprint), Some(last_key), None) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return Err(PageError::InvalidToken);
    };
    if token_collection != collection || token_fingerprint != fingerprint {
        return Err(PageError::InvalidToken);
    }
    Ok(last_key.to_string())
}

/// How a list operation words a [`PageError`] in the service's modeled
/// `ValidationException`: each message names the input member at fault, the way the
/// boundary validator's own refusals do, and is built the same way for all five list
/// operations.
pub fn page_refusal(operation: &str, error: PageError) -> String {
    match error {
        PageError::InvalidToken => format!(
            "nextToken is not a pagination token issued by a previous {operation} call \
             with the same parameters"
        ),
        PageError::MaxResultsOutOfRange {
            requested,
            modeled_max,
        } => format!("maxResults must be between 1 and {modeled_max}; {requested} was supplied"),
    }
}

/// Take one page out of an ordered `(key, item)` sequence.
///
/// `max_results` is the caller's page size, refused when it falls outside the
/// operation's own modeled `MaxResults` range; with none sent, a page holds up to
/// `modeled_max`, the maximum that range declares. A `NextToken` is returned only when
/// items remain, and resuming with it starts at the item after the last one returned.
pub fn page<T>(
    collection: &str,
    fingerprint: &str,
    ordered: impl IntoIterator<Item = (String, T)>,
    next_token: Option<&str>,
    max_results: Option<i32>,
    modeled_max: i32,
) -> Result<Page<T>, PageError> {
    if let Some(requested) = max_results {
        if !(1..=modeled_max).contains(&requested) {
            return Err(PageError::MaxResultsOutOfRange {
                requested,
                modeled_max,
            });
        }
    }
    let page_size = max_results.unwrap_or(modeled_max) as usize;
    let resume_after = match next_token {
        Some(token) => Some(decode_page_token(token, collection, fingerprint)?),
        None => None,
    };

    let mut skipping = resume_after.is_some();
    let mut items = Vec::new();
    let mut last_key = String::new();
    let mut more_remain = false;

    for (key, item) in ordered {
        if skipping {
            if resume_after.as_deref() == Some(key.as_str()) {
                skipping = false;
            }
            continue;
        }
        if items.len() == page_size {
            more_remain = true;
            break;
        }
        last_key = key;
        items.push(item);
    }

    if skipping {
        // The key the token names is not in this sequence at all, so there is no
        // position to resume from.
        return Err(PageError::InvalidToken);
    }

    let next_token = more_remain.then(|| encode_page_token(collection, fingerprint, &last_key));
    Ok(Page { items, next_token })
}

// ---------------------------------------------------------------------------
// The seeded catalog
// ---------------------------------------------------------------------------

/// Base instant for the seeded catalog's timestamps: 2023-11-27T00:00:00Z, expressed
/// in seconds since the Unix epoch. Fixed, so timestamps are reproducible across runs.
const CATALOG_EPOCH: i64 = 1_701_043_200;

/// One day in seconds, the step between seeded catalog timestamps.
const DAY: i64 = 86_400;

// The compliance frameworks this catalog's FRAMEWORK mappings name, each written as
// the framework's own published title. The modeled `FrameworkName` admits 3..250
// characters, so the published title is what goes on the wire; the item beside it in
// a mapping row is that framework's own identifier for the requirement.
const NIST_SP_800_53_R5: &str = "NIST Special Publication 800-53 Revision 5";
const NIST_CSF_V1_1: &str = "NIST Cybersecurity Framework Version 1.1";
const PCI_DSS_V4_0: &str = "Payment Card Industry Data Security Standard v4.0";
const CIS_AWS_FOUNDATIONS_V3_0_0: &str = "CIS Amazon Web Services Foundations Benchmark v3.0.0";
const CIS_CONTROLS_V8_0: &str = "CIS Critical Security Controls Version 8.0";
const CIS_GCP_FOUNDATION_V2_0_0: &str = "CIS Google Cloud Platform Foundation Benchmark v2.0.0";

/// Build the catalog this emulator serves.
///
/// Small but complete: every objective names a domain that is in `domains`, every
/// common control names an objective and a domain that are in `objectives` /
/// `domains`, and every mapping names a control (and, for a common-control or
/// related-control mapping, a common control or control) that is in `controls` /
/// `common_controls`. Each list/get operation therefore returns entries the others
/// can resolve.
fn seed_catalog() -> ComAmazonawsControlcatalogControlcatalogState {
    let mut domains = BTreeMap::new();
    let mut objectives = BTreeMap::new();
    let mut common_controls = BTreeMap::new();
    let mut controls = BTreeMap::new();
    let mut control_mappings = BTreeMap::new();

    // --- domains -----------------------------------------------------------
    let domain_seed: [(&str, &str, &str); 3] = [
        (
            "data-protection",
            "Data protection",
            "Controls that protect the confidentiality and integrity of data at rest and in transit.",
        ),
        (
            "identity-and-access-management",
            "Identity and access management",
            "Controls that govern who can reach Amazon Web Services resources, and what they may do with them.",
        ),
        (
            "logging-and-monitoring",
            "Logging and monitoring",
            "Controls that record and observe activity across Amazon Web Services accounts.",
        ),
    ];
    for (index, (slug, name, description)) in domain_seed.iter().enumerate() {
        let arn = catalog_arn("domain", &catalog_id(slug));
        let create_time = CATALOG_EPOCH + (index as i64) * DAY;
        domains.insert(
            arn.clone(),
            Domain {
                arn,
                name: (*name).to_string(),
                description: (*description).to_string(),
                create_time,
                last_update_time: create_time + DAY,
            },
        );
    }
    let domain_arn = |slug: &str| catalog_arn("domain", &catalog_id(slug));

    // --- objectives --------------------------------------------------------
    let objective_seed: [(&str, &str, &str, &str); 5] = [
        (
            "encryption-of-data-at-rest",
            "Encryption of data at rest",
            "Encrypt stored data so that it cannot be read without the corresponding key.",
            "data-protection",
        ),
        (
            "encryption-of-data-in-transit",
            "Encryption of data in transit",
            "Protect data while it moves between clients, services and Regions.",
            "data-protection",
        ),
        (
            "manage-access",
            "Manage access",
            "Grant and review the access that identities hold over resources.",
            "identity-and-access-management",
        ),
        (
            "limit-exposure",
            "Limit exposure",
            "Keep resources from being reachable more widely than intended.",
            "identity-and-access-management",
        ),
        (
            "manage-logs",
            "Manage logs",
            "Capture, retain and protect records of activity.",
            "logging-and-monitoring",
        ),
    ];
    for (index, (slug, name, description, domain_slug)) in objective_seed.iter().enumerate() {
        let arn = catalog_arn("objective", &catalog_id(slug));
        let create_time = CATALOG_EPOCH + (index as i64) * DAY;
        objectives.insert(
            arn.clone(),
            Objective {
                arn,
                name: (*name).to_string(),
                description: (*description).to_string(),
                domain_arn: domain_arn(domain_slug),
                create_time,
                last_update_time: create_time + DAY,
            },
        );
    }
    let objective_arn = |slug: &str| catalog_arn("objective", &catalog_id(slug));

    // --- common controls ---------------------------------------------------
    let common_control_seed: [(&str, &str, &str, &str, &str); 6] = [
        (
            "encrypt-data-at-rest-in-storage-services",
            "Encrypt data at rest in storage services",
            "Require server-side encryption for data held by storage services.",
            "encryption-of-data-at-rest",
            "data-protection",
        ),
        (
            "rotate-encryption-keys",
            "Rotate encryption keys",
            "Replace encryption key material on a defined schedule.",
            "encryption-of-data-at-rest",
            "data-protection",
        ),
        (
            "enforce-encryption-in-transit",
            "Enforce encryption in transit",
            "Require TLS for connections that carry data between endpoints.",
            "encryption-of-data-in-transit",
            "data-protection",
        ),
        (
            "require-multi-factor-authentication",
            "Require multi-factor authentication",
            "Require a second authentication factor for human access.",
            "manage-access",
            "identity-and-access-management",
        ),
        (
            "restrict-public-access",
            "Restrict public access",
            "Keep resources from being made reachable from the public internet.",
            "limit-exposure",
            "identity-and-access-management",
        ),
        (
            "enable-centralized-logging",
            "Enable centralized logging",
            "Record activity to a central, tamper-resistant log store.",
            "manage-logs",
            "logging-and-monitoring",
        ),
    ];
    for (index, (slug, name, description, objective_slug, domain_slug)) in
        common_control_seed.iter().enumerate()
    {
        let arn = catalog_arn("common-control", &catalog_id(slug));
        let create_time = CATALOG_EPOCH + (index as i64) * DAY;
        common_controls.insert(
            arn.clone(),
            CommonControl {
                arn,
                name: (*name).to_string(),
                description: (*description).to_string(),
                objective_arn: objective_arn(objective_slug),
                domain_arn: domain_arn(domain_slug),
                create_time,
                last_update_time: create_time + DAY,
            },
        );
    }
    let common_control_arn = |slug: &str| catalog_arn("common-control", &catalog_id(slug));

    // --- controls ----------------------------------------------------------
    // Regions a REGIONAL control in this catalog can be deployed in.
    let deployable = |regions: &[&str]| regions.iter().map(|r| (*r).to_string()).collect::<Vec<_>>();
    let control_seed: Vec<Control> = vec![
        Control {
            arn: catalog_arn("control", &catalog_id("s3-bucket-server-side-encryption-enabled")),
            control_tower_identifier: None,
            aliases: vec!["SH.S3.4".to_string()],
            name: "Require server-side encryption on Amazon S3 buckets".to_string(),
            description: "Checks whether an Amazon S3 bucket has a default server-side encryption configuration."
                .to_string(),
            behavior: "DETECTIVE".to_string(),
            severity: Some("MEDIUM".to_string()),
            region_configuration: RegionConfig {
                scope: "REGIONAL".to_string(),
                deployable_regions: deployable(&["us-east-1", "us-west-2", "eu-west-1"]),
            },
            implementation: Some(Implementation {
                implementation_type: "AWS::Config::ConfigRule".to_string(),
                identifier: Some("S3_BUCKET_SERVER_SIDE_ENCRYPTION_ENABLED".to_string()),
            }),
            parameter_requirement_summary: Some("NONE".to_string()),
            parameters: Vec::new(),
            create_time: CATALOG_EPOCH,
            governed_resources: vec!["AWS::S3::Bucket".to_string()],
            governed_providers: vec!["AWS".to_string()],
        },
        Control {
            arn: catalog_arn("control", &catalog_id("restrict-root-user-access-keys")),
            control_tower_identifier: Some("AWS-GR_RESTRICT_ROOT_USER_ACCESS_KEYS".to_string()),
            aliases: vec!["CT.IAM.PV.1".to_string(), "SH.IAM.4".to_string()],
            name: "Disallow the creation of access keys for the root user".to_string(),
            description: "Prevents the creation of access keys for the Amazon Web Services account root user."
                .to_string(),
            behavior: "PREVENTIVE".to_string(),
            severity: Some("CRITICAL".to_string()),
            region_configuration: RegionConfig {
                // A control implemented by a service control policy governs every
                // Region, so its scope is GLOBAL and it reports no deployable Regions.
                scope: "GLOBAL".to_string(),
                deployable_regions: Vec::new(),
            },
            implementation: Some(Implementation {
                implementation_type: "AWS::Organizations::Policy::SERVICE_CONTROL_POLICY"
                    .to_string(),
                identifier: None,
            }),
            parameter_requirement_summary: Some("OPTIONAL".to_string()),
            parameters: vec![ControlParam {
                name: "ExemptedPrincipalArns".to_string(),
                requirement: Some("OPTIONAL".to_string()),
            }],
            create_time: CATALOG_EPOCH + DAY,
            governed_resources: vec!["AWS::IAM::AccessKey".to_string()],
            governed_providers: vec!["AWS".to_string()],
        },
        Control {
            arn: catalog_arn("control", &catalog_id("cloud-trail-multi-region-enabled")),
            control_tower_identifier: Some("AWS-GR_CLOUDTRAIL_ENABLED".to_string()),
            aliases: vec!["SH.CloudTrail.1".to_string()],
            name: "Require a multi-Region CloudTrail trail".to_string(),
            description: "Checks whether an account records management events in all Regions through a CloudTrail trail."
                .to_string(),
            behavior: "DETECTIVE".to_string(),
            severity: Some("HIGH".to_string()),
            region_configuration: RegionConfig {
                scope: "REGIONAL".to_string(),
                deployable_regions: deployable(&[
                    "ap-southeast-2",
                    "eu-west-1",
                    "us-east-1",
                    "us-west-2",
                ]),
            },
            implementation: Some(Implementation {
                implementation_type: "AWS::Config::ConfigRule".to_string(),
                identifier: Some("MULTI_REGION_CLOUD_TRAIL_ENABLED".to_string()),
            }),
            parameter_requirement_summary: Some("NONE".to_string()),
            parameters: Vec::new(),
            create_time: CATALOG_EPOCH + 2 * DAY,
            governed_resources: vec!["AWS::CloudTrail::Trail".to_string()],
            governed_providers: vec!["AWS".to_string()],
        },
        Control {
            arn: catalog_arn("control", &catalog_id("require-encrypted-ebs-volumes")),
            control_tower_identifier: None,
            aliases: Vec::new(),
            name: "Require encryption on Amazon EBS volumes at creation".to_string(),
            description: "Rejects a CloudFormation deployment that would create an unencrypted Amazon EBS volume."
                .to_string(),
            behavior: "PROACTIVE".to_string(),
            severity: Some("MEDIUM".to_string()),
            region_configuration: RegionConfig {
                scope: "REGIONAL".to_string(),
                deployable_regions: deployable(&["eu-central-1", "us-east-1"]),
            },
            implementation: Some(Implementation {
                implementation_type: "AWS::CloudFormation::Type::HOOK".to_string(),
                identifier: Some("AWS.EBS.EncryptionHook".to_string()),
            }),
            parameter_requirement_summary: Some("REQUIRED".to_string()),
            parameters: vec![
                ControlParam {
                    name: "AllowedRegions".to_string(),
                    requirement: Some("REQUIRED".to_string()),
                },
                ControlParam {
                    name: "ExemptedResourceArns".to_string(),
                    requirement: Some("OPTIONAL".to_string()),
                },
            ],
            create_time: CATALOG_EPOCH + 3 * DAY,
            governed_resources: vec![
                "AWS::EC2::Volume".to_string(),
                "AWS::EC2::Instance".to_string(),
            ],
            governed_providers: vec!["AWS".to_string()],
        },
        Control {
            arn: catalog_arn("control", &catalog_id("deny-public-rds-snapshots")),
            control_tower_identifier: Some("AWS-GR_RDS_SNAPSHOT_PUBLIC_PROHIBITED".to_string()),
            aliases: vec!["CT.RDS.PV.4".to_string()],
            name: "Disallow public Amazon RDS database snapshots".to_string(),
            description: "Prevents an Amazon RDS database snapshot from being shared publicly."
                .to_string(),
            behavior: "PREVENTIVE".to_string(),
            severity: Some("HIGH".to_string()),
            region_configuration: RegionConfig {
                scope: "GLOBAL".to_string(),
                deployable_regions: Vec::new(),
            },
            implementation: Some(Implementation {
                implementation_type: "AWS::Organizations::Policy::SERVICE_CONTROL_POLICY"
                    .to_string(),
                identifier: None,
            }),
            parameter_requirement_summary: Some("NONE".to_string()),
            parameters: Vec::new(),
            create_time: CATALOG_EPOCH + 4 * DAY,
            governed_resources: vec!["AWS::RDS::DBSnapshot".to_string()],
            governed_providers: vec!["AWS".to_string()],
        },
        Control {
            arn: catalog_arn("control", &catalog_id("require-mfa-for-console-access")),
            control_tower_identifier: None,
            aliases: vec!["SH.IAM.5".to_string()],
            name: "Require multi-factor authentication for console users".to_string(),
            description: "Checks whether every identity with a console password also has multi-factor authentication enabled."
                .to_string(),
            behavior: "DETECTIVE".to_string(),
            severity: Some("LOW".to_string()),
            region_configuration: RegionConfig {
                scope: "REGIONAL".to_string(),
                deployable_regions: deployable(&["us-east-1"]),
            },
            implementation: Some(Implementation {
                implementation_type: "AWS::SecurityHub::SecurityControl".to_string(),
                identifier: Some("IAM.5".to_string()),
            }),
            parameter_requirement_summary: Some("NONE".to_string()),
            parameters: Vec::new(),
            create_time: CATALOG_EPOCH + 5 * DAY,
            governed_resources: vec!["AWS::IAM::User".to_string()],
            governed_providers: vec!["AWS".to_string()],
        },
        Control {
            arn: catalog_arn("control", &catalog_id("gcp-storage-bucket-uniform-access")),
            control_tower_identifier: None,
            aliases: Vec::new(),
            name: "Require uniform bucket-level access on Cloud Storage buckets".to_string(),
            description: "Checks whether a Google Cloud Storage bucket enforces uniform bucket-level access."
                .to_string(),
            behavior: "DETECTIVE".to_string(),
            severity: Some("MEDIUM".to_string()),
            region_configuration: RegionConfig {
                scope: "REGIONAL".to_string(),
                deployable_regions: deployable(&["us-east-1"]),
            },
            implementation: Some(Implementation {
                implementation_type: "AWS::SecurityHub::SecurityControl".to_string(),
                identifier: Some("GCP.Storage.1".to_string()),
            }),
            parameter_requirement_summary: Some("NONE".to_string()),
            parameters: Vec::new(),
            create_time: CATALOG_EPOCH + 6 * DAY,
            governed_resources: vec!["storage.googleapis.com/Bucket".to_string()],
            governed_providers: vec!["GCP".to_string()],
        },
    ];
    for control in control_seed {
        controls.insert(control.arn.clone(), control);
    }
    let control_arn = |slug: &str| catalog_arn("control", &catalog_id(slug));

    // --- control mappings --------------------------------------------------
    // The catalog records, for every control it publishes, that control's WHOLE
    // mapping set, which is the only wire source for "what does this control
    // satisfy, implement and relate to":
    //
    //  * COMMON_CONTROL — the common control the control implements;
    //  * FRAMEWORK — one entry per published compliance-framework item the control
    //    satisfies. A control normally satisfies items in SEVERAL frameworks at
    //    once, so one entry per control is not the catalog's shape;
    //  * RELATED_CONTROL — one entry per control it is published alongside, with
    //    how the two relate: COMPLEMENTARY where both can be enabled and reinforce
    //    one another, ALTERNATIVE where either one satisfies the common control the
    //    pair implements, MUTUALLY_EXCLUSIVE where enabling both would conflict
    //    (this catalog publishes no pair that does, so it records none).
    //
    // Each framework row names a published framework and that framework's own
    // identifier for the requirement the control satisfies; the identifier is
    // quoted in the comment beside the row. Relations are recorded in both
    // directions, because a query filters on one control at a time and each side of
    // a pair has to report the relation when it is the one queried.
    let framework = |name: &str, item: &str| MappingDetails::Framework {
        name: name.to_string(),
        item: item.to_string(),
    };
    let implements = |common_control_slug: &str| MappingDetails::CommonControl {
        common_control_arn: common_control_arn(common_control_slug),
    };
    let related = |control_slug: &str, relation_type: &str| MappingDetails::RelatedControl {
        control_arn: Some(control_arn(control_slug)),
        relation_type: relation_type.to_string(),
    };
    let mapping_seed: Vec<(&str, Vec<MappingDetails>)> = vec![
        (
            "s3-bucket-server-side-encryption-enabled",
            vec![
                implements("encrypt-data-at-rest-in-storage-services"),
                // "Protection of information at rest".
                framework(NIST_SP_800_53_R5, "SC-28"),
                // "Data-at-rest is protected".
                framework(NIST_CSF_V1_1, "PR.DS-1"),
                // "Primary account number is rendered unreadable wherever it is stored".
                framework(PCI_DSS_V4_0, "3.5.1"),
                // "Encrypt sensitive data at rest".
                framework(CIS_CONTROLS_V8_0, "3.11"),
                related("require-encrypted-ebs-volumes", "COMPLEMENTARY"),
            ],
        ),
        (
            "restrict-root-user-access-keys",
            vec![
                implements("require-multi-factor-authentication"),
                // "Ensure no 'root' user account access key exists".
                framework(CIS_AWS_FOUNDATIONS_V3_0_0, "1.4"),
                // "Least privilege".
                framework(NIST_SP_800_53_R5, "AC-6"),
                // "Identities and credentials are issued, managed and revoked".
                framework(NIST_CSF_V1_1, "PR.AC-1"),
                // "Group, shared and generic accounts are managed".
                framework(PCI_DSS_V4_0, "8.2.2"),
                // Both controls implement the multi-factor-authentication common
                // control for the same provider through different implementation
                // types (an Organizations policy and a Security Hub control), so
                // either one satisfies it: the catalog records them as ALTERNATIVE.
                related("require-mfa-for-console-access", "ALTERNATIVE"),
            ],
        ),
        (
            "cloud-trail-multi-region-enabled",
            vec![
                implements("enable-centralized-logging"),
                // "Event logging".
                framework(NIST_SP_800_53_R5, "AU-2"),
                // "Event data are collected and correlated".
                framework(NIST_CSF_V1_1, "DE.AE-3"),
                // "Ensure CloudTrail is enabled in all Regions".
                framework(CIS_AWS_FOUNDATIONS_V3_0_0, "3.1"),
                // "Audit logs are enabled and active for all system components".
                framework(PCI_DSS_V4_0, "10.2.1"),
            ],
        ),
        (
            "require-encrypted-ebs-volumes",
            vec![
                implements("encrypt-data-at-rest-in-storage-services"),
                // "Protection of information at rest".
                framework(NIST_SP_800_53_R5, "SC-28"),
                // "Data-at-rest is protected".
                framework(NIST_CSF_V1_1, "PR.DS-1"),
                // "Ensure EBS volume encryption is enabled".
                framework(CIS_AWS_FOUNDATIONS_V3_0_0, "2.2.1"),
                related("s3-bucket-server-side-encryption-enabled", "COMPLEMENTARY"),
            ],
        ),
        (
            "deny-public-rds-snapshots",
            vec![
                implements("restrict-public-access"),
                // "Access enforcement".
                framework(NIST_SP_800_53_R5, "AC-3"),
                // "Protections against data leaks are implemented".
                framework(NIST_CSF_V1_1, "PR.DS-5"),
                // "Configure data access control lists".
                framework(CIS_CONTROLS_V8_0, "3.3"),
                // Both controls implement the restrict-public-access common control,
                // each over the resources of its own provider, so an estate that spans
                // both enables both: COMPLEMENTARY rather than ALTERNATIVE.
                related("gcp-storage-bucket-uniform-access", "COMPLEMENTARY"),
            ],
        ),
        (
            "require-mfa-for-console-access",
            vec![
                implements("require-multi-factor-authentication"),
                // "Identification and authentication (organizational users)".
                framework(NIST_SP_800_53_R5, "IA-2"),
                // "Users, devices and other assets are authenticated".
                framework(NIST_CSF_V1_1, "PR.AC-7"),
                // "Ensure MFA is enabled for all IAM users that have a console password".
                framework(CIS_AWS_FOUNDATIONS_V3_0_0, "1.10"),
                // "Multi-factor authentication is implemented for all access".
                framework(PCI_DSS_V4_0, "8.4.2"),
                related("restrict-root-user-access-keys", "ALTERNATIVE"),
            ],
        ),
        (
            "gcp-storage-bucket-uniform-access",
            vec![
                implements("restrict-public-access"),
                // "Ensure that uniform bucket-level access is enabled on Cloud Storage buckets".
                framework(CIS_GCP_FOUNDATION_V2_0_0, "5.2"),
                // "Access enforcement".
                framework(NIST_SP_800_53_R5, "AC-3"),
                // "Protections against data leaks are implemented".
                framework(NIST_CSF_V1_1, "PR.DS-5"),
                related("deny-public-rds-snapshots", "COMPLEMENTARY"),
            ],
        ),
    ];
    let mapping_seed: Vec<(String, MappingDetails)> = mapping_seed
        .into_iter()
        .flat_map(|(control_slug, mappings)| {
            let arn = control_arn(control_slug);
            mappings
                .into_iter()
                .map(move |mapping| (arn.clone(), mapping))
        })
        .collect();
    for (control_arn, mapping) in mapping_seed {
        let (mapping_type, target) = match &mapping {
            MappingDetails::Framework { name, item } => {
                ("FRAMEWORK".to_string(), format!("{name}/{item}"))
            }
            MappingDetails::CommonControl { common_control_arn } => {
                ("COMMON_CONTROL".to_string(), common_control_arn.clone())
            }
            MappingDetails::RelatedControl {
                control_arn,
                relation_type,
            } => (
                "RELATED_CONTROL".to_string(),
                format!(
                    "{}/{relation_type}",
                    control_arn.clone().unwrap_or_default()
                ),
            ),
        };
        let key = format!("{control_arn}|{mapping_type}|{target}");
        control_mappings.insert(
            key,
            ControlMappingRecord {
                control_arn,
                mapping_type,
                mapping,
            },
        );
    }

    ComAmazonawsControlcatalogControlcatalogState {
        domains,
        objectives,
        common_controls,
        controls,
        control_mappings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every reference in the seeded catalog resolves to an entry the matching list
    /// or get operation also returns.
    #[test]
    fn the_seeded_catalog_is_referentially_closed() {
        let state = ComAmazonawsControlcatalogControlcatalogState::default();
        for (arn, objective) in &state.objectives {
            assert!(
                state.domains.contains_key(&objective.domain_arn),
                "objective {arn}: domain {} is not in the catalog",
                objective.domain_arn
            );
        }
        for (arn, common_control) in &state.common_controls {
            assert!(
                state.objectives.contains_key(&common_control.objective_arn),
                "common control {arn}: objective {} is not in the catalog",
                common_control.objective_arn
            );
            assert!(
                state.domains.contains_key(&common_control.domain_arn),
                "common control {arn}: domain {} is not in the catalog",
                common_control.domain_arn
            );
        }
        for (key, mapping) in &state.control_mappings {
            assert!(
                state.controls.contains_key(&mapping.control_arn),
                "mapping {key}: control {} is not in the catalog",
                mapping.control_arn
            );
            match &mapping.mapping {
                MappingDetails::CommonControl { common_control_arn } => assert!(
                    state.common_controls.contains_key(common_control_arn),
                    "mapping {key}: common control {common_control_arn} is not in the catalog"
                ),
                MappingDetails::RelatedControl { control_arn, .. } => {
                    let related = control_arn.as_deref().unwrap_or_default();
                    assert!(
                        state.controls.contains_key(related),
                        "mapping {key}: related control {related} is not in the catalog"
                    );
                }
                MappingDetails::Framework { .. } => {}
            }
        }
    }

    /// A token resumes only in the collection and filter that issued it.
    #[test]
    fn a_token_from_another_collection_is_refused() {
        let items: Vec<(String, u8)> = vec![("a".to_string(), 1), ("b".to_string(), 2)];
        let first = page("domains", "", items.clone(), None, Some(1), 100)
            .expect("the first page has no token to decode");
        let token = first.next_token.expect("two items, page size 1: more remain");
        assert!(
            page("objectives", "", items.clone(), Some(&token), Some(1), 100).is_err(),
            "case replay-across-collections: a domains token must not resume objectives"
        );
        assert!(
            page("domains", "other-filter", items, Some(&token), Some(1), 100).is_err(),
            "case replay-across-filters: a token must not resume a different filter"
        );
    }
}
