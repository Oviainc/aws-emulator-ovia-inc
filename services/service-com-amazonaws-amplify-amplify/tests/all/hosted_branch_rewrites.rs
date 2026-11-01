// SPDX-License-Identifier: CC0-1.0

//! The REWRITE and REDIRECT rules an app carries (`App$customRules`), as the hosting
//! edge of its branches applies them.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` — through the
//! same harness the rest of the edge is asserted with
//! ([`crate::hosted_branch_site::Hosted`]): the rules are installed with `UpdateApp` and
//! the site is then VISITED, with no credential of any kind, at the host the control
//! plane reports for the branch.
//!
//! The rule under test: the edge resolves a request path against the owning app's rules
//! IN ORDER, and the first rule whose `source` pattern matches decides the answer, per
//! the behaviour its `status` documents
//! (`smithy_query(query="doc",shape="CustomRule",member="status")`: `200` a rewrite,
//! `301`/`302` a permanent/temporary redirect, `404` a not-found, `404-200` a rewrite of
//! what the site does not publish) — with what the `<*>` placeholder captured out of the
//! path substituted into the `target`. When no rule decides, the request is answered
//! from the file the path itself names.
//!
//! The table spans what the rule generalises over: the form of the source (literal,
//! placeholder at the end, placeholder inside the pattern), every documented status, the
//! ordering of two rules that both match, a target the deployment does not publish, a
//! target the match cannot fill, a target no answer can carry as a `Location`, and the
//! attribute-conditioned rule this edge cannot evaluate.
//!
//! How MANY rules one app carries is bounded where they are written, because the edge
//! walks them on every request (`crate::state::MAX_CUSTOM_RULES_PER_APP`), so the last
//! suite drives both sides of that bound.

use serde_json::{json, Value};

use service_com_amazonaws_amplify_amplify::state::MAX_CUSTOM_RULES_PER_APP;

use crate::app_branch_lifecycle::{string_member, Emulator};
use crate::hosted_branch_site::{Hosted, SUCCEED};

/// The site every case here is resolved against: a built web app's files, under the
/// paths a build actually deploys them at.
fn deployed_site() -> Vec<(&'static str, &'static [u8])> {
    vec![
        ("index.html", b"<html>home</html>" as &[u8]),
        ("styles/main.css", b"body{color:red}" as &[u8]),
        ("styles/sub/app.js", b"nested()" as &[u8]),
        ("scripts/app.js", b"bundled()" as &[u8]),
        ("images/logo.png", b"\x89PNG\r\n" as &[u8]),
        ("docs/guide.html", b"<html>guide</html>" as &[u8]),
    ]
}

/// What one row expects the edge to answer: the status, the body (when the answer
/// carries one), the media type (for an answer served from an object) and the
/// `Location` (for a redirect).
struct Expected {
    status: u16,
    body: Option<&'static [u8]>,
    content_type: Option<&'static str>,
    location: Option<&'static str>,
}

/// An answer served from the object at `target`'s path.
fn served(body: &'static [u8], content_type: &'static str) -> Expected {
    Expected {
        status: 200,
        body: Some(body),
        content_type: Some(content_type),
        location: None,
    }
}

/// A redirect to `location`.
fn redirected(status: u16, location: &'static str) -> Expected {
    Expected {
        status,
        body: Some(b""),
        content_type: None,
        location: Some(location),
    }
}

/// An answer for a path nothing resolves.
fn not_found() -> Expected {
    Expected {
        status: 404,
        body: None,
        content_type: None,
        location: None,
    }
}

/// Deploy [`deployed_site`] on a fresh app and branch, and answer the harness.
async fn deployed(name: &str) -> Hosted {
    let hosted = Hosted::new(name, &json!({ "branchName": "main" })).await;
    let status = hosted.deploy("the site", &deployed_site()).await;
    assert_eq!(
        status, SUCCEED,
        "app {name:?}: the deployment the rules are resolved against must succeed, got {status}"
    );
    hosted
}

/// Install `rules` on the app with `UpdateApp`, and assert the record the API reports
/// back carries exactly them: the rules the edge applies are the ones the app's record
/// says it carries.
async fn install_rules(hosted: &Hosted, case: &str, rules: &Value) {
    let updated = hosted
        .emulator
        .update_app(&hosted.app_id, &json!({ "customRules": rules }))
        .await;
    assert_eq!(
        updated.status, 200,
        "case {case:?}: expected HTTP 200 from UpdateApp, got {updated:?}"
    );
    let read_back = hosted.emulator.get_app(&hosted.app_id).await.member("app");
    assert_eq!(
        read_back.get("customRules"),
        Some(rules),
        "case {case:?}: GetApp must report the rules that were installed, got {:?}",
        read_back.get("customRules")
    );
}

/// The ordered rules the main table is resolved against. The list spans the forms a
/// `source` takes and every status the member documents.
fn table_rules() -> Value {
    json!([
        // A placeholder at the end of the pattern, substituted into the target.
        { "source": "/assets/<*>", "target": "/styles/<*>", "status": "200" },
        // A literal source, with no placeholder at all.
        { "source": "/build/app.bundle.js", "target": "/scripts/app.js", "status": "200" },
        // A placeholder INSIDE the pattern, with literal text on both sides.
        { "source": "/img/<*>.png", "target": "/images/<*>.png", "status": "200" },
        // A rewrite onto an object whose media type differs from the requested path's.
        { "source": "/mix/<*>.css", "target": "/scripts/app.js", "status": "200" },
        { "source": "/old/guide.html", "target": "/docs/guide.html", "status": "301" },
        { "source": "/temp/<*>", "target": "/docs/<*>", "status": "302" },
        { "source": "/secret/<*>", "target": "/index.html", "status": "404" },
        // A rewrite onto a path this deployment does not publish.
        { "source": "/gone/<*>", "target": "/missing.css", "status": "200" },
        // Conditioned on an attribute of the visitor, which no request carries here.
        { "source": "/geo/<*>", "target": "/styles/<*>", "status": "200", "condition": "US" },
        // Two rules matching the same request: the FIRST one decides.
        { "source": "/both/<*>", "target": "/styles/<*>", "status": "200" },
        { "source": "/both/<*>", "target": "/docs/guide.html", "status": "301" },
        // A target asking for a second capture the source never took.
        { "source": "/greedy/<*>", "target": "/docs/<*>/<*>", "status": "200" },
        // A redirect target that no answer can carry as a `Location`.
        { "source": "/crlf/<*>", "target": "/docs/<*>\r\nX-Injected: yes", "status": "301" },
        // A status the member documents none of asks for none of the five behaviours.
        { "source": "/odd/<*>", "target": "/styles/<*>", "status": "307" },
    ])
}

/// The rule, over a table spanning the source forms and every documented status.
#[tokio::test]
async fn the_edge_resolves_a_request_against_the_app_s_custom_rules() {
    let hosted = deployed("hosted-rules").await;

    // Before any rule exists, every path the rules will cover is a 404: the rules are
    // what starts mapping them onto the deployed objects.
    for path in ["/assets/main.css", "/build/app.bundle.js", "/img/logo.png"] {
        let visited = hosted.visit(path).await;
        assert_eq!(
            visited.status, 404,
            "case {path:?}: with no rule installed the path names no object, got {visited:?}"
        );
    }

    install_rules(&hosted, "the table's rules", &table_rules()).await;

    let rows: Vec<(&str, Expected)> = vec![
        ("/assets/main.css", served(b"body{color:red}", "text/css")),
        // The capture is the whole remainder, so it can carry separators.
        ("/assets/sub/app.js", served(b"nested()", "text/javascript")),
        (
            "/build/app.bundle.js",
            served(b"bundled()", "text/javascript"),
        ),
        ("/img/logo.png", served(b"\x89PNG\r\n", "image/png")),
        // The media type is the SERVED object's, not the requested path's.
        ("/mix/theme.css", served(b"bundled()", "text/javascript")),
        ("/old/guide.html", redirected(301, "/docs/guide.html")),
        ("/temp/guide.html", redirected(302, "/docs/guide.html")),
        ("/secret/key.txt", not_found()),
        ("/gone/theme.css", not_found()),
        // A rule this edge cannot evaluate decides nothing, so the path resolves as it
        // did before: the deployment publishes no `geo/` object.
        ("/geo/main.css", not_found()),
        // The first of the two matching rules wins, so this is the rewrite and not the
        // redirect that follows it.
        ("/both/main.css", served(b"body{color:red}", "text/css")),
        // A target the match cannot fill, and one that cannot be sent as a `Location`,
        // each decide nothing — the request is answered, not aborted.
        ("/greedy/guide.html", not_found()),
        ("/crlf/guide.html", not_found()),
        ("/odd/main.css", not_found()),
        // A placeholder stands for something, so the pattern does not match the
        // directory above what it captures.
        ("/assets/", not_found()),
        // A path no rule matches is still answered by the deployment itself.
        ("/styles/main.css", served(b"body{color:red}", "text/css")),
        ("/", served(b"<html>home</html>", "text/html")),
        ("/docs/guide.html", served(b"<html>guide</html>", "text/html")),
    ];

    for (path, expected) in rows {
        let visited = hosted.visit(path).await;
        assert_eq!(
            visited.status, expected.status,
            "case {path:?}: expected HTTP {}, got {visited:?}",
            expected.status
        );
        if let Some(body) = expected.body {
            assert_eq!(
                visited.body, body,
                "case {path:?}: expected the body {:?}, got {:?}",
                String::from_utf8_lossy(body),
                String::from_utf8_lossy(&visited.body)
            );
        }
        if let Some(content_type) = expected.content_type {
            assert_eq!(
                visited.content_type.as_deref(),
                Some(content_type),
                "case {path:?}: expected the media type {content_type:?}, got {visited:?}"
            );
        }
        assert_eq!(
            visited.location.as_deref(),
            expected.location,
            "case {path:?}: expected the Location {:?}, got {visited:?}",
            expected.location
        );
    }

    // A HEAD of a rewritten path is the same answer without the body.
    let head = hosted
        .visit_host("HEAD", &hosted.host, "/assets/main.css")
        .await;
    assert_eq!(
        (head.status, head.content_type.as_deref()),
        (200, Some("text/css")),
        "case \"HEAD of a rewritten path\": the rewrite answers a HEAD too, got {head:?}"
    );
    assert!(
        head.body.is_empty(),
        "case \"HEAD of a rewritten path\": a HEAD carries no body, got {head:?}"
    );

    // Visiting resolves rules; it changes nothing a later visitor sees.
    let again = hosted.visit("/assets/main.css").await;
    assert_eq!(
        (again.status, &again.body),
        (200, &b"body{color:red}".to_vec()),
        "case \"a second visit\": resolving a rule must not change the site, got {again:?}"
    );
}

/// A `404-200` rule is the catch-all: it answers only what the site does not publish.
#[tokio::test]
async fn a_404_rewrite_answers_only_the_paths_the_site_does_not_publish() {
    let hosted = deployed("hosted-spa").await;
    install_rules(
        &hosted,
        "a single-page app's catch-all",
        &json!([
            { "source": "/api/<*>", "target": "/docs/<*>", "status": "200" },
            { "source": "/<*>", "target": "/index.html", "status": "404-200" },
        ]),
    )
    .await;

    // Each row: the path, the status, and the body the edge answers with.
    let rows: Vec<(&str, u16, &[u8])> = vec![
        // Published paths answer themselves: the catch-all does not apply to them.
        ("/styles/main.css", 200, b"body{color:red}" as &[u8]),
        ("/scripts/app.js", 200, b"bundled()" as &[u8]),
        ("/", 200, b"<html>home</html>" as &[u8]),
        // A rule ahead of the catch-all still decides.
        ("/api/guide.html", 200, b"<html>guide</html>" as &[u8]),
        // Everything the site does not publish is the app's entry point, with 200.
        ("/profile", 200, b"<html>home</html>" as &[u8]),
        ("/orders/42/items", 200, b"<html>home</html>" as &[u8]),
        ("/missing.css", 200, b"<html>home</html>" as &[u8]),
    ];
    for (path, status, body) in rows {
        let visited = hosted.visit(path).await;
        assert_eq!(
            visited.status, status,
            "case {path:?}: expected HTTP {status}, got {visited:?}"
        );
        assert_eq!(
            visited.body, body,
            "case {path:?}: expected the body {:?}, got {:?}",
            String::from_utf8_lossy(body),
            String::from_utf8_lossy(&visited.body)
        );
    }
}

/// The rules the edge applies are the ones the app CURRENTLY carries, and they are that
/// app's alone.
#[tokio::test]
async fn the_edge_applies_the_rules_the_app_carries_now() {
    let hosted = deployed("hosted-rule-updates").await;
    let other = deployed("hosted-rule-neighbour").await;

    // Each row: the case, the rules the app carries after it, and what `/assets/main.css`
    // answers once they are installed.
    type Row<'a> = (&'a str, Value, u16, &'a [u8]);
    let rows: Vec<Row<'_>> = vec![
        (
            "the rule the issue's client installs",
            json!([{ "source": "/assets/<*>", "target": "/styles/<*>", "status": "200" }]),
            200,
            b"body{color:red}" as &[u8],
        ),
        (
            "a replacement list pointing the same source elsewhere",
            json!([{ "source": "/assets/<*>", "target": "/scripts/app.js", "status": "200" }]),
            200,
            b"bundled()" as &[u8],
        ),
        (
            "a list that no longer carries the rule",
            json!([{ "source": "/other/<*>", "target": "/styles/<*>", "status": "200" }]),
            404,
            b"" as &[u8],
        ),
        ("an empty list", json!([]), 404, b"" as &[u8]),
    ];

    for (case, rules, status, body) in rows {
        install_rules(&hosted, case, &rules).await;
        let visited = hosted.visit("/assets/main.css").await;
        assert_eq!(
            visited.status, status,
            "case {case:?}: expected HTTP {status} for the rewritten path, got {visited:?}"
        );
        if status == 200 {
            assert_eq!(
                visited.body, body,
                "case {case:?}: expected the body {:?}, got {:?}",
                String::from_utf8_lossy(body),
                String::from_utf8_lossy(&visited.body)
            );
        }
        // The rules are the ONE app's: the neighbour's edge, deployed the same way, is
        // not resolved against them.
        let neighbour = other.visit("/assets/main.css").await;
        assert_eq!(
            neighbour.status, 404,
            "case {case:?}: another app's edge must not apply these rules, got {neighbour:?}"
        );
    }
}

/// A rule installed with `CreateApp` is applied by the edge the same way an updated one
/// is: the edge reads the app's record, not one writer's.
#[tokio::test]
async fn a_rule_created_with_the_app_is_applied_too() {
    let emulator = Emulator::new();
    let created = emulator
        .create_app(&json!({
            "name": "hosted-created-rules",
            "customRules": [{ "source": "/assets/<*>", "target": "/styles/<*>", "status": "200" }],
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "expected HTTP 200 from CreateApp, got {created:?}"
    );
    let app_id = string_member(&created.member("app"), "appId");

    let hosted = Hosted::adopt(emulator, app_id, &json!({ "branchName": "main" })).await;
    let status = hosted.deploy("the site", &deployed_site()).await;
    assert_eq!(
        status, SUCCEED,
        "the deployment the rule is resolved against must succeed, got {status}"
    );

    let visited = hosted.visit("/assets/main.css").await;
    assert_eq!(
        (visited.status, visited.content_type.as_deref()),
        (200, Some("text/css")),
        "a rule created with the app must be applied, got {visited:?}"
    );
    assert_eq!(
        visited.body,
        b"body{color:red}",
        "expected the object the target names, got {:?}",
        String::from_utf8_lossy(&visited.body)
    );
}

/// `count` rules, each matching its own source and rewriting onto the deployed
/// stylesheet, so a list of any length is a list the edge could actually resolve.
fn rules_numbering(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| {
                json!({
                    "source": format!("/r{index}/<*>"),
                    "target": "/styles/<*>",
                    "status": "200",
                })
            })
            .collect(),
    )
}

/// How many rules one app may carry is bounded, and the bound is applied where the rules
/// are WRITTEN — so the edge never walks a list no writer accepted.
#[tokio::test]
async fn the_number_of_rules_one_app_carries_is_bounded() {
    let hosted = deployed("hosted-rule-bound").await;

    // Each row: the case, how many rules the list carries, and the status `UpdateApp`
    // answers — the bound itself, one either side of it, and the empty list.
    let rows: Vec<(&str, usize, u16)> = vec![
        ("the empty list", 0, 200),
        ("one rule", 1, 200),
        ("one under the bound", MAX_CUSTOM_RULES_PER_APP - 1, 200),
        ("exactly the bound", MAX_CUSTOM_RULES_PER_APP, 200),
        ("one over the bound", MAX_CUSTOM_RULES_PER_APP + 1, 400),
    ];
    for (case, count, status) in rows {
        let reply = hosted
            .emulator
            .update_app(
                &hosted.app_id,
                &json!({ "customRules": rules_numbering(count) }),
            )
            .await;
        assert_eq!(
            reply.status, status,
            "case {case:?}: expected HTTP {status} from UpdateApp for {count} rules, got {reply:?}"
        );
    }

    // The list the writer accepted at the bound is the one the edge resolves against, so
    // a rule in it still answers.
    let visited = hosted.visit("/r0/main.css").await;
    assert_eq!(
        (visited.status, visited.content_type.as_deref()),
        (200, Some("text/css")),
        "case \"a rule inside the accepted list\": it must still resolve, got {visited:?}"
    );

    // A create carrying more than the bound is refused by the error that operation
    // models for a quota, and the app is not created.
    let refused = hosted
        .emulator
        .create_app(&json!({
            "name": "hosted-rule-bound-create",
            "customRules": rules_numbering(MAX_CUSTOM_RULES_PER_APP + 1),
        }))
        .await;
    assert_eq!(
        refused.status, 429,
        "case \"a create over the bound\": expected HTTP 429, got {refused:?}"
    );
    assert_eq!(
        refused.error_code.as_deref(),
        Some("LimitExceededException"),
        "case \"a create over the bound\": expected the modelled limit error, got {refused:?}"
    );
    let accepted = hosted
        .emulator
        .create_app(&json!({
            "name": "hosted-rule-bound-create",
            "customRules": rules_numbering(MAX_CUSTOM_RULES_PER_APP),
        }))
        .await;
    assert_eq!(
        accepted.status, 200,
        "case \"a create at the bound\": expected HTTP 200, got {accepted:?}"
    );
}
