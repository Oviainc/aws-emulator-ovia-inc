// SPDX-License-Identifier: CC0-1.0

//! `TagServerCertificate`, end to end over the awsQuery wire, read back through
//! `GetServerCertificate` and `ListServerCertificateTags`.
//!
//! RULE under test: a tag call MERGES the supplied tags into the tag set the named server
//! certificate persists — a supplied key the certificate already holds has its value
//! overwritten and a key it does not hold is added — bounded by the per-resource tag
//! quota, with a request carrying any invalid tag storing nothing; and the merged set is
//! what every later read reports, sorted by tag key. `TagServerCertificate`'s own
//! documentation: "if a tag with the same key name already exists, then that tag is
//! overwritten with the new value", and "if any one of the tags is invalid or if you
//! exceed the allowed maximum number of tags, then the entire request fails";
//! `UploadServerCertificate$Tags`: "the returned list of tags is sorted by tag key".
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, XML members) of a certificate that was
//! PERSISTED rather than a handler called directly. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns, the parameter and tag readers are
//! `tests/all/role_lifecycle.rs`'s, the tag-list builders are `tests/all/role_tags.rs`'s,
//! the signing principal and its policy documents are
//! `tests/all/identity_policy_enforcement.rs`'s and the real key pair and self-signed
//! certificate an upload needs come from `tests/all/server_certificates.rs`, so a request
//! is built and read exactly one way across this crate.
//!
//! The suites are tables whose rows differ along what the rule generalises over: the held
//! tag count, the number of calls, whether a supplied key is new or held, the case of a
//! key, keys repeated inside one request, the certificate's path, the case the addressed
//! name is spelled in, the alphabet and the published length bounds of `tagKeyType`
//! (1..128) and `tagValueType` (0..256), and the published per-resource tag quota.

use std::collections::BTreeMap;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::MAX_TAGS_PER_ENTITY;
use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::{principal, statement};
use crate::role_lifecycle::{borrowed, param, reported_tags};
// `fixture` is the quota fixture `tests/all/role_tag_removal.rs` exports for every tag
// suite in this crate, so the rows here are built from the one definition of it.
use crate::role_tag_removal::fixture;
use crate::role_tags::{expected_map, tags, with_tag_params};
use crate::server_certificates::{expected_arn, material};
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call, call_signed_by, element, elements, error_code,
    router,
};

/// Upload the certificate `name` at `path` carrying `initial`, so a tag call has a
/// persisted record to merge into. A real key pair and a real self-signed certificate are
/// generated per upload, so nothing here depends on a canned PEM.
async fn upload_certificate(
    router: &MultiServiceRouter,
    name: &str,
    path: Option<&str>,
    initial: &[(String, String)],
) {
    let leaf = material(
        &rcgen::PKCS_ECDSA_P256_SHA256,
        (2020, 2040),
        "server.example.com",
    );
    let mut leading = vec![
        param("ServerCertificateName", name),
        param("CertificateBody", &leaf.certificate),
        param("PrivateKey", &leaf.private_key),
    ];
    if let Some(path) = path {
        leading.push(param("Path", path));
    }
    let params = with_tag_params(leading, initial);
    let (status, body) = call(router, "UploadServerCertificate", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {name:?}: the fixture UploadServerCertificate must succeed, got {status}: \
         {body}"
    );
}

/// One `TagServerCertificate` call: the HTTP status and the XML body.
async fn tag_certificate(
    router: &MultiServiceRouter,
    name: &str,
    list: &[(String, String)],
) -> (u16, String) {
    let params = with_tag_params(vec![param("ServerCertificateName", name)], list);
    call(router, "TagServerCertificate", &borrowed(&params)).await
}

/// One `TagServerCertificate` call presented as signed by `access_key_id`.
async fn tag_certificate_signed(
    router: &MultiServiceRouter,
    access_key_id: &str,
    name: &str,
    list: &[(String, String)],
) -> (u16, String) {
    let params = with_tag_params(vec![param("ServerCertificateName", name)], list);
    call_signed_by(
        router,
        Some(access_key_id),
        "TagServerCertificate",
        &borrowed(&params),
    )
    .await
}

/// The tag set `GetServerCertificate` reports for `name`, as a map, plus the tag keys in
/// the order the response carried them — so a duplicated key cannot hide inside a map and
/// the reported ORDER is assertable.
async fn stored_tags(
    router: &MultiServiceRouter,
    name: &str,
) -> (BTreeMap<String, String>, Vec<String>) {
    let (status, body) = call(
        router,
        "GetServerCertificate",
        &[("ServerCertificateName", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: GetServerCertificate must report the tagged certificate, got \
         {status}: {body}"
    );
    (reported_tags(&body), elements(&body, "Key"))
}

/// The tag set `ListServerCertificateTags` reports for `name` — the other reader of the
/// same stored set, so both halves of the round trip are asserted on every row.
async fn listed_tags(
    router: &MultiServiceRouter,
    name: &str,
) -> (BTreeMap<String, String>, Vec<String>) {
    let (status, body) = call(
        router,
        "ListServerCertificateTags",
        &[("ServerCertificateName", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: ListServerCertificateTags must report the tagged certificate, got \
         {status}: {body}"
    );
    (reported_tags(&body), elements(&body, "Key"))
}

/// Assert both readers report `expected`, sorted by tag key.
async fn assert_reads(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    expected: &BTreeMap<String, String>,
) {
    let sorted: Vec<String> = expected.keys().cloned().collect();

    let (reported, reported_keys) = stored_tags(router, name).await;
    assert_eq!(
        &reported, expected,
        "case {case:?}: expected GetServerCertificate to report the tag set {expected:?}, \
         got {reported:?}"
    );
    assert_eq!(
        reported_keys, sorted,
        "case {case:?}: expected the reported keys sorted by key {sorted:?}, got \
         {reported_keys:?}"
    );

    let (listed, listed_keys) = listed_tags(router, name).await;
    assert_eq!(
        &listed, expected,
        "case {case:?}: expected ListServerCertificateTags to report the tag set \
         {expected:?}, got {listed:?}"
    );
    assert_eq!(
        listed_keys, sorted,
        "case {case:?}: expected the listed keys sorted by key {sorted:?}, got \
         {listed_keys:?}"
    );
}

/// One row: a certificate uploaded at `path` carrying `initial`, then `calls` successive
/// `TagServerCertificate` requests, then the tag set the certificate must hold.
struct MergeCase {
    name: &'static str,
    /// `Path`, or `None` for an upload that omits it: an omitted path "defaults to a slash
    /// (/)" (`UploadServerCertificate$Path`). The path is part of the record the tags hang
    /// off, so rows span it.
    path: Option<&'static str>,
    initial: Vec<(String, String)>,
    calls: Vec<Vec<(String, String)>>,
    expected: Vec<(String, String)>,
}

/// RULE: `TagServerCertificate` merges its tags into the certificate's stored set — a held
/// key is overwritten, a new key is added — and every read reports exactly that merged
/// set, sorted by tag key. One row per thing the rule generalises over: the held tag
/// count, the number of calls, new versus held keys, the case of a key, a key repeated
/// inside one request, the certificate's path, and the published length bounds of
/// `tagKeyType` and `tagValueType`.
#[tokio::test]
async fn tagging_merges_its_tags_into_the_persisted_set() {
    // `tagValueType` is length 0..256 and `tagKeyType` length 1..128
    // (`smithy_query(query="shape", shape="tagKeyType,tagValueType")`), so these are the
    // published bounds a stored tag must survive.
    let longest_value = "v".repeat(256);
    let longest_key = "k".repeat(128);

    let cases = vec![
        MergeCase {
            name: "a certificate holding no tags gains one",
            path: None,
            initial: Vec::new(),
            calls: vec![tags(&[("Department", "Engineering")])],
            expected: tags(&[("Department", "Engineering")]),
        },
        MergeCase {
            name: "a new key is added beside the held ones",
            path: Some("/"),
            initial: tags(&[("team", "edge"), ("env", "prod")]),
            calls: vec![tags(&[("owner", "ops")])],
            expected: tags(&[("team", "edge"), ("env", "prod"), ("owner", "ops")]),
        },
        MergeCase {
            name: "a held key is overwritten rather than duplicated",
            path: Some("/cloudfront/"),
            initial: tags(&[("team", "edge"), ("env", "prod")]),
            calls: vec![tags(&[("env", "staging")])],
            expected: tags(&[("team", "edge"), ("env", "staging")]),
        },
        MergeCase {
            name: "one call both overwrites and adds",
            path: Some("/division_abc/subdivision_xyz/"),
            initial: tags(&[("a", "1"), ("b", "2")]),
            calls: vec![tags(&[("b", "two"), ("c", "3")])],
            expected: tags(&[("a", "1"), ("b", "two"), ("c", "3")]),
        },
        MergeCase {
            name: "successive calls accumulate, the last value winning",
            path: None,
            initial: Vec::new(),
            calls: vec![
                tags(&[("k1", "v1")]),
                tags(&[("k2", "v2")]),
                tags(&[("k1", "v1-again")]),
            ],
            expected: tags(&[("k1", "v1-again"), ("k2", "v2")]),
        },
        MergeCase {
            // "Tag keys and values are case sensitive" ("Tagging IAM resources", IAM User
            // Guide), so two spellings of one word are two tags.
            name: "keys differing only in case are distinct tags",
            path: None,
            initial: tags(&[("Env", "prod")]),
            calls: vec![tags(&[("env", "dev"), ("ENV", "test")])],
            expected: tags(&[("Env", "prod"), ("env", "dev"), ("ENV", "test")]),
        },
        MergeCase {
            name: "a key repeated inside one request settles on its last value",
            path: None,
            initial: Vec::new(),
            calls: vec![tags(&[("dup", "first"), ("dup", "last")])],
            expected: tags(&[("dup", "last")]),
        },
        MergeCase {
            name: "a value at the published minimum length",
            path: Some("/edge/"),
            initial: tags(&[("team", "edge")]),
            calls: vec![tags(&[("empty", "")])],
            expected: tags(&[("team", "edge"), ("empty", "")]),
        },
        MergeCase {
            name: "a held value overwritten with the empty value",
            path: None,
            initial: tags(&[("env", "prod")]),
            calls: vec![tags(&[("env", "")])],
            expected: tags(&[("env", "")]),
        },
        MergeCase {
            name: "a value at the published maximum length",
            path: None,
            initial: Vec::new(),
            calls: vec![tags(&[("long", longest_value.as_str())])],
            expected: tags(&[("long", longest_value.as_str())]),
        },
        MergeCase {
            name: "a key at the published maximum length",
            path: None,
            initial: Vec::new(),
            calls: vec![tags(&[(longest_key.as_str(), "fits")])],
            expected: tags(&[(longest_key.as_str(), "fits")]),
        },
        MergeCase {
            // `tagKeyType`'s pattern is `^[\p{L}\p{Z}\p{N}_.:/=+\-@]+$` and
            // `tagValueType`'s the same with `*`, so spaces, non-ASCII letters, digits and
            // every punctuation character of that class belong to the class the rule
            // covers.
            name: "a key and a value spanning the modelled alphabet",
            path: None,
            initial: Vec::new(),
            calls: vec![tags(&[("Cost Center_1.2:3/4=5+6-7@8", "Ünité 9 / A=B+C-D@E")])],
            expected: tags(&[("Cost Center_1.2:3/4=5+6-7@8", "Ünité 9 / A=B+C-D@E")]),
        },
        MergeCase {
            // The whole tag set a certificate may carry, arriving in one call on a
            // certificate that holds none.
            name: "the published maximum number of tags in one call",
            path: None,
            initial: Vec::new(),
            calls: vec![fixture(MAX_TAGS_PER_ENTITY)],
            expected: fixture(MAX_TAGS_PER_ENTITY),
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        // One certificate per row on its own engine, because an account holds a bounded
        // number of server certificates.
        let router = router();
        let name = format!("merge-{index}");
        upload_certificate(&router, &name, case.path, &case.initial).await;

        for (call_index, list) in case.calls.iter().enumerate() {
            let (status, body) = tag_certificate(&router, &name, list).await;
            assert_eq!(
                status, 200,
                "case {:?}: TagServerCertificate call {call_index} must succeed, got \
                 {status}: {body}",
                case.name
            );
        }

        assert_reads(&router, case.name, &name, &expected_map(&case.expected)).await;
    }
}

/// RULE: a certificate is addressed by the name the account holds it under and IAM entity
/// names "are not distinguished by case", so any spelling of a held name tags that one
/// certificate — and the tags it leaves behind are reported for every spelling too.
#[tokio::test]
async fn a_held_name_is_tagged_without_regard_to_case() {
    let router = router();
    let held = tags(&[("team", "edge")]);
    let name = "Mixed_Case.Cert+1@example";
    upload_certificate(&router, name, Some("/edge/"), &held).await;

    let mut expected = expected_map(&held);
    for (index, spelling) in [name.to_uppercase(), name.to_lowercase(), name.to_string()]
        .into_iter()
        .enumerate()
    {
        let added = tags(&[(&format!("spelling-{index}"), spelling.as_str())]);
        let (status, body) = tag_certificate(&router, &spelling, &added).await;
        assert_eq!(
            status, 200,
            "case {spelling:?}: every spelling of a held name must tag that certificate, \
             got {status}: {body}"
        );
        expected.extend(added);
        // Whatever spelling the call named, the ONE record carries the merged set, read
        // back under the name the upload stored it as.
        assert_reads(&router, &spelling, name, &expected).await;
    }
}

/// RULE: the `aws:` tag-key prefix is reserved ("You cannot create tag keys that begin
/// with `aws:`", "Tagging IAM resources"), so a tag request naming one is refused with the
/// modelled `InvalidInputException` (HTTP 400) and the ENTIRE request is dropped — no tag
/// of it reaches the certificate.
#[tokio::test]
async fn a_reserved_tag_key_is_refused_and_stores_nothing() {
    let held = tags(&[("team", "edge")]);
    let refused = [
        ("the prefix alone", tags(&[("aws:", "x")])),
        (
            "a reserved key",
            tags(&[("aws:cloudformation:stack-name", "stack")]),
        ),
        // The prefix is reserved however it is capitalised.
        ("an upper-case prefix", tags(&[("AWS:Owner", "aws")])),
        ("a mixed-case prefix", tags(&[("Aws:Owner", "aws")])),
        (
            "a valid tag beside a reserved one",
            tags(&[("owner", "ops"), ("aws:service", "iam")]),
        ),
        (
            "a reserved key before a valid one",
            tags(&[("aws:service", "iam"), ("owner", "ops")]),
        ),
    ];

    let router = router();
    for (index, (case, list)) in refused.iter().enumerate() {
        let name = format!("reserved-{index}");
        upload_certificate(&router, &name, None, &held).await;

        let (status, body) = tag_certificate(&router, &name, list).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a reserved tag key, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );

        // The whole request failed, so the certificate still holds exactly what its upload
        // stored: neither the reserved key nor the valid one beside it.
        assert_reads(&router, case, &name, &expected_map(&held)).await;
    }
}

/// RULE: a resource carries at most [`MAX_TAGS_PER_ENTITY`] tags (AWS's "IAM and STS
/// quotas": "Number of tags per IAM resource: 50"), and the bound is taken against the
/// MERGED set — so overwriting a held key is never refused, while a call that would carry
/// the certificate past the bound is refused with the modelled `LimitExceededException`
/// (HTTP 409) and stores nothing.
#[tokio::test]
async fn the_tag_bound_is_taken_against_the_merged_set() {
    let bound = MAX_TAGS_PER_ENTITY;

    // One row per position relative to the published bound: just inside it, at it, and the
    // first call that would pass it.
    let accepted = [
        (
            "one below the bound, one new key",
            bound - 1,
            tags(&[("new-a", "a")]),
        ),
        (
            "two below the bound, two new keys",
            bound - 2,
            tags(&[("new-a", "a"), ("new-b", "b")]),
        ),
        (
            "at the bound, overwriting a held key",
            bound,
            tags(&[("key-00", "overwritten")]),
        ),
        (
            "at the bound, overwriting several held keys",
            bound,
            tags(&[("key-01", "one"), ("key-02", "two")]),
        ),
    ];

    let router = router();
    for (index, (case, held_count, list)) in accepted.iter().enumerate() {
        let held = fixture(*held_count);
        let name = format!("bound-ok-{index}");
        upload_certificate(&router, &name, None, &held).await;

        let (status, body) = tag_certificate(&router, &name, list).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 inside the tag bound, got {status}: {body}"
        );

        let mut expected = expected_map(&held);
        expected.extend(list.iter().cloned());
        assert!(
            expected.len() <= bound,
            "case {case:?}: a certificate must never hold more than {bound} tags, got {}",
            expected.len()
        );
        assert_reads(&router, case, &name, &expected).await;
    }

    let refused = [
        ("at the bound, one new key", bound, 1usize),
        ("one below the bound, two new keys", bound - 1, 2),
        ("two below the bound, three new keys", bound - 2, 3),
    ];

    for (index, (case, held_count, new_keys)) in refused.iter().enumerate() {
        let held = fixture(*held_count);
        let name = format!("bound-refused-{index}");
        upload_certificate(&router, &name, None, &held).await;

        let list: Vec<(String, String)> = (0..*new_keys)
            .map(|position| (format!("new-{position}"), format!("value-{position}")))
            .collect();
        let (status, body) = tag_certificate(&router, &name, &list).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409 past the tag bound, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "LimitExceeded",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );
        // The message names the limit the request exceeded and the certificate it was
        // taken against.
        assert_eq!(
            element(&body, "Message"),
            format!("Cannot assign more than {bound} tags to IAM server certificate {name}"),
            "case {case:?}: expected the refusal to name the quota and the certificate, \
             got {body}"
        );

        // Nothing of the refused request was stored.
        assert_reads(&router, case, &name, &expected_map(&held)).await;
    }
}

/// RULE: a name the account holds no certificate under "referenced a resource entity that
/// does not exist" — HTTP 404 `NoSuchEntity`, never an empty success — and the refusal
/// changes nothing a later caller sees.
#[tokio::test]
async fn tagging_a_certificate_that_was_never_uploaded_is_refused() {
    let router = router();
    let held = tags(&[("team", "edge")]);
    upload_certificate(&router, "held-cert", None, &held).await;
    let added = tags(&[("owner", "ops")]);

    for name in ["never-uploaded", "held-cert.other", "A+B=C,d@e-f_g"] {
        let (status, body) = tag_certificate(&router, name, &added).await;
        assert_eq!(
            status, 404,
            "case {name:?}: expected HTTP 404 for a name the account holds no certificate \
             under, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {name:?}: expected NoSuchEntity, got {body}"
        );
        // IAM names the certificate the request named, so the message's variable part is
        // that name and its fixed text does not depend on the row.
        assert_eq!(
            element(&body, "Message"),
            format!("The Server Certificate with name {name} cannot be found."),
            "case {name:?}: expected the message to name the certificate, got {body}"
        );
    }

    // Every refusal above left the held certificate's tags exactly as its upload stored
    // them.
    assert_reads(&router, "held-cert", "held-cert", &expected_map(&held)).await;
}

/// RULE: `Tags` is a REQUIRED member (`smithy_query(query="operation",
/// operation="TagServerCertificate")`), so a request carrying none of its members is
/// refused before the operation runs and changes nothing.
#[tokio::test]
async fn a_required_member_absent_is_refused_before_the_operation_runs() {
    let router = router();
    let held = tags(&[("team", "edge")]);
    upload_certificate(&router, "required-members", None, &held).await;

    let (status, body) = call(
        &router,
        "TagServerCertificate",
        &[("ServerCertificateName", "required-members")],
    )
    .await;
    assert_eq!(
        status, 400,
        "a request carrying no tag list must be refused with HTTP 400, got {status}: {body}"
    );

    assert_reads(
        &router,
        "required-members",
        "required-members",
        &expected_map(&held),
    )
    .await;
}

/// What a signed row expects the call to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decided {
    /// HTTP 200: the policies in force permit the tagging.
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

/// RULE: IAM authorizes its own API, so a signed tag call is decided against the signing
/// principal's identity-based policies before anything is stored — a `Deny` about the
/// request refuses it whatever any `Allow` says, an `Allow` with no such `Deny` serves it,
/// and a principal with neither is refused with HTTP 403 `AccessDenied` naming the
/// principal, the action and the certificate the decision was made on, with nothing
/// stored.
#[tokio::test]
async fn a_signed_tag_is_decided_against_the_callers_identity_policies() {
    let held = tags(&[("team", "edge")]);
    let added = tags(&[("env", "prod"), ("team", "platform")]);
    // The ARN the decision is made on: the one the certificate every row uploads carries,
    // spelled through the grammar reader this crate's test tree shares.
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
                "allow-tag".to_string(),
                statement("Allow", "iam:TagServerCertificate", "*"),
            )],
            Decided::Served,
        ),
        (
            "an Allow scoped to the certificate the request addresses",
            vec![(
                "allow-this-cert".to_string(),
                statement("Allow", "iam:TagServerCertificate", &certificate_arn),
            )],
            Decided::Served,
        ),
        (
            "an Allow whose action element is a wildcard run",
            vec![(
                "allow-tag-star".to_string(),
                statement("Allow", "iam:Tag*", "*"),
            )],
            Decided::Served,
        ),
        (
            "an Allow for another server-certificate action only",
            vec![(
                "allow-update".to_string(),
                statement("Allow", "iam:UpdateServerCertificate", "*"),
            )],
            Decided::NoAllow,
        ),
        (
            "an Allow scoped to another certificate's ARN",
            vec![(
                "allow-other-cert".to_string(),
                statement(
                    "Allow",
                    "iam:TagServerCertificate",
                    &expected_arn("/", "some-other-cert"),
                ),
            )],
            Decided::NoAllow,
        ),
        (
            "a Deny naming the action beside an Allow",
            vec![
                (
                    "allow-tag".to_string(),
                    statement("Allow", "iam:TagServerCertificate", "*"),
                ),
                (
                    "deny-tag".to_string(),
                    statement("Deny", "iam:TagServerCertificate", "*"),
                ),
            ],
            Decided::ExplicitDeny,
        ),
        (
            "a Deny scoped to the certificate the request addresses",
            vec![
                (
                    "allow-tag".to_string(),
                    statement("Allow", "iam:TagServerCertificate", "*"),
                ),
                (
                    "deny-this-cert".to_string(),
                    statement("Deny", "iam:TagServerCertificate", &certificate_arn),
                ),
            ],
            Decided::ExplicitDeny,
        ),
    ];

    for (index, (case, policies, expect)) in rows.iter().enumerate() {
        // Every row drives its own engine, so the certificate each one tags carries only
        // the tags its own upload stored.
        let router = router();
        upload_certificate(&router, "signed-cert", None, &held).await;
        let user_name = format!("tagger{index}");
        let access_key_id = principal(&router, case, &user_name, policies).await;

        let (status, body) =
            tag_certificate_signed(&router, &access_key_id, "signed-cert", &added).await;

        match expect {
            Decided::Served => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the call to be served, got {status}: {body}"
                );
                // The merged set: the held tag whose key the request overwrote, plus the
                // new one.
                let mut expected = expected_map(&held);
                expected.extend(added.iter().cloned());
                assert_reads(&router, case, "signed-cert", &expected).await;
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
                    "because no identity-based policy allows the iam:TagServerCertificate \
                     action"
                        .to_string()
                };
                assert_access_denied_message(
                    case,
                    &element(&body, "Message"),
                    &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
                    "iam:TagServerCertificate",
                    "server certificate signed-cert",
                    &reason,
                );
                // A refused call stores nothing.
                assert_reads(&router, case, "signed-cert", &expected_map(&held)).await;
            }
        }
    }
}
