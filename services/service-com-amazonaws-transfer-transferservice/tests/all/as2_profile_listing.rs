// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AS2 profile surface: `CreateProfile` -> `ListProfiles`,
//! with `DescribeProfile` as the member-by-member read back.
//!
//! RULE: `ListProfiles` reports every stored profile exactly once, each projected from
//! the record its create persisted (`Arn`, `ProfileId`, `As2Id`, `ProfileType`), in
//! `ProfileId` order, narrowed to one `ProfileType` when the request names one and
//! spanning every type when it does not, bounded by `MaxResults` with a `NextToken`
//! handed back only while further records remain.
//!
//! Every test drives the real wire path through the shared [`crate::harness`] — `POST /`
//! with `X-Amz-Target: TransferService.<Op>` — so what is asserted is what an SDK sees,
//! not what a handler returns. Each suite asserts the rule over a TABLE whose rows
//! differ along what it generalises over: the `ProfileType` value, the form and length
//! of `As2Id`, how many `CertificateIds` and `Tags` the record holds, how many profiles
//! are stored, the filter, and the page bound.
//!
//! `CreateProfile`'s own minting rule and `DescribeProfile`'s not-found answer are
//! pinned by `tests/all/profile_lifecycle.rs`; this suite uses them as the writer and
//! the member-by-member reader behind the listing.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::harness::{
    assert_members, drain_pages, is_modelled_profile_id, member_of, Emulator, ACCOUNT,
    CERTIFICATE_IDS, PROFILE_TYPES, REGION,
};

/// `As2Id` is modelled `len 1..128`: the longest id the model admits.
const LONGEST_AS2_ID_LEN: usize = 128;

/// A list member of a `CreateProfile` request as the profile's reader reports it back:
/// the list the request named, or the EMPTY list when it named none — both
/// `CertificateIds` and `Tags` are collections the profile always holds, so the reader
/// reports them whether or not the create supplied them.
fn requested_list(request: &Value, member: &str) -> Value {
    match member_of(request, member) {
        Value::Null => json!([]),
        supplied => supplied.clone(),
    }
}

/// One row of the profile table: the case name and the `CreateProfile` request.
struct Row {
    case: &'static str,
    request: Value,
}

/// The profile table every suite below creates.
///
/// The rows differ along the `ProfileType` value (both members of the set), the length
/// and character content of `As2Id` (both modelled bounds, plus the printable
/// punctuation its pattern `^[\u{0020}-\u{007E}\s]*$` admits), the number of
/// `CertificateIds` (none, one, all three) and of `Tags` (none, two) — and one row
/// repeats another's whole configuration, which must mint an independent profile
/// rather than being folded into it.
fn rows() -> Vec<Row> {
    let longest_as2_id = "a".repeat(LONGEST_AS2_ID_LEN);
    vec![
        Row {
            case: "LOCAL, no certificates and no tags",
            request: json!({"As2Id": "local-one", "ProfileType": "LOCAL"}),
        },
        Row {
            case: "PARTNER, no certificates and no tags",
            request: json!({"As2Id": "partner-one", "ProfileType": "PARTNER"}),
        },
        Row {
            case: "LOCAL with the shortest legal As2Id and one certificate",
            request: json!({
                "As2Id": "l",
                "ProfileType": "LOCAL",
                "CertificateIds": [CERTIFICATE_IDS[0]],
            }),
        },
        Row {
            case: "PARTNER with the longest legal As2Id, three certificates and two tags",
            request: json!({
                "As2Id": longest_as2_id,
                "ProfileType": "PARTNER",
                "CertificateIds": CERTIFICATE_IDS,
                "Tags": [
                    {"Key": "team", "Value": "integration"},
                    {"Key": "env", "Value": "test"},
                ],
            }),
        },
        Row {
            case: "PARTNER repeating an earlier row's whole configuration",
            request: json!({"As2Id": "partner-one", "ProfileType": "PARTNER"}),
        },
        Row {
            case: "LOCAL whose As2Id carries the punctuation the pattern admits",
            request: json!({"As2Id": "AS2-From_v1.0+{test}/[x]", "ProfileType": "LOCAL"}),
        },
    ]
}

/// One created profile: the row that made it, its minted id, and what `ListProfiles`
/// must project for it.
struct Created {
    case: &'static str,
    profile_id: String,
    request: Value,
}

impl Created {
    /// `ProfileType` as the create named it.
    fn profile_type(&self) -> &str {
        member_of(&self.request, "ProfileType")
            .as_str()
            .expect("every row names a ProfileType")
    }

    /// The `ListedProfile` the stored record projects to: the whole member set the
    /// `ListedProfile` shape declares.
    fn listed(&self) -> Value {
        json!({
            "Arn": format!("arn:aws:transfer:{REGION}:{ACCOUNT}:profile/{}", self.profile_id),
            "ProfileId": self.profile_id,
            "As2Id": member_of(&self.request, "As2Id"),
            "ProfileType": self.profile_type(),
        })
    }
}

/// Create every row of [`rows`] and assert each create minted a modelled, unique id.
async fn create_table(emulator: &Emulator) -> Vec<Created> {
    let mut created: Vec<Created> = Vec::new();
    for row in rows() {
        let profile_id = emulator.create_profile(row.request.clone()).await;
        assert!(
            is_modelled_profile_id(&profile_id),
            "case {:?}: expected a ProfileId matching ^p-([0-9a-f]{{17}})$, got {profile_id:?}",
            row.case
        );
        assert!(
            created.iter().all(|other| other.profile_id != profile_id),
            "case {:?}: expected an independent ProfileId, got the already-minted {profile_id:?}",
            row.case
        );
        created.push(Created {
            case: row.case,
            profile_id,
            request: row.request,
        });
    }
    created
}

/// The ids of `created` that a listing filtered by `profile_type` must report, in
/// `ProfileId` order.
fn expected_ids(created: &[Created], profile_type: Option<&str>) -> Vec<String> {
    let mut ids: Vec<String> = created
        .iter()
        .filter(|profile| profile_type.is_none_or(|want| profile.profile_type() == want))
        .map(|profile| profile.profile_id.clone())
        .collect();
    ids.sort();
    ids
}

/// RULE: every profile a create persisted is reported by `ListProfiles` exactly once,
/// projected from the stored record, and `DescribeProfile` reads the same record back
/// member by member.
#[tokio::test]
async fn list_profiles_reports_every_created_profile_projected_from_the_stored_record() {
    let emulator = Emulator::new();
    let created = create_table(&emulator).await;

    let answer = emulator.call("ListProfiles", json!({})).await;
    assert_eq!(
        answer.status, 200,
        "ListProfiles: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    let listed = answer
        .get("Profiles")
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("ListProfiles reported no Profiles list: {}", answer.body));

    assert_eq!(
        listed.len(),
        created.len(),
        "case \"every created profile\": expected {} profiles, got {listed:?}",
        created.len()
    );
    assert!(
        answer.get("NextToken").is_null(),
        "case \"every created profile\": expected no NextToken on a complete listing, got {:?}",
        answer.get("NextToken")
    );

    // `ProfileId` order: the listing is resumable, so it is ordered by the key a
    // `NextToken` names.
    let listed_ids: Vec<String> = listed
        .iter()
        .map(|profile| {
            member_of(profile, "ProfileId")
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        listed_ids,
        expected_ids(&created, None),
        "case \"every created profile\": expected every id once in ProfileId order, got {listed_ids:?}"
    );

    for profile in &created {
        let found = listed
            .iter()
            .find(|item| member_of(item, "ProfileId").as_str() == Some(profile.profile_id.as_str()))
            .unwrap_or_else(|| panic!("case {:?}: expected it listed in {listed:?}", profile.case));
        assert_eq!(
            found,
            &profile.listed(),
            "case {:?}: expected the stored projection {}, got {found}",
            profile.case,
            profile.listed()
        );

        // Round trip: the same record, read back through `DescribeProfile`. Both list
        // members are collections the profile always holds, so a create that named
        // neither reads back as the empty list rather than as an absent member.
        let described = emulator
            .call("DescribeProfile", json!({"ProfileId": &profile.profile_id}))
            .await;
        assert_eq!(
            described.status, 200,
            "case {:?}: expected DescribeProfile 200, got {} with body {}",
            profile.case, described.status, described.body
        );
        let got = member_of(&described.body, "Profile");
        assert_members(
            profile.case,
            got,
            &json!({
                "Arn": format!("arn:aws:transfer:{REGION}:{ACCOUNT}:profile/{}", profile.profile_id),
                "ProfileId": profile.profile_id,
                "As2Id": member_of(&profile.request, "As2Id"),
                "ProfileType": profile.profile_type(),
                "CertificateIds": requested_list(&profile.request, "CertificateIds"),
                "Tags": requested_list(&profile.request, "Tags"),
            }),
        );
    }
}

/// RULE: `ProfileType` narrows the listing to that type, and an absent one lists every
/// type — "If not supplied in the request, the command lists all types of profiles"
/// (`ListProfiles.ProfileType`).
///
/// The rows are the filter: each member of `ProfileType`'s modelled value set, and no
/// filter at all.
#[tokio::test]
async fn list_profiles_narrowed_to_a_profile_type_reports_only_that_type() {
    let emulator = Emulator::new();
    let created = create_table(&emulator).await;

    let mut filtered_ids: BTreeSet<String> = BTreeSet::new();
    for profile_type in PROFILE_TYPES {
        let case = format!("ProfileType={profile_type}");
        let answer = emulator
            .call("ListProfiles", json!({"ProfileType": profile_type}))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        let listed = answer
            .get("Profiles")
            .as_array()
            .cloned()
            .unwrap_or_default();

        let want = expected_ids(&created, Some(profile_type));
        let got: Vec<String> = listed
            .iter()
            .map(|profile| {
                member_of(profile, "ProfileId")
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert!(
            !want.is_empty(),
            "case {case:?}: the table must hold at least one profile of this type"
        );
        assert_eq!(
            got, want,
            "case {case:?}: expected exactly that type's ids in ProfileId order, got {got:?}"
        );
        for profile in &listed {
            assert_eq!(
                member_of(profile, "ProfileType").as_str(),
                Some(profile_type),
                "case {case:?}: expected only {profile_type} profiles, got {profile}"
            );
        }
        filtered_ids.extend(got);
    }

    // The two filters partition the unfiltered listing: no stored profile is reachable
    // under neither filter, and none under both.
    let unfiltered: BTreeSet<String> = expected_ids(&created, None).into_iter().collect();
    assert_eq!(
        filtered_ids, unfiltered,
        "case \"the filters partition the store\": expected the union of the per-type \
         listings to be every stored id, got {filtered_ids:?}"
    );
}

/// RULE: a page carries at most `MaxResults` profiles and a `NextToken` ONLY while
/// further ones remain, and resuming from that token walks the filtered set exactly
/// once in `ProfileId` order.
///
/// The rows are the page bound crossed with the filter: both published bounds of
/// `MaxResults`' modelled `range 1..1000`, sizes below, equal to and above the stored
/// count (so the exact-fit page must NOT hand back a token), and no bound at all.
#[tokio::test]
async fn list_profiles_honours_every_page_bound_and_resumes_from_its_token() {
    let emulator = Emulator::new();
    let created = create_table(&emulator).await;

    for filter in [None, Some("LOCAL"), Some("PARTNER")] {
        let want = expected_ids(&created, filter);
        let base = match filter {
            Some(profile_type) => json!({"ProfileType": profile_type}),
            None => json!({}),
        };

        for limit in [Some(1), Some(2), Some(3), Some(5), Some(6), Some(7), Some(1000), None] {
            let case = format!("ProfileType={filter:?} MaxResults={limit:?}");
            let (page_sizes, ids) = drain_pages(
                &emulator,
                "ListProfiles",
                &base,
                "Profiles",
                "ProfileId",
                limit,
            )
            .await;

            assert_eq!(
                ids, want,
                "case {case:?}: expected every matching id once in ProfileId order, got {ids:?}"
            );
            match limit {
                Some(limit) => {
                    let limit = usize::try_from(limit).expect("a positive page bound");
                    for size in &page_sizes {
                        assert!(
                            *size <= limit,
                            "case {case:?}: a page carried {size} profiles, above the bound {limit}"
                        );
                    }
                    // `want.len().div_ceil(limit)` pages, and no trailing empty page:
                    // a page that exactly exhausts the set carries no token.
                    let want_pages = want.len().div_ceil(limit).max(1);
                    assert_eq!(
                        page_sizes.len(),
                        want_pages,
                        "case {case:?}: expected {want_pages} page(s), got {page_sizes:?}"
                    );
                }
                None => assert_eq!(
                    page_sizes,
                    vec![want.len()],
                    "case {case:?}: expected a single unbounded page, got {page_sizes:?}"
                ),
            }
        }
    }
}

/// RULE: a `NextToken` the service did not issue is refused with
/// `InvalidNextTokenException` (400) rather than silently restarting the listing, and a
/// `MaxResults` outside the modelled `range 1..1000` is refused at the boundary while
/// both bounds themselves are accepted.
#[tokio::test]
async fn an_unissued_next_token_and_an_out_of_range_max_results_are_refused() {
    let emulator = Emulator::new();
    create_table(&emulator).await;

    // Rows: not hex at all, odd-length hex, and hex that decodes to bytes that are
    // not UTF-8 — none of them a token this service issued.
    for token in ["not-a-token", "zz", "abc", "c0ffee00", "ff"] {
        let answer = emulator
            .call("ListProfiles", json!({"NextToken": token}))
            .await;
        let case = format!("NextToken={token:?}");
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidNextTokenException",
            "case {case:?}: expected InvalidNextTokenException, got {}",
            answer.body
        );
    }

    // One PAST each published bound of `MaxResults`' modelled range 1..1000.
    for bad in [0, 1001] {
        let answer = emulator
            .call("ListProfiles", json!({"MaxResults": bad}))
            .await;
        assert_eq!(
            answer.status, 400,
            "case \"MaxResults={bad}\": expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
    }

    // Both published bounds themselves are accepted.
    for good in [1, 1000] {
        let answer = emulator
            .call("ListProfiles", json!({"MaxResults": good}))
            .await;
        assert_eq!(
            answer.status, 200,
            "case \"MaxResults={good}\": expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
    }

    // A value outside `ProfileType`'s modelled set is refused at the boundary too, so
    // the filter can never be a free-text match.
    for bad in ["local", "REMOTE", ""] {
        let answer = emulator
            .call("ListProfiles", json!({"ProfileType": bad}))
            .await;
        assert_eq!(
            answer.status, 400,
            "case \"ProfileType={bad:?}\": expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
    }
}

/// RULE: `ListProfiles` reports stored state and changes none of it — an account with
/// no profiles lists none, and repeated listings (paged or not) answer the same thing.
#[tokio::test]
async fn list_profiles_reports_an_empty_account_and_leaves_the_store_unchanged() {
    let emulator = Emulator::new();

    // Rows: before any create, and after a create + the full paging walk.
    let empty = emulator.call("ListProfiles", json!({})).await;
    assert_eq!(
        empty.status, 200,
        "case \"no profiles stored\": expected 200, got {} with body {}",
        empty.status, empty.body
    );
    assert_eq!(
        empty.get("Profiles"),
        &json!([]),
        "case \"no profiles stored\": expected an empty Profiles list, got {}",
        empty.body
    );
    assert!(
        empty.get("NextToken").is_null(),
        "case \"no profiles stored\": expected no NextToken, got {:?}",
        empty.get("NextToken")
    );

    let created = create_table(&emulator).await;
    let want = expected_ids(&created, None);

    let first = emulator.call("ListProfiles", json!({})).await;
    // Page through the whole set one profile at a time, then list again: a read must
    // not consume, reorder or expire anything a later caller sees.
    let (_, paged) = drain_pages(
        &emulator,
        "ListProfiles",
        &json!({}),
        "Profiles",
        "ProfileId",
        Some(1),
    )
    .await;
    assert_eq!(
        paged, want,
        "case \"paged walk\": expected every id once, got {paged:?}"
    );
    let again = emulator.call("ListProfiles", json!({})).await;
    assert_eq!(
        again.body, first.body,
        "case \"re-read after paging\": expected the same listing as {}, got {}",
        first.body, again.body
    );

    // And each record is still readable member by member.
    for profile in &created {
        let described = emulator
            .call("DescribeProfile", json!({"ProfileId": &profile.profile_id}))
            .await;
        assert_eq!(
            described.status, 200,
            "case {:?}: expected the profile still readable, got {} with body {}",
            profile.case, described.status, described.body
        );
    }
}
