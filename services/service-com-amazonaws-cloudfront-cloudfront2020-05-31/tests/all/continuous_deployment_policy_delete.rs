// SPDX-License-Identifier: CC0-1.0

//! `DeleteContinuousDeploymentPolicy` end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront states for this operation
//! ("Deletes a continuous deployment policy. You cannot delete a continuous deployment
//! policy that's attached to a primary distribution. First update your distribution to
//! remove the continuous deployment policy, then you can delete the policy"):
//!
//! * an accepted delete answers HTTP 204 with no body and REMOVES the record: the id
//!   afterwards names no policy, the listing no longer carries it, and the staging
//!   distribution domain names it held are free for a later create to claim;
//! * an `If-Match` that is absent, carries no version token at all, or carries a
//!   well-formed token that is not the policy's current one is "missing or not valid" (HTTP
//!   400 `InvalidIfMatchVersion`) — what the header has to carry is this policy's current
//!   version; the FORM of the header is a property of the request, so a header no version
//!   could make usable is answered before the id is resolved, and so is a header for an id
//!   that is not even in the shape a policy id takes, which names no policy for any version
//!   to belong to; the current token is accepted bare, quoted or weak;
//! * a policy a distribution still names through its `ContinuousDeploymentPolicyId` is
//!   `ContinuousDeploymentPolicyInUse` (HTTP 409) and stays readable until the distribution
//!   is updated to drop it, after which the same delete lands;
//! * a well-formed policy id no stored policy holds is `NoSuchContinuousDeploymentPolicy`
//!   (HTTP 404), and every refused delete leaves the policy and its version token exactly as
//!   they were.
//!
//! The rows differ along everything these rules generalise over: how many staging
//! distribution domain names a policy names, whether it is enabled, which of the two traffic
//! configurations it carries (and whether it carries one at all), the form the `If-Match`
//! header takes, and the shape of an id no policy holds. The configuration table and its XML
//! writer are the crate's own, in `continuous_deployment_policies.rs`; the recorded case is
//! one row.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    CONTINUOUS_DEPLOYMENT_POLICY_IN_USE_MESSAGE, NO_SUCH_CONTINUOUS_DEPLOYMENT_POLICY_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::continuous_deployment_policies::{create_policy, get_policy, policy_table, staged,
    Policy};
use crate::distribution_lifecycle::{
    call, config, create_distribution, emulator, update_distribution, Answer, API_VERSION,
};

// ---------------------------------------------------------------------------
// Calling the delete
// ---------------------------------------------------------------------------

/// `DELETE /2020-05-31/continuous-deployment-policy/{Id}` with the `If-Match` header
/// `if_match` (left off entirely when `None`). The operation binds no payload.
async fn delete_policy(router: &MultiServiceRouter, id: &str, if_match: Option<&str>) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/{API_VERSION}/continuous-deployment-policy/{id}"),
        None,
        if_match,
    )
    .await
}

/// Create `policy` and report the id and version token the create minted.
async fn seed(router: &MultiServiceRouter, policy: &Policy) -> (String, String) {
    let created = create_policy(router, &policy.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201 from the create, got {} with {}",
        policy.case, created.status, created.raw
    );
    let id = created
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();
    let etag = created.header("etag").to_string();
    (id, etag)
}

/// Assert the policy `id` names is still readable at `etag`, carrying `policy`'s
/// configuration — what a refused delete has to leave behind.
async fn assert_still_stored(
    router: &MultiServiceRouter,
    id: &str,
    policy: &Policy,
    etag: &str,
    context: &str,
) {
    let case = policy.case;
    let fetched = get_policy(router, id).await;
    assert_eq!(
        fetched.status, 200,
        "case {case} ({context}): expected the refused delete to leave the policy readable, got \
         {} with {}",
        fetched.status, fetched.raw
    );
    let config = fetched
        .root()
        .at("ContinuousDeploymentPolicyConfig")
        .unwrap_or_else(|| {
            panic!(
                "case {case} ({context}): the read carries no ContinuousDeploymentPolicyConfig: {}",
                fetched.raw
            )
        });
    policy.assert_matches(config, context);
    assert_eq!(
        fetched.header("etag"),
        etag,
        "case {case} ({context}): expected the stored version token {etag}, got {:?}",
        fetched.header("etag")
    );
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// An accepted delete answers 204 with no body and removes the record: the id afterwards
/// names no policy, and the staging distribution domain names the policy held are free, so a
/// create that claims the same ones is accepted again.
///
/// Asserted over the whole configuration table, and over each form the current version token
/// takes in the header (bare, quoted, weak).
#[tokio::test]
async fn an_accepted_delete_removes_the_policy() {
    let router = emulator();

    for (index, policy) in staged(&router, "delete", policy_table()).await.iter().enumerate() {
        let case = policy.case;
        let form = ["{token}", "\"{token}\"", "W/\"{token}\""][index % 3];
        let (id, etag) = seed(&router, policy).await;

        let deleted = delete_policy(&router, &id, Some(&form.replace("{token}", &etag))).await;
        assert_eq!(
            deleted.status, 204,
            "case {case} (If-Match as {form}): expected HTTP 204, got {} with {}",
            deleted.status, deleted.raw
        );
        assert!(
            deleted.raw.trim().is_empty(),
            "case {case}: expected no body from the delete, got {}",
            deleted.raw
        );

        // Gone means gone: the id names no policy, and a second delete of it is the
        // modelled 404 rather than another 204 — the id is still in the shape a policy id
        // takes, so it is addressable and what the refusal reports is the absent policy.
        let read = get_policy(&router, &id).await;
        assert_eq!(
            read.status, 404,
            "case {case}: expected the deleted policy to be gone, got {} with {}",
            read.status, read.raw
        );
        let again = delete_policy(&router, &id, Some(&etag)).await;
        assert_eq!(
            again.status, 404,
            "case {case}: expected a second delete to report 404, got {} with {}",
            again.status, again.raw
        );
        assert_eq!(
            again.error_code(),
            "NoSuchContinuousDeploymentPolicy",
            "case {case}: expected NoSuchContinuousDeploymentPolicy, got {}",
            again.raw
        );

        // The staging distribution domain names the policy claimed are free again: the one
        // policy per staging distribution rule no longer counts the deleted record.
        let (reclaimed_id, _) = seed(&router, policy).await;
        assert_ne!(
            reclaimed_id, id,
            "case {case}: expected the re-create to mint a fresh id, got {reclaimed_id} again"
        );
    }
}

/// An `If-Match` the policy's version cannot satisfy is refused and removes nothing: the
/// header has to carry THIS policy's current version, so one that is absent, is no version
/// token at all, or is a well-formed token that is not the policy's is `InvalidIfMatchVersion`
/// (HTTP 400) — "the If-Match version is missing or not valid" covers a version that is not
/// this resource's. A header no version could make usable is answered even for an id no
/// policy holds, since the header is the REQUEST's; a usable one is answered on the id, which
/// is addressable as long as it is in the shape a policy id takes.
#[tokio::test]
async fn an_if_match_the_policy_cannot_satisfy_is_refused() {
    let router = emulator();
    let table = staged(&router, "deleteifm", policy_table()).await;
    let subject = &table[0];
    let other = &table[1];

    let (_, other_etag) = seed(&router, other).await;
    let (id, etag) = seed(&router, subject).await;

    // Each row: the header, and whether the value is a version TOKEN at all — which is what
    // decides whether an id no policy holds is answered on the header or on the id.
    let rows: [(&str, Option<String>, bool); 6] = [
        ("no If-Match at all", None, false),
        (
            "an If-Match that is no version token",
            Some("not-a-version".to_string()),
            false,
        ),
        ("an empty If-Match", Some(String::new()), false),
        (
            "an If-Match carrying a lowercase token",
            Some(etag.to_lowercase()),
            false,
        ),
        (
            "a version token that is another policy's",
            Some(other_etag.clone()),
            true,
        ),
        (
            "a well-formed token this service never minted",
            Some("E1Z2Y3X4W5V6U7".to_string()),
            true,
        ),
    ];

    for (case, if_match, usable_form) in &rows {
        let answer = delete_policy(&router, &id, if_match.as_deref()).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidIfMatchVersion",
            "case {case}: expected InvalidIfMatchVersion, got {}",
            answer.raw
        );
        assert_still_stored(&router, &id, subject, &etag, case).await;

        // The same header against a well-formed policy id no policy holds: a form no
        // version could make usable is answered on the header, a usable one on the id —
        // which is what a caller deleting an unknown policy with a bogus version is told.
        let unknown = delete_policy(
            &router,
            "00000000-0000-0000-0000-000000000000",
            if_match.as_deref(),
        )
        .await;
        let (expected_status, expected_code) = if *usable_form {
            (404, "NoSuchContinuousDeploymentPolicy")
        } else {
            (400, "InvalidIfMatchVersion")
        };
        assert_eq!(
            unknown.status, expected_status,
            "case {case} on an unknown id: expected HTTP {expected_status}, got {} with {}",
            unknown.status, unknown.raw
        );
        assert_eq!(
            unknown.error_code(),
            expected_code,
            "case {case} on an unknown id: expected {expected_code}, got {}",
            unknown.raw
        );
    }

    // The policy's own current token deletes it.
    let deleted = delete_policy(&router, &id, Some(&etag)).await;
    assert_eq!(
        deleted.status, 204,
        "case the current token: expected HTTP 204, got {} with {}",
        deleted.status, deleted.raw
    );
}

/// A policy a distribution still names cannot be deleted: the delete is refused with
/// `ContinuousDeploymentPolicyInUse` (HTTP 409) and the sentence that condition carries, the
/// policy stays readable at its version, and the same delete lands once the distribution is
/// updated to drop the policy. The distribution outlives the policy.
#[tokio::test]
async fn a_policy_a_distribution_names_cannot_be_deleted() {
    let router = emulator();
    let table = staged(&router, "deleteinuse", policy_table()).await;

    for (index, policy) in table.iter().enumerate().take(3) {
        let case = policy.case;
        let (id, etag) = seed(&router, policy).await;

        let row = config("in-use", &format!("ref-cdp-delete-{index}"));
        let created = create_distribution(&router, &row.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201 from CreateDistribution, got {} with {}",
            created.status, created.raw
        );
        let distribution_id = created
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string();
        let distribution_etag = created.header("etag").to_string();

        // `ContinuousDeploymentPolicyId` is the member a primary distribution attaches a
        // policy through, declared after the ones the row's document states.
        let attached = update_distribution(
            &router,
            &distribution_id,
            &row.xml().replace(
                "</DistributionConfig>",
                &format!(
                    "<ContinuousDeploymentPolicyId>{id}</ContinuousDeploymentPolicyId>\
                     </DistributionConfig>"
                ),
            ),
            Some(&distribution_etag),
        )
        .await;
        assert_eq!(
            attached.status, 200,
            "case {case}: expected HTTP 200 attaching the policy, got {} with {}",
            attached.status, attached.raw
        );
        let attached_etag = attached.header("etag").to_string();

        let refused = delete_policy(&router, &id, Some(&etag)).await;
        assert_eq!(
            refused.status, 409,
            "case {case}: expected HTTP 409 while the distribution names the policy, got {} with \
             {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "ContinuousDeploymentPolicyInUse",
            "case {case}: expected ContinuousDeploymentPolicyInUse, got {}",
            refused.raw
        );
        assert_eq!(
            refused.root().find_text("Message").unwrap_or_default(),
            CONTINUOUS_DEPLOYMENT_POLICY_IN_USE_MESSAGE,
            "case {case}: expected the modelled in-use sentence, got {}",
            refused.raw
        );
        assert_still_stored(&router, &id, policy, &etag, "while in use").await;

        // Dropping the policy from the distribution clears the way, and the same delete
        // then lands.
        let detached = update_distribution(
            &router,
            &distribution_id,
            &row.xml(),
            Some(&attached_etag),
        )
        .await;
        assert_eq!(
            detached.status, 200,
            "case {case}: expected HTTP 200 detaching the policy, got {} with {}",
            detached.status, detached.raw
        );
        let deleted = delete_policy(&router, &id, Some(&etag)).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected HTTP 204 once detached, got {} with {}",
            deleted.status, deleted.raw
        );
        assert_eq!(
            get_policy(&router, &id).await.status,
            404,
            "case {case}: expected the deleted policy to be gone"
        );
    }
}

/// An id no stored policy holds is `NoSuchContinuousDeploymentPolicy` (HTTP 404) with the
/// modelled sentence on the READ, whatever shape the id takes. The DELETE of the same id
/// answers on the `If-Match` where the id is not even in the shape a policy id takes — such
/// an id names no policy for any version to belong to — and on the absent policy where it
/// is. The policy that does exist is untouched either way.
#[tokio::test]
async fn an_id_no_policy_holds_is_refused() {
    let router = emulator();
    let table = staged(&router, "deleteabsent", policy_table()).await;
    let present = &table[0];
    let (id, etag) = seed(&router, present).await;

    let mut flipped: Vec<char> = id.chars().collect();
    let last = flipped.len() - 1;
    flipped[last] = if flipped[last] == 'a' { 'b' } else { 'a' };
    let rows: [(&str, String, u16, &str); 4] = [
        (
            "a UUID never minted",
            "00000000-0000-0000-0000-000000000000".to_string(),
            404,
            "NoSuchContinuousDeploymentPolicy",
        ),
        (
            "the existing id with one character changed",
            flipped.into_iter().collect(),
            404,
            "NoSuchContinuousDeploymentPolicy",
        ),
        (
            "the existing id in upper case",
            id.to_uppercase(),
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "a token that is no UUID",
            "not-a-policy-id".to_string(),
            400,
            "InvalidIfMatchVersion",
        ),
    ];

    for (case, candidate, delete_status, delete_code) in &rows {
        let read = get_policy(&router, candidate).await;
        assert_eq!(
            read.status, 404,
            "case {case}: expected HTTP 404 for {candidate:?}, got {} with {}",
            read.status, read.raw
        );
        assert_eq!(
            read.error_code(),
            "NoSuchContinuousDeploymentPolicy",
            "case {case}: expected NoSuchContinuousDeploymentPolicy for {candidate:?}, got {}",
            read.raw
        );
        assert_eq!(
            read.root().find_text("Message").unwrap_or_default(),
            NO_SUCH_CONTINUOUS_DEPLOYMENT_POLICY_MESSAGE,
            "case {case}: expected the modelled message, got {}",
            read.raw
        );

        let answer = delete_policy(&router, candidate, Some(&etag)).await;
        assert_eq!(
            answer.status, *delete_status,
            "case {case}: expected HTTP {delete_status} from the delete of {candidate:?}, got \
             {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            *delete_code,
            "case {case}: expected {delete_code} for {candidate:?}, got {}",
            answer.raw
        );
    }

    assert_still_stored(&router, &id, present, &etag, "after the refusals").await;
}
