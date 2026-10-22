// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-marketplacecommerceanalytics-marketplacecommerceanalytics20150701 service.
//!
//! Marketplace Commerce Analytics has no reporting operation: both modelled
//! operations (`GenerateDataSet` and `StartSupportDataExport`) accept a
//! publication request, mint a request id, and report progress out-of-band
//! through S3 and SNS. So the record below is the whole observable surface of
//! the service inside the emulator: the request exactly as it was accepted, and
//! the artifact the asynchronous publication would write for it.

use std::collections::{BTreeMap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::{DateTime, Format};
use sha2::{Digest, Sha256};

/// Which modelled operation accepted a request.
///
/// The two operations differ in the date member they take (`dataSetPublicationDate`
/// versus `fromDate`) and in the file name their publication writes, so the
/// record keeps the distinction rather than flattening it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestKind {
    /// `GenerateDataSet`.
    GenerateDataSet,
    /// `StartSupportDataExport` (`@deprecated` in the model, still served).
    StartSupportDataExport,
}

/// One accepted data-set publication request, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct DataSetRequest {
    /// The value returned to the caller as `dataSetRequestId`.
    pub data_set_request_id: String,
    /// The operation that accepted this request.
    pub kind: RequestKind,
    /// A `DataSetType` value for `GenerateDataSet`, a `SupportDataSetType`
    /// value for `StartSupportDataExport`.
    pub data_set_type: String,
    /// `GenerateDataSetRequest$dataSetPublicationDate`; `None` for a support export.
    pub publication_date: Option<DateTime>,
    /// `StartSupportDataExportRequest$fromDate`; `None` for a data set.
    pub from_date: Option<DateTime>,
    pub role_name_arn: String,
    pub destination_s3_bucket_name: String,
    /// Optional: absent means the artifact is published to the bucket root.
    pub destination_s3_prefix: Option<String>,
    pub sns_topic_arn: String,
    /// Stored verbatim: the model documents these as "returned, unmodified, in
    /// the Amazon SNS notification message and the data set metadata file".
    pub customer_defined_values: BTreeMap<String, String>,
    /// When the emulator accepted the request.
    pub created_at: SystemTime,
}

/// The CSV object an accepted request's asynchronous publication writes, plus
/// the notification it carries.
///
/// Keyed in [`ComAmazonawsMarketplacecommerceanalyticsMarketplacecommerceanalytics20150701State::published`]
/// by [`artifact_key`], so re-requesting the same data set for the same
/// destination overwrites it — "If a file with the same name already exists
/// (e.g. if the same data set is requested twice), the original file will be
/// overwritten by the new file."
#[derive(Debug, Clone)]
pub struct PublishedArtifact {
    /// The object key inside the bucket: `{prefix/}{objectName}`.
    pub s3_key: String,
    /// `destinationS3BucketName`, the friendly bucket name.
    pub bucket: String,
    /// The topic notified once the object is available.
    pub sns_topic_arn: String,
    /// Echoed, unmodified, into the notification and the metadata file.
    pub customer_defined_values: BTreeMap<String, String>,
    /// The id of the most recent request that wrote this object.
    pub last_request_id: String,
}

/// Per-service state for com-amazonaws-marketplacecommerceanalytics-marketplacecommerceanalytics20150701. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMarketplacecommerceanalyticsMarketplacecommerceanalytics20150701State {
    /// Every accepted request, keyed by the `dataSetRequestId` returned for it.
    pub requests: HashMap<String, DataSetRequest>,
    /// The ids in the order they were accepted, so the newest request is
    /// identifiable without a timestamp comparison.
    pub request_order: Vec<String>,
    /// The artifact each destination object currently holds, keyed by
    /// [`artifact_key`].
    pub published: HashMap<String, PublishedArtifact>,
    /// How many requests this store has ever minted an id for. It only ever
    /// grows, so it is what makes a minted id unique inside this process; see
    /// [`Self::mint_request_id`].
    pub minted_ordinal: u64,
}

impl ComAmazonawsMarketplacecommerceanalyticsMarketplacecommerceanalytics20150701State {
    /// Persist `request` and the artifact its publication writes, and answer the
    /// `dataSetRequestId` the caller gets back.
    ///
    /// The artifact is written under the key the data set type, the date and the
    /// destination derive, so a second request for the same object replaces the
    /// first one's artifact while still being recorded as its own request.
    ///
    /// Both stores are bounded by [`MAX_RECORDS_PER_STORE`] and checked BEFORE
    /// anything is written, so a refused request leaves the state exactly as it
    /// was: every accepted request adds a record no operation can delete (the
    /// service models none), and `published` is keyed on the caller-supplied
    /// bucket and prefix, so a caller chooses how many distinct keys it asks
    /// for. `Err` carries the message to refuse with.
    pub fn accept(
        &mut self,
        request: DataSetRequest,
        object_name: String,
    ) -> Result<String, String> {
        let id = request.data_set_request_id.clone();
        let s3_key = object_key(request.destination_s3_prefix.as_deref(), &object_name);
        let key = artifact_key(&request.destination_s3_bucket_name, &s3_key);

        if self.requests.len() >= MAX_RECORDS_PER_STORE {
            return Err(limit_exceeded_message("accepted data set requests"));
        }
        // Re-requesting an object the store already holds overwrites that one
        // artifact, so only a NEW destination object grows this store.
        if !self.published.contains_key(&key) && self.published.len() >= MAX_RECORDS_PER_STORE {
            return Err(limit_exceeded_message("published data set objects"));
        }

        self.published.insert(
            key,
            PublishedArtifact {
                s3_key,
                bucket: request.destination_s3_bucket_name.clone(),
                sns_topic_arn: request.sns_topic_arn.clone(),
                customer_defined_values: request.customer_defined_values.clone(),
                last_request_id: id.clone(),
            },
        );
        self.requests.insert(id.clone(), request);
        self.request_order.push(id.clone());
        Ok(id)
    }

    /// The id of the most recently accepted request, if any.
    pub fn newest_request_id(&self) -> Option<&str> {
        self.request_order.last().map(String::as_str)
    }

    /// Mint a fresh `dataSetRequestId`.
    ///
    /// The model documents it as "a unique identifier representing a specific
    /// request ... used to correlate a request with notifications from the SNS
    /// topic": it names the CALL, not the data set, so two identical calls must
    /// get two different ids. The id is a correlation handle and not a secret,
    /// so it is DERIVED rather than drawn from a random source, from two values
    /// that cannot repeat together: this store's ever-growing ordinal, which
    /// separates two calls inside one process, and the instant of the call,
    /// which separates two processes. SHA-256 (the `sha2` crate) spreads them
    /// over 16 bytes, rendered as a UUID whose version nibble is 8 — RFC 9562's
    /// version for a custom, implementation-defined layout, which is what this
    /// is.
    pub fn mint_request_id(&mut self) -> String {
        self.minted_ordinal += 1;
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since_epoch| since_epoch.as_nanos())
            .unwrap_or_default();
        request_id(nanos, self.minted_ordinal)
    }
}

/// The `dataSetRequestId` for the `ordinal`-th id a store mints, at `nanos`
/// since the epoch. Split out of `mint_request_id` so the rendering is
/// exercisable without a clock.
fn request_id(nanos: u128, ordinal: u64) -> String {
    let digest = Sha256::digest(format!("{nanos}:{ordinal}").as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 9562 fixes two fields in every UUID layout: the version (the high
    // nibble of octet 6) and the two-bit variant (the high bits of octet 8).
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// The maximum number of records this emulator keeps in ANY ONE of its
/// caller-driven stores (accepted requests, published destination objects).
///
/// This is NOT an AWS quota: Marketplace Commerce Analytics publishes none, and
/// nothing here claims to reproduce one. It is the emulator's own memory
/// ceiling. Every accepted request adds a record that no modelled operation can
/// delete — the service models only the two accept operations — and `published`
/// is keyed on the caller-supplied bucket and prefix, so without a ceiling a
/// caller could grow the process until the host runs out of memory. A request
/// that would cross it is refused with the service's only modelled fault,
/// `MarketplaceCommerceAnalyticsException`.
pub const MAX_RECORDS_PER_STORE: usize = 1_000;

/// The message a store at [`MAX_RECORDS_PER_STORE`] refuses a new record with;
/// `store` names the records it holds, e.g. `"accepted data set requests"`.
pub fn limit_exceeded_message(store: &str) -> String {
    format!(
        "this emulator holds at most {MAX_RECORDS_PER_STORE} {store} \
         and cannot accept another request"
    )
}

/// The longest object key S3 will store: "The name for a key is a sequence of
/// Unicode characters whose UTF-8 encoding is at most 1,024 bytes long"
/// (Amazon S3 User Guide, object key naming guidelines). `destinationS3Prefix`
/// carries no modelled length constraint, so a prefix long enough to push the
/// published object's key past this bound names an object the publication could
/// never write — and bounds the size of one stored record besides.
pub const MAX_S3_KEY_BYTES: usize = 1024;

/// The object key an artifact is published under inside its bucket.
///
/// `destinationS3Prefix` is documented as "similar to a directory path in
/// standard file systems" — bucket `mybucket` with prefix `myprefix/mydatasets`
/// publishes `outputfile` to `s3://mybucket/myprefix/mydatasets/outputfile` —
/// and "if no prefix is provided, the data set will be published to the S3
/// bucket root". Surrounding slashes are not part of the path, so they are
/// trimmed before joining.
pub fn object_key(prefix: Option<&str>, object_name: &str) -> String {
    match prefix.map(|p| p.trim_matches('/')).filter(|p| !p.is_empty()) {
        Some(prefix) => format!("{prefix}/{object_name}"),
        None => object_name.to_string(),
    }
}

/// Why a destination object's key is one S3 could not store, or `None` when the
/// key fits inside [`MAX_S3_KEY_BYTES`].
///
/// The caller names the member at fault and the bound it crossed, the way the
/// boundary validator words a refusal.
pub fn oversized_key_message(prefix: Option<&str>, object_name: &str) -> Option<String> {
    let key = object_key(prefix, object_name);
    (key.len() > MAX_S3_KEY_BYTES).then(|| {
        format!(
            "destinationS3Prefix makes the published object key {} bytes long; \
             an S3 object key is at most {MAX_S3_KEY_BYTES} bytes",
            key.len()
        )
    })
}

/// Why `bucket` is not a name S3 could hold an object under, or `None` when it
/// is usable.
///
/// The model documents the member as "the name (friendly name, not ARN) of the
/// destination S3 bucket" and bounds it only by length, so the naming rules are
/// the service's, not the model's. Amazon S3's bucket naming rules (Amazon S3
/// User Guide): a name "can consist only of lowercase letters, numbers, dots
/// (.), and hyphens (-)", "must begin and end with a letter or number", "must
/// not contain two adjacent periods" and "must not be formatted as an IP
/// address (for example, 192.168.5.4)". A name with a slash fails the character
/// rule, which is also what keeps [`artifact_key`] unambiguous: with no slash in
/// a bucket name, one `{bucket}/{key}` string names exactly one destination
/// object.
///
/// The LENGTH rules are deliberately not applied: the model bounds the member at
/// 1..63 and a handler must not refuse input the model accepts.
pub fn malformed_bucket_name_message(bucket: &str) -> Option<String> {
    let rule = if !bucket
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-')
    {
        "a bucket name holds only lowercase letters, numbers, dots (.) and hyphens (-)"
    } else if !bucket.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        || !bucket.ends_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        "a bucket name begins and ends with a letter or a number"
    } else if bucket.contains("..") {
        "a bucket name holds no two adjacent periods"
    } else if is_ipv4_shaped(bucket) {
        "a bucket name is not formatted as an IP address"
    } else {
        return None;
    };
    Some(format!(
        "destinationS3BucketName {bucket:?} is not an S3 bucket name: {rule}"
    ))
}

/// Whether `candidate` is four dot-separated decimal octets, the form S3's
/// bucket naming rules refuse ("must not be formatted as an IP address (for
/// example, 192.168.5.4)").
fn is_ipv4_shaped(candidate: &str) -> bool {
    let octets: Vec<&str> = candidate.split('.').collect();
    octets.len() == 4
        && octets.iter().all(|octet| {
            !octet.is_empty()
                && octet.len() <= 3
                && octet.chars().all(|c| c.is_ascii_digit())
                && octet.parse::<u16>().is_ok_and(|value| value <= 255)
        })
}

/// The key [`ComAmazonawsMarketplacecommerceanalyticsMarketplacecommerceanalytics20150701State::published`]
/// holds an artifact under: the destination object, bucket included, since the
/// same object name in two buckets is two different files.
///
/// Unambiguous because a bucket name carries no slash (see
/// [`malformed_bucket_name_message`]), so the first path segment is always the
/// bucket and the rest is always the key.
pub fn artifact_key(bucket: &str, s3_key: &str) -> String {
    format!("{bucket}/{s3_key}")
}

/// The UTC date (`YYYY-MM-DD`) and time (`HH:MM:SS`) of `when`, or `None` when
/// the instant cannot be rendered as a calendar date at all.
///
/// `Format::DateTime` is RFC3339, so the rendering is
/// `YYYY-MM-DDTHH:MM:SS[.fff]Z`; the publication file names carry no subsecond
/// component, so it is dropped.
fn utc_parts(when: &DateTime) -> Option<(String, String)> {
    let rendered = when.fmt(Format::DateTime).ok()?;
    let (date, rest) = rendered.split_once('T')?;
    let time = rest.trim_end_matches('Z');
    let time = time.split_once('.').map_or(time, |(seconds, _)| seconds);
    Some((date.to_string(), time.to_string()))
}

/// The CSV file name `GenerateDataSet` publishes.
///
/// The operation documents it as `{data_set_type}_YYYY-MM-DD.csv`, the date
/// being the requested `dataSetPublicationDate`. `None` when that timestamp
/// names no renderable calendar date.
pub fn generated_data_set_object_name(
    data_set_type: &str,
    publication_date: &DateTime,
) -> Option<String> {
    let (date, _) = utc_parts(publication_date)?;
    Some(format!("{data_set_type}_{date}.csv"))
}

/// The CSV file name `StartSupportDataExport` publishes.
///
/// That operation documents a different name from `GenerateDataSet`'s:
/// `{data_set_type}_YYYY-MM-DD'T'HH-mm-ss'Z'.csv`, down to the second, the
/// instant being the requested `fromDate`. `None` when that timestamp names no
/// renderable calendar date.
pub fn support_data_export_object_name(
    data_set_type: &str,
    from_date: &DateTime,
) -> Option<String> {
    let (date, time) = utc_parts(from_date)?;
    Some(format!("{data_set_type}_{date}T{}Z.csv", time.replace(':', "-")))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMarketplacecommerceanalyticsMarketplacecommerceanalytics20150701State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMarketplacecommerceanalyticsMarketplacecommerceanalytics20150701State>)
}
