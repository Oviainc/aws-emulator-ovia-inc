// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ElastiCache tagging slice: `AddTagsToResource`,
//! `ListTagsForResource` and `RemoveTagsFromResource`.
//!
//! Every assertion goes over the wire through [`crate::harness`]: an `awsQuery`
//! form-urlencoded request is dispatched through the same `MultiServiceRouter` the
//! production emulator mounts, and only the HTTP status and the XML body are
//! inspected. Nothing reaches into the service's state struct, so a response that is
//! not computed from persisted state cannot pass.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * a tag set belongs to ONE resource, and there is ONE copy of it: tags a create's
//!   `Tags` member stored are the tags `ListTagsForResource` reports and the tags
//!   `AddTagsToResource` / `RemoveTagsFromResource` change, for every kind of resource
//!   an ElastiCache ARN can name;
//! * `AddTagsToResource` MERGES: a key the resource does not carry is added, a key it
//!   does carry takes the new value, and a key the request does not name is left alone;
//! * a tag's VALUE is stored as the request gave it — "a tag key must be accompanied
//!   by a tag value, although null is accepted", and a null value is reported as the
//!   empty one;
//! * a tag a caller may not write is refused and changes NOTHING: a `Key` the request
//!   omitted or left empty, a key or value past the length ElastiCache publishes, a key
//!   under AWS's reserved `aws:` prefix, and a request that would push the resource past
//!   the documented 50 tags;
//! * `RemoveTagsFromResource` removes exactly the keys `TagKeys` names, and a key the
//!   resource does not carry is the modelled `TagNotFoundFault` with nothing removed;
//! * an ARN is resolved WHOLE: a value that is not an ElastiCache resource ARN is the
//!   modelled 400 `InvalidARNFault`, and a well-formed ARN naming no stored resource —
//!   including another Region's, account's or partition's spelling of a stored name — is
//!   that resource kind's own not-found fault;
//! * `ListTagsForResource` is a READ: two of them agree and neither changes what a
//!   later caller sees.

use crate::cache_cluster_lifecycle::{account_subnet_ids, subnet_group_params};
use crate::harness::{params, unescape_xml, Emulator, Reply, PAST_TRANSITION, REGION};
use service_com_amazonaws_elasticache_amazonelasticachev9::state;

/// The account id every generated emulator presents, which every ARN it mints carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The cache parameter group family every group this suite creates belongs to, from
/// `CacheParameterGroup$CacheParameterGroupFamily`'s own valid values.
const FAMILY: &str = "redis7";

/// One tag's awsQuery form pairs at (1-based) `index`.
///
/// `Tags.Tag.<n>.Key` / `Tags.Tag.<n>.Value`: `TagList`'s member carries
/// `xmlName: Tag` (`smithy_query query="shape" shape="TagList"`). A tag whose value is
/// `None` sends NO `Value` pair at all — the "although null is accepted" form.
fn tag_form(index: usize, key: &str, value: Option<&str>) -> Vec<(String, String)> {
    let mut pairs = vec![(format!("Tags.Tag.{index}.Key"), key.to_string())];
    if let Some(value) = value {
        pairs.push((format!("Tags.Tag.{index}.Value"), value.to_string()));
    }
    pairs
}

/// `AddTagsToResource` on `arn`, carrying `tags` as its `Tags` member.
///
/// `pub(crate)` so the suite about WHICH rule a refusal states drives the same
/// three tagging operations through the same request builders rather than
/// carrying a second copy of them.
pub(crate) async fn add_tags(
    emulator: &Emulator,
    arn: &str,
    tags: &[(&str, Option<&str>)],
) -> Reply {
    let mut pairs = params(&[("ResourceName", arn)]);
    for (index, (key, value)) in tags.iter().enumerate() {
        pairs.extend(tag_form(index + 1, key, *value));
    }
    emulator.call("AddTagsToResource", &pairs).await
}

/// `AddTagsToResource` on `arn` from already-built `Tags` form pairs — what a row
/// sends that cannot be expressed as key/value pairs (a tag with no `Key` at all).
async fn add_tags_raw(emulator: &Emulator, arn: &str, tags: &[(String, String)]) -> Reply {
    let mut pairs = params(&[("ResourceName", arn)]);
    pairs.extend(tags.iter().cloned());
    emulator.call("AddTagsToResource", &pairs).await
}

/// `ListTagsForResource` on `arn`.
pub(crate) async fn list_tags(emulator: &Emulator, arn: &str) -> Reply {
    emulator
        .call("ListTagsForResource", &params(&[("ResourceName", arn)]))
        .await
}

/// `RemoveTagsFromResource` on `arn`, naming `keys`.
///
/// `TagKeys.member.<n>`: `KeyList`'s member carries no `xmlName`
/// (`smithy_query query="shape" shape="KeyList"`), so it keeps the default spelling.
pub(crate) async fn remove_tags(emulator: &Emulator, arn: &str, keys: &[&str]) -> Reply {
    let mut pairs = params(&[("ResourceName", arn)]);
    for (index, key) in keys.iter().enumerate() {
        pairs.push((format!("TagKeys.member.{}", index + 1), (*key).to_string()));
    }
    emulator.call("RemoveTagsFromResource", &pairs).await
}

/// The text of `<name>` inside one `<Tag>` element, as the EMPTY string when the
/// element is present and empty (`<name/>` or `<name></name>`) — which is how the
/// awsQuery envelope carries a tag value that is present and empty, and what the
/// harness's text reader cannot tell from an absent member.
fn element(block: &str, name: &str, case: &str, body: &str) -> String {
    let open = format!("<{name}>");
    if let Some(start) = block.find(&open) {
        let tail = &block[start + open.len()..];
        let end = tail
            .find(&format!("</{name}>"))
            .unwrap_or_else(|| panic!("case {case:?}: unterminated <{name}> in {body}"));
        return unescape_xml(&tail[..end]);
    }
    if block.contains(&format!("<{name}/>")) || block.contains(&format!("<{name} />")) {
        return String::new();
    }
    panic!("case {case:?}: no <{name}> in the tag element {block:?} of {body}")
}

/// The `TagList` a tagging response carries, as `(Key, Value)` pairs in the order the
/// wire carried them.
///
/// Each `<Tag>` element is read WHOLE rather than through the harness's per-element
/// text reader, so a tag whose value is present and empty is read as the empty value
/// instead of dropping out of the list and misaligning every pair after it.
fn tag_list(reply: &Reply, case: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = reply.body.as_str();
    while let Some(at) = rest.find("<Tag") {
        let after = &rest[at + "<Tag".len()..];
        let open_end = after
            .find('>')
            .unwrap_or_else(|| panic!("case {case:?}: unterminated tag in {}", reply.body));
        // `<TagList>` opens with `<Tag` too, so only an open tag whose NAME ends here
        // is a tag element — the rest of the open tag is either nothing or the
        // attributes the protocol puts on the first one (`<Tag xmlns="…">`).
        let attributes = &after[..open_end];
        let body = &after[open_end + 1..];
        if !(attributes.is_empty() || attributes.starts_with(char::is_whitespace)) {
            rest = body;
            continue;
        }
        let end = body
            .find("</Tag>")
            .unwrap_or_else(|| panic!("case {case:?}: unterminated <Tag> in {}", reply.body));
        let block = &body[..end];
        out.push((
            element(block, "Key", case, &reply.body),
            element(block, "Value", case, &reply.body),
        ));
        rest = &body[end..];
    }
    out
}

/// Assert `reply` succeeded and reports exactly `expected`, in that order.
fn assert_tags(case: &str, where_from: &str, reply: &Reply, expected: &[(&str, &str)]) {
    assert_eq!(
        reply.status, 200,
        "case {case:?} ({where_from}): expected 200, got {} {}",
        reply.status, reply.body
    );
    let actual = tag_list(reply, case);
    let expected: Vec<(String, String)> = expected
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    assert_eq!(
        actual, expected,
        "case {case:?} ({where_from}): expected tags {expected:?}, got {actual:?} in {}",
        reply.body
    );
}

/// One refusal row: the case name, the `Tags` form pairs the request carries, and the
/// awsQuery error code it must be answered with.
type TagRefusalCase = (&'static str, Vec<(String, String)>, &'static str);

/// One kind of resource an ElastiCache ARN can name: the ARN's resource-type segment,
/// and the status plus awsQuery error code the three tagging operations answer for an
/// ARN of that kind that resolves to no stored resource (the model lists one not-found
/// fault per kind; `smithy_query query="operation" operation="AddTagsToResource"`).
struct Kind {
    segment: &'static str,
    status: u16,
    not_found: &'static str,
    /// Whether this emulator can create a resource of this kind at all. The three it
    /// cannot — the EC2-Classic cache security group, the reserved cache node bought
    /// from an offering and the serverless cache snapshot — have no resource an ARN
    /// could resolve to here, which is exactly what their faults say.
    creatable: bool,
}

/// Every resource kind an ElastiCache ARN can name, creatable here or not.
const KINDS: &[Kind] = &[
    Kind {
        segment: "cluster",
        status: 404,
        not_found: "CacheClusterNotFound",
        creatable: true,
    },
    Kind {
        segment: "snapshot",
        status: 404,
        not_found: "SnapshotNotFoundFault",
        creatable: true,
    },
    Kind {
        segment: "parametergroup",
        status: 404,
        not_found: "CacheParameterGroupNotFound",
        creatable: true,
    },
    Kind {
        segment: "subnetgroup",
        status: 400,
        not_found: "CacheSubnetGroupNotFoundFault",
        creatable: true,
    },
    Kind {
        segment: "replicationgroup",
        status: 404,
        not_found: "ReplicationGroupNotFoundFault",
        creatable: true,
    },
    Kind {
        segment: "serverlesscache",
        status: 404,
        not_found: "ServerlessCacheNotFoundFault",
        creatable: true,
    },
    Kind {
        segment: "user",
        status: 404,
        not_found: "UserNotFound",
        creatable: true,
    },
    Kind {
        segment: "usergroup",
        status: 404,
        not_found: "UserGroupNotFound",
        creatable: true,
    },
    Kind {
        segment: "globalreplicationgroup",
        status: 404,
        not_found: "GlobalReplicationGroupNotFoundFault",
        creatable: true,
    },
    Kind {
        segment: "reserved-instance",
        status: 404,
        not_found: "ReservedCacheNodeNotFound",
        creatable: false,
    },
    Kind {
        segment: "securitygroup",
        status: 404,
        not_found: "CacheSecurityGroupNotFound",
        creatable: false,
    },
    Kind {
        segment: "serverlesscachesnapshot",
        status: 404,
        not_found: "ServerlessCacheSnapshotNotFoundFault",
        creatable: false,
    },
];

/// Create one resource of `segment` named `name`, carrying `tags` as its create's own
/// `Tags` member, and answer the ARN the service minted for it.
///
/// The ARN is read out of the CREATE's response, so every later call in a case names
/// the resource the way a client would.
async fn create_resource(
    emulator: &Emulator,
    segment: &str,
    name: &str,
    tags: &[(&str, Option<&str>)],
) -> String {
    let (action, mut pairs) = match segment {
        "cluster" => (
            "CreateCacheCluster",
            params(&[("CacheClusterId", name), ("CacheNodeType", "cache.t3.micro")]),
        ),
        "snapshot" => {
            // A snapshot is taken of a cluster that is `available`, so the source
            // cluster comes first and the clock is given its transition span.
            let source = format!("{name}-source");
            let created = emulator
                .call(
                    "CreateCacheCluster",
                    &params(&[
                        ("CacheClusterId", source.as_str()),
                        ("CacheNodeType", "cache.t3.micro"),
                    ]),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {name:?}: the source cluster must be created, got {} {}",
                created.status, created.body
            );
            std::thread::sleep(PAST_TRANSITION);
            (
                "CreateSnapshot",
                params(&[("SnapshotName", name), ("CacheClusterId", &source)]),
            )
        }
        "parametergroup" => (
            "CreateCacheParameterGroup",
            params(&[
                ("CacheParameterGroupName", name),
                ("CacheParameterGroupFamily", FAMILY),
                ("Description", "the tagging suite"),
            ]),
        ),
        "subnetgroup" => {
            let subnets = account_subnet_ids(emulator).await;
            (
                "CreateCacheSubnetGroup",
                subnet_group_params(name, "the tagging suite", &subnets[..2]),
            )
        }
        "replicationgroup" => (
            "CreateReplicationGroup",
            params(&[
                ("ReplicationGroupId", name),
                ("ReplicationGroupDescription", "the tagging suite"),
                ("CacheNodeType", "cache.t3.micro"),
            ]),
        ),
        "serverlesscache" => (
            "CreateServerlessCache",
            params(&[("ServerlessCacheName", name), ("Engine", "redis")]),
        ),
        "globalreplicationgroup" => {
            // A Global datastore is built around a replication group that is
            // `available`, so that primary comes first and the clock is given
            // its transition span. The datastore's own id is the region's
            // prefix and this name, which is why every later call in the case
            // names it by the ARN the create answered with.
            let primary = format!("{name}-primary");
            let created = emulator
                .call(
                    "CreateReplicationGroup",
                    &params(&[
                        ("ReplicationGroupId", primary.as_str()),
                        ("ReplicationGroupDescription", "the tagging suite"),
                        ("CacheNodeType", "cache.t3.micro"),
                    ]),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {name:?}: the primary must be created, got {} {}",
                created.status, created.body
            );
            std::thread::sleep(PAST_TRANSITION);
            (
                "CreateGlobalReplicationGroup",
                params(&[
                    ("GlobalReplicationGroupIdSuffix", name),
                    ("PrimaryReplicationGroupId", &primary),
                ]),
            )
        }
        "user" => (
            "CreateUser",
            params(&[
                ("UserId", name),
                ("UserName", name),
                ("Engine", "redis"),
                ("AccessString", "on ~* +@all"),
                ("NoPasswordRequired", "true"),
            ]),
        ),
        "usergroup" => (
            "CreateUserGroup",
            params(&[
                ("UserGroupId", name),
                ("Engine", "redis"),
                // "The list of user IDs that belong to the user group": every group
                // has to list the account's built-in `default` user.
                ("UserIds.member.1", "default"),
            ]),
        ),
        other => panic!("case {name:?}: this suite cannot create a {other}"),
    };
    for (index, (key, value)) in tags.iter().enumerate() {
        pairs.extend(tag_form(index + 1, key, *value));
    }
    let created = emulator.call(action, &pairs).await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: {action} must answer 200, got {} {}",
        created.status, created.body
    );
    let arn = created
        .text("ARN")
        .unwrap_or_else(|| panic!("case {name:?}: {action} must report an ARN, got {}", created.body))
        .to_string();
    arn
}

#[tokio::test]
async fn a_tag_set_round_trips_for_every_kind_of_resource_this_emulator_can_create() {
    // The axis: WHICH kind of resource the ARN names. Every kind keeps its tags on its
    // own record, so the rule has to hold for each of them and not just for the cache
    // cluster the recorded case used.
    for kind in KINDS.iter().filter(|kind| kind.creatable) {
        let emulator = Emulator::new();
        let case = kind.segment;
        let name = format!("tagged-{}", kind.segment);
        // The create's own `Tags` member writes the SAME tag set the tagging
        // operations read and write, so a create-time tag is part of every row.
        let arn = create_resource(
            &emulator,
            kind.segment,
            &name,
            &[("created-by", Some("the-create"))],
        )
        .await;

        let listed = list_tags(&emulator, &arn).await;
        assert_tags(
            case,
            "the create's tags, listed",
            &listed,
            &[("created-by", "the-create")],
        );

        let added = add_tags(
            &emulator,
            &arn,
            &[("team", Some("cache")), ("env", Some("prod"))],
        )
        .await;
        let after_add = [
            ("created-by", "the-create"),
            ("team", "cache"),
            ("env", "prod"),
        ];
        assert_tags(case, "AddTagsToResource", &added, &after_add);
        let listed = list_tags(&emulator, &arn).await;
        assert_tags(case, "listed after the add", &listed, &after_add);

        // A MERGE: `env` takes the new value in place, `owner` is appended, and
        // `created-by` / `team` are left alone.
        let merged = add_tags(
            &emulator,
            &arn,
            &[("env", Some("staging")), ("owner", Some("platform"))],
        )
        .await;
        let after_merge = [
            ("created-by", "the-create"),
            ("team", "cache"),
            ("env", "staging"),
            ("owner", "platform"),
        ];
        assert_tags(case, "the merging add", &merged, &after_merge);
        let listed = list_tags(&emulator, &arn).await;
        assert_tags(case, "listed after the merge", &listed, &after_merge);

        let removed = remove_tags(&emulator, &arn, &["team", "created-by"]).await;
        let after_remove = [("env", "staging"), ("owner", "platform")];
        assert_tags(case, "RemoveTagsFromResource", &removed, &after_remove);

        // A list is a READ: two of them agree, and the second sees what the first
        // did not change.
        let first = list_tags(&emulator, &arn).await;
        let second = list_tags(&emulator, &arn).await;
        assert_tags(case, "the first list after the remove", &first, &after_remove);
        assert_tags(
            case,
            "the second list after the remove",
            &second,
            &after_remove,
        );

        // Every tag can go, and the resource is then reported with an EMPTY tag set
        // rather than with a fault.
        let emptied = remove_tags(&emulator, &arn, &["env", "owner"]).await;
        assert_tags(case, "removing the last tags", &emptied, &[]);
        let listed = list_tags(&emulator, &arn).await;
        assert_tags(case, "listed with no tags left", &listed, &[]);
    }
}

#[tokio::test]
async fn a_tag_value_is_stored_as_the_request_gave_it() {
    let emulator = Emulator::new();
    let arn = create_resource(&emulator, "cluster", "value-forms", &[]).await;

    // The axis: the FORM of the value — its length (none, one character, the published
    // bound), whether the request named `Value` at all, and which characters it
    // carries (ASCII, non-ASCII, every symbol the published tag character set allows).
    // The characters the XML envelope escapes are OUTSIDE that character set, so they
    // belong to the refusal table in
    // [`a_tag_outside_the_published_character_set_is_refused`] rather than here.
    let at_bound = "v".repeat(state::MAX_TAG_VALUE_LEN);
    let symbols: String = state::TAG_SYMBOLS.iter().collect();
    let cases: [(&str, Option<&str>, &str); 7] = [
        ("an ASCII value", Some("cache"), "cache"),
        ("one character", Some("x"), "x"),
        // "A tag key must be accompanied by a tag value, although null is accepted":
        // an empty value and an absent one are both stored, and both report empty.
        ("an empty value", Some(""), ""),
        ("no Value member at all", None, ""),
        (
            "a non-ASCII value",
            Some("déjà-vu-日本語-ß"),
            "déjà-vu-日本語-ß",
        ),
        (
            "every symbol the published character set allows",
            Some(symbols.as_str()),
            symbols.as_str(),
        ),
        (
            "a value at the published length bound",
            Some(at_bound.as_str()),
            at_bound.as_str(),
        ),
    ];

    for (index, (case, sent, expected)) in cases.iter().enumerate() {
        let key = format!("k{index}");
        let added = add_tags(&emulator, &arn, &[(key.as_str(), *sent)]).await;
        assert_eq!(
            added.status, 200,
            "case {case:?}: AddTagsToResource must answer 200, got {} {}",
            added.status, added.body
        );
        let listed = list_tags(&emulator, &arn).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: ListTagsForResource must answer 200, got {} {}",
            listed.status, listed.body
        );
        let actual = tag_list(&listed, case);
        let held = actual
            .iter()
            .find(|(held, _)| *held == key)
            .map(|(_, value)| value.clone());
        assert_eq!(
            held.as_deref(),
            Some(*expected),
            "case {case:?}: expected the value {expected:?} under {key}, got {held:?} in {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_tag_key_at_the_published_bound_is_accepted() {
    let emulator = Emulator::new();
    let arn = create_resource(&emulator, "cluster", "key-bounds", &[]).await;

    // The bound itself is a row: "Maximum key length — 128 Unicode characters"
    // (`state::MAX_TAG_KEY_LEN`), counted in CHARACTERS, so a non-ASCII key of the
    // same character count is accepted too.
    let ascii = "k".repeat(state::MAX_TAG_KEY_LEN);
    let non_ascii = "é".repeat(state::MAX_TAG_KEY_LEN);
    for (case, key) in [
        ("an ASCII key at the bound", ascii.as_str()),
        ("a non-ASCII key at the bound", non_ascii.as_str()),
        ("a key that merely contains aws:", "team-aws:owned"),
    ] {
        let added = add_tags(&emulator, &arn, &[(key, Some("v"))]).await;
        assert_eq!(
            added.status, 200,
            "case {case:?}: AddTagsToResource must answer 200, got {} {}",
            added.status, added.body
        );
        let listed = list_tags(&emulator, &arn).await;
        let actual = tag_list(&listed, case);
        assert!(
            actual.iter().any(|(held, _)| held == key),
            "case {case:?}: expected the key to be stored, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_tag_a_caller_may_not_use_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let arn = create_resource(&emulator, "cluster", "refusals", &[("keep", Some("me"))]).await;

    let long_key = "k".repeat(state::MAX_TAG_KEY_LEN + 1);
    let long_value = "v".repeat(state::MAX_TAG_VALUE_LEN + 1);
    // The axis: WHICH rule the tag breaks — the key's presence, each length bound one
    // past its published value, the reserved prefix in either letter case, and the
    // per-resource tag count one past the documented maximum.
    let mut cases: Vec<TagRefusalCase> = vec![
        (
            "a tag with no Key at all",
            vec![("Tags.Tag.1.Value".to_string(), "orphan".to_string())],
            "InvalidParameterValue",
        ),
        (
            "an empty Key",
            vec![
                ("Tags.Tag.1.Key".to_string(), String::new()),
                ("Tags.Tag.1.Value".to_string(), "v".to_string()),
            ],
            "InvalidParameterValue",
        ),
        (
            "a Key one character past the bound",
            vec![
                ("Tags.Tag.1.Key".to_string(), long_key.clone()),
                ("Tags.Tag.1.Value".to_string(), "v".to_string()),
            ],
            "InvalidParameterValue",
        ),
        (
            "a Value one character past the bound",
            vec![
                ("Tags.Tag.1.Key".to_string(), "k".to_string()),
                ("Tags.Tag.1.Value".to_string(), long_value.clone()),
            ],
            "InvalidParameterValue",
        ),
        (
            "the reserved aws: prefix",
            vec![
                ("Tags.Tag.1.Key".to_string(), "aws:cost-centre".to_string()),
                ("Tags.Tag.1.Value".to_string(), "v".to_string()),
            ],
            "InvalidParameterValue",
        ),
        (
            "the reserved prefix in another letter case",
            vec![
                ("Tags.Tag.1.Key".to_string(), "AWS:Cost-Centre".to_string()),
                ("Tags.Tag.1.Value".to_string(), "v".to_string()),
            ],
            "InvalidParameterValue",
        ),
        (
            "a good tag beside a refused one",
            vec![
                ("Tags.Tag.1.Key".to_string(), "fine".to_string()),
                ("Tags.Tag.1.Value".to_string(), "v".to_string()),
                ("Tags.Tag.2.Key".to_string(), "aws:managed".to_string()),
                ("Tags.Tag.2.Value".to_string(), "v".to_string()),
            ],
            "InvalidParameterValue",
        ),
    ];
    // One past "the maximum number of tags permitted on a resource is 50"
    // (`TagQuotaPerResourceExceeded`'s own documentation).
    let mut over_quota = Vec::new();
    for index in 1..=state::MAX_TAGS_PER_RESOURCE + 1 {
        over_quota.push((format!("Tags.Tag.{index}.Key"), format!("q{index}")));
        over_quota.push((format!("Tags.Tag.{index}.Value"), format!("v{index}")));
    }
    cases.push((
        "one tag past the per-resource maximum",
        over_quota,
        "TagQuotaPerResourceExceeded",
    ));

    for (case, tags, expected) in &cases {
        let reply = add_tags_raw(&emulator, &arn, tags).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            *expected,
            "case {case:?}: expected {expected}, got {} in {}",
            reply.error_code(),
            reply.body
        );
        // The refusal changed NOTHING: the resource still carries exactly the tag its
        // create gave it.
        let listed = list_tags(&emulator, &arn).await;
        assert_tags(case, "listed after the refusal", &listed, &[("keep", "me")]);
    }

    // A caller may not REMOVE a reserved key either: the same rule holds wherever a
    // request names tag keys.
    for case in ["aws:cost-centre", "AWS:Cost-Centre"] {
        let reply = remove_tags(&emulator, &arn, &[case]).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: RemoveTagsFromResource must answer 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {} in {}",
            reply.error_code(),
            reply.body
        );
    }
}

/// The character set a tag key or a tag value may use: "unicode letters, digits,
/// whitespace, or one of these symbols: `_ . : / = + \ - @`", the set ElastiCache
/// names in its own refusal and this crate states once as
/// `state::is_tag_character` / `state::TAG_SYMBOLS`. Neither `Tag$Key` nor
/// `Tag$Value` carries a modelled `@pattern`, so nothing before the handler refuses a
/// character outside it.
///
/// The table's axes: WHICH class of character the text carries (letters of several
/// scripts, digits, whitespace, each permitted symbol — and, on the refusal side, a
/// pictograph, the characters the XML envelope escapes, and punctuation the set omits),
/// WHICH half of the pair carries it (the key or the value), and WHICH entry point the
/// request came through (a create's own `Tags` and `AddTagsToResource`). The recorded
/// emoji-in-a-value case is one row of it.
#[tokio::test]
async fn a_tag_outside_the_published_character_set_is_refused() {
    let emulator = Emulator::new();
    let arn = create_resource(&emulator, "cluster", "charset", &[("keep", Some("me"))]).await;

    // Every class the published set names, one row each, plus one row per symbol in
    // it — so a validator that refused a legal character fails here.
    let mut accepted: Vec<(String, String)> = vec![
        (
            "unicode letters of three scripts".to_string(),
            "Ünicode-日本語-Кириллица".to_string(),
        ),
        ("digits".to_string(), "0123456789".to_string()),
        (
            "letters, digits and whitespace".to_string(),
            "cost centre 42".to_string(),
        ),
        (
            "whitespace that is not a space".to_string(),
            "two\twords".to_string(),
        ),
    ];
    for symbol in state::TAG_SYMBOLS {
        accepted.push((format!("the symbol {symbol:?}"), format!("a{symbol}b")));
    }
    for (index, (case, text)) in accepted.iter().enumerate() {
        let key = format!("ok{index}");
        let added = add_tags(&emulator, &arn, &[(key.as_str(), Some(text.as_str()))]).await;
        assert_eq!(
            added.status, 200,
            "case {case:?}: AddTagsToResource must answer 200, got {} {}",
            added.status, added.body
        );
        let listed = list_tags(&emulator, &arn).await;
        let held = tag_list(&listed, case);
        assert!(
            held.contains(&(key.clone(), text.clone())),
            "case {case:?}: expected {text:?} to be stored under {key}, got {held:?}"
        );
    }

    // The refused side: one row per kind of character the set leaves out.
    let refused: [(&str, &str); 7] = [
        ("a pictograph", "party-\u{1F389}-time"),
        ("an ampersand", "a&b"),
        ("angle brackets", "a<b>c"),
        ("a double quote", "say-\"what\""),
        ("a percent sign", "100%-cache"),
        ("a comma", "one,two"),
        ("a parenthesis", "cache-(prod)"),
    ];
    let before = tag_list(&list_tags(&emulator, &arn).await, "before the refusals");
    for (case, text) in refused {
        // Both halves of the pair carry the same rule, and so does every entry point
        // that names tags: `AddTagsToResource` and a create's own `Tags`.
        for (member, key, value) in [("Value", "charset-key", text), ("Key", text, "v")] {
            let added = add_tags(&emulator, &arn, &[(key, Some(value))]).await;
            assert_eq!(
                (added.status, added.error_code()),
                (400, "InvalidParameterValue"),
                "case {case:?} in the tag {member}: AddTagsToResource must answer 400 \
                 InvalidParameterValue, got {} {}",
                added.status,
                added.body
            );

            // The same text through a CREATE's `Tags`: refused, and the resource the
            // create named is not stored either.
            let name = "charset-group";
            let created = emulator
                .call(
                    "CreateCacheParameterGroup",
                    &[
                        params(&[
                            ("CacheParameterGroupName", name),
                            ("CacheParameterGroupFamily", FAMILY),
                            ("Description", "the tagging suite"),
                        ]),
                        tag_form(1, key, Some(value)),
                    ]
                    .concat(),
                )
                .await;
            assert_eq!(
                (created.status, created.error_code()),
                (400, "InvalidParameterValue"),
                "case {case:?} in the tag {member}: CreateCacheParameterGroup must answer 400 \
                 InvalidParameterValue, got {} {}",
                created.status,
                created.body
            );
            let described = emulator
                .call(
                    "DescribeCacheParameterGroups",
                    &params(&[("CacheParameterGroupName", name)]),
                )
                .await;
            assert_eq!(
                (described.status, described.error_code()),
                (404, "CacheParameterGroupNotFound"),
                "case {case:?} in the tag {member}: the refused create must have stored no \
                 group, got {} {}",
                described.status,
                described.body
            );
        }
    }
    // Not one refusal changed the tags the resource holds.
    let after = tag_list(&list_tags(&emulator, &arn).await, "after the refusals");
    assert_eq!(
        after, before,
        "the refused requests must change nothing, got {after:?} against {before:?}"
    );
}

#[tokio::test]
async fn the_per_resource_tag_maximum_bounds_the_merged_set() {
    let emulator = Emulator::new();
    let arn = create_resource(&emulator, "cluster", "quota", &[]).await;

    // Filled to exactly the maximum, which is the bound itself.
    let filling: Vec<(String, String)> = (1..=state::MAX_TAGS_PER_RESOURCE)
        .flat_map(|index| {
            [
                (format!("Tags.Tag.{index}.Key"), format!("q{index}")),
                (format!("Tags.Tag.{index}.Value"), format!("v{index}")),
            ]
        })
        .collect();
    let filled = add_tags_raw(&emulator, &arn, &filling).await;
    assert_eq!(
        filled.status, 200,
        "case at the maximum: AddTagsToResource must answer 200, got {} {}",
        filled.status, filled.body
    );
    assert_eq!(
        tag_list(&filled, "at the maximum").len(),
        state::MAX_TAGS_PER_RESOURCE,
        "case at the maximum: expected {} tags, got {}",
        state::MAX_TAGS_PER_RESOURCE,
        filled.body
    );

    // Re-tagging a key the resource already carries adds no tag, so it stays within
    // the maximum and is accepted...
    let replaced = add_tags(&emulator, &arn, &[("q1", Some("replaced"))]).await;
    assert_eq!(
        replaced.status, 200,
        "case re-tagging an existing key: must answer 200, got {} {}",
        replaced.status, replaced.body
    );
    let held = tag_list(&replaced, "re-tagging an existing key");
    assert_eq!(
        held.len(),
        state::MAX_TAGS_PER_RESOURCE,
        "case re-tagging an existing key: expected {} tags, got {held:?}",
        state::MAX_TAGS_PER_RESOURCE
    );
    assert!(
        held.contains(&("q1".to_string(), "replaced".to_string())),
        "case re-tagging an existing key: expected q1 to take the new value, got {held:?}"
    );

    // ...while one NEW key would push the resource past the maximum, and is refused
    // without changing the set it already holds.
    let refused = add_tags(&emulator, &arn, &[("one-too-many", Some("v"))]).await;
    assert_eq!(
        refused.status, 400,
        "case one new key past the maximum: expected 400, got {} {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "TagQuotaPerResourceExceeded",
        "case one new key past the maximum: expected TagQuotaPerResourceExceeded, got {} in {}",
        refused.error_code(),
        refused.body
    );
    let listed = list_tags(&emulator, &arn).await;
    let after = tag_list(&listed, "one new key past the maximum");
    assert_eq!(
        after, held,
        "case one new key past the maximum: the refused add must change nothing, got {after:?}"
    );
}

#[tokio::test]
async fn removing_a_key_the_resource_does_not_carry_removes_nothing() {
    let emulator = Emulator::new();
    let arn = create_resource(&emulator, "cluster", "removals", &[]).await;
    let added = add_tags(
        &emulator,
        &arn,
        &[("team", Some("cache")), ("env", Some("prod"))],
    )
    .await;
    assert_tags("the setup", "AddTagsToResource", &added, &[
        ("team", "cache"),
        ("env", "prod"),
    ]);

    // The axis: WHERE the unknown key sits in `TagKeys` — alone, before a known key,
    // after one — because the whole list is checked before the first removal.
    for (case, keys) in [
        ("an unknown key alone", vec!["ghost"]),
        ("an unknown key first", vec!["ghost", "team"]),
        ("an unknown key last", vec!["team", "ghost"]),
        ("two unknown keys", vec!["ghost", "spectre"]),
    ] {
        let reply = remove_tags(&emulator, &arn, &keys).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected 404, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "TagNotFound",
            "case {case:?}: expected TagNotFound, got {} in {}",
            reply.error_code(),
            reply.body
        );
        let listed = list_tags(&emulator, &arn).await;
        assert_tags(
            case,
            "listed after the refused remove",
            &listed,
            &[("team", "cache"), ("env", "prod")],
        );
    }

    // Removing a key the resource DOES carry answers with what it is left with.
    let removed = remove_tags(&emulator, &arn, &["env"]).await;
    assert_tags(
        "removing a held key",
        "RemoveTagsFromResource",
        &removed,
        &[("team", "cache")],
    );
    // Removing it twice is not idempotent on this API: the second call names a tag the
    // resource no longer carries, which is the modelled `TagNotFoundFault`.
    let again = remove_tags(&emulator, &arn, &["env"]).await;
    assert_eq!(
        again.status, 404,
        "case removing a key twice: expected 404, got {} {}",
        again.status, again.body
    );
}

#[tokio::test]
async fn an_arn_that_names_no_stored_resource_is_that_kinds_own_fault() {
    let emulator = Emulator::new();
    // One stored cluster, so the Region / account / partition rows below differ from a
    // resolving ARN in exactly one field.
    let stored = create_resource(&emulator, "cluster", "resolved", &[]).await;
    assert_eq!(
        stored,
        state::cluster_arn(REGION, "resolved"),
        "the create must mint the ARN this service's own helper builds"
    );

    // The axis: the KIND the ARN names. Every kind is a row, including the ones this
    // emulator creates no resource of.
    for kind in KINDS {
        let arn = format!(
            "arn:aws:elasticache:{REGION}:{ACCOUNT}:{}:no-such-resource",
            kind.segment
        );
        assert_refused_by_every_operation(&emulator, kind.segment, &arn, kind.status, kind.not_found)
            .await;
    }

    // The same name in another Region, account or partition is another resource, and
    // this store holds none of them: an ARN is resolved WHOLE.
    for (case, arn) in [
        (
            "another Region",
            state::cluster_arn("eu-west-1", "resolved"),
        ),
        (
            "another account",
            format!("arn:aws:elasticache:{REGION}:111122223333:cluster:resolved"),
        ),
        (
            "another partition",
            format!("arn:aws-cn:elasticache:{REGION}:{ACCOUNT}:cluster:resolved"),
        ),
        (
            "another resource type carrying the same name",
            format!("arn:aws:elasticache:{REGION}:{ACCOUNT}:snapshot:resolved"),
        ),
    ] {
        let expected = if case == "another resource type carrying the same name" {
            "SnapshotNotFoundFault"
        } else {
            "CacheClusterNotFound"
        };
        assert_refused_by_every_operation(&emulator, case, &arn, 404, expected).await;
    }
}

#[tokio::test]
async fn a_value_that_is_not_an_elasticache_arn_is_the_modelled_invalid_arn_fault() {
    let emulator = Emulator::new();
    create_resource(&emulator, "cluster", "well-formed", &[]).await;

    // The axis: WHICH part of the ARN form is wrong — the `arn` prefix, the partition,
    // the service, the Region, the account, the resource type, the resource name, and
    // the separator between the last two.
    let cases = [
        ("no ARN form at all", "not-an-arn".to_string()),
        ("the empty string", String::new()),
        (
            "another service's ARN",
            format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/well-formed"),
        ),
        ("a global service's ARN", "arn:aws:s3:::a-bucket".to_string()),
        (
            "a resource type this service has none of",
            format!("arn:aws:elasticache:{REGION}:{ACCOUNT}:widget:well-formed"),
        ),
        (
            "no partition",
            format!("arn::elasticache:{REGION}:{ACCOUNT}:cluster:well-formed"),
        ),
        (
            "no Region",
            format!("arn:aws:elasticache::{ACCOUNT}:cluster:well-formed"),
        ),
        (
            "no account",
            format!("arn:aws:elasticache:{REGION}::cluster:well-formed"),
        ),
        (
            "no resource name",
            format!("arn:aws:elasticache:{REGION}:{ACCOUNT}:cluster:"),
        ),
        (
            "no separator before the resource name",
            format!("arn:aws:elasticache:{REGION}:{ACCOUNT}:cluster"),
        ),
        (
            "the wrong prefix",
            format!("urn:aws:elasticache:{REGION}:{ACCOUNT}:cluster:well-formed"),
        ),
    ];

    for (case, arn) in &cases {
        assert_refused_by_every_operation(&emulator, case, arn, 400, "InvalidARN").await;
    }
}

/// Assert all THREE tagging operations refuse `arn` with `status` and `code`.
///
/// The resolution is one implementation shared by the three, so each one has to answer
/// the same way for the same ARN — which is what keeps a caller from writing a tag
/// through one operation that another cannot read.
async fn assert_refused_by_every_operation(
    emulator: &Emulator,
    case: &str,
    arn: &str,
    status: u16,
    code: &str,
) {
    let replies = [
        ("AddTagsToResource", add_tags(emulator, arn, &[("k", Some("v"))]).await),
        ("ListTagsForResource", list_tags(emulator, arn).await),
        ("RemoveTagsFromResource", remove_tags(emulator, arn, &["k"]).await),
    ];
    for (operation, reply) in &replies {
        assert_eq!(
            reply.status, status,
            "case {case:?} ({operation}): expected {status}, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            code,
            "case {case:?} ({operation}): expected {code}, got {} in {}",
            reply.error_code(),
            reply.body
        );
    }
}
