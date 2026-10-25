// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a general purpose bucket's S3 INTELLIGENT-TIERING
//! CONFIGURATIONS: `PutBucketIntelligentTieringConfiguration` records one under the id the
//! request names and `ListBucketIntelligentTieringConfigurations` reports the ones the
//! bucket carries.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite owns), so what is pinned is the wire behaviour — status, body,
//! error code — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * A bucket carries MANY configurations, each identified by its own id: "Puts a S3
//!   Intelligent-Tiering configuration to the specified bucket. You can have up to 1,000 S3
//!   Intelligent-Tiering configurations per bucket"
//!   (`PutBucketIntelligentTieringConfiguration`'s own documentation), so a put under a new
//!   id ADDS one and a put under an id already carried replaces that one alone.
//! * The payload IS the configuration: what a put names is what the listing reports back,
//!   member for member, and the configuration's own `Id` is the id the request identifies
//!   ("The ID used to identify the S3 Intelligent-Tiering configuration").
//! * The listing reports the configurations the bucket carries, in ascending id order, and
//!   PAGES: `IsTruncated` "indicates whether the returned list … is complete. A value of
//!   true indicates that the list is not complete and the NextContinuationToken will be
//!   provided for a subsequent request", and the request's `ContinuationToken` comes back
//!   on the response.
//! * A tiering's `Days` is bounded by what that member documents: "The minimum number of
//!   days specified for Archive Access tier must be at least 90 days and Deep Archive
//!   Access tier must be at least 180 days. The maximum can be up to 2 years (730 days)" —
//!   a value outside the range, a `Status` or an `AccessTier` outside its enum, or a
//!   payload `Id` that is not the one the request identifies, is refused with
//!   `InvalidArgument` (HTTP 400) and nothing is recorded.
//! * A call naming a bucket this emulator does not hold answers `NoSuchBucket` (HTTP 404);
//!   one naming an `ExpectedBucketOwner` that is not the account's answers HTTP 403
//!   `AccessDenied`.
//! * The listing is a REPORT: two identical listings of an unchanged bucket answer the
//!   same thing.

use quick_xml::events::Event;
use quick_xml::Reader;
use service_com_amazonaws_s3_amazons3::state::{
    INVALID_ARGUMENT_CODE, MAX_INTELLIGENT_TIERING_CONFIGURATIONS_PER_BUCKET,
    NO_SUCH_BUCKET_CODE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, Answer};

/// The XML namespace botocore puts on an S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The two values `IntelligentTieringAccessTier` takes, and the fewest days `Tiering$Days`
/// documents for each: "the minimum number of days specified for Archive Access tier must
/// be at least 90 days and Deep Archive Access tier must be at least 180 days"
/// (`smithy_query(query="doc",shape="Tiering",member="Days")`, whose enum values come from
/// `smithy_query(query="shape",shape="IntelligentTieringAccessTier")`).
const ACCESS_TIER_FLOORS: [(&str, i32); 2] = [("ARCHIVE_ACCESS", 90), ("DEEP_ARCHIVE_ACCESS", 180)];

/// "The maximum can be up to 2 years (730 days)" — the same documentation, for either
/// tier.
const MAX_DAYS: i32 = 730;

/// One `Tiering` of a configuration: the days of no access and the tier they transition
/// to.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TieringRow {
    days: i32,
    access_tier: String,
}

impl TieringRow {
    fn new(days: i32, access_tier: &str) -> Self {
        Self {
            days,
            access_tier: access_tier.to_string(),
        }
    }

    fn xml(&self) -> String {
        format!(
            "<Tiering><Days>{}</Days><AccessTier>{}</AccessTier></Tiering>",
            self.days, self.access_tier
        )
    }
}

/// An `IntelligentTieringFilter`, in the three forms the shape allows: a `Prefix`, a single
/// `Tag`, or an `And` operator carrying a prefix and tags
/// (`smithy_query(query="shape",shape="IntelligentTieringFilter")`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct FilterRow {
    prefix: Option<String>,
    tag: Option<(String, String)>,
    and_prefix: Option<String>,
    and_tags: Vec<(String, String)>,
    /// Whether the filter carries an `And` container at all, so a filter whose `And` names
    /// only a prefix is distinguishable from one naming that prefix directly.
    has_and: bool,
}

impl FilterRow {
    fn prefix(prefix: &str) -> Self {
        Self {
            prefix: Some(prefix.to_string()),
            ..Self::default()
        }
    }

    fn tag(key: &str, value: &str) -> Self {
        Self {
            tag: Some((key.to_string(), value.to_string())),
            ..Self::default()
        }
    }

    fn and(prefix: Option<&str>, tags: &[(&str, &str)]) -> Self {
        Self {
            and_prefix: prefix.map(str::to_string),
            and_tags: tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
            has_and: true,
            ..Self::default()
        }
    }

    fn xml(&self) -> String {
        let mut body = String::from("<Filter>");
        if let Some(prefix) = &self.prefix {
            body.push_str(&format!("<Prefix>{prefix}</Prefix>"));
        }
        if let Some((key, value)) = &self.tag {
            body.push_str(&format!(
                "<Tag><Key>{key}</Key><Value>{value}</Value></Tag>"
            ));
        }
        if self.has_and {
            body.push_str("<And>");
            if let Some(prefix) = &self.and_prefix {
                body.push_str(&format!("<Prefix>{prefix}</Prefix>"));
            }
            for (key, value) in &self.and_tags {
                body.push_str(&format!(
                    "<Tag><Key>{key}</Key><Value>{value}</Value></Tag>"
                ));
            }
            body.push_str("</And>");
        }
        body.push_str("</Filter>");
        body
    }
}

/// One `IntelligentTieringConfiguration`, as a request carries it and as the listing brings
/// it back.
///
/// The same type on both sides, so a round-trip assertion compares what was sent with what
/// was reported rather than with a hand-spelled expectation. The members are the ones
/// `smithy_query(query="shape",shape="IntelligentTieringConfiguration")` declares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ConfigurationRow {
    id: String,
    filter: Option<FilterRow>,
    status: String,
    tierings: Vec<TieringRow>,
}

impl ConfigurationRow {
    /// A configuration with the two required scalars and one tiering at its tier's floor.
    fn enabled(id: &str) -> Self {
        Self {
            id: id.to_string(),
            filter: None,
            status: "Enabled".to_string(),
            tierings: vec![TieringRow::new(
                ACCESS_TIER_FLOORS[0].1,
                ACCESS_TIER_FLOORS[0].0,
            )],
        }
    }

    fn with_status(mut self, status: &str) -> Self {
        self.status = status.to_string();
        self
    }

    fn with_filter(mut self, filter: FilterRow) -> Self {
        self.filter = Some(filter);
        self
    }

    fn with_tierings(mut self, tierings: Vec<TieringRow>) -> Self {
        self.tierings = tierings;
        self
    }

    /// The configuration as XML, in the member order the shape declares. `Tierings` is
    /// `xmlFlattened` with `xmlName` `Tiering`, so each entry is a bare `<Tiering>`.
    fn xml(&self) -> String {
        let mut body = format!("<IntelligentTieringConfiguration xmlns=\"{XMLNS}\">");
        body.push_str(&format!("<Id>{}</Id>", self.id));
        if let Some(filter) = &self.filter {
            body.push_str(&filter.xml());
        }
        body.push_str(&format!("<Status>{}</Status>", self.status));
        for tiering in &self.tierings {
            body.push_str(&tiering.xml());
        }
        body.push_str("</IntelligentTieringConfiguration>");
        body
    }
}

/// `PUT /<bucket>?intelligent-tiering&id=<id>` — `PutBucketIntelligentTieringConfiguration`
/// with `configuration` as its payload.
async fn put_configuration(
    router: &MultiServiceRouter,
    bucket: &str,
    id: &str,
    configuration: &ConfigurationRow,
) -> Answer {
    put_payload(router, bucket, id, &configuration.xml(), &[]).await
}

/// The same put with the payload spelled out and any extra bound headers, for a container
/// no [`ConfigurationRow`] can express.
async fn put_payload(
    router: &MultiServiceRouter,
    bucket: &str,
    id: &str,
    payload: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}?intelligent-tiering&id={id}"),
        headers,
        payload,
    )
    .await
}

/// `GET /<bucket>?intelligent-tiering` — `ListBucketIntelligentTieringConfigurations`,
/// optionally continuing at `token`.
async fn list_configurations(
    router: &MultiServiceRouter,
    bucket: &str,
    token: Option<&str>,
    headers: &[(&str, &str)],
) -> Answer {
    let uri = match token {
        None => format!("/{bucket}?intelligent-tiering"),
        Some(token) => format!("/{bucket}?intelligent-tiering&continuation-token={token}"),
    };
    send(router, "GET", &uri, headers, "").await
}

/// The configurations a listing body reports, in document order.
///
/// Parsed with `quick-xml` (already a dependency of this workspace), the way the
/// replication suite beside this one reads a container, rather than by slicing the body.
fn reported_configurations(answer: &Answer) -> Vec<ConfigurationRow> {
    let body = String::from_utf8_lossy(&answer.body).into_owned();
    let mut reader = Reader::from_str(&body);

    let mut configurations: Vec<ConfigurationRow> = Vec::new();
    let mut path: Vec<String> = Vec::new();
    loop {
        match reader
            .read_event()
            .unwrap_or_else(|error| panic!("the response body must be well-formed XML: {error}"))
        {
            Event::Start(start) => {
                let name = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                match name.as_str() {
                    "IntelligentTieringConfiguration" => {
                        configurations.push(ConfigurationRow::default())
                    }
                    "Filter" => {
                        if let Some(configuration) = configurations.last_mut() {
                            configuration.filter = Some(FilterRow::default());
                        }
                    }
                    "And" => {
                        if let Some(filter) = configurations
                            .last_mut()
                            .and_then(|configuration| configuration.filter.as_mut())
                        {
                            filter.has_and = true;
                        }
                    }
                    "Tiering" => {
                        if let Some(configuration) = configurations.last_mut() {
                            configuration
                                .tierings
                                .push(TieringRow::new(0, ""));
                        }
                    }
                    "Tag" => {
                        let under_and = path.iter().any(|element| element == "And");
                        if let Some(filter) = configurations
                            .last_mut()
                            .and_then(|configuration| configuration.filter.as_mut())
                        {
                            if under_and {
                                filter.and_tags.push((String::new(), String::new()));
                            } else {
                                filter.tag = Some((String::new(), String::new()));
                            }
                        }
                    }
                    _ => {}
                }
                path.push(name);
            }
            Event::Text(text) => {
                let value = text
                    .xml10_content()
                    .unwrap_or_else(|error| panic!("the response body must decode: {error}"))
                    .into_owned();
                let under_and = path.iter().any(|element| element == "And");
                let here: Vec<&str> = path.iter().map(String::as_str).collect();
                let Some(configuration) = configurations.last_mut() else {
                    continue;
                };
                // The member a value belongs to is its PATH, not its tag: `Prefix` sits
                // both directly on a `Filter` and inside its `And`, and `Tag` likewise.
                match here[here.len().saturating_sub(2)..] {
                    ["IntelligentTieringConfiguration", "Id"] => configuration.id = value,
                    ["IntelligentTieringConfiguration", "Status"] => configuration.status = value,
                    ["Tiering", "Days"] => {
                        if let Some(tiering) = configuration.tierings.last_mut() {
                            tiering.days = value.parse().unwrap_or_default();
                        }
                    }
                    ["Tiering", "AccessTier"] => {
                        if let Some(tiering) = configuration.tierings.last_mut() {
                            tiering.access_tier = value;
                        }
                    }
                    ["Filter", "Prefix"] | ["And", "Prefix"] => {
                        if let Some(filter) = configuration.filter.as_mut() {
                            if under_and {
                                filter.and_prefix = Some(value);
                            } else {
                                filter.prefix = Some(value);
                            }
                        }
                    }
                    ["Tag", "Key"] => {
                        if let Some(filter) = configuration.filter.as_mut() {
                            if under_and {
                                if let Some(tag) = filter.and_tags.last_mut() {
                                    tag.0 = value;
                                }
                            } else if let Some(tag) = filter.tag.as_mut() {
                                tag.0 = value;
                            }
                        }
                    }
                    ["Tag", "Value"] => {
                        if let Some(filter) = configuration.filter.as_mut() {
                            if under_and {
                                if let Some(tag) = filter.and_tags.last_mut() {
                                    tag.1 = value;
                                }
                            } else if let Some(tag) = filter.tag.as_mut() {
                                tag.1 = value;
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
    configurations
}

/// Whether a listing reports itself truncated.
fn is_truncated(answer: &Answer) -> bool {
    answer
        .element("IsTruncated")
        .unwrap_or_else(|| panic!("a listing reports IsTruncated, got {:?}", body_of(answer)))
        == "true"
}

/// The listing body as text, for an assertion message.
fn body_of(answer: &Answer) -> String {
    String::from_utf8_lossy(&answer.body).into_owned()
}

/// Stand up a bucket named `name`, which is what a configuration may be put on.
async fn bucket(router: &MultiServiceRouter, name: &str) {
    let created = create_bucket(router, name, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket {name:?} must be created, got {}",
        created.status
    );
}

/// The table of configurations every row of which must round-trip through the bucket
/// unchanged.
///
/// The rows differ along what the store-and-report rule generalises over: which `Status`
/// the configuration carries; whether it names no filter, a prefix filter, a single tag, or
/// an `And` operator over a prefix and one or more tags; how MANY tierings it carries,
/// which tier each names, and whether a tiering's `Days` sits at its tier's documented
/// floor, in between, or at the published maximum.
fn round_trip_rows() -> Vec<(&'static str, ConfigurationRow)> {
    let (archive, archive_floor) = ACCESS_TIER_FLOORS[0];
    let (deep, deep_floor) = ACCESS_TIER_FLOORS[1];
    vec![
        (
            "the required members only, archive access at its floor",
            ConfigurationRow::enabled("minimal"),
        ),
        (
            "a disabled configuration",
            ConfigurationRow::enabled("disabled").with_status("Disabled"),
        ),
        (
            "deep archive access at its own floor",
            ConfigurationRow::enabled("deep-floor")
                .with_tierings(vec![TieringRow::new(deep_floor, deep)]),
        ),
        (
            "both tiers, the second at the published maximum",
            ConfigurationRow::enabled("both-tiers").with_tierings(vec![
                TieringRow::new(archive_floor, archive),
                TieringRow::new(MAX_DAYS, deep),
            ]),
        ),
        (
            "a prefix filter and a mid-range number of days",
            ConfigurationRow::enabled("prefix-filtered")
                .with_filter(FilterRow::prefix("logs/2026/"))
                .with_tierings(vec![TieringRow::new(365, archive)]),
        ),
        (
            "a single-tag filter",
            ConfigurationRow::enabled("tag-filtered")
                .with_filter(FilterRow::tag("archive", "yes")),
        ),
        (
            "an And operator over a prefix and two tags",
            ConfigurationRow::enabled("and-filtered")
                .with_filter(FilterRow::and(
                    Some("cold/"),
                    &[("tier", "deep"), ("team", "analytics")],
                ))
                .with_tierings(vec![TieringRow::new(deep_floor, deep)]),
        ),
        (
            "an And operator over tags alone",
            ConfigurationRow::enabled("and-tags-only")
                .with_filter(FilterRow::and(None, &[("retain", "long")]))
                .with_status("Disabled"),
        ),
    ]
}

#[tokio::test]
async fn a_configuration_round_trips_through_the_bucket() {
    let router = router();

    for (index, (case, configuration)) in round_trip_rows().into_iter().enumerate() {
        let name = format!("itc-round-trip-{index}");
        bucket(&router, &name).await;

        let put = put_configuration(&router, &name, &configuration.id, &configuration).await;
        assert_eq!(
            put.status,
            200,
            "case {case:?}: expected the put to answer 200, got {} ({})",
            put.status,
            body_of(&put)
        );
        assert!(
            put.body.is_empty(),
            "case {case:?}: a successful put carries no body, got {:?}",
            body_of(&put)
        );

        let listed = list_configurations(&router, &name, None, &[]).await;
        assert_eq!(
            listed.status,
            200,
            "case {case:?}: expected the listing to answer 200, got {} ({})",
            listed.status,
            body_of(&listed)
        );
        let reported = reported_configurations(&listed);
        assert_eq!(
            reported,
            vec![configuration.clone()],
            "case {case:?}: expected the configuration that was put, got {reported:?}"
        );
        assert!(
            !is_truncated(&listed),
            "case {case:?}: a single-configuration listing is complete, got {:?}",
            body_of(&listed)
        );

        // A listing is a REPORT: reading it again answers the same thing.
        let again = reported_configurations(&list_configurations(&router, &name, None, &[]).await);
        assert_eq!(
            again, reported,
            "case {case:?}: a second listing must report the same configurations, got {again:?}"
        );
    }
}

#[tokio::test]
async fn a_bucket_carries_one_configuration_per_id() {
    let router = router();
    let name = "itc-many-ids";
    bucket(&router, name).await;

    let empty = list_configurations(&router, name, None, &[]).await;
    assert_eq!(
        empty.status,
        200,
        "a bucket carrying no configuration still lists, got {} ({})",
        empty.status,
        body_of(&empty)
    );
    let nothing = reported_configurations(&empty);
    assert!(
        nothing.is_empty(),
        "a bucket carrying no configuration reports none, got {nothing:?}"
    );

    // Put them in an order that is NOT the id order, so what is asserted is the listing's
    // own ordering rather than the order of arrival.
    let arrival = ["zeta", "alpha", "mu"];
    for id in arrival {
        let put = put_configuration(&router, name, id, &ConfigurationRow::enabled(id)).await;
        assert_eq!(
            put.status,
            200,
            "case {id:?}: expected the put to answer 200, got {} ({})",
            put.status,
            body_of(&put)
        );
    }

    let listed = reported_configurations(&list_configurations(&router, name, None, &[]).await);
    let ids: Vec<&str> = listed
        .iter()
        .map(|configuration| configuration.id.as_str())
        .collect();
    assert_eq!(
        ids,
        vec!["alpha", "mu", "zeta"],
        "case {arrival:?}: expected every configuration once, in ascending id order, got {ids:?}"
    );

    // A put under an id the bucket already carries replaces THAT configuration and leaves
    // the others exactly as they were.
    let replacement = ConfigurationRow::enabled("mu")
        .with_status("Disabled")
        .with_filter(FilterRow::prefix("replaced/"));
    let again = put_configuration(&router, name, "mu", &replacement).await;
    assert_eq!(
        again.status,
        200,
        "case \"replace mu\": expected the put to answer 200, got {} ({})",
        again.status,
        body_of(&again)
    );
    let after = reported_configurations(&list_configurations(&router, name, None, &[]).await);
    assert_eq!(
        after,
        vec![
            ConfigurationRow::enabled("alpha"),
            replacement,
            ConfigurationRow::enabled("zeta"),
        ],
        "case \"replace mu\": expected the replacement in place of mu alone, got {after:?}"
    );
}

#[tokio::test]
async fn the_listing_pages_over_the_configurations() {
    let router = router();
    let name = "itc-paging";
    bucket(&router, name).await;

    // Enough configurations that the listing cannot report them in one page: the ids are
    // zero-padded so ascending id order is also ascending numeric order.
    let held = 101;
    for index in 0..held {
        let id = format!("configuration-{index:04}");
        let put = put_configuration(&router, name, &id, &ConfigurationRow::enabled(&id)).await;
        assert_eq!(
            put.status,
            200,
            "case {id:?}: expected the put to answer 200, got {} ({})",
            put.status,
            body_of(&put)
        );
    }

    let first = list_configurations(&router, name, None, &[]).await;
    assert_eq!(
        first.status,
        200,
        "case \"first page\": expected 200, got {} ({})",
        first.status,
        body_of(&first)
    );
    let first_page = reported_configurations(&first);
    assert!(
        first_page.len() < held && !first_page.is_empty(),
        "case \"first page\": expected a partial page of the {held} configurations, got {}",
        first_page.len()
    );
    assert!(
        is_truncated(&first),
        "case \"first page\": expected IsTruncated true, got {:?}",
        body_of(&first)
    );
    assert!(
        first.element("ContinuationToken").is_none(),
        "case \"first page\": a request carrying no token is answered with none, got {:?}",
        first.element("ContinuationToken")
    );
    let token = first.element("NextContinuationToken").unwrap_or_else(|| {
        panic!(
            "case \"first page\": a truncated listing hands back a token, got {:?}",
            body_of(&first)
        )
    });

    let second = list_configurations(&router, name, Some(&token), &[]).await;
    assert_eq!(
        second.status,
        200,
        "case \"second page\": expected 200, got {} ({})",
        second.status,
        body_of(&second)
    );
    let second_page = reported_configurations(&second);
    assert!(
        !is_truncated(&second),
        "case \"second page\": the rest fits in one page, got {:?}",
        body_of(&second)
    );
    assert_eq!(
        second.element("ContinuationToken").as_deref(),
        Some(token.as_str()),
        "case \"second page\": expected the request's token reported back, got {:?}",
        second.element("ContinuationToken")
    );
    assert!(
        second.element("NextContinuationToken").is_none(),
        "case \"second page\": a complete listing hands back no token, got {:?}",
        second.element("NextContinuationToken")
    );

    // The two pages together are every configuration, each exactly once and in order.
    let paged: Vec<String> = first_page
        .iter()
        .chain(second_page.iter())
        .map(|configuration| configuration.id.clone())
        .collect();
    let expected: Vec<String> = (0..held)
        .map(|index| format!("configuration-{index:04}"))
        .collect();
    assert_eq!(
        paged, expected,
        "case \"both pages\": expected every configuration once, in id order, got {paged:?}"
    );

    // A token this listing never issued is not a cursor.
    let refused = list_configurations(&router, name, Some("not-a-token"), &[]).await;
    assert_eq!(
        refused.status,
        400,
        "case \"forged token\": expected 400, got {} ({})",
        refused.status,
        body_of(&refused)
    );
    assert_eq!(
        refused.error_code(),
        INVALID_ARGUMENT_CODE,
        "case \"forged token\": expected {INVALID_ARGUMENT_CODE}, got {}",
        refused.error_code()
    );
}

/// One refused-put row: the case, the id the request identifies, the payload it carries and
/// the error code the refusal must answer with — `None` where the value is refused at the
/// request boundary against the model (an enum member outside its value set), whose code is
/// the framework's own, so the row pins the 400 and that nothing was recorded.
type RefusalRow<'a> = (&'a str, &'a str, String, Option<&'static str>);

/// The puts that must be refused, spanning the rule each refusal states: the payload id
/// must be the one the request identifies, `Status` and `AccessTier` must name a value of
/// their enum, and `Days` must sit inside the range its documentation publishes — one row
/// just below each tier's floor and one just past the maximum.
fn refusal_rows() -> Vec<RefusalRow<'static>> {
    let (archive, archive_floor) = ACCESS_TIER_FLOORS[0];
    let (deep, deep_floor) = ACCESS_TIER_FLOORS[1];
    vec![
        (
            "a payload naming another id",
            "wanted",
            ConfigurationRow::enabled("other").xml(),
            Some(INVALID_ARGUMENT_CODE),
        ),
        (
            "a status outside the enum",
            "bad-status",
            ConfigurationRow::enabled("bad-status")
                .with_status("Paused")
                .xml(),
            None,
        ),
        (
            "an access tier outside the enum",
            "bad-tier",
            ConfigurationRow::enabled("bad-tier")
                .with_tierings(vec![TieringRow::new(200, "FROZEN_ACCESS")])
                .xml(),
            None,
        ),
        (
            "archive access one day below its floor",
            "archive-too-soon",
            ConfigurationRow::enabled("archive-too-soon")
                .with_tierings(vec![TieringRow::new(archive_floor - 1, archive)])
                .xml(),
            Some(INVALID_ARGUMENT_CODE),
        ),
        (
            "deep archive access one day below its floor",
            "deep-too-soon",
            ConfigurationRow::enabled("deep-too-soon")
                .with_tierings(vec![TieringRow::new(deep_floor - 1, deep)])
                .xml(),
            Some(INVALID_ARGUMENT_CODE),
        ),
        (
            "one day past the published maximum",
            "too-late",
            ConfigurationRow::enabled("too-late")
                .with_tierings(vec![TieringRow::new(MAX_DAYS + 1, deep)])
                .xml(),
            Some(INVALID_ARGUMENT_CODE),
        ),
    ]
}

#[tokio::test]
async fn a_configuration_outside_its_published_bounds_is_refused() {
    let router = router();
    let name = "itc-refusals";
    bucket(&router, name).await;

    // One configuration the bucket carries throughout, so each refusal can be seen to have
    // left the bucket exactly as it was.
    let held = ConfigurationRow::enabled("held");
    let put = put_configuration(&router, name, &held.id, &held).await;
    assert_eq!(
        put.status,
        200,
        "the held configuration must be recorded, got {} ({})",
        put.status,
        body_of(&put)
    );

    for (case, id, payload, code) in refusal_rows() {
        let refused = put_payload(&router, name, id, &payload, &[]).await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: expected 400, got {} ({})",
            refused.status,
            body_of(&refused)
        );
        if let Some(code) = code {
            assert_eq!(
                refused.error_code(),
                code,
                "case {case:?}: expected {code}, got {}",
                refused.error_code()
            );
        }

        let after = reported_configurations(&list_configurations(&router, name, None, &[]).await);
        assert_eq!(
            after,
            vec![held.clone()],
            "case {case:?}: a refused put records nothing, got {after:?}"
        );
    }
}

#[tokio::test]
async fn both_operations_refuse_a_bucket_that_is_not_the_accounts() {
    let router = router();
    let name = "itc-ownership";
    bucket(&router, name).await;

    // A bucket this emulator does not hold: both the put and the listing answer
    // `NoSuchBucket`.
    let configuration = ConfigurationRow::enabled("absent-bucket");
    for (case, answer) in [
        (
            "put on an absent bucket",
            put_configuration(&router, "itc-no-such-bucket", "absent-bucket", &configuration).await,
        ),
        (
            "list of an absent bucket",
            list_configurations(&router, "itc-no-such-bucket", None, &[]).await,
        ),
    ] {
        assert_eq!(
            answer.status,
            404,
            "case {case:?}: expected 404, got {} ({})",
            answer.status,
            body_of(&answer)
        );
        assert_eq!(
            answer.error_code(),
            NO_SUCH_BUCKET_CODE,
            "case {case:?}: expected {NO_SUCH_BUCKET_CODE}, got {}",
            answer.error_code()
        );
    }

    // An expected owner that is not the account's: HTTP 403 `AccessDenied` from either
    // operation.
    let other_account = [("x-amz-expected-bucket-owner", "209876543210")];
    for (case, answer) in [
        (
            "put naming another owner",
            put_payload(
                &router,
                name,
                &configuration.id,
                &configuration.xml(),
                &other_account,
            )
            .await,
        ),
        (
            "list naming another owner",
            list_configurations(&router, name, None, &other_account).await,
        ),
    ] {
        assert_eq!(
            answer.status,
            403,
            "case {case:?}: expected 403, got {} ({})",
            answer.status,
            body_of(&answer)
        );
        assert_eq!(
            answer.error_code(),
            "AccessDenied",
            "case {case:?}: expected AccessDenied, got {}",
            answer.error_code()
        );
    }

    // The refused put recorded nothing.
    let after = reported_configurations(&list_configurations(&router, name, None, &[]).await);
    assert!(
        after.is_empty(),
        "case \"put naming another owner\": a refused put records nothing, got {after:?}"
    );
}

#[tokio::test]
async fn the_per_bucket_ceiling_is_the_published_one() {
    // The ceiling the published documentation names, so a change to the constant cannot
    // quietly make the emulator hold a different number of configurations.
    assert_eq!(
        MAX_INTELLIGENT_TIERING_CONFIGURATIONS_PER_BUCKET, 1_000,
        "a bucket may carry up to 1,000 S3 Intelligent-Tiering configurations, got {MAX_INTELLIGENT_TIERING_CONFIGURATIONS_PER_BUCKET}"
    );
}
