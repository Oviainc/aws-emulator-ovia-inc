// SPDX-License-Identifier: CC0-1.0

//! The application / authentication-method slice end to end over the `awsJson1_1` wire.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! example:
//!
//! 1. an authentication method of an application is KEYED BY ITS
//!    `AuthenticationMethodType`: a `PutApplicationAuthenticationMethod` adds the method
//!    of that type when the application carries none and REPLACES it when it does, and
//!    `GetApplicationAuthenticationMethod` reports exactly the method the last put of
//!    that type stored — the `IamAuthenticationMethod$ActorPolicy` document verbatim, for
//!    every JSON object the `@document` member admits;
//! 2. the put is `@idempotent` and the get is a REPORT: repeating either answers
//!    identically and changes nothing a later caller sees;
//! 3. the method belongs to ONE application: a put on one application is invisible on
//!    every other;
//! 4. an `ApplicationArn` this account holds no application for, and an
//!    `AuthenticationMethodType` the application carries no method for, are refused
//!    `ResourceNotFoundException` — never a 200 with the optional `AuthenticationMethod`
//!    omitted — and the refused put stores nothing;
//! 5. an `ActorPolicy` that is not a JSON object is refused `ValidationException`, since
//!    the member is documented as "An IAM policy document in JSON", and that refusal
//!    stores nothing;
//! 6. a request outside the modelled constraints — an `AuthenticationMethodType` the
//!    enum does not carry, an `AuthenticationMethod` union variant the model does not
//!    declare — is refused `ValidationException` by the boundary, on both the put and
//!    the get.
//! 7. because the method lives ON the application record, the put resolves the
//!    application at the INSTANT of the call: an application whose instance the clock has
//!    already removed no longer exists, so the put is refused `ResourceNotFoundException`
//!    and stores nothing, while an application whose instance is still being deleted does
//!    exist and is served.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints,
//! the `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than
//! bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns. Where a test has
//! to be on the far side of a due instant it moves the STORED instant rather than
//! sleeping — the transition itself is still applied by the service's own
//! `materialize_due`, exactly as in production.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    call, error_code, hold_deletion, router, router_over, seeded_instance_arn,
};

/// The OAuth 2.0 application provider every application below is created under.
///
/// SOURCE: `state::BUILT_IN_APPLICATION_PROVIDERS`, the catalogue
/// `ListApplicationProviders` reports, whose `custom` entry is the "SSO Custom
/// Application Provider".
const CUSTOM_PROVIDER_ARN: &str = "arn:aws:sso::aws:applicationProvider/custom";

/// The only value the model's `AuthenticationMethodType` enum admits.
///
/// SOURCE: `smithy_query(query="shape",shape="AuthenticationMethodType")` — `IAM`.
const IAM: &str = "IAM";

/// Create one application and return its `ApplicationArn`.
async fn create_application(
    router: &MultiServiceRouter,
    instance_arn: &str,
    name: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreateApplication",
        &json!({
            "InstanceArn": instance_arn,
            "ApplicationProviderArn": CUSTOM_PROVIDER_ARN,
            "Name": name,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateApplication must answer 200, got {status} {body}"
    );
    body["ApplicationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: CreateApplication returned no ARN: {body}"))
        .to_string()
}

/// `PutApplicationAuthenticationMethod` with an IAM method carrying `actor_policy`.
async fn put_iam_method(
    router: &MultiServiceRouter,
    application_arn: &str,
    actor_policy: &Value,
) -> (u16, Value) {
    put_method(
        router,
        application_arn,
        IAM,
        &json!({"Iam": {"ActorPolicy": actor_policy}}),
    )
    .await
}

/// The same put, over any `AuthenticationMethodType` and any `AuthenticationMethod`
/// union body — including ones the model refuses.
async fn put_method(
    router: &MultiServiceRouter,
    application_arn: &str,
    method_type: &str,
    method: &Value,
) -> (u16, Value) {
    call(
        router,
        "PutApplicationAuthenticationMethod",
        &json!({
            "ApplicationArn": application_arn,
            "AuthenticationMethodType": method_type,
            "AuthenticationMethod": method,
        }),
    )
    .await
}

/// `GetApplicationAuthenticationMethod` for one type of one application.
async fn get_method(
    router: &MultiServiceRouter,
    application_arn: &str,
    method_type: &str,
) -> (u16, Value) {
    call(
        router,
        "GetApplicationAuthenticationMethod",
        &json!({
            "ApplicationArn": application_arn,
            "AuthenticationMethodType": method_type,
        }),
    )
    .await
}

/// The body a get must answer with when `actor_policy` was put as an IAM method.
fn expected_iam_body(actor_policy: &Value) -> Value {
    json!({"AuthenticationMethod": {"Iam": {"ActorPolicy": actor_policy}}})
}

/// The `ActorPolicy` documents rule 1 is asserted over.
///
/// `IamAuthenticationMethod$ActorPolicy` is an `@document` member
/// (`smithy_query(query="shape",shape="IamAuthenticationMethod")`) documented as "An IAM
/// policy document in JSON", so the rows differ along everything a JSON object can differ
/// along: member count, nesting depth, the value kinds inside (string, integer, float,
/// boolean, null, list, object), the shape of `Statement` (one object, a list, many) and
/// the overall size.
fn actor_policies() -> Vec<(&'static str, Value)> {
    vec![
        (
            "the smallest JSON object the rule admits",
            json!({}),
        ),
        (
            "a single statement with scalar Action and Resource",
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    {"Effect": "Allow", "Action": "s3:GetObject", "Resource": "*"}
                ],
            }),
        ),
        (
            "a Statement that is one object rather than a list",
            json!({
                "Version": "2012-10-17",
                "Statement": {
                    "Effect": "Deny",
                    "NotAction": "iam:*",
                    "NotResource": "arn:aws:iam::*:role/admin",
                },
            }),
        ),
        (
            "list-valued Action and Resource across three statements",
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Sid": "ReadWrite",
                        "Effect": "Allow",
                        "Action": ["s3:GetObject", "s3:PutObject", "s3:ListBucket"],
                        "Resource": ["arn:aws:s3:::bucket", "arn:aws:s3:::bucket/*"],
                    },
                    {"Effect": "Allow", "Action": ["sts:TagSession"], "Resource": ["*"]},
                    {"Effect": "Deny", "Action": ["s3:DeleteBucket"], "Resource": ["*"]},
                ],
            }),
        ),
        (
            "a nested Condition block, six levels of objects deep",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Action": "sts:AssumeRole",
                    "Resource": "*",
                    "Condition": {
                        "StringEqualsIfExists": {
                            "aws:PrincipalTag/team": ["platform", "identity"],
                            "nested": {"deeper": {"deepest": "value"}},
                        },
                    },
                }],
            }),
        ),
        (
            "every JSON value kind a document admits",
            json!({
                "string": "value",
                "integer": 42,
                "negative": -7,
                "float": 2.5,
                "true": true,
                "false": false,
                "null": Value::Null,
                "empty_list": [],
                "empty_object": {},
                "mixed_list": ["a", 1, true, Value::Null, {"k": "v"}, []],
            }),
        ),
        (
            "a large policy of fifty generated statements",
            json!({
                "Version": "2012-10-17",
                "Statement": (0..50)
                    .map(|index| json!({
                        "Sid": format!("Statement{index}"),
                        "Effect": if index % 2 == 0 { "Allow" } else { "Deny" },
                        "Action": format!("service{index}:Action"),
                        "Resource": format!("arn:aws:service{index}:::resource/{index}"),
                    }))
                    .collect::<Vec<Value>>(),
            }),
        ),
    ]
}

/// Rules 1 and 2: every row is stored and read back verbatim, a repeated put stores the
/// same method, and a repeated get reports it unchanged.
#[tokio::test]
async fn every_put_authentication_method_reads_back_exactly_what_it_stored() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;

    for (case, policy) in actor_policies() {
        // One application per row, so each row observes a FIRST put as well as a repeat.
        let application_arn =
            create_application(&router, &instance_arn, "method-roundtrip").await;

        // Rule 4: before the put there is no method of this type to read.
        let (status, body) = get_method(&router, &application_arn, IAM).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a get before any put must be refused 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException before the put, got {body}"
        );

        let (status, body) = put_iam_method(&router, &application_arn, &policy).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the put must answer 200, got {status} {body}"
        );
        assert!(
            body["AuthenticationMethod"].is_null(),
            "case {case:?}: the put's output is modelled as Unit, got {body}"
        );

        let (status, read) = get_method(&router, &application_arn, IAM).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the get after the put must answer 200, got {status} {read}"
        );
        assert_eq!(
            read,
            expected_iam_body(&policy),
            "case {case:?}: the get must report exactly the method the put stored, got {read}"
        );

        // Rule 2: the put is idempotent and the get is a report, so repeating both
        // answers identically.
        let (status, body) = put_iam_method(&router, &application_arn, &policy).await;
        assert_eq!(
            status, 200,
            "case {case:?}: repeating the put must answer 200, got {status} {body}"
        );
        let (status, again) = get_method(&router, &application_arn, IAM).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the get after the repeated put must answer 200, got {status} {again}"
        );
        assert_eq!(
            again, read,
            "case {case:?}: repeating the put must leave the same method stored, got {again}"
        );
        let (status, third) = get_method(&router, &application_arn, IAM).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a third get must answer 200, got {status} {third}"
        );
        assert_eq!(
            third, read,
            "case {case:?}: a get must change nothing a later caller sees, got {third}"
        );
    }
}

/// Rule 1: a put of a type the application already carries REPLACES the stored method,
/// in every direction the document can change.
#[tokio::test]
async fn a_second_put_replaces_the_method_of_the_same_type() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let application_arn = create_application(&router, &instance_arn, "replaced-method").await;

    // The transitions a replacement can take: growing, shrinking, swapping a value for
    // one of another JSON kind, and emptying the document down to the bare object.
    let policies = actor_policies();
    let mut previous: Option<Value> = None;
    for (case, policy) in policies.iter().chain(policies.iter().rev()) {
        let (status, body) = put_iam_method(&router, &application_arn, policy).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the replacing put must answer 200, got {status} {body}"
        );
        let (status, read) = get_method(&router, &application_arn, IAM).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the get after the replacing put must answer 200, got {status} {read}"
        );
        assert_eq!(
            read,
            expected_iam_body(policy),
            "case {case:?}: the second put must REPLACE the stored method, got {read}"
        );
        if let Some(previous) = &previous {
            if previous != policy {
                assert_ne!(
                    read,
                    expected_iam_body(previous),
                    "case {case:?}: the replaced method must no longer be readable, got {read}"
                );
            }
        }
        previous = Some(policy.clone());
    }
}

/// Rule 3: the method belongs to one application, so a put on one is invisible on every
/// other — including on applications created before and after it.
#[tokio::test]
async fn an_authentication_method_belongs_to_one_application() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;

    let names = ["app-one", "app-two", "app-three", "app-four"];
    let mut arns: Vec<String> = Vec::new();
    for name in names {
        arns.push(create_application(&router, &instance_arn, name).await);
    }

    // Each application gets its OWN policy, so a leak between two of them is visible as
    // the wrong document and not only as a 200.
    for (index, arn) in arns.iter().enumerate() {
        let policy = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": format!("app{index}:Call"),
                "Resource": arn,
            }],
        });
        let (status, body) = put_iam_method(&router, arn, &policy).await;
        assert_eq!(
            status, 200,
            "case {:?}: the put must answer 200, got {status} {body}",
            names[index]
        );
    }

    for (index, arn) in arns.iter().enumerate() {
        let expected = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": format!("app{index}:Call"),
                "Resource": arn,
            }],
        });
        let (status, read) = get_method(&router, arn, IAM).await;
        assert_eq!(
            status, 200,
            "case {:?}: the get must answer 200, got {status} {read}",
            names[index]
        );
        assert_eq!(
            read,
            expected_iam_body(&expected),
            "case {:?}: each application must report its OWN method, got {read}",
            names[index]
        );
    }

    // An application created after all those puts carries no method at all.
    let latecomer = create_application(&router, &instance_arn, "app-five").await;
    let (status, body) = get_method(&router, &latecomer, IAM).await;
    assert_eq!(
        status, 400,
        "case \"app-five\": an application nobody put a method on must be refused 400, got \
         {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case \"app-five\": expected ResourceNotFoundException, got {body}"
    );
}

/// Rule 4: an `ApplicationArn` no create minted is refused by both the put and the get,
/// and the refused put stores nothing anywhere.
#[tokio::test]
async fn an_application_arn_no_create_minted_is_refused_not_found() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let instance_id = instance_arn.rsplit('/').next().unwrap_or_default().to_string();
    // One application exists, so the not-found is about the ARN asked for and not about
    // an empty store.
    let stored = create_application(&router, &instance_arn, "stored-app").await;
    let account = stored.split(':').nth(4).unwrap_or_default().to_string();
    let suffix = stored.rsplit("/apl-").next().unwrap_or_default().to_string();
    let application_arn = |partition: &str, account: &str, instance: &str, suffix: &str| {
        format!("arn:{partition}:sso::{account}:application/{instance}/apl-{suffix}")
    };

    // The rows span the parts of the ARN that identify the application: the `apl-`
    // suffix, the instance segment, the account segment and the partition.
    let cases: Vec<(&str, String)> = vec![
        (
            "a pattern-valid apl- suffix that was never minted",
            application_arn("aws", &account, &instance_id, "0000000000000000"),
        ),
        (
            "the stored suffix under another instance",
            application_arn("aws", &account, "ssoins-0000000000000000", &suffix),
        ),
        (
            "the stored application under another account",
            application_arn("aws", "210987654321", &instance_id, &suffix),
        ),
        (
            "the stored application in another partition",
            application_arn("aws-us-gov", &account, &instance_id, &suffix),
        ),
    ];

    let policy = json!({"Version": "2012-10-17", "Statement": []});
    for (case, arn) in cases {
        assert_ne!(
            arn, stored,
            "case {case:?}: the row must name a DIFFERENT ARN than the stored one"
        );
        let (status, body) = put_iam_method(&router, &arn, &policy).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a put under an unminted ARN must be refused 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException from the put, got {body}"
        );

        let (status, body) = get_method(&router, &arn, IAM).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a get under an unminted ARN must be refused 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException from the get, got {body}"
        );
    }

    // The refused puts stored nothing, on the stored application or anywhere else.
    let (status, body) = get_method(&router, &stored, IAM).await;
    assert_eq!(
        status, 400,
        "a refused put must store no method on the stored application, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException for a method no put stored, got {body}"
    );
}

/// Rule 5: an `ActorPolicy` that is not a JSON object is refused, and the refusal leaves
/// the method the application already carries untouched.
#[tokio::test]
async fn an_actor_policy_that_is_not_a_json_object_is_refused() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let application_arn = create_application(&router, &instance_arn, "bad-policy-app").await;

    let stored = json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Action": "s3:*", "Resource": "*"}],
    });
    let (status, body) = put_iam_method(&router, &application_arn, &stored).await;
    assert_eq!(status, 200, "the seed put must answer 200, got {status} {body}");

    // The rows span every JSON value kind that is NOT an object, including a list of
    // statements — which looks like half a policy but is not one.
    let cases: Vec<(&str, Value)> = vec![
        ("a string", json!("{\"Version\":\"2012-10-17\"}")),
        ("an integer", json!(1)),
        ("a float", json!(1.5)),
        ("a boolean", json!(true)),
        ("a null", Value::Null),
        ("an empty list", json!([])),
        (
            "a bare list of statements",
            json!([{"Effect": "Allow", "Action": "s3:*", "Resource": "*"}]),
        ),
    ];

    for (case, actor_policy) in cases {
        let (status, body) = put_iam_method(&router, &application_arn, &actor_policy).await;
        assert_eq!(
            status, 400,
            "case {case:?}: an ActorPolicy that is not a JSON object must be refused 400, \
             got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }

    // Every refusal left the stored method exactly as the accepted put left it.
    let (status, read) = get_method(&router, &application_arn, IAM).await;
    assert_eq!(
        status, 200,
        "the stored method must survive the refusals, got {status} {read}"
    );
    assert_eq!(
        read,
        expected_iam_body(&stored),
        "a refused put must not change the stored method, got {read}"
    );
}

/// Rule 6: a request outside the modelled constraints is refused `ValidationException`,
/// on the put and on the get, and stores nothing.
#[tokio::test]
async fn requests_outside_the_modeled_constraints_are_refused() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let application_arn = create_application(&router, &instance_arn, "constrained-app").await;
    let policy = json!({"Version": "2012-10-17", "Statement": []});
    let iam_method = json!({"Iam": {"ActorPolicy": policy}});

    // `AuthenticationMethodType` values the model's enum does not carry: the rows span
    // the case, a near-miss suffix, a value of another enum of this service and the
    // empty string.
    let unknown_types = [
        ("the only value in another case", "Iam"),
        ("a lowercase value", "iam"),
        ("a near-miss with a suffix", "IAM_V2"),
        ("a value of another enum", "OIDC_JWT"),
        ("the empty string", ""),
    ];
    for (case, method_type) in unknown_types {
        let (status, body) =
            put_method(&router, &application_arn, method_type, &iam_method).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a put with an unmodelled AuthenticationMethodType must be \
             refused 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException from the put, got {body}"
        );

        let (status, body) = get_method(&router, &application_arn, method_type).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a get with an unmodelled AuthenticationMethodType must be \
             refused 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException from the get, got {body}"
        );
    }

    // `AuthenticationMethod` bodies the model's union does not admit: a variant it does
    // not declare, two variants at once, and no variant at all.
    let unknown_methods: Vec<(&str, Value)> = vec![
        (
            "a union variant the model does not declare",
            json!({"Saml": {"ActorPolicy": policy}}),
        ),
        ("a union with no variant set", json!({})),
        (
            "an Iam variant missing its required ActorPolicy",
            json!({"Iam": {}}),
        ),
    ];
    for (case, method) in unknown_methods {
        let (status, body) = put_method(&router, &application_arn, IAM, &method).await;
        assert_eq!(
            status, 400,
            "case {case:?}: an unmodelled AuthenticationMethod must be refused 400, got \
             {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }

    // None of the refusals stored anything.
    let (status, body) = get_method(&router, &application_arn, IAM).await;
    assert_eq!(
        status, 400,
        "a refused request must store no method, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException for a method no put stored, got {body}"
    );
}

/// The `ActorPolicy` the application created `index` places along a table carries, so a
/// method read back under the wrong application is visible as the wrong document.
fn policy_for(index: usize) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": format!("Application{index}"),
            "Effect": "Allow",
            "Action": format!("app{index}:Call"),
            "Resource": "*",
        }],
    })
}

/// Rule 7: the method lives ON the application record, so the put resolves the
/// application at the INSTANT of the call. An application whose instance is still being
/// deleted exists, so the put is served and the get reports it; once the clock has made
/// that removal due the application is gone with it, so the put and the get are both
/// refused `ResourceNotFoundException` and nothing is stored under the ARN — and an
/// application created afterwards inherits no method.
#[tokio::test]
async fn a_put_resolves_the_application_at_the_instant_of_the_call() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let instance_arn = seeded_instance_arn(&router).await;

    // The rows differ along the position of the application in a set of several, so the
    // rule is asserted over more than the one application a delete happens to visit
    // first.
    let names = ["held-one", "held-two", "held-three"];
    let mut arns: Vec<String> = Vec::new();
    for name in names {
        arns.push(create_application(&router, &instance_arn, name).await);
    }
    for (index, arn) in arns.iter().enumerate() {
        let (status, body) = put_iam_method(&router, arn, &policy_for(index)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the seed put must answer 200, got {status} {body}",
            names[index]
        );
    }

    let deleted_at = SystemTime::now();
    let (status, body) = call(
        &router,
        "DeleteInstance",
        &json!({"InstanceArn": instance_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteInstance must answer 200, got {status} {body}"
    );

    // While the removal the delete owes is still pending, every application the instance
    // holds still exists: the put of a REPLACEMENT method is served and the get reports
    // exactly it.
    hold_deletion(&engine, deleted_at + Duration::from_secs(3_600));
    for (index, arn) in arns.iter().enumerate() {
        let replacement = policy_for(index + 100);
        let (status, body) = put_iam_method(&router, arn, &replacement).await;
        assert_eq!(
            status, 200,
            "case {:?}: a put while the instance removal is pending must answer 200, got \
             {status} {body}",
            names[index]
        );
        let (status, read) = get_method(&router, arn, IAM).await;
        assert_eq!(
            status, 200,
            "case {:?}: the get while the removal is pending must answer 200, got {status} \
             {read}",
            names[index]
        );
        assert_eq!(
            read,
            expected_iam_body(&replacement),
            "case {:?}: the put must have replaced the stored method, got {read}",
            names[index]
        );
    }

    // Once the removal falls due the applications are gone, so neither half of the pair
    // resolves their ARNs any more.
    hold_deletion(&engine, deleted_at - Duration::from_secs(1));
    for (index, arn) in arns.iter().enumerate() {
        let (status, body) = put_iam_method(&router, arn, &policy_for(index)).await;
        assert_eq!(
            status, 400,
            "case {:?}: a put on an application whose instance is gone must be refused 400, \
             got {status} {body}",
            names[index]
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {:?}: expected ResourceNotFoundException from the put, got {body}",
            names[index]
        );

        let (status, body) = get_method(&router, arn, IAM).await;
        assert_eq!(
            status, 400,
            "case {:?}: a get on an application whose instance is gone must be refused 400, \
             got {status} {body}",
            names[index]
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {:?}: expected ResourceNotFoundException from the get, got {body}",
            names[index]
        );
    }

    // The refusals were about the missing application record and nothing else: an
    // application created under a new instance carries no method until a put stores one,
    // and that put is served.
    let (status, created) = call(&router, "CreateInstance", &json!({"Name": "AfterDelete"})).await;
    assert_eq!(
        status, 200,
        "CreateInstance must answer 200 once the quota is free, got {status} {created}"
    );
    let new_instance_arn = created["InstanceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateInstance returned no InstanceArn: {created}"))
        .to_string();
    let fresh = create_application(&router, &new_instance_arn, "held-one").await;
    let (status, body) = get_method(&router, &fresh, IAM).await;
    assert_eq!(
        status, 400,
        "a fresh application must inherit no method, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException on a fresh application, got {body}"
    );
    let stored = policy_for(7);
    let (status, body) = put_iam_method(&router, &fresh, &stored).await;
    assert_eq!(
        status, 200,
        "the put on the fresh application must answer 200, got {status} {body}"
    );
    let (status, read) = get_method(&router, &fresh, IAM).await;
    assert_eq!(
        status, 200,
        "the get on the fresh application must answer 200, got {status} {read}"
    );
    assert_eq!(
        read,
        expected_iam_body(&stored),
        "the fresh application must report the method its own put stored, got {read}"
    );
}
