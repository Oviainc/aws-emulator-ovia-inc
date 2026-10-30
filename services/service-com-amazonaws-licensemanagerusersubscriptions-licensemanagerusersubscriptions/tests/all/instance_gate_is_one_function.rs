// SPDX-License-Identifier: CC0-1.0

//! The instance-authorization gate is ONE function, and every operation that acts on a
//! caller-named EC2 instance is decided by it — driven over the real wire path, so each call
//! is deserialized, constraint-checked, handled and serialized exactly as a client's would
//! be.
//!
//! The rule under test: whether this service acts on the EC2 instance a request names is
//! decided by one gate over the account's current fleet, which also carries the single
//! fail-closed refusal a request it stops is answered with. Three things follow, and each is
//! asserted on every operation whose input carries an `InstanceId` — `AssociateUser`,
//! `ListUserAssociations` and `DisassociateUser` (`smithy_query(query="operation",
//! operation="AssociateUser,ListUserAssociations,DisassociateUser")`), the three that used
//! to spell the gate out one by one:
//!
//! * an id the account has no instance of is refused with the same code, status and
//!   sentence on each of them;
//! * the decision follows the account's fleet as it stands at the call, so registering that
//!   very id and repeating the very same request is served on each of them — nothing about
//!   the earlier refusal is remembered;
//! * a request that names NO instance has nothing to authorize, so the gate stops none of
//!   them: `DisassociateUser` addressed by `InstanceUserArn` alone is served.
//!
//! The harness (router + engine, with Amazon EC2 beside this service and the account's
//! directory fleet and network the registrations resolve against) is the one in
//! [`super::identity_provider_subscriptions`]; the registration and the two association
//! calls come from [`super::instance_fleet_resolution`]. Only the rule is new here.

use serde_json::json;

use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::state::seed_account_instances;
use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState as ServiceState;

// ONE wire harness per service: these come from the sibling suites rather than a second copy
// of the same dispatch plumbing.
use crate::identity_provider_subscriptions::{
    as_str, call, harness, provider_union, Harness, Reply, REGION,
};
use crate::instance_fleet_resolution::{associate, list_associations, register_provider};

/// The directory the registration these associations are made through is for — the one
/// [`register_provider`] registers, which the harness puts in the account's directory fleet.
const DIRECTORY: &str = "d-0123456789";

/// The code, sentence and status this service fails the gate closed with
/// (`scaffolded::UNAUTHORIZED_EXCEPTION_CODE`, `UNAUTHORIZED_EXCEPTION_MESSAGE` and
/// `UNAUTHORIZED_EXCEPTION_STATUS`). Spelled out here rather than read from the production
/// constants, so a change to them fails a test instead of silently changing what this suite
/// accepts.
const UNAUTHORIZED_CODE: &str = "UnauthorizedException";
const UNAUTHORIZED_MESSAGE: &str =
    "Unauthorized. Please check the request and access policy and try again.";
const REFUSAL_STATUS: u16 = 401;

/// The status a disassociation whose teardown was accepted carries, spelled out for the same
/// reason.
const STATUS_DISASSOCIATING: &str = "DISASSOCIATING";

/// `DisassociateUser` by the `(Username, InstanceId, IdentityProvider)` members, the
/// addressing that NAMES an instance.
async fn disassociate(h: &Harness, username: &str, instance_id: &str) -> Reply {
    call(
        h,
        "/user/DisassociateUser",
        &json!({
            "Username": username,
            "InstanceId": instance_id,
            "IdentityProvider": provider_union(DIRECTORY),
        }),
    )
    .await
}

/// `DisassociateUser` by `InstanceUserArn` alone — the addressing that names NO instance, so
/// the gate has nothing to authorize.
async fn disassociate_by_arn(h: &Harness, arn: &str) -> Reply {
    call(h, "/user/DisassociateUser", &json!({ "InstanceUserArn": arn })).await
}

/// One id the account has no instance of, as the request that names it on each gated
/// operation. The rows differ in the FORM and SIZE of the id (`InstanceId` is a plain
/// `String` in this service's model, with no pattern to vary against) and in whether the id
/// is a near miss on an instance the fleet seam does hold.
#[derive(Debug, Clone)]
struct Row {
    case: &'static str,
    instance_id: String,
}

/// Assert one fail-closed refusal: the code, the status and the sentence, naming the case and
/// showing what was actually answered.
fn refused_closed(reply: &Reply, case: &str) {
    assert_eq!(
        reply.error_type.as_deref(),
        Some(UNAUTHORIZED_CODE),
        "case {case:?}: expected {UNAUTHORIZED_CODE}, got {reply:?}"
    );
    assert_eq!(
        reply.status, REFUSAL_STATUS,
        "case {case:?}: expected HTTP {REFUSAL_STATUS}, got {reply:?}"
    );
    assert_eq!(
        reply.message(),
        Some(UNAUTHORIZED_MESSAGE),
        "case {case:?}: wrong refusal sentence in {reply:?}"
    );
}

/// RULE: one gate over the account's current fleet decides every operation that acts on a
/// caller-named instance — an id the account has no instance of is refused identically on
/// each of them, and the same request on the same id is served once the account has it.
#[tokio::test]
async fn one_gate_decides_every_operation_that_names_an_instance() {
    let h = harness();
    register_provider(&h).await;

    let seeded = seed_account_instances();
    let held = seeded[0].0.clone();
    let rows = [
        Row {
            case: "an id of EC2's 17-hex-digit form",
            instance_id: "i-0aa11bb22cc33dd44".to_string(),
        },
        Row {
            case: "an id of EC2's 8-hex-digit form",
            instance_id: "i-0aa11bb2".to_string(),
        },
        Row {
            case: "an upper-case spelling of an instance of the fleet seam",
            instance_id: held.to_ascii_uppercase(),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let username = format!("user{index}");

        // Before the account has it: the one refusal, on each of the three operations.
        for (operation, reply) in [
            (
                "AssociateUser",
                associate(&h, &username, &row.instance_id).await,
            ),
            (
                "ListUserAssociations",
                list_associations(&h, &row.instance_id).await,
            ),
            (
                "DisassociateUser",
                disassociate(&h, &username, &row.instance_id).await,
            ),
        ] {
            refused_closed(&reply, &format!("{operation}: {}", row.case));
        }

        // The account acquires exactly that instance, in a zone of the Region the requests
        // are signed for.
        {
            let store = h.engine.state::<ServiceState>();
            store
                .lock()
                .register_account_instance(row.instance_id.clone(), format!("{REGION}a"));
        }

        // And the very same requests are served: the gate reads the fleet as it stands, so
        // nothing of the earlier refusal is remembered — on any of the three.
        let case = format!("AssociateUser: {}, now held", row.case);
        let reply = associate(&h, &username, &row.instance_id).await;
        let body = reply.ok(&case);
        let summary = body
            .get("InstanceUserSummary")
            .unwrap_or_else(|| panic!("case {case:?}: no InstanceUserSummary in {body:?}"));
        assert_eq!(
            as_str(summary, "InstanceId", &case),
            row.instance_id,
            "case {case:?}: expected the instance the request named, got {summary:?}"
        );

        let case = format!("ListUserAssociations: {}, now held", row.case);
        let listed = list_associations(&h, &row.instance_id).await;
        let body = listed.ok(&case);
        let summaries = body
            .get("InstanceUserSummaries")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no InstanceUserSummaries in {body:?}"));
        assert!(
            summaries.iter().any(|entry| entry
                .get("Username")
                .and_then(serde_json::Value::as_str)
                == Some(username.as_str())),
            "case {case:?}: expected {username} among {summaries:?}"
        );

        let case = format!("DisassociateUser: {}, now held", row.case);
        let ended = disassociate(&h, &username, &row.instance_id).await;
        let body = ended.ok(&case);
        let summary = body
            .get("InstanceUserSummary")
            .unwrap_or_else(|| panic!("case {case:?}: no InstanceUserSummary in {body:?}"));
        assert_eq!(
            as_str(summary, "Status", &case),
            STATUS_DISASSOCIATING,
            "case {case:?}: expected {STATUS_DISASSOCIATING}, got {summary:?}"
        );
    }
}

/// RULE, the other side of the same gate: a request that names NO instance has nothing to
/// authorize, so the gate stops it nowhere — `DisassociateUser` addressed by
/// `InstanceUserArn` alone ends the association that ARN names, whatever instance it is on.
///
/// The rows differ in where the instance the association was made on came from: this
/// service's own fleet seam, and one the account acquired at runtime.
#[tokio::test]
async fn a_request_that_names_no_instance_authorizes_nothing() {
    let h = harness();
    register_provider(&h).await;

    let seeded = seed_account_instances();
    let acquired = "i-0cc33dd44ee55ff66";
    {
        let store = h.engine.state::<ServiceState>();
        store
            .lock()
            .register_account_instance(acquired, format!("{REGION}b"));
    }
    let rows = [
        ("an instance of this service's own fleet seam", seeded[1].0.clone()),
        ("an instance the account acquired at runtime", acquired.to_string()),
    ];

    for (index, (case, instance_id)) in rows.iter().enumerate() {
        let username = format!("arn-addressed{index}");

        // The association the ARN-only request ends, read back off the call that minted it.
        let reply = associate(&h, &username, instance_id).await;
        let body = reply.ok(case);
        let summary = body
            .get("InstanceUserSummary")
            .unwrap_or_else(|| panic!("case {case:?}: no InstanceUserSummary in {body:?}"));
        let arn = as_str(summary, "InstanceUserArn", case).to_string();

        // The body names no `InstanceId`, so the gate authorizes nothing and the request is
        // decided by the association the ARN resolves.
        let ended = disassociate_by_arn(&h, &arn).await;
        let body = ended.ok(case);
        let summary = body
            .get("InstanceUserSummary")
            .unwrap_or_else(|| panic!("case {case:?}: no InstanceUserSummary in {body:?}"));
        assert_eq!(
            as_str(summary, "InstanceUserArn", case),
            arn,
            "case {case:?}: expected the association the ARN named, got {summary:?}"
        );
        assert_eq!(
            as_str(summary, "Status", case),
            STATUS_DISASSOCIATING,
            "case {case:?}: expected {STATUS_DISASSOCIATING}, got {summary:?}"
        );
    }
}
