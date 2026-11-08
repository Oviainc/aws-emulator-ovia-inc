// SPDX-License-Identifier: CC0-1.0

//! The LOG-FILE leg of CloudTrail delivery: a served call written into the bucket of every
//! trail that is logging it, as the gzipped JSON object CloudTrail delivers there.
//!
//! An account has three readers of its management events. Two of them read what CloudTrail
//! holds or publishes: its own `LookupEvents`, over the log [`crate::management_events`]
//! keeps, and a rule on the default event bus, over the fan-out
//! [`crate::api_call_events`] makes. The third reads the trail's BUCKET — which is what an
//! audit pipeline consumes (a table over the log prefix, a SIEM ingestion, log-file
//! validation), and what a client that pointed a trail at a bucket then lists.
//!
//! So a log file hangs off the SAME seam the recording and the fan-out do
//! ([`crate::audited_operation`]): the service that served the call records it, the log
//! answers which trails of the account deliver a call of that kind
//! ([`crate::management_events::ManagementEventLog::record_delivered`]), and what comes back
//! is delivered here — one object per logging trail, into the bucket and under the key
//! prefix that trail was created with. No service states the destination for itself: the
//! trail's own record lives in CloudTrail's store, which projects the destination onto the
//! log ([`crate::management_events::DeliveringTrail`]).
//!
//! Amazon S3 is reached the way any emulated service reaches another: through the in-process
//! callback, with the client this crate's one construction of it builds
//! ([`crate::s3_client`]), so this module names no service crate. Delivery is BEST EFFORT —
//! an emulator with no object store mounted, or a bucket a client has since deleted, has
//! nowhere to deliver, and a caller is never failed because a log file could not be written.

use std::io::Write;
use std::time::SystemTime;

use smithy_server_runtime::EmbeddedEmulator;

/// The first segment of every CloudTrail log-file key, under whatever prefix the trail
/// carries.
pub const LOG_FILE_ROOT_SEGMENT: &str = "AWSLogs";

/// The segment naming the SERVICE whose files a prefix holds, which is also the middle field
/// of a log file's own name.
pub const LOG_FILE_SERVICE_SEGMENT: &str = "CloudTrail";

/// What a log file's name ends with: JSON, gzipped.
pub const LOG_FILE_SUFFIX: &str = ".json.gz";

/// The member a log file's document holds its records in.
pub const RECORDS_MEMBER: &str = "Records";

/// The media type of the object a log file is: a gzip stream.
const GZIP_MEDIA_TYPE: &str = "application/x-gzip";

/// How many characters of the unique field a log file's name ends with — the field that
/// tells two files of the same minute apart.
const UNIQUE_FIELD_LEN: usize = 16;

/// One log file CloudTrail delivers for a call: the records it carries, and the object they
/// are written as.
///
/// Built by the log that recorded the call — so the records are the very documents
/// `LookupEvents` reports and the two cannot disagree — and written by
/// [`DeliveredLogFile::deliver`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveredLogFile {
    /// The bucket the trail delivers into (`Trail$S3BucketName`).
    pub bucket: String,
    /// The object key, as [`log_file_key`] lays it out.
    pub key: String,
    /// The Region the object store is addressed in, which is the Region the call was
    /// recorded in.
    pub region: String,
    /// The records the file carries, each one a CloudTrail event document
    /// ([`crate::management_events::ManagementEvent::to_cloudtrail_event_json`]).
    pub records: Vec<serde_json::Value>,
}

impl DeliveredLogFile {
    /// The object's bytes: the file's JSON document, gzipped — or `None` when the framing
    /// itself failed.
    ///
    /// A log file is `{"Records": [ … ]}` — the member a consumer reads the events out of —
    /// and it is GZIPPED, which is part of what the object is rather than a transport
    /// encoding: a reader fetches the object and decompresses it. Writing into a `Vec`
    /// cannot fail for want of space, so a failure here would be the encoder's own; it
    /// answers `None` rather than a truncated stream, because no object at all is more
    /// honest than one a reader cannot decompress.
    #[must_use]
    pub fn body(&self) -> Option<Vec<u8>> {
        let document = serde_json::json!({ RECORDS_MEMBER: self.records });
        let mut encoder =
            ::flate2::write::GzEncoder::new(Vec::new(), ::flate2::Compression::default());
        encoder.write_all(document.to_string().as_bytes()).ok()?;
        encoder.finish().ok()
    }

    /// Write this log file into the trail's bucket.
    ///
    /// Nothing about the call that produced it changes: a failure is traced and the
    /// operation's own answer stands, for the reason the module docs give.
    pub async fn deliver(self, callback: &EmbeddedEmulator) {
        let Some(body) = self.body() else {
            ::tracing::warn!(
                bucket = %self.bucket,
                key = %self.key,
                "a trail's log file could not be framed, so none was delivered"
            );
            return;
        };
        let client = crate::s3_client::s3_client(callback, &self.region);
        let answered = client
            .put_object()
            .bucket(&self.bucket)
            .key(&self.key)
            .content_type(GZIP_MEDIA_TYPE)
            .body(::aws_sdk_s3::primitives::ByteStream::from(body))
            .send()
            .await;
        if let Err(error) = answered {
            ::tracing::warn!(
                bucket = %self.bucket,
                key = %self.key,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a trail's log file could not be delivered"
            );
        }
    }
}

/// The prefix every log file of one trail lands under, whatever Region's calls it carries:
/// `<key-prefix>/AWSLogs/<account-id>/CloudTrail/`.
///
/// One Region deeper is [`log_file_key_prefix`], which is where the files of THAT Region's
/// calls land; this is the whole area a trail delivers into, which is what a reader asking
/// what a trail has delivered looks at — a trail that logs several Regions delivers under
/// one such area and one subprefix per Region.
#[must_use]
pub fn trail_log_file_prefix(key_prefix: Option<&str>, account_id: &str) -> String {
    let prefix = key_prefix
        .map(|prefix| prefix.trim_matches('/'))
        .filter(|prefix| !prefix.is_empty())
        .map(|prefix| format!("{prefix}/"))
        .unwrap_or_default();
    format!("{prefix}{LOG_FILE_ROOT_SEGMENT}/{account_id}/{LOG_FILE_SERVICE_SEGMENT}/")
}

/// The prefix EVERY log file of a trail is delivered under:
/// `<key-prefix>/AWSLogs/<account-id>/CloudTrail/<region>/`, with a trailing separator.
///
/// `key_prefix` is what the trail was created with (`Trail$S3KeyPrefix`) — "the Amazon S3
/// key prefix that comes after the name of the bucket you have designated for log file
/// delivery" — which goes ahead of the whole layout; a trail that carries none delivers at
/// the bucket's root. The prefix's own separators are not doubled: a value written with a
/// leading or trailing `/` names the same prefix as one without.
///
/// It is the one place this layout is written, for both readers of it: the delivery that
/// WRITES the keys ([`log_file_key`]) and the permission question CloudTrail asks of a
/// destination bucket's policy before it accepts a trail, which is about the very keys that
/// delivery writes.
#[must_use]
pub fn log_file_key_prefix(key_prefix: Option<&str>, account_id: &str, region: &str) -> String {
    format!("{}{region}/", trail_log_file_prefix(key_prefix, account_id))
}

/// The key a log file takes: the documented layout, under the trail's own key prefix.
///
/// [`log_file_key_prefix`] down to the Region, then the DAY the records belong to, then the
/// file's own name — `<account-id>_CloudTrail_<region>_<YYYYMMDDTHHmm>Z_<unique>.json.gz`:
/// the account, the service, the Region, the MINUTE the records belong to, and a unique
/// field that tells two files of the same minute apart.
#[must_use]
pub fn log_file_key(
    key_prefix: Option<&str>,
    account_id: &str,
    region: &str,
    at: SystemTime,
    unique: &str,
) -> String {
    let at = ::chrono::DateTime::<::chrono::Utc>::from(at);
    let day = at.format("%Y/%m/%d");
    let minute = at.format("%Y%m%dT%H%MZ");
    let prefix = log_file_key_prefix(key_prefix, account_id, region);
    format!(
        "{prefix}{day}/\
         {account_id}_{LOG_FILE_SERVICE_SEGMENT}_{region}_{minute}_{unique}{LOG_FILE_SUFFIX}"
    )
}

/// The unique field one log file's name ends with: [`UNIQUE_FIELD_LEN`] characters drawn
/// fresh, so two files delivered in the same minute to the same trail are two objects and
/// not one overwriting the other.
///
/// Drawn from the same maintained generator the log's own event ids come from, never derived
/// from the request or from the records: the field identifies the FILE and nothing about its
/// contents.
#[must_use]
pub fn unique_field() -> String {
    ::uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(UNIQUE_FIELD_LEN)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    /// An instant with a known UTC calendar rendering: 2026-11-02T10:23:43Z.
    fn at() -> SystemTime {
        ::chrono::DateTime::parse_from_rfc3339("2026-11-02T10:23:43Z")
            .expect("the fixture instant is a well-formed timestamp")
            .into()
    }

    /// RULE: the key is the documented layout — the log-file prefix down to the DAY, and a
    /// file name carrying the account, the service, the Region and the MINUTE — with the
    /// trail's own key prefix ahead of it and nothing else changed.
    #[test]
    fn a_log_file_key_carries_the_documented_layout() {
        let rows = [
            (
                "no key prefix",
                None,
                "AWSLogs/000000000000/CloudTrail/us-east-1/2026/11/02/\
                 000000000000_CloudTrail_us-east-1_20261102T1023Z_abcdef0123456789.json.gz",
            ),
            (
                "a one-segment prefix",
                Some("audit"),
                "audit/AWSLogs/000000000000/CloudTrail/us-east-1/2026/11/02/\
                 000000000000_CloudTrail_us-east-1_20261102T1023Z_abcdef0123456789.json.gz",
            ),
            (
                "a nested prefix written with separators around it",
                Some("/logs/prod/"),
                "logs/prod/AWSLogs/000000000000/CloudTrail/us-east-1/2026/11/02/\
                 000000000000_CloudTrail_us-east-1_20261102T1023Z_abcdef0123456789.json.gz",
            ),
            (
                "an empty prefix, which names none",
                Some(""),
                "AWSLogs/000000000000/CloudTrail/us-east-1/2026/11/02/\
                 000000000000_CloudTrail_us-east-1_20261102T1023Z_abcdef0123456789.json.gz",
            ),
        ];
        for (case, key_prefix, expected) in rows {
            let actual = log_file_key(
                key_prefix,
                "000000000000",
                "us-east-1",
                at(),
                "abcdef0123456789",
            );
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the object a log file is, is a gzip stream whose JSON holds the records under
    /// `Records`.
    #[test]
    fn a_log_file_body_is_gzipped_json_holding_its_records() {
        for count in [0_usize, 1, 3] {
            let records: Vec<serde_json::Value> = (0..count)
                .map(|index| serde_json::json!({"eventName": format!("Call{index}")}))
                .collect();
            let file = DeliveredLogFile {
                bucket: "trail-logs".to_string(),
                key: "k".to_string(),
                region: "us-east-1".to_string(),
                records: records.clone(),
            };
            let body = file
                .body()
                .unwrap_or_else(|| panic!("case {count} records: the file must have a body"));
            let mut decoded = String::new();
            ::flate2::read::GzDecoder::new(body.as_slice())
                .read_to_string(&mut decoded)
                .unwrap_or_else(|error| panic!("case {count} records: not a gzip stream: {error}"));
            let document: serde_json::Value = serde_json::from_str(&decoded)
                .unwrap_or_else(|error| panic!("case {count} records: not JSON: {error}"));
            let actual = document.get(RECORDS_MEMBER).cloned().unwrap_or_default();
            assert_eq!(
                actual,
                serde_json::Value::Array(records),
                "case {count} records: expected them under {RECORDS_MEMBER}, got {actual}"
            );
        }
    }

    /// RULE: a unique field is drawn fresh, so two files of one minute do not collide.
    #[test]
    fn unique_fields_are_drawn_fresh() {
        let first = unique_field();
        let second = unique_field();
        assert_eq!(
            (first.len(), second.len()),
            (UNIQUE_FIELD_LEN, UNIQUE_FIELD_LEN),
            "case two fields: expected {UNIQUE_FIELD_LEN} characters each, got \
             {first:?} and {second:?}"
        );
        assert_ne!(
            first, second,
            "case two fields: expected distinct fields, got {first:?} twice"
        );
    }
}
