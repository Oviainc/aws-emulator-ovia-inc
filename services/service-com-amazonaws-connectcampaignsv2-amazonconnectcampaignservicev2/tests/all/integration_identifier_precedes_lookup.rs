// SPDX-License-Identifier: CC0-1.0

//! The ARN an `IntegrationIdentifier` carries is judged BEFORE anything is looked up,
//! end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves, so
//! each assertion exercises the restJson1 wire protocol, the modelled constraints and
//! the handlers together. The rule under test, over the identifier union of
//! `DeleteConnectInstanceIntegration`:
//!
//! * a value outside the `@length`/`@pattern` of the shape the identifier's ARN member
//!   targets is a BAD REQUEST — HTTP 400 `ValidationException` naming that member — and
//!   it is that whatever state the instance named beside it is in, because a value the
//!   shape forbids is one no integration could have been stored under; a malformed
//!   input is therefore never reported as a missing resource;
//! * the converse, so the refusal is about the input and not the request: the same
//!   well-formed identifier is the modelled HTTP 404 `ResourceNotFoundException` when
//!   the instance or the record it names does not exist, and a success when it does.
//!
//! The bad values each row is built from, and the bounds they are outside of, live
//! once in [`super::union_member_arn_constraints`], which pins them to `smithy-model/`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, error_code, router};
use super::connect_instance_integrations::{list_integrations, put_integration, summaries};
use super::union_member_arn_constraints::{
    arns_outside_arns_bounds, arns_outside_lambda_arns_bounds, assert_refused, BadValue,
};

/// The region every case here signs for: one region holds every record a case reads,
/// and nothing in the rule under test is region-dependent.
const REGION: &str = "us-east-1";

/// How one row builds the identifier under test from an ARN.
type Build = fn(&str) -> Value;

/// One ARN-valued member of `IntegrationIdentifier`: the member, how an identifier
/// carrying it is built, the path a refusal names it by (relative to the operation's
/// own member), and the values the shape IT targets forbids — a Lambda function's ARN
/// targets a different shape from the other two members
/// (`smithy_query(query="shape", shape="IntegrationIdentifier")`).
type UnionMemberCases = (&'static str, Build, &'static str, Vec<BadValue>);

/// Every ARN-valued member of the identifier union, each crossed with the bad values
/// of the shape it targets.
fn identifier_members() -> Vec<UnionMemberCases> {
    vec![
        (
            "customerProfiles.domainArn",
            |arn| json!({"customerProfiles": {"domainArn": arn}}),
            "customerProfiles.domainArn",
            arns_outside_arns_bounds(),
        ),
        (
            "qConnect.knowledgeBaseArn",
            |arn| json!({"qConnect": {"knowledgeBaseArn": arn}}),
            "qConnect.knowledgeBaseArn",
            arns_outside_arns_bounds(),
        ),
        (
            "lambda.functionArn",
            |arn| json!({"lambda": {"functionArn": arn}}),
            "lambda.functionArn",
            arns_outside_lambda_arns_bounds(),
        ),
    ]
}

/// One well-formed integration per kind the union declares, with the identifier that
/// names it: what a case puts so that each refusal below is weighed against an
/// instance that really holds something, and what the positive control deletes.
fn well_formed() -> Vec<(&'static str, Value, Value)> {
    vec![
        (
            "a Customer Profiles domain",
            json!({"customerProfiles": {
                "domainArn": "arn:aws:profile:us-east-1:000000000000:domains/held-domain",
                "objectTypeNames": {},
            }}),
            json!({"customerProfiles": {
                "domainArn": "arn:aws:profile:us-east-1:000000000000:domains/held-domain",
            }}),
        ),
        (
            "a Q Connect knowledge base",
            json!({"qConnect": {
                "knowledgeBaseArn":
                    "arn:aws:wisdom:us-east-1:000000000000:knowledge-base/11111111-2222-3333-4444-555555555555",
            }}),
            json!({"qConnect": {
                "knowledgeBaseArn":
                    "arn:aws:wisdom:us-east-1:000000000000:knowledge-base/11111111-2222-3333-4444-555555555555",
            }}),
        ),
        (
            "a Lambda function",
            json!({"lambda": {
                "functionArn": "arn:aws:lambda:us-east-1:000000000000:function:held-handler",
            }}),
            json!({"lambda": {
                "functionArn": "arn:aws:lambda:us-east-1:000000000000:function:held-handler",
            }}),
        ),
    ]
}

/// `DeleteConnectInstanceIntegration` for `instance_id` with `identifier`.
async fn delete_integration(
    router: &MultiServiceRouter,
    instance_id: &str,
    identifier: &Value,
) -> (u16, Value) {
    call(
        router,
        REGION,
        "POST",
        &format!("/v2/connect-instance/{instance_id}/integrations/delete"),
        &json!({"integrationIdentifier": identifier}),
    )
    .await
}

/// The integrations `instance_id` holds, read through the list.
async fn held(router: &MultiServiceRouter, instance_id: &str, case: &str) -> Vec<Value> {
    let (status, body) = list_integrations(router, instance_id, "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the list, got {status} {body}"
    );
    summaries(&body)
}

/// Assert `response` is the modelled not-found refusal.
fn assert_not_found(case: &str, status: u16, response: &Value) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected HTTP 404, got {status} {response}"
    );
    assert!(
        error_code(response).ends_with("ResourceNotFoundException"),
        "case {case:?}: expected ResourceNotFoundException, got {}",
        error_code(response)
    );
}

#[tokio::test]
async fn a_malformed_identifier_is_a_bad_request_whatever_the_instance_holds() {
    let router = router();

    // The three instance states a delete can meet, so no refusal below can be an
    // instance gate answering: an id the account's Amazon Connect registry does not
    // hold, one it holds whose onboarding job has not finished, and one onboarded and
    // holding a well-formed integration of every kind.
    let unknown = "identifier-bounds-unknown";
    let registered = "identifier-bounds-registered";
    let onboarded = "identifier-bounds-onboarded";
    router.register_connect_instance(REGION, registered);
    super::connect_instance_integrations::onboard(&router, onboarded).await;

    let fixtures = well_formed();
    for (case, config, _) in &fixtures {
        let (status, body) = put_integration(&router, onboarded, config).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 seeding the integration, got {status} {body}"
        );
    }
    let before = held(&router, onboarded, "the seeded integrations").await;

    for (member, build, path, bad_values) in &identifier_members() {
        for (value_case, value, requirement) in bad_values {
            let identifier = build(value);
            for instance in [unknown, registered, onboarded] {
                let case = format!("{member} sent {value_case} for instance {instance:?}");
                let (status, body) = delete_integration(&router, instance, &identifier).await;
                assert_refused(
                    &case,
                    status,
                    &body,
                    &format!("integrationIdentifier.{path}"),
                    requirement,
                );
            }

            // A refused delete removes nothing.
            let case = format!("{member} sent {value_case}");
            let after = held(&router, onboarded, &case).await;
            assert_eq!(
                after, before,
                "case {case:?}: expected the refusals to leave the integrations alone, \
                 got {after:?} (was {before:?})"
            );
        }
    }

    // The converse: the very same identifiers, carrying an ARN each shape admits, are
    // not bad requests — against an instance or a record that does not exist they are
    // the modelled not-found, and against the record that does they succeed.
    for (case, _, identifier) in &fixtures {
        for instance in [unknown, registered] {
            let case = format!("{case} for instance {instance:?}");
            let (status, body) = delete_integration(&router, instance, identifier).await;
            assert_not_found(&case, status, &body);
        }

        let (status, body) = delete_integration(&router, onboarded, identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 deleting the held integration, got {status} {body}"
        );

        // Removed, so the same identifier now names no record: still not a bad request.
        let (status, body) = delete_integration(&router, onboarded, identifier).await;
        assert_not_found(&format!("{case} already removed"), status, &body);
    }

    let remaining = held(&router, onboarded, "every held integration deleted").await;
    assert_eq!(
        remaining,
        Vec::<Value>::new(),
        "case \"every held integration deleted\": expected the instance to hold nothing, \
         got {remaining:?}"
    );
}

#[tokio::test]
async fn an_identifier_whose_required_arn_is_absent_is_a_bad_request() {
    let router = router();
    let onboarded = "identifier-required-onboarded";
    super::connect_instance_integrations::onboard(&router, onboarded).await;

    // Each identifier structure's ARN member is `@required`, so a union member
    // carrying none of it names no integration either — one row per member.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "customerProfiles with no domainArn",
            json!({"customerProfiles": {}}),
            "domainArn",
        ),
        (
            "qConnect with no knowledgeBaseArn",
            json!({"qConnect": {}}),
            "knowledgeBaseArn",
        ),
        (
            "lambda with no functionArn",
            json!({"lambda": {}}),
            "functionArn",
        ),
    ];

    for (case, identifier, member) in cases {
        let (status, body) = delete_integration(&router, onboarded, &identifier).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {}",
            error_code(&body)
        );
        let message = body["message"].as_str().unwrap_or("<no message>");
        assert!(
            message.contains(member) && message.contains("Member must not be null"),
            "case {case:?}: expected the refusal to state that {member:?} must not be \
             null, got {message:?}"
        );
    }
}
