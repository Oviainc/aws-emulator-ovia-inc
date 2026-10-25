// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests pinning WHAT DECIDES `ListTagsForResource`: the
//! `ResourceArn` it is given, and nothing about the organisation's Firewall
//! Manager default-administrator association.
//!
//! The rule asserted here, in one sentence: a Firewall Manager call whose only
//! resource identifier is the model's generic `ResourceArn` is answered from
//! that ARN alone — the ARN's FORM first (a string that is not an ARN is HTTP
//! 400 `ValidationException`, whose message names the offending value), then its
//! RESOLUTION against the resources this service stores (an ARN naming none of
//! them is the modelled HTTP 400 `ResourceNotFoundException`), otherwise the tag
//! set the named resource persists — so its answer does not move with the state
//! of the organisation's default-administrator association, while the
//! organisation-scoped operations, which name no resource of their own, are
//! exactly the ones whose answer does.
//!
//! The class the rule ranges over is the CROSS PRODUCT of two tables:
//!
//! * [`ASSOCIATION_STATES`] — the four states the administrator slot can be in:
//!   no association held at all, an active one, a tear-down asked for and still
//!   pending, and a tear-down the clock has completed. The first and the last
//!   hold no active administrator, the middle two do.
//! * [`read_cases`] — the `ResourceArn` forms: the ARN of a stored policy
//!   carrying the `TagList` it was created with, of a stored protocols list
//!   carrying its own distinct tags, of a stored and UNTAGGED applications list,
//!   two well-formed ARNs (of two different resource kinds) that name nothing,
//!   and a string that is not an ARN.
//!
//! Every row is driven in more than one Region, and each state also drives one
//! organisation-scoped operation (`ListPolicies`) as the CONTRAST: its answer
//! flips between HTTP 200 and the HTTP 400 administrator refusal across the same
//! four states the tag read answers identically in — which is what shows the
//! gate is in place and working rather than merely open.
//!
//! This suite complements `tagging_administrator_gate`, which crosses the same
//! rule over the three tagging operations with and without an association: what
//! is added here is the association's LIFECYCLE (a tear-down pending, and one
//! completed), a resource created through the gated writer `PutPolicy` with a
//! `TagList`, and the side-by-side contrast with a gated operation in each
//! state.
//!
//! The wire harness is [`crate::policy_lifecycle`]'s, so these assertions are
//! about the same deserialize → handle → serialize walk a client drives.

use serde_json::json;
use service_com_amazonaws_fms_awsfms_20180101::ops::types::Tag;
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::admin_account_disassociation::wait_out_the_disassociation;
use crate::administrator_gated_operations::{
    associate_admin, error_message, seed_apps_list, seed_protocols_list, with_store,
};
use crate::policy_lifecycle::{call_in, emulator_with_engine, error_code, ACCOUNT, REGION};
use crate::tagging_and_managing_admins::{list_tags_in, minimal_policy, owned, tag_list};

/// Regions every row is asserted in: the suite's own Region and another, so an
/// answer that moved with the Region the request is signed for could not pass.
const REGIONS: &[&str] = &[REGION, "eu-central-1"];

/// A `ListId` (`com.amazonaws.fms#ListId`: 36 characters matching
/// `^[a-z0-9A-Z-]{36}$`) for the seeded protocols list.
const PROTOCOLS_LIST_ID: &str = "5c1f0a7e-2b84-4d36-9f51-7ae0c4d3b812";

/// A second well-formed `ListId`, for the seeded applications list.
const APPS_LIST_ID: &str = "7d2e9b40-6c15-48aa-b3e7-1f5490dcae63";

/// A `PolicyId` (`com.amazonaws.fms#PolicyId`: 36 characters matching
/// `^[a-z0-9A-Z-]{36}$`) that names no policy.
const UNKNOWN_POLICY_ID: &str = "2f6b8d31-9e47-4c02-85da-b71e3c9f6a05";

/// A `Base62Id` (`com.amazonaws.fms#Base62Id`: 22 characters matching
/// `^[a-z0-9A-Z]{22}$`) that names no resource set.
const UNKNOWN_RESOURCE_SET_ID: &str = "Ab3kR9tZ1qLm7XpW2sVy0C";

/// The `TagList` the policy is CREATED with, through `PutPolicy` — the gated
/// writer, so it runs while the association is still active, whatever state the
/// row then moves the slot into.
const POLICY_TAGS: &[(&str, &str)] = &[("stage", "prod"), ("team", "net")];

/// The tags the seeded protocols list carries, distinct from the policy's, so a
/// read that reported the wrong resource's set could not pass.
const PROTOCOLS_LIST_TAGS: &[(&str, &str)] = &[("scope", "protocols"), ("stage", "test")];

/// The state of the organisation's administrator slot a row is driven in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Association {
    /// The organisation holds no administrator association at all.
    NoneHeld,
    /// An association is in place and active.
    Active,
    /// A tear-down has been asked for and has not completed: the association is
    /// still the one in force until the completion instant passes.
    TearDownPending,
    /// The tear-down has completed, so the slot is free again.
    TornDown,
}

impl Association {
    /// Whether this state leaves an ACTIVE default administrator — what the
    /// organisation-scoped gate decides on.
    fn holds_an_administrator(self) -> bool {
        matches!(self, Self::Active | Self::TearDownPending)
    }
}

/// The administrator-slot states the rule is asserted in.
const ASSOCIATION_STATES: &[Association] = &[
    Association::NoneHeld,
    Association::Active,
    Association::TearDownPending,
    Association::TornDown,
];

/// What a tag read answers for one `ResourceArn`, once that ARN is the only
/// thing deciding it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Expect {
    /// The ARN names a stored resource: the call reports exactly this tag set
    /// (empty for a resource carrying no tags).
    Tags(Vec<(String, String)>),
    /// The ARN is well formed and names no stored resource.
    NotFound,
    /// The value is not an ARN at all.
    Malformed,
}

/// One row of the ARN table.
struct ReadCase {
    case: &'static str,
    arn: String,
    expect: Expect,
}

/// The `ResourceArn` class the rule is asserted over, given the ARNs of the
/// resources the store holds.
///
/// The rows differ in the FORM of the value (a complete ARN, a string that is no
/// ARN), in the KIND of resource a complete ARN names (policy, protocols list,
/// applications list, resource set), in whether it names a stored one, and in
/// the tag set a stored one carries (several tags, a different several, none).
fn read_cases(policy_arn: &str, protocols_list_arn: &str, apps_list_arn: &str) -> Vec<ReadCase> {
    vec![
        ReadCase {
            case: "the ARN of a stored policy created with a TagList",
            arn: policy_arn.to_string(),
            expect: Expect::Tags(owned(POLICY_TAGS)),
        },
        ReadCase {
            case: "the ARN of a stored protocols list carrying its own tags",
            arn: protocols_list_arn.to_string(),
            expect: Expect::Tags(owned(PROTOCOLS_LIST_TAGS)),
        },
        ReadCase {
            // An untagged resource reports an EMPTY list: it exists and carries
            // no tags, which is not the same as nothing being known about it.
            case: "the ARN of a stored applications list carrying no tags",
            arn: apps_list_arn.to_string(),
            expect: Expect::Tags(Vec::new()),
        },
        ReadCase {
            case: "a policy ARN naming no policy",
            arn: state::policy_arn(REGION, UNKNOWN_POLICY_ID),
            expect: Expect::NotFound,
        },
        ReadCase {
            case: "a resource-set ARN naming no resource set",
            arn: state::resource_set_arn(REGION, UNKNOWN_RESOURCE_SET_ID),
            expect: Expect::NotFound,
        },
        ReadCase {
            case: "a string that is not an ARN at all",
            arn: "no-arn-here-just-a-string".to_string(),
            expect: Expect::Malformed,
        },
    ]
}

/// Create a policy carrying `POLICY_TAGS` through `PutPolicy` and return the ARN
/// the service minted for it.
async fn put_tagged_policy(router: &MultiServiceRouter, region: &str) -> String {
    let (status, body) = call_in(
        router,
        "PutPolicy",
        &json!({
            "Policy": minimal_policy("association.states.policy"),
            "TagList": tag_list(POLICY_TAGS),
        }),
        region,
    )
    .await;
    assert_eq!(
        status, 200,
        "PutPolicy must answer 200 while the association is active, got {status} with {body}"
    );
    body["PolicyArn"]
        .as_str()
        .unwrap_or_else(|| panic!("PutPolicy reported no PolicyArn: {body}"))
        .to_string()
}

/// `pairs` as the record field a seeded resource holds.
fn tags(pairs: &[(&str, &str)]) -> Vec<Tag> {
    pairs
        .iter()
        .map(|(key, value)| Tag {
            key: (*key).to_string(),
            value: (*value).to_string(),
        })
        .collect()
}

/// The ARNs of the three taggable resources one emulator holds.
struct StoredResources {
    policy: String,
    protocols_list: String,
    apps_list: String,
}

/// An emulator holding one policy, one protocols list and one applications list,
/// with the administrator slot moved into `association`.
///
/// The resources are created the same way in every state — the policy through
/// the gated `PutPolicy`, which needs the association, the two lists through the
/// store the way their own gated writers write them — so the only thing that
/// differs between the rows is the state of the slot.
async fn emulator_in(association: Association) -> (MultiServiceRouter, StoredResources) {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;
    let policy = put_tagged_policy(&router, REGION).await;

    with_store(&engine, |store| {
        seed_protocols_list(
            store,
            PROTOCOLS_LIST_ID,
            "association.states.protocols",
            &["TCP", "UDP"],
            false,
        );
        seed_apps_list(
            store,
            REGION,
            APPS_LIST_ID,
            "association.states.apps",
            &[],
            false,
        );
        if let Some(record) = store.protocols_lists.get_mut(PROTOCOLS_LIST_ID) {
            record.tags = tags(PROTOCOLS_LIST_TAGS);
        }
    });

    match association {
        // The association is dropped from the store, which is the state of an
        // organisation that never made one while its resources outlive it. No
        // API reaches it: `DisassociateAdminAccount` keeps the record so
        // `GetAdminAccount` can report what became of the account.
        Association::NoneHeld => with_store(&engine, |store| store.admin_account = None),
        Association::Active => {}
        Association::TearDownPending | Association::TornDown => {
            let (status, body) =
                call_in(&router, "DisassociateAdminAccount", &json!({}), REGION).await;
            assert_eq!(
                status, 200,
                "state {association:?}: the holder's disassociation must be accepted, got \
                 {status} with {body}"
            );
            if association == Association::TornDown {
                wait_out_the_disassociation();
            }
        }
    }

    (
        router,
        StoredResources {
            policy,
            protocols_list: state::protocols_list_arn(REGION, PROTOCOLS_LIST_ID),
            apps_list: state::apps_list_arn(REGION, APPS_LIST_ID),
        },
    )
}

/// Assert the administrator slot really is in `association`, read back through
/// `GetAdminAccount` rather than assumed.
async fn assert_slot_state(router: &MultiServiceRouter, association: Association) {
    let (status, body) = call_in(router, "GetAdminAccount", &json!({}), REGION).await;
    let role_status = body["RoleStatus"].as_str().unwrap_or_default().to_string();
    match association {
        Association::NoneHeld => assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "state {association:?}: no association may be reported, got {status} with {body}"
        ),
        Association::Active => assert_eq!(
            (status, role_status.as_str()),
            (200, "READY"),
            "state {association:?}: the association must be ready, got {status} with {body}"
        ),
        Association::TearDownPending => assert_eq!(
            (status, role_status.as_str()),
            (200, "PENDING_DELETION"),
            "state {association:?}: the tear-down must still be pending, got {status} with {body}"
        ),
        Association::TornDown => assert_eq!(
            (status, role_status.as_str()),
            (200, "DELETED"),
            "state {association:?}: the tear-down must have completed, got {status} with {body}"
        ),
    }
}

/// Drive one ARN row in `region` and assert the ARN alone decided the answer.
async fn assert_arn_decides(
    router: &MultiServiceRouter,
    association: Association,
    row: &ReadCase,
    region: &str,
) {
    let case = format!("{association:?} / {} in {region}", row.case);
    match &row.expect {
        Expect::Tags(expected) => {
            let (status, code, held) = list_tags_in(router, &row.arn, region).await;
            assert_eq!(
                (status, code.as_str()),
                (200, ""),
                "case {case:?}: a stored resource's tags are reported whatever the association \
                 state, got {status} / {code:?}"
            );
            assert_eq!(
                &held, expected,
                "case {case:?}: expected the persisted tag set {expected:?}, got {held:?}"
            );
        }
        Expect::NotFound => {
            let (status, code, held) = list_tags_in(router, &row.arn, region).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ResourceNotFoundException"),
                "case {case:?}: a well-formed ARN naming nothing is the modelled not-found, got \
                 {status} / {code:?} with {held:?}"
            );
        }
        Expect::Malformed => {
            let (status, body) = call_in(
                router,
                "ListTagsForResource",
                &json!({ "ResourceArn": &row.arn }),
                region,
            )
            .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {case:?}: a value that is not an ARN is refused on its form, got {status} \
                 with {body}"
            );
            let message = error_message(&body);
            assert!(
                message.contains(row.arn.as_str()),
                "case {case:?}: the form refusal is built from the request, so it names the \
                 offending value {:?}, got {message:?}",
                row.arn
            );
        }
    }
}

/// The contrast: one organisation-scoped operation, in the same state and
/// Region. With no ACTIVE administrator it answers the HTTP 400 administrator
/// refusal, whose message names the calling account and the Region the request
/// was signed for; with one it is answered.
async fn assert_gated_operation_follows_the_slot(
    router: &MultiServiceRouter,
    association: Association,
    region: &str,
) {
    let case = format!("{association:?} / ListPolicies in {region}");
    let (status, body) = call_in(router, "ListPolicies", &json!({}), region).await;
    if association.holds_an_administrator() {
        assert_eq!(
            status, 200,
            "case {case:?}: an organisation-scoped read is answered while an administrator is \
             associated, got {status} with {body}"
        );
    } else {
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "AccessDeniedException"),
            "case {case:?}: an organisation-scoped read is refused with no administrator \
             associated, got {status} with {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("No default admin could be found for account {ACCOUNT} in Region {region}"),
            "case {case:?}: the refusal names the calling account and the Region the request was \
             signed for, got {body}"
        );
    }
}

/// `ListTagsForResource` is decided by its `ResourceArn` in EVERY administrator
/// slot state, while an organisation-scoped operation in the same state follows
/// the slot — the whole cross product, in more than one Region.
#[tokio::test]
async fn a_tag_read_is_decided_by_its_arn_in_every_administrator_slot_state() {
    for association in ASSOCIATION_STATES.iter().copied() {
        let (router, stored) = emulator_in(association).await;
        assert_slot_state(&router, association).await;

        for region in REGIONS {
            assert_gated_operation_follows_the_slot(&router, association, region).await;
            for row in read_cases(&stored.policy, &stored.protocols_list, &stored.apps_list) {
                assert_arn_decides(&router, association, &row, region).await;
            }
        }
    }
}

/// One `ListTagsForResource` answer as the shared reader hands it back:
/// `(http status, error code or "", the tag set)`.
type TagRead = (u16, String, Vec<(String, String)>);

/// The answer is the SAME answer in every state: the tag sets read back with no
/// administrator association, with one, and on either side of a tear-down are
/// identical, and a repeated read answers identically — the read writes nothing.
#[tokio::test]
async fn the_same_arn_reads_back_the_same_tag_set_in_every_slot_state() {
    let mut answers: Vec<(Association, Vec<TagRead>)> = Vec::new();

    for association in ASSOCIATION_STATES.iter().copied() {
        let (router, stored) = emulator_in(association).await;
        let arns = [&stored.policy, &stored.protocols_list, &stored.apps_list];

        let mut per_state = Vec::new();
        for arn in arns {
            let first = list_tags_in(&router, arn, REGION).await;
            let again = list_tags_in(&router, arn, REGION).await;
            assert_eq!(
                again, first,
                "state {association:?}: a repeated read of {arn:?} must answer identically, got \
                 {again:?} after {first:?}"
            );
            per_state.push(first);
        }
        answers.push((association, per_state));
    }

    let (first_state, expected) = answers
        .first()
        .expect("the association-state table is not empty")
        .clone();
    for (association, got) in &answers {
        assert_eq!(
            got, &expected,
            "state {association:?}: the tag sets must be the ones state {first_state:?} reads, \
             got {got:?}"
        );
    }
}

/// A policy's tags outlive the association that created it: the `TagList`
/// `PutPolicy` was given is still what the read reports after the administrator
/// has been disassociated and the tear-down has completed — and the read neither
/// restored the association nor changed what the gated operation answers.
#[tokio::test]
async fn tags_created_under_an_association_are_still_reported_after_it_is_torn_down() {
    let (router, stored) = emulator_in(Association::TornDown).await;

    let (status, code, held) = list_tags_in(&router, &stored.policy, REGION).await;
    assert_eq!(
        (status, code.as_str(), held.as_slice()),
        (200, "", owned(POLICY_TAGS).as_slice()),
        "the policy's created TagList must still be reported after the tear-down, got {status} / \
         {code:?} / {held:?}"
    );

    // The read changed nothing: the slot is still free and the gated operation
    // still answers the refusal.
    assert_slot_state(&router, Association::TornDown).await;
    assert_gated_operation_follows_the_slot(&router, Association::TornDown, REGION).await;

    // And a tag read of a well-formed ARN naming nothing still resolves nothing.
    let (status, code, _) =
        list_tags_in(&router, &state::policy_arn(REGION, UNKNOWN_POLICY_ID), REGION).await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ResourceNotFoundException"),
        "an ARN naming nothing must still resolve to nothing, got {status} / {code:?}"
    );
}
