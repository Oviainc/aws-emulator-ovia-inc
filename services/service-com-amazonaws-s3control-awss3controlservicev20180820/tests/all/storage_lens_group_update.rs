// SPDX-License-Identifier: CC0-1.0

//! `UpdateStorageLensGroup`, driven end to end over HTTP.
//!
//! The rule under test: an update of a Storage Lens group the account the
//! request names already owns replaces that group's document WHOLE, under the
//! name the URI and the document both carry, so every later
//! `GetStorageLensGroup` of that name reports exactly the members the submitted
//! document carried — with `StorageLensGroupArn`, which the model documents as
//! read-only, assigned by the service from the group's own identity and the
//! region the request was signed for rather than taken from the request. The
//! name is the group's identity, so a request whose URI and document name
//! different groups is refused rather than renaming one, and a name the account
//! owns no group of is refused as missing rather than created. Neither refusal
//! changes any stored group, and an update reaches only the one group it names:
//! another account's group of the same name, and this account's other names,
//! are untouched.
//!
//! Every row below differs along something that rule generalises over: which
//! `StorageLensGroupFilter` criteria the document before and the document after
//! the update filter on (prefix, suffix, object tag, age, size, and the
//! `And`/`Or` combinations of them), how many values each criterion lists, how
//! deeply each document nests, whether the submitted document already carries a
//! `StorageLensGroupArn`, the length and character mix of the group's name, the
//! region the request is signed for, and the account that owns the group. The
//! documents and names are the create suite's table, reused rather than copied,
//! so an update is exercised over every document shape a create records.
//!
//! The group's Amazon Web Services resource tags are a separate resource an
//! update carries no member for; `TagResource` / `ListTagsForResource` are still
//! scaffolds with their own gap issues, so nothing here can read a tag set
//! through the front door and this suite pins only that the document a read
//! reports is the updated one.
//!
//! Every assertion goes through the same front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router, which deserializes it
//! per the Smithy model, runs the handler and serializes the restXml response.

use crate::access_point_lifecycle::{
    call_in_region, Emulator, Reply, ACCOUNT, OTHER_ACCOUNT, REGION, XMLNS,
};
use crate::storage_lens_configuration::{document_xml, leaf, sorted, sorted_leaves, Leaf};
use crate::storage_lens_group::{
    create_group, expected_arn, get_group, groups, names, Group, NAME_MAX_LEN, OTHER_REGION,
};

/// The body of an `UpdateStorageLensGroup` call carrying `leaves` as its
/// `StorageLensGroup` document.
///
/// `pub(crate)` because the sibling suite that pins which filter members an update
/// reports back (`storage_lens_group_absent_bounds`) submits its documents through this
/// one builder rather than writing a second one.
pub(crate) fn update_body(leaves: &[Leaf]) -> String {
    format!(
        "<UpdateStorageLensGroupRequest xmlns=\"{XMLNS}\">\
         <StorageLensGroup>{}</StorageLensGroup>\
         </UpdateStorageLensGroupRequest>",
        document_xml(leaves)
    )
}

/// Update the group `name` of `account_id` to `group`'s document, submitted
/// under `document_name` (the name the document itself carries — the same name
/// as the URI's, except in the test that pins a disagreement).
async fn update_group_as(
    emulator: &Emulator,
    account_id: &str,
    name: &str,
    document_name: &str,
    group: &Group,
) -> Reply {
    emulator
        .call(
            "PUT",
            &format!("/v20180820/storagelensgroup/{name}"),
            account_id,
            &update_body(&group.submitted(document_name)),
        )
        .await
}

/// Update the group `name` of `account_id` to `group`'s document.
async fn update_group(
    emulator: &Emulator,
    account_id: &str,
    name: &str,
    group: &Group,
) -> Reply {
    update_group_as(emulator, account_id, name, name, group).await
}

/// The pairs of table rows an update is driven over: each document updated to
/// the next row's, and each updated to the row the same distance from the other
/// end — so each pair is a different change of shape (a richer document over a
/// poorer one, a poorer one over a richer one, `And` over `Or`, and back).
fn pairs(count: usize) -> Vec<(usize, usize)> {
    let mut pairs: Vec<(usize, usize)> = (0..count).map(|from| (from, (from + 1) % count)).collect();
    pairs.extend((0..count).map(|from| (from, count - 1 - from)));
    pairs
}

#[tokio::test]
async fn an_update_replaces_the_whole_document_and_every_read_reports_it() {
    let groups = groups();
    let names = names();

    for (from, to) in pairs(groups.len()) {
        let before = &groups[from];
        let after = &groups[to];
        let name = &names[from % names.len()];
        let case = format!("{:?} updated to {:?} as {name:?}", before.case, after.case);

        let emulator = Emulator::start();
        let created = create_group(&emulator, ACCOUNT, name, before, None).await;
        assert_eq!(
            created.status, 204,
            "case {case}: expected the create to be accepted with 204, got {} {}",
            created.status, created.body
        );

        let updated = update_group(&emulator, ACCOUNT, name, after).await;
        assert_eq!(
            updated.status, 204,
            "case {case}: expected the update to be accepted with 204, got {} {}",
            updated.status, updated.body
        );

        let read = get_group(&emulator, ACCOUNT, name).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from the read, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            sorted_leaves(&read.body),
            sorted(&after.expected(name, REGION)),
            "case {case}: the read must report the document the update recorded"
        );

        // A read reports; it must not change what the next caller sees.
        let again = get_group(&emulator, ACCOUNT, name).await;
        assert_eq!(
            sorted_leaves(&again.body),
            sorted_leaves(&read.body),
            "case {case}: a second read must report the same document"
        );

        // The name is still taken: an update changed the group's document, not
        // whether the account owns a group of that name.
        let recreated = create_group(&emulator, ACCOUNT, name, before, None).await;
        assert_eq!(
            (recreated.status, recreated.code().as_str()),
            (400, "InvalidRequestException"),
            "case {case}: a create of the updated name must still be refused, got {} {}",
            recreated.status,
            recreated.body
        );
    }
}

#[tokio::test]
async fn repeated_updates_of_one_document_report_the_same_group() {
    let emulator = Emulator::start();
    let groups = groups();
    let name = "idempotent-group";

    let created = create_group(&emulator, ACCOUNT, name, &groups[0], None).await;
    assert_eq!(
        created.status, 204,
        "expected the create to be accepted with 204, got {} {}",
        created.status, created.body
    );

    // The same document submitted three times over: an update replaces the
    // document, so applying one twice is the same as applying it once.
    let target = &groups[9];
    let mut reported: Vec<Vec<Leaf>> = Vec::new();
    for round in 0..3 {
        let updated = update_group(&emulator, ACCOUNT, name, target).await;
        assert_eq!(
            updated.status, 204,
            "round {round}: expected the update to be accepted with 204, got {} {}",
            updated.status, updated.body
        );
        let read = get_group(&emulator, ACCOUNT, name).await;
        assert_eq!(
            sorted_leaves(&read.body),
            sorted(&target.expected(name, REGION)),
            "round {round}: the read must report the document the update recorded"
        );
        reported.push(sorted_leaves(&read.body));
    }
    assert!(
        reported.windows(2).all(|pair| pair[0] == pair[1]),
        "case {:?}: repeated updates reported different documents: {reported:?}",
        target.case
    );
}

#[tokio::test]
async fn every_value_a_partial_filter_condition_carries_is_reported_back() {
    // Each row is a document whose object-age or object-size condition carries
    // ONE of its two bounds, reached directly below `Filter` and below each of
    // its `And` / `Or` operators, since all three carry the same two
    // conditions. The bound the document DOES carry must come back with exactly
    // the value it was submitted with, whatever the update replaced.
    //
    // The sibling bound a row leaves out is reported by NEITHER name: a member the
    // document did not carry is not materialised on the response, which the
    // `storage_lens_group_absent_bounds` suite pins over the whole class of partial
    // conditions.
    let partial: Vec<(Group, &str, &str)> = vec![
        (
            Group {
                case: "only a lower size bound",
                leaves: vec![leaf("Filter/MatchObjectSize/BytesGreaterThan", "1024")],
            },
            "BytesGreaterThan",
            "1024",
        ),
        (
            Group {
                case: "only an upper size bound, past 32 bits",
                leaves: vec![leaf("Filter/MatchObjectSize/BytesLessThan", "5368709120")],
            },
            "BytesLessThan",
            "5368709120",
        ),
        (
            Group {
                case: "only a lower age bound",
                leaves: vec![leaf("Filter/MatchObjectAge/DaysGreaterThan", "30")],
            },
            "DaysGreaterThan",
            "30",
        ),
        (
            Group {
                case: "only an upper age bound",
                leaves: vec![leaf("Filter/MatchObjectAge/DaysLessThan", "365")],
            },
            "DaysLessThan",
            "365",
        ),
        (
            Group {
                case: "an And carrying one size bound beside a prefix",
                leaves: vec![
                    leaf("Filter/And/MatchAnyPrefix/Prefix", "logs/"),
                    leaf("Filter/And/MatchObjectSize/BytesGreaterThan", "1048576"),
                ],
            },
            "BytesGreaterThan",
            "1048576",
        ),
        (
            Group {
                case: "an Or carrying one age bound beside a suffix",
                leaves: vec![
                    leaf("Filter/Or/MatchAnySuffix/Suffix", ".tmp"),
                    leaf("Filter/Or/MatchObjectAge/DaysLessThan", "7"),
                ],
            },
            "DaysLessThan",
            "7",
        ),
    ];

    let emulator = Emulator::start();
    let full = &groups()[9];
    for (index, (group, bound, value)) in partial.iter().enumerate() {
        let case = group.case;
        let name = format!("partial-{index}");

        // The group starts from a document carrying EVERY criterion, so what
        // the read reports after the update is the updated document rather
        // than what was there before.
        let created = create_group(&emulator, ACCOUNT, &name, full, None).await;
        assert_eq!(
            created.status, 204,
            "case {case:?}: expected the create to be accepted with 204, got {} {}",
            created.status, created.body
        );

        let updated = update_group(&emulator, ACCOUNT, &name, group).await;
        assert_eq!(
            updated.status, 204,
            "case {case:?}: expected the update to be accepted with 204, got {} {}",
            updated.status, updated.body
        );

        let read = get_group(&emulator, ACCOUNT, &name).await;
        assert_eq!(
            read.texts(bound),
            vec![value.to_string()],
            "case {case:?}: expected {bound} to be reported as {value}, got {}",
            read.body
        );
        // The criteria the updated document does NOT carry are gone: the
        // document the create recorded was replaced, not merged into.
        for absent in ["Prefix", "Suffix", "Key", "Value"] {
            if group
                .leaves
                .iter()
                .any(|(path, _)| path.split('/').next_back() == Some(absent))
            {
                continue;
            }
            assert!(
                read.texts(absent).is_empty(),
                "case {case:?}: the update left {absent} from the previous document behind in {}",
                read.body
            );
        }
    }
}

#[tokio::test]
async fn the_updated_arn_follows_the_group_identity_and_the_signing_region() {
    let groups = groups();
    // The row that submits its own `StorageLensGroupArn`: whatever an update
    // carries in that member must be replaced by the ARN the service assigns.
    let submits_an_arn = groups
        .iter()
        .find(|group| group.case == "a document that already carries an ARN")
        .expect("the row that submits an ARN");

    for region in [REGION, OTHER_REGION] {
        for name in ["regional-group", "regional_group_2"] {
            let case = format!("region {region}, name {name:?}");
            // A fresh emulator per case keeps the name free, since a name is
            // unique within its account.
            let emulator = Emulator::start();
            let created = call_in_region(
                &emulator.router,
                region,
                "POST",
                "/v20180820/storagelensgroup",
                ACCOUNT,
                &groups[0].request_body(name, None),
            )
            .await;
            assert_eq!(
                created.status, 204,
                "case {case}: expected the create to be accepted with 204, got {} {}",
                created.status, created.body
            );

            let updated = call_in_region(
                &emulator.router,
                region,
                "PUT",
                &format!("/v20180820/storagelensgroup/{name}"),
                ACCOUNT,
                &update_body(&submits_an_arn.submitted(name)),
            )
            .await;
            assert_eq!(
                updated.status, 204,
                "case {case}: expected the update to be accepted with 204, got {} {}",
                updated.status, updated.body
            );

            let read = call_in_region(
                &emulator.router,
                region,
                "GET",
                &format!("/v20180820/storagelensgroup/{name}"),
                ACCOUNT,
                "",
            )
            .await;
            assert_eq!(
                read.texts("StorageLensGroupArn"),
                vec![expected_arn(name, region)],
                "case {case}: the group must report exactly the ARN the service assigns"
            );
        }
    }
}

#[tokio::test]
async fn updating_a_group_that_is_not_there_is_not_found_and_records_nothing() {
    let emulator = Emulator::start();
    let groups = groups();

    // A group really is there, so "not found" is about the name asked for
    // rather than about an empty store.
    let present = "present";
    let created = create_group(&emulator, ACCOUNT, present, &groups[0], None).await;
    assert_eq!(
        created.status, 204,
        "expected the create to be accepted with 204, got {} {}",
        created.status, created.body
    );

    // Each row is a name the account owns no group of: one differing only in
    // case, a longer one, a shorter one, and the longest a name can be.
    let longest = format!("u{}", "v".repeat(NAME_MAX_LEN - 1));
    for absent in [
        "PRESENT".to_string(),
        format!("{present}-2"),
        "p".to_string(),
        longest,
    ] {
        let refused = update_group(&emulator, ACCOUNT, &absent, &groups[3]).await;
        assert_eq!(
            (refused.status, refused.code().as_str()),
            (404, "NoSuchConfiguration"),
            "case {absent:?}: expected the update of a missing group to be refused, got {} {}",
            refused.status,
            refused.body
        );

        // An update is not a create: the refusal recorded no group.
        let read = get_group(&emulator, ACCOUNT, &absent).await;
        assert_eq!(
            read.status, 404,
            "case {absent:?}: the refused update recorded a group, answered {} {}",
            read.status, read.body
        );
    }

    // The group that is there is untouched by the refusals.
    let read = get_group(&emulator, ACCOUNT, present).await;
    assert_eq!(
        sorted_leaves(&read.body),
        sorted(&groups[0].expected(present, REGION)),
        "the refused updates changed the group that was there"
    );
}

#[tokio::test]
async fn an_update_whose_uri_and_document_name_different_groups_is_refused() {
    let groups = groups();
    let present = "named-in-the-uri";

    // Each row is a document name that disagrees with the URI's: another group
    // the account owns, a name it owns no group of, a case-only difference, and
    // the longest a name can be.
    let longest = format!("w{}", "x".repeat(NAME_MAX_LEN - 1));
    let other_owned = "also-owned";
    for document_name in [
        other_owned.to_string(),
        "owned-by-nobody".to_string(),
        "NAMED-IN-THE-URI".to_string(),
        longest,
    ] {
        let emulator = Emulator::start();
        for name in [present, other_owned] {
            let created = create_group(&emulator, ACCOUNT, name, &groups[0], None).await;
            assert_eq!(
                created.status, 204,
                "case {document_name:?}: expected the create of {name:?} to be accepted \
                 with 204, got {} {}",
                created.status, created.body
            );
        }

        let refused =
            update_group_as(&emulator, ACCOUNT, present, &document_name, &groups[5]).await;
        assert_eq!(
            (refused.status, refused.code().as_str()),
            (400, "InvalidRequestException"),
            "case {document_name:?}: an update naming two groups must be refused, got {} {}",
            refused.status,
            refused.body
        );

        // Neither the group the URI named nor the one the document named moved,
        // and no group was recorded under a name nobody owned.
        for name in [present, other_owned] {
            let read = get_group(&emulator, ACCOUNT, name).await;
            assert_eq!(
                sorted_leaves(&read.body),
                sorted(&groups[0].expected(name, REGION)),
                "case {document_name:?}: the refused update changed the group {name:?}"
            );
        }
        if document_name != other_owned {
            let read = get_group(&emulator, ACCOUNT, &document_name).await;
            assert_eq!(
                read.status, 404,
                "case {document_name:?}: the refused update recorded a group under the \
                 document's name, answered {} {}",
                read.status, read.body
            );
        }
    }
}

#[tokio::test]
async fn an_update_reaches_only_the_group_it_names() {
    let emulator = Emulator::start();
    let groups = groups();
    let shared = "shared-name";
    let untouched = "untouched";

    // Two names owned by the caller, both starting from the same document.
    for name in [shared, untouched] {
        let created = create_group(&emulator, ACCOUNT, name, &groups[1], None).await;
        assert_eq!(
            created.status, 204,
            "name {name:?}: expected the create to be accepted with 204, got {} {}",
            created.status, created.body
        );
    }

    let target = &groups[8];
    let updated = update_group(&emulator, ACCOUNT, shared, target).await;
    assert_eq!(
        updated.status, 204,
        "expected the update to be accepted with 204, got {} {}",
        updated.status, updated.body
    );

    // Only the name the request named changed.
    for (name, expected) in [(shared, target), (untouched, &groups[1])] {
        let read = get_group(&emulator, ACCOUNT, name).await;
        assert_eq!(
            sorted_leaves(&read.body),
            sorted(&expected.expected(name, REGION)),
            "name {name:?}: expected the document of case {:?}",
            expected.case
        );
    }

    // An update addressed in another account is refused ahead of the lookup, so
    // it replaces nothing: the document stays the one this account put.
    let theirs = &groups[2];
    let refused = update_group(&emulator, OTHER_ACCOUNT, shared, theirs).await;
    assert_eq!(
        (refused.status, refused.code().as_str()),
        (403, "AccessDenied"),
        "expected an update for another account to be refused, got {} {}",
        refused.status,
        refused.body
    );
    let read = get_group(&emulator, ACCOUNT, shared).await;
    assert_eq!(
        sorted_leaves(&read.body),
        sorted(&target.expected(shared, REGION)),
        "expected the refused update to leave the stored document alone"
    );
}
