// SPDX-License-Identifier: CC0-1.0

//! The Route 53 Profile lifecycle, end to end over the wire.
//!
//! Every assertion here goes through `MultiServiceRouter::dispatch`, so it exercises
//! the same restJson1 path a real SDK client takes: routing, deserialization,
//! boundary constraint validation, the handler, and error rendering.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * a created Profile is persisted, and `GetProfile` / `ListProfiles` report exactly
//!   what was stored — for every name the modelled `Name` accepts, with or without
//!   `Tags`;
//! * a Profile's `Arn` is
//!   `arn:aws:route53profiles:<region>:<account>:profile/<Id>` for whatever region the
//!   request is signed for;
//! * a repeated `ClientToken` reports the Profile that token already minted instead of
//!   creating another;
//! * `ListProfiles` and `ListProfileAssociations` return every stored record exactly
//!   once across pages, for every page size, and emit a resumable token only while
//!   items remain; a token neither issued is `InvalidNextTokenException`;
//! * `ListProfileAssociations` narrows by `profileId` and `resourceId`;
//! * `AssociateProfile` persists an association readable by `GetProfileAssociation`,
//!   refuses a (ProfileId, ResourceId) pair that is already associated, refuses a
//!   `ResourceId` another Profile already holds as having spent that VPC's one
//!   Profile association, and refuses a `ProfileId` that names nothing;
//! * `DisassociateProfile` removes the association and reports it `DELETING`;
//! * `DeleteProfile` refuses with `ConflictException` while the Profile still has
//!   associations, and otherwise removes it and reports it `DELETING`;
//! * an id that names nothing is `ResourceNotFoundException`, and a `MaxResults`
//!   outside the modelled range is refused.

use std::collections::BTreeSet;

use serde_json::json;

use super::harness::{Emulator, ACCOUNT_ID};

/// The names in this table are the classes the modelled `Name` shape accepts —
/// `@length(0..64)` with pattern `^(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)$`: a single
/// character, the plain alphanumeric case, a hyphen, an underscore with an interior
/// space, an apostrophe, a digit-leading name that is not all digits, and exactly the
/// maximum length.
fn accepted_names() -> Vec<String> {
    vec![
        "a".to_string(),
        "testprofile".to_string(),
        "test-profile".to_string(),
        "My_Profile 1".to_string(),
        "o'brien".to_string(),
        "0a".to_string(),
        "A".repeat(64),
    ]
}

/// Whether `id` is `<prefix>-` followed by 16 lowercase hex digits — the id shape
/// this emulator mints, inside `ResourceId`'s modelled 1..64 length.
fn is_minted_id(id: &str, prefix: &str) -> bool {
    match id.strip_prefix(prefix).and_then(|rest| rest.strip_prefix('-')) {
        Some(hex) => hex.len() == 16 && hex.chars().all(|c| c.is_ascii_hexdigit()),
        None => false,
    }
}

#[tokio::test]
async fn a_created_profile_is_persisted_and_read_back_whole() {
    // Two regions, because the ARN is built from the request's credential scope.
    for region in ["us-east-1", "eu-west-2"] {
        let em = Emulator::in_region(region);

        for name in accepted_names() {
            for tags in [
                None,
                Some(json!([])),
                Some(json!([{ "Key": "env", "Value": "test" }])),
                Some(json!([{ "Key": "env", "Value": "test" }, { "Key": "team", "Value": "net" }])),
            ] {
                let token = format!("tok-{region}-{name}-{}", tags.is_some());
                let mut body = json!({ "Name": name, "ClientToken": token });
                if let Some(tags) = tags.clone() {
                    body["Tags"] = tags;
                }
                let case =
                    format!("region={region} name={name:?} tags={tags:?}");

                let created = em.create_profile(body).await;
                assert_eq!(
                    created.status, 200,
                    "case {case}: expected 200, got {} / {:?}",
                    created.status, created.body
                );
                let profile = created.member("Profile").clone();
                let id = created.nested_string("Profile", "Id");

                assert!(
                    is_minted_id(&id, "rp"),
                    "case {case}: expected a minted `rp-<16 hex>` Id, got {id:?}"
                );
                assert_eq!(
                    created.nested_string("Profile", "Name"),
                    name,
                    "case {case}: the created Name must be the one requested, got {profile:?}"
                );
                assert_eq!(
                    created.nested_string("Profile", "Arn"),
                    format!("arn:aws:route53profiles:{region}:{ACCOUNT_ID}:profile/{id}"),
                    "case {case}: unexpected Arn in {profile:?}"
                );
                assert_eq!(
                    created.nested_string("Profile", "OwnerId"),
                    ACCOUNT_ID,
                    "case {case}: unexpected OwnerId in {profile:?}"
                );
                assert_eq!(
                    created.nested_string("Profile", "Status"),
                    "COMPLETE",
                    "case {case}: a created Profile is COMPLETE, got {profile:?}"
                );
                assert_eq!(
                    created.nested_string("Profile", "ShareStatus"),
                    "NOT_SHARED",
                    "case {case}: a created Profile is NOT_SHARED, got {profile:?}"
                );
                assert_eq!(
                    created.nested_string("Profile", "ClientToken"),
                    token,
                    "case {case}: ClientToken must be echoed, got {profile:?}"
                );
                assert_eq!(
                    profile["CreationTime"], profile["ModificationTime"],
                    "case {case}: a Profile nothing has changed was modified when created, \
                     got {profile:?}"
                );
                assert!(
                    !profile["CreationTime"].is_null(),
                    "case {case}: CreationTime must be reported, got {profile:?}"
                );

                // The read-back is the same record, member for member.
                let fetched = em.get_profile(&id).await;
                assert_eq!(
                    fetched.status, 200,
                    "case {case}: GetProfile expected 200, got {} / {:?}",
                    fetched.status, fetched.body
                );
                assert_eq!(
                    *fetched.member("Profile"),
                    profile,
                    "case {case}: GetProfile must report the created record, got {:?}",
                    fetched.body
                );

                // And the summary the list reports is that record's subset.
                let listed = em.list_profiles("").await;
                let summary = listed
                    .list("ProfileSummaries")
                    .iter()
                    .find(|entry| entry["Id"] == profile["Id"])
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case}: the created Profile must appear in ListProfiles, got {:?}",
                            listed.body
                        )
                    })
                    .clone();
                assert_eq!(
                    summary,
                    json!({
                        "Id": profile["Id"],
                        "Arn": profile["Arn"],
                        "Name": profile["Name"],
                        "ShareStatus": profile["ShareStatus"],
                    }),
                    "case {case}: the summary must be the stored record's subset, got {summary:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn repeating_a_client_token_reports_the_first_profile() {
    let em = Emulator::new();

    // The token values differ in form (short, long, punctuated, exactly the modelled
    // 255-character maximum of `CreatorRequestId`); every one must be honoured the
    // same way.
    for token in [
        "t".to_string(),
        "tok-1".to_string(),
        "9f8e7d6c-5b4a-3210-9f8e-7d6c5b4a3210".to_string(),
        "z".repeat(255),
    ] {
        let first = em
            .create_profile(json!({ "Name": "first-name", "ClientToken": token }))
            .await;
        assert_eq!(
            first.status, 200,
            "case token={token:?}: expected 200, got {} / {:?}",
            first.status, first.body
        );
        let before = em.list_profiles("").await.list("ProfileSummaries").len();

        // A repeat under the same token, even naming something else, reports the
        // Profile that token already minted and creates nothing.
        let repeat = em
            .create_profile(json!({ "Name": "second-name", "ClientToken": token }))
            .await;
        assert_eq!(
            repeat.status, 200,
            "case token={token:?}: a repeat expected 200, got {} / {:?}",
            repeat.status, repeat.body
        );
        assert_eq!(
            *repeat.member("Profile"),
            *first.member("Profile"),
            "case token={token:?}: a repeated ClientToken must report the first Profile, \
             got {:?}",
            repeat.body
        );
        let after = em.list_profiles("").await.list("ProfileSummaries").len();
        assert_eq!(
            after, before,
            "case token={token:?}: a repeated ClientToken must not store a second Profile"
        );
    }
}

#[tokio::test]
async fn list_profiles_pages_every_profile_exactly_once() {
    let em = Emulator::new();

    let mut created: BTreeSet<String> = BTreeSet::new();
    for index in 0..7 {
        let reply = em.create_named(&format!("paged-{index}")).await;
        assert_eq!(
            reply.status, 200,
            "setup: create paged-{index} expected 200, got {:?}",
            reply.body
        );
        created.insert(reply.nested_string("Profile", "Id"));
    }

    // Every page size from one item to more than the whole store, plus the absent
    // `MaxResults`, must walk the same set exactly once.
    for page_size in [None, Some(1), Some(2), Some(3), Some(7), Some(8), Some(100)] {
        let case = format!("maxResults={page_size:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = match page_size {
                Some(size) => format!("?maxResults={size}"),
                None => String::new(),
            };
            if let Some(token) = next.as_deref() {
                query.push(if query.is_empty() { '?' } else { '&' });
                query.push_str(&format!("nextToken={token}"));
            }
            let reply = em.list_profiles(&query).await;
            assert_eq!(
                reply.status, 200,
                "case {case}: ListProfiles expected 200, got {} / {:?}",
                reply.status, reply.body
            );
            let page = reply.list("ProfileSummaries");
            assert!(
                page.len() <= page_size.unwrap_or(100) as usize,
                "case {case}: a page must hold at most maxResults items, got {}",
                page.len()
            );
            for entry in page {
                seen.push(
                    entry["Id"]
                        .as_str()
                        .unwrap_or_else(|| panic!("case {case}: a summary without an Id"))
                        .to_string(),
                );
            }
            pages += 1;
            assert!(
                pages <= 16,
                "case {case}: pagination did not terminate after {pages} pages"
            );
            match reply.body["NextToken"].as_str() {
                Some(token) => next = Some(token.to_string()),
                None => break,
            }
        }

        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {case}: no Profile may appear twice across pages, got {seen:?}"
        );
        assert_eq!(
            unique, created,
            "case {case}: every stored Profile must appear exactly once, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_next_token_the_operation_did_not_issue_is_refused() {
    let em = Emulator::new();
    for index in 0..3 {
        em.create_named(&format!("tokened-{index}")).await;
        em.associate(
            &em.create_named(&format!("assoc-owner-{index}"))
                .await
                .nested_string("Profile", "Id"),
            &format!("vpc-0000000{index}"),
            &format!("assoc-{index}"),
        )
        .await;
    }

    // A token one list operation issued resumes only that operation: the marker it
    // carries names the issuer, so the other list refuses it rather than reading it
    // as some unrelated resume point.
    let profiles_token = em
        .list_profiles("?maxResults=1")
        .await
        .body["NextToken"]
        .as_str()
        .expect("a resumable ListProfiles token while items remain")
        .to_string();
    let associations_token = em
        .list_associations("?maxResults=1")
        .await
        .body["NextToken"]
        .as_str()
        .expect("a resumable ListProfileAssociations token while items remain")
        .to_string();

    let bogus = [
        String::new(),
        "not-hex".to_string(),
        hex::encode("ListProfiles/1/"),
        hex::encode("some-other-marker/1/rp-0000000000000001"),
    ];

    for token in bogus
        .iter()
        .cloned()
        .chain([associations_token.clone()])
        .collect::<Vec<_>>()
    {
        let reply = em.list_profiles(&format!("?nextToken={token}")).await;
        assert_eq!(
            reply.error.as_deref(),
            Some("InvalidNextTokenException"),
            "case nextToken={token:?}: ListProfiles must refuse a token it did not issue, \
             got {} / {:?}",
            reply.status,
            reply.body
        );
    }

    for token in bogus
        .iter()
        .cloned()
        .chain([profiles_token.clone()])
        .collect::<Vec<_>>()
    {
        let reply = em.list_associations(&format!("?nextToken={token}")).await;
        assert_eq!(
            reply.error.as_deref(),
            Some("InvalidNextTokenException"),
            "case nextToken={token:?}: ListProfileAssociations must refuse a token it did not \
             issue, got {} / {:?}",
            reply.status,
            reply.body
        );
    }

    // The operation's own token still resumes it.
    let resumed = em
        .list_profiles(&format!("?maxResults=1&nextToken={profiles_token}"))
        .await;
    assert_eq!(
        resumed.status, 200,
        "a token ListProfiles issued must resume it, got {} / {:?}",
        resumed.status, resumed.body
    );
}

#[tokio::test]
async fn max_results_is_accepted_on_its_bounds_and_refused_outside_them() {
    let em = Emulator::new();
    em.create_named("bounded").await;

    // `MaxResults` is modelled `@range(min: 1, max: 100)` on both list operations.
    for (value, expected_ok) in [(1, true), (100, true), (0, false), (101, false)] {
        for (operation, reply) in [
            (
                "ListProfiles",
                em.list_profiles(&format!("?maxResults={value}")).await,
            ),
            (
                "ListProfileAssociations",
                em.list_associations(&format!("?maxResults={value}")).await,
            ),
        ] {
            if expected_ok {
                assert_eq!(
                    reply.status, 200,
                    "case {operation} maxResults={value}: a value on the modelled bound must be \
                     accepted, got {} / {:?}",
                    reply.status, reply.body
                );
            } else {
                assert_eq!(
                    reply.status, 400,
                    "case {operation} maxResults={value}: a value outside the modelled range must \
                     be refused, got {} / {:?}",
                    reply.status, reply.body
                );
            }
        }
    }
}

#[tokio::test]
async fn an_id_that_names_nothing_is_not_found() {
    let em = Emulator::new();
    // Forms differing in shape: a well-formed but unused minted id, a bare word, and
    // exactly the modelled 64-character maximum of `ResourceId`.
    for id in [
        "rp-0123456789abcdef".to_string(),
        "nosuchprofile".to_string(),
        "z".repeat(64),
    ] {
        for (operation, reply) in [
            ("GetProfile", em.get_profile(&id).await),
            ("DeleteProfile", em.delete_profile(&id).await),
            ("GetProfileAssociation", em.get_association(&id).await),
            ("DisassociateProfile", em.disassociate(&id, "vpc-00000001").await),
        ] {
            assert_eq!(
                reply.error.as_deref(),
                Some("ResourceNotFoundException"),
                "case {operation} id={id:?}: an unknown id must be ResourceNotFoundException, \
                 got {} / {:?}",
                reply.status,
                reply.body
            );
        }
    }

    // Associating with an unknown Profile is the same refusal.
    let reply = em.associate("rp-0123456789abcdef", "vpc-00000001", "a1").await;
    assert_eq!(
        reply.error.as_deref(),
        Some("ResourceNotFoundException"),
        "AssociateProfile with an unknown ProfileId must be ResourceNotFoundException, \
         got {} / {:?}",
        reply.status,
        reply.body
    );
}

#[tokio::test]
async fn the_association_lifecycle_is_persisted_and_reversible() {
    let em = Emulator::new();
    let profile = em.create_named("assoc-host").await;
    let profile_id = profile.nested_string("Profile", "Id");
    // A second Profile, so the "one Profile per resource" rule has a rival to refuse.
    let rival_id = em
        .create_named("assoc-rival")
        .await
        .nested_string("Profile", "Id");

    // Rows differing in the VPC id's form and in the association name's class.
    let rows = [
        ("vpc-12345678", "assoc-1"),
        ("vpc-0a1b2c3d4e5f67890", "My_Assoc 2"),
        ("v", "a"),
    ];

    for (resource_id, name) in rows {
        let case = format!("resourceId={resource_id:?} name={name:?}");

        let created = em.associate(&profile_id, resource_id, name).await;
        assert_eq!(
            created.status, 200,
            "case {case}: AssociateProfile expected 200, got {} / {:?}",
            created.status, created.body
        );
        let association = created.member("ProfileAssociation").clone();
        let association_id = created.nested_string("ProfileAssociation", "Id");
        assert!(
            is_minted_id(&association_id, "rpassoc"),
            "case {case}: expected a minted `rpassoc-<16 hex>` Id, got {association_id:?}"
        );
        assert_eq!(
            created.nested_string("ProfileAssociation", "Name"),
            name,
            "case {case}: the created Name must be the one requested, got {association:?}"
        );
        assert_eq!(
            created.nested_string("ProfileAssociation", "ProfileId"),
            profile_id,
            "case {case}: unexpected ProfileId in {association:?}"
        );
        assert_eq!(
            created.nested_string("ProfileAssociation", "ResourceId"),
            resource_id,
            "case {case}: unexpected ResourceId in {association:?}"
        );
        assert_eq!(
            created.nested_string("ProfileAssociation", "OwnerId"),
            ACCOUNT_ID,
            "case {case}: unexpected OwnerId in {association:?}"
        );
        assert_eq!(
            created.nested_string("ProfileAssociation", "Status"),
            "COMPLETE",
            "case {case}: a created association is COMPLETE, got {association:?}"
        );

        // Readable back whole, and listable under either filter.
        let fetched = em.get_association(&association_id).await;
        assert_eq!(
            *fetched.member("ProfileAssociation"),
            association,
            "case {case}: GetProfileAssociation must report the created record, got {:?}",
            fetched.body
        );
        for query in [
            format!("?profileId={profile_id}"),
            format!("?resourceId={resource_id}"),
            format!("?profileId={profile_id}&resourceId={resource_id}"),
        ] {
            let listed = em.list_associations(&query).await;
            assert!(
                listed.list("ProfileAssociations").contains(&association),
                "case {case}: {query} must report the association, got {:?}",
                listed.body
            );
        }

        // The same (ProfileId, ResourceId) pair cannot be associated twice, whatever
        // name the repeat gives it.
        for repeat_name in [name, "a-different-name"] {
            let duplicate = em.associate(&profile_id, resource_id, repeat_name).await;
            assert_eq!(
                duplicate.error.as_deref(),
                Some("ResourceExistsException"),
                "case {case} repeat_name={repeat_name:?}: a duplicate pair must be refused, \
                 got {} / {:?}",
                duplicate.status,
                duplicate.body
            );
        }

        // A resource can have only one Profile associated with it, so another Profile
        // over the same `ResourceId` has spent that VPC's allowance — and the first
        // association is untouched.
        let taken = em.associate(&rival_id, resource_id, "rival-assoc").await;
        assert_eq!(
            taken.error.as_deref(),
            Some("LimitExceededException"),
            "case {case}: a second Profile over the same ResourceId must be refused, \
             got {} / {:?}",
            taken.status,
            taken.body
        );
        assert_eq!(
            *em.get_association(&association_id)
                .await
                .member("ProfileAssociation"),
            association,
            "case {case}: a refused association must not have changed the stored one"
        );
        assert!(
            em.list_associations(&format!("?profileId={rival_id}"))
                .await
                .list("ProfileAssociations")
                .is_empty(),
            "case {case}: a refused association must not have been stored"
        );

        // The Profile cannot be deleted while this association stands.
        let blocked = em.delete_profile(&profile_id).await;
        assert_eq!(
            blocked.error.as_deref(),
            Some("ConflictException"),
            "case {case}: DeleteProfile must refuse while associations remain, got {} / {:?}",
            blocked.status,
            blocked.body
        );

        // Disassociating reports the removed record as DELETING, and it is gone.
        let removed = em.disassociate(&profile_id, resource_id).await;
        assert_eq!(
            removed.status, 200,
            "case {case}: DisassociateProfile expected 200, got {} / {:?}",
            removed.status, removed.body
        );
        assert_eq!(
            removed.nested_string("ProfileAssociation", "Id"),
            association_id,
            "case {case}: the removed association must be the stored one, got {:?}",
            removed.body
        );
        assert_eq!(
            removed.nested_string("ProfileAssociation", "Status"),
            "DELETING",
            "case {case}: a removed association is DELETING, got {:?}",
            removed.body
        );
        assert_eq!(
            em.get_association(&association_id).await.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: a removed association must no longer be readable"
        );
        let listed = em
            .list_associations(&format!("?profileId={profile_id}"))
            .await;
        assert!(
            listed.list("ProfileAssociations").is_empty(),
            "case {case}: a removed association must no longer be listed, got {:?}",
            listed.body
        );
        assert_eq!(
            em.disassociate(&profile_id, resource_id).await.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: a pair that is not associated must be ResourceNotFoundException"
        );
    }

    // With every association gone the Profile deletes, reports DELETING, and vanishes
    // from both the read and the list.
    let deleted = em.delete_profile(&profile_id).await;
    assert_eq!(
        deleted.status, 200,
        "DeleteProfile expected 200, got {} / {:?}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.nested_string("Profile", "Id"),
        profile_id,
        "the removed Profile must be the stored one, got {:?}",
        deleted.body
    );
    assert_eq!(
        deleted.nested_string("Profile", "Status"),
        "DELETING",
        "a removed Profile is DELETING, got {:?}",
        deleted.body
    );
    assert_eq!(
        em.get_profile(&profile_id).await.error.as_deref(),
        Some("ResourceNotFoundException"),
        "a deleted Profile must no longer be readable"
    );
    let listed = em.list_profiles("").await;
    assert!(
        listed
            .list("ProfileSummaries")
            .iter()
            .all(|entry| entry["Id"] != json!(profile_id)),
        "a deleted Profile must no longer be listed, got {:?}",
        listed.body
    );
}

#[tokio::test]
async fn list_profile_associations_narrows_by_filter_and_pages_every_record_once() {
    let em = Emulator::new();

    // Two Profiles, each associated with a different number of VPCs, plus one VPC
    // shared between them — so each filter has a distinguishable answer.
    let mut owners: Vec<String> = Vec::new();
    for index in 0..2 {
        owners.push(
            em.create_named(&format!("filter-owner-{index}"))
                .await
                .nested_string("Profile", "Id"),
        );
    }
    let mut expected: Vec<(String, String, String)> = Vec::new();
    for (owner_index, profile_id) in owners.iter().enumerate() {
        // A resource can have only one Profile, so every VPC here belongs to exactly
        // one owner; the two owners hold different numbers of them, so a `profileId`
        // filter has a distinguishable answer.
        for vpc_index in 0..=owner_index + 1 {
            let resource_id = format!("vpc-{owner_index}{vpc_index}");
            let reply = em
                .associate(profile_id, &resource_id, &format!("a-{owner_index}-{vpc_index}"))
                .await;
            assert_eq!(
                reply.status, 200,
                "setup: associate {profile_id}/{resource_id} expected 200, got {:?}",
                reply.body
            );
            expected.push((
                reply.nested_string("ProfileAssociation", "Id"),
                profile_id.clone(),
                resource_id,
            ));
        }
    }

    // A VPC one Profile already holds cannot be taken by the other.
    let (_, held_by, held_vpc) = expected[0].clone();
    let rival = owners
        .iter()
        .find(|owner| **owner != held_by)
        .expect("a second owner Profile");
    let taken = em.associate(rival, &held_vpc, "rival-assoc").await;
    assert_eq!(
        taken.error.as_deref(),
        Some("LimitExceededException"),
        "a VPC already associated with one Profile must be refused to another, got {} / {:?}",
        taken.status,
        taken.body
    );

    // Each filter combination, against the set it should select.
    let unique_vpc = expected[0].2.clone();
    let unique_vpc_owner = expected[0].1.clone();
    let other_owner = rival.clone();
    let filters: Vec<(String, Vec<String>)> = {
        let mut cases = vec![(
            String::new(),
            expected.iter().map(|(id, _, _)| id.clone()).collect(),
        )];
        for profile_id in &owners {
            cases.push((
                format!("profileId={profile_id}"),
                expected
                    .iter()
                    .filter(|(_, owner, _)| owner == profile_id)
                    .map(|(id, _, _)| id.clone())
                    .collect(),
            ));
        }
        // `resourceId` alone selects the single association holding that VPC.
        cases.push((
            format!("resourceId={unique_vpc}"),
            expected
                .iter()
                .filter(|(_, _, resource)| *resource == unique_vpc)
                .map(|(id, _, _)| id.clone())
                .collect(),
        ));
        // Both filters together: the matching pair selects it, a mismatched pair
        // selects nothing even though each half matches something.
        cases.push((
            format!("profileId={unique_vpc_owner}&resourceId={unique_vpc}"),
            expected
                .iter()
                .filter(|(_, owner, resource)| {
                    *owner == unique_vpc_owner && *resource == unique_vpc
                })
                .map(|(id, _, _)| id.clone())
                .collect(),
        ));
        cases.push((
            format!("profileId={other_owner}&resourceId={unique_vpc}"),
            Vec::new(),
        ));
        cases.push(("resourceId=vpc-nosuchvpc".to_string(), Vec::new()));
        cases.push(("profileId=rp-0123456789abcdef".to_string(), Vec::new()));
        cases
    };

    for (filter, wanted) in filters {
        let wanted: BTreeSet<String> = wanted.into_iter().collect();
        for page_size in [None, Some(1), Some(2), Some(100)] {
            let case = format!("filter={filter:?} maxResults={page_size:?}");
            let mut seen: Vec<String> = Vec::new();
            let mut next: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut params: Vec<String> = Vec::new();
                if !filter.is_empty() {
                    params.push(filter.clone());
                }
                if let Some(size) = page_size {
                    params.push(format!("maxResults={size}"));
                }
                if let Some(token) = next.as_deref() {
                    params.push(format!("nextToken={token}"));
                }
                let query = if params.is_empty() {
                    String::new()
                } else {
                    format!("?{}", params.join("&"))
                };
                let reply = em.list_associations(&query).await;
                assert_eq!(
                    reply.status, 200,
                    "case {case}: expected 200, got {} / {:?}",
                    reply.status, reply.body
                );
                for entry in reply.list("ProfileAssociations") {
                    seen.push(
                        entry["Id"]
                            .as_str()
                            .unwrap_or_else(|| {
                                panic!("case {case}: an association without an Id")
                            })
                            .to_string(),
                    );
                }
                pages += 1;
                assert!(
                    pages <= 16,
                    "case {case}: pagination did not terminate after {pages} pages"
                );
                match reply.body["NextToken"].as_str() {
                    Some(token) => next = Some(token.to_string()),
                    None => break,
                }
            }
            let unique: BTreeSet<String> = seen.iter().cloned().collect();
            assert_eq!(
                unique.len(),
                seen.len(),
                "case {case}: no association may appear twice across pages, got {seen:?}"
            );
            assert_eq!(
                unique, wanted,
                "case {case}: the filter must select exactly its own associations, got {seen:?}"
            );
        }
    }
}
