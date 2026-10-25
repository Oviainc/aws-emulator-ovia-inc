// SPDX-License-Identifier: CC0-1.0

//! `DeleteCachePolicy` against a cache policy AWS published, end to end over HTTP.
//!
//! The rule under test: which refusal a delete of a cache policy gets is decided by WHO owns
//! the target. A policy AWS created is read-only to the account that reads it — nothing a
//! caller does creates, changes or removes one — so the delete is refused for what the
//! request asked for (`IllegalDelete`, HTTP 400) whatever else is true of the request or of
//! the store, and the policy stays exactly as AWS publishes it; a policy this account
//! created is refused only while a stored cache behaviour attaches it (`CachePolicyInUse`,
//! HTTP 409) and is deletable once nothing does.
//!
//! That the refusal belongs to the TARGET is what this suite adds to
//! [`crate::managed_cache_policy_catalog`], which spans the versions a request can state
//! (none, the current one, a stale one, a value that is no version token): here the rows span
//! what the STORE says about the policy — which cache behaviour of a stored distribution
//! attaches it (the default one, one ordered one, the default and a later ordered one at
//! once, or nothing at all) — over every entry of the published catalogue, beside the
//! account-owned control under the same attachments. A caller told "in use" would detach and
//! retry, and a caller told "does not exist" would create its own; both are the wrong answer
//! for a policy that is simply not this account's to delete.
//!
//! The wire harness is [`crate::distribution_lifecycle`]'s, the cache policy requests are
//! [`crate::cache_policy_lifecycle`]'s, the delete is
//! [`crate::distributions_by_cache_policy_id`]'s and the listing is
//! [`crate::cache_policy_list`]'s, reused rather than copied.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    ILLEGAL_DELETE_MANAGED_POLICY_MESSAGE, MANAGED_CACHE_POLICIES, NO_SUCH_CACHE_POLICY_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cache_policy_list::{list_policies, page_ids};
use crate::cache_policy_lifecycle::{create_policy, get_policy, Policy};
use crate::distribution_lifecycle::{create_distribution, emulator, is_distribution_id};
use crate::distributions_by_cache_policy_id::{config_xml, delete_policy, Row};

/// What the store can say about the policy under the delete: which cache behaviour of a
/// stored distribution attaches it. Index 0 is the policy each case is about, the only one
/// the distribution is built against.
const ATTACHMENTS: &[Row] = &[
    Row {
        case: "no stored cache behaviour attaches the policy",
        default_policy: None,
        ordered: &[None],
    },
    Row {
        case: "the default cache behaviour attaches the policy",
        default_policy: Some(0),
        ordered: &[],
    },
    Row {
        case: "one ordered cache behaviour attaches the policy",
        default_policy: None,
        ordered: &[Some(0)],
    },
    Row {
        case: "the default behaviour and a later ordered one both attach the policy",
        default_policy: Some(0),
        ordered: &[None, Some(0)],
    },
];

/// Store one distribution whose cache behaviours attach `policy_id` the way `row` says, and
/// report its id.
async fn store_distribution(
    router: &MultiServiceRouter,
    row: &Row,
    policy_id: &str,
    case: &str,
) -> String {
    let policies = vec![policy_id.to_string()];
    let created = create_distribution(router, &config_xml("ref-delete-target", row, &policies)).await;
    assert_eq!(
        created.status, 201,
        "case {case} ({}): expected HTTP 201 from CreateDistribution, got {} with {}",
        row.case, created.status, created.raw
    );
    let id = created
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();
    assert!(
        is_distribution_id(&id),
        "case {case} ({}): expected a minted distribution id, got {id:?}",
        row.case
    );
    id
}

/// The `(MinTTL, ETag)` a read of the policy `id` reports, asserted served.
async fn read_policy(router: &MultiServiceRouter, id: &str, case: &str) -> (String, String) {
    let answer = get_policy(router, id).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected HTTP 200 from GetCachePolicy, got {} with {}",
        answer.status, answer.raw
    );
    (
        answer
            .root()
            .text_at("CachePolicyConfig/MinTTL")
            .unwrap_or_default()
            .to_string(),
        answer.header("etag").to_string(),
    )
}

/// A published policy is refused the delete for what the request ASKED FOR, whatever the
/// store says about it: attached by the default behaviour, by an ordered one, by both, or by
/// nothing at all, the answer is `IllegalDelete` and the policy is unchanged afterwards.
#[tokio::test]
async fn a_published_policy_is_refused_the_delete_whatever_attaches_it() {
    for managed in MANAGED_CACHE_POLICIES {
        for row in ATTACHMENTS {
            let case = managed.name;
            let router = emulator();
            store_distribution(&router, row, managed.id, case).await;

            // The version the request states is the policy's own current one, so nothing
            // about the precondition can be what refuses this delete.
            let (min_ttl, etag) = read_policy(&router, managed.id, case).await;
            let refused = delete_policy(&router, managed.id, Some(&etag)).await;
            assert_eq!(
                refused.status, 400,
                "case {case} ({}): expected HTTP 400 from DeleteCachePolicy, got {} with {}",
                row.case, refused.status, refused.raw
            );
            assert_eq!(
                refused.error_code(),
                "IllegalDelete",
                "case {case} ({}): expected IllegalDelete rather than an attachment or \
                 existence refusal, got {:?}",
                row.case,
                refused.error_code()
            );
            let message = refused.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, ILLEGAL_DELETE_MANAGED_POLICY_MESSAGE,
                "case {case} ({}): expected the delete-not-allowed sentence, got {message:?}",
                row.case
            );

            // The refusal left the published policy exactly where it was, at the same
            // version and the same configuration.
            let (min_ttl_after, etag_after) = read_policy(&router, managed.id, case).await;
            assert_eq!(
                (min_ttl_after.as_str(), etag_after.as_str()),
                (min_ttl.as_str(), etag.as_str()),
                "case {case} ({}): expected the refused delete to leave the policy unchanged",
                row.case
            );
        }
    }
}

/// The CONTROL class under the SAME attachments: a policy this account created is refused
/// only while a cache behaviour attaches it, and is deleted — really gone — when none does.
/// So the two refusals separate the two kinds of policy rather than the two kinds of
/// request.
#[tokio::test]
async fn an_account_owned_policy_under_the_same_attachments_is_the_in_use_refusal() {
    for row in ATTACHMENTS {
        let case = "a policy this account created";
        let router = emulator();

        let created = create_policy(
            &router,
            &Policy {
                case: "a policy this account created",
                name: "account-owned-under-the-delete-gate".to_string(),
                min_ttl: 3,
                ..Policy::default()
            }
            .xml(),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case} ({}): expected HTTP 201 from CreateCachePolicy, got {} with {}",
            row.case, created.status, created.raw
        );
        let id = created
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string();
        let etag = created.header("etag").to_string();

        store_distribution(&router, row, &id, case).await;

        let attached = row.default_policy == Some(0) || row.ordered.contains(&Some(0));
        let answer = delete_policy(&router, &id, Some(&etag)).await;
        if attached {
            assert_eq!(
                answer.status, 409,
                "case {case} ({}): expected HTTP 409 while a cache behaviour attaches it, got \
                 {} with {}",
                row.case, answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "CachePolicyInUse",
                "case {case} ({}): expected CachePolicyInUse, got {:?}",
                row.case,
                answer.error_code()
            );
            assert_eq!(
                get_policy(&router, &id).await.status,
                200,
                "case {case} ({}): expected the refused delete to leave the policy readable",
                row.case
            );
        } else {
            assert_eq!(
                answer.status, 204,
                "case {case} ({}): expected HTTP 204 when nothing attaches it, got {} with {}",
                row.case, answer.status, answer.raw
            );
            let gone = get_policy(&router, &id).await;
            assert_eq!(
                gone.status, 404,
                "case {case} ({}): expected HTTP 404 from the read after the delete, got {} \
                 with {}",
                row.case, gone.status, gone.raw
            );
            assert_eq!(
                gone.root().find_text("Message").unwrap_or_default(),
                NO_SUCH_CACHE_POLICY_MESSAGE,
                "case {case} ({}): expected the does-not-exist sentence after the delete, got {}",
                row.case,
                gone.raw
            );
        }
    }
}

/// A refused delete removes nothing: after one aimed at every entry of the published
/// catalogue, the listing still reports every one of them, and the account's own policy —
/// created beside them and never a delete's target — is still listed too.
#[tokio::test]
async fn every_refused_delete_leaves_the_collection_whole() {
    let router = emulator();
    let case = "the published catalogue after a delete aimed at each entry";

    let created = create_policy(
        &router,
        &Policy {
            case: "a policy this account created",
            name: "account-owned-bystander".to_string(),
            min_ttl: 5,
            ..Policy::default()
        }
        .xml(),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {case}: expected HTTP 201 from CreateCachePolicy, got {} with {}",
        created.status, created.raw
    );
    let own_id = created
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();

    for managed in MANAGED_CACHE_POLICIES {
        let refused = delete_policy(&router, managed.id, None).await;
        assert_eq!(
            refused.error_code(),
            "IllegalDelete",
            "case {case}: expected IllegalDelete for {}, got {:?}",
            managed.name,
            refused.error_code()
        );
    }

    let listed = list_policies(&router, "?Type=managed").await;
    assert_eq!(
        listed.status, 200,
        "case {case}: expected HTTP 200 from ListCachePolicies, got {} with {}",
        listed.status, listed.raw
    );
    let mut expected: Vec<String> = MANAGED_CACHE_POLICIES
        .iter()
        .map(|managed| managed.id.to_string())
        .collect();
    expected.sort();
    let mut served = page_ids(&listed);
    served.sort();
    assert_eq!(
        served, expected,
        "case {case}: expected every published policy to survive the refused deletes, got {}",
        listed.raw
    );

    let listed = list_policies(&router, "?Type=custom").await;
    assert_eq!(
        page_ids(&listed),
        vec![own_id.clone()],
        "case {case}: expected the account's own policy to be listed untouched, got {}",
        listed.raw
    );
}
