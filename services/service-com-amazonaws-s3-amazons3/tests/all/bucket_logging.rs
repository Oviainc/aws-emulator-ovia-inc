// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a general purpose bucket's LOGGING STATUS: `PutBucketLogging`
//! records it and `GetBucketLogging` reports it.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite owns), so what is pinned is the wire behaviour — status, body,
//! error code — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * A bucket's logging status is whatever the last `PutBucketLogging` recorded, and
//!   `GetBucketLogging` "returns the logging status of a bucket" (that operation's own
//!   documentation): the `LoggingEnabled` container is reported back with every member the
//!   put named and no member it did not.
//! * "To enable logging, you use `LoggingEnabled` and its children request elements. To
//!   disable logging, you use an empty `BucketLoggingStatus` request element"
//!   (`PutBucketLogging`'s own documentation): every bucket HAS a logging status, so a
//!   bucket that was never configured — and one an empty element disabled again — answers
//!   the modelled 200 with a `BucketLoggingStatus` carrying no `LoggingEnabled`, not a
//!   not-found.
//! * `GetBucketLogging` is a READ: two reads in a row report the same status, and reading
//!   never enables or disables anything.
//! * "You can have your logs delivered to any bucket that you own, including the same
//!   bucket that is being logged" (`LoggingEnabled$TargetBucket`) and "All logs are saved
//!   to buckets in the same Amazon Web Services Region as the source bucket"
//!   (`PutBucketLogging`): a target this emulator does not hold answers
//!   `InvalidTargetBucketForLogging` (HTTP 400) and one in another Region
//!   `CrossLocationLoggingProhibited` (HTTP 403).
//! * "Only one format, `PartitionedPrefix` or `SimplePrefix`, is allowed"
//!   (`TargetObjectKeyFormat`): a container naming both is refused with `MalformedXML`
//!   (HTTP 400).
//! * "You use the Grantee request element to grant access to other people. The Permissions
//!   request element specifies the kind of access the grantee has to the logs"
//!   (`PutBucketLogging`): a `TargetGrant` is a grant on the delivered logs, named by the
//!   same `Grantee` every other grant in this API uses, so a grant whose grantee's `Type`
//!   and members disagree, which names a group URI outside the predefined set, or which
//!   carries no grantee or no permission at all, is refused with the code and status that
//!   same broken grant gets from `PutBucketAcl`.
//! * A call naming a bucket this emulator does not hold answers `NoSuchBucket` (HTTP 404);
//!   one naming an `ExpectedBucketOwner` that is not the account's answers HTTP 403
//!   `AccessDenied`. Every refused put leaves the stored status exactly as it was.

use quick_xml::events::Event;
use quick_xml::Reader;
use service_com_amazonaws_s3_amazons3::ops::types::{
    BUCKET_LOCATION_CONSTRAINT_VALUES, BUCKET_LOGS_PERMISSION_VALUES, PARTITION_DATE_SOURCE_VALUES,
};
use service_com_amazonaws_s3_amazons3::state::{
    CROSS_LOCATION_LOGGING_PROHIBITED_CODE, CROSS_LOCATION_LOGGING_PROHIBITED_MESSAGE,
    DEFAULT_BUCKET_REGION, EMAIL_GRANTEE_DISCONTINUED_REGIONS, EU_LOCATION_CONSTRAINT_REGION,
    GRANTEE_BY_EMAIL, GRANTEE_CANONICAL_USER, GRANTEE_GROUP, INVALID_ARGUMENT_CODE,
    INVALID_TARGET_BUCKET_FOR_LOGGING_CODE, INVALID_TARGET_BUCKET_FOR_LOGGING_MESSAGE,
    LOG_DELIVERY_GROUP_URI, MALFORMED_ACL_CODE, MALFORMED_XML_CODE, MALFORMED_XML_MESSAGE,
    MAX_ACL_GRANTS, METHOD_NOT_ALLOWED_CODE, NO_SUCH_BUCKET_CODE,
    UNRESOLVABLE_GRANT_BY_EMAIL_CODE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, Answer};

/// The XML namespace botocore puts on an S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The namespace `Grantee$Type` travels in as an XML ATTRIBUTE, the way the ACL suite
/// spells it: the member carries `xmlName` `xsi:type` with `xmlNamespace`
/// `http://www.w3.org/2001/XMLSchema-instance` (`smithy_query(query="shape",
/// shape="TargetGrant")`).
const XSI: &str = "http://www.w3.org/2001/XMLSchema-instance";

/// Which of the two key formats a `TargetObjectKeyFormat` names, since "only one format,
/// PartitionedPrefix or SimplePrefix, is allowed"
/// (`smithy_query(query="shape",shape="TargetObjectKeyFormat")`).
#[derive(Debug, Clone, PartialEq, Eq)]
enum KeyFormat {
    /// `SimplePrefix` is an EMPTY structure: "To specify SimplePrefix format, set
    /// SimplePrefix to {}".
    Simple,
    /// `PartitionedPrefix`, with the `PartitionDateSource` it names (when any).
    Partitioned(Option<String>),
}

/// One `TargetGrant`: who the logs are granted to, named how, and what they may do.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetGrantRow {
    /// The `Grantee$Type` attribute value.
    grantee_type: String,
    /// The element naming the grantee (`URI` for a group, `ID` for a canonical user) and
    /// its value.
    named_by: (String, String),
    /// The `TargetGrant$Permission`, a `BucketLogsPermission` value.
    permission: String,
}

/// A `LoggingEnabled` container, as a request carries it and as a report brings it back.
///
/// The same type on both sides, so a round-trip assertion compares what was sent with what
/// was reported rather than with a hand-spelled expectation. The members are the ones
/// `smithy_query(query="shape",shape="LoggingEnabled")` declares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Logging {
    target_bucket: String,
    target_prefix: String,
    grants: Vec<TargetGrantRow>,
    key_format: Option<KeyFormat>,
}

impl Logging {
    /// The two members `LoggingEnabled` REQUIRES.
    fn to(target_bucket: &str, target_prefix: &str) -> Self {
        Self {
            target_bucket: target_bucket.to_string(),
            target_prefix: target_prefix.to_string(),
            ..Self::default()
        }
    }

    fn with_key_format(mut self, format: KeyFormat) -> Self {
        self.key_format = Some(format);
        self
    }

    fn granting(mut self, grantee_type: &str, element: &str, value: &str, permission: &str) -> Self {
        self.grants.push(TargetGrantRow {
            grantee_type: grantee_type.to_string(),
            named_by: (element.to_string(), value.to_string()),
            permission: permission.to_string(),
        });
        self
    }

    /// The container as XML, in the member order the shape declares.
    ///
    /// `LoggingEnabled$TargetGrants` is NOT `xmlFlattened` and its member carries `xmlName`
    /// `Grant` (`smithy_query(query="shape",shape="TargetGrants")`), so the grants are
    /// `<Grant>` children of a `<TargetGrants>` element.
    fn xml(&self) -> String {
        let mut body = format!("<TargetBucket>{}</TargetBucket>", self.target_bucket);
        if !self.grants.is_empty() {
            body.push_str("<TargetGrants>");
            for grant in &self.grants {
                let (element, value) = &grant.named_by;
                body.push_str(&format!(
                    "<Grant><Grantee xmlns:xsi=\"{XSI}\" xsi:type=\"{}\">\
                     <{element}>{value}</{element}></Grantee>\
                     <Permission>{}</Permission></Grant>",
                    grant.grantee_type, grant.permission
                ));
            }
            body.push_str("</TargetGrants>");
        }
        body.push_str(&format!("<TargetPrefix>{}</TargetPrefix>", self.target_prefix));
        match &self.key_format {
            None => {}
            Some(KeyFormat::Simple) => {
                body.push_str("<TargetObjectKeyFormat><SimplePrefix></SimplePrefix></TargetObjectKeyFormat>");
            }
            Some(KeyFormat::Partitioned(source)) => {
                body.push_str("<TargetObjectKeyFormat><PartitionedPrefix>");
                if let Some(source) = source {
                    body.push_str(&format!(
                        "<PartitionDateSource>{source}</PartitionDateSource>"
                    ));
                }
                body.push_str("</PartitionedPrefix></TargetObjectKeyFormat>");
            }
        }
        body
    }
}

/// The `BucketLoggingStatus` payload enabling `logging`, or — for `None` — the empty
/// element that disables logging again.
fn status_payload(logging: Option<&Logging>) -> String {
    match logging {
        None => format!("<BucketLoggingStatus xmlns=\"{XMLNS}\"></BucketLoggingStatus>"),
        Some(logging) => format!(
            "<BucketLoggingStatus xmlns=\"{XMLNS}\"><LoggingEnabled>{}</LoggingEnabled>\
             </BucketLoggingStatus>",
            logging.xml()
        ),
    }
}

/// `PUT /<bucket>?logging` — `PutBucketLogging`.
async fn put_logging(
    router: &MultiServiceRouter,
    bucket: &str,
    logging: Option<&Logging>,
) -> Answer {
    put_logging_payload(router, bucket, &status_payload(logging), &[]).await
}

/// `PutBucketLogging` with the payload spelled out, for a status no [`Logging`] can
/// express, and with any extra bound headers.
async fn put_logging_payload(
    router: &MultiServiceRouter,
    bucket: &str,
    payload: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(router, "PUT", &format!("/{bucket}?logging"), headers, payload).await
}

/// `GET /<bucket>?logging` — `GetBucketLogging`.
async fn get_logging(router: &MultiServiceRouter, bucket: &str) -> Answer {
    get_logging_as(router, bucket, &[]).await
}

/// `GetBucketLogging` with extra bound headers (the expected bucket owner).
async fn get_logging_as(
    router: &MultiServiceRouter,
    bucket: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(router, "GET", &format!("/{bucket}?logging"), headers, "").await
}

/// The logging status a `GetBucketLogging` body reports: the `LoggingEnabled` it carries,
/// or `None` when the status container carries none.
///
/// Parsed with `quick-xml` (already a dependency of this workspace), the way the
/// replication suite beside this one reads a container, rather than by slicing the body.
fn reported_logging(answer: &Answer) -> Option<Logging> {
    let body = String::from_utf8_lossy(&answer.body).into_owned();
    let mut reader = Reader::from_str(&body);

    let mut logging: Option<Logging> = None;
    let mut path: Vec<String> = Vec::new();
    loop {
        let event = reader
            .read_event()
            .unwrap_or_else(|error| panic!("the response body must be well-formed XML: {error}"));
        // An EMPTY element (`<SimplePrefix/>`) carries no end event, so it must not be
        // pushed onto the path a text value is placed by.
        let is_start = matches!(event, Event::Start(_));
        match event {
            Event::Start(ref start) | Event::Empty(ref start) => {
                let name = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                match name.as_str() {
                    "LoggingEnabled" => logging = Some(Logging::default()),
                    "SimplePrefix" => {
                        if let Some(held) = logging.as_mut() {
                            held.key_format = Some(KeyFormat::Simple);
                        }
                    }
                    "PartitionedPrefix" => {
                        if let Some(held) = logging.as_mut() {
                            held.key_format = Some(KeyFormat::Partitioned(None));
                        }
                    }
                    "Grantee" => {
                        // `Grantee$Type` is an XML ATTRIBUTE, so it is read off the start
                        // tag rather than from a text event.
                        let grantee_type = start
                            .attributes()
                            .flatten()
                            .find(|attribute| attribute.key.as_ref() == b"xsi:type")
                            .map(|attribute| {
                                String::from_utf8_lossy(attribute.value.as_ref()).into_owned()
                            })
                            .unwrap_or_default();
                        if let Some(held) = logging.as_mut() {
                            held.grants.push(TargetGrantRow {
                                grantee_type,
                                named_by: (String::new(), String::new()),
                                permission: String::new(),
                            });
                        }
                    }
                    _ => {}
                }
                if is_start {
                    path.push(name);
                }
            }
            Event::Text(text) => {
                let value = text
                    .xml10_content()
                    .unwrap_or_else(|error| panic!("the response body must decode: {error}"))
                    .into_owned();
                let Some(held) = logging.as_mut() else {
                    continue;
                };
                let here: Vec<&str> = path.iter().map(String::as_str).collect();
                // The member a value belongs to is its PATH, not its tag alone.
                match here[here.len().saturating_sub(2)..] {
                    ["LoggingEnabled", "TargetBucket"] => held.target_bucket = value,
                    ["LoggingEnabled", "TargetPrefix"] => held.target_prefix = value,
                    ["PartitionedPrefix", "PartitionDateSource"] => {
                        held.key_format = Some(KeyFormat::Partitioned(Some(value)));
                    }
                    ["Grantee", element] => {
                        if let Some(grant) = held.grants.last_mut() {
                            grant.named_by = (element.to_string(), value);
                        }
                    }
                    ["Grant", "Permission"] => {
                        if let Some(grant) = held.grants.last_mut() {
                            grant.permission = value;
                        }
                    }
                    _ => {}
                }
            }
            Event::End(_) => {
                path.pop();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    logging
}

/// Create the bucket named `name` in the Region named by `location_constraint` (the
/// account's default Region when `None`), asserting it was created.
async fn bucket_in(
    router: &MultiServiceRouter,
    name: &str,
    location_constraint: Option<&str>,
) {
    let created = create_bucket(router, name, location_constraint, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket {name:?} must be created, got {} ({})",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
}

/// The table of logging statuses every row of which must round-trip unchanged.
///
/// The rows differ along what the store-and-report rule generalises over: which target
/// bucket the logs go to (another bucket or the bucket being logged itself), the FORM of
/// the target prefix (a path, a dotted name, the empty string), which key format the
/// container names and which `PartitionDateSource` that format carries, and how many
/// target grants it names, to which kind of grantee and with which permission.
///
/// `TARGET` and `SELF_LOGGED` are both created by the caller, so every row names a target
/// the account holds.
fn round_trip_rows(target: &str, self_logged: &str) -> Vec<(&'static str, Logging)> {
    vec![
        (
            "the two required members only",
            Logging::to(target, "access-logs/"),
        ),
        (
            "an empty target prefix",
            Logging::to(target, ""),
        ),
        (
            "a dotted, deep target prefix",
            Logging::to(target, "logs/2026/raw.v2/source-bucket-"),
        ),
        (
            "logs delivered to the bucket being logged itself",
            Logging::to(self_logged, "self/"),
        ),
        (
            "the simple key format",
            Logging::to(target, "simple/").with_key_format(KeyFormat::Simple),
        ),
        (
            "the partitioned key format naming no date source",
            Logging::to(target, "partitioned/").with_key_format(KeyFormat::Partitioned(None)),
        ),
        (
            "the partitioned key format on event time",
            Logging::to(target, "by-event/")
                .with_key_format(KeyFormat::Partitioned(Some("EventTime".to_string()))),
        ),
        (
            // `DeliveryTime` is the `PartitionDateSource` value the issue did not cite;
            // both come from `PARTITION_DATE_SOURCE_VALUES`, asserted exhaustively by
            // [`every_published_partition_date_source_round_trips`].
            "the partitioned key format on delivery time",
            Logging::to(target, "by-delivery/")
                .with_key_format(KeyFormat::Partitioned(Some("DeliveryTime".to_string()))),
        ),
        (
            "one target grant to the log delivery group",
            Logging::to(target, "granted/").granting(
                GRANTEE_GROUP,
                "URI",
                LOG_DELIVERY_GROUP_URI,
                "READ",
            ),
        ),
        (
            "two target grants, a group and a canonical user, reported in arrival order",
            Logging::to(target, "granted-twice/")
                .granting(GRANTEE_GROUP, "URI", LOG_DELIVERY_GROUP_URI, "FULL_CONTROL")
                .granting(
                    GRANTEE_CANONICAL_USER,
                    "ID",
                    "79a59df900b949e55d96a1e698fbacedfd6e09d98eacf8f8d5218e7cd47ef2be",
                    "WRITE",
                ),
        ),
        (
            "grants and a key format together",
            Logging::to(target, "everything/")
                .granting(GRANTEE_GROUP, "URI", LOG_DELIVERY_GROUP_URI, "READ")
                .with_key_format(KeyFormat::Partitioned(Some("EventTime".to_string()))),
        ),
    ]
}

#[tokio::test]
async fn a_logging_status_round_trips_through_the_bucket() {
    let router = router();
    let target = "logging-target";
    bucket_in(&router, target, None).await;

    for (index, (case, logging)) in round_trip_rows(target, "logging-self-0")
        .into_iter()
        .enumerate()
    {
        let source = format!("logging-round-trip-{index}");
        bucket_in(&router, &source, None).await;
        // The row that logs into itself names this bucket, so it is created under that
        // name too; every other row's target is `target`.
        if logging.target_bucket == "logging-self-0" {
            bucket_in(&router, "logging-self-0", None).await;
        }

        let put = put_logging(&router, &source, Some(&logging)).await;
        assert_eq!(
            put.status,
            200,
            "case {case:?}: expected the put to answer 200, got {} ({})",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        assert!(
            put.body.is_empty(),
            "case {case:?}: a successful put carries no body, got {:?}",
            String::from_utf8_lossy(&put.body)
        );

        let read = get_logging(&router, &source).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: expected the read to answer 200, got {} ({})",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let reported = reported_logging(&read);
        assert_eq!(
            reported.as_ref(),
            Some(&logging),
            "case {case:?}: expected the reported status to be {logging:?}, got {reported:?}"
        );

        // A read changes nothing: the second one reports the same status as the first.
        let again = reported_logging(&get_logging(&router, &source).await);
        assert_eq!(
            again, reported,
            "case {case:?}: a second read must report {reported:?}, got {again:?}"
        );
    }
}

#[tokio::test]
async fn a_never_configured_bucket_reports_an_empty_logging_status() {
    let router = router();
    bucket_in(&router, "logging-unset", None).await;

    let read = get_logging(&router, "logging-unset").await;
    assert_eq!(
        read.status,
        200,
        "a bucket that was never configured must answer 200, got {} ({})",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
    let reported = reported_logging(&read);
    assert_eq!(
        reported, None,
        "a bucket that was never configured must report no LoggingEnabled, got {reported:?}"
    );
    let body = String::from_utf8_lossy(&read.body).into_owned();
    assert!(
        body.contains("BucketLoggingStatus"),
        "the empty status is still a BucketLoggingStatus document, got {body:?}"
    );
}

#[tokio::test]
async fn an_empty_status_element_disables_logging_again() {
    let router = router();
    let target = "logging-disable-target";
    bucket_in(&router, target, None).await;

    // Each row is enabled, read back, disabled, read back: the rule holds whatever the
    // configuration being turned off was.
    let rows = [
        ("the required members only", Logging::to(target, "off/")),
        (
            "a granted, partitioned configuration",
            Logging::to(target, "off-rich/")
                .granting(GRANTEE_GROUP, "URI", LOG_DELIVERY_GROUP_URI, "WRITE")
                .with_key_format(KeyFormat::Partitioned(Some("EventTime".to_string()))),
        ),
    ];

    for (index, (case, logging)) in rows.into_iter().enumerate() {
        let source = format!("logging-disable-{index}");
        bucket_in(&router, &source, None).await;

        assert_eq!(
            put_logging(&router, &source, Some(&logging)).await.status,
            200,
            "case {case:?}: the enabling put must answer 200"
        );
        let enabled = reported_logging(&get_logging(&router, &source).await);
        assert_eq!(
            enabled.as_ref(),
            Some(&logging),
            "case {case:?}: expected {logging:?} to be reported, got {enabled:?}"
        );

        let disabled = put_logging(&router, &source, None).await;
        assert_eq!(
            disabled.status,
            200,
            "case {case:?}: the empty status element must answer 200, got {} ({})",
            disabled.status,
            String::from_utf8_lossy(&disabled.body)
        );

        let read = get_logging(&router, &source).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: a disabled bucket must answer 200, got {}",
            read.status
        );
        let reported = reported_logging(&read);
        assert_eq!(
            reported, None,
            "case {case:?}: a disabled bucket reports no LoggingEnabled, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_put_replaces_the_whole_status_rather_than_merging_it() {
    let router = router();
    let first_target = "logging-replace-first";
    let second_target = "logging-replace-second";
    for bucket in ["logging-replace-source", first_target, second_target] {
        bucket_in(&router, bucket, None).await;
    }

    // Each row is put over the row before it, so every member of the stored status has to
    // come from the LAST payload and nothing from the one it replaced.
    let rows = [
        (
            "a granted, partitioned configuration",
            Logging::to(first_target, "first/")
                .granting(GRANTEE_GROUP, "URI", LOG_DELIVERY_GROUP_URI, "READ")
                .with_key_format(KeyFormat::Partitioned(Some("DeliveryTime".to_string()))),
        ),
        (
            "the bare required members, replacing it",
            Logging::to(second_target, "second/"),
        ),
        (
            "a simple-prefix configuration, replacing that",
            Logging::to(first_target, "third/").with_key_format(KeyFormat::Simple),
        ),
    ];

    for (case, logging) in rows {
        assert_eq!(
            put_logging(&router, "logging-replace-source", Some(&logging))
                .await
                .status,
            200,
            "case {case:?}: the put must answer 200"
        );
        let reported = reported_logging(&get_logging(&router, "logging-replace-source").await);
        assert_eq!(
            reported.as_ref(),
            Some(&logging),
            "case {case:?}: expected the status to be exactly {logging:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn every_published_bucket_logs_permission_is_accepted() {
    let router = router();
    let target = "logging-permission-target";
    bucket_in(&router, target, None).await;

    // The catalog comes from the model itself (`BUCKET_LOGS_PERMISSION_VALUES`, the values
    // of `BucketLogsPermission`), so a model update cannot leave a permission untested.
    for (index, permission) in BUCKET_LOGS_PERMISSION_VALUES.iter().enumerate() {
        let source = format!("logging-permission-{index}");
        bucket_in(&router, &source, None).await;

        let logging = Logging::to(target, "perm/").granting(
            GRANTEE_GROUP,
            "URI",
            LOG_DELIVERY_GROUP_URI,
            permission,
        );
        let put = put_logging(&router, &source, Some(&logging)).await;
        assert_eq!(
            put.status,
            200,
            "case {permission:?}: expected the put to answer 200, got {} ({})",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        let reported = reported_logging(&get_logging(&router, &source).await);
        assert_eq!(
            reported.as_ref(),
            Some(&logging),
            "case {permission:?}: expected {logging:?} to be reported, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn every_published_partition_date_source_round_trips() {
    let router = router();
    let target = "logging-source-target";
    bucket_in(&router, target, None).await;

    // The catalog comes from the model itself (`PARTITION_DATE_SOURCE_VALUES`, the values
    // of `PartitionDateSource`).
    for (index, source_value) in PARTITION_DATE_SOURCE_VALUES.iter().enumerate() {
        let source = format!("logging-date-source-{index}");
        bucket_in(&router, &source, None).await;

        let logging = Logging::to(target, "dated/")
            .with_key_format(KeyFormat::Partitioned(Some((*source_value).to_string())));
        assert_eq!(
            put_logging(&router, &source, Some(&logging)).await.status,
            200,
            "case {source_value:?}: the put must answer 200"
        );
        let reported = reported_logging(&get_logging(&router, &source).await);
        assert_eq!(
            reported.as_ref(),
            Some(&logging),
            "case {source_value:?}: expected {logging:?} to be reported, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_target_bucket_the_account_does_not_hold_is_refused() {
    let router = router();
    bucket_in(&router, "logging-bad-target-source", None).await;
    // A bucket in another Region, so the cross-location row has a real target to name.
    bucket_in(&router, "logging-elsewhere", Some(EU_LOCATION_CONSTRAINT_REGION)).await;

    let rows: Vec<(&str, Logging, u16, &str, &str)> = vec![
        (
            "a target bucket that does not exist",
            Logging::to("logging-no-such-target", "logs/"),
            400,
            INVALID_TARGET_BUCKET_FOR_LOGGING_CODE,
            INVALID_TARGET_BUCKET_FOR_LOGGING_MESSAGE,
        ),
        (
            "an empty target bucket name",
            Logging::to("", "logs/"),
            400,
            INVALID_TARGET_BUCKET_FOR_LOGGING_CODE,
            INVALID_TARGET_BUCKET_FOR_LOGGING_MESSAGE,
        ),
        (
            "a target bucket in another Region",
            Logging::to("logging-elsewhere", "logs/"),
            403,
            CROSS_LOCATION_LOGGING_PROHIBITED_CODE,
            CROSS_LOCATION_LOGGING_PROHIBITED_MESSAGE,
        ),
    ];

    for (case, logging, status, code, message) in rows {
        let answer = put_logging(&router, "logging-bad-target-source", Some(&logging)).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} ({})",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case:?}: expected error code {code:?}, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.error_message(),
            message,
            "case {case:?}: expected message {message:?}, got {:?}",
            answer.error_message()
        );

        // A refused put leaves the bucket's status exactly as it was: still unset.
        let reported = reported_logging(&get_logging(&router, "logging-bad-target-source").await);
        assert_eq!(
            reported, None,
            "case {case:?}: a refused put stores nothing, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_key_format_naming_both_forms_is_refused() {
    let router = router();
    bucket_in(&router, "logging-both-formats", None).await;
    bucket_in(&router, "logging-both-target", None).await;

    let payload = format!(
        "<BucketLoggingStatus xmlns=\"{XMLNS}\"><LoggingEnabled>\
         <TargetBucket>logging-both-target</TargetBucket>\
         <TargetPrefix>both/</TargetPrefix>\
         <TargetObjectKeyFormat><SimplePrefix></SimplePrefix>\
         <PartitionedPrefix><PartitionDateSource>EventTime</PartitionDateSource>\
         </PartitionedPrefix></TargetObjectKeyFormat>\
         </LoggingEnabled></BucketLoggingStatus>"
    );
    let answer = put_logging_payload(&router, "logging-both-formats", &payload, &[]).await;
    assert_eq!(
        answer.status,
        400,
        "a container naming both key formats must answer 400, got {} ({})",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.error_code(),
        MALFORMED_XML_CODE,
        "expected error code {MALFORMED_XML_CODE:?}, got {:?}",
        answer.error_code()
    );
    assert_eq!(
        answer.error_message(),
        MALFORMED_XML_MESSAGE,
        "expected message {MALFORMED_XML_MESSAGE:?}, got {:?}",
        answer.error_message()
    );
    let reported = reported_logging(&get_logging(&router, "logging-both-formats").await);
    assert_eq!(
        reported, None,
        "a refused put stores nothing, got {reported:?}"
    );
}

#[tokio::test]
async fn a_status_naming_more_grants_than_an_acl_may_hold_is_refused() {
    let router = router();
    bucket_in(&router, "logging-grant-ceiling", None).await;
    bucket_in(&router, "logging-grant-ceiling-target", None).await;

    // The published ceiling itself, one below it and one above it: "An ACL can have up to
    // 100 grants" (`MAX_ACL_GRANTS`), and a target grant is a grant on the delivered logs.
    let rows: [(usize, u16); 3] = [
        (MAX_ACL_GRANTS - 1, 200),
        (MAX_ACL_GRANTS, 200),
        (MAX_ACL_GRANTS + 1, 400),
    ];

    for (count, status) in rows {
        let mut logging = Logging::to("logging-grant-ceiling-target", "many/");
        for _ in 0..count {
            logging = logging.granting(GRANTEE_GROUP, "URI", LOG_DELIVERY_GROUP_URI, "READ");
        }
        let answer = put_logging(&router, "logging-grant-ceiling", Some(&logging)).await;
        assert_eq!(
            answer.status, status,
            "case {count} grants: expected HTTP {status}, got {} ({})",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if status != 200 {
            assert_eq!(
                answer.error_code(),
                MALFORMED_ACL_CODE,
                "case {count} grants: expected error code {MALFORMED_ACL_CODE:?}, got {:?}",
                answer.error_code()
            );
        }
    }
}

#[tokio::test]
async fn a_bucket_the_emulator_does_not_hold_is_not_found() {
    let router = router();
    bucket_in(&router, "logging-present-target", None).await;

    let read = get_logging(&router, "logging-absent").await;
    assert_eq!(
        read.status,
        404,
        "a read of an absent bucket must answer 404, got {} ({})",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
    assert_eq!(
        read.error_code(),
        NO_SUCH_BUCKET_CODE,
        "expected error code {NO_SUCH_BUCKET_CODE:?}, got {:?}",
        read.error_code()
    );

    let written = put_logging(
        &router,
        "logging-absent",
        Some(&Logging::to("logging-present-target", "logs/")),
    )
    .await;
    assert_eq!(
        written.status,
        404,
        "a put on an absent bucket must answer 404, got {} ({})",
        written.status,
        String::from_utf8_lossy(&written.body)
    );
    assert_eq!(
        written.error_code(),
        NO_SUCH_BUCKET_CODE,
        "expected error code {NO_SUCH_BUCKET_CODE:?}, got {:?}",
        written.error_code()
    );
}

#[tokio::test]
async fn an_expected_bucket_owner_that_is_not_the_account_is_denied() {
    let router = router();
    bucket_in(&router, "logging-owner", None).await;
    bucket_in(&router, "logging-owner-target", None).await;
    let logging = Logging::to("logging-owner-target", "owned/");
    assert_eq!(
        put_logging(&router, "logging-owner", Some(&logging)).await.status,
        200,
        "the baseline put must answer 200"
    );

    // Three account ids that are not this emulator's, so the rule is asserted on more than
    // one spelling of "somebody else".
    for other in ["111122223333", "000000000001", "999999999999"] {
        let read = get_logging_as(&router, "logging-owner", &[("x-amz-expected-bucket-owner", other)])
            .await;
        assert_eq!(
            read.status, 403,
            "case {other:?}: a read for another owner must answer 403, got {}",
            read.status
        );
        assert_eq!(
            read.error_code(),
            "AccessDenied",
            "case {other:?}: expected AccessDenied, got {:?}",
            read.error_code()
        );

        let written = put_logging_payload(
            &router,
            "logging-owner",
            &status_payload(None),
            &[("x-amz-expected-bucket-owner", other)],
        )
        .await;
        assert_eq!(
            written.status, 403,
            "case {other:?}: a put for another owner must answer 403, got {}",
            written.status
        );

        // The denied put changed nothing: the status is still the one the baseline set.
        let reported = reported_logging(&get_logging(&router, "logging-owner").await);
        assert_eq!(
            reported.as_ref(),
            Some(&logging),
            "case {other:?}: a denied put stores nothing, got {reported:?}"
        );
    }
}

/// Who one `<Grant>` of a `<TargetGrants>` names, spelled out member by member.
///
/// A `Grantee` makes only its `Type` required and every id-carrying member optional
/// (`smithy_query(query="shape",shape="TargetGrant")`), so a payload can name a grantee
/// whose `Type` and members disagree — which no [`Logging`] row can express.
#[derive(Debug, Clone, Copy)]
struct RawGrantee {
    /// The `Grantee$Type` attribute value.
    grantee_type: &'static str,
    /// The element naming the grantee (`ID`, `URI` or `EmailAddress`) and its value, or
    /// `None` for a grantee that names no member at all.
    named_by: Option<(&'static str, &'static str)>,
}

/// One `<Grant>` of a `<TargetGrants>`.
///
/// `TargetGrant$Grantee` and `TargetGrant$Permission` are both OPTIONAL in the model (same
/// query), so a grant can carry neither who nor what.
#[derive(Debug, Clone, Copy)]
struct RawGrant {
    grantee: Option<RawGrantee>,
    permission: Option<&'static str>,
}

impl RawGrant {
    fn xml(&self) -> String {
        let mut body = String::from("<Grant>");
        if let Some(grantee) = self.grantee {
            body.push_str(&format!(
                "<Grantee xmlns:xsi=\"{XSI}\" xsi:type=\"{}\">",
                grantee.grantee_type
            ));
            if let Some((element, value)) = grantee.named_by {
                body.push_str(&format!("<{element}>{value}</{element}>"));
            }
            body.push_str("</Grantee>");
        }
        if let Some(permission) = self.permission {
            body.push_str(&format!("<Permission>{permission}</Permission>"));
        }
        body.push_str("</Grant>");
        body
    }
}

/// A `BucketLoggingStatus` enabling logging to `target_bucket` whose target grants are
/// spelled out by [`RawGrant`], in the member order `LoggingEnabled` declares.
fn status_payload_with_grants(
    target_bucket: &str,
    target_prefix: &str,
    grants: &[RawGrant],
) -> String {
    let spelled: String = grants.iter().map(RawGrant::xml).collect();
    format!(
        "<BucketLoggingStatus xmlns=\"{XMLNS}\"><LoggingEnabled>\
         <TargetBucket>{target_bucket}</TargetBucket>\
         <TargetGrants>{spelled}</TargetGrants>\
         <TargetPrefix>{target_prefix}</TargetPrefix>\
         </LoggingEnabled></BucketLoggingStatus>"
    )
}

/// A group URI that is not one of the predefined groups S3 publishes, so a `Group` grantee
/// naming it names no group at all.
const NOT_A_PREDEFINED_GROUP_URI: &str = "http://acs.amazonaws.com/groups/s3/LogDeliver";

/// A canonical user id of the published form, for the rows that name a grantee properly.
const A_CANONICAL_USER_ID: &str = "79a59df900b949e55d96a1e698fbacedfd6e09d98eacf8f8d5218e7cd47ef2be";

#[tokio::test]
async fn a_target_grant_that_is_not_a_grant_s3_can_hold_is_refused() {
    let router = router();
    let source = "logging-grant-rules";
    let target = "logging-grant-rules-target";
    bucket_in(&router, source, None).await;
    bucket_in(&router, target, None).await;

    // A status the bucket really carries, so every refusal below is asserted to leave the
    // stored status alone as well as to be refused.
    let baseline = Logging::to(target, "baseline/").granting(
        GRANTEE_GROUP,
        "URI",
        LOG_DELIVERY_GROUP_URI,
        "READ",
    );
    assert_eq!(
        put_logging(&router, source, Some(&baseline)).await.status,
        200,
        "the baseline put must answer 200"
    );
    // Both buckets are in the account's default Region, which is one of the Regions S3 has
    // discontinued Email Grantee ACLs in — what the email row expects a 405 for.
    assert!(
        EMAIL_GRANTEE_DISCONTINUED_REGIONS.contains(&DEFAULT_BUCKET_REGION),
        "the default Region {DEFAULT_BUCKET_REGION:?} must be one of \
         {EMAIL_GRANTEE_DISCONTINUED_REGIONS:?} for the email-grantee row to mean what it says"
    );

    let group = |named_by| RawGrantee {
        grantee_type: GRANTEE_GROUP,
        named_by,
    };
    let rows: Vec<(&str, Vec<RawGrant>, u16, &str)> = vec![
        (
            "a Group grantee naming no URI",
            vec![RawGrant {
                grantee: Some(group(None)),
                permission: Some("READ"),
            }],
            400,
            MALFORMED_ACL_CODE,
        ),
        (
            "a Group grantee whose URI is blank",
            vec![RawGrant {
                grantee: Some(group(Some(("URI", "   ")))),
                permission: Some("READ"),
            }],
            400,
            MALFORMED_ACL_CODE,
        ),
        (
            "a Group grantee naming a URI outside the predefined groups",
            vec![RawGrant {
                grantee: Some(group(Some(("URI", NOT_A_PREDEFINED_GROUP_URI)))),
                permission: Some("READ"),
            }],
            400,
            INVALID_ARGUMENT_CODE,
        ),
        (
            "a CanonicalUser grantee naming no ID",
            vec![RawGrant {
                grantee: Some(RawGrantee {
                    grantee_type: GRANTEE_CANONICAL_USER,
                    named_by: None,
                }),
                permission: Some("FULL_CONTROL"),
            }],
            400,
            MALFORMED_ACL_CODE,
        ),
        (
            "a CanonicalUser grantee named by a URI instead of an ID",
            vec![RawGrant {
                grantee: Some(RawGrantee {
                    grantee_type: GRANTEE_CANONICAL_USER,
                    named_by: Some(("URI", LOG_DELIVERY_GROUP_URI)),
                }),
                permission: Some("WRITE"),
            }],
            400,
            MALFORMED_ACL_CODE,
        ),
        (
            "a grant naming no Permission",
            vec![RawGrant {
                grantee: Some(group(Some(("URI", LOG_DELIVERY_GROUP_URI)))),
                permission: None,
            }],
            400,
            MALFORMED_ACL_CODE,
        ),
        (
            "a grant naming no Grantee",
            vec![RawGrant {
                grantee: None,
                permission: Some("READ"),
            }],
            400,
            MALFORMED_ACL_CODE,
        ),
        (
            // The rule holds for EVERY grant of the list, not just for the first one.
            "a grant S3 can hold followed by one it cannot",
            vec![
                RawGrant {
                    grantee: Some(RawGrantee {
                        grantee_type: GRANTEE_CANONICAL_USER,
                        named_by: Some(("ID", A_CANONICAL_USER_ID)),
                    }),
                    permission: Some("FULL_CONTROL"),
                },
                RawGrant {
                    grantee: Some(group(Some(("URI", NOT_A_PREDEFINED_GROUP_URI)))),
                    permission: Some("READ"),
                },
            ],
            400,
            INVALID_ARGUMENT_CODE,
        ),
        (
            "an email grantee, where S3 has discontinued Email Grantee ACLs",
            vec![RawGrant {
                grantee: Some(RawGrantee {
                    grantee_type: GRANTEE_BY_EMAIL,
                    named_by: Some(("EmailAddress", "log-reader@example.com")),
                }),
                permission: Some("READ"),
            }],
            405,
            METHOD_NOT_ALLOWED_CODE,
        ),
    ];

    for (case, grants, status, code) in rows {
        let payload = status_payload_with_grants(target, "refused/", &grants);
        let answer = put_logging_payload(&router, source, &payload, &[]).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} ({})",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case:?}: expected error code {code:?}, got {:?}",
            answer.error_code()
        );
        assert!(
            !answer.error_message().is_empty(),
            "case {case:?}: the refusal must carry a message saying which rule the grant \
             breaks, got {:?}",
            answer.error_message()
        );

        let reported = reported_logging(&get_logging(&router, source).await);
        assert_eq!(
            reported.as_ref(),
            Some(&baseline),
            "case {case:?}: a refused put stores nothing, so {baseline:?} must still be \
             reported, got {reported:?}"
        );
    }
}

/// A Region S3 has NOT discontinued Email Grantee ACLs in: the first
/// `BucketLocationConstraint` value that is a Region code (rather than the legacy `EU`
/// alias) and is not one of `EMAIL_GRANTEE_DISCONTINUED_REGIONS`. Taken from those two
/// published catalogs so a change to either cannot leave this row naming a Region that is
/// in the discontinued set after all.
fn region_still_taking_email_grantees() -> &'static str {
    BUCKET_LOCATION_CONSTRAINT_VALUES
        .iter()
        .copied()
        .find(|region| {
            region.contains('-') && !EMAIL_GRANTEE_DISCONTINUED_REGIONS.contains(region)
        })
        .expect("the model must publish a Region outside the discontinued-email set")
}

#[tokio::test]
async fn an_email_grantee_resolves_to_no_account_where_s3_still_takes_one() {
    let router = router();
    let region = region_still_taking_email_grantees();
    let source = "logging-email-grantee";
    let target = "logging-email-grantee-target";
    // Source and target in the SAME Region, so the cross-location rule is not what refuses
    // this put.
    bucket_in(&router, source, Some(region)).await;
    bucket_in(&router, target, Some(region)).await;

    let payload = status_payload_with_grants(
        target,
        "by-email/",
        &[RawGrant {
            grantee: Some(RawGrantee {
                grantee_type: GRANTEE_BY_EMAIL,
                named_by: Some(("EmailAddress", "log-reader@example.com")),
            }),
            permission: Some("READ"),
        }],
    );
    let answer = put_logging_payload(&router, source, &payload, &[]).await;
    assert_eq!(
        answer.status,
        400,
        "case {region:?}: an email grantee that matches no account must answer 400, got {} ({})",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.error_code(),
        UNRESOLVABLE_GRANT_BY_EMAIL_CODE,
        "case {region:?}: expected error code {UNRESOLVABLE_GRANT_BY_EMAIL_CODE:?}, got {:?}",
        answer.error_code()
    );

    let reported = reported_logging(&get_logging(&router, source).await);
    assert_eq!(
        reported, None,
        "case {region:?}: a refused put stores nothing, got {reported:?}"
    );
}
