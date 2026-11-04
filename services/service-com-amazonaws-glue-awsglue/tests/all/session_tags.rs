// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the tags of a Glue interactive session: what `CreateSession`
//! recorded, read back through `GetTags`.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a Glue resource whose create takes a `Tags` map carries that tag
//! set, and `GetTags` reports the tag set of the resource its `ResourceArn` names — a
//! session among them, named by the `session/<session-id>` ARN of the endpoint that created
//! it. An ARN of a session this endpoint holds none of names no entity, which is the
//! not-found refusal every other kind's miss earns.
//!
//! The rows differ along what the rule generalises over: how many tags the create carried
//! (none, one, several, the modelled maximum), what their keys and values hold, which
//! Region the session was created through, and whether the session named still exists.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, SESSION_STATUS_READY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{
    call, call_in_region, emulator, emulator_on, error_code, ok, ACCOUNT_ID, REGION,
};

/// The IAM role a session create names: `CreateSession$Role`, an `OrchestrationRoleArn`.
const ROLE: &str = "arn:aws:iam::000000000000:role/GlueSessionRole";

/// The most tags one tag set holds: "len 0..50" on `TagsMap`
/// (`smithy_query operation=GetTags`), which is the shape both `CreateSession$Tags` and
/// `GetTags$Tags` target — so a create at this bound proves the whole map is read back.
const MAX_TAGS: usize = 50;

/// Another Region's endpoint, for the rows that prove a session belongs to the Region its
/// create was served in.
const OTHER_REGION: &str = "eu-west-1";

/// The ARN of the session `id` of this account, as served in `region`: the value a tagging
/// call names a session by.
fn session_arn(region: &str, account: &str, id: &str) -> String {
    format!("arn:aws:glue:{region}:{account}:session/{id}")
}

/// Put the stored session `id` into the settled status its provisioning ends in, so a
/// delete has something to act on without this suite waiting out the clock. The way the
/// session suites reach a settled session (see `tests/all/session_deletion.rs`).
fn settle(engine: &Arc<engine::InMemoryEngine>, region: &str, id: &str) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let entry = store
        .session_mut(region, id)
        .unwrap_or_else(|| panic!("{id} was created"));
    entry.settles_at = None;
    entry.status = SESSION_STATUS_READY.to_string();
}

/// Create a session with `tags`, through `region`'s endpoint.
async fn create_session(router: &MultiServiceRouter, region: &str, id: &str, tags: &Value) {
    let mut body = json!({
        "Id": id,
        "Role": ROLE,
        "Command": { "Name": "glueetl", "PythonVersion": "3" },
    });
    if !tags.is_null() {
        body["Tags"] = tags.clone();
    }
    let (status, answer) = call_in_region(router, region, "CreateSession", &body).await;
    assert_eq!(
        status, 200,
        "CreateSession for {id} must answer 200, got {status} with {answer}"
    );
}

/// The `Tags` map of a `GetTags` answer, as an ordered map so two reads compare by content.
fn tags_of(case: &str, body: &Value) -> BTreeMap<String, String> {
    body.get("Tags")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case}: GetTags must report a Tags map, got {body}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: a tag value is a string, got {value}"))
                    .to_string(),
            )
        })
        .collect()
}

/// The same map, built from what a create sent, so the expectation is the request's own
/// value rather than a second spelling of it.
fn sent_tags(tags: &Value) -> BTreeMap<String, String> {
    tags.as_object()
        .map(|members| {
            members
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().expect("a sent tag value is a string").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The tags a create recorded
// ---------------------------------------------------------------------------

/// `GetTags` on a session ARN reports the tag set `CreateSession` recorded for it — the
/// whole map, whatever it holds — and a session created with no tags reports the empty map
/// rather than a refusal.
#[tokio::test]
async fn session_tags_are_read_back_through_get_tags() {
    let router = emulator();

    let many: Value = Value::Object(
        (0..MAX_TAGS)
            .map(|index| (format!("key-{index}"), json!(format!("value-{index}"))))
            .collect(),
    );

    // (case, the session id, the Tags the create carried)
    let cases: Vec<(&str, &str, Value)> = vec![
        ("no tags at all", "sess-untagged", Value::Null),
        ("an explicitly empty map", "sess-empty", json!({})),
        ("one tag", "sess-one", json!({ "team": "data-eng" })),
        (
            "several tags",
            "sess-several",
            json!({ "team": "data-eng", "purpose": "exploration", "env": "dev" }),
        ),
        (
            "a tag whose value is empty",
            "sess-empty-value",
            json!({ "owner": "" }),
        ),
        (
            "keys and values outside ASCII",
            "sess-unicode",
            json!({ "équipe": "données", "目的": "分析" }),
        ),
        ("the modelled maximum of tags", "sess-max", many),
    ];

    for (case, id, tags) in &cases {
        create_session(&router, REGION, id, tags).await;
        let arn = session_arn(REGION, ACCOUNT_ID, id);
        let body = ok(&router, "GetTags", &json!({ "ResourceArn": arn })).await;
        let expected = sent_tags(tags);
        let got = tags_of(case, &body);
        assert_eq!(
            got, expected,
            "case {case}: GetTags must report the tags the create recorded; expected \
             {expected:?}, got {got:?}"
        );
    }

    // Each session carries its OWN tag set: reading every one back after every create has
    // landed still reports that session's tags, so no create overwrote another's.
    for (case, id, tags) in &cases {
        let arn = session_arn(REGION, ACCOUNT_ID, id);
        let body = ok(&router, "GetTags", &json!({ "ResourceArn": arn })).await;
        let got = tags_of(case, &body);
        assert_eq!(
            got,
            sent_tags(tags),
            "case {case}: a later read must report the same tag set, got {got:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A session ARN this endpoint holds nothing for
// ---------------------------------------------------------------------------

/// A session ARN naming no session of this endpoint's estate is refused with the modelled
/// `EntityNotFoundException` — not accepted as untaggable, and not answered with somebody
/// else's tags. The rows differ along which part of the ARN misses: the id, the Region, the
/// account, and a session that was deleted after it was created.
#[tokio::test]
async fn a_session_arn_this_endpoint_holds_nothing_for_is_not_found() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    create_session(&router, REGION, "sess-local", &json!({ "team": "data-eng" })).await;
    create_session(&router, OTHER_REGION, "sess-elsewhere", &json!({ "team": "eu" })).await;
    create_session(&router, REGION, "sess-doomed", &json!({ "team": "temporary" })).await;
    settle(&engine, REGION, "sess-doomed");
    ok(&router, "DeleteSession", &json!({ "Id": "sess-doomed" })).await;

    let rows: Vec<(&str, String)> = vec![
        (
            "an id nothing created",
            session_arn(REGION, ACCOUNT_ID, "sess-never-created"),
        ),
        (
            "the local id recased",
            session_arn(REGION, ACCOUNT_ID, "SESS-LOCAL"),
        ),
        (
            "a session of another Region",
            session_arn(OTHER_REGION, ACCOUNT_ID, "sess-elsewhere"),
        ),
        (
            "the local session addressed through another Region's ARN",
            session_arn(OTHER_REGION, ACCOUNT_ID, "sess-local"),
        ),
        (
            "a session of another account",
            session_arn(REGION, "999999999999", "sess-local"),
        ),
        (
            "a session that was deleted",
            session_arn(REGION, ACCOUNT_ID, "sess-doomed"),
        ),
    ];

    for (case, arn) in &rows {
        let (status, body) = call(&router, "GetTags", &json!({ "ResourceArn": arn })).await;
        assert_eq!(
            status, 400,
            "case {case}: {arn} names no session here and must be refused, got {status} with \
             {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {arn}, got {} in {body}",
            error_code(&body)
        );
        assert!(
            body.get("Tags").is_none(),
            "case {case}: a refused read reports no tags, got {body}"
        );
    }

    // The session that DOES exist still reads back, so the misses above are misses and not
    // a broken resolution.
    let body = ok(
        &router,
        "GetTags",
        &json!({ "ResourceArn": session_arn(REGION, ACCOUNT_ID, "sess-local") }),
    )
    .await;
    assert_eq!(
        tags_of("the session that exists", &body),
        sent_tags(&json!({ "team": "data-eng" })),
        "the local session's own tags must still be served, got {body}"
    );

    // The Region's own endpoint serves the session it created, which is what makes the
    // cross-Region rows above about the Region and not about the id.
    let (status, body) = call_in_region(
        &router,
        OTHER_REGION,
        "GetTags",
        &json!({ "ResourceArn": session_arn(OTHER_REGION, ACCOUNT_ID, "sess-elsewhere") }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the Region that created a session must serve its tags, got {status} with {body}"
    );
    assert_eq!(
        tags_of("the session of another Region", &body),
        sent_tags(&json!({ "team": "eu" })),
        "the other Region's session must report its own tags, got {body}"
    );
}
