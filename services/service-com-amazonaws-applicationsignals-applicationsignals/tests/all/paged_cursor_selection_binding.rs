// SPDX-License-Identifier: CC0-1.0

//! The page cursor of a read whose ordered set the REQUEST picks out is bound to the whole
//! selection it was counted in.
//!
//! The rule under test: the `NextToken` of these reads is a bare OFFSET into a set this
//! service recomputes per call, so it counts only within the set it was issued over — every
//! request member that picks that set out (the subject the request names, the filters that
//! narrow it, the period it covers) travels inside the token, and a token replayed against
//! another selection is refused with the modelled `ValidationException` (HTTP 400, naming
//! `NextToken`) rather than honoured as an offset that skips the other selection's first
//! entries. A member that picks out nothing — one that only changes what a reported entry
//! carries, or a second spelling of one subject — leaves the token honoured.
//!
//! `paged_cursor_subject_binding` pins the subject half of that rule for the reads whose
//! subject is one entity. The table below is the rest of the selection, and it carries the
//! audit read, whose whole selection — the targets it names, the auditors it filters by and
//! the period it covers — is stated by the request. Every row differs along what the rule
//! generalises over: WHICH read, WHICH facet of the selection varies, and where the model
//! binds the facet and the page members (a query member, a key-attribute map in the body, a
//! list of audit targets in the body).
//!
//! The harness is `slo_lifecycle`'s, so this suite speaks to the same mounted router as every
//! other.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::slo_lifecycle::{call, create, router};

/// The services the seeded objectives monitor: two, so no row's two selections share a set.
const SERVICES: [&str; 2] = ["select-alpha", "select-beta"];

/// How many objectives monitor each seeded service. Two is the smallest set a cursor can
/// divide, which is what makes a page bound of one issue a token for every row.
const OBJECTIVES_PER_SERVICE: usize = 2;

/// The environment every seeded entity is hosted in.
const ENVIRONMENT: &str = "prod";

/// Where a read's model binds `MaxResults` and `NextToken`.
#[derive(Clone, Copy)]
enum PageMembers {
    /// In the query string, beside whatever else the route carries.
    Query,
    /// In the JSON body, as `ListAuditFindings` and `ListServiceStates` bind them.
    Body,
}

/// One request to a paged read, with the whole selection already spelled into its route and
/// its body: only the page bound and the cursor vary from call to call.
#[derive(Clone)]
struct Request {
    /// How this request picks its set out, for the assertion messages.
    selection: String,
    method: &'static str,
    /// The route, including any query members the selection is spelled in.
    path: String,
    page_members: PageMembers,
    body: Option<Value>,
}

/// One read, pointed at two selections that differ in exactly ONE facet.
struct Case {
    /// The operation under test, named in every assertion.
    read: &'static str,
    /// The output member carrying the page.
    list_member: &'static str,
    /// The facet that differs between the two selections.
    facet: &'static str,
    /// Two selections of this read, each with a set of its own to page.
    selections: [Request; 2],
}

/// One selection spelled two ways, or with a member that picks out nothing changed: the token
/// one spelling issues must resume the other.
struct Equivalent {
    read: &'static str,
    list_member: &'static str,
    /// What differs between the two requests, which is not part of the selection.
    difference: &'static str,
    issuer: Request,
    resumed: Request,
    /// What identifies a reported entry across the difference.
    key: EntryKey,
}

/// How one reported entry is identified when two spellings of ONE selection are compared: a
/// member outside the selection may still change what an entry CARRIES, so the entries are
/// compared on what identifies them rather than verbatim.
type EntryKey = fn(&Value) -> String;

/// Every member of a reported entry, for a difference that changes none of them.
fn whole_entry(entry: &Value) -> String {
    entry.to_string()
}

/// The objective one audit finding assessed: its SLO auditor result names the record the
/// finding was computed from.
fn audited_objective(entry: &Value) -> String {
    entry["AuditorResults"][0]["Data"]["SloName"].to_string()
}

/// The service one reported state belongs to, which is what identifies the entry; the filters
/// it echoes are the request's own spelling of them.
fn stated_service(entry: &Value) -> String {
    entry["Service"].to_string()
}

/// `path` with `extra` appended as a query member, whichever way the route already ends.
fn with_query(path: &str, extra: &str) -> String {
    let separator = if path.contains('?') { '&' } else { '?' };
    format!("{path}{separator}{extra}")
}

/// One page of `request`, bounded by `max_results` and resuming from `token` when given.
async fn page(
    router: &MultiServiceRouter,
    request: &Request,
    max_results: usize,
    token: Option<&str>,
) -> (u16, Value) {
    let mut path = request.path.clone();
    let mut body = request.body.clone();
    match request.page_members {
        PageMembers::Query => {
            path = with_query(&path, &format!("MaxResults={max_results}"));
            if let Some(token) = token {
                path = with_query(&path, &format!("NextToken={token}"));
            }
        }
        PageMembers::Body => {
            let mut carried = body.unwrap_or_else(|| json!({}));
            carried["MaxResults"] = json!(max_results);
            if let Some(token) = token {
                carried["NextToken"] = json!(token);
            }
            body = Some(carried);
        }
    }
    call(router, request.method, &path, body).await
}

/// The entries a page reports, in order.
fn entries(answer: &Value, list_member: &str) -> Vec<Value> {
    answer[list_member].as_array().cloned().unwrap_or_default()
}

/// The entries a page reports, on what identifies them.
fn keyed(answer: &Value, list_member: &str, key: EntryKey) -> Vec<String> {
    entries(answer, list_member).iter().map(key).collect()
}

/// The key attributes of one Application Signals service entity.
fn service_entity(name: &str) -> Value {
    json!({ "Type": "Service", "Name": name, "Environment": ENVIRONMENT })
}

/// The far end the seeded edges of one service point at — one per service, so the dependents
/// of one are not the dependents of the other.
fn sink_entity(service: &str) -> Value {
    json!({
        "Type": "RemoteService",
        "Name": format!("{service}-sink"),
        "Environment": ENVIRONMENT
    })
}

/// One audit target, in the shape the request carries it: the kind, and the entity union
/// member naming what to audit.
fn target(kind: &str, data: Value) -> Value {
    json!({ "Type": kind, "Data": data })
}

/// Seed the objectives every set below is derived from, and hand back the ARN the create
/// minted for each, in creation order.
///
/// Each objective records, for the service it monitors, one operation and one edge out of that
/// operation into that service's own far end — so the same records give every seeded service
/// [`OBJECTIVES_PER_SERVICE`] findings, operations, dependencies and change events, and its
/// far end that many dependents.
async fn seeded(router: &MultiServiceRouter) -> Vec<String> {
    let mut arns = Vec::new();
    for service in SERVICES {
        for index in 0..OBJECTIVES_PER_SERVICE {
            let name = format!("{service}-slo-{index}");
            let (status, created) = create(
                router,
                json!({
                    "Name": name,
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
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {name:?}: expected the setup create to succeed, got {status} with {created}"
            );
            let arn = created["Slo"]["Arn"].as_str().unwrap_or_else(|| {
                panic!("case {name:?}: expected the created objective to carry an Arn, got {created}")
            });
            arns.push(arn.to_string());
        }
    }
    arns
}

/// Seconds since the epoch, for the periods below.
fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
}

/// The audit read over `targets`, with the rest of its members as `extra` states them.
fn audit_request(selection: &str, window: &str, targets: Value, extra: Value) -> Request {
    let mut body = json!({ "AuditTargets": targets });
    if let Some(extra) = extra.as_object() {
        for (member, value) in extra {
            body[member] = value.clone();
        }
    }
    Request {
        selection: selection.to_string(),
        method: "POST",
        // `StartTime` and `EndTime` are the only query-bound members of this read: its
        // `MaxResults` and `NextToken` are body members.
        path: format!("/auditFindings?{window}"),
        page_members: PageMembers::Body,
        body: Some(body),
    }
}

/// The state read over `window`, narrowed by `filters` when any are stated.
fn states_request(selection: &str, start: i64, end: i64, filters: Option<Value>) -> Request {
    let mut body = json!({ "StartTime": start, "EndTime": end });
    if let Some(filters) = filters {
        body["AttributeFilters"] = filters;
    }
    Request {
        selection: selection.to_string(),
        method: "POST",
        path: "/service/states".to_string(),
        // Every input member of this read, its page members included, is a body member.
        page_members: PageMembers::Body,
        body: Some(body),
    }
}

/// Every row of the rule: one read, and two selections of it that differ in one facet.
fn varied_selections(now: i64) -> Vec<Case> {
    let (start, end) = (now - 3_600, now + 3_600);
    let narrow = format!("StartTime={start}&EndTime={end}");
    // A second period, wider on both sides: it holds the same records, so honouring a token
    // across the two would skip entries rather than run off the end of a shorter set.
    let (wide_start, wide_end) = (now - 7_200, now + 7_200);
    let wide = format!("StartTime={wide_start}&EndTime={wide_end}");

    // The reads that take their subject as a key-attribute map in the body and their period,
    // page bound and cursor in the query string.
    let entity_period = |read: &'static str,
                         list_member: &'static str,
                         path: &'static str,
                         subject_of: fn(&str) -> Value| Case {
        read,
        list_member,
        facet: "the period it covers",
        selections: [&narrow, &wide].map(|window| Request {
            selection: format!("{} over {window}", SERVICES[0]),
            method: "POST",
            path: format!("{path}?{window}"),
            page_members: PageMembers::Query,
            body: Some(json!({ "KeyAttributes": subject_of(SERVICES[0]) })),
        }),
    };

    vec![
        Case {
            read: "ListAuditFindings",
            list_member: "AuditFindings",
            facet: "the targets it names",
            selections: SERVICES.map(|service| {
                audit_request(
                    &format!("the findings of {service}"),
                    &narrow,
                    json!([target("service", json!({ "Service": service_entity(service) }))]),
                    json!({}),
                )
            }),
        },
        Case {
            read: "ListAuditFindings",
            list_member: "AuditFindings",
            facet: "the auditors it filters by",
            selections: [
                audit_request(
                    "no auditor filter",
                    &narrow,
                    json!([target(
                        "service",
                        json!({ "Service": service_entity(SERVICES[0]) })
                    )]),
                    json!({}),
                ),
                audit_request(
                    "the slo auditor only",
                    &narrow,
                    json!([target(
                        "service",
                        json!({ "Service": service_entity(SERVICES[0]) })
                    )]),
                    // `slo` is the one auditor whose input this emulator holds
                    // (`smithy_query(query="doc", shape="ListAuditFindingsInput",
                    // member="Auditors")` lists it first), so this filter keeps every finding
                    // the unfiltered read reports.
                    json!({ "Auditors": ["slo"] }),
                ),
            ],
        },
        Case {
            read: "ListAuditFindings",
            list_member: "AuditFindings",
            facet: "the period it covers",
            selections: [&narrow, &wide].map(|window| {
                audit_request(
                    &format!("the findings of {} over {window}", SERVICES[0]),
                    window,
                    json!([target(
                        "service",
                        json!({ "Service": service_entity(SERVICES[0]) })
                    )]),
                    json!({}),
                )
            }),
        },
        entity_period(
            "ListServiceOperations",
            "ServiceOperations",
            "/service-operations",
            service_entity,
        ),
        entity_period(
            "ListServiceDependencies",
            "ServiceDependencies",
            "/service-dependencies",
            service_entity,
        ),
        // The dependents of a far end are the edges INTO it, so this read's subject is the
        // entity the seeded edges point at.
        entity_period(
            "ListServiceDependents",
            "ServiceDependents",
            "/service-dependents",
            sink_entity,
        ),
        Case {
            read: "ListEntityEvents",
            list_member: "ChangeEvents",
            facet: "the period it covers",
            selections: [(start, end), (wide_start, wide_end)].map(|(start, end)| Request {
                selection: format!("{} over {start}..{end}", SERVICES[0]),
                method: "POST",
                path: "/events".to_string(),
                page_members: PageMembers::Query,
                body: Some(json!({
                    "Entity": service_entity(SERVICES[0]),
                    "StartTime": start,
                    "EndTime": end
                })),
            }),
        },
        Case {
            read: "ListServices",
            list_member: "ServiceSummaries",
            facet: "the period it covers",
            selections: [&narrow, &wide].map(|window| Request {
                selection: format!("the services over {window}"),
                method: "GET",
                path: format!("/services?{window}"),
                page_members: PageMembers::Query,
                body: None,
            }),
        },
        Case {
            read: "ListServiceStates",
            list_member: "ServiceStates",
            facet: "the period it covers",
            selections: [(start, end), (wide_start, wide_end)]
                .map(|(start, end)| states_request(&format!("{start}..{end}"), start, end, None)),
        },
        Case {
            read: "ListServiceStates",
            list_member: "ServiceStates",
            facet: "the attribute filters that narrow it",
            selections: [
                states_request("no attribute filter", start, end, None),
                states_request(
                    "the services of one environment",
                    start,
                    end,
                    Some(json!([{
                        "AttributeFilterName": "Environment",
                        "AttributeFilterValues": [ENVIRONMENT]
                    }])),
                ),
            ],
        },
        Case {
            read: "ListAuditFindings",
            list_member: "AuditFindings",
            facet: "which objectives its targets name",
            selections: [[0, 1], [2, 3]].map(|pair| {
                audit_request(
                    &format!("the objectives {pair:?}"),
                    &narrow,
                    json!(pair
                        .iter()
                        .map(|index| target(
                            "slo",
                            json!({ "Slo": { "SloName": objective_name(*index) } }),
                        ))
                        .collect::<Vec<Value>>()),
                    json!({}),
                )
            }),
        },
        Case {
            read: "ListAuditFindings",
            list_member: "AuditFindings",
            facet: "how many targets it names",
            selections: [
                audit_request(
                    "two objectives",
                    &narrow,
                    json!([
                        target("slo", json!({ "Slo": { "SloName": objective_name(0) } })),
                        target("slo", json!({ "Slo": { "SloName": objective_name(1) } }))
                    ]),
                    json!({}),
                ),
                audit_request(
                    "one of those objectives",
                    &narrow,
                    json!([target(
                        "slo",
                        json!({ "Slo": { "SloName": objective_name(0) } })
                    )]),
                    json!({}),
                ),
            ],
        },
    ]
}

/// The name the objective at `index` of the seeded order was created under.
fn objective_name(index: usize) -> String {
    format!(
        "{}-slo-{}",
        SERVICES[index / OBJECTIVES_PER_SERVICE],
        index % OBJECTIVES_PER_SERVICE
    )
}

/// Every row of the other half of the rule: ONE selection, stated two ways.
fn equivalent_spellings(arns: &[String], now: i64) -> Vec<Equivalent> {
    let (start, end) = (now - 3_600, now + 3_600);
    let window = format!("StartTime={start}&EndTime={end}");
    let objectives_by_name = json!([
        target("slo", json!({ "Slo": { "SloName": objective_name(0) } })),
        target("slo", json!({ "Slo": { "SloName": objective_name(1) } }))
    ]);
    // The same two objectives the other way round, one of them named twice: the findings are
    // keyed by the objective each assessed, so neither the order nor the repeat picks out a
    // different set.
    let objectives_reordered = json!([
        target("slo", json!({ "Slo": { "SloName": objective_name(1) } })),
        target("slo", json!({ "Slo": { "SloName": objective_name(0) } })),
        target("slo", json!({ "Slo": { "SloName": objective_name(1) } }))
    ]);
    let objectives_by_arn = json!([
        target("slo", json!({ "Slo": { "SloArn": arns[0] } })),
        target("slo", json!({ "Slo": { "SloArn": arns[1] } }))
    ]);
    let service_target = json!([target(
        "service",
        json!({ "Service": service_entity(SERVICES[0]) })
    )]);

    vec![
        Equivalent {
            read: "ListAuditFindings",
            list_member: "AuditFindings",
            difference: "its targets listed in another order, one of them named twice",
            issuer: audit_request(
                "two objectives",
                &window,
                objectives_by_name.clone(),
                json!({}),
            ),
            resumed: audit_request(
                "the same objectives reordered and repeated",
                &window,
                objectives_reordered,
                json!({}),
            ),
            key: audited_objective,
        },
        Equivalent {
            read: "ListAuditFindings",
            list_member: "AuditFindings",
            // The modelled `ServiceLevelObjectiveEntity` names an objective by `SloName` or by
            // `SloArn`, so one objective target has two spellings.
            difference: "its objective targets spelled as ARNs rather than names",
            issuer: audit_request(
                "two objectives by name",
                &window,
                objectives_by_name,
                json!({}),
            ),
            resumed: audit_request(
                "the same two objectives by ARN",
                &window,
                objectives_by_arn,
                json!({}),
            ),
            key: audited_objective,
        },
        Equivalent {
            read: "ListAuditFindings",
            list_member: "AuditFindings",
            // `DetailLevel` chooses how much context each reported finding carries, not which
            // findings are reported, so it picks no set out.
            difference: "a detail level, which picks out no set of its own",
            issuer: audit_request(
                "no detail level",
                &window,
                service_target.clone(),
                json!({}),
            ),
            resumed: audit_request(
                "the detailed level",
                &window,
                service_target,
                json!({ "DetailLevel": "DETAILED" }),
            ),
            key: audited_objective,
        },
        Equivalent {
            read: "ListServiceStates",
            list_member: "ServiceStates",
            difference: "the same filters in another order, with one value named twice",
            issuer: states_request(
                "two filters",
                start,
                end,
                Some(json!([
                    { "AttributeFilterName": "Environment", "AttributeFilterValues": [ENVIRONMENT] },
                    { "AttributeFilterName": "Type", "AttributeFilterValues": ["Service"] }
                ])),
            ),
            resumed: states_request(
                "the same two filters, reordered",
                start,
                end,
                Some(json!([
                    { "AttributeFilterName": "Type", "AttributeFilterValues": ["Service", "Service"] },
                    { "AttributeFilterName": "Environment", "AttributeFilterValues": [ENVIRONMENT] }
                ])),
            ),
            key: stated_service,
        },
        Equivalent {
            read: "ListServiceOperations",
            list_member: "ServiceOperations",
            difference: "nothing at all, so the token resumes the selection it was issued for",
            issuer: Request {
                selection: SERVICES[0].to_string(),
                method: "POST",
                path: format!("/service-operations?{window}"),
                page_members: PageMembers::Query,
                body: Some(json!({ "KeyAttributes": service_entity(SERVICES[0]) })),
            },
            resumed: Request {
                selection: SERVICES[0].to_string(),
                method: "POST",
                path: format!("/service-operations?{window}"),
                page_members: PageMembers::Query,
                body: Some(json!({ "KeyAttributes": service_entity(SERVICES[0]) })),
            },
            key: whole_entry,
        },
    ]
}

/// A token issued over one selection is not an offset into another: replaying it is refused,
/// the other selection's own page is left as it was, and the token still resumes the selection
/// it was issued for.
#[tokio::test]
async fn a_token_issued_over_one_selection_is_refused_for_another() {
    let router = router();
    seeded(&router).await;
    let now = now_seconds();

    for case in varied_selections(now) {
        let Case {
            read,
            list_member,
            facet,
            selections: [issuer, other],
        } = &case;
        let label = format!(
            "{read} differing in {facet}: token of {:?} on {:?}",
            issuer.selection, other.selection
        );

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
            "case {label}: expected the other selection's own first page to succeed, got {status} with {before}"
        );
        assert_eq!(
            entries(&before, list_member).len(),
            1,
            "case {label}: expected the other selection to hold a first page of its own, got {before}"
        );

        let (status, refusal) = page(&router, other, 1, Some(&token)).await;
        assert_eq!(
            status, 400,
            "case {label}: expected HTTP 400 for a token issued over another selection, got {status} with {refusal}"
        );
        assert_eq!(
            refusal["__type"]
                .as_str()
                .map(|kind| kind.rsplit('#').next_back().unwrap_or(kind)),
            Some("ValidationException"),
            "case {label}: expected a ValidationException, got {refusal}"
        );
        assert!(
            refusal["message"]
                .as_str()
                .is_some_and(|message| message.contains("NextToken")),
            "case {label}: expected the refusal to name NextToken, got {refusal}"
        );
        assert!(
            refusal.get(*list_member).is_none() && refusal.get("NextToken").is_none(),
            "case {label}: a refused cursor carries no page and no cursor, got {refusal}"
        );

        let (status, after) = page(&router, other, 1, None).await;
        assert_eq!(
            status, 200,
            "case {label}: expected the read after the refusal to succeed, got {status} with {after}"
        );
        assert_eq!(
            entries(&after, list_member),
            entries(&before, list_member),
            "case {label}: expected the refused token to leave the other selection's page as it was, got {after}"
        );

        // The same token still counts in the set it WAS issued over, so the refusal above is
        // earned by the selection it was replayed against and by nothing else.
        let (status, resumed) = page(&router, issuer, 1, Some(&token)).await;
        assert_eq!(
            status, 200,
            "case {label}: expected the issuing selection to honour its own token, got {status} with {resumed}"
        );
        assert_eq!(
            entries(&resumed, list_member).len(),
            1,
            "case {label}: expected the resumed page to carry the entry after the first, got {resumed}"
        );
        assert_ne!(
            entries(&resumed, list_member),
            entries(&first, list_member),
            "case {label}: expected the resumed page to advance past the first entry"
        );
    }
}

/// A token resumes the selection it was issued over however the resuming request SPELLS that
/// selection, and whatever it says about members the selection does not depend on.
#[tokio::test]
async fn a_token_resumes_every_spelling_of_the_selection_it_was_issued_over() {
    let router = router();
    let arns = seeded(&router).await;
    let now = now_seconds();

    for case in equivalent_spellings(&arns, now) {
        let Equivalent {
            read,
            list_member,
            difference,
            issuer,
            resumed,
            key,
        } = &case;
        let label = format!("{read} resumed with {difference}");

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

        let (issuer_status, by_issuer) = page(&router, issuer, 1, Some(&token)).await;
        let (status, by_other_spelling) = page(&router, resumed, 1, Some(&token)).await;
        assert_eq!(
            status, issuer_status,
            "case {label}: expected the same status for either spelling, got {status} and {issuer_status}"
        );
        assert_eq!(
            status, 200,
            "case {label}: expected the resumed read to succeed, got {status} with {by_other_spelling}"
        );
        assert_eq!(
            keyed(&by_other_spelling, list_member, *key),
            keyed(&by_issuer, list_member, *key),
            "case {label}: expected the token to resume the same page for either spelling, got {by_other_spelling} and {by_issuer}"
        );
        assert_ne!(
            keyed(&by_other_spelling, list_member, *key),
            keyed(&first, list_member, *key),
            "case {label}: expected the resumed page to advance past the first entry, got {by_other_spelling}"
        );
    }
}
