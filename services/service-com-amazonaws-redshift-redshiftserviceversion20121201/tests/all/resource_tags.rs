// SPDX-License-Identifier: CC0-1.0

//! Resource tagging (`CreateTags`, `DescribeTags`, `DeleteTags`), end to end
//! over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The suite asserts the RULE rather than one request: a tag is a key/value pair
//! held ON the resource its `ResourceName` ARN names, so what a write persists is
//! what every read of that resource reports, a key written twice is updated
//! rather than duplicated, and a key deleted is gone. It is asserted over a TABLE
//! of the resource TYPES an ARN can name, a table of the value FORMS a tag can
//! carry (ASCII, multi-byte, empty, each published length bound and one past it),
//! the refusals, the `TagKeys` / `TagValues` filters, and the documented
//! `MaxRecords` window of `DescribeTags` with its `Marker` cursor.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};

/// The Region the shared harness signs every request for, which is the Region
/// the ARNs this suite names and reads are built from.
const REGION: &str = "us-west-2";

/// The cluster most cases tag.
const CLUSTER: &str = "tagged-cluster";

/// The `(Key, Value)` of every tag an awsQuery body carries, in document order.
///
/// Scanned per `<Tag>` block rather than by zipping two flat element lists, so a
/// tag whose value is the EMPTY string — which the serializer may render as
/// `<Value></Value>` or `<Value/>` — still pairs with its own key.
fn tags_in(xml: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<Tag>") {
        let inner = &rest[start + "<Tag>".len()..];
        let Some(end) = inner.find("</Tag>") else { break };
        let block = &inner[..end];
        found.push((
            all_text(block, "Key").into_iter().next().unwrap_or_default(),
            all_text(block, "Value")
                .into_iter()
                .next()
                .unwrap_or_default(),
        ));
        rest = &inner[end + "</Tag>".len()..];
    }
    found
}

/// The `(ResourceType, Key, Value)` of every `TaggedResource` a `DescribeTags`
/// body carries, in the order the page lists them.
fn tagged_resources_in(xml: &str) -> Vec<(String, String, String)> {
    let types = all_text(xml, "ResourceType");
    tags_in(xml)
        .into_iter()
        .enumerate()
        .map(|(index, (key, value))| {
            (
                types.get(index).cloned().unwrap_or_default(),
                key,
                value,
            )
        })
        .collect()
}

/// The `ResourceName` of every `TaggedResource` a `DescribeTags` body carries.
fn tagged_arns_in(xml: &str) -> Vec<String> {
    all_text(xml, "ResourceName")
}

/// One `(name, value)` form parameter list, owned so a case can build it.
type Params = Vec<(String, String)>;

/// Borrow an owned parameter list for [`Emulator::call`].
fn borrowed(params: &Params) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// The form parameters that carry `tags` on a `CreateTags` request.
fn tag_params(tags: &[(&str, &str)]) -> Params {
    let mut params = Params::new();
    for (index, (key, value)) in tags.iter().enumerate() {
        let ordinal = index + 1;
        params.push((format!("Tags.Tag.{ordinal}.Key"), (*key).to_string()));
        params.push((format!("Tags.Tag.{ordinal}.Value"), (*value).to_string()));
    }
    params
}

/// Tag `resource_name` with `tags`.
async fn create_tags(emulator: &Emulator, resource_name: &str, tags: &[(&str, &str)]) -> Answer {
    let mut params = vec![("ResourceName".to_string(), resource_name.to_string())];
    params.extend(tag_params(tags));
    emulator.call("CreateTags", &borrowed(&params)).await
}

/// Delete `keys` from `resource_name`'s tags.
async fn delete_tags(emulator: &Emulator, resource_name: &str, keys: &[&str]) -> Answer {
    let mut params = vec![("ResourceName".to_string(), resource_name.to_string())];
    for (index, key) in keys.iter().enumerate() {
        let ordinal = index + 1;
        params.push((format!("TagKeys.TagKey.{ordinal}"), (*key).to_string()));
    }
    emulator.call("DeleteTags", &borrowed(&params)).await
}

/// Read one resource's tags back through `DescribeTags`.
async fn describe_tags_of(emulator: &Emulator, resource_name: &str) -> Answer {
    emulator
        .call("DescribeTags", &[("ResourceName", resource_name)])
        .await
}

/// Create the cluster every single-resource case tags.
async fn seeded_cluster(emulator: &Emulator, identifier: &str) -> String {
    let created = emulator.create_minimal(identifier).await;
    assert_eq!(
        created.status, 200,
        "case seed cluster {identifier}: expected 200, got {} with body {}",
        created.status, created.body
    );
    state::cluster_arn(identifier, REGION)
}

/// One row of the resource-type table: the case name, the calls that create the
/// resource, the ARN that names it, the `ResourceType` `DescribeTags` must report
/// for it, and the `Describe*` call (action plus the member naming it) that reads
/// the resource's own record back.
struct ResourceCase {
    case: &'static str,
    create: Vec<(&'static str, Params)>,
    arn: String,
    resource_type: &'static str,
    read_back: (&'static str, Params),
}

/// One row per resource type an ARN can name in this store.
fn resource_cases() -> Vec<ResourceCase> {
    let owned = |pairs: &[(&str, &str)]| -> Params {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect()
    };
    vec![
        ResourceCase {
            case: "a provisioned cluster",
            create: vec![(
                "CreateCluster",
                owned(&[
                    ("ClusterIdentifier", "tags-cluster"),
                    ("NodeType", "ra3.xlplus"),
                    ("MasterUsername", "admin"),
                ]),
            )],
            arn: state::cluster_arn("tags-cluster", REGION),
            resource_type: "cluster",
            read_back: (
                "DescribeClusters",
                owned(&[("ClusterIdentifier", "tags-cluster")]),
            ),
        },
        ResourceCase {
            case: "a cluster snapshot",
            create: vec![
                (
                    "CreateCluster",
                    owned(&[
                        ("ClusterIdentifier", "tags-snap-source"),
                        ("NodeType", "ra3.xlplus"),
                        ("MasterUsername", "admin"),
                    ]),
                ),
                (
                    "CreateClusterSnapshot",
                    owned(&[
                        ("ClusterIdentifier", "tags-snap-source"),
                        ("SnapshotIdentifier", "tags-snap"),
                    ]),
                ),
            ],
            arn: state::snapshot_arn("tags-snap-source", "tags-snap", REGION),
            resource_type: "snapshot",
            read_back: (
                "DescribeClusterSnapshots",
                owned(&[("SnapshotIdentifier", "tags-snap")]),
            ),
        },
        ResourceCase {
            case: "a cluster parameter group",
            create: vec![(
                "CreateClusterParameterGroup",
                owned(&[
                    ("ParameterGroupName", "tags-group"),
                    ("ParameterGroupFamily", "redshift-1.0"),
                    ("Description", "a group a case tags"),
                ]),
            )],
            arn: state::parameter_group_arn("tags-group", REGION),
            resource_type: "parametergroup",
            read_back: (
                "DescribeClusterParameterGroups",
                owned(&[("ParameterGroupName", "tags-group")]),
            ),
        },
        ResourceCase {
            case: "a cluster subnet group",
            create: vec![(
                "CreateClusterSubnetGroup",
                owned(&[
                    ("ClusterSubnetGroupName", "tags-subnets"),
                    ("Description", "the subnets a case tags"),
                    ("SubnetIds.SubnetIdentifier.1", "subnet-0123456789abcdef0"),
                ]),
            )],
            arn: state::subnet_group_arn("tags-subnets", REGION),
            resource_type: "subnetgroup",
            read_back: (
                "DescribeClusterSubnetGroups",
                owned(&[("ClusterSubnetGroupName", "tags-subnets")]),
            ),
        },
    ]
}

#[tokio::test]
async fn tags_round_trip_through_every_resource_type_an_arn_names() {
    let emulator = Emulator::new();

    for case in resource_cases() {
        for (action, params) in &case.create {
            let created = emulator.call(action, &borrowed(params)).await;
            assert_eq!(
                created.status,
                200,
                "case {}: expected the seeding {action} to answer 200, got {} with body {}",
                case.case,
                created.status,
                created.body
            );
        }

        let tagged = create_tags(
            &emulator,
            &case.arn,
            &[("owner", "analytics"), ("environment", "test")],
        )
        .await;
        assert_eq!(
            tagged.status, 200,
            "case {}: expected CreateTags on {} to answer 200, got {} with body {}",
            case.case, case.arn, tagged.status, tagged.body
        );

        // Read back through DescribeTags: the pairs persisted, in request order,
        // against this resource's ARN and type.
        let reported = describe_tags_of(&emulator, &case.arn).await;
        assert_eq!(
            reported.status, 200,
            "case {}: expected DescribeTags to answer 200, got {} with body {}",
            case.case, reported.status, reported.body
        );
        let expected = vec![
            (
                case.resource_type.to_string(),
                "owner".to_string(),
                "analytics".to_string(),
            ),
            (
                case.resource_type.to_string(),
                "environment".to_string(),
                "test".to_string(),
            ),
        ];
        assert_eq!(
            tagged_resources_in(&reported.body),
            expected,
            "case {}: expected {expected:?}, got {:?}",
            case.case,
            tagged_resources_in(&reported.body)
        );
        assert!(
            tagged_arns_in(&reported.body)
                .iter()
                .all(|arn| *arn == case.arn),
            "case {}: expected every entry to carry {}, got {:?}",
            case.case,
            case.arn,
            tagged_arns_in(&reported.body)
        );

        // And read back through the resource's OWN record: one tag store, so the
        // resource a `Describe*` reports carries the same pairs.
        let (action, params) = &case.read_back;
        let record = emulator.call(action, &borrowed(params)).await;
        assert_eq!(
            record.status, 200,
            "case {}: expected {action} to answer 200, got {} with body {}",
            case.case, record.status, record.body
        );
        let on_record = tags_in(&record.body);
        assert_eq!(
            on_record,
            vec![
                ("owner".to_string(), "analytics".to_string()),
                ("environment".to_string(), "test".to_string()),
            ],
            "case {}: expected {action} to report the tags the write persisted, got {on_record:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn a_tag_value_of_any_form_round_trips_unchanged() {
    let emulator = Emulator::new();
    let arn = seeded_cluster(&emulator, CLUSTER).await;

    // One row per form the rule generalises over: the character set, the size
    // (including each published bound) and the empty value. The bounds come from
    // the Amazon Redshift Cluster Management Guide's tag restrictions, which
    // `state::TAG_KEY_MAX_LEN` / `state::TAG_VALUE_MAX_LEN` carry.
    let longest_value: String = "v".repeat(state::TAG_VALUE_MAX_LEN);
    let longest_key: String = std::iter::once('k')
        .chain(std::iter::repeat_n('x', state::TAG_KEY_MAX_LEN - 1))
        .collect();
    let cases: Vec<(&str, String, String)> = vec![
        ("ascii", "owner".to_string(), "admin".to_string()),
        (
            "multi-byte characters",
            "proprietário".to_string(),
            "análise de dados 日本語 🎉".to_string(),
        ),
        ("the empty value", "empty".to_string(), String::new()),
        (
            "punctuation and spaces",
            "cost center".to_string(),
            "team: analytics/eu-west+1 = 2".to_string(),
        ),
        ("the longest key", longest_key, "at the key bound".to_string()),
        ("the longest value", "longest".to_string(), longest_value),
    ];

    for (case, key, value) in &cases {
        let tagged = create_tags(&emulator, &arn, &[(key.as_str(), value.as_str())]).await;
        assert_eq!(
            tagged.status, 200,
            "case {case}: expected CreateTags to answer 200, got {} with body {}",
            tagged.status, tagged.body
        );
    }

    let reported = describe_tags_of(&emulator, &arn).await;
    assert_eq!(
        reported.status, 200,
        "case read back: expected 200, got {} with body {}",
        reported.status, reported.body
    );
    let persisted = tags_in(&reported.body);
    for (case, key, value) in &cases {
        let found = persisted
            .iter()
            .find(|(stored_key, _)| stored_key == key)
            .cloned();
        assert_eq!(
            found,
            Some((key.clone(), value.clone())),
            "case {case}: expected the pair back unchanged, got {found:?}"
        );
    }
    assert_eq!(
        persisted.len(),
        cases.len(),
        "case read back: expected {} tags, got {persisted:?}",
        cases.len()
    );
}

#[tokio::test]
async fn a_key_the_resource_already_carries_is_updated_in_place() {
    let emulator = Emulator::new();
    let arn = seeded_cluster(&emulator, CLUSTER).await;

    assert_eq!(
        create_tags(
            &emulator,
            &arn,
            &[("owner", "first"), ("environment", "test")]
        )
        .await
        .status,
        200,
        "case seed two tags: expected 200"
    );

    // Each row writes ONE request and states the pairs the resource must carry
    // afterwards, in order: an existing key keeps its position and takes the new
    // value, a new key is appended, and a key repeated inside one request lands
    // once with the later value.
    type UpsertCase<'a> = (&'a str, Vec<(&'a str, &'a str)>, Vec<(&'a str, &'a str)>);
    let cases: Vec<UpsertCase> = vec![
        (
            "an existing key takes the new value",
            vec![("owner", "second")],
            vec![("owner", "second"), ("environment", "test")],
        ),
        (
            "a new key is appended",
            vec![("team", "analytics")],
            vec![
                ("owner", "second"),
                ("environment", "test"),
                ("team", "analytics"),
            ],
        ),
        (
            "a key repeated in one request lands once",
            vec![("owner", "third"), ("owner", "fourth")],
            vec![
                ("owner", "fourth"),
                ("environment", "test"),
                ("team", "analytics"),
            ],
        ),
        (
            "an empty value replaces a non-empty one",
            vec![("team", "")],
            vec![("owner", "fourth"), ("environment", "test"), ("team", "")],
        ),
    ];

    for (case, written, expected) in cases {
        let tagged = create_tags(&emulator, &arn, &written).await;
        assert_eq!(
            tagged.status, 200,
            "case {case}: expected 200, got {} with body {}",
            tagged.status, tagged.body
        );
        let reported = describe_tags_of(&emulator, &arn).await;
        let persisted = tags_in(&reported.body);
        let expected: Vec<(String, String)> = expected
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            persisted, expected,
            "case {case}: expected {expected:?}, got {persisted:?}"
        );
    }
}

#[tokio::test]
async fn a_tag_outside_the_published_restrictions_is_refused_and_persists_nothing() {
    let emulator = Emulator::new();
    let arn = seeded_cluster(&emulator, CLUSTER).await;

    // Each row is a tag and whether the published restrictions admit it: a key
    // of 1..=128 characters, a value of 0..=256, and neither starting with the
    // reserved `aws:` prefix. Every bound is tested AT the bound and one past
    // it.
    let too_long_key: String = "k".repeat(state::TAG_KEY_MAX_LEN + 1);
    let longest_key: String = "j".repeat(state::TAG_KEY_MAX_LEN);
    let too_long_value: String = "v".repeat(state::TAG_VALUE_MAX_LEN + 1);
    let longest_value: String = "w".repeat(state::TAG_VALUE_MAX_LEN);
    let cases: Vec<(&str, String, String, bool)> = vec![
        ("an empty key", String::new(), "x".to_string(), false),
        (
            "a key one past the bound",
            too_long_key,
            "x".to_string(),
            false,
        ),
        ("a key at the bound", longest_key, "x".to_string(), true),
        (
            "a value one past the bound",
            "over".to_string(),
            too_long_value,
            false,
        ),
        (
            "a value at the bound",
            "at-bound".to_string(),
            longest_value,
            true,
        ),
        (
            "the reserved prefix on the key",
            format!("{}owner", state::RESERVED_TAG_PREFIX),
            "x".to_string(),
            false,
        ),
        (
            "the reserved prefix on the value",
            "reserved-value".to_string(),
            format!("{}managed", state::RESERVED_TAG_PREFIX),
            false,
        ),
        (
            "the reserved prefix in any case",
            "AWS:Owner".to_string(),
            "x".to_string(),
            false,
        ),
    ];

    let mut accepted: Vec<(String, String)> = Vec::new();
    for (case, key, value, admitted) in &cases {
        let answer = create_tags(&emulator, &arn, &[(key.as_str(), value.as_str())]).await;
        if *admitted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} with body {}",
                answer.status, answer.body
            );
            accepted.push((key.clone(), value.clone()));
        } else {
            assert_eq!(
                (answer.status, answer.error_code()),
                (400, Some("InvalidTagFault".to_string())),
                "case {case}: expected 400 InvalidTagFault, got {} / {:?}",
                answer.status,
                answer.error_code()
            );
        }
    }

    let persisted = tags_in(&describe_tags_of(&emulator, &arn).await.body);
    assert_eq!(
        persisted, accepted,
        "case refused tags persist nothing: expected only {accepted:?}, got {persisted:?}"
    );
}

#[tokio::test]
async fn one_resource_carries_no_more_tags_than_the_quota() {
    let emulator = Emulator::new();
    let arn = seeded_cluster(&emulator, CLUSTER).await;

    let full: Vec<(String, String)> = (0..state::TAGS_PER_RESOURCE_QUOTA)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect();
    let at_quota: Vec<(&str, &str)> = full
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let filled = create_tags(&emulator, &arn, &at_quota).await;
    assert_eq!(
        filled.status, 200,
        "case at the quota: expected 200 for {} tags, got {} with body {}",
        state::TAGS_PER_RESOURCE_QUOTA,
        filled.status,
        filled.body
    );

    let over = create_tags(&emulator, &arn, &[("one-too-many", "x")]).await;
    assert_eq!(
        (over.status, over.error_code()),
        (400, Some("TagLimitExceededFault".to_string())),
        "case one past the quota: expected 400 TagLimitExceededFault, got {} / {:?}",
        over.status,
        over.error_code()
    );

    // One request carrying more entries than a resource may hold is refused on
    // its length alone, whatever the resource already carries: the fresh cluster
    // below carries none.
    let fresh = seeded_cluster(&emulator, "quota-fresh-cluster").await;
    let oversized: Vec<(String, String)> = (0..=state::TAGS_PER_RESOURCE_QUOTA)
        .map(|index| (format!("bulk-{index:02}"), format!("value-{index:02}")))
        .collect();
    let bulk: Vec<(&str, &str)> = oversized
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let refused = create_tags(&emulator, &fresh, &bulk).await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (400, Some("TagLimitExceededFault".to_string())),
        "case one request past the quota: expected 400 TagLimitExceededFault for {} tags, got {} / {:?}",
        bulk.len(),
        refused.status,
        refused.error_code()
    );
    let untouched = tags_in(&describe_tags_of(&emulator, &fresh).await.body);
    assert!(
        untouched.is_empty(),
        "case one request past the quota: expected the resource to stay untagged, got {untouched:?}"
    );

    // "The attempt will fail": the refused request persisted nothing, and a
    // request that only UPDATES an existing key stays inside the quota.
    let persisted = tags_in(&describe_tags_of(&emulator, &arn).await.body);
    assert_eq!(
        persisted.len(),
        state::TAGS_PER_RESOURCE_QUOTA,
        "case one past the quota: expected the resource to still carry {} tags, got {}",
        state::TAGS_PER_RESOURCE_QUOTA,
        persisted.len()
    );
    let updated = create_tags(&emulator, &arn, &[("key-00", "replaced")]).await;
    assert_eq!(
        updated.status, 200,
        "case updating a key at the quota: expected 200, got {} with body {}",
        updated.status, updated.body
    );
    let persisted = tags_in(&describe_tags_of(&emulator, &arn).await.body);
    assert_eq!(
        persisted.first().cloned(),
        Some(("key-00".to_string(), "replaced".to_string())),
        "case updating a key at the quota: expected the first tag updated, got {:?}",
        persisted.first()
    );
}

#[tokio::test]
async fn deleting_tags_removes_only_the_named_keys() {
    let emulator = Emulator::new();
    let arn = seeded_cluster(&emulator, CLUSTER).await;

    assert_eq!(
        create_tags(
            &emulator,
            &arn,
            &[
                ("owner", "admin"),
                ("environment", "test"),
                ("team", "analytics"),
            ]
        )
        .await
        .status,
        200,
        "case seed three tags: expected 200"
    );

    // Each row deletes keys and states what the resource must carry afterwards:
    // a key it does not carry removes nothing (so the call is idempotent), the
    // match is case-sensitive, and several keys go in one call.
    type DeleteCase<'a> = (&'a str, Vec<&'a str>, Vec<(&'a str, &'a str)>);
    let cases: Vec<DeleteCase> = vec![
        (
            "a key the resource does not carry",
            vec!["absent"],
            vec![
                ("owner", "admin"),
                ("environment", "test"),
                ("team", "analytics"),
            ],
        ),
        (
            "a key in the wrong case",
            vec!["Owner"],
            vec![
                ("owner", "admin"),
                ("environment", "test"),
                ("team", "analytics"),
            ],
        ),
        (
            "one key",
            vec!["environment"],
            vec![("owner", "admin"), ("team", "analytics")],
        ),
        (
            "the same key again",
            vec!["environment"],
            vec![("owner", "admin"), ("team", "analytics")],
        ),
        ("two keys at once", vec!["owner", "team"], vec![]),
    ];

    for (case, keys, expected) in cases {
        let deleted = delete_tags(&emulator, &arn, &keys).await;
        assert_eq!(
            deleted.status, 200,
            "case {case}: expected 200, got {} with body {}",
            deleted.status, deleted.body
        );
        let persisted = tags_in(&describe_tags_of(&emulator, &arn).await.body);
        let expected: Vec<(String, String)> = expected
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            persisted, expected,
            "case {case}: expected {expected:?}, got {persisted:?}"
        );
    }
}

#[tokio::test]
async fn a_resource_name_that_names_no_resource_is_refused() {
    let emulator = Emulator::new();
    let arn = seeded_cluster(&emulator, CLUSTER).await;
    assert_eq!(
        create_tags(&emulator, &arn, &[("owner", "admin")])
            .await
            .status,
        200,
        "case seed one tag: expected 200"
    );

    // Each row is a `ResourceName` that names nothing this account holds: a
    // resource id no record carries, a type with no resources, another
    // service's ARN, an ARN of another Region or another account — an ARN names
    // the resource of ONE partition, Region and account — and strings that are
    // not a Redshift resource ARN at all.
    let cases: Vec<(&str, String)> = vec![
        (
            "a cluster that does not exist",
            state::cluster_arn("no-such-cluster", REGION),
        ),
        (
            "this cluster in another Region",
            state::cluster_arn(CLUSTER, "eu-west-1"),
        ),
        (
            "this cluster in another account",
            format!("arn:aws:redshift:{REGION}:123456789012:cluster:{CLUSTER}"),
        ),
        (
            "this cluster in another partition",
            format!(
                "arn:aws-cn:redshift:{REGION}:{}:cluster:{CLUSTER}",
                engine::emulator_account_id()
            ),
        ),
        (
            "a snapshot that does not exist",
            state::snapshot_arn(CLUSTER, "no-such-snapshot", REGION),
        ),
        (
            "a parameter group that does not exist",
            state::parameter_group_arn("no-such-group", REGION),
        ),
        (
            "a resource type this account holds none of",
            engine::emulator_arn("redshift", REGION, "snapshotcopygrant:no-such-grant"),
        ),
        (
            "another service's ARN",
            engine::emulator_arn("rds", REGION, "db:tagged-cluster"),
        ),
        (
            "a Redshift ARN with no resource type",
            engine::emulator_arn("redshift", REGION, "tagged-cluster"),
        ),
        (
            "a Redshift ARN with an unpublished resource type",
            engine::emulator_arn("redshift", REGION, "widget:tagged-cluster"),
        ),
        ("not an ARN at all", CLUSTER.to_string()),
    ];

    for (case, resource_name) in cases {
        for (action, answer) in [
            (
                "CreateTags",
                create_tags(&emulator, &resource_name, &[("owner", "admin")]).await,
            ),
            (
                "DeleteTags",
                delete_tags(&emulator, &resource_name, &["owner"]).await,
            ),
            (
                "DescribeTags",
                describe_tags_of(&emulator, &resource_name).await,
            ),
        ] {
            assert_eq!(
                (answer.status, answer.error_code()),
                (404, Some("ResourceNotFoundFault".to_string())),
                "case {case} on {action}: expected 404 ResourceNotFoundFault, got {} / {:?}",
                answer.status,
                answer.error_code()
            );
        }
    }

    // The resource that DOES exist is untouched by any of those refusals.
    let persisted = tags_in(&describe_tags_of(&emulator, &arn).await.body);
    assert_eq!(
        persisted,
        vec![("owner".to_string(), "admin".to_string())],
        "case refusals leave the tagged resource alone: got {persisted:?}"
    );
}

#[tokio::test]
async fn describe_tags_selects_by_resource_type_and_by_tag() {
    let emulator = Emulator::new();

    // Two tagged resources: a cluster carrying TWO tags and a parameter group
    // carrying one, so both what a filter selects and how much of a selected
    // resource it reports are observable.
    let cluster = seeded_cluster(&emulator, "filter-cluster").await;
    let group = emulator
        .call(
            "CreateClusterParameterGroup",
            &[
                ("ParameterGroupName", "filter-group"),
                ("ParameterGroupFamily", "redshift-1.0"),
                ("Description", "a group a filter selects"),
            ],
        )
        .await;
    assert_eq!(
        group.status, 200,
        "case seed filter-group: expected 200, got {} with body {}",
        group.status, group.body
    );
    let group_arn = state::parameter_group_arn("filter-group", REGION);
    for (resource, tags) in [
        (&cluster, vec![("owner", "admin"), ("environment", "test")]),
        (&group_arn, vec![("owner", "tester")]),
    ] {
        let tagged = create_tags(&emulator, resource, &tags).await;
        assert_eq!(
            tagged.status, 200,
            "case seed tags on {resource}: expected 200, got {} with body {}",
            tagged.status, tagged.body
        );
    }

    // Each row is a request filter and the `(ResourceType, Key, Value)` entries
    // it must select. The `ResourceType` spellings cover the ARN segment, the
    // name the model publishes and a case the issue did not cite. The filters
    // select RESOURCES, so a row whose key or value matches ONE tag of the
    // cluster reports BOTH of its tags.
    type FilterCase<'a> = (
        &'a str,
        Vec<(&'a str, &'a str)>,
        Vec<(&'a str, &'a str, &'a str)>,
    );
    let cluster_tags = vec![
        ("cluster", "owner", "admin"),
        ("cluster", "environment", "test"),
    ];
    let group_tags = vec![("parametergroup", "owner", "tester")];
    let both = || -> Vec<(&str, &str, &str)> {
        cluster_tags
            .iter()
            .chain(group_tags.iter())
            .copied()
            .collect()
    };
    let cases: Vec<FilterCase> = vec![
        ("no filter reports every tag", vec![], both()),
        (
            "the ARN segment of one type",
            vec![("ResourceType", "cluster")],
            cluster_tags.clone(),
        ),
        (
            "the published name of one type",
            vec![("ResourceType", "Parameter group")],
            group_tags.clone(),
        ),
        (
            "a type spelled in another case",
            vec![("ResourceType", "SubnetGroup")],
            vec![],
        ),
        (
            "a published type this account holds none of",
            vec![("ResourceType", "Snapshot copy grant")],
            vec![],
        ),
        (
            "one tag value selects the whole resource",
            vec![("TagValues.TagValue.1", "admin")],
            cluster_tags.clone(),
        ),
        (
            "a key only one of a resource's tags carries selects the whole resource",
            vec![("TagKeys.TagKey.1", "environment")],
            cluster_tags.clone(),
        ),
        (
            "two tag values are inclusive",
            vec![
                ("TagValues.TagValue.1", "admin"),
                ("TagValues.TagValue.2", "tester"),
            ],
            both(),
        ),
        (
            "one tag key",
            vec![("TagKeys.TagKey.1", "owner")],
            both(),
        ),
        (
            "a key nothing carries",
            vec![("TagKeys.TagKey.1", "absent")],
            vec![],
        ),
        (
            "a key and a value together",
            vec![
                ("TagKeys.TagKey.1", "owner"),
                ("TagValues.TagValue.1", "admin"),
            ],
            cluster_tags.clone(),
        ),
        (
            "a type and a tag together",
            vec![
                ("ResourceType", "cluster"),
                ("TagValues.TagValue.1", "tester"),
            ],
            vec![],
        ),
        (
            "an ARN with a tag key it carries",
            vec![
                ("ResourceName", cluster.as_str()),
                ("TagKeys.TagKey.1", "environment"),
            ],
            cluster_tags.clone(),
        ),
        (
            "an ARN with a tag key it does not carry",
            vec![
                ("ResourceName", cluster.as_str()),
                ("TagKeys.TagKey.1", "absent"),
            ],
            vec![],
        ),
    ];

    for (case, filter, expected) in cases {
        let answer = emulator.call("DescribeTags", &filter).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        let expected: Vec<(String, String, String)> = expected
            .into_iter()
            .map(|(resource_type, key, value)| {
                (
                    resource_type.to_string(),
                    key.to_string(),
                    value.to_string(),
                )
            })
            .collect();
        let reported = tagged_resources_in(&answer.body);
        assert_eq!(
            reported, expected,
            "case {case}: expected {expected:?}, got {reported:?}"
        );
    }

    // The documented limitations: an ARN cannot be combined with a resource type,
    // nor with the pagination members.
    let combinations: Vec<(&str, Vec<(&str, &str)>)> = vec![
        (
            "an ARN with a resource type",
            vec![
                ("ResourceName", cluster.as_str()),
                ("ResourceType", "cluster"),
            ],
        ),
        (
            "an ARN with MaxRecords",
            vec![("ResourceName", cluster.as_str()), ("MaxRecords", "20")],
        ),
        (
            "an ARN with a Marker",
            vec![("ResourceName", cluster.as_str()), ("Marker", "0.deadbeef")],
        ),
    ];
    for (case, filter) in combinations {
        let answer = emulator.call("DescribeTags", &filter).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, Some("InvalidParameterCombination".to_string())),
            "case {case}: expected 400 InvalidParameterCombination, got {} / {:?}",
            answer.status,
            answer.error_code()
        );
    }

    // A resource type the service does not publish names no resource at all.
    let unknown = emulator
        .call("DescribeTags", &[("ResourceType", "widget")])
        .await;
    assert_eq!(
        (unknown.status, unknown.error_code()),
        (400, Some("InvalidParameterValue".to_string())),
        "case an unpublished resource type: expected 400 InvalidParameterValue, got {} / {:?}",
        unknown.status,
        unknown.error_code()
    );
}

#[tokio::test]
async fn describe_tags_pages_inside_its_documented_max_records_window() {
    let emulator = Emulator::new();
    let arn = seeded_cluster(&emulator, CLUSTER).await;

    // Twelve tags, so a page of the window's minimum leaves a second page.
    let tags: Vec<(String, String)> = (0..12)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect();
    let written: Vec<(&str, &str)> = tags
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    assert_eq!(
        create_tags(&emulator, &arn, &written).await.status,
        200,
        "case seed twelve tags: expected 200"
    );

    // Each row is a `MaxRecords` and whether the service admits it: "The
    // MaxRecords parameter can be a range from 10 to 50 results to return in a
    // request" (`state::TAG_MAX_RECORDS_WINDOW`), tested at each bound and one
    // past each. Only the minimum is enforced, so a page past the published
    // maximum is served.
    let window = state::TAG_MAX_RECORDS_WINDOW;
    let cases: Vec<(&str, i32, bool)> = vec![
        ("one below the minimum", window.min - 1, false),
        ("the minimum", window.min, true),
        ("the maximum", window.max, true),
        ("one above the maximum", window.max + 1, true),
        ("zero", 0, false),
        ("negative", -1, false),
    ];
    for (case, max_records, admitted) in cases {
        let value = max_records.to_string();
        let answer = emulator
            .call("DescribeTags", &[("MaxRecords", value.as_str())])
            .await;
        if admitted {
            assert_eq!(
                answer.status, 200,
                "case MaxRecords {case}: expected 200, got {} with body {}",
                answer.status, answer.body
            );
        } else {
            assert_eq!(
                (answer.status, answer.error_code()),
                (400, Some("InvalidParameterValue".to_string())),
                "case MaxRecords {case}: expected 400 InvalidParameterValue, got {} / {:?}",
                answer.status,
                answer.error_code()
            );
        }
    }

    // A page of the minimum hands back a cursor, and the cursor resumes the same
    // sequence exactly where the page ended.
    let page_size = window.min.to_string();
    let first = emulator
        .call("DescribeTags", &[("MaxRecords", page_size.as_str())])
        .await;
    let first_page = tags_in(&first.body);
    assert_eq!(
        first_page.len(),
        window.min as usize,
        "case the first page: expected {} entries, got {first_page:?}",
        window.min
    );
    let marker = first.text("Marker");
    let marker = marker.unwrap_or_else(|| {
        panic!(
            "case the first page: expected a Marker, got body {}",
            first.body
        )
    });
    let second = emulator
        .call(
            "DescribeTags",
            &[("MaxRecords", page_size.as_str()), ("Marker", &marker)],
        )
        .await;
    assert_eq!(
        second.status, 200,
        "case the second page: expected 200, got {} with body {}",
        second.status, second.body
    );
    let second_page = tags_in(&second.body);
    assert_eq!(
        second_page.len(),
        tags.len() - window.min as usize,
        "case the second page: expected the remainder, got {second_page:?}"
    );
    assert_eq!(
        second.text("Marker"),
        None,
        "case the second page: expected no Marker once the sequence is exhausted, got {:?}",
        second.text("Marker")
    );
    let walked: Vec<(String, String)> = first_page.into_iter().chain(second_page).collect();
    assert_eq!(
        walked, tags,
        "case the walked pages: expected every tag once, in order, got {walked:?}"
    );

    // A cursor this service did not hand out is refused rather than served as a
    // mid-page offset.
    let bogus = emulator
        .call("DescribeTags", &[("Marker", "7.not-a-marker")])
        .await;
    assert_eq!(
        (bogus.status, bogus.error_code()),
        (400, Some("InvalidParameterValue".to_string())),
        "case a forged cursor: expected 400 InvalidParameterValue, got {} / {:?}",
        bogus.status,
        bogus.error_code()
    );

    // A request that names NO MaxRecords is served the window's maximum — the
    // most the operation serves unasked — and a cursor for the rest. Two
    // resources at the per-resource tag quota put more entries in the store than
    // one page can carry.
    let crowded = Emulator::new();
    for identifier in ["page-cluster-a", "page-cluster-b"] {
        let arn = seeded_cluster(&crowded, identifier).await;
        let full: Vec<(String, String)> = (0..state::TAGS_PER_RESOURCE_QUOTA)
            .map(|index| (format!("{identifier}-{index:02}"), format!("v{index:02}")))
            .collect();
        let written: Vec<(&str, &str)> = full
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        assert_eq!(
            create_tags(&crowded, &arn, &written).await.status,
            200,
            "case seed {identifier} at the quota: expected 200"
        );
    }
    let unasked = crowded.call("DescribeTags", &[]).await;
    assert_eq!(
        unasked.status, 200,
        "case no MaxRecords: expected 200, got {} with body {}",
        unasked.status, unasked.body
    );
    assert_eq!(
        tags_in(&unasked.body).len(),
        window.max as usize,
        "case no MaxRecords: expected the window's maximum {} entries, got {}",
        window.max,
        tags_in(&unasked.body).len()
    );
    assert!(
        unasked.text("Marker").is_some(),
        "case no MaxRecords: expected a Marker for the remaining entries, got body {}",
        unasked.body
    );
}
