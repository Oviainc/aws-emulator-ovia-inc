// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the Region's default-view pointer and of the CLASS each
//! refusal around views and indexes belongs to.
//!
//! Every assertion here goes over the wire: the suite mounts this service on a
//! [`MultiServiceRouter`] over ONE shared engine and dispatches restJson1 requests at
//! it, so nothing is proved about a handler that is not also proved about the serialized
//! request and response an SDK sends and receives. That matters twice over here:
//! `GetDefaultView` and `DisassociateDefaultView` both have a `smithy.api#Unit` input and
//! so carry no body at all, and the whole point of the refusal rows is which status, code
//! and message text reach the client.
//!
//! The Region of a request is the Region of its SigV4 credential scope, which is why
//! every call carries an `Authorization` header: it is the only place the wire states
//! which Region "the Region in which you call this operation" means.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Comfortably past `crate::state::INDEX_SETTLE_DELAY`, so an index left in `CREATING`
/// or `DELETING` is certain to have settled by the next call.
const PAST_SETTLE: Duration = Duration::from_millis(900);

/// AWS's wholly fixed `ConflictException` text for the view-conflict class, which is
/// what `crate::state::VIEW_CONFLICT_MESSAGE` holds: no view name, no Region, no ARN.
const VIEW_CONFLICT_MESSAGE: &str = "The request failed because either the request \
                                     parameters didn't match to the original request or \
                                     a view with the specified name already exists in \
                                     this AWS Region.";

/// One emulator: the service mounted on a router over a single shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

/// What one call answered: the HTTP status, the restJson1 error code (the
/// `x-amzn-errortype` header), and the decoded JSON body.
struct Answer {
    status: u16,
    error: Option<String>,
    body: Value,
}

impl Answer {
    /// The body of a call that had to succeed.
    fn ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} ({:?}) with body {}",
            self.status, self.error, self.body
        );
        &self.body
    }

    /// Assert this call failed with `status` and the error code `code`.
    fn failed(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            (self.status, self.error.as_deref()),
            (status, Some(code)),
            "case {case:?}: expected HTTP {status} {code}, got {} ({:?}) with body {}",
            self.status,
            self.error,
            self.body
        );
    }

    /// The error's `Message` member, however the envelope cases it.
    fn error_message(&self) -> String {
        self.body
            .get("Message")
            .or_else(|| self.body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_resourceexplorer2_resourceexplorer::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Dispatch `POST path` as a client signed for `region` would, with `body` as the
    /// payload — `None` meaning no body at all, which is what an SDK sends for an
    /// operation whose input is `smithy.api#Unit`.
    async fn send(&self, region: &str, path: &str, body: Option<Value>) -> Answer {
        let payload = match body {
            Some(value) => serde_json::to_vec(&value).expect("the request body serializes"),
            None => Vec::new(),
        };
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://resource-explorer-2.{region}.amazonaws.com{path}"
            ))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{region}\
                     /resource-explorer-2/aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .body(Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Answer {
            status,
            error,
            body,
        }
    }

    /// `POST /{operation}` with a JSON body.
    async fn call(&self, region: &str, operation: &str, body: Value) -> Answer {
        self.send(region, &format!("/{operation}"), Some(body))
            .await
    }

    /// `POST /GetDefaultView`, carrying NO body: the input is `smithy.api#Unit`.
    async fn get_default_view(&self, region: &str) -> Answer {
        self.send(region, "/GetDefaultView", None).await
    }

    /// `POST /DisassociateDefaultView`, carrying NO body, for the same reason.
    async fn disassociate_default_view(&self, region: &str) -> Answer {
        self.send(region, "/DisassociateDefaultView", None).await
    }
}

/// `CreateIndex` in `region`, asserting success.
async fn create_index(emulator: &Emulator, region: &str, token: &str) -> String {
    let answer = emulator
        .call(region, "CreateIndex", json!({ "ClientToken": token }))
        .await;
    answer.ok(region)["Arn"]
        .as_str()
        .expect("CreateIndex answers an Arn")
        .to_string()
}

/// `CreateView` in `region`, asserting success, and the view ARN it answered with.
async fn create_view(emulator: &Emulator, region: &str, name: &str) -> String {
    let answer = emulator
        .call(region, "CreateView", json!({ "ViewName": name }))
        .await;
    answer.ok(name)["View"]["ViewArn"]
        .as_str()
        .expect("CreateView answers a View.ViewArn")
        .to_string()
}

/// The `ViewArn` a `GetDefaultView` answer carries, or `None` when the member is absent.
///
/// The ABSENCE is the point: the member is optional in the model, so a Region with no
/// default is reported by omitting it rather than by an error, and a caller reads it
/// exactly this way.
fn reported_default(answer: &Answer, case: &str) -> Option<String> {
    let body = answer.ok(case);
    assert!(
        body.is_object(),
        "case {case:?}: GetDefaultView answers an object, got {body}"
    );
    match body.get("ViewArn") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: ViewArn is not a string, got {body}"))
                .to_string(),
        ),
    }
}

/// The set of view ARNs `ListViews` reports for `region`.
async fn listed_views(emulator: &Emulator, region: &str, case: &str) -> BTreeSet<String> {
    let answer = emulator.call(region, "ListViews", json!({})).await;
    answer.ok(case)["Views"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|arn| arn.as_str().map(str::to_string))
        .collect()
}

// ---------------------------------------------------------------------------
// The Region's default-view pointer
// ---------------------------------------------------------------------------

/// The default view is a per-Region POINTER: reading it reports the ARN the most recent
/// association stored, reading a Region that has none succeeds with the member absent,
/// and clearing it is a success whether or not there was one to clear.
///
/// The rows span Regions and, within each Region, several views, so the answer is shown
/// to be keyed on the request's Region and on the last association rather than on one
/// value. Both wire forms of the `smithy.api#Unit` input are exercised: no body at all,
/// and the empty JSON object.
#[tokio::test]
async fn the_default_view_pointer_is_per_region_and_optional() {
    let emulator = Emulator::new();
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];

    // Nothing has been associated anywhere: every Region answers 200 with no ViewArn.
    for region in regions {
        for (case, body) in [("no body", None), ("an empty JSON object", Some(json!({})))] {
            let case = format!("{region} before any association, {case}");
            let answer = emulator.send(region, "/GetDefaultView", body).await;
            assert_eq!(
                reported_default(&answer, &case),
                None,
                "case {case:?}: a Region with no default view reports none, got {}",
                answer.body
            );
        }
        // Clearing a Region that has no default is a no-op success, twice over.
        for attempt in ["first", "second"] {
            let case = format!("{region} disassociate with no default, {attempt}");
            emulator.disassociate_default_view(region).await.ok(&case);
            let answer = emulator.get_default_view(region).await;
            assert_eq!(
                reported_default(&answer, &case),
                None,
                "case {case:?}: clearing nothing cannot create a default, got {}",
                answer.body
            );
        }
    }

    // One Region at a time, so a pointer stored for one Region is shown not to leak into
    // another. Within a Region each association OVERWRITES the previous one.
    let mut per_region: Vec<(&str, Vec<String>)> = Vec::new();
    for region in regions {
        create_index(&emulator, region, "default-view-pointer").await;
        let views = vec![
            create_view(&emulator, region, "first-default").await,
            create_view(&emulator, region, "second-default").await,
            create_view(&emulator, region, "third-default").await,
        ];
        for view in &views {
            let case = format!("{region} associate {view}");
            let associated = emulator
                .call(region, "AssociateDefaultView", json!({ "ViewArn": view }))
                .await;
            assert_eq!(
                associated.ok(&case)["ViewArn"].as_str(),
                Some(view.as_str()),
                "case {case:?}: AssociateDefaultView answers the ARN it stored, got {}",
                associated.body
            );
            let answer = emulator.get_default_view(region).await;
            assert_eq!(
                reported_default(&answer, &case).as_deref(),
                Some(view.as_str()),
                "case {case:?}: the most recent association is the default, got {}",
                answer.body
            );
        }
        per_region.push((region, views));
    }

    // Every Region kept its OWN pointer: the last view associated there.
    for (region, views) in &per_region {
        let case = format!("{region} after every Region was associated");
        let answer = emulator.get_default_view(region).await;
        assert_eq!(
            reported_default(&answer, &case).as_deref(),
            views.last().map(String::as_str),
            "case {case:?}: a pointer belongs to its own Region, got {}",
            answer.body
        );
    }

    // Clearing one Region's pointer leaves every other Region's alone.
    let (cleared, cleared_views) = per_region[0].clone();
    emulator
        .disassociate_default_view(cleared)
        .await
        .ok("disassociate the first Region");
    let answer = emulator.get_default_view(cleared).await;
    assert_eq!(
        reported_default(&answer, "the cleared Region"),
        None,
        "case {cleared:?}: a cleared Region has no default view, got {}",
        answer.body
    );
    // The views themselves are untouched: only the pointer went.
    assert_eq!(
        listed_views(&emulator, cleared, "after disassociating").await,
        cleared_views.iter().cloned().collect::<BTreeSet<String>>(),
        "case {cleared:?}: clearing the default deletes no view"
    );
    for (region, views) in per_region.iter().skip(1) {
        let case = format!("{region} after another Region was cleared");
        let answer = emulator.get_default_view(region).await;
        assert_eq!(
            reported_default(&answer, &case).as_deref(),
            views.last().map(String::as_str),
            "case {case:?}: clearing one Region cannot clear another, got {}",
            answer.body
        );
    }

    // Deleting the view that IS the default leaves its Region with no default, and so
    // does deleting the Region's index, which takes every view with it.
    let (deleted_view_region, views) = per_region[1].clone();
    let default = views.last().expect("the Region has views").clone();
    emulator
        .call(
            deleted_view_region,
            "DeleteView",
            json!({ "ViewArn": default }),
        )
        .await
        .ok("delete the default view");
    let answer = emulator.get_default_view(deleted_view_region).await;
    assert_eq!(
        reported_default(&answer, "after deleting the default view"),
        None,
        "case {deleted_view_region:?}: a deleted view is nobody's default, got {}",
        answer.body
    );

    let (deleted_index_region, _) = per_region[2].clone();
    let index = emulator
        .call(deleted_index_region, "GetIndex", json!({}))
        .await;
    let index_arn = index.ok("read the index to delete")["Arn"]
        .as_str()
        .expect("GetIndex answers an Arn")
        .to_string();
    emulator
        .call(
            deleted_index_region,
            "DeleteIndex",
            json!({ "Arn": index_arn }),
        )
        .await
        .ok("delete the index");
    tokio::time::sleep(PAST_SETTLE).await;
    let answer = emulator.get_default_view(deleted_index_region).await;
    assert_eq!(
        reported_default(&answer, "after the index was deleted"),
        None,
        "case {deleted_index_region:?}: deleting the index takes the default with the \
         views, got {}",
        answer.body
    );
}

// ---------------------------------------------------------------------------
// CreateView: a name outside the modelled pattern is an authorization failure
// ---------------------------------------------------------------------------

/// A `ViewName` the modelled `@pattern` does not admit cannot appear in a view ARN any
/// policy can match, so the refusal is the authorization class — 401
/// `UnauthorizedException` with the message `Unauthorized` — and no view is stored.
///
/// The pattern is `^[a-zA-Z0-9\-]{1,64}$` (smithy_query: shape `ViewName`), so the rows
/// walk what it rules out: both length bounds overshot, and every character class it
/// excludes. The bounds themselves (1 and 64 characters) and a name the issue never
/// cited are served, in the same Region, after each refusal.
#[tokio::test]
async fn create_view_refuses_a_name_outside_the_modelled_pattern_as_unauthorized() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    create_index(&emulator, region, "view-name-pattern").await;

    let rejected: Vec<(&str, String)> = vec![
        ("empty, one short of the minimum", String::new()),
        ("one character past the maximum", "a".repeat(65)),
        ("far past the maximum", format!("rex-{}", "x".repeat(96))),
        ("a space", "has a space".to_string()),
        ("an underscore", "under_score".to_string()),
        ("a dot", "dotted.name".to_string()),
        ("a slash", "slashed/name".to_string()),
        ("a colon", "colon:name".to_string()),
        ("a non-ASCII letter", "cafè-view".to_string()),
        ("leading and trailing whitespace", " padded ".to_string()),
    ];
    for (case, name) in &rejected {
        let answer = emulator
            .call(region, "CreateView", json!({ "ViewName": name }))
            .await;
        answer.failed(case, 401, "UnauthorizedException");
        assert_eq!(
            answer.error_message(),
            "Unauthorized",
            "case {case:?}: the refusal carries nothing from the request, got {}",
            answer.body
        );
        let rendered = answer.body.to_string();
        assert!(
            !rendered.contains("viewName") && !rendered.contains("pattern"),
            "case {case:?}: an authorization failure reports no parameter detail, got \
             {rendered}"
        );
        assert_eq!(
            listed_views(&emulator, region, case).await,
            BTreeSet::new(),
            "case {case:?}: a refused CreateView must store nothing"
        );
    }

    // Each published bound of the pattern, plus a compliant name the refusals above did
    // not touch: all served in the same Region.
    let served = ["a", &"b".repeat(64), "Mixed-Case-42"];
    let mut stored = BTreeSet::new();
    for name in served {
        stored.insert(create_view(&emulator, region, name).await);
    }
    assert_eq!(
        listed_views(&emulator, region, "after the compliant names").await,
        stored,
        "a compliant name is still served in a Region whose refusals stored nothing"
    );
}

// ---------------------------------------------------------------------------
// An unresolvable view ARN is an authorization failure
// ---------------------------------------------------------------------------

/// A `ViewArn` that resolves to no view this account holds is refused with the modelled
/// 401 `UnauthorizedException` and the message `Unauthorized`, by every operation that
/// names a view ARN and models that error — the ARN cannot be matched, so the answer
/// says nothing about whether such a view exists.
///
/// The rows span every way an ARN can fail to resolve: a view that was deleted, a view
/// whose Region's index was deleted, identifiers never minted, another Region, another
/// account, and values that are not ARNs at all.
#[tokio::test]
async fn an_unresolvable_view_arn_is_refused_as_unauthorized() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let other = "eu-west-1";
    create_index(&emulator, region, "unresolvable-views").await;
    create_index(&emulator, other, "unresolvable-views").await;
    let held = create_view(&emulator, region, "still-held").await;

    let deleted = create_view(&emulator, region, "deleted-view").await;
    emulator
        .call(region, "DeleteView", json!({ "ViewArn": deleted.clone() }))
        .await
        .ok("delete a view to unresolve its ARN");

    // A view whose Region's index was deleted goes with the index.
    let orphaned = create_view(&emulator, other, "orphaned-view").await;
    let index = emulator.call(other, "GetIndex", json!({})).await;
    let index_arn = index.ok("the other Region's index")["Arn"]
        .as_str()
        .expect("GetIndex answers an Arn")
        .to_string();
    emulator
        .call(other, "DeleteIndex", json!({ "Arn": index_arn }))
        .await
        .ok("delete the other Region's index");
    tokio::time::sleep(PAST_SETTLE).await;

    let unresolvable: Vec<(&str, String)> = vec![
        ("a view just deleted", deleted),
        ("a view whose Region's index was deleted", orphaned),
        (
            "a view id never minted",
            engine::emulator_arn(
                "resource-explorer-2",
                region,
                "view/absent/0f8c2a1e-5d3b-8a47-9c61-2b7e4d905a13",
            ),
        ),
        (
            "a view in a Region with no index",
            engine::emulator_arn(
                "resource-explorer-2",
                "ap-northeast-1",
                "view/absent/3a6e9c25-1b74-8de0-95f2-8c417b6d2035",
            ),
        ),
        (
            "another account's view",
            format!(
                "arn:aws:resource-explorer-2:{region}:999999999999:view/other/\
                 5c2d8f31-7a46-8b92-ae03-1d49f6520b78"
            ),
        ),
        ("an index ARN, not a view", engine::emulator_arn("resource-explorer-2", region, "index/7d1f4b60-9e2c-8351-a4d8-6c3b05e71924")),
        ("a bare name, not an ARN", "not-even-an-arn".to_string()),
    ];

    for (case, arn) in &unresolvable {
        for (operation, body) in [
            ("GetView", json!({ "ViewArn": arn })),
            ("DeleteView", json!({ "ViewArn": arn })),
            ("Search", json!({ "QueryString": "", "ViewArn": arn })),
        ] {
            let case = format!("{operation} on {case}");
            let answer = emulator.call(region, operation, body).await;
            answer.failed(&case, 401, "UnauthorizedException");
            assert_eq!(
                answer.error_message(),
                "Unauthorized",
                "case {case:?}: the refusal carries nothing from the request, got {}",
                answer.body
            );
            assert!(
                !answer.body.to_string().contains(arn.as_str()),
                "case {case:?}: the refusal must not echo the ARN it could not match, \
                 got {}",
                answer.body
            );
        }
    }

    // None of those refusals removed anything: the view that IS held still reads back.
    let answer = emulator
        .call(region, "GetView", json!({ "ViewArn": held.clone() }))
        .await;
    assert_eq!(
        answer.ok("the held view after every refusal")["View"]["ViewArn"].as_str(),
        Some(held.as_str()),
        "a refused call must leave the store untouched, got {}",
        answer.body
    );
}

// ---------------------------------------------------------------------------
// The view-conflict class carries AWS's fixed wording
// ---------------------------------------------------------------------------

/// Both halves of the view-conflict class — a name already in use, and a replayed
/// `ClientToken` carrying different parameters — are answered with 409
/// `ConflictException` and AWS's WHOLLY FIXED text: no view name, no Region, no ARN.
///
/// The rows vary the name's form and length and the Region the conflict happens in, so
/// the message is shown to be the same text whatever the request was.
#[tokio::test]
async fn a_view_conflict_carries_the_fixed_message_for_its_class() {
    let emulator = Emulator::new();
    let regions = ["us-east-1", "eu-west-1"];
    for region in regions {
        create_index(&emulator, region, "view-conflicts").await;
    }

    let names = ["dup", "Duplicate-Name-42", &"d".repeat(64)];
    for region in regions {
        for name in names {
            let arn = create_view(&emulator, region, name).await;

            // Half one: the same name in the same Region.
            let case = format!("{region}/{name} duplicate name");
            let answer = emulator
                .call(region, "CreateView", json!({ "ViewName": name }))
                .await;
            answer.failed(&case, 409, "ConflictException");
            assert_eq!(
                answer.error_message(),
                VIEW_CONFLICT_MESSAGE,
                "case {case:?}: the view-conflict class carries AWS's fixed text, got {}",
                answer.body
            );
            let rendered = answer.body.to_string();
            assert!(
                !rendered.contains(name) && !rendered.contains(region) && !rendered.contains(&arn),
                "case {case:?}: the refusal interpolates neither the name, the Region nor \
                 the existing ARN, got {rendered}"
            );
        }
    }

    // Half two: a ClientToken replayed with different parameters. The replay that MATCHES
    // returns the view it created, so only a mismatch conflicts.
    let region = regions[0];
    let token = "a-client-token";
    let request = json!({
        "ViewName": "token-view",
        "ClientToken": token,
        "Tags": { "env": "prod" },
    });
    let first = emulator.call(region, "CreateView", request.clone()).await;
    let created = first.ok("the token's first use")["View"].clone();
    let replay = emulator.call(region, "CreateView", request).await;
    assert_eq!(
        replay.ok("the token replayed unchanged")["View"],
        created,
        "case replay: the same token and parameters return the same view, got {}",
        replay.body
    );

    // Rows: each a DIFFERENT way the replayed request fails to match the original.
    let mismatches: Vec<(&str, Value)> = vec![
        (
            "another name",
            json!({ "ViewName": "other-view", "ClientToken": token }),
        ),
        (
            "other tags",
            json!({ "ViewName": "token-view", "ClientToken": token, "Tags": { "env": "dev" } }),
        ),
        (
            "an added filter",
            json!({
                "ViewName": "token-view",
                "ClientToken": token,
                "Tags": { "env": "prod" },
                "Filters": { "FilterString": "region:us-east-1" },
            }),
        ),
        (
            "an added included property",
            json!({
                "ViewName": "token-view",
                "ClientToken": token,
                "Tags": { "env": "prod" },
                "IncludedProperties": [{ "Name": "tags" }],
            }),
        ),
    ];
    for (case, body) in mismatches {
        let answer = emulator.call(region, "CreateView", body).await;
        answer.failed(case, 409, "ConflictException");
        assert_eq!(
            answer.error_message(),
            VIEW_CONFLICT_MESSAGE,
            "case {case:?}: both halves of the class share one fixed text, got {}",
            answer.body
        );
        assert!(
            !answer.body.to_string().contains(token),
            "case {case:?}: the refusal does not echo the ClientToken, got {}",
            answer.body
        );
    }
}

// ---------------------------------------------------------------------------
// A settled index deletion is terminal, not absence
// ---------------------------------------------------------------------------

/// A deletion that has settled leaves the index record in the terminal `DELETED` state:
/// the Region holds no index any more, so `GetIndex` and `ListIndexes` report none and a
/// new index can be created — but a second `DeleteIndex` on the SAME ARN is refused with
/// the modelled 400 `ValidationException` naming that state, not with a not-found.
///
/// The rows span Regions, and each Region's second delete is repeated so the refusal is
/// shown to be stable rather than a one-off transition.
#[tokio::test]
async fn re_deleting_a_settled_index_is_refused_as_a_state_conflict() {
    let emulator = Emulator::new();
    let regions = ["us-east-1", "eu-west-1", "sa-east-1"];

    let mut arns = Vec::new();
    for region in regions {
        arns.push(create_index(&emulator, region, "terminal-index").await);
    }
    tokio::time::sleep(PAST_SETTLE).await;

    for (region, arn) in regions.iter().zip(&arns) {
        let case = format!("{region} first delete");
        let deleted = emulator
            .call(region, "DeleteIndex", json!({ "Arn": arn }))
            .await;
        assert_eq!(
            deleted.ok(&case)["State"].as_str(),
            Some("DELETING"),
            "case {case:?}: deletion is an asynchronous background task, got {}",
            deleted.body
        );
    }
    tokio::time::sleep(PAST_SETTLE).await;

    for (region, arn) in regions.iter().zip(&arns) {
        // The Region holds no index: both readers say so.
        let case = format!("{region} after the deletion settled");
        emulator
            .call(region, "GetIndex", json!({}))
            .await
            .failed(&case, 404, "ResourceNotFoundException");
        let listed = emulator.call(region, "ListIndexes", json!({})).await;
        assert_eq!(
            listed.ok(&case)["Indexes"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a deleted index is not listed, got {}",
            listed.body
        );

        // The record of the deletion is still there, in its terminal state, so deleting
        // the same ARN again is a state conflict however often it is tried.
        for attempt in ["first", "second"] {
            let case = format!("{region} re-delete, {attempt}");
            let answer = emulator
                .call(region, "DeleteIndex", json!({ "Arn": arn }))
                .await;
            answer.failed(&case, 400, "ValidationException");
            let message = answer.error_message();
            assert!(
                message.contains("DELETED"),
                "case {case:?}: the refusal names the state the record is in, got \
                 {message:?}"
            );
            assert!(
                message.contains(arn.as_str()),
                "case {case:?}: the refusal names the index it refused, got {message:?}"
            );
        }

        // An ARN no DeleteIndex ever minted is still a not-found: nothing was recorded
        // for it, which is a different thing to say.
        let never_minted = engine::emulator_arn(
            "resource-explorer-2",
            region,
            "index/7d1f4b60-9e2c-8351-a4d8-6c3b05e71924",
        );
        emulator
            .call(region, "DeleteIndex", json!({ "Arn": never_minted }))
            .await
            .failed(&format!("{region} an index never created"), 404, "ResourceNotFoundException");
    }

    // The Region is free again: a new index comes up, under a new ARN, and can itself be
    // deleted.
    for (region, old) in regions.iter().zip(&arns) {
        let fresh = create_index(&emulator, region, "after-the-terminal-index").await;
        assert_ne!(
            &fresh, old,
            "case {region:?}: a new index is a new resource, not the deleted one"
        );
        let case = format!("{region} delete the replacement");
        emulator
            .call(region, "DeleteIndex", json!({ "Arn": fresh }))
            .await
            .ok(&case);
    }
}
