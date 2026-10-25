// SPDX-License-Identifier: CC0-1.0

//! The tag set a `CreateConnectionGroup` submits, end to end over HTTP.
//!
//! The rule under test: the `Tags` a create names — "a complex type that contains zero or
//! more `Tag` elements" — become the created resource's ONE tag set, so the create's own
//! answer, every later read of the group and the service's tag surface
//! (`ListTagsForResource` on the ARN the create minted) all report exactly that set, and a
//! later `TagResource` on that ARN adds into the very same set rather than into a second
//! copy of it. A tag set no CloudFront resource can carry is refused before anything is
//! minted, so the refused create leaves no group and no tag behind, and a tag set is
//! associated with the group rather than part of its configuration, so tagging it mints no
//! new version token.
//!
//! Every row differs along what that rule generalises over: whether the request names
//! `Items` at all, how many tags it names (none, one, several, enough to fill the
//! published maximum), whether a tag names a `Value` and whether that value is empty,
//! which characters the key and the value use and how long they are, the order the tags
//! are submitted in, and — for the add — how many tags the group already carries and
//! whether the added keys overlap them. The character and length rows are built from what
//! `TagKey` and `TagValue` document (a key "between 1 and 128 characters", a value
//! "between 0 and 256 characters", "valid characters include `a-z`, `A-Z`, `0-9`, space,
//! and the special characters `_ - . : / = + @`", and a pattern that also admits any
//! Unicode letter, space or digit).
//!
//! The harness (the router, the request helpers, the element scanner, the tag helpers and
//! the connection group calls) is the lifecycle, `distribution_tags`, `tag_resource` and
//! `connection_groups` suites', reused rather than copied.

use std::collections::BTreeMap;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::MAX_TAGS_PER_RESOURCE;
use smithy_server_runtime::MultiServiceRouter;

use crate::connection_groups::{create_connection_group, get_connection_group};
use crate::distribution_lifecycle::{emulator, Answer, API_VERSION};
use crate::distribution_tags::{
    list_tags_for_resource, reported_order, reported_tags, tag, tag_sets_no_resource_can_carry,
    tags_xml, ReportedTags, SubmittedTag,
};
use crate::tag_resource::tag_resource;

// ---------------------------------------------------------------------------
// Calling the surface
// ---------------------------------------------------------------------------

/// A `CreateConnectionGroupRequest` naming `name` and whichever tag set the row submits.
fn create_body(name: &str, submitted: Option<&[SubmittedTag]>) -> String {
    format!(
        "<CreateConnectionGroupRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Name>{name}</Name>{}</CreateConnectionGroupRequest>",
        tags_xml(submitted)
    )
}

/// The `Tags` element of a `ConnectionGroup` document, read as a tag set.
fn group_tags(answer: &Answer, case: &str) -> ReportedTags {
    let tags = answer.root().at("Tags").unwrap_or_else(|| {
        panic!(
            "case {case}: expected the connection group to carry a Tags element, got {}",
            answer.raw
        )
    });
    reported_tags(tags)
}

/// The tag set `arn` reports through `ListTagsForResource`.
async fn tags_of(router: &MultiServiceRouter, arn: &str, case: &str) -> ReportedTags {
    let answer = list_tags_for_resource(router, arn).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected ListTagsForResource on {arn} to answer 200, got {} ({})",
        answer.status, answer.raw
    );
    reported_tags(answer.root())
}

/// The `(Id, Arn)` a create answered with, and the version token it minted.
fn identity(answer: &Answer, case: &str) -> (String, String, String) {
    let group = answer.root();
    let id = group.text_at("Id").unwrap_or_default().to_string();
    let arn = group.text_at("Arn").unwrap_or_default().to_string();
    assert!(
        !id.is_empty() && !arn.is_empty(),
        "case {case}: expected the create to answer an Id and an Arn, got {}",
        answer.raw
    );
    (id, arn, answer.header("etag").to_string())
}

// ---------------------------------------------------------------------------
// The tag sets a create submits
// ---------------------------------------------------------------------------

/// One create: the group's name, and the tag set the request names.
#[derive(Debug, Clone)]
struct TagCase {
    case: &'static str,
    /// A name is "unique within your Amazon Web Services account", so each row needs one
    /// of its own; the TAG SET is what the row varies.
    name: &'static str,
    /// `None` leaves `Items` out of the request entirely, which is a different document
    /// from one carrying an empty `Items` list — both mean "no tags".
    submitted: Option<Vec<SubmittedTag>>,
}

impl TagCase {
    fn submitted(&self) -> Option<&[SubmittedTag]> {
        self.submitted.as_deref()
    }

    /// The tag set every read must report for this row: one entry per submitted key,
    /// carrying the value that key was submitted with.
    fn expected(&self) -> ReportedTags {
        self.submitted
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect()
    }
}

/// The table the create assertions run over.
fn tag_table() -> Vec<TagCase> {
    vec![
        TagCase {
            case: "no Items element",
            name: "cg-tags-no-items",
            submitted: None,
        },
        TagCase {
            case: "an empty Items list",
            name: "cg-tags-empty-items",
            submitted: Some(Vec::new()),
        },
        TagCase {
            case: "one tag",
            name: "cg-tags-one",
            submitted: Some(vec![tag("environment", Some("production"))]),
        },
        TagCase {
            case: "a tag whose Value is left out",
            name: "cg-tags-no-value",
            submitted: Some(vec![tag("team", None)]),
        },
        TagCase {
            case: "a tag whose Value is empty",
            name: "cg-tags-empty-value",
            submitted: Some(vec![tag("owner", Some(""))]),
        },
        TagCase {
            case: "the documented special characters",
            name: "cg-tags-special-characters",
            submitted: Some(vec![tag(
                "cost.center:unit/01=a+b-c@d e",
                Some("a b_c-d.e:f/g=h+i@j"),
            )]),
        },
        TagCase {
            case: "unicode letters",
            name: "cg-tags-unicode",
            submitted: Some(vec![tag("proyecto", Some("producción"))]),
        },
        TagCase {
            // Submitted in descending key order, so a row reported in key order cannot be
            // passing by echoing the request's order.
            case: "three tags submitted in descending key order",
            name: "cg-tags-descending",
            submitted: Some(vec![
                tag("zone", Some("eu")),
                tag("mode", None),
                tag("app", Some("checkout")),
            ]),
        },
        TagCase {
            case: "a key at its minimum length",
            name: "cg-tags-shortest-key",
            submitted: Some(vec![tag("k", Some("v"))]),
        },
        TagCase {
            case: "a key and a value at their maximum length",
            name: "cg-tags-longest",
            submitted: Some(vec![tag(&"k".repeat(128), Some(&"v".repeat(256)))]),
        },
        TagCase {
            case: "the maximum tag count",
            name: "cg-tags-full-set",
            submitted: Some(
                (0..MAX_TAGS_PER_RESOURCE)
                    .map(|index| tag(&format!("tag-{index:03}"), Some(&format!("value-{index}"))))
                    .collect(),
            ),
        },
    ]
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

/// A create PERSISTS the tag set it names: its own answer, a later `GetConnectionGroup`
/// and `ListTagsForResource` on the minted ARN all report exactly the submitted tags, in
/// one stable key order whatever order the request listed them in.
#[tokio::test]
async fn each_create_stores_its_tag_set_and_every_tag_read_reports_it() {
    let router = emulator();

    for row in tag_table() {
        let case = row.case;
        let expected = row.expected();

        let body = create_body(row.name, row.submitted());
        let created = create_connection_group(&router, &body).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected the create to answer 201, got {} ({})",
            created.status, created.raw
        );
        let (id, arn, _) = identity(&created, case);

        let answered = group_tags(&created, case);
        assert_eq!(
            answered, expected,
            "case {case}: expected the created group to carry {expected:?}, got {answered:?}"
        );

        let read = get_connection_group(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected GetConnectionGroup on {id} to answer 200, got {} ({})",
            read.status, read.raw
        );
        let stored = group_tags(&read, case);
        assert_eq!(
            stored, expected,
            "case {case}: expected the stored group to report {expected:?}, got {stored:?}"
        );

        let reported = tags_of(&router, &arn, case).await;
        assert_eq!(
            reported, expected,
            "case {case}: expected the tag read of {arn} to report {expected:?}, got \
             {reported:?}"
        );

        let listed = list_tags_for_resource(&router, &arn).await;
        let order = reported_order(listed.root());
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(
            order, sorted,
            "case {case}: expected the tag read to report the keys in one stable order, got \
             {order:?}"
        );
    }
}

/// A tag set no CloudFront resource can carry is refused — `InvalidTagging` for the rules
/// the model cannot state, `ValidationException` for the ones it can — and the refused
/// create MINTS NOTHING: the name it asked for is still free afterwards, and the group the
/// next create stores carries only the tags THAT create named.
#[tokio::test]
async fn a_tag_set_no_group_can_carry_is_refused_and_creates_nothing() {
    let router = emulator();

    for (case, tags, status, code) in tag_sets_no_resource_can_carry() {
        let name = format!("cg-refused-{case}");
        let refused = create_connection_group(&router, &create_body(&name, Some(&tags))).await;
        assert_eq!(
            refused.status, status,
            "case {case}: expected HTTP {status}, got {} ({})",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            code,
            "case {case}: expected {code}, got {:?} ({})",
            refused.error_code(),
            refused.raw
        );

        let accepted = create_connection_group(
            &router,
            &create_body(&name, Some(&[tag("ok", Some("yes"))])),
        )
        .await;
        assert_eq!(
            accepted.status, 201,
            "case {case}: expected the refused name {name} to still be free, got {} ({})",
            accepted.status, accepted.raw
        );
        let (_, arn, _) = identity(&accepted, case);
        let expected: ReportedTags =
            BTreeMap::from([("ok".to_string(), Some("yes".to_string()))]);
        let reported = tags_of(&router, &arn, case).await;
        assert_eq!(
            reported, expected,
            "case {case}: expected the accepted create to leave {expected:?}, got {reported:?}"
        );
    }
}

/// A tag set is ASSOCIATED with the connection group rather than part of its
/// configuration: a `TagResource` on the ARN the create minted adds into the very set the
/// create stored — a key the group already carries takes the value the call names, every
/// other key is left exactly as it was — the group's own document reports that same set,
/// and the group's version token is unchanged.
#[tokio::test]
async fn tagging_adds_into_the_set_the_create_stored() {
    let router = emulator();

    // `(case, what the create named, what the tagging call then names)`.
    let rows: Vec<(&str, Vec<SubmittedTag>, Vec<SubmittedTag>)> = vec![
        (
            "an untagged group",
            Vec::new(),
            vec![tag("stage", Some("prod"))],
        ),
        (
            "an add naming no tag at all",
            vec![tag("stage", Some("prod"))],
            Vec::new(),
        ),
        (
            "keys the group does not carry",
            vec![tag("stage", Some("prod"))],
            vec![tag("team", Some("edge")), tag("owner", None)],
        ),
        (
            "a key the group already carries",
            vec![tag("stage", Some("prod")), tag("team", Some("edge"))],
            vec![tag("stage", Some("beta"))],
        ),
        (
            "a full set taking a new value for a carried key",
            (0..MAX_TAGS_PER_RESOURCE)
                .map(|index| tag(&format!("carried-{index:03}"), Some("original")))
                .collect(),
            vec![tag("carried-000", Some("rewritten"))],
        ),
    ];

    for (index, (case, carried, added)) in rows.into_iter().enumerate() {
        let name = format!("cg-add-{index:02}");
        let created = create_connection_group(&router, &create_body(&name, Some(&carried))).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected the create to answer 201, got {} ({})",
            created.status, created.raw
        );
        let (id, arn, etag) = identity(&created, case);

        let mut expected: ReportedTags = carried.into_iter().collect();
        for (key, value) in added.clone() {
            expected.insert(key, value);
        }

        let tagged = tag_resource(&router, &arn, Some(&added)).await;
        assert_eq!(
            tagged.status, 204,
            "case {case}: expected TagResource on {arn} to answer 204, got {} ({})",
            tagged.status, tagged.raw
        );

        let reported = tags_of(&router, &arn, case).await;
        assert_eq!(
            reported, expected,
            "case {case}: expected the tag read of {arn} to report {expected:?}, got \
             {reported:?}"
        );

        let read = get_connection_group(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected GetConnectionGroup on {id} to answer 200, got {} ({})",
            read.status, read.raw
        );
        let stored = group_tags(&read, case);
        assert_eq!(
            stored, expected,
            "case {case}: expected the group document to report {expected:?}, got {stored:?}"
        );
        assert_eq!(
            read.header("etag"),
            etag,
            "case {case}: expected the version token {etag} to be unchanged by tagging, got \
             {:?}",
            read.header("etag")
        );
    }
}
