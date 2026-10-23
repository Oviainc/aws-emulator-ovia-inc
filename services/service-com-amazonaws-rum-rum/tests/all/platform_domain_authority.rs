// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for which app monitors have to name a domain, and what a monitor
//! that names none reads back as.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so routing, deserialization, the modelled constraints, the handler
//! and the response envelope are all exercised, not just a handler body.
//!
//! The rule pinned here: an app monitor's domain authority describes the internet domain
//! an application is served from, so naming it is REQUIRED only for a monitor on the web
//! platform (the default), and a monitor on any other platform is created with neither
//! `Domain` nor `DomainList` and reads back carrying neither. The other half of the rule
//! is platform-independent: whichever platform a monitor is for, its domain authority is
//! held in at most one of the two forms, so naming both is refused for every platform —
//! and naming one of them is accepted for every platform.
//!
//! The tables span the whole `AppMonitorPlatform` catalog rather than the one recorded
//! platform, both ways of naming a domain, the monitor created with no domain at all, and
//! the three reads that have to agree about it (`CreateAppMonitor` -> `GetAppMonitor` ->
//! `ListAppMonitors`) plus the `DeleteAppMonitor` that tears it down.

use serde_json::{json, Value};
use service_com_amazonaws_rum_rum::state::{
    CREATED_STATE, DEFAULT_DEOBFUSCATION_STATUS, DEFAULT_PLATFORM, DEFAULT_SESSION_SAMPLE_RATE,
    WEB_PLATFORM,
};

use crate::support::{call, emulator, get_app_monitor, is_modelled_app_monitor_id, tag_map};

/// Every value `AppMonitorPlatform` admits.
/// Source: `smithy_query(query="shape", shape="AppMonitorPlatform")` against
/// `services/service-com-amazonaws-rum-rum/smithy-model/com-amazonaws-rum-rum.json`.
const PLATFORMS: [&str; 3] = ["Web", "Android", "iOS"];

/// One row of the create table: how the request names its platform, and whether a monitor
/// on that platform must name its domain authority.
struct PlatformRow {
    case: String,
    /// The `Platform` member the request carries, or `None` to leave it out (the model
    /// documents `Web` as the default).
    requested: Option<&'static str>,
    /// The platform the stored monitor must then report.
    stored: &'static str,
}

/// The platform table: every value of the catalog, plus the request that names none and
/// so takes the documented default.
fn platform_rows() -> Vec<PlatformRow> {
    let mut rows: Vec<PlatformRow> = PLATFORMS
        .iter()
        .map(|platform| PlatformRow {
            case: format!("Platform={platform}"),
            requested: Some(platform),
            stored: platform,
        })
        .collect();
    rows.push(PlatformRow {
        case: "Platform omitted (the documented default)".to_string(),
        requested: None,
        stored: DEFAULT_PLATFORM,
    });
    rows
}

/// A create body naming `name`, the row's platform, and whatever `domain` adds (nothing,
/// a `Domain`, a `DomainList`, or both).
fn create_body(name: &str, requested: Option<&str>, domain: &Value) -> Value {
    let mut body = json!({"Name": name});
    let object = body.as_object_mut().expect("a JSON object");
    if let Some(platform) = requested {
        object.insert("Platform".to_string(), json!(platform));
    }
    for (member, value) in domain.as_object().expect("a JSON object") {
        object.insert(member.clone(), value.clone());
    }
    body
}

#[tokio::test]
async fn only_a_web_app_monitor_has_to_name_its_domain_authority() {
    // A create that names NEITHER Domain nor DomainList: refused for the web platform
    // (explicit or defaulted), accepted for every platform that is not served from an
    // internet domain.
    let (router, _engine) = emulator();

    for PlatformRow {
        case,
        requested,
        stored,
    } in platform_rows()
    {
        let case = case.as_str();
        let name = format!(
            "domainless-{}-{}",
            stored.to_lowercase(),
            if requested.is_some() { "named" } else { "default" }
        );
        let reply = call(
            &router,
            "POST",
            "/appmonitor",
            Some(create_body(&name, requested, &json!({}))),
        )
        .await;

        if stored == WEB_PLATFORM {
            assert_eq!(
                reply.status, 400,
                "case {case:?}: a web app monitor must name its domain authority, so this \
                 create must be refused with HTTP 400, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            assert_eq!(
                reply.error_code(case),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                String::from_utf8_lossy(&reply.body)
            );
            let read = call(&router, "GET", &format!("/appmonitor/{name}"), None).await;
            assert_eq!(
                read.status, 404,
                "case {case:?}: a refused create must store nothing, got {} {:?}",
                read.status,
                String::from_utf8_lossy(&read.body)
            );
            continue;
        }

        assert_eq!(
            reply.status, 200,
            "case {case:?}: an application on this platform is not served from an internet \
             domain, so a create naming none must answer HTTP 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let id = reply.json(case)["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateAppMonitor returned no Id"))
            .to_string();
        assert!(
            is_modelled_app_monitor_id(&id),
            "case {case:?}: expected Id to match AppMonitorId's pattern, got {id:?}"
        );

        // The round trip: the monitor the create minted is readable, carries the platform
        // it was created for, and carries NEITHER domain member.
        let monitor = get_app_monitor(&router, case, &name).await;
        assert_eq!(
            monitor["Id"].as_str(),
            Some(id.as_str()),
            "case {case:?}: the read must report the Id the create minted, got {:?}",
            monitor["Id"]
        );
        assert_eq!(
            monitor["Platform"].as_str(),
            Some(stored),
            "case {case:?}: expected Platform {stored:?}, got {:?}",
            monitor["Platform"]
        );
        for absent in ["Domain", "DomainList"] {
            assert_eq!(
                monitor[absent],
                Value::Null,
                "case {case:?}: the create named no {absent}, so the read must report none, \
                 got {:?}",
                monitor[absent]
            );
        }
        // The defaults the model documents for a create that named nothing else either.
        assert_eq!(
            monitor["State"].as_str(),
            Some(CREATED_STATE),
            "case {case:?}: expected State {CREATED_STATE}, got {:?}",
            monitor["State"]
        );
        assert_eq!(
            monitor["LastModified"],
            Value::Null,
            "case {case:?}: nothing has changed this monitor, so it must report no \
             LastModified, got {:?}",
            monitor["LastModified"]
        );
        assert_eq!(
            monitor["CustomEvents"]["Status"].as_str(),
            Some("DISABLED"),
            "case {case:?}: custom events default to DISABLED, got {:?}",
            monitor["CustomEvents"]
        );
        assert_eq!(
            monitor["DeobfuscationConfiguration"]["JavaScriptSourceMaps"]["Status"].as_str(),
            Some(DEFAULT_DEOBFUSCATION_STATUS),
            "case {case:?}: expected the default JavaScriptSourceMaps status \
             {DEFAULT_DEOBFUSCATION_STATUS:?}, got {:?}",
            monitor["DeobfuscationConfiguration"]
        );
        assert_eq!(
            monitor["DataStorage"]["CwLog"]["CwLogEnabled"].as_bool(),
            Some(false),
            "case {case:?}: CloudWatch Logs copies default to off, got {:?}",
            monitor["DataStorage"]
        );
        assert_eq!(
            monitor["AppMonitorConfiguration"]["SessionSampleRate"].as_f64(),
            Some(DEFAULT_SESSION_SAMPLE_RATE),
            "case {case:?}: expected the documented default session sample rate \
             {DEFAULT_SESSION_SAMPLE_RATE}, got {:?}",
            monitor["AppMonitorConfiguration"]
        );
        assert_eq!(
            tag_map(&monitor),
            json!({}),
            "case {case:?}: the create named no Tags, got {:?}",
            monitor["Tags"]
        );

        // The summary projection agrees with the detail one, member for member.
        let summary = summary_for(&router, case, &name).await;
        assert_eq!(
            summary["Platform"].as_str(),
            Some(stored),
            "case {case:?}: the summary must report Platform {stored:?}, got {:?}",
            summary["Platform"]
        );
        assert_eq!(
            summary["Id"].as_str(),
            Some(id.as_str()),
            "case {case:?}: the summary must report the minted Id, got {:?}",
            summary["Id"]
        );
        assert_eq!(
            summary["State"].as_str(),
            Some(CREATED_STATE),
            "case {case:?}: the summary must report State {CREATED_STATE}, got {:?}",
            summary["State"]
        );
        assert_eq!(
            summary["LastModified"],
            Value::Null,
            "case {case:?}: the summary of an unmodified monitor must report no \
             LastModified, got {:?}",
            summary["LastModified"]
        );

        // And the monitor a client created this way can be torn down again.
        let deleted = call(&router, "DELETE", &format!("/appmonitor/{name}"), None).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: deleting the created monitor must answer HTTP 200, got {} {:?}",
            deleted.status,
            String::from_utf8_lossy(&deleted.body)
        );
    }
}

#[tokio::test]
async fn naming_a_domain_is_admitted_and_naming_both_forms_is_refused_on_every_platform() {
    // The platform-independent half of the rule, over the product of the catalog and the
    // ways a request can name its domain authority: ONE form is accepted and stored,
    // BOTH forms are refused, whatever the platform.
    let (router, _engine) = emulator();

    let forms: [(&str, Value); 3] = [
        ("Domain", json!({"Domain": "shop.example.com"})),
        (
            "DomainList",
            json!({"DomainList": ["a.example.com", "b.example.com"]}),
        ),
        (
            "both forms at once",
            json!({"Domain": "shop.example.com", "DomainList": ["a.example.com"]}),
        ),
    ];

    for PlatformRow {
        case,
        requested,
        stored,
    } in platform_rows()
    {
        for (form, domain) in &forms {
            let case = format!("{case}, {form}");
            let case = case.as_str();
            let name = format!(
                "{}-{}-{}",
                stored.to_lowercase(),
                if requested.is_some() { "named" } else { "default" },
                form.replace(' ', "-").to_lowercase()
            );
            let reply = call(
                &router,
                "POST",
                "/appmonitor",
                Some(create_body(&name, requested, domain)),
            )
            .await;

            let names_both = domain["Domain"].is_string() && domain["DomainList"].is_array();
            if names_both {
                assert_eq!(
                    reply.status, 400,
                    "case {case:?}: domain authority is held in at most one form, so this \
                     create must be refused with HTTP 400, got {} {:?}",
                    reply.status,
                    String::from_utf8_lossy(&reply.body)
                );
                assert_eq!(
                    reply.error_code(case),
                    "ValidationException",
                    "case {case:?}: expected ValidationException, got {:?}",
                    String::from_utf8_lossy(&reply.body)
                );
                continue;
            }

            assert_eq!(
                reply.status, 200,
                "case {case:?}: one form of domain authority is admitted on every platform, \
                 so this create must answer HTTP 200, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            let monitor = get_app_monitor(&router, case, &name).await;
            assert_eq!(
                monitor["Platform"].as_str(),
                Some(stored),
                "case {case:?}: expected Platform {stored:?}, got {:?}",
                monitor["Platform"]
            );
            assert_eq!(
                monitor["Domain"], domain["Domain"],
                "case {case:?}: expected Domain {:?}, got {:?}",
                domain["Domain"], monitor["Domain"]
            );
            assert_eq!(
                monitor["DomainList"], domain["DomainList"],
                "case {case:?}: expected DomainList {:?}, got {:?}",
                domain["DomainList"], monitor["DomainList"]
            );
        }
    }
}

/// The `ListAppMonitors` summary for `name`, from the one page the whole store fits on.
async fn summary_for(
    router: &smithy_server_runtime::multi_service_router::MultiServiceRouter,
    case: &str,
    name: &str,
) -> Value {
    let reply = call(router, "POST", "/appmonitors", None).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected ListAppMonitors to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    reply.json(case)["AppMonitorSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListAppMonitors reported no summaries"))
        .iter()
        .find(|summary| summary["Name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("case {case:?}: no summary for {name:?}"))
        .clone()
}
