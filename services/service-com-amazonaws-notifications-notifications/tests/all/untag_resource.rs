// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UntagResource` and the reader that reports its
//! result, `ListTagsForResource`, driven over HTTP through the same
//! multi-service router the emulator boots in production.
//!
//! Every case asserts the RULE the operation follows, not one recorded request:
//! an untag removes EXACTLY the keys it names from the tag set persisted on the
//! `NotificationConfiguration` the arn names — every other stored key keeps its
//! value, a key the resource does not carry is already in the state asked for
//! (so the call succeeds and removes nothing in its place), and an arn no
//! configuration is stored under is a 404. Table rows vary along what that rule
//! generalises over: how many keys are stored, how many the request names,
//! whether each named key is stored or not, the key alphabet and size (the
//! published `TagKey` maximum), the order the keys arrive in, and whether a key
//! is repeated within one request. The bounds the rows draw from are the
//! model's own (`TagKey`'s `^(?!aws:).{1,128}$` pattern, `TagValue`'s `@length`
//! 0..256, `TagKeys`' and `TagMap`'s `@length` 0..200).
//!
//! The merge half of tagging lives in `tests/all/resource_tags.rs`; this suite
//! is the removal half and the bound they share.

use serde_json::{json, Map, Value};

// The HTTP harness this suite drives the service through, shared with every
// other suite in this crate.
use crate::harness::{encode_path_label, error_code, Emulator};

// The untagging-specific helpers this suite adds to the shared harness. An
// inherent `impl` on a type declared elsewhere in the same crate is what keeps
// them beside the tests that use them; the names differ from the other suites'
// so one type carries one method per helper.
impl Emulator {
    /// Create a configuration carrying `tags` and return its arn, failing the
    /// test on anything other than the modelled 201.
    async fn create_tagged_record(&self, name: &str, tags: &[(String, String)]) -> String {
        let body = json!({
            "name": name,
            "description": "a configuration under test",
            "tags": tag_map(tags),
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
    async fn write_tags(&self, arn: &str, tags: &[(String, String)]) -> (u16, Value) {
        let uri = format!("/tags/{}", encode_path_label(arn));
        self.send("POST", &uri, Some(json!({ "tags": tag_map(tags) })))
            .await
    }

    /// `UntagResource`: `DELETE /tags/{arn}?tagKeys=…`. `tagKeys` is an
    /// `@httpQuery` list, so each key is one repetition of the parameter.
    async fn remove_tags(&self, arn: &str, keys: &[String]) -> (u16, Value) {
        let query: Vec<String> = keys
            .iter()
            // `encode_path_label` percent-encodes everything outside the
            // unreserved set, which is what a query value needs too.
            .map(|key| format!("tagKeys={}", encode_path_label(key)))
            .collect();
        let uri = format!("/tags/{}?{}", encode_path_label(arn), query.join("&"));
        self.send("DELETE", &uri, None).await
    }

    /// `ListTagsForResource`: `GET /tags/{arn}`, as status and body.
    async fn read_tags(&self, arn: &str) -> (u16, Value) {
        let uri = format!("/tags/{}", encode_path_label(arn));
        self.send("GET", &uri, None).await
    }

    /// The tag set `ListTagsForResource` reports for `arn`, sorted so a
    /// comparison does not depend on map iteration order. Fails the test unless
    /// the read answered the modelled 200.
    async fn tag_set(&self, arn: &str, case: &str) -> Vec<(String, String)> {
        let (status, response) = self.read_tags(arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource must answer 200, got {status} {response:?}"
        );
        let mut reported: Vec<(String, String)> = response["tags"]
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
        reported.sort();
        reported
    }

    /// `DeleteNotificationConfiguration`, so this suite can assert untagging a
    /// record that is gone is a 404.
    async fn delete_tagged_record(&self, arn: &str) -> (u16, Value) {
        let uri = format!("/notification-configurations/{}", encode_path_label(arn));
        self.send("DELETE", &uri, None).await
    }
}

/// A tag list as the JSON object the wire carries.
fn tag_map(tags: &[(String, String)]) -> Value {
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

/// A key list from borrowed literals.
fn keys(entries: &[&str]) -> Vec<String> {
    entries.iter().map(|key| (*key).to_string()).collect()
}

/// The same list sorted, which is the order [`Emulator::tag_set`] answers in.
fn sorted(entries: &[(String, String)]) -> Vec<(String, String)> {
    let mut owned = entries.to_vec();
    owned.sort();
    owned
}

/// The longest key the modelled `TagKey` pattern `^(?!aws:).{1,128}$` admits.
fn longest_key() -> String {
    "k".repeat(128)
}

/// One row of the removal table: the tags the configuration is created with,
/// the keys one `UntagResource` call names, and the tag set the reader must
/// report afterwards.
struct RemovalCase {
    case: &'static str,
    initial: Vec<(String, String)>,
    removed: Vec<String>,
    expected: Vec<(String, String)>,
}

/// The removal table. Rows differ in how many tags are stored, how many keys
/// the request names, whether each named key is stored, the key alphabet and
/// size (including `TagKey`'s published maximum), the order keys arrive in, and
/// whether a key is repeated within one request.
fn removal_cases() -> Vec<RemovalCase> {
    let long_key = longest_key();
    let many: Vec<(String, String)> = (0..40)
        .map(|index| (format!("key-{index:03}"), format!("value-{index:03}")))
        .collect();

    vec![
        RemovalCase {
            case: "the only stored key is removed, leaving an empty tag set",
            initial: pairs(&[("env", "prod")]),
            removed: keys(&["env"]),
            expected: Vec::new(),
        },
        RemovalCase {
            case: "one of several keys is removed, the rest keep their values",
            initial: pairs(&[("env", "prod"), ("team", "platform"), ("tier", "1")]),
            removed: keys(&["team"]),
            expected: pairs(&[("env", "prod"), ("tier", "1")]),
        },
        RemovalCase {
            case: "several keys are removed in one request",
            initial: pairs(&[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")]),
            removed: keys(&["b", "d"]),
            expected: pairs(&[("a", "1"), ("c", "3")]),
        },
        RemovalCase {
            case: "every stored key is removed at once",
            initial: pairs(&[("a", "1"), ("b", "2"), ("c", "3")]),
            removed: keys(&["a", "b", "c"]),
            expected: Vec::new(),
        },
        RemovalCase {
            case: "a key the resource does not carry removes nothing",
            initial: pairs(&[("env", "prod")]),
            removed: keys(&["never-set"]),
            expected: pairs(&[("env", "prod")]),
        },
        RemovalCase {
            case: "a mix of stored and unstored keys removes only the stored ones",
            initial: pairs(&[("env", "prod"), ("tier", "1")]),
            removed: keys(&["env", "never-set", "also-never-set"]),
            expected: pairs(&[("tier", "1")]),
        },
        RemovalCase {
            case: "an untagged resource stays untagged",
            initial: Vec::new(),
            removed: keys(&["env", "tier"]),
            expected: Vec::new(),
        },
        RemovalCase {
            // `TagValue`'s `@length` minimum is 0, so a key holding the empty
            // value is a stored key like any other.
            case: "a key holding the empty value is removed like any other",
            initial: pairs(&[("empty", ""), ("kept", "")]),
            removed: keys(&["empty"]),
            expected: pairs(&[("kept", "")]),
        },
        RemovalCase {
            // `TagKey`'s pattern constrains only the `aws:` prefix and the
            // length, so punctuation and non-ASCII are ordinary keys.
            case: "punctuation and non-ASCII keys are removed by their exact text",
            initial: pairs(&[
                ("team:platform/eu-west", "on call"),
                ("étiquette", "valeur"),
                ("k", "a"),
            ]),
            removed: keys(&["team:platform/eu-west", "étiquette"]),
            expected: pairs(&[("k", "a")]),
        },
        RemovalCase {
            // `TagKey` admits 128 characters, so a key at that bound is
            // addressable too.
            case: "the longest modelled key is removable",
            initial: vec![(long_key.clone(), "at the bound".to_string())],
            removed: vec![long_key.clone()],
            expected: Vec::new(),
        },
        RemovalCase {
            case: "key order within one request does not matter",
            initial: pairs(&[("aa", "1"), ("mm", "2"), ("zz", "3")]),
            removed: keys(&["zz", "aa"]),
            expected: pairs(&[("mm", "2")]),
        },
        RemovalCase {
            case: "a key repeated within one request is removed once",
            initial: pairs(&[("env", "prod"), ("tier", "1")]),
            removed: keys(&["env", "env", "env"]),
            expected: pairs(&[("tier", "1")]),
        },
        RemovalCase {
            case: "a large stored set loses only the named half",
            initial: many.clone(),
            removed: many
                .iter()
                .enumerate()
                .filter(|(index, _)| index % 2 == 0)
                .map(|(_, (key, _))| key.clone())
                .collect(),
            expected: many
                .iter()
                .enumerate()
                .filter(|(index, _)| index % 2 == 1)
                .map(|(_, pair)| pair.clone())
                .collect(),
        },
    ]
}

/// RULE: an untag removes exactly the keys it names from the resource's stored
/// tag set, leaves every other key's value alone, and is idempotent — so the
/// reader reports the same set however many times the call is repeated.
#[tokio::test]
async fn untag_resource_removes_exactly_the_keys_it_names() {
    for (index, case) in removal_cases().into_iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("untag-case-{index}");
        let arn = emulator.create_tagged_record(&name, &case.initial).await;

        // The reader starts from what create persisted, so a later difference
        // is the untag's doing and not the create's.
        assert_eq!(
            emulator.tag_set(&arn, case.case).await,
            sorted(&case.initial),
            "case {:?}: the reader must report the tags create persisted",
            case.case
        );

        let (status, response) = emulator.remove_tags(&arn, &case.removed).await;
        assert_eq!(
            status, 200,
            "case {:?}: UntagResource must answer 200, got {status} {response:?}",
            case.case
        );
        assert!(
            response.as_object().is_some_and(Map::is_empty) || response == Value::Null,
            "case {:?}: UntagResource answers no members, got {response:?}",
            case.case
        );

        let reported = emulator.tag_set(&arn, case.case).await;
        assert_eq!(
            reported,
            sorted(&case.expected),
            "case {:?}: expected the remaining tag set {:?}, got {reported:?}",
            case.case,
            sorted(&case.expected)
        );

        // `@idempotent`: the named keys are already gone, so repeating the call
        // succeeds and removes nothing further.
        let (status, response) = emulator.remove_tags(&arn, &case.removed).await;
        assert_eq!(
            status, 200,
            "case {:?}: repeating UntagResource must answer 200, got {status} {response:?}",
            case.case
        );
        let again = emulator.tag_set(&arn, case.case).await;
        assert_eq!(
            again, reported,
            "case {:?}: repeating UntagResource must not change the tag set, got {again:?}",
            case.case
        );

        // A read changes nothing a later reader sees.
        let third = emulator.tag_set(&arn, case.case).await;
        assert_eq!(
            third, reported,
            "case {:?}: a read must not change what the next read reports, got {third:?}",
            case.case
        );

        // Untagging touches the tag set only: the configuration itself is
        // still there, reporting what create left on it.
        let uri = format!("/notification-configurations/{}", encode_path_label(&arn));
        let (status, configuration) = emulator.send("GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: the configuration must survive an untag, got {status} {configuration:?}",
            case.case
        );
        assert_eq!(
            configuration["name"],
            json!(name),
            "case {:?}: an untag must not change the configuration's name, got {configuration:?}",
            case.case
        );
    }
}

/// RULE: a removal is keyed by one arn, so it reaches no other resource's tag
/// set — including a key of the same name stored on another resource.
#[tokio::test]
async fn an_untag_reaches_only_the_arn_it_names() {
    let emulator = Emulator::new();

    let arns = [
        emulator
            .create_tagged_record("untag-isolation-a", &pairs(&[("shared", "a"), ("own", "a")]))
            .await,
        emulator
            .create_tagged_record("untag-isolation-b", &pairs(&[("shared", "b"), ("own", "b")]))
            .await,
        emulator
            .create_tagged_record("untag-isolation-c", &pairs(&[("shared", "c"), ("own", "c")]))
            .await,
    ];

    for (index, arn) in arns.iter().enumerate() {
        let case = format!("untag resource {index}");
        let (status, response) = emulator.remove_tags(arn, &keys(&["shared"])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UntagResource must answer 200, got {status} {response:?}"
        );

        for (other_index, other) in arns.iter().enumerate() {
            let reported = emulator.tag_set(other, &case).await;
            let letter = ["a", "b", "c"][other_index];
            let expected = if other_index <= index {
                pairs(&[("own", letter)])
            } else {
                sorted(&pairs(&[("own", letter), ("shared", letter)]))
            };
            assert_eq!(
                reported, expected,
                "case {case:?}: resource {other_index} must report {expected:?}, got {reported:?}"
            );
        }
    }
}

/// RULE: an arn no configuration is stored under has no tag set to remove from,
/// so `UntagResource` answers `ResourceNotFoundException` (404) for it —
/// whatever the arn's own shape, and including an arn whose configuration was
/// deleted.
#[tokio::test]
async fn untag_resource_reports_an_unknown_arn_as_not_found() {
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
        .create_tagged_record("untag-live-config", &pairs(&[("env", "dev")]))
        .await;
    let deleted = emulator
        .create_tagged_record("untag-doomed-config", &pairs(&[("env", "dev")]))
        .await;
    let (status, response) = emulator.delete_tagged_record(&deleted).await;
    assert_eq!(
        status, 200,
        "the delete that sets up the deleted-arn case must answer 200, got {status} {response:?}"
    );
    unknown.push(deleted);

    for arn in &unknown {
        let (status, response) = emulator.remove_tags(arn, &keys(&["env"])).await;
        assert_eq!(
            status, 404,
            "case {arn:?}: an unknown arn is a 404, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {arn:?}: the 404 is a ResourceNotFoundException, got {response:?}"
        );
    }

    // The refused calls left the live resource exactly as it was.
    assert_eq!(
        emulator.tag_set(&live, "live after refusals").await,
        pairs(&[("env", "dev")]),
        "the live configuration's tags must survive the refused untag calls"
    );
}

/// RULE: the tag set a resource holds is bounded by `TagMap`'s modelled
/// `@length` maximum, and a removal frees room under that bound — so a write
/// refused at the bound succeeds once as many keys as it adds have been
/// removed.
#[tokio::test]
async fn a_removal_frees_room_under_the_modelled_tag_maximum() {
    // `TagMap`'s `@length` is 0..200, the shape both `TagResource`'s input and
    // `ListTagsForResource`'s output are typed as.
    const MAX_TAGS: usize = 200;

    let emulator = Emulator::new();
    let arn = emulator.create_tagged_record("untag-bound", &[]).await;

    let at_bound: Vec<(String, String)> = (0..MAX_TAGS)
        .map(|index| (format!("key-{index:03}"), format!("value-{index:03}")))
        .collect();
    let (status, response) = emulator.write_tags(&arn, &at_bound).await;
    assert_eq!(
        status, 200,
        "case at the bound ({MAX_TAGS} tags): expected 200, got {status} {response:?}"
    );

    // At the bound a new key does not fit.
    let (status, response) = emulator.write_tags(&arn, &pairs(&[("late", "x")])).await;
    assert_eq!(
        status, 400,
        "case at the bound: a new key must be refused, got {status} {response:?}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "case at the bound: the refusal is a ValidationException, got {response:?}"
    );

    // Removing two keys frees room for two new ones.
    let (status, response) = emulator
        .remove_tags(&arn, &keys(&["key-000", "key-001"]))
        .await;
    assert_eq!(
        status, 200,
        "case freeing room: UntagResource must answer 200, got {status} {response:?}"
    );
    let freed = emulator.tag_set(&arn, "freeing room").await;
    assert_eq!(
        freed.len(),
        MAX_TAGS - 2,
        "case freeing room: two removals must leave two fewer tags, got {} tags",
        freed.len()
    );

    let (status, response) = emulator
        .write_tags(&arn, &pairs(&[("late", "x"), ("later", "y")]))
        .await;
    assert_eq!(
        status, 200,
        "case after freeing room: the write must now fit, got {status} {response:?}"
    );
    let reported = emulator.tag_set(&arn, "after freeing room").await;
    assert_eq!(
        reported.len(),
        MAX_TAGS,
        "case after freeing room: the resource must be back at the bound, got {} tags",
        reported.len()
    );
    for (key, value) in pairs(&[("late", "x"), ("later", "y")]) {
        assert!(
            reported.contains(&(key.clone(), value.clone())),
            "case after freeing room: expected {key:?} to report {value:?}, got {reported:?}"
        );
    }
    for removed in keys(&["key-000", "key-001"]) {
        assert!(
            !reported.iter().any(|(key, _)| *key == removed),
            "case after freeing room: {removed:?} must stay removed, got {reported:?}"
        );
    }
}
