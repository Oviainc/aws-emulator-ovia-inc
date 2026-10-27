// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the modelled input constraints this service applies, and for
//! the words it states a constraint refusal in.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so routing, deserialization, the framework's own boundary walk, the
//! handler and the response envelope are all exercised rather than just a handler body.
//!
//! Three rules are pinned, each over a TABLE that spans the class rather than the one
//! member a report cited:
//!
//! 1. a value a modelled `@length` or `@pattern` rejects is refused with HTTP 400
//!    `ValidationException` and nothing is written — including where the pattern is
//!    written with look-around, which the framework's boundary walk cannot compile and so
//!    silently admits (`AppMonitorDomain`, `AppMonitorName`);
//! 2. a constraint broken by an ELEMENT of a list is stated at the LIST member, with the
//!    list rendered as the value and the element shape's whole modelled constraint set
//!    bracketed — one validation error per member, however many elements offended;
//! 3. an enum clause states the membership the model declares, in the order this service
//!    lists that enum's values.
//!
//! Every expected pattern, bound and value set below is cited from the model
//! (`smithy_query(query="shape", shape=…)` against
//! `services/service-com-amazonaws-rum-rum/smithy-model/com-amazonaws-rum-rum.json`), and
//! is spelled out here rather than read back out of the crate, so the test pins the
//! rendering instead of agreeing with it by construction.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{call, emulator, message, percent_encoded, put_metrics_destination, Reply};

/// The error code every refusal below carries: the `ValidationException` this service's
/// model declares for an app-monitor call (`@httpError(400)`).
const VALIDATION_CODE: &str = "ValidationException";

// ---------------------------------------------------------------------------------------
// The constraints the model declares, as a refusal quotes them
// ---------------------------------------------------------------------------------------

/// `AppMonitorDomain`'s `@pattern` as a refusal quotes it: the modelled expression without
/// the `^` / `$` pair the model writes around it to demand a whole-string match.
/// Source: `smithy_query(query="shape", shape="AppMonitorDomain")`.
const DOMAIN_PATTERN: &str = r"(localhost)$|^((25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.){3}(25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)$|(?=^[a-zA-Z0-9\.\*-]{4,253}$)(?!.*\.-)(?!.*-\.)(?!.*\.\.)(?!.*[^\.]{64,})^(\*\.)?(?![-\.\*])[^\*]{1,}\.(\*|(?!.*--)(?=.*[a-zA-Z])[^\*]{1,}[^\*-])";

/// `AppMonitorName`'s `@pattern`, quoted the same way.
/// Source: `smithy_query(query="shape", shape="AppMonitorName")`.
const NAME_PATTERN: &str = r"(?!\.)[\.\-_#A-Za-z0-9]+";

/// `Url`'s `@pattern` — the element shape of `Pages`, which `ExcludedPages` and
/// `IncludedPages` both target. It carries no anchor pair, so a refusal quotes it verbatim:
/// a find expression wrapped in `.*`, which is how AWS publishes it (the CloudWatch RUM API
/// reference's `Url` pattern, and the expression a recorded refusal of an excluded page
/// quotes). Source: `smithy_query(query="shape", shape="Url")`.
const URL_PATTERN: &str = r".*https?:\/\/(www\.)?[-a-zA-Z0-9@:%._\+~#=]{1,256}\.[a-zA-Z0-9()]{1,6}\b([-a-zA-Z0-9()@:%_\+.~#?&*//=]*).*";

/// The whole constraint set `AppMonitorDomain` declares, in the order a clause about a
/// `DomainList` element states them: `@length 1..253` then the `@pattern`.
/// Source: `smithy_query(query="shape", shape="AppMonitorDomainList")`, member.
fn domain_element_set() -> String {
    element_set(253, 1, DOMAIN_PATTERN)
}

/// The whole constraint set `Url` declares, in the same order: `@length 1..1260` then the
/// `@pattern`. Source: `smithy_query(query="shape", shape="Pages")`, member.
fn url_element_set() -> String {
    element_set(1260, 1, URL_PATTERN)
}

/// One element shape's constraint set as a clause brackets it.
fn element_set(max: u32, min: u32, pattern: &str) -> String {
    format!(
        "Member must satisfy constraint: [Member must have length less than or equal to {max}, \
         Member must have length greater than or equal to {min}, Member must satisfy regular \
         expression pattern: {pattern}]"
    )
}

/// The whole message a request with ONE offending member is refused with: AWS's envelope
/// over the value submitted, the member's dotted path and the requirement it failed.
fn refusal_message(value: &str, member: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
         constraint: {requirement}"
    )
}

/// The requirement a scalar `@pattern` violation states.
fn pattern_requirement(pattern: &str) -> String {
    format!("Member must satisfy regular expression pattern: {pattern}")
}

/// Assert that `reply` is the refusal `expected`, with the modelled status and code.
fn assert_refused(case: &str, reply: &Reply, expected: &str) {
    assert_eq!(
        reply.status,
        400,
        "case {case:?}: expected HTTP 400, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    assert_eq!(
        reply.error_code(case),
        VALIDATION_CODE,
        "case {case:?}: expected {VALIDATION_CODE}, got {:?}",
        String::from_utf8_lossy(&reply.body)
    );
    let body = reply.json(case);
    let stated = message(&body).unwrap_or_default();
    assert_eq!(
        stated, expected,
        "case {case:?}: expected the message {expected:?}, got {stated:?}"
    );
}

/// `CreateAppMonitor` with `body`.
async fn create_call(router: &MultiServiceRouter, body: Value) -> Reply {
    call(router, "POST", "/appmonitor", Some(body)).await
}

/// Whether `GetAppMonitor` finds a monitor under `name` — so a refused create can be shown
/// to have written nothing.
async fn exists(router: &MultiServiceRouter, name: &str) -> bool {
    call(router, "GET", &format!("/appmonitor/{name}"), None)
        .await
        .status
        == 200
}

// ---------------------------------------------------------------------------------------
// Rule 1: the modelled patterns, including the ones written with look-around
// ---------------------------------------------------------------------------------------

/// One domain row: the value, and the requirement the refusal must state — `None` when
/// `AppMonitorDomain`'s modelled constraints admit it.
struct DomainRow {
    case: &'static str,
    domain: String,
    requirement: Option<String>,
}

/// The domain table: the rows differ along what the pattern generalises over — which of its
/// three branches a value takes (the literal host, a dotted-quad address, a DNS name), the
/// wildcard form, every way a DNS name can fall outside the grammar, and the published
/// bounds on the name's total length and on one label's length, each tested at the bound
/// and one past it.
fn domain_rows() -> Vec<DomainRow> {
    let row = |case: &'static str, domain: &str, admitted: bool| DomainRow {
        case,
        domain: domain.to_string(),
        requirement: (!admitted).then(|| pattern_requirement(DOMAIN_PATTERN)),
    };
    // A name of exactly 253 characters — the `@length` ceiling and the DNS grammar's own —
    // built from labels inside the 63-character label ceiling: 63 + 63 + 63 + 57 joined by
    // three dots is 249, and `.com` takes it to 253.
    let label = "a".repeat(63);
    let longest = format!("{label}.{label}.{label}.{}.com", "a".repeat(57));
    let over_length = format!("{label}.{label}.{label}.{}.com", "a".repeat(58));
    vec![
        row("the literal host the first branch admits", "localhost", true),
        row("a dotted-quad address", "192.0.2.1", true),
        row("the largest dotted-quad address", "255.255.255.255", true),
        row("an octet with a leading zero", "01.2.3.4", true),
        row("an address with an octet past 255", "256.0.2.1", false),
        row("three octets, so neither an address nor a name", "1.2.3", false),
        row("a two-label DNS name", "example.com", true),
        row("a three-label DNS name", "shop.example.com", true),
        row("a wildcard name", "*.example.com", true),
        // The last label is `123`, which holds no letter — but the grammar's run before
        // the final dot may itself contain dots, so the name is admitted on the split at
        // the FIRST dot, whose last label `cd.123` does hold one.
        row("a name whose last label is numeric", "ab.cd.123", true),
        row("the shortest name the grammar admits (4 characters)", "a.bc", true),
        row("one character short of that bound", "a.c", false),
        row("a name with no dot at all", "nodothere", false),
        row("a name starting with a hyphen", "-not-a-valid-domain-", false),
        row("a name whose last label ends in a hyphen", "example.com-", false),
        row("a name with an empty label", "ex..ample.com", false),
        row("a name with a hyphen before a dot", "example-.com", false),
        row("a name with a dot before a hyphen", "example.-com", false),
        row("a name with a doubled hyphen in its last label", "example.co--m", false),
        // One label at the published 63-character ceiling, and one past it.
        row(
            "a label at the 63-character ceiling",
            &format!("{label}.com"),
            true,
        ),
        row(
            "a label one character past that ceiling",
            &format!("{}.com", "a".repeat(64)),
            false,
        ),
        // The published `@length` ceiling, and one character past it — which the modelled
        // `@length` refuses, so its clause states that bound rather than the pattern.
        row("a name at the 253-character @length ceiling", &longest, true),
        DomainRow {
            case: "a name one character past the @length ceiling",
            domain: over_length,
            requirement: Some(
                "Member must have length less than or equal to 253".to_string(),
            ),
        },
    ]
}

/// RULE: a `Domain` the modelled `@pattern` rejects is refused with the modelled
/// `ValidationException`, nothing is created, and one the pattern admits is stored and
/// reads back.
#[tokio::test]
async fn a_domain_outside_the_modelled_pattern_is_refused_and_nothing_is_created() {
    let (router, _engine) = emulator();

    for (index, DomainRow {
        case,
        domain,
        requirement,
    }) in domain_rows().into_iter().enumerate()
    {
        let name = format!("domain-row-{index}");
        let reply = create_call(&router, json!({"Name": name, "Domain": domain})).await;

        if let Some(requirement) = requirement {
            assert_refused(
                case,
                &reply,
                &refusal_message(&domain, "domain", &requirement),
            );
            assert!(
                !exists(&router, &name).await,
                "case {case:?}: a refused create must write nothing, but {name:?} reads back"
            );
            continue;
        }

        assert_eq!(
            reply.status,
            200,
            "case {case:?}: the modelled pattern admits {domain:?}, so the create must answer \
             HTTP 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let read = call(&router, "GET", &format!("/appmonitor/{name}"), None).await;
        assert_eq!(
            read.json(case)["AppMonitor"]["Domain"].as_str(),
            Some(domain.as_str()),
            "case {case:?}: the stored monitor must report the domain it was created with, \
             got {:?}",
            read.json(case)["AppMonitor"]["Domain"]
        );
    }
}

/// One name row: the value, and whether `AppMonitorName`'s modelled pattern admits it.
struct NameRow {
    case: &'static str,
    name: &'static str,
    admitted: bool,
}

/// The name table: every character class the pattern's class admits, and each way a name
/// can fall outside it — the leading dot its look-ahead forbids, and a character the class
/// does not list.
fn name_rows() -> Vec<NameRow> {
    vec![
        NameRow {
            case: "letters and digits",
            name: "Monitor9",
            admitted: true,
        },
        NameRow {
            case: "the punctuation the class admits",
            name: "a.b-c_d#e",
            admitted: true,
        },
        NameRow {
            case: "a single character",
            name: "a",
            admitted: true,
        },
        NameRow {
            case: "a trailing dot (only a LEADING one is forbidden)",
            name: "monitor.",
            admitted: true,
        },
        NameRow {
            case: "the leading dot the look-ahead forbids",
            name: ".monitor",
            admitted: false,
        },
        NameRow {
            case: "a name that is only a dot",
            name: ".",
            admitted: false,
        },
        NameRow {
            case: "a space, which the class does not admit",
            name: "my monitor",
            admitted: false,
        },
        NameRow {
            case: "a slash, which the class does not admit",
            name: "my/monitor",
            admitted: false,
        },
    ]
}

/// RULE: the same rule on the other shape whose pattern is written with look-around — the
/// `Name` a create gives a monitor.
#[tokio::test]
async fn a_name_outside_the_modelled_pattern_is_refused_and_nothing_is_created() {
    let (router, _engine) = emulator();

    for NameRow {
        case,
        name,
        admitted,
    } in name_rows()
    {
        let reply = create_call(&router, json!({"Name": name, "Domain": "example.com"})).await;

        if !admitted {
            assert_refused(
                case,
                &reply,
                &refusal_message(name, "name", &pattern_requirement(NAME_PATTERN)),
            );
            continue;
        }
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: the modelled pattern admits {name:?}, so the create must answer \
             HTTP 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
    }
}

/// RULE: a request that breaks TWO members is two validation errors, stated in one
/// envelope in the order the input declares them — not the first one found.
#[tokio::test]
async fn two_offending_members_are_two_validation_errors_in_one_envelope() {
    let (router, _engine) = emulator();
    let case = "a create whose Name and Domain both fall outside their patterns";
    let reply = create_call(&router, json!({"Name": ".bad", "Domain": "ex..ample.com"})).await;
    let expected = format!(
        "2 validation errors detected: Value '.bad' at 'name' failed to satisfy constraint: \
         {}; Value 'ex..ample.com' at 'domain' failed to satisfy constraint: {}",
        pattern_requirement(NAME_PATTERN),
        pattern_requirement(DOMAIN_PATTERN)
    );
    assert_refused(case, &reply, &expected);
    assert!(
        !exists(&router, ".bad").await,
        "case {case:?}: a refused create must write nothing"
    );
}

/// RULE: the domain constraints hold for the operation that REPLACES a monitor's domain
/// authority too, and a refused update leaves the stored value alone.
#[tokio::test]
async fn an_update_refuses_a_domain_outside_the_pattern_and_changes_nothing() {
    let (router, _engine) = emulator();
    let case = "UpdateAppMonitor with a domain outside the pattern";
    let name = "updated-monitor";
    let created = create_call(&router, json!({"Name": name, "Domain": "before.example.com"})).await;
    assert_eq!(
        created.status,
        200,
        "case {case:?}: the setup create must answer 200, got {} {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );

    let refused = call(
        &router,
        "PATCH",
        &format!("/appmonitor/{name}"),
        Some(json!({"Domain": "ex..ample.com"})),
    )
    .await;
    assert_refused(
        case,
        &refused,
        &refusal_message(
            "ex..ample.com",
            "domain",
            &pattern_requirement(DOMAIN_PATTERN),
        ),
    );

    let read = call(&router, "GET", &format!("/appmonitor/{name}"), None).await;
    assert_eq!(
        read.json(case)["AppMonitor"]["Domain"].as_str(),
        Some("before.example.com"),
        "case {case:?}: a refused update must leave the stored domain alone, got {:?}",
        read.json(case)["AppMonitor"]["Domain"]
    );
}

// ---------------------------------------------------------------------------------------
// Rule 2: a constraint broken by a list element is stated at the list member
// ---------------------------------------------------------------------------------------

/// One list row: the create body, the list member a clause must be reported at, the value
/// that member held, and the element shape's constraint set.
struct ListRow {
    case: &'static str,
    body: Value,
    member: &'static str,
    rendered: &'static str,
    set: String,
}

/// The list table: the rows differ along which list member carries the offence (two members
/// sharing one element shape, and a member of another), whether the list sits at the top
/// level of the input or inside a nested structure, how many elements offend (one of one,
/// one of several, several of several) and which position they sit at.
fn list_rows() -> Vec<ListRow> {
    let pages = |pages: Value| json!({"AppMonitorConfiguration": {"ExcludedPages": pages}});
    vec![
        ListRow {
            case: "the only element of a nested page list",
            body: pages(json!(["/admin"])),
            member: "appMonitorConfiguration.excludedPages",
            rendered: "[/admin]",
            set: url_element_set(),
        },
        ListRow {
            case: "the second element of a nested page list",
            body: pages(json!(["https://example.com/ok", "/admin"])),
            member: "appMonitorConfiguration.excludedPages",
            rendered: "[https://example.com/ok, /admin]",
            set: url_element_set(),
        },
        ListRow {
            case: "two offending elements of one list are ONE validation error",
            body: pages(json!(["/admin", "/billing"])),
            member: "appMonitorConfiguration.excludedPages",
            rendered: "[/admin, /billing]",
            set: url_element_set(),
        },
        ListRow {
            case: "the other member that targets the same element shape",
            body: json!({"AppMonitorConfiguration": {"IncludedPages": ["/admin"]}}),
            member: "appMonitorConfiguration.includedPages",
            rendered: "[/admin]",
            set: url_element_set(),
        },
        ListRow {
            case: "a list at the top level of the input, over another element shape",
            body: json!({"DomainList": ["-not-a-valid-domain-"]}),
            member: "domainList",
            rendered: "[-not-a-valid-domain-]",
            set: domain_element_set(),
        },
        ListRow {
            case: "one offending element among valid ones, at the top level",
            body: json!({"DomainList": ["a.example.com", "ex..ample.com", "b.example.com"]}),
            member: "domainList",
            rendered: "[a.example.com, ex..ample.com, b.example.com]",
            set: domain_element_set(),
        },
    ]
}

/// RULE: a constraint broken by a list ELEMENT is reported at the LIST member — the whole
/// list as the value, no element position in the path, and the element shape's whole
/// modelled constraint set bracketed — whatever the member and however many elements
/// offended.
#[tokio::test]
async fn a_constraint_broken_by_a_list_element_is_stated_at_the_list_member() {
    let (router, _engine) = emulator();

    for (index, row) in list_rows().into_iter().enumerate() {
        let case = row.case;
        let mut body = json!({"Name": format!("list-row-{index}"), "Domain": "example.com"});
        let object = body.as_object_mut().expect("a JSON object");
        for (member, value) in row.body.as_object().expect("a JSON object") {
            // A row that brings its own domain authority replaces the default one, so the
            // request names exactly one form of it.
            if member == "DomainList" {
                object.remove("Domain");
            }
            object.insert(member.clone(), value.clone());
        }
        let reply = create_call(&router, body).await;
        assert_refused(
            case,
            &reply,
            &refusal_message(row.rendered, row.member, &row.set),
        );
    }
}

// ---------------------------------------------------------------------------------------
// Rule 3: the order an enum clause lists the admitted values in
// ---------------------------------------------------------------------------------------

/// `MetricDestination`'s admitted values in the order this service's refusals list them.
const METRIC_DESTINATION_LISTED: [&str; 2] = ["Evidently", "CloudWatch"];

/// The same two values as the MODEL declares them, which the set a clause states must hold
/// whatever order it states them in.
/// Source: `smithy_query(query="shape", shape="MetricDestination")`.
const METRIC_DESTINATION_DECLARED: [&str; 2] = ["CloudWatch", "Evidently"];

/// The values a clause brackets, read back out of the message it stated.
fn bracketed(case: &str, stated: &str) -> Vec<String> {
    let opened = stated
        .split_once("enum value set: [")
        .unwrap_or_else(|| panic!("case {case:?}: the message states no enum value set: {stated:?}"))
        .1;
    let set = opened
        .strip_suffix(']')
        .unwrap_or_else(|| panic!("case {case:?}: the value set is not closed: {stated:?}"));
    set.split(", ").map(str::to_string).collect()
}

/// RULE: an enum clause states the membership the model declares, in the order this service
/// lists that enum's values — over every operation that carries the enum, whether the value
/// travels in the body or in the query string, and whatever value the request offends with.
#[tokio::test]
async fn an_enum_clause_lists_the_admitted_values_in_the_order_the_service_lists_them() {
    let (router, _engine) = emulator();
    let monitor = "enum-order-monitor";
    let created = create_call(&router, json!({"Name": monitor, "Domain": "example.com"})).await;
    assert_eq!(
        created.status,
        200,
        "the setup create must answer 200, got {} {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    put_metrics_destination(&router, "setup", monitor, "CloudWatch", None).await;

    // (case, method, path, body) — one row per operation that carries a `Destination`, and
    // one more offending value, since the set a clause states is a fact about the enum and
    // cannot vary with the request.
    let rows: Vec<(&str, &str, String, Option<Value>, &str)> = vec![
        (
            "BatchCreateRumMetricDefinitions, in the body",
            "POST",
            format!("/rummetrics/{monitor}/metrics"),
            Some(json!({
                "Destination": "NotARealDestination",
                "MetricDefinitions": [{"Name": "JsErrorCount"}],
            })),
            "NotARealDestination",
        ),
        (
            "BatchCreateRumMetricDefinitions, a value that differs only in case",
            "POST",
            format!("/rummetrics/{monitor}/metrics"),
            Some(json!({
                "Destination": "cloudwatch",
                "MetricDefinitions": [{"Name": "JsErrorCount"}],
            })),
            "cloudwatch",
        ),
        (
            "PutRumMetricsDestination, another operation, in the body",
            "POST",
            format!("/rummetrics/{monitor}/metricsdestination"),
            Some(json!({"Destination": "Elsewhere"})),
            "Elsewhere",
        ),
        (
            "BatchGetRumMetricDefinitions, in the query string",
            "GET",
            format!(
                "/rummetrics/{monitor}/metrics?destination={}",
                percent_encoded("Nowhere")
            ),
            None,
            "Nowhere",
        ),
        (
            "DeleteRumMetricsDestination, in the query string",
            "DELETE",
            format!(
                "/rummetrics/{monitor}/metricsdestination?destination={}",
                percent_encoded("Nowhere")
            ),
            None,
            "Nowhere",
        ),
    ];

    let expected_set = format!(
        "Member must satisfy enum value set: [{}]",
        METRIC_DESTINATION_LISTED.join(", ")
    );
    for (case, method, path, body, value) in rows {
        let reply = call(&router, method, &path, body).await;
        assert_refused(
            case,
            &reply,
            &refusal_message(value, "destination", &expected_set),
        );
        // Whatever the order, the membership is the model's: the clause neither adds a
        // value the model does not declare nor drops one it does.
        let stated = message(&reply.json(case)).unwrap_or_default().to_string();
        let brackets: BTreeSet<String> = bracketed(case, &stated).into_iter().collect();
        let declared: BTreeSet<String> = METRIC_DESTINATION_DECLARED
            .iter()
            .map(|value| (*value).to_string())
            .collect();
        assert_eq!(
            brackets, declared,
            "case {case:?}: the set stated must hold exactly the values the model declares, \
             got {stated:?}"
        );
    }
}
