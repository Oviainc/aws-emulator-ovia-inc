// SPDX-License-Identifier: CC0-1.0

//! An app's CUSTOM HEADERS: the document the API carries for them, and the headers the
//! hosting edge serves because of it.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` — through the
//! same harness the other edge suites use ([`crate::hosted_branch_site::Hosted`]): the
//! document is installed with `UpdateApp`/`CreateApp`, the site is published by a manual
//! deployment, and the site is then visited at the host the control plane reports for the
//! branch.
//!
//! The two rules under test, each over a table spanning the class it generalises over
//! rather than the one recorded document:
//!
//! 1. **`customHeaders` carries a document the service PARSES.** `App$customHeaders`
//!    "describes the custom HTTP headers for the Amplify app" and is modelled as a plain
//!    string (`smithy_query(query="shape",shape="CustomHeaders")`), so the member carries
//!    a document — a list of rules, each a path pattern and the headers to serve for it —
//!    written in YAML, of which JSON is a subset. What every reader is answered with is
//!    the rule list the service read, as JSON: the outer `customHeaders` key is not part
//!    of it, and the document a reader is answered with declares the same rules when it
//!    is submitted again.
//! 2. **A served response carries the headers the matching rules state.** Every rule
//!    whose pattern matches the path contributes its `key`/`value` pairs — `*` matches
//!    within one path segment and `**` across them — in addition to the file's own media
//!    type, and where two matching rules name the same header the later rule's value is
//!    served. Before a document is configured no such header appears.

use serde_json::{json, Value};

use crate::app_branch_lifecycle::{string_member, Emulator};
use crate::hosted_branch_site::{Hosted, SUCCEED};

/// The document the edge cases install: a site-wide rule, a rule for one directory's
/// stylesheets that restates one of the site-wide headers, and a rule for one exact path.
///
/// Written in YAML under the outer `customHeaders` key, which is how the service's own
/// documentation writes it.
const DOCUMENT: &str = "customHeaders:\n  \
                        - pattern: '**/*'\n    headers:\n      \
                        - key: 'Strict-Transport-Security'\n        value: 'max-age=31536000'\n      \
                        - key: 'X-Scope'\n        value: 'site'\n  \
                        - pattern: '/styles/*.css'\n    headers:\n      \
                        - key: 'X-Scope'\n        value: 'styles'\n      \
                        - key: 'Cache-Control'\n        value: 'public, max-age=3600'\n  \
                        - pattern: '/data/config.json'\n    headers:\n      \
                        - key: 'X-Exact'\n        value: 'config'\n";

/// The rule list [`DOCUMENT`] declares, which is what every reader of `customHeaders` is
/// answered with.
fn declared_rules() -> Value {
    json!([
        {
            "pattern": "**/*",
            "headers": [
                { "key": "Strict-Transport-Security", "value": "max-age=31536000" },
                { "key": "X-Scope", "value": "site" },
            ],
        },
        {
            "pattern": "/styles/*.css",
            "headers": [
                { "key": "X-Scope", "value": "styles" },
                { "key": "Cache-Control", "value": "public, max-age=3600" },
            ],
        },
        {
            "pattern": "/data/config.json",
            "headers": [{ "key": "X-Exact", "value": "config" }],
        },
    ])
}

/// The files the deployment every edge case visits publishes.
fn site() -> Vec<(&'static str, &'static [u8])> {
    vec![
        ("index.html", b"<html>home</html>" as &[u8]),
        ("styles/main.css", b"body{color:red}" as &[u8]),
        ("styles/deep/nested.css", b"body{color:blue}" as &[u8]),
        ("assets/js/app.js", b"console.log(1)" as &[u8]),
        ("data/config.json", br#"{"a":1}"# as &[u8]),
    ]
}

/// The `customHeaders` member of `app_id`, as a reader is answered it.
async fn reported_document(emulator: &Emulator, app_id: &str) -> String {
    string_member(&emulator.get_app(app_id).await.member("app"), "customHeaders")
}

/// Rule 1: whatever notation a document is written in, a reader is answered with the
/// rules it declares, as JSON.
#[tokio::test]
async fn custom_headers_is_reported_as_the_json_of_the_rules_it_declares() {
    let emulator = Emulator::new();

    // Each row: the case, the document submitted, and the JSON a reader must be answered
    // with — `Value::Null` for a document the service cannot read as a rule list, which
    // is kept exactly as submitted.
    let rows: Vec<(&str, String, Value)> = vec![
        ("the YAML document under its outer key", DOCUMENT.to_string(), declared_rules()),
        (
            "one rule with one header",
            "customHeaders:\n  - pattern: '**/*'\n    headers:\n      - key: 'X-One'\n        \
             value: 'yes'\n"
                .to_string(),
            json!([{ "pattern": "**/*", "headers": [{ "key": "X-One", "value": "yes" }] }]),
        ),
        (
            "a rule declaring no header at all",
            "customHeaders:\n  - pattern: '**'\n".to_string(),
            json!([{ "pattern": "**", "headers": [] }]),
        ),
        (
            "scalars written unquoted",
            "customHeaders:\n  - pattern: /*.json\n    headers:\n      - key: X-Max-Age\n        \
             value: 31536000\n"
                .to_string(),
            json!([{
                "pattern": "/*.json",
                "headers": [{ "key": "X-Max-Age", "value": "31536000" }],
            }]),
        ),
        (
            "the JSON form a reader is answered with",
            serde_json::to_string(&declared_rules()).expect("the rules serialize"),
            declared_rules(),
        ),
        (
            "the empty document",
            String::new(),
            json!(""),
        ),
        (
            "a document that is not a rule list",
            "not a custom-headers document".to_string(),
            Value::Null,
        ),
    ];

    for (case, submitted, expected) in rows {
        let app_id = emulator.app_id_of_new_app(&format!("headers-{}", case.len())).await;
        let updated = emulator
            .update_app(&app_id, &json!({ "customHeaders": submitted }))
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateApp, got {updated:?}"
        );

        let reported = reported_document(&emulator, &app_id).await;
        assert_eq!(
            reported,
            string_member(&updated.member("app"), "customHeaders"),
            "case {case:?}: UpdateApp and GetApp must report the same document"
        );
        match &expected {
            // A document the service cannot read as a rule list is kept as submitted.
            Value::Null => assert_eq!(
                reported, submitted,
                "case {case:?}: an unreadable document must be kept as submitted, got \
                 {reported:?}"
            ),
            Value::String(text) => assert_eq!(
                &reported, text,
                "case {case:?}: expected the document {text:?}, got {reported:?}"
            ),
            rules => {
                let parsed: Value = serde_json::from_str(&reported).unwrap_or_else(|why| {
                    panic!("case {case:?}: the reported document must be JSON, got {reported:?} ({why})")
                });
                assert_eq!(
                    &parsed, rules,
                    "case {case:?}: expected the rules {rules:?}, got {parsed:?}"
                );
                // Re-submitting what the reader was answered with declares the same
                // rules, so a read-modify-write round trip is stable.
                let again = emulator
                    .update_app(&app_id, &json!({ "customHeaders": reported.clone() }))
                    .await;
                assert_eq!(
                    again.status, 200,
                    "case {case:?}: expected HTTP 200 from the second UpdateApp, got {again:?}"
                );
                assert_eq!(
                    reported_document(&emulator, &app_id).await,
                    reported,
                    "case {case:?}: re-submitting the reported document must report it \
                     unchanged"
                );
            }
        }
    }
}

/// Rule 1, on `CreateApp`: the document is read by the create as well as by the update,
/// and an app created without one carries no rules.
#[tokio::test]
async fn create_app_reads_the_document_the_same_way() {
    let emulator = Emulator::new();

    let created = emulator
        .create_app(&json!({ "name": "headers-at-create", "customHeaders": DOCUMENT }))
        .await;
    assert_eq!(
        created.status, 200,
        "expected HTTP 200 from CreateApp, got {created:?}"
    );
    let app_id = string_member(&created.member("app"), "appId");
    let reported = reported_document(&emulator, &app_id).await;
    let parsed: Value = serde_json::from_str(&reported)
        .unwrap_or_else(|why| panic!("the reported document must be JSON, got {reported:?} ({why})"));
    assert_eq!(
        parsed,
        declared_rules(),
        "case \"CreateApp\": expected the declared rules, got {parsed:?}"
    );

    let plain = emulator.app_id_of_new_app("headers-at-create-none").await;
    assert_eq!(
        reported_document(&emulator, &plain).await,
        "",
        "case \"no document\": an app created without one carries none"
    );
}

/// Rule 2: the headers a served response carries are every matching rule's.
#[tokio::test]
async fn a_served_file_carries_the_headers_its_matching_rules_state() {
    let hosted = Hosted::new("hosted-headers", &json!({ "branchName": "main" })).await;
    let settled = hosted.deploy("the whole site", &site()).await;
    assert_eq!(settled, SUCCEED, "the deployment must succeed, got {settled}");

    // Before the document is configured, no configured header appears anywhere.
    for (path, _, _) in header_cases() {
        let visited = hosted.visit(path).await;
        assert_eq!(
            visited.status, 200,
            "case {path:?}: the published path must be served, got {visited:?}"
        );
        for name in ["strict-transport-security", "x-scope", "cache-control", "x-exact"] {
            assert!(
                visited.header(name).is_none(),
                "case {path:?}: no header is configured yet, so {name:?} must be absent, \
                 got {visited:?}"
            );
        }
    }

    let installed = hosted
        .emulator
        .update_app(&hosted.app_id, &json!({ "customHeaders": DOCUMENT }))
        .await;
    assert_eq!(
        installed.status, 200,
        "expected HTTP 200 from UpdateApp, got {installed:?}"
    );

    for (path, media_type, expected) in header_cases() {
        let visited = hosted.visit(path).await;
        assert_eq!(
            visited.status, 200,
            "case {path:?}: the published path must still be served, got {visited:?}"
        );
        assert_eq!(
            visited.content_type.as_deref(),
            Some(media_type),
            "case {path:?}: the file's own media type must be answered, got {visited:?}"
        );
        for (name, value) in expected {
            assert_eq!(
                visited.header(name),
                value,
                "case {path:?}: expected the header {name:?} to be {value:?}, got {:?}",
                visited.header(name)
            );
        }
    }

    // A rule installed on the app is the app's: visiting the site neither drops nor
    // rewrites the document it was installed from.
    let reported = reported_document(&hosted.emulator, &hosted.app_id).await;
    let parsed: Value = serde_json::from_str(&reported)
        .unwrap_or_else(|why| panic!("the reported document must be JSON, got {reported:?} ({why})"));
    assert_eq!(
        parsed,
        declared_rules(),
        "a visit must leave the app's header rules as they were, got {parsed:?}"
    );

    // A document that declares no rule takes the headers away again.
    let cleared = hosted
        .emulator
        .update_app(&hosted.app_id, &json!({ "customHeaders": "" }))
        .await;
    assert_eq!(
        cleared.status, 200,
        "expected HTTP 200 from UpdateApp, got {cleared:?}"
    );
    let visited = hosted.visit("/index.html").await;
    assert!(
        visited.header("x-scope").is_none() && visited.status == 200,
        "case \"the document cleared\": a rule that is no longer configured serves no \
         header, got {visited:?}"
    );
}

/// Rule 1, at the bound: one app carries a bounded number of header rules, because the
/// stored rules are walked for every request its branches' edges serve. A document at the
/// bound is stored; one past it is refused and writes nothing.
#[tokio::test]
async fn a_document_past_the_rule_bound_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("headers-bounded").await;

    // The bound this service publishes, read from the crate rather than restated.
    let limit = service_com_amazonaws_amplify_amplify::state::MAX_CUSTOM_HEADER_RULES_PER_APP;
    let document_of = |rules: usize| {
        (0..rules).fold("customHeaders:\n".to_string(), |mut document, index| {
            document.push_str(&format!(
                "  - pattern: 'p{index}/*'\n    headers:\n      - key: 'X-R{index}'\n        \
                 value: '{index}'\n"
            ));
            document
        })
    };

    // At the bound: stored, and every rule reported.
    let at_bound = emulator
        .update_app(&app_id, &json!({ "customHeaders": document_of(limit) }))
        .await;
    assert_eq!(
        at_bound.status, 200,
        "case \"at the bound\": a document of {limit} rules must be stored, got {at_bound:?}"
    );
    let stored = reported_document(&emulator, &app_id).await;
    let parsed: Value = serde_json::from_str(&stored)
        .unwrap_or_else(|why| panic!("the reported document must be JSON, got {stored:?} ({why})"));
    assert_eq!(
        parsed.as_array().map(Vec::len),
        Some(limit),
        "case \"at the bound\": every declared rule must be reported, got {parsed:?}"
    );

    // One past it: refused, and the app still carries the document it had.
    let past_bound = emulator
        .update_app(&app_id, &json!({ "customHeaders": document_of(limit + 1) }))
        .await;
    assert_eq!(
        past_bound.status, 400,
        "case \"past the bound\": a document of {} rules must be refused, got {past_bound:?}",
        limit + 1
    );
    let message = past_bound.message().unwrap_or_default();
    assert!(
        message.contains(&format!("{}", limit + 1)) && message.contains(&app_id),
        "case \"past the bound\": the refusal must name the rules supplied and the app, \
         got {message:?}"
    );
    assert_eq!(
        reported_document(&emulator, &app_id).await,
        stored,
        "case \"past the bound\": a refused update must leave the document as it was"
    );

    // The same bound on the create, where the refusal is the modelled limit error.
    let created = emulator
        .create_app(&json!({
            "name": "headers-bounded-create",
            "customHeaders": document_of(limit + 1),
        }))
        .await;
    assert_eq!(
        created.status, 429,
        "case \"past the bound at create\": expected the modelled limit refusal, got \
         {created:?}"
    );
}

/// Each case of rule 2: the path visited, the media type of the object that answers it,
/// and the headers the response must carry — `None` for one no matching rule states.
///
/// The rows span the pattern language the document is written in: a path only the
/// site-wide rule matches, one a directory rule also matches (restating a site-wide
/// header), one a segment-bound `*` must NOT reach, an exact path, and the root (which
/// answers the site's root document).
type HeaderCase = (&'static str, &'static str, Vec<(&'static str, Option<&'static str>)>);

fn header_cases() -> Vec<HeaderCase> {
    vec![
        (
            "/index.html",
            "text/html",
            vec![
                ("strict-transport-security", Some("max-age=31536000")),
                ("x-scope", Some("site")),
                ("cache-control", None),
                ("x-exact", None),
            ],
        ),
        (
            "/styles/main.css",
            "text/css",
            vec![
                ("strict-transport-security", Some("max-age=31536000")),
                // Two rules match this path and both state `X-Scope`: the later one is
                // the value served.
                ("x-scope", Some("styles")),
                ("cache-control", Some("public, max-age=3600")),
                ("x-exact", None),
            ],
        ),
        (
            "/styles/deep/nested.css",
            "text/css",
            vec![
                ("strict-transport-security", Some("max-age=31536000")),
                // `/styles/*.css` is segment-bound, so it does not reach a nested path.
                ("x-scope", Some("site")),
                ("cache-control", None),
            ],
        ),
        (
            "/assets/js/app.js",
            "text/javascript",
            vec![
                ("strict-transport-security", Some("max-age=31536000")),
                ("x-scope", Some("site")),
                ("cache-control", None),
            ],
        ),
        (
            "/data/config.json",
            "application/json",
            vec![
                ("strict-transport-security", Some("max-age=31536000")),
                ("x-scope", Some("site")),
                ("x-exact", Some("config")),
            ],
        ),
        (
            "/",
            "text/html",
            vec![
                ("strict-transport-security", Some("max-age=31536000")),
                ("x-scope", Some("site")),
            ],
        ),
    ]
}
