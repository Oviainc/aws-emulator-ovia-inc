// SPDX-License-Identifier: CC0-1.0

//! The quota on the registry's pull time update exclusion list, end to end over the
//! wire.
//!
//! The rule under test: a caller-grown collection is bounded by the maximum the model
//! declares for the list its read side reports, so a register that would carry the
//! registry past that many exclusions is refused with the `LimitExceededException` the
//! operation models (HTTP 400) and stores nothing, while every exclusion registered
//! below the bound stays listed.
//!
//! For this collection the model states the number: `ListPullTimeUpdateExclusions`'s
//! `pullTimeUpdateExclusions` member targets `PullTimeUpdateExclusionList`, declared
//! `@length(min: 0, max: 100)` — read with
//! `smithy_query(query="shape",shape="PullTimeUpdateExclusionList")`. A listing that
//! cannot report more than that many entries cannot be backed by more than that many, so
//! the register side refuses at exactly that count.
//!
//! The rows span what the rule generalises over: where the boundary falls (the last
//! register that fits, the first that does not, and every register after it), the form of
//! the principal that is refused (`PrincipalArn`'s partitions, principal kinds, accounts,
//! paths, name alphabet and length ceiling, from
//! `smithy_query(query="shape",shape="PrincipalArn")`), and whether a slot freed by a
//! deregister is usable again.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state::MAX_PULL_TIME_UPDATE_EXCLUSIONS;

// The wire harness this crate shares — one emulator and one awsJson1_1 call.
use crate::wire::{call, ok, refused, REGISTRY_ID};

/// The maximum number of entries `PullTimeUpdateExclusionList` can hold, spelled out
/// here rather than imported from the crate, so an emulator-chosen bound that drifts off
/// the modelled one fails this suite. Source: `PullTimeUpdateExclusionList`'s
/// `@length(min: 0, max: 100)` (`smithy_query(query="shape",
/// shape="PullTimeUpdateExclusionList")`).
const MODELLED_EXCLUSION_LIST_MAX: usize = 100;

/// The longest prefix the ARN rows below share, used to build a principal ARN of exactly
/// `PrincipalArn`'s modelled maximum length (`@length(min: 0, max: 200)`).
const ROLE_PREFIX: &str = "arn:aws:iam::000000000000:role/";

/// The modelled ceiling on `PrincipalArn`.
const MAX_PRINCIPAL_ARN_LEN: usize = 200;

/// Register `principal_arn` and answer the call's status and wire error code.
async fn register(router: &MultiServiceRouter, principal_arn: &str) -> (u16, String) {
    let answer = call(
        router,
        "RegisterPullTimeUpdateExclusion",
        &json!({"principalArn": principal_arn}),
    )
    .await;
    let code = answer.error_code().to_string();
    (answer.status, code)
}

/// Every exclusion the registry holds, walked page by page.
async fn all_exclusions(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let mut all: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(ref next) = token {
            request["nextToken"] = json!(next);
        }
        let body = ok(router, case, "ListPullTimeUpdateExclusions", &request).await;
        let page: Vec<String> = body["pullTimeUpdateExclusions"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: a listing always carries pullTimeUpdateExclusions: {body:?}")
            })
            .iter()
            .map(|entry| {
                entry
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: an exclusion is a principal ARN string: {entry:?}")
                    })
                    .to_string()
            })
            .collect();
        assert!(
            page.len() <= MODELLED_EXCLUSION_LIST_MAX,
            "case {case:?}: a page may not exceed the modelled maximum of \
             {MODELLED_EXCLUSION_LIST_MAX} entries, got {}",
            page.len()
        );
        all.extend(page);
        token = body["nextToken"].as_str().map(ToString::to_string);
        if token.is_none() {
            return all;
        }
        assert!(
            all.len() <= 10_000,
            "case {case:?}: pagination did not terminate ({} entries so far)",
            all.len()
        );
    }
}

/// Fill the registry's exclusion list to exactly the bound, answering the ARNs stored.
/// Each register below the bound must succeed, which is the other half of the rule: the
/// refusal starts AT the bound, not before it.
async fn fill_to_the_bound(router: &MultiServiceRouter) -> Vec<String> {
    let mut stored = Vec::new();
    for index in 0..MODELLED_EXCLUSION_LIST_MAX {
        let arn = format!("{ROLE_PREFIX}bulk-{index:05}");
        let case = format!("the register that brings the list to {} entries", index + 1);
        let (status, code) = register(router, &arn).await;
        assert_eq!(
            (status, code.as_str()),
            (200, ""),
            "case {case:?}: a register below the bound must succeed, got {status} / {code:?}"
        );
        stored.push(arn);
    }
    stored
}

/// The emulator's own bound is the one the model declares — not a number of its own.
#[test]
fn the_exclusion_cap_is_the_modelled_list_maximum() {
    assert_eq!(
        MAX_PULL_TIME_UPDATE_EXCLUSIONS, MODELLED_EXCLUSION_LIST_MAX,
        "case {:?}: the exclusion list bound must be the modelled maximum {}, got {}",
        "the configured cap", MODELLED_EXCLUSION_LIST_MAX, MAX_PULL_TIME_UPDATE_EXCLUSIONS
    );
}

/// The boundary: every register up to the modelled maximum is served, and the one past
/// it — whatever principal it names — is refused with `LimitExceededException` and
/// stores nothing, leaving the earlier exclusions listed.
#[tokio::test]
async fn a_register_past_the_modelled_maximum_is_refused_and_stores_nothing() {
    let router = crate::wire::emulator();
    let stored = fill_to_the_bound(&router).await;

    // The refused principal's form is irrelevant to the rule, so the rows span
    // `PrincipalArn`: both principal kinds, one- and two-segment partitions, another
    // account, a path, the name alphabet `[\w+=,.@-]`, and an ARN of exactly the modelled
    // length ceiling.
    let longest = format!(
        "{ROLE_PREFIX}{}",
        "a".repeat(MAX_PRINCIPAL_ARN_LEN - ROLE_PREFIX.len())
    );
    let rows: Vec<(&str, String)> = vec![
        (
            "a role in the commercial partition",
            format!("{ROLE_PREFIX}one-too-many"),
        ),
        (
            "a user in the commercial partition",
            "arn:aws:iam::000000000000:user/past-the-bound".to_string(),
        ),
        (
            "a role in a one-segment partition",
            "arn:aws-us-gov:iam::210987654321:role/gov-past-the-bound".to_string(),
        ),
        (
            "a user in a two-segment partition",
            "arn:aws-iso-b:iam::123456789012:user/iso-past-the-bound".to_string(),
        ),
        (
            "a role carrying a path",
            "arn:aws:iam::123456789012:role/team/sub/path-past-the-bound".to_string(),
        ),
        (
            "a name spanning the admitted alphabet",
            "arn:aws:iam::123456789012:user/odd+name=x,y.z@past-1_2".to_string(),
        ),
        ("a principal ARN of the maximum length", longest),
    ];

    for (case, arn) in &rows {
        let body = refused(
            &router,
            case,
            "RegisterPullTimeUpdateExclusion",
            &json!({"principalArn": arn}),
            "LimitExceededException",
        )
        .await;
        // The refusal is phrased from the registry the request addressed and the bound it
        // met, so it names both rather than a principal of the moment.
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(REGISTRY_ID)
                && message.contains(&MODELLED_EXCLUSION_LIST_MAX.to_string()),
            "case {case:?}: the refusal must name the registry {REGISTRY_ID} and the bound \
             {MODELLED_EXCLUSION_LIST_MAX}, got {message:?}"
        );

        let listed = all_exclusions(&router, case).await;
        assert_eq!(
            listed.len(),
            MODELLED_EXCLUSION_LIST_MAX,
            "case {case:?}: a refused register leaves the list at {MODELLED_EXCLUSION_LIST_MAX} \
             entries, got {}",
            listed.len()
        );
        assert!(
            !listed.contains(arn),
            "case {case:?}: the refused principal must not be stored, but the listing \
             reports it among {} entries",
            listed.len()
        );
    }

    // Everything registered below the bound is still there, exactly once each.
    let mut listed = all_exclusions(&router, "after every refusal").await;
    let mut expected = stored;
    listed.sort();
    expected.sort();
    assert_eq!(
        listed, expected,
        "case {:?}: every exclusion registered below the bound stays listed",
        "after every refusal"
    );
}

/// The bound is on how many exclusions the list HOLDS, not on how many registers were
/// ever made: a deregister frees exactly one slot, which the next register fills, after
/// which the list is full again.
#[tokio::test]
async fn a_slot_freed_by_a_deregister_takes_exactly_one_more_register() {
    let router = crate::wire::emulator();
    let stored = fill_to_the_bound(&router).await;

    for (freed, removed) in stored.iter().take(3).enumerate() {
        let case = format!("slot {freed} freed by a deregister");
        ok(
            &router,
            &case,
            "DeregisterPullTimeUpdateExclusion",
            &json!({"principalArn": removed}),
        )
        .await;

        let replacement = format!("{ROLE_PREFIX}replacement-{freed:05}");
        let (status, code) = register(&router, &replacement).await;
        assert_eq!(
            (status, code.as_str()),
            (200, ""),
            "case {case:?}: the register filling a freed slot must succeed, \
             got {status} / {code:?}"
        );

        // And the list is full again, so the next register is refused once more.
        refused(
            &router,
            &case,
            "RegisterPullTimeUpdateExclusion",
            &json!({"principalArn": format!("{ROLE_PREFIX}refused-again-{freed:05}")}),
            "LimitExceededException",
        )
        .await;

        let listed = all_exclusions(&router, &case).await;
        assert_eq!(
            listed.len(),
            MODELLED_EXCLUSION_LIST_MAX,
            "case {case:?}: the list holds {MODELLED_EXCLUSION_LIST_MAX} entries again, got {}",
            listed.len()
        );
        assert!(
            listed.contains(&replacement),
            "case {case:?}: the exclusion that filled the freed slot must be listed"
        );
        assert!(
            !listed.contains(removed),
            "case {case:?}: the deregistered principal must be gone from the listing"
        );
    }
}
