// SPDX-License-Identifier: CC0-1.0

//! `GetHumanReadableSummary`, end to end over the awsQuery wire.
//!
//! Every test drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is asserted is
//! the HTTP behaviour (status, awsQuery error code, XML members) a client on the
//! socket sees. The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns
//! and the other suites in this crate share.
//!
//! The rule being asserted: this read summarizes exactly ONE kind of entity, so an
//! `EntityArn` naming anything else — an entity of another kind, a resource of
//! another service, or no resource at all — is refused as a validation failure
//! whose message says the resource type is what was wrong, an ARN naming a delegation request the account does not hold is
//! refused as a missing entity, and a delegation request the account holds reports
//! the state of ITS summary generation beside the locale asked for, with no content
//! while that state is not `AVAILABLE`. The read writes nothing.
//!
//! Each suite is a TABLE whose rows differ along what that rule generalises over:
//! the identifier's alphabet and both published length bounds, the entity kind the
//! ARN names, whether the account holds that entity, the locale asked for (including
//! both published bounds and none at all), and the way a value fails to name a
//! delegation request.

use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::DelegationRequest;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

// `router_with_engine` is the shared harness's router whose engine the test also
// holds: this suite seeds the delegation requests `CreateDelegationRequest` will
// write once that operation is implemented (it is still a scaffolded 501, with its
// own stub issue).
use crate::user_lifecycle::{
    account_id, call, element, elements, error_code, router, router_with_engine,
};

/// A trust policy with a single statement, which every role fixture needs to exist.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"glue.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The document a policy this account creates carries.
const CUSTOMER_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

/// The delegation request identifiers the suite spans.
///
/// Source: `delegationRequestIdType` on
/// `smithy_query(query="operation", operation="GetDelegationRequest")` — length
/// 16..128, pattern `^[\w-]+$` — so the rows carry the whole published alphabet
/// (letters in both cases, digits, `_`, `-`) and BOTH published length bounds.
fn delegation_request_ids() -> Vec<String> {
    vec![
        // The published minimum length.
        "abcdef0123456789".to_string(),
        // The published maximum length.
        "d".repeat(128),
        // The whole published alphabet, in a mixed case.
        "Delegation_Request-42_XYZ".to_string(),
        // All digits, and a leading run of zeroes: an identifier is a handle, not a
        // number.
        "000000000000000042".to_string(),
        // Hyphen-separated, as a minted handle often reads.
        "7f3c-9a1b-4d2e-8c55".to_string(),
    ]
}

/// The locales the suite spans.
///
/// Source: `localeType` (length 2..12) and
/// `GetHumanReadableSummaryRequest$Locale`, whose "supported locale strings are
/// based on the Supported languages of the Amazon Web Services Management Console"
/// — whose language list carries both the bare language tags (`en`, `de`, `id`,
/// `ja`) and the language-and-region ones (`pt-BR`, `zh-CN`, `zh-TW`). `None` is the
/// request that asks for no locale at all.
fn locales() -> Vec<Option<String>> {
    vec![
        None,
        // The published minimum length, and a console language tag.
        Some("en".to_string()),
        Some("id".to_string()),
        Some("pt-BR".to_string()),
        Some("zh-CN".to_string()),
        // A longer well-formed tag, and one exactly at the published maximum of 12.
        Some("zh-Hant-TW".to_string()),
        Some("pt-BR-x-test".to_string()),
    ]
}

/// One `GetHumanReadableSummary` call for `entity_arn`, asking for `locale` when the
/// row names one.
async fn summary(
    router: &MultiServiceRouter,
    entity_arn: &str,
    locale: Option<&str>,
) -> (u16, String) {
    let mut params = vec![("EntityArn", entity_arn)];
    if let Some(locale) = locale {
        params.push(("Locale", locale));
    }
    call(router, "GetHumanReadableSummary", &params).await
}

/// Seed the delegation request `delegation_request_id` names into the service's OWN
/// store, answering the ARN that names it.
///
/// This is the state a `CreateDelegationRequest` ("creates an IAM delegation request
/// for temporary access delegation") will write once that operation is implemented —
/// it is still a scaffolded 501 with its own stub issue — so the request is written
/// into the service's own store rather than into a second copy of it.
fn seed_delegation_request(engine: &engine::InMemoryEngine, delegation_request_id: &str) -> String {
    let store = state::service_state(engine).expect("the IAM store");
    store.lock().delegation_requests.insert(
        delegation_request_id.to_string(),
        DelegationRequest {
            delegation_request_id: Some(delegation_request_id.to_string()),
            description: Some("temporary access for a partner engagement".to_string()),
            requestor_id: Some(account_id().to_string()),
            // One of the values `stateType` declares
            // (`ops::types::STATE_TYPE_VALUES`).
            state: Some("PENDING_APPROVAL".to_string()),
            session_duration: Some(3_600),
            create_date: Some(state::now()),
            ..DelegationRequest::default()
        },
    );
    state::delegation_request_arn(delegation_request_id)
}

/// How many delegation requests the account holds, read straight out of the store:
/// what a reporting operation must leave exactly as it found it.
fn held_delegation_requests(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = state::service_state(engine).expect("the IAM store");
    let held = store.lock().delegation_requests.keys().cloned().collect();
    held
}

/// The `SummaryState` a 200 response reports, with the case named on every failure.
fn summary_state(case: &str, status: u16, body: &str) -> String {
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status}: {body}"
    );
    element(body, "SummaryState")
}

/// RULE: an `EntityArn` naming a delegation request the account does not hold
/// references "a resource entity that does not exist", so the read is refused with
/// the modelled `NoSuchEntityException` (HTTP 404, awsQuery code `NoSuchEntity`)
/// whose message names the request asked for.
#[tokio::test]
async fn a_delegation_request_the_account_does_not_hold_is_refused_as_a_missing_entity() {
    for delegation_request_id in delegation_request_ids() {
        let (router, engine) = router_with_engine();
        // A row where the account holds ANOTHER delegation request, so what is
        // asserted is the lookup of the identifier the ARN carries rather than
        // whether any request exists at all.
        seed_delegation_request(&engine, "held0123456789ab");

        let arn = state::delegation_request_arn(&delegation_request_id);
        for locale in locales() {
            let case = format!("{arn} locale {locale:?}");
            let (status, body) = summary(&router, &arn, locale.as_deref()).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {case:?}: expected NoSuchEntity, got {:?}: {body}",
                error_code(&body)
            );
            // "The error message describes the resource": the entity type that was
            // looked for, then the identifier the request named.
            assert_eq!(
                element(&body, "Message"),
                format!("DelegationRequest not found: {delegation_request_id}"),
                "case {case:?}: the refusal must describe the delegation request \
                 asked for, got {:?}",
                element(&body, "Message")
            );
        }

        // The refused read wrote nothing: the account holds exactly the request it
        // held before.
        assert_eq!(
            held_delegation_requests(&engine),
            vec!["held0123456789ab".to_string()],
            "case {delegation_request_id:?}: a refused read persists nothing, got {:?}",
            held_delegation_requests(&engine)
        );
    }

    // And the same refusal for an ARN that carries ANOTHER account's id: the account
    // holds the identifier, but not the request that ARN names.
    let (router, engine) = router_with_engine();
    let held = "crossaccount01234";
    seed_delegation_request(&engine, held);
    for foreign_account in ["999999999999", "000000000001"] {
        let arn = format!("arn:aws:iam::{foreign_account}:delegation-request/{held}");
        let (status, body) = summary(&router, &arn, Some("en")).await;
        assert_eq!(
            status, 404,
            "case {arn:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {arn:?}: another account's ARN names no request this account holds, \
             got {:?}: {body}",
            error_code(&body)
        );
    }
    // The ARN of the account's OWN request still resolves, so the account check did
    // not break the lookup.
    let (status, body) = summary(&router, &state::delegation_request_arn(held), None).await;
    let reported = summary_state("the account's own request", status, &body);
    assert_eq!(
        reported, "NOT_AVAILABLE",
        "the account's own request is still read, got {reported:?}: {body}"
    );
}

/// RULE: a delegation request the account HOLDS is an entity of the one type a
/// summary is served for, so the read reports the state of that request's summary
/// generation — and, while that state is not `AVAILABLE`, no summary content —
/// beside the locale the request asked for.
#[tokio::test]
async fn a_held_delegation_request_reports_the_state_of_its_summary_generation() {
    for delegation_request_id in delegation_request_ids() {
        let (router, engine) = router_with_engine();
        let arn = seed_delegation_request(&engine, &delegation_request_id);

        for locale in locales() {
            let case = format!("{arn} locale {locale:?}");
            let (status, body) = summary(&router, &arn, locale.as_deref()).await;
            let reported = summary_state(&case, status, &body);
            // Nothing has generated a summary for this request, which is what
            // `NOT_AVAILABLE` reports: the state is one of the four
            // `summaryStateType` declares, and it is not `AVAILABLE`.
            assert_eq!(
                reported, "NOT_AVAILABLE",
                "case {case:?}: expected the state of a summary nothing generated, \
                 got {reported:?}: {body}"
            );
            assert!(
                elements(&body, "SummaryContent").is_empty(),
                "case {case:?}: content is non-empty only for an AVAILABLE summary, \
                 got {:?}",
                elements(&body, "SummaryContent")
            );
            // "The locale that this response was generated for. This maps to the
            // input locale."
            assert_eq!(
                elements(&body, "Locale"),
                locale.iter().cloned().collect::<Vec<String>>(),
                "case {case:?}: expected the response to map the input locale, got \
                 {:?}",
                elements(&body, "Locale")
            );
        }
    }
}

/// RULE: this read summarizes exactly one kind of entity — "at this time, the only
/// supported entity type is delegation-request" — so an `EntityArn` that names
/// anything else is a value whose form is wrong for that member and is refused as a
/// validation failure (HTTP 400, awsQuery code `ValidationError`) whose message says
/// the resource type is what was wrong,
/// whether or not the account holds the entity it names.
#[tokio::test]
async fn an_entity_arn_of_any_other_kind_is_refused_as_a_validation_failure() {
    let router = router();
    let account = account_id();

    // Rows: one per entity kind this account can create, each HELD at the moment it
    // is asked about, so the refusal is about the entity's KIND and not about it
    // being absent.
    let (status, body) = call(&router, "CreateUser", &[("UserName", "summarized")]).await;
    assert_eq!(status, 200, "CreateUser must succeed, got {status}: {body}");
    let user_arn = element(&body, "Arn");

    let (status, body) = call(
        &router,
        "CreateRole",
        &[
            ("RoleName", "SummarizedRole"),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
            ("Path", "/eng/"),
        ],
    )
    .await;
    assert_eq!(status, 200, "CreateRole must succeed, got {status}: {body}");
    let role_arn = element(&body, "Arn");

    let (status, body) = call(&router, "CreateGroup", &[("GroupName", "Summarized")]).await;
    assert_eq!(status, 200, "CreateGroup must succeed, got {status}: {body}");
    let group_arn = element(&body, "Arn");

    let (status, body) = call(
        &router,
        "CreatePolicy",
        &[
            ("PolicyName", "SummarizedPolicy"),
            ("PolicyDocument", CUSTOMER_DOCUMENT),
        ],
    )
    .await;
    assert_eq!(status, 200, "CreatePolicy must succeed, got {status}: {body}");
    let policy_arn = element(&body, "Arn");

    let (status, body) = call(
        &router,
        "CreateInstanceProfile",
        &[("InstanceProfileName", "SummarizedProfile")],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateInstanceProfile must succeed, got {status}: {body}"
    );
    let instance_profile_arn = element(&body, "Arn");

    let held = [
        user_arn,
        role_arn,
        group_arn,
        policy_arn,
        instance_profile_arn,
    ];
    // And the same ARNs for entities nobody created, plus a policy in AWS's own
    // namespace and a resource of another service entirely: the refusal does not
    // turn on whether the entity exists, because its kind is already wrong.
    let other_kinds = [
        format!("arn:aws:iam::{account}:user/never-created"),
        format!("arn:aws:iam::{account}:user/eng/tools/never-created"),
        format!("arn:aws:iam::{account}:group/NeverCreated"),
        format!("arn:aws:iam::{account}:role/NeverCreated"),
        format!("arn:aws:iam::{account}:policy/NeverCreated"),
        format!("arn:aws:iam::{account}:instance-profile/NeverCreated"),
        format!("arn:aws:iam::{account}:saml-provider/NeverCreated"),
        format!("arn:aws:iam::{account}:oidc-provider/example.com/never"),
        "arn:aws:iam::aws:policy/ReadOnlyAccess".to_string(),
        "arn:aws:s3:::never-created-bucket".to_string(),
    ];

    for arn in held.into_iter().chain(other_kinds) {
        for locale in [None, Some("en"), Some("zh-CN")] {
            let case = format!("{arn} locale {locale:?}");
            let (status, body) = summary(&router, &arn, locale).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationError",
                "case {case:?}: expected ValidationError, got {:?}: {body}",
                error_code(&body)
            );
            assert!(
                element(&body, "Message") == "Invalid resource type in ARN",
                "case {case:?}: the refusal must say the resource type is the problem, \
                 got {:?}",
                element(&body, "Message")
            );
            assert!(
                elements(&body, "SummaryState").is_empty(),
                "case {case:?}: a refused read reports no summary state, got {:?}",
                elements(&body, "SummaryState")
            );
        }
    }
}

/// RULE: a value that is not an Amazon Resource Name at all —
/// `arn:<partition>:<service>:<region>:<account>:<resource>` — names no resource in
/// any service, so it too is refused as the same validation failure rather
/// than looked up.
#[tokio::test]
async fn a_value_that_is_not_an_arn_is_refused_as_a_validation_failure() {
    let router = router();
    let account = account_id();
    // Rows: no colon-separated structure at all, each required field missing in
    // turn, a resource-less ARN, a leading literal in the wrong case, and a
    // delegation request addressed with a resource body no identifier can have.
    let not_arns = [
        "this-is-not-an-arn-at-all".to_string(),
        "arn/aws/iam/123456789012/delegation-request".to_string(),
        format!("arn:aws:iam::{account}"),
        format!("arn:aws:iam::{account}:"),
        format!(":aws:iam::{account}:delegation-request/abcdef0123456789"),
        format!("arn::iam::{account}:delegation-request/abcdef0123456789"),
        format!("arn:aws:::{account}:delegation-request/abcdef0123456789"),
        format!("ARN:aws:iam::{account}:delegation-request/abcdef0123456789"),
        format!("arn:aws:iam::{account}:delegation-request/"),
        format!("arn:aws:iam::{account}:delegation-request/nested/abcdef0123456"),
        format!("arn:aws:iam::{account}:delegation-requests/abcdef0123456789"),
    ];

    for value in not_arns {
        let case = value.clone();
        let (status, body) = summary(&router, &value, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {case:?}: expected ValidationError, got {:?}: {body}",
            error_code(&body)
        );
    }
}

/// RULE: a value outside the published length of its modelled type is refused before
/// the operation runs, while each published bound itself is accepted — so a value the
/// model forbids never reaches the lookup.
#[tokio::test]
async fn a_value_outside_a_published_length_is_refused() {
    let (router, engine) = router_with_engine();
    let arn = seed_delegation_request(&engine, "boundscheck012345");

    // `arnType` is length 20..2048 and `localeType` is length 2..12: one past each
    // bound of each.
    let prefix = format!("arn:aws:iam::{}:delegation-request/", account_id());
    let one_past_the_longest_arn = format!("{prefix}{}", "d".repeat(2049 - prefix.len()));
    assert_eq!(
        one_past_the_longest_arn.chars().count(),
        2049,
        "the over-long row must be one past the published 2048"
    );
    let out_of_range: [(&str, String, Option<String>); 4] = [
        (
            "an EntityArn one character short",
            "arn:aws:s3:::bucket".to_string(),
            None,
        ),
        (
            "an EntityArn one character past the maximum",
            one_past_the_longest_arn,
            None,
        ),
        (
            "a Locale one character short",
            arn.clone(),
            Some("e".to_string()),
        ),
        (
            "a Locale one character past the maximum",
            arn.clone(),
            Some("pt-BR-x-tests".to_string()),
        ),
    ];

    for (case, entity_arn, locale) in out_of_range {
        let (status, body) = summary(&router, &entity_arn, locale.as_deref()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {case:?}: expected the modelled length to be refused as invalid, \
             got {:?}: {body}",
            error_code(&body)
        );
    }

    // Each published bound itself is READ rather than rejected for its length: the
    // shortest ARN the type admits reaches the handler, whose refusal names the
    // member and the entity kind it needs, and both locale bounds reach the held
    // request.
    let shortest_arn = "arn:aws:s3:::bucket0";
    assert_eq!(
        shortest_arn.chars().count(),
        20,
        "the row at the bound must be exactly the published 20"
    );
    let (status, body) = summary(&router, shortest_arn, None).await;
    assert_eq!(
        status, 400,
        "an ARN at the published minimum length is read, got {status}: {body}"
    );
    assert!(
        element(&body, "Message") == "Invalid resource type in ARN",
        "an ARN at the published minimum length reaches the handler, got {:?}",
        element(&body, "Message")
    );
    for locale in ["en", "pt-BR-x-test"] {
        let (status, body) = summary(&router, &arn, Some(locale)).await;
        let reported = summary_state(locale, status, &body);
        assert_eq!(
            reported, "NOT_AVAILABLE",
            "case {locale:?}: a locale at a published bound is served, got {reported:?}: {body}"
        );
    }
}

/// RULE: a reporting operation changes nothing a later caller sees, however many
/// times it is called — neither the entities the account holds nor the answer the
/// next identical call gets.
#[tokio::test]
async fn reading_a_summary_changes_nothing_a_later_caller_sees() {
    let (router, engine) = router_with_engine();
    let held = seed_delegation_request(&engine, "repeated01234567");
    let missing = state::delegation_request_arn("neverCreated01234");
    let (status, body) = call(&router, "CreateUser", &[("UserName", "repeated")]).await;
    assert_eq!(status, 200, "CreateUser must succeed, got {status}: {body}");
    let user_arn = element(&body, "Arn");

    // Rows: the held delegation request, an ARN of another entity kind, and a
    // delegation request the account does not hold — every branch the read takes.
    // The `RequestId` of the response envelope is per-call, so what is compared is
    // every member the read itself answers with.
    for arn in [held, user_arn, missing] {
        let mut answers: Vec<(u16, Vec<String>)> = Vec::new();
        for read in 0..3 {
            let (status, body) = summary(&router, &arn, Some("en")).await;
            let answered: Vec<String> = ["SummaryState", "Locale", "SummaryContent", "Code"]
                .iter()
                .flat_map(|member| elements(&body, member))
                .collect();
            answers.push((status, answered));
            assert_eq!(
                answers[0], answers[read],
                "case {arn:?}: read {read} answered {:?} where the first answered {:?}",
                answers[read], answers[0]
            );
        }
    }

    // And the store still holds exactly the one delegation request that was seeded:
    // no read created, moved or dropped one.
    assert_eq!(
        held_delegation_requests(&engine),
        vec!["repeated01234567".to_string()],
        "a read persists nothing, got {:?}",
        held_delegation_requests(&engine)
    );
}
