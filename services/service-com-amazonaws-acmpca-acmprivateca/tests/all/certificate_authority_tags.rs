// SPDX-License-Identifier: CC0-1.0

//! A private CA's tag set, driven over the wire: `TagCertificateAuthority` writes it and
//! `UntagCertificateAuthority` removes from it, both read back through `ListTags`.
//!
//! The rule under test is the one the operations share: a tag key identifies ONE tag of a
//! CA, so tagging leaves the CA holding the union of the tags it already had and the keys
//! the request names (a key already held taking the request's value), and untagging
//! removes a named key — by value when the request gives one. Every suite below asserts
//! that over a TABLE of tag sets; the tag the issue cited is one row. `ListTags`'s own
//! paging and continuation-token rules are covered by `tag_listing`.
//!
//! The emulator harness is `private_ca_lifecycle`'s, shared rather than rebuilt here.

use serde_json::{json, Value};

use crate::private_ca_lifecycle::{
    root_ca_request, string, wait_for_the_clock, Emulator, ACCOUNT, REGION,
};

/// `TagKey`'s published length bounds, from
/// `smithy_query(query="shape",shape="Tag")`: `Key` is `len 1..128`.
const TAG_KEY_MAX: usize = 128;

/// `TagValue`'s published length bounds, from the same member list: `Value` is
/// `len 0..256`.
const TAG_VALUE_MAX: usize = 256;

/// `TagList`'s published length bounds (`len 1..50`), which bound ONE request's list.
const TAG_LIST_MAX: usize = 50;

/// The most tags one private CA holds: "You can associate up to 50 tags with a private
/// CA." (`TooManyTagsException` documentation).
const TAGS_PER_CA_MAX: usize = 50;

/// A created CA's ARN, with no wait: tagging a CA is accepted in `CREATING` too, so a
/// suite that is not about the clock does not wait for it.
async fn created_ca(emulator: &Emulator, case: &str) -> String {
    let created = emulator
        .ok(case, "CreateCertificateAuthority", &root_ca_request(case))
        .await;
    string(&created, "CertificateAuthorityArn", case)
}

/// A created CA that also carries `tags` from birth.
async fn created_ca_with_tags(emulator: &Emulator, case: &str, tags: &Value) -> String {
    let mut body = root_ca_request(case);
    if !tags.is_null() {
        body["Tags"] = tags.clone();
    }
    let created = emulator
        .ok(case, "CreateCertificateAuthority", &body)
        .await;
    string(&created, "CertificateAuthorityArn", case)
}

/// The `Tags` member of a `ListTags` answer as (key, value) pairs, in the order the
/// service reported them. A tag stored without a value reports no `Value` member, which
/// is `None` here.
fn pairs(answer: &Value) -> Vec<(String, Option<String>)> {
    answer
        .get("Tags")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .map(|tag| {
                    (
                        tag.get("Key")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        tag.get("Value").and_then(Value::as_str).map(str::to_string),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every tag `ListTags` reports for `arn` in one page (a CA holds at most fifty tags, so
/// one page without `MaxResults` is the whole set).
async fn listed_tags(
    emulator: &Emulator,
    case: &str,
    arn: &str,
) -> Vec<(String, Option<String>)> {
    let answer = emulator
        .ok(case, "ListTags", &json!({"CertificateAuthorityArn": arn}))
        .await;
    pairs(&answer)
}

/// A tag set as (key, value) pairs: the form every expectation in this suite is written
/// in, with `None` for a tag that carries no value.
type Pairs<'a> = Vec<(&'a str, Option<&'a str>)>;

/// One row of the tagging table: the tags the CA is created with, the tag lists that
/// successive `TagCertificateAuthority` calls carry, and the set `ListTags` must report
/// once they are all applied.
struct TaggingRow<'a> {
    case: &'a str,
    created_with: Value,
    requests: Vec<Value>,
    expected: Pairs<'a>,
}

/// One row of the untagging table: the tag list one `UntagCertificateAuthority` call
/// carries, and the set `ListTags` must report afterwards.
struct UntaggingRow<'a> {
    case: &'a str,
    removed: Value,
    expected: Pairs<'a>,
}

/// The expected pairs in the key order `ListTags` reports.
fn expected_pairs(rows: &[(&str, Option<&str>)]) -> Vec<(String, Option<String>)> {
    let mut expected: Vec<(String, Option<String>)> = rows
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.map(str::to_string)))
        .collect();
    expected.sort_by(|left, right| left.0.cmp(&right.0));
    expected
}

/// A tag set is the union of what the CA already held and what the request names: a key
/// it already carries takes the new value, a key it does not carry is added, and the
/// request's own order does not matter. One row per way a tag set can differ — the count,
/// the value, whether there is a value at all, the script the key and value are written
/// in, the length, and the order the request lists them in.
#[tokio::test]
async fn tagging_leaves_the_union_of_the_stored_and_requested_tags() {
    let emulator = Emulator::new();
    let long_key = "k".repeat(TAG_KEY_MAX);
    let long_value = "v".repeat(TAG_VALUE_MAX);

    let rows = vec![
        TaggingRow {
            case: "one tag onto an untagged CA",
            created_with: Value::Null,
            requests: vec![json!([{"Key": "owner", "Value": "pki"}])],
            expected: vec![("owner", Some("pki"))],
        },
        TaggingRow {
            case: "three tags in one request",
            created_with: Value::Null,
            requests: vec![json!([
                {"Key": "owner", "Value": "pki"},
                {"Key": "tier", "Value": "root"},
                {"Key": "env", "Value": "prod"},
            ])],
            expected: vec![
                ("owner", Some("pki")),
                ("tier", Some("root")),
                ("env", Some("prod")),
            ],
        },
        TaggingRow {
            case: "a stored key takes the requested value",
            created_with: json!([{"Key": "owner", "Value": "pki"}]),
            requests: vec![json!([{"Key": "owner", "Value": "sre"}])],
            expected: vec![("owner", Some("sre"))],
        },
        TaggingRow {
            case: "a new key joins the stored ones",
            created_with: json!([{"Key": "owner", "Value": "pki"}]),
            requests: vec![json!([{"Key": "tier", "Value": "root"}])],
            expected: vec![("owner", Some("pki")), ("tier", Some("root"))],
        },
        TaggingRow {
            case: "a tag with no value",
            created_with: Value::Null,
            requests: vec![json!([{"Key": "owner"}])],
            expected: vec![("owner", None)],
        },
        TaggingRow {
            case: "a tag with an empty value",
            created_with: Value::Null,
            requests: vec![json!([{"Key": "owner", "Value": ""}])],
            expected: vec![("owner", Some(""))],
        },
        TaggingRow {
            case: "a valueless tag takes a value and loses it again",
            created_with: json!([{"Key": "owner"}]),
            requests: vec![
                json!([{"Key": "owner", "Value": "pki"}]),
                json!([{"Key": "owner"}]),
            ],
            expected: vec![("owner", None)],
        },
        TaggingRow {
            case: "keys and values outside ASCII",
            created_with: Value::Null,
            requests: vec![json!([
                {"Key": "propriétaire", "Value": "équipe PKI"},
                {"Key": "所有者", "Value": "日本語の値"},
                {"Key": "Ωμέγα", "Value": "τιμή"},
            ])],
            expected: vec![
                ("propriétaire", Some("équipe PKI")),
                ("所有者", Some("日本語の値")),
                ("Ωμέγα", Some("τιμή")),
            ],
        },
        TaggingRow {
            case: "a key and a value at their published maximum length",
            created_with: Value::Null,
            requests: vec![json!([{"Key": long_key, "Value": long_value}])],
            expected: vec![(long_key.as_str(), Some(long_value.as_str()))],
        },
        TaggingRow {
            case: "two requests whose keys partly overlap",
            created_with: json!([{"Key": "a", "Value": "1"}, {"Key": "b", "Value": "2"}]),
            requests: vec![json!([{"Key": "b", "Value": "two"}, {"Key": "c", "Value": "3"}])],
            expected: vec![("a", Some("1")), ("b", Some("two")), ("c", Some("3"))],
        },
        TaggingRow {
            case: "the request's own order does not change the set",
            created_with: Value::Null,
            requests: vec![json!([
                {"Key": "zeta", "Value": "last"},
                {"Key": "alpha", "Value": "first"},
            ])],
            expected: vec![("alpha", Some("first")), ("zeta", Some("last"))],
        },
    ];

    for row in &rows {
        let case = row.case;
        let arn = created_ca_with_tags(&emulator, case, &row.created_with).await;
        for tags in &row.requests {
            emulator
                .ok(
                    case,
                    "TagCertificateAuthority",
                    &json!({"CertificateAuthorityArn": arn, "Tags": tags}),
                )
                .await;
        }
        let listed = listed_tags(&emulator, case, &arn).await;
        assert_eq!(
            listed,
            expected_pairs(&row.expected),
            "case {case:?}: expected ListTags to report {:?}, got {listed:?}",
            expected_pairs(&row.expected)
        );
    }
}

/// Untagging: "If you do not specify the value portion of the tag when calling this
/// action, the tag will be removed regardless of value. If you specify a value, the tag
/// is removed only if it is associated with the specified value."
#[tokio::test]
async fn untagging_removes_a_key_by_itself_and_a_key_with_its_value() {
    let emulator = Emulator::new();

    let stored = json!([
        {"Key": "owner", "Value": "pki"},
        {"Key": "tier", "Value": "root"},
        {"Key": "bare"},
    ]);
    let rows = vec![
        UntaggingRow {
            case: "a key with no value removes it whatever its value",
            removed: json!([{"Key": "owner"}]),
            expected: vec![("tier", Some("root")), ("bare", None)],
        },
        UntaggingRow {
            case: "a key with its stored value removes it",
            removed: json!([{"Key": "owner", "Value": "pki"}]),
            expected: vec![("tier", Some("root")), ("bare", None)],
        },
        UntaggingRow {
            case: "a key with another value removes nothing",
            removed: json!([{"Key": "owner", "Value": "sre"}]),
            expected: vec![("owner", Some("pki")), ("tier", Some("root")), ("bare", None)],
        },
        UntaggingRow {
            case: "a key the CA does not carry removes nothing",
            removed: json!([{"Key": "absent", "Value": "x"}]),
            expected: vec![("owner", Some("pki")), ("tier", Some("root")), ("bare", None)],
        },
        UntaggingRow {
            case: "two keys in one request",
            removed: json!([{"Key": "owner"}, {"Key": "tier", "Value": "root"}]),
            expected: vec![("bare", None)],
        },
        UntaggingRow {
            case: "a valueless stored tag matches the empty value",
            removed: json!([{"Key": "bare", "Value": ""}]),
            expected: vec![("owner", Some("pki")), ("tier", Some("root"))],
        },
        UntaggingRow {
            case: "a valueless stored tag removed by its key",
            removed: json!([{"Key": "bare"}]),
            expected: vec![("owner", Some("pki")), ("tier", Some("root"))],
        },
        UntaggingRow {
            case: "every key at once",
            removed: json!([{"Key": "owner"}, {"Key": "tier"}, {"Key": "bare"}]),
            expected: vec![],
        },
    ];

    for row in &rows {
        let case = row.case;
        let arn = created_ca_with_tags(&emulator, case, &stored).await;
        emulator
            .ok(
                case,
                "UntagCertificateAuthority",
                &json!({"CertificateAuthorityArn": arn, "Tags": row.removed}),
            )
            .await;
        let listed = listed_tags(&emulator, case, &arn).await;
        assert_eq!(
            listed,
            expected_pairs(&row.expected),
            "case {case:?}: expected ListTags to report {:?} after untagging, got {listed:?}",
            expected_pairs(&row.expected)
        );
    }

    // A CA left with no tags reports no `Tags` member at all: `TagList` is modelled
    // `len 1..50`, so it has no empty value.
    let case = "an untagged CA";
    let arn = created_ca(&emulator, "untagged-ca").await;
    let answer = emulator
        .ok(case, "ListTags", &json!({"CertificateAuthorityArn": arn}))
        .await;
    assert!(
        answer.get("Tags").is_none(),
        "case {case:?}: expected no Tags member for a CA that holds none, got {answer:?}"
    );
    assert!(
        answer.get("NextToken").is_none(),
        "case {case:?}: expected no NextToken for a CA that holds no tags, got {answer:?}"
    );
}

/// The ARN rules every one of the three tag operations reads a CA under: a form this
/// service never mints is an invalid ARN, and a well-formed ARN that names nothing stored
/// is not found.
#[tokio::test]
async fn the_tag_operations_refuse_an_arn_by_form_and_then_by_existence() {
    let emulator = Emulator::new();
    let malformed = [
        (
            "no ca id",
            format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority"),
        ),
        (
            "another resource type",
            format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:private-ca/abc"),
        ),
        (
            "an extra resource segment",
            format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/abc/extra"),
        ),
        (
            "an account segment that is not twelve digits",
            format!("arn:aws:acm-pca:{REGION}:1234:certificate-authority/abc"),
        ),
    ];
    let unknown = format!(
        "arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/\
         00000000-0000-4000-8000-000000000000"
    );

    for (operation, extra) in [
        (
            "TagCertificateAuthority",
            json!({"Tags": [{"Key": "owner", "Value": "pki"}]}),
        ),
        (
            "UntagCertificateAuthority",
            json!({"Tags": [{"Key": "owner"}]}),
        ),
        ("ListTags", json!({})),
    ] {
        for (case, arn) in &malformed {
            let case = format!("{operation}: {case}");
            let mut body = extra.clone();
            body["CertificateAuthorityArn"] = json!(arn);
            emulator
                .refused(&case, operation, &body, "InvalidArnException")
                .await;
        }
        let case = format!("{operation}: a CA that was never created");
        let mut body = extra.clone();
        body["CertificateAuthorityArn"] = json!(unknown);
        emulator
            .refused(&case, operation, &body, "ResourceNotFoundException")
            .await;
    }
}

/// The tag-set rules `TagCertificateAuthority` and `UntagCertificateAuthority` hold every
/// request to: the `aws:` prefix is reserved, and one request cannot name a key twice.
/// A refused request leaves the CA's tag set exactly as it was.
#[tokio::test]
async fn the_tag_operations_refuse_a_reserved_or_repeated_key() {
    let emulator = Emulator::new();
    let stored = json!([{"Key": "owner", "Value": "pki"}]);

    let rows: Vec<(&str, Value)> = vec![
        (
            "a reserved prefix",
            json!([{"Key": "aws:created-by", "Value": "me"}]),
        ),
        (
            "another reserved key",
            json!([{"Key": "aws:cloudformation:stack-name", "Value": "pki"}]),
        ),
        (
            "a reserved key beside an ordinary one",
            json!([{"Key": "tier", "Value": "root"}, {"Key": "aws:owner", "Value": "me"}]),
        ),
        (
            "a repeated key",
            json!([{"Key": "tier", "Value": "root"}, {"Key": "tier", "Value": "leaf"}]),
        ),
        (
            "a repeated key with the same value",
            json!([{"Key": "tier", "Value": "root"}, {"Key": "tier", "Value": "root"}]),
        ),
    ];

    for operation in ["TagCertificateAuthority", "UntagCertificateAuthority"] {
        for (case, tags) in &rows {
            let case = format!("{operation}: {case}");
            let arn = created_ca_with_tags(&emulator, &case, &stored).await;
            emulator
                .refused(
                    &case,
                    operation,
                    &json!({"CertificateAuthorityArn": arn, "Tags": tags}),
                    "InvalidTagException",
                )
                .await;
            let listed = listed_tags(&emulator, &case, &arn).await;
            assert_eq!(
                listed,
                expected_pairs(&[("owner", Some("pki"))]),
                "case {case:?}: a refused request must leave the tag set alone, got {listed:?}"
            );
        }
    }
}

/// The modelled bounds on a tag and on a request's tag list, each at its published limit
/// and one past it. `TagKey` is `len 1..128`, `TagValue` `len 0..256` and `TagList`
/// `len 1..50` (`smithy_query(query="shape",shape="Tag,TagList")`), so the boundary
/// validator refuses the row past each bound before any handler runs.
#[tokio::test]
async fn a_tag_request_is_held_to_the_modelled_bounds() {
    let emulator = Emulator::new();
    let arn = created_ca(&emulator, "bounded").await;

    let rows: Vec<(String, Value, bool)> = vec![
        (
            format!("a key of {TAG_KEY_MAX} characters"),
            json!([{"Key": "k".repeat(TAG_KEY_MAX), "Value": "v"}]),
            true,
        ),
        (
            format!("a key of {} characters", TAG_KEY_MAX + 1),
            json!([{"Key": "k".repeat(TAG_KEY_MAX + 1), "Value": "v"}]),
            false,
        ),
        (
            "a key of no characters".to_string(),
            json!([{"Key": "", "Value": "v"}]),
            false,
        ),
        (
            "a value of no characters".to_string(),
            json!([{"Key": "empty-value", "Value": ""}]),
            true,
        ),
        (
            format!("a value of {TAG_VALUE_MAX} characters"),
            json!([{"Key": "long-value", "Value": "v".repeat(TAG_VALUE_MAX)}]),
            true,
        ),
        (
            format!("a value of {} characters", TAG_VALUE_MAX + 1),
            json!([{"Key": "longer-value", "Value": "v".repeat(TAG_VALUE_MAX + 1)}]),
            false,
        ),
        (
            format!("a list of {TAG_LIST_MAX} tags"),
            json!((0..TAG_LIST_MAX)
                .map(|index| json!({"Key": format!("listed-{index:02}")}))
                .collect::<Vec<Value>>()),
            true,
        ),
        (
            format!("a list of {} tags", TAG_LIST_MAX + 1),
            json!((0..=TAG_LIST_MAX)
                .map(|index| json!({"Key": format!("over-{index:02}")}))
                .collect::<Vec<Value>>()),
            false,
        ),
        (
            "a list of no tags".to_string(),
            json!([]),
            false,
        ),
    ];

    for (case, tags, accepted) in &rows {
        // Each row is applied to its own CA, so one row's fifty tags cannot push the
        // next row past the per-CA ceiling.
        let arn = if *accepted {
            created_ca(&emulator, case).await
        } else {
            arn.clone()
        };
        let (status, answer) = emulator
            .call(
                "TagCertificateAuthority",
                &json!({"CertificateAuthorityArn": arn, "Tags": tags}),
            )
            .await;
        let expected = if *accepted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case:?}: expected TagCertificateAuthority to answer {expected}, got \
             {status} {answer:?}"
        );
    }
}

/// "You can associate up to 50 tags with a private CA." The ceiling is on the CA, not on
/// one request: tags accumulate across requests, the request that would take the CA past
/// fifty is refused and writes nothing, and re-tagging a key the CA already holds is
/// accepted at the ceiling because it adds no tag.
#[tokio::test]
async fn a_ca_holds_at_most_the_published_number_of_tags() {
    let emulator = Emulator::new();
    let case = "the per-CA tag ceiling";
    let arn = created_ca(&emulator, "ceiling").await;

    let tags = |range: std::ops::Range<usize>| -> Value {
        json!(range
            .map(|index| json!({"Key": format!("key-{index:02}"), "Value": "v"}))
            .collect::<Vec<Value>>())
    };

    // Two accepted requests of twenty-five leave the CA at the ceiling.
    for range in [0..25, 25..TAGS_PER_CA_MAX] {
        emulator
            .ok(
                case,
                "TagCertificateAuthority",
                &json!({"CertificateAuthorityArn": arn, "Tags": tags(range)}),
            )
            .await;
    }
    let listed = listed_tags(&emulator, case, &arn).await;
    assert_eq!(
        listed.len(),
        TAGS_PER_CA_MAX,
        "case {case:?}: expected the CA to hold {TAGS_PER_CA_MAX} tags, got {}",
        listed.len()
    );

    // One more key is refused, and the refusal leaves the stored set alone.
    for (over, extra) in [
        ("one key past the ceiling", tags(TAGS_PER_CA_MAX..TAGS_PER_CA_MAX + 1)),
        ("ten keys past the ceiling", tags(TAGS_PER_CA_MAX..TAGS_PER_CA_MAX + 10)),
    ] {
        let case = format!("{case}: {over}");
        emulator
            .refused(
                &case,
                "TagCertificateAuthority",
                &json!({"CertificateAuthorityArn": arn, "Tags": extra}),
                "TooManyTagsException",
            )
            .await;
        let after = listed_tags(&emulator, &case, &arn).await;
        assert_eq!(
            after, listed,
            "case {case:?}: a refused request must leave the tag set alone, got {after:?}"
        );
    }

    // A key the CA already holds adds no tag, so it is accepted at the ceiling.
    emulator
        .ok(
            case,
            "TagCertificateAuthority",
            &json!({
                "CertificateAuthorityArn": arn,
                "Tags": [{"Key": "key-00", "Value": "replaced"}],
            }),
        )
        .await;
    let after = listed_tags(&emulator, case, &arn).await;
    assert_eq!(
        after.len(),
        TAGS_PER_CA_MAX,
        "case {case:?}: replacing a value must not change the tag count, got {}",
        after.len()
    );
    assert_eq!(
        after.first().map(|(key, value)| (key.as_str(), value.as_deref())),
        Some(("key-00", Some("replaced"))),
        "case {case:?}: expected key-00 to carry the new value, got {after:?}"
    );

    // Removing a tag makes room again: the ceiling is on what the CA holds now.
    emulator
        .ok(
            case,
            "UntagCertificateAuthority",
            &json!({"CertificateAuthorityArn": arn, "Tags": [{"Key": "key-00"}]}),
        )
        .await;
    emulator
        .ok(
            case,
            "TagCertificateAuthority",
            &json!({
                "CertificateAuthorityArn": arn,
                "Tags": [{"Key": "key-99", "Value": "v"}],
            }),
        )
        .await;
    let after = listed_tags(&emulator, case, &arn).await;
    assert_eq!(
        after.len(),
        TAGS_PER_CA_MAX,
        "case {case:?}: expected the CA to be back at the ceiling, got {after:?}"
    );
    assert!(
        after.iter().any(|(key, _)| key == "key-99")
            && !after.iter().any(|(key, _)| key == "key-00"),
        "case {case:?}: expected key-99 to have replaced key-00, got {after:?}"
    );
}

/// A CA's tags are readable in every status this service reports it in: tags given at
/// creation time are already there while it is `CREATING`, and tagging is accepted both
/// before and after the clock settles the CA into `PENDING_CERTIFICATE`.
#[tokio::test]
async fn tags_are_written_and_read_in_every_status_a_ca_reports() {
    let emulator = Emulator::new();
    let case = "tagging across the CA's statuses";
    let arn = created_ca_with_tags(
        &emulator,
        "statuses",
        &json!([{"Key": "at-creation", "Value": "yes"}]),
    )
    .await;

    // Still CREATING: the tag the create call carried is already stored, and another one
    // is accepted.
    let creating = emulator
        .ok(
            case,
            "DescribeCertificateAuthority",
            &json!({"CertificateAuthorityArn": arn}),
        )
        .await;
    let status = creating
        .get("CertificateAuthority")
        .map(|ca| string(ca, "Status", case))
        .unwrap_or_default();
    assert_eq!(
        status, "CREATING",
        "case {case:?}: expected a freshly created CA to be CREATING, got {status:?}"
    );
    emulator
        .ok(
            case,
            "TagCertificateAuthority",
            &json!({
                "CertificateAuthorityArn": arn,
                "Tags": [{"Key": "while-creating", "Value": "yes"}],
            }),
        )
        .await;

    wait_for_the_clock();

    let settled = emulator
        .ok(
            case,
            "DescribeCertificateAuthority",
            &json!({"CertificateAuthorityArn": arn}),
        )
        .await;
    let status = settled
        .get("CertificateAuthority")
        .map(|ca| string(ca, "Status", case))
        .unwrap_or_default();
    assert_eq!(
        status, "PENDING_CERTIFICATE",
        "case {case:?}: expected the CA to have settled, got {status:?}"
    );
    emulator
        .ok(
            case,
            "TagCertificateAuthority",
            &json!({
                "CertificateAuthorityArn": arn,
                "Tags": [{"Key": "after-settling", "Value": "yes"}],
            }),
        )
        .await;

    let listed = listed_tags(&emulator, case, &arn).await;
    assert_eq!(
        listed,
        expected_pairs(&[
            ("at-creation", Some("yes")),
            ("while-creating", Some("yes")),
            ("after-settling", Some("yes")),
        ]),
        "case {case:?}: expected every tag written along the way to be readable, got {listed:?}"
    );
}

/// A CA's tags are readable in every status this service reports it in: tags given at
/// creation time are already there while it is `CREATING`, and tagging is accepted both
/// before and after the clock settles the CA into `PENDING_CERTIFICATE`. A CA that has
/// been DELETED is the one status that refuses the write.
#[tokio::test]
async fn a_deleted_ca_refuses_a_tag_write() {
    let emulator = Emulator::new();
    let case = "a deleted CA";
    let arn = created_ca_with_tags(
        &emulator,
        "deleted",
        &json!([{"Key": "before-deletion", "Value": "yes"}]),
    )
    .await;
    // A CA is deletable from PENDING_CERTIFICATE, which the clock owes it after creation.
    wait_for_the_clock();
    emulator
        .ok(
            case,
            "DeleteCertificateAuthority",
            &json!({"CertificateAuthorityArn": arn}),
        )
        .await;
    let described = emulator
        .ok(
            case,
            "DescribeCertificateAuthority",
            &json!({"CertificateAuthorityArn": arn}),
        )
        .await;
    let status = described
        .get("CertificateAuthority")
        .map(|ca| string(ca, "Status", case))
        .unwrap_or_default();
    assert_eq!(
        status, "DELETED",
        "case {case:?}: expected the deleted CA to report DELETED, got {status:?}"
    );

    for (operation, tags) in [
        (
            "TagCertificateAuthority",
            json!([{"Key": "after-deletion", "Value": "no"}]),
        ),
        (
            "UntagCertificateAuthority",
            json!([{"Key": "before-deletion"}]),
        ),
    ] {
        let case = format!("{case}: {operation}");
        emulator
            .refused(
                &case,
                operation,
                &json!({"CertificateAuthorityArn": arn, "Tags": tags}),
                "InvalidStateException",
            )
            .await;
        let listed = listed_tags(&emulator, &case, &arn).await;
        assert_eq!(
            listed,
            expected_pairs(&[("before-deletion", Some("yes"))]),
            "case {case:?}: a refused write must leave the stored tag set alone, got {listed:?}"
        );
    }
}

/// The tag set belongs to the CA it was written to: tagging one CA leaves another CA's
/// tags alone, whichever order the two are written in.
#[tokio::test]
async fn one_cas_tags_are_invisible_to_another() {
    let emulator = Emulator::new();
    let case = "two CAs";
    let first = created_ca(&emulator, "first-ca").await;
    let second = created_ca(&emulator, "second-ca").await;

    for (arn, key) in [(&first, "only-on-first"), (&second, "only-on-second")] {
        emulator
            .ok(
                case,
                "TagCertificateAuthority",
                &json!({"CertificateAuthorityArn": arn, "Tags": [{"Key": key, "Value": "yes"}]}),
            )
            .await;
    }

    for (arn, expected) in [(&first, "only-on-first"), (&second, "only-on-second")] {
        let listed = listed_tags(&emulator, case, arn).await;
        assert_eq!(
            listed,
            expected_pairs(&[(expected, Some("yes"))]),
            "case {case:?}: expected {arn} to carry only {expected:?}, got {listed:?}"
        );
    }
}
