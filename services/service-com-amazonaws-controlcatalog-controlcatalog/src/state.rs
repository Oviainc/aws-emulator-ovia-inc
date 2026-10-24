// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-controlcatalog-controlcatalog service.
//!
//! ControlCatalog is a read-only catalog: every one of its six operations reports
//! what Amazon Web Services publishes, and none of them creates, updates or deletes
//! anything. The catalog therefore lives in this service's state, seeded once (the
//! engine builds the state through `Default` on first touch, atomically) with the
//! ontology the catalog publishes — every domain, every objective under it and every
//! common control under those objectives, in [`published_ontology`] — together with a
//! deterministic set of controls and control mappings. Every handler projects its
//! response out of these records, so two
//! processes talking to one emulator — and two runs of the same emulator — see the
//! same catalog.
//!
//! The collections are `BTreeMap`s keyed by ARN: that key order is the stable total
//! order the paginated list operations walk, and it is what a `NextToken` resumes
//! after (see [`page`]).

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};
use smithy_server_runtime::{Violation, ViolationKind};

/// The minimum every one of this service's `MaxList*Results` shapes declares with
/// `@range` (`min: 1`); only the maximum differs between operations.
const MIN_MAX_RESULTS: i32 = 1;

/// The validation path of the page-size member, as every list operation's input
/// spells it (`MaxResults`, lowerCamel in a message path).
const MAX_RESULTS_MEMBER: &str = "maxResults";

/// The controls the catalog publishes, as data.
mod published_controls;
/// The domain/objective/common-control ontology the catalog publishes, as data.
mod published_ontology;

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
///
/// A `MaxResults` outside the modeled range is a violation of that `@range`, so it is
/// worded by the one renderer this service words every constraint violation with
/// ([`crate::service::constraint_envelope`]) rather than by a sentence of its own —
/// member and constraint, with no rendering of the rejected value.
pub fn page_refusal(operation: &str, error: PageError) -> String {
    match error {
        PageError::InvalidToken => format!(
            "nextToken is not a pagination token issued by a previous {operation} call \
             with the same parameters"
        ),
        PageError::MaxResultsOutOfRange {
            requested,
            modeled_max,
        } => {
            // The modeled range is `MIN_MAX_RESULTS..=modeled_max`, so a refused value
            // is either below the minimum or above the maximum.
            let kind = if requested < MIN_MAX_RESULTS {
                ViolationKind::MinValue(MIN_MAX_RESULTS.to_string())
            } else {
                ViolationKind::MaxValue(modeled_max.to_string())
            };
            // `value` is `None` because this service's renderer never states the
            // rejected value: the member WAS present, it is simply not rendered.
            let violation = Violation {
                path: MAX_RESULTS_MEMBER.to_string(),
                kind,
                value: None,
            };
            crate::service::constraint_envelope(std::slice::from_ref(&violation)).unwrap_or_else(
                || {
                    // The renderer words every numeric bound, so this is unreachable;
                    // wording it here too keeps a refusal from ever being answered with
                    // no message at all.
                    format!(
                        "1 validation error detected: Value at '{MAX_RESULTS_MEMBER}' failed to \
                         satisfy constraint: Member must have value between \
                         {MIN_MAX_RESULTS} and {modeled_max}"
                    )
                },
            )
        }
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
        if !(MIN_MAX_RESULTS..=modeled_max).contains(&requested) {
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

/// Build the catalog this emulator serves.
///
/// The ontology (domains, objectives, common controls) is the whole published table in
/// [`published_ontology`], not a sample of it, because a client walks domain ->
/// objective -> common control for any domain the catalog publishes.
///
/// Referentially closed: every objective names a domain that is in `domains`, every
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

    // --- the published ontology: domains, objectives, common controls ------
    //
    // ONE pass over the published ontology, which carries the parentage with it: an
    // objective is inserted under the domain whose entry holds it, and a common
    // control under the objective whose entry holds it. The catalog therefore cannot
    // be seeded with an entry whose parent is missing, and every objective and every
    // common control the ontology publishes is reachable through the filter that
    // narrows to its parent.
    //
    // Timestamps are positional and fixed (see CATALOG_EPOCH), so the catalog a client
    // reads is identical in every process and on every run.
    let mut objective_position: i64 = 0;
    let mut common_control_position: i64 = 0;
    for (domain_position, (domain_slug, domain_name, domain_description, domain_objectives)) in
        published_ontology::PUBLISHED_ONTOLOGY.iter().enumerate()
    {
        let domain_arn = catalog_arn("domain", &catalog_id(domain_slug));
        let create_time = CATALOG_EPOCH + (domain_position as i64) * DAY;
        domains.insert(
            domain_arn.clone(),
            Domain {
                arn: domain_arn.clone(),
                name: (*domain_name).to_string(),
                description: (*domain_description).to_string(),
                create_time,
                // Equal to `CreateTime` until a revision moves it; see the domain seed.
                last_update_time: create_time,
            },
        );
        for (objective_slug, objective_name, objective_description, objective_common_controls) in
            *domain_objectives
        {
            let objective_arn = catalog_arn("objective", &catalog_id(objective_slug));
            let create_time = CATALOG_EPOCH + objective_position * DAY;
            objective_position += 1;
            objectives.insert(
                objective_arn.clone(),
                Objective {
                    arn: objective_arn.clone(),
                    name: (*objective_name).to_string(),
                    description: (*objective_description).to_string(),
                    domain_arn: domain_arn.clone(),
                    create_time,
                    // Equal to `CreateTime` until a revision moves it; see the domain.
                    last_update_time: create_time,
                },
            );
            for (slug, name, description) in *objective_common_controls {
                let arn = catalog_arn("common-control", &catalog_id(slug));
                let create_time = CATALOG_EPOCH + common_control_position * DAY;
                common_control_position += 1;
                common_controls.insert(
                    arn.clone(),
                    CommonControl {
                        arn,
                        name: (*name).to_string(),
                        description: (*description).to_string(),
                        objective_arn: objective_arn.clone(),
                        domain_arn: domain_arn.clone(),
                        create_time,
                        // Equal to `CreateTime` until a revision moves it; see the domain.
                        last_update_time: create_time,
                    },
                );
            }
        }
    }
    let common_control_arn = |slug: &str| catalog_arn("common-control", &catalog_id(slug));

    // --- controls ----------------------------------------------------------
    //
    // ONE pass over the published control table, which carries one row per published
    // control: its identity, the surface that implements it, the parameters that
    // surface takes, and the common control and framework items it is published
    // against. The ARN is minted from the row's slug, exactly as the ontology's are.
    //
    // Timestamps are positional and fixed (see CATALOG_EPOCH), so the catalog a client
    // reads is identical in every process and on every run.
    for (position, seed) in published_controls::PUBLISHED_CONTROLS.iter().enumerate() {
        let arn = catalog_arn("control", &catalog_id(seed.slug));
        controls.insert(
            arn.clone(),
            Control {
                arn,
                control_tower_identifier: seed.control_tower_identifier.map(str::to_string),
                // Derived from the identifiers the designators are a function of, so
                // one surface's designator cannot land on another surface's control.
                aliases: published_controls::aliases(seed),
                name: seed.name.to_string(),
                description: seed.description.to_string(),
                behavior: seed.behavior.to_string(),
                severity: Some(seed.severity.to_string()),
                region_configuration: RegionConfig {
                    scope: seed.scope.to_string(),
                    deployable_regions: seed
                        .deployable_regions
                        .iter()
                        .map(|region| (*region).to_string())
                        .collect(),
                },
                implementation: Some(Implementation {
                    implementation_type: seed.implementation_type.to_string(),
                    identifier: seed.implementation_identifier.map(str::to_string),
                }),
                // A summary OF the parameters below, never an independent fact about
                // the control.
                parameter_requirement_summary: Some(
                    published_controls::parameter_requirement_summary(seed.parameters).to_string(),
                ),
                parameters: seed
                    .parameters
                    .iter()
                    .map(|(name, requirement)| ControlParam {
                        name: (*name).to_string(),
                        requirement: Some((*requirement).to_string()),
                    })
                    .collect(),
                create_time: CATALOG_EPOCH + (position as i64) * DAY,
                governed_resources: seed
                    .governed_resources
                    .iter()
                    .map(|resource| (*resource).to_string())
                    .collect(),
                governed_providers: seed
                    .governed_providers
                    .iter()
                    .map(|provider| (*provider).to_string())
                    .collect(),
            },
        );
    }
    let control_arn = |slug: &str| catalog_arn("control", &catalog_id(slug));

    // --- control mappings --------------------------------------------------
    // The catalog records, for every control it publishes, that control's WHOLE
    // mapping set, which is the only wire source for "what does this control
    // satisfy, implement and relate to":
    //
    //  * COMMON_CONTROL — the common control the control implements (its row's
    //    `implements`);
    //  * FRAMEWORK — one entry per published compliance-framework item the control
    //    satisfies (its row's `frameworks`, each naming a published framework and that
    //    framework's own identifier for the requirement). A control normally satisfies
    //    items in SEVERAL frameworks at once, so one entry per control is not the
    //    catalog's shape;
    //  * RELATED_CONTROL — one entry per control it is published alongside, with how
    //    the two relate (`published_controls::RELATED_CONTROLS`). Relations are
    //    recorded in both directions, because a query filters on one control at a time
    //    and each side of a pair has to report the relation when it is the one queried.
    let mut mapping_seed: Vec<(String, MappingDetails)> = Vec::new();
    for seed in published_controls::PUBLISHED_CONTROLS {
        let arn = control_arn(seed.slug);
        mapping_seed.push((
            arn.clone(),
            MappingDetails::CommonControl {
                common_control_arn: common_control_arn(seed.implements),
            },
        ));
        for &(framework_name, item) in seed.frameworks {
            mapping_seed.push((
                arn.clone(),
                MappingDetails::Framework {
                    name: framework_name.to_string(),
                    item: item.to_string(),
                },
            ));
        }
    }
    for &(one, other, relation_type) in published_controls::RELATED_CONTROLS {
        for (from, to) in [(one, other), (other, one)] {
            mapping_seed.push((
                control_arn(from),
                MappingDetails::RelatedControl {
                    control_arn: Some(control_arn(to)),
                    relation_type: relation_type.to_string(),
                },
            ));
        }
    }
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

    /// Every entry the published ontology declares is seeded exactly once.
    ///
    /// A slug duplicated anywhere in the ontology would hash to the ARN that keys the
    /// catalog, so one of the two entries would silently replace the other and the
    /// served population would shrink without a diagnostic. Comparing the seeded
    /// counts with the declared ones is what makes such an editing mistake fail loudly.
    #[test]
    fn every_published_entry_is_seeded_exactly_once() {
        let state = ComAmazonawsControlcatalogControlcatalogState::default();
        let declared_domains = published_ontology::PUBLISHED_ONTOLOGY.len();
        let declared_objectives: usize = published_ontology::PUBLISHED_ONTOLOGY
            .iter()
            .map(|(_, _, _, objectives)| objectives.len())
            .sum();
        let declared_common_controls: usize = published_ontology::PUBLISHED_ONTOLOGY
            .iter()
            .flat_map(|(_, _, _, objectives)| objectives.iter())
            .map(|(_, _, _, common_controls)| common_controls.len())
            .sum();

        assert_eq!(
            state.domains.len(),
            declared_domains,
            "case domains: the ontology declares {declared_domains} domains, the catalog \
             serves {}; a duplicated domain slug would do this",
            state.domains.len()
        );
        assert_eq!(
            state.objectives.len(),
            declared_objectives,
            "case objectives: the ontology declares {declared_objectives} objectives, the \
             catalog serves {}; a duplicated objective slug would do this",
            state.objectives.len()
        );
        assert_eq!(
            state.common_controls.len(),
            declared_common_controls,
            "case common controls: the ontology declares {declared_common_controls} common \
             controls, the catalog serves {}; a duplicated common-control slug would do this",
            state.common_controls.len()
        );
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
