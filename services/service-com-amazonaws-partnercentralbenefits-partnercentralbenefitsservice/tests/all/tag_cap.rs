// SPDX-License-Identifier: CC0-1.0

//! The per-resource tag cap, on EVERY path that puts tags on a resource.
//!
//! Every assertion goes through the real request path — the shared
//! [`crate::benefit_application_lifecycle::Harness`], i.e. `MultiServiceRouter::dispatch`
//! with `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>` — so what it pins is the emulator's HTTP
//! behaviour, not an internal helper.
//!
//! The rule under test: the number of tags one resource carries is bounded by the
//! service's per-resource tag cap ([`MAX_TAGS_PER_RESOURCE`], the model's own
//! `CreateBenefitApplicationInput$Tags` `length {max: 200}`), so NO accepted request on
//! ANY path that adds tags ever leaves a resource holding more than that, and a request
//! that would is refused with the error that bound's own path states — the modelled
//! `ValidationException` (HTTP 400) where the bound is the modelled list length the
//! request violates, the modelled `ServiceQuotaExceededException` (HTTP 402) where it is
//! the resource's stored set a merge would overflow — while the resource keeps the set it
//! had.
//!
//! `CreateBenefitApplication$Tags` and `TagResource$tags` are every input member of this
//! service that ADDS tags (`UntagResource` only removes, and `ListTagsForResource` only
//! reports), so both are driven here; the stored set is read back through
//! `ListTagsForResource` every time.
//!
//! The tables span what the rule generalises over rather than the one call the issue
//! named: the number of tags the request carries (none, one, a middling count, one under
//! the cap, exactly the cap, one past it, far past it), whether those keys are new to the
//! resource or ones it already holds, whether the request's own list repeats a key, which
//! path they arrive on, and how much of the cap the resource had already used. The
//! reported call — a create carrying more tags than the cap — is one row among them.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::service::constraint_requirement;
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::MAX_TAGS_PER_RESOURCE;
use smithy_server_runtime::ViolationKind;

use crate::benefit_application_lifecycle::{error_code, Harness, CATALOG};
use crate::reserved_tag_keys::application_count;
use crate::resource_tagging::{benefit_id, ok, tag_payload, tags_of};
use crate::service_error_rules::{assert_validation_invariants, clause, message_of, one};

/// The cap as the request-side bound it also is: the modelled maximum length of a create's
/// `Tags` list.
const CAP: usize = MAX_TAGS_PER_RESOURCE;

/// One row of the create table: a tag list, and whether it lies within the cap.
struct CreateRow {
    case: String,
    tags: Vec<(String, String)>,
    within: bool,
}

/// `count` distinct tags, named apart from any other row's by `prefix`.
fn distinct_tags(prefix: &str, count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| (format!("{prefix}-{index}"), format!("v{index}")))
        .collect()
}

/// A tag list as [`tag_payload`] wants it.
fn borrowed(tags: &[(String, String)]) -> Vec<(&str, &str)> {
    tags.iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect()
}

/// A tag list as the store keeps it: the SET its keys name, so a repeated key counts once.
fn as_map(tags: &[(String, String)]) -> BTreeMap<String, String> {
    tags.iter().cloned().collect()
}

/// The `Message` a refusal of a `Tags` list longer than the modelled maximum states: the
/// walk's own length clause, built from the same wording the service words every
/// constraint refusal with, so this test pins no copy of that sentence.
fn length_message(member: &str, max: usize) -> String {
    let requirement = constraint_requirement(&ViolationKind::MaxLength(max as i64))
        .expect("the service words a maximum-length violation");
    one(&clause(member, &requirement))
}

/// Create one application carrying `tags` and return `(status, body)`.
async fn create(
    harness: &Harness,
    token: &str,
    benefit: &str,
    tags: &[(String, String)],
) -> (u16, Value) {
    harness
        .call(
            "CreateBenefitApplication",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": token,
                "BenefitIdentifier": benefit,
                "Name": token,
                "Tags": tag_payload(&borrowed(tags)),
            }),
        )
        .await
}

/// How many applications [`CATALOG`] holds, read over the wire — the sibling suite's
/// helper, so the two tables count the same way.
async fn applications(harness: &Harness, case: &str) -> usize {
    application_count(harness, case, CATALOG).await
}

// ---------------------------------------------------------------------------
// The create path
// ---------------------------------------------------------------------------

/// A create is served for every tag count up to the cap and refused past it, and a refused
/// create mints nothing.
///
/// The accepted rows are read back through `ListTagsForResource`: what the create stored is
/// exactly the SET its list named — a repeated key counts once — and never more than the
/// cap.
#[tokio::test]
async fn a_create_is_served_up_to_the_tag_cap_and_refused_past_it() {
    let harness = Harness::new();
    let benefit = benefit_id(&harness, "create", CATALOG).await;

    let rows: Vec<CreateRow> = vec![
        CreateRow {
            case: "no tags at all".to_string(),
            tags: Vec::new(),
            within: true,
        },
        CreateRow {
            case: "one tag".to_string(),
            tags: distinct_tags("one", 1),
            within: true,
        },
        CreateRow {
            case: "a middling count".to_string(),
            tags: distinct_tags("mid", CAP / 2),
            within: true,
        },
        CreateRow {
            case: "one under the cap".to_string(),
            tags: distinct_tags("under", CAP - 1),
            within: true,
        },
        CreateRow {
            case: "exactly the cap".to_string(),
            tags: distinct_tags("at", CAP),
            within: true,
        },
        CreateRow {
            // The cap counts the SET, so a list AT the cap whose keys all repeat is served
            // and stores one tag.
            case: "a full list naming one key over and over".to_string(),
            tags: (0..CAP)
                .map(|index| ("repeated".to_string(), format!("v{index}")))
                .collect(),
            within: true,
        },
        CreateRow {
            // The reported call: more tags than the cap.
            case: "one past the cap".to_string(),
            tags: distinct_tags("past", CAP + 1),
            within: false,
        },
        CreateRow {
            case: "well past the cap".to_string(),
            tags: distinct_tags("far", CAP * 2),
            within: false,
        },
        CreateRow {
            // Past the cap by COUNT even though the keys it names are one: the list length
            // is a fact about the request, refused before the set is computed.
            case: "a list past the cap naming one key over and over".to_string(),
            tags: (0..CAP + 1)
                .map(|index| ("repeated".to_string(), format!("v{index}")))
                .collect(),
            within: false,
        },
    ];

    let mut expected_applications = applications(&harness, "before").await;
    for (index, row) in rows.iter().enumerate() {
        let (case, tags) = (&row.case, &row.tags);
        let token = format!("tok-cap-create-{index}");
        let (status, out) = create(&harness, &token, &benefit, tags).await;

        if row.within {
            assert_eq!(
                status, 200,
                "case {case:?}: a create within the cap must be served, got {status} {out:?}"
            );
            expected_applications += 1;
            let arn = out["Arn"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: the create reported no Arn: {out:?}")
            });
            let got = tags_of(&harness, case, arn).await;
            assert_eq!(
                got,
                as_map(tags),
                "case {case:?}: expected the created tag set {:?}, got {got:?}",
                as_map(tags)
            );
            assert!(
                got.len() <= CAP,
                "case {case:?}: a stored tag set may never exceed {CAP}, got {}",
                got.len()
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a create past the cap must be refused, got {status} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {out:?}"
            );
            let message = message_of(&out).unwrap_or_else(|| {
                panic!("case {case:?}: the refusal carries no modelled Message member: {out:?}")
            });
            assert_eq!(
                message,
                length_message("tags", CAP),
                "case {case:?}: expected the refusal to state the {CAP}-tag maximum, \
                 got {message:?}"
            );
            assert_validation_invariants(case, &message);
        }

        let count = applications(&harness, case).await;
        assert_eq!(
            count, expected_applications,
            "case {case:?}: expected {expected_applications} applications, got {count}"
        );
    }
}

/// A create refused for carrying too many tags never happened: its `ClientToken` is not
/// consumed, so the same token still mints an application once the list is within the cap.
#[tokio::test]
async fn a_create_refused_on_the_cap_leaves_its_client_token_unused() {
    let harness = Harness::new();
    let benefit = benefit_id(&harness, "token", CATALOG).await;
    let token = "tok-cap-create-retry";

    let oversized = distinct_tags("retry", CAP + 1);
    let (status, out) = create(&harness, token, &benefit, &oversized).await;
    assert_eq!(
        status, 400,
        "case \"token\": a create past the cap must be refused, got {status} {out:?}"
    );

    let within = distinct_tags("retry", CAP);
    let (status, out) = create(&harness, token, &benefit, &within).await;
    assert_eq!(
        status, 200,
        "case \"token\": the retry within the cap must be served, got {status} {out:?}"
    );
    let arn = out["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"token\": the create reported no Arn: {out:?}"));
    let got = tags_of(&harness, "token", arn).await;
    assert_eq!(
        got,
        as_map(&within),
        "case \"token\": the retry must store exactly the {CAP} tags it carried, got {} tags",
        got.len()
    );
}

// ---------------------------------------------------------------------------
// The tagging path, over a resource the create already filled
// ---------------------------------------------------------------------------

/// However much of the cap a create already used, a later `TagResource` is served only
/// while the merged set stays within it: the cap is a property of the RESOURCE, not of one
/// request.
///
/// Each row creates an application carrying `seeded` tags, then merges one NEW key onto it.
#[tokio::test]
async fn the_cap_bounds_a_resource_however_its_tags_arrived() {
    let harness = Harness::new();
    let benefit = benefit_id(&harness, "merge", CATALOG).await;

    // The share of the cap the create already used.
    let rows: Vec<(&str, usize)> = vec![
        ("an empty resource", 0),
        ("one tag from the create", 1),
        ("half the cap from the create", CAP / 2),
        ("one under the cap from the create", CAP - 1),
        ("the whole cap from the create", CAP),
    ];

    for (index, (case, seeded_count)) in rows.iter().enumerate() {
        let seeded = distinct_tags(&format!("seed{index}"), *seeded_count);
        let token = format!("tok-cap-merge-{index}");
        let (status, out) = create(&harness, &token, &benefit, &seeded).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the seeding create must be served, got {status} {out:?}"
        );
        let arn = out["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create reported no Arn: {out:?}"))
            .to_string();
        assert_eq!(
            tags_of(&harness, case, &arn).await,
            as_map(&seeded),
            "case {case:?}: the create must store exactly the tags it carried"
        );

        // One key the resource does not hold: served exactly while there is room for it.
        let (status, out) = harness
            .call(
                "TagResource",
                &json!({
                    "resourceArn": arn,
                    "tags": tag_payload(&[("one-more", "v")]),
                }),
            )
            .await;
        let mut expected = as_map(&seeded);
        if *seeded_count < CAP {
            assert_eq!(
                status, 200,
                "case {case:?}: a merge within the cap must be served, got {status} {out:?}"
            );
            expected.insert("one-more".to_string(), "v".to_string());
        } else {
            assert_eq!(
                status, 402,
                "case {case:?}: a merge past the cap must be refused, got {status} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ServiceQuotaExceededException",
                "case {case:?}: expected ServiceQuotaExceededException, got {out:?}"
            );
        }
        let got = tags_of(&harness, case, &arn).await;
        assert_eq!(
            got, expected,
            "case {case:?}: expected the tag set {expected:?}, got {got:?}"
        );
        assert!(
            got.len() <= CAP,
            "case {case:?}: a stored tag set may never exceed {CAP}, got {}",
            got.len()
        );

        // Re-tagging keys the resource already holds adds no key, so it is served whatever
        // the count — including on a resource sitting exactly on the cap.
        if !seeded.is_empty() {
            let overwrites: Vec<(String, String)> = seeded
                .iter()
                .map(|(key, _)| (key.clone(), "w".to_string()))
                .collect();
            ok(
                &harness,
                case,
                "TagResource",
                &json!({
                    "resourceArn": arn,
                    "tags": tag_payload(&borrowed(&overwrites)),
                }),
            )
            .await;
            let got = tags_of(&harness, case, &arn).await;
            assert!(
                got.len() <= CAP
                    && seeded
                        .iter()
                        .all(|(key, _)| got.get(key).map(String::as_str) == Some("w")),
                "case {case:?}: an overwrite must update every seeded key and add none, \
                 got {got:?}"
            );
        }
    }
}

/// `TagResource$tags` carries no modelled length at all, so a single merge far larger than
/// the cap is refused by the resource's own bound — and the resource keeps the tags the
/// create gave it.
#[tokio::test]
async fn a_single_merge_far_past_the_cap_is_refused_and_changes_nothing() {
    let harness = Harness::new();
    let benefit = benefit_id(&harness, "batch", CATALOG).await;
    let seeded = distinct_tags("batch", 3);
    let (status, out) = create(&harness, "tok-cap-batch", &benefit, &seeded).await;
    assert_eq!(
        status, 200,
        "case \"batch\": the seeding create must be served, got {status} {out:?}"
    );
    let arn = out["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"batch\": the create reported no Arn: {out:?}"))
        .to_string();

    // (case, how many new keys the one merge carries, whether it fits)
    let rows: Vec<(&str, usize, bool)> = vec![
        ("filling the cap exactly", CAP - seeded.len(), true),
        ("one key past a full resource", 1, false),
        ("a whole cap's worth onto a full resource", CAP, false),
        ("twice the cap in one call", CAP * 2, false),
    ];

    let mut expected = as_map(&seeded);
    for (case, count, fits) in rows {
        let additions = distinct_tags(case.split(' ').next().unwrap_or("add"), count);
        let body = json!({
            "resourceArn": arn,
            "tags": tag_payload(&borrowed(&additions)),
        });
        let (status, out) = harness.call("TagResource", &body).await;
        if fits {
            assert_eq!(
                status, 200,
                "case {case:?}: a merge within the cap must be served, got {status} {out:?}"
            );
            expected.extend(additions.iter().cloned());
        } else {
            assert_eq!(
                status, 402,
                "case {case:?}: a merge past the cap must be refused, got {status} {out:?}"
            );
            assert_eq!(
                error_code(&out),
                "ServiceQuotaExceededException",
                "case {case:?}: expected ServiceQuotaExceededException, got {out:?}"
            );
        }
        let got = tags_of(&harness, case, &arn).await;
        assert_eq!(
            got, expected,
            "case {case:?}: expected a tag set of {} entries, got {}",
            expected.len(),
            got.len()
        );
        assert!(
            got.len() <= CAP,
            "case {case:?}: a stored tag set may never exceed {CAP}, got {}",
            got.len()
        );
    }
}
