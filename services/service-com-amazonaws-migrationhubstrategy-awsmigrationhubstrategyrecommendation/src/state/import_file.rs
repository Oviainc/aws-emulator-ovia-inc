// SPDX-License-Identifier: CC0-1.0

//! The import file an import file task ingests, and the records it brings into this
//! service's state.
//!
//! `StartImportFileTask` "Starts a file import" and its `S3Bucket`/`s3key` name "the S3
//! bucket where the import file is located" and "the Amazon S3 key name of the import
//! file", so the task's work is to READ that object and ingest the resources it declares:
//! `GetImportFileTaskResponse$numberOfRecordsSuccess` is "the number of records
//! successfully imported" and `$numberOfRecordsFailed` the ones that were not, and the
//! resources that were imported are the portfolio the read path then reports — a server
//! through `ListServers`/`GetServerDetails`/`GetServerStrategies`, a process or a database
//! through `ListApplicationComponents`/`GetApplicationComponentDetails`.
//!
//! The import file's own format is not part of the API model, so this module holds it: a
//! JSON document listing the resources to import, each entry naming its resource type and
//! the attributes of the machine or workload it describes. Member names are matched
//! case-insensitively, because the format is a document a customer writes rather than a
//! modelled wire shape.
//!
//! Nothing here invents a record: an entry whose resource type this service does not
//! import, or that names neither an id nor a name to hold it by, counts towards
//! `numberOfRecordsFailed` and materialises nothing, which is why the two counts a
//! completed task reports always add up to the entries its file declared.

use std::collections::{BTreeMap, BTreeSet};

use ::serde_json::Value;

use super::analysis;
use super::{
    ApplicationComponentRecord, ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState,
    RegionState, ServerRecord, COLLECTION_TO_BE_SCHEDULED, IMPORT_FAILED, IMPORT_SUCCESS,
};
use crate::ops::types::Group;

/// The import file's resource type for a SERVER — the machine the portfolio holds as a
/// server of its inventory.
pub const RESOURCE_TYPE_SERVER: &str = "SERVER";

/// The import file's resource type for a PROCESS running on one of those servers, which
/// the portfolio holds as an application component.
pub const RESOURCE_TYPE_PROCESS: &str = "PROCESS";

/// The import file's resource type for a DATABASE running on one of those servers, the
/// other kind of application component the service holds.
pub const RESOURCE_TYPE_DATABASE: &str = "DATABASE";

/// `ResourceSubType`'s value for an application component that is a process, from
/// `smithy_query(query="shape",shape="ResourceSubType")`.
const SUB_TYPE_PROCESS: &str = "Process";

/// `ResourceSubType`'s value for one that is a database, from the same catalog.
const SUB_TYPE_DATABASE: &str = "Database";

/// `OSType`'s two values, from `smithy_query(query="shape",shape="OSType")`. The import
/// file's operating-system text is free-form, so it is held to this catalog before it is
/// stored: a read reports an `OSType` the model declares or nothing at all.
const OS_TYPE_LINUX: &str = "LINUX";
const OS_TYPE_WINDOWS: &str = "WINDOWS";

/// One resource an import file declares, as the document spells it out.
///
/// Every member is optional: the document is the customer's, so what it carries is what
/// the ingestion has to work with, and a member it leaves out is left out of the record
/// too rather than filled with a value nothing declared.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeclaredResource {
    /// Which kind of resource the entry declares — [`RESOURCE_TYPE_SERVER`] and the two
    /// component types above are the ones this service imports.
    pub resource_type: Option<String>,
    /// The id the import file gives the resource, by which another entry of the same file
    /// can refer to it. It is NOT the id the service reports: that one is minted here, so
    /// two imports naming the same file id stay two resources.
    pub external_id: Option<String>,
    /// The resource's name, reported as `ServerDetail$name` /
    /// `ApplicationComponentDetail$name`.
    pub name: Option<String>,
    /// The operating system the entry declares, free-form text.
    pub os_type: Option<String>,
    /// The operating-system version it declares, free-form text.
    pub os_version: Option<String>,
    /// The CPU architecture it declares — `SystemInfo$cpuArchitecture`.
    pub cpu_architecture: Option<String>,
    /// The file system type it declares — `SystemInfo$fileSystemType`.
    pub file_system_type: Option<String>,
    /// The servers this process or database runs on, named by each server's own entry (its
    /// resource id or its name) wherever an import declared it. Empty on a server entry,
    /// and on a component entry that names none.
    pub server_references: Vec<String>,
    /// The application type of a process or database — `ApplicationComponentDetail$appType`.
    pub app_type: Option<String>,
}

impl DeclaredResource {
    /// The entry's resource type in the upper case the format spells the types in, so a
    /// document written in another case declares the same resources.
    fn kind(&self) -> Option<String> {
        self.resource_type
            .as_deref()
            .map(|declared| declared.trim().to_ascii_uppercase())
    }

    /// The `OSType` the entry's operating-system text names, or `None` when it names
    /// neither of the two the model declares.
    ///
    /// `OSType` declares `LINUX` and `WINDOWS` alone, while the import document's own
    /// operating-system text is free-form, so the text is held to that catalog before it
    /// is stored: a read reports an `OSType` the model declares or nothing at all. The
    /// entry's own OS-type member decides it; the version text is only consulted when that
    /// member names neither value, because a document carries the system in whichever of
    /// the two it fills in. The match ignores case for the same reason the member names do.
    fn os_type_value(&self) -> Option<String> {
        named_os_type(self.os_type.as_deref()).or_else(|| named_os_type(self.os_version.as_deref()))
    }

    /// The server record this entry becomes, under the id `id` the service minted for it
    /// and the import groups `groups` the import came in with.
    ///
    /// The recommendation is the baseline every freshly ingested server carries
    /// ([`analysis::import_baseline`]) — the import has brought the server in, and no
    /// assessment has analysed it yet — and the data collection status is the one of a
    /// server no collection task has been scheduled for.
    fn to_server(&self, id: String, groups: &[Group]) -> ServerRecord {
        let baseline = analysis::import_baseline();
        ServerRecord {
            id,
            name: self.name.clone(),
            // The id the file declared for the machine, kept so a LATER import's components
            // can name the server by it ([`server_ids_named`]).
            external_id: self.external_id.clone(),
            os_type: self.os_type_value(),
            os_version: self.os_version.clone(),
            file_system_type: self.file_system_type.clone(),
            cpu_architecture: self.cpu_architecture.clone(),
            data_collection_status: Some(COLLECTION_TO_BE_SCHEDULED.to_string()),
            strategy: baseline.strategy,
            target_destination: baseline.target_destination,
            transformation_tool: baseline.transformation_tool,
            group_id: groups.to_vec(),
            ..ServerRecord::default()
        }
    }

    /// The application component this entry becomes, under the id `id`, running on the
    /// servers `server_ids` of this same import, with the import's groups.
    ///
    /// `inclusionStatus` is the including one: an imported component counts towards its
    /// server's recommendation until a caller excludes it
    /// (`UpdateApplicationComponentConfig`). The recommendation members are left unset —
    /// a component's recommendation is what the account's saved preferences select for its
    /// kind, which is an assessment's output and not the import's.
    fn to_component(
        &self,
        id: String,
        server_ids: Vec<String>,
        groups: &[Group],
    ) -> ApplicationComponentRecord {
        let sub_type = match self.kind().as_deref() {
            Some(RESOURCE_TYPE_DATABASE) => SUB_TYPE_DATABASE,
            _ => SUB_TYPE_PROCESS,
        };
        ApplicationComponentRecord {
            id,
            name: self.name.clone(),
            app_type: self.app_type.clone(),
            resource_sub_type: Some(sub_type.to_string()),
            inclusion_status: Some(analysis::INCLUDE_IN_ASSESSMENT.to_string()),
            os_version: self.os_version.clone(),
            associated_server_ids: server_ids,
            group_id: groups.to_vec(),
            ..ApplicationComponentRecord::default()
        }
    }

    /// The outcome this entry's record has, under the id `id` it was imported as, or with
    /// the `reason` it was not imported — what the import's status report lists.
    fn outcome(&self, id: Option<String>, reason: Option<String>) -> RecordOutcome {
        RecordOutcome {
            resource_type: self.resource_type.clone(),
            resource_name: self.name.clone(),
            resource_id: id,
            reason,
        }
    }
}

/// The `OSType` the free-form text `declared` names, or `None` for text that names
/// neither of the two values the model declares.
fn named_os_type(declared: Option<&str>) -> Option<String> {
    let text = declared?.to_ascii_lowercase();
    if text.contains("windows") {
        Some(OS_TYPE_WINDOWS.to_string())
    } else if text.contains("linux") {
        Some(OS_TYPE_LINUX.to_string())
    } else {
        None
    }
}

/// What one record of the import file turned into, which is what the import's status
/// report lists one of per record.
#[derive(Debug, Clone)]
pub struct RecordOutcome {
    /// The resource type the entry declared, as it declared it.
    pub resource_type: Option<String>,
    /// The name the entry declared.
    pub resource_name: Option<String>,
    /// The id the service minted for the resource, for a record it imported.
    pub resource_id: Option<String>,
    /// Why the record was not imported, for one that was not.
    pub reason: Option<String>,
}

impl RecordOutcome {
    /// Whether this record was imported, which is what the two record counts count.
    pub fn imported(&self) -> bool {
        self.reason.is_none()
    }
}

/// Everything one import's ingestion produced: the records it brings into the portfolio,
/// the outcome of every entry its file declared, and the terminal status the task that ran
/// it therefore reaches.
#[derive(Debug, Clone, Default)]
pub struct Ingestion {
    /// The servers the import ingested, each under the id minted for it.
    pub servers: Vec<ServerRecord>,
    /// The application components it ingested.
    pub components: Vec<ApplicationComponentRecord>,
    /// One outcome per entry of the file, in the order the file declared them.
    pub outcomes: Vec<RecordOutcome>,
    /// Why the import file could not be read at all — no such object, an object past the
    /// ceiling, bytes that are not an import document — for an import that read none of
    /// it. `None` for a file this import did read, including one declaring no resource.
    pub unreadable: Option<String>,
}

impl Ingestion {
    /// The ingestion of an import whose file could not be read: it brings in no record,
    /// and the task that ran it fails.
    pub fn unreadable(reason: String) -> Self {
        Self {
            unreadable: Some(reason),
            ..Self::default()
        }
    }

    /// `numberOfRecordsSuccess` — "the number of records successfully imported".
    pub fn records_success(&self) -> i32 {
        count_of(self.outcomes.iter().filter(|outcome| outcome.imported()).count())
    }

    /// `numberOfRecordsFailed` — the records of the file that were not imported.
    pub fn records_failed(&self) -> i32 {
        count_of(
            self.outcomes
                .iter()
                .filter(|outcome| !outcome.imported())
                .count(),
        )
    }

    /// The terminal `ImportFileTaskStatus` the task that ran this ingestion reaches.
    ///
    /// The status reports whether the import BROUGHT THE PORTFOLIO ANYTHING, and the two
    /// record counts report how its file's entries fared: an import whose file declared
    /// records it could bring in and records it could not still SUCCEEDED
    /// ([`IMPORT_SUCCESS`]) with `numberOfRecordsFailed` naming the rest, which is the
    /// recorded behaviour of an import of such a file, and so does one whose file asked
    /// nothing of it by declaring no resource at all. An import that read none of the
    /// file it was asked to read, or that could bring in no record of it, FAILED
    /// ([`IMPORT_FAILED`]) — nothing of what the caller asked for is in the portfolio.
    pub fn terminal_status(&self) -> &'static str {
        if self.unreadable.is_some() {
            return IMPORT_FAILED;
        }
        if self.outcomes.is_empty() || self.outcomes.iter().any(RecordOutcome::imported) {
            IMPORT_SUCCESS
        } else {
            IMPORT_FAILED
        }
    }
}

/// A counted number of records as the `Integer` members report it.
fn count_of(counted: usize) -> i32 {
    i32::try_from(counted).unwrap_or(i32::MAX)
}

/// The entry of `object` the first of `names` it holds names, as text.
///
/// The import file is a document a customer writes, so a member it spells `OSType`,
/// `osType` or `ostype` names the same thing and the match ignores case. `names` is in
/// PREFERENCE order, for the one attribute two of the format's members can carry: the
/// first one present is the one read, whatever order the document itself lists them in. A
/// member that is not a string, or is blank, names nothing.
fn member<'a>(object: &'a ::serde_json::Map<String, Value>, names: &[&str]) -> Option<&'a str> {
    names.iter().find_map(|name| {
        object
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .and_then(|(_, value)| value.as_str())
            .map(str::trim)
            .filter(|text| !text.is_empty())
    })
}

/// EVERY value the first of `names` the entry holds names: one per entry for a member
/// holding a list, and the single value for one holding a string.
///
/// The format carries the servers an application component runs on as a LIST
/// (`AssociatedServerIds`), because a component can run on more than one — which is what
/// `ApplicationComponentDetail$moreServerAssociationExists` ("set to true if the
/// application component is running on multiple servers") reports — so the reader takes
/// every entry of it rather than the first. A document that names the one server directly
/// (`AssociatedServerId`) names exactly that one. Matched the same way [`member`] matches:
/// without regard to case, in preference order, and a blank value names nothing.
fn member_values(object: &::serde_json::Map<String, Value>, names: &[&str]) -> Vec<String> {
    let named = |value: &Value| -> Vec<String> {
        let listed = match value {
            Value::Array(entries) => entries.iter().filter_map(Value::as_str).collect(),
            other => other.as_str().into_iter().collect::<Vec<&str>>(),
        };
        listed
            .into_iter()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .collect()
    };
    names
        .iter()
        .find_map(|name| {
            object
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| named(value))
                .filter(|values| !values.is_empty())
        })
        .unwrap_or_default()
}

/// The resources the import document `body` declares, or `None` when it is not an import
/// document at all (not JSON, or carrying no list of resources).
///
/// An entry the list holds that is not an object declares nothing, so it is kept as an
/// empty resource: the ingestion is what refuses it, and the file's record count still
/// accounts for it.
pub fn declared_resources(body: &str) -> Option<Vec<DeclaredResource>> {
    let document: Value = ::serde_json::from_str(body).ok()?;
    let object = document.as_object()?;
    let resources = resources_list(object)?;
    Some(
        resources
            .iter()
            .map(|entry| match entry.as_object() {
                None => DeclaredResource::default(),
                Some(entry) => DeclaredResource {
                    resource_type: member(entry, &["ResourceType"]).map(str::to_string),
                    external_id: member(entry, &["ResourceId"]).map(str::to_string),
                    name: member(entry, &["ResourceName"]).map(str::to_string),
                    os_type: member(entry, &["OSType"]).map(str::to_string),
                    // The format carries the system's version and the distribution it
                    // belongs to separately; `OSInfo` has one free-text `version` for
                    // both, so the more precise of the two is what it reports.
                    os_version: member(entry, &["OSVersion", "OSDistribution"])
                        .map(str::to_string),
                    cpu_architecture: member(entry, &["CPUArchitecture"]).map(str::to_string),
                    file_system_type: member(entry, &["FileSystemType"]).map(str::to_string),
                    // The servers a process or a database runs on, named by each server's
                    // resource id or name. The format carries them as a list, so a
                    // component can declare more than one ([`member_values`]) — bounded
                    // here by [`MAX_ASSOCIATED_SERVERS_PER_RECORD`], since the list is
                    // the caller's.
                    server_references: {
                        let mut named = member_values(
                            entry,
                            &["AssociatedServerIds", "AssociatedServerId"],
                        );
                        named.truncate(MAX_ASSOCIATED_SERVERS_PER_RECORD);
                        named
                    },
                    // And the application the component runs — reported as
                    // `ApplicationComponentDetail$appType` — under the names the format
                    // carries it by.
                    app_type: member(entry, &["AppType", "ApplicationType"]).map(str::to_string),
                },
            })
            .collect(),
    )
}

/// The document's own list of the resources to import, matched without regard to case.
fn resources_list(object: &::serde_json::Map<String, Value>) -> Option<&Vec<Value>> {
    object
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("Resources"))
        .and_then(|(_, value)| value.as_array())
}

/// How many servers ONE component entry may name.
///
/// The list is the caller's (it comes out of an import document a caller wrote), and every
/// name in it is resolved against the servers this file and this Region's portfolio hold,
/// so without a bound one entry could make the ingestion cost the product of two
/// caller-chosen sizes. It is far above any component of a real portfolio — a process runs
/// on a handful of machines, not on dozens — so no workflow this service serves meets it.
pub const MAX_ASSOCIATED_SERVERS_PER_RECORD: usize = 64;

/// How many bytes of one import file this service reads.
///
/// The object is one a caller wrote, so its size is caller-chosen: this is the emulator's
/// OWN ceiling on what one import call buffers, far above any import document a workflow
/// it serves writes. A larger object is reported as unreadable rather than buffered
/// ([`super::fetch_import_file`], which stops reading at this many bytes).
pub const MAX_IMPORT_FILE_BYTES: usize = 8 * 1024 * 1024;

/// How many records of one import file this service ingests.
///
/// AWS publishes an adjustable quota this emulator does not model, so this is the
/// emulator's OWN bound on what one import can add to the store — far above any workflow
/// it serves, and what stops a document a caller controls growing the state without end.
/// An entry past it is counted towards `numberOfRecordsFailed` with that reason rather
/// than silently dropped.
pub const MAX_IMPORT_RECORDS: usize = 10_000;

/// Ingest the resources `declared` for an import started in `region` with the groups
/// `groups`: mint the id the service reports each record by, build the record, and answer
/// what the import produced.
///
/// The id comes from the store's own counter, so a record of one Region never shares an id
/// with a record of another and two imports of the same file are two sets of records. A
/// process or a database is associated with the servers its entry names, by the resource id
/// or the name each of those servers was DECLARED under, wherever it was declared: in this
/// same file, whatever order it lists its entries in, or by an import that already brought
/// the server into this Region's portfolio (the resource id an import file declares for a
/// machine is the account's own name for it, so it names the same server across the imports
/// that declare it). An entry naming servers NOTHING has declared names no server of this
/// account, so it is counted as a record the import could not bring in.
///
/// Every reference is resolved through [`ServerIndex`], built ONCE for the whole import, so
/// an import's cost stays linear in the entries its file declares and in the inventory it
/// resolves against rather than the product of the two.
///
/// The outcomes come back in the order the file declared its entries, whichever pass
/// decided each one.
pub fn ingest(
    store: &mut ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState,
    region: &str,
    declared: &[DeclaredResource],
    groups: &[Group],
) -> Ingestion {
    let mut ingestion = Ingestion::default();
    // One outcome per entry of the file, keyed by the entry's position in it, so the two
    // passes below fill one list in the file's own order.
    let mut outcomes: BTreeMap<usize, RecordOutcome> = BTreeMap::new();
    // Where every reference of this import is resolved: the servers of THIS file, by the
    // references its other entries can name them with, and then the ones the portfolio
    // already held.
    let mut index = ServerIndex::default();

    // The entries this import cannot bring in at all, and the servers, which the
    // components are resolved against.
    for (position, resource) in declared.iter().enumerate() {
        if let Some(reason) = refusal_of(resource) {
            outcomes.insert(position, resource.outcome(None, Some(reason)));
            continue;
        }
        if resource.kind().as_deref() != Some(RESOURCE_TYPE_SERVER) {
            continue;
        }
        if ingestion.servers.len() >= MAX_IMPORT_RECORDS {
            outcomes.insert(position, resource.outcome(None, Some(too_many_records())));
            continue;
        }
        let id = store.mint_server_id(region);
        for reference in [resource.external_id.as_deref(), resource.name.as_deref()]
            .into_iter()
            .flatten()
        {
            index.declare(reference, &id);
        }
        outcomes.insert(position, resource.outcome(Some(id.clone()), None));
        ingestion.servers.push(resource.to_server(id, groups));
    }

    // The references this file's components name and this file does not itself declare are
    // the ONLY thing the Region's portfolio is searched for, and it is searched once.
    index.index_portfolio(store.region(region), &references_named(declared));

    // Then the processes and databases running on them.
    for (position, resource) in declared.iter().enumerate() {
        if outcomes.contains_key(&position) {
            continue;
        }
        let kind = resource.kind();
        match kind.as_deref() {
            Some(RESOURCE_TYPE_PROCESS | RESOURCE_TYPE_DATABASE) => {
                if ingestion.servers.len() + ingestion.components.len() >= MAX_IMPORT_RECORDS {
                    outcomes.insert(position, resource.outcome(None, Some(too_many_records())));
                    continue;
                }
                // The servers the entry names, when it names any. A component runs ON a
                // server (`ApplicationComponentDetail$associatedServerId` — "the ID of the
                // server that the application component is running on", beside
                // `moreServerAssociationExists` for a component running on several), so an
                // entry whose named servers NOTHING has declared names no server of this
                // account: there is nothing to attach the component to, and the record is
                // counted as one the import could not bring in rather than materialised as
                // a component running nowhere.
                let server_ids = if resource.server_references.is_empty() {
                    Vec::new()
                } else {
                    let resolved = index.ids_for(&resource.server_references);
                    if resolved.is_empty() {
                        outcomes.insert(
                            position,
                            resource.outcome(
                                None,
                                Some(
                                    "the record names a server no import has declared"
                                        .to_string(),
                                ),
                            ),
                        );
                        continue;
                    }
                    resolved
                };
                let id = store.mint_application_component_id(region);
                outcomes.insert(position, resource.outcome(Some(id.clone()), None));
                ingestion
                    .components
                    .push(resource.to_component(id, server_ids, groups));
            }
            _ => {
                outcomes.insert(
                    position,
                    resource.outcome(
                        None,
                        Some(
                            "the record declares a resource type this service does not import"
                                .to_string(),
                        ),
                    ),
                );
            }
        }
    }

    ingestion.outcomes = outcomes.into_values().collect();
    ingestion
}

/// Every reference this file's component entries name, deduplicated: the only references
/// the Region's portfolio is searched for.
///
/// Deduplicating here is what keeps a document that repeats one name from costing anything
/// per repetition, and the set is bounded by the file itself — at most
/// [`MAX_ASSOCIATED_SERVERS_PER_RECORD`] names per entry, from a document bounded by
/// [`MAX_IMPORT_FILE_BYTES`].
fn references_named(declared: &[DeclaredResource]) -> BTreeSet<&str> {
    declared
        .iter()
        .filter(|resource| {
            matches!(
                resource.kind().as_deref(),
                Some(RESOURCE_TYPE_PROCESS | RESOURCE_TYPE_DATABASE)
            )
        })
        .flat_map(|resource| resource.server_references.iter().map(String::as_str))
        .collect()
}

/// Where an import resolves the servers a component entry names: one index of reference to
/// server id, built ONCE for the whole import.
///
/// Resolving a reference is a single lookup, so an import costs one pass over the entries
/// its file declares plus one pass over the inventory it resolves against — never the
/// PRODUCT of the entries, the references each one names and the servers the account holds.
/// Both passes are what build this index: the servers of the file being imported
/// ([`Self::declare`], during the ingestion's first pass) and, for the references that file
/// does not itself declare, the servers the Region's portfolio already held
/// ([`Self::index_portfolio`]).
///
/// A reference resolves to the FILE's own servers where it declares any, because those are
/// the ones the document being imported declares; only a reference nothing in it declares
/// reaches back to what an earlier import brought in. Either way a server is named the same
/// two ways — by the resource id it was declared under ([`ServerRecord::external_id`]) or by
/// its name — so one spelling of a reference resolves wherever the server came from. Each
/// reference holds at most [`MAX_ASSOCIATED_SERVERS_PER_RECORD`] ids, so a name many servers
/// share cannot put an unbounded list of associations on a component record.
#[derive(Debug, Default)]
struct ServerIndex {
    /// Reference to the ids of the servers THIS file declares under it.
    declared_here: BTreeMap<String, Vec<String>>,
    /// Reference to the ids of the servers the Region's portfolio already held under it,
    /// for the references this file does not declare.
    held_already: BTreeMap<String, Vec<String>>,
}

impl ServerIndex {
    /// Record `id` as a server `reference` names, in `index`, up to the per-reference bound
    /// and never twice.
    fn record(index: &mut BTreeMap<String, Vec<String>>, reference: &str, id: &str) {
        let ids = index.entry(reference.to_string()).or_default();
        if ids.len() < MAX_ASSOCIATED_SERVERS_PER_RECORD && !ids.iter().any(|held| held == id) {
            ids.push(id.to_string());
        }
    }

    /// Note that the file being imported declares the server `id` under `reference`.
    fn declare(&mut self, reference: &str, id: &str) {
        Self::record(&mut self.declared_here, reference, id);
    }

    /// Index the servers `portfolio` already holds under the references `wanted`, in ONE
    /// pass over it — and only for the references the file does not itself declare, which
    /// take precedence.
    fn index_portfolio(&mut self, portfolio: &RegionState, wanted: &BTreeSet<&str>) {
        if wanted.is_empty() {
            return;
        }
        for server in portfolio.servers.values() {
            for reference in [server.external_id.as_deref(), server.name.as_deref()]
                .into_iter()
                .flatten()
            {
                if wanted.contains(reference) && !self.declared_here.contains_key(reference) {
                    Self::record(&mut self.held_already, reference, &server.id);
                }
            }
        }
    }

    /// The ids the reference `named` names, or an empty slice when nothing declared it.
    fn ids_named(&self, named: &str) -> &[String] {
        const NONE: &[String] = &[];
        self.declared_here
            .get(named)
            .or_else(|| self.held_already.get(named))
            .map_or(NONE, Vec::as_slice)
    }

    /// The ids the references `named` name between them, in the order they are named, each
    /// id once and at most [`MAX_ASSOCIATED_SERVERS_PER_RECORD`] of them: the servers one
    /// component entry runs on.
    fn ids_for(&self, named: &[String]) -> Vec<String> {
        let mut resolved: Vec<String> = Vec::new();
        for reference in named {
            for id in self.ids_named(reference) {
                if resolved.len() >= MAX_ASSOCIATED_SERVERS_PER_RECORD {
                    return resolved;
                }
                if !resolved.iter().any(|held| held == id) {
                    resolved.push(id.clone());
                }
            }
        }
        resolved
    }
}

/// Why `resource` cannot be brought in whatever its type, or `None` when it can.
///
/// A record the service cannot hold by anything is not imported: `ResourceType` is what
/// says which resource the entry declares, and a record carrying neither an id nor a name
/// is one no read could ever name.
fn refusal_of(resource: &DeclaredResource) -> Option<String> {
    if resource.kind().is_none() {
        return Some("the record declares no resource type".to_string());
    }
    if resource.name.is_none() && resource.external_id.is_none() {
        return Some("the record declares neither a resource id nor a name".to_string());
    }
    None
}

/// The reason an entry past [`MAX_IMPORT_RECORDS`] was not brought in.
fn too_many_records() -> String {
    format!("one import brings in at most {MAX_IMPORT_RECORDS} records")
}
