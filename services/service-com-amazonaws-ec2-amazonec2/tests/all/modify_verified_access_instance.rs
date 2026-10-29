// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ModifyVerifiedAccessInstance`, driven over the wire the service
//! actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rule under test: a modify APPLIES THE MEMBERS THE REQUEST CARRIES to the stored
//! instance, leaves every member it does not carry as it was, stamps the instance's
//! `LastUpdatedTime`, and answers the record the store now holds — so a later read of the
//! same instance reports what the modify left behind. Around that:
//!
//! 1. every shape a modify request can take applies exactly what it carries, and the members
//!    the service owns (the id and `CreationTime`) are not touched;
//! 2. the answer is the STORE's record rather than the request echoed back, so
//!    `DescribeVerifiedAccessInstances` reports the same instance afterwards;
//! 3. an instance id this account holds no instance for modifies nothing and is refused, and
//!    so is a subdomain that names no domain and a client token past the service's bound;
//! 4. one `ClientToken` names one modification — a repeat applies nothing further, and the
//!    same token carrying a DIFFERENT request is refused rather than applied twice — and a
//!    `DryRun` reports the outcome the request would have had while writing nothing.
//!
//! Each table row is a distinct input along what the rule generalises over — which members
//! the request carries, whether the instance already held a value for them, which reference
//! is unresolvable and in which form, how long a client token is — so no row is the
//! specification.

use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, MAX_CLIENT_TOKEN_CHARS};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, call, element, error_code, error_message};

/// The Region every call here is signed for: a Region that is not the emulator's default is
/// the stronger case, and the instance is a regional entity.
const REGION: &str = "eu-west-1";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// Stand up an emulator with nothing seeded: every instance a case modifies is one a create
/// under test minted.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One `CreateVerifiedAccessInstance` call, answering the id it minted.
async fn create_instance(router: &MultiServiceRouter, case: &str, params: &Params) -> String {
    let (status, body) = call(router, REGION, "CreateVerifiedAccessInstance", params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the create under test must succeed, got {status} with body {body}"
    );
    element(&body, "verifiedAccessInstanceId").unwrap_or_else(|| {
        panic!("case {case:?}: the create must report the instance it minted, got {body}")
    })
}

/// One `ModifyVerifiedAccessInstance` call on `instance_id`, with `params` beside the id.
async fn modify(
    router: &MultiServiceRouter,
    instance_id: &str,
    params: &[(String, String)],
) -> (u16, String) {
    let mut body = vec![p("VerifiedAccessInstanceId", instance_id)];
    body.extend_from_slice(params);
    call(router, REGION, "ModifyVerifiedAccessInstance", &body).await
}

/// The instance `instance_id` as `DescribeVerifiedAccessInstances` reports it: the
/// `(description, subDomain, lastUpdatedTime)` of the one instance in that report.
async fn described(
    router: &MultiServiceRouter,
    case: &str,
    instance_id: &str,
) -> (Option<String>, Option<String>, Option<String>) {
    let (status, body) = call(
        router,
        REGION,
        "DescribeVerifiedAccessInstances",
        &[p("VerifiedAccessInstanceId.1", instance_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the describe of {instance_id} must succeed, got {status} with body \
         {body}"
    );
    (
        element(&body, "description"),
        element(&body, "subDomain"),
        element(&body, "lastUpdatedTime"),
    )
}

/// The description the STORE kept for `instance_id`, read off the state the handlers share.
fn stored_description(engine: &engine::InMemoryEngine, instance_id: &str) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .verified_access_instances
        .get(instance_id)
        .and_then(|instance| instance.description.clone())
}

/// RULE: each shape a modify request can take applies exactly the members it carries, leaves
/// the ones it does not carry as they were, and the service's own members — the id and the
/// creation time — are not touched. The answer is the store's record, so the describe of the
/// same instance reports it too.
#[tokio::test]
async fn a_modify_applies_what_it_carries_and_leaves_the_rest() {
    // (case, the create's optional members, the modify's members, the description the
    // instance must then report, the subdomain it must then report).
    type Row = (
        &'static str,
        Params,
        Params,
        Option<&'static str>,
        Option<&'static str>,
    );
    let rows: Vec<Row> = vec![
        (
            "a description onto an instance that had none",
            Vec::new(),
            vec![p("Description", "the sales applications")],
            Some("the sales applications"),
            None,
        ),
        (
            "a description over one the create set",
            vec![p("Description", "the sales applications")],
            vec![p("Description", "the support applications")],
            Some("the support applications"),
            None,
        ),
        (
            "a subdomain onto an instance that had none",
            Vec::new(),
            vec![p("CidrEndpointsCustomSubDomain", "access.example.com")],
            None,
            Some("access.example.com"),
        ),
        (
            "a subdomain over one the create set",
            vec![p("CidrEndpointsCustomSubDomain", "access.example.com")],
            vec![p("CidrEndpointsCustomSubDomain", "support.example.org")],
            None,
            Some("support.example.org"),
        ),
        (
            "both members at once",
            Vec::new(),
            vec![
                p("Description", "the support applications"),
                p("CidrEndpointsCustomSubDomain", "support.example.org"),
            ],
            Some("the support applications"),
            Some("support.example.org"),
        ),
        (
            "one member while the other stays as the create left it",
            vec![
                p("Description", "the sales applications"),
                p("CidrEndpointsCustomSubDomain", "access.example.com"),
            ],
            vec![p("Description", "the support applications")],
            Some("the support applications"),
            Some("access.example.com"),
        ),
        (
            "no member at all beside the instance id",
            vec![
                p("Description", "the sales applications"),
                p("CidrEndpointsCustomSubDomain", "access.example.com"),
            ],
            Vec::new(),
            Some("the sales applications"),
            Some("access.example.com"),
        ),
    ];

    for (case, created_with, modified_with, description, sub_domain) in rows {
        let (router, engine) = emulator();
        // FIPS support and a tag are on every instance under test: neither is a member a
        // modify carries, so both must read back untouched afterwards.
        let mut create_params = vec![
            p("FIPSEnabled", "true"),
            p("TagSpecification.1.ResourceType", "verified-access-instance"),
            p("TagSpecification.1.Tag.1.Key", "Team"),
            p("TagSpecification.1.Tag.1.Value", "platform"),
        ];
        create_params.extend(created_with);
        let instance_id = create_instance(&router, case, &create_params).await;
        let (_, _, created_at) = described(&router, case, &instance_id).await;

        let (status, body) = modify(&router, &instance_id, &modified_with).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "verifiedAccessInstanceId").as_deref(),
            Some(instance_id.as_str()),
            "case {case:?}: expected the instance the request named, got {:?}",
            element(&body, "verifiedAccessInstanceId")
        );
        assert_eq!(
            element(&body, "description").as_deref(),
            description,
            "case {case:?}: expected description {description:?}, got {:?}",
            element(&body, "description")
        );
        assert_eq!(
            element(&body, "subDomain").as_deref(),
            sub_domain,
            "case {case:?}: expected subDomain {sub_domain:?}, got {:?}",
            element(&body, "subDomain")
        );
        // The members a modify does not carry are the instance's own: FIPS support, the tags
        // and the trust providers a create attached (none).
        assert_eq!(
            element(&body, "fipsEnabled").as_deref(),
            Some("true"),
            "case {case:?}: a modify carries no FIPS member, so the instance keeps the \
             create's answer, got {:?}",
            element(&body, "fipsEnabled")
        );
        assert_eq!(
            element(&body, "key").as_deref(),
            Some("Team"),
            "case {case:?}: a modify carries no tag, so the instance keeps the create's, got \
             {body}"
        );

        // The service's own members: the creation time is not touched, and the last updated
        // time is the instant this modification was applied — so it is at or after it.
        let (described_description, described_sub_domain, last_updated) =
            described(&router, case, &instance_id).await;
        let creation_time = element(&body, "creationTime").unwrap_or_default();
        assert!(
            as_timestamp(&creation_time).is_some(),
            "case {case:?}: expected an ISO 8601 creationTime, got {creation_time:?}"
        );
        let updated_at = as_timestamp(last_updated.as_deref().unwrap_or_default());
        assert!(
            updated_at
                .zip(as_timestamp(&creation_time))
                .is_some_and(|(updated, created)| updated >= created),
            "case {case:?}: expected a lastUpdatedTime at or after the creationTime \
             {creation_time:?}, got {last_updated:?}"
        );
        assert_eq!(
            element(&body, "lastUpdatedTime"),
            last_updated,
            "case {case:?}: the modify answers the stored record, so its lastUpdatedTime is \
             the one a later read reports, got {:?}",
            element(&body, "lastUpdatedTime")
        );
        assert_eq!(
            created_at.and_then(|at| as_timestamp(&at)),
            as_timestamp(&creation_time),
            "case {case:?}: a modify does not move the creationTime, got {creation_time:?}"
        );

        // RULE, read back: the answer is the STORE's record, not the request echoed.
        assert_eq!(
            described_description.as_deref(),
            description,
            "case {case:?}: expected the describe to report description {description:?}, got \
             {described_description:?}"
        );
        assert_eq!(
            described_sub_domain.as_deref(),
            sub_domain,
            "case {case:?}: expected the describe to report subDomain {sub_domain:?}, got \
             {described_sub_domain:?}"
        );
        assert_eq!(
            stored_description(&engine, &instance_id).as_deref(),
            description,
            "case {case:?}: expected the store to keep description {description:?}, got {:?}",
            stored_description(&engine, &instance_id)
        );
    }
}

/// RULE's refusals: an instance id this account holds no instance for modifies nothing,
/// whatever the id looks like, and so does a subdomain that names no domain or a client token
/// past the bound this service accepts — each on the bound and one past it.
#[tokio::test]
async fn a_modify_refuses_an_unresolvable_instance_and_a_value_it_cannot_take() {
    let token = |chars: usize| "t".repeat(chars);

    // (case, an id no create minted) — the long and the short EC2 id form, an id of another
    // Verified Access resource kind, and a string that is no EC2 id at all.
    let unminted: Vec<(&str, &str)> = vec![
        ("an unminted long-form instance id", "vai-0123456789abcdef0"),
        ("an unminted short-form instance id", "vai-01234567"),
        ("a group id", "vagr-0123456789abcdef0"),
        ("an id of no EC2 resource", "not-an-instance"),
    ];
    for (case, instance_id) in unminted {
        let (router, engine) = emulator();
        let held = create_instance(&router, case, &vec![p("Description", "the one instance")]).await;
        let (status, body) = modify(
            &router,
            instance_id,
            &[p("Description", "a description nothing keeps")],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for an instance nothing minted, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidVerifiedAccessInstanceId.NotFound",
            "case {case:?}: expected the instance's own NotFound code, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The verified-access-instance ID '{instance_id}' does not exist"),
            "case {case:?}: expected the refusal to name the id the request gave, got {}",
            error_message(&body)
        );
        assert_eq!(
            stored_description(&engine, &held).as_deref(),
            Some("the one instance"),
            "case {case:?}: a refused modify changes no other instance, got {:?}",
            stored_description(&engine, &held)
        );
    }

    // (case, the members beside the instance id, the HTTP status, the error code an accepted
    // modify reports as the empty string).
    let rows: Vec<(&str, Params, u16, &str)> = vec![
        (
            "a subdomain that names a domain",
            vec![p("CidrEndpointsCustomSubDomain", "access.example.com")],
            200,
            "",
        ),
        (
            "a subdomain that is the empty name",
            vec![p("CidrEndpointsCustomSubDomain", "")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a subdomain that is nothing but blanks",
            vec![p("CidrEndpointsCustomSubDomain", "   ")],
            400,
            "InvalidParameterValue",
        ),
        (
            "as long a client token as the service takes",
            vec![p("ClientToken", token(MAX_CLIENT_TOKEN_CHARS))],
            200,
            "",
        ),
        (
            "one client token character too many",
            vec![p("ClientToken", token(MAX_CLIENT_TOKEN_CHARS + 1))],
            400,
            "InvalidParameterValue",
        ),
    ];
    for (case, params, expected_status, expected_code) in rows {
        let (router, engine) = emulator();
        let instance_id =
            create_instance(&router, case, &vec![p("Description", "the one instance")]).await;
        let mut members = vec![p("Description", "the modified instance")];
        members.extend(params);
        let (status, body) = modify(&router, &instance_id, &members).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the {expected_code:?} refusal, got {body}"
        );
        let expected_description = if expected_status == 200 {
            "the modified instance"
        } else {
            "the one instance"
        };
        assert_eq!(
            stored_description(&engine, &instance_id).as_deref(),
            Some(expected_description),
            "case {case:?}: expected the store to hold {expected_description:?}, got {:?}",
            stored_description(&engine, &instance_id)
        );
    }
}

/// RULE's idempotency: one `ClientToken` names one modification, so a repeat of the request
/// applies nothing further — not even a second `LastUpdatedTime` — while the same token
/// carrying a different request is refused rather than applied a second time, and a token the
/// store has not seen applies a modification of its own.
#[tokio::test]
async fn one_client_token_names_one_modification() {
    let (router, engine) = emulator();
    let instance_id = create_instance(
        &router,
        "the instance under test",
        &vec![
            p("ClientToken", "the-create-token"),
            p("Description", "the instance under test"),
        ],
    )
    .await;

    let first_params = vec![
        p("ClientToken", "the-first-token"),
        p("Description", "the modified instance"),
    ];
    let (status, body) = modify(&router, &instance_id, &first_params).await;
    assert_eq!(
        status, 200,
        "the first modify must succeed, got {status} with body {body}"
    );
    let applied_at = element(&body, "lastUpdatedTime").unwrap_or_default();

    // (case, the members of the next call, its HTTP status, its error code, the description
    // the store must hold afterwards, whether its lastUpdatedTime must be the first's).
    type Row = (&'static str, Params, u16, &'static str, &'static str, bool);
    let rows: Vec<Row> = vec![
        (
            "the same token and the same request",
            first_params.clone(),
            200,
            "",
            "the modified instance",
            true,
        ),
        (
            "the same token and a different description",
            vec![
                p("ClientToken", "the-first-token"),
                p("Description", "a description nothing keeps"),
            ],
            400,
            "IdempotentParameterMismatch",
            "the modified instance",
            false,
        ),
        (
            "the same token and one member more",
            vec![
                p("ClientToken", "the-first-token"),
                p("Description", "the modified instance"),
                p("CidrEndpointsCustomSubDomain", "access.example.com"),
            ],
            400,
            "IdempotentParameterMismatch",
            "the modified instance",
            false,
        ),
        (
            // A token is read against the request it was given with, so the token the CREATE
            // was given names no modification: this is a modification, not a replay of it.
            "the token the create was given",
            vec![
                p("ClientToken", "the-create-token"),
                p("Description", "modified under the create's token"),
            ],
            200,
            "",
            "modified under the create's token",
            false,
        ),
        (
            "a token the store has not seen",
            vec![
                p("ClientToken", "the-second-token"),
                p("Description", "modified again"),
            ],
            200,
            "",
            "modified again",
            false,
        ),
        (
            "no token at all",
            vec![p("Description", "modified without a token")],
            200,
            "",
            "modified without a token",
            false,
        ),
    ];

    for (case, params, expected_status, expected_code, expected_description, replayed) in rows {
        let (status, body) = modify(&router, &instance_id, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the {expected_code:?} refusal, got {body}"
        );
        assert_eq!(
            stored_description(&engine, &instance_id).as_deref(),
            Some(expected_description),
            "case {case:?}: expected the store to hold {expected_description:?}, got {:?}",
            stored_description(&engine, &instance_id)
        );
        if replayed {
            assert_eq!(
                element(&body, "lastUpdatedTime").as_deref(),
                Some(applied_at.as_str()),
                "case {case:?}: a replay applies nothing further, so the instance keeps the \
                 lastUpdatedTime the first call stamped, got {:?}",
                element(&body, "lastUpdatedTime")
            );
        }
        // The subdomain is only ever set by the row that was refused, so an instance that
        // still reports none is one no refused call wrote to.
        if expected_status != 200 {
            assert_eq!(
                described(&router, case, &instance_id).await.1,
                None,
                "case {case:?}: a refused modify writes nothing, so the instance has no \
                 subdomain"
            );
        }
    }
}

/// RULE's `DryRun`: the flag reports the outcome the request WOULD have had — the refusal
/// when it would have been refused — and writes nothing either way.
#[tokio::test]
async fn a_dry_run_reports_the_outcome_and_writes_nothing() {
    // (case, whether the request names the instance that exists, the members beside the flag,
    // the HTTP status, the error code).
    let rows: Vec<(&str, bool, Params, u16, &str)> = vec![
        (
            "a modify that would have succeeded",
            true,
            vec![p("Description", "a description nothing keeps")],
            412,
            "DryRunOperation",
        ),
        (
            "a modify that carries no member to apply",
            true,
            Vec::new(),
            412,
            "DryRunOperation",
        ),
        (
            "a modify that would have been refused for its subdomain",
            true,
            vec![p("CidrEndpointsCustomSubDomain", "")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a modify that would have been refused for its instance id",
            false,
            vec![p("Description", "a description nothing keeps")],
            400,
            "InvalidVerifiedAccessInstanceId.NotFound",
        ),
    ];

    for (case, resolvable, extra, expected_status, expected_code) in rows {
        let (router, engine) = emulator();
        let instance_id =
            create_instance(&router, case, &vec![p("Description", "the one instance")]).await;
        let named = if resolvable {
            instance_id.clone()
        } else {
            "vai-0123456789abcdef0".to_string()
        };
        let mut params = vec![p("DryRun", "true")];
        params.extend(extra);
        let (status, body) = modify(&router, &named, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the {expected_code:?} refusal, got {body}"
        );
        assert_eq!(
            stored_description(&engine, &instance_id).as_deref(),
            Some("the one instance"),
            "case {case:?}: a dry run writes nothing, got {:?}",
            stored_description(&engine, &instance_id)
        );
    }
}
