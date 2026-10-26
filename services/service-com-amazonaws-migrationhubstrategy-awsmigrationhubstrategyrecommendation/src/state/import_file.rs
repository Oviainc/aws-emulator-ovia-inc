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

use std::collections::BTreeMap;

use ::serde_json::Value;

use super::analysis;
use super::{
    ApplicationComponentRecord, ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState,
    ServerRecord, COLLECTION_TO_BE_SCHEDULED, IMPORT_FAILED, IMPORT_PARTIAL_SUCCESS, IMPORT_SUCCESS,
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
    /// The server this process or database runs on, named by that server's own entry (its
    /// file id or its name). Absent on a server entry.
    pub server_reference: Option<String>,
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
    /// `ImportFileTaskStatus` declares one value per outcome an import can have, so the
    /// status is that outcome and not a fixed success: an import that read nothing it was
    /// asked to read, or that failed every record its file declared, FAILED
    /// ([`IMPORT_FAILED`]); one that brought in some records and failed others is a
    /// PARTIAL success; one that brought in every record its file declared — including a
    /// file that declared none, which asked nothing of it — succeeded.
    pub fn terminal_status(&self) -> &'static str {
        if self.unreadable.is_some() {
            return IMPORT_FAILED;
        }
        if self.outcomes.iter().all(RecordOutcome::imported) {
            IMPORT_SUCCESS
        } else if self.outcomes.iter().any(RecordOutcome::imported) {
            IMPORT_PARTIAL_SUCCESS
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
                    server_reference: member(entry, &["AssociatedServerId"]).map(str::to_string),
                    app_type: member(entry, &["AppType"]).map(str::to_string),
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
/// process or a database is associated with the server its entry names (by that server's
/// file id or name) WITHIN the same import file, wherever in the file that server is
/// declared: the servers are taken first and the components resolved against them, so the
/// association is the one the document declares and not an artefact of the order it lists
/// its entries in. An entry naming a server the file does not declare is imported with no
/// association rather than with one guessed from another import.
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
    // The servers of THIS file, by the names its other entries can refer to them with.
    let mut servers_by_reference: Vec<(String, String)> = Vec::new();

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
            servers_by_reference.push((reference.to_string(), id.clone()));
        }
        outcomes.insert(position, resource.outcome(Some(id.clone()), None));
        ingestion.servers.push(resource.to_server(id, groups));
    }

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
                let id = store.mint_application_component_id(region);
                let server_ids: Vec<String> = resource
                    .server_reference
                    .as_deref()
                    .map(|named| {
                        servers_by_reference
                            .iter()
                            .filter(|(reference, _)| reference == named)
                            .map(|(_, server_id)| server_id.clone())
                            .collect()
                    })
                    .unwrap_or_default();
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
