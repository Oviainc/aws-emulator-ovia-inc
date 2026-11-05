// SPDX-License-Identifier: CC0-1.0

//! Deleting the tag set of an S3 Storage Lens configuration, driven end to end
//! over HTTP.
//!
//! The rule under test: a delete of a resource's tag set removes the WHOLE tag
//! set of the existing resource the URI names for the account the request
//! names, leaving the resource and every other resource's tag set untouched, so
//! a later tagging read reports no tag and a later tagging write starts from an
//! empty set; a resource the account does not own is refused with that
//! resource's not-found refusal instead.
//!
//! Every row below differs along something that rule generalises over: how many
//! tags the set held before the delete (none, one, several, the documented
//! maximum), which characters and lengths its keys and values used, the order
//! they were submitted in, which configuration document the tagged
//! configuration carries, how many times the delete is repeated, and which
//! account and configuration id the request names.
//!
//! The router, the request builder, the document and tag-set tables and the XML
//! readers are the access point and Storage Lens configuration suites', reused
//! rather than copied.

use crate::access_point_lifecycle::{Emulator, Reply, ACCOUNT, OTHER_ACCOUNT, REGION};
use crate::storage_lens_configuration::{
    documents, get_configuration, get_tagging, put_configuration, put_tagging, reported_tags,
    sorted, sorted_leaves, tag, tag_sets, Document, NO_CONTENT,
};

/// No tag at all, as a tagging read reports it.
fn no_tags() -> Vec<(String, String)> {
    Vec::new()
}

async fn delete_tagging(emulator: &Emulator, account_id: &str, config_id: &str) -> Reply {
    emulator
        .call(
            "DELETE",
            &format!("/v20180820/storagelens/{config_id}/tagging"),
            account_id,
            "",
        )
        .await
}

/// Put `document` under `config_id` with `tags`, failing the test loudly if the
/// setup itself did not take.
async fn tagged_configuration(
    emulator: &Emulator,
    account_id: &str,
    config_id: &str,
    document: &Document,
    tags: &[(String, String)],
) {
    let put = put_configuration(emulator, account_id, config_id, document, Some(tags)).await;
    assert_eq!(
        put.status, NO_CONTENT,
        "setting up {config_id:?}: expected {NO_CONTENT} from the put, got {} {}",
        put.status,
        put.body
    );
    let reported = get_tagging(emulator, account_id, config_id).await;
    assert_eq!(
        reported_tags(&reported.body),
        tags.to_vec(),
        "setting up {config_id:?}: the tag set was not stored"
    );
}

#[tokio::test]
async fn a_delete_removes_the_whole_tag_set_and_leaves_the_configuration() {
    let emulator = Emulator::start();
    let documents = documents();

    for (index, (case, tags)) in tag_sets().into_iter().enumerate() {
        let config_id = format!("delete-tagging-{index}");
        let document = &documents[index % documents.len()];
        tagged_configuration(&emulator, ACCOUNT, &config_id, document, &tags).await;

        let deleted = delete_tagging(&emulator, ACCOUNT, &config_id).await;
        assert_eq!(
            deleted.status, NO_CONTENT,
            "case {case:?}: expected {NO_CONTENT} from the tagging delete, got {} {}",
            deleted.status,
            deleted.body
        );

        let reported = get_tagging(&emulator, ACCOUNT, &config_id).await;
        assert_eq!(
            reported.status,
            200,
            "case {case:?}: expected 200 from the tagging read, got {} {}",
            reported.status,
            reported.body
        );
        assert_eq!(
            reported_tags(&reported.body),
            no_tags(),
            "case {case:?}: the delete must remove every tag, actual {:?}",
            reported_tags(&reported.body)
        );

        // The configuration itself is untouched: its document still reads back
        // exactly as the put stored it, and it is still there to be tagged.
        let read = get_configuration(&emulator, ACCOUNT, &config_id).await;
        assert_eq!(
            sorted_leaves(&read.body),
            sorted(&document.expected(&config_id, REGION)),
            "case {case:?}: the tagging delete changed the configuration document"
        );
    }
}

#[tokio::test]
async fn repeating_the_delete_is_the_same_answer_and_a_later_put_starts_from_empty() {
    let emulator = Emulator::start();
    let documents = documents();

    for (index, (case, tags)) in tag_sets().into_iter().enumerate() {
        let config_id = format!("repeat-{index}");
        let document = &documents[index % documents.len()];
        tagged_configuration(&emulator, ACCOUNT, &config_id, document, &tags).await;

        // The end state the call asks for is the state it leaves, so a delete
        // of a set already emptied answers exactly as the first one did —
        // including for a configuration that was never tagged (the `no tags at
        // all` row).
        for attempt in 1..=3 {
            let deleted = delete_tagging(&emulator, ACCOUNT, &config_id).await;
            assert_eq!(
                deleted.status, NO_CONTENT,
                "case {case:?} (delete {attempt}): expected {NO_CONTENT}, got {} {}",
                deleted.status,
                deleted.body
            );
            let reported = get_tagging(&emulator, ACCOUNT, &config_id).await;
            assert_eq!(
                reported_tags(&reported.body),
                no_tags(),
                "case {case:?} (delete {attempt}): tags came back, actual {:?}",
                reported_tags(&reported.body)
            );
        }

        // A tagging put after the delete is a fresh tag set rather than a merge
        // with the deleted one.
        let replacement = vec![tag("after", "delete"), tag("set", "")];
        let put = put_tagging(&emulator, ACCOUNT, &config_id, &replacement).await;
        assert_eq!(
            put.status, NO_CONTENT,
            "case {case:?}: expected {NO_CONTENT} re-tagging after the delete, got {} {}",
            put.status,
            put.body
        );
        let reported = get_tagging(&emulator, ACCOUNT, &config_id).await;
        assert_eq!(
            reported_tags(&reported.body),
            replacement,
            "case {case:?}: the tag set after the delete must be only the new tags, actual {:?}",
            reported_tags(&reported.body)
        );
    }
}

#[tokio::test]
async fn a_delete_touches_only_the_configuration_the_request_names() {
    let emulator = Emulator::start();
    let documents = documents();
    let mine = &documents[2];
    let theirs = &documents[5];

    // Three tagged configurations: two ids in one account, and the SAME id in
    // another account. Deleting one account's tag set must leave the other
    // configuration's and the other account's alone.
    let kept_tags = vec![tag("owner", "kept"), tag("sheet", "two")];
    let config_id = "shared-id";

    tagged_configuration(
        &emulator,
        ACCOUNT,
        config_id,
        mine,
        &[tag("owner", "mine")],
    )
    .await;
    tagged_configuration(&emulator, ACCOUNT, "kept-id", theirs, &kept_tags).await;

    let deleted = delete_tagging(&emulator, ACCOUNT, config_id).await;
    assert_eq!(
        deleted.status, NO_CONTENT,
        "expected {NO_CONTENT} from the tagging delete, got {} {}",
        deleted.status,
        deleted.body
    );

    for (case, account_id, id, expected) in [
        ("the named configuration", ACCOUNT, config_id, no_tags()),
        (
            "another configuration of the same account",
            ACCOUNT,
            "kept-id",
            kept_tags.clone(),
        ),
    ] {
        let reported = get_tagging(&emulator, account_id, id).await;
        assert_eq!(
            reported_tags(&reported.body),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            reported_tags(&reported.body)
        );
    }

    // The same delete addressed in another account is refused, and the
    // configuration it named is still readable here, so the refusal dropped
    // nothing.
    let refused = delete_tagging(&emulator, OTHER_ACCOUNT, config_id).await;
    assert_eq!(
        (refused.status, refused.code().as_str()),
        (403, "AccessDenied"),
        "expected a tagging delete for another account to be refused, got {} {}",
        refused.status,
        refused.body
    );
    let read = get_configuration(&emulator, ACCOUNT, config_id).await;
    assert_eq!(
        sorted_leaves(&read.body),
        sorted(&mine.expected(config_id, REGION)),
        "the tagging delete changed the configuration document"
    );
}

#[tokio::test]
async fn a_delete_against_a_configuration_that_is_not_there_is_not_found() {
    let emulator = Emulator::start();

    // A configuration really is there, so "not found" is about the id and the
    // account asked for rather than about an empty store.
    tagged_configuration(
        &emulator,
        ACCOUNT,
        "present",
        &documents()[0],
        &[tag("k", "v")],
    )
    .await;

    for (case, account_id, config_id) in [
        ("an id nothing was ever put under", ACCOUNT, "absent"),
        (
            "an id that differs from a stored one only in case",
            ACCOUNT,
            "Present",
        ),
    ] {
        let refused = delete_tagging(&emulator, account_id, config_id).await;
        assert_eq!(
            refused.status,
            404,
            "case {case:?}: expected 404, got {} {}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.code(),
            "NoSuchConfiguration",
            "case {case:?}: unexpected refusal {}",
            refused.body
        );
    }

    // A delete addressed in another account is refused ahead of the lookup, so
    // a stored id and an absent one are the same answer there.
    for (case, config_id) in [("a stored id", "present"), ("an absent id", "absent")] {
        let refused = delete_tagging(&emulator, OTHER_ACCOUNT, config_id).await;
        assert_eq!(
            (refused.status, refused.code().as_str()),
            (403, "AccessDenied"),
            "case {case:?}: expected a delete for another account to be refused, got {} {}",
            refused.status,
            refused.body
        );
    }

    // The configuration that IS there kept its tag set through every refusal.
    let reported = get_tagging(&emulator, ACCOUNT, "present").await;
    assert_eq!(
        reported_tags(&reported.body),
        vec![tag("k", "v")],
        "a refused delete changed another configuration's tag set, actual {:?}",
        reported_tags(&reported.body)
    );
}
