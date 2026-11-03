// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the browser-profile lifecycle (`CreateBrowserProfile`,
//! `GetBrowserProfile`, `DeleteBrowserProfile`).
//!
//! Every test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised. The store is the one shared
//! engine, so what one request writes is what the next request reads.

use std::sync::Arc;
use std::time::Duration;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the tests sign for.
const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`engine::mock_account_id`).
const ACCOUNT: &str = "000000000000";

/// The longest `BrowserProfileName` the model admits: the pattern
/// `^[a-zA-Z][a-zA-Z0-9_]{0,47}$` is one leading letter and up to 47 more characters
/// (`smithy_query shape=BrowserProfileName`).
const MAX_NAME_LEN: usize = 48;

/// `Description`'s published bounds, `len 1..4096` (`smithy_query
/// operation=CreateBrowserProfile`).
const MIN_DESCRIPTION_LEN: usize = 1;
const MAX_DESCRIPTION_LEN: usize = 4096;

/// The span `src/state.rs` gives a browser profile's clock-owed removal, plus slack, so
/// a test can wait the clock out.
const PAST_REMOVAL: Duration = Duration::from_millis(3_300);

/// A `clientToken` inside the modelled `ClientToken` constraints (len 33..256,
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`, `smithy_query operation=CreateBrowserProfile`).
fn client_token(seed: &str) -> String {
    let token = format!("{seed}{}", "0".repeat(40));
    token[..40].to_string()
}

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service in `region`. It is what the router
/// routes on, and what the handlers read the ARN region from.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/bedrock-agentcore/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization(REGION))
        .header("host", "bedrock-agentcore-control.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `CreateBrowserProfile`: `PUT /browser-profiles`
/// (`smithy_query operation=CreateBrowserProfile`).
async fn create_profile(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    send(router, "PUT", "/browser-profiles", Some(body)).await
}

/// `GetBrowserProfile`: `GET /browser-profiles/{profileId}`.
async fn get_profile(router: &MultiServiceRouter, profile_id: &str) -> (u16, Value) {
    send(
        router,
        "GET",
        &format!("/browser-profiles/{profile_id}"),
        None,
    )
    .await
}

/// `DeleteBrowserProfile`: `DELETE /browser-profiles/{profileId}`, with the optional
/// `clientToken` on the query string (`smithy_query operation=DeleteBrowserProfile`).
async fn delete_profile(
    router: &MultiServiceRouter,
    profile_id: &str,
    client_token: Option<&str>,
) -> (u16, Value) {
    let uri = match client_token {
        Some(token) => format!("/browser-profiles/{profile_id}?clientToken={token}"),
        None => format!("/browser-profiles/{profile_id}"),
    };
    send(router, "DELETE", &uri, None).await
}

/// The error code `restJson1` puts in `__type` / `x-amzn-errortype`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `BrowserProfileId`'s pattern, `^[a-zA-Z][a-zA-Z0-9_]{0,47}-[a-zA-Z0-9]{10}$`
/// (`smithy_query shape=BrowserProfileId`), checked by hand because this crate carries
/// no regex engine.
fn is_modelled_profile_id(id: &str, name: &str) -> bool {
    let Some(suffix) = id.strip_prefix(&format!("{name}-")) else {
        return false;
    };
    suffix.len() == 10 && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Assert one `DateTimestamp` member's wire form: a JSON STRING in the canonical RFC
/// 3339 form, naming a real instant. `createdAt` and `lastUpdatedAt` target
/// `DateTimestamp`, which carries `@timestampFormat("date-time")` (`smithy_query
/// operation=GetBrowserProfile`), so an epoch-seconds number is not the modelled type.
fn assert_date_time(case: &str, member: &str, value: &Value) {
    let rendered = value.as_str().unwrap_or_else(|| {
        panic!("case {case:?}: {member} must be an RFC 3339 date-time string, got {value:?}")
    });
    let parsed = DateTime::from_str(rendered, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {member} must parse as a date-time ({error}), got {rendered:?}")
    });
    assert!(
        parsed.secs() > 0,
        "case {case:?}: {member} must name a real instant, got {rendered:?}"
    );
}

/// A browser profile is created from the request and read back through
/// `GetBrowserProfile`, for every shape of the request `CreateBrowserProfile` accepts.
///
/// The rows differ along the members the request carries and the sizes they take — the
/// name's length and character classes, the description at each published bound of
/// `Description` (`len 1..4096`), the tag map's size, and whether an idempotency token
/// is sent — which is what the stored profile reports. A profile is `READY` from its
/// creation on: `BrowserProfileStatus` is `READY | SAVING | DELETING | DELETED`
/// (`smithy_query shape=BrowserProfileStatus`), with no creating status, so nothing is
/// owed a fresh profile; and nothing has saved session data into it yet.
#[tokio::test]
async fn a_browser_profile_is_created_and_read_back_for_every_request_shape() {
    let router = router();

    let min_description = "x".repeat(MIN_DESCRIPTION_LEN);
    let max_description = "d".repeat(MAX_DESCRIPTION_LEN);
    let longest_name = format!("n{}", "A".repeat(MAX_NAME_LEN - 1));

    // (case, name, the members beyond `name`)
    let rows: [(&str, String, Value); 6] = [
        ("only the required name", "plainProfile".to_string(), json!({})),
        (
            "the shortest description the model admits",
            "minDescribed".to_string(),
            json!({"description": min_description}),
        ),
        (
            "the longest description the model admits",
            "maxDescribed".to_string(),
            json!({"description": max_description}),
        ),
        (
            "a tagged profile",
            "taggedProfile".to_string(),
            json!({
                "description": "the shared shopping profile",
                "tags": {"owner": "retail", "environment": "test"}
            }),
        ),
        (
            "an empty tag map",
            "untaggedProfile".to_string(),
            json!({"tags": {}}),
        ),
        (
            "a one-letter name, an idempotency token and underscores nowhere",
            "p".to_string(),
            json!({"clientToken": client_token("firstShape")}),
        ),
    ];

    // The longest name the pattern admits, and one carrying the underscores and digits
    // it allows, are the same rule at its edges.
    let edge_rows: [(&str, String, Value); 2] = [
        (
            "the longest name the model admits",
            longest_name,
            json!({}),
        ),
        (
            "a name carrying underscores and digits",
            "profile_2_browser".to_string(),
            json!({"description": "second generation"}),
        ),
    ];

    for (case, name, extra) in rows.into_iter().chain(edge_rows) {
        let mut body = json!({"name": name});
        for (member, value) in extra.as_object().expect("the row's members are an object") {
            body[member] = value.clone();
        }

        let (status, created) = create_profile(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateBrowserProfile, got {status} ({created})"
        );

        let id = text(&created, "profileId");
        assert!(
            is_modelled_profile_id(&id, &name),
            "case {case:?}: expected a profileId of the form <name>-<10 alphanumerics>, got {id:?}"
        );
        assert_eq!(
            text(&created, "profileArn"),
            format!("arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:browser-profile/{id}"),
            "case {case:?}: expected the modelled BrowserProfileArn for {id}, got {}",
            text(&created, "profileArn")
        );
        assert_eq!(
            text(&created, "status"),
            "READY",
            "case {case:?}: a created profile is READY, got {}",
            text(&created, "status")
        );
        assert_date_time(case, "createdAt", &created["createdAt"]);

        let (status, fetched) = get_profile(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetBrowserProfile, got {status} ({fetched})"
        );
        assert_eq!(
            text(&fetched, "profileId"),
            id,
            "case {case:?}: GetBrowserProfile reports the profile addressed, got {}",
            text(&fetched, "profileId")
        );
        assert_eq!(
            text(&fetched, "name"),
            name,
            "case {case:?}: the name reads back as sent, got {}",
            text(&fetched, "name")
        );
        assert_eq!(
            fetched["description"],
            body.get("description").cloned().unwrap_or(Value::Null),
            "case {case:?}: the description reads back as sent, got {}",
            fetched["description"]
        );
        assert_eq!(
            text(&fetched, "profileArn"),
            text(&created, "profileArn"),
            "case {case:?}: GetBrowserProfile reports the ARN CreateBrowserProfile handed back, \
             got {}",
            text(&fetched, "profileArn")
        );
        assert_eq!(
            text(&fetched, "status"),
            "READY",
            "case {case:?}: a profile nothing has deleted or saved into stays READY, got {}",
            text(&fetched, "status")
        );
        assert_eq!(
            fetched["createdAt"], created["createdAt"],
            "case {case:?}: GetBrowserProfile reports the create instant \
             CreateBrowserProfile handed back, got {} against {}",
            fetched["createdAt"], created["createdAt"]
        );
        assert_eq!(
            fetched["lastUpdatedAt"], fetched["createdAt"],
            "case {case:?}: expected lastUpdatedAt and createdAt to be the one create instant, \
             got {} and {}",
            fetched["lastUpdatedAt"], fetched["createdAt"]
        );
        for member in ["createdAt", "lastUpdatedAt"] {
            assert_date_time(case, member, &fetched[member]);
        }
        for member in [
            "lastSavedAt",
            "lastSavedBrowserSessionId",
            "lastSavedBrowserId",
        ] {
            assert!(
                fetched[member].is_null(),
                "case {case:?}: nothing has saved session data into a fresh profile, so \
                 {member} is absent, got {}",
                fetched[member]
            );
        }

        // A read reports the stored profile without changing it, so a second read
        // answers exactly as the first did.
        let (status, reread) = get_profile(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the second GetBrowserProfile, got {status} \
             ({reread})"
        );
        assert_eq!(
            reread, fetched,
            "case {case:?}: a read does not change what the next reader sees, got {reread} \
             against {fetched}"
        );
    }
}

/// What `CreateBrowserProfile` refuses, and with which modelled error: a name this
/// account already holds, and a member outside the constraints the model publishes.
///
/// The rows differ along the rule that refuses the request, and each names the status
/// and error code a client branches on. The bounds come from `BrowserProfileName`'s
/// pattern and `Description`'s `len 1..4096` (`smithy_query
/// operation=CreateBrowserProfile`), each tested one step past its published edge.
#[tokio::test]
async fn create_browser_profile_refuses_a_duplicate_name_and_an_unmodelled_member() {
    let router = router();

    let (status, created) = create_profile(&router, &json!({"name": "taken"})).await;
    assert_eq!(
        status, 200,
        "the first create of a name succeeds, got {status} ({created})"
    );
    let taken_id = text(&created, "profileId");

    // (case, the request body, expected status, expected error code)
    let rows: [(&str, Value, u16, &str); 6] = [
        (
            "the same name again",
            json!({"name": "taken"}),
            409,
            "ConflictException",
        ),
        (
            "the same name with a different description",
            json!({"name": "taken", "description": "another profile"}),
            409,
            "ConflictException",
        ),
        (
            "a name one character past the pattern",
            json!({"name": format!("n{}", "A".repeat(MAX_NAME_LEN))}),
            400,
            "ValidationException",
        ),
        (
            "a name the pattern does not admit",
            json!({"name": "1profile"}),
            400,
            "ValidationException",
        ),
        (
            "a description one character past its length bound",
            json!({"name": "overlongDescription", "description": "d".repeat(MAX_DESCRIPTION_LEN + 1)}),
            400,
            "ValidationException",
        ),
        (
            "a description one character short of its length bound",
            json!({"name": "emptyDescription", "description": ""}),
            400,
            "ValidationException",
        ),
    ];

    for (case, body, expected_status, expected_code) in rows {
        let (status, answered) = create_profile(&router, &body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} ({answered})"
        );
        assert_eq!(
            error_code(&answered),
            expected_code,
            "case {case:?}: expected {expected_code}, got {} ({answered})",
            error_code(&answered)
        );
    }

    // A name that differs only in case is its own profile: the uniqueness rule is over
    // the name as sent.
    let (status, other) = create_profile(&router, &json!({"name": "Taken"})).await;
    assert_eq!(
        status, 200,
        "a name differing in case is its own profile, got {status} ({other})"
    );
    assert_ne!(
        text(&other, "profileId"),
        taken_id,
        "a name differing in case mints its own profile, got {}",
        text(&other, "profileId")
    );

    // The refused creates persisted nothing, and the one profile the name holds is
    // still the one the first create minted.
    let (status, fetched) = get_profile(&router, &taken_id).await;
    assert_eq!(
        status, 200,
        "the profile the first create minted is still readable, got {status} ({fetched})"
    );
    assert!(
        fetched["description"].is_null(),
        "a refused create does not overwrite the stored profile, got {}",
        fetched["description"]
    );
}

/// A `clientToken` replayed against `CreateBrowserProfile` answers with the profile it
/// first produced, and an id naming no profile is the modelled 404.
#[tokio::test]
async fn a_replayed_client_token_mints_one_profile_and_an_unknown_id_is_not_found() {
    let router = router();

    let token = client_token("idem");
    let body = json!({"name": "onceProfile", "clientToken": token});
    let (status, first) = create_profile(&router, &body).await;
    assert_eq!(
        status, 200,
        "the first create succeeds, got {status} ({first})"
    );

    // (case, the replayed body)
    let rows: [(&str, Value); 2] = [
        ("the same request again", body.clone()),
        (
            "the same token with other members changed",
            json!({"name": "onceProfile", "clientToken": token, "description": "changed"}),
        ),
    ];

    for (case, replayed) in rows {
        let (status, answered) = create_profile(&router, &replayed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a replayed token is not an error, got {status} ({answered})"
        );
        assert_eq!(
            text(&answered, "profileId"),
            text(&first, "profileId"),
            "case {case:?}: a replayed token answers with the first profile, got {}",
            text(&answered, "profileId")
        );
    }

    // The replays minted nothing, so the stored profile still carries what the FIRST
    // request asked for.
    let (status, fetched) = get_profile(&router, &text(&first, "profileId")).await;
    assert_eq!(
        status, 200,
        "the profile the token produced is readable, got {status} ({fetched})"
    );
    assert!(
        fetched["description"].is_null(),
        "a replayed token does not rewrite the stored profile, got {}",
        fetched["description"]
    );

    // A different token under the same name is a second create of a held name.
    let (status, conflicted) = create_profile(
        &router,
        &json!({"name": "onceProfile", "clientToken": client_token("other")}),
    )
    .await;
    assert_eq!(
        status, 409,
        "a fresh token does not bypass the unique-name rule, got {status} ({conflicted})"
    );

    // (case, the id addressed, expected status)
    let reads: [(&str, String, u16); 3] = [
        ("the minted id", text(&first, "profileId"), 200),
        (
            "an id no profile holds",
            "absentProfile-0123456789".to_string(),
            404,
        ),
        (
            "an id whose stem a browser, not a profile, could hold",
            "onceProfile-9876543210".to_string(),
            404,
        ),
    ];

    for (case, profile_id, expected_status) in reads {
        let (status, answered) = get_profile(&router, &profile_id).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from GetBrowserProfile, got {status} \
             ({answered})"
        );
        if expected_status == 404 {
            assert_eq!(
                error_code(&answered),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {} ({answered})",
                error_code(&answered)
            );
            assert_eq!(
                text(&answered, "message"),
                format!("Browser profile with ID {profile_id} does not exist"),
                "case {case:?}: the refusal names the profile id the request carried, got {}",
                text(&answered, "message")
            );
        }
    }
}

/// A browser and a browser profile are separate resources: one name can hold both, and
/// neither read reaches the other's store.
#[tokio::test]
async fn a_browser_profile_and_a_browser_of_one_name_are_separate_resources() {
    let router = router();

    let name = "sharedName";
    let (status, profile) = create_profile(&router, &json!({"name": name})).await;
    assert_eq!(
        status, 200,
        "the profile is created, got {status} ({profile})"
    );
    let (status, browser) = send(
        &router,
        "PUT",
        "/browsers",
        Some(&json!({"name": name, "networkConfiguration": {"networkMode": "PUBLIC"}})),
    )
    .await;
    assert_eq!(
        status, 202,
        "a browser of the same name is created, got {status} ({browser})"
    );

    let profile_id = text(&profile, "profileId");
    let browser_id = text(&browser, "browserId");
    assert_ne!(
        profile_id, browser_id,
        "the two resources mint their own ids, got {profile_id} and {browser_id}"
    );

    let (status, answered) = get_profile(&router, &browser_id).await;
    assert_eq!(
        status, 404,
        "a browser id addresses no browser profile, got {status} ({answered})"
    );
    let (status, answered) = send(&router, "GET", &format!("/browsers/{profile_id}"), None).await;
    assert_eq!(
        status, 404,
        "a profile id addresses no browser, got {status} ({answered})"
    );
}

/// A deletion reports the profile it addressed as `DELETING`, the profile reads back in
/// that status until the removal the clock owes comes due, and from then on every call
/// addressing it is the modelled 404 — and the name it held is free again.
///
/// The rows differ along what the deleted profile carries (nothing beyond its name, a
/// description, tags), how many times the delete is sent, and whether it carries the
/// optional `clientToken`: the deletion is `@idempotent` (`smithy_query
/// operation=DeleteBrowserProfile`), so a repeat while it is under way reports the same
/// profile at the same instant rather than re-arming the clock. One profile in the
/// router is never deleted, so the removal is shown to be per-profile.
#[tokio::test]
async fn a_deleted_browser_profile_reports_deleting_and_is_then_not_found() {
    let router = router();

    // The profile nobody deletes: a delete of one profile leaves it alone.
    let (status, untouched) = create_profile(&router, &json!({"name": "keptProfile"})).await;
    assert_eq!(
        status, 200,
        "the profile nobody deletes is created, got {status} ({untouched})"
    );
    let untouched_id = text(&untouched, "profileId");

    // (case, name, the members beyond `name`, how many deletes to send, the delete's
    //  clientToken)
    let rows: [(&str, &str, Value, usize, Option<String>); 4] = [
        ("only the required name", "bareProfile", json!({}), 1, None),
        (
            "a described profile",
            "describedProfile",
            json!({"description": "the shared shopping profile"}),
            1,
            None,
        ),
        (
            "a tagged profile deleted twice",
            "taggedProfile",
            json!({"tags": {"owner": "retail", "environment": "test"}}),
            2,
            None,
        ),
        (
            "deleted three times with an idempotency token",
            "tokenProfile",
            json!({"description": "deleted with a token", "clientToken": client_token("createIt")}),
            3,
            Some(client_token("deleteIt")),
        ),
    ];

    // Each row is created, deleted and read while its removal is still pending; the
    // clock is then waited out ONCE for all of them.
    let mut deleted: Vec<(&str, &str, String, Value)> = Vec::new();
    for (case, name, extra, deletes, token) in rows {
        let mut body = json!({"name": name});
        for (member, value) in extra.as_object().expect("the row's members are an object") {
            body[member] = value.clone();
        }
        let (status, created) = create_profile(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateBrowserProfile, got {status} ({created})"
        );
        let id = text(&created, "profileId");

        let mut first_delete = Value::Null;
        for attempt in 1..=deletes {
            let (status, answered) = delete_profile(&router, &id, token.as_deref()).await;
            assert_eq!(
                status, 200,
                "case {case:?} (delete {attempt}): expected HTTP 200 from DeleteBrowserProfile, \
                 got {status} ({answered})"
            );
            assert_eq!(
                text(&answered, "profileId"),
                id,
                "case {case:?} (delete {attempt}): the deletion reports the profile addressed, \
                 got {}",
                text(&answered, "profileId")
            );
            assert_eq!(
                text(&answered, "profileArn"),
                text(&created, "profileArn"),
                "case {case:?} (delete {attempt}): the deletion reports the ARN the create \
                 handed back, got {}",
                text(&answered, "profileArn")
            );
            assert_eq!(
                text(&answered, "status"),
                "DELETING",
                "case {case:?} (delete {attempt}): an accepted deletion reports DELETING, got {}",
                text(&answered, "status")
            );
            assert_date_time(case, "lastUpdatedAt", &answered["lastUpdatedAt"]);
            assert!(
                answered["lastSavedAt"].is_null(),
                "case {case:?} (delete {attempt}): no browser session saved into this profile, \
                 so lastSavedAt is absent, got {}",
                answered["lastSavedAt"]
            );
            if attempt == 1 {
                first_delete = answered;
            } else {
                assert_eq!(
                    answered, first_delete,
                    "case {case:?} (delete {attempt}): a repeated delete reports the first \
                     deletion unchanged, got {answered} against {first_delete}"
                );
            }
        }

        // The profile is still readable while its removal is pending, and reports the
        // status the deletion put it in.
        let (status, fetched) = get_profile(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a profile whose removal is still pending is readable, got {status} \
             ({fetched})"
        );
        assert_eq!(
            text(&fetched, "status"),
            "DELETING",
            "case {case:?}: a profile whose deletion is under way reports DELETING, got {}",
            text(&fetched, "status")
        );
        assert_eq!(
            fetched["lastUpdatedAt"], first_delete["lastUpdatedAt"],
            "case {case:?}: the read reports the instant the deletion recorded, got {} against {}",
            fetched["lastUpdatedAt"], first_delete["lastUpdatedAt"]
        );
        assert_eq!(
            text(&fetched, "name"),
            name,
            "case {case:?}: the stored profile is unchanged beyond its status, got {}",
            text(&fetched, "name")
        );

        deleted.push((case, name, id, first_delete));
    }

    // The profile nobody deleted is untouched by any of it.
    let (status, kept) = get_profile(&router, &untouched_id).await;
    assert_eq!(
        status, 200,
        "the profile nobody deleted is readable, got {status} ({kept})"
    );
    assert_eq!(
        text(&kept, "status"),
        "READY",
        "deleting one profile leaves another READY, got {}",
        text(&kept, "status")
    );

    tokio::time::sleep(PAST_REMOVAL).await;

    for (case, name, id, first_delete) in deleted {
        // (the call, its HTTP method and URI tail)
        let reads: [(&str, (u16, Value)); 2] = [
            ("GetBrowserProfile", get_profile(&router, &id).await),
            (
                "DeleteBrowserProfile",
                delete_profile(&router, &id, None).await,
            ),
        ];
        for (operation, (status, answered)) in reads {
            assert_eq!(
                status, 404,
                "case {case:?}: a removed profile is not found by {operation}, got {status} \
                 ({answered})"
            );
            assert_eq!(
                error_code(&answered),
                "ResourceNotFoundException",
                "case {case:?}: {operation} expected ResourceNotFoundException, got {} \
                 ({answered})",
                error_code(&answered)
            );
            assert_eq!(
                text(&answered, "message"),
                format!("Browser profile with ID {id} does not exist"),
                "case {case:?}: {operation}'s refusal names the profile id the request carried, \
                 got {}",
                text(&answered, "message")
            );
        }

        // The name the removed profile held is free again, and the create that takes it
        // mints its own profile rather than resurrecting the deleted one.
        let (status, recreated) = create_profile(&router, &json!({"name": name})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the name a removed profile held is free again, got {status} \
             ({recreated})"
        );
        let new_id = text(&recreated, "profileId");
        assert!(
            is_modelled_profile_id(&new_id, name),
            "case {case:?}: the second create mints a modelled profileId, got {new_id:?}"
        );
        assert_ne!(
            new_id, id,
            "case {case:?}: the second create mints its own profile, got {new_id}"
        );
        assert_eq!(
            text(&recreated, "status"),
            "READY",
            "case {case:?}: the profile taking the freed name is READY, got {}",
            text(&recreated, "status")
        );
        assert_ne!(
            recreated["createdAt"], first_delete["lastUpdatedAt"],
            "case {case:?}: the second create is its own instant, got {}",
            recreated["createdAt"]
        );
    }

    // And the profile nobody deleted survives the span too.
    let (status, kept) = get_profile(&router, &untouched_id).await;
    assert_eq!(
        status, 200,
        "the profile nobody deleted is still readable once the span has passed, got {status} \
         ({kept})"
    );
    assert_eq!(
        text(&kept, "status"),
        "READY",
        "a profile nobody deleted stays READY, got {}",
        text(&kept, "status")
    );
}

/// What `DeleteBrowserProfile` refuses: an id naming no profile of this account, and an
/// id outside `BrowserProfileId`'s published pattern.
///
/// The rows differ along why the request is refused — a well-formed id no profile holds,
/// an id whose stem a browser could hold, and ids one step outside the pattern
/// `^[a-zA-Z][a-zA-Z0-9_]{0,47}-[a-zA-Z0-9]{10}$` (`smithy_query shape=BrowserProfileId`).
#[tokio::test]
async fn delete_browser_profile_refuses_an_unknown_and_an_unmodelled_profile_id() {
    let router = router();

    // A profile that exists, so a refusal is about the id addressed and not an empty
    // store; it is never deleted by this test.
    let (status, created) = create_profile(&router, &json!({"name": "livingProfile"})).await;
    assert_eq!(
        status, 200,
        "the profile the refusals are measured against is created, got {status} ({created})"
    );

    // (case, the profileId addressed, expected status, expected error code)
    let rows: [(&str, &str, u16, &str); 5] = [
        (
            "an id no profile holds",
            "absentProfile-0123456789",
            404,
            "ResourceNotFoundException",
        ),
        (
            "an id whose stem a living profile holds",
            "livingProfile-9876543210",
            404,
            "ResourceNotFoundException",
        ),
        (
            "an id whose suffix is one character short of the pattern",
            "shortProfile-012345678",
            400,
            "ValidationException",
        ),
        (
            "an id whose suffix is one character past the pattern",
            "longProfile-01234567890",
            400,
            "ValidationException",
        ),
        (
            "an id the pattern does not admit at all",
            "1profile-0123456789",
            400,
            "ValidationException",
        ),
    ];

    for (case, profile_id, expected_status, expected_code) in rows {
        let (status, answered) = delete_profile(&router, profile_id, None).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from DeleteBrowserProfile, got \
             {status} ({answered})"
        );
        assert_eq!(
            error_code(&answered),
            expected_code,
            "case {case:?}: expected {expected_code}, got {} ({answered})",
            error_code(&answered)
        );
        if expected_status == 404 {
            assert_eq!(
                text(&answered, "message"),
                format!("Browser profile with ID {profile_id} does not exist"),
                "case {case:?}: the refusal names the profile id the request carried, got {}",
                text(&answered, "message")
            );
        }
    }

    // A refused delete persisted nothing: the living profile is still READY.
    let (status, fetched) = get_profile(&router, &text(&created, "profileId")).await;
    assert_eq!(
        status, 200,
        "the living profile is still readable, got {status} ({fetched})"
    );
    assert_eq!(
        text(&fetched, "status"),
        "READY",
        "a refused delete does not move another profile's status, got {}",
        text(&fetched, "status")
    );
}
