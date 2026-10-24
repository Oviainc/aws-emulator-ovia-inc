// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteIpamPolicy`, driven over `ec2Query` (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! * Deleting an IPAM policy moves EXACTLY the policy the request names into the transient
//!   `delete-in-progress` state, with the clock owing it `delete-complete`, and answers that
//!   policy as the store holds it — its id, its ARN, the account that owns it, the IPAM it
//!   belongs to and the tags assigned to it.
//! * An id outside the form this service ISSUES for an `IpamPolicyId` is refused with
//!   `InvalidIpamPolicyId.Malformed` before any lookup; an id of that form naming no policy
//!   of the account is refused with `InvalidIpamPolicyId.NotFound`; and a policy whose
//!   deletion has already been asked for is refused with `IncorrectState`. None of them
//!   writes anything.
//! * `DryRun` answers the permission question with `DryRunOperation` and writes nothing,
//!   whether or not the policy it names exists.
//!
//! Each table row is a distinct input along what those rules generalise over: how many tags
//! the policy carries, which Region and which IPAM it belongs to, how many policies the
//! account holds beside it, and which part of the id is outside the issued form.

use std::time::Duration;

use ec2::ops::types::{IPAM_POLICY_STATE_VALUES, IpamPolicy, Tag};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, IPAM_POLICY_DELETE_COMPLETE_MESSAGE,
    IPAM_POLICY_DELETE_IN_PROGRESS_MESSAGE, IPAM_POLICY_ID_PREFIX,
    IPAM_POLICY_STATE_CREATE_COMPLETE, IPAM_POLICY_STATE_DELETE_COMPLETE,
    IPAM_POLICY_STATE_DELETE_IN_PROGRESS, IpamPolicyRecord, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// The IPAM the seeded policies belong to by default.
const IPAM_ID: &str = "ipam-0123456789abcdef0";

/// A well-formed `IpamPolicyId` in the long (17 hexadecimal digit) form that the account
/// holds no policy for.
///
/// Spelled from the top of the hexadecimal range rather than the bottom: this store mints
/// ids from a monotonic counter (`next_id`), so an id this high cannot collide with one a
/// seeded policy was given however many ids a run has minted — the same reasoning as the
/// envelope suite's `vpc-0ffffffffffffffff`.
const UNKNOWN_POLICY_ID: &str = "ipam-policy-0ffffffffffffffff";

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is certainly due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// One call of `action` signed for the Region above.
async fn invoke(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// The `IpamPolicyId=` field every request here carries, plus `extra` form fields.
fn delete_params(policy_id: &str, extra: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut params = vec![("IpamPolicyId".to_string(), policy_id.to_string())];
    params.extend(
        extra
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string())),
    );
    params
}

/// Seed one IPAM policy in service, and answer the id this service minted for it.
fn seed_policy(
    engine: &engine::InMemoryEngine,
    ipam_id: &str,
    region: &str,
    tags: &[(&str, &str)],
) -> String {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let policy_id = guard
        .insert_ipam_policy(IpamPolicyRecord::in_state(
            IpamPolicy {
                ipam_id: Some(ipam_id.to_string()),
                ipam_policy_region: Some(region.to_string()),
                tags: Some(
                    tags.iter()
                        .map(|(key, value)| Tag {
                            key: Some((*key).to_string()),
                            value: Some((*value).to_string()),
                        })
                        .collect(),
                ),
                ..Default::default()
            },
            IPAM_POLICY_STATE_CREATE_COMPLETE,
        ))
        .expect("the store holds the seeded policy");
    // Every id this service ISSUES carries this resource's own prefix, which is what makes
    // an id under any other prefix malformed rather than missing.
    assert!(
        policy_id.starts_with(&format!("{IPAM_POLICY_ID_PREFIX}-")),
        "the minted policy id {policy_id:?} must carry the issued prefix \
         {IPAM_POLICY_ID_PREFIX:?}"
    );
    policy_id
}

/// The state the stored policy `policy_id` is in right now, read straight out of the store;
/// `None` when the account holds no policy under that id.
fn stored_state(engine: &engine::InMemoryEngine, policy_id: &str) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .ipam_policy(policy_id)
        .map(|record| record.state().to_string())
}

/// The `StateMessage` the stored policy `policy_id` carries right now.
fn stored_state_message(engine: &engine::InMemoryEngine, policy_id: &str) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .ipam_policy(policy_id)
        .and_then(|record| record.view.state_message.clone())
}

/// One policy to delete: a distinct input along what the delete generalises over.
struct Row {
    /// What this row is called in a failure message.
    case: &'static str,
    /// The IPAM the policy belongs to.
    ipam_id: &'static str,
    /// The Region the policy is in.
    region: &'static str,
    /// The tags assigned to it.
    tags: &'static [(&'static str, &'static str)],
}

/// The policies the delete is asserted over: no tags, one tag and several, in three Regions
/// and under two IPAMs.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a policy with no tags, in the Region the request is signed for",
            ipam_id: IPAM_ID,
            region: REGION,
            tags: &[],
        },
        Row {
            case: "a policy with one tag, in another Region",
            ipam_id: IPAM_ID,
            region: "eu-west-2",
            tags: &[("Name", "public-ipv4")],
        },
        Row {
            case: "a policy with three tags, under another IPAM",
            ipam_id: "ipam-00000000000000abc",
            region: "ap-southeast-1",
            tags: &[("Name", "edge"), ("Team", "network"), ("Env", "prod")],
        },
    ]
}

/// The state words this suite asserts on are the model's own: each is a member of the
/// catalog the model publishes for `IpamPolicyState`, so a row cannot pin a word the API
/// does not have.
#[test]
fn the_asserted_state_words_are_the_models_own() {
    for (case, word) in [
        ("a policy in service", IPAM_POLICY_STATE_CREATE_COMPLETE),
        (
            "a policy being deleted",
            IPAM_POLICY_STATE_DELETE_IN_PROGRESS,
        ),
        (
            "a policy whose deletion is done",
            IPAM_POLICY_STATE_DELETE_COMPLETE,
        ),
    ] {
        assert!(
            IPAM_POLICY_STATE_VALUES.contains(&word),
            "case {case:?}: expected {word:?} to be a member of the modelled catalog, which \
             carries {IPAM_POLICY_STATE_VALUES:?}"
        );
    }
}

/// A delete answers the policy it removed in the transient `delete-in-progress` state with
/// `delete-complete` owed by the clock, and touches no other policy of the account.
#[tokio::test]
async fn deleting_a_policy_reports_it_deleting_and_reaches_delete_complete() {
    for row in rows() {
        let case = row.case;
        let (router, engine) = super::ec2_query::emulator();
        let policy_id = seed_policy(&engine, row.ipam_id, row.region, row.tags);
        // The control policy every row keeps beside the one it deletes, so each delete is
        // checked to touch only the policy it names.
        let control_id = seed_policy(&engine, IPAM_ID, REGION, &[("Name", "control")]);

        let (status, body) = invoke(&router, "DeleteIpamPolicy", &delete_params(&policy_id, &[])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        // The policy the response reports is the one the request named, as the store holds
        // it: its own id, the ARN this service gave it, the owning account, the IPAM it
        // belongs to and its Region.
        assert_eq!(
            element(&body, "ipamPolicyId").as_deref(),
            Some(policy_id.as_str()),
            "case {case:?}: expected the deleted policy's id, got {body}"
        );
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(IPAM_POLICY_STATE_DELETE_IN_PROGRESS),
            "case {case:?}: a deletion is answered while it is still under way, got {body}"
        );
        assert_eq!(
            element(&body, "stateMessage").as_deref(),
            Some(IPAM_POLICY_DELETE_IN_PROGRESS_MESSAGE),
            "case {case:?}: expected the message of the state it is in, got {body}"
        );
        assert_eq!(
            element(&body, "ipamId").as_deref(),
            Some(row.ipam_id),
            "case {case:?}: expected the IPAM the policy belongs to, got {body}"
        );
        assert_eq!(
            element(&body, "ipamPolicyRegion").as_deref(),
            Some(row.region),
            "case {case:?}: expected the policy's own Region, got {body}"
        );
        let arn = element(&body, "ipamPolicyArn").unwrap_or_default();
        assert!(
            arn.contains(&format!("ipam-policy/{policy_id}")) && arn.contains(row.region),
            "case {case:?}: expected the policy's ARN to name it and its Region, got {arn:?}"
        );
        assert!(
            element(&body, "ownerId").is_some_and(|owner| !owner.is_empty()),
            "case {case:?}: expected the account that owns the policy, got {body}"
        );
        let reported_keys = elements(&element(&body, "tagSet").unwrap_or_default(), "key");
        let expected_keys: Vec<String> = row
            .tags
            .iter()
            .map(|(key, _)| (*key).to_string())
            .collect();
        assert_eq!(
            reported_keys, expected_keys,
            "case {case:?}: expected the tags assigned to the policy, got {reported_keys:?}"
        );

        // The write the response reported is in the store, and the control policy is
        // untouched.
        assert_eq!(
            stored_state(&engine, &policy_id).as_deref(),
            Some(IPAM_POLICY_STATE_DELETE_IN_PROGRESS),
            "case {case:?}: the store must hold the deletion the response reported"
        );
        assert_eq!(
            stored_state(&engine, &control_id).as_deref(),
            Some(IPAM_POLICY_STATE_CREATE_COMPLETE),
            "case {case:?}: no other policy of the account may change"
        );

        // The move the clock owed falls due on its own. A later request is what reads it
        // back: the control policy's own delete still succeeds, which is what materialises
        // it.
        tokio::time::sleep(past_the_transition()).await;
        let (status, body) =
            invoke(&router, "DeleteIpamPolicy", &delete_params(&control_id, &[])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the control policy must still be deletable, got {status} with \
             {body}"
        );
        assert_eq!(
            stored_state(&engine, &policy_id).as_deref(),
            Some(IPAM_POLICY_STATE_DELETE_COMPLETE),
            "case {case:?}: the clock owed this policy its terminal state"
        );
        assert_eq!(
            stored_state_message(&engine, &policy_id).as_deref(),
            Some(IPAM_POLICY_DELETE_COMPLETE_MESSAGE),
            "case {case:?}: the state message moves with the state the clock reached"
        );
    }
}

/// What one refusal row sends, and the code it must be refused with.
struct Refusal {
    /// What this row is called in a failure message.
    case: &'static str,
    /// The `IpamPolicyId` to send.
    policy_id: &'static str,
    /// The EC2 error code the request must be refused with.
    code: &'static str,
}

/// An id outside the form this service issues is refused as malformed before any lookup, and
/// an id of that form naming no policy of the account is refused as not found. Nothing is
/// written by either.
#[tokio::test]
async fn an_unresolvable_id_is_refused_with_this_resources_own_code() {
    // Every id below is read against the form this service issues: `<prefix>-<8 or 17
    // lowercase hex digits>` under this resource's own prefix.
    let rows = [
        Refusal {
            // The id the recorded corpus probes: a seventeen-hex-digit suffix under a prefix
            // this API does not issue.
            case: "a suffix of the long form under a prefix this API does not issue",
            policy_id: "ipam-pol-0000000000000000f",
            code: "InvalidIpamPolicyId.Malformed",
        },
        Refusal {
            case: "an id of another resource kind",
            policy_id: "ipam-pool-0000000000000000f",
            code: "InvalidIpamPolicyId.Malformed",
        },
        Refusal {
            case: "a suffix that is not hexadecimal",
            policy_id: "ipam-policy-0000000000000000g",
            code: "InvalidIpamPolicyId.Malformed",
        },
        Refusal {
            case: "an uppercase suffix",
            policy_id: "ipam-policy-000000000000000FF",
            code: "InvalidIpamPolicyId.Malformed",
        },
        Refusal {
            case: "a suffix of neither published length",
            policy_id: "ipam-policy-000000000",
            code: "InvalidIpamPolicyId.Malformed",
        },
        Refusal {
            case: "not an id at all",
            policy_id: "my-policy",
            code: "InvalidIpamPolicyId.Malformed",
        },
        Refusal {
            case: "a well-formed long-form id the account holds no policy for",
            policy_id: UNKNOWN_POLICY_ID,
            code: "InvalidIpamPolicyId.NotFound",
        },
        Refusal {
            case: "a well-formed short-form id the account holds no policy for",
            policy_id: "ipam-policy-0fffffff",
            code: "InvalidIpamPolicyId.NotFound",
        },
    ];

    for row in rows {
        let case = row.case;
        let (router, engine) = super::ec2_query::emulator();
        let policy_id = seed_policy(&engine, IPAM_ID, REGION, &[("Name", "kept")]);

        let (status, body) =
            invoke(&router, "DeleteIpamPolicy", &delete_params(row.policy_id, &[])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {case:?}: expected the code {expected:?}, got body {body}",
            expected = row.code,
        );
        assert_eq!(
            stored_state(&engine, &policy_id).as_deref(),
            Some(IPAM_POLICY_STATE_CREATE_COMPLETE),
            "case {case:?}: a refused request must leave the account's policy as it was"
        );
    }
}

/// A policy whose deletion has already been asked for has nothing left to delete, so a
/// second delete is refused — while the deletion is under way and once the clock has
/// finished it.
#[tokio::test]
async fn a_second_delete_is_refused_as_an_incorrect_state() {
    for (case, wait) in [
        ("while the deletion is still under way", false),
        ("once the clock has finished the deletion", true),
    ] {
        let (router, engine) = super::ec2_query::emulator();
        let policy_id = seed_policy(&engine, IPAM_ID, REGION, &[]);

        let (status, body) = invoke(&router, "DeleteIpamPolicy", &delete_params(&policy_id, &[])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        if wait {
            tokio::time::sleep(past_the_transition()).await;
        }

        let (status, body) = invoke(&router, "DeleteIpamPolicy", &delete_params(&policy_id, &[])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "IncorrectState",
            "case {case:?}: expected IncorrectState, got body {body}"
        );
        let expected_state = if wait {
            IPAM_POLICY_STATE_DELETE_COMPLETE
        } else {
            IPAM_POLICY_STATE_DELETE_IN_PROGRESS
        };
        assert_eq!(
            stored_state(&engine, &policy_id).as_deref(),
            Some(expected_state),
            "case {case:?}: the refused repeat must not have restarted the deletion"
        );
    }
}

/// A `DryRun` request answers the permission question with `DryRunOperation` and writes
/// nothing — whether or not the policy it names exists; an id that is not of this kind at
/// all is still refused as malformed, because that is a question about the request's own
/// form.
#[tokio::test]
async fn a_dry_run_answers_the_permission_question_and_writes_nothing() {
    for (case, requested, expected_status, expected_code) in [
        ("a policy the account holds", None, DRY_RUN_STATUS, "DryRunOperation"),
        (
            "a well-formed id the account holds no policy for",
            Some(UNKNOWN_POLICY_ID),
            DRY_RUN_STATUS,
            "DryRunOperation",
        ),
        (
            "an id outside the issued form",
            Some("ipam-pol-0000000000000000f"),
            400,
            "InvalidIpamPolicyId.Malformed",
        ),
    ] {
        let (router, engine) = super::ec2_query::emulator();
        let policy_id = seed_policy(&engine, IPAM_ID, REGION, &[("Name", "kept")]);

        let (status, body) = invoke(
            &router,
            "DeleteIpamPolicy",
            &delete_params(
                requested.unwrap_or(policy_id.as_str()),
                &[("DryRun", "true")],
            ),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the code {expected_code:?}, got body {body}"
        );
        assert_eq!(
            stored_state(&engine, &policy_id).as_deref(),
            Some(IPAM_POLICY_STATE_CREATE_COMPLETE),
            "case {case:?}: a dry run must write nothing"
        );
    }
}
