// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this service's resource tagging: `TagResource` and the
//! reader that reports its result, `ListTagsForResource`, driven over HTTP
//! through the same multi-service router the emulator boots in production.
//!
//! Every case asserts the RULE the two operations follow, not one recorded
//! request: a tag write MERGES the supplied map into the tag set persisted on
//! the `NotificationConfiguration` the arn names (each supplied key added or
//! overwritten, every other key left alone) and the reader reports exactly that
//! set. Table rows vary along what the rule generalises over — how many keys a
//! write carries, whether each key is new or already stored, the key and value
//! sizes (both published `TagKey`/`TagValue` bounds), the alphabet, the order
//! the keys arrive in, and how many writes are chained — and the bounds the rows
//! draw from are the model's own (`TagKey`'s `^(?!aws:).{1,128}$` pattern,
//! `TagValue`'s `@length` 0..256, `TagMap`'s `@length` 0..200).

use serde_json::{json, Map, Value};

// The HTTP harness this suite drives the service through, shared with every
// other suite in this crate.
use crate::harness::{encode_path_label, error_code, Emulator};

// The tagging-specific helpers this suite adds to the shared harness. An
// inherent `impl` on a type declared elsewhere in the same crate is what keeps
// them beside the tests that use them.
impl Emulator {
    /// Create a configuration carrying `tags` and return its arn, failing the
    /// test on anything other than the modelled 201.
    async fn create_configuration_with_tags(&self, name: &str, tags: &[(String, String)]) -> String {
        let body = json!({
            "name": name,
            "description": "a configuration under test",
            "tags": tag_object(tags),
        });
        let (status, response) = self
            .send("POST", "/notification-configurations", Some(body))
            .await;
        assert_eq!(status, 201, "create must answer 201, got {response:?}");
        response["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("create must answer an arn, got {response:?}"))
            .to_string()
    }

    /// `TagResource`: `POST /tags/{arn}` with the tag map in the body.
    async fn tag_resource(&self, arn: &str, tags: &[(String, String)]) -> (u16, Value) {
        let uri = format!("/tags/{}", encode_path_label(arn));
        self.send("POST", &uri, Some(json!({ "tags": tag_object(tags) })))
            .await
    }

    /// `UntagResource`-free read path: `ListTagsForResource` is `GET /tags/{arn}`.
    async fn list_tags_for_resource(&self, arn: &str) -> (u16, Value) {
        let uri = format!("/tags/{}", encode_path_label(arn));
        self.send("GET", &uri, None).await
    }

    /// The tag set `ListTagsForResource` reports for `arn`, sorted so a
    /// comparison does not depend on map iteration order. Fails the test unless
    /// the read answered the modelled 200.
    async fn reported_tags(&self, arn: &str, case: &str) -> Vec<(String, String)> {
        let (status, response) = self.list_tags_for_resource(arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource must answer 200, got {status} {response:?}"
        );
        let mut pairs: Vec<(String, String)> = response["tags"]
            .as_object()
            .unwrap_or_else(|| {
                panic!("case {case:?}: ListTagsForResource must answer a tags map, got {response:?}")
            })
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: a tag value must be a string, got {value:?}")
                        })
                        .to_string(),
                )
            })
            .collect();
        pairs.sort();
        pairs
    }

    /// `DeleteNotificationConfiguration`, so this suite can assert tagging a
    /// record that is gone is a 404.
    async fn delete_configuration(&self, arn: &str) -> (u16, Value) {
        let uri = format!("/notification-configurations/{}", encode_path_label(arn));
        self.send("DELETE", &uri, None).await
    }
}

/// A tag list as the JSON object the wire carries.
fn tag_object(tags: &[(String, String)]) -> Value {
    let mut map = Map::new();
    for (key, value) in tags {
        map.insert(key.clone(), Value::String(value.clone()));
    }
    Value::Object(map)
}

/// A tag list from borrowed literals, so a table row reads as pairs.
fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The same list sorted, which is the order [`Emulator::reported_tags`] answers
/// in.
fn sorted(entries: &[(String, String)]) -> Vec<(String, String)> {
    let mut owned = entries.to_vec();
    owned.sort();
    owned
}

/// The longest key the modelled `TagKey` pattern `^(?!aws:).{1,128}$` admits.
fn longest_key() -> String {
    "k".repeat(128)
}

/// The longest value the modelled `TagValue` `@length` (0..256) admits.
fn longest_value() -> String {
    "v".repeat(256)
}

/// One row of the merge table: the tags the configuration is created with, the
/// chain of `TagResource` calls applied to it, and the tag set the reader must
/// report afterwards.
struct MergeCase {
    case: &'static str,
    initial: Vec<(String, String)>,
    writes: Vec<Vec<(String, String)>>,
    expected: Vec<(String, String)>,
}

/// The merge table. Rows differ in the number of keys a write carries (none,
/// one, several), whether each key is new or already stored, key and value size
/// (both published bounds, plus the empty value `TagValue`'s minimum admits),
/// the alphabet, the order keys arrive in, and how many writes are chained.
fn merge_cases() -> Vec<MergeCase> {
    let long_key = longest_key();
    let long_value = longest_value();
    vec![
        MergeCase {
            case: "one new key onto an untagged configuration",
            initial: Vec::new(),
            writes: vec![pairs(&[("env", "prod")])],
            expected: pairs(&[("env", "prod")]),
        },
        MergeCase {
            case: "several new keys in one write, onto an untagged configuration",
            initial: Vec::new(),
            writes: vec![pairs(&[("env", "prod"), ("team", "platform"), ("tier", "1")])],
            expected: pairs(&[("env", "prod"), ("team", "platform"), ("tier", "1")]),
        },
        MergeCase {
            case: "a disjoint key leaves the stored keys alone",
            initial: pairs(&[("env", "dev")]),
            writes: vec![pairs(&[("owner", "notifications-team")])],
            expected: pairs(&[("env", "dev"), ("owner", "notifications-team")]),
        },
        MergeCase {
            case: "an existing key is overwritten, not duplicated",
            initial: pairs(&[("env", "dev"), ("tier", "2")]),
            writes: vec![pairs(&[("env", "prod")])],
            expected: pairs(&[("env", "prod"), ("tier", "2")]),
        },
        MergeCase {
            case: "an empty tag map changes nothing",
            initial: pairs(&[("env", "dev")]),
            writes: vec![Vec::new()],
            expected: pairs(&[("env", "dev")]),
        },
        MergeCase {
            case: "repeating the same write is idempotent",
            initial: pairs(&[("env", "dev")]),
            writes: vec![
                pairs(&[("env", "prod"), ("tier", "1")]),
                pairs(&[("env", "prod"), ("tier", "1")]),
                pairs(&[("env", "prod"), ("tier", "1")]),
            ],
            expected: pairs(&[("env", "prod"), ("tier", "1")]),
        },
        MergeCase {
            case: "chained writes accumulate, last write wins per key",
            initial: Vec::new(),
            writes: vec![
                pairs(&[("a", "1")]),
                pairs(&[("b", "2"), ("c", "3")]),
                pairs(&[("a", "9"), ("d", "4")]),
            ],
            expected: pairs(&[("a", "9"), ("b", "2"), ("c", "3"), ("d", "4")]),
        },
        MergeCase {
            case: "key order within a write does not matter",
            initial: Vec::new(),
            writes: vec![
                pairs(&[("zz", "last"), ("mm", "middle"), ("aa", "first")]),
                pairs(&[("aa", "first"), ("mm", "middle"), ("zz", "last")]),
            ],
            expected: pairs(&[("aa", "first"), ("mm", "middle"), ("zz", "last")]),
        },
        MergeCase {
            // Both published sizes: `TagKey`'s 128-character maximum and
            // `TagValue`'s 256-character maximum.
            case: "the longest modelled key and value",
            initial: Vec::new(),
            writes: vec![vec![(long_key.clone(), long_value.clone())]],
            expected: vec![(long_key.clone(), long_value.clone())],
        },
        MergeCase {
            // `TagValue`'s `@length` minimum is 0, so an empty value is a value
            // and not a removal.
            case: "an empty value is stored, not treated as a removal",
            initial: pairs(&[("env", "dev")]),
            writes: vec![pairs(&[("env", "")])],
            expected: pairs(&[("env", "")]),
        },
        MergeCase {
            // `TagKey`'s pattern constrains only the `aws:` prefix and the
            // length, so punctuation and non-ASCII are ordinary keys/values.
            case: "punctuation and non-ASCII in keys and values",
            initial: pairs(&[("cost-centre", "42")]),
            writes: vec![pairs(&[
                ("team:platform/eu-west", "on call"),
                ("étiquette", "valeur-ünïcode"),
                ("k", "a"),
            ])],
            expected: pairs(&[
                ("cost-centre", "42"),
                ("team:platform/eu-west", "on call"),
                ("étiquette", "valeur-ünïcode"),
                ("k", "a"),
            ]),
        },
    ]
}

/// RULE: a tag write merges the supplied map into the tag set persisted on the
/// resource the arn names — every supplied key is added or overwritten and
/// every other stored key keeps its value — and the reader reports exactly the
/// resulting set.
#[tokio::test]
async fn tag_resource_merges_the_supplied_map_into_the_stored_tag_set() {
    for (index, case) in merge_cases().into_iter().enumerate() {
        let emulator = Emulator::new();
        let arn = emulator
            .create_configuration_with_tags(&format!("tag-case-{index}"), &case.initial)
            .await;

        // The reader starts from what create persisted, so a later difference
        // is the write's doing and not the create's.
        assert_eq!(
            emulator.reported_tags(&arn, case.case).await,
            sorted(&case.initial),
            "case {:?}: the reader must report the tags create persisted",
            case.case
        );

        for (write_index, write) in case.writes.iter().enumerate() {
            let (status, response) = emulator.tag_resource(&arn, write).await;
            assert_eq!(
                status, 200,
                "case {:?} write {write_index}: TagResource must answer 200, got {status} {response:?}",
                case.case
            );
            assert!(
                response.as_object().is_some_and(Map::is_empty) || response == Value::Null,
                "case {:?} write {write_index}: TagResource answers no members, got {response:?}",
                case.case
            );
        }

        let reported = emulator.reported_tags(&arn, case.case).await;
        assert_eq!(
            reported,
            sorted(&case.expected),
            "case {:?}: expected the merged tag set {:?}, got {reported:?}",
            case.case,
            sorted(&case.expected)
        );

        // A read changes nothing a later reader sees.
        let again = emulator.reported_tags(&arn, case.case).await;
        assert_eq!(
            again, reported,
            "case {:?}: a read must not change what the next read reports, got {again:?}",
            case.case
        );

        // Tagging touches the tag set only: the configuration itself still
        // reports what create and update left on it.
        let uri = format!("/notification-configurations/{}", encode_path_label(&arn));
        let (status, configuration) = emulator.send("GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: the configuration must survive a tag write, got {status} {configuration:?}",
            case.case
        );
        assert_eq!(
            configuration["name"],
            json!(format!("tag-case-{index}")),
            "case {:?}: a tag write must not change the configuration's name, got {configuration:?}",
            case.case
        );
    }
}

/// RULE: a tag set belongs to one resource, so a write keyed by one arn is
/// invisible through every other arn.
#[tokio::test]
async fn a_tag_write_reaches_only_the_arn_it_names() {
    let emulator = Emulator::new();

    let arns = [
        emulator
            .create_configuration_with_tags("isolation-a", &pairs(&[("shared", "start")]))
            .await,
        emulator
            .create_configuration_with_tags("isolation-b", &pairs(&[("shared", "start")]))
            .await,
        emulator
            .create_configuration_with_tags("isolation-c", &[])
            .await,
    ];

    for (index, arn) in arns.iter().enumerate() {
        let case = format!("write to resource {index}");
        let write = pairs(&[("shared", "written"), ("only", "here")]);
        let (status, response) = emulator.tag_resource(arn, &write).await;
        assert_eq!(
            status, 200,
            "case {case:?}: TagResource must answer 200, got {status} {response:?}"
        );

        for (other_index, other) in arns.iter().enumerate() {
            let reported = emulator.reported_tags(other, &case).await;
            if other_index <= index {
                assert_eq!(
                    reported,
                    sorted(&write),
                    "case {case:?}: resource {other_index} was written and must report the merged set, got {reported:?}"
                );
            } else {
                let untouched = if other_index == 2 {
                    Vec::new()
                } else {
                    pairs(&[("shared", "start")])
                };
                assert_eq!(
                    reported, untouched,
                    "case {case:?}: resource {other_index} was not written and must keep its own tags, got {reported:?}"
                );
            }
        }
    }
}

/// RULE: an arn no configuration is stored under has no tag set, so both tag
/// operations answer `ResourceNotFoundException` (404) for it — whatever the
/// arn's own shape, and including an arn whose configuration was deleted.
#[tokio::test]
async fn the_tag_operations_report_an_unknown_arn_as_not_found() {
    let emulator = Emulator::new();

    // Well-formed arns (they satisfy the modelled
    // `^arn:[a-z-]{3,10}:notifications::[0-9]{12}:configuration/[a-z0-9]{27}$`
    // pattern, so they reach the handler) that were never created. Rows differ
    // in partition, account id and the alphabet of the id segment.
    let mut unknown = vec![
        "arn:aws:notifications::000000000000:configuration/aaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        "arn:aws-cn:notifications::123456789012:configuration/z0y9x8w7v6u5t4s3r2q1p0onmlk"
            .to_string(),
        "arn:aws-us-gov:notifications::999999999999:configuration/999999999999999999999999999"
            .to_string(),
    ];

    // A live configuration exists beside them, so a 404 cannot be "the store is
    // empty" — and one more that is created, tagged and then deleted, so a
    // deleted arn is covered by the same rule.
    let live = emulator
        .create_configuration_with_tags("live-config", &pairs(&[("env", "dev")]))
        .await;
    let deleted = emulator
        .create_configuration_with_tags("doomed-config", &pairs(&[("env", "dev")]))
        .await;
    let (status, response) = emulator.delete_configuration(&deleted).await;
    assert_eq!(
        status, 200,
        "the delete that sets up the deleted-arn case must answer 200, got {status} {response:?}"
    );
    unknown.push(deleted);

    for arn in &unknown {
        for (operation, status, response) in [
            ("TagResource", emulator.tag_resource(arn, &pairs(&[("env", "prod")])).await),
            ("ListTagsForResource", emulator.list_tags_for_resource(arn).await),
        ]
        .map(|(operation, (status, response))| (operation, status, response))
        {
            assert_eq!(
                status, 404,
                "case {operation} {arn:?}: an unknown arn is a 404, got {status} {response:?}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {operation} {arn:?}: the 404 is a ResourceNotFoundException, got {response:?}"
            );
        }
    }

    // The refused calls left the live resource exactly as it was.
    assert_eq!(
        emulator.reported_tags(&live, "live after refusals").await,
        pairs(&[("env", "dev")]),
        "the live configuration's tags must survive the refused calls"
    );
}

/// RULE: a resource's tag set is bounded by `TagMap`'s modelled `@length`
/// maximum, so a write that would leave more tags than that behind is refused
/// with a 400 `ValidationException` and writes NOTHING, while a write that only
/// overwrites keys already stored still succeeds at the bound.
#[tokio::test]
async fn a_write_past_the_modelled_tag_maximum_is_refused_whole() {
    // `TagMap`'s `@length` is 0..200, which is the shape both `TagResource`'s
    // input and `ListTagsForResource`'s output are typed as.
    const MAX_TAGS: usize = 200;

    let emulator = Emulator::new();
    let arn = emulator
        .create_configuration_with_tags("tag-bound", &[])
        .await;

    // Exactly the bound is accepted: `key-000` .. `key-199`.
    let at_bound: Vec<(String, String)> = (0..MAX_TAGS)
        .map(|index| (format!("key-{index:03}"), format!("value-{index:03}")))
        .collect();
    let (status, response) = emulator.tag_resource(&arn, &at_bound).await;
    assert_eq!(
        status, 200,
        "case at the bound ({MAX_TAGS} tags): expected 200, got {status} {response:?}"
    );
    assert_eq!(
        emulator.reported_tags(&arn, "at the bound").await.len(),
        MAX_TAGS,
        "case at the bound: the resource must report every written tag"
    );

    // One past it is refused, and the refusal writes nothing: not the new key,
    // and not the overwrite that shared the request with it.
    let past_bound = pairs(&[("key-000", "rewritten"), ("one-too-many", "x")]);
    let (status, response) = emulator.tag_resource(&arn, &past_bound).await;
    assert_eq!(
        status, 400,
        "case one past the bound: expected 400, got {status} {response:?}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "case one past the bound: expected a ValidationException, got {response:?}"
    );
    let after_refusal = emulator.reported_tags(&arn, "one past the bound").await;
    assert_eq!(
        after_refusal,
        sorted(&at_bound),
        "case one past the bound: a refused write must change nothing, got {} tags",
        after_refusal.len()
    );

    // At the bound, a write that only overwrites stored keys still succeeds:
    // the resulting set is no bigger than the one already held.
    let overwrite = pairs(&[("key-000", "rewritten"), ("key-199", "rewritten")]);
    let (status, response) = emulator.tag_resource(&arn, &overwrite).await;
    assert_eq!(
        status, 200,
        "case overwrite at the bound: expected 200, got {status} {response:?}"
    );
    let reported = emulator.reported_tags(&arn, "overwrite at the bound").await;
    assert_eq!(
        reported.len(),
        MAX_TAGS,
        "case overwrite at the bound: an overwrite must not grow the tag set, got {} tags",
        reported.len()
    );
    for (key, value) in &overwrite {
        assert!(
            reported.contains(&(key.clone(), value.clone())),
            "case overwrite at the bound: expected {key:?} to report {value:?}, got {reported:?}"
        );
    }
}
