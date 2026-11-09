// SPDX-License-Identifier: CC0-1.0

//! What `ListServerCertificateTags` reports after an `UntagServerCertificate`, end to end
//! over the awsQuery wire.
//!
//! RULE under test: the removal an untag call asks for is PERSISTED, so the `Tags` list the
//! next read reports is the held set less exactly the keys the call named — the removed keys
//! are absent from its members and the list's CARDINALITY falls by the number of held keys
//! the call named. `UntagServerCertificate`: "removes the specified tags from the IAM server
//! certificate", with "the tags with matching keys are removed from the specified IAM server
//! certificate" (`UntagServerCertificate$TagKeys`); `ListServerCertificateTags$Tags` is a
//! required `tagListType` (length 0..50) carrying "the list of tags that are currently
//! attached to the IAM server certificate".
//!
//! `tests/all/server_certificate_tag_removal.rs` holds the same rule along the MEMBERSHIP
//! axis — which keys survive a removal, how keys compare, the certificate's path and name
//! casing, the published key bounds and the per-resource quota — and owns the untag call and
//! the tag fixtures this suite shares. The axis here is the reported LIST itself: how many
//! `<member>` elements the required `Tags` list carries after a removal, and that a read
//! does not change what the next reader sees, so a surviving set of exactly one tag is
//! reported as exactly one member however often it is read.
//!
//! The rows differ along what the rule generalises over: the held tag count, how many keys
//! one call names, whether a named key is held, and the surviving cardinality (none, one,
//! several, and all but one of a certificate at the published tag quota).
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour of a certificate that was PERSISTED rather than a handler called directly.
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns, the certificate
//! upload and both tag readers are `tests/all/server_certificate_tag_updates.rs`'s, the
//! key-list and quota fixtures are `tests/all/role_tag_removal.rs`'s and the tag-list
//! builders are `tests/all/role_tags.rs`'s, so a request is built and read exactly one way
//! across this crate.

use std::collections::BTreeMap;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::MAX_TAGS_PER_ENTITY;
use smithy_server_runtime::MultiServiceRouter;

use crate::role_tag_removal::{fixture, keys};
use crate::role_tags::{expected_map, tags};
// The untag call is the one `tests/all/server_certificate_tag_removal.rs` owns, and the
// upload and both readers are `tests/all/server_certificate_tag_updates.rs`'s, so a request
// is built and read exactly one way across this crate.
use crate::server_certificate_tag_removal::untag_certificate;
use crate::server_certificate_tag_updates::{assert_reads, listed_tags, upload_certificate};
use crate::user_lifecycle::{call, element, error_code, router};

/// How many tag members the `ListServerCertificateTags` response carries, counted off the
/// elements the document really holds rather than off a map, so a member repeating a key
/// cannot hide inside one.
async fn listed_member_count(router: &MultiServiceRouter, name: &str) -> usize {
    listed_tags(router, name).await.1.len()
}

/// One row: a certificate carrying `initial`, one untag call naming `remove`, and the tag
/// set the listing must then report.
struct Row {
    case: &'static str,
    initial: Vec<(String, String)>,
    remove: Vec<String>,
    expected: Vec<(String, String)>,
}

/// The rows: one per surviving cardinality the rule has to hold at.
fn rows() -> Vec<Row> {
    // The published per-resource tag quota, so the last row starts from the largest tag set
    // a certificate can hold.
    let at_quota = fixture(MAX_TAGS_PER_ENTITY);
    vec![
        Row {
            case: "one key removed from a two-key set leaves one member",
            initial: tags(&[("Env", "probe"), ("Tier", "web")]),
            remove: keys(&["Tier"]),
            expected: tags(&[("Env", "probe")]),
        },
        Row {
            case: "the other key of the same two-key set leaves the other member",
            initial: tags(&[("Env", "probe"), ("Tier", "web")]),
            remove: keys(&["Env"]),
            expected: tags(&[("Tier", "web")]),
        },
        Row {
            case: "both keys of a two-key set leave no members",
            initial: tags(&[("Env", "probe"), ("Tier", "web")]),
            remove: keys(&["Env", "Tier"]),
            expected: Vec::new(),
        },
        Row {
            case: "two of four keys leave two members",
            initial: tags(&[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")]),
            remove: keys(&["b", "d"]),
            expected: tags(&[("a", "1"), ("c", "3")]),
        },
        Row {
            case: "a key the certificate does not hold leaves every member",
            initial: tags(&[("Env", "probe"), ("Tier", "web")]),
            remove: keys(&["Owner"]),
            expected: tags(&[("Env", "probe"), ("Tier", "web")]),
        },
        Row {
            case: "one held key named beside two unheld ones leaves the rest",
            initial: tags(&[("a", "1"), ("b", "2"), ("c", "3")]),
            remove: keys(&["b", "x", "y"]),
            expected: tags(&[("a", "1"), ("c", "3")]),
        },
        Row {
            case: "all but one key of a certificate at the published tag quota",
            initial: at_quota.clone(),
            remove: at_quota[1..].iter().map(|(key, _)| key.clone()).collect(),
            expected: at_quota[..1].to_vec(),
        },
    ]
}

/// RULE: the keys an untag call names are gone from the persisted set, so the `Tags` list
/// the next `ListServerCertificateTags` reports carries exactly the survivors — one
/// `<member>` per surviving tag and none for a removed key — and `GetServerCertificate`
/// reports the same set.
#[tokio::test]
async fn the_listing_reports_only_the_tags_the_untag_left() {
    let router = router();
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let certificate_name = format!("listed-after-untag-{index}");
        upload_certificate(&router, &certificate_name, None, &row.initial).await;

        // The listing before the removal is the set the upload stored: the row's arithmetic
        // starts from a set the service really holds.
        let before = listed_member_count(&router, &certificate_name).await;
        assert_eq!(
            before,
            row.initial.len(),
            "case {case:?}: expected the upload's {} tags to be listed, got {before} members",
            row.initial.len()
        );

        let (status, body) = untag_certificate(&router, &certificate_name, &row.remove).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UntagServerCertificate must succeed, got {status}: {body}"
        );

        // Both readers of the same persisted set must report exactly the survivors, sorted
        // by tag key, so the removal is in the state rather than in one response.
        let expected: BTreeMap<String, String> = expected_map(&row.expected);
        assert_reads(&router, case, &certificate_name, &expected).await;
        // And the list's CARDINALITY is the arithmetic of the removal: one member per
        // survivor, none for a removed key.
        let reported = listed_member_count(&router, &certificate_name).await;
        assert_eq!(
            reported,
            expected.len(),
            "case {case:?}: expected {} listed member(s) after the removal, got {reported}",
            expected.len()
        );
    }
}

/// RULE: a listing is a READ, so reading the surviving set does not change it — the same
/// cardinality and the same members come back however many times the listing is made, and a
/// second untag naming the already-removed key leaves them alone.
#[tokio::test]
async fn reading_the_surviving_set_leaves_it_as_it_is() {
    let router = router();
    let held = tags(&[("Env", "probe"), ("Tier", "web")]);
    let expected: BTreeMap<String, String> = expected_map(&tags(&[("Env", "probe")]));

    upload_certificate(&router, "read-repeatedly", None, &held).await;
    let (status, body) = untag_certificate(&router, "read-repeatedly", &keys(&["Tier"])).await;
    assert_eq!(
        status, 200,
        "the fixture UntagServerCertificate must succeed, got {status}: {body}"
    );

    for attempt in 0..4 {
        // Every other read is preceded by a repeat of the same untag call, which has
        // nothing left to remove.
        if attempt % 2 == 1 {
            let (status, body) =
                untag_certificate(&router, "read-repeatedly", &keys(&["Tier"])).await;
            assert_eq!(
                status, 200,
                "attempt {attempt}: repeating the untag must succeed, got {status}: {body}"
            );
        }
        let (listed, listed_keys) = listed_tags(&router, "read-repeatedly").await;
        assert_eq!(
            listed, expected,
            "attempt {attempt}: expected the surviving set {expected:?}, got {listed:?}"
        );
        assert_eq!(
            listed_keys,
            vec!["Env".to_string()],
            "attempt {attempt}: expected exactly the surviving member, got {listed_keys:?}"
        );
    }
}

/// RULE: the listing addresses a certificate the account holds; a name it holds none under
/// "referenced a resource entity that does not exist", so the read is refused with the
/// modelled `NoSuchEntityException` (HTTP 404, awsQuery code `NoSuchEntity`) rather than
/// reporting an empty list — including for a certificate whose every tag was removed and
/// which therefore reports NO members at all.
#[tokio::test]
async fn an_emptied_set_is_still_a_held_certificate() {
    let router = router();
    upload_certificate(
        &router,
        "emptied-cert",
        None,
        &tags(&[("Env", "probe"), ("Tier", "web")]),
    )
    .await;
    let (status, body) =
        untag_certificate(&router, "emptied-cert", &keys(&["Env", "Tier"])).await;
    assert_eq!(
        status, 200,
        "the fixture UntagServerCertificate must succeed, got {status}: {body}"
    );

    // Every tag gone is an EMPTY list on a held certificate, not an absent certificate.
    let (listed, listed_keys) = listed_tags(&router, "emptied-cert").await;
    assert!(
        listed.is_empty() && listed_keys.is_empty(),
        "expected a held certificate with no tags to report no members, got {listed:?}"
    );

    // A name the account holds no certificate under is the modelled absence instead.
    let absent = "no-such-cert";
    let (status, body) = call(
        &router,
        "ListServerCertificateTags",
        &[("ServerCertificateName", absent)],
    )
    .await;
    assert_eq!(status, 404, "expected HTTP 404, got {status}: {body}");
    assert_eq!(
        error_code(&body),
        "NoSuchEntity",
        "expected the modelled awsQuery code, got {body}"
    );
    assert_eq!(
        element(&body, "Message"),
        format!("The Server Certificate with name {absent} cannot be found."),
        "expected the message to name the certificate, got {body}"
    );
}
