// SPDX-License-Identifier: CC0-1.0

//! The AWS-managed cache policy catalogue as a MUTATION target, end to end over HTTP.
//!
//! The rule under test: a cache policy AWS published belongs to AWS, not to the account
//! reading it. Every account has the same ones, so a read of one is served exactly as a
//! read of a policy this account created; but nothing a caller does here creates, changes or
//! removes one, so a call that would mutate one is refused for what the REQUEST asked for —
//! `IllegalDelete` for the delete and `IllegalUpdate` for the update, both HTTP 400 — and
//! the policy keeps serving the configuration AWS gave it. That is a different answer from
//! "does not exist": a caller told the resource is missing would try to create one it must
//! not own.
//!
//! The refusal is a property of the TARGET rather than of the version stated, so it holds
//! whatever the request's `If-Match` says — the current version, a stale one, a value that
//! is no version token, or no header at all.
//!
//! The rows are EVERY entry of the published catalogue
//! ([`service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::MANAGED_CACHE_POLICIES`],
//! whose source the state module cites), beside a control row: a policy this account
//! creates is deletable, and the read after the delete is the 404 that means it really is
//! gone — which is what makes these two classes distinguishable.
//!
//! `ListCachePolicies`'s `managed` filter over the same catalogue is asserted by
//! [`crate::cache_policy_list`]; this suite is about the mutations and the single-policy
//! reads.
//!
//! The wire harness is the one [`crate::distribution_lifecycle`] builds.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    ILLEGAL_DELETE_MANAGED_POLICY_MESSAGE, ILLEGAL_UPDATE_MANAGED_POLICY_MESSAGE,
    MANAGED_CACHE_POLICIES, NO_SUCH_CACHE_POLICY_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cache_policy_lifecycle::{create_policy, get_policy, get_policy_config, Policy};
use crate::cache_policy_update::update_policy;
use crate::distribution_lifecycle::emulator;
use crate::distributions_by_cache_policy_id::delete_policy;

/// What a read of the policy `id` reports: `(Name, MinTTL, ETag)`, asserted served.
async fn read_policy(router: &MultiServiceRouter, id: &str, case: &str) -> (String, String, String) {
    let answer = get_policy(router, id).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected HTTP 200 from GetCachePolicy, got {} with {}",
        answer.status, answer.raw
    );
    let document = answer.root();
    assert_eq!(
        document.text_at("Id"),
        Some(id),
        "case {case}: expected the read to report Id {id:?}, got {}",
        answer.raw
    );
    (
        document
            .text_at("CachePolicyConfig/Name")
            .unwrap_or_default()
            .to_string(),
        document
            .text_at("CachePolicyConfig/MinTTL")
            .unwrap_or_default()
            .to_string(),
        answer.header("etag").to_string(),
    )
}

/// The `If-Match` forms a mutation of a published policy is refused under, whatever they
/// state: no header at all, the policy's current version, a well-formed version that is not
/// it, and a value that is no version token.
///
/// `None` is the row that states no header; `Some(None)` is the row that states the
/// policy's own current version, which only the test knows.
fn if_match_rows(current: &str) -> Vec<(&'static str, Option<String>)> {
    vec![
        ("no-if-match-at-all", None),
        ("current-version", Some(current.to_string())),
        ("stale-version", Some("E1STALEVERSION".to_string())),
        ("not-a-version-token", Some("not-a-version".to_string())),
    ]
}

/// A published policy is readable in this account and immune to both mutations: the delete
/// is `IllegalDelete`, the update is `IllegalUpdate`, and every read before, between and
/// after reports the one configuration AWS gave it at the one version.
#[tokio::test]
async fn a_published_cache_policy_is_readable_and_immutable() {
    let router = emulator();

    for managed in MANAGED_CACHE_POLICIES {
        let case = managed.name;
        let id = managed.id;

        // Read first: the catalogue is there in a store no call has written to, under the
        // identifier and name AWS publishes.
        let (name, min_ttl, etag) = read_policy(&router, id, case).await;
        assert_eq!(
            name, managed.name,
            "case {case}: expected the published Name, got {name:?}"
        );
        assert_eq!(
            min_ttl,
            managed.min_ttl.to_string(),
            "case {case}: expected the published MinTTL {}, got {min_ttl:?}",
            managed.min_ttl
        );
        assert!(
            !etag.is_empty(),
            "case {case}: expected a version token in the ETag header, got {etag:?}"
        );

        // `GetCachePolicyConfig` answers from the same catalogue entry.
        let config_answer = get_policy_config(&router, id).await;
        assert_eq!(
            config_answer.status, 200,
            "case {case}: expected HTTP 200 from GetCachePolicyConfig, got {} with {}",
            config_answer.status, config_answer.raw
        );
        assert_eq!(
            config_answer.root().text_at("Name"),
            Some(managed.name),
            "case {case}: expected the config read to report the published Name, got {}",
            config_answer.raw
        );

        for (how, if_match) in if_match_rows(&etag) {
            let deleted = delete_policy(&router, id, if_match.as_deref()).await;
            assert_eq!(
                deleted.status, 400,
                "case {case} ({how}): expected HTTP 400 from DeleteCachePolicy, got {} with {}",
                deleted.status, deleted.raw
            );
            assert_eq!(
                deleted.error_code(),
                "IllegalDelete",
                "case {case} ({how}): expected IllegalDelete, got {:?}",
                deleted.error_code()
            );
            let message = deleted.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, ILLEGAL_DELETE_MANAGED_POLICY_MESSAGE,
                "case {case} ({how}): expected the delete-not-allowed sentence, got {message:?}"
            );

            // The update a caller could otherwise land is refused on the same rule: the
            // configuration submitted is a valid one, so only the target decides the answer.
            let replacement = Policy {
                case: "a configuration a create would accept",
                name: format!("rewrite-{id}"),
                min_ttl: 7,
                ..Policy::default()
            };
            let updated = update_policy(&router, id, &replacement.xml(), if_match.as_deref()).await;
            assert_eq!(
                updated.status, 400,
                "case {case} ({how}): expected HTTP 400 from UpdateCachePolicy, got {} with {}",
                updated.status, updated.raw
            );
            assert_eq!(
                updated.error_code(),
                "IllegalUpdate",
                "case {case} ({how}): expected IllegalUpdate, got {:?}",
                updated.error_code()
            );
            let message = updated.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, ILLEGAL_UPDATE_MANAGED_POLICY_MESSAGE,
                "case {case} ({how}): expected the update-not-allowed sentence, got {message:?}"
            );

            // Still there, unchanged, at the same version: a refused mutation of a
            // published policy leaves the catalogue exactly as AWS publishes it.
            let (name_after, min_ttl_after, etag_after) = read_policy(&router, id, case).await;
            assert_eq!(
                (
                    name_after.as_str(),
                    min_ttl_after.as_str(),
                    etag_after.as_str()
                ),
                (name.as_str(), min_ttl.as_str(), etag.as_str()),
                "case {case} ({how}): expected the published policy to be unchanged after the \
                 refusals"
            );
        }
    }
}

/// The CONTROL class: a policy this account created is deletable with its own version, and
/// the read after the delete is `NoSuchCachePolicy` — so "delete is not allowed" and "does
/// not exist" really are two different answers about two different kinds of policy.
#[tokio::test]
async fn an_account_owned_cache_policy_is_deletable() {
    let router = emulator();
    let case = "a policy this account created";

    let policy = Policy {
        case: "a policy this account created",
        name: "account-owned-deletable".to_string(),
        min_ttl: 1,
        ..Policy::default()
    };
    let created = create_policy(&router, &policy.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {case}: expected HTTP 201 from CreateCachePolicy, got {} with {}",
        created.status, created.raw
    );
    let id = created
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();
    let etag = created.header("etag").to_string();

    let deleted = delete_policy(&router, &id, Some(&etag)).await;
    assert_eq!(
        deleted.status, 204,
        "case {case}: expected HTTP 204 from DeleteCachePolicy, got {} with {}",
        deleted.status, deleted.raw
    );

    let gone = get_policy(&router, &id).await;
    assert_eq!(
        gone.status, 404,
        "case {case}: expected HTTP 404 from the read after the delete, got {} with {}",
        gone.status, gone.raw
    );
    assert_eq!(
        gone.error_code(),
        "NoSuchCachePolicy",
        "case {case}: expected NoSuchCachePolicy after the delete, got {:?}",
        gone.error_code()
    );
    let message = gone.root().find_text("Message").unwrap_or_default();
    assert_eq!(
        message, NO_SUCH_CACHE_POLICY_MESSAGE,
        "case {case}: expected the does-not-exist sentence, got {message:?}"
    );
}

/// A cache policy id outside BOTH sources — the published catalogue and the account's own
/// records — names no policy at all, so a mutation of one is the 404 rather than either
/// illegal-mutation refusal. The ids are shaped like the catalogue's (a UUID) and unlike it.
#[tokio::test]
async fn an_id_in_neither_source_does_not_exist() {
    let router = emulator();

    for (case, id) in [
        ("uuid-shaped", "11111111-2222-3333-4444-555555555555"),
        ("not-an-id-at-all", "nope"),
    ] {
        let deleted = delete_policy(&router, id, Some("E1STALEVERSION")).await;
        assert_eq!(
            deleted.status, 404,
            "case {case}: expected HTTP 404 from DeleteCachePolicy for {id:?}, got {} with {}",
            deleted.status, deleted.raw
        );
        assert_eq!(
            deleted.error_code(),
            "NoSuchCachePolicy",
            "case {case}: expected NoSuchCachePolicy for {id:?}, got {:?}",
            deleted.error_code()
        );

        let read = get_policy(&router, id).await;
        assert_eq!(
            read.status, 404,
            "case {case}: expected HTTP 404 from GetCachePolicy for {id:?}, got {} with {}",
            read.status, read.raw
        );
    }
}

/// Every entry of the published catalogue is a distinct policy under a distinct identifier,
/// and the whole catalogue is readable through the one route — so a client that picks a
/// managed policy by id finds the one AWS publishes under it.
#[tokio::test]
async fn the_whole_published_catalogue_is_readable() {
    let router = emulator();
    let mut seen: Vec<(&str, &str)> = Vec::new();

    for managed in MANAGED_CACHE_POLICIES {
        assert!(
            !seen.iter().any(|(id, _)| *id == managed.id),
            "case {}: the catalogue repeats the id {}",
            managed.name,
            managed.id
        );
        assert!(
            !seen.iter().any(|(_, name)| *name == managed.name),
            "case {}: the catalogue repeats the name {}",
            managed.name,
            managed.name
        );
        seen.push((managed.id, managed.name));

        let answer = get_policy(&router, managed.id).await;
        assert_eq!(
            answer.status, 200,
            "case {}: expected HTTP 200 reading the published policy {}, got {} with {}",
            managed.name, managed.id, answer.status, answer.raw
        );
        assert_eq!(
            answer.root().text_at("CachePolicyConfig/Name"),
            Some(managed.name),
            "case {}: expected the published Name, got {}",
            managed.name,
            answer.raw
        );
    }
    assert!(
        !seen.is_empty(),
        "expected the published catalogue to hold at least one policy"
    );
}
