// SPDX-License-Identifier: CC0-1.0

//! `GetKeyGroupConfig` end to end over HTTP.
//!
//! The rule under test: a config read REPORTS the stored record of the resource an id
//! names — the configuration exactly as the last mutation stored it, beside the version
//! token that mutation handed back — it writes nothing, and an id no stored resource holds
//! is that resource's "no such" error. For a key group that means the `KeyGroupConfig`
//! payload and the `ETag` header are the same ones `CreateKeyGroup` handed back and
//! `GetKeyGroup` reports around the group's identity, a `Comment` the create left out stays
//! absent rather than becoming an empty element, and an id no group holds is
//! `NoSuchResource` (HTTP 404) with the absent-group sentence whatever shape the id takes —
//! this read's own recorded answer, which differs from `GetKeyGroup`'s.
//!
//! The rows are the key group table of `key_group_lifecycle.rs`, reused rather than copied:
//! they span what the rule generalises over — how many public key ids a group names (one,
//! two, and exactly the published quota), whether those ids are ones `CreatePublicKey`
//! minted or opaque strings a client submitted, and whether the comment is absent, empty,
//! plain ASCII, XML metacharacters or exactly the published 128-character bound. The
//! recorded case is one row.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::NO_SUCH_KEY_GROUP_MESSAGE;

use crate::distribution_lifecycle::{call, emulator, is_version_token, Answer, API_VERSION};
use crate::key_group_lifecycle::{create_group, get_group, mint_public_keys, table, Group};
use crate::response_headers_policy_lifecycle::text;

/// `GET /2020-05-31/key-group/{Id}/config`.
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

/// The config read answers the stored configuration and the stored version token: the same
/// ones the create handed back and `GetKeyGroup` reports, over and over, because a read
/// changes nothing a later caller sees.
#[tokio::test]
async fn the_config_read_reports_the_stored_configuration_and_its_version() {
    let router = emulator();

    for row in table(&router).await {
        let case = row.case.clone();
        let created = create_group(&router, &row.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201 from CreateKeyGroup, got {} with {}",
            created.status, created.raw
        );
        let id = text(created.root(), "Id", &case, "the create", &created.raw);
        let etag = created.header("etag").to_string();
        assert!(
            is_version_token(&etag),
            "case {case}: expected a version token from the create, got {etag:?}"
        );

        let read = get_group_config(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected HTTP 200 from GetKeyGroupConfig, got {} with {}",
            read.status, read.raw
        );
        // `KeyGroupConfig` is this output's `@httpPayload`, so the configuration IS the
        // document's root element.
        row.assert_matches(read.root(), "the config read", &read.raw);
        assert_eq!(
            read.header("etag"),
            etag,
            "case {case}: expected the config read to report the version the create handed \
             back ({etag:?}), got {:?}",
            read.header("etag")
        );

        // Parity with `GetKeyGroup`: both reads answer from the ONE record.
        let whole = get_group(&router, &id).await;
        assert_eq!(
            whole.status, 200,
            "case {case}: expected HTTP 200 from GetKeyGroup, got {} with {}",
            whole.status, whole.raw
        );
        let nested = whole.root().at("KeyGroupConfig").unwrap_or_else(|| {
            panic!(
                "case {case}: GetKeyGroup carried no KeyGroupConfig: {}",
                whole.raw
            )
        });
        row.assert_matches(nested, "GetKeyGroup parity", &whole.raw);
        assert_eq!(
            whole.header("etag"),
            etag,
            "case {case}: expected one version token across both reads, got {:?} and {etag:?}",
            whole.header("etag")
        );

        // A read writes nothing: the same request answers the same document and version.
        let again = get_group_config(&router, &id).await;
        assert_eq!(
            again.raw, read.raw,
            "case {case}: expected a repeated config read to answer the same document, got \
             {} then {}",
            read.raw, again.raw
        );
        assert_eq!(
            again.header("etag"),
            etag,
            "case {case}: expected the version to survive a read, got {:?}",
            again.header("etag")
        );
    }
}

/// An id no key group holds is `NoSuchResource` (HTTP 404) with the absent-group sentence,
/// whatever shape the id takes — this read's own recorded answer, which differs from
/// `GetKeyGroup`'s. The refusals leave the group that does exist readable at its stored
/// version.
#[tokio::test]
async fn the_config_read_refuses_an_id_no_key_group_holds() {
    let router = emulator();
    let minted = mint_public_keys(&router, "key-group-config-missing", 2).await;
    let present = Group {
        case: "the one group that exists".to_string(),
        name: "key-group-config-present".to_string(),
        items: minted.clone(),
        comment: Some("the stored group's comment".to_string()),
    };
    let created = create_group(&router, &present.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201 from CreateKeyGroup, got {} with {}",
        present.case, created.status, created.raw
    );
    let id = text(
        created.root(),
        "Id",
        &present.case,
        "the create",
        &created.raw,
    );
    let etag = created.header("etag").to_string();

    // Rows: a UUID never minted, the existing id with one character changed, the existing
    // id in upper case, an id from the public key namespace, one from the distribution
    // namespace, a token that is no id at all, and the group's NAME rather than its id. The
    // first two are key group ids the store holds nothing for; the rest were never key
    // group ids at all.
    let mut flipped: Vec<char> = id.chars().collect();
    let last = flipped.len() - 1;
    flipped[last] = if flipped[last] == 'a' { 'b' } else { 'a' };
    let absent: [(&str, String); 7] = [
        (
            "a UUID never minted",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "the existing id with one character changed",
            flipped.into_iter().collect(),
        ),
        ("the existing id in upper case", id.to_uppercase()),
        ("an id from the public key namespace", minted[0].clone()),
        (
            "an id from the distribution namespace",
            "E1AAAAAAAAAAAA".to_string(),
        ),
        ("a token that is no id at all", "not-a-key-group".to_string()),
        ("a key group name rather than an id", present.name.clone()),
    ];

    for (case, candidate) in &absent {
        let answer = get_group_config(&router, candidate).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404 for {candidate:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchResource",
            "case {case}: expected NoSuchResource for {candidate:?}, got {}",
            answer.raw
        );
        // The error names the resource the operation ADDRESSES, so the key group surface
        // answers the key group sentence rather than the tagging surface's generic one.
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, NO_SUCH_KEY_GROUP_MESSAGE,
            "case {case}: expected the key group not-found message for {candidate:?}, got \
             {message:?} in {}",
            answer.raw
        );
    }

    let read = get_group_config(&router, &id).await;
    assert_eq!(
        read.status, 200,
        "case {}: expected HTTP 200 after the refusals, got {} with {}",
        present.case, read.status, read.raw
    );
    present.assert_matches(read.root(), "after the refusals", &read.raw);
    assert_eq!(
        read.header("etag"),
        etag,
        "case {}: expected the version token to survive the refusals, got {:?}",
        present.case,
        read.header("etag")
    );
}
