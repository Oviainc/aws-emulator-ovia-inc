// SPDX-License-Identifier: CC0-1.0

//! Carrying an accepted connector transfer OUT against the SFTP endpoint the
//! connector's `Url` names, and starting the workflow the file's arrival makes due.
//!
//! `StartFileTransfer` "begins a file transfer between local Amazon Web Services
//! storage and a remote AS2 or SFTP server": the call accepts the request, and the
//! transfer itself then runs. Where the remote server is one this emulator serves
//! ([`super::sftp_target`]), the transfer is work the emulator can really do — the
//! bytes are moved by Amazon S3 itself, over the in-process callback, from the
//! `SendFilePaths` the request named into the remote directory it named, resolved
//! through the home directory of the user the connector's secret authenticates as.
//! Each file's outcome is RECORDED on the accepted transfer, so what
//! `ListFileTransferResults` reports is the outcome of an attempt that ran, and a file
//! nothing attempted stays `QUEUED`.
//!
//! A file arriving on a Transfer Family server is also what starts that server's
//! workflow: "a file is uploaded ... Transfer Family starts an execution of the
//! workflow attached to the server's `WorkflowDetails.OnUpload`". So a delivered file
//! starts ONE execution per attached workflow, and each `CUSTOM` step of that workflow
//! is handed its callback token and its Lambda `Target` is invoked with the documented
//! event. The step then stays pending until `SendWorkflowStepState` reports an outcome
//! for that token — the at-most-once callback this module is the producer of.
//!
//! Nothing here claims an outcome for work that did not run: a transfer with no
//! session, no destination or no S3 answer records nothing and is logged for an
//! operator, and a failure records the error code and message the storage answered.
//!
//! Nothing here LEAVES the process either. The endpoint a transfer runs against is
//! resolved to a server this emulator itself holds before anything is attempted — a
//! connector whose `Url` names anything else is not connected to at all — and every
//! call this module makes (the storage read and write, the step's function) travels
//! over the emulator's own in-process callback to a service mounted beside this one. No
//! socket is opened to an address a caller named.

use ::std::time::Duration;

use ::serde_json::{json, Value};

use crate::ops::types::{
    FileLocation, S3FileLocation, ServiceMetadata, SftpConnectorConfig, UserDetails,
};
use crate::state as st;
use crate::state::sftp_target;

/// How long one accepted transfer's delivery may take before it is abandoned.
///
/// The emulator's own bound on a background task, chosen on the same basis as the
/// store bounds in [`crate::state`]: a delivery that is still running when it elapses
/// is given up on rather than held open, and the files it never reported on stay
/// `QUEUED`, because nothing transferred them.
pub(crate) const DELIVERY_DEADLINE: Duration = Duration::from_secs(60);

/// How many deliveries this process may have in flight at once.
///
/// An EMULATOR ceiling, and the companion to the per-connector bound on accepted
/// transfers: that bounds the records ONE connector can hold, this bounds how many
/// deliveries can be running at the same time, so a caller looping on
/// `StartFileTransfer` cannot spawn tasks without end. An accepted transfer that finds
/// every permit taken is not delivered and says so in the log, which leaves its files
/// `QUEUED` — the same outcome as an endpoint nothing can connect to.
pub(crate) const MAX_IN_FLIGHT_DELIVERIES: usize = 8;

/// How many deliveries are in flight right now; see [`MAX_IN_FLIGHT_DELIVERIES`].
static IN_FLIGHT_DELIVERIES: ::std::sync::atomic::AtomicUsize =
    ::std::sync::atomic::AtomicUsize::new(0);

/// One task's claim on an in-flight permit, released when the task ends — including
/// when it ends by panicking, by timing out or by being dropped at runtime shutdown,
/// because the release is a `Drop`.
pub(crate) struct DeliveryPermit;

impl DeliveryPermit {
    /// Claim a permit, or `None` when [`MAX_IN_FLIGHT_DELIVERIES`] are already in
    /// flight.
    pub(crate) fn claim() -> Option<Self> {
        IN_FLIGHT_DELIVERIES
            .try_update(
                ::std::sync::atomic::Ordering::SeqCst,
                ::std::sync::atomic::Ordering::SeqCst,
                |in_flight| (in_flight < MAX_IN_FLIGHT_DELIVERIES).then_some(in_flight + 1),
            )
            .ok()
            .map(|_| Self)
    }
}

impl Drop for DeliveryPermit {
    fn drop(&mut self) {
        IN_FLIGHT_DELIVERIES.fetch_sub(1, ::std::sync::atomic::Ordering::SeqCst);
    }
}

/// The `FailureCode` for a failure the storage answered no code of its own for: the
/// body of a read that did not arrive, or an answer that carried no error code at all.
/// The only failure wording this module supplies, and it states exactly that.
const STREAM_FAILURE_CODE: &str = "InternalError";

/// One accepted OUTBOUND transfer, as the handler that accepted it hands it over.
#[derive(Debug, Clone)]
pub struct Outbound {
    /// The Region the accepting request was signed for: where the secret, the storage
    /// and the step's function are addressed.
    pub region: String,
    pub connector_id: String,
    /// The identifier the call answered with, which is where each file's outcome is
    /// recorded.
    pub transfer_id: String,
    /// The connector's `Url`, whose endpoint this transfer runs against.
    pub url: Option<String>,
    /// The connector's SFTP settings: the secret the session authenticates with and
    /// the host keys it trusts.
    pub sftp_config: SftpConnectorConfig,
    /// `RemoteDirectoryPath`: "the destination for one or more files that are
    /// transferred to the partner's SFTP server. If you don't specify a
    /// RemoteDirectoryPath, the destination for transferred files is the SFTP user's
    /// home directory".
    pub remote_directory_path: Option<String>,
    /// The `SendFilePaths` the request named, each with its POSITION in the accepted
    /// record — which is the position its result is reported at.
    pub files: Vec<(usize, String)>,
}

/// Run `outbound` on a task of its own, so the call that accepted it answers at once.
///
/// Called with the state lock RELEASED and the record already stored: the delivery
/// resolves the record again when it has an outcome to write, so a connector deleted
/// while it runs leaves nothing to write to.
pub fn spawn(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    state: ::engine::ServiceState<st::ComAmazonawsTransferTransferserviceState>,
    outbound: Outbound,
) {
    if outbound.files.is_empty() {
        return;
    }
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        ::tracing::warn!(
            connector_id = %outbound.connector_id,
            transfer_id = %outbound.transfer_id,
            "no async runtime is available to run this connector transfer on"
        );
        return;
    };
    let Some(permit) = DeliveryPermit::claim() else {
        ::tracing::warn!(
            connector_id = %outbound.connector_id,
            transfer_id = %outbound.transfer_id,
            in_flight = MAX_IN_FLIGHT_DELIVERIES,
            "this emulator is already running as many connector transfers as it runs \
             at once, so this one was not attempted"
        );
        return;
    };
    // The callback is an owned handle into this same emulator, so the task outlives the
    // request that produced it.
    let callback = ctx.callback().clone();
    let connector_id = outbound.connector_id.clone();
    let transfer_id = outbound.transfer_id.clone();
    runtime.spawn(async move {
        // Moved into the task, so the permit is held for exactly as long as the task
        // runs and is released however it ends.
        let _permit = permit;
        if ::tokio::time::timeout(DELIVERY_DEADLINE, deliver(&callback, &state, outbound))
            .await
            .is_err()
        {
            ::tracing::warn!(
                %connector_id,
                %transfer_id,
                deadline_secs = DELIVERY_DEADLINE.as_secs(),
                "this connector transfer did not finish inside the deadline and was \
                 abandoned"
            );
        }
    });
}

/// Carry `outbound` out: one attempt per named file, then the workflow each delivered
/// file starts.
pub async fn deliver(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    state: &::engine::ServiceState<st::ComAmazonawsTransferTransferserviceState>,
    outbound: Outbound,
) {
    // The session the transfer runs over is the connection the connector would make:
    // no session, no transfer — and nothing is recorded for files nothing carried.
    let probe = sftp_target::probe(
        callback,
        state,
        &outbound.region,
        outbound.url.as_deref(),
        &outbound.sftp_config,
    )
    .await;
    let Some(session) = probe.session else {
        ::tracing::info!(
            connector_id = %outbound.connector_id,
            transfer_id = %outbound.transfer_id,
            status = probe.status,
            "no SFTP session could be established for this connector, so its transfer \
             was not attempted"
        );
        return;
    };

    for (index, source) in &outbound.files {
        let Some((source_bucket, source_key)) = split_storage_path(source) else {
            ::tracing::warn!(
                connector_id = %outbound.connector_id,
                transfer_id = %outbound.transfer_id,
                "a SendFilePaths entry names no Amazon S3 object, so nothing was \
                 transferred for it"
            );
            continue;
        };
        // Where the file lands on the remote server, resolved through the home
        // directory of the user the session authenticated as.
        let remote = remote_destination(state, &outbound, &session, &source_key);
        let Some((bucket, key)) = remote else {
            ::tracing::warn!(
                connector_id = %outbound.connector_id,
                transfer_id = %outbound.transfer_id,
                server_id = %session.server_id,
                "the remote destination of this transfer resolves to no storage \
                 location of the SFTP user's home directory, so nothing was \
                 transferred for it"
            );
            continue;
        };

        // The transfer itself: the file is READ from the storage the request named and
        // WRITTEN where it lands on the remote server, which is the pair of storage
        // operations an outbound SFTP transfer onto a Transfer Family server's S3
        // domain is made of. Its answer is this file's outcome; nothing else decides it.
        let outcome = transfer_one(
            callback,
            &outbound.region,
            &(source_bucket, source_key),
            &(bucket.clone(), key.clone()),
        )
        .await;
        let etag = match outcome {
            Ok(etag) => etag,
            Err(failed) => {
                state.lock().record_transfer_outcome(
                    &outbound.connector_id,
                    &outbound.transfer_id,
                    *index,
                    failed,
                );
                continue;
            }
        };

        state.lock().record_transfer_outcome(
            &outbound.connector_id,
            &outbound.transfer_id,
            *index,
            st::ConnectorFileOutcome::completed(),
        );

        // The file has landed on the server, which is what starts that server's upload
        // workflows.
        let started = {
            let mut guard = state.lock();
            start_upload_executions(
                &mut guard,
                &session,
                &outbound.transfer_id,
                &S3FileLocation {
                    bucket: Some(bucket.clone()),
                    key: Some(key.clone()),
                    version_id: None,
                    etag: etag.clone(),
                },
            )
        };
        for invocation in started {
            invoke_custom_step(callback, &outbound.region, &invocation).await;
        }
    }
}

/// Transfer ONE file: read it from `source` and write it to `destination`, both in
/// this emulator's own storage, and answer the `ETag` the write reported.
///
/// `Err` carries the outcome of an attempt that FAILED, with the error code and message
/// the storage answered — never one this service worded for itself.
async fn transfer_one(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    source: &(String, String),
    destination: &(String, String),
) -> Result<Option<String>, st::ConnectorFileOutcome> {
    let s3 = s3_client(callback, region);
    let read = s3
        .get_object()
        .bucket(&source.0)
        .key(&source.1)
        .send()
        .await
        .map_err(|error| failure(&error))?;
    // The file is read whole before it is written, which is what the destination's
    // write needs: the object is one this emulator's OWN storage already holds in
    // memory, so the copy adds that object's size for the length of this one file's
    // attempt and no longer — and the attempt is itself bounded by
    // [`DELIVERY_DEADLINE`] and by the modelled `len 1..10` of the paths one transfer
    // may name.
    let body = read
        .body
        .collect()
        .await
        .map_err(|error| {
            st::ConnectorFileOutcome::failed(
                STREAM_FAILURE_CODE,
                &format!("the file could not be read: {error}"),
            )
        })?
        .into_bytes();
    let written = s3
        .put_object()
        .bucket(&destination.0)
        .key(&destination.1)
        .body(::aws_sdk_s3::primitives::ByteStream::from(body))
        .send()
        .await
        .map_err(|error| failure(&error))?;
    Ok(written.e_tag().map(str::to_string))
}

/// The failed outcome an S3 answer earns: the code and message that answer carried.
pub(crate) fn failure<E: ::aws_smithy_types::error::metadata::ProvideErrorMetadata>(
    error: &E,
) -> st::ConnectorFileOutcome {
    st::ConnectorFileOutcome::failed(
        error.code().unwrap_or(STREAM_FAILURE_CODE),
        error
            .message()
            .unwrap_or("the file could not be transferred"),
    )
}

/// Where one file lands on the remote server, as a storage location.
///
/// The remote directory is the one the request named, or the session user's home
/// directory when it named none ("if you don't specify a `RemoteDirectoryPath`, the
/// destination for transferred files is the SFTP user's home directory"), and the file
/// keeps its own name within it. The path is then resolved the way the server resolves
/// a path FOR THAT USER, so a path outside what the user's home directory covers
/// resolves to nothing — exactly what the server would refuse.
fn remote_destination(
    state: &::engine::ServiceState<st::ComAmazonawsTransferTransferserviceState>,
    outbound: &Outbound,
    session: &sftp_target::Session,
    source_key: &str,
) -> Option<(String, String)> {
    let file_name = source_key.rsplit('/').next().filter(|name| !name.is_empty())?;
    let guard = state.lock();
    let user = guard
        .users
        .get(&(session.server_id.clone(), session.user_name.clone()))?;
    let directory = outbound
        .remote_directory_path
        .clone()
        .or_else(|| user.home_directory.clone())?;
    let remote_path = format!("{}/{file_name}", directory.trim_end_matches('/'));
    split_storage_path(&user_path_target(user, &remote_path)?)
}

/// The absolute storage path `remote_path` resolves to for `user`.
///
/// The two `HomeDirectoryType` values are the two resolutions the model documents:
/// `LOGICAL` resolves a path through the user's `HomeDirectoryMappings` ("the mappings
/// ... specify what Amazon S3 or Amazon EFS paths and keys should be visible to your
/// user and how you want to make them visible"), while `PATH` makes "the absolute
/// Amazon S3 bucket or EFS path" the user sees its own, so the remote path IS the
/// storage path — and only within the user's own home directory, the one tree that
/// user can write to.
pub(crate) fn user_path_target(user: &st::User, remote_path: &str) -> Option<String> {
    if user.home_directory_type.as_deref() == Some(st::HOME_DIRECTORY_TYPE_LOGICAL) {
        return logical_target(&user.home_directory_mappings, remote_path);
    }
    let home = user.home_directory.as_deref().unwrap_or_default();
    is_within(home, remote_path).then(|| remote_path.to_string())
}

/// The storage path a `LOGICAL` user's mappings resolve `remote_path` to: the LONGEST
/// `Entry` that covers the path, with its `Target` in place of that entry.
fn logical_target(
    mappings: &[crate::ops::types::HomeDirectoryMapEntry],
    remote_path: &str,
) -> Option<String> {
    let covering = mappings
        .iter()
        .filter(|mapping| is_within(&mapping.entry, remote_path))
        .max_by_key(|mapping| mapping.entry.trim_end_matches('/').len())?;
    let entry = covering.entry.trim_end_matches('/');
    let rest = remote_path.strip_prefix(entry).unwrap_or_default();
    Some(format!("{}{rest}", covering.target.trim_end_matches('/')))
}

/// Whether `path` is `directory` itself or something under it. An empty `directory`
/// covers every path: the user was given no home directory to be confined to.
fn is_within(directory: &str, path: &str) -> bool {
    let directory = directory.trim_end_matches('/');
    directory.is_empty() || path == directory || path.starts_with(&format!("{directory}/"))
}

/// Split a `bucket/key` storage path into its two parts.
///
/// `SendFilePaths` is "one or more source paths for the Amazon S3 storage ... for
/// example, `amzn-s3-demo-bucket/myfile.txt`", and the same `/bucket/key` form is what
/// a path on a Transfer Family server's S3 domain resolves to; a leading slash is part
/// of neither. `None` for a path that names no object (no key at all).
///
/// Read through [`super::remote_storage::object_path`], the ONE parse every connector
/// path of this service becomes an object address through, so a path this accepts is
/// one that stays inside the bucket it names.
pub fn split_storage_path(path: &str) -> Option<(String, String)> {
    super::remote_storage::object_path(path).map(|object| (object.bucket, object.key))
}

/// One `CUSTOM` step of a started execution, and what it is invoked with.
#[derive(Debug, Clone)]
pub struct CustomStepInvocation {
    /// The function the step names: `Target` is "the ARN for the Lambda function that
    /// is being called".
    pub target: String,
    /// "Timeout, in seconds, for the step" — the bound on waiting for that function.
    pub timeout_seconds: Option<i32>,
    /// The event the function is handed.
    pub event: Value,
}

/// Start one execution per workflow the server's `WorkflowDetails.OnUpload` attaches,
/// for a file that has just landed, and answer the `CUSTOM` step invocations they owe.
///
/// Each `CUSTOM` step of the workflow is handed a callback token of its own and stays
/// pending until `SendWorkflowStepState` reports an outcome for it; the other step
/// types are applied by the service itself and wait for nothing. A workflow the store
/// no longer holds starts no execution — the attachment names a workflow that was
/// deleted.
fn start_upload_executions(
    state: &mut st::ComAmazonawsTransferTransferserviceState,
    session: &sftp_target::Session,
    transfer_id: &str,
    file_location: &S3FileLocation,
) -> Vec<CustomStepInvocation> {
    let Some(server) = state.servers.get(&session.server_id) else {
        return Vec::new();
    };
    let domain = server.domain.clone();
    let attached: Vec<String> = server
        .workflow_details
        .as_ref()
        .and_then(|details| details.on_upload.clone())
        .unwrap_or_default()
        .into_iter()
        .map(|detail| detail.workflow_id)
        .collect();

    let mut invocations = Vec::new();
    for workflow_id in attached {
        if !state.workflows.contains_key(&workflow_id) {
            continue;
        }
        // The execution store is BOUNDED: an upload the store has no room for starts
        // no run rather than growing it, and the warning is what says so. Taken before
        // anything is minted, so a refused upload leaves no half-written record.
        if !state.take_execution_capacity(&workflow_id) {
            ::tracing::warn!(
                %workflow_id,
                max_per_workflow = st::MAX_EXECUTIONS_PER_WORKFLOW,
                max_stored = st::MAX_STORED_EXECUTIONS,
                "this emulator already holds as many workflow executions as it stores, \
                 so the upload started none"
            );
            continue;
        }
        let Some(workflow) = state.workflows.get(&workflow_id) else {
            continue;
        };
        // The `CUSTOM` steps of this workflow, each with the function it names.
        let targets: Vec<(String, Option<i32>)> = workflow
            .steps
            .iter()
            .filter(|step| step.r#type.as_deref() == Some(st::WORKFLOW_STEP_TYPE_CUSTOM))
            .filter_map(|step| step.custom_step_details.as_ref())
            .filter_map(|details| {
                details
                    .target
                    .clone()
                    .map(|target| (target, details.timeout_seconds))
            })
            .collect();

        let execution_id = state.mint_execution_id();
        let service_metadata = ServiceMetadata {
            user_details: UserDetails {
                user_name: session.user_name.clone(),
                server_id: session.server_id.clone(),
                // The session the file arrived on is the connector transfer that
                // carried it, which is the only session identifier this upload has.
                session_id: Some(transfer_id.to_string()),
            },
        };
        let mut step_states = ::std::collections::BTreeMap::new();
        for (target, timeout_seconds) in targets {
            let token = state.mint_callback_token();
            invocations.push(CustomStepInvocation {
                event: custom_step_event(
                    &token,
                    &workflow_id,
                    &execution_id,
                    &service_metadata,
                    &domain,
                    file_location,
                ),
                target,
                timeout_seconds,
            });
            step_states.insert(token, None);
        }

        state.workflow_executions.insert(
            (workflow_id.clone(), execution_id.clone()),
            st::WorkflowExecution {
                workflow_id,
                execution_id,
                initial_file_location: Some(FileLocation {
                    s3_file_location: Some(file_location.clone()),
                    efs_file_location: None,
                }),
                service_metadata: Some(service_metadata),
                step_states,
            },
        );
    }
    invocations
}

/// The event one `CUSTOM` step's function is invoked with.
///
/// "The `ExecutionId`, `WorkflowId`, and `Token` are passed to the target resource
/// during execution of a custom step of a workflow. You must include those with their
/// callback as well as providing a status" (`SendWorkflowStepState`): so the event
/// carries the token the callback is made with, the execution details that callback
/// names, the session details of the upload that started the run, and the location of
/// the file the step acts on. Every value is read from the started execution — none is
/// invented.
fn custom_step_event(
    token: &str,
    workflow_id: &str,
    execution_id: &str,
    service_metadata: &ServiceMetadata,
    domain: &str,
    file_location: &S3FileLocation,
) -> Value {
    json!({
        "token": token,
        "serviceMetadata": {
            "executionDetails": {
                "workflowId": workflow_id,
                "executionId": execution_id,
            },
            "transferDetails": {
                "serverId": service_metadata.user_details.server_id,
                "userName": service_metadata.user_details.user_name,
                "sessionId": service_metadata.user_details.session_id,
            },
        },
        "fileLocation": {
            "domain": domain,
            "bucket": file_location.bucket,
            "key": file_location.key,
            "versionId": file_location.version_id,
            "eTag": file_location.etag,
        },
    })
}

/// Invoke the function one `CUSTOM` step names with its event.
///
/// The step does NOT consume what the function returns: a custom step "stays pending
/// until `SendWorkflowStepState` reports an outcome for" its token, so the invocation
/// only has to reach the function. It is bounded by the step's own documented
/// `TimeoutSeconds` when it names one, and a function that cannot be invoked leaves
/// the step pending — which is what a callback that never arrives means — rather than
/// an outcome this service reported on the function's behalf.
async fn invoke_custom_step(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    invocation: &CustomStepInvocation,
) {
    let call = lambda_client(callback, region)
        .invoke()
        .function_name(&invocation.target)
        .invocation_type(::aws_sdk_lambda::types::InvocationType::RequestResponse)
        .payload(::aws_sdk_lambda::primitives::Blob::new(
            invocation.event.to_string().into_bytes(),
        ))
        .send();
    let budget = invocation
        .timeout_seconds
        .and_then(|seconds| u64::try_from(seconds).ok())
        .map_or(DELIVERY_DEADLINE, Duration::from_secs);
    match ::tokio::time::timeout(budget, call).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => ::tracing::warn!(
            target = %invocation.target,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "the Lambda function a custom workflow step names could not be invoked; \
             the step stays pending on its callback"
        ),
        Err(_) => ::tracing::warn!(
            target = %invocation.target,
            budget_secs = budget.as_secs(),
            "the Lambda function a custom workflow step names did not answer inside \
             the step's timeout; the step stays pending on its callback"
        ),
    }
}

/// An Amazon S3 client over the IN-PROCESS callback into this same emulator: the
/// storage a transfer reads from and writes to is this emulator's own, so the transfer
/// never leaves the process. Same client policy as [`lambda_client`].
pub(crate) fn s3_client(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        .force_path_style(true)
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// A Lambda client over the IN-PROCESS callback into this same emulator: a custom
/// step's `Target` is a function this emulator holds, so the invocation never leaves
/// the process.
///
/// The transport is in-process, so everything a network client needs and this one
/// cannot have is disabled: retries, timeouts, stalled-stream protection and identity
/// caching each need a sleep implementation the callback transport is not given.
fn lambda_client(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_lambda::Client {
    let config = ::aws_sdk_lambda::Config::builder()
        .behavior_version(::aws_sdk_lambda::config::BehaviorVersion::latest())
        .region(::aws_sdk_lambda::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://lambda.{region}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_lambda::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_lambda::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_lambda::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_lambda::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_lambda::Client::from_conf(config)
}
