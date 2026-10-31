// SPDX-License-Identifier: CC0-1.0

//! `BatchUpdateRecommendationResourceExclusion` over an entry list that names a
//! recommendation resource owned by ANOTHER account, driven over HTTP through the
//! same `MultiServiceRouter` the emulator serves with.
//!
//! The rule: **an operation whose input names a Trusted Advisor resource by an
//! ARN whose account segment is an account other than the calling one is
//! answered for that resource first — 403 `AccessDeniedException` naming the
//! refused ARN — whatever the caller's support plan, before the identifier is
//! resolved and before anything is written.** No support plan grants an account
//! access to another account's resources, and every entry of this batch carries
//! its owning account in an ARN segment the caller writes:
//! `RecommendationResourceExclusion$arn` is `RecommendationResourceArn`,
//! `^arn:[\w-]+:trustedadvisor::\d{12}:recommendation-resource\/[\w-]+\/[\w-]+$`
//! (`smithy_query(query="shape",shape="RecommendationResourceExclusion,RecommendationResourceArn",
//! model_path="services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json")`).
//!
//! This is the one WRITE path of the service that takes a list of caller-supplied
//! resource ARNs, so the table below spans what the write would depend on if the
//! account scope were not applied first: the `type` of the recommendation that
//! owns the named resource (a `standard` one, whose resources this operation does
//! apply to, as much as a `priority` one, whose resources it refuses per entry),
//! whether the store really holds that other account's resource, which exclusion
//! status it holds today, both `isExcluded` flags, the entry's position in the
//! list, and every support plan AWS offers. A refused batch is refused WHOLE: not
//! one stored resource of any account moves, the calling account's own entries
//! included. The counterpart test pins that the ACCOUNT SEGMENT is the whole test
//! — an own-account ARN still reaches the operation's own behaviour, and an
//! own-account ARN the store holds nothing for still comes back as one per-entry
//! error rather than as this refusal.
//!
//! The same rule over the account-level identifiers of the other operations that
//! take one is pinned in
//! `tests/all/account_scoped_recommendation_identifier.rs`; the arbitrary-AWS-resource
//! half (`ListRecommendationsForResource`) in `tests/all/support_level_gate.rs`.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::{service_state, SupportPlan};
use smithy_server_runtime::MultiServiceRouter;

use crate::owning_account_from_arn::store_member_recommendation;
use crate::recommendation_loop::{
    call, dispatch, emulator_on_plan, error_code, label, recommendation_arn, resource_arn,
};

/// The path this operation binds (`PUT /v1/batch-update-recommendation-resource-exclusion`).
const BATCH_PATH: &str = "/v1/batch-update-recommendation-resource-exclusion";

/// The modelled maximum of `RecommendationResourceExclusionList` (`length 1..25`).
const MAX_ENTRIES: usize = 25;

/// The refusal this rule answers with: the status the model binds
/// `AccessDeniedException` to, and the shape's own name, which restJson1 carries
/// both in the body's `__type` and in the `x-amzn-errortype` header.
const DENIED: (u16, &str) = (403, "AccessDeniedException");

/// A seeded check whose recommendation is `standard` — the type whose resources
/// this operation applies to, so an entry naming one of them is the entry that
/// would reach the write. Source: the `SEED` table in `src/state.rs`.
const STANDARD_CHECK: &str = "amazon-ebs-snapshots";

/// A seeded check whose recommendation is `priority`, the type this operation
/// refuses per entry. Same source.
const PRIORITY_CHECK: &str = "low-utilization-ec2-instances";

/// One target of an entry: what it is about, the check whose recommendation owns
/// the named resource, and the resource id.
struct Target {
    name: &'static str,
    check_id: &'static str,
    resource_id: &'static str,
}

/// The resources an entry names, written out by hand from the `SEED` table in
/// `src/state.rs`: a `standard` recommendation's resource seeded `included` and
/// one seeded `excluded` (so a write would be observable whichever flag the entry
/// carries), a `priority` recommendation's resource, a resource id no
/// recommendation of either check holds, and a check the store has no
/// recommendation for at all.
const TARGETS: &[Target] = &[
    Target {
        name: "a standard recommendation's resource seeded included",
        check_id: STANDARD_CHECK,
        resource_id: "res-ebs-0001",
    },
    Target {
        name: "a standard recommendation's resource seeded excluded",
        check_id: STANDARD_CHECK,
        resource_id: "res-ebs-0002",
    },
    Target {
        name: "a prioritized recommendation's resource",
        check_id: PRIORITY_CHECK,
        resource_id: "res-ec2-0001",
    },
    Target {
        name: "a resource id the store holds nothing for",
        check_id: STANDARD_CHECK,
        resource_id: "res-ebs-9999",
    },
    Target {
        name: "a check the store has no recommendation for",
        check_id: "no-such-check",
        resource_id: "res-0001",
    },
];

/// The member accounts whose copies of both checks the store really holds in
/// these tests. Each is a 12-digit `AccountId` other than the caller's, one of
/// them with leading zeros.
const STORED_ACCOUNTS: &[&str] = &["999988887777", "000000000012"];

/// An account the store holds nothing at all for: the refusal is about the
/// identifier's account segment, so it must not depend on what the identifier
/// resolves to — a foreign ARN that IS stored and one that is not are answered
/// the same way, and the answer discloses no existence.
const UNSTORED_ACCOUNT: &str = "111122223333";

/// One account form the rows span: what it is about, the ARN partition and the
/// account id. The partition is left free because `RecommendationResourceArn`'s
/// `[\w-]+` admits any, and the account segment alone decides this refusal.
struct AccountForm {
    name: String,
    partition: &'static str,
    account: &'static str,
}

/// Every foreign-account form: the two enrolled member accounts, an account the
/// store holds nothing for, and an enrolled one named in another partition.
fn foreign_forms() -> Vec<AccountForm> {
    let mut forms: Vec<AccountForm> = STORED_ACCOUNTS
        .iter()
        .map(|account| AccountForm {
            name: format!("the enrolled member account {account}"),
            partition: "aws",
            account,
        })
        .collect();
    forms.push(AccountForm {
        name: format!("the account {UNSTORED_ACCOUNT}, which the store holds nothing for"),
        partition: "aws",
        account: UNSTORED_ACCOUNT,
    });
    forms.push(AccountForm {
        name: format!(
            "the enrolled member account {} in another partition",
            STORED_ACCOUNTS[0]
        ),
        partition: "aws-us-gov",
        account: STORED_ACCOUNTS[0],
    });
    forms
}

/// A recommendation-resource ARN in `partition` owned by `account` — the form
/// `RecommendationResourceArn` declares.
fn resource_arn_in(partition: &str, account: &str, check_id: &str, resource_id: &str) -> String {
    format!(
        "arn:{partition}:trustedadvisor::{account}:recommendation-resource/\
         {check_id}-recommendation/{resource_id}"
    )
}

/// The request body for a list of `(arn, isExcluded)` entries.
fn body(entries: &[(String, bool)]) -> Value {
    json!({
        "recommendationResourceExclusions": entries
            .iter()
            .map(|(arn, is_excluded)| json!({"arn": arn, "isExcluded": is_excluded}))
            .collect::<Vec<Value>>(),
    })
}

/// One row of the refusal table: what the row is about, the entries to send, and
/// the ARN the refusal must name.
struct Row {
    name: String,
    entries: Vec<(String, bool)>,
    refused: String,
}

/// What every stored recommendation resource holds that this operation writes:
/// its `exclusionStatus` and the `lastUpdatedAt` the write would stamp, keyed by
/// the resource's own ARN — every account's rows, not only the caller's, so a
/// write to another account's resource is caught wherever it lands.
///
/// Read in process: an account-level list reports only the caller's own
/// resources, and the organization surface covers only prioritized
/// recommendations, so no modelled read reports a member account's `standard`
/// resource.
fn stored_exclusions(engine: &engine::InMemoryEngine) -> BTreeMap<String, (String, i64)> {
    let state = service_state(engine).expect("the in-memory engine carries this service's state");
    let state = state.lock();
    state
        .recommendation_resources
        .iter()
        .map(|(arn, resource)| {
            (
                arn.clone(),
                (
                    resource.exclusion_status.clone(),
                    resource.last_updated_at_millis,
                ),
            )
        })
        .collect()
}

/// Assert that one batch is refused for the resource it names: the status the
/// model binds `AccessDeniedException` to, the error shape in both places
/// restJson1 carries it, a message naming the refused ARN, and no per-entry error
/// list at all — the whole request is refused, so there is no partial result.
async fn assert_refused_whole(router: &MultiServiceRouter, row: &Row) {
    let Row {
        name,
        entries,
        refused,
    } = row;
    let (status, errortype, response) =
        dispatch(router, "PUT", BATCH_PATH, Some(body(entries))).await;
    let (expected_status, expected_code) = DENIED;
    assert_eq!(
        status, expected_status,
        "case {name}: expected HTTP {expected_status}, got {status} with {response}"
    );
    assert_eq!(
        error_code(&response),
        expected_code,
        "case {name}: expected __type {expected_code}, got {response}"
    );
    assert_eq!(
        errortype.as_deref(),
        Some(expected_code),
        "case {name}: expected x-amzn-errortype {expected_code}, got {errortype:?}"
    );
    assert_eq!(
        response["message"].as_str(),
        Some(format!("Access denied to resource: {refused}").as_str()),
        "case {name}: expected the refusal to name {refused}, got {response}"
    );
    assert!(
        response["batchUpdateRecommendationResourceExclusionErrors"].is_null(),
        "case {name}: expected the whole request to be refused with no per-entry result, got \
         {response}"
    );
}

/// Store every member account's copy of both checks' recommendations and
/// resources, so the rows below name another account's resource that the store
/// really holds — including resources of a `standard` recommendation, the ones an
/// applied entry would write.
fn enrol_member_accounts(engine: &engine::InMemoryEngine) {
    for account in STORED_ACCOUNTS {
        for check_id in [STANDARD_CHECK, PRIORITY_CHECK] {
            store_member_recommendation(engine, check_id, account);
        }
    }
}

/// The whole refusal table: one single-entry row per account form, target and
/// `isExcluded` flag, then the rows whose refused entry sits beside others.
fn rows() -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for form in foreign_forms() {
        for target in TARGETS {
            for is_excluded in [true, false] {
                let arn = resource_arn_in(
                    form.partition,
                    form.account,
                    target.check_id,
                    target.resource_id,
                );
                rows.push(Row {
                    name: format!(
                        "{} of {}, isExcluded {is_excluded}",
                        target.name, form.name
                    ),
                    entries: vec![(arn.clone(), is_excluded)],
                    refused: arn,
                });
            }
        }
    }

    // The refused entry beside the CALLING account's own: one entry the caller
    // may not touch refuses the whole request, whichever position it holds.
    let own = resource_arn(STANDARD_CHECK, "res-ebs-0001");
    let own_other = resource_arn(STANDARD_CHECK, "res-ebs-0002");
    let foreign = resource_arn_in("aws", STORED_ACCOUNTS[0], STANDARD_CHECK, "res-ebs-0001");
    rows.push(Row {
        name: "a foreign entry after an own-account one".to_string(),
        entries: vec![(own.clone(), true), (foreign.clone(), true)],
        refused: foreign.clone(),
    });
    rows.push(Row {
        name: "a foreign entry before an own-account one".to_string(),
        entries: vec![(foreign.clone(), true), (own.clone(), true)],
        refused: foreign.clone(),
    });

    // Several foreign entries from different accounts: the refusal names the
    // first one in list order, which is the one the caller has to drop first.
    let second_foreign =
        resource_arn_in("aws", STORED_ACCOUNTS[1], STANDARD_CHECK, "res-ebs-0001");
    let third_foreign = resource_arn_in("aws", UNSTORED_ACCOUNT, PRIORITY_CHECK, "res-ec2-0001");
    rows.push(Row {
        name: "three foreign entries from three accounts".to_string(),
        entries: vec![
            (foreign.clone(), true),
            (second_foreign, false),
            (third_foreign, true),
        ],
        refused: foreign.clone(),
    });

    // A batch at the modelled maximum whose LAST entry is the foreign one: every
    // entry is scoped, not only the first.
    let mut at_max: Vec<(String, bool)> = vec![(own, true), (own_other, false)];
    at_max.extend((at_max.len()..MAX_ENTRIES - 1).map(|index| {
        (
            resource_arn(STANDARD_CHECK, &format!("res-ebs-{index:04}")),
            index % 2 == 0,
        )
    }));
    at_max.push((foreign.clone(), true));
    assert_eq!(
        at_max.len(),
        MAX_ENTRIES,
        "case the maximum-length batch: expected {MAX_ENTRIES} entries, got {}",
        at_max.len()
    );
    rows.push(Row {
        name: format!("a foreign entry last in a batch of {MAX_ENTRIES}"),
        entries: at_max,
        refused: foreign,
    });

    rows
}

#[tokio::test]
async fn a_batch_entry_in_another_account_is_refused_and_writes_nothing() {
    // Every plan AWS offers: the resource refusal does not depend on the support
    // plan, so it answers the unentitled caller and the entitled one alike — and
    // it is this refusal, not the support-level one, that an entitled caller gets.
    for plan in SupportPlan::ALL {
        let (router, engine) = emulator_on_plan(*plan);
        enrol_member_accounts(&engine);
        let before = stored_exclusions(&engine);

        for row in rows() {
            let row = Row {
                name: format!("{} on {plan:?}", row.name),
                ..row
            };
            assert_refused_whole(&router, &row).await;

            // Nothing was written: not the refused entry's resource, not the
            // other accounts' resources, and not the calling account's own
            // entries beside the refused one.
            let after = stored_exclusions(&engine);
            assert_eq!(
                after, before,
                "case {}: expected the refused batch to write nothing, got {after:?} where the \
                 store held {before:?}",
                row.name
            );
        }
    }
}

#[tokio::test]
async fn the_account_segment_alone_decides_the_refusal() {
    let (router, engine) = emulator_on_plan(SupportPlan::Business);
    // The store also holds other accounts' copies of the same recommendations,
    // so the rows below are answered while foreign rows are present.
    enrol_member_accounts(&engine);

    // An ARN carrying the CALLER's own account is not this refusal's business,
    // whatever else about it differs — including a partition the store keys no
    // resource under, which is answered as the operation answers any ARN naming
    // no stored resource: one per-entry error, HTTP 200, and no write.
    let before = stored_exclusions(&engine);
    let own_other_partition = resource_arn_in(
        "aws-us-gov",
        engine::EMULATOR_ACCOUNT_ID,
        STANDARD_CHECK,
        "res-ebs-0001",
    );
    let unknown_own = resource_arn(STANDARD_CHECK, "res-ebs-9999");
    for (case, arn) in [
        ("the caller's own account in another partition", &own_other_partition),
        ("an own-account ARN naming no stored resource", &unknown_own),
    ] {
        let (status, response) =
            call(&router, "PUT", BATCH_PATH, Some(body(&[(arn.clone(), true)]))).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {response}"
        );
        let errors = response["batchUpdateRecommendationResourceExclusionErrors"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no per-entry error list in {response}"));
        assert_eq!(
            errors.len(),
            1,
            "case {case}: expected exactly one per-entry error, got {errors:?}"
        );
        assert_eq!(
            errors[0]["arn"].as_str(),
            Some(arn.as_str()),
            "case {case}: expected the error entry to name {arn}, got {errors:?}"
        );
        assert_eq!(
            errors[0]["errorCode"].as_str(),
            Some("ResourceNotFoundException"),
            "case {case}: expected a per-entry ResourceNotFoundException, got {errors:?}"
        );
        assert_eq!(
            errors[0]["errorMessage"].as_str(),
            Some(format!("no recommendation resource with the identifier {arn}").as_str()),
            "case {case}: expected the per-entry message to name {arn}, got {errors:?}"
        );
        let after = stored_exclusions(&engine);
        assert_eq!(
            after, before,
            "case {case}: expected nothing to be written, got {after:?} where the store held \
             {before:?}"
        );
    }

    // And an own-account entry that DOES resolve still applies, read back
    // through `ListRecommendationResources`: rows differ along the flag asked
    // for and along the status the resource held before it.
    for (resource_id, is_excluded, expected) in [
        ("res-ebs-0001", true, "excluded"),
        ("res-ebs-0002", false, "included"),
        ("res-ebs-0001", false, "included"),
    ] {
        let arn = resource_arn(STANDARD_CHECK, resource_id);
        let case = format!("{resource_id} to isExcluded {is_excluded}");
        let (status, response) =
            call(&router, "PUT", BATCH_PATH, Some(body(&[(arn.clone(), is_excluded)]))).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {response}"
        );
        let errors = response["batchUpdateRecommendationResourceExclusionErrors"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no per-entry error list in {response}"));
        assert!(
            errors.is_empty(),
            "case {case}: expected no per-entry error, got {errors:?}"
        );

        let path = format!(
            "/v1/recommendations/{}/resources?maxResults=200",
            label(&recommendation_arn(STANDARD_CHECK))
        );
        let (read_status, listed) = call(&router, "GET", &path, None).await;
        assert_eq!(
            read_status, 200,
            "case {case}: expected the read back to be served, got {read_status} with {listed}"
        );
        let actual = listed["recommendationResourceSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no recommendationResourceSummaries in {listed}"))
            .iter()
            .find(|summary| summary["arn"].as_str() == Some(arn.as_str()))
            .and_then(|summary| summary["exclusionStatus"].as_str())
            .unwrap_or_else(|| panic!("case {case}: {arn} is not reported: {listed}"));
        assert_eq!(
            actual, expected,
            "case {case}: expected exclusionStatus {expected}, got {actual}"
        );
    }

    // The other accounts' copies of the same resources are untouched by every
    // own-account write above: their rows still hold what they held.
    let after = stored_exclusions(&engine);
    for account in STORED_ACCOUNTS {
        for resource_id in ["res-ebs-0001", "res-ebs-0002"] {
            let arn = resource_arn_in("aws", account, STANDARD_CHECK, resource_id);
            assert_eq!(
                after.get(&arn),
                before.get(&arn),
                "case {arn}: expected another account's resource to be untouched, got {:?} where \
                 the store held {:?}",
                after.get(&arn),
                before.get(&arn)
            );
        }
    }
}
