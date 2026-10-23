// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the tagging slice — `TagResource`, `UntagResource` and
//! `ListTagsForResource` — over every taggable resource this service holds.
//!
//! Every call goes through the real `MultiServiceRouter`, so the restJson1 routes, the
//! `@httpLabel` ARN binding, the `tagKeys` query binding and the error envelopes are
//! exercised as a client hits them. Nothing reaches into `crate::state`: a tag is only
//! ever read back through an operation, both through `ListTagsForResource` and through
//! the resource's own reader.

use serde_json::{json, Value};

use crate::global_network_lifecycle::{error_code, expected_arn, Emulator};

/// The taggable resource kinds this service's state holds; each is also the ARN resource
/// type of that kind (`arn:aws:networkmanager::<account>:<kind>/<id>`).
const GLOBAL_NETWORK: &str = "global-network";
/// See [`GLOBAL_NETWORK`].
const SITE: &str = "site";
/// See [`GLOBAL_NETWORK`].
const LINK: &str = "link";

/// The documented maximum length of a tag key, in characters: "Constraints: Maximum
/// length of 128 characters" (`smithy_query query="doc" shape="Tag" member="Key"`).
const MAX_KEY_CHARS: usize = 128;
/// The documented maximum length of a tag value: "Constraints: Maximum length of 256
/// characters" (`smithy_query query="doc" shape="Tag" member="Value"`).
const MAX_VALUE_CHARS: usize = 256;

/// AWS's published tagging limit — 50 tags per resource.
const MAX_TAGS: usize = 50;

/// Percent-encode `value` so it survives one URI segment or one query value.
///
/// `/tags/{ResourceArn}` binds a NON-greedy `@httpLabel`
/// (`smithy_query query="operation" operation="TagResource"`), so a client sends the
/// ARN's `:` and `/` encoded — which is what botocore does.
pub(crate) fn escape(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The `/tags/{ResourceArn}` URI of `arn`.
pub(crate) fn tags_uri(arn: &str) -> String {
    format!("/tags/{}", escape(arn))
}

/// The `UntagResource` URI: the keys travel as repeated `tagKeys` query parameters
/// (`TagKeys` is bound `@httpQuery("tagKeys")`).
pub(crate) fn untag_uri(arn: &str, keys: &[&str]) -> String {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", escape(key)))
        .collect::<Vec<String>>()
        .join("&");
    format!("{}?{query}", tags_uri(arn))
}

/// One modelled `Tag`.
fn tag(key: &str, value: &str) -> Value {
    json!({ "Key": key, "Value": value })
}

/// The string member `member` of `value`, or a panic naming what was there instead.
fn string_member(value: &Value, member: &str) -> String {
    value[member]
        .as_str()
        .unwrap_or_else(|| panic!("expected a string {member} in {value}"))
        .to_string()
}

/// `body` with `Tags` added when the row carries them.
fn body_with(mut body: Value, tags: &Value) -> Value {
    if !tags.is_null() {
        if let Some(object) = body.as_object_mut() {
            object.insert("Tags".to_string(), tags.clone());
        }
    }
    body
}

/// A created resource, with everything a test needs to read its tags back two ways.
struct Taggable {
    kind: &'static str,
    arn: String,
    id: String,
    /// The resource's own reader (`DescribeGlobalNetworks` / `GetSites` / `GetLinks`).
    read_uri: String,
    /// The list member that reader answers with, and the id member inside it.
    list: &'static str,
    id_member: &'static str,
}

/// Create a resource of `kind` carrying `tags` (`Value::Null` for "no `Tags` member").
async fn create_taggable(emulator: &Emulator, kind: &'static str, tags: &Value) -> Taggable {
    match kind {
        GLOBAL_NETWORK => {
            let (status, created) = emulator
                .post(
                    "/global-networks",
                    body_with(json!({ "Description": "tagged" }), tags),
                )
                .await;
            assert_eq!(
                status, 200,
                "CreateGlobalNetwork({tags}): expected 200, got {status} with {created}"
            );
            Taggable {
                kind,
                arn: string_member(&created["GlobalNetwork"], "GlobalNetworkArn"),
                id: string_member(&created["GlobalNetwork"], "GlobalNetworkId"),
                read_uri: "/global-networks".to_string(),
                list: "GlobalNetworks",
                id_member: "GlobalNetworkId",
            }
        }
        SITE => {
            let network = emulator.create_global_network("tagging").await;
            let (status, created) = emulator
                .post(
                    &format!("/global-networks/{network}/sites"),
                    body_with(json!({ "Description": "tagged" }), tags),
                )
                .await;
            assert_eq!(
                status, 200,
                "CreateSite({tags}): expected 200, got {status} with {created}"
            );
            Taggable {
                kind,
                arn: string_member(&created["Site"], "SiteArn"),
                id: string_member(&created["Site"], "SiteId"),
                read_uri: format!("/global-networks/{network}/sites"),
                list: "Sites",
                id_member: "SiteId",
            }
        }
        LINK => {
            let network = emulator.create_global_network("tagging").await;
            let site = emulator.create_site(&network, "tagging").await;
            let (status, created) = emulator
                .post(
                    &format!("/global-networks/{network}/links"),
                    body_with(
                        json!({
                            "SiteId": site,
                            "Bandwidth": { "UploadSpeed": 10, "DownloadSpeed": 20 },
                        }),
                        tags,
                    ),
                )
                .await;
            assert_eq!(
                status, 200,
                "CreateLink({tags}): expected 200, got {status} with {created}"
            );
            Taggable {
                kind,
                arn: string_member(&created["Link"], "LinkArn"),
                id: string_member(&created["Link"], "LinkId"),
                read_uri: format!("/global-networks/{network}/links"),
                list: "Links",
                id_member: "LinkId",
            }
        }
        other => panic!("no taggable resource kind {other:?}"),
    }
}

/// The `TagList` `ListTagsForResource` answers for `arn`, with the status.
async fn list_tags(emulator: &Emulator, arn: &str) -> (u16, Value) {
    let (status, body) = emulator.get(&tags_uri(arn)).await;
    if status == 200 {
        (status, body["TagList"].clone())
    } else {
        (status, body)
    }
}

/// The `Tags` the resource's OWN reader reports for it — the same stored list.
async fn tags_through_resource_reader(emulator: &Emulator, resource: &Taggable) -> Value {
    let (status, body) = emulator.get(&resource.read_uri).await;
    assert_eq!(
        status, 200,
        "reading {} back: expected 200, got {status} with {body}",
        resource.kind
    );
    let items = body[resource.list]
        .as_array()
        .unwrap_or_else(|| panic!("expected a {} array in {body}", resource.list));
    let item = items
        .iter()
        .find(|item| item[resource.id_member] == json!(resource.id))
        .unwrap_or_else(|| panic!("expected {} in {body}", resource.id));
    item["Tags"].clone()
}

/// One row of the tag/untag/list round-trip table.
struct TagCase {
    name: &'static str,
    kind: &'static str,
    /// The `Tags` the create carries, `Value::Null` for none.
    initial: Value,
    /// The `Tags` the `TagResource` call applies.
    applied: Value,
    /// The tag list both readers must answer once `applied` is merged in.
    after_tag: Value,
    /// The keys the `UntagResource` call removes.
    removed: Vec<&'static str>,
    /// The tag list both readers must answer once those keys are gone.
    after_untag: Value,
}

/// The rows of the round-trip table. They differ along the resource kind (every taggable
/// kind this service holds), along how many tags the create carried, along whether the
/// applied keys are new or already held (the overwrite case), and along the character set
/// and size of the keys and values — including the documented maximum key and value
/// lengths. The removal rows also name a key the resource does not carry, which is
/// ignored rather than refused.
fn tag_cases() -> Vec<TagCase> {
    vec![
        TagCase {
            name: "global network created untagged, two tags added",
            kind: GLOBAL_NETWORK,
            initial: Value::Null,
            applied: json!([tag("env", "test"), tag("tier", "primary")]),
            after_tag: json!([tag("env", "test"), tag("tier", "primary")]),
            removed: vec!["env"],
            after_untag: json!([tag("tier", "primary")]),
        },
        TagCase {
            name: "site created tagged, one key overwritten and one added",
            kind: SITE,
            initial: json!([tag("env", "dev"), tag("owner", "ops")]),
            applied: json!([tag("env", "prod"), tag("team", "net")]),
            // A key already held keeps its position and takes the new value; a new key is
            // appended. Tag keys are unique per resource, so `env` must not appear twice.
            after_tag: json!([tag("env", "prod"), tag("owner", "ops"), tag("team", "net")]),
            removed: vec!["owner", "never-set"],
            after_untag: json!([tag("env", "prod"), tag("team", "net")]),
        },
        TagCase {
            name: "link, a non-ASCII key and an empty value",
            kind: LINK,
            initial: Value::Null,
            applied: json!([tag("ключ", "значение"), tag("blank", "")]),
            after_tag: json!([tag("ключ", "значение"), tag("blank", "")]),
            removed: vec!["ключ"],
            after_untag: json!([tag("blank", "")]),
        },
        TagCase {
            name: "global network, a key and value at the documented maximum lengths",
            kind: GLOBAL_NETWORK,
            initial: json!([]),
            applied: json!([tag(&"k".repeat(MAX_KEY_CHARS), &"v".repeat(MAX_VALUE_CHARS))]),
            after_tag: json!([tag(&"k".repeat(MAX_KEY_CHARS), &"v".repeat(MAX_VALUE_CHARS))]),
            removed: vec![],
            after_untag: json!([tag(&"k".repeat(MAX_KEY_CHARS), &"v".repeat(MAX_VALUE_CHARS))]),
        },
        TagCase {
            name: "link, re-applying the key it was created with",
            kind: LINK,
            initial: json!([tag("env", "test")]),
            applied: json!([tag("env", "test")]),
            after_tag: json!([tag("env", "test")]),
            removed: vec!["env"],
            after_untag: json!([]),
        },
    ]
}

#[tokio::test]
async fn tag_untag_and_list_round_trip_through_stored_state_for_every_taggable_resource() {
    for case in tag_cases() {
        let name = case.name;
        let emulator = Emulator::new();
        let resource = create_taggable(&emulator, case.kind, &case.initial).await;

        // The tags the create carried are what the resource starts with.
        let expected_initial = if case.initial.is_null() {
            json!([])
        } else {
            case.initial.clone()
        };
        let (status, listed) = list_tags(&emulator, &resource.arn).await;
        assert_eq!(
            status, 200,
            "case {name}: ListTagsForResource expected 200, got {status} with {listed}"
        );
        assert_eq!(
            listed, expected_initial,
            "case {name}: ListTagsForResource must answer the tags the create stored, got {listed}"
        );

        // TAG: the applied set is merged into the stored one.
        let (status, tagged) = emulator
            .post(&tags_uri(&resource.arn), json!({ "Tags": case.applied }))
            .await;
        assert_eq!(
            status, 200,
            "case {name}: TagResource expected 200, got {status} with {tagged}"
        );

        let (status, listed) = list_tags(&emulator, &resource.arn).await;
        assert_eq!(
            status, 200,
            "case {name}: ListTagsForResource expected 200, got {status} with {listed}"
        );
        assert_eq!(
            listed, case.after_tag,
            "case {name}: expected {} after TagResource, got {listed}",
            case.after_tag
        );
        let through_reader = tags_through_resource_reader(&emulator, &resource).await;
        assert_eq!(
            through_reader, case.after_tag,
            "case {name}: the resource's own reader must answer the same stored tags, got {through_reader}"
        );

        if case.removed.is_empty() {
            continue;
        }

        // UNTAG: only the named keys go, and a key the resource never had is ignored.
        let (status, untagged) = emulator
            .delete(&untag_uri(&resource.arn, &case.removed))
            .await;
        assert_eq!(
            status, 200,
            "case {name}: UntagResource expected 200, got {status} with {untagged}"
        );
        let (_, listed) = list_tags(&emulator, &resource.arn).await;
        assert_eq!(
            listed, case.after_untag,
            "case {name}: expected {} after UntagResource, got {listed}",
            case.after_untag
        );
        let through_reader = tags_through_resource_reader(&emulator, &resource).await;
        assert_eq!(
            through_reader, case.after_untag,
            "case {name}: the resource's own reader must answer the same stored tags, got {through_reader}"
        );

        // Removing the same keys again answers the same: removal is idempotent.
        let (status, untagged) = emulator
            .delete(&untag_uri(&resource.arn, &case.removed))
            .await;
        assert_eq!(
            status, 200,
            "case {name}: a repeated UntagResource expected 200, got {status} with {untagged}"
        );
        let (_, listed) = list_tags(&emulator, &resource.arn).await;
        assert_eq!(
            listed, case.after_untag,
            "case {name}: a repeated UntagResource must change nothing, got {listed}"
        );
    }
}

#[tokio::test]
async fn a_tag_key_reserved_by_aws_is_refused_wherever_a_key_arrives() {
    // "Do not use the aws: prefix in your tag names or values because it is reserved for
    // AWS use. You can't edit or delete tag names or values with this prefix" (AWS
    // tagging documentation). The rows carry the bare prefix, an AWS-generated key, the
    // prefix in other cases, and keys that only look like it — which are ordinary keys.
    let cases: Vec<(&str, &str, bool)> = vec![
        ("the bare reserved prefix", "aws:", true),
        ("an AWS-generated key", "aws:cloudformation:stack-name", true),
        ("the prefix in upper case", "AWS:Region", true),
        ("the prefix in mixed case", "Aws:service", true),
        ("a key that merely begins with aws", "awsome", false),
        ("the prefix away from the start", "my-aws:key", false),
        ("aws without the colon", "aws", false),
    ];

    for (case, key, refused) in cases {
        let emulator = Emulator::new();
        let resource = create_taggable(&emulator, GLOBAL_NETWORK, &Value::Null).await;
        let want_status = if refused { 400 } else { 200 };

        let (status, body) = emulator
            .post(
                &tags_uri(&resource.arn),
                json!({ "Tags": [tag(key, "whatever")] }),
            )
            .await;
        assert_eq!(
            status, want_status,
            "case {case}: TagResource({key:?}) expected {want_status}, got {status} with {body}"
        );
        let (_, listed) = list_tags(&emulator, &resource.arn).await;
        let expected = if refused {
            // A refused request stores nothing.
            json!([])
        } else {
            json!([tag(key, "whatever")])
        };
        assert_eq!(
            listed, expected,
            "case {case}: expected {expected} stored, got {listed}"
        );
        if refused {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case}: wrong error code in {body}"
            );
        }

        // The same key is refused on the way OUT: a reserved tag cannot be deleted either.
        let (status, body) = emulator.delete(&untag_uri(&resource.arn, &[key])).await;
        assert_eq!(
            status, want_status,
            "case {case}: UntagResource({key:?}) expected {want_status}, got {status} with {body}"
        );
        if refused {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case}: wrong error code in {body}"
            );
        }

        // And on a create, which carries the same `Tags` member.
        let (status, body) = emulator
            .post(
                "/global-networks",
                json!({ "Description": "reserved", "Tags": [tag(key, "whatever")] }),
            )
            .await;
        assert_eq!(
            status, want_status,
            "case {case}: CreateGlobalNetwork({key:?}) expected {want_status}, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn tag_keys_and_values_outside_the_documented_lengths_are_refused() {
    // 128 characters for a key and 256 for a value are the documented maximums
    // (`smithy_query query="doc" shape="Tag" member="Key"` / `"Value"`); the rows carry
    // each bound and one past it, plus the empty key a tag may never have.
    let cases: Vec<(&str, String, String, u16)> = vec![
        ("a plain key and value", "env".to_string(), "test".to_string(), 200),
        ("an empty value", "env".to_string(), String::new(), 200),
        ("an empty key", String::new(), "test".to_string(), 400),
        (
            "a key at the documented maximum",
            "k".repeat(MAX_KEY_CHARS),
            "v".to_string(),
            200,
        ),
        (
            "a key one past the documented maximum",
            "k".repeat(MAX_KEY_CHARS + 1),
            "v".to_string(),
            400,
        ),
        (
            "a value at the documented maximum",
            "k".to_string(),
            "v".repeat(MAX_VALUE_CHARS),
            200,
        ),
        (
            "a value one past the documented maximum",
            "k".to_string(),
            "v".repeat(MAX_VALUE_CHARS + 1),
            400,
        ),
    ];

    for (case, key, value, want) in cases {
        let emulator = Emulator::new();
        let resource = create_taggable(&emulator, GLOBAL_NETWORK, &Value::Null).await;
        let (status, body) = emulator
            .post(
                &tags_uri(&resource.arn),
                json!({ "Tags": [tag(&key, &value)] }),
            )
            .await;
        assert_eq!(
            status, want,
            "case {case}: expected {want}, got {status} with {body}"
        );
        let (_, listed) = list_tags(&emulator, &resource.arn).await;
        if want == 200 {
            assert_eq!(
                listed,
                json!([tag(&key, &value)]),
                "case {case}: the tag must be stored as sent, got {listed}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case}: wrong error code in {body}"
            );
            assert_eq!(
                listed,
                json!([]),
                "case {case}: a refused request must store nothing, got {listed}"
            );
        }
    }
}

#[tokio::test]
async fn an_arn_naming_no_resource_is_refused_by_every_tagging_operation() {
    let emulator = Emulator::new();
    // A resource that DOES exist, so the rows below differ only in the ARN.
    let resource = create_taggable(&emulator, GLOBAL_NETWORK, &Value::Null).await;

    // A string that is not an ARN of this service is a bad request (400); a well-formed
    // ARN of this service that names no stored resource is not found (404) — including a
    // resource type this emulator holds none of, and this account's own resource read
    // under another account.
    let cases: Vec<(&str, String, u16)> = vec![
        ("not an ARN at all", "not-an-arn".to_string(), 400),
        (
            "an ARN with no resource id",
            "arn:aws:networkmanager::000000000000:global-network".to_string(),
            400,
        ),
        (
            "an ARN with an empty resource id",
            "arn:aws:networkmanager::000000000000:global-network/".to_string(),
            400,
        ),
        (
            "an ARN of another service",
            "arn:aws:ec2:us-west-2:000000000000:instance/i-0123456789abcdef0".to_string(),
            400,
        ),
        (
            "an ARN with an empty partition",
            "arn::networkmanager::000000000000:global-network/global-network-00000000000000001"
                .to_string(),
            400,
        ),
        (
            "a global network that does not exist",
            expected_arn(GLOBAL_NETWORK, "global-network-000000000000000ff"),
            404,
        ),
        (
            "a site that does not exist",
            expected_arn(SITE, "site-000000000000000ff"),
            404,
        ),
        (
            "a link that does not exist",
            expected_arn(LINK, "link-000000000000000ff"),
            404,
        ),
        (
            "a resource type this service holds none of",
            expected_arn("core-network", "core-network-000000000000000ff"),
            404,
        ),
        (
            "an existing resource under another account",
            resource.arn.replace("000000000000", "123456789012"),
            404,
        ),
        (
            "an existing resource under a region this global service does not use",
            resource
                .arn
                .replace("networkmanager::", "networkmanager:us-west-2:"),
            404,
        ),
    ];

    let want_code = |want: u16| {
        if want == 400 {
            "ValidationException"
        } else {
            "ResourceNotFoundException"
        }
    };

    for (case, arn, want) in cases {
        let (status, body) = emulator
            .post(&tags_uri(&arn), json!({ "Tags": [tag("env", "test")] }))
            .await;
        assert_eq!(
            status, want,
            "case {case}: TagResource expected {want}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            want_code(want),
            "case {case}: TagResource returned the wrong error code in {body}"
        );

        let (status, body) = emulator.delete(&untag_uri(&arn, &["env"])).await;
        assert_eq!(
            status, want,
            "case {case}: UntagResource expected {want}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            want_code(want),
            "case {case}: UntagResource returned the wrong error code in {body}"
        );

        let (status, body) = list_tags(&emulator, &arn).await;
        assert_eq!(
            status, want,
            "case {case}: ListTagsForResource expected {want}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            want_code(want),
            "case {case}: ListTagsForResource returned the wrong error code in {body}"
        );
    }

    // The resource itself was never touched by any of those refusals.
    let (status, listed) = list_tags(&emulator, &resource.arn).await;
    assert_eq!(
        status, 200,
        "the existing resource must still be readable, got {status} with {listed}"
    );
    assert_eq!(
        listed,
        json!([]),
        "no refused call may have stored a tag, got {listed}"
    );
}

#[tokio::test]
async fn tagging_past_the_per_resource_tag_quota_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let resource = create_taggable(&emulator, GLOBAL_NETWORK, &Value::Null).await;

    let full: Vec<Value> = (0..MAX_TAGS)
        .map(|index| tag(&format!("key-{index:02}"), &format!("value-{index}")))
        .collect();
    let (status, body) = emulator
        .post(&tags_uri(&resource.arn), json!({ "Tags": full }))
        .await;
    assert_eq!(
        status, 200,
        "tagging up to the quota: expected 200, got {status} with {body}"
    );
    let (_, listed) = list_tags(&emulator, &resource.arn).await;
    assert_eq!(
        listed.as_array().map(Vec::len),
        Some(MAX_TAGS),
        "tagging up to the quota must store every tag, got {listed}"
    );

    // One more NEW key would leave the resource past the quota, so it is refused with the
    // modelled quota error and nothing is stored.
    let (status, refused) = emulator
        .post(
            &tags_uri(&resource.arn),
            json!({ "Tags": [tag("one-too-many", "x")] }),
        )
        .await;
    assert_eq!(
        status, 402,
        "tagging past the quota: expected 402, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ServiceQuotaExceededException",
        "tagging past the quota: wrong error code in {refused}"
    );
    let (_, after) = list_tags(&emulator, &resource.arn).await;
    assert_eq!(
        after, listed,
        "a refused quota call must store nothing, got {after}"
    );

    // Re-tagging a key the resource already carries never trips the quota: the merge
    // replaces that key rather than adding one.
    let (status, body) = emulator
        .post(
            &tags_uri(&resource.arn),
            json!({ "Tags": [tag("key-00", "replaced")] }),
        )
        .await;
    assert_eq!(
        status, 200,
        "re-tagging an existing key at the quota: expected 200, got {status} with {body}"
    );
    let (_, after) = list_tags(&emulator, &resource.arn).await;
    assert_eq!(
        after.as_array().map(Vec::len),
        Some(MAX_TAGS),
        "re-tagging an existing key must not add one, got {after}"
    );
    assert_eq!(
        after[0],
        tag("key-00", "replaced"),
        "re-tagging must replace the value in place, got {after}"
    );

    // A create carrying more tags than the quota allows is refused the same way, and
    // stores no resource at all.
    let too_many: Vec<Value> = (0..=MAX_TAGS)
        .map(|index| tag(&format!("key-{index:02}"), "x"))
        .collect();
    let (status, refused) = emulator
        .post(
            "/global-networks",
            json!({ "Description": "too many tags", "Tags": too_many }),
        )
        .await;
    assert_eq!(
        status, 402,
        "creating with more tags than the quota: expected 402, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ServiceQuotaExceededException",
        "creating with more tags than the quota: wrong error code in {refused}"
    );
    let (status, networks) = emulator.get("/global-networks").await;
    assert_eq!(
        status, 200,
        "DescribeGlobalNetworks: expected 200, got {status} with {networks}"
    );
    assert_eq!(
        networks["GlobalNetworks"].as_array().map(Vec::len),
        Some(1),
        "a refused create must store no global network, got {networks}"
    );
}
