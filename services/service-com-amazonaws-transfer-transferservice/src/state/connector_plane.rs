// SPDX-License-Identifier: CC0-1.0

//! The connector operations that act on the remote filesystem itself, rather than
//! carrying local bytes to it: a move, a delete and a directory listing.
//!
//! Each one is accepted and answered with an identifier, like a transfer
//! ([`super::connector_delivery`]), and the work then runs against the endpoint the
//! connector's `Url` names. Where that endpoint is an SFTP server this emulator serves
//! ([`super::sftp_target`]), the work is something the emulator can really do, because
//! that server's files are objects of the emulator's own Amazon S3: a move relocates
//! the object, a delete removes it, and a listing reads the directory and writes what
//! it found to the output file. Each one's outcome is RECORDED on the accepted record,
//! so `ListFileTransferResults` reports the outcome of an attempt that ran, and a path
//! nothing attempted stays `QUEUED`.
//!
//! Every remote path is resolved the way the SERVER resolves a path for the user the
//! session authenticated as, exactly as a transfer's destination is, so a path outside
//! what that user's home directory covers resolves to nothing — what the server itself
//! would refuse. Nothing here leaves the process: the endpoint is a server this
//! emulator holds, and the storage calls travel over the emulator's own in-process
//! callback.

use ::serde_json::{json, Value};

use super::connector_delivery::{
    failure, s3_client, user_path_target, DeliveryPermit, DELIVERY_DEADLINE,
    MAX_IN_FLIGHT_DELIVERIES,
};
use super::remote_storage::{self, ObjectPath};
use super::sftp_target::{self, Session};
use crate::ops::types::SftpConnectorConfig;
use crate::state as st;

/// How many entries ONE directory listing reports.
///
/// `MaxItems` is "an optional parameter where you can specify the maximum number of
/// file/directory names to retrieve. The default value is 1,000", so that documented
/// default is also the ceiling this emulator serves a listing at: a caller-chosen value
/// is clamped to it, which bounds both the walk and the output document it writes.
const MAX_LISTED_ENTRIES: i32 = 1_000;

/// One accepted connector operation on the remote filesystem, as the handler that
/// accepted it hands it over.
#[derive(Debug, Clone)]
pub struct RemoteOperation {
    /// The Region the accepting request was signed for: where the secret and the
    /// storage are addressed.
    pub region: String,
    pub connector_id: String,
    /// The identifier the call answered with (`MoveId`, `DeleteId` or `ListingId`),
    /// which is where the outcome is recorded.
    pub operation_id: String,
    /// The connector's `Url`, whose endpoint this operation runs against.
    pub url: Option<String>,
    /// The connector's SFTP settings: the secret the session authenticates with and
    /// the host keys it trusts.
    pub sftp_config: SftpConnectorConfig,
    /// What the operation was asked to do.
    pub work: RemoteWork,
}

/// The three things a connector is asked to do to the remote filesystem.
#[derive(Debug, Clone)]
pub enum RemoteWork {
    /// `StartRemoteMove` "moves or renames a file or directory on the remote SFTP
    /// server".
    Move {
        source_path: String,
        target_path: String,
    },
    /// `StartRemoteDelete` "deletes a file or directory on the remote SFTP server".
    Delete { delete_path: String },
    /// `StartDirectoryListing` "retrieves a list of the contents of a directory from a
    /// remote SFTP server", written to the output file it answered the name of.
    Listing {
        remote_directory_path: String,
        output_directory_path: String,
        output_file_name: String,
        max_items: Option<i32>,
    },
}

impl RemoteWork {
    /// What the operation is called, for a log line.
    fn name(&self) -> &'static str {
        match self {
            Self::Move { .. } => "move",
            Self::Delete { .. } => "delete",
            Self::Listing { .. } => "directory listing",
        }
    }
}

/// Run `operation` on a task of its own, so the call that accepted it answers at once.
///
/// The same machinery an accepted transfer is delivered on: ONE permit per in-flight
/// delivery ([`MAX_IN_FLIGHT_DELIVERIES`]) and the same deadline, so neither kind of
/// work can accumulate tasks or hold one open for ever.
pub fn spawn(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    state: ::engine::ServiceState<st::ComAmazonawsTransferTransferserviceState>,
    operation: RemoteOperation,
) {
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        ::tracing::warn!(
            connector_id = %operation.connector_id,
            operation_id = %operation.operation_id,
            work = operation.work.name(),
            "no async runtime is available to run this connector operation on"
        );
        return;
    };
    let Some(permit) = DeliveryPermit::claim() else {
        ::tracing::warn!(
            connector_id = %operation.connector_id,
            operation_id = %operation.operation_id,
            work = operation.work.name(),
            in_flight = MAX_IN_FLIGHT_DELIVERIES,
            "this emulator is already running as many connector deliveries as it runs \
             at once, so this operation was not attempted"
        );
        return;
    };
    // The callback is an owned handle into this same emulator, so the task outlives the
    // request that produced it.
    let callback = ctx.callback().clone();
    let connector_id = operation.connector_id.clone();
    let operation_id = operation.operation_id.clone();
    let work = operation.work.name();
    runtime.spawn(async move {
        // Moved into the task, so the permit is held for exactly as long as the task
        // runs and is released however it ends.
        let _permit = permit;
        if ::tokio::time::timeout(DELIVERY_DEADLINE, deliver(&callback, &state, operation))
            .await
            .is_err()
        {
            ::tracing::warn!(
                %connector_id,
                %operation_id,
                work,
                deadline_secs = DELIVERY_DEADLINE.as_secs(),
                "this connector operation did not finish inside the deadline and was \
                 abandoned"
            );
        }
    });
}

/// Carry `operation` out over the session the connector would open, and record its
/// outcome — nothing at all when no session could be established, because then nothing
/// acted on the path.
pub async fn deliver(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    state: &::engine::ServiceState<st::ComAmazonawsTransferTransferserviceState>,
    operation: RemoteOperation,
) {
    let probe = sftp_target::probe(
        callback,
        state,
        &operation.region,
        operation.url.as_deref(),
        &operation.sftp_config,
    )
    .await;
    let Some(session) = probe.session else {
        ::tracing::info!(
            connector_id = %operation.connector_id,
            operation_id = %operation.operation_id,
            work = operation.work.name(),
            status = probe.status,
            "no SFTP session could be established for this connector, so its operation \
             was not attempted"
        );
        return;
    };

    let outcome = match &operation.work {
        RemoteWork::Move {
            source_path,
            target_path,
        } => {
            move_entry(
                callback,
                state,
                &operation,
                &session,
                source_path,
                target_path,
            )
            .await
        }
        RemoteWork::Delete { delete_path } => {
            delete_entry(callback, state, &operation, &session, delete_path).await
        }
        RemoteWork::Listing {
            remote_directory_path,
            output_directory_path,
            output_file_name,
            max_items,
        } => {
            list_directory(
                callback,
                state,
                &operation,
                &session,
                remote_directory_path,
                output_directory_path,
                output_file_name,
                *max_items,
            )
            .await
        }
    };

    // The record carries ONE path — the one the operation addresses — so its outcome is
    // the outcome of position zero. `None` is the attempt that could not be made at
    // all, and claims nothing about the path.
    if let Some(outcome) = outcome {
        state.lock().record_transfer_outcome(
            &operation.connector_id,
            &operation.operation_id,
            0,
            outcome,
        );
    }
}

/// Move the entry at `source_path` to `target_path`: the target holds what the source
/// held and the source is gone, which is what a move on the remote server leaves.
async fn move_entry(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    state: &::engine::ServiceState<st::ComAmazonawsTransferTransferserviceState>,
    operation: &RemoteOperation,
    session: &Session,
    source_path: &str,
    target_path: &str,
) -> Option<st::ConnectorFileOutcome> {
    let source = session_object(state, session, source_path)?;
    let target = session_object(state, session, target_path)?;
    let s3 = s3_client(callback, &operation.region);

    // The copy is what carries the bytes; its answer is this operation's outcome, so a
    // source the storage does not hold fails with the code and message the storage
    // itself answered rather than one this service worded.
    if let Err(refused) = s3
        .copy_object()
        .bucket(&target.bucket)
        .key(&target.key)
        // "The name of the source bucket and key name of the source object, separated
        // by a slash" — the form `CopySource` is documented in.
        .copy_source(format!("{}/{}", source.bucket, source.key))
        .send()
        .await
    {
        return Some(failure(&refused));
    }
    // Only now is the source released: a move that could not write the target leaves
    // the file where it was.
    if let Err(refused) = s3
        .delete_object()
        .bucket(&source.bucket)
        .key(&source.key)
        .send()
        .await
    {
        return Some(failure(&refused));
    }
    Some(st::ConnectorFileOutcome::completed())
}

/// Delete the entry at `delete_path`, which is absent afterwards.
async fn delete_entry(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    state: &::engine::ServiceState<st::ComAmazonawsTransferTransferserviceState>,
    operation: &RemoteOperation,
    session: &Session,
    delete_path: &str,
) -> Option<st::ConnectorFileOutcome> {
    let target = session_object(state, session, delete_path)?;
    let s3 = s3_client(callback, &operation.region);

    // The path is resolved FIRST: "deletes a file or directory on the remote SFTP
    // server" is an operation on something that is there, and S3's own delete answers
    // success for a key it never held — so what decides this outcome is the read, whose
    // error code and message are the storage's own.
    if let Err(refused) = s3
        .head_object()
        .bucket(&target.bucket)
        .key(&target.key)
        .send()
        .await
    {
        return Some(failure(&refused));
    }
    match s3
        .delete_object()
        .bucket(&target.bucket)
        .key(&target.key)
        .send()
        .await
    {
        Ok(_) => Some(st::ConnectorFileOutcome::completed()),
        Err(refused) => Some(failure(&refused)),
    }
}

/// List `remote_directory_path` and write what it holds to the output file.
#[allow(clippy::too_many_arguments)]
async fn list_directory(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    state: &::engine::ServiceState<st::ComAmazonawsTransferTransferserviceState>,
    operation: &RemoteOperation,
    session: &Session,
    remote_directory_path: &str,
    output_directory_path: &str,
    output_file_name: &str,
    max_items: Option<i32>,
) -> Option<st::ConnectorFileOutcome> {
    let listed = session_directory(state, session, remote_directory_path)?;
    // "Specifies the path (bucket and prefix) in Amazon S3 storage to store the results
    // of the directory listing": the output is written to local storage, not to the
    // remote server, so it is not resolved through the session user's home directory.
    let output = remote_storage::directory_path(output_directory_path)?.child(output_file_name);
    let s3 = s3_client(callback, &operation.region);

    // One level only: the operation "returns a list of all files and directories in the
    // remote directory (up to the maximum value), but does not return files or folders
    // in sub-directories".
    let answer = s3
        .list_objects_v2()
        .bucket(&listed.bucket)
        .prefix(listed.prefix())
        .delimiter("/")
        .max_keys(
            max_items
                .unwrap_or(MAX_LISTED_ENTRIES)
                .clamp(1, MAX_LISTED_ENTRIES),
        )
        .send()
        .await;
    let answer = match answer {
        Ok(answer) => answer,
        Err(refused) => return Some(failure(&refused)),
    };

    let document = listing_document(&listed, &answer);
    match s3
        .put_object()
        .bucket(&output.bucket)
        .key(&output.key)
        .body(::aws_sdk_s3::primitives::ByteStream::from(
            document.into_bytes(),
        ))
        .send()
        .await
    {
        Ok(_) => Some(st::ConnectorFileOutcome::completed()),
        Err(refused) => Some(failure(&refused)),
    }
}

/// The JSON document a directory listing is written as.
///
/// The layout the operation documents for the output file: each file's `filePath`, its
/// `modifiedTimestamp` ("the last time the file was modified, in UTC time format … If
/// the remote file attributes don't contain a timestamp, it is omitted") and its `size`
/// ("the size of the file, in bytes … optional"); the `path` of each remote directory;
/// and `truncated`, "a flag indicating whether the list output contains all of the
/// items contained in the remote directory or not".
fn listing_document(
    listed: &ObjectPath,
    answer: &::aws_sdk_s3::operation::list_objects_v2::ListObjectsV2Output,
) -> String {
    let prefix = listed.prefix();
    let files: Vec<Value> = answer
        .contents()
        .iter()
        .filter(|object| object.key().is_some_and(|key| key != prefix))
        .map(|object| {
            let mut file = ::serde_json::Map::new();
            let path = ObjectPath {
                bucket: listed.bucket.clone(),
                key: object.key().unwrap_or_default().to_string(),
            };
            file.insert("filePath".to_string(), json!(path.absolute()));
            if let Some(modified) = object
                .last_modified()
                .and_then(|at| at.fmt(::aws_smithy_types::date_time::Format::DateTime).ok())
            {
                file.insert("modifiedTimestamp".to_string(), json!(modified));
            }
            if let Some(size) = object.size() {
                file.insert("size".to_string(), json!(size));
            }
            Value::Object(file)
        })
        .collect();
    // A common prefix is a DIRECTORY below the listed one: object storage holds the
    // objects rather than the directories, so this is what a directory entry is.
    let paths: Vec<Value> = answer
        .common_prefixes()
        .iter()
        .filter_map(|common| {
            let name = remote_storage::file_name(common.prefix()?)?;
            Some(json!({"path": listed.child(name).absolute()}))
        })
        .collect();
    json!({
        "files": files,
        "paths": paths,
        "truncated": answer.is_truncated().unwrap_or(false),
    })
    .to_string()
}

/// The object a remote FILE path names, resolved for the session's user — `None` when
/// that user resolves the path to nothing, which is what the server would refuse.
fn session_object(
    state: &::engine::ServiceState<st::ComAmazonawsTransferTransferserviceState>,
    session: &Session,
    remote_path: &str,
) -> Option<ObjectPath> {
    remote_storage::object_path(&session_path(state, session, remote_path)?)
}

/// The same resolution for a remote DIRECTORY path, which may name a bucket root.
fn session_directory(
    state: &::engine::ServiceState<st::ComAmazonawsTransferTransferserviceState>,
    session: &Session,
    remote_path: &str,
) -> Option<ObjectPath> {
    remote_storage::directory_path(&session_path(state, session, remote_path)?)
}

/// The storage path `remote_path` resolves to for the user the session authenticated
/// as: the one resolution [`user_path_target`] implements for a transfer's destination,
/// so a move, a delete and a listing see exactly the files that user does.
fn session_path(
    state: &::engine::ServiceState<st::ComAmazonawsTransferTransferserviceState>,
    session: &Session,
    remote_path: &str,
) -> Option<String> {
    let guard = state.lock();
    let user = guard
        .users
        .get(&(session.server_id.clone(), session.user_name.clone()))?;
    user_path_target(user, remote_path)
}
