// SPDX-License-Identifier: CC0-1.0

//! The page cursor of a read whose projection is ONE subject's set is bound to that subject.
//!
//! The rule under test: a `NextToken` is an offset into the ordered set of the subject the
//! read that issued it was pointed at, so it resolves only against THAT subject — a token
//! replayed against another subject's set is refused with the modelled `ValidationException`
//! (HTTP 400, naming `NextToken`) rather than honoured as an offset that skips the other
//! subject's first entries — while a token resumes the subject it was issued for whatever
//! spelling of that subject the resuming request uses.
//!
//! Every per-subject paged read whose subject is the ENTITY or the objective a request names is
//! a row of the table below, and each row carries two distinct subjects of its own: the rule
//! generalises over WHICH read, over how the request names its subject (a path label, a
//! key-attribute map in the body), and over which subject of that read is named — not over one
//! recorded pair. The reads whose selection is wider than one such subject — the audit read,
//! which names a LIST of targets, and the account-wide reads, whose selection is the period and
//! the filters alone — are `paged_cursor_selection_binding`'s rows, which pins the same rule for
//! the rest of a selection.
//!
//! The request helpers are `slo_lifecycle`'s and the window configurations `exclusion_windows`'
//! own, so this suite speaks to the same harness as every other.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::exclusion_windows::{batch, create_slo, window, DURATION_UNITS};
use crate::slo_lifecycle::{arn_path_segment, call, create, name_path_segment, router};

/// How many entries each subject below holds. Two is the smallest set a cursor can divide,
/// which is what makes a page bound of one issue a token for every row.
const ENTRIES_PER_SUBJECT: usize = 2;

/// How many exclusion windows each SLO subject holds: more than [`ENTRIES_PER_SUBJECT`], so
/// an offset carried across subjects would land inside the other subject's set rather than
/// past its end.
const WINDOWS_PER_SLO: usize = 4;

/// The environment every seeded entity is hosted in, as `slo_lifecycle::sli_config` names it.
const ENVIRONMENT: &str = "prod";

/// One request to a paged read, with the subject already spelled into its route or its body:
/// only the page bound and the cursor vary from call to call.
#[derive(Clone)]
struct Request {
    /// How the request names its subject, for the assertion messages.
    subject: String,
    method: &'static str,
    /// The route, including any path label naming the subject.
    path: String,
    /// The query members this read carries beside the page ones, `&`-terminated when any.
    query_prefix: String,
    /// The body, for a read that takes its subject (and its period) there.
    body: Option<Value>,
}

/// One paged read, pointed at two DIFFERENT subjects, each holding a set of its own.
struct Case {
    /// The operation under test, named in every assertion.
    read: &'static str,
    /// The output member carrying the page.
    list_member: &'static str,
    /// Two different subjects of this read.
    subjects: [Request; 2],
    /// The FIRST subject, named another way, where the read admits a second spelling of one
    /// subject: a token issued for one spelling must still resume the other.
    other_form: Option<Request>,
}

/// One page of `request`, bounded by `max_results` and resuming from `token` when given.
async fn page(
    router: &MultiServiceRouter,
    request: &Request,
    max_results: usize,
    token: Option<&str>,
) -> (u16, Value) {
    let mut uri = format!(
        "{}?{}MaxResults={max_results}",
        request.path, request.query_prefix
    );
    if let Some(token) = token {
        uri.push_str(&format!("&NextToken={token}"));
    }
    call(router, request.method, &uri, request.body.clone()).await
}

/// The entries a page reports, in order.
fn entries(answer: &Value, list_member: &str) -> Vec<Value> {
    answer[list_member].as_array().cloned().unwrap_or_default()
}

/// The key attributes of one Application Signals service entity.
fn service_entity(name: &str) -> Value {
    json!({ "Type": "Service", "Name": name, "Environment": ENVIRONMENT })
}

/// The identity of the far end every seeded edge points at, one per service subject, so the
/// dependents of one are not the dependents of the other.
fn sink_entity(service: &str) -> Value {
    json!({ "Type": "RemoteService", "Name": format!("{service}-sink"), "Environment": ENVIRONMENT })
}

/// Seed the SLOs that make up the entity subjects' sets, and the exclusion windows that make
/// up the SLO subjects' sets, then hand back the table every rule below is asserted over.
///
/// Each seeded SLO records, for the service it monitors, one operation and one edge out of
/// that operation into that service's own far end — so the same records give every entity
/// subject exactly [`ENTRIES_PER_SUBJECT`] operations, dependencies, dependents and change
/// events.
async fn seeded(router: &MultiServiceRouter) -> Vec<Case> {
    let services = ["cursor-alpha", "cursor-beta"];
    for service in services {
        for index in 0..ENTRIES_PER_SUBJECT {
            let body = json!({
                "Name": format!("{service}-slo-{index}"),
                "SliConfig": {
                    "SliMetricConfig": {
                        "KeyAttributes": service_entity(service),
                        "OperationName": format!("{service}-op-{index}"),
                        "MetricType": "LATENCY",
                        "PeriodSeconds": 60,
                        "DependencyConfig": {
                            "DependencyKeyAttributes": sink_entity(service),
                            "DependencyOperationName": format!("Call-{index}")
                        }
                    },
                    "MetricThreshold": 100.0,
                    "ComparisonOperator": "LessThan"
                }
            });
            let (status, created) = create(router, body).await;
            assert_eq!(
                status, 200,
                "case {service:?} entry {index}: expected the setup create to succeed, got {status} with {created}"
            );
        }
    }

    // The SLO subjects of the exclusion-window read, each holding windows no other SLO holds:
    // the stored set is identified by configuration, so no two subjects may collide.
    let slos = ["cursor-slo-a", "cursor-slo-b"];
    for (slot, name) in slos.iter().enumerate() {
        create_slo(router, name).await;
        let windows: Vec<Value> = (0..WINDOWS_PER_SLO)
            .map(|index| {
                window(
                    DURATION_UNITS[index % DURATION_UNITS.len()],
                    // `Window.Duration` targets `ExclusionDuration`, `range 1..`
                    // (`smithy_query(shape="Window")`), so every duration is at least 1.
                    (slot * 100 + index + 1) as i64,
                    Some(1_700_000_000 + (slot * 1_000 + index) as i64),
                    None,
                    Some(&format!("{name} window {index}")),
                )
            })
            .collect();
        let (status, updated) = batch(router, &[(*name).to_string()], &windows, &[]).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected the setup add of {WINDOWS_PER_SLO} windows to succeed, got {status} with {updated}"
        );
    }

    // A window around the instant the records above were written at, for the reads that take
    // their period in the query string, and the same period as body members for the one that
    // takes it there.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64;
    let (start, end) = (now - 3_600, now + 3_600);
    let window_query = format!("StartTime={start}&EndTime={end}&");

    let entity_read = |read: &'static str,
                       list_member: &'static str,
                       path: &'static str,
                       subject_of: fn(&str) -> Value| Case {
        read,
        list_member,
        subjects: services.map(|service| Request {
            subject: service.to_string(),
            method: "POST",
            path: path.to_string(),
            query_prefix: window_query.clone(),
            body: Some(json!({ "KeyAttributes": subject_of(service) })),
        }),
        other_form: None,
    };

    vec![
        Case {
            read: "ListServiceLevelObjectiveExclusionWindows",
            list_member: "ExclusionWindows",
            subjects: slos.map(|name| Request {
                subject: name.to_string(),
                method: "GET",
                path: format!("/slo/{}/exclusion-windows", name_path_segment(name)),
                query_prefix: String::new(),
                body: None,
            }),
            // The modelled `ServiceLevelObjectiveId` admits a bare name or the SLO's ARN, so
            // the same subject has two spellings here.
            other_form: Some(Request {
                subject: format!("{} by ARN", slos[0]),
                method: "GET",
                path: format!("/slo/{}/exclusion-windows", arn_path_segment(slos[0])),
                query_prefix: String::new(),
                body: None,
            }),
        },
        entity_read(
            "ListServiceOperations",
            "ServiceOperations",
            "/service-operations",
            service_entity,
        ),
        entity_read(
            "ListServiceDependencies",
            "ServiceDependencies",
            "/service-dependencies",
            service_entity,
        ),
        // The dependents of a far end are the edges INTO it, so this read's subject is the
        // entity the seeded edges point at.
        entity_read(
            "ListServiceDependents",
            "ServiceDependents",
            "/service-dependents",
            sink_entity,
        ),
        Case {
            read: "ListEntityEvents",
            list_member: "ChangeEvents",
            subjects: services.map(|service| Request {
                subject: service.to_string(),
                method: "POST",
                path: "/events".to_string(),
                query_prefix: String::new(),
                body: Some(json!({
                    "Entity": service_entity(service),
                    "StartTime": start,
                    "EndTime": end
                })),
            }),
            other_form: None,
        },
    ]
}

/// A token issued for one subject's set is not an offset into another's: replaying it is
/// refused, and the read it was replayed against still answers its own first page.
#[tokio::test]
async fn a_token_issued_for_one_subject_is_refused_for_another() {
    let router = router();
    for case in seeded(&router).await {
        let Case {
            read,
            list_member,
            subjects: [issuer, other],
            ..
        } = &case;
        let label = format!("{read} token of {:?} on {:?}", issuer.subject, other.subject);

        let (status, first) = page(&router, issuer, 1, None).await;
        assert_eq!(
            status, 200,
            "case {label}: expected the first page to succeed, got {status} with {first}"
        );
        let token = first["NextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {label}: expected a NextToken on a page of one of several entries, got {first}")
            })
            .to_string();

        let (status, before) = page(&router, other, 1, None).await;
        assert_eq!(
            status, 200,
            "case {label}: expected the other subject's own first page to succeed, got {status} with {before}"
        );
        assert_eq!(
            entries(&before, list_member).len(),
            1,
            "case {label}: expected the other subject to hold a first page of its own, got {before}"
        );

        let (status, refusal) = page(&router, other, 1, Some(&token)).await;
        assert_eq!(
            status, 400,
            "case {label}: expected HTTP 400 for a token issued for another subject, got {status} with {refusal}"
        );
        assert!(
            refusal["message"]
                .as_str()
                .is_some_and(|message| message.contains("NextToken")),
            "case {label}: expected the refusal to name NextToken, got {refusal}"
        );

        let (status, after) = page(&router, other, 1, None).await;
        assert_eq!(
            status, 200,
            "case {label}: expected the read after the refusal to succeed, got {status} with {after}"
        );
        assert_eq!(
            entries(&after, list_member),
            entries(&before, list_member),
            "case {label}: expected the refused token to leave the other subject's page as it was, got {after}"
        );
    }
}

/// A token resumes the set it WAS issued for: walking a subject one entry at a time reports
/// every entry of that subject exactly once, in the order a single page reports them, and a
/// token issued for one spelling of a subject resumes a read that spells it the other way.
#[tokio::test]
async fn a_token_resumes_the_subject_it_was_issued_for() {
    let router = router();
    for case in seeded(&router).await {
        let Case {
            read,
            list_member,
            subjects,
            other_form,
        } = &case;
        for subject in subjects {
            let label = format!("{read} walk of {:?}", subject.subject);
            // The whole set in one page: every row's set is smaller than 10, the smallest
            // maximum any of these reads' `MaxResults` admits.
            let (status, whole_page) = page(&router, subject, 10, None).await;
            assert_eq!(
                status, 200,
                "case {label}: expected the whole set in one page, got {status} with {whole_page}"
            );
            let whole = entries(&whole_page, list_member);
            assert!(
                whole.len() > 1,
                "case {label}: expected a set a cursor can divide, got {whole_page}"
            );

            let mut walked: Vec<Value> = Vec::new();
            let mut token: Option<String> = None;
            for step in 0..=whole.len() {
                let (status, answer) = page(&router, subject, 1, token.as_deref()).await;
                assert_eq!(
                    status, 200,
                    "case {label} page {step}: expected the page to succeed, got {status} with {answer}"
                );
                let reported = entries(&answer, list_member);
                assert_eq!(
                    reported.len(),
                    1,
                    "case {label} page {step}: expected exactly the bound of one entry, got {answer}"
                );
                walked.extend(reported);
                token = answer["NextToken"].as_str().map(ToString::to_string);
                if token.is_none() {
                    break;
                }
            }
            assert!(
                token.is_none(),
                "case {label}: expected the walk to end within one page per entry, got {token:?}"
            );
            assert_eq!(
                walked, whole,
                "case {label}: expected the walk to report every entry of this subject exactly once, in order"
            );
        }

        let Some(other_form) = other_form else {
            continue;
        };
        let issuer = &subjects[0];
        let label = format!(
            "{read} token of {:?} resumed as {:?}",
            issuer.subject, other_form.subject
        );
        let (status, first) = page(&router, issuer, 1, None).await;
        assert_eq!(
            status, 200,
            "case {label}: expected the first page to succeed, got {status} with {first}"
        );
        let token = first["NextToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label}: expected a NextToken, got {first}"))
            .to_string();
        let (status, resumed_other_form) = page(&router, other_form, 1, Some(&token)).await;
        let (issuer_status, resumed) = page(&router, issuer, 1, Some(&token)).await;
        assert_eq!(
            status, issuer_status,
            "case {label}: expected the same status for either spelling of the subject, got {status} and {issuer_status}"
        );
        assert_eq!(
            status, 200,
            "case {label}: expected the resumed read to succeed, got {status} with {resumed_other_form}"
        );
        assert_eq!(
            entries(&resumed_other_form, list_member),
            entries(&resumed, list_member),
            "case {label}: expected the token to resume the same page for either spelling, got {resumed_other_form} and {resumed}"
        );
    }
}
