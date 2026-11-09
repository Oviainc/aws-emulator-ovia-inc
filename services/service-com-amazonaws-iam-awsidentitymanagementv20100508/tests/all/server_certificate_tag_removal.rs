// SPDX-License-Identifier: CC0-1.0

//! `UntagServerCertificate`, end to end over the awsQuery wire, read back through
//! `GetServerCertificate` and `ListServerCertificateTags`.
//!
//! RULE under test: an untag call drops from the named server certificate's PERSISTED tag
//! set exactly the tags whose keys it names, comparing keys exactly, and leaves every
//! other held tag with its value — so a named key the certificate does not hold removes
//! nothing and the call still succeeds, which makes it idempotent, and a removed key is
//! free to be tagged again. `UntagServerCertificate`'s own documentation: "removes the
//! specified tags from the IAM server certificate", with "the tags with matching keys are
//! removed from the specified IAM server certificate"
//! (`UntagServerCertificate$TagKeys`). The certificate is addressed by its IAM entity
//! name, which is unique within the account and not distinguished by case, and a name the
//! account holds no certificate under is refused with the modelled
//! `NoSuchEntityException`.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, XML members) of a certificate that was
//! PERSISTED rather than a handler called directly. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns, the key-list and quota fixtures are
//! `tests/all/role_tag_removal.rs`'s, the tag-list builders are `tests/all/role_tags.rs`'s,
//! the certificate upload and both tag readers are `tests/all/server_certificate_tag_updates.rs`'s
//! and the signing principal and its policy documents are
//! `tests/all/identity_policy_enforcement.rs`'s, so a request is built and read exactly one
//! way across this crate.
//!
//! The suites are tables whose rows differ along what the rule generalises over: the held
//! tag count, how many keys one call names, whether a named key is held or not, the case of
//! a key, the case the certificate's name is spelled in, the certificate's path, the number
//! of calls, the alphabet and length bounds `tagKeyType` publishes (1..128), and the
//! published per-resource tag quota.

use std::collections::BTreeMap;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::MAX_TAGS_PER_ENTITY;
use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::{principal, statement};
use crate::role_lifecycle::{borrowed, param};
use crate::role_tag_removal::{fixture, keys, with_key_params};
use crate::role_tags::{expected_map, tags};
use crate::server_certificate_tag_updates::{
    assert_reads, stored_tags, tag_certificate, upload_certificate,
};
use crate::server_certificates::expected_arn;
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call, call_signed_by, element, error_code, router,
};

/// One `UntagServerCertificate` call: the HTTP status and the XML body.
pub(crate) async fn untag_certificate(
    router: &MultiServiceRouter,
    name: &str,
    removed: &[String],
) -> (u16, String) {
    let params = with_key_params(vec![param("ServerCertificateName", name)], removed);
    call(router, "UntagServerCertificate", &borrowed(&params)).await
}

/// One `UntagServerCertificate` call presented as signed by `access_key_id`.
async fn untag_certificate_signed(
    router: &MultiServiceRouter,
    access_key_id: &str,
    name: &str,
    removed: &[String],
) -> (u16, String) {
    let params = with_key_params(vec![param("ServerCertificateName", name)], removed);
    call_signed_by(
        router,
        Some(access_key_id),
        "UntagServerCertificate",
        &borrowed(&params),
    )
    .await
}

/// One row: a certificate uploaded at `path` carrying `initial`, then one
/// `UntagServerCertificate` naming `remove`, then the tag set the certificate must hold.
struct RemoveCase {
    name: &'static str,
    /// `Path`, or `None` for an upload that omits it: an omitted path "defaults to a slash
    /// (/)" (`UploadServerCertificate$Path`). The path is part of the record the tags hang
    /// off, so rows span it.
    path: Option<&'static str>,
    initial: Vec<(String, String)>,
    remove: Vec<String>,
    expected: Vec<(String, String)>,
}

/// RULE: `UntagServerCertificate` drops exactly the held tags whose keys the request names
/// and leaves every other held tag untouched, so `GetServerCertificate` and
/// `ListServerCertificateTags` both report the remaining set. One row per thing the rule
/// generalises over: the held tag count, how many keys one call names, held versus unheld
/// keys, the case of a key, the empty tag value, the certificate's path, the published
/// key-length bound and alphabet, and the per-resource tag quota.
#[tokio::test]
async fn untagging_removes_exactly_the_named_keys() {
    // `tagKeyType` is length 1..128 (`smithy_query(query="shape", shape="Tag")`), so this
    // is the longest key a stored tag can be named by.
    let longest_key = "k".repeat(128);
    // `tagKeyType`'s pattern is `^[\p{L}\p{Z}\p{N}_.:/=+\-@]+$`, so spaces, non-ASCII
    // letters, digits and that class's punctuation all name tags.
    let alphabet_key = "Cost Center_1.2:3/4=5+6-7@8";
    let quota = MAX_TAGS_PER_ENTITY;

    let cases = vec![
        RemoveCase {
            name: "the only held tag is removed, leaving none",
            path: None,
            initial: tags(&[("Department", "Engineering")]),
            remove: keys(&["Department"]),
            expected: Vec::new(),
        },
        RemoveCase {
            name: "one of several held tags is removed",
            path: Some("/"),
            initial: tags(&[("team", "edge"), ("env", "prod"), ("owner", "ops")]),
            remove: keys(&["env"]),
            expected: tags(&[("team", "edge"), ("owner", "ops")]),
        },
        RemoveCase {
            name: "several held keys are removed in one call",
            path: Some("/cloudfront/"),
            initial: tags(&[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")]),
            remove: keys(&["a", "c"]),
            expected: tags(&[("b", "2"), ("d", "4")]),
        },
        RemoveCase {
            name: "every held key is removed in one call",
            path: Some("/division_abc/subdivision_xyz/"),
            initial: tags(&[("a", "1"), ("b", "2"), ("c", "3")]),
            remove: keys(&["a", "b", "c"]),
            expected: Vec::new(),
        },
        RemoveCase {
            name: "a key the certificate does not hold removes nothing",
            path: None,
            initial: tags(&[("team", "edge"), ("env", "prod")]),
            remove: keys(&["absent"]),
            expected: tags(&[("team", "edge"), ("env", "prod")]),
        },
        RemoveCase {
            name: "a held key named beside an unheld one",
            path: None,
            initial: tags(&[("team", "edge"), ("env", "prod")]),
            remove: keys(&["team", "absent"]),
            expected: tags(&[("env", "prod")]),
        },
        RemoveCase {
            name: "a key named on a certificate holding no tags",
            path: Some("/edge/"),
            initial: Vec::new(),
            remove: keys(&["team"]),
            expected: Vec::new(),
        },
        RemoveCase {
            // "Tag keys and values are case sensitive" ("Tagging IAM resources", IAM User
            // Guide), so a key spelled in another case names another tag.
            name: "keys differing only in case are distinct tags",
            path: None,
            initial: tags(&[("Env", "prod"), ("env", "dev"), ("ENV", "test")]),
            remove: keys(&["env"]),
            expected: tags(&[("Env", "prod"), ("ENV", "test")]),
        },
        RemoveCase {
            name: "a key whose case the certificate does not hold removes nothing",
            path: None,
            initial: tags(&[("Env", "prod")]),
            remove: keys(&["env", "ENV"]),
            expected: tags(&[("Env", "prod")]),
        },
        RemoveCase {
            name: "a tag carrying the empty value is removed by its key",
            path: None,
            initial: tags(&[("empty", ""), ("keep", "v")]),
            remove: keys(&["empty"]),
            expected: tags(&[("keep", "v")]),
        },
        RemoveCase {
            name: "a key at the published maximum length",
            path: None,
            initial: tags(&[(longest_key.as_str(), "fits"), ("team", "edge")]),
            remove: keys(&[longest_key.as_str()]),
            expected: tags(&[("team", "edge")]),
        },
        RemoveCase {
            name: "a key at the published minimum length",
            path: None,
            initial: tags(&[("k", "short"), ("team", "edge")]),
            remove: keys(&["k"]),
            expected: tags(&[("team", "edge")]),
        },
        RemoveCase {
            name: "a key spanning the modelled alphabet",
            path: None,
            initial: tags(&[(alphabet_key, "x"), ("keep", "y")]),
            remove: keys(&[alphabet_key]),
            expected: tags(&[("keep", "y")]),
        },
        RemoveCase {
            name: "one key removed from a certificate at the tag quota",
            path: None,
            initial: fixture(quota),
            remove: keys(&["key-00"]),
            expected: fixture(quota)[1..].to_vec(),
        },
        RemoveCase {
            // `tagKeyListType` is length 0..50, and the quota is 50 tags per resource, so
            // this names the most keys one call can carry.
            name: "every key of a certificate at the tag quota removed in one call",
            path: None,
            initial: fixture(quota),
            remove: fixture(quota).iter().map(|(key, _)| key.clone()).collect(),
            expected: Vec::new(),
        },
    ];

    let router = router();
    for (index, case) in cases.iter().enumerate() {
        let certificate_name = format!("remove-{index}");
        let name = case.name;
        upload_certificate(&router, &certificate_name, case.path, &case.initial).await;

        let (status, body) = untag_certificate(&router, &certificate_name, &case.remove).await;
        assert_eq!(
            status, 200,
            "case {name:?}: UntagServerCertificate must succeed, got {status}: {body}"
        );

        // Both readers of the stored set must report the survivors, sorted by tag key.
        assert_reads(
            &router,
            name,
            &certificate_name,
            &expected_map(&case.expected),
        )
        .await;
    }
}

/// RULE: removing a key the certificate does not hold removes nothing and is not an error,
/// so repeating an untag call leaves the same tag set however many times it is made.
#[tokio::test]
async fn repeating_an_untag_call_changes_nothing_further() {
    let held = tags(&[("team", "edge"), ("env", "prod"), ("owner", "ops")]);
    let repeated = [
        (
            "one held key",
            keys(&["env"]),
            tags(&[("team", "edge"), ("owner", "ops")]),
        ),
        (
            "two held keys",
            keys(&["env", "owner"]),
            tags(&[("team", "edge")]),
        ),
        ("only unheld keys", keys(&["absent", "other"]), held.clone()),
        ("every held key", keys(&["team", "env", "owner"]), Vec::new()),
    ];

    let router = router();
    for (index, (case, removed, expected)) in repeated.iter().enumerate() {
        let certificate_name = format!("repeat-{index}");
        upload_certificate(&router, &certificate_name, None, &held).await;

        // Three identical calls: the first may change the set, the rest must not.
        for attempt in 0..3 {
            let (status, body) = untag_certificate(&router, &certificate_name, removed).await;
            assert_eq!(
                status, 200,
                "case {case:?}: attempt {attempt} must succeed, got {status}: {body}"
            );
            assert_reads(
                &router,
                &format!("{case} attempt {attempt}"),
                &certificate_name,
                &expected_map(expected),
            )
            .await;
        }
    }
}

/// RULE: an untag call names its certificate by an IAM entity name, which is unique within
/// the account and "not distinguished by case"
/// (`UploadServerCertificate$ServerCertificateName`), so any casing of a held name untags
/// the SAME certificate — and a name the account holds no certificate under is refused with
/// the modelled `NoSuchEntityException`, which changes nothing a later caller sees.
#[tokio::test]
async fn the_certificate_is_found_by_its_case_folded_name() {
    let router = router();
    let held: Vec<(String, String)> = (0..4)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    upload_certificate(&router, "MyCertificate", None, &held).await;

    // Each spelling untags the one held certificate, so the set shrinks across them
    // instead of each spelling reaching a certificate of its own.
    for (index, spelling) in [
        "MyCertificate",
        "mycertificate",
        "MYCERTIFICATE",
        "mYcErTiFiCaTe",
    ]
    .iter()
    .enumerate()
    {
        let (status, body) =
            untag_certificate(&router, spelling, &keys(&[&format!("key-{index}")])).await;
        assert_eq!(
            status, 200,
            "case {spelling:?}: expected 200 for a held name, got {status}: {body}"
        );
        let expected: BTreeMap<String, String> = held[index + 1..].iter().cloned().collect();
        let (reported, reported_keys) = stored_tags(&router, spelling).await;
        assert_eq!(
            reported, expected,
            "case {spelling:?}: expected the remaining tag set {expected:?}, got {reported:?}"
        );
        assert_eq!(
            reported_keys.len(),
            expected.len(),
            "case {spelling:?}: expected {} stored tags, got {reported_keys:?}",
            expected.len()
        );
    }

    // A name the account holds no certificate under is the modelled absence, worded as
    // every operation naming a server certificate by name words it.
    for absent in [
        "OtherCertificate",
        "mycertificate2",
        "my_certificate",
        "A+B=C,d@e-f_g",
    ] {
        let (status, body) = untag_certificate(&router, absent, &keys(&["team"])).await;
        assert_eq!(
            status, 404,
            "case {absent:?}: expected 404 for a name the account does not hold, got {status}: \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {absent:?}: expected the modelled awsQuery code, got {body}"
        );
        assert_eq!(
            element(&body, "Message"),
            format!("The Server Certificate with name {absent} cannot be found."),
            "case {absent:?}: expected the message to name the certificate, got {body}"
        );
    }
}

/// One row: a certificate uploaded with `initial`, then an `UntagServerCertificate` naming
/// `removed`, then a `TagServerCertificate` supplying `re_tagged`.
struct ReTagCase {
    name: &'static str,
    initial: Vec<(String, String)>,
    removed: Vec<String>,
    re_tagged: Vec<(String, String)>,
}

/// RULE: an untagged key is simply not held any more, so it can be tagged again — and
/// because the per-resource tag quota is taken against the set the certificate actually
/// holds, removing a tag frees room inside it.
#[tokio::test]
async fn a_removed_key_can_be_tagged_again() {
    let quota = MAX_TAGS_PER_ENTITY;

    // One row per starting point: a certificate holding one tag, one holding several, and
    // one at the published quota, where the re-tag only fits because a key was removed.
    let rows = [
        ReTagCase {
            name: "a certificate holding one tag",
            initial: tags(&[("env", "prod")]),
            removed: keys(&["env"]),
            re_tagged: tags(&[("env", "staging")]),
        },
        ReTagCase {
            name: "a certificate holding several tags",
            initial: tags(&[("a", "1"), ("b", "2")]),
            removed: keys(&["a", "b"]),
            re_tagged: tags(&[("a", "one"), ("c", "3")]),
        },
        ReTagCase {
            name: "a certificate at the published tag quota",
            initial: fixture(quota),
            removed: keys(&["key-00", "key-01"]),
            re_tagged: tags(&[("new-a", "a"), ("new-b", "b")]),
        },
    ];

    let router = router();
    for (index, case) in rows.iter().enumerate() {
        let certificate_name = format!("retag-{index}");
        let name = case.name;
        upload_certificate(&router, &certificate_name, None, &case.initial).await;

        let (status, body) = untag_certificate(&router, &certificate_name, &case.removed).await;
        assert_eq!(
            status, 200,
            "case {name:?}: UntagServerCertificate must succeed, got {status}: {body}"
        );

        let (status, body) = tag_certificate(&router, &certificate_name, &case.re_tagged).await;
        assert_eq!(
            status, 200,
            "case {name:?}: re-tagging a removed key must succeed, got {status}: {body}"
        );

        // What the certificate holds now: the initial set less the removed keys, plus the
        // re-tagged ones.
        let mut expected: BTreeMap<String, String> = expected_map(&case.initial);
        for key in &case.removed {
            expected.remove(key);
        }
        expected.extend(case.re_tagged.iter().cloned());
        assert_reads(&router, name, &certificate_name, &expected).await;

        // The room the removal freed is all there is: one more new key than was removed
        // passes the quota, so the service must refuse it and keep the set it reports.
        let one_too_many = tags(&[("over-the-quota", "x")]);
        let (status, body) = tag_certificate(&router, &certificate_name, &one_too_many).await;
        let expected_status = if expected.len() < quota { 200 } else { 409 };
        assert_eq!(
            status, expected_status,
            "case {name:?}: a certificate holding {} of {quota} tags must answer \
             {expected_status} to one more key, got {status}: {body}",
            expected.len()
        );
        if status == 200 {
            expected.extend(one_too_many.iter().cloned());
        }
        assert_reads(&router, name, &certificate_name, &expected).await;
    }
}

/// What a signed row expects the call to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decided {
    /// HTTP 200: the policies in force permit the untagging.
    Served,
    /// HTTP 403 `AccessDenied` because a statement about the request says `Deny`.
    ExplicitDeny,
    /// HTTP 403 `AccessDenied` because no statement about the request says `Allow`.
    NoAllow,
}

/// One row of the signed table: the case's name, the identity-based policy documents in
/// force on its caller as `(PolicyName, PolicyDocument)` pairs, and what the call must
/// answer.
type SignedRow = (&'static str, Vec<(String, String)>, Decided);

/// RULE: IAM authorizes its own API, so a signed untag call is decided against the signing
/// principal's identity-based policies before anything is removed — a `Deny` about the
/// request refuses it whatever any `Allow` says, an `Allow` with no such `Deny` serves it,
/// and a principal with neither is refused with HTTP 403 `AccessDenied` naming the
/// principal, the action and the certificate the decision was made on, with nothing
/// removed.
#[tokio::test]
async fn a_signed_untag_is_decided_against_the_callers_identity_policies() {
    let held = tags(&[("team", "edge"), ("env", "prod")]);
    let removed = keys(&["env"]);
    // The ARN the decision is made on: the one the certificate every row uploads carries.
    let certificate_arn = expected_arn("/", "signed-cert");

    let rows: Vec<SignedRow> = vec![
        (
            "no identity-based policy at all",
            Vec::new(),
            Decided::NoAllow,
        ),
        (
            "an Allow naming the action on every resource",
            vec![(
                "allow-untag".to_string(),
                statement("Allow", "iam:UntagServerCertificate", "*"),
            )],
            Decided::Served,
        ),
        (
            "an Allow scoped to the certificate the request addresses",
            vec![(
                "allow-this-cert".to_string(),
                statement("Allow", "iam:UntagServerCertificate", &certificate_arn),
            )],
            Decided::Served,
        ),
        (
            "an Allow for the tag action only, not the untag action",
            vec![(
                "allow-tag".to_string(),
                statement("Allow", "iam:TagServerCertificate", "*"),
            )],
            Decided::NoAllow,
        ),
        (
            "an Allow scoped to another certificate's ARN",
            vec![(
                "allow-other-cert".to_string(),
                statement(
                    "Allow",
                    "iam:UntagServerCertificate",
                    &expected_arn("/", "some-other-cert"),
                ),
            )],
            Decided::NoAllow,
        ),
        (
            "a Deny naming the action beside an Allow",
            vec![
                (
                    "allow-untag".to_string(),
                    statement("Allow", "iam:UntagServerCertificate", "*"),
                ),
                (
                    "deny-untag".to_string(),
                    statement("Deny", "iam:UntagServerCertificate", "*"),
                ),
            ],
            Decided::ExplicitDeny,
        ),
    ];

    for (index, (case, policies, expect)) in rows.iter().enumerate() {
        // Every row drives its own engine, so the certificate each one untags carries only
        // the tags its own upload stored.
        let router = router();
        upload_certificate(&router, "signed-cert", None, &held).await;
        let user_name = format!("untagger{index}");
        let access_key_id = principal(&router, case, &user_name, policies).await;

        let (status, body) =
            untag_certificate_signed(&router, &access_key_id, "signed-cert", &removed).await;

        match expect {
            Decided::Served => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the call to be served, got {status}: {body}"
                );
                assert_reads(
                    &router,
                    case,
                    "signed-cert",
                    &expected_map(&tags(&[("team", "edge")])),
                )
                .await;
            }
            Decided::ExplicitDeny | Decided::NoAllow => {
                assert_eq!(
                    status, 403,
                    "case {case:?}: expected HTTP 403, got {status}: {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "AccessDenied",
                    "case {case:?}: expected the error code AccessDenied, got {body}"
                );
                let reason = if *expect == Decided::ExplicitDeny {
                    "with an explicit deny in an identity-based policy".to_string()
                } else {
                    "because no identity-based policy allows the iam:UntagServerCertificate \
                     action"
                        .to_string()
                };
                assert_access_denied_message(
                    case,
                    &element(&body, "Message"),
                    &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
                    "iam:UntagServerCertificate",
                    "server certificate signed-cert",
                    &reason,
                );
                // A refused call removes nothing.
                assert_reads(&router, case, "signed-cert", &expected_map(&held)).await;
            }
        }
    }
}
