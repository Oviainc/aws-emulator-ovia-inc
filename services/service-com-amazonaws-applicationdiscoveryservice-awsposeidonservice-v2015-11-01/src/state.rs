// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-applicationdiscoveryservice-awsposeidonservice-v2015-11-01 service.
//!
//! Application Discovery Service reports every discovered item — an application, a
//! server, a process, a connection — as a FLAT map of `<type>.<attribute>` strings
//! (`ListConfigurations` / `DescribeConfigurations` both answer `list<map<string,string>>`).
//! This module owns the stored form of those items and the single flattening helper both
//! operations report through, so the two can never disagree on a key or a value.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::SystemTime;

/// The flat-key prefix an application's attributes carry, per the ADS
/// `<configurationType>.<attribute>` naming the `Configuration` map uses.
const APPLICATION_PREFIX: &str = "application";

/// How many `ListConfigurations` page cursors stay resolvable at once.
///
/// A page token is state the service holds for a client that may never come back, so
/// the set is bounded: paging a listing of any size needs only the cursor just handed
/// out, and an abandoned walk ages out instead of accumulating.
const MAX_LIVE_LIST_TOKENS: usize = 4096;

/// How many applications the store holds before `CreateApplication` refuses.
///
/// Application Discovery Service publishes an adjustable per-account limit on
/// applications, and this emulator models neither Service Quotas nor an account's
/// adjustments — so this is the emulator's OWN bound on the store rather than a claim
/// about any account's limit. It sits far above any workflow an emulator serves, and it
/// is what stops a caller growing this process's memory without end: every record it
/// holds is released by `DeleteApplications`, which also forgets that application's
/// tags, so the tag map cannot outgrow this bound either.
pub const APPLICATION_QUOTA: usize = 1_000;

/// A stored application configuration — the resource `CreateApplication` mints,
/// `UpdateApplication` mutates and `DeleteApplications` removes.
#[derive(Debug, Clone)]
pub struct Application {
    /// The minted `d-application-<hex>` id every other operation resolves it by.
    pub configuration_id: String,
    /// `ApplicationName` — `@required` on create, so always present.
    pub name: String,
    /// `ApplicationDescription`, absent until a create or update supplies one.
    pub description: Option<String>,
    /// `ApplicationWave`, absent until a create or update supplies one.
    pub wave: Option<String>,
    /// When the application was minted.
    pub created_time: SystemTime,
    /// When a create or update last changed it.
    pub last_modified_time: SystemTime,
    /// Configuration ids of the servers associated with this application
    /// (`AssociateConfigurationItemsToApplication`); its size is the reported
    /// `application.serverCount`.
    pub associated_configuration_ids: BTreeSet<String>,
}

impl Application {
    /// A freshly minted application, created and last modified at `now`.
    pub fn new(
        configuration_id: String,
        name: String,
        description: Option<String>,
        wave: Option<String>,
        now: SystemTime,
    ) -> Self {
        Self {
            configuration_id,
            name,
            description,
            wave,
            created_time: now,
            last_modified_time: now,
            associated_configuration_ids: BTreeSet::new(),
        }
    }

    /// This application as the flat `Configuration` map ADS reports.
    ///
    /// The ONE place an application is flattened: `ListConfigurations` and
    /// `DescribeConfigurations` both report through it, so both answer identical keys
    /// and values for the same stored record. An attribute the record does not hold is
    /// absent from the map rather than reported as an empty string.
    pub fn attributes(&self) -> HashMap<String, String> {
        let mut attrs = HashMap::new();
        attrs.insert(
            format!("{APPLICATION_PREFIX}.configurationId"),
            self.configuration_id.clone(),
        );
        attrs.insert(format!("{APPLICATION_PREFIX}.name"), self.name.clone());
        if let Some(description) = &self.description {
            attrs.insert(
                format!("{APPLICATION_PREFIX}.description"),
                description.clone(),
            );
        }
        if let Some(wave) = &self.wave {
            attrs.insert(format!("{APPLICATION_PREFIX}.wave"), wave.clone());
        }
        if !self.associated_configuration_ids.is_empty() {
            attrs.insert(
                format!("{APPLICATION_PREFIX}.serverCount"),
                self.associated_configuration_ids.len().to_string(),
            );
        }
        attrs
    }
}

/// Where a `ListConfigurations` page ended: what a `nextToken` resolves to.
///
/// The token handed to a client is opaque and server-minted; the offset it resumes
/// from is never read off the wire, so a client cannot page a different listing by
/// editing it.
#[derive(Debug, Clone)]
pub struct ListCursor {
    /// The `configurationType` the page was taken over; resuming with another type
    /// is refused.
    pub configuration_type: String,
    /// The index, in that listing, of the first item of the next page.
    pub offset: usize,
    /// The mint order of the token this cursor backs, so the oldest cursors age out
    /// without the eviction path having to read a token's text.
    pub minted_seq: u64,
}

/// Per-service state for com-amazonaws-applicationdiscoveryservice-awsposeidonservice-v2015-11-01. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsApplicationdiscoveryserviceAwsposeidonserviceV20151101State {
    /// Applications by configuration id. A `BTreeMap`, so every listing and every
    /// page of one is taken in a stable configuration-id order.
    pub applications: BTreeMap<String, Application>,
    /// Monotonic counter behind the minted `d-application-<hex>` ids.
    pub next_application_seq: u64,
    /// Live `ListConfigurations` page cursors, by the opaque token handed out.
    pub list_tokens: HashMap<String, ListCursor>,
    /// Monotonic counter behind the minted page tokens.
    pub next_list_token_seq: u64,
    /// Tags by the configuration id they are attached to (`CreateTags`/`DeleteTags`).
    /// Empty until that slice lands; `GetDiscoverySummary` already counts from it.
    pub tags: BTreeMap<String, BTreeSet<(String, String)>>,
    /// Discovered servers, as their flat `server.*` attribute maps, by configuration
    /// id. Empty until agent/import discovery lands; `GetDiscoverySummary` and the
    /// `SERVER` listing already read from it.
    pub servers: BTreeMap<String, HashMap<String, String>>,
}

impl ComAmazonawsApplicationdiscoveryserviceAwsposeidonserviceV20151101State {
    /// Mint the next unused application configuration id.
    ///
    /// ADS ids are `d-application-<lowercase hex>`; the hex comes from a monotonic
    /// counter, so an id is unique within the store and never collides with a live one.
    pub fn mint_application_id(&mut self) -> String {
        loop {
            let seq = self.next_application_seq;
            self.next_application_seq = self.next_application_seq.saturating_add(1);
            let id = format!("d-application-{seq:010x}");
            if !self.applications.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an opaque `nextToken` resuming `configuration_type` at `offset`.
    ///
    /// Only the most recently minted [`MAX_LIVE_LIST_TOKENS`] cursors are kept, so a
    /// client that pages forever cannot grow this store without bound. A cursor past
    /// that window stops resolving, and `ListConfigurations` then refuses the token
    /// with `InvalidParameterValueException` exactly as it refuses one this service
    /// never issued — it never silently restarts the listing from the top.
    pub fn issue_list_token(&mut self, configuration_type: &str, offset: usize) -> String {
        let minted_seq = self.next_list_token_seq;
        self.next_list_token_seq = self.next_list_token_seq.saturating_add(1);
        let token = format!("{minted_seq:032x}");
        self.list_tokens.insert(
            token.clone(),
            ListCursor {
                configuration_type: configuration_type.to_string(),
                offset,
                minted_seq,
            },
        );
        if self.list_tokens.len() > MAX_LIVE_LIST_TOKENS {
            let oldest_kept = minted_seq.saturating_sub(MAX_LIVE_LIST_TOKENS as u64);
            self.list_tokens
                .retain(|_, cursor| cursor.minted_seq >= oldest_kept);
        }
        token
    }

    /// The cursor a client-supplied token resolves to, or `None` when this emulator
    /// never minted it.
    pub fn resolve_list_token(&self, token: &str) -> Option<&ListCursor> {
        self.list_tokens.get(token)
    }

    /// The stored attributes of one configuration item, whatever its type, or `None`
    /// when no item holds that id.
    pub fn attributes_of(&self, configuration_id: &str) -> Option<HashMap<String, String>> {
        if let Some(application) = self.applications.get(configuration_id) {
            return Some(application.attributes());
        }
        self.servers.get(configuration_id).cloned()
    }

    /// Every stored configuration of `configuration_type`, in configuration-id order.
    ///
    /// `PROCESS` and `CONNECTION` are discovered only by a collector, which this
    /// emulator does not run, so they list empty rather than fail.
    pub fn configurations_of_type(&self, configuration_type: &str) -> Vec<HashMap<String, String>> {
        match configuration_type {
            "APPLICATION" => self
                .applications
                .values()
                .map(Application::attributes)
                .collect(),
            "SERVER" => self.servers.values().cloned().collect(),
            _ => Vec::new(),
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsApplicationdiscoveryserviceAwsposeidonserviceV20151101State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsApplicationdiscoveryserviceAwsposeidonserviceV20151101State>)
}
