// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cloudsearch-a9searchcloudconfigservice2013 service.
//!
//! One store: the search domains of this account, each holding its own index-field
//! configuration. Every operation computes its answer from this store; nothing is
//! canned and nothing is echoed back that was not written here first.
//!
//! ## The two configurations of a domain
//!
//! CloudSearch keeps a **pending** and a **deployed** copy of every configuration
//! option. `DefineIndexField` / `DeleteIndexField` change the pending copy and leave
//! the domain needing an index rebuild (`RequiresIndexDocuments`); `IndexDocuments`
//! starts the rebuild that promotes pending to deployed. The modelled `OptionState`
//! doc names exactly those three positions — `RequiresIndexDocuments`, `Processing`,
//! `Active` — so [`FieldEntry::state`] derives them from the stored copies rather
//! than storing a string a caller could contradict.
//!
//! ## What the clock owes
//!
//! Three transitions are owed by the clock rather than by a later call, and each one
//! is recorded as the instant it becomes due ([`PROVISIONING_DELAY`] after the call
//! that starts it). [`ComAmazonawsCloudsearchA9searchcloudconfigservice2013State::advance`]
//! materialises the ones already due; no handler ever sleeps.
//!
//! * domain initialisation — the modelled `DomainStatus$Created` doc: "It can take
//!   several minutes to initialize a domain when CreateDomain is called. Newly created
//!   search domains are returned from DescribeDomains with a false value for Created
//!   until domain creation is complete."
//! * index rebuild — `IndexDocuments` activates the pending options.
//! * resource cleanup after `DeleteDomain` — the modelled `DomainStatus$Deleted` doc:
//!   "Newly deleted search domains are returned from DescribeDomains with a true value
//!   for IsDeleted for several minutes until resource cleanup is complete."
//!
//! ## The store is bounded
//!
//! Both growth sites carry an explicit cap, enforced at the write rather than left to
//! the caller: at most [`MAX_DOMAINS`] domain records, and at most
//! [`MAX_INDEX_FIELDS_PER_DOMAIN`] field entries in any one of them, each entry holding
//! at most two `IndexField` copies (pending + deployed). The product
//! domains × fields × copies is therefore finite whatever a caller does, which also
//! bounds what `DescribeDomains` / `DescribeIndexFields` can be made to serialise and
//! the filter scans they run over. A write past either cap is refused with the modelled
//! `LimitExceededException` rather than silently evicting a resource someone else
//! created.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::{DomainStatus, IndexField, IndexFieldStatus, OptionStatus, ServiceEndpoint};

/// How long this emulator takes over a transition the real service measures in
/// minutes: domain initialisation, an index rebuild, and post-delete cleanup.
///
/// AWS publishes no duration for any of the three ("several minutes" is not a
/// contract), so the span is CHOSEN. It is deliberately non-zero: a caller that reads
/// straight back after `CreateDomain` or `IndexDocuments` must still observe the
/// transient state and the answers that go with it.
pub const PROVISIONING_DELAY: Duration = Duration::from_millis(500);

/// The CloudSearch API version a domain created through this (2013-01-01) model
/// speaks, which is what `ListDomainNames` reports per domain: its `DomainNameMap`
/// values target `APIVersion` — "The Amazon CloudSearch API version for a domain:
/// 2011-02-01 or 2013-01-01."
pub const API_VERSION: &str = "2013-01-01";

/// The instance type a new domain starts on. One of the values
/// `PartitionInstanceType` lists in this model (see
/// `crate::ops::types::PARTITION_INSTANCE_TYPE_VALUES`); the smallest, which is what
/// a domain with no scaling parameters runs on.
pub const DEFAULT_SEARCH_INSTANCE_TYPE: &str = "search.small";

/// The region used when a request carries no SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The most search domains this store holds.
///
/// A CHOSEN bound, not a modelled one: the Smithy model declares no domain quota, so
/// nothing here can cite one. The figure is set at the 100 search domains per account
/// that the CloudSearch developer guide gives as the service's default quota, which is
/// why a test may assert on it — but the cap's purpose is that the store be FINITE, and
/// that purpose is unaffected if the published figure is different. A `CreateDomain`
/// past it is refused with the modelled `LimitExceededException` — "The request was
/// rejected because a resource limit has already been met" — which is also what caps
/// the size of a `DescribeDomains` / `ListDomainNames` answer.
pub const MAX_DOMAINS: usize = 100;

/// The most index fields one domain holds.
///
/// A CHOSEN bound on the same terms as [`MAX_DOMAINS`], set at the 200 index fields per
/// domain the CloudSearch developer guide gives as the default quota. A
/// `DefineIndexField` introducing a NEW name past it is refused with the modelled
/// `LimitExceededException`; redefining a name already stored does not grow the store
/// and is always allowed.
pub const MAX_INDEX_FIELDS_PER_DOMAIN: usize = 200;

/// `OptionState` — the option's latest value is not deployed and needs `IndexDocuments`.
pub const STATE_REQUIRES_INDEX_DOCUMENTS: &str = "RequiresIndexDocuments";
/// `OptionState` — the option's latest value is in the process of being activated.
pub const STATE_PROCESSING: &str = "Processing";
/// `OptionState` — the option's latest value is completely deployed.
pub const STATE_ACTIVE: &str = "Active";

/// The region this request was signed for, which every ARN and endpoint this service
/// mints is built from. Falls back to [`DEFAULT_REGION`] for an unsigned request.
pub fn region_from_headers(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The one options member of `IndexField` that each `IndexFieldType` accepts.
///
/// Both columns come from the model, not from a guess: the keys are every value of
/// `smithy_query(query="shape", shape="IndexFieldType")` and the values are the
/// matching `IndexField` members from `smithy_query(query="shape",
/// shape="IndexField")`. The modelled `IndexField` doc states the relation — "The
/// supported options depend on the IndexFieldType" — and
/// [`mismatched_options_members`] is what enforces it.
pub const OPTIONS_MEMBER_BY_FIELD_TYPE: &[(&str, &str)] = &[
    ("int", "IntOptions"),
    ("double", "DoubleOptions"),
    ("literal", "LiteralOptions"),
    ("text", "TextOptions"),
    ("date", "DateOptions"),
    ("latlon", "LatLonOptions"),
    ("int-array", "IntArrayOptions"),
    ("double-array", "DoubleArrayOptions"),
    ("literal-array", "LiteralArrayOptions"),
    ("text-array", "TextArrayOptions"),
    ("date-array", "DateArrayOptions"),
];

/// The field name the service reserves for a document's relevance score.
///
/// From the modelled `IndexField$IndexFieldName` doc: "The name `score` is reserved
/// and cannot be used as a field name."
pub const RESERVED_FIELD_NAME: &str = "score";

/// The `IndexField` options member that goes with `field_type`, or `None` for a type
/// this model does not declare.
pub fn options_member_for_field_type(field_type: &str) -> Option<&'static str> {
    OPTIONS_MEMBER_BY_FIELD_TYPE
        .iter()
        .find(|(name, _)| *name == field_type)
        .map(|(_, member)| *member)
}

/// Every options member actually set on `field`, by its modelled member name.
///
/// Public so a test can assert that a stored field carries exactly the one structure
/// its `IndexFieldType` accepts and nothing else.
pub fn options_members_set(field: &IndexField) -> Vec<&'static str> {
    let present: [(&'static str, bool); 11] = [
        ("IntOptions", field.int_options.is_some()),
        ("DoubleOptions", field.double_options.is_some()),
        ("LiteralOptions", field.literal_options.is_some()),
        ("TextOptions", field.text_options.is_some()),
        ("DateOptions", field.date_options.is_some()),
        ("LatLonOptions", field.lat_lon_options.is_some()),
        ("IntArrayOptions", field.int_array_options.is_some()),
        ("DoubleArrayOptions", field.double_array_options.is_some()),
        ("LiteralArrayOptions", field.literal_array_options.is_some()),
        ("TextArrayOptions", field.text_array_options.is_some()),
        ("DateArrayOptions", field.date_array_options.is_some()),
    ];
    present
        .into_iter()
        .filter_map(|(member, set)| set.then_some(member))
        .collect()
}

/// The options members set on `field` that its own `IndexFieldType` does not accept,
/// in modelled member order. Empty means the definition is self-consistent.
pub fn mismatched_options_members(field: &IndexField) -> Vec<&'static str> {
    let accepted = options_member_for_field_type(&field.index_field_type);
    options_members_set(field)
        .into_iter()
        .filter(|member| Some(*member) != accepted)
        .collect()
}

/// One index field of a domain: its pending copy, its deployed copy, and the
/// bookkeeping the modelled `OptionStatus` reports.
///
/// `pending` is `None` exactly while the field is awaiting deletion — it has been
/// taken out of the pending configuration but the deployed index still serves it.
#[derive(Debug, Clone)]
pub struct FieldEntry {
    /// The latest configuration a caller defined, absent once it is pending deletion.
    pub pending: Option<IndexField>,
    /// The configuration the live index currently serves, absent until a rebuild
    /// has promoted a pending copy.
    pub deployed: Option<IndexField>,
    pub creation_date: SystemTime,
    pub update_date: SystemTime,
    pub update_version: i32,
    pub pending_deletion: bool,
    /// When the rebuild that activates this entry's change becomes due; `None` when
    /// no rebuild is in flight for it.
    pub activating_at: Option<Instant>,
    /// Whether the pending copy still differs from the deployed one.
    pub needs_index: bool,
}

impl FieldEntry {
    /// A newly defined field: pending only, awaiting `IndexDocuments`.
    fn new(options: IndexField, now: SystemTime) -> Self {
        Self {
            pending: Some(options),
            deployed: None,
            creation_date: now,
            update_date: now,
            update_version: 1,
            pending_deletion: false,
            activating_at: None,
            needs_index: true,
        }
    }

    /// The modelled `OptionStatus$State`, derived from the stored copies.
    pub fn state(&self) -> &'static str {
        if self.activating_at.is_some() {
            STATE_PROCESSING
        } else if self.needs_index {
            STATE_REQUIRES_INDEX_DOCUMENTS
        } else {
            STATE_ACTIVE
        }
    }

    /// This entry's `OptionStatus`.
    pub fn status(&self) -> OptionStatus {
        OptionStatus {
            creation_date: ::aws_smithy_types::DateTime::from(self.creation_date),
            update_date: ::aws_smithy_types::DateTime::from(self.update_date),
            update_version: self.update_version,
            state: self.state().to_string(),
            pending_deletion: Some(self.pending_deletion),
        }
    }

    /// The `IndexFieldStatus` for one view of this entry: the deployed copy when
    /// `deployed` is asked for, else the pending one. `None` when that copy does not
    /// exist, which is how a caller's `Deployed` filter drops a field.
    pub fn index_field_status(&self, deployed: bool) -> Option<IndexFieldStatus> {
        let options = if deployed {
            self.deployed.clone()?
        } else {
            self.pending.clone()?
        };
        Some(IndexFieldStatus {
            options,
            status: self.status(),
        })
    }
}

/// One search domain, and everything the lifecycle slice of this service stores
/// about it.
#[derive(Debug, Clone)]
pub struct Domain {
    pub domain_name: String,
    pub domain_id: String,
    pub arn: String,
    pub doc_service_endpoint: String,
    pub search_service_endpoint: String,
    pub search_instance_type: String,
    pub search_partition_count: i32,
    pub search_instance_count: i32,
    pub created_at: SystemTime,
    /// When initialisation completes. While set, `Created` is false.
    pub creating_at: Option<Instant>,
    pub deleted: bool,
    /// When post-delete resource cleanup completes and the record is purged.
    pub cleanup_at: Option<Instant>,
    pub requires_index_documents: bool,
    /// When the rebuild `IndexDocuments` started completes.
    pub indexing_at: Option<Instant>,
    /// Index fields by name, in name order — which is the order a rebuild reports
    /// them in.
    pub index_fields: BTreeMap<String, FieldEntry>,
}

impl Domain {
    /// Allocate a domain named `domain_name` in `region`, initialising from now.
    ///
    /// Every identifier is computed, never random: the id and the ARN carry the
    /// emulator's canonical account id, and the two service endpoints follow the
    /// form the modelled `ServiceUrl` doc shows
    /// (`doc-<domain>-<id>.<region>.cloudsearch.amazonaws.com`).
    pub fn new(domain_name: &str, region: &str, now: SystemTime, clock: Instant) -> Self {
        let account = ::engine::emulator_account_id();
        Self {
            domain_name: domain_name.to_string(),
            domain_id: format!("{account}/{domain_name}"),
            arn: ::engine::emulator_arn("cloudsearch", region, &format!("domain/{domain_name}")),
            doc_service_endpoint: format!(
                "doc-{domain_name}-{account}.{region}.cloudsearch.amazonaws.com"
            ),
            search_service_endpoint: format!(
                "search-{domain_name}-{account}.{region}.cloudsearch.amazonaws.com"
            ),
            search_instance_type: DEFAULT_SEARCH_INSTANCE_TYPE.to_string(),
            search_partition_count: 1,
            search_instance_count: 1,
            created_at: now,
            creating_at: Some(clock + PROVISIONING_DELAY),
            deleted: false,
            cleanup_at: None,
            requires_index_documents: false,
            indexing_at: None,
            index_fields: BTreeMap::new(),
        }
    }

    /// Whether initialisation has finished. Until it has, the domain reports
    /// `Created=false` and has no service endpoints yet.
    pub fn created(&self) -> bool {
        self.creating_at.is_none()
    }

    /// Whether the domain is still usable: a deleted one is not, even while its
    /// record lingers for cleanup.
    pub fn is_live(&self) -> bool {
        !self.deleted
    }

    /// `DomainStatus$Processing` — "True if processing is being done to activate the
    /// current domain configuration": initialisation, an index rebuild, or cleanup.
    pub fn processing(&self) -> bool {
        self.creating_at.is_some() || self.indexing_at.is_some() || self.cleanup_at.is_some()
    }

    /// This domain's `DomainStatus`, computed from the stored record.
    pub fn status(&self) -> DomainStatus {
        let created = self.created();
        DomainStatus {
            domain_id: self.domain_id.clone(),
            domain_name: self.domain_name.clone(),
            arn: Some(self.arn.clone()),
            created: Some(created),
            deleted: Some(self.deleted),
            // The endpoints exist only once the domain is initialised: before that
            // there is nothing listening to report.
            doc_service: created.then(|| ServiceEndpoint {
                endpoint: Some(self.doc_service_endpoint.clone()),
            }),
            search_service: created.then(|| ServiceEndpoint {
                endpoint: Some(self.search_service_endpoint.clone()),
            }),
            requires_index_documents: self.requires_index_documents,
            processing: Some(self.processing()),
            search_instance_type: Some(self.search_instance_type.clone()),
            search_partition_count: Some(self.search_partition_count),
            search_instance_count: Some(self.search_instance_count),
            limits: None,
        }
    }

    /// Define (or redefine) one index field in the pending configuration.
    ///
    /// Redefining a name updates it in place and bumps `UpdateVersion`, keeping the
    /// original `CreationDate`; either way the domain now needs an index rebuild.
    /// Returns the entry's resulting `IndexFieldStatus`, or
    /// `Err(MAX_INDEX_FIELDS_PER_DOMAIN)` when a NEW name would take the domain past
    /// its index-field cap — the caller answers that with the modelled
    /// `LimitExceededException`. Redefining a stored name never grows the store, so it
    /// is not capped.
    pub fn define_index_field(
        &mut self,
        options: IndexField,
        now: SystemTime,
    ) -> Result<IndexFieldStatus, usize> {
        let name = options.index_field_name.clone();
        match self.index_fields.get_mut(&name) {
            Some(entry) => {
                entry.pending = Some(options);
                entry.update_date = now;
                entry.update_version += 1;
                entry.pending_deletion = false;
                entry.activating_at = None;
                entry.needs_index = true;
            }
            None => {
                if self.index_fields.len() >= MAX_INDEX_FIELDS_PER_DOMAIN {
                    return Err(MAX_INDEX_FIELDS_PER_DOMAIN);
                }
                self.index_fields
                    .insert(name.clone(), FieldEntry::new(options, now));
            }
        }
        self.requires_index_documents = true;
        let entry = &self.index_fields[&name];
        Ok(IndexFieldStatus {
            options: entry
                .pending
                .clone()
                .expect("a just-defined field has a pending copy"),
            status: entry.status(),
        })
    }

    /// Take one index field out of the pending configuration.
    ///
    /// `None` when the name is not in the pending configuration — either it was never
    /// defined or it is already awaiting deletion. The returned status carries the
    /// options that were removed and `PendingDeletion=true`; the entry itself survives
    /// only while a deployed copy still serves the live index, and is purged by the
    /// next rebuild. The domain is marked as needing that rebuild only when there IS a
    /// deployed copy to retire.
    pub fn delete_index_field(&mut self, name: &str, now: SystemTime) -> Option<IndexFieldStatus> {
        let entry = self.index_fields.get_mut(name)?;
        let options = entry.pending.take()?;
        entry.update_date = now;
        entry.update_version += 1;
        entry.pending_deletion = true;
        entry.activating_at = None;
        entry.needs_index = true;
        let status = entry.status();
        if entry.deployed.is_none() {
            // Nothing was ever deployed under this name, so there is nothing left for a
            // rebuild to retire: the field is gone now, and this deletion on its own
            // gives the domain no new work. The flag is therefore NOT raised here — it
            // stays whatever an earlier, still-undeployed change left it, so it never
            // claims work the store does not hold.
            self.index_fields.remove(name);
        } else {
            // A deployed copy still serves the live index: retiring it is work only a
            // rebuild can do, so the domain needs one.
            self.requires_index_documents = true;
        }
        Some(IndexFieldStatus { options, status })
    }

    /// Start the rebuild that deploys the pending configuration, returning the names
    /// of the fields it indexes, in name order.
    ///
    /// The call itself satisfies the outstanding `IndexDocuments` requirement, so
    /// `RequiresIndexDocuments` drops immediately; the options reach `Active` only
    /// when the rebuild the clock owes completes.
    pub fn start_index_documents(&mut self, clock: Instant) -> Vec<String> {
        let due = clock + PROVISIONING_DELAY;
        let mut indexed = Vec::new();
        for (name, entry) in &mut self.index_fields {
            // A rebuild re-indexes the whole configuration, so every entry goes
            // through `Processing`: the ones awaiting deletion are retired by it and
            // the rest have their pending copy deployed.
            entry.activating_at = Some(due);
            if entry.pending.is_some() {
                indexed.push(name.clone());
            }
        }
        self.requires_index_documents = false;
        self.indexing_at = Some(due);
        indexed
    }

    /// Mark the domain deleted and start resource cleanup. Its index fields go with
    /// it; the record itself lingers until cleanup is due, which is what keeps it
    /// visible to `DescribeDomains` with `Deleted=true`.
    pub fn start_delete(&mut self, clock: Instant) {
        self.deleted = true;
        self.cleanup_at = Some(clock + PROVISIONING_DELAY);
        self.indexing_at = None;
        self.requires_index_documents = false;
        self.index_fields.clear();
    }

    /// Materialise whichever of this domain's owed transitions `clock` has reached.
    /// `true` means the record itself is now gone (cleanup completed).
    fn advance(&mut self, clock: Instant) -> bool {
        if self.creating_at.is_some_and(|due| clock >= due) {
            self.creating_at = None;
        }
        if self.indexing_at.is_some_and(|due| clock >= due) {
            self.indexing_at = None;
            self.index_fields.retain(|_, entry| {
                let activating = entry.activating_at.take().is_some();
                if !activating {
                    return true;
                }
                if entry.pending_deletion {
                    // The rebuild retired the field: its deployed copy is gone too.
                    return false;
                }
                entry.deployed = entry.pending.clone();
                entry.needs_index = false;
                true
            });
        }
        self.cleanup_at.is_some_and(|due| clock >= due)
    }
}

/// Per-service state for com-amazonaws-cloudsearch-a9searchcloudconfigservice2013. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCloudsearchA9searchcloudconfigservice2013State {
    /// Every domain record this account holds, by name, in name order.
    domains: BTreeMap<String, Domain>,
}

impl ComAmazonawsCloudsearchA9searchcloudconfigservice2013State {
    /// Materialise every transition the clock has already made due, and purge the
    /// domains whose post-delete cleanup has completed.
    ///
    /// Call this ONCE at the top of a handler, with one instant, so the whole request
    /// is answered as of a single point in time. These writes are owed by the clock —
    /// they would have happened whether or not anybody called — which is what lets a
    /// reporting operation make them.
    pub fn advance(&mut self, clock: Instant) {
        self.domains.retain(|_, domain| !domain.advance(clock));
    }

    /// The record stored under `name`, deleted-and-cleaning ones included.
    pub fn domain(&self, name: &str) -> Option<&Domain> {
        self.domains.get(name)
    }

    /// The still-usable domain stored under `name`. A deleted one answers `None`, so
    /// a handler cannot serve a domain that is only waiting to be cleaned up.
    pub fn live_domain_mut(&mut self, name: &str) -> Option<&mut Domain> {
        self.domains.get_mut(name).filter(|domain| domain.is_live())
    }

    /// The record stored under `name`, mutably, deleted ones included.
    pub fn domain_mut(&mut self, name: &str) -> Option<&mut Domain> {
        self.domains.get_mut(name)
    }

    /// Store a newly allocated domain. The caller has already established that the
    /// name is free.
    ///
    /// `Err(MAX_DOMAINS)` when the store is already at its domain cap — the caller
    /// answers that with the modelled `LimitExceededException`. Nothing is evicted to
    /// make room: a resource a caller created stays until that caller deletes it.
    pub fn insert_domain(&mut self, domain: Domain) -> Result<&Domain, usize> {
        if self.domains.len() >= MAX_DOMAINS {
            return Err(MAX_DOMAINS);
        }
        let name = domain.domain_name.clone();
        self.domains.insert(name.clone(), domain);
        Ok(&self.domains[&name])
    }

    /// Every record, in name order — what `DescribeDomains` reports from.
    pub fn all_domains(&self) -> impl Iterator<Item = &Domain> {
        self.domains.values()
    }

    /// Every still-usable domain, in name order — what `ListDomainNames` reports from.
    pub fn live_domains(&self) -> impl Iterator<Item = &Domain> {
        self.domains.values().filter(|domain| domain.is_live())
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCloudsearchA9searchcloudconfigservice2013State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCloudsearchA9searchcloudconfigservice2013State>)
}
