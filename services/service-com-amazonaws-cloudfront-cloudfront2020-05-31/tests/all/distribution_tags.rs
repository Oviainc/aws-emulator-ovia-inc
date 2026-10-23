// SPDX-License-Identifier: CC0-1.0

//! `CreateDistributionWithTags` and the tag set it associates with the new distribution,
//! end to end over HTTP.
//!
//! The rule under test: `CreateDistributionWithTags` creates the SAME resource
//! `CreateDistribution` creates — minting the id, ARN, domain name and first version
//! token and storing the submitted `DistributionConfig` whole, under the same refusals —
//! and additionally associates the submitted tag set with that resource, so
//! `ListTagsForResource` on the new distribution's ARN reports exactly one tag per
//! submitted key, each carrying the value that key was submitted with (and no value
//! where the request named none), independently of how many tags were submitted or in
//! which order.
//!
//! Every row below differs along what that rule generalises over: how many tags the
//! request carries (none at all, an empty list, one, several, the published maximum),
//! whether a tag names a `Value` and whether that value is empty, which characters the
//! key and value use, the length of the key and the value, and the order the tags are
//! submitted in. The distribution half of each row is drawn from the configuration table
//! the lifecycle suite already defines, so the create is exercised over several
//! configurations rather than one.
//!
//! The harness (the router, the request helpers, the element scanner and the
//! configuration table) is the lifecycle suite's, reused rather than copied.

use std::collections::BTreeMap;
use std::time::Duration;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MAX_TAGS_PER_RESOURCE, PROPAGATION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, config, config_table, create_distribution, delete_distribution, emulator,
    get_distribution, is_distribution_id, is_version_token, Answer, Config, Node, ACCOUNT_ID,
    API_VERSION, HOST,
};

/// A tag as a request names it: a key, and a value the request may leave out entirely.
///
/// `pub(crate)` so the sibling tagging suites in `tests/all/` name a tag the same way
/// rather than each declaring its own.
pub(crate) type SubmittedTag = (String, Option<String>);

/// The tag set a read reports: key -> the value that key carries, if any.
pub(crate) type ReportedTags = BTreeMap<String, Option<String>>;

// ---------------------------------------------------------------------------
// Calling the tagging surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/distribution?WithTags`, whose payload is a
/// `DistributionConfigWithTags`.
pub(crate) async fn create_distribution_with_tags(
    router: &MultiServiceRouter,
    config: &Config,
    submitted: Option<&[SubmittedTag]>,
) -> Answer {
    let body = format!(
        "<DistributionConfigWithTags \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">{}{}\
         </DistributionConfigWithTags>",
        config.xml(),
        tags_xml(submitted)
    );
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/distribution?WithTags"),
        Some(&body),
        None,
    )
    .await
}

/// `GET /2020-05-31/tagging?Resource=<arn>`.
///
/// The ARN is sent as it is: every character a CloudFront ARN uses is legal in a query
/// value, and none of them is `&`, `=` or `%`, so there is nothing for an encoding to
/// change.
pub(crate) async fn list_tags_for_resource(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/tagging?Resource={arn}"),
        None,
        None,
    )
    .await
}

/// The `Tags` element a request carries.
///
/// `None` is a request that names no `Items` at all (`<Tags/>`), which is a different
/// document from one carrying an empty `Items` list — both mean "no tags", and the rule
/// has to hold for each.
pub(crate) fn tags_xml(submitted: Option<&[SubmittedTag]>) -> String {
    let Some(items) = submitted else {
        return "<Tags/>".to_string();
    };
    let mut out = String::from("<Tags><Items>");
    for (key, value) in items {
        out.push_str("<Tag>");
        out.push_str(&format!("<Key>{key}</Key>"));
        // A tag whose `Value` the request leaves out carries no value at all, which is
        // not the same as an empty one.
        if let Some(value) = value {
            out.push_str(&format!("<Value>{value}</Value>"));
        }
        out.push_str("</Tag>");
    }
    out.push_str("</Items></Tags>");
    out
}

/// The tag set a `Tags` element reports, read out of the answered document.
pub(crate) fn reported_tags(tags: &Node) -> ReportedTags {
    tags.at("Items")
        .map(|items| {
            items
                .children_named("Tag")
                .into_iter()
                .map(|tag| {
                    (
                        tag.text_at("Key").unwrap_or_default().to_string(),
                        tag.at("Value").map(|value| value.text.clone()),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The tags a `Tags` element reports, in the order the document lists them.
pub(crate) fn reported_order(tags: &Node) -> Vec<String> {
    tags.at("Items")
        .map(|items| {
            items
                .children_named("Tag")
                .into_iter()
                .map(|tag| tag.text_at("Key").unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The tag sets under test
// ---------------------------------------------------------------------------

/// One tag set a client submits with a create.
#[derive(Debug, Clone)]
struct TagCase {
    case: &'static str,
    /// `None` leaves `Items` out of the request entirely.
    submitted: Option<Vec<SubmittedTag>>,
}

impl TagCase {
    fn submitted(&self) -> Option<&[SubmittedTag]> {
        self.submitted.as_deref()
    }

    /// The tag set a read must report for this row: one entry per submitted key,
    /// carrying the value that key was submitted with.
    fn expected(&self) -> ReportedTags {
        self.submitted
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect()
    }
}

pub(crate) fn tag(key: &str, value: Option<&str>) -> SubmittedTag {
    (key.to_string(), value.map(str::to_string))
}

/// The table every tag assertion runs over.
///
/// The character and length rows are built from what `TagKey` and `TagValue` document:
/// "valid characters include `a-z`, `A-Z`, `0-9`, space, and the special characters
/// `_ - . : / = + @`", a key length "between 1 and 128 characters" and a value length
/// "between 0 and 256 characters" — each bound is a row here, and one PAST each bound is
/// a row of [`tag_sets_no_resource_can_carry`]. The key pattern also admits any Unicode
/// letter, space or digit (`\p{L}\p{Z}\p{N}`), which the accented row covers.
fn tag_table() -> Vec<TagCase> {
    vec![
        TagCase {
            case: "no-items-element",
            submitted: None,
        },
        TagCase {
            case: "empty-items",
            submitted: Some(Vec::new()),
        },
        TagCase {
            case: "one-tag",
            submitted: Some(vec![tag("environment", Some("production"))]),
        },
        TagCase {
            case: "value-left-out",
            submitted: Some(vec![tag("team", None)]),
        },
        TagCase {
            case: "empty-value",
            submitted: Some(vec![tag("owner", Some(""))]),
        },
        TagCase {
            case: "special-characters",
            submitted: Some(vec![tag(
                "cost.center:unit/01=a+b-c@d e",
                Some("a b_c-d.e:f/g=h+i@j"),
            )]),
        },
        TagCase {
            case: "unicode-letters",
            submitted: Some(vec![tag("proyecto", Some("producción"))]),
        },
        TagCase {
            // Submitted in descending key order, so a row that is reported in key order
            // cannot be passing by echoing the request's order.
            case: "three-tags-descending",
            submitted: Some(vec![
                tag("zone", Some("eu")),
                tag("mode", None),
                tag("app", Some("checkout")),
            ]),
        },
        TagCase {
            case: "key-at-minimum-length",
            submitted: Some(vec![tag("k", Some("v"))]),
        },
        TagCase {
            case: "key-and-value-at-maximum-length",
            submitted: Some(vec![tag(
                &"k".repeat(128),
                Some(&"v".repeat(256)),
            )]),
        },
        TagCase {
            case: "maximum-tag-count",
            submitted: Some(
                (0..MAX_TAGS_PER_RESOURCE)
                    .map(|index| tag(&format!("tag-{index:03}"), Some(&format!("value-{index}"))))
                    .collect(),
            ),
        },
    ]
}

/// A configuration for `reference`, drawn from the lifecycle suite's table so each row
/// creates a DIFFERENT distribution configuration rather than the same one.
pub(crate) fn config_for(index: usize, reference: &str) -> Config {
    let table = config_table();
    let template = table[index % table.len()].clone();
    Config {
        caller_reference: reference.to_string(),
        ..template
    }
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

/// `CreateDistributionWithTags` persists the distribution AND its tag set: the create
/// answers with the record it stored, `GetDistribution` reads the same record back, and
/// `ListTagsForResource` on its ARN reports exactly the submitted tags.
#[tokio::test]
async fn create_with_tags_persists_the_distribution_and_its_tags() {
    let router = emulator();

    for (index, row) in tag_table().into_iter().enumerate() {
        let case = row.case;
        let config = config_for(index, &format!("ref-{case}"));
        let answer = create_distribution_with_tags(&router, &config, row.submitted()).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected the create to answer 201, got {} ({})",
            answer.status, answer.raw
        );

        let distribution = answer.root();
        let id = distribution.text_at("Id").unwrap_or_default().to_string();
        assert!(
            is_distribution_id(&id),
            "case {case}: expected an `E`+13 distribution id, got {id:?}"
        );
        let etag = answer.header("etag").to_string();
        assert!(
            is_version_token(&etag),
            "case {case}: expected an `E`+13 version token in the ETag header, got {etag:?}"
        );
        let arn = distribution.text_at("ARN").unwrap_or_default().to_string();
        assert_eq!(
            arn,
            format!("arn:aws:cloudfront::{ACCOUNT_ID}:distribution/{id}"),
            "case {case}: expected the ARN of {id}, got {arn:?}"
        );
        assert_eq!(
            answer.header("location"),
            format!("https://{HOST}/{API_VERSION}/distribution/{id}"),
            "case {case}: expected the Location of {id}, got {:?}",
            answer.header("location")
        );
        // The same create rules as `CreateDistribution`: the new distribution is not yet
        // propagated, and the configuration it reports is the one submitted.
        assert_eq!(
            distribution.text_at("Status"),
            Some("InProgress"),
            "case {case}: expected a freshly created distribution to be InProgress, got {:?}",
            distribution.text_at("Status")
        );
        let created_config = distribution
            .at("DistributionConfig")
            .unwrap_or_else(|| panic!("case {case}: the create answered no DistributionConfig"));
        config.assert_matches(created_config, "create with tags");

        // Read the distribution back through its own reader: the record, not the request.
        let read = get_distribution(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected GetDistribution to answer 200, got {} ({})",
            read.status, read.raw
        );
        let read_distribution = read.root();
        assert_eq!(
            read_distribution.text_at("Id"),
            Some(id.as_str()),
            "case {case}: expected GetDistribution to report {id}, got {:?}",
            read_distribution.text_at("Id")
        );
        assert_eq!(
            read_distribution.text_at("ARN"),
            Some(arn.as_str()),
            "case {case}: expected GetDistribution to report ARN {arn}, got {:?}",
            read_distribution.text_at("ARN")
        );
        let read_config = read_distribution
            .at("DistributionConfig")
            .unwrap_or_else(|| panic!("case {case}: GetDistribution answered no config"));
        config.assert_matches(read_config, "get after create with tags");

        // The tag set the create associated with the new resource.
        let tags = list_tags_for_resource(&router, &arn).await;
        assert_eq!(
            tags.status, 200,
            "case {case}: expected ListTagsForResource to answer 200, got {} ({})",
            tags.status, tags.raw
        );
        let actual = reported_tags(tags.root());
        assert_eq!(
            actual,
            row.expected(),
            "case {case}: expected the tag set {:?}, got {actual:?}",
            row.expected()
        );
        // A tag set is keyed by tag key, so it is reported in key order whatever order
        // the request listed it in.
        let order = reported_order(tags.root());
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(
            order, sorted,
            "case {case}: expected the tags in key order, got {order:?}"
        );
    }
}

/// Two distributions created with tags do not share a tag set: each ARN reports only its
/// own tags, and neither create disturbs the other.
#[tokio::test]
async fn each_distribution_carries_only_its_own_tags() {
    let router = emulator();
    let rows: Vec<TagCase> = tag_table()
        .into_iter()
        .filter(|row| row.submitted().is_some_and(|tags| !tags.is_empty()))
        .collect();

    // Create every row first, then read them all back: a later create must not change
    // what an earlier distribution's ARN reports.
    let mut created: Vec<(TagCase, String)> = Vec::new();
    for (index, row) in rows.into_iter().enumerate() {
        let case = row.case;
        let config = config_for(index, &format!("isolated-{case}"));
        let answer = create_distribution_with_tags(&router, &config, row.submitted()).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected the create to answer 201, got {} ({})",
            answer.status, answer.raw
        );
        let arn = answer
            .root()
            .text_at("ARN")
            .unwrap_or_default()
            .to_string();
        created.push((row, arn));
    }

    for (row, arn) in &created {
        let case = row.case;
        let tags = list_tags_for_resource(&router, arn).await;
        let actual = reported_tags(tags.root());
        assert_eq!(
            actual,
            row.expected(),
            "case {case}: expected {arn} to still report {:?}, got {actual:?}",
            row.expected()
        );
    }
}

/// A distribution created WITHOUT tags carries the empty tag set, and an ARN no resource
/// carries is `NoSuchResource` — before the resource exists and after it is deleted.
#[tokio::test]
async fn tags_are_read_from_the_resource_record() {
    let router = emulator();

    // Nothing is stored yet, so a well-formed ARN names no resource.
    let unknown = format!("arn:aws:cloudfront::{ACCOUNT_ID}:distribution/E00000000000A");
    let missing = list_tags_for_resource(&router, &unknown).await;
    assert_eq!(
        missing.status, 404,
        "case unknown-arn: expected 404, got {} ({})",
        missing.status, missing.raw
    );
    assert_eq!(
        missing.error_code(),
        "NoSuchResource",
        "case unknown-arn: expected NoSuchResource, got {:?}",
        missing.error_code()
    );

    // `CreateDistribution` carries no tag set, so its distribution has no tags — not a
    // missing resource.
    let plain = create_distribution(&router, &config("untagged", "ref-untagged").xml()).await;
    assert_eq!(
        plain.status, 201,
        "case untagged: expected the create to answer 201, got {} ({})",
        plain.status, plain.raw
    );
    let plain_arn = plain.root().text_at("ARN").unwrap_or_default().to_string();
    let plain_tags = list_tags_for_resource(&router, &plain_arn).await;
    assert_eq!(
        plain_tags.status, 200,
        "case untagged: expected ListTagsForResource to answer 200, got {} ({})",
        plain_tags.status, plain_tags.raw
    );
    let actual = reported_tags(plain_tags.root());
    assert!(
        actual.is_empty(),
        "case untagged: expected no tags, got {actual:?}"
    );

    // A tag set lives on the resource record: once the distribution is deleted, its ARN
    // names no resource again.
    let disabled = Config {
        enabled: false,
        ..config_for(0, "ref-deleted")
    };
    let created = create_distribution_with_tags(
        &router,
        &disabled,
        Some(&[tag("retention", Some("temporary"))]),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case deleted: expected the create to answer 201, got {} ({})",
        created.status, created.raw
    );
    let id = created.root().text_at("Id").unwrap_or_default().to_string();
    let arn = created.root().text_at("ARN").unwrap_or_default().to_string();
    let etag = created.header("etag").to_string();
    let before = reported_tags(list_tags_for_resource(&router, &arn).await.root());
    assert_eq!(
        before.len(),
        1,
        "case deleted: expected the created distribution to carry its one tag, got {before:?}"
    );

    // The distribution was created disabled, and a delete lands only once that
    // configuration has also finished propagating, so the clock is given its span first.
    tokio::time::sleep(PROPAGATION_DELAY + Duration::from_millis(250)).await;
    let deleted = delete_distribution(&router, &id, Some(&etag)).await;
    assert_eq!(
        deleted.status, 204,
        "case deleted: expected the delete to answer 204, got {} ({})",
        deleted.status, deleted.raw
    );
    let after = list_tags_for_resource(&router, &arn).await;
    assert_eq!(
        after.status, 404,
        "case deleted: expected the deleted ARN to answer 404, got {} ({})",
        after.status, after.raw
    );
    assert_eq!(
        after.error_code(),
        "NoSuchResource",
        "case deleted: expected NoSuchResource, got {:?}",
        after.error_code()
    );
}

/// A tag set no resource can carry, and the error each one is refused with.
///
/// `(case, tags, expected status, expected error code)`. The two `InvalidTagging` rows
/// are the rules the model cannot state — at most one tag per key, at most
/// [`MAX_TAGS_PER_RESOURCE`] tags — and the two `ValidationException` rows are one past
/// `TagKey`'s documented maximum length (128) and one past `TagValue`'s (256).
fn tag_sets_no_resource_can_carry() -> Vec<(&'static str, Vec<SubmittedTag>, u16, &'static str)> {
    vec![
        (
            "duplicate-key",
            vec![tag("stage", Some("alpha")), tag("stage", Some("beta"))],
            400,
            "InvalidTagging",
        ),
        (
            "duplicate-key-among-many",
            vec![
                tag("a", Some("1")),
                tag("b", Some("2")),
                tag("a", Some("3")),
                tag("c", None),
            ],
            400,
            "InvalidTagging",
        ),
        (
            "one-past-the-maximum-tag-count",
            (0..=MAX_TAGS_PER_RESOURCE)
                .map(|index| tag(&format!("tag-{index:03}"), Some("v")))
                .collect(),
            400,
            "InvalidTagging",
        ),
        (
            "key-one-past-its-maximum-length",
            vec![tag(&"k".repeat(129), Some("v"))],
            400,
            "ValidationException",
        ),
        (
            "value-one-past-its-maximum-length",
            vec![tag("size", Some(&"v".repeat(257)))],
            400,
            "ValidationException",
        ),
    ]
}

/// A create carrying a tag set no resource can carry is refused, and claims NOTHING: the
/// caller reference it named is still free afterwards.
#[tokio::test]
async fn a_tag_set_no_resource_can_carry_is_refused_and_creates_nothing() {
    let router = emulator();

    for (case, tags, status, code) in tag_sets_no_resource_can_carry() {
        let reference = format!("ref-refused-{case}");
        let config = config_for(0, &reference);
        let refused = create_distribution_with_tags(&router, &config, Some(&tags)).await;
        assert_eq!(
            refused.status, status,
            "case {case}: expected {status}, got {} ({})",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            code,
            "case {case}: expected {code}, got {:?}",
            refused.error_code()
        );

        // Nothing was claimed: the same caller reference still creates a distribution.
        let accepted =
            create_distribution_with_tags(&router, &config, Some(&[tag("ok", Some("yes"))])).await;
        assert_eq!(
            accepted.status, 201,
            "case {case}: expected the caller reference to be free after the refusal, got {} ({})",
            accepted.status, accepted.raw
        );
    }
}

/// The create refusals are the SAME ones `CreateDistribution` answers, because both
/// operations run the same create: a caller reference already associated with a
/// distribution carrying a DIFFERENT configuration, and a list wrapper whose `Quantity`
/// disagrees with its `Items`.
#[tokio::test]
async fn create_with_tags_runs_the_same_create_refusals() {
    let router = emulator();

    // A caller reference names at most one distribution, whichever create claimed it
    // first — so each direction of the pair is a row. The second create of each pair
    // carries a different configuration under that reference, which is what makes it a
    // second distribution rather than the same request again.
    let first = config("shared-reference", "ref-shared-one");
    let plain = create_distribution(&router, &first.xml()).await;
    assert_eq!(
        plain.status, 201,
        "case plain-then-with-tags: expected the first create to answer 201, got {} ({})",
        plain.status, plain.raw
    );
    let first_changed = Config {
        comment: "a different configuration".to_string(),
        ..first.clone()
    };
    let conflict =
        create_distribution_with_tags(&router, &first_changed, Some(&[tag("k", Some("v"))])).await;
    assert_eq!(
        conflict.status, 409,
        "case plain-then-with-tags: expected 409, got {} ({})",
        conflict.status, conflict.raw
    );
    assert_eq!(
        conflict.error_code(),
        "DistributionAlreadyExists",
        "case plain-then-with-tags: expected DistributionAlreadyExists, got {:?}",
        conflict.error_code()
    );

    let second = config("shared-reference", "ref-shared-two");
    let tagged = create_distribution_with_tags(&router, &second, Some(&[tag("k", Some("v"))])).await;
    assert_eq!(
        tagged.status, 201,
        "case with-tags-then-plain: expected the first create to answer 201, got {} ({})",
        tagged.status, tagged.raw
    );
    let second_changed = Config {
        enabled: !second.enabled,
        ..second.clone()
    };
    let reverse = create_distribution(&router, &second_changed.xml()).await;
    assert_eq!(
        reverse.status, 409,
        "case with-tags-then-plain: expected 409, got {} ({})",
        reverse.status, reverse.raw
    );
    assert_eq!(
        reverse.error_code(),
        "DistributionAlreadyExists",
        "case with-tags-then-plain: expected DistributionAlreadyExists, got {:?}",
        reverse.error_code()
    );

    // And the other half of that rule holds across the two operations as well: the SAME
    // configuration under a reference already in use is the same request again, answered
    // with the distribution that reference already names.
    let repeat = create_distribution_with_tags(&router, &first, Some(&[tag("k", Some("v"))])).await;
    assert_eq!(
        repeat.status, 201,
        "case repeat-with-tags: expected 201 for the same create again, got {} ({})",
        repeat.status, repeat.raw
    );
    assert_eq!(
        repeat.root().text_at("Id"),
        plain.root().text_at("Id"),
        "case repeat-with-tags: expected the distribution the first create made, got {:?}",
        repeat.root().text_at("Id")
    );

    // Every CloudFront list travels in a wrapper that states its own length, and a
    // request whose `Quantity` disagrees with its `Items` is refused — here the
    // `Aliases` wrapper, rewritten to claim one more entry than it carries.
    let mut aliased = config_for(1, "ref-inconsistent");
    aliased.aliases = vec!["cdn.example.com".to_string()];
    let body = aliased
        .xml()
        .replace("<Aliases><Quantity>1</Quantity>", "<Aliases><Quantity>2</Quantity>");
    let payload = format!(
        "<DistributionConfigWithTags \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">{body}{}\
         </DistributionConfigWithTags>",
        tags_xml(Some(&[tag("k", Some("v"))]))
    );
    let refused = call(
        &router,
        "POST",
        &format!("/{API_VERSION}/distribution?WithTags"),
        Some(&payload),
        None,
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case inconsistent-quantities: expected 400, got {} ({})",
        refused.status, refused.raw
    );
    assert_eq!(
        refused.error_code(),
        "InconsistentQuantities",
        "case inconsistent-quantities: expected InconsistentQuantities, got {:?}",
        refused.error_code()
    );
}

/// A tag read is about the ARN, not about one kind of resource: a key value store this
/// emulator holds reports its OWN tag set from the same reader, under the same at-most-
/// one-tag-per-key rule (which its create refuses with the `InvalidArgument` its model
/// declares).
#[tokio::test]
async fn a_tag_read_resolves_every_resource_kind_the_store_holds() {
    let router = emulator();

    for (index, row) in tag_table().into_iter().enumerate() {
        let case = row.case;
        // `KeyValueStoreName`'s pattern is `^[a-zA-Z0-9-_]{1,64}$`, so the store name is
        // not a place to vary characters — the tag set is what each row varies.
        let name = format!("store-{index:02}");
        let body = format!(
            "<CreateKeyValueStoreRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>{name}</Name><Comment>tagged {case}</Comment>{}\
             </CreateKeyValueStoreRequest>",
            tags_xml(row.submitted())
        );
        let created = call(
            &router,
            "POST",
            &format!("/{API_VERSION}/key-value-store"),
            Some(&body),
            None,
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected the key value store create to answer 201, got {} ({})",
            created.status, created.raw
        );
        let arn = created
            .root()
            .text_at("ARN")
            .unwrap_or_default()
            .to_string();
        assert!(
            arn.starts_with(&format!("arn:aws:cloudfront::{ACCOUNT_ID}:key-value-store/")),
            "case {case}: expected a key value store ARN, got {arn:?}"
        );

        let tags = list_tags_for_resource(&router, &arn).await;
        assert_eq!(
            tags.status, 200,
            "case {case}: expected ListTagsForResource to answer 200 for {arn}, got {} ({})",
            tags.status, tags.raw
        );
        let actual = reported_tags(tags.root());
        assert_eq!(
            actual,
            row.expected(),
            "case {case}: expected {arn} to report {:?}, got {actual:?}",
            row.expected()
        );
    }

    // The same tag-set rule, reported as the refusal this create models.
    let duplicate = format!(
        "<CreateKeyValueStoreRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Name>store-duplicate</Name><Comment>duplicate key</Comment>{}\
         </CreateKeyValueStoreRequest>",
        tags_xml(Some(&[tag("stage", Some("alpha")), tag("stage", Some("beta"))]))
    );
    let refused = call(
        &router,
        "POST",
        &format!("/{API_VERSION}/key-value-store"),
        Some(&duplicate),
        None,
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case kvs-duplicate-key: expected 400, got {} ({})",
        refused.status, refused.raw
    );
    assert_eq!(
        refused.error_code(),
        "InvalidArgument",
        "case kvs-duplicate-key: expected InvalidArgument, got {:?}",
        refused.error_code()
    );
}
