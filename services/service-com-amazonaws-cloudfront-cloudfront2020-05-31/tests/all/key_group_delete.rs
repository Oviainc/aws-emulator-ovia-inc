// SPDX-License-Identifier: CC0-1.0

//! `DeleteKeyGroup`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront's delete states ("Deletes a
//! key group"):
//!
//! * the delete REMOVES the one stored record every read of that group answered from, so
//!   afterwards the id names no key group through either reader (`NoSuchResource`, HTTP 404
//!   with the key group surface's own sentence) and the name it held is free for a later
//!   create to claim under a new id;
//! * the delete is gated on the group's own version token: an absent `If-Match` and one that
//!   is no version token at all are `InvalidIfMatchVersion` (HTTP 400), a well-formed token
//!   that is not this group's current one is `PreconditionFailed` (HTTP 412), and in either
//!   case the group stays exactly where it was;
//! * "You cannot delete a key group that is referenced in a cache behavior": a group a
//!   stored distribution lists among a behaviour's trusted key groups is `ResourceInUse`
//!   (HTTP 409) and stays readable, and once no distribution references it the same delete
//!   lands;
//! * the removal releases what the group HELD: a public key a deleted group listed is no
//!   longer in use by it, so `DeletePublicKey` — refused `PublicKeyInUse` while the group
//!   stood — then removes the key and its own reader reports it gone.
//!
//! The rows span what those rules generalise over: every key group
//! [`crate::key_group_lifecycle::table`] builds (one, two and the published quota of public
//! key ids, ids `CreatePublicKey` minted and opaque ones a client submitted, comments that
//! are absent, empty, plain ASCII, XML metacharacters or exactly on the published bound);
//! every form an `If-Match` takes (absent, empty, bare, quoted, weak, lower case, another
//! group's token, a value that is no token); WHERE a reference sits (the default cache
//! behaviour, one ordered behaviour, both at once, a behaviour trusting two groups, and a
//! wrapper that is not `Enabled`); HOW the reference is released (an update that drops it or
//! a delete of the whole referencing distribution); and ids of several shapes the store
//! holds nothing for.
//!
//! The wire harness is [`crate::distribution_lifecycle`]'s, the key group writer and readers
//! are [`crate::key_group_lifecycle`]'s, the trusting distribution is
//! [`crate::distributions_by_key_group`]'s document and the public key calls are the public
//! key suites' — reused rather than copied.

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, create_distribution, delete_distribution, emulator, is_minted_uuid, is_version_token,
    past_propagation, update_distribution, Answer, API_VERSION,
};
use crate::distributions_by_key_group::{config_xml, Row};
use crate::key_group_lifecycle::{create_group, get_group, mint_public_keys, table, Group};
use crate::public_key_delete::delete_key;
use crate::public_key_lifecycle::get_key;

/// The sentences CloudFront sends for these refusals, spelled out here rather than taken from
/// the crate, so a reworded refusal fails this suite.
const NO_SUCH_KEY_GROUP_MESSAGE: &str = "The specified key group does not exist.";
const KEY_GROUP_IN_USE_MESSAGE: &str =
    "The specified key group is currently associated with a cache behavior. Please \
     disassociate the key group before deleting.";
const INVALID_IF_MATCH_VERSION_MESSAGE: &str =
    "The If-Match version is missing or not valid for the resource.";
/// The key group surface shares the POLICY family's precondition sentence.
const PRECONDITION_FAILED_MESSAGE: &str =
    "The request failed because it didn't meet the preconditions for this operation.";
const NO_SUCH_PUBLIC_KEY_MESSAGE: &str = "The specified CloudFront public key does not exist.";

/// How one row below spells a version token in the `If-Match` header: bare, as CloudFront's
/// own `ETag` header reports it, or quoted or weak, as HTTP spells an entity tag.
type Spelling = fn(&str) -> String;

// ---------------------------------------------------------------------------
// Calling the surface
// ---------------------------------------------------------------------------

/// `DELETE /2020-05-31/key-group/{Id}` under the version token `if_match` — the route and the
/// header the model binds this operation to (`IfMatch` travels in `If-Match`).
async fn delete_group(router: &MultiServiceRouter, id: &str, if_match: Option<&str>) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/{API_VERSION}/key-group/{id}"),
        None,
        if_match,
    )
    .await
}

/// `GET /2020-05-31/key-group/{Id}/config` — the second reader of the one record.
async fn get_group_config(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/key-group/{id}/config"),
        None,
        None,
    )
    .await
}

/// Create `row` and report its minted id and current version token.
async fn create(router: &MultiServiceRouter, row: &Group) -> (String, String) {
    let answer = create_group(router, &row.xml()).await;
    assert_eq!(
        answer.status, 201,
        "case {:?}: expected HTTP 201 from CreateKeyGroup, got {} with {}",
        row.case, answer.status, answer.raw
    );
    let id = answer.root().text_at("Id").unwrap_or_default().to_string();
    let etag = answer.header("etag").to_string();
    assert!(
        is_minted_uuid(&id),
        "case {:?}: expected a minted key group id, got {id:?}",
        row.case
    );
    assert!(
        is_version_token(&etag),
        "case {:?}: expected a minted version token, got {etag:?}",
        row.case
    );
    (id, etag)
}

/// Assert that `id` names no key group any more: both readers answer `NoSuchResource` with the
/// key group surface's own sentence.
async fn assert_gone(router: &MultiServiceRouter, id: &str, case: &str) {
    for (reader, answer) in [
        ("GetKeyGroup", get_group(router, id).await),
        ("GetKeyGroupConfig", get_group_config(router, id).await),
    ] {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404 from {reader} after the delete, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchResource",
            "case {case:?}: expected NoSuchResource from {reader} after the delete, got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, NO_SUCH_KEY_GROUP_MESSAGE,
            "case {case:?}: expected the modelled missing-group sentence from {reader}, got \
             {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The suites
// ---------------------------------------------------------------------------

/// Every key group the create accepts is removed by its delete under its version token: the
/// delete answers HTTP 204 with no body, both readers then report the id as naming nothing,
/// deleting it again is `NoSuchResource`, and the name it held is free for a NEW group under a
/// new id.
#[tokio::test]
async fn every_stored_key_group_is_removed_by_its_delete() {
    let router = emulator();

    for row in table(&router).await {
        let (id, etag) = create(&router, &row).await;

        // Round trip: the group is readable before the delete, so the 404 after it is the
        // delete's doing and not a group that was never stored.
        let fetched = get_group(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {:?}: expected HTTP 200 from GetKeyGroup before the delete, got {} with {}",
            row.case, fetched.status, fetched.raw
        );

        let deleted = delete_group(&router, &id, Some(&etag)).await;
        assert_eq!(
            deleted.status, 204,
            "case {:?}: expected HTTP 204 from DeleteKeyGroup, got {} with {}",
            row.case, deleted.status, deleted.raw
        );
        assert!(
            deleted.raw.trim().is_empty(),
            "case {:?}: expected no body from a 204, got {}",
            row.case,
            deleted.raw
        );

        assert_gone(&router, &id, &row.case).await;
        let again = delete_group(&router, &id, Some(&etag)).await;
        assert_eq!(
            again.status, 404,
            "case {:?}: expected HTTP 404 from a second delete, got {} with {}",
            row.case, again.status, again.raw
        );
        assert_eq!(
            again.error_code(),
            "NoSuchResource",
            "case {:?}: expected NoSuchResource from a second delete, got {}",
            row.case,
            again.raw
        );

        // The name the deleted group held identifies nothing any more, so a create under it
        // mints a SECOND group rather than being refused as a repeated name.
        let (reused_id, reused_etag) = create(&router, &row).await;
        assert_ne!(
            reused_id, id,
            "case {:?}: expected a freshly minted id for the group recreated under the freed \
             name, got the deleted one {id:?}",
            row.case
        );

        // Leave the store as this row found it, so the next row's name is the only one in
        // play.
        let cleanup = delete_group(&router, &reused_id, Some(&reused_etag)).await;
        assert_eq!(
            cleanup.status, 204,
            "case {:?}: expected HTTP 204 deleting the recreated group, got {} with {}",
            row.case, cleanup.status, cleanup.raw
        );
    }
}

/// The delete is gated on the group's OWN version token.
///
/// The rows span every form an `If-Match` takes: absent, empty, a value that is no version
/// token at all, a token spelled in lower case, a well-formed token belonging to another
/// group, and the current token bare, quoted and weak — the three ways HTTP and CloudFront's
/// own `ETag` header spell one.
#[tokio::test]
async fn the_version_token_gates_the_delete() {
    let router = emulator();
    let keys = mint_public_keys(&router, "key-group-delete-gate", 2).await;

    let other = Group {
        case: "another group, whose token is not the subject's".to_string(),
        name: "key-group-delete-gate-other".to_string(),
        items: keys[..1].to_vec(),
        comment: None,
    };
    let (_, other_etag) = create(&router, &other).await;

    let subject = Group {
        case: "the group under the version gate".to_string(),
        name: "key-group-delete-gate-subject".to_string(),
        items: keys.clone(),
        comment: Some("under the version gate".to_string()),
    };

    // Each refusal is asserted on its own freshly created group, and the group is read back
    // after it: a refused delete removes nothing.
    let refusals: &[(&str, Option<&str>, u16, &str, &str)] = &[
        (
            "no If-Match at all",
            None,
            400,
            "InvalidIfMatchVersion",
            INVALID_IF_MATCH_VERSION_MESSAGE,
        ),
        (
            "an If-Match that is no version token",
            Some("not-a-version"),
            400,
            "InvalidIfMatchVersion",
            INVALID_IF_MATCH_VERSION_MESSAGE,
        ),
        (
            "an empty If-Match",
            Some(""),
            400,
            "InvalidIfMatchVersion",
            INVALID_IF_MATCH_VERSION_MESSAGE,
        ),
        (
            "an If-Match carrying a lowercase token",
            Some("e1stale1version"),
            400,
            "InvalidIfMatchVersion",
            INVALID_IF_MATCH_VERSION_MESSAGE,
        ),
        (
            "a version token that is another group's",
            Some(other_etag.as_str()),
            412,
            "PreconditionFailed",
            PRECONDITION_FAILED_MESSAGE,
        ),
    ];

    for (case, if_match, status, code, message) in refusals {
        let (id, etag) = create(&router, &subject).await;
        let answer = delete_group(&router, &id, *if_match).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected HTTP {status}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            *code,
            "case {case:?}: expected {code}, got {}",
            answer.raw
        );
        let served = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            served, *message,
            "case {case:?}: expected the modelled sentence {message:?}, got {served:?}"
        );

        let fetched = get_group(&router, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected the refused delete to leave the group readable, got {} \
             with {}",
            fetched.status, fetched.raw
        );
        assert_eq!(
            fetched.header("etag"),
            etag,
            "case {case:?}: expected the refused delete to leave the version token alone, got \
             {:?}",
            fetched.header("etag")
        );

        // Clear the way for the next row, which creates a group under the same name.
        let cleanup = delete_group(&router, &id, Some(&etag)).await;
        assert_eq!(
            cleanup.status, 204,
            "case {case:?}: expected the current token to delete the group, got {} with {}",
            cleanup.status, cleanup.raw
        );
    }

    // The current token, spelled each of the three admissible ways, deletes the group.
    let spellings: &[(&str, Spelling)] = &[
        ("the current token, bare", |etag| etag.to_string()),
        ("the current token, quoted", |etag| format!("\"{etag}\"")),
        ("the current token, weak", |etag| format!("W/\"{etag}\"")),
    ];
    for (case, spell) in spellings {
        let (id, etag) = create(&router, &subject).await;
        let answer = delete_group(&router, &id, Some(&spell(&etag))).await;
        assert_eq!(
            answer.status, 204,
            "case {case:?}: expected HTTP 204, got {} with {}",
            answer.status, answer.raw
        );
        assert_gone(&router, &id, case).await;
    }
}

/// Where a reference can sit: the rows say which behaviours of one stored distribution trust
/// the group under the delete (index 0 is that group, index 1 a spare nothing is built
/// against).
const REFERENCES: &[Row] = &[
    Row {
        case: "no stored cache behaviour trusts the group",
        default_groups: &[],
        ordered: &[&[]],
        enabled: true,
    },
    Row {
        case: "the default cache behaviour trusts the group",
        default_groups: &[0],
        ordered: &[],
        enabled: true,
    },
    Row {
        case: "one ordered cache behaviour trusts the group",
        default_groups: &[],
        ordered: &[&[0]],
        enabled: true,
    },
    Row {
        case: "the default behaviour and a later ordered one both trust the group",
        default_groups: &[0],
        ordered: &[&[], &[0]],
        enabled: true,
    },
    Row {
        case: "one behaviour trusts the group together with another",
        default_groups: &[0, 1],
        ordered: &[&[]],
        enabled: true,
    },
    Row {
        case: "the wrapper names the group with Enabled false",
        default_groups: &[0],
        ordered: &[],
        enabled: false,
    },
];

/// Whether `row` names the key group at `index` in any of its behaviours — the rule the
/// handler implements, computed here from the row rather than from the handler.
fn references(row: &Row, index: usize) -> bool {
    row.default_groups.contains(&index) || row.ordered.iter().any(|groups| groups.contains(&index))
}

/// A key group a stored cache behaviour trusts cannot be deleted, and once no distribution
/// references it the same delete lands — the STORED `TrustedKeyGroups` list is the gate, and
/// the `Enabled` flag beside it says whether signed access is switched on rather than whether
/// the group is named.
///
/// The reference is released two ways, one per half of the suite: an update that drops it, and
/// a delete of the whole referencing distribution.
#[tokio::test]
async fn a_key_group_a_cache_behaviour_trusts_cannot_be_deleted() {
    for row in REFERENCES {
        let router = emulator();
        let (subject_id, subject_etag, groups) = seed_groups(&router).await;

        let created = create_distribution(
            &router,
            &config_xml("key-group-in-use-distribution", row, &groups),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected HTTP 201 from CreateDistribution, got {} with {}",
            row.case, created.status, created.raw
        );
        let distribution_id = created.root().text_at("Id").unwrap_or_default().to_string();
        let distribution_etag = created.header("etag").to_string();

        let answer = delete_group(&router, &subject_id, Some(&subject_etag)).await;
        if !references(row, 0) {
            // Nothing references the group, so the delete lands straight away — which is what
            // makes the refusals below the reference's doing rather than the distribution's
            // mere presence.
            assert_eq!(
                answer.status, 204,
                "case {:?}: expected HTTP 204 when no behaviour trusts the group, got {} with {}",
                row.case, answer.status, answer.raw
            );
            assert_gone(&router, &subject_id, row.case).await;
            continue;
        }

        assert_in_use(&answer, row.case);
        let fetched = get_group(&router, &subject_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {:?}: expected the refused delete to leave the group readable, got {} with {}",
            row.case, fetched.status, fetched.raw
        );

        // The reference is released by an update that drops it, and the SAME delete lands.
        let dropped = Row {
            case: row.case,
            default_groups: &[],
            ordered: &[&[]],
            enabled: row.enabled,
        };
        let updated = update_distribution(
            &router,
            &distribution_id,
            &config_xml("key-group-in-use-distribution", &dropped, &groups),
            Some(&distribution_etag),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected HTTP 200 from UpdateDistribution, got {} with {}",
            row.case, updated.status, updated.raw
        );

        let deleted = delete_group(&router, &subject_id, Some(&subject_etag)).await;
        assert_eq!(
            deleted.status, 204,
            "case {:?}: expected HTTP 204 once nothing references the group, got {} with {}",
            row.case, deleted.status, deleted.raw
        );
        assert_gone(&router, &subject_id, row.case).await;
    }
}

/// The OTHER way the reference is released: the whole referencing distribution is removed —
/// disabled, left to propagate, then deleted — after which the same delete lands. One row,
/// because what differs from the suite above is how the reference goes away rather than where
/// it sat.
#[tokio::test]
async fn a_key_group_is_deletable_once_the_trusting_distribution_is_gone() {
    let case = "the trusting distribution is deleted outright";
    let router = emulator();
    let (subject_id, subject_etag, groups) = seed_groups(&router).await;
    let row = &REFERENCES[1];

    let body = config_xml("key-group-in-use-distribution", row, &groups);
    let created = create_distribution(&router, &body).await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: expected HTTP 201 from CreateDistribution, got {} with {}",
        created.status, created.raw
    );
    let distribution_id = created.root().text_at("Id").unwrap_or_default().to_string();
    let distribution_etag = created.header("etag").to_string();

    assert_in_use(
        &delete_group(&router, &subject_id, Some(&subject_etag)).await,
        case,
    );

    // "You can delete a distribution only if it's disabled": the same configuration with
    // `Enabled` false, which is the one element of the document this changes.
    let disabled_body = body.replace(
        "<Enabled>true</Enabled></DistributionConfig>",
        "<Enabled>false</Enabled></DistributionConfig>",
    );
    assert_ne!(
        disabled_body, body,
        "case {case:?}: expected the disabling rewrite to change the submitted document"
    );
    let disabled = update_distribution(
        &router,
        &distribution_id,
        &disabled_body,
        Some(&distribution_etag),
    )
    .await;
    assert_eq!(
        disabled.status, 200,
        "case {case:?}: expected HTTP 200 disabling the distribution, got {} with {}",
        disabled.status, disabled.raw
    );
    tokio::time::sleep(past_propagation()).await;
    let removed = delete_distribution(
        &router,
        &distribution_id,
        Some(disabled.header("etag")),
    )
    .await;
    assert_eq!(
        removed.status, 204,
        "case {case:?}: expected HTTP 204 from DeleteDistribution, got {} with {}",
        removed.status, removed.raw
    );

    let deleted = delete_group(&router, &subject_id, Some(&subject_etag)).await;
    assert_eq!(
        deleted.status, 204,
        "case {case:?}: expected HTTP 204 once the trusting distribution is gone, got {} with {}",
        deleted.status, deleted.raw
    );
    assert_gone(&router, &subject_id, case).await;
}

/// Two key groups for the reference cases: the subject the rows reference at index 0, and a
/// spare at index 1 that only the two-group row names. Reports
/// `(subject id, subject version token, both ids)`.
async fn seed_groups(router: &MultiServiceRouter) -> (String, String, Vec<String>) {
    let keys = mint_public_keys(router, "key-group-in-use", 2).await;
    let subject = Group {
        case: "the group under the reference gate".to_string(),
        name: "key-group-in-use-subject".to_string(),
        items: keys[..1].to_vec(),
        comment: None,
    };
    let spare = Group {
        case: "a second group the rows may name beside the subject".to_string(),
        name: "key-group-in-use-spare".to_string(),
        items: keys[1..].to_vec(),
        comment: None,
    };
    let (subject_id, subject_etag) = create(router, &subject).await;
    let (spare_id, _) = create(router, &spare).await;
    (
        subject_id.clone(),
        subject_etag,
        vec![subject_id, spare_id],
    )
}

/// Assert `answer` is the modelled still-referenced refusal of this surface.
fn assert_in_use(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 409,
        "case {case:?}: expected HTTP 409 while a cache behaviour trusts the group, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "ResourceInUse",
        "case {case:?}: expected ResourceInUse while referenced, got {}",
        answer.raw
    );
    let served = answer.root().find_text("Message").unwrap_or_default();
    assert_eq!(
        served, KEY_GROUP_IN_USE_MESSAGE,
        "case {case:?}: expected the modelled in-use sentence, got {served:?}"
    );
}

/// The removal releases what the group HELD: while the group stands, the public keys it lists
/// cannot be removed (`PublicKeyInUse`); once the group is gone, the very same delete removes
/// each key and its own reader reports it gone.
///
/// The rows differ in how many keys one group lists and at which position of its list the key
/// under the delete sits, since the reference is the list and not one entry of it.
#[tokio::test]
async fn deleting_a_group_frees_the_public_keys_it_listed() {
    let cases: &[(&str, usize)] = &[
        ("a group listing one public key", 1),
        ("a group listing two public keys", 2),
        ("a group listing three public keys", 3),
    ];

    for (case, count) in cases {
        let router = emulator();
        let keys = mint_public_keys(&router, "key-group-release", *count).await;
        let group = Group {
            case: (*case).to_string(),
            name: format!("key-group-release-{count}"),
            items: keys.clone(),
            comment: None,
        };
        let (group_id, group_etag) = create(&router, &group).await;

        // Every key the group lists is in use by it, whichever position it sits at.
        for (position, key) in keys.iter().enumerate() {
            let read = get_key(&router, key).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected the public key at position {position} to be readable, \
                 got {} with {}",
                read.status, read.raw
            );
            let key_etag = read.header("etag").to_string();
            let refused = delete_key(&router, key, Some(&key_etag)).await;
            assert_eq!(
                refused.status, 409,
                "case {case:?}: expected HTTP 409 for the public key at position {position} \
                 while the group lists it, got {} with {}",
                refused.status, refused.raw
            );
            assert_eq!(
                refused.error_code(),
                "PublicKeyInUse",
                "case {case:?}: expected PublicKeyInUse for the public key at position \
                 {position}, got {}",
                refused.raw
            );
        }

        let deleted = delete_group(&router, &group_id, Some(&group_etag)).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected HTTP 204 from DeleteKeyGroup, got {} with {}",
            deleted.status, deleted.raw
        );
        assert_gone(&router, &group_id, case).await;

        // The group is gone, so nothing lists the keys any more and each delete lands.
        for (position, key) in keys.iter().enumerate() {
            let read = get_key(&router, key).await;
            let key_etag = read.header("etag").to_string();
            let removed = delete_key(&router, key, Some(&key_etag)).await;
            assert_eq!(
                removed.status, 204,
                "case {case:?}: expected HTTP 204 for the public key at position {position} \
                 once the group is gone, got {} with {}",
                removed.status, removed.raw
            );
            let gone = get_key(&router, key).await;
            assert_eq!(
                gone.status, 404,
                "case {case:?}: expected HTTP 404 from GetPublicKey after the key's delete, \
                 got {} with {}",
                gone.status, gone.raw
            );
            assert_eq!(
                gone.error_code(),
                "NoSuchPublicKey",
                "case {case:?}: expected NoSuchPublicKey after the key's delete, got {}",
                gone.raw
            );
            assert_eq!(
                gone.root().find_text("Message").unwrap_or_default(),
                NO_SUCH_PUBLIC_KEY_MESSAGE,
                "case {case:?}: expected the modelled missing-key sentence, got {}",
                gone.raw
            );
        }
    }
}

/// An id the store holds no key group for is `NoSuchResource`, whatever shape the id takes —
/// never a 204, which would claim something was removed.
#[tokio::test]
async fn an_id_no_key_group_holds_is_refused() {
    let router = emulator();
    let keys = mint_public_keys(&router, "key-group-absent", 1).await;
    let row = Group {
        case: "the only group the account holds".to_string(),
        name: "key-group-the-only-one".to_string(),
        items: keys.clone(),
        comment: None,
    };
    let (known, etag) = create(&router, &row).await;

    let mut flipped: Vec<char> = known.chars().collect();
    let last = flipped.len() - 1;
    flipped[last] = if flipped[last] == 'a' { 'b' } else { 'a' };
    let absent: [(&str, String); 5] = [
        (
            "a key group id never minted",
            "f9a6a0b4-8d39-4a1e-9f67-1b0f7f5d2c3e".to_string(),
        ),
        (
            "a known id with one character changed",
            flipped.into_iter().collect(),
        ),
        ("a known id in upper case", known.to_uppercase()),
        (
            "a token that is no key group id",
            "not-a-key-group-id".to_string(),
        ),
        ("the name rather than the id", row.name.clone()),
    ];

    for (case, candidate) in absent {
        let answer = delete_group(&router, &candidate, Some(&etag)).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404 for {candidate:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchResource",
            "case {case:?}: expected NoSuchResource for {candidate:?}, got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, NO_SUCH_KEY_GROUP_MESSAGE,
            "case {case:?}: expected the modelled missing-group sentence, got {message:?}"
        );
    }

    // None of those refusals touched the group the account really holds.
    let fetched = get_group(&router, &known).await;
    assert_eq!(
        fetched.status, 200,
        "case the only group the account holds: expected it to survive every refusal, got {} \
         with {}",
        fetched.status, fetched.raw
    );
}
