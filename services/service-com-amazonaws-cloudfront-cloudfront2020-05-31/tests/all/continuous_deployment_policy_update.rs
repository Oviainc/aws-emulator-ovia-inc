// SPDX-License-Identifier: CC0-1.0

//! `UpdateContinuousDeploymentPolicy`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront states for the update of a
//! continuous deployment policy:
//!
//! * an accepted update REPLACES the stored configuration whole — "when you update a
//!   continuous deployment policy configuration, all the fields are updated with the values
//!   that are provided in the request. You cannot update some fields independent of others"
//!   — so a read afterwards answers with the submitted configuration down to the members it
//!   left out, under the id the policy was created with, beside the fresh version token and
//!   the `LastModifiedTime` the update handed back;
//! * the version token a mutation hands back is the only one an `If-Match` against that
//!   policy satisfies afterwards: an absent or malformed header is `InvalidIfMatchVersion`
//!   (HTTP 400), a well-formed token that is not the current one is `PreconditionFailed`
//!   (HTTP 412), and the current one — bare, quoted or weak — is accepted; so of two updates
//!   that start from ONE token exactly one lands and the other is refused;
//! * a mutation addressed to an id the store holds no record for is refused "does not exist"
//!   (`NoSuchContinuousDeploymentPolicy`, HTTP 404) BEFORE any precondition is evaluated, so
//!   a deleted id is unusable rather than merely unreadable;
//! * every constraint the create path refuses a configuration with holds on an update too (a
//!   `Quantity` that disagrees with its `Items`, a `TrafficConfig` missing the configuration
//!   its `Type` selects, a weight or a session stickiness TTL outside its published range,
//!   `IdleTTL` past `MaximumTTL`, a single-header name without the documented prefix, more
//!   staging distribution domain names than one policy may name), and a refused update
//!   writes NOTHING: the stored configuration, its version token and its
//!   `LastModifiedTime` are exactly what they were;
//! * a staging distribution takes part in at most ONE policy, so an update that moves a
//!   policy onto a domain name another policy already routes traffic to is
//!   `StagingDistributionInUse` (HTTP 409), while an update that keeps the policy's own
//!   domain names is not a conflict with itself.
//!
//! The rows span what those rules generalise over: every configuration of
//! [`crate::continuous_deployment_policies::policy_table`] (how many staging distribution
//! domain names the policy names, whether it is enabled, which of the two traffic
//! configurations it carries and whether it carries one at all, whether a single-weight
//! configuration names session stickiness, the weight and the TTLs at each published bound),
//! every form an `If-Match` takes (absent, empty, malformed, a lowercased token, another
//! policy's live token, a well-formed token never minted, the policy's own superseded token,
//! and the current one bare, quoted and weak), every form an id takes that names no policy,
//! and each documented configuration constraint at one value past its bound.
//!
//! The wire harness is the one [`crate::distribution_lifecycle`] builds and the policy
//! create, read, configuration table and configuration writer are
//! [`crate::continuous_deployment_policies`]'s, reused rather than copied.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    INVALID_IF_MATCH_VERSION_MESSAGE, MAX_SESSION_STICKINESS_TTL,
    MAX_STAGING_DISTRIBUTION_DNS_NAMES, MAX_TRAFFIC_WEIGHT, MIN_SESSION_STICKINESS_TTL,
    NO_SUCH_CONTINUOUS_DEPLOYMENT_POLICY_MESSAGE, PRECONDITION_FAILED_MESSAGE,
    STAGING_DISTRIBUTION_IN_USE_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::continuous_deployment_policies::{create_policy, get_policy, policy_table,
    staging_domains, Policy, Traffic};
use crate::distribution_lifecycle::{
    call, emulator, inconsistent_quantities_message, is_version_token, Answer, API_VERSION,
};

// ---------------------------------------------------------------------------
// Calling the update
// ---------------------------------------------------------------------------

/// `PUT /2020-05-31/continuous-deployment-policy/{Id}` with the
/// `ContinuousDeploymentPolicyConfig` document `body` and the `If-Match` header `if_match`
/// (left off entirely when `None`) — the route, the payload member and the header the model
/// binds this operation to.
///
/// `pub(crate)` so a sibling suite that updates a policy goes over this one wire path rather
/// than growing a copy of it.
pub(crate) async fn update_policy(
    router: &MultiServiceRouter,
    id: &str,
    body: &str,
    if_match: Option<&str>,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/continuous-deployment-policy/{id}"),
        Some(body),
        if_match,
    )
    .await
}

/// `DELETE /2020-05-31/continuous-deployment-policy/{Id}` under the version token
/// `if_match` — the call that makes an id name no policy any more.
async fn delete_policy(router: &MultiServiceRouter, id: &str, if_match: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/{API_VERSION}/continuous-deployment-policy/{id}"),
        None,
        Some(if_match),
    )
    .await
}

/// The same row over staging distributions this call CREATES, stamped with `suffix`, so two
/// rows of the table can be stored side by side without either claiming a staging
/// distribution the other holds.
///
/// A policy's `StagingDistributionDnsNames` names the staging DISTRIBUTION by the domain
/// name CloudFront assigned it, so a row that is to be stored names distributions this
/// service really holds rather than domain names written here.
async fn renamed(router: &MultiServiceRouter, policy: &Policy, suffix: &str) -> Policy {
    let mut row = policy.clone();
    row.dns_names = staging_domains(router, suffix, policy.dns_names.len()).await;
    row
}

/// Create `policy` and report the id and version token the create minted.
async fn seed(router: &MultiServiceRouter, policy: &Policy) -> (String, String) {
    let created = create_policy(router, &policy.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201 from CreateContinuousDeploymentPolicy, got {} with {}",
        policy.case, created.status, created.raw
    );
    let id = created.root().text_at("Id").unwrap_or_default().to_string();
    let etag = created.header("etag").to_string();
    (id, etag)
}

/// Assert that the policy `id` names reports `policy`'s configuration at the version token
/// `etag` — what an accepted update has to leave stored, and what a refused one must not
/// have touched.
async fn assert_stored(
    router: &MultiServiceRouter,
    id: &str,
    policy: &Policy,
    etag: &str,
    context: &str,
) {
    let case = policy.case;
    let read = get_policy(router, id).await;
    assert_eq!(
        read.status, 200,
        "case {case} ({context}): expected the policy to be readable, got {} with {}",
        read.status, read.raw
    );
    assert_eq!(
        read.root().text_at("Id"),
        Some(id),
        "case {case} ({context}): expected the id {id}, got {:?}",
        read.root().text_at("Id")
    );
    let config = read
        .root()
        .at("ContinuousDeploymentPolicyConfig")
        .unwrap_or_else(|| {
            panic!(
                "case {case} ({context}): the read carries no ContinuousDeploymentPolicyConfig: {}",
                read.raw
            )
        });
    policy.assert_matches(config, context);
    assert_eq!(
        read.header("etag"),
        etag,
        "case {case} ({context}): expected the stored version token {etag}, got {:?}",
        read.header("etag")
    );
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// An accepted update replaces the stored configuration WHOLE: the read afterwards answers
/// with the submitted configuration down to the members it left out, under the id the create
/// minted, and the fresh version token the update handed back is the one the read reports.
/// Asserted over every pair (row, next row) of the table, so each case moves the policy
/// between two configurations that differ in the traffic configuration they carry, the number
/// of staging distribution domain names, the enabled flag and the stickiness TTLs.
#[tokio::test]
async fn an_accepted_update_replaces_the_stored_configuration() {
    let router = emulator();
    let table = policy_table();
    let mut tokens = BTreeSet::new();

    for (index, policy) in table.iter().enumerate() {
        let case = policy.case;
        let before = renamed(&router, policy, &format!("u{index}a")).await;
        let after = renamed(&router, &table[(index + 1) % table.len()], &format!("u{index}b")).await;
        let (id, created_etag) = seed(&router, &before).await;
        assert_stored(&router, &id, &before, &created_etag, "after the create").await;

        // The current token is accepted bare, quoted and weak: the header is an HTTP entity
        // tag, and CloudFront reports it bare in the `ETag` header.
        let form = ["{token}", "\"{token}\"", "W/\"{token}\""][index % 3];
        let updated = update_policy(
            &router,
            &id,
            &after.xml(),
            Some(&form.replace("{token}", &created_etag)),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case} (If-Match as {form}): expected HTTP 200, got {} with {}",
            updated.status, updated.raw
        );
        assert_eq!(
            updated.root().text_at("Id"),
            Some(id.as_str()),
            "case {case}: expected the update to keep the id {id}, got {:?}",
            updated.root().text_at("Id")
        );
        let updated_etag = updated.header("etag").to_string();
        assert!(
            is_version_token(&updated_etag),
            "case {case}: expected a version token from the update, got {updated_etag:?}"
        );
        assert_ne!(
            updated_etag, created_etag,
            "case {case}: expected the update to mint a fresh version token, got \
             {created_etag} again"
        );
        assert!(
            tokens.insert(updated_etag.clone()),
            "case {case}: expected a version token no other policy holds, got {updated_etag}"
        );
        let last_modified = updated
            .root()
            .text_at("LastModifiedTime")
            .unwrap_or_default()
            .to_string();
        assert!(
            !last_modified.is_empty(),
            "case {case}: expected a LastModifiedTime from the update, got {}",
            updated.raw
        );

        // The body the update answered with is the record it wrote: the configuration it
        // reports is the submitted one, and the read agrees with it member for member.
        let written = updated
            .root()
            .at("ContinuousDeploymentPolicyConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: the update carries no ContinuousDeploymentPolicyConfig: {}",
                    updated.raw
                )
            });
        after.assert_matches(written, "in the update's own answer");
        assert_stored(&router, &id, &after, &updated_etag, "after the update").await;
        let read = get_policy(&router, &id).await;
        assert_eq!(
            read.root().text_at("LastModifiedTime"),
            Some(last_modified.as_str()),
            "case {case}: expected the read to report the written LastModifiedTime \
             {last_modified}, got {:?}",
            read.root().text_at("LastModifiedTime")
        );
    }
}

/// The version token the last mutation handed back is the only one an `If-Match` against the
/// policy satisfies: a header that is absent or is no version token at all is
/// `InvalidIfMatchVersion` (HTTP 400), a well-formed token that is not the current one —
/// another policy's, one never minted, or this policy's own superseded token — is
/// `PreconditionFailed` (HTTP 412), and every refusal writes nothing.
#[tokio::test]
async fn an_if_match_that_is_not_the_current_version_is_refused() {
    let router = emulator();
    let table = policy_table();
    let subject = renamed(&router, &table[2], "ifm-a").await;
    let other = renamed(&router, &table[3], "ifm-b").await;
    let target = renamed(&router, &table[0], "ifm-c").await;

    let (_, other_etag) = seed(&router, &other).await;
    let (id, first_etag) = seed(&router, &subject).await;

    // One accepted update first, so the policy carries a SUPERSEDED token as well as a
    // current one: a token this service really minted for this policy, and no longer
    // satisfies.
    let moved = update_policy(&router, &id, &target.xml(), Some(&first_etag)).await;
    assert_eq!(
        moved.status, 200,
        "case the current token: expected HTTP 200, got {} with {}",
        moved.status, moved.raw
    );
    let current_etag = moved.header("etag").to_string();

    // Each row: the header, and the refusal it earns — a form no version could make valid,
    // or a well-formed token that is not this policy's current one.
    let rows: [(&str, Option<String>, u16, &str, &str); 7] = [
        (
            "no If-Match at all",
            None,
            400,
            "InvalidIfMatchVersion",
            INVALID_IF_MATCH_VERSION_MESSAGE,
        ),
        (
            "an empty If-Match",
            Some(String::new()),
            400,
            "InvalidIfMatchVersion",
            INVALID_IF_MATCH_VERSION_MESSAGE,
        ),
        (
            "an If-Match that is no version token",
            Some("not-a-version".to_string()),
            400,
            "InvalidIfMatchVersion",
            INVALID_IF_MATCH_VERSION_MESSAGE,
        ),
        (
            "an If-Match carrying a lowercased token",
            Some(current_etag.to_lowercase()),
            400,
            "InvalidIfMatchVersion",
            INVALID_IF_MATCH_VERSION_MESSAGE,
        ),
        (
            "a version token that is another policy's",
            Some(other_etag.clone()),
            412,
            "PreconditionFailed",
            PRECONDITION_FAILED_MESSAGE,
        ),
        (
            "a well-formed token this service never minted",
            Some("E1Z2Y3X4W5V6U7".to_string()),
            412,
            "PreconditionFailed",
            PRECONDITION_FAILED_MESSAGE,
        ),
        (
            "the policy's own superseded token",
            Some(first_etag.clone()),
            412,
            "PreconditionFailed",
            PRECONDITION_FAILED_MESSAGE,
        ),
    ];

    for (case, if_match, status, code, message) in &rows {
        let refused = update_policy(&router, &id, &subject.xml(), if_match.as_deref()).await;
        assert_eq!(
            refused.status, *status,
            "case {case}: expected HTTP {status}, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            *code,
            "case {case}: expected {code}, got {}",
            refused.raw
        );
        assert_eq!(
            refused.root().find_text("Message").unwrap_or_default(),
            *message,
            "case {case}: expected the modelled sentence, got {}",
            refused.raw
        );
        // Nothing was written: the configuration the accepted update stored is still the
        // one the read answers with, at the token it minted.
        assert_stored(&router, &id, &target, &current_etag, case).await;
    }

    // The current token still lands, so the refusals above spent nothing.
    let accepted = update_policy(&router, &id, &subject.xml(), Some(&current_etag)).await;
    assert_eq!(
        accepted.status, 200,
        "case the current token after the refusals: expected HTTP 200, got {} with {}",
        accepted.status, accepted.raw
    );
    assert_stored(
        &router,
        &id,
        &subject,
        accepted.header("etag"),
        "after the accepted update",
    )
    .await;
}

/// An id no stored policy holds is `NoSuchContinuousDeploymentPolicy` (HTTP 404) with the
/// modelled sentence — whatever shape the id takes and whatever the `If-Match` header
/// carries, since the id is what the mutation is about — and the policy that does exist is
/// untouched.
#[tokio::test]
async fn an_id_no_policy_holds_is_refused() {
    let router = emulator();
    let table = policy_table();
    let present = renamed(&router, &table[1], "missing-a").await;
    let deleted = renamed(&router, &table[4], "missing-b").await;

    let (id, etag) = seed(&router, &present).await;
    let (gone_id, gone_etag) = seed(&router, &deleted).await;
    assert_eq!(
        delete_policy(&router, &gone_id, &gone_etag).await.status,
        204,
        "case a deleted policy: expected HTTP 204 from the delete"
    );

    let mut flipped: Vec<char> = id.chars().collect();
    let last = flipped.len() - 1;
    flipped[last] = if flipped[last] == 'a' { 'b' } else { 'a' };
    let rows: [(&str, String); 5] = [
        ("an id whose policy was deleted", gone_id.clone()),
        (
            "a UUID never minted",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "the existing id with one character changed",
            flipped.into_iter().collect(),
        ),
        ("the existing id in upper case", id.to_uppercase()),
        ("a token that is no UUID", "not-a-policy-id".to_string()),
    ];

    for (case, candidate) in &rows {
        // Both a usable header form and one no version could make valid: the id is decided
        // first either way.
        for (header_case, if_match) in [
            ("with the live policy's token", Some(etag.as_str())),
            ("with no If-Match at all", None),
        ] {
            let refused = update_policy(&router, candidate, &present.xml(), if_match).await;
            assert_eq!(
                refused.status, 404,
                "case {case} ({header_case}): expected HTTP 404 for {candidate:?}, got {} with {}",
                refused.status, refused.raw
            );
            assert_eq!(
                refused.error_code(),
                "NoSuchContinuousDeploymentPolicy",
                "case {case} ({header_case}): expected NoSuchContinuousDeploymentPolicy, got {}",
                refused.raw
            );
            assert_eq!(
                refused.root().find_text("Message").unwrap_or_default(),
                NO_SUCH_CONTINUOUS_DEPLOYMENT_POLICY_MESSAGE,
                "case {case} ({header_case}): expected the modelled message, got {}",
                refused.raw
            );
        }
    }

    assert_stored(&router, &id, &present, &etag, "after the refusals").await;
}

/// Every configuration constraint the create path refuses a payload with holds on an update
/// too, and a refused update writes nothing. One row per documented constraint, each at a
/// value one past its published bound; the bounds themselves are
/// [`crate::continuous_deployment_policies::policy_table`]'s rows, which this suite accepts
/// above.
#[tokio::test]
async fn a_configuration_the_create_would_refuse_is_refused() {
    let router = emulator();
    let table = policy_table();
    let stored = renamed(&router, &table[2], "bad-a").await;
    let (id, etag) = seed(&router, &stored).await;

    // The `Quantity` of the one list wrapper this configuration carries, stated as one more
    // than the number of `Items` the document really carries.
    let names = stored.dns_names.len();
    let mismatched = stored.xml().replace(
        &format!("<Quantity>{names}</Quantity>"),
        &format!("<Quantity>{}</Quantity>", names + 1),
    );
    let refused = update_policy(&router, &id, &mismatched, Some(&etag)).await;
    assert_eq!(
        refused.status, 400,
        "case a Quantity that disagrees with its Items: expected HTTP 400, got {} with {}",
        refused.status, refused.raw
    );
    assert_eq!(
        refused.error_code(),
        "InconsistentQuantities",
        "case a Quantity that disagrees with its Items: expected InconsistentQuantities, got {}",
        refused.raw
    );
    assert_eq!(
        refused.root().find_text("Message").unwrap_or_default(),
        inconsistent_quantities_message(
            "StagingDistributionDnsNames",
            i32::try_from(names + 1).expect("a small table"),
            names
        ),
        "case a Quantity that disagrees with its Items: expected the sentence built from the \
         request, got {}",
        refused.raw
    );
    assert_stored(
        &router,
        &id,
        &stored,
        &etag,
        "a Quantity that disagrees with its Items",
    )
    .await;

    // A `TrafficConfig` that states a `Type` without the configuration that `Type` selects
    // is spelled out here: the table's writer cannot express it, since each of its rows
    // carries the configuration its type names.
    let typed_without_config = |kind: &str| {
        format!(
            "<ContinuousDeploymentPolicyConfig \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <StagingDistributionDnsNames><Quantity>1</Quantity><Items>\
             <DnsName>dbad000abcdef8.cloudfront.net</DnsName></Items>\
             </StagingDistributionDnsNames><Enabled>true</Enabled>\
             <TrafficConfig><Type>{kind}</Type></TrafficConfig>\
             </ContinuousDeploymentPolicyConfig>"
        )
    };

    // Each row names the constraint it breaks, one value past the published bound. The
    // bounds are the service's own constants, which are the documented ones:
    // `ContinuousDeploymentSingleWeightConfig$Weight` "between 0 and 0.15",
    // `SessionStickinessConfig$IdleTTL`/`$MaximumTTL` "300–3600 seconds" with `IdleTTL` "less
    // than or equal to MaximumTTL", `ContinuousDeploymentSingleHeaderConfig$Header` "must
    // contain the prefix aws-cf-cd-", and at most
    // `MAX_STAGING_DISTRIBUTION_DNS_NAMES` staging distribution domain names.
    let mut too_many = table[0].clone();
    too_many.dns_names = (0..=MAX_STAGING_DISTRIBUTION_DNS_NAMES)
        .map(|index| format!("dbadmany{index}abcdef8.cloudfront.net"))
        .collect();
    let rows: Vec<(&str, String)> = vec![
        (
            "a weight one step past the published upper bound",
            Policy {
                case: "a weight past the upper bound",
                dns_names: vec!["dbad111abcdef8.cloudfront.net".to_string()],
                enabled: true,
                traffic: Traffic::SingleWeight {
                    weight: MAX_TRAFFIC_WEIGHT + 0.01,
                    stickiness: None,
                },
            }
            .xml(),
        ),
        (
            "a negative weight, one step below the lower bound",
            Policy {
                case: "a weight below the lower bound",
                dns_names: vec!["dbad222abcdef8.cloudfront.net".to_string()],
                enabled: true,
                traffic: Traffic::SingleWeight {
                    weight: -0.01,
                    stickiness: None,
                },
            }
            .xml(),
        ),
        (
            "an IdleTTL one second below the published lower bound",
            Policy {
                case: "an IdleTTL below the lower bound",
                dns_names: vec!["dbad333abcdef8.cloudfront.net".to_string()],
                enabled: true,
                traffic: Traffic::SingleWeight {
                    weight: 0.1,
                    stickiness: Some((
                        MIN_SESSION_STICKINESS_TTL - 1,
                        MAX_SESSION_STICKINESS_TTL,
                    )),
                },
            }
            .xml(),
        ),
        (
            "a MaximumTTL one second past the published upper bound",
            Policy {
                case: "a MaximumTTL past the upper bound",
                dns_names: vec!["dbad444abcdef8.cloudfront.net".to_string()],
                enabled: true,
                traffic: Traffic::SingleWeight {
                    weight: 0.1,
                    stickiness: Some((
                        MIN_SESSION_STICKINESS_TTL,
                        MAX_SESSION_STICKINESS_TTL + 1,
                    )),
                },
            }
            .xml(),
        ),
        (
            "an IdleTTL past its MaximumTTL",
            Policy {
                case: "an IdleTTL past MaximumTTL",
                dns_names: vec!["dbad555abcdef8.cloudfront.net".to_string()],
                enabled: true,
                traffic: Traffic::SingleWeight {
                    weight: 0.1,
                    stickiness: Some((900, 600)),
                },
            }
            .xml(),
        ),
        (
            "a single-header name without the documented prefix",
            Policy {
                case: "a header without the prefix",
                dns_names: vec!["dbad666abcdef8.cloudfront.net".to_string()],
                enabled: true,
                traffic: Traffic::SingleHeader {
                    header: "x-staging".to_string(),
                    value: "true".to_string(),
                },
            }
            .xml(),
        ),
        (
            "one staging distribution domain name past the bound",
            too_many.xml(),
        ),
        (
            "a SingleWeight type without its SingleWeightConfig",
            typed_without_config("SingleWeight"),
        ),
        (
            "a SingleHeader type without its SingleHeaderConfig",
            typed_without_config("SingleHeader"),
        ),
    ];

    for (case, body) in &rows {
        let refused = update_policy(&router, &id, body, Some(&etag)).await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected HTTP 400, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument, got {}",
            refused.raw
        );
        assert_stored(&router, &id, &stored, &etag, case).await;
    }

    // The same policy still takes a configuration within every bound, so the refusals above
    // left the record usable.
    let accepted = update_policy(&router, &id, &stored.xml(), Some(&etag)).await;
    assert_eq!(
        accepted.status, 200,
        "case a configuration within every bound: expected HTTP 200, got {} with {}",
        accepted.status, accepted.raw
    );
}

/// A staging distribution takes part in at most ONE policy: an update that moves a policy
/// onto a domain name another policy already routes traffic to is `StagingDistributionInUse`
/// (HTTP 409) and writes nothing, while an update that keeps the policy's own domain names —
/// changing only what else the configuration states — is not a conflict with itself.
#[tokio::test]
async fn an_update_claiming_another_policys_staging_distribution_is_refused() {
    let router = emulator();
    let table = policy_table();

    for (index, policy) in table.iter().enumerate().take(3) {
        let case = policy.case;
        let subject = renamed(&router, policy, &format!("inuse{index}a")).await;
        let holder = renamed(&router, &table[(index + 1) % table.len()], &format!("inuse{index}b"))
            .await;

        let (holder_id, _) = seed(&router, &holder).await;
        let (id, etag) = seed(&router, &subject).await;

        // The domain names the OTHER policy holds, carried by this policy's configuration.
        let mut claiming = subject.clone();
        claiming.dns_names = holder.dns_names.clone();
        let refused = update_policy(&router, &id, &claiming.xml(), Some(&etag)).await;
        assert_eq!(
            refused.status, 409,
            "case {case}: expected HTTP 409 claiming another policy's staging distribution, got \
             {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "StagingDistributionInUse",
            "case {case}: expected StagingDistributionInUse, got {}",
            refused.raw
        );
        // The sentence this code carries is the service's own fixed one: it states that a
        // staging distribution of the account is already another policy's, and names
        // neither the policy that holds it nor anything else from the request.
        let message = refused.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, STAGING_DISTRIBUTION_IN_USE_MESSAGE,
            "case {case}: expected {STAGING_DISTRIBUTION_IN_USE_MESSAGE:?}, got {message:?}"
        );
        assert!(
            !message.contains(&holder_id),
            "case {case}: the sentence must not name the policy holding the staging \
             distribution ({holder_id}), got {message:?}"
        );
        assert_stored(&router, &id, &subject, &etag, "claiming a held domain name").await;

        // Keeping its OWN domain names is no conflict: the policy may change everything else
        // the configuration states while routing traffic to the same staging distribution.
        let mut kept = table[(index + 2) % table.len()].clone();
        kept.dns_names = subject.dns_names.clone();
        let accepted = update_policy(&router, &id, &kept.xml(), Some(&etag)).await;
        assert_eq!(
            accepted.status, 200,
            "case {case}: expected HTTP 200 keeping its own staging distribution, got {} with {}",
            accepted.status, accepted.raw
        );
        assert_stored(
            &router,
            &id,
            &kept,
            accepted.header("etag"),
            "keeping its own domain names",
        )
        .await;
    }
}
