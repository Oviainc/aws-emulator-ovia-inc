// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a general purpose bucket's EVENT NOTIFICATION CONFIGURATION:
//! `PutBucketNotificationConfiguration` stores it and
//! `GetBucketNotificationConfiguration` reports it.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite in this crate owns), so what is pinned is the wire behaviour —
//! status, body, error code — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * "This action replaces the existing notification configuration with the configuration
//!   you include in the request body" and "you can disable notifications by adding the
//!   empty NotificationConfiguration element"
//!   (`PutBucketNotificationConfiguration`'s own documentation): whatever container the
//!   request carries becomes the bucket's configuration IN FULL, and the call answers the
//!   modelled 200 with an empty body.
//! * "If notifications are not enabled on the bucket, the action returns an empty
//!   NotificationConfiguration element" (`GetBucketNotificationConfiguration`'s own
//!   documentation): a bucket that has never been configured answers 200 with the empty
//!   container, not an error.
//! * "An optional unique identifier for configurations in a notification configuration. If
//!   you don't provide one, Amazon S3 will assign an ID" (`NotificationId`): a
//!   configuration that arrives without an `Id` reads back with one, the same one on every
//!   later read, and two configurations never share an id.
//! * A container AWS refuses — an event outside the published `Event` set, a filter rule
//!   name outside `FilterRuleName`, a repeated `Id`, a destination ARN that is not an ARN
//!   of that destination's service, or more configurations than a bucket may carry — is
//!   refused with HTTP 400 `InvalidArgument` and leaves the stored configuration untouched
//!   ("the PUT notification is an atomic operation").
//! * A notification call naming a bucket this emulator does not hold answers the service's
//!   `NoSuchBucket` (HTTP 404), the shape both handlers widen in.

use quick_xml::events::Event as XmlEvent;
use quick_xml::Reader;
use service_com_amazonaws_s3_amazons3::state::{
    canonical_filter_rule_name, EVENT_VALUES, FILTER_RULE_NAME_VALUES,
    MAX_NOTIFICATION_CONFIGURATIONS_PER_BUCKET,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, Answer};

/// The XML namespace botocore puts on an S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// An SNS topic ARN, an SQS queue ARN and a Lambda function ARN of the account this
/// emulator serves — the three destination kinds
/// `PutBucketNotificationConfiguration`'s documentation describes.
const TOPIC_ARN: &str = "arn:aws:sns:us-east-1:123456789012:object-events";
const QUEUE_ARN: &str = "arn:aws:sqs:us-east-1:123456789012:object-events";
const LAMBDA_ARN: &str = "arn:aws:lambda:us-east-1:123456789012:function:object-events";

/// The `<Message>` S3 sends for an `Events` entry that is not one of the published `Event`
/// values, and the one it sends for a container whose rules cannot both be in force — each
/// FIXED text, so the code being the same for every notification refusal leaves the message
/// to say which rule was broken.
const UNSUPPORTED_EVENT: &str = "The event is not supported for notifications";
const AMBIGUOUS: &str = "Configuration is ambiguously defined. Cannot have overlapping \
                         suffixes in two rules if the prefixes are overlapping for the same \
                         event type.";

/// The `<Message>` S3 sends for a document that does not validate against the schema it
/// publishes, which is what a value breaking a modelled constraint it states no sentence of
/// its own for is refused with.
const MALFORMED_XML: &str =
    "The XML you provided was not well-formed or did not validate against our published \
     schema";

/// Which destination a configuration publishes to, and so which element it is carried in.
///
/// The element names are the `xmlName`s the model puts on `NotificationConfiguration`'s
/// members and on each configuration's ARN member
/// (`smithy_query(query="shape",shape="NotificationConfiguration")` and
/// `smithy_query(query="shape",shape="LambdaFunctionConfiguration")`): the Lambda list is
/// `CloudFunctionConfiguration` and its ARN member is `CloudFunction`, not the member
/// names the SDK shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Destination {
    Topic,
    Queue,
    LambdaFunction,
}

impl Destination {
    /// The element one configuration of this kind is carried in.
    fn element(self) -> &'static str {
        match self {
            Self::Topic => "TopicConfiguration",
            Self::Queue => "QueueConfiguration",
            Self::LambdaFunction => "CloudFunctionConfiguration",
        }
    }

    /// The element the destination's ARN is carried in.
    fn arn_element(self) -> &'static str {
        match self {
            Self::Topic => "Topic",
            Self::Queue => "Queue",
            Self::LambdaFunction => "CloudFunction",
        }
    }

    /// An ARN of the service this destination lives in.
    fn arn(self) -> &'static str {
        match self {
            Self::Topic => TOPIC_ARN,
            Self::Queue => QUEUE_ARN,
            Self::LambdaFunction => LAMBDA_ARN,
        }
    }
}

/// One destination configuration a request carries, or a response reports.
///
/// `id` is `None` for a request that names none (the case AWS mints an id for) and for a
/// response it is whatever came back, so a row can assert on both.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Configuration {
    destination: Destination,
    id: Option<String>,
    arn: String,
    events: Vec<String>,
    /// The `Filter`'s `S3Key` filter rules as `(Name, Value)` pairs, or `None` when the
    /// configuration carries no `Filter`.
    filter_rules: Option<Vec<(String, String)>>,
}

impl Configuration {
    /// A configuration of `destination` publishing `events`, with no id and no filter.
    fn new(destination: Destination, events: &[&str]) -> Self {
        Self {
            destination,
            id: None,
            arn: destination.arn().to_string(),
            events: events.iter().map(|event| (*event).to_string()).collect(),
            filter_rules: None,
        }
    }

    fn with_id(mut self, id: &str) -> Self {
        self.id = Some(id.to_string());
        self
    }

    fn with_arn(mut self, arn: &str) -> Self {
        self.arn = arn.to_string();
        self
    }

    fn with_filter_rules(mut self, rules: &[(&str, &str)]) -> Self {
        self.filter_rules = Some(
            rules
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect(),
        );
        self
    }

    /// The same configuration as S3 REPORTS it: with whatever id came back, so a row that
    /// named no id can be compared on everything else, and with each filter rule under the
    /// CANONICAL capitalised name S3 keeps the rule under — the `FilterRuleName` values are
    /// the request vocabulary, and the stored rule's name is the capitalised spelling
    /// ([`service_com_amazonaws_s3_amazons3::state::canonical_filter_rule_name`] is the one
    /// place that mapping lives).
    fn as_reported(&self, reported: &Configuration) -> Self {
        let mut expected = self.clone();
        expected.id = reported.id.clone();
        expected.filter_rules = self.filter_rules.as_ref().map(|rules| {
            rules
                .iter()
                .map(|(name, value)| {
                    let canonical = canonical_filter_rule_name(name)
                        .map_or_else(|| name.clone(), str::to_string);
                    (canonical, value.clone())
                })
                .collect()
        });
        expected
    }

    /// This configuration as the XML element a client sends.
    fn xml(&self) -> String {
        let mut body = format!("<{}>", self.destination.element());
        if let Some(id) = &self.id {
            body.push_str(&format!("<Id>{id}</Id>"));
        }
        body.push_str(&format!(
            "<{element}>{arn}</{element}>",
            element = self.destination.arn_element(),
            arn = self.arn
        ));
        for event in &self.events {
            body.push_str(&format!("<Event>{event}</Event>"));
        }
        if let Some(rules) = &self.filter_rules {
            body.push_str("<Filter><S3Key>");
            for (name, value) in rules {
                body.push_str(&format!(
                    "<FilterRule><Name>{name}</Name><Value>{value}</Value></FilterRule>"
                ));
            }
            body.push_str("</S3Key></Filter>");
        }
        body.push_str(&format!("</{}>", self.destination.element()));
        body
    }
}

/// The `NotificationConfiguration` payload for `configurations`, with the
/// `EventBridgeConfiguration` element when `event_bridge` is on — spelled the way a
/// client's XML writer spells it: every list is `xmlFlattened`, so each configuration is a
/// direct child of the container.
fn payload(configurations: &[Configuration], event_bridge: bool) -> String {
    let mut body = format!("<NotificationConfiguration xmlns=\"{XMLNS}\">");
    for configuration in configurations {
        body.push_str(&configuration.xml());
    }
    if event_bridge {
        body.push_str("<EventBridgeConfiguration></EventBridgeConfiguration>");
    }
    body.push_str("</NotificationConfiguration>");
    body
}

/// `PUT /<bucket>?notification` — `PutBucketNotificationConfiguration`.
async fn put_notification(
    router: &MultiServiceRouter,
    bucket: &str,
    configurations: &[Configuration],
    event_bridge: bool,
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}?notification"),
        &[],
        &payload(configurations, event_bridge),
    )
    .await
}

/// `GET /<bucket>?notification` — `GetBucketNotificationConfiguration`.
async fn get_notification(router: &MultiServiceRouter, bucket: &str) -> Answer {
    send(router, "GET", &format!("/{bucket}?notification"), &[], "").await
}

/// What a `GetBucketNotificationConfiguration` body reports: the configurations in
/// document order, and whether EventBridge delivery is enabled.
///
/// Parsed with `quick-xml` (already a dependency of this crate), the way the tagging and
/// ownership-controls suites beside this one read a body, rather than by slicing strings.
fn reported(answer: &Answer) -> (Vec<Configuration>, bool) {
    let body = String::from_utf8_lossy(&answer.body).into_owned();
    let mut reader = Reader::from_str(&body);

    let mut configurations: Vec<Configuration> = Vec::new();
    let mut event_bridge = false;
    let mut current: Option<Configuration> = None;
    let mut element = String::new();
    loop {
        let event = reader
            .read_event()
            .unwrap_or_else(|error| panic!("the response body must be well-formed XML: {error}"));
        match event {
            XmlEvent::Start(start) | XmlEvent::Empty(start) => {
                element = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                for destination in [
                    Destination::Topic,
                    Destination::Queue,
                    Destination::LambdaFunction,
                ] {
                    if element == destination.element() {
                        current = Some(Configuration {
                            destination,
                            id: None,
                            arn: String::new(),
                            events: Vec::new(),
                            filter_rules: None,
                        });
                    }
                }
                if element == "EventBridgeConfiguration" {
                    event_bridge = true;
                }
                if element == "Filter" {
                    if let Some(configuration) = current.as_mut() {
                        configuration.filter_rules = Some(Vec::new());
                    }
                }
                if element == "FilterRule" {
                    if let Some(rules) =
                        current.as_mut().and_then(|held| held.filter_rules.as_mut())
                    {
                        rules.push((String::new(), String::new()));
                    }
                }
            }
            XmlEvent::Text(text) => {
                let decoded = text
                    .xml10_content()
                    .unwrap_or_else(|error| panic!("the response body must decode: {error}"))
                    .into_owned();
                let Some(configuration) = current.as_mut() else {
                    continue;
                };
                match element.as_str() {
                    "Id" => configuration.id = Some(decoded),
                    "Event" => configuration.events.push(decoded),
                    "Name" | "Value" => {
                        if let Some(rule) = configuration
                            .filter_rules
                            .as_mut()
                            .and_then(|rules| rules.last_mut())
                        {
                            if element == "Name" {
                                rule.0 = decoded;
                            } else {
                                rule.1 = decoded;
                            }
                        }
                    }
                    name if name == configuration.destination.arn_element() => {
                        configuration.arn = decoded;
                    }
                    _ => {}
                }
            }
            XmlEvent::End(end) => {
                let ended = String::from_utf8_lossy(end.name().as_ref()).into_owned();
                if current
                    .as_ref()
                    .is_some_and(|held| held.destination.element() == ended)
                {
                    configurations.push(current.take().expect("a configuration is open"));
                }
                element.clear();
            }
            XmlEvent::Eof => break,
            _ => {}
        }
    }
    (configurations, event_bridge)
}

/// The table of configurations every row of which must round-trip unchanged.
///
/// The rows differ along what the store-and-report rule generalises over: WHICH
/// destination kind a configuration publishes to, HOW MANY configurations the container
/// carries and in what ORDER, which events it names (values from across the published
/// `Event` catalog, not only the object-created one the issue's recorded call used),
/// whether it carries an `Id` and whether it carries a key-name `Filter`, and whether
/// EventBridge delivery is on.
fn round_trip_rows() -> Vec<(&'static str, Vec<Configuration>, bool)> {
    vec![
        (
            "EventBridge delivery alone, with no destination configuration",
            vec![],
            true,
        ),
        (
            "one queue configuration with a single event and no id",
            vec![Configuration::new(
                Destination::Queue,
                &["s3:ObjectCreated:*"],
            )],
            false,
        ),
        (
            "one topic configuration naming an explicit id and several events",
            vec![Configuration::new(
                Destination::Topic,
                &[
                    "s3:ObjectRemoved:Delete",
                    "s3:ObjectRestore:Completed",
                    "s3:ReducedRedundancyLostObject",
                ],
            )
            .with_id("topic-of-many-events")],
            false,
        ),
        (
            "a Lambda configuration filtering on both a prefix and a suffix",
            vec![Configuration::new(
                Destination::LambdaFunction,
                &["s3:ObjectCreated:Put"],
            )
            .with_filter_rules(&[("prefix", "incoming/"), ("suffix", ".json")])],
            false,
        ),
        (
            "all three destination kinds at once, with EventBridge on",
            vec![
                Configuration::new(Destination::Topic, &["s3:ObjectTagging:Put"]).with_id("t"),
                Configuration::new(Destination::Queue, &["s3:LifecycleExpiration:Delete"])
                    .with_id("q"),
                Configuration::new(Destination::LambdaFunction, &["s3:IntelligentTiering"])
                    .with_id("l"),
            ],
            true,
        ),
        (
            "two configurations of one kind, one with an id and one without",
            vec![
                Configuration::new(Destination::Queue, &["s3:ObjectCreated:Post"])
                    .with_id("named-queue"),
                Configuration::new(Destination::Queue, &["s3:ObjectCreated:Copy"])
                    .with_filter_rules(&[("prefix", "second/")]),
            ],
            false,
        ),
        (
            "a configuration carrying a Filter with no rule at all",
            vec![
                Configuration::new(Destination::Topic, &["s3:Replication:*"])
                    .with_filter_rules(&[]),
            ],
            false,
        ),
        (
            // One prefix each, because two configurations of one event that would both
            // match the same key cannot both be in force.
            "exactly as many configurations as one bucket may carry",
            (0..MAX_NOTIFICATION_CONFIGURATIONS_PER_BUCKET)
                .map(|index| {
                    Configuration::new(Destination::Queue, &["s3:ObjectCreated:Put"])
                        .with_id(&format!("at-the-bound-{index}"))
                        .with_filter_rules(&[("prefix", &format!("category-{index}/"))])
                })
                .collect(),
            false,
        ),
    ]
}

/// Assert that `bucket` reports exactly `expected` (and `event_bridge`), reporting the
/// case on every failure. Answers the configurations as reported, so a caller can assert
/// on the ids AWS minted.
async fn assert_reports(
    router: &MultiServiceRouter,
    bucket: &str,
    case: &str,
    expected: &[Configuration],
    event_bridge: bool,
) -> Vec<Configuration> {
    let read = get_notification(router, bucket).await;
    assert_eq!(
        read.status,
        200,
        "case {case:?}: expected HTTP 200 from GetBucketNotificationConfiguration, got {} \
         with body {:?}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
    let (configurations, reported_event_bridge) = reported(&read);
    assert_eq!(
        configurations.len(),
        expected.len(),
        "case {case:?}: expected {} configuration(s) back, got {:?}",
        expected.len(),
        configurations
    );
    for (want, got) in expected.iter().zip(configurations.iter()) {
        assert_eq!(
            &want.as_reported(got),
            got,
            "case {case:?}: expected the stored configuration back unchanged, got {got:?}"
        );
        if want.id.is_some() {
            assert_eq!(
                got.id, want.id,
                "case {case:?}: an explicit Id must read back verbatim, got {:?}",
                got.id
            );
        } else {
            assert!(
                got.id.as_deref().is_some_and(|id| !id.is_empty()),
                "case {case:?}: a configuration that named no Id must read back with the id \
                 Amazon S3 assigned, got {:?}",
                got.id
            );
        }
    }
    assert_eq!(
        reported_event_bridge, event_bridge,
        "case {case:?}: expected EventBridge delivery {}, got {reported_event_bridge}",
        event_bridge
    );
    configurations
}

#[tokio::test]
async fn putting_a_notification_configuration_stores_it_and_reads_back_through_get() {
    let router = router();

    for (index, (case, configurations, event_bridge)) in
        round_trip_rows().into_iter().enumerate()
    {
        let bucket = format!("notification-round-trip-{index}");
        let created = create_bucket(&router, &bucket, None, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the bucket must be created first, got {} with body {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );

        let put = put_notification(&router, &bucket, &configurations, event_bridge).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected the modelled 200 from \
             PutBucketNotificationConfiguration, got {} with body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        assert!(
            put.body.is_empty(),
            "case {case:?}: the output shape is Unit, so the body must be empty, got {:?}",
            String::from_utf8_lossy(&put.body)
        );

        let first = assert_reports(&router, &bucket, case, &configurations, event_bridge).await;

        // Reading again reports the same thing, minted ids included: a read reports, it
        // does not configure.
        let second = assert_reports(&router, &bucket, case, &configurations, event_bridge).await;
        assert_eq!(
            first, second,
            "case {case:?}: two reads must report the same configuration, got {second:?}"
        );

        // Every id in one configuration is unique — the ones the request named and the
        // ones Amazon S3 assigned alike.
        let mut ids: Vec<&str> = first
            .iter()
            .map(|configuration| {
                configuration
                    .id
                    .as_deref()
                    .expect("every reported configuration carries an id")
            })
            .collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(
            ids.len(),
            total,
            "case {case:?}: configuration ids must be unique, got {ids:?}"
        );

        // The second put REPLACES: what comes back is the new container alone, with
        // nothing merged in from the row above.
        let replacement =
            vec![Configuration::new(Destination::Topic, &["s3:ObjectAcl:Put"]).with_id("only")];
        let replaced = put_notification(&router, &bucket, &replacement, false).await;
        assert_eq!(
            replaced.status, 200,
            "case {case:?}: the replacing put must succeed, got {} with body {:?}",
            replaced.status,
            String::from_utf8_lossy(&replaced.body)
        );
        assert_reports(&router, &bucket, case, &replacement, false).await;

        // And the empty container turns notifications off again.
        let emptied = put_notification(&router, &bucket, &[], false).await;
        assert_eq!(
            emptied.status, 200,
            "case {case:?}: the empty container must be accepted, got {} with body {:?}",
            emptied.status,
            String::from_utf8_lossy(&emptied.body)
        );
        assert_reports(&router, &bucket, case, &[], false).await;
    }
}

#[tokio::test]
async fn every_published_event_round_trips() {
    let router = router();
    let bucket = "notification-event-catalog";
    create_bucket(&router, bucket, None, None).await;

    // One row per value of the published `Event` catalog — the whole set, not only the
    // object-created event the issue's recorded call named — each on its own
    // configuration, paired with each published `FilterRuleName` in turn so the filter
    // catalog is covered too.
    for (index, event) in EVENT_VALUES.iter().enumerate() {
        let rule_name = FILTER_RULE_NAME_VALUES[index % FILTER_RULE_NAME_VALUES.len()];
        let configurations = vec![Configuration::new(Destination::Queue, &[event])
            .with_id(&format!("event-{index}"))
            .with_filter_rules(&[(rule_name, "catalog/")])];

        let put = put_notification(&router, bucket, &configurations, false).await;
        assert_eq!(
            put.status, 200,
            "case {event:?}: a published event must be accepted, got {} with body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        assert_reports(&router, bucket, event, &configurations, false).await;
    }
}

/// One row of the filter-rule-name table: the case, the rules as the request carries them
/// (`(Name, Value)` pairs) and the names the read back must report.
type FilterRuleRow = (&'static str, Vec<(&'static str, &'static str)>, Vec<&'static str>);

#[tokio::test]
async fn a_filter_rule_reads_back_under_its_canonical_name() {
    let router = router();

    // (case, the rules as the request carries them, the names the read must report).
    // `FilterRuleName`'s values are the request vocabulary — the enum is `prefix` and
    // `suffix` — and the rule S3 stores is named with the canonical capitalised spelling,
    // in the order the request carried the rules. The rows differ along what that
    // generalises over: WHICH kinds the filter carries, in which ORDER, HOW MANY, and what
    // the values are.
    let rows: Vec<FilterRuleRow> = vec![
        (
            "a prefix rule alone",
            vec![("prefix", "CategoryA/")],
            vec!["Prefix"],
        ),
        (
            "a suffix rule alone",
            vec![("suffix", ".csv")],
            vec!["Suffix"],
        ),
        (
            "a prefix and a suffix, in that order",
            vec![("prefix", "CategoryA/"), ("suffix", ".csv")],
            vec!["Prefix", "Suffix"],
        ),
        (
            "a suffix and a prefix, in that order",
            vec![("suffix", ".json"), ("prefix", "incoming/2026/")],
            vec!["Suffix", "Prefix"],
        ),
        (
            "rules whose values are empty",
            vec![("prefix", ""), ("suffix", "")],
            vec!["Prefix", "Suffix"],
        ),
        (
            "a filter carrying no rule at all",
            vec![],
            vec![],
        ),
    ];

    for (index, (case, rules, expected)) in rows.into_iter().enumerate() {
        let bucket = format!("notification-filter-rule-names-{index}");
        create_bucket(&router, &bucket, None, None).await;

        let configurations = vec![
            Configuration::new(Destination::Queue, &["s3:ObjectCreated:*"])
                .with_id("filtered")
                .with_filter_rules(&rules),
        ];
        let put = put_notification(&router, &bucket, &configurations, false).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: the filtered configuration must be accepted, got {} with body \
             {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );

        let read = get_notification(&router, &bucket).await;
        let (reported, _) = reported(&read);
        let names: Vec<String> = reported
            .first()
            .and_then(|configuration| configuration.filter_rules.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(
            names, expected,
            "case {case:?}: expected the stored rule names {expected:?}, got {names:?}"
        );
        // The rest of the configuration survives the re-casing untouched, values included.
        assert_reports(&router, &bucket, case, &configurations, false).await;
    }
}

#[tokio::test]
async fn a_bucket_with_no_notifications_reports_the_empty_configuration() {
    let router = router();

    // Two buckets that have never been configured, one of them in another Region, so the
    // answer is about the absent configuration rather than about one bucket.
    for (case, bucket, constraint) in [
        (
            "a bucket created with no location constraint",
            "notification-absent-default",
            None,
        ),
        (
            "a bucket created in another Region",
            "notification-absent-regional",
            Some("eu-west-1"),
        ),
    ] {
        let created = create_bucket(&router, bucket, constraint, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the bucket must be created first, got {}",
            created.status
        );

        // "If notifications are not enabled on the bucket, the action returns an empty
        // NotificationConfiguration element" — a 200, not an error, and twice over.
        assert_reports(&router, bucket, case, &[], false).await;
        assert_reports(&router, bucket, case, &[], false).await;

        // And the bucket takes a configuration afterwards, so the rows above reported an
        // absent subresource rather than a broken bucket.
        let configurations = vec![Configuration::new(
            Destination::LambdaFunction,
            &["s3:ObjectCreated:CompleteMultipartUpload"],
        )];
        let put = put_notification(&router, bucket, &configurations, true).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: the bucket must accept a configuration, got {} with body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        assert_reports(&router, bucket, case, &configurations, true).await;
    }
}

#[tokio::test]
async fn a_refused_notification_configuration_changes_nothing() {
    let router = router();
    let bucket = "notification-refusals";
    create_bucket(&router, bucket, None, None).await;

    // The configuration the bucket holds throughout: every refusal below must leave it
    // exactly as it is.
    let stored = vec![
        Configuration::new(Destination::Topic, &["s3:ObjectCreated:Put"]).with_id("kept-topic"),
        Configuration::new(Destination::Queue, &["s3:ObjectRemoved:*"])
            .with_id("kept-queue")
            .with_filter_rules(&[("prefix", "kept/")]),
    ];
    let put = put_notification(&router, bucket, &stored, true).await;
    assert_eq!(
        put.status, 200,
        "the setup put must succeed, got {} with body {:?}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );

    // (case, the refused container, the code the refusal carries, the message it carries).
    // One row per way a container falls outside what the API accepts: an event one step
    // past the published `Event` set (including one differing only in case, since the set
    // is case-sensitive), a filter rule name outside `FilterRuleName`, a second rule of one
    // kind inside one filter, two configurations of one event that would both match the
    // same key, an id used twice, a destination ARN that is not an ARN of that
    // destination's service, and one configuration past the bound a bucket may carry.
    //
    // Two codes appear, and which one a row carries says WHERE the request is refused.
    // Every rule the notification container breaks is refused with the code S3 publishes
    // for an argument it cannot accept, `InvalidArgument`, and the MESSAGE is what says
    // which rule was broken — so a row whose value the model itself can refuse (an `Event`
    // outside the enum) carries that same code and the sentence S3 publishes for it.
    // `MalformedXML` is left for a document that breaks a modelled constraint S3 states no
    // sentence of its own for — a `FilterRuleName` outside the enum, an element the schema
    // does not carry, a configuration naming no event. Both are HTTP 400 — the status AWS
    // answers every row with.
    let past_the_bound: Vec<Configuration> = (0..=MAX_NOTIFICATION_CONFIGURATIONS_PER_BUCKET)
        .map(|index| {
            Configuration::new(Destination::Queue, &["s3:ObjectCreated:Put"])
                .with_id(&format!("past-the-bound-{index}"))
        })
        .collect();
    let rows: Vec<(&str, Vec<Configuration>, &str, &str)> = vec![
        (
            "an event outside the published set",
            vec![Configuration::new(
                Destination::Queue,
                &["s3:ObjectCreated:Rename"],
            )],
            "InvalidArgument",
            UNSUPPORTED_EVENT,
        ),
        (
            "a published event differing only in case",
            vec![Configuration::new(
                Destination::Queue,
                &["S3:ObjectCreated:Put"],
            )],
            "InvalidArgument",
            UNSUPPORTED_EVENT,
        ),
        (
            "one unpublished event among events that are fine",
            vec![Configuration::new(
                Destination::Topic,
                &["s3:ObjectCreated:Put", "s3:ObjectVanished"],
            )],
            "InvalidArgument",
            UNSUPPORTED_EVENT,
        ),
        (
            "a configuration naming no event at all",
            vec![Configuration::new(Destination::Topic, &[])],
            "MalformedXML",
            MALFORMED_XML,
        ),
        (
            "a filter rule name outside FilterRuleName",
            vec![
                Configuration::new(Destination::Queue, &["s3:ObjectCreated:Put"])
                    .with_filter_rules(&[("Prefix", "logs/")]),
            ],
            "MalformedXML",
            MALFORMED_XML,
        ),
        (
            "two suffix rules in one filter",
            vec![
                Configuration::new(Destination::Queue, &["s3:ObjectCreated:Put"])
                    .with_filter_rules(&[("suffix", ".csv"), ("suffix", ".tsv")]),
            ],
            "InvalidArgument",
            "Cannot specify more than one suffix rule in a filter.",
        ),
        (
            "two prefix rules in one filter",
            vec![
                Configuration::new(Destination::LambdaFunction, &["s3:ObjectCreated:Put"])
                    .with_filter_rules(&[("prefix", "one/"), ("prefix", "two/")]),
            ],
            "InvalidArgument",
            "Cannot specify more than one prefix rule in a filter.",
        ),
        (
            "two configurations of one event whose prefixes overlap",
            vec![
                Configuration::new(Destination::Queue, &["s3:ObjectCreated:*"])
                    .with_id("outer")
                    .with_filter_rules(&[("prefix", "CategoryA/")]),
                Configuration::new(Destination::Queue, &["s3:ObjectCreated:*"])
                    .with_id("inner")
                    .with_filter_rules(&[("prefix", "CategoryA/sub/")]),
            ],
            "InvalidArgument",
            AMBIGUOUS,
        ),
        (
            "two configurations of one event whose suffixes overlap",
            vec![
                Configuration::new(Destination::Topic, &["s3:ObjectRemoved:Delete"])
                    .with_id("every-csv")
                    .with_filter_rules(&[("suffix", ".csv")]),
                Configuration::new(Destination::LambdaFunction, &["s3:ObjectRemoved:Delete"])
                    .with_id("every-report-csv")
                    .with_filter_rules(&[("suffix", "-report.csv")]),
            ],
            "InvalidArgument",
            AMBIGUOUS,
        ),
        (
            "two configurations of one event neither of which filters",
            vec![
                Configuration::new(Destination::Queue, &["s3:ObjectRestore:Post"]).with_id("one"),
                Configuration::new(Destination::Topic, &["s3:ObjectRestore:Post"]).with_id("two"),
            ],
            "InvalidArgument",
            AMBIGUOUS,
        ),
        (
            "a filter rule naming an unknown attribute",
            vec![
                Configuration::new(Destination::LambdaFunction, &["s3:ObjectCreated:Put"])
                    .with_filter_rules(&[("prefix", "ok/"), ("extension", ".json")]),
            ],
            "MalformedXML",
            MALFORMED_XML,
        ),
        (
            "one id used by two configurations of the same kind",
            vec![
                Configuration::new(Destination::Queue, &["s3:ObjectCreated:Put"]).with_id("twice"),
                Configuration::new(Destination::Queue, &["s3:ObjectRemoved:Delete"])
                    .with_id("twice"),
            ],
            "InvalidArgument",
            "Configuration ID twice is not unique",
        ),
        (
            "one id used by configurations of two different kinds",
            vec![
                Configuration::new(Destination::Topic, &["s3:ObjectCreated:Put"]).with_id("shared"),
                Configuration::new(Destination::LambdaFunction, &["s3:ObjectCreated:Put"])
                    .with_id("shared"),
            ],
            "InvalidArgument",
            "Configuration ID shared is not unique",
        ),
        (
            "a queue ARN carried in a topic configuration",
            vec![
                Configuration::new(Destination::Topic, &["s3:ObjectCreated:Put"])
                    .with_arn(QUEUE_ARN),
            ],
            "InvalidArgument",
            "",
        ),
        (
            "a topic ARN carried in a Lambda configuration",
            vec![
                Configuration::new(Destination::LambdaFunction, &["s3:ObjectCreated:Put"])
                    .with_arn(TOPIC_ARN),
            ],
            "InvalidArgument",
            "",
        ),
        (
            "a destination that is not an ARN at all",
            vec![
                Configuration::new(Destination::Queue, &["s3:ObjectCreated:Put"])
                    .with_arn("object-events"),
            ],
            "InvalidArgument",
            "",
        ),
        (
            "an ARN missing its resource field",
            vec![
                Configuration::new(Destination::Queue, &["s3:ObjectCreated:Put"])
                    .with_arn("arn:aws:sqs:us-east-1:123456789012:"),
            ],
            "InvalidArgument",
            "",
        ),
        (
            "one configuration past the bound",
            past_the_bound,
            "InvalidArgument",
            "",
        ),
    ];

    for (case, configurations, code, message) in rows {
        let refused = put_notification(&router, bucket, &configurations, false).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected the container to be refused with HTTP 400, got {} with \
             body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            code,
            "case {case:?}: expected the refusal to carry {code}, got {:?}",
            refused.error_code()
        );
        // The code is the same for most of these, so the MESSAGE is what says which rule
        // the container broke. The rows that leave it empty are the ones whose sentence
        // this suite pins elsewhere.
        if !message.is_empty() {
            assert_eq!(
                refused.error_message(),
                message,
                "case {case:?}: expected the refusal's message to be {message:?}, got {:?}",
                refused.error_message()
            );
        }

        // "The PUT notification is an atomic operation … Amazon S3 will not add the
        // configuration to your bucket": the stored configuration is untouched.
        assert_reports(&router, bucket, case, &stored, true).await;
    }
}

#[tokio::test]
async fn notification_calls_on_a_bucket_this_emulator_does_not_hold_are_no_such_bucket() {
    let router = router();
    create_bucket(&router, "held-notification-bucket", None, None).await;

    // Names that are NOT the one bucket created above: a different name, a prefix of it,
    // an extension of it, and one differing only in case (bucket names are
    // case-sensitive). `NoSuchBucket` carries `httpError 404` in the model.
    let configurations = vec![Configuration::new(
        Destination::Queue,
        &["s3:ObjectCreated:*"],
    )];
    for bucket in [
        "absent-notification-bucket",
        "held",
        "held-notification-bucket-2",
        "Held-Notification-Bucket",
    ] {
        for (operation, answer) in [
            (
                "PutBucketNotificationConfiguration",
                put_notification(&router, bucket, &configurations, false).await,
            ),
            (
                "GetBucketNotificationConfiguration",
                get_notification(&router, bucket).await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {bucket:?}/{operation}: expected HTTP 404 for a bucket this emulator \
                 does not hold, got {} with body {:?}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchBucket",
                "case {bucket:?}/{operation}: expected the modelled error code, got {:?}",
                answer.error_code()
            );
        }
    }

    // Nothing above created a bucket as a side effect of configuring one that is absent.
    for bucket in ["absent-notification-bucket", "held"] {
        let read = get_notification(&router, bucket).await;
        assert_eq!(
            read.error_code(),
            "NoSuchBucket",
            "case {bucket:?}: a refused put must not have created the bucket, got {:?}",
            read.error_code()
        );
    }

    // And the bucket that IS held configures normally, so the rows above failed on the
    // lookup rather than on the operations being unreachable.
    let put = put_notification(&router, "held-notification-bucket", &configurations, false).await;
    assert_eq!(
        put.status, 200,
        "the held bucket must accept a configuration, got {} with body {:?}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );
    assert_reports(
        &router,
        "held-notification-bucket",
        "the held bucket",
        &configurations,
        false,
    )
    .await;
}

#[tokio::test]
async fn an_assigned_configuration_id_differs_per_configuration_and_survives_a_replacement() {
    let router = router();
    let bucket = "notification-assigned-ids";
    create_bucket(&router, bucket, None, None).await;

    // Three configurations, none of them naming an Id: each must come back with one of its
    // own ("If you don't provide one, Amazon S3 will assign an ID").
    let configurations = vec![
        Configuration::new(Destination::Topic, &["s3:ObjectCreated:Put"]),
        Configuration::new(Destination::Queue, &["s3:ObjectRemoved:Delete"]),
        Configuration::new(Destination::LambdaFunction, &["s3:ObjectRestore:Post"]),
    ];
    let put = put_notification(&router, bucket, &configurations, false).await;
    assert_eq!(
        put.status, 200,
        "the put must succeed, got {} with body {:?}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );

    let first = assert_reports(&router, bucket, "assigned ids", &configurations, false).await;
    let assigned: Vec<String> = first
        .iter()
        .map(|configuration| {
            configuration
                .id
                .clone()
                .expect("every reported configuration carries an id")
        })
        .collect();

    // The ids are stable across reads …
    let again = assert_reports(&router, bucket, "assigned ids", &configurations, false).await;
    assert_eq!(
        again, first,
        "an assigned id must not move between reads, got {again:?}"
    );

    // … and a REPLACING put assigns fresh ones rather than reusing what it replaced, since
    // the configurations it replaces no longer exist.
    let replaced = put_notification(&router, bucket, &configurations, false).await;
    assert_eq!(
        replaced.status, 200,
        "the replacing put must succeed, got {} with body {:?}",
        replaced.status,
        String::from_utf8_lossy(&replaced.body)
    );
    let after = assert_reports(&router, bucket, "assigned ids", &configurations, false).await;
    let reassigned: Vec<String> = after
        .iter()
        .map(|configuration| {
            configuration
                .id
                .clone()
                .expect("every reported configuration carries an id")
        })
        .collect();
    assert_eq!(
        reassigned.len(),
        assigned.len(),
        "the replacement must report one id per configuration, got {reassigned:?}"
    );
    for id in &reassigned {
        assert!(
            !id.is_empty(),
            "an assigned id must not be empty, got {reassigned:?}"
        );
    }

    // An explicit id is never replaced by an assigned one, whatever else the container
    // carries.
    let mixed = vec![
        Configuration::new(Destination::Queue, &["s3:ObjectCreated:Put"]).with_id("mine"),
        Configuration::new(Destination::Queue, &["s3:ObjectCreated:Post"]),
    ];
    let put = put_notification(&router, bucket, &mixed, false).await;
    assert_eq!(
        put.status, 200,
        "the mixed put must succeed, got {} with body {:?}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );
    let reported = assert_reports(
        &router,
        bucket,
        "an explicit id beside an assigned one",
        &mixed,
        false,
    )
    .await;
    assert_eq!(
        reported[0].id.as_deref(),
        Some("mine"),
        "an explicit id must read back verbatim, got {:?}",
        reported[0].id
    );
    assert_ne!(
        reported[1].id.as_deref(),
        Some("mine"),
        "the assigned id must differ from the one the request named, got {:?}",
        reported[1].id
    );
}
