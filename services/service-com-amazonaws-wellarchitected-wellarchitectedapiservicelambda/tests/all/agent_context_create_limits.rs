// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the LIMITS `CreateAgentContext` holds a create to — the part
//! of this operation's modelled surface the round-trip suite beside it
//! (`super::agent_contexts`) does not reach: the profile its path names has to RESOLVE,
//! the contexts of one profile are metered, and every string the content carries is held
//! to its published `@length`.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one request:
//!
//! 1. "Creates a context associated with an optimization profile": the profile the context
//!    is associated with is RESOLVED, so a well-formed `AgentProfileArn` that names no
//!    stored profile is this operation's modelled `404 ResourceNotFoundException` carrying
//!    the ARN AS SENT and the missing resource's type, and records nothing. A value the
//!    modelled `AgentProfileArn` pattern refuses never reaches that lookup: the boundary
//!    refuses it as a `400 ValidationException` naming the member.
//! 2. Contexts are metered PER PROFILE: a profile records contexts up to the ceiling, the
//!    create past it is refused `402 ServiceQuotaExceededException` naming the profile as
//!    sent, the kind of resource the quota meters and the `@required` `QuotaCode` /
//!    `ServiceCode`, and records nothing — while a profile below its own ceiling still
//!    records contexts.
//! 3. Every `@length` clause the model puts on the strings a context carries bounds the
//!    create: `title` (`@length(1, 1000)`, `@required`), `content.industry`
//!    (`@length(..1000)`) and the three 10,000-character free-form members are accepted ON
//!    each published bound and read back as sent, and refused `400 ValidationException` one
//!    PAST it, recording nothing.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The request builders for a context live in `super::agent_contexts` (a
//! context is created by exactly one operation) and the profile the contexts hang off is
//! minted by `super::agent_goals::profile_arn_of`.

use serde_json::{json, Value};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::MAX_CONTEXTS_PER_AGENT_PROFILE;

use super::agent_contexts::{create_context, get_context, id_of, summary};
use super::agent_goals::{absent_profile_arn, profile_arn_of};
use super::workload_harness::{assert_not_found, member, Answer, Client};

/// The only value the `ContextType` enum publishes
/// (`smithy_query(query="shape", shape="ContextType")`).
const CONTEXT_TYPE: &str = "APPLICATION";

/// `ResourceNotFoundException$ResourceType` for the agent profile a create resolves, as
/// this service names it. `ExceptionResourceType` is a free-form string, so the type names
/// the kind of resource the lookup missed.
const AGENT_PROFILE_RESOURCE_TYPE: &str = "AGENT_PROFILE";

/// `ServiceQuotaExceededException$ResourceType` for a context: the kind of resource the
/// per-profile quota meters, worded the way every operation that resolves one words it.
const CONTEXT_RESOURCE_TYPE: &str = "CONTEXT";

/// `ServiceQuotaExceededException$ServiceCode` is `@required`, and the service a quota
/// failure of this API belongs to is fixed by the service itself: its `arnNamespace` /
/// signing name (`smithy_query(query="service_info")`).
const SERVICE_CODE: &str = "wellarchitected";

/// The published `@length` maximum of `ContextContent$industry`
/// (`smithy_query(query="shape", shape="ContextContent")`).
const INDUSTRY_MAX: usize = 1_000;

/// The published `@length` maximum of the free-form `ContextContent` members
/// `applicationOverview`, `architectureOverview` and `additionalContext` (same source as
/// [`INDUSTRY_MAX`]).
const FREE_FORM_MAX: usize = 10_000;

/// The published `@length` bounds of `CreateAgentContextRequest$title`
/// (`@length(1, 1000)` — `smithy_query(query="operation", operation="CreateAgentContext")`).
const TITLE_MIN: usize = 1;
/// See [`TITLE_MIN`].
const TITLE_MAX: usize = 1_000;

/// A valid `CreateAgentContext` body carrying `content` as given.
fn context_body(title: &str, content: Value) -> Value {
    json!({ "title": title, "contextType": CONTEXT_TYPE, "content": content })
}

/// Every context the store holds, across every profile: what a refused create must leave
/// untouched.
fn total_contexts(client: &Client) -> usize {
    let state = client.store();
    let store = state.lock();
    store.agent_contexts.values().map(|contexts| contexts.len()).sum()
}

/// The id of the context a create was ACCEPTED for, so a row can read it back. The modelled
/// `UUID` shape of that id and the whole summary projection are pinned by the round-trip
/// suite (`super::agent_contexts`), so what this suite asserts about an accepted create is
/// the status and what the store then reports.
fn accepted_context_id(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateAgentContext 200, got {answer:?}"
    );
    let id = id_of(summary(answer));
    assert!(
        !id.is_empty(),
        "case {case:?}: the answered context must carry its @required id, got {answer:?}"
    );
    id
}

/// Rule 1: the profile a create names has to resolve. The rows differ along HOW the ARN
/// fails to name a stored profile — the resource name, the Region, the partition and the
/// account — and then along how a value breaks the modelled pattern outright.
#[tokio::test]
async fn a_create_against_a_profile_that_is_not_stored_resolves_nothing() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "context-limits-host", 1).await;
    let present = create_context(
        &client,
        &profile_arn,
        context_body("a context of the profile that exists", json!({})),
    )
    .await;
    let present_id = accepted_context_id(&present, "the-profile-that-exists");
    let before = total_contexts(&client);

    // Well-formed `AgentProfileArn`s (the modelled
    // `^arn:aws([a-z0-9-]+)?:wellarchitected:[a-z0-9-]{6,64}:\d{12}:agent-profile/[a-zA-Z0-9_-]+$`)
    // that no stored profile carries.
    let absent: Vec<(&str, String)> = vec![
        ("absent-hex-resource", absent_profile_arn(&"0".repeat(32))),
        ("absent-name-resource", absent_profile_arn("some_profile-7")),
        ("absent-single-character", absent_profile_arn("x")),
        (
            "another-region",
            format!(
                "arn:aws:wellarchitected:ap-southeast-2:{}:agent-profile/elsewhere",
                engine::emulator_account_id()
            ),
        ),
        (
            "another-partition",
            format!(
                "arn:aws-us-gov:wellarchitected:us-gov-west-1:{}:agent-profile/elsewhere",
                engine::emulator_account_id()
            ),
        ),
        (
            "another-account",
            "arn:aws:wellarchitected:us-west-2:210987654321:agent-profile/elsewhere".to_string(),
        ),
    ];
    for (case, arn) in &absent {
        assert_ne!(
            arn, &profile_arn,
            "case {case:?}: the row must not name the profile that exists"
        );
        let answer = create_context(
            &client,
            arn,
            context_body(
                &format!("a context for {case}"),
                json!({ "regions": ["us-east-1"] }),
            ),
        )
        .await;
        assert_not_found(
            case,
            &answer,
            AGENT_PROFILE_RESOURCE_TYPE,
            arn,
            &format!("agent profile with ARN {arn}"),
        );
    }

    // Values the modelled `AgentProfileArn` pattern refuses: the boundary walk refuses
    // them before the lookup, so they are a 400 naming the member and never a 404.
    let malformed: Vec<(&str, String)> = vec![
        ("not-an-arn-at-all", "not-an-agent-profile".to_string()),
        (
            "another-service",
            format!(
                "arn:aws:iam::{}:role/NotAProfile",
                engine::emulator_account_id()
            ),
        ),
        (
            "wrong-resource-kind",
            format!(
                "arn:aws:wellarchitected:us-west-2:{}:profile/{}",
                engine::emulator_account_id(),
                "0".repeat(32)
            ),
        ),
        (
            "region-too-short",
            format!(
                "arn:aws:wellarchitected:usw2:{}:agent-profile/elsewhere",
                engine::emulator_account_id()
            ),
        ),
        (
            "account-too-short",
            "arn:aws:wellarchitected:us-west-2:12345:agent-profile/elsewhere".to_string(),
        ),
        (
            "resource-outside-its-character-set",
            format!(
                "arn:aws:wellarchitected:us-west-2:{}:agent-profile/not a profile",
                engine::emulator_account_id()
            ),
        ),
    ];
    for (case, arn) in &malformed {
        let answer = create_context(
            &client,
            arn,
            context_body(&format!("a context for {case}"), json!({})),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        assert!(
            member(&answer, "Message").contains("profileArn"),
            "case {case:?}: the refusal must name the member, got {answer:?}"
        );
    }

    assert_eq!(
        total_contexts(&client),
        before,
        "case \"unresolvable-profiles\": a refused create records nothing"
    );
    let read = get_context(&client, &profile_arn, &present_id).await;
    assert_eq!(
        read.status, 200,
        "case \"the-profile-that-exists\": the stored context must still resolve, got {read:?}"
    );
}

/// Rule 2: the per-profile context quota. Checked at the ceiling, which is accepted, one
/// past it, which is the modelled 402, and on a SECOND profile, whose own count is below
/// the ceiling — so the refusal is that profile's quota and not a global one.
#[tokio::test]
async fn the_context_quota_bounds_one_profile_and_is_metered_per_profile() {
    // The ceiling the emulator meters a profile's contexts against. Read from the constant
    // the handler reads, so what this suite asserts is the BEHAVIOUR at and past the
    // ceiling rather than the number itself.
    const QUOTA: usize = MAX_CONTEXTS_PER_AGENT_PROFILE;

    let client = Client::new();
    let host = profile_arn_of(&client, "context-quota-host", 1).await;
    let neighbour = profile_arn_of(&client, "context-quota-neighbour", 1).await;

    let mut last_id = String::new();
    for index in 1..=QUOTA {
        let case = format!("context-{index}");
        let answer = create_context(
            &client,
            &host,
            context_body(
                &format!("context {index} inside the quota"),
                json!({ "awsServices": ["lambda"] }),
            ),
        )
        .await;
        last_id = accepted_context_id(&answer, &case);
    }
    assert_eq!(
        client.store().lock().agent_context_count(&host),
        QUOTA,
        "case \"at-the-quota\": every context inside the quota must be recorded"
    );

    let past = create_context(
        &client,
        &host,
        context_body("the context one past the quota", json!({})),
    )
    .await;
    assert_eq!(
        past.status, 402,
        "case \"one-past-the-quota\": expected HTTP 402, got {past:?}"
    );
    assert_eq!(
        past.error_code, "ServiceQuotaExceededException",
        "case \"one-past-the-quota\": expected ServiceQuotaExceededException, got {past:?}"
    );
    assert_eq!(
        member(&past, "ResourceId"),
        host,
        "case \"one-past-the-quota\": ResourceId must be the profile as sent, got {past:?}"
    );
    assert_eq!(
        member(&past, "ResourceType"),
        CONTEXT_RESOURCE_TYPE,
        "case \"one-past-the-quota\": ResourceType must name what the quota meters, \
         got {past:?}"
    );
    assert_eq!(
        member(&past, "ServiceCode"),
        SERVICE_CODE,
        "case \"one-past-the-quota\": ServiceCode is @required and names this service, \
         got {past:?}"
    );
    // `QuotaCode` is `@required` too, but the model publishes no numeric Service Quotas
    // code for this quota, so what the row pins is that the member carries one rather than
    // the particular word the service chose for it.
    assert!(
        !member(&past, "QuotaCode").is_empty(),
        "case \"one-past-the-quota\": QuotaCode is @required, got {past:?}"
    );
    assert_eq!(
        client.store().lock().agent_context_count(&host),
        QUOTA,
        "case \"one-past-the-quota\": a refused create must record nothing"
    );

    // The refusal is the quota and not a broken store: the last context inside it resolves.
    let last = get_context(&client, &host, &last_id).await;
    assert_eq!(
        last.status, 200,
        "case \"at-the-quota\": the last context inside the quota must resolve, got {last:?}"
    );

    // Metered per profile: another profile, below its own ceiling, still records contexts.
    for (case, content) in [
        ("neighbour-bare-content", json!({})),
        (
            "neighbour-populated-content",
            json!({ "regions": ["eu-west-1"], "criticality": "NON_CRITICAL" }),
        ),
    ] {
        let answer = create_context(
            &client,
            &neighbour,
            context_body(&format!("a {case} context"), content),
        )
        .await;
        let id = accepted_context_id(&answer, case);
        let read = get_context(&client, &neighbour, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the neighbour's context must resolve, got {read:?}"
        );
    }
    assert_eq!(
        client.store().lock().agent_context_count(&neighbour),
        2,
        "case \"neighbour\": the quota of one profile does not meter another"
    );
}

/// Rule 3: the `@length` bounds on the strings a context carries, checked ON each published
/// bound (accepted and read back as sent) and one PAST it (refused, recording nothing).
#[tokio::test]
async fn the_context_string_length_bounds_are_enforced_on_each_published_bound() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "context-strings-host", 1).await;

    // (case, the content member the row varies — empty for the title, the value).
    let accepted: Vec<(&str, &str, String)> = vec![
        ("title-at-the-minimum", "", "t".repeat(TITLE_MIN)),
        ("title-at-the-maximum", "", "t".repeat(TITLE_MAX)),
        ("industry-at-the-maximum", "industry", "i".repeat(INDUSTRY_MAX)),
        (
            "application-overview-at-the-maximum",
            "applicationOverview",
            "a".repeat(FREE_FORM_MAX),
        ),
        (
            "architecture-overview-at-the-maximum",
            "architectureOverview",
            "r".repeat(FREE_FORM_MAX),
        ),
        (
            "additional-context-at-the-maximum",
            "additionalContext",
            "c".repeat(FREE_FORM_MAX),
        ),
    ];
    for (case, name, value) in &accepted {
        let body = if name.is_empty() {
            context_body(value, json!({}))
        } else {
            let mut content = serde_json::Map::new();
            content.insert((*name).to_string(), json!(value));
            context_body(
                &format!("a context with an {case} content"),
                Value::Object(content),
            )
        };
        let answer = create_context(&client, &profile_arn, body).await;
        let id = accepted_context_id(&answer, case);
        let read = get_context(&client, &profile_arn, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the created context must resolve, got {read:?}"
        );
        let stored = summary(&read);
        let reported = if name.is_empty() {
            stored.get("title")
        } else {
            stored.get("content").and_then(|content| content.get(*name))
        };
        assert_eq!(
            reported.and_then(Value::as_str),
            Some(value.as_str()),
            "case {case:?}: the stored value must be the one as sent, got {read:?}"
        );
    }

    // One past each published bound, the empty string where the model sets a minimum of
    // one, and each absent `@required` member.
    let stored = total_contexts(&client);
    let refused: Vec<(&str, &str, Value)> = vec![
        (
            "title-below-the-minimum",
            "title",
            context_body("", json!({})),
        ),
        (
            "title-past-the-maximum",
            "title",
            context_body(&"t".repeat(TITLE_MAX + 1), json!({})),
        ),
        (
            "industry-past-the-maximum",
            "industry",
            context_body(
                "a context past the industry maximum",
                json!({ "industry": "i".repeat(INDUSTRY_MAX + 1) }),
            ),
        ),
        (
            "application-overview-past-the-maximum",
            "applicationOverview",
            context_body(
                "a context past the applicationOverview maximum",
                json!({ "applicationOverview": "a".repeat(FREE_FORM_MAX + 1) }),
            ),
        ),
        (
            "architecture-overview-past-the-maximum",
            "architectureOverview",
            context_body(
                "a context past the architectureOverview maximum",
                json!({ "architectureOverview": "r".repeat(FREE_FORM_MAX + 1) }),
            ),
        ),
        (
            "additional-context-past-the-maximum",
            "additionalContext",
            context_body(
                "a context past the additionalContext maximum",
                json!({ "additionalContext": "c".repeat(FREE_FORM_MAX + 1) }),
            ),
        ),
        (
            "absent-required-title",
            "title",
            json!({ "contextType": CONTEXT_TYPE, "content": {} }),
        ),
        (
            "absent-required-context-type",
            "contextType",
            json!({ "title": "a context with no contextType", "content": {} }),
        ),
        (
            "absent-required-content",
            "content",
            json!({ "title": "a context with no content", "contextType": CONTEXT_TYPE }),
        ),
    ];
    for (case, name, body) in refused {
        let answer = create_context(&client, &profile_arn, body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        // Which clause fired, not merely that something was refused: the refusal names the
        // member whose value broke its published clause.
        assert!(
            member(&answer, "Message").contains(name),
            "case {case:?}: the refusal must name the member {name:?}, got {answer:?}"
        );
    }
    assert_eq!(
        total_contexts(&client),
        stored,
        "case \"refused-rows\": a refused create records nothing"
    );
}
