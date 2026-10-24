// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DELETION lifecycle of an RDS license server endpoint,
//! driven over the real wire path: every call goes through the router's `dispatch`, so
//! the request is deserialized, constraint-checked, handled and serialized exactly as a
//! client's would be.
//!
//! The RULE under test, asserted over a TABLE of inputs rather than one recorded
//! request: deleting a license server endpoint is ASYNCHRONOUS — which is what
//! `LicenseServerEndpointProvisioningStatus`
//! (`smithy_query(query="shape", shape="LicenseServerEndpointProvisioningStatus")`:
//! PROVISIONING | PROVISIONING_FAILED | PROVISIONED | DELETING | DELETION_FAILED |
//! DELETED) models `DELETING` and `DELETED` for — so the endpoint stays REPORTED in
//! `DELETING` after the call returns and reaches the terminal `DELETED` when the clock
//! owes it, instead of vanishing between two reads.
//!
//! What each suite below pins:
//!
//! * the delete answer and every later read report the same stored status, from
//!   `DELETING` through to the terminal `DELETED`, which is stable across further reads;
//! * the status the endpoint was in when the deletion was accepted (`PROVISIONING` or
//!   `PROVISIONED`) does not change that lifecycle;
//! * `DeleteLicenseServerEndpoint` is `@idempotent`: a repeat delete reports the status
//!   reached and restarts nothing;
//! * a deletion frees the `(IdentityProviderArn, ServerType)` pair at once, so a create
//!   for the same pair mints a NEW endpoint while the deleted one is still reported;
//! * an endpoint being deleted is no longer a taggable resource;
//! * deregistering the identity provider deletes its endpoints the same way, because how
//!   a deletion becomes observable does not depend on what asked for it.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::scaffolded::{
    BAD_REQUEST_EXCEPTION_CODE, INVALID_INPUT_RESOURCE_ARN_MESSAGE,
};

// ONE wire harness per service: the dispatch plumbing, the signing identity and the
// provider-registration helpers come from the sibling suites rather than a second copy.
use super::identity_provider_subscriptions::{
    as_str, call, encoded_label, harness, send, Harness, Reply,
};
use super::license_server_endpoints::{
    create_endpoint, list_endpoints, listed_arns, seed, ENDPOINTS, SERVER_TYPE,
};

/// How long a read waits before it must see a transition the clock owed.
///
/// A clock-owed transition in this emulator falls due inside 250-750 ms of the call that
/// started it, so a read this far later must observe it whatever span the service chose
/// inside that window. The same wait the provisioning suite uses.
const PAST_THE_OWED_TRANSITION: Duration = Duration::from_millis(900);

async fn delete_endpoint(h: &Harness, arn: &str) -> Reply {
    // `DeleteLicenseServerEndpoint` is bound to POST
    // /license-server/DeleteLicenseServerEndpoint (smithy_query: http.uri).
    call(
        h,
        "/license-server/DeleteLicenseServerEndpoint",
        &json!({ "LicenseServerEndpointArn": arn, "ServerType": SERVER_TYPE }),
    )
    .await
}

async fn deregister(h: &Harness, body: &Value) -> Reply {
    call(h, "/identity-provider/DeregisterIdentityProvider", body).await
}

/// `ListTagsForResource` binds `ResourceArn` to ONE path segment (`GET
/// /tags/{ResourceArn}`), so the `:` and `/` inside an ARN are percent-encoded with the
/// shared fixture's encoder.
async fn list_tags(h: &Harness, arn: &str) -> Reply {
    send(h, "GET", &format!("/tags/{}", encoded_label(arn)), None).await
}

async fn tag(h: &Harness, arn: &str, tags: &Value) -> Reply {
    send(
        h,
        "PUT",
        &format!("/tags/{}", encoded_label(arn)),
        Some(&json!({ "Tags": tags })),
    )
    .await
}

/// The endpoint the list reports for `arn`, or `None` when the list does not report it.
fn listed_endpoint(body: &Value, arn: &str) -> Option<Value> {
    body.get("LicenseServerEndpoints")
        .and_then(Value::as_array)?
        .iter()
        .find(|endpoint| {
            endpoint
                .get("LicenseServerEndpointArn")
                .and_then(Value::as_str)
                == Some(arn)
        })
        .cloned()
}

/// Assert `ListLicenseServerEndpoints` reports `arn` with `expected_status`, and that an
/// endpoint on its way out reports no network address.
async fn assert_listed_with_status(h: &Harness, case: &str, arn: &str, expected_status: &str) {
    let reply = list_endpoints(h, &json!({})).await;
    let body = reply.ok(case);
    let listed = listed_endpoint(body, arn).unwrap_or_else(|| {
        panic!("case {case:?}: the list must still report {arn:?}, got {body:?}")
    });
    assert_eq!(
        as_str(&listed, "LicenseServerEndpointProvisioningStatus", case),
        expected_status,
        "case {case:?}: expected the endpoint to be reported as {expected_status}, got {listed:?}"
    );
    // There is no address to connect to on an endpoint that is on its way out, exactly as
    // there is none on one still being provisioned.
    if expected_status == "DELETING" || expected_status == "DELETED" {
        assert!(
            listed.get("ServerEndpoint").is_none_or(Value::is_null),
            "case {case:?}: an endpoint being deleted has no address to reach, got {listed:?}"
        );
    }
}

/// The status the delete answer reports for the endpoint it accepted the deletion of.
fn answered_status(reply: &Reply, case: &str) -> String {
    let endpoint = &reply.ok(case)["LicenseServerEndpoint"];
    as_str(endpoint, "LicenseServerEndpointProvisioningStatus", case).to_string()
}

/// RULE: a deletion accepted while the endpoint is still `PROVISIONING` answers
/// `DELETING`, keeps the endpoint reported in `DELETING`, and reaches the terminal
/// `DELETED` once the clock owes it — which is where it stays.
#[tokio::test]
async fn a_deletion_is_observable_in_deleting_and_then_in_deleted() {
    let h = harness();
    let created = seed(&h).await;

    for (case, _, endpoint_arn) in &created {
        let reply = delete_endpoint(&h, endpoint_arn).await;
        assert_eq!(
            answered_status(&reply, case),
            "DELETING",
            "case {case:?}: the delete answers the status the deletion put the endpoint in, \
             got {:?}",
            reply.body
        );
        // A client reading straight after the call still sees the transient status.
        assert_listed_with_status(&h, case, endpoint_arn, "DELETING").await;
    }

    // Every deleted endpoint is still reported, in creation order: nothing vanished
    // between two reads.
    let reply = list_endpoints(&h, &json!({})).await;
    let expected: Vec<String> = created
        .iter()
        .map(|(_, _, endpoint_arn)| endpoint_arn.clone())
        .collect();
    assert_eq!(
        listed_arns(reply.ok("every deleting endpoint"), "every deleting endpoint"),
        expected,
        "a deleting endpoint stays in the list, in creation order, got {:?}",
        reply.body
    );

    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

    for (case, _, endpoint_arn) in &created {
        let case = format!("{case} (past the owed transition)");
        assert_listed_with_status(&h, &case, endpoint_arn, "DELETED").await;
        // `DELETED` is terminal: a further read reports the same thing, and an
        // `@idempotent` repeat delete reports it without restarting anything.
        assert_listed_with_status(&h, &format!("{case}, read again"), endpoint_arn, "DELETED")
            .await;
        let again = delete_endpoint(&h, endpoint_arn).await;
        assert_eq!(
            answered_status(&again, &case),
            "DELETED",
            "case {case:?}: a repeat delete must not move the terminal status back, got {:?}",
            again.body
        );
        assert_listed_with_status(&h, &format!("{case}, after a repeat delete"), endpoint_arn, "DELETED")
            .await;
    }
}

/// RULE: the same lifecycle holds for an endpoint deleted after it finished provisioning
/// — what the deletion does is decided by the deletion, not by the status the endpoint
/// was in when it was asked for.
#[tokio::test]
async fn an_endpoint_deleted_after_it_was_provisioned_follows_the_same_lifecycle() {
    let h = harness();
    let created = seed(&h).await;

    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    for (case, _, endpoint_arn) in &created {
        // The precondition: these endpoints finished provisioning before the delete.
        assert_listed_with_status(&h, &format!("{case} (before the delete)"), endpoint_arn, "PROVISIONED")
            .await;
    }

    for (case, _, endpoint_arn) in &created {
        let reply = delete_endpoint(&h, endpoint_arn).await;
        assert_eq!(
            answered_status(&reply, case),
            "DELETING",
            "case {case:?}: deleting a provisioned endpoint answers DELETING, got {:?}",
            reply.body
        );
        assert_listed_with_status(&h, case, endpoint_arn, "DELETING").await;
    }

    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    for (case, _, endpoint_arn) in &created {
        assert_listed_with_status(
            &h,
            &format!("{case} (past the owed transition)"),
            endpoint_arn,
            "DELETED",
        )
        .await;
    }
}

/// RULE: a deletion frees the `(IdentityProviderArn, ServerType)` pair immediately, so a
/// create for the same pair mints a NEW endpoint — while the deleted one goes on being
/// reported in the status it reached.
#[tokio::test]
async fn a_create_after_a_deletion_mints_a_new_endpoint() {
    let h = harness();
    let created = seed(&h).await;

    for (row, (case, provider_arn, endpoint_arn)) in ENDPOINTS.iter().zip(&created) {
        delete_endpoint(&h, endpoint_arn).await.ok(case);

        let again = create_endpoint(&h, &row.request(provider_arn)).await;
        let body = again.ok(case);
        let fresh_arn = as_str(body, "LicenseServerEndpointArn", case).to_string();
        assert_ne!(
            &fresh_arn, endpoint_arn,
            "case {case:?}: a create after a deletion must mint a new endpoint, got {body:?}"
        );
        // The new endpoint starts its own lifecycle, and the deleted one keeps reporting
        // the status it reached rather than being resurrected.
        assert_listed_with_status(&h, &format!("{case} (the new endpoint)"), &fresh_arn, "PROVISIONING")
            .await;
        assert_listed_with_status(&h, &format!("{case} (the deleted endpoint)"), endpoint_arn, "DELETING")
            .await;
    }
}

/// RULE: an endpoint whose deletion was accepted is no longer a resource the tagging
/// surface resolves — it is reported so a client can watch it go, not so it can be
/// changed. The tagging surface refuses an ARN it does not resolve with
/// `BadRequestException` / "Invalid input resource arn".
#[tokio::test]
async fn an_endpoint_being_deleted_is_no_longer_taggable() {
    let h = harness();
    let created = seed(&h).await;

    for (row, (case, _, endpoint_arn)) in ENDPOINTS.iter().zip(&created) {
        // The precondition: a live endpoint resolves, reporting whatever tags its create
        // supplied (an untagged row reports an empty map).
        let before = list_tags(&h, endpoint_arn).await;
        let tags = before.ok(case)["Tags"].clone();
        assert_eq!(
            tags.as_object().is_some_and(|tags| !tags.is_empty()),
            row.tagged,
            "case {case:?}: a live endpoint reports the tags its create stored, got {:?}",
            before.body
        );

        delete_endpoint(&h, endpoint_arn).await.ok(case);

        for (surface, reply) in [
            ("ListTagsForResource", list_tags(&h, endpoint_arn).await),
            (
                "TagResource",
                tag(&h, endpoint_arn, &json!({ "team": "licensing" })).await,
            ),
        ] {
            let case = format!("{case}/{surface} (after the deletion)");
            assert_eq!(
                reply.status, 400,
                "case {case:?}: expected HTTP 400, got {reply:?}"
            );
            assert_eq!(
                reply.error_type.as_deref(),
                Some(BAD_REQUEST_EXCEPTION_CODE),
                "case {case:?}: expected {BAD_REQUEST_EXCEPTION_CODE}, got {reply:?}"
            );
            assert_eq!(
                reply.message(),
                Some(INVALID_INPUT_RESOURCE_ARN_MESSAGE),
                "case {case:?}: expected the tagging surface's fixed sentence, got {reply:?}"
            );
        }

        // The endpoint is still REPORTED, which is the whole point of the two statuses.
        assert_listed_with_status(&h, case, endpoint_arn, "DELETING").await;
    }
}

/// RULE: the endpoints of a deregistered identity provider are deleted the same
/// asynchronous way — how a deletion becomes observable does not depend on what asked
/// for it.
#[tokio::test]
async fn deregistering_a_provider_deletes_its_endpoints_the_same_way() {
    let h = harness();
    let created = seed(&h).await;

    for (case, provider_arn, endpoint_arn) in &created {
        let reply = deregister(&h, &json!({ "IdentityProviderArn": provider_arn })).await;
        reply.ok(case);
        assert_listed_with_status(&h, case, endpoint_arn, "DELETING").await;
    }

    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    for (case, _, endpoint_arn) in &created {
        assert_listed_with_status(
            &h,
            &format!("{case} (past the owed transition)"),
            endpoint_arn,
            "DELETED",
        )
        .await;
    }
}
