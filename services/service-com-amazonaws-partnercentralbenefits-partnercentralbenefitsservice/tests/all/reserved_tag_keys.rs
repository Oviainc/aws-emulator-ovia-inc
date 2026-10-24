// SPDX-License-Identifier: CC0-1.0

//! The reserved `aws:` tag-key namespace, end to end over the wire.
//!
//! Every assertion goes through the real request path — the shared
//! [`crate::benefit_application_lifecycle::Harness`], i.e. `MultiServiceRouter::dispatch`
//! with `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>` — so what it pins is the emulator's HTTP
//! behaviour, not an internal helper.
//!
//! The rule under test: a tag key a CALLER supplies may not lie in the AWS-reserved `aws:`
//! namespace, so every operation that takes caller-supplied tag keys refuses such a
//! request with the modelled `ValidationException` (HTTP 400) and changes no tag — a
//! caller can neither create, overwrite nor remove a key in that namespace. The modelled
//! `TagKey` pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$` (`smithy_query(query="shape",
//! shape="Tag")`) admits `:`, so the rule is NOT a modelled constraint and each handler
//! applies it: `TagResource$tags`, `UntagResource$tagKeys` and
//! `CreateBenefitApplication$Tags` are every input member of this service that carries a
//! tag key (`ListTagsForResource` only reports what is stored).
//!
//! The tables span what the rule generalises over rather than the one key the issue named:
//! the key's case (`aws:`, `AWS:`, `Aws:`), what follows the prefix (nothing, one segment,
//! a further `:`-separated namespace, non-ASCII letters, a suffix taking the key to the
//! published 128-character maximum), the position of the prefix inside the key (at the
//! start, after another character, at the end), and the near misses that are NOT reserved
//! (`aws` with no colon, `awsome`, `aws.`, another service's namespace). Each row is
//! driven through all three operations, and the `aws:createdBy` key the issue reported is
//! one row among them.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    is_reserved_tag_key, reserved_tag_key_requirement, APPLICATION_RESOURCE,
    RESERVED_TAG_KEY_PREFIX, SEEDED_CATALOGS,
};

use crate::benefit_application_lifecycle::{error_code, Harness};
use crate::resource_tagging::{
    application_arn, benefit_id, expected_arn, filler, ok, tag_payload, tags_of,
};
use crate::service_error_rules::{assert_validation_invariants, clause, message_of, one};

/// The published maximum length of the modelled `TagKey` (`length {min: 1, max: 128}`,
/// from `smithy_query(query="shape",shape="Tag")`), so a row can sit exactly on it.
const TAG_KEY_MAX_LEN: usize = 128;

/// One row of the key table: a tag key, and whether it lies in the reserved namespace.
struct Key {
    case: &'static str,
    key: String,
    reserved: bool,
}

/// The keys every table below is driven over.
///
/// The reserved rows differ in the case of the prefix, in what follows it, and in the
/// length of the whole key; the admissible rows are the near misses — the same letters
/// without the `:`, the prefix somewhere other than the start, and another service's
/// `:`-separated namespace — so the refusal is pinned to the reserved PREFIX and not to
/// the letters `aws` or to the presence of a colon.
fn keys() -> Vec<Key> {
    vec![
        Key {
            case: "the prefix alone",
            key: RESERVED_TAG_KEY_PREFIX.to_string(),
            reserved: true,
        },
        Key {
            // The key the issue reported.
            case: "the reported key",
            key: "aws:createdBy".to_string(),
            reserved: true,
        },
        Key {
            case: "an AWS system tag with a further namespace",
            key: "aws:cloudformation:stack-name".to_string(),
            reserved: true,
        },
        Key {
            case: "the prefix in upper case",
            key: "AWS:createdBy".to_string(),
            reserved: true,
        },
        Key {
            case: "the prefix in mixed case",
            key: "Aws:CreatedBy".to_string(),
            reserved: true,
        },
        Key {
            case: "a reserved key whose suffix is non-ASCII letters",
            key: "aws:créé-par".to_string(),
            reserved: true,
        },
        Key {
            case: "a reserved key at the published 128-character maximum",
            key: format!(
                "{RESERVED_TAG_KEY_PREFIX}{}",
                filler(TAG_KEY_MAX_LEN - RESERVED_TAG_KEY_PREFIX.len())
            ),
            reserved: true,
        },
        Key {
            case: "the prefix letters without the colon",
            key: "aws".to_string(),
            reserved: false,
        },
        Key {
            case: "a word that merely opens with those letters",
            key: "awsome".to_string(),
            reserved: false,
        },
        Key {
            case: "the prefix after another character",
            key: "myaws:createdBy".to_string(),
            reserved: false,
        },
        Key {
            case: "the prefix at the end of the key",
            key: "createdBy:aws".to_string(),
            reserved: false,
        },
        Key {
            case: "the prefix letters with another separator",
            key: "aws.createdBy".to_string(),
            reserved: false,
        },
        Key {
            case: "another service's namespace",
            key: "ec2:createdBy".to_string(),
            reserved: false,
        },
        Key {
            case: "a non-ASCII key of the same length as the prefix",
            key: "日本:x".to_string(),
            reserved: false,
        },
        Key {
            case: "an admissible key at the published 128-character maximum",
            key: filler(TAG_KEY_MAX_LEN),
            reserved: false,
        },
    ]
}

/// The tag set every resource below is created with, so what a refused call did NOT change
/// is visible.
fn seeded() -> Vec<(&'static str, &'static str)> {
    vec![("env", "test"), ("kept", "yes")]
}
/// That same set as the store keeps it.
fn seeded_map() -> BTreeMap<String, String> {
    seeded()
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Require that `operation` refuses `body` as a reserved-key violation of `member`: HTTP
/// 400, the modelled `ValidationException`, and a message that names the member the caller
/// must fix and the requirement it failed.
async fn assert_reserved_refusal(
    harness: &Harness,
    case: &str,
    operation: &str,
    member: &str,
    body: &Value,
) {
    let (status, out) = harness.call(operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to refuse a reserved tag key with 400, \
         got {status} {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ValidationException",
        "case {case:?}: expected ValidationException from {operation}, got {out:?}"
    );
    let message = message_of(&out).unwrap_or_else(|| {
        panic!("case {case:?}: the refusal carries no modelled Message member: {out:?}")
    });
    assert_eq!(
        message,
        one(&clause(member, &reserved_tag_key_requirement())),
        "case {case:?}: expected the refusal to name {member:?} and the reserved prefix, \
         got {message:?}"
    );
    assert_validation_invariants(case, &message);
    let named = out["FieldList"]
        .as_array()
        .and_then(|fields| fields.first())
        .and_then(|field| field["Name"].as_str());
    assert_eq!(
        named,
        Some(member),
        "case {case:?}: the refusal's FieldList must name {member:?}, got {out:?}"
    );
}

// ---------------------------------------------------------------------------
// The predicate itself
// ---------------------------------------------------------------------------

/// Every row of the key table is classified the way the rule says: the reserved namespace
/// is the `aws:` prefix at the START of the key, whatever case it is spelled in, and
/// nothing else.
#[test]
fn the_reserved_namespace_is_the_prefix_and_nothing_else() {
    for row in keys() {
        let got = is_reserved_tag_key(&row.key);
        assert_eq!(
            got, row.reserved,
            "case {:?}: expected is_reserved_tag_key({:?}) to be {}, got {got}",
            row.case, row.key, row.reserved
        );
    }
}

// ---------------------------------------------------------------------------
// TagResource
// ---------------------------------------------------------------------------

/// `TagResource` refuses a reserved key and stores nothing, in every catalog the service
/// serves; a key outside the namespace is still merged and read back.
#[tokio::test]
async fn tag_resource_refuses_a_reserved_key_and_serves_every_other() {
    for catalog in SEEDED_CATALOGS {
        let harness = Harness::new();
        let arn = application_arn(
            &harness,
            catalog,
            catalog,
            &format!("tok-reserved-tag-{catalog}"),
            &seeded(),
        )
        .await;
        let mut expected = seeded_map();

        for row in keys() {
            let case = format!("{catalog}: TagResource with {}", row.case);
            let body = json!({
                "resourceArn": arn,
                "tags": tag_payload(&[(row.key.as_str(), "mine")]),
            });
            if row.reserved {
                assert_reserved_refusal(&harness, &case, "TagResource", "tags", &body).await;
            } else {
                ok(&harness, &case, "TagResource", &body).await;
                expected.insert(row.key.clone(), "mine".to_string());
            }
            let got = tags_of(&harness, &case, &arn).await;
            assert_eq!(
                got, expected,
                "case {case:?}: expected the tag set {expected:?}, got {got:?}"
            );
            assert!(
                !got.keys().any(|key| is_reserved_tag_key(key)),
                "case {case:?}: no reserved key may ever be stored, got {got:?}"
            );
        }
    }
}

/// One reserved key among admissible ones refuses the WHOLE call: a tagging call is not
/// partially applied, so none of its keys is stored.
#[tokio::test]
async fn one_reserved_key_refuses_the_whole_tagging_call() {
    let harness = Harness::new();
    let catalog = SEEDED_CATALOGS[0];
    let arn = application_arn(&harness, "mixed", catalog, "tok-reserved-mixed", &seeded()).await;

    // The reserved key's position inside the list is what these rows differ along.
    let rows: Vec<(&str, Vec<(&str, &str)>)> = vec![
        (
            "the reserved key first",
            vec![("aws:createdBy", "mine"), ("own", "1")],
        ),
        (
            "the reserved key last",
            vec![("own", "1"), ("AWS:createdBy", "mine")],
        ),
        (
            "the reserved key in the middle",
            vec![("own", "1"), ("aws:cloudformation:stack-name", "mine"), ("other", "2")],
        ),
    ];

    for (case, tags) in rows {
        assert_reserved_refusal(
            &harness,
            case,
            "TagResource",
            "tags",
            &json!({ "resourceArn": arn, "tags": tag_payload(&tags) }),
        )
        .await;
        let got = tags_of(&harness, case, &arn).await;
        assert_eq!(
            got,
            seeded_map(),
            "case {case:?}: a refused merge must store none of its keys, got {got:?}"
        );
    }
}

/// The reserved-key rule is a fact about the REQUEST, so it is applied before the resource
/// is looked up: a reserved key with a `resourceArn` that names no resource is still the
/// 400, not the lookup's 404.
#[tokio::test]
async fn a_reserved_key_is_refused_before_the_resource_lookup() {
    let harness = Harness::new();
    let missing = expected_arn(
        SEEDED_CATALOGS[0],
        APPLICATION_RESOURCE,
        "benappl-00000000000000",
    );

    assert_reserved_refusal(
        &harness,
        "TagResource on an ARN that names no resource",
        "TagResource",
        "tags",
        &json!({
            "resourceArn": missing,
            "tags": tag_payload(&[("aws:createdBy", "mine")]),
        }),
    )
    .await;
    assert_reserved_refusal(
        &harness,
        "UntagResource on an ARN that names no resource",
        "UntagResource",
        "tagKeys",
        &json!({ "resourceArn": missing, "tagKeys": ["aws:createdBy"] }),
    )
    .await;
}

// ---------------------------------------------------------------------------
// UntagResource
// ---------------------------------------------------------------------------

/// `UntagResource` refuses a reserved key — a caller cannot DELETE one either — and
/// removes nothing; a key outside the namespace is still removed.
#[tokio::test]
async fn untag_resource_refuses_a_reserved_key_and_serves_every_other() {
    let harness = Harness::new();
    let catalog = SEEDED_CATALOGS[0];

    for (index, row) in keys().into_iter().enumerate() {
        let case = format!("UntagResource with {}", row.case);
        // The resource carries the row's own key (when it is one a caller may create) plus
        // the seeded set, so a removal has something to remove and something to leave.
        let mut tags: Vec<(&str, &str)> = seeded();
        if !row.reserved {
            tags.push((row.key.as_str(), "mine"));
        }
        let arn = application_arn(
            &harness,
            &case,
            catalog,
            &format!("tok-reserved-untag-{index}"),
            &tags,
        )
        .await;

        // Either way what must remain is the seeded set: a reserved key is refused and
        // nothing is removed, an admissible one is removed and the seeded keys stay.
        let body = json!({ "resourceArn": arn, "tagKeys": [row.key.clone()] });
        if row.reserved {
            assert_reserved_refusal(&harness, &case, "UntagResource", "tagKeys", &body).await;
        } else {
            ok(&harness, &case, "UntagResource", &body).await;
        }
        let got = tags_of(&harness, &case, &arn).await;
        assert_eq!(
            got,
            seeded_map(),
            "case {case:?}: expected the tag set {:?}, got {got:?}",
            seeded_map()
        );
    }
}

/// One reserved key among admissible ones refuses the whole removal, so a caller cannot
/// smuggle a reserved deletion in beside its own keys — and the admissible keys it named
/// stay put.
#[tokio::test]
async fn one_reserved_key_refuses_the_whole_removal() {
    let harness = Harness::new();
    let catalog = SEEDED_CATALOGS[0];
    let arn = application_arn(
        &harness,
        "mixed removal",
        catalog,
        "tok-reserved-untag-mixed",
        &seeded(),
    )
    .await;

    for (case, tag_keys) in [
        ("the reserved key first", vec!["aws:createdBy", "env"]),
        ("the reserved key last", vec!["env", "Aws:CreatedBy"]),
    ] {
        assert_reserved_refusal(
            &harness,
            case,
            "UntagResource",
            "tagKeys",
            &json!({ "resourceArn": arn, "tagKeys": tag_keys }),
        )
        .await;
        let got = tags_of(&harness, case, &arn).await;
        assert_eq!(
            got,
            seeded_map(),
            "case {case:?}: a refused removal must remove none of its keys, got {got:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// CreateBenefitApplication
// ---------------------------------------------------------------------------

/// A create supplies tag keys just as `TagResource` does, so it answers the same refusal
/// and mints nothing; a create whose keys are all the caller's own still round-trips
/// through `ListTagsForResource`.
#[tokio::test]
async fn create_refuses_a_reserved_key_and_mints_nothing() {
    let harness = Harness::new();
    let catalog = SEEDED_CATALOGS[0];
    let benefit = benefit_id(&harness, "create", catalog).await;

    let before = application_count(&harness, "create", catalog).await;
    for (index, row) in keys().into_iter().enumerate() {
        let case = format!("CreateBenefitApplication with {}", row.case);
        let token = format!("tok-reserved-create-{index}");
        let body = json!({
            "Catalog": catalog,
            "ClientToken": token,
            "BenefitIdentifier": benefit,
            "Name": token,
            "Tags": tag_payload(&[(row.key.as_str(), "mine"), ("env", "test")]),
        });

        if row.reserved {
            assert_reserved_refusal(&harness, &case, "CreateBenefitApplication", "Tags", &body)
                .await;
        } else {
            let created = ok(&harness, &case, "CreateBenefitApplication", &body).await;
            let arn = created["Arn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: the create reported no Arn: {created:?}"));
            let got = tags_of(&harness, &case, arn).await;
            let expected: BTreeMap<String, String> = [
                (row.key.clone(), "mine".to_string()),
                ("env".to_string(), "test".to_string()),
            ]
            .into_iter()
            .collect();
            assert_eq!(
                got, expected,
                "case {case:?}: expected the created tag set {expected:?}, got {got:?}"
            );
        }
    }

    // Exactly the admissible rows created an application: a refused create mints nothing.
    let admissible = keys().iter().filter(|row| !row.reserved).count();
    let after = application_count(&harness, "create", catalog).await;
    assert_eq!(
        after,
        before + admissible,
        "case \"create\": expected {admissible} new applications, got {}",
        after - before
    );
}

/// The `ClientToken` of a refused create is NOT consumed: the create never happened, so the
/// same token still mints an application once the reserved key is gone.
#[tokio::test]
async fn a_refused_create_leaves_its_client_token_unused() {
    let harness = Harness::new();
    let catalog = SEEDED_CATALOGS[0];
    let benefit = benefit_id(&harness, "token", catalog).await;
    let token = "tok-reserved-create-retry";
    let mut body = json!({
        "Catalog": catalog,
        "ClientToken": token,
        "BenefitIdentifier": benefit,
        "Name": token,
        "Tags": tag_payload(&[("aws:createdBy", "mine")]),
    });

    assert_reserved_refusal(&harness, "token", "CreateBenefitApplication", "Tags", &body).await;

    body["Tags"] = tag_payload(&[("createdBy", "mine")]);
    let created = ok(&harness, "token", "CreateBenefitApplication", &body).await;
    let arn = created["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"token\": the create reported no Arn: {created:?}"));
    let got = tags_of(&harness, "token", arn).await;
    assert_eq!(
        got,
        [("createdBy".to_string(), "mine".to_string())]
            .into_iter()
            .collect::<BTreeMap<String, String>>(),
        "case \"token\": the retry must store exactly its own keys, got {got:?}"
    );
}

/// How many applications `catalog` holds, read over the wire.
async fn application_count(harness: &Harness, case: &str, catalog: &str) -> usize {
    let out = ok(
        harness,
        case,
        "ListBenefitApplications",
        &json!({ "Catalog": catalog, "MaxResults": 100 }),
    )
    .await;
    out["BenefitApplicationSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a summary list, got {out:?}"))
        .len()
}
