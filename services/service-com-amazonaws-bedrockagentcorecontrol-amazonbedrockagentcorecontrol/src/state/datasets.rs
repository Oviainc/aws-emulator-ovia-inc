// SPDX-License-Identifier: CC0-1.0

//! The evaluation DATASET's state: one [`DatasetRecord`] per `datasetId`,
//! holding the configuration the create persisted, the examples its source
//! yielded into the DRAFT working copy, and the transition out of `CREATING`
//! the clock owes it.
//!
//! The examples live on the record rather than in a store of their own, because
//! an example is addressed only THROUGH the dataset that holds it
//! (`/datasets/{datasetId}/examples`) and is gone when the dataset is — the same
//! reason a gateway target lives inside its gateway.
//!
//! The ingestion of a source lives here too ([`ingest_dataset_source`]): an
//! inline source carries its examples in the request, and an Amazon S3 source
//! names a JSONL file this emulator READS out of its own object storage, so the
//! number of examples a dataset reports is the number it really holds either
//! way.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::Document;
use ::serde_json::Value;

use crate::ops::types::{
    CreateDatasetInput, CreateDatasetOutput, DataSourceType, GetDatasetOutput,
};

use super::{sorted_map, timestamp};

/// The ARN resource type a dataset carries — `DatasetArn` is
/// `…:bedrock-agentcore:{region}:{account}:dataset/{datasetId}` (`smithy_query
/// shape=DatasetArn`) — and the resource noun a missing dataset, or a missing
/// VERSION of one, is reported by.
const ARN_TYPE_DATASET: &str = "dataset";
pub const RESOURCE_DATASET: &str = "dataset";
pub const RESOURCE_DATASET_VERSION: &str = "dataset version";

/// `DatasetStatus` members this service drives. The enum's full value list is
/// `CREATING | UPDATING | DELETING | ACTIVE | CREATE_FAILED | UPDATE_FAILED |
/// DELETE_FAILED` (`smithy_query shape=DatasetStatus`).
pub const DATASET_STATUS_CREATING: &str = "CREATING";
pub const DATASET_STATUS_ACTIVE: &str = "ACTIVE";
pub const DATASET_STATUS_CREATE_FAILED: &str = "CREATE_FAILED";

/// `DraftStatus` members a dataset reports. The enum's full value list is
/// `MODIFIED | UNMODIFIED` (`smithy_query shape=DraftStatus`), and `MODIFIED`
/// is what the DRAFT of a dataset with no published version is: "DRAFT has
/// changes not yet reflected in any published version, or no versions have been
/// published yet".
pub const DRAFT_STATUS_MODIFIED: &str = "MODIFIED";

/// The version a dataset read resolves to when the request names none: "Use the
/// `datasetVersion` query parameter to retrieve a specific version's metadata.
/// If absent, defaults to DRAFT" (`smithy_query doc shape=GetDataset`).
/// `DatasetVersion` is `^(DRAFT|[0-9]+)$`, so every other spelling is a
/// published version number.
pub const DATASET_VERSION_DRAFT: &str = "DRAFT";

/// How long a freshly created dataset stays `CREATING` before the clock owes it
/// the status its ingestion earned.
///
/// The create is asynchronous — "Returns immediately with status CREATING. Poll
/// GetDataset until status transitions to ACTIVE or CREATE_FAILED"
/// (`smithy_query doc shape=CreateDataset`) — and AWS documents no duration for
/// the transition, so the span is chosen: long enough that a caller polling
/// straight after the call still observes `CREATING`, short enough to keep a
/// client script quick.
pub const DATASET_SETTLE_DELAY: Duration = Duration::from_secs(3);

/// How many datasets this emulator will hold.
///
/// `CreateDataset` models `ServiceQuotaExceededException` (HTTP 402) for
/// exceeding the real service's quota; this number is not a fidelity claim about
/// that quota but the emulator's own ceiling, set generously so no realistic
/// workflow meets it, so a caller looping on create cannot grow this process's
/// memory without bound.
pub const MAX_DATASETS: usize = 1_000;

/// How many examples one ingestion will yield into a dataset.
///
/// An INLINE source is already bounded by the model — `DatasetExampleList` is
/// `length 1..1000` (`smithy_query shape=InlineExamplesSource`) — and the same
/// ceiling is applied to the examples read out of a JSONL file, so neither
/// source can grow this process's memory without bound.
pub const MAX_DATASET_EXAMPLES: usize = 1_000;

/// How many bytes of a caller's JSONL file this emulator reads, beside
/// [`MAX_DATASET_EXAMPLES`] and for the same reason.
pub const MAX_DATASET_FILE_BYTES: i64 = 32 * 1024 * 1024;

/// How deeply nested one example read out of a JSONL file may be.
///
/// The bound is this module's own: it keeps the conversion below from being
/// driven as deep as a caller's file asks. It is measured with an explicit
/// stack rather than by recursion, so measuring cannot itself overflow.
pub const MAX_EXAMPLE_DEPTH: usize = 64;

/// `arn:aws:bedrock-agentcore:{region}:{account}:dataset/{datasetId}` —
/// `DatasetArn`'s pattern.
pub fn dataset_arn(region: &str, dataset_id: &str) -> String {
    ::engine::emulator_arn(
        super::ARN_SERVICE,
        region,
        &format!("{ARN_TYPE_DATASET}/{dataset_id}"),
    )
}

/// What ingesting a dataset's source established.
#[derive(Debug, Clone)]
pub enum DatasetIngestion {
    /// The examples the source yielded, in the order the source lists them.
    Ingested(Vec<Document>),
    /// Nothing could be ingested. The sentence is what the dataset reports as
    /// its `failureReason` once the clock owes it `CREATE_FAILED`.
    Failed(String),
}

/// The examples the `source` of a create yields, read where they live.
///
/// An inline source carries them in the request. An Amazon S3 source names a
/// JSONL file in the caller's own bucket, which is READ out of this emulator's
/// own object storage over the in-process callback, so the examples a dataset
/// holds are the ones the file really holds. A file object storage holds nothing
/// for — or one that is not a JSONL file of examples — is a failed ingestion:
/// "Initial ingestion failed. DRAFT record exists but contains no examples"
/// (`smithy_query shape=DatasetStatus`).
pub async fn ingest_dataset_source(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    source: &DataSourceType,
) -> DatasetIngestion {
    match source {
        DataSourceType::inlineExamples(inline) => {
            if inline.examples.len() > MAX_DATASET_EXAMPLES {
                return DatasetIngestion::Failed(too_many_examples_reason());
            }
            DatasetIngestion::Ingested(inline.examples.clone())
        }
        DataSourceType::s3Source(s3) => ingest_s3_file(ctx, region, &s3.s3_uri).await,
    }
}

/// The examples the JSONL file at `uri` holds.
async fn ingest_s3_file(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    uri: &str,
) -> DatasetIngestion {
    let Some((bucket, key)) = object_address(uri) else {
        return DatasetIngestion::Failed(unreadable_reason(uri));
    };
    let answer = super::s3_client(ctx, region)
        .get_object()
        .bucket(&bucket)
        .key(&key)
        .send()
        .await;
    let object = match answer {
        Ok(object) => {
            // The size of the file, bounded BEFORE its bytes are collected: what
            // object storage reports for the object is what reading it would
            // allocate.
            if object
                .content_length()
                .is_some_and(|length| length > MAX_DATASET_FILE_BYTES)
            {
                return DatasetIngestion::Failed(oversized_reason(uri));
            }
            object
        }
        Err(error) => {
            if !names_nothing(&error) {
                // The answer says nothing about the caller's object — an
                // emulator assembled without object storage answers the
                // router's own `UnknownService`, and a transport failure answers
                // nothing at all — so it is worth a warning. The caller's own
                // values are rendered in their escaped `Debug` form, so a key
                // carrying a newline cannot forge a log line.
                ::tracing::warn!(
                    bucket = ?bucket,
                    key = ?key,
                    detail = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the dataset file could not be read from this emulator's object storage, so \
                     no examples were ingested",
                );
            }
            // Either way nothing was ingested, so the dataset reports the
            // ingestion as failed rather than an empty success.
            return DatasetIngestion::Failed(unreadable_reason(uri));
        }
    };
    let bytes = match object.body.collect().await {
        Ok(body) => {
            let bytes = body.into_bytes();
            if i64::try_from(bytes.len()).unwrap_or(i64::MAX) > MAX_DATASET_FILE_BYTES {
                return DatasetIngestion::Failed(oversized_reason(uri));
            }
            bytes
        }
        Err(error) => {
            ::tracing::warn!(
                bucket = ?bucket,
                key = ?key,
                detail = %error,
                "the dataset file could not be read from this emulator's object storage, so no \
                 examples were ingested",
            );
            return DatasetIngestion::Failed(unreadable_reason(uri));
        }
    };
    let Ok(text) = ::std::str::from_utf8(&bytes) else {
        return DatasetIngestion::Failed(not_jsonl_reason(uri));
    };
    parse_jsonl_examples(uri, text)
}

/// The examples a JSONL file holds: one example per non-blank line, in the order
/// the file lists them, each line a JSON value — `DatasetExampleList`'s member
/// is a `document` (`smithy_query shape=DatasetExampleList`), so an example is
/// whatever JSON the schema type gives it.
fn parse_jsonl_examples(uri: &str, text: &str) -> DatasetIngestion {
    let mut examples: Vec<Document> = Vec::new();
    for (index, line) in text
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
    {
        // The line NUMBER a reader counts, so the sentence names the line of the
        // file.
        let number = index + 1;
        if examples.len() >= MAX_DATASET_EXAMPLES {
            return DatasetIngestion::Failed(too_many_examples_reason());
        }
        let Ok(value) = ::serde_json::from_str::<Value>(line) else {
            return DatasetIngestion::Failed(format!(
                "Line {number} of the dataset file at {uri} is not JSON. Each example must be a \
                 JSON value on its own line."
            ));
        };
        if depth_exceeded(&value) {
            return DatasetIngestion::Failed(format!(
                "Line {number} of the dataset file at {uri} is nested deeper than the \
                 {MAX_EXAMPLE_DEPTH} levels an ingestion reads. Flatten the example and create \
                 the dataset again."
            ));
        }
        examples.push(json_to_document(&value));
    }
    if examples.is_empty() {
        return DatasetIngestion::Failed(format!(
            "The dataset file at {uri} holds no examples. Each example must be a JSON value on \
             its own line."
        ));
    }
    DatasetIngestion::Ingested(examples)
}

/// Why a file this emulator could not read ingested nothing.
fn unreadable_reason(uri: &str) -> String {
    format!(
        "The dataset file at {uri} could not be read, so no examples were ingested. Make sure \
         that the object exists and that the service has access to it, then create the dataset \
         again."
    )
}

/// Why a file larger than this emulator reads ingested nothing.
fn oversized_reason(uri: &str) -> String {
    format!(
        "The dataset file at {uri} is larger than the {MAX_DATASET_FILE_BYTES} bytes an \
         ingestion reads. Split the file and create the dataset again."
    )
}

/// Why a file that is not JSONL text ingested nothing.
fn not_jsonl_reason(uri: &str) -> String {
    format!(
        "The dataset file at {uri} is not a JSONL file: it is not UTF-8 text. Each example must \
         be a JSON value on its own line."
    )
}

/// Why a source carrying more examples than one ingestion yields ingested
/// nothing.
fn too_many_examples_reason() -> String {
    format!(
        "A dataset holds at most {MAX_DATASET_EXAMPLES} examples. Split the examples across \
         datasets and create the dataset again."
    )
}

/// The bucket and key an `S3Uri` names. The member's own pattern is
/// `^s3://[a-z0-9][a-z0-9.\-]{1,61}[a-z0-9]/.{1,1024}$` (`smithy_query
/// shape=S3Source`), so a value that reached a handler names a bucket and a
/// non-empty key under it; a value that does not is read as naming no object.
fn object_address(uri: &str) -> Option<(String, String)> {
    let (bucket, key) = uri.strip_prefix("s3://")?.split_once('/')?;
    (!bucket.is_empty() && !key.is_empty()).then(|| (bucket.to_string(), key.to_string()))
}

/// Whether object storage's answer is object storage ITSELF saying it holds
/// nothing at the address the read named: the status and the codes it sends for
/// a missing bucket, key or version.
fn names_nothing<E>(
    error: &::aws_sdk_s3::error::SdkError<
        E,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool
where
    E: ::aws_smithy_types::error::metadata::ProvideErrorMetadata,
{
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    error
        .raw_response()
        .map(|response| response.status().as_u16())
        == Some(super::OBJECT_ABSENT_STATUS)
        && super::ABSENT_OBJECT_CODES.contains(&error.code().unwrap_or_default())
}

/// Whether `value` nests deeper than [`MAX_EXAMPLE_DEPTH`], walked with an
/// explicit stack so the measurement itself cannot overflow.
fn depth_exceeded(value: &Value) -> bool {
    let mut pending: Vec<(&Value, usize)> = vec![(value, 1)];
    while let Some((current, depth)) = pending.pop() {
        if depth > MAX_EXAMPLE_DEPTH {
            return true;
        }
        match current {
            Value::Array(items) => pending.extend(items.iter().map(|item| (item, depth + 1))),
            Value::Object(members) => {
                pending.extend(members.values().map(|member| (member, depth + 1)));
            }
            _ => {}
        }
    }
    false
}

/// One JSON value as the `document` an example is modelled as. The caller has
/// already established that the value nests no deeper than
/// [`MAX_EXAMPLE_DEPTH`], which bounds this walk.
fn json_to_document(value: &Value) -> Document {
    match value {
        Value::Null => Document::Null,
        Value::Bool(flag) => Document::Bool(*flag),
        Value::Number(number) => Document::Number(json_number(number)),
        Value::String(text) => Document::String(text.clone()),
        Value::Array(items) => Document::Array(items.iter().map(json_to_document).collect()),
        Value::Object(members) => Document::Object(
            members
                .iter()
                .map(|(name, member)| (name.clone(), json_to_document(member)))
                .collect(),
        ),
    }
}

/// One JSON number as the document number form that holds it without loss: an
/// unsigned integer, a signed one, or a float for everything else.
fn json_number(number: &::serde_json::Number) -> ::aws_smithy_types::Number {
    use ::aws_smithy_types::Number;

    if let Some(value) = number.as_u64() {
        Number::PosInt(value)
    } else if let Some(value) = number.as_i64() {
        Number::NegInt(value)
    } else {
        Number::Float(number.as_f64().unwrap_or_default())
    }
}

/// One dataset: the configuration `CreateDataset` persisted, the examples its
/// source yielded into the DRAFT working copy, and the transition out of
/// `CREATING` the clock owes it.
///
/// Every read of the dataset reports this record, so a create and a later read
/// cannot disagree about its name, its configuration, how many examples it holds
/// or its status.
#[derive(Debug, Clone)]
pub struct DatasetRecord {
    pub dataset_id: String,
    pub dataset_arn: String,
    pub dataset_name: String,
    pub description: Option<String>,
    /// Immutable after creation, as the model documents the member.
    pub schema_type: String,
    pub kms_key_arn: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// The DRAFT working copy's examples, in the order the source listed them.
    pub draft_examples: Vec<Document>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The status the dataset reports until [`Self::settles_at`].
    pub transient_status: String,
    /// The status it reports from then on: what its ingestion earned.
    pub settled_status: String,
    /// When the clock owes the move from one to the other. Stored, never slept
    /// on: a read materialises the transition the clock already made due.
    pub settles_at: SystemTime,
    /// Why the settled status is a failure state, for a dataset whose ingestion
    /// failed; `None` for one that ingested its source.
    pub failure_reason: Option<String>,
}

impl DatasetRecord {
    /// The dataset a create stores for the request `requested` in `region`,
    /// under the id that create minted, holding what `ingestion` yielded.
    ///
    /// A new dataset is `CREATING`, not yet settled: the transition to the
    /// status its ingestion earned — `ACTIVE`, or `CREATE_FAILED` for a source
    /// that yielded nothing — is owed by the clock rather than by this call.
    pub fn created(
        region: &str,
        dataset_id: String,
        requested: CreateDatasetInput,
        ingestion: DatasetIngestion,
        now: SystemTime,
    ) -> Self {
        let (draft_examples, settled_status, failure_reason) = match ingestion {
            DatasetIngestion::Ingested(examples) => {
                (examples, DATASET_STATUS_ACTIVE.to_string(), None)
            }
            DatasetIngestion::Failed(reason) => (
                // "DRAFT record exists but contains no examples."
                Vec::new(),
                DATASET_STATUS_CREATE_FAILED.to_string(),
                Some(reason),
            ),
        };
        Self {
            dataset_arn: dataset_arn(region, &dataset_id),
            dataset_id,
            dataset_name: requested.dataset_name,
            description: requested.description,
            schema_type: requested.schema_type,
            kms_key_arn: requested.kms_key_arn,
            tags: sorted_map(requested.tags),
            draft_examples,
            created_at: now,
            updated_at: now,
            transient_status: DATASET_STATUS_CREATING.to_string(),
            settled_status,
            settles_at: now + DATASET_SETTLE_DELAY,
            failure_reason,
        }
    }

    /// The status at `now`: the transient one until the clock owes the
    /// transition, the settled one from then on. No read performs it — it is due
    /// at that instant whether or not anybody looks.
    pub fn status_at(&self, now: SystemTime) -> &str {
        if now < self.settles_at {
            &self.transient_status
        } else {
            &self.settled_status
        }
    }

    /// Why the dataset is in its current status, reported only once that status
    /// IS the failure state the clock owed it: a dataset still `CREATING` has
    /// nothing to report yet.
    pub fn failure_reason_at(&self, now: SystemTime) -> Option<String> {
        if now < self.settles_at {
            return None;
        }
        self.failure_reason.clone()
    }

    /// How many examples the DRAFT holds, as the `Long` the responses declare.
    pub fn example_count(&self) -> i64 {
        i64::try_from(self.draft_examples.len()).unwrap_or(i64::MAX)
    }

    /// The tags as a response reports them: the map the create carried, or no
    /// member at all for a dataset carrying none, so an untagged dataset is not
    /// reported as one holding an empty tag set.
    fn reported_tags(&self) -> Option<HashMap<String, String>> {
        if self.tags.is_empty() {
            return None;
        }
        Some(
            self.tags
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        )
    }

    /// What the create of this dataset reports: the four members
    /// `CreateDatasetResponse` declares, every one of them off the record just
    /// written.
    pub fn create_output(&self, now: SystemTime) -> CreateDatasetOutput {
        CreateDatasetOutput {
            dataset_arn: self.dataset_arn.clone(),
            dataset_id: self.dataset_id.clone(),
            status: self.status_at(now).to_string(),
            created_at: timestamp(self.created_at),
        }
    }

    /// What a read of this dataset's DRAFT reports.
    pub fn get_output(&self, now: SystemTime) -> GetDatasetOutput {
        GetDatasetOutput {
            dataset_arn: self.dataset_arn.clone(),
            dataset_id: self.dataset_id.clone(),
            dataset_version: DATASET_VERSION_DRAFT.to_string(),
            dataset_name: self.dataset_name.clone(),
            description: self.description.clone(),
            status: self.status_at(now).to_string(),
            // The DRAFT's publish synchronization state: this dataset holds no
            // published version, and `MODIFIED` is what a DRAFT with "no
            // versions … published yet" reports.
            draft_status: Some(DRAFT_STATUS_MODIFIED.to_string()),
            failure_reason: self.failure_reason_at(now),
            schema_type: self.schema_type.clone(),
            kms_key_arn: self.kms_key_arn.clone(),
            example_count: self.example_count(),
            // "Presigned Amazon S3 URL to download the consolidated dataset
            // file for the resolved version. … Omitted if the file does not yet
            // exist": this emulator consolidates no file into object storage, so
            // there is no URL to publish and none is invented.
            download_url: None,
            download_url_expires_at: None,
            created_at: timestamp(self.created_at),
            updated_at: timestamp(self.updated_at),
            tags: self.reported_tags(),
        }
    }
}

impl super::ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState {
    /// Mint a `datasetId` for `name` that no stored dataset holds: the name +
    /// `-` + 10 alphanumerics, which `DatasetId`'s pattern
    /// `^[a-zA-Z0-9_-]{1,110}$` admits for every `DatasetName`
    /// (`^[a-zA-Z][a-zA-Z0-9_]{0,47}$`) a create can carry.
    pub fn mint_dataset_id(&mut self, name: &str, at: SystemTime) -> String {
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = format!("{name}-{}", super::suffix_for(name, self.mint_counter, at));
            if !self.datasets.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// The dataset a previous request to `operation` carrying this `clientToken`
    /// produced, if the record is still held and the dataset still exists.
    pub fn replayed_dataset_result(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<String> {
        let dataset_id = self
            .dataset_idempotency
            .get(&super::token_key(operation, client_token?))?;
        self.datasets
            .contains_key(dataset_id)
            .then(|| dataset_id.clone())
    }

    /// Record what a `clientToken` produced for `operation`, evicting the oldest
    /// record once `MAX_REMEMBERED_CLIENT_TOKENS` are held. An evicted token is
    /// indistinguishable from one whose idempotency window has passed: the next
    /// request carrying it is served as a fresh one.
    pub fn remember_dataset_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        dataset_id: &str,
    ) {
        let Some(token) = client_token else {
            return;
        };
        super::remember_token(
            &mut self.dataset_idempotency,
            &mut self.dataset_idempotency_order,
            super::token_key(operation, &token),
            dataset_id.to_string(),
        );
    }
}
