// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListShareInvitations`.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded request:
//!
//! 1. An invitation is offered to the principal a resource was shared WITH and is
//!    outstanding while its share is still waiting to be answered, so the operation
//!    reports exactly the shares whose `SharedWith` names the CALLING principal and
//!    whose `ShareStatus` is `PENDING`: a share offered to any other principal is never
//!    reported, and neither is one in any of the seven other published statuses.
//! 2. A reported invitation carries the share it stands for: its own
//!    `ShareInvitationId` (`^[0-9a-f]{32}$`, unique, and not the `ShareId` of the share),
//!    the sharer, the recipient, the permission, the resource kind `WORKLOAD`, and the
//!    shared workload's id and name — and none of the members belonging to invitations
//!    of another resource kind.
//! 3. It narrows by `ShareResourceType` over every published member and by the one name
//!    prefix a request may name; the four name prefixes are mutually exclusive, so a
//!    request naming two of them is refused `400 ValidationException`.
//! 4. It pages over every invitation exactly once at any `MaxResults`, refuses a
//!    `NextToken` it did not issue and a `MaxResults` outside the published range, and
//!    changes nothing a later caller sees.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.
//!
//! The shares the invitations stand for are seeded through
//! `Client::record_workload_share` (see its doc): a share whose recipient is the caller
//! has no HTTP path in a single-account emulator, while the share offered to ANOTHER
//! principal — the negative row of rule 1 — is created over the wire.

use serde_json::Value;

use super::workload_harness::{created_id, minimal_create, Answer, Client};

/// The `PermissionType` members the model publishes —
/// `smithy_query(query="shape", shape="PermissionType")` lists exactly these two.
const PERMISSION_TYPES: [&str; 2] = ["CONTRIBUTOR", "READONLY"];

/// The `ShareStatus` members the model publishes —
/// `smithy_query(query="shape", shape="ShareStatus")`.
const SHARE_STATUSES: [&str; 8] = [
    "ACCEPTED",
    "ASSOCIATED",
    "ASSOCIATING",
    "EXPIRED",
    "FAILED",
    "PENDING",
    "REJECTED",
    "REVOKED",
];

/// The `ShareResourceType` members the model publishes —
/// `smithy_query(query="shape", shape="ShareResourceType")`.
const SHARE_RESOURCE_TYPES: [&str; 4] = ["WORKLOAD", "LENS", "PROFILE", "TEMPLATE"];

/// The mutually exclusive name-prefix inputs, in the order this operation's doc names
/// them: "WorkloadNamePrefix, LensNamePrefix, ProfileNamePrefix, and TemplateNamePrefix
/// are mutually exclusive."
const NAME_PREFIX_MEMBERS: [&str; 4] = [
    "WorkloadNamePrefix",
    "LensNamePrefix",
    "ProfileNamePrefix",
    "TemplateNamePrefix",
];

/// The `ShareInvitationSummaries` of a successful `ListShareInvitations`.
fn invitations(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListShareInvitations 200, got {answer:?}"
    );
    answer
        .body
        .get("ShareInvitationSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: ListShareInvitations must report ShareInvitationSummaries, \
                 got {answer:?}"
            )
        })
}

fn member<'a>(value: &'a Value, name: &str) -> &'a str {
    value.get(name).and_then(Value::as_str).unwrap_or_default()
}

/// The `ShareInvitationId`s a page reported, in the order it reported them.
fn invitation_ids(page: &[Value]) -> Vec<String> {
    page.iter()
        .map(|invitation| member(invitation, "ShareInvitationId").to_string())
        .collect()
}

/// A workload created over HTTP plus one share of it seeded into the shared state.
struct Seeded {
    workload_id: String,
    workload_name: String,
    share_id: String,
    invitation_id: String,
    permission_type: String,
    status: String,
}

/// Create a workload over HTTP and record one share of it with `shared_with`.
async fn seed(
    client: &Client,
    workload_name: &str,
    shared_with: &str,
    permission_type: &str,
    status: &str,
) -> Seeded {
    let workload_id = created_id(
        &client
            .create(minimal_create(workload_name, &format!("tok-{workload_name}")))
            .await,
        workload_name,
    );
    let (share_id, invitation_id) =
        client.record_workload_share(&workload_id, shared_with, permission_type, status);
    Seeded {
        workload_id,
        workload_name: workload_name.to_string(),
        share_id,
        invitation_id,
        permission_type: permission_type.to_string(),
        status: status.to_string(),
    }
}

/// Rules 1 and 2: the operation reports exactly the `PENDING` shares addressed to the
/// caller, each as the invitation its recipient was offered. The rows span every
/// published `ShareStatus` crossed with every published `PermissionType`, plus a share
/// offered over HTTP to a principal that is not the caller.
#[tokio::test]
async fn invitations_are_the_pending_shares_addressed_to_the_calling_principal() {
    let client = Client::new();
    let caller = engine::emulator_account_id();

    let mut seeded: Vec<Seeded> = Vec::new();
    for status in SHARE_STATUSES {
        for permission_type in PERMISSION_TYPES {
            let name = format!(
                "invitation-{}-{}",
                status.to_lowercase(),
                permission_type.to_lowercase()
            );
            seeded.push(seed(&client, &name, caller, permission_type, status).await);
        }
    }

    // The negative row of rule 1, over the wire: a share offered to another principal is
    // not an invitation for this caller. It is created on the workload whose own share is
    // outstanding, so the same workload carries both an invitation and a share that is
    // none of this caller's business.
    let host = seeded
        .iter()
        .find(|case| case.status == "PENDING")
        .expect("one seeded share waits on its invitation");
    let offered_elsewhere = client
        .create_share(
            &host.workload_id,
            serde_json::json!({
                "SharedWith": "111122223333",
                "PermissionType": "READONLY",
                "ClientRequestToken": "tok-offered-elsewhere",
            }),
        )
        .await;
    assert_eq!(
        offered_elsewhere.status, 200,
        "case \"offered-elsewhere\": expected CreateWorkloadShare 200, got {offered_elsewhere:?}"
    );

    let listed = client.list_share_invitations("").await;
    let reported = invitations(&listed, "every-invitation");
    let reported_ids = invitation_ids(&reported);

    for case in &seeded {
        let expected = case.status == "PENDING";
        assert_eq!(
            reported_ids.contains(&case.invitation_id),
            expected,
            "case {:?}: a share in status {:?} must{} be reported as an invitation, got {listed:?}",
            case.workload_name,
            case.status,
            if expected { "" } else { " not" }
        );
    }
    let pending = seeded.iter().filter(|case| case.status == "PENDING").count();
    assert_eq!(
        reported.len(),
        pending,
        "case \"every-invitation\": expected {pending} invitation(s), got {listed:?}"
    );

    // Rule 2: every member of the summary, against the share it stands for.
    let read_back = client.get(&host.workload_id).await;
    let owner = read_back
        .body
        .get("Workload")
        .map(|workload| member(workload, "Owner"))
        .unwrap_or_default()
        .to_string();
    assert!(
        !owner.is_empty(),
        "the workload under test must report its owning account, got {read_back:?}"
    );
    for case in seeded.iter().filter(|case| case.status == "PENDING") {
        let found = reported
            .iter()
            .find(|invitation| member(invitation, "ShareInvitationId") == case.invitation_id)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the pending share must be reported, got {listed:?}",
                    case.workload_name
                )
            });
        let id = member(found, "ShareInvitationId");
        assert!(
            id.len() == 32 && id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "case {:?}: ShareInvitationId must match ^[0-9a-f]{{32}}$, got {id:?}",
            case.workload_name
        );
        assert_ne!(
            id, case.share_id,
            "case {:?}: the invitation is its own resource, so its id is not the ShareId, \
             got {found:?}",
            case.workload_name
        );
        assert_eq!(
            member(found, "ShareResourceType"),
            "WORKLOAD",
            "case {:?}: a share of a workload is a WORKLOAD invitation, got {found:?}",
            case.workload_name
        );
        assert_eq!(
            member(found, "WorkloadId"),
            case.workload_id,
            "case {:?}: the invitation must name the shared workload, got {found:?}",
            case.workload_name
        );
        assert_eq!(
            member(found, "WorkloadName"),
            case.workload_name,
            "case {:?}: the invitation must name the shared workload, got {found:?}",
            case.workload_name
        );
        assert_eq!(
            member(found, "PermissionType"),
            case.permission_type,
            "case {:?}: the invitation must carry the permission the share grants, got {found:?}",
            case.workload_name
        );
        assert_eq!(
            member(found, "SharedWith"),
            caller,
            "case {:?}: the invitation is the one offered to the caller, got {found:?}",
            case.workload_name
        );
        assert_eq!(
            member(found, "SharedBy"),
            owner,
            "case {:?}: the invitation must name the account that shared the workload, \
             got {found:?}",
            case.workload_name
        );
        // The members of an invitation of another resource kind belong to that kind.
        for absent in [
            "LensName",
            "LensArn",
            "ProfileName",
            "ProfileArn",
            "TemplateName",
            "TemplateArn",
        ] {
            assert_eq!(
                found.get(absent),
                None,
                "case {:?}: a workload invitation carries no {absent}, got {found:?}",
                case.workload_name
            );
        }
    }

    // Every reported invitation is a distinct resource.
    let mut unique = reported_ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        reported_ids.len(),
        "case \"every-invitation\": no invitation may be reported twice, got {reported_ids:?}"
    );

    // Reading does not mutate: a second read reports the same invitations, and the
    // shares they stand for still wait on them.
    let again = client.list_share_invitations("").await;
    assert_eq!(
        reported,
        invitations(&again, "repeat-read"),
        "case \"repeat-read\": a read must not change what the next caller sees, got \
         {listed:?} then {again:?}"
    );
    let shares = client.list_shares(&host.workload_id, "").await;
    let host_status = shares
        .body
        .get("WorkloadShareSummaries")
        .and_then(Value::as_array)
        .and_then(|summaries| {
            summaries
                .iter()
                .find(|summary| member(summary, "ShareId") == host.share_id)
                .map(|summary| member(summary, "Status"))
        })
        .unwrap_or_default();
    assert_eq!(
        host_status, host.status,
        "case \"repeat-read\": reading the invitations must not move a share's status, \
         got {shares:?}"
    );
}

/// Rule 3: narrowing by `ShareResourceType` over every published member, and by the one
/// name prefix a request may name. The prefix rows differ along the prefix's length and
/// along how many of the shared workloads it selects; the prefixes of the other resource
/// kinds select nothing, because a workload invitation carries no name for them to match.
#[tokio::test]
async fn invitations_narrow_by_resource_type_and_by_the_matching_name_prefix() {
    let client = Client::new();
    let caller = engine::emulator_account_id();

    // Names chosen so prefixes of different lengths select different counts.
    for (index, workload_name) in ["alpha-invitation", "alpaca-invitation", "beta-invitation"]
        .into_iter()
        .enumerate()
    {
        seed(
            &client,
            workload_name,
            caller,
            PERMISSION_TYPES[index % PERMISSION_TYPES.len()],
            "PENDING",
        )
        .await;
    }
    const SEEDED: usize = 3;

    for resource_type in SHARE_RESOURCE_TYPES {
        // Only workload shares can exist in this emulator: `CreateLensShare`,
        // `CreateProfileShare` and `CreateTemplateShare` are still stubs with their own
        // issues, so no invitation of those kinds has been offered.
        let expected = if resource_type == "WORKLOAD" { SEEDED } else { 0 };
        let listed = client
            .list_share_invitations(&format!("ShareResourceType={resource_type}"))
            .await;
        let found = invitations(&listed, resource_type);
        assert_eq!(
            found.len(),
            expected,
            "case {resource_type:?}: expected {expected} invitation(s) of that kind, got {listed:?}"
        );
        for invitation in &found {
            assert_eq!(
                member(invitation, "ShareResourceType"),
                resource_type,
                "case {resource_type:?}: every reported invitation must be of that kind, \
                 got {listed:?}"
            );
        }
    }

    // A `ShareResourceType` outside the published set is a bad request, not an empty page.
    let bogus = client
        .list_share_invitations("ShareResourceType=NOT_A_RESOURCE_TYPE")
        .await;
    assert_eq!(
        bogus.status, 400,
        "case \"unknown-resource-type\": expected HTTP 400, got {bogus:?}"
    );
    assert_eq!(
        bogus.error_code, "ValidationException",
        "case \"unknown-resource-type\": expected ValidationException, got {bogus:?}"
    );

    for (case, prefix, expected) in [
        ("every-workload", "", SEEDED),
        ("one-letter", "a", 2),
        ("two-letters", "al", 2),
        ("one-workload", "alpha", 1),
        ("another-workload", "b", 1),
        ("exact-name", "beta-invitation", 1),
        ("no-workload", "zzz", 0),
    ] {
        let listed = client
            .list_share_invitations(&format!("WorkloadNamePrefix={prefix}"))
            .await;
        let found = invitations(&listed, case);
        assert_eq!(
            found.len(),
            expected,
            "case {case:?}: expected {expected} invitation(s) for prefix {prefix:?}, got {listed:?}"
        );
        for invitation in &found {
            let name = member(invitation, "WorkloadName");
            assert!(
                name.starts_with(prefix),
                "case {case:?}: {name:?} does not start with {prefix:?}, got {listed:?}"
            );
        }
    }

    // A prefix that names another resource kind's name selects invitations of that kind,
    // of which there are none. `TemplateNamePrefix` is modelled with a pattern that the
    // empty string does not satisfy (`smithy_query(query="operation",
    // operation="ListShareInvitations")`), so the empty row covers the two prefixes whose
    // only constraint is a maximum length.
    for (member_name, prefixes) in [
        ("LensNamePrefix", ["", "alpha"].as_slice()),
        ("ProfileNamePrefix", ["", "alpha"].as_slice()),
        ("TemplateNamePrefix", ["alpha"].as_slice()),
    ] {
        for prefix in prefixes {
            let case = format!("{member_name}={prefix}");
            let listed = client
                .list_share_invitations(&format!("{member_name}={prefix}"))
                .await;
            assert_eq!(
                invitations(&listed, &case).len(),
                0,
                "case {case:?}: a workload invitation carries no name for that prefix, \
                 got {listed:?}"
            );
        }
    }

    // A `ShareResourceType` and a prefix of another kind select nothing together, for the
    // same reason.
    let crossed = client
        .list_share_invitations("ShareResourceType=WORKLOAD&LensNamePrefix=alpha")
        .await;
    assert_eq!(
        invitations(&crossed, "workload-with-a-lens-prefix").len(),
        0,
        "case \"workload-with-a-lens-prefix\": expected no invitation, got {crossed:?}"
    );
}

/// Rule 3, mutual exclusion: a request naming two of the four name prefixes is refused
/// `400 ValidationException` naming each prefix it set, while each one alone is served.
/// The rows are every unordered PAIR of the four published members.
#[tokio::test]
async fn the_four_name_prefixes_are_mutually_exclusive() {
    let client = Client::new();
    seed(
        &client,
        "exclusive-invitation",
        engine::emulator_account_id(),
        "READONLY",
        "PENDING",
    )
    .await;

    for (index, first) in NAME_PREFIX_MEMBERS.iter().enumerate() {
        let alone = client.list_share_invitations(&format!("{first}=a")).await;
        assert_eq!(
            alone.status, 200,
            "case {first:?}: one prefix alone must be served, got {alone:?}"
        );

        for second in &NAME_PREFIX_MEMBERS[index + 1..] {
            let case = format!("{first}+{second}");
            let refused = client
                .list_share_invitations(&format!("{first}=a&{second}=b"))
                .await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: two mutually exclusive prefixes must be a 400, got {refused:?}"
            );
            assert_eq!(
                refused.error_code, "ValidationException",
                "case {case:?}: expected ValidationException, got {refused:?}"
            );
            let fields: Vec<&str> = refused
                .body
                .get("Fields")
                .and_then(Value::as_array)
                .map(|fields| {
                    fields
                        .iter()
                        .map(|field| member(field, "Name"))
                        .collect()
                })
                .unwrap_or_default();
            assert!(
                fields.contains(first) && fields.contains(second),
                "case {case:?}: the refusal must name both members it covers, got {refused:?}"
            );
        }
    }
}

/// Rule 4, paging: at any `MaxResults` the pages enumerate every invitation exactly once
/// and the last page carries no `NextToken`. Rows are the published `@range(1, 50)`
/// minimum, values between it and the invitation count, the count itself and the
/// published maximum; a value past either bound, and a token this service never issued,
/// are refused.
#[tokio::test]
async fn paging_enumerates_every_invitation_exactly_once_at_any_page_size() {
    let client = Client::new();
    let caller = engine::emulator_account_id();

    const INVITATION_COUNT: usize = 7;
    let mut all: Vec<String> = Vec::new();
    for index in 0..INVITATION_COUNT {
        let seeded = seed(
            &client,
            &format!("paged-invitation-{index}"),
            caller,
            PERMISSION_TYPES[index % PERMISSION_TYPES.len()],
            "PENDING",
        )
        .await;
        all.push(seeded.invitation_id);
    }
    all.sort();

    for page_size in [1usize, 2, 3, INVITATION_COUNT, 50] {
        let case = format!("max-results-{page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            pages += 1;
            assert!(
                pages <= INVITATION_COUNT + 1,
                "case {case:?}: paging must terminate, walked {walked:?}"
            );
            let query = match &next_token {
                Some(token) => format!("MaxResults={page_size}&NextToken={token}"),
                None => format!("MaxResults={page_size}"),
            };
            let listed = client.list_share_invitations(&query).await;
            let page = invitations(&listed, &case);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} invitation(s), got {listed:?}"
            );
            walked.extend(invitation_ids(&page));
            next_token = listed
                .body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }
        let mut sorted = walked.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            walked.len(),
            "case {case:?}: no invitation may be reported twice, got {walked:?}"
        );
        assert_eq!(
            sorted, all,
            "case {case:?}: paging must enumerate every invitation, got {walked:?}"
        );
    }

    // The published bounds are served; a value past either of them is refused.
    for (case, max_results, expected) in [
        ("published-minimum", 1, 200),
        ("published-maximum", 50, 200),
        ("below-the-minimum", 0, 400),
        ("above-the-maximum", 51, 400),
    ] {
        let listed = client
            .list_share_invitations(&format!("MaxResults={max_results}"))
            .await;
        assert_eq!(
            listed.status, expected,
            "case {case:?}: expected HTTP {expected} for MaxResults={max_results}, got {listed:?}"
        );
        if expected == 400 {
            assert_eq!(
                listed.error_code, "ValidationException",
                "case {case:?}: expected ValidationException, got {listed:?}"
            );
        }
    }

    // A `NextToken` this service never issued is a bad request, not an empty page.
    for (case, token) in [
        ("not-hex", "notatoken"),
        ("hex-but-not-a-cursor", "6162636465"),
        ("a-workload-shares-cursor", "7b2273223a2261227d"),
        ("a-workloads-cursor", "7b226e223a2261222c2269223a2262227d"),
    ] {
        let listed = client
            .list_share_invitations(&format!("NextToken={token}"))
            .await;
        assert_eq!(
            listed.status, 400,
            "case {case:?}: expected HTTP 400 for a foreign NextToken, got {listed:?}"
        );
        assert_eq!(
            listed.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {listed:?}"
        );
    }
}
