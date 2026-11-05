// SPDX-License-Identifier: CC0-1.0

//! Deleting an S3 Storage Lens configuration, driven end to end over HTTP.
//!
//! The rule under test: a delete of a resource removes the WHOLE resource the
//! URI names for the account the request names — the configuration document and
//! the tag set it carried — leaving every other account's and every other id's
//! resource untouched, so every later read of that id reports the resource's
//! not-found refusal, it is listed no more, and a resource later created under
//! the same id inherits nothing of the deleted one; a resource the account does
//! not own is refused with that same not-found refusal instead of being
//! answered. That refusal carries the fixed message S3 answers
//! `NoSuchConfiguration` with, naming no configuration id.
//!
//! Every row below differs along something that rule generalises over: which
//! configuration document the deleted configuration carried and how deeply it
//! nested, how many tags it held (none, one, several, the documented maximum)
//! and which characters and lengths their keys and values used, how long the
//! configuration id is and which of the characters the wire can address it by
//! it uses, how many configurations the account held beside it, which account
//! owns it, and how many times the delete is repeated.
//!
//! The router, the request builder, the document and tag-set tables and the XML
//! readers are the access point and Storage Lens configuration suites', reused
//! rather than copied.

use crate::access_point_lifecycle::{Emulator, Reply, ACCOUNT, OTHER_ACCOUNT, REGION};
use crate::storage_lens_configuration::{
    documents, get_configuration, get_tagging, put_configuration, reported_tags, sorted,
    sorted_leaves, tag, tag_sets, Document, NO_CONTENT,
};

/// What a refusal of a Storage Lens configuration the account does not hold
/// says: the fixed text S3 answers `NoSuchConfiguration` with, which names no
/// configuration id, so every row below expects the same sentence.
const NO_SUCH_CONFIGURATION: &str = "The specified configuration does not exist.";

/// Configuration ids the deleted rows are put under, spanning the lengths the
/// model declares for `ConfigId` (len 1..64, from the
/// `DeleteStorageLensConfiguration` input member) and the characters the wire
/// can address a configuration by, rather than one shape of id.
fn config_ids() -> Vec<String> {
    vec![
        "d".to_string(),
        "Dashboard-2".to_string(),
        "reports-monthly".to_string(),
        "reports_hourly".to_string(),
        "MixedCase_Id-With_Every-Addressable-Character_0123456789".to_string(),
        "y".repeat(64),
        "lens-delete".to_string(),
        "lens-delete-parquet".to_string(),
        "deleted-prefixes".to_string(),
        "deleted-had-an-arn".to_string(),
    ]
}

async fn delete_configuration(emulator: &Emulator, account_id: &str, config_id: &str) -> Reply {
    emulator
        .call(
            "DELETE",
            &format!("/v20180820/storagelens/{config_id}"),
            account_id,
            "",
        )
        .await
}

/// The configuration ids the account's list reports, in the order the answer
/// carries them. `Id` is the only element of a list entry that is an id, so the
/// entries' ids are the `Id` texts of the answer.
async fn listed_ids(emulator: &Emulator, account_id: &str) -> Vec<String> {
    let listed = emulator
        .call("GET", "/v20180820/storagelens", account_id, "")
        .await;
    assert_eq!(
        listed.status, 200,
        "expected 200 from the list, got {} {}",
        listed.status, listed.body
    );
    listed.texts("Id")
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
        put.status, put.body
    );
    let read = get_configuration(emulator, account_id, config_id).await;
    assert_eq!(
        read.status, 200,
        "setting up {config_id:?}: expected the configuration to be readable, got {} {}",
        read.status, read.body
    );
    let reported = get_tagging(emulator, account_id, config_id).await;
    assert_eq!(
        reported_tags(&reported.body),
        tags.to_vec(),
        "setting up {config_id:?}: the tag set was not stored"
    );
}

/// Every read of one configuration answers `NoSuchConfiguration` with HTTP 404.
async fn assert_gone(emulator: &Emulator, account_id: &str, config_id: &str, case: &str) {
    for (reader, reply) in [
        (
            "GetStorageLensConfiguration",
            get_configuration(emulator, account_id, config_id).await,
        ),
        (
            "GetStorageLensConfigurationTagging",
            get_tagging(emulator, account_id, config_id).await,
        ),
    ] {
        assert_eq!(
            reply.status, 404,
            "case {case:?} ({reader}): expected 404 after the delete, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            "NoSuchConfiguration",
            "case {case:?} ({reader}): unexpected refusal {}",
            reply.body
        );
        assert_eq!(
            reply.text("Message").as_deref(),
            Some(NO_SUCH_CONFIGURATION),
            "case {case:?} ({reader}): unexpected refusal message {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn a_delete_removes_the_configuration_and_its_tag_set() {
    let emulator = Emulator::start();
    let documents = documents();
    let ids = config_ids();

    for (index, (case, tags)) in tag_sets().into_iter().enumerate() {
        let config_id = &ids[index % ids.len()];
        let document = &documents[index % documents.len()];
        tagged_configuration(&emulator, ACCOUNT, config_id, document, &tags).await;

        let deleted = delete_configuration(&emulator, ACCOUNT, config_id).await;
        assert_eq!(
            deleted.status, NO_CONTENT,
            "case {case:?}: expected {NO_CONTENT} from the delete, got {} {}",
            deleted.status, deleted.body
        );
        assert!(
            deleted.body.trim().is_empty(),
            "case {case:?}: the operation models no output, so expected an empty body, got {}",
            deleted.body
        );

        // The configuration is gone for every reader of it, and it is listed no
        // more.
        assert_gone(&emulator, ACCOUNT, config_id, case).await;
        let listed = listed_ids(&emulator, ACCOUNT).await;
        assert!(
            !listed.contains(config_id),
            "case {case:?}: {config_id:?} is still listed after the delete, listed {listed:?}"
        );

        // Nothing of the deleted record survives for a configuration later put
        // under the same id: that put carries no `Tags`, so its tag set is
        // empty rather than the deleted configuration's.
        let replacement = &documents[(index + 1) % documents.len()];
        let put = put_configuration(&emulator, ACCOUNT, config_id, replacement, None).await;
        assert_eq!(
            put.status, NO_CONTENT,
            "case {case:?}: expected {NO_CONTENT} re-putting the id after the delete, got {} {}",
            put.status, put.body
        );
        let read = get_configuration(&emulator, ACCOUNT, config_id).await;
        assert_eq!(
            sorted_leaves(&read.body),
            sorted(&replacement.expected(config_id, REGION)),
            "case {case:?}: the re-put configuration is not the document put"
        );
        let reported = get_tagging(&emulator, ACCOUNT, config_id).await;
        assert_eq!(
            reported_tags(&reported.body),
            Vec::<(String, String)>::new(),
            "case {case:?}: the deleted tag set came back on the re-put configuration, actual {:?}",
            reported_tags(&reported.body)
        );
    }
}

#[tokio::test]
async fn a_delete_touches_only_the_configuration_the_request_names() {
    let emulator = Emulator::start();
    let documents = documents();
    let mine = &documents[1];
    let theirs = &documents[4];
    let config_id = "shared-id";

    // Two configurations in the caller's account: deleting one must leave the
    // other exactly as it was. A third call deletes the same id in another
    // account, which is refused before anything is removed.
    let kept_tags = vec![tag("owner", "kept"), tag("sheet", "two")];
    tagged_configuration(&emulator, ACCOUNT, config_id, mine, &[tag("owner", "mine")]).await;
    tagged_configuration(&emulator, ACCOUNT, "kept-id", theirs, &kept_tags).await;

    let refused = delete_configuration(&emulator, OTHER_ACCOUNT, config_id).await;
    assert_eq!(
        (refused.status, refused.code().as_str()),
        (403, "AccessDenied"),
        "expected a delete for another account to be refused, got {} {}",
        refused.status,
        refused.body
    );

    let deleted = delete_configuration(&emulator, ACCOUNT, config_id).await;
    assert_eq!(
        deleted.status, NO_CONTENT,
        "expected {NO_CONTENT} from the delete, got {} {}",
        deleted.status, deleted.body
    );

    assert_gone(&emulator, ACCOUNT, config_id, "the deleted configuration").await;

    for (case, account_id, id, document, expected_tags) in [(
        "another configuration of the same account",
        ACCOUNT,
        "kept-id",
        theirs,
        kept_tags.clone(),
    )] {
        let read = get_configuration(&emulator, account_id, id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            sorted_leaves(&read.body),
            sorted(&document.expected(id, REGION)),
            "case {case:?}: the delete changed this configuration's document"
        );
        let reported = get_tagging(&emulator, account_id, id).await;
        assert_eq!(
            reported_tags(&reported.body),
            expected_tags,
            "case {case:?}: the delete changed this configuration's tag set, actual {:?}",
            reported_tags(&reported.body)
        );
    }

    // The account's own listing lost exactly the deleted id.
    assert_eq!(
        listed_ids(&emulator, ACCOUNT).await,
        vec!["kept-id".to_string()],
        "the delete must drop only the configuration it names from the account's list"
    );
}

#[tokio::test]
async fn a_delete_of_a_configuration_that_is_not_there_is_not_found() {
    let emulator = Emulator::start();

    // A configuration really is there, so "not found" is about the id and the
    // account asked for rather than about an empty store.
    tagged_configuration(&emulator, ACCOUNT, "present", &documents()[0], &[tag("k", "v")]).await;

    for (case, account_id, config_id) in [
        ("an id nothing was ever put under", ACCOUNT, "absent"),
        (
            "an id that differs from a stored one only in case",
            ACCOUNT,
            "Present",
        ),
    ] {
        let refused = delete_configuration(&emulator, account_id, config_id).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected 404, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            "NoSuchConfiguration",
            "case {case:?}: unexpected refusal {}",
            refused.body
        );
        assert_eq!(
            refused.text("Message").as_deref(),
            Some(NO_SUCH_CONFIGURATION),
            "case {case:?}: unexpected refusal message {}",
            refused.body
        );
    }

    // A delete addressed in another account is refused ahead of the lookup, so
    // a stored id and an absent one are the same answer there.
    for (case, config_id) in [
        ("a stored id", "present"),
        ("an id nothing was ever put under", "absent"),
    ] {
        let refused = delete_configuration(&emulator, OTHER_ACCOUNT, config_id).await;
        assert_eq!(
            (refused.status, refused.code().as_str()),
            (403, "AccessDenied"),
            "case {case:?}: expected a delete for another account to be refused, got {} {}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.text("AccountId").as_deref(),
            Some(OTHER_ACCOUNT),
            "case {case:?}: expected the refusal to name the account asked for, got {}",
            refused.body
        );
    }

    // The configuration that IS there came through every refusal whole.
    let read = get_configuration(&emulator, ACCOUNT, "present").await;
    assert_eq!(
        sorted_leaves(&read.body),
        sorted(&documents()[0].expected("present", REGION)),
        "a refused delete changed the configuration that is there"
    );
    let reported = get_tagging(&emulator, ACCOUNT, "present").await;
    assert_eq!(
        reported_tags(&reported.body),
        vec![tag("k", "v")],
        "a refused delete changed another configuration's tag set, actual {:?}",
        reported_tags(&reported.body)
    );
}

#[tokio::test]
async fn repeating_the_delete_is_refused_and_a_later_put_restores_the_id() {
    let emulator = Emulator::start();
    let document = &documents()[2];
    let config_id = "repeat-delete";

    tagged_configuration(&emulator, ACCOUNT, config_id, document, &[tag("round", "one")]).await;
    let deleted = delete_configuration(&emulator, ACCOUNT, config_id).await;
    assert_eq!(
        deleted.status, NO_CONTENT,
        "expected {NO_CONTENT} from the first delete, got {} {}",
        deleted.status, deleted.body
    );

    // Once the configuration is gone there is nothing of that id to delete, so
    // every further delete is the same not-found refusal as a delete of an id
    // nothing was ever put under.
    for attempt in 1..=3 {
        let refused = delete_configuration(&emulator, ACCOUNT, config_id).await;
        assert_eq!(
            refused.status, 404,
            "delete {attempt} after the removal: expected 404, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            "NoSuchConfiguration",
            "delete {attempt} after the removal: unexpected refusal {}",
            refused.body
        );
        assert_eq!(
            refused.text("Message").as_deref(),
            Some(NO_SUCH_CONFIGURATION),
            "delete {attempt} after the removal: unexpected refusal message {}",
            refused.body
        );
    }

    // Deleting an id does not reserve it: a put stores a configuration under it
    // again, and that configuration is deletable in its turn.
    let tags = vec![tag("round", "two")];
    tagged_configuration(&emulator, ACCOUNT, config_id, document, &tags).await;
    let deleted = delete_configuration(&emulator, ACCOUNT, config_id).await;
    assert_eq!(
        deleted.status, NO_CONTENT,
        "expected {NO_CONTENT} deleting the re-put configuration, got {} {}",
        deleted.status, deleted.body
    );
    assert_gone(&emulator, ACCOUNT, config_id, "the re-put configuration").await;
}
