// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this service's AUTHORIZATION gate: the two rules that are
//! decided before a request reaches the approval-team store.
//!
//! 1. An operation that identifies its resource by ARN is refused with
//!    `AccessDeniedException` (403) when that ARN names a resource outside the
//!    caller's own account — decided BEFORE the resource is resolved, so the answer
//!    never depends on what is stored under it (an ARN inside the caller's account
//!    that no team carries stays `ResourceNotFoundException`, 404).
//! 2. `CreateApprovalTeam` may only be called by the management account of the
//!    organization the store records; any other caller is refused with
//!    `AccessDeniedException` (403) and nothing is written.
//!
//! Both tables span the class each rule generalises over — rule 1 over the account
//! field of the ARN (several foreign accounts, foreign regions and partitions, an ARN
//! whose team name DOES exist in the caller's account, and an ARN of maximal length),
//! applied to every operation that takes one; rule 2 over who the store records as the
//! management account (nobody, another account, the caller).
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the route, the URI bindings, the error
//! envelope and the HTTP status are all exercised.

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{error_code, expected_arn, text, Harness, REGION};

/// `AccessDeniedException$Message` is `@length(max: 1000)`, so a refusal naming a
/// resource ARN (`ApprovalTeamArn` is `length(20..2048)`) is cut to this bound.
const MAX_MESSAGE: usize = 1000;

/// A create body the modelled constraints accept: one approver, one policy, `MofN` with
/// M = 1. The authorization gate is reached whatever the body carries, so one valid body
/// is enough for these tables.
fn valid_create_body(name: &str) -> Value {
    json!({
        "Name": name,
        "Description": "authorization gate",
        "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
        "Approvers": [{
            "PrimaryIdentityId": "identity-0001",
            "PrimaryIdentitySourceArn":
                format!(
                    "arn:aws:mpa:{REGION}:{}:identity-source/ssoins-0001",
                    engine::emulator_account_id()
                ),
        }],
        "Policies": [{
            "PolicyArn": "arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy1/$DEFAULT"
        }],
    })
}

/// The refusal a cross-account ARN earns, as the rule builds it: the caller principal,
/// the attempted `mpa:` action and the ARN that was asked for — cut to the member's
/// modelled bound.
pub(crate) fn expected_denial(action: &str, arn: &str) -> String {
    let full = format!(
        "User: arn:aws:iam::{}:user/emulator is not authorized to perform: mpa:{action} on \
         resource: {arn} because no resource-based policy allows the mpa:{action} action",
        engine::emulator_account_id()
    );
    if full.len() <= MAX_MESSAGE {
        full
    } else {
        full[..MAX_MESSAGE].to_string()
    }
}

/// A team ARN in `account`, with the partition, region and team name given.
fn team_arn(partition: &str, region: &str, account: &str, name: &str) -> String {
    format!("arn:{partition}:mpa:{region}:{account}:approval-team/{name}")
}

/// One row of the cross-account table: an ARN, and whether the account it names is the
/// caller's own.
struct ArnCase {
    case: &'static str,
    arn: String,
    own_account: bool,
}

/// The ARN table. Rows differ along the account field (the one thing the rule keys on),
/// the region, the partition, whether a team of that name exists in the caller's own
/// account, and the ARN's length.
fn arn_cases(existing_team_name: &str) -> Vec<ArnCase> {
    let own = engine::emulator_account_id();
    vec![
        ArnCase {
            case: "another account, same region",
            arn: team_arn("aws", REGION, "999999999999", "probe-team"),
            own_account: false,
        },
        ArnCase {
            case: "another account, another region",
            arn: team_arn("aws", "eu-west-1", "210987654321", "probe-team"),
            own_account: false,
        },
        ArnCase {
            case: "another account, one digit from the caller's",
            arn: team_arn("aws", REGION, "000000000001", "probe-team"),
            own_account: false,
        },
        ArnCase {
            case: "another account, another partition",
            arn: team_arn("aws-us-gov", "us-gov-west-1", "100000000000", "probe-team"),
            own_account: false,
        },
        ArnCase {
            // The denial is decided before the resource is resolved, so an ARN whose
            // team name DOES name a team of the caller's is refused just the same: the
            // account field is what decides.
            case: "another account, a team name the caller does own",
            arn: team_arn("aws", REGION, "999999999999", existing_team_name),
            own_account: false,
        },
        ArnCase {
            // `ApprovalTeamArn` is `length(20..2048)`: an ARN at that bound makes the
            // refusal longer than `Message`'s own `length(max: 1000)`, which it must
            // still respect.
            case: "another account, an ARN at the modelled length bound",
            arn: {
                let prefix = team_arn("aws", REGION, "999999999999", "");
                let name = "n".repeat(2048 - prefix.len());
                format!("{prefix}{name}")
            },
            own_account: false,
        },
        ArnCase {
            case: "the caller's own account, no such team",
            arn: team_arn("aws", REGION, own, "never-created"),
            own_account: true,
        },
    ]
}

/// Rule 1, over every operation that identifies its resource by ARN: an ARN outside the
/// caller's account is `AccessDeniedException` (403) and an ARN inside it that names no
/// stored team is `ResourceNotFoundException` (404) — existence decides nothing about
/// which of the two a caller gets.
#[tokio::test]
async fn a_resource_arn_outside_the_callers_account_is_denied_before_resolution() {
    let harness = Harness::new();

    // A real team of the caller's, so a row can ask for its name in another account.
    let (status, created) = harness.create(&valid_create_body("owned-team")).await;
    assert_eq!(status, 200, "create returned {created:?}");
    let owned_arn = text(&created, "Arn");
    assert_eq!(owned_arn, expected_arn("owned-team"), "create minted the wrong Arn");

    for ArnCase {
        case,
        arn,
        own_account,
    } in arn_cases("owned-team")
    {
        // (operation name, the call)
        let calls: [(&str, (u16, Value)); 3] = [
            ("GetApprovalTeam", harness.get(&arn).await),
            (
                "UpdateApprovalTeam",
                harness.update(&arn, &json!({ "Description": "denied" })).await,
            ),
            (
                "StartActiveApprovalTeamDeletion",
                harness.start_deletion(&arn, None).await,
            ),
        ];

        for (action, (status, body)) in calls {
            let (expected_status, expected_code) = if own_account {
                (404, "ResourceNotFoundException")
            } else {
                (403, "AccessDeniedException")
            };
            assert_eq!(
                status, expected_status,
                "case {case:?} / {action}: expected HTTP {expected_status}, got {status} \
                 with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?} / {action}: expected error code {expected_code}, got {body:?}"
            );
            let message = text(&body, "Message");
            if own_account {
                continue;
            }
            assert_eq!(
                message,
                expected_denial(action, &arn),
                "case {case:?} / {action}: wrong denial message, got {message:?}"
            );
            assert!(
                message.len() <= MAX_MESSAGE,
                "case {case:?} / {action}: Message is {} bytes, past the modelled bound of \
                 {MAX_MESSAGE}",
                message.len()
            );
        }
    }

    // A refused request is a refusal: the caller's own team is exactly as it was, so no
    // cross-account call changed or removed anything.
    let (status, team) = harness.get(&owned_arn).await;
    assert_eq!(status, 200, "the caller's own team is gone: {team:?}");
    assert_eq!(
        text(&team, "VersionId"),
        text(&created, "VersionId"),
        "a denied cross-account update bumped the caller's own team: {team:?}"
    );
    assert_eq!(
        text(&team, "Description"),
        "authorization gate",
        "a denied cross-account update changed the caller's own team: {team:?}"
    );
    assert_ne!(
        text(&team, "Status"),
        "DELETING",
        "a denied cross-account deletion started one on the caller's own team: {team:?}"
    );
}

/// Rule 2: `CreateApprovalTeam` is permitted to the management account of the recorded
/// organization and to nobody else — including when no organization is recorded, which
/// is the state this emulator starts in.
#[tokio::test]
async fn create_is_refused_unless_the_caller_is_the_organizations_management_account() {
    // (case, recorded management account, permitted)
    let rows: [(&str, Option<&str>, bool); 4] = [
        ("no organization recorded", None, false),
        ("another account is the management account", Some("999999999999"), false),
        (
            "an account one digit from the caller's is the management account",
            Some("000000000001"),
            false,
        ),
        (
            "the caller's own account is the management account",
            Some(engine::emulator_account_id()),
            true,
        ),
    ];

    for (case, management, permitted) in rows {
        let harness = Harness::without_organization();
        if let Some(account) = management {
            harness.record_management_account(account);
        }

        let (status, body) = harness.create(&valid_create_body("gated-team")).await;
        if permitted {
            assert_eq!(status, 200, "case {case:?}: create returned {body:?}");
            let arn = text(&body, "Arn");
            let (status, team) = harness.get(&arn).await;
            assert_eq!(status, 200, "case {case:?}: the created team reads back as {team:?}");
            assert_eq!(text(&team, "Name"), "gated-team", "case {case:?}: wrong Name");
            continue;
        }

        assert_eq!(status, 403, "case {case:?}: create returned {status} with {body:?}");
        assert_eq!(
            error_code(&body),
            "AccessDeniedException",
            "case {case:?}: wrong error code in {body:?}"
        );
        assert_eq!(
            text(&body, "Message"),
            "Action can only be performed by an organization\u{2019}s management account.",
            "case {case:?}: wrong refusal message"
        );

        // A refused create wrote NOTHING: no team under the ARN it would have minted,
        // none in the store, and nothing for `ListApprovalTeams` to report.
        let (status, team) = harness.get(&expected_arn("gated-team")).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused create still recorded a team: {team:?}"
        );
        let stored = {
            let store = state::service_state(&*harness.engine).expect("the service store");
            let guard = store.lock();
            guard.approval_teams.len()
        };
        assert_eq!(stored, 0, "case {case:?}: the store holds {stored} team(s) after a refusal");
        let (status, listed) = harness.list("").await;
        assert_eq!(status, 200, "case {case:?}: list returned {listed:?}");
        let teams = listed
            .get("ApprovalTeams")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        assert_eq!(teams, 0, "case {case:?}: list reported {teams} team(s) after a refusal");
    }
}

/// The ARN helpers the gate is built on, over the account field alone: the caller's own
/// account is the only one `is_caller_account_arn` accepts, and an ARN with no account
/// field is not the caller's.
#[test]
fn only_the_callers_own_account_owns_an_arn() {
    let own = engine::emulator_account_id();

    // (case, arn, the account it embeds, owned by the caller)
    let rows: [(&str, String, Option<&str>, bool); 6] = [
        (
            "the caller's own account",
            team_arn("aws", REGION, own, "team"),
            Some(own),
            true,
        ),
        (
            "the caller's account in another partition",
            team_arn("aws-cn", "cn-north-1", own, "team"),
            Some(own),
            true,
        ),
        (
            "another account",
            team_arn("aws", REGION, "999999999999", "team"),
            Some("999999999999"),
            false,
        ),
        (
            "an account one digit away",
            team_arn("aws", REGION, "000000000001", "team"),
            Some("000000000001"),
            false,
        ),
        (
            "an account-less ARN",
            "arn:aws:mpa:::aws:policy/IdentityCenter/P/$DEFAULT".to_string(),
            Some(""),
            false,
        ),
        ("not an ARN at all", "approval-team/team".to_string(), None, false),
    ];

    for (case, arn, account, owned) in rows {
        assert_eq!(
            state::arn_account_id(&arn),
            account,
            "case {case:?}: wrong account read out of {arn:?}"
        );
        assert_eq!(
            state::is_caller_account_arn(&arn),
            owned,
            "case {case:?}: expected owned == {owned} for {arn:?}"
        );
        let authorized = state::authorize_resource_arn("GetApprovalTeam", &arn);
        assert_eq!(
            authorized.is_ok(),
            owned,
            "case {case:?}: expected authorize_resource_arn to be ok == {owned} for {arn:?}"
        );
        if let Err(denied) = authorized {
            assert_eq!(
                denied.message,
                expected_denial("GetApprovalTeam", &arn),
                "case {case:?}: wrong denial message"
            );
        }
    }
}
