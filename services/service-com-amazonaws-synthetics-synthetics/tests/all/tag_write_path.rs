// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of this service's whole TAG surface: the write path
//! (`TagResource`, `UntagResource`) and every reader that reports a resource's tags.
//!
//! Two rules are asserted here, one row per input class rather than one recorded request:
//!
//! 1. A response that reports a canary or a group reports its tag map as a MEMBER
//!    whenever it reports the resource — the stored pairs, or an empty map for a resource
//!    that carries none — so `CreateCanary`/`GetCanary`/`DescribeCanaries`,
//!    `CreateGroup`/`GetGroup` and `ListTagsForResource` are projections of one stored
//!    field and a client can read the member, write it back and compare it either way.
//!    The modelled `length 1..50` on `TagMap`
//!    (`smithy_query(query="shape",shape="TagMap")`) bounds a map a CALLER sends, which is
//!    why it is `TagResource`'s input that may not be empty and not this projection.
//! 2. A tag write changes exactly the keys it names: `TagResource` MERGES ("If you specify
//!    a new tag key for the resource, this tag is appended … If you specify a tag key that
//!    is already associated with the resource, the new tag value that you specify replaces
//!    the previous value", `smithy_query(query="doc",shape="TagResource")`) and
//!    `UntagResource` removes the named keys and nothing else — and both are visible
//!    through every reader of that resource, because the tags are stored ON the resource.
//!
//! Every assertion goes over the wire through the same `MultiServiceRouter` the emulator
//! boots, through the harness the lifecycle suite already defines.

use serde_json::{json, Map, Value};

use crate::canary_lifecycle::{create_body, Answer, Synthetics};

/// `TagMap` is modelled `length 1..50` — "You can associate as many as 50 tags with a
/// canary or group" (`smithy_query(query="shape",shape="TagMap")`).
const MAX_TAGS: usize = 50;

/// `TagKey` is modelled `length 1..128` (`smithy_query(query="shape",shape="TagKey")`).
const MAX_TAG_KEY_LEN: usize = 128;

/// `TagValue` is modelled `length 0..256`
/// (`smithy_query(query="shape",shape="TagValue")`).
const MAX_TAG_VALUE_LEN: usize = 256;

/// The fixed sentence every tag operation answers an unresolvable `ResourceArn` with.
const RESOURCE_NOT_FOUND: &str = "Resource was not found";

/// Which collection a row's resource lives in. `ResourceArn`'s pattern admits exactly
/// these two (`…:\d{12}:(canary|group):[0-9a-z_\-]+$`).
///
/// `pub(crate)` with the tag helpers below because the sibling untag suite drives the same
/// two routes rather than growing a second harness for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Canary,
    Group,
}

impl Kind {
    pub(crate) fn case(self) -> &'static str {
        match self {
            Self::Canary => "canary",
            Self::Group => "group",
        }
    }
}

/// Owned key/value pairs from literals, so a table row reads as the tags it applies.
pub(crate) fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// `pairs` as the JSON object a `TagMap` is on the wire.
pub(crate) fn tag_map(entries: Vec<(String, String)>) -> Value {
    Value::Object(
        entries
            .into_iter()
            .map(|(key, value)| (key, Value::String(value)))
            .collect::<Map<String, Value>>(),
    )
}

/// `MAX_TAGS` distinct, legal tags: the most a resource may carry.
///
/// `TagKey` is `length 1..128, pattern ^(?!aws:)[a-zA-Z+-=._:/]+$` (the `+-=` range spans
/// `+ , - . / 0-9 : ; < =`, so digits are in the alphabet).
pub(crate) fn full_tag_set() -> Vec<(String, String)> {
    (0..MAX_TAGS)
        .map(|index| (format!("key{index}"), format!("value{index}")))
        .collect()
}

/// `POST /tags/{ResourceArn}` — the `TagResource` route
/// (`smithy_query(query="operation",operation="TagResource")`).
pub(crate) async fn tag(
    aws: &Synthetics,
    resource_arn: &str,
    tags: Vec<(String, String)>,
) -> Answer {
    aws.call(
        "POST",
        &format!("/tags/{resource_arn}"),
        Some(json!({ "Tags": tag_map(tags) })),
    )
    .await
}

/// `DELETE /tags/{ResourceArn}?tagKeys=…` — the `UntagResource` route, whose `TagKeys`
/// member is bound to the repeated `tagKeys` query parameter
/// (`smithy_query(query="operation",operation="UntagResource")`).
pub(crate) async fn untag(aws: &Synthetics, resource_arn: &str, keys: &[String]) -> Answer {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={key}"))
        .collect::<Vec<_>>()
        .join("&");
    aws.call("DELETE", &format!("/tags/{resource_arn}?{query}"), None)
        .await
}

/// `GET /tags/{ResourceArn}` — the `ListTagsForResource` route.
pub(crate) async fn list_tags(aws: &Synthetics, resource_arn: &str) -> Answer {
    aws.call("GET", &format!("/tags/{resource_arn}"), None).await
}

/// Assert a 200, naming the case on failure.
pub(crate) fn expect_ok(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} ({:?})",
        answer.status, answer.body
    );
}

/// Create a resource of `kind` named `name`, with `tags` when the create carries them,
/// and answer the create's response together with the resource's ARN.
///
/// A group's ARN is reported by the create (`Group$Arn`); the modelled `Canary` structure
/// carries no ARN member, so a canary's is the `ResourceArn` form its own documentation
/// states — `arn:aws:synthetics:Region:account-id:canary:canary-name`
/// (`smithy_query(query="doc",shape="TagResourceRequest",member="ResourceArn")`).
pub(crate) async fn seed(
    aws: &Synthetics,
    kind: Kind,
    name: &str,
    tags: Option<Vec<(String, String)>>,
    region: &str,
    account: &str,
) -> (Answer, String) {
    match kind {
        Kind::Canary => {
            let mut body = create_body(name);
            if let Some(tags) = tags {
                body["Tags"] = tag_map(tags);
            }
            let created = aws.create(body).await;
            expect_ok(name, &created);
            (
                created,
                format!("arn:aws:synthetics:{region}:{account}:canary:{name}"),
            )
        }
        Kind::Group => {
            let mut body = Map::new();
            body.insert("Name".to_string(), json!(name));
            if let Some(tags) = tags {
                body.insert("Tags".to_string(), tag_map(tags));
            }
            let created = aws.call("POST", "/group", Some(Value::Object(body))).await;
            expect_ok(name, &created);
            let arn = created.string_at("Group.Arn");
            (created, arn)
        }
    }
}

/// The resource's own tag member, read back through the implemented reader for its kind.
pub(crate) async fn tags_on_the_resource(
    aws: &Synthetics,
    kind: Kind,
    name: &str,
    arn: &str,
) -> Value {
    let read = match kind {
        Kind::Canary => aws.get(name).await,
        Kind::Group => aws.call("GET", &format!("/group/{arn}"), None).await,
    };
    expect_ok(name, &read);
    match kind {
        Kind::Canary => read.at("Canary.Tags").clone(),
        Kind::Group => read.at("Group.Tags").clone(),
    }
}

/// One resource to create and then read the tag map of.
#[derive(Debug)]
struct Row {
    case: &'static str,
    kind: Kind,
    /// The canary name, or the group name — the identity the create is addressed by.
    name: &'static str,
    /// The tags the create carries, or `None` when the member is omitted.
    tags: Option<Vec<(String, String)>>,
}

/// Rule 1: the tag map is a member of every response that reports the resource, including
/// for a resource that carries no tags at all, where it is an empty map.
///
/// The rows differ along resource kind and tag count — none, one, and the modelled
/// maximum — which is what the rule generalises over.
#[tokio::test]
async fn every_reader_reports_a_tag_map_and_an_untagged_resource_reports_an_empty_one() {
    let region = "eu-west-2";
    let aws = Synthetics::new(region);
    let account = emulator_server::emulator_account_id();

    let rows = vec![
        Row {
            case: "canary-untagged",
            kind: Kind::Canary,
            name: "untagged-probe",
            tags: None,
        },
        Row {
            case: "canary-one-tag",
            kind: Kind::Canary,
            name: "tagged-probe",
            tags: Some(pairs(&[("env", "prod")])),
        },
        Row {
            case: "canary-max-tags",
            kind: Kind::Canary,
            name: "fifty-tagged-probe",
            tags: Some(full_tag_set()),
        },
        Row {
            case: "group-untagged",
            kind: Kind::Group,
            name: "untagged-group",
            tags: None,
        },
        Row {
            case: "group-two-tags",
            kind: Kind::Group,
            name: "tagged-group",
            tags: Some(pairs(&[("team", "observability"), ("cost.center", "4711")])),
        },
        Row {
            case: "group-max-tags",
            kind: Kind::Group,
            name: "fifty-tagged-group",
            tags: Some(full_tag_set()),
        },
    ];

    for row in rows {
        let (case, kind, name) = (row.case, row.kind, row.name);
        // The empty map is what a resource with no tags reports, so the expectation for a
        // create that carried none is `{}` and not a missing member.
        let expected = tag_map(row.tags.clone().unwrap_or_default());
        let (created, arn) = seed(&aws, kind, name, row.tags, region, account).await;

        let reported_by_create = match kind {
            Kind::Canary => created.at("Canary.Tags").clone(),
            Kind::Group => created.at("Group.Tags").clone(),
        };
        assert_eq!(
            reported_by_create, expected,
            "case {case}/create: expected the tag map as a member, got {reported_by_create:?}"
        );

        let reported_by_read = tags_on_the_resource(&aws, kind, name, &arn).await;
        assert_eq!(
            reported_by_read, expected,
            "case {case}/read: expected the tag map as a member, got {reported_by_read:?}"
        );

        let listed = list_tags(&aws, &arn).await;
        expect_ok(case, &listed);
        assert_eq!(
            listed.at("Tags"),
            &expected,
            "case {case}/list-tags: expected the tag map as a member, got {:?}",
            listed.at("Tags")
        );

        // The collection reader projects the same stored field as the single-resource one.
        if kind == Kind::Canary {
            let described = aws.describe(json!({})).await;
            expect_ok(case, &described);
            let listed_canary = described
                .at("Canaries")
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: expected a Canaries array"))
                .iter()
                .find(|canary| canary["Name"] == json!(name))
                .unwrap_or_else(|| panic!("case {case}: expected {name} among the canaries"))
                .clone();
            assert_eq!(
                listed_canary["Tags"], expected,
                "case {case}/describe-canaries: expected the tag map as a member, got {:?}",
                listed_canary["Tags"]
            );
        }
    }
}

/// What one step of a tag write sequence does.
#[derive(Debug)]
enum Write {
    /// `TagResource` with these pairs.
    Tag(Vec<(String, String)>),
    /// `UntagResource` with these keys.
    Untag(Vec<String>),
}

/// One step: the write, and the WHOLE tag set the resource carries afterwards.
#[derive(Debug)]
struct Step {
    case: &'static str,
    write: Write,
    expected: Vec<(String, String)>,
}

/// The write classes the rule generalises over: a first write onto an empty set, several
/// keys in one call, a key re-valued, an empty value, a key and a value at the modelled
/// maximum length, one key removed, a key that was never there removed, and every key
/// removed — which takes the resource back to the empty map.
fn steps() -> Vec<Step> {
    let long_key = "k".repeat(MAX_TAG_KEY_LEN);
    let long_value = "v".repeat(MAX_TAG_VALUE_LEN);
    vec![
        Step {
            case: "first-write-onto-an-untagged-resource",
            write: Write::Tag(pairs(&[("env", "dev")])),
            expected: pairs(&[("env", "dev")]),
        },
        Step {
            case: "two-new-keys-appended-in-one-call",
            write: Write::Tag(pairs(&[("team", "core"), ("tier:level", "gold")])),
            expected: pairs(&[("env", "dev"), ("team", "core"), ("tier:level", "gold")]),
        },
        Step {
            case: "an-existing-key-re-valued",
            write: Write::Tag(pairs(&[("env", "prod")])),
            expected: pairs(&[("env", "prod"), ("team", "core"), ("tier:level", "gold")]),
        },
        Step {
            // `TagValue` is `length 0..256`: an empty value is legal and must be kept.
            case: "a-tag-whose-value-is-empty",
            write: Write::Tag(pairs(&[("cost.center", "")])),
            expected: pairs(&[
                ("cost.center", ""),
                ("env", "prod"),
                ("team", "core"),
                ("tier:level", "gold"),
            ]),
        },
        Step {
            case: "a-key-and-a-value-at-the-modelled-maximum-length",
            write: Write::Tag(vec![(long_key.clone(), long_value.clone())]),
            expected: vec![
                ("cost.center".to_string(), String::new()),
                ("env".to_string(), "prod".to_string()),
                (long_key.clone(), long_value.clone()),
                ("team".to_string(), "core".to_string()),
                ("tier:level".to_string(), "gold".to_string()),
            ],
        },
        Step {
            case: "one-key-removed",
            write: Write::Untag(vec!["team".to_string()]),
            expected: vec![
                ("cost.center".to_string(), String::new()),
                ("env".to_string(), "prod".to_string()),
                (long_key.clone(), long_value.clone()),
                ("tier:level".to_string(), "gold".to_string()),
            ],
        },
        Step {
            // A key the resource does not carry: removing it removes nothing and is not a
            // failure, so the call is idempotent.
            case: "a-key-that-was-never-there-removed",
            write: Write::Untag(vec!["never-set".to_string()]),
            expected: vec![
                ("cost.center".to_string(), String::new()),
                ("env".to_string(), "prod".to_string()),
                (long_key.clone(), long_value.clone()),
                ("tier:level".to_string(), "gold".to_string()),
            ],
        },
        Step {
            case: "every-remaining-key-removed",
            write: Write::Untag(vec![
                "cost.center".to_string(),
                "env".to_string(),
                long_key,
                "tier:level".to_string(),
            ]),
            expected: Vec::new(),
        },
        Step {
            // And the resource is tagged again afterwards: an emptied tag set is a normal
            // state of the resource, not a deleted one.
            case: "tagged-again-after-every-key-was-removed",
            write: Write::Tag(pairs(&[("env", "staging")])),
            expected: pairs(&[("env", "staging")]),
        },
    ]
}

/// Rule 2, over both resource kinds: each write changes exactly the keys it names, and
/// what it leaves behind is what every reader of that resource reports.
#[tokio::test]
async fn tag_resource_merges_and_untag_resource_removes_only_the_keys_named() {
    let region = "us-west-2";
    let aws = Synthetics::new(region);
    let account = emulator_server::emulator_account_id();

    for kind in [Kind::Canary, Kind::Group] {
        let name = format!("{}-tag-write-path", kind.case());
        let (_, arn) = seed(&aws, kind, &name, None, region, account).await;

        for step in steps() {
            let case = format!("{}/{}", kind.case(), step.case);
            let answer = match &step.write {
                Write::Tag(tags) => tag(&aws, &arn, tags.clone()).await,
                Write::Untag(keys) => untag(&aws, &arn, keys).await,
            };
            expect_ok(&case, &answer);

            let expected = tag_map(step.expected);
            let listed = list_tags(&aws, &arn).await;
            expect_ok(&case, &listed);
            assert_eq!(
                listed.at("Tags"),
                &expected,
                "case {case}: expected ListTagsForResource to report the tags the write \
                 left, got {:?}",
                listed.at("Tags")
            );

            let on_resource = tags_on_the_resource(&aws, kind, &name, &arn).await;
            assert_eq!(
                on_resource, expected,
                "case {case}: expected the resource's own tag member to report the tags \
                 the write left, got {on_resource:?}"
            );
        }
    }
}

/// A write past the documented tag count is refused and stores nothing, and a write that
/// only re-values keys the resource already carries fits at the maximum.
#[tokio::test]
async fn a_write_that_would_exceed_the_documented_tag_count_is_refused_and_writes_nothing() {
    let region = "eu-central-1";
    let aws = Synthetics::new(region);
    let account = emulator_server::emulator_account_id();

    for kind in [Kind::Canary, Kind::Group] {
        let case = kind.case();
        let name = format!("{case}-at-the-tag-limit");
        // Filled to exactly the maximum by the create, so the next call is the one past it.
        let (_, arn) = seed(&aws, kind, &name, Some(full_tag_set()), region, account).await;

        let refused = tag(&aws, &arn, pairs(&[("one-too-many", "v")])).await;
        assert_eq!(
            refused.status, 400,
            "case {case}/one-past-the-maximum: expected 400, got {} ({:?})",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("BadRequestException"),
            "case {case}/one-past-the-maximum: expected BadRequestException, got {:?}",
            refused.error_code
        );
        let listed = list_tags(&aws, &arn).await;
        expect_ok(case, &listed);
        assert_eq!(
            listed.at("Tags"),
            &tag_map(full_tag_set()),
            "case {case}/one-past-the-maximum: expected the refused write to change \
             nothing, got {:?}",
            listed.at("Tags")
        );

        // Re-valuing a key the resource already carries adds none, so it fits.
        let revalued = tag(&aws, &arn, pairs(&[("key0", "re-valued")])).await;
        expect_ok(&format!("{case}/re-value-at-the-maximum"), &revalued);
        let mut expected = full_tag_set();
        expected[0] = ("key0".to_string(), "re-valued".to_string());
        let listed = list_tags(&aws, &arn).await;
        expect_ok(case, &listed);
        assert_eq!(
            listed.at("Tags"),
            &tag_map(expected),
            "case {case}/re-value-at-the-maximum: expected the re-valued key, got {:?}",
            listed.at("Tags")
        );
    }
}

/// A tag write against a well-formed `ResourceArn` that names no resource this store
/// holds is refused with the modelled 404, carrying the one fixed sentence every tag
/// operation answers that miss with — and the resources that do exist are untouched.
#[tokio::test]
async fn a_tag_write_against_an_unresolvable_resource_arn_is_refused_with_the_modelled_404() {
    let region = "ap-southeast-2";
    let aws = Synthetics::new(region);
    let account = emulator_server::emulator_account_id();

    // A resource that DOES exist, so each miss below is one ARN field away from a hit.
    let live_tags = pairs(&[("env", "prod")]);
    let (_, live_arn) = seed(
        &aws,
        Kind::Group,
        "live-group",
        Some(live_tags.clone()),
        region,
        account,
    )
    .await;
    let live_id = live_arn
        .rsplit(':')
        .next()
        .expect("a group ARN ends in its id")
        .to_string();

    let rows = vec![
        (
            "unknown-canary-name",
            format!("arn:aws:synthetics:{region}:{account}:canary:no-such-canary"),
        ),
        (
            "unknown-group-id",
            format!("arn:aws:synthetics:{region}:{account}:group:0123456789ab"),
        ),
        (
            // The live group's id, in another Region: a resource lives in exactly one.
            "existing-group-wrong-region",
            format!("arn:aws:synthetics:us-west-1:{account}:group:{live_id}"),
        ),
        (
            "existing-group-wrong-account",
            format!("arn:aws:synthetics:{region}:210987654321:group:{live_id}"),
        ),
        (
            // The right resource segment under the wrong kind: tags do not leak across
            // the two collections.
            "existing-group-id-as-a-canary",
            format!("arn:aws:synthetics:{region}:{account}:canary:{live_id}"),
        ),
    ];

    for (case, arn) in rows {
        for (verb, answer) in [
            ("tag", tag(&aws, &arn, pairs(&[("env", "dev")])).await),
            ("untag", untag(&aws, &arn, &["env".to_string()]).await),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case}/{verb}: expected 404, got {} ({:?})",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code.as_deref(),
                Some("NotFoundException"),
                "case {case}/{verb}: expected NotFoundException, got {:?}",
                answer.error_code
            );
            let message = answer.at("Message").as_str().unwrap_or_default().to_string();
            assert_eq!(
                message, RESOURCE_NOT_FOUND,
                "case {case}/{verb}: expected the fixed missing-resource sentence, got \
                 {message:?}"
            );
        }
    }

    let listed = list_tags(&aws, &live_arn).await;
    expect_ok("live-group", &listed);
    assert_eq!(
        listed.at("Tags"),
        &tag_map(live_tags),
        "case live-group: expected the seeded tags to survive every miss, got {:?}",
        listed.at("Tags")
    );
}
