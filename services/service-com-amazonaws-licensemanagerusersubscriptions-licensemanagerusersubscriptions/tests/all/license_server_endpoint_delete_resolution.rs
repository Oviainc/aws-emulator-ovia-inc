// SPDX-License-Identifier: CC0-1.0

//! How `DeleteLicenseServerEndpoint` resolves the ONE resource it addresses, and what it
//! answers when that resource cannot be resolved — driven over the real wire path, so
//! every request is deserialized, constraint-checked, handled and serialized exactly as a
//! client's would be.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! * **The refusal of an unresolvable `LicenseServerEndpointArn` is this service's
//!   recorded authorization refusal**, not a not-found: whatever class the ARN belongs to
//!   (never created, another account, Region or partition, another resource kind, a near
//!   miss on a live one, or an endpoint the store no longer holds), the answer is HTTP 400
//!   with the `UnauthorizedException` code and its fixed sentence, discloses nothing about
//!   the ARN, and changes nothing in the store. The error set a model declares for an
//!   operation is the union of what that operation may return, not a mapping from a
//!   condition to a code: the same `ResourceNotFoundException` is declared for
//!   `CreateLicenseServerEndpoint`, whose unresolvable `IdentityProviderArn` is answered
//!   with `ValidationException` / "Identity provider is not valid" — so neither
//!   license-server operation reports an identifier that resolves to nothing as the
//!   modelled 404.
//! * **A `ServerType` outside the value set the model declares is refused at the request
//!   boundary**, before the resolution runs: HTTP 400 `ValidationException` naming the
//!   member and the admitted set, identically for an ARN that resolves and one that does
//!   not, and the endpoint it named is left exactly as it was. The one value the model
//!   admits goes on to the resolution and deletes the endpoint.

use serde_json::json;

use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState;

// ONE wire harness per service: the dispatch plumbing and the endpoint fixtures come from
// the sibling suites rather than a second copy.
use super::identity_provider_subscriptions::{
    arn_prefix, as_str, call, harness, Harness, Reply, ACCOUNT, REGION, SIGNING_NAME,
};
use super::license_server_endpoint_deletion::assert_listed_with_status;
use super::license_server_endpoints::{list_endpoints, listed_arns, seed, SERVER_TYPE};
use super::resource_resolution_refusals::assert_fails_closed;

/// `ServerType` values the model does NOT admit.
///
/// The admitted set is exactly `[RDS_SAL]` (`smithy_query(query="shape",
/// shape="ServerType")`), spelled out in [`SERVER_TYPE`] rather than read from the
/// production constant. The rows vary along the ways a value can miss a published
/// enum: another server family, the wrong case, trailing whitespace, a different
/// separator, and the empty string.
const OFF_LIST_SERVER_TYPES: &[(&str, &str)] = &[
    ("another server family", "SQL_SERVER"),
    ("the admitted value in lower case", "rds_sal"),
    ("the admitted value with trailing whitespace", "RDS_SAL "),
    ("the admitted value with another separator", "RDS-SAL"),
    ("the empty string", ""),
];

async fn delete_endpoint(h: &Harness, arn: &str, server_type: &str) -> Reply {
    // `DeleteLicenseServerEndpoint` is bound to POST
    // /license-server/DeleteLicenseServerEndpoint (smithy_query: http.uri).
    call(
        h,
        "/license-server/DeleteLicenseServerEndpoint",
        &json!({ "LicenseServerEndpointArn": arn, "ServerType": server_type }),
    )
    .await
}

/// The modelled `ResourceNotFoundException` is declared at HTTP 404
/// (`smithy_query(query="shape", shape="ResourceNotFoundException")`), so this is what
/// "not answered as a not-found" means on the wire.
fn assert_not_reported_as_not_found(case: &str, reply: &Reply) {
    assert_ne!(
        reply.status, 404,
        "case {case:?}: an unresolvable endpoint ARN is not reported as a not-found, got {reply:?}"
    );
    assert_ne!(
        reply.error_type.as_deref(),
        Some("ResourceNotFoundException"),
        "case {case:?}: an unresolvable endpoint ARN is not reported as a not-found, got {reply:?}"
    );
}

/// Drop `arn` from the store altogether, so the ARN names an endpoint this service once
/// held and holds no longer.
///
/// What normally drops a deleted endpoint is the retention sweep
/// (`MAX_DELETED_ENDPOINTS_RETAINED`), which a test would have to create a thousand
/// endpoints to reach, so the row is prepared through the same state seam the suites seed
/// the account's directory fleet through.
fn forget_endpoint(h: &Harness, arn: &str) {
    let store = h.engine.state::<ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState>();
    let removed = store.lock().remove_license_server_endpoint(arn);
    assert!(
        removed.is_some(),
        "the row has to start from an endpoint the store held, got nothing for {arn:?}"
    );
}

/// RULE: a `LicenseServerEndpointArn` that resolves to no endpoint this service holds is
/// refused on the authorization gate — HTTP 400, the `UnauthorizedException` code, the
/// fixed sentence, nothing from the request disclosed — and never as the modelled 404,
/// whatever class of unresolvable ARN it is; and the refusal changes nothing.
#[tokio::test]
async fn an_endpoint_arn_that_resolves_to_nothing_fails_closed_rather_than_not_found() {
    let h = harness();
    let created = seed(&h).await;
    let live = created[0].2.clone();
    // The last row's endpoint is taken out of the store, so one case names an endpoint
    // this service really did mint.
    let forgotten = created[created.len() - 1].2.clone();
    forget_endpoint(&h, &forgotten);

    let near_miss = {
        let mut arn = live.clone();
        arn.pop();
        format!("{arn}0")
    };
    let endpoint_prefix = arn_prefix("license-server-endpoint");
    let uppercase_id = live
        .rsplit('/')
        .next()
        .expect("a minted ARN carries a resource id")
        .to_ascii_uppercase();

    let rows: Vec<(String, String)> = vec![
        (
            "an endpoint ARN of the form this service mints, never created".to_string(),
            format!("{endpoint_prefix}lse-000000009999"),
        ),
        (
            "an endpoint ARN in another account".to_string(),
            format!(
                "arn:aws:{SIGNING_NAME}:{REGION}:111111111111:license-server-endpoint/lse-000000000001"
            ),
        ),
        (
            "an endpoint ARN in another Region".to_string(),
            format!(
                "arn:aws:{SIGNING_NAME}:eu-west-1:{ACCOUNT}:license-server-endpoint/lse-000000000001"
            ),
        ),
        (
            "an endpoint ARN in another partition".to_string(),
            format!(
                "arn:aws-us-gov:{SIGNING_NAME}:us-gov-west-1:{ACCOUNT}:license-server-endpoint/lse-000000000001"
            ),
        ),
        (
            "an ARN of another resource kind this service mints".to_string(),
            format!("{}ip-000000009999", arn_prefix("identity-provider")),
        ),
        ("a near miss on a live endpoint ARN".to_string(), near_miss),
        (
            "an uppercase spelling of a live endpoint's id".to_string(),
            format!("{endpoint_prefix}{uppercase_id}"),
        ),
        (
            "an endpoint the store no longer holds".to_string(),
            forgotten.clone(),
        ),
    ];

    for (class, arn) in &rows {
        let case = format!("DeleteLicenseServerEndpoint/{class}");
        let reply = delete_endpoint(&h, arn, SERVER_TYPE).await;
        assert_fails_closed(&case, arn, &reply);
        assert_not_reported_as_not_found(&case, &reply);
    }

    // A refused delete deletes nothing: the endpoints the seeding created are still
    // reported, in creation order, minus only the one taken out of the store directly.
    let reply = list_endpoints(&h, &json!({})).await;
    let expected: Vec<String> = created
        .iter()
        .map(|(_, _, endpoint_arn)| endpoint_arn.clone())
        .filter(|endpoint_arn| endpoint_arn != &forgotten)
        .collect();
    assert_eq!(
        listed_arns(reply.ok("after every refused delete"), "after every refused delete"),
        expected,
        "a refused delete must leave the store as it was, got {:?}",
        reply.body
    );
    for (case, _, endpoint_arn) in &created {
        if endpoint_arn == &forgotten {
            continue;
        }
        assert_listed_with_status(
            &h,
            &format!("{case} (after every refused delete)"),
            endpoint_arn,
            "PROVISIONING",
        )
        .await;
    }
}

/// RULE: a `ServerType` outside the set the model declares is refused at the request
/// boundary with the modelled `ValidationException` naming the member and the admitted
/// set — the same answer whether the ARN resolves or not, because the boundary runs before
/// the resolution — and the endpoint the request named is left exactly as it was. The one
/// admitted value reaches the resolution and deletes the endpoint.
#[tokio::test]
async fn a_server_type_outside_the_modelled_set_is_refused_before_the_resolution() {
    let h = harness();
    let created = seed(&h).await;
    let live = created[0].2.clone();
    let unresolvable = format!("{}lse-000000009999", arn_prefix("license-server-endpoint"));

    for (class, server_type) in OFF_LIST_SERVER_TYPES {
        for (which, arn) in [("a live endpoint", &live), ("an unknown ARN", &unresolvable)] {
            let case = format!("ServerType/{class}/{which}");
            let reply = delete_endpoint(&h, arn, server_type).await;
            assert_eq!(
                reply.status, 400,
                "case {case:?}: ValidationException is modelled at HTTP 400, got {reply:?}"
            );
            assert_eq!(
                reply.error_type.as_deref(),
                Some("ValidationException"),
                "case {case:?}: expected ValidationException, got {reply:?}"
            );
            let message = reply.message().unwrap_or_default();
            assert!(
                message.contains("serverType"),
                "case {case:?}: the refusal must name the member it is about, got {reply:?}"
            );
            assert!(
                message.contains(SERVER_TYPE),
                "case {case:?}: the refusal must name the admitted value set, got {reply:?}"
            );
        }
        // The refused request named a live endpoint: it is still there, in the status it
        // was in, so nothing was deleted on the way to the refusal.
        assert_listed_with_status(
            &h,
            &format!("ServerType/{class} (the endpoint it named)"),
            &live,
            "PROVISIONING",
        )
        .await;
    }

    // The control: the one value the model admits reaches the resolution and the deletion
    // goes through, on the same path every row above was refused on.
    let reply = delete_endpoint(&h, &live, SERVER_TYPE).await;
    let body = reply.ok("the admitted ServerType");
    assert_eq!(
        as_str(
            &body["LicenseServerEndpoint"],
            "LicenseServerEndpointProvisioningStatus",
            "the admitted ServerType"
        ),
        "DELETING",
        "the admitted ServerType must delete the endpoint it named, got {body:?}"
    );
    assert_listed_with_status(&h, "the admitted ServerType", &live, "DELETING").await;
}
