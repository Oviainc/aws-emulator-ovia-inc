// SPDX-License-Identifier: CC0-1.0

//! WHICH identifier an authorization refusal names the app monitor by, and which one the
//! decision behind it is taken on.
//!
//! The rule under test: a refused call is answered with `AccessDeniedException` (HTTP 403)
//! whose sentence names the resource by the identifier the OPERATION's own request
//! addresses it by — the data plane addresses the monitor by its minted `AppMonitorId` and
//! takes no other resource member (`smithy_query(query="operation",
//! operation="PutRumEvents")`: `Id` is the only `@httpLabel`, `uri /appmonitors/{Id}/`), so
//! its refusal names that id, while the operations that address the monitor by its `Name`
//! name the monitor's ARN. The policy DECISION is taken on the ARN whichever operation it
//! is: a statement's `Resource` is matched against the ARN, so what a refusal spells
//! changes nothing about who is allowed what.
//!
//! The first table spans what the rule generalises over: the denial class (and so the
//! clause the sentence ends in), the kind of principal the signing credential speaks for
//! (an STS session and an IAM user, spelled differently), the app monitor the call is made
//! on (a fresh one per row, so the id the refusal must name differs every time and no row
//! can pass by naming a value another row also holds), and whether the batch carries an
//! `Alias`.
//!
//! The second table is what keeps the rule honest: on ONE monitor and for ONE principal,
//! the data-plane refusal names the id the request carried and the `Name`-addressed
//! operation's refusal names the ARN — the identifier is the one the operation takes,
//! rather than one of the two everywhere.
//!
//! The third test is the positive control, and the evidence that the ARN is still what the
//! decision is made on: a grant whose `Resource` is the monitor's ARN — the id appears
//! nowhere in it — turns the same refused call into an accepted one, and removing it refuses
//! it again with the same sentence.

use serde_json::Value;

use crate::authorization::{
    account, identity_policy, monitor_arn, role_arn, rum_events_body, statement, Emulator, Reply,
};

/// The status and wire code every refusal below answers with, as the model declares for
/// `AccessDeniedException` (`errorKind` client, `httpError` 403).
const ACCESS_DENIED_CODE: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 403;

/// The success status `PutRumEvents` declares (`http.code` 200).
const OK_STATUS: u16 = 200;

/// The action the data plane is authorized against, and the one the `Name`-addressed
/// operation the contrast table reads it against is.
const DATA_PLANE_ACTION: &str = "rum:PutRumEvents";
const CONTROL_PLANE_ACTION: &str = "rum:GetAppMonitor";

/// An action of this service that no row below is authorized against.
const UNRELATED_ACTION: &str = "rum:ListAppMonitors";

/// Which clause a refusal ends in: the kind of policy that took the decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Clause {
    /// No statement of the caller's identity-based policies allowed the action.
    NoIdentityPolicy,
    /// A statement of the caller's identity-based policies denied it outright.
    IdentityDeny,
    /// A statement of the resource's own policy denied it outright.
    ResourceDeny,
}

impl Clause {
    /// The clause as IAM words it for `action`.
    fn text(self, action: &str) -> String {
        match self {
            Self::NoIdentityPolicy => {
                format!("because no identity-based policy allows the {action} action")
            }
            Self::IdentityDeny => "with an explicit deny in an identity-based policy".to_string(),
            Self::ResourceDeny => "with an explicit deny in a resource-based policy".to_string(),
        }
    }
}

/// The kind of principal the signing credential speaks for, which decides how the refusal
/// spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrincipalKind {
    /// An STS session over an IAM role: `sts:GetCallerIdentity` reports the session ARN,
    /// which the harness takes the session name `probe` in.
    Session,
    /// An IAM user signing with a long-term key it holds.
    User,
}

impl PrincipalKind {
    fn slug(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::User => "user",
        }
    }

    /// The ARN a refusal names a principal of this kind by.
    fn arn(self, name: &str) -> String {
        match self {
            Self::Session => format!("arn:aws:sts::{}:assumed-role/{name}/probe", account()),
            Self::User => format!("arn:aws:iam::{}:user/{name}", account()),
        }
    }
}

/// Every kind of principal the sentence has to spell.
const PRINCIPAL_KINDS: [PrincipalKind; 2] = [PrincipalKind::Session, PrincipalKind::User];

/// One denial class: which policies are in force, and the clause that answers for them.
struct DenialCase {
    case: &'static str,
    /// The caller's inline identity-based policy document, written over every action the
    /// case's calls ask for — so one principal carrying it is in this class for each of
    /// them.
    identity: fn(&[&str]) -> Option<String>,
    /// The app monitor's resource-based policy document, with the monitor's own ARN (the
    /// argument) as its `Resource`.
    resource: fn(&str) -> Option<String>,
    /// The `Alias` the batch carries, which the `rum:alias` condition key compares against
    /// and which changes nothing about the identifier the refusal names.
    alias: Option<&'static str>,
    clause: Clause,
}

/// The denial classes the sentence has to tell apart: an implicit deny reached two ways,
/// and an explicit `Deny` from each kind of policy.
fn denial_cases() -> Vec<DenialCase> {
    vec![
        DenialCase {
            case: "a principal holding no policy at all",
            identity: |_| None,
            resource: |_| None,
            alias: None,
            clause: Clause::NoIdentityPolicy,
        },
        DenialCase {
            case: "an identity policy allowing another action of this service",
            identity: |_| Some(identity_policy(&statement("Allow", UNRELATED_ACTION, "*"))),
            resource: |_| None,
            alias: Some("live"),
            clause: Clause::NoIdentityPolicy,
        },
        DenialCase {
            case: "an identity policy allowing the service and denying these actions",
            identity: |actions| {
                let denies: Vec<String> = actions
                    .iter()
                    .map(|action| statement("Deny", action, "*"))
                    .collect();
                Some(identity_policy(&format!(
                    "{},{}",
                    statement("Allow", "rum:*", "*"),
                    denies.join(",")
                )))
            },
            resource: |_| None,
            alias: None,
            clause: Clause::IdentityDeny,
        },
        DenialCase {
            case: "an identity policy that allows and a resource policy denying the ARN",
            identity: |_| Some(identity_policy(&statement("Allow", "rum:*", "*"))),
            resource: |arn| Some(monitor_policy("Deny", "*", "rum:*", arn)),
            alias: Some("staging"),
            clause: Clause::ResourceDeny,
        },
    ]
}

/// A resource policy about `principal` and `action` whose `Resource` is `arn`, the app
/// monitor's own ARN — the identifier a policy statement names a RUM resource by, and the
/// one the decision is therefore taken on.
fn monitor_policy(effect: &str, principal: &str, action: &str, arn: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
         \"Principal\":{{\"AWS\":\"{principal}\"}},\"Action\":\"{action}\",\
         \"Resource\":\"{arn}\"}}]}}"
    )
}

/// The whole sentence: the principal, the action, the resource the refusal names, and the
/// clause naming the policy kind that withheld the grant.
fn expected_message(principal_arn: &str, action: &str, resource: &str, clause: Clause) -> String {
    format!(
        "User: {principal_arn} is not authorized to perform: {action} on resource: {resource} {}",
        clause.text(action)
    )
}

/// Make the principal of `kind` named `principal`, holding `identity` when it holds a
/// document, and answer the access key id its calls are signed with.
async fn principal_key(
    emulator: &Emulator,
    kind: PrincipalKind,
    principal: &str,
    identity: Option<&str>,
) -> String {
    match kind {
        PrincipalKind::Session => emulator.assumed_role_key(principal, identity).await,
        PrincipalKind::User => emulator.iam_user_key(principal, identity).await,
    }
}

/// One `PutRumEvents` batch for the monitor whose minted id is `id`, signed with `key`.
async fn put_batch(emulator: &Emulator, key: &str, id: &str, alias: Option<&str>) -> Reply {
    let body: Value = rum_events_body(id, alias);
    emulator
        .rum(key, "POST", &format!("/appmonitors/{id}/"), Some(body))
        .await
}

/// Assert `reply` is the modelled refusal and that its message is exactly `expected`.
fn assert_refused(reply: &Reply, case: &str, expected: &str) {
    assert_eq!(
        reply.status, ACCESS_DENIED_STATUS,
        "case {case:?}: expected HTTP {ACCESS_DENIED_STATUS}, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    assert_eq!(
        reply.error_code(case),
        ACCESS_DENIED_CODE,
        "case {case:?}: expected {ACCESS_DENIED_CODE}, got {:?}",
        String::from_utf8_lossy(&reply.body)
    );
    let actual = reply.message(case);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected the message {expected:?}, got {actual:?}"
    );
}

#[tokio::test]
async fn a_refused_batch_is_named_by_the_id_the_request_carried() {
    for (class_index, class) in denial_cases().iter().enumerate() {
        for kind in PRINCIPAL_KINDS {
            // One emulator per (class, principal kind): every row inside gets its own app
            // monitor — and so its own minted id — and its own principal, so no row
            // inherits another's grant or another's identifier.
            let emulator = Emulator::new();
            for row in 0..2 {
                let case = format!(
                    "PutRumEvents refused for {} ({}, monitor {row})",
                    class.case,
                    kind.slug()
                );
                let monitor = format!("data-plane-{class_index}-{}-{row}", kind.slug());
                let id = emulator.create_monitor(&monitor).await;
                let principal = format!("denied-{class_index}-{}-{row}", kind.slug());
                let identity = (class.identity)(&[DATA_PLANE_ACTION]);
                let key = principal_key(&emulator, kind, &principal, identity.as_deref()).await;
                if let Some(document) = (class.resource)(&monitor_arn(&monitor)) {
                    emulator.put_resource_policy(&monitor, &document).await;
                }

                let reply = put_batch(&emulator, &key, &id, class.alias).await;
                // The id the request carried, not the ARN derived from the monitor's name.
                let expected =
                    expected_message(&kind.arn(&principal), DATA_PLANE_ACTION, &id, class.clause);
                assert_refused(&reply, &case, &expected);
            }
        }
    }
}

#[tokio::test]
async fn the_name_addressed_operation_on_the_same_monitor_is_named_by_its_arn() {
    for (class_index, class) in denial_cases().iter().enumerate() {
        let emulator = Emulator::new();
        let monitor = format!("both-planes-{class_index}");
        let id = emulator.create_monitor(&monitor).await;
        let arn = monitor_arn(&monitor);
        let principal = format!("both-planes-{class_index}");
        // ONE principal for both calls, holding the class's document written over both
        // actions — so each call is in this class and the two refusals differ only in the
        // operation that renders them.
        let identity = (class.identity)(&[DATA_PLANE_ACTION, CONTROL_PLANE_ACTION]);
        let key = principal_key(
            &emulator,
            PrincipalKind::Session,
            &principal,
            identity.as_deref(),
        )
        .await;
        if let Some(document) = (class.resource)(&arn) {
            emulator.put_resource_policy(&monitor, &document).await;
        }
        let principal_arn = PrincipalKind::Session.arn(&principal);

        let data_plane = put_batch(&emulator, &key, &id, class.alias).await;
        assert_refused(
            &data_plane,
            &format!("PutRumEvents refused for {}", class.case),
            &expected_message(&principal_arn, DATA_PLANE_ACTION, &id, class.clause),
        );

        let control_plane = emulator
            .rum(&key, "GET", &format!("/appmonitor/{monitor}"), None)
            .await;
        assert_refused(
            &control_plane,
            &format!("GetAppMonitor refused for {}", class.case),
            &expected_message(&principal_arn, CONTROL_PLANE_ACTION, &arn, class.clause),
        );
    }
}

/// The positive control: the refusals above are decisions taken on the monitor's ARN, so a
/// grant naming that ARN — and nowhere the minted id — turns the same call into an accepted
/// one, and removing the grant refuses it again with the same sentence.
///
/// The rows differ along where the grant comes from: the monitor's own resource policy,
/// which the data-plane caller holds no identity policy beside, and an identity policy
/// naming the ARN.
#[tokio::test]
async fn a_grant_naming_the_monitor_arn_accepts_the_same_batch() {
    for (row, from_resource_policy) in [true, false].into_iter().enumerate() {
        let emulator = Emulator::new();
        let monitor = format!("granted-{row}");
        let id = emulator.create_monitor(&monitor).await;
        let arn = monitor_arn(&monitor);
        let principal = format!("granted-{row}");
        let case = if from_resource_policy {
            format!("a resource policy granting {DATA_PLANE_ACTION} on the ARN")
        } else {
            format!("an identity policy granting {DATA_PLANE_ACTION} on the ARN")
        };
        let identity = (!from_resource_policy)
            .then(|| identity_policy(&statement("Allow", DATA_PLANE_ACTION, &arn)));
        let key = emulator
            .assumed_role_key(&principal, identity.as_deref())
            .await;
        let expected = expected_message(
            &PrincipalKind::Session.arn(&principal),
            DATA_PLANE_ACTION,
            &id,
            Clause::NoIdentityPolicy,
        );

        if from_resource_policy {
            // Before the grant: refused, about the id the request carried.
            let before = put_batch(&emulator, &key, &id, None).await;
            assert_refused(&before, &format!("{case}: before it is put"), &expected);
            emulator
                .put_resource_policy(
                    &monitor,
                    &monitor_policy("Allow", &role_arn(&principal), DATA_PLANE_ACTION, &arn),
                )
                .await;
        }

        let allowed = put_batch(&emulator, &key, &id, None).await;
        assert_eq!(
            allowed.status, OK_STATUS,
            "case {case:?}: expected HTTP {OK_STATUS}, got {} {:?}",
            allowed.status,
            String::from_utf8_lossy(&allowed.body)
        );

        if from_resource_policy {
            // The grant is evaluated on every call, so removing it refuses the same batch
            // again — with the sentence the tables above pin.
            emulator.delete_resource_policy(&monitor).await;
            let after = put_batch(&emulator, &key, &id, None).await;
            assert_refused(&after, &format!("{case}: after it is deleted"), &expected);
        }
    }
}
