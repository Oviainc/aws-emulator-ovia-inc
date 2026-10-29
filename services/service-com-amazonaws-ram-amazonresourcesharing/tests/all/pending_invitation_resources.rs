// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the invitation a resource share sends, for
//! `GetResourceShareInvitations`, which reports those invitations, and for
//! `ListPendingInvitationResources`, which reports what an unanswered one is offering.
//!
//! The rules under test, each asserted over a table of inputs rather than one case:
//!
//! * sharing with a principal OUTSIDE the owning account's organization sends that
//!   account an invitation to the share, and sharing with one inside it (or with an
//!   entity that names no account) sends none; an account already holding an
//!   unanswered invitation to a share is not invited to it twice;
//! * `GetResourceShareInvitations` reports the stored invitations, selected by the
//!   invitation and resource-share ARNs the request names, and refuses an ARN nothing
//!   has;
//! * `ListPendingInvitationResources` reports the resources the invitation's share
//!   currently shares — every `RESOURCE` association of that share and nothing else —
//!   with each resource's `type` and `resourceRegionScope` read off its own ARN, and
//!   its `status` following the association that shares it;
//! * `resourceRegionScope` selects by that scope, and defaults to `ALL`;
//! * both reads are `@paginated`: a page holds at most `maxResults` entries, and
//!   following `nextToken` walks every entry exactly once;
//! * an invitation ARN no invitation has is refused with
//!   `ResourceShareInvitationArnNotFoundException`, a value that is not an ARN with
//!   `MalformedArnException`, and an invitation that was rejected or that expired with
//!   the error naming what became of it — each reporting the ARN it refused as its
//!   message.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_ram_amazonresourcesharing::state::{
    ASSOCIATION_READY_AFTER_MILLIS, INVITATION_EXPIRES_AFTER_MILLIS, INVITATION_STATUS_REJECTED,
};
use smithy_server_runtime::MultiServiceRouter;

// Every suite in this crate calls the service the one way: through `tests/all/harness.rs`.
use super::harness::*;

/// An account outside the one that owns the shares, so a principal in it is external
/// and gets invited.
const FOREIGN_ACCOUNT: &str = "111122223333";
/// A second one, so a share can invite more than one account.
const OTHER_FOREIGN_ACCOUNT: &str = "444455556666";
/// An IAM role in [`FOREIGN_ACCOUNT`]: a principal that names a user or role rather
/// than a whole account.
const FOREIGN_ROLE: &str = "arn:aws:iam::111122223333:role/Reader";

/// A resource of a Region-scoped service: its ARN names the Region it lives in, so
/// it "can be accessed only by using requests that target the Amazon Web Services
/// Region in which the resource exists" — `REGIONAL`.
const REGIONAL_RESOURCE: &str = LOCAL_RESOURCE;
/// A second regional one.
const REGIONAL_RESOURCE_TWO: &str = LOCAL_RESOURCE_TWO;
/// A resource of a global service: its ARN names no Region, so it "can be accessed
/// from any Amazon Web Services Region" — `GLOBAL`.
const GLOBAL_RESOURCE: &str = "arn:aws:route53:::hostedzone/Z11111111111111111111";
/// A second global one, of another global service.
const GLOBAL_RESOURCE_TWO: &str = "arn:aws:cloudfront::000000000000:distribution/E22222222222";

// ---------------------------------------------------------------------------
// Who gets invited
// ---------------------------------------------------------------------------

/// A row of the invitation-minting table: one principal, and what sharing with it
/// must have stored.
struct InviteCase {
    case: &'static str,
    principal: &'static str,
    /// The account the invitation must name as its receiver, or `None` when this
    /// principal must be shared with directly and invited to nothing.
    receiver: Option<&'static str>,
    /// The IAM principal the invitation must name, when the entity names one.
    receiver_arn: Option<&'static str>,
}

#[tokio::test]
async fn only_a_principal_outside_the_organization_is_invited() {
    // The rule generalises over the FORM of the principal, so the rows differ along
    // it: inside the organization by account id, by organization id and by
    // organizational-unit ARN; outside it by account id and by IAM principal ARN; and
    // an entity that names no account at all.
    let cases = vec![
        InviteCase {
            case: "the owning account itself",
            principal: ACCOUNT,
            receiver: None,
            receiver_arn: None,
        },
        InviteCase {
            case: "the owner's organization, by id",
            principal: "o-abcd1234ef",
            receiver: None,
            receiver_arn: None,
        },
        InviteCase {
            case: "an organizational unit of the owner's organization",
            principal: "arn:aws:organizations::000000000000:ou/o-abcd1234ef/ou-ab12-cdef3456",
            receiver: None,
            receiver_arn: None,
        },
        InviteCase {
            case: "an account outside the organization",
            principal: FOREIGN_ACCOUNT,
            receiver: Some(FOREIGN_ACCOUNT),
            receiver_arn: None,
        },
        InviteCase {
            case: "an IAM role in an account outside the organization",
            principal: FOREIGN_ROLE,
            receiver: Some(FOREIGN_ACCOUNT),
            receiver_arn: Some(FOREIGN_ROLE),
        },
        InviteCase {
            case: "a service principal, which names no account to invite",
            principal: "ec2.amazonaws.com",
            receiver: None,
            receiver_arn: None,
        },
    ];

    for InviteCase {
        case,
        principal,
        receiver,
        receiver_arn,
    } in cases
    {
        let (router, engine) = emulator_with_engine();
        let share = create_ok(
            &router,
            case,
            &json!({
                "name": "share",
                "principals": [principal],
                "resourceArns": [REGIONAL_RESOURCE],
            }),
        )
        .await;
        let share_arn = arn_of(case, &share);

        let reported = invitations_of_share(&router, case, &share_arn).await;
        match receiver {
            None => assert!(
                reported.is_empty(),
                "case {case:?}: expected no invitation, got {reported:?}"
            ),
            Some(expected) => {
                assert_eq!(
                    reported.len(),
                    1,
                    "case {case:?}: expected exactly one invitation, got {reported:?}"
                );
                let invitation = &reported[0];
                assert_eq!(
                    member(invitation, "receiverAccountId"),
                    Some(expected),
                    "case {case:?}: expected receiverAccountId {expected:?}, got {invitation:?}"
                );
                assert_eq!(
                    member(invitation, "receiverArn"),
                    receiver_arn,
                    "case {case:?}: expected receiverArn {receiver_arn:?}, got {invitation:?}"
                );
                assert_eq!(
                    member(invitation, "senderAccountId"),
                    Some(ACCOUNT),
                    "case {case:?}: expected the owning account as sender, got {invitation:?}"
                );
                assert_eq!(
                    member(invitation, "status"),
                    Some("PENDING"),
                    "case {case:?}: a fresh invitation must be PENDING, got {invitation:?}"
                );
                assert_eq!(
                    member(invitation, "resourceShareArn"),
                    Some(share_arn.as_str()),
                    "case {case:?}: expected the share's ARN, got {invitation:?}"
                );
                assert_eq!(
                    member(invitation, "resourceShareName"),
                    Some("share"),
                    "case {case:?}: expected the share's name, got {invitation:?}"
                );
                assert!(
                    invitation.get("invitationTimestamp").is_some(),
                    "case {case:?}: expected an invitationTimestamp, got {invitation:?}"
                );
                let arn = assert_invitation_arn(case, invitation);

                // The window the invitation is answerable in is not a reported
                // member, so it is read from the record that stores it.
                let state = store(&engine);
                let locked = state.lock();
                let record = locked
                    .invitations
                    .get(&arn)
                    .unwrap_or_else(|| panic!("case {case:?}: {arn} must be stored"));
                assert_eq!(
                    record.expires_at - record.invitation_timestamp,
                    INVITATION_EXPIRES_AFTER_MILLIS,
                    "case {case:?}: expected the invitation to expire \
                     {INVITATION_EXPIRES_AFTER_MILLIS}ms after it was sent, got {record:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_account_already_holding_an_unanswered_invitation_is_not_invited_twice() {
    let router = emulator();
    let case = "the same account shared with twice";
    let share = create_ok(
        &router,
        case,
        &json!({"name": "share", "principals": [FOREIGN_ACCOUNT]}),
    )
    .await;
    let share_arn = arn_of(case, &share);
    let first = invitations_of_share(&router, case, &share_arn).await;
    assert_eq!(
        first.len(),
        1,
        "case {case:?}: expected one invitation after the create, got {first:?}"
    );

    // Sharing with the same account again — as the account, and as an IAM principal
    // of it — adds no second invitation: the unanswered one is what it answers.
    for principal in [json!(FOREIGN_ACCOUNT), json!(FOREIGN_ROLE)] {
        let (status, body) = associate(
            &router,
            &json!({"resourceShareArn": share_arn, "principals": [principal]}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: associate expected 200, got {status} with {body:?}"
        );
    }
    let again = invitations_of_share(&router, case, &share_arn).await;
    assert_eq!(
        again.len(),
        1,
        "case {case:?}: expected still one invitation, got {again:?}"
    );

    // A DIFFERENT account outside the organization is a second invitation.
    let (status, body) = associate(
        &router,
        &json!({"resourceShareArn": share_arn, "principals": [OTHER_FOREIGN_ACCOUNT]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: associate expected 200, got {status} with {body:?}"
    );
    let receivers: BTreeSet<String> = invitations_of_share(&router, case, &share_arn)
        .await
        .iter()
        .filter_map(|invitation| member(invitation, "receiverAccountId").map(str::to_string))
        .collect();
    assert_eq!(
        receivers,
        BTreeSet::from([
            FOREIGN_ACCOUNT.to_string(),
            OTHER_FOREIGN_ACCOUNT.to_string()
        ]),
        "case {case:?}: expected one invitation per invited account, got {receivers:?}"
    );
}

// ---------------------------------------------------------------------------
// Reporting the invitations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn invitations_are_selected_by_the_arns_the_request_names() {
    let router = emulator();
    let case = "two shares, each inviting one account";
    let first_share = arn_of(
        case,
        &create_ok(
            &router,
            case,
            &json!({"name": "first", "principals": [FOREIGN_ACCOUNT]}),
        )
        .await,
    );
    let second_share = arn_of(
        case,
        &create_ok(
            &router,
            case,
            &json!({"name": "second", "principals": [OTHER_FOREIGN_ACCOUNT]}),
        )
        .await,
    );
    let first_invitation = only_invitation_arn(&router, case, &first_share).await;
    let second_invitation = only_invitation_arn(&router, case, &second_share).await;
    let both = BTreeSet::from([first_invitation.clone(), second_invitation.clone()]);

    // The rule generalises over which ARNs a request names, so the rows differ along
    // the filter used and how many entries it names.
    let cases = vec![
        ("no filter, which reports every invitation", json!({}), both.clone()),
        (
            "one invitation ARN",
            json!({"resourceShareInvitationArns": [first_invitation]}),
            BTreeSet::from([first_invitation.clone()]),
        ),
        (
            "both invitation ARNs",
            json!({"resourceShareInvitationArns": [second_invitation, first_invitation]}),
            both.clone(),
        ),
        (
            "one share ARN",
            json!({"resourceShareArns": [second_share]}),
            BTreeSet::from([second_invitation.clone()]),
        ),
        (
            "both filters, which select the invitations satisfying each",
            json!({
                "resourceShareArns": [first_share],
                "resourceShareInvitationArns": [first_invitation],
            }),
            BTreeSet::from([first_invitation.clone()]),
        ),
        (
            "both filters naming different resources, which select nothing",
            json!({
                "resourceShareArns": [second_share],
                "resourceShareInvitationArns": [first_invitation],
            }),
            BTreeSet::new(),
        ),
    ];

    for (case, body, expected) in cases {
        let reported: BTreeSet<String> = invitations_ok(&router, case, &body)
            .await
            .iter()
            .filter_map(|invitation| {
                member(invitation, "resourceShareInvitationArn").map(str::to_string)
            })
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // And the read is paginated: every page holds at most `maxResults` invitations
    // and following `nextToken` walks each exactly once.
    for page_size in [1_u64, 2, 5] {
        let case = format!("maxResults {page_size} over two invitations");
        let walked = walk_pages(
            &router,
            case.as_str(),
            &PAGED_INVITATIONS,
            &json!({"maxResults": page_size}),
            page_size,
            both.len(),
        )
        .await;
        assert_eq!(
            walked, both,
            "case {case:?}: expected every invitation across the pages, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn an_invitation_or_share_the_store_does_not_hold_is_refused() {
    let router = emulator();
    let case = "a share with a pending invitation";
    let share_arn = arn_of(
        case,
        &create_ok(
            &router,
            case,
            &json!({"name": "share", "principals": [FOREIGN_ACCOUNT]}),
        )
        .await,
    );
    let unknown_invitation = unknown_invitation_arn();
    let unknown_share = format!(
        "arn:aws:ram:{REGION}:{ACCOUNT}:resource-share/00000000-0000-4000-8000-000000000000"
    );

    let cases = vec![
        (
            "an invitation ARN no invitation has",
            json!({"resourceShareInvitationArns": [unknown_invitation]}),
            "ResourceShareInvitationArnNotFoundException",
            Some(unknown_invitation.clone()),
        ),
        (
            "a share ARN no share has",
            json!({"resourceShareArns": [unknown_share]}),
            "UnknownResourceException",
            Some(format!("ResourceShare {unknown_share} could not be found.")),
        ),
        (
            "an invitation filter entry that is not an invitation ARN",
            json!({"resourceShareInvitationArns": [share_arn]}),
            "MalformedArnException",
            None,
        ),
        (
            "a share filter entry that is not a share ARN",
            json!({"resourceShareArns": ["not-an-arn"]}),
            "MalformedArnException",
            None,
        ),
        (
            "a nextToken this operation never issued",
            json!({"nextToken": "not-a-token"}),
            "InvalidNextTokenException",
            None,
        ),
    ];

    for (case, body, expected_code, expected_message) in cases {
        let (status, reported) = get_invitations(&router, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {reported:?}"
        );
        assert_eq!(
            error_code(&reported),
            expected_code,
            "case {case:?}: expected {expected_code}, got {reported:?}"
        );
        if let Some(expected_message) = expected_message {
            assert_eq!(
                error_message(&reported),
                expected_message,
                "case {case:?}: expected the message {expected_message:?}, got {reported:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What an unanswered invitation is offering
// ---------------------------------------------------------------------------

/// A row of the reported-resources table: the resources a share is created with, and
/// what the invitation to it must report for each.
struct ResourcesCase {
    case: &'static str,
    resources: Vec<&'static str>,
    /// `(arn, type, resourceRegionScope)` for every resource that must be reported.
    expected: Vec<(&'static str, &'static str, &'static str)>,
}

#[tokio::test]
async fn a_pending_invitation_reports_the_resources_of_its_share() {
    // The rule generalises over the resources a share holds, so the rows differ along
    // their COUNT and their Region scope, with the types read off the ARNs of four
    // different services.
    let cases = vec![
        ResourcesCase {
            case: "a share with no resources",
            resources: vec![],
            expected: vec![],
        },
        ResourcesCase {
            case: "one regional resource",
            resources: vec![REGIONAL_RESOURCE],
            expected: vec![(REGIONAL_RESOURCE, "ec2:subnet", "REGIONAL")],
        },
        ResourcesCase {
            case: "two regional resources",
            resources: vec![REGIONAL_RESOURCE, REGIONAL_RESOURCE_TWO],
            expected: vec![
                (REGIONAL_RESOURCE, "ec2:subnet", "REGIONAL"),
                (REGIONAL_RESOURCE_TWO, "ec2:subnet", "REGIONAL"),
            ],
        },
        ResourcesCase {
            case: "one global resource, whose ARN names no Region",
            resources: vec![GLOBAL_RESOURCE],
            expected: vec![(GLOBAL_RESOURCE, "route53:hostedzone", "GLOBAL")],
        },
        ResourcesCase {
            case: "global and regional resources together",
            resources: vec![GLOBAL_RESOURCE_TWO, REGIONAL_RESOURCE, GLOBAL_RESOURCE],
            expected: vec![
                (GLOBAL_RESOURCE_TWO, "cloudfront:distribution", "GLOBAL"),
                (REGIONAL_RESOURCE, "ec2:subnet", "REGIONAL"),
                (GLOBAL_RESOURCE, "route53:hostedzone", "GLOBAL"),
            ],
        },
    ];

    for ResourcesCase {
        case,
        resources,
        expected,
    } in cases
    {
        let router = emulator();
        let (share_arn, invitation_arn) = shared_with_invitation(&router, case, &resources).await;

        let body = list_ok(
            &router,
            case,
            &json!({"resourceShareInvitationArn": invitation_arn}),
        )
        .await;
        let reported = resources_of(case, &body);
        assert_eq!(
            reported.len(),
            expected.len(),
            "case {case:?}: expected {} resources, got {body:?}",
            expected.len()
        );
        for (arn, kind, scope) in &expected {
            let resource = reported
                .iter()
                .find(|resource| member(resource, "arn") == Some(arn))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a resource {arn:?}, got {reported:?}")
                });
            assert_eq!(
                member(resource, "type"),
                Some(*kind),
                "case {case:?}: expected type {kind:?} for {arn:?}, got {resource:?}"
            );
            assert_eq!(
                member(resource, "resourceRegionScope"),
                Some(*scope),
                "case {case:?}: expected resourceRegionScope {scope:?} for {arn:?}, got {resource:?}"
            );
            assert_eq!(
                member(resource, "resourceShareArn"),
                Some(share_arn.as_str()),
                "case {case:?}: expected the share's ARN on {arn:?}, got {resource:?}"
            );
            assert!(
                resource.get("creationTime").is_some(),
                "case {case:?}: expected a creationTime on {arn:?}, got {resource:?}"
            );
        }
        assert!(
            body.get("nextToken").and_then(Value::as_str).is_none(),
            "case {case:?}: a complete page must report no nextToken, got {body:?}"
        );
    }
}

#[tokio::test]
async fn the_resources_reported_are_the_ones_the_share_holds_at_the_time_of_the_read() {
    let router = emulator();
    let case = "a share whose resources change after the invitation was sent";
    let (share_arn, invitation_arn) =
        shared_with_invitation(&router, case, &[REGIONAL_RESOURCE]).await;

    // A resource associated AFTER the invitation was sent is part of what the
    // invitation offers, and one taken back out of the share is not.
    let (status, body) = associate(
        &router,
        &json!({"resourceShareArn": share_arn, "resourceArns": [GLOBAL_RESOURCE]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: associate expected 200, got {status} with {body:?}"
    );
    assert_eq!(
        reported_arns(&router, case, &invitation_arn, &json!({})).await,
        BTreeSet::from([REGIONAL_RESOURCE.to_string(), GLOBAL_RESOURCE.to_string()]),
        "case {case:?}: expected both resources after the second associate"
    );

    let (status, body) = disassociate(
        &router,
        &json!({"resourceShareArn": share_arn, "resourceArns": [REGIONAL_RESOURCE]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: disassociate expected 200, got {status} with {body:?}"
    );
    assert_eq!(
        reported_arns(&router, case, &invitation_arn, &json!({})).await,
        BTreeSet::from([GLOBAL_RESOURCE.to_string()]),
        "case {case:?}: a resource the share no longer shares must not be reported"
    );

    // A principal association is not a resource, however many of them the share has.
    let (status, body) = associate(
        &router,
        &json!({"resourceShareArn": share_arn, "principals": [OTHER_FOREIGN_ACCOUNT]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: associate expected 200, got {status} with {body:?}"
    );
    assert_eq!(
        reported_arns(&router, case, &invitation_arn, &json!({})).await,
        BTreeSet::from([GLOBAL_RESOURCE.to_string()]),
        "case {case:?}: only RESOURCE associations are reported as resources"
    );
}

#[tokio::test]
async fn a_resources_status_follows_the_association_that_shares_it() {
    let router = emulator();
    let case = "a resource whose association has not settled yet";
    let (_, invitation_arn) = shared_with_invitation(&router, case, &[REGIONAL_RESOURCE]).await;

    let body = list_ok(
        &router,
        case,
        &json!({"resourceShareInvitationArn": invitation_arn}),
    )
    .await;
    let resource = resources_of(case, &body).first().cloned().unwrap_or_default();
    assert_eq!(
        member(&resource, "status"),
        Some("PENDING"),
        "case {case:?}: a resource still being associated is PENDING, got {resource:?}"
    );

    // Reaching `ASSOCIATED` is a move the clock owes the association; once it falls
    // due the resource is available. Nothing in the service sleeps for it — the test
    // does, because the span is what makes the transient status observable at all.
    std::thread::sleep(Duration::from_millis(
        u64::try_from(ASSOCIATION_READY_AFTER_MILLIS).unwrap_or_default() + 100,
    ));
    let case = "a resource whose association has settled";
    let body = list_ok(
        &router,
        case,
        &json!({"resourceShareInvitationArn": invitation_arn}),
    )
    .await;
    let resource = resources_of(case, &body).first().cloned().unwrap_or_default();
    assert_eq!(
        member(&resource, "status"),
        Some("AVAILABLE"),
        "case {case:?}: an associated resource is AVAILABLE, got {resource:?}"
    );
}

// ---------------------------------------------------------------------------
// Filtering and pagination
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_region_scope_filter_selects_by_the_resources_own_scope() {
    let router = emulator();
    let case = "a share holding both global and regional resources";
    let (_, invitation_arn) = shared_with_invitation(
        &router,
        case,
        &[
            REGIONAL_RESOURCE,
            REGIONAL_RESOURCE_TWO,
            GLOBAL_RESOURCE,
            GLOBAL_RESOURCE_TWO,
        ],
    )
    .await;

    let regional = BTreeSet::from([
        REGIONAL_RESOURCE.to_string(),
        REGIONAL_RESOURCE_TWO.to_string(),
    ]);
    let global = BTreeSet::from([GLOBAL_RESOURCE.to_string(), GLOBAL_RESOURCE_TWO.to_string()]);
    let all: BTreeSet<String> = regional.union(&global).cloned().collect();

    // Every value the modelled `ResourceRegionScopeFilter` admits, plus the absent
    // member, whose documented default is `ALL`.
    let cases = vec![
        ("no filter, which defaults to ALL", json!({}), all.clone()),
        ("ALL", json!({"resourceRegionScope": "ALL"}), all),
        (
            "REGIONAL",
            json!({"resourceRegionScope": "REGIONAL"}),
            regional,
        ),
        ("GLOBAL", json!({"resourceRegionScope": "GLOBAL"}), global),
    ];
    for (case, extra, expected) in cases {
        let reported = reported_arns(&router, case, &invitation_arn, &extra).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn following_next_token_walks_every_resource_exactly_once() {
    let router = emulator();
    let case = "four resources";
    let shared = [
        REGIONAL_RESOURCE,
        REGIONAL_RESOURCE_TWO,
        GLOBAL_RESOURCE,
        GLOBAL_RESOURCE_TWO,
    ];
    let (_, invitation_arn) = shared_with_invitation(&router, case, &shared).await;
    let expected: BTreeSet<String> = shared.iter().map(|arn| (*arn).to_string()).collect();

    // The rule generalises over the page size, so the rows differ along it: one
    // resource per page, a size that does not divide the total, the exact total, and
    // more than the total.
    for page_size in [1_u64, 3, 4, 7] {
        let case = format!("maxResults {page_size} over four resources");
        let walked = walk_pages(
            &router,
            case.as_str(),
            &PAGED_RESOURCES,
            &json!({"resourceShareInvitationArn": invitation_arn, "maxResults": page_size}),
            page_size,
            shared.len(),
        )
        .await;
        assert_eq!(
            walked, expected,
            "case {case:?}: expected every resource across the pages, got {walked:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// What the read refuses
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_invitation_the_store_cannot_serve_is_refused() {
    let router = emulator();
    let case = "a share with a pending invitation";
    let (share_arn, invitation_arn) =
        shared_with_invitation(&router, case, &[REGIONAL_RESOURCE]).await;
    let unknown_invitation = unknown_invitation_arn();

    let cases = vec![
        (
            "an invitation ARN no invitation has",
            json!({"resourceShareInvitationArn": unknown_invitation}),
            400,
            "ResourceShareInvitationArnNotFoundException",
            Some(unknown_invitation.clone()),
        ),
        (
            "an ARN of another resource of this service",
            json!({"resourceShareInvitationArn": share_arn}),
            400,
            "ResourceShareInvitationArnNotFoundException",
            Some(share_arn.clone()),
        ),
        (
            "a value that is not an ARN",
            json!({"resourceShareInvitationArn": "not-an-arn"}),
            400,
            "MalformedArnException",
            None,
        ),
        (
            "an ARN missing its resource field",
            json!({"resourceShareInvitationArn": "arn:aws:ram:us-west-2:000000000000:"}),
            400,
            "MalformedArnException",
            None,
        ),
        (
            "a nextToken this operation never issued",
            json!({"resourceShareInvitationArn": invitation_arn, "nextToken": "not-a-token"}),
            400,
            "InvalidNextTokenException",
            None,
        ),
        (
            "a resourceRegionScope outside the modelled enum, which the modelled \
             constraint refuses before the handler runs",
            json!({"resourceShareInvitationArn": invitation_arn, "resourceRegionScope": "LOCAL"}),
            400,
            "ValidationException",
            None,
        ),
    ];

    for (case, body, expected_status, expected_code, expected_message) in cases {
        let (status, reported) = list_pending(&router, &body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} with {reported:?}"
        );
        assert_eq!(
            error_code(&reported),
            expected_code,
            "case {case:?}: expected {expected_code}, got {reported:?}"
        );
        if let Some(expected_message) = expected_message {
            assert_eq!(
                error_message(&reported),
                expected_message,
                "case {case:?}: expected the message {expected_message:?}, got {reported:?}"
            );
        }
    }
}

/// What became of an invitation a row puts out of the unanswered state.
#[derive(Clone, Copy, Debug)]
enum Answer {
    Rejected,
    Expired,
}

#[tokio::test]
async fn an_answered_or_expired_invitation_is_no_longer_pending() {
    // The rule generalises over what became of the invitation, so the rows differ
    // along it: an answer the invited account gave, and an expiry the clock made due.
    let cases = vec![
        (
            "an invitation the invited account rejected",
            Answer::Rejected,
            "ResourceShareInvitationAlreadyRejectedException",
            "REJECTED",
        ),
        (
            "an invitation nobody answered in time",
            Answer::Expired,
            "ResourceShareInvitationExpiredException",
            "EXPIRED",
        ),
    ];

    for (case, answer, expected_code, expected_status) in cases {
        let (router, engine) = emulator_with_engine();
        let (_, invitation_arn) = shared_with_invitation(&router, case, &[REGIONAL_RESOURCE]).await;

        // Answering an invitation, and waiting out the days it takes to expire, are
        // not what this suite is about: the record is put into the state the read has
        // to refuse, and the read is what is asserted.
        {
            let state = store(&engine);
            let mut locked = state.lock();
            let invitation = locked
                .invitations
                .get_mut(&invitation_arn)
                .unwrap_or_else(|| panic!("case {case:?}: the invitation must be stored"));
            match answer {
                Answer::Rejected => invitation.status = INVITATION_STATUS_REJECTED.to_string(),
                // Dating the window's close in the past is what makes the expiry the
                // clock owes due; the read is what materialises it.
                Answer::Expired => invitation.expires_at = 0,
            }
        }

        let (status, reported) = list_pending(
            &router,
            &json!({"resourceShareInvitationArn": invitation_arn}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {reported:?}"
        );
        assert_eq!(
            error_code(&reported),
            expected_code,
            "case {case:?}: expected {expected_code}, got {reported:?}"
        );
        assert_eq!(
            error_message(&reported),
            invitation_arn,
            "case {case:?}: expected the refused ARN as the message, got {reported:?}"
        );

        // And the invitation itself still reports what became of it.
        let reported = invitations_ok(
            &router,
            case,
            &json!({"resourceShareInvitationArns": [invitation_arn]}),
        )
        .await;
        assert_eq!(
            reported
                .first()
                .and_then(|invitation| member(invitation, "status")),
            Some(expected_status),
            "case {case:?}: expected the invitation to report {expected_status}, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A string member of a reported object.
fn member<'a>(value: &'a Value, member: &str) -> Option<&'a str> {
    value.get(member).and_then(Value::as_str)
}

/// `ListPendingInvitationResources` over the wire.
async fn list_pending(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/listpendinginvitationresources", Some(body)).await
}

/// The same call, which must have succeeded.
async fn list_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, reported) = list_pending(router, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} with {reported:?}"
    );
    reported
}

/// `GetResourceShareInvitations` over the wire.
async fn get_invitations(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/getresourceshareinvitations", Some(body)).await
}

/// The invitations a successful `GetResourceShareInvitations` reported.
async fn invitations_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> Vec<Value> {
    let (status, reported) = get_invitations(router, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} with {reported:?}"
    );
    reported
        .get("resourceShareInvitations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: response carried no resourceShareInvitations, got {reported:?}")
        })
}

/// The invitations one share has sent, as the API reports them.
async fn invitations_of_share(
    router: &MultiServiceRouter,
    case: &str,
    share_arn: &str,
) -> Vec<Value> {
    invitations_ok(router, case, &json!({"resourceShareArns": [share_arn]})).await
}

/// The ARN of the one invitation a share must have sent.
async fn only_invitation_arn(router: &MultiServiceRouter, case: &str, share_arn: &str) -> String {
    let reported = invitations_of_share(router, case, share_arn).await;
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected the share to have invited one account, got {reported:?}"
    );
    assert_invitation_arn(case, &reported[0])
}

/// Assert a reported invitation carries an invitation ARN of this service, in this
/// Region and the account that sent it, and answer it.
fn assert_invitation_arn(case: &str, invitation: &Value) -> String {
    let arn = member(invitation, "resourceShareInvitationArn").unwrap_or_else(|| {
        panic!("case {case:?}: invitation carried no resourceShareInvitationArn, got {invitation:?}")
    });
    let prefix = format!("arn:aws:ram:{REGION}:{ACCOUNT}:resource-share-invitation/");
    let id = arn
        .strip_prefix(&prefix)
        .unwrap_or_else(|| panic!("case {case:?}: expected an ARN starting {prefix}, got {arn:?}"));
    // The id segment RAM uses is a UUID: 8-4-4-4-12 lowercase hex.
    assert_eq!(
        id.split('-').map(str::len).collect::<Vec<_>>(),
        vec![8, 4, 4, 4, 12],
        "case {case:?}: expected a UUID id segment, got {id:?}"
    );
    arn.to_string()
}

/// An invitation ARN of exactly the form this service issues, which no invitation has.
fn unknown_invitation_arn() -> String {
    format!(
        "arn:aws:ram:{REGION}:{ACCOUNT}:resource-share-invitation/00000000-0000-4000-8000-000000000000"
    )
}

/// The resources a `ListPendingInvitationResources` response reported.
fn resources_of(case: &str, body: &Value) -> Vec<Value> {
    body.get("resources")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: response carried no resources, got {body:?}"))
}

/// The resource ARNs one page of the read reported, for a request that must fit in a
/// single page.
async fn reported_arns(
    router: &MultiServiceRouter,
    case: &str,
    invitation_arn: &str,
    extra: &Value,
) -> BTreeSet<String> {
    let mut body = json!({"resourceShareInvitationArn": invitation_arn});
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    let reported = list_ok(router, case, &body).await;
    assert!(
        reported.get("nextToken").and_then(Value::as_str).is_none(),
        "case {case:?}: expected a single page, got {reported:?}"
    );
    resources_of(case, &reported)
        .iter()
        .filter_map(|resource| member(resource, "arn").map(str::to_string))
        .collect()
}

/// Which paginated read `walk_pages` walks: its URI, the member its page carries and
/// the key that identifies one entry.
struct Paged {
    uri: &'static str,
    list: &'static str,
    key: &'static str,
}

/// `ListPendingInvitationResources`, as `walk_pages` names it.
const PAGED_RESOURCES: Paged = Paged {
    uri: "/listpendinginvitationresources",
    list: "resources",
    key: "arn",
};

/// `GetResourceShareInvitations`, as `walk_pages` names it.
const PAGED_INVITATIONS: Paged = Paged {
    uri: "/getresourceshareinvitations",
    list: "resourceShareInvitations",
    key: "resourceShareInvitationArn",
};

/// Walk every page of a paginated read, asserting each page holds at most `page_size`
/// entries and that no entry is reported twice, and answer the entries walked.
///
/// One walker for both operations: they page the same way, over the entries `paged`
/// names.
async fn walk_pages(
    router: &MultiServiceRouter,
    case: &str,
    paged: &Paged,
    request: &Value,
    page_size: u64,
    total: usize,
) -> BTreeSet<String> {
    let (uri, list, key) = (paged.uri, paged.list, paged.key);
    let mut seen: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut body = request.clone();
        if let Some(token) = &next_token {
            body["nextToken"] = json!(token);
        }
        let (status, page) = call(router, "POST", uri, Some(&body)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {page:?}"
        );
        let entries = page
            .get(list)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: page carried no {list}, got {page:?}"));
        assert!(
            entries.len() as u64 <= page_size,
            "case {case:?}: a page must hold at most {page_size} entries, got {page:?}"
        );
        seen.extend(
            entries
                .iter()
                .filter_map(|entry| member(entry, key).map(str::to_string)),
        );
        next_token = page
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        pages += 1;
        assert!(
            pages <= total + 1,
            "case {case:?}: pagination must terminate, walked {pages} pages for {seen:?}"
        );
        if next_token.is_none() {
            break;
        }
    }
    let unique: BTreeSet<String> = seen.iter().cloned().collect();
    assert_eq!(
        seen.len(),
        unique.len(),
        "case {case:?}: every entry must be reported once, got {seen:?}"
    );
    unique
}

/// Create a share holding `resources` and shared with an account outside the
/// organization, and answer `(share ARN, the ARN of the invitation it sent)`.
async fn shared_with_invitation(
    router: &MultiServiceRouter,
    case: &str,
    resources: &[&str],
) -> (String, String) {
    let share = create_ok(
        router,
        case,
        &json!({
            "name": "invited-share",
            "principals": [FOREIGN_ACCOUNT],
            "resourceArns": resources,
        }),
    )
    .await;
    let share_arn = arn_of(case, &share);
    let invitation_arn = only_invitation_arn(router, case, &share_arn).await;
    (share_arn, invitation_arn)
}
