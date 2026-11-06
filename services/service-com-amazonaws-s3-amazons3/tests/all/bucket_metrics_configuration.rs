// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a general purpose bucket's CloudWatch request-metrics
//! configurations: `PutBucketMetricsConfiguration` records one under the id that identifies
//! it and `GetBucketMetricsConfiguration` reports that one back.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite owns), so what is pinned is the wire behaviour — status, body,
//! error code — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * `PutBucketMetricsConfiguration` "Sets a metrics configuration (specified by the
//!   metrics configuration ID) for the bucket … If you're updating an existing metrics
//!   configuration, note that this is a full replacement of the existing metrics
//!   configuration. If you don't include the elements you want to keep, they are erased"
//!   (the operation's own documentation): the `MetricsConfiguration` payload becomes the
//!   configuration stored under THAT id in full, every other id is left alone, and the call
//!   answers [`PUT_BUCKET_METRICS_CONFIGURATION_SUCCESS_STATUS`] with an empty body.
//! * `GetBucketMetricsConfiguration` reports exactly that container, with every member the
//!   request named and no member it did not; a bucket carrying none under the id answers
//!   `NoSuchConfiguration` (HTTP 404).
//! * "You can have up to 1,000 metrics configurations per bucket" and the operation's
//!   special error "TooManyConfigurations … HTTP Status Code: HTTP 400 Bad Request": a put
//!   that would create one past [`MAX_METRICS_CONFIGURATIONS_PER_BUCKET`] is refused, while
//!   replacing an id already stored creates nothing and is not.
//! * "The ID has a 64 character limit and can only contain letters, numbers, periods,
//!   dashes, and underscores" (`MetricsConfiguration$Id`): that rule is published on a
//!   member of the DOCUMENT, so an `Id` outside it — too long, or outside the character
//!   class — is a document the published schema does not validate: `MalformedXML` (HTTP 400)
//!   carrying that code's fixed published message, which is how S3 reports it for THIS
//!   family. A call whose `id` argument states no configuration AT ALL addresses none for a
//!   document rule to be reported against, and answers [`INVALID_CONFIGURATION_ID_CODE`]
//!   with its own fixed sentence.
//! * Everything else about the DOCUMENT that the published schema does not validate is the
//!   same `MalformedXML`: a `Filter` that is not EXACTLY ONE of "a prefix, an object tag, an
//!   access point ARN, or a conjunction (MetricsAndOperator)" (`MetricsFilter`'s own
//!   documentation), an `And` naming fewer than the "at least two predicates"
//!   `MetricsFilter$And` requires, a payload `Id` that is not the configuration the request
//!   addresses, and an access point ARN member that is not an access point ARN.
//! * A call naming a bucket this emulator does not hold answers `NoSuchBucket` (HTTP 404);
//!   one naming an `ExpectedBucketOwner` that is not the account's answers HTTP 403
//!   `AccessDenied`. Every refusal leaves the stored configurations exactly as they were.

use quick_xml::events::Event;
use quick_xml::Reader;
use service_com_amazonaws_s3_amazons3::ops::put_bucket_metrics_configuration::PUT_BUCKET_METRICS_CONFIGURATION_SUCCESS_STATUS;
use service_com_amazonaws_s3_amazons3::state::{
    INVALID_CONFIGURATION_ID_CODE, INVALID_CONFIGURATION_ID_MESSAGE, MALFORMED_XML_CODE,
    MALFORMED_XML_MESSAGE, MAX_METRICS_CONFIGURATIONS_PER_BUCKET, MAX_METRICS_ID_CHARS,
    NO_SUCH_BUCKET_CODE, NO_SUCH_CONFIGURATION_CODE, TOO_MANY_CONFIGURATIONS_CODE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, Answer};

/// The XML namespace botocore puts on an S3 request payload.
pub(crate) const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// An access point ARN, as `MetricsFilter$AccessPointArn` takes one.
pub(crate) const ACCESS_POINT_ARN: &str = "arn:aws:s3:us-east-1:000000000000:accesspoint/reports";

/// One `MetricsFilter`, as a request carries it and as a report brings it back.
///
/// The variants are the members `smithy_query(query="shape",shape="MetricsFilter")` declares
/// for that union; the same type on both sides, so a round-trip assertion compares the
/// filter that was sent with the filter that was reported rather than with a hand-spelled
/// expectation.
///
/// `pub(crate)`, like the harness in [`super::bucket_lifecycle`]: the listing suite beside
/// this one puts and compares configurations through this same type instead of spelling a
/// second XML writer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Filter {
    Prefix(String),
    Tag(String, String),
    AccessPointArn(String),
    And {
        prefix: Option<String>,
        tags: Vec<(String, String)>,
        access_point_arn: Option<String>,
    },
}

impl Filter {
    /// The filter as XML, in the member order the shape declares. `MetricsAndOperator$Tags`
    /// is `xmlFlattened` with `xmlName` `Tag`, so each tag is its own `<Tag>` child of
    /// `<And>`.
    fn xml(&self) -> String {
        let inner = match self {
            Self::Prefix(prefix) => format!("<Prefix>{prefix}</Prefix>"),
            Self::Tag(key, value) => tag_xml(key, value),
            Self::AccessPointArn(arn) => format!("<AccessPointArn>{arn}</AccessPointArn>"),
            Self::And {
                prefix,
                tags,
                access_point_arn,
            } => {
                let mut body = String::from("<And>");
                if let Some(prefix) = prefix {
                    body.push_str(&format!("<Prefix>{prefix}</Prefix>"));
                }
                for (key, value) in tags {
                    body.push_str(&tag_xml(key, value));
                }
                if let Some(arn) = access_point_arn {
                    body.push_str(&format!("<AccessPointArn>{arn}</AccessPointArn>"));
                }
                body.push_str("</And>");
                body
            }
        };
        format!("<Filter>{inner}</Filter>")
    }

    /// An `And` of `prefix` and `tags`, the conjunction form a request uses when one
    /// predicate is not enough.
    pub(crate) fn and(prefix: Option<&str>, tags: &[(&str, &str)], access_point_arn: Option<&str>) -> Self {
        Self::And {
            prefix: prefix.map(str::to_string),
            tags: tags
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
            access_point_arn: access_point_arn.map(str::to_string),
        }
    }
}

/// One `Tag` element, as `Tag$Key` and `Tag$Value` spell it.
fn tag_xml(key: &str, value: &str) -> String {
    format!("<Tag><Key>{key}</Key><Value>{value}</Value></Tag>")
}

/// The `MetricsConfiguration` payload for `id` and `filter`, spelled the way a client's XML
/// writer spells it.
pub(crate) fn configuration_payload(id: &str, filter: Option<&Filter>) -> String {
    let mut body = format!("<MetricsConfiguration xmlns=\"{XMLNS}\"><Id>{id}</Id>");
    if let Some(filter) = filter {
        body.push_str(&filter.xml());
    }
    body.push_str("</MetricsConfiguration>");
    body
}

/// `PUT /<bucket>?metrics&id=<id>` — `PutBucketMetricsConfiguration`.
pub(crate) async fn put_metrics(
    router: &MultiServiceRouter,
    bucket: &str,
    id: &str,
    filter: Option<&Filter>,
) -> Answer {
    put_metrics_payload(router, bucket, id, &configuration_payload(id, filter), &[]).await
}

/// `PutBucketMetricsConfiguration` with the payload spelled out — for a container no
/// [`Filter`] can express — and with any extra bound headers.
pub(crate) async fn put_metrics_payload(
    router: &MultiServiceRouter,
    bucket: &str,
    id: &str,
    payload: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}?metrics&id={id}"),
        headers,
        payload,
    )
    .await
}

/// `GET /<bucket>?metrics&id=<id>` — `GetBucketMetricsConfiguration`.
pub(crate) async fn get_metrics(router: &MultiServiceRouter, bucket: &str, id: &str) -> Answer {
    get_metrics_with_headers(router, bucket, id, &[]).await
}

/// `GetBucketMetricsConfiguration` with extra bound headers.
async fn get_metrics_with_headers(
    router: &MultiServiceRouter,
    bucket: &str,
    id: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "GET",
        &format!("/{bucket}?metrics&id={id}"),
        headers,
        "",
    )
    .await
}

/// The `Id` and the `Filter` of the one configuration a `GetBucketMetricsConfiguration`
/// body reports.
fn reported_configuration(answer: &Answer) -> (String, Option<Filter>) {
    reported_configurations(answer).into_iter().next().unwrap_or_default()
}

/// The `Id` and the `Filter` of EVERY `MetricsConfiguration` a body reports, in document
/// order: one for a `GetBucketMetricsConfiguration` body, as many as the page carries for a
/// `ListBucketMetricsConfigurations` body (which the listing suite beside this one reads
/// through this same parser rather than a second one).
///
/// Parsed with `quick-xml` (already a dependency of this workspace), the way the
/// replication suite beside this one reads a container, rather than by slicing the body.
/// The member a value belongs to is its PATH, not its tag: `Prefix`, `AccessPointArn` and
/// `Tag` each sit directly on a `Filter` and inside its `And`.
pub(crate) fn reported_configurations(answer: &Answer) -> Vec<(String, Option<Filter>)> {
    let body = String::from_utf8_lossy(&answer.body).into_owned();
    let mut reader = Reader::from_str(&body);

    let mut reported: Vec<(String, Option<Filter>)> = Vec::new();
    // The configuration being read: a `MetricsConfiguration` opening starts a new one, so
    // the previous one is complete and is pushed then.
    let mut id = String::new();
    let mut filter: Option<Filter> = None;
    let mut reading = false;
    let mut path: Vec<String> = Vec::new();
    // An element OPENING is what declares which member is present, so a member whose value
    // is the empty string (`<Prefix></Prefix>`, which carries no text event at all) is
    // reported as present-and-empty rather than as absent.
    let begin = |name: &str, path: &[String], filter: &mut Option<Filter>| {
        let parent = path.last().map(String::as_str).unwrap_or_default();
        match (parent, name) {
            ("Filter", "Prefix") => *filter = Some(Filter::Prefix(String::new())),
            ("Filter", "AccessPointArn") => *filter = Some(Filter::AccessPointArn(String::new())),
            ("Filter", "Tag") => *filter = Some(Filter::Tag(String::new(), String::new())),
            ("Filter", "And") => {
                *filter = Some(Filter::And {
                    prefix: None,
                    tags: Vec::new(),
                    access_point_arn: None,
                });
            }
            ("And", "Prefix") => {
                if let Some(Filter::And { prefix, .. }) = filter.as_mut() {
                    *prefix = Some(String::new());
                }
            }
            ("And", "AccessPointArn") => {
                if let Some(Filter::And {
                    access_point_arn, ..
                }) = filter.as_mut()
                {
                    *access_point_arn = Some(String::new());
                }
            }
            ("And", "Tag") => {
                if let Some(Filter::And { tags, .. }) = filter.as_mut() {
                    tags.push((String::new(), String::new()));
                }
            }
            _ => {}
        }
    };
    loop {
        match reader
            .read_event()
            .unwrap_or_else(|error| panic!("the response body must be well-formed XML: {error}"))
        {
            Event::Empty(empty) => {
                let name = String::from_utf8_lossy(empty.name().as_ref()).into_owned();
                begin(&name, &path, &mut filter);
            }
            Event::Start(start) => {
                let name = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                if name == "MetricsConfiguration" {
                    if reading {
                        reported.push((std::mem::take(&mut id), filter.take()));
                    }
                    reading = true;
                }
                begin(&name, &path, &mut filter);
                path.push(name);
            }
            Event::Text(text) => {
                let value = text
                    .xml10_content()
                    .unwrap_or_else(|error| panic!("the response body must decode: {error}"))
                    .into_owned();
                let here: Vec<&str> = path.iter().map(String::as_str).collect();
                let inside_and = here.contains(&"And");
                match here[here.len().saturating_sub(2)..] {
                    ["MetricsConfiguration", "Id"] => id = value,
                    ["Filter", "Prefix"] => filter = Some(Filter::Prefix(value)),
                    ["Filter", "AccessPointArn"] => filter = Some(Filter::AccessPointArn(value)),
                    ["And", "Prefix"] => {
                        if let Some(Filter::And { prefix, .. }) = filter.as_mut() {
                            *prefix = Some(value);
                        }
                    }
                    ["And", "AccessPointArn"] => {
                        if let Some(Filter::And {
                            access_point_arn, ..
                        }) = filter.as_mut()
                        {
                            *access_point_arn = Some(value);
                        }
                    }
                    ["Tag", member] if inside_and => {
                        if let Some(Filter::And { tags, .. }) = filter.as_mut() {
                            if let Some((key, tag_value)) = tags.last_mut() {
                                match member {
                                    "Key" => *key = value,
                                    "Value" => *tag_value = value,
                                    _ => {}
                                }
                            }
                        }
                    }
                    ["Tag", member] => {
                        if let Some(Filter::Tag(key, tag_value)) = filter.as_mut() {
                            match member {
                                "Key" => *key = value,
                                "Value" => *tag_value = value,
                                _ => {}
                            }
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
    if reading {
        reported.push((id, filter));
    }
    reported
}

/// Stand up a bucket named `name`, which is what a metrics configuration is put on.
pub(crate) async fn bucket(router: &MultiServiceRouter, name: &str) {
    let created = create_bucket(router, name, None, None).await;
    assert_eq!(
        created.status, 200,
        "case setup: the bucket {name:?} must be created, got {}",
        created.status
    );
}

/// An id exactly [`MAX_METRICS_ID_CHARS`] characters long — the published ceiling, which a
/// put must still take.
fn id_at_the_ceiling() -> String {
    "a".repeat(MAX_METRICS_ID_CHARS)
}

/// The table of configurations every row of which must round-trip unchanged.
///
/// The rows differ along what the store-and-report rule generalises over: WHICH form the
/// filter takes (none at all, each of the three single predicates
/// `smithy_query(query="shape",shape="MetricsFilter")` declares, and the `And`
/// conjunction), HOW MANY predicates the conjunction carries and in which combination, and
/// how the id is SPELLED — including every character class
/// `MetricsConfiguration$Id` allows and the 64-character ceiling it names.
fn round_trip_rows() -> Vec<(&'static str, String, Option<Filter>)> {
    vec![
        (
            "no filter at all: every object in the bucket",
            "EntireBucket".to_string(),
            None,
        ),
        (
            "a prefix predicate",
            "documents".to_string(),
            Some(Filter::Prefix("documents/".to_string())),
        ),
        (
            "an empty prefix predicate",
            "empty-prefix".to_string(),
            Some(Filter::Prefix(String::new())),
        ),
        (
            "a single tag predicate",
            "billing_tag".to_string(),
            Some(Filter::Tag("team".to_string(), "billing".to_string())),
        ),
        (
            "an access point ARN predicate",
            "reports.ap".to_string(),
            Some(Filter::AccessPointArn(ACCESS_POINT_ARN.to_string())),
        ),
        (
            "an And of a prefix and one tag",
            "prefix-and-tag".to_string(),
            Some(Filter::and(
                Some("logs/"),
                &[("team", "platform")],
                None,
            )),
        ),
        (
            "an And of two tags",
            "two_tags".to_string(),
            Some(Filter::and(
                None,
                &[("team", "platform"), ("tier", "gold")],
                None,
            )),
        ),
        (
            "an And of a prefix, a tag and an access point ARN",
            "All.3-predicates_0".to_string(),
            Some(Filter::and(
                Some("reports/"),
                &[("team", "finance")],
                Some(ACCESS_POINT_ARN),
            )),
        ),
        (
            "an id at the 64-character ceiling",
            id_at_the_ceiling(),
            Some(Filter::Prefix("ceiling/".to_string())),
        ),
    ]
}

#[tokio::test]
async fn a_metrics_configuration_round_trips_under_its_own_id() {
    let router = router();

    // The status a successful put answers is `204 No Content`, which the recorded service
    // sends for this subresource write where the model's `@http` code says 200 — pinned
    // against the LITERAL here, so the constant every other assertion of these suites reads
    // cannot drift back to the model's code unnoticed.
    assert_eq!(
        PUT_BUCKET_METRICS_CONFIGURATION_SUCCESS_STATUS, 204,
        "a successful metrics put answers 204 No Content, got {PUT_BUCKET_METRICS_CONFIGURATION_SUCCESS_STATUS}"
    );

    for (index, (case, id, filter)) in round_trip_rows().into_iter().enumerate() {
        let name = format!("metrics-round-trip-{index}");
        bucket(&router, &name).await;

        let put = put_metrics(&router, &name, &id, filter.as_ref()).await;
        assert_eq!(
            put.status,
            PUT_BUCKET_METRICS_CONFIGURATION_SUCCESS_STATUS,
            "case {case:?}: expected the put to answer \
             {PUT_BUCKET_METRICS_CONFIGURATION_SUCCESS_STATUS}, got {} ({})",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        assert!(
            put.body.is_empty(),
            "case {case:?}: a successful put carries no body, got {:?}",
            String::from_utf8_lossy(&put.body)
        );

        let read = get_metrics(&router, &name, &id).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: expected the read to answer 200, got {} ({})",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let (reported_id, reported_filter) = reported_configuration(&read);
        assert_eq!(
            reported_id, id,
            "case {case:?}: expected Id {id:?}, got {reported_id:?}"
        );
        assert_eq!(
            reported_filter, filter,
            "case {case:?}: expected the filter that was put ({filter:?}), got \
             {reported_filter:?}"
        );

        // "this is a full replacement of the existing metrics configuration. If you don't
        // include the elements you want to keep, they are erased": a second put under the
        // same id leaves the bucket carrying only what IT named.
        let replacement = put_metrics(&router, &name, &id, None).await;
        assert_eq!(
            replacement.status, PUT_BUCKET_METRICS_CONFIGURATION_SUCCESS_STATUS,
            "case {case:?}: expected the replacing put to answer \
             {PUT_BUCKET_METRICS_CONFIGURATION_SUCCESS_STATUS}, got {}",
            replacement.status
        );
        let (_, after) = reported_configuration(&get_metrics(&router, &name, &id).await);
        assert_eq!(
            after, None,
            "case {case:?}: expected the replacement's filter (none), got {after:?}"
        );
    }
}

/// A configuration is addressable on its own: a put under one id neither changes nor hides
/// what another id carries, and a read of an id the bucket carries nothing under reports
/// that rather than somebody else's configuration.
#[tokio::test]
async fn each_id_carries_its_own_configuration() {
    let router = router();
    let name = "metrics-per-id";
    bucket(&router, name).await;

    // (id, filter)
    let stored: Vec<(&str, Filter)> = vec![
        ("logs", Filter::Prefix("logs/".to_string())),
        (
            "gold-tier",
            Filter::Tag("tier".to_string(), "gold".to_string()),
        ),
        (
            "reports",
            Filter::and(Some("reports/"), &[("team", "finance")], None),
        ),
    ];
    for (id, filter) in &stored {
        let put = put_metrics(&router, name, id, Some(filter)).await;
        assert_eq!(
            put.status,
            PUT_BUCKET_METRICS_CONFIGURATION_SUCCESS_STATUS,
            "case {id:?}: expected the put to answer \
             {PUT_BUCKET_METRICS_CONFIGURATION_SUCCESS_STATUS}, got {} ({})",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
    }

    for (id, filter) in &stored {
        let read = get_metrics(&router, name, id).await;
        assert_eq!(
            read.status,
            200,
            "case {id:?}: expected the read to answer 200, got {} ({})",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let (reported_id, reported_filter) = reported_configuration(&read);
        assert_eq!(
            (reported_id.as_str(), reported_filter.as_ref()),
            (*id, Some(filter)),
            "case {id:?}: expected each id to report its own configuration, got \
             {reported_id:?} / {reported_filter:?}"
        );
    }

    let absent = get_metrics(&router, name, "never-put").await;
    assert_eq!(
        absent.status, 404,
        "case \"an id the bucket carries nothing under\": expected 404, got {}",
        absent.status
    );
    assert_eq!(
        absent.error_code(),
        NO_SUCH_CONFIGURATION_CODE,
        "case \"an id the bucket carries nothing under\": expected \
         {NO_SUCH_CONFIGURATION_CODE}, got {}",
        absent.error_code()
    );
}

/// Every request the service cannot record is refused with the code AWS documents for it,
/// and leaves the configuration the bucket already carried in place.
#[tokio::test]
async fn a_configuration_s3_cannot_record_is_refused_and_changes_nothing() {
    let router = router();
    let name = "metrics-refusals";
    bucket(&router, name).await;

    let stored_id = "already-here";
    let stored = Filter::Prefix("kept/".to_string());
    let put = put_metrics(&router, name, stored_id, Some(&stored)).await;
    assert_eq!(
        put.status, PUT_BUCKET_METRICS_CONFIGURATION_SUCCESS_STATUS,
        "case setup: expected the put to answer \
         {PUT_BUCKET_METRICS_CONFIGURATION_SUCCESS_STATUS}, got {}",
        put.status
    );

    // One character past the published ceiling, far past it, and characters the member does
    // not allow — the ends of the id rule, each spelled a different way.
    let too_long = "a".repeat(MAX_METRICS_ID_CHARS + 1);
    let far_too_long = "b".repeat(MAX_METRICS_ID_CHARS * 4);

    // (case, id in the query, payload, expected status, expected code, expected message)
    //
    // Every row whose DOCUMENT the published schema does not validate — the id rule
    // included, since that rule is published on a document member — is the schema refusal
    // ([`MALFORMED_XML_CODE`], with that code's fixed published message); a call whose `id`
    // argument states no configuration at all addresses none, and is refused as that
    // ([`INVALID_CONFIGURATION_ID_CODE`]).
    let rows: Vec<(&str, String, String, u16, &str, &str)> = vec![
        (
            "an id one character past the 64-character ceiling",
            too_long.clone(),
            configuration_payload(&too_long, None),
            400,
            MALFORMED_XML_CODE,
            MALFORMED_XML_MESSAGE,
        ),
        (
            "an id many characters past the 64-character ceiling",
            far_too_long.clone(),
            configuration_payload(&far_too_long, None),
            400,
            MALFORMED_XML_CODE,
            MALFORMED_XML_MESSAGE,
        ),
        (
            "an id carrying a character the member does not allow",
            "not+allowed".to_string(),
            configuration_payload("not+allowed", None),
            400,
            MALFORMED_XML_CODE,
            MALFORMED_XML_MESSAGE,
        ),
        (
            "an id carrying a path separator, which is no id character either",
            "not/allowed".to_string(),
            configuration_payload("not/allowed", None),
            400,
            MALFORMED_XML_CODE,
            MALFORMED_XML_MESSAGE,
        ),
        (
            "an id argument that states no configuration at all",
            String::new(),
            configuration_payload("", None),
            400,
            INVALID_CONFIGURATION_ID_CODE,
            INVALID_CONFIGURATION_ID_MESSAGE,
        ),
        (
            "an empty payload Id beside an argument that names one",
            "named".to_string(),
            configuration_payload("", None),
            400,
            MALFORMED_XML_CODE,
            MALFORMED_XML_MESSAGE,
        ),
        (
            "an And naming one predicate where the operator takes two",
            "one-predicate".to_string(),
            format!(
                "<MetricsConfiguration xmlns=\"{XMLNS}\"><Id>one-predicate</Id>\
                 <Filter><And><Prefix>only/</Prefix></And></Filter></MetricsConfiguration>"
            ),
            400,
            MALFORMED_XML_CODE,
            MALFORMED_XML_MESSAGE,
        ),
        (
            "an And naming no predicate at all",
            "no-predicate".to_string(),
            format!(
                "<MetricsConfiguration xmlns=\"{XMLNS}\"><Id>no-predicate</Id>\
                 <Filter><And></And></Filter></MetricsConfiguration>"
            ),
            400,
            MALFORMED_XML_CODE,
            MALFORMED_XML_MESSAGE,
        ),
        (
            "a payload Id that is not the configuration the request names",
            "query-id".to_string(),
            configuration_payload("payload-id", None),
            400,
            MALFORMED_XML_CODE,
            MALFORMED_XML_MESSAGE,
        ),
    ];

    for (case, id, payload, status, code, message) in rows {
        let answer = put_metrics_payload(&router, name, &id, &payload, &[]).await;
        assert_eq!(
            answer.status,
            status,
            "case {case:?}: expected HTTP {status}, got {} ({})",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case:?}: expected the code {code}, got {}",
            answer.error_code()
        );
        // Each of these codes carries its own FIXED published sentence, with nothing of the
        // request in it.
        assert_eq!(
            answer.error_message(),
            message,
            "case {case:?}: expected the message {message:?}, got {}",
            answer.error_message()
        );

        // The refusal recorded nothing: the configuration that WAS stored is unchanged.
        let (reported_id, reported_filter) =
            reported_configuration(&get_metrics(&router, name, stored_id).await);
        assert_eq!(
            (reported_id.as_str(), reported_filter.as_ref()),
            (stored_id, Some(&stored)),
            "case {case:?}: expected the stored configuration to survive the refusal, got \
             {reported_id:?} / {reported_filter:?}"
        );
    }

    // A bucket this emulator does not hold, on both sides of the round trip.
    for (case, answer) in [
        (
            "a put naming a bucket that does not exist",
            put_metrics(&router, "metrics-no-such-bucket", "any", None).await,
        ),
        (
            "a read naming a bucket that does not exist",
            get_metrics(&router, "metrics-no-such-bucket", "any").await,
        ),
    ] {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {}",
            answer.status
        );
        assert_eq!(
            answer.error_code(),
            NO_SUCH_BUCKET_CODE,
            "case {case:?}: expected the code {NO_SUCH_BUCKET_CODE}, got {}",
            answer.error_code()
        );
    }

    // An expected bucket owner that is not this account's, on both sides.
    let not_the_owner = [("x-amz-expected-bucket-owner", "111122223333")];
    for (case, answer) in [
        (
            "a put naming another account as the expected bucket owner",
            put_metrics_payload(
                &router,
                name,
                "owner-mismatch",
                &configuration_payload("owner-mismatch", None),
                &not_the_owner,
            )
            .await,
        ),
        (
            "a read naming another account as the expected bucket owner",
            get_metrics_with_headers(&router, name, stored_id, &not_the_owner).await,
        ),
    ] {
        assert_eq!(
            answer.status, 403,
            "case {case:?}: expected HTTP 403, got {}",
            answer.status
        );
        assert_eq!(
            answer.error_code(),
            "AccessDenied",
            "case {case:?}: expected the code AccessDenied, got {}",
            answer.error_code()
        );
    }

    // Nothing any refusal above named reached the store.
    for id in ["query-id", "payload-id", "one-predicate", "owner-mismatch"] {
        let read = get_metrics(&router, name, id).await;
        assert_eq!(
            read.error_code(),
            NO_SUCH_CONFIGURATION_CODE,
            "case {id:?}: a refused put records nothing, so the read must answer \
             {NO_SUCH_CONFIGURATION_CODE}, got {} ({})",
            read.error_code(),
            read.status
        );
    }
}

/// "You can have up to 1,000 metrics configurations per bucket": the 1,001st NEW
/// configuration is refused with the operation's own `TooManyConfigurations`, while
/// replacing one of the 1,000 creates none and still succeeds.
#[tokio::test]
async fn one_configuration_past_the_per_bucket_limit_is_refused() {
    let router = router();
    let name = "metrics-at-the-limit";
    bucket(&router, name).await;

    for index in 0..MAX_METRICS_CONFIGURATIONS_PER_BUCKET {
        let id = format!("configuration-{index}");
        let answer = put_metrics(&router, name, &id, None).await;
        assert_eq!(
            answer.status,
            PUT_BUCKET_METRICS_CONFIGURATION_SUCCESS_STATUS,
            "case {id:?}: a configuration within the limit must be recorded, got {} ({})",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    let past = put_metrics(&router, name, "one-too-many", None).await;
    assert_eq!(
        past.status, 400,
        "case \"one configuration past the limit\": expected HTTP 400, got {}",
        past.status
    );
    assert_eq!(
        past.error_code(),
        TOO_MANY_CONFIGURATIONS_CODE,
        "case \"one configuration past the limit\": expected the code \
         {TOO_MANY_CONFIGURATIONS_CODE}, got {}",
        past.error_code()
    );

    // A REPLACEMENT at the ceiling creates no new configuration, so the limit does not
    // refuse it.
    let replaced = Filter::Prefix("replaced/".to_string());
    let replacement = put_metrics(&router, name, "configuration-0", Some(&replaced)).await;
    assert_eq!(
        replacement.status,
        PUT_BUCKET_METRICS_CONFIGURATION_SUCCESS_STATUS,
        "case \"replacing a configuration at the limit\": expected HTTP \
         {PUT_BUCKET_METRICS_CONFIGURATION_SUCCESS_STATUS}, got {} ({})",
        replacement.status,
        String::from_utf8_lossy(&replacement.body)
    );
    let (_, reported) =
        reported_configuration(&get_metrics(&router, name, "configuration-0").await);
    assert_eq!(
        reported,
        Some(replaced),
        "case \"replacing a configuration at the limit\": expected the replacing filter, \
         got {reported:?}"
    );
}
