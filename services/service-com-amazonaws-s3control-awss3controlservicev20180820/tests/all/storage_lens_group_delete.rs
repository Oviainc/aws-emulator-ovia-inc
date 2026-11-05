// SPDX-License-Identifier: CC0-1.0

//! `DeleteStorageLensGroup`, driven end to end over HTTP.
//!
//! The rule under test: a delete removes the one Storage Lens group the (account, name)
//! pair it names and answers the modelled 204, so every later read of that name is the
//! missing-resource refusal, the name is free for a create again, and everything the
//! record carried — including the group's Amazon Web Services resource tags — goes with
//! it. A name the account owns no group of is refused as missing rather than answered as
//! a deletion that happened, and that refusal removes nothing.
//!
//! The rows span what the rule generalises over: the document the group was created with
//! (the whole create table, reused rather than copied), the length and character mix of
//! the name, whether the group carried tags, and whether another account owns a group of
//! the same name.
//!
//! The delete is implemented here because it is the end of the Storage Lens group round
//! trip the generic tagging surface is written against: a tagged group's tags are
//! released by its delete, so the two belong to one change.
//!
//! Every assertion goes through the same front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router, which deserializes it per the
//! Smithy model, runs the handler and serializes the restXml response.

use crate::access_point_lifecycle::{Emulator, Reply, ACCOUNT, OTHER_ACCOUNT, REGION};
use crate::storage_lens_configuration::{sorted, sorted_leaves};
use crate::storage_lens_group::{create_group, get_group, groups, names, NAME_MAX_LEN};

/// `DELETE /v20180820/storagelensgroup/<name>`.
async fn delete_group(emulator: &Emulator, account_id: &str, name: &str) -> Reply {
    emulator
        .call(
            "DELETE",
            &format!("/v20180820/storagelensgroup/{name}"),
            account_id,
            "",
        )
        .await
}

/// `GET /v20180820/tags/<arn>` — the group's tag set, read through the generic tagging
/// surface, which is what a deleted group must no longer have.
async fn list_tags(emulator: &Emulator, account_id: &str, resource_arn: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/v20180820/tags/{resource_arn}"),
            account_id,
            "",
        )
        .await
}

#[tokio::test]
async fn a_delete_removes_the_one_group_it_names_and_frees_its_name() {
    let groups = groups();
    let names = names();

    for (group, name) in groups.iter().zip(names) {
        let case = group.case;
        let emulator = Emulator::start();

        // A second group of this account, so a delete reaching more than the
        // pair it names would be visible.
        let kept_name = "kept-group".to_string();
        for created_name in [&name, &kept_name] {
            let created = create_group(&emulator, ACCOUNT, created_name, group, None).await;
            assert_eq!(
                created.status, 204,
                "case {case:?}: expected the create of {created_name} to be accepted with 204, \
                 got {} {}",
                created.status, created.body
            );
        }
        let read = get_group(&emulator, ACCOUNT, &name).await;
        let arn = read
            .text("StorageLensGroupArn")
            .unwrap_or_else(|| panic!("a StorageLensGroupArn in {}", read.body));

        let deleted = delete_group(&emulator, ACCOUNT, &name).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected the delete to be accepted with 204, got {} {}",
            deleted.status, deleted.body
        );

        // The group is gone: the read of that name is the missing-resource refusal, and
        // so is a read of its tags through the ARN it used to report.
        let read = get_group(&emulator, ACCOUNT, &name).await;
        assert_eq!(
            (read.status, read.code().as_str()),
            (404, "NoSuchConfiguration"),
            "case {case:?}: a deleted group must read as missing, got {} {}",
            read.status,
            read.body
        );
        let tags = list_tags(&emulator, ACCOUNT, &arn).await;
        assert_eq!(
            (tags.status, tags.code().as_str()),
            (404, "NoSuchResource"),
            "case {case:?}: a deleted group must carry no tags to read, got {} {}",
            tags.status,
            tags.body
        );

        // Deleting it again is refused as missing: the first delete removed it, and the
        // second has nothing to remove.
        let again = delete_group(&emulator, ACCOUNT, &name).await;
        assert_eq!(
            (again.status, again.code().as_str()),
            (404, "NoSuchConfiguration"),
            "case {case:?}: a repeated delete must be refused as missing, got {} {}",
            again.status,
            again.body
        );

        // The account's other group is untouched, and a delete addressed in
        // another account is refused ahead of the lookup.
        let kept = get_group(&emulator, ACCOUNT, &kept_name).await;
        assert_eq!(
            sorted_leaves(&kept.body),
            sorted(&group.expected(&kept_name, REGION)),
            "case {case:?}: the delete reached the account's other group"
        );
        let denied = delete_group(&emulator, OTHER_ACCOUNT, &kept_name).await;
        assert_eq!(
            (denied.status, denied.code().as_str()),
            (403, "AccessDenied"),
            "case {case:?}: expected a delete for another account to be refused, got {} {}",
            denied.status,
            denied.body
        );

        // The name is free again: a create under it is accepted rather than refused as a
        // conflict.
        let recreated = create_group(&emulator, ACCOUNT, &name, &groups[0], None).await;
        assert_eq!(
            recreated.status, 204,
            "case {case:?}: expected a create under the freed name to be accepted, got {} {}",
            recreated.status, recreated.body
        );
    }
}

#[tokio::test]
async fn deleting_a_group_that_is_not_there_is_refused_and_removes_nothing() {
    let emulator = Emulator::start();
    let groups = groups();

    // A group really is there, so "not found" is about the name asked for rather than
    // about an empty store.
    let present = "present";
    let created = create_group(&emulator, ACCOUNT, present, &groups[0], None).await;
    assert_eq!(
        created.status, 204,
        "expected the create to be accepted with 204, got {} {}",
        created.status, created.body
    );

    // Each row is a name the account owns no group of: one differing only in case, a
    // longer one, a shorter one, and the longest a name can be.
    let longest = format!("d{}", "e".repeat(NAME_MAX_LEN - 1));
    for absent in [
        "PRESENT".to_string(),
        format!("{present}-2"),
        "p".to_string(),
        longest,
    ] {
        let refused = delete_group(&emulator, ACCOUNT, &absent).await;
        assert_eq!(
            (refused.status, refused.code().as_str()),
            (404, "NoSuchConfiguration"),
            "case {absent:?}: expected the delete of a missing group to be refused, got {} {}",
            refused.status,
            refused.body
        );
    }

    // The group that is there is untouched by the refusals.
    let read = get_group(&emulator, ACCOUNT, present).await;
    assert_eq!(
        sorted_leaves(&read.body),
        sorted(&groups[0].expected(present, REGION)),
        "the refused deletes changed the group that was there"
    );
}
