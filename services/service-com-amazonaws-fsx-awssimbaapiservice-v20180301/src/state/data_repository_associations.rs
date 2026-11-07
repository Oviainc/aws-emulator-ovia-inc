// SPDX-License-Identifier: CC0-1.0

//! The data-repository-association slice of this service's state: the links
//! `CreateDataRepositoryAssociation` writes between a directory of an FSx for Lustre file
//! system and an Amazon S3 bucket or prefix, and what
//! `DescribeDataRepositoryAssociations` reads.
//!
//! An association is a resource of its own — "the system-generated, unique ID of the data
//! repository association" (`DataRepositoryAssociation$AssociationId`) — so it is minted and
//! stored here rather than folded into the file system's record, and it belongs to its file
//! system: the records go when the file system does.
//!
//! The S3 bucket an association's `DataRepositoryPath` names is NOT a resource of this
//! service; [`super::s3_data_repository`] resolves it where the service that owns buckets
//! holds it.

use std::collections::BTreeMap;
use std::time::{Instant, SystemTime};

use crate::ops::types::{DataRepositoryAssociation, S3DataRepositoryConfiguration};

use super::{
    epoch_date_time, fsx_arn, tag_list, within_error_message_length,
    ComAmazonawsFsxAwssimbaapiserviceV20180301State, PendingTransition, TransitionTarget,
};

/// `DataRepositoryLifecycle` members this slice uses, from the modelled enum
/// (`smithy_query(query="shape", shape="DataRepositoryLifecycle")`: CREATING | AVAILABLE |
/// MISCONFIGURED | UPDATING | DELETING | FAILED). A created association starts `CREATING`
/// ("the data repository association between the file system or cache and the data
/// repository is being created") and becomes `AVAILABLE` ("the data repository association
/// is available for use"); the other members describe work this slice never starts.
pub const DATA_REPOSITORY_LIFECYCLE_CREATING: &str = "CREATING";
/// See [`DATA_REPOSITORY_LIFECYCLE_CREATING`].
pub const DATA_REPOSITORY_LIFECYCLE_AVAILABLE: &str = "AVAILABLE";
/// See [`DATA_REPOSITORY_LIFECYCLE_CREATING`]. "`DELETING` - The data repository association
/// is undergoing a customer initiated deletion."
pub const DATA_REPOSITORY_LIFECYCLE_DELETING: &str = "DELETING";

/// Whether the data in the file system that corresponds to an association is deleted with
/// it, when the delete request states nothing, from
/// `DeleteDataRepositoryAssociationResponse$DeleteDataInFileSystem` ("Default is false").
pub const DEFAULT_DELETE_DATA_IN_FILE_SYSTEM: bool = false;

/// The resource type an association's `ResourceARN` names, as the `<type>/<id>` half of an
/// FSx ARN (one of [`super::FSX_RESOURCE_TYPES`]).
pub const ASSOCIATION_RESOURCE_TYPE: &str = "association";

/// The most data repository associations one file system holds, from
/// `CreateDataRepositoryAssociation`'s own documentation: "You can have a maximum of 8 data
/// repository associations on a file system."
///
/// A create past it is refused with `BadRequest` rather than the modelled
/// `ServiceLimitExceeded`: that error's `Limit` member is REQUIRED and the modelled
/// `ServiceLimit` enum (`smithy_query(query="shape", shape="ServiceLimit")`) has no value
/// naming data repository associations, so there is no limit value to report, and
/// `BadRequest` is "a generic error indicating a failure with a client request".
pub const MAX_DATA_REPOSITORY_ASSOCIATIONS_PER_FILE_SYSTEM: usize = 8;

/// The most associations this store holds AT ALL: the per-file-system maximum over the
/// account's own bound on live file systems ([`super::MAX_FILE_SYSTEMS`]), which is the most
/// a client can hold at once.
pub const MAX_DATA_REPOSITORY_ASSOCIATIONS: usize =
    super::MAX_FILE_SYSTEMS * MAX_DATA_REPOSITORY_ASSOCIATIONS_PER_FILE_SYSTEM;

/// The largest page `DescribeDataRepositoryAssociations` returns, from the modelled
/// `MaxResults` member's own `LimitedMaxResults` `range 1..25`.
pub const MAX_DATA_REPOSITORY_ASSOCIATIONS_PER_PAGE: usize = 25;

/// The modelled `FilterName`s `DescribeDataRepositoryAssociations` selects associations by,
/// from its own documentation: "use the `file-system-id` filter with the ID of the file
/// system ... the `file-cache-id` filter with the ID of the cache ... the
/// `data-repository-type` filter with a value of S3 or NFS".
pub const DATA_REPOSITORY_ASSOCIATION_FILTER_NAMES: &[&str] = &[
    DATA_REPOSITORY_ASSOCIATION_FILTER_FILE_SYSTEM_ID,
    DATA_REPOSITORY_ASSOCIATION_FILTER_FILE_CACHE_ID,
    DATA_REPOSITORY_ASSOCIATION_FILTER_DATA_REPOSITORY_TYPE,
];
/// See [`DATA_REPOSITORY_ASSOCIATION_FILTER_NAMES`].
pub const DATA_REPOSITORY_ASSOCIATION_FILTER_FILE_SYSTEM_ID: &str = "file-system-id";
/// See [`DATA_REPOSITORY_ASSOCIATION_FILTER_NAMES`].
pub const DATA_REPOSITORY_ASSOCIATION_FILTER_FILE_CACHE_ID: &str = "file-cache-id";
/// See [`DATA_REPOSITORY_ASSOCIATION_FILTER_NAMES`].
pub const DATA_REPOSITORY_ASSOCIATION_FILTER_DATA_REPOSITORY_TYPE: &str = "data-repository-type";

/// The data-repository types the `data-repository-type` filter admits, from the same
/// sentence: "a value of S3 or NFS". A value outside them is the modelled
/// `InvalidDataRepositoryType`.
pub const DATA_REPOSITORY_TYPES: &[&str] = &[DATA_REPOSITORY_TYPE_S3, "NFS"];
/// See [`DATA_REPOSITORY_TYPES`]. Every association this slice stores links an S3 bucket or
/// prefix, so this is the type each of them is selected by.
pub const DATA_REPOSITORY_TYPE_S3: &str = "S3";

/// Whether an import metadata task runs after a create that states nothing, from
/// `CreateDataRepositoryAssociation$BatchImportMetaDataOnCreate` ("Default is false").
pub const DEFAULT_BATCH_IMPORT_META_DATA_ON_CREATE: bool = false;

/// The stripe chunk size an association has when the create states none, from
/// `CreateDataRepositoryAssociation$ImportedFileChunkSize` ("The default chunk size is 1,024
/// MiB (1 GiB)").
pub const DEFAULT_IMPORTED_FILE_CHUNK_SIZE: i32 = 1024;

/// The scheme a `DataRepositoryPath` of an FSx for Lustre association carries, from
/// `CreateDataRepositoryAssociation$DataRepositoryPath`: "The path can be an S3 bucket or
/// prefix in the format `s3://bucket-name/prefix/` (where prefix is optional)."
pub const S3_DATA_REPOSITORY_SCHEME: &str = "s3://";

/// [`ComAmazonawsFsxAwssimbaapiserviceV20180301State::insert_data_repository_association`]
/// refused the record: as many associations are already held as the service's own limit
/// allows. See [`MAX_DATA_REPOSITORY_ASSOCIATIONS_PER_FILE_SYSTEM`] for which error the
/// caller answers with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataRepositoryAssociationLimitReached {
    /// The file system already holds [`MAX_DATA_REPOSITORY_ASSOCIATIONS_PER_FILE_SYSTEM`].
    PerFileSystem,
    /// The store already holds [`MAX_DATA_REPOSITORY_ASSOCIATIONS`].
    Store,
}

/// What a `CreateDataRepositoryAssociation` `ClientRequestToken` replays.
#[derive(Debug, Clone)]
pub struct IdempotentDataRepositoryAssociation {
    /// The association the first call with this token created.
    pub association_id: String,
    /// A FIXED-SIZE digest of that call's parameters. A replay whose parameters differ is
    /// `IncompatibleParameterError`, which AWS documents for this operation. A digest rather
    /// than the parameters themselves, so one remembered token costs a bounded amount of
    /// memory however large the request that minted it was.
    pub fingerprint: String,
}

/// One stored data repository association. Mirrors the modelled
/// `DataRepositoryAssociation` shape so
/// [`DataRepositoryAssociationRecord::to_data_repository_association`] is a projection and
/// never a second source of truth.
#[derive(Debug, Clone)]
pub struct DataRepositoryAssociationRecord {
    pub association_id: String,
    /// The file system this association belongs to; the record is removed with it.
    pub file_system_id: String,
    pub lifecycle: String,
    pub file_system_path: Option<String>,
    pub data_repository_path: String,
    pub batch_import_meta_data_on_create: bool,
    pub imported_file_chunk_size: i32,
    pub s3: Option<S3DataRepositoryConfiguration>,
    pub creation_time: SystemTime,
    pub resource_arn: String,
    pub tags: BTreeMap<String, String>,
    pub pending: Option<PendingTransition>,
}

impl DataRepositoryAssociationRecord {
    /// This record as the modelled `DataRepositoryAssociation` shape. Every member comes
    /// from the stored record, so a response can never disagree with what a later read
    /// sees.
    pub fn to_data_repository_association(&self) -> DataRepositoryAssociation {
        DataRepositoryAssociation {
            association_id: Some(self.association_id.clone()),
            resource_arn: Some(self.resource_arn.clone()),
            file_system_id: Some(self.file_system_id.clone()),
            lifecycle: Some(self.lifecycle.clone()),
            // Nothing failed: an association this slice stores was accepted, and the one
            // state that would fail it — a data repository that cannot be resolved — is
            // refused on the create instead of stored as a misconfigured association.
            failure_details: None,
            file_system_path: self.file_system_path.clone(),
            data_repository_path: Some(self.data_repository_path.clone()),
            batch_import_meta_data_on_create: Some(self.batch_import_meta_data_on_create),
            imported_file_chunk_size: Some(self.imported_file_chunk_size),
            s3: self.s3.clone(),
            tags: tag_list(&self.tags),
            creation_time: Some(epoch_date_time(self.creation_time)),
            // An Amazon File Cache resource is not one this service stores — `CreateFileCache`
            // is still 501 — so an association here belongs to a file system and reports no
            // cache, no cache path and none of the NFS members a cache association carries.
            file_cache_id: None,
            file_cache_path: None,
            data_repository_subdirectories: None,
            nfs: None,
        }
    }

    /// Whether this association satisfies one `Filter` of
    /// `DescribeDataRepositoryAssociations`: the value the filter names, read off the stored
    /// record.
    ///
    /// `None` for a filter name no stored association carries a value for, which selects
    /// nothing rather than everything.
    pub fn filtered_value(&self, filter_name: &str) -> Option<&str> {
        match filter_name {
            DATA_REPOSITORY_ASSOCIATION_FILTER_FILE_SYSTEM_ID => Some(self.file_system_id.as_str()),
            // Every association this slice stores links an S3 bucket or prefix.
            DATA_REPOSITORY_ASSOCIATION_FILTER_DATA_REPOSITORY_TYPE => Some(DATA_REPOSITORY_TYPE_S3),
            // A cache is a resource this slice does not store, so no value of this filter
            // selects an association of one.
            _ => None,
        }
    }
}

impl ComAmazonawsFsxAwssimbaapiserviceV20180301State {
    /// Apply every association lifecycle transition the clock has already made due. Called
    /// from [`ComAmazonawsFsxAwssimbaapiserviceV20180301State::settle_due_transitions`],
    /// which every handler runs.
    pub(super) fn settle_due_data_repository_association_transitions(&mut self, now: Instant) {
        let mut removed: Vec<String> = Vec::new();
        for (id, record) in &mut self.data_repository_associations {
            let Some(pending) = record.pending else {
                continue;
            };
            if pending.due > now {
                continue;
            }
            match pending.target {
                TransitionTarget::Available => {
                    record.lifecycle = DATA_REPOSITORY_LIFECYCLE_AVAILABLE.to_string();
                    record.pending = None;
                }
                TransitionTarget::Removed => removed.push(id.clone()),
            }
        }
        for id in removed {
            self.data_repository_associations.remove(&id);
        }
    }

    /// Store a newly created association, refusing once its file system holds
    /// [`MAX_DATA_REPOSITORY_ASSOCIATIONS_PER_FILE_SYSTEM`] or the store holds
    /// [`MAX_DATA_REPOSITORY_ASSOCIATIONS`].
    ///
    /// The ONE write path for a new record, so neither bound can be bypassed by a later
    /// operation that also creates associations.
    pub fn insert_data_repository_association(
        &mut self,
        record: DataRepositoryAssociationRecord,
    ) -> Result<(), DataRepositoryAssociationLimitReached> {
        if self
            .data_repository_associations
            .contains_key(&record.association_id)
        {
            self.data_repository_associations
                .insert(record.association_id.clone(), record);
            return Ok(());
        }
        if self.data_repository_associations.len() >= MAX_DATA_REPOSITORY_ASSOCIATIONS {
            return Err(DataRepositoryAssociationLimitReached::Store);
        }
        if self.data_repository_association_count_of_file_system(&record.file_system_id)
            >= MAX_DATA_REPOSITORY_ASSOCIATIONS_PER_FILE_SYSTEM
        {
            return Err(DataRepositoryAssociationLimitReached::PerFileSystem);
        }
        self.data_repository_associations
            .insert(record.association_id.clone(), record);
        Ok(())
    }

    /// Move one stored association into `DELETING` and let the clock remove it, answering the
    /// record as it now stands.
    ///
    /// `None` when the store holds no such association, which the caller answers with the
    /// modelled `DataRepositoryAssociationNotFound`. An association that is ALREADY deleting
    /// is answered as it stands rather than moved again, so a retried delete answers as the
    /// first one did (the operation is `@idempotent`).
    pub fn begin_data_repository_association_deletion(
        &mut self,
        association_id: &str,
    ) -> Option<&DataRepositoryAssociationRecord> {
        let record = self.data_repository_associations.get_mut(association_id)?;
        if record.lifecycle != DATA_REPOSITORY_LIFECYCLE_DELETING {
            record.lifecycle = DATA_REPOSITORY_LIFECYCLE_DELETING.to_string();
            record.pending = Some(PendingTransition::due_now_plus_span(
                TransitionTarget::Removed,
            ));
        }
        Some(record)
    }

    /// How many stored associations belong to `file_system_id`.
    pub fn data_repository_association_count_of_file_system(&self, file_system_id: &str) -> usize {
        self.data_repository_associations
            .values()
            .filter(|record| record.file_system_id == file_system_id)
            .count()
    }

    /// The stored association of `file_system_id` whose `FileSystemPath` overlaps `path`, if
    /// any.
    ///
    /// "Two data repository associations cannot have overlapping file system paths. For
    /// example, if a data repository is associated with file system path `/ns1/`, then you
    /// cannot link another data repository with file system path `/ns1/ns2`"
    /// (`CreateDataRepositoryAssociation$FileSystemPath`). Two directory paths overlap when
    /// one is the other or a directory under it, which is what comparing them with a single
    /// trailing separator decides.
    pub fn overlapping_data_repository_association(
        &self,
        file_system_id: &str,
        path: &str,
    ) -> Option<&DataRepositoryAssociationRecord> {
        let requested = directory_prefix(path);
        self.data_repository_associations.values().find(|record| {
            if record.file_system_id != file_system_id {
                return false;
            }
            let stored = directory_prefix(record.file_system_path.as_deref().unwrap_or_default());
            requested.starts_with(&stored) || stored.starts_with(&requested)
        })
    }

    /// Remove every association of `file_system_id`.
    ///
    /// An association belongs to its file system — AWS deletes the associations of a file
    /// system with it — so the records go when the file system does, which is also what
    /// keeps this store bounded by the live file systems.
    pub fn remove_data_repository_associations_of_file_system(&mut self, file_system_id: &str) {
        self.data_repository_associations
            .retain(|_, record| record.file_system_id != file_system_id);
    }

    /// Remember a `ClientRequestToken` replay of a create, dropping the oldest once
    /// [`super::MAX_IDEMPOTENCY_TOKENS`] are held.
    ///
    /// The ONE write path for `data_repository_association_idempotency_tokens`, so the bound
    /// cannot be bypassed.
    pub fn remember_idempotent_data_repository_association(
        &mut self,
        token: String,
        replay: IdempotentDataRepositoryAssociation,
    ) {
        super::remember_token(
            &mut self.data_repository_association_idempotency_tokens,
            &mut self.data_repository_association_token_order,
            token,
            replay,
        );
    }

    /// Forget one remembered create token.
    pub fn forget_data_repository_association_idempotency_token(&mut self, token: &str) {
        super::forget_token(
            &mut self.data_repository_association_idempotency_tokens,
            &mut self.data_repository_association_token_order,
            token,
        );
    }

    /// Mint the next `AssociationId`.
    ///
    /// Shaped to the modelled `DataRepositoryAssociationId` pattern `^(dra-[0-9a-f]{8,})$`
    /// with `length 13..23`: `dra-` plus 17 lowercase hex digits, the form AWS documents as
    /// "the system-generated, unique ID of the data repository association".
    pub fn mint_data_repository_association_id(&mut self) -> String {
        self.next_data_repository_association_id_counter += 1;
        format!(
            "dra-{:017x}",
            self.next_data_repository_association_id_counter
        )
    }
}

/// One file-system directory path as a prefix that can be compared with another: with one
/// trailing separator, so `/ns1` and `/ns1/` are the same directory while `/ns1` and
/// `/ns10` are not.
fn directory_prefix(path: &str) -> String {
    format!("{}/", path.trim_end_matches('/'))
}

/// The `BadRequest$Message` for a `DescribeDataRepositoryAssociations` `Filter` whose name
/// this listing does not select by, from the shared wording in
/// [`super::unsupported_filter_name_message_for`] with this listing's own accepted names.
pub fn unsupported_data_repository_association_filter_name_message(filter_name: &str) -> String {
    super::unsupported_filter_name_message_for(
        "DescribeDataRepositoryAssociations",
        DATA_REPOSITORY_ASSOCIATION_FILTER_NAMES,
        filter_name,
    )
}

/// The pagination scope of one `DescribeDataRepositoryAssociations` listing, from the shared
/// [`super::filtered_page_scope`] under this listing's own name.
pub fn data_repository_associations_page_scope(
    selected: &[String],
    filters: &[(String, Vec<String>)],
) -> String {
    super::filtered_page_scope("data-repository-associations", selected, filters)
}

/// The `DataRepositoryAssociationNotFound$Message` for an `AssociationIds` entry this store
/// holds no association for: a sentence naming the id the caller asked about, by the same
/// rule as every other not-found message here (see
/// [`super::file_system_not_found_message`]).
pub fn data_repository_association_not_found_message(association_id: &str) -> String {
    within_error_message_length(format!(
        "Data repository association '{association_id}' does not exist."
    ))
}

/// The `BadRequest$Message` a refused association insert is reported with: which bound it
/// hit, and the limit that states it.
pub fn data_repository_association_limit_message(
    limit: DataRepositoryAssociationLimitReached,
) -> String {
    match limit {
        DataRepositoryAssociationLimitReached::PerFileSystem => {
            within_error_message_length(format!(
                "This file system already has the maximum of \
                 {MAX_DATA_REPOSITORY_ASSOCIATIONS_PER_FILE_SYSTEM} data repository associations."
            ))
        }
        DataRepositoryAssociationLimitReached::Store => within_error_message_length(format!(
            "This account already has the maximum of {MAX_DATA_REPOSITORY_ASSOCIATIONS} data \
             repository associations in this Region."
        )),
    }
}

/// The `InvalidDataRepositoryType$Message` for a `data-repository-type` filter value
/// `DescribeDataRepositoryAssociations` does not select by: the value the request carried,
/// and the types it accepts, rendered from [`DATA_REPOSITORY_TYPES`] rather than written out
/// again so the sentence cannot drift from what the handler applies.
pub fn unsupported_data_repository_type_message(value: &str) -> String {
    within_error_message_length(format!(
        "The data repository type '{value}' is not supported. The supported types are {}.",
        DATA_REPOSITORY_TYPES.join(", ")
    ))
}

/// The ARN of a stored association, through the one canonical builder
/// ([`super::fsx_arn`]).
pub fn data_repository_association_arn(region: &str, association_id: &str) -> String {
    fsx_arn(region, ASSOCIATION_RESOURCE_TYPE, association_id)
}
