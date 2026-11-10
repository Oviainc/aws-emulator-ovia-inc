// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the WORDING of this service's two refusal sentences that a client
//! parses: the authorization denial and the quota denial.
//!
//! The rules under test:
//!
//! * an authorization refusal states the resource the action was judged against in an
//!   `on resource: <arn>` clause — the ARN of the resource the request addresses, or the
//!   wildcard ARN of the collection a create or a List acts on — and only an action bound
//!   to no resource type at all states no clause;
//! * a quota refusal states the resource type whose per-account quota was reached and the
//!   account it belongs to, and NOT the numeric limit, which travels in the exception's
//!   `serviceCode`/`quotaCode` members because a quota is adjustable per account.
//!
//! Both are driven over the wire (awsJson1_0, `x-amz-target: B2BI.<Operation>`) through the
//! same router the emulator boots, and the authorization rows speak for the assumed-role
//! session the IAM and STS stores of the shared engine hold — the harness
//! [`crate::authorization::Emulator`] seeds.

use serde_json::{json, Value};

use crate::authorization::{Emulator, ROLE, SESSION_KEY_ID, SESSION_NAME};
use crate::onboarding_loop::{
    b2bi_router, call, call_as_signed_for, error_code, given_profile, given_transformer, text,
    ACCOUNT,
};

/// The identity ARN the seeded session speaks for, as `sts:GetCallerIdentity` reports it —
/// the `User:` a refusal names.
fn session_arn() -> String {
    format!("arn:aws:sts::{ACCOUNT}:assumed-role/{ROLE}/{SESSION_NAME}")
}

/// The sentence IAM states an authorization failure with: who, which action, which
/// resource (nothing where the action is bound to no resource type) and which rule refused
/// it.
fn expected_refusal(operation: &str, resource: Option<&str>, reason: &str) -> String {
    let on_resource = match resource {
        Some(arn) => format!(" on resource: {arn}"),
        None => String::new(),
    };
    format!(
        "User: {} is not authorized to perform: b2bi:{operation}{on_resource} {reason}",
        session_arn()
    )
}

/// One row of the authorization table: the case, the operation, a request body satisfying
/// its modelled required members, and the resource type and id the refusal must name
/// (`None` for an action bound to no resource type).
type DenialRow = (String, &'static str, Value, Option<(String, String)>);

/// RULE: an authorization refusal names the resource the action was judged against — the
/// ARN of the resource the request addresses, the wildcard ARN of the collection a create
/// or a List acts on (`arn:aws:b2bi:<region>:<account>:<type>/*`, since neither names one
/// member of it), and nothing at all only for an action bound to no resource type — and it
/// states which of IAM's two rules refused the request.
///
/// The rows differ along what the rule generalises over: the four resource types this
/// service owns, both shapes a request takes (addressing one resource, and addressing a
/// whole collection), the action with no resource type, the Region the request is signed
/// for (the clause carries the request's own Region, not a stored ARN's) and both of IAM's
/// refusals (an explicit `Deny`, and the default deny of a principal no policy speaks for).
#[tokio::test]
async fn an_authorization_refusal_names_the_resource_the_action_targets() {
    let emulator = Emulator::new();
    emulator.given_session();

    // Two resources to address by id, created as the ambient identity (which no role
    // policy binds), so a row can assert the instance form of the clause.
    let profile_id = given_profile(&emulator.router, "wording").await;
    let transformer_id = given_transformer(&emulator.router, "wording").await;

    let x12 = json!({ "transactionSet": "X12_850", "version": "VERSION_4010" });
    let collection = |kind: &str| Some((kind.to_string(), "*".to_string()));
    let rows: Vec<DenialRow> = vec![
        (
            "a create, which names no profile until it mints one".to_string(),
            "CreateProfile",
            json!({
                "name": "denied",
                "phone": "555-555-0101",
                "businessName": "Denied Corp",
                "logging": "DISABLED"
            }),
            collection("profile"),
        ),
        (
            "a list of every profile the account holds".to_string(),
            "ListProfiles",
            json!({}),
            collection("profile"),
        ),
        (
            "a read of one profile".to_string(),
            "GetProfile",
            json!({ "profileId": profile_id.clone() }),
            Some(("profile".to_string(), profile_id.clone())),
        ),
        (
            "a delete of one profile".to_string(),
            "DeleteProfile",
            json!({ "profileId": profile_id.clone() }),
            Some(("profile".to_string(), profile_id.clone())),
        ),
        (
            "a create of a capability".to_string(),
            "CreateCapability",
            json!({
                "name": "denied",
                "type": "edi",
                "configuration": {
                    "edi": {
                        "type": { "x12Details": x12.clone() },
                        "inputLocation": { "bucketName": "inbound-bucket", "key": "in/" },
                        "outputLocation": { "bucketName": "outbound-bucket", "key": "out/" },
                        "transformerId": transformer_id.clone()
                    }
                }
            }),
            collection("capability"),
        ),
        (
            "a list of capabilities".to_string(),
            "ListCapabilities",
            json!({}),
            collection("capability"),
        ),
        (
            "a create of a partnership".to_string(),
            "CreatePartnership",
            json!({
                "profileId": profile_id.clone(),
                "name": "denied",
                "email": "partner@example.com",
                "capabilities": []
            }),
            collection("partnership"),
        ),
        (
            "a list of partnerships".to_string(),
            "ListPartnerships",
            json!({}),
            collection("partnership"),
        ),
        (
            "a create of a transformer".to_string(),
            "CreateTransformer",
            json!({
                "name": "denied",
                "inputConversion": {
                    "fromFormat": "X12",
                    "formatOptions": { "x12": x12.clone() }
                }
            }),
            collection("transformer"),
        ),
        (
            "a list of transformers".to_string(),
            "ListTransformers",
            json!({}),
            collection("transformer"),
        ),
        (
            "a read of one transformer".to_string(),
            "GetTransformer",
            json!({ "transformerId": transformer_id.clone() }),
            Some(("transformer".to_string(), transformer_id.clone())),
        ),
        (
            "a mapping dry run, which is bound to no resource type".to_string(),
            "TestMapping",
            json!({
                "inputFileContent": "{\"total\":1}",
                "mappingTemplate": "$",
                "fileFormat": "JSON"
            }),
            None,
        ),
    ];

    // The clause carries the Region the request was signed for, so the table runs in two:
    // the suite's usual one and another, where a Region read off anything but the request
    // would show up.
    for region in ["us-west-2", "eu-central-1"] {
        for (case, operation, body, scope) in &rows {
            let resource = scope
                .as_ref()
                .map(|(kind, id)| format!("arn:aws:b2bi:{region}:{ACCOUNT}:{kind}/{id}"));

            // IAM's two refusals: an explicit `Deny` beside an `Allow` of everything, and
            // a principal whose role carries no policy at all.
            let refusals = [
                (
                    "an explicit Deny",
                    Some(json!({
                        "Statement": [
                            { "Effect": "Allow", "Action": "b2bi:*", "Resource": "*" },
                            {
                                "Effect": "Deny",
                                "Action": format!("b2bi:{operation}"),
                                "Resource": "*"
                            }
                        ]
                    })),
                    "with an explicit deny in an identity-based policy".to_string(),
                ),
                (
                    "no policy at all",
                    None,
                    format!("because no identity-based policy allows the b2bi:{operation} action"),
                ),
            ];

            for (refusal, policy, reason) in refusals {
                match &policy {
                    Some(document) => emulator.given_role_policy(document),
                    None => emulator.given_no_role_policy(),
                }
                let case = format!("{case} in {region}, refused by {refusal}");
                let (status, response) = call_as_signed_for(
                    &emulator.router,
                    SESSION_KEY_ID,
                    region,
                    operation,
                    body.clone(),
                )
                .await;
                assert_eq!(
                    status, 403,
                    "case {case:?}: expected HTTP 403, got {status} {response}"
                );
                assert_eq!(
                    error_code(&response),
                    "AccessDeniedException",
                    "case {case:?}: expected an AccessDeniedException, got {response}"
                );
                let expected = expected_refusal(operation, resource.as_deref(), &reason);
                let message = text(&response, "message");
                assert_eq!(
                    message, expected,
                    "case {case:?}: expected message {expected:?}, got {message:?}"
                );
                // The modelled `ErrorMessage` bound this sentence has to fit in
                // (`smithy_query(query="shape",shape="AccessDeniedException")`: len 10..1024).
                assert!(
                    (10..=1024).contains(&message.len()),
                    "case {case:?}: the message must satisfy the modelled len 10..1024, got \
                     {} characters: {message:?}",
                    message.len()
                );
            }
        }
    }
}

/// RULE: a create refused by a per-account quota states the resource type whose quota was
/// reached and the account it belongs to, and never the numeric limit — a quota is
/// adjustable per account, so the value a caller can read travels in the `serviceCode` and
/// `quotaCode` members that name the quota rather than in the sentence.
///
/// The rows differ along what the rule generalises over: how the account reached its quota
/// (one create past it, and concurrent racers past the last free slot, so every losing
/// racer is checked too) and the `logging` value of the refused create, because a
/// logging-ENABLED create provisions a log group through another service before the quota
/// refusal can be answered.
#[tokio::test]
async fn a_quota_refusal_names_the_resource_type_and_the_account_not_the_limit() {
    let limit = service_com_amazonaws_b2bi_b2bi::state::MAX_PROFILES_PER_ACCOUNT;
    let expected = format!("Profile limit reached for AWS account {ACCOUNT}");

    for over_subscription in ["one create past the quota", "three concurrent racers"] {
        for logging in ["DISABLED", "ENABLED"] {
            let case = format!("{over_subscription}, logging={logging}");
            let router = b2bi_router();
            for index in 0..limit {
                let (status, body) = call(
                    &router,
                    "CreateProfile",
                    json!({
                        "name": format!("filler-{index}"),
                        "phone": "555-555-0100",
                        "businessName": "Acme Corp",
                        "logging": "DISABLED"
                    }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: profile #{index} of {limit} must be accepted, got \
                     {status} {body}"
                );
            }

            let refused = |suffix: usize| {
                call(
                    &router,
                    "CreateProfile",
                    json!({
                        "name": format!("over-quota-{suffix}"),
                        "phone": "555-555-0100",
                        "businessName": "Acme Corp",
                        "logging": logging
                    }),
                )
            };
            let answers = if over_subscription == "three concurrent racers" {
                let (a, b, c) = tokio::join!(refused(0), refused(1), refused(2));
                vec![a, b, c]
            } else {
                vec![refused(0).await]
            };

            for (status, body) in &answers {
                assert_eq!(
                    *status, 402,
                    "case {case:?}: a create past the quota must be refused, got {status} {body}"
                );
                assert_eq!(
                    error_code(body),
                    "ServiceQuotaExceededException",
                    "case {case:?}: expected a ServiceQuotaExceededException, got {body}"
                );
                let message = text(body, "message");
                assert_eq!(
                    message, expected,
                    "case {case:?}: expected message {expected:?}, got {message:?}"
                );
                assert!(
                    !message.contains(&limit.to_string()),
                    "case {case:?}: the numeric limit belongs in the quota members, not the \
                     sentence, got {message:?}"
                );
                // The quota the sentence leaves the value to, as Service Quotas publishes
                // it for this service.
                assert_eq!(
                    (
                        text(body, "serviceCode"),
                        text(body, "quotaCode"),
                        text(body, "resourceType"),
                        text(body, "resourceId")
                    ),
                    (
                        "b2bi".to_string(),
                        "ProfilesPerAccount".to_string(),
                        "profile".to_string(),
                        ACCOUNT.to_string()
                    ),
                    "case {case:?}: the refusal must name the quota it reached, got {body}"
                );
            }

            // Nothing was created by a refused call: the account still holds exactly its
            // quota.
            let (_, listed) = call(&router, "ListProfiles", json!({})).await;
            assert_eq!(
                listed["profiles"].as_array().map(Vec::len).unwrap_or_default(),
                limit,
                "case {case:?}: the account must still hold exactly {limit} profiles, got \
                 {listed}"
            );
        }
    }
}
