// SPDX-License-Identifier: CC0-1.0

//! Which STATUS and which error CODE this service puts on the wire, driven end to end
//! over its own `restJson1` protocol through the same `MultiServiceRouter` the
//! production emulator dispatches with.
//!
//! Three rules, each asserted over a TABLE whose rows differ along what the rule
//! generalizes over (the identifier's form, the operation, the referenced resource
//! type, the tag set); the recorded request of the issue these tests close is ONE row
//! of each.
//!
//! RULE 1 (the form of a service-generated identifier is part of the request's
//! SHAPE): a `scopeId` that is not of the form this service generates — the UUID shape
//! `state::scope_id` mints, which is what the real service's "internally-generated
//! identifier"s take — names no scope under any state of the store, so EVERY operation
//! that takes one refuses it with the modelled `ValidationException` (400) before it
//! consults the store, never as the absence of a resource. The rows span the ways a
//! value can fail that form (not hex, no groups, a group short or long, a group too
//! few or too many, trailing text) and every operation of this service that carries a
//! `scopeId` label — `UpdateScope` excepted, which is still a 501 stub.
//!
//! RULE 2 (an unresolvable reference is not a malformed request): a request that names
//! an ARN-bearing resource of this service (a scope or a monitor), by id or by ARN,
//! which the store does not hold is refused with the operation's own modelled
//! resolution error: `ResourceNotFoundException` (404) where the operation models one,
//! and `AccessDeniedException` (403) where it models none — a reference the service
//! cannot resolve it also cannot authorize, and a scope and a monitor each carry their
//! own ARN and so their own resource-level permission. The rows span the member the
//! reference arrives in (`scopeArn`, `scopeId`, `monitorName`), the resource type, the
//! operation, and how the reference became unresolvable (never minted, deleted, an ARN
//! of another resource type, of another service, of another Region).
//!
//! RULE 3 (a bodiless success is 204): a successful call whose output shape declares
//! NO members has no content to report, so the tag mutations answer 204 No Content.
//! Their `@http` codes are defaulted in the model, so the status is a behavioural fact
//! the recorded responses state, not something the model pins. The rows span the
//! mutation, the resource type and the tag set; the boundary is pinned too, by the
//! calls that keep 200.

use serde_json::{json, Value};
use service_com_amazonaws_networkflowmonitor_networkflowmonitor as nfm;

use crate::harness::{extra_region, targets, Answer, Emulator, ACCOUNT_ID, REGION, SIGNING_NAME};
use crate::resource_tags::{create, KINDS};

/// The status a success that reports content answers with (the protocol default the
/// model leaves in place).
const OK: u16 = 200;

/// The status a success with no content to report answers with.
const NO_CONTENT: u16 = 204;

/// The status a fault in the request's own shape answers with.
const BAD_REQUEST: u16 = 400;

/// The status a reference the service can neither resolve nor authorize answers with.
const FORBIDDEN: u16 = 403;

/// The status the absence of a resource answers with.
const NOT_FOUND: u16 = 404;

const VALIDATION: &str = "ValidationException";
const ACCESS_DENIED: &str = "AccessDeniedException";
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// A `queryId` of the form this service mints that no `StartQuery*` here minted. Every
/// row that uses it is decided by the SCOPE or MONITOR its URI names, which the
/// handler resolves before any query lookup, so the id only has to be well formed.
const UNMINTED_QUERY_ID: &str = "00000000-0000-4000-9000-ffffffffffff";

/// Whether `answer` carries `code`, whichever slot this protocol puts it in:
/// the harness's own accessor, so every suite reads the code one way.
fn is_error(answer: &Answer, code: &str) -> bool {
    answer.is_error(code)
}

/// Assert a refusal's STATUS and CODE — the two facts a client branches on.
fn assert_refusal(case: &str, answer: &Answer, status: u16, code: &str) {
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {} with body {}",
        answer.status, answer.body
    );
    assert!(
        is_error(answer, code),
        "case {case:?}: expected {code}, got {:?} / body {}",
        answer.error_type,
        answer.body
    );
}

/// The body the two workload-insights `StartQuery*` operations require: the window and
/// the two enums their models mark `@required`
/// (`smithy_query(query="operation",operation="StartQueryWorkloadInsightsTopContributors")`).
fn workload_insights_query() -> Value {
    json!({
        "startTime": "2026-10-23T17:00:00Z",
        "endTime": "2026-10-23T18:00:00Z",
        "metricName": "TIMEOUTS",
        "destinationCategory": "INTRA_AZ"
    })
}

/// The same for the monitor family, whose metric enum is `MonitorMetric`.
fn monitor_query() -> Value {
    json!({
        "startTime": "2026-10-23T17:00:00Z",
        "endTime": "2026-10-23T18:00:00Z",
        "metricName": "ROUND_TRIP_TIME",
        "destinationCategory": "INTRA_AZ"
    })
}

/// One operation of this service that carries a `scopeId` `@httpLabel`.
struct ScopeOp {
    /// The operation, named in every assertion.
    operation: &'static str,
    method: &'static str,
    /// The operation's own `@http.uri`, with `{scopeId}` and `{queryId}` to fill in.
    template: &'static str,
    /// Whether the request carries a `StartQuery*` body.
    body: bool,
    /// What this operation answers for a WELL-FORMED `scopeId` the store does not
    /// hold: its own modelled resolution error.
    absent_status: u16,
    absent_code: &'static str,
}

/// Every implemented operation of this service whose input carries a `scopeId`, with
/// the resolution error its model declares. `UpdateScope` (`PATCH /scopes/{scopeId}`)
/// is absent because it is still a 501 scaffold — it has its own issue.
const SCOPE_OPS: &[ScopeOp] = &[
    ScopeOp {
        operation: "GetScope",
        method: "GET",
        template: "/scopes/{scopeId}",
        body: false,
        absent_status: NOT_FOUND,
        absent_code: RESOURCE_NOT_FOUND,
    },
    ScopeOp {
        operation: "DeleteScope",
        method: "DELETE",
        template: "/scopes/{scopeId}",
        body: false,
        absent_status: NOT_FOUND,
        absent_code: RESOURCE_NOT_FOUND,
    },
    ScopeOp {
        operation: "StartQueryWorkloadInsightsTopContributors",
        method: "POST",
        template: "/workloadInsights/{scopeId}/topContributorsQueries",
        body: true,
        absent_status: FORBIDDEN,
        absent_code: ACCESS_DENIED,
    },
    ScopeOp {
        operation: "StartQueryWorkloadInsightsTopContributorsData",
        method: "POST",
        template: "/workloadInsights/{scopeId}/topContributorsDataQueries",
        body: true,
        absent_status: FORBIDDEN,
        absent_code: ACCESS_DENIED,
    },
    ScopeOp {
        operation: "GetQueryStatusWorkloadInsightsTopContributors",
        method: "GET",
        template: "/workloadInsights/{scopeId}/topContributorsQueries/{queryId}/status",
        body: false,
        absent_status: FORBIDDEN,
        absent_code: ACCESS_DENIED,
    },
    ScopeOp {
        operation: "GetQueryStatusWorkloadInsightsTopContributorsData",
        method: "GET",
        template: "/workloadInsights/{scopeId}/topContributorsDataQueries/{queryId}/status",
        body: false,
        absent_status: FORBIDDEN,
        absent_code: ACCESS_DENIED,
    },
    ScopeOp {
        operation: "StopQueryWorkloadInsightsTopContributors",
        method: "DELETE",
        template: "/workloadInsights/{scopeId}/topContributorsQueries/{queryId}",
        body: false,
        absent_status: FORBIDDEN,
        absent_code: ACCESS_DENIED,
    },
    ScopeOp {
        operation: "StopQueryWorkloadInsightsTopContributorsData",
        method: "DELETE",
        template: "/workloadInsights/{scopeId}/topContributorsDataQueries/{queryId}",
        body: false,
        absent_status: FORBIDDEN,
        absent_code: ACCESS_DENIED,
    },
    ScopeOp {
        operation: "GetQueryResultsWorkloadInsightsTopContributors",
        method: "GET",
        template: "/workloadInsights/{scopeId}/topContributorsQueries/{queryId}/results",
        body: false,
        absent_status: NOT_FOUND,
        absent_code: RESOURCE_NOT_FOUND,
    },
    ScopeOp {
        operation: "GetQueryResultsWorkloadInsightsTopContributorsData",
        method: "GET",
        template: "/workloadInsights/{scopeId}/topContributorsDataQueries/{queryId}/results",
        body: false,
        absent_status: NOT_FOUND,
        absent_code: RESOURCE_NOT_FOUND,
    },
];

impl ScopeOp {
    /// This operation's request for `scope_id`.
    async fn call(&self, emulator: &Emulator, scope_id: &str) -> Answer {
        let uri = self
            .template
            .replace("{scopeId}", scope_id)
            .replace("{queryId}", UNMINTED_QUERY_ID);
        let body = self.body.then(workload_insights_query);
        emulator.call(self.method, &uri, body).await
    }
}

/// Values that are NOT of the form a `scopeId` takes. Each row says what about the
/// form it breaks, so the table spans the class rather than repeating one spelling;
/// the first is the identifier the recorded request carried.
const MALFORMED_SCOPE_IDS: &[(&str, &str)] = &[
    ("an identifier of another service's form", "sc-tor-b66da349"),
    ("a bare word", "not-a-scope-id"),
    ("a single digit", "0"),
    ("a monitor-style name", "scope_1.live-2"),
    (
        "the right 32 hex digits with no groups",
        "00000000000040008000000000000001",
    ),
    ("one group short", "00000000-0000-4000-8000"),
    (
        "one group too many",
        "00000000-0000-4000-8000-000000000001-0000",
    ),
    (
        "a group one character short",
        "0000000-0000-4000-8000-000000000001",
    ),
    (
        "a group one character long",
        "000000000-0000-4000-8000-000000000001",
    ),
    (
        "a non-hex character in a group",
        "0000000g-0000-4000-8000-000000000001",
    ),
    (
        "a well-formed id with trailing text",
        "00000000-0000-4000-8000-000000000001x",
    ),
];

/// Values that ARE of the form a `scopeId` takes but name no scope in the store. The
/// hex is accepted in either case, and the version and variant nibbles are not part of
/// the form, so a client handed an id by another account's service is told the scope is
/// absent rather than that its request is malformed.
const WELL_FORMED_UNKNOWN_SCOPE_IDS: &[(&str, &str)] = &[
    (
        "an id of the shape this service mints, never minted here",
        "00000000-0000-4000-8000-00000000ffff",
    ),
    ("all-f hex", "ffffffff-ffff-4fff-8fff-ffffffffffff"),
    ("the same hex upper-cased", "FFFFFFFF-FFFF-4FFF-8FFF-FFFFFFFFFFFE"),
    (
        "other version and variant nibbles",
        "abcdef01-2345-6789-abcd-ef0123456789",
    ),
];

/// RULE 1: a `scopeId` of another form is a fault in the request's shape — 400
/// `ValidationException` — for every operation that takes one, decided before the
/// store is consulted.
#[tokio::test]
async fn a_scope_id_of_another_form_is_refused_for_its_shape() {
    let emulator = Emulator::new();
    // A real scope exists, so a refusal is about the id's form and not about an empty
    // store.
    let live = emulator
        .create_scope(json!({ "targets": targets(&[("123456789012", "us-east-1")]) }))
        .await;
    assert_eq!(
        live.status, OK,
        "the anchor scope must be created, got {} with body {}",
        live.status, live.body
    );

    for (fault, scope_id) in MALFORMED_SCOPE_IDS {
        for op in SCOPE_OPS {
            let case = format!("{} / {fault} ({scope_id:?})", op.operation);
            let answer = op.call(&emulator, scope_id).await;
            assert_refusal(&case, &answer, BAD_REQUEST, VALIDATION);
            assert_eq!(
                answer.text("message"),
                nfm::state::INVALID_SCOPE_ID,
                "case {case:?}: expected the invalid-identifier wording, got {:?}",
                answer.text("message")
            );
        }
    }

    // The complement: the SAME operations never answer that refusal for the live id,
    // so the rows above are about the form and nothing else. Each operation gets a
    // fresh store, because one of them (`DeleteScope`) removes the scope it is given.
    for op in SCOPE_OPS {
        let case = format!("{} / the live scope id", op.operation);
        let emulator = Emulator::new();
        let live = emulator
            .create_scope(json!({ "targets": targets(&[("123456789012", "us-east-1")]) }))
            .await;
        let answer = op.call(&emulator, &live.text("scopeId")).await;
        assert_ne!(
            answer.text("message"),
            nfm::state::INVALID_SCOPE_ID,
            "case {case:?}: a well-formed id must not be refused for its form, got {} with \
             body {}",
            answer.status,
            answer.body
        );
        assert_ne!(
            answer.status, FORBIDDEN,
            "case {case:?}: a scope the store holds must not be refused as unauthorizable, \
             got body {}",
            answer.body
        );
    }
}

/// RULE 2, through a `scopeId`: a WELL-FORMED id the store does not hold is the
/// operation's own resolution error — 404 where it models `ResourceNotFoundException`,
/// 403 where it models none.
#[tokio::test]
async fn a_well_formed_scope_id_the_store_does_not_hold_is_the_operations_resolution_error() {
    let emulator = Emulator::new();
    let live = emulator
        .create_scope(json!({ "targets": targets(&[("123456789012", "us-east-1")]) }))
        .await;
    let live_id = live.text("scopeId");

    for (form, scope_id) in WELL_FORMED_UNKNOWN_SCOPE_IDS {
        assert_ne!(
            *scope_id, live_id,
            "the unknown-id table must not name the live scope"
        );
        for op in SCOPE_OPS {
            let case = format!("{} / {form} ({scope_id:?})", op.operation);
            let answer = op.call(&emulator, scope_id).await;
            assert_refusal(&case, &answer, op.absent_status, op.absent_code);
        }
    }
}

/// One reference that resolves to nothing, and what the operation carrying it answers.
struct Reference {
    /// What this row varies, named in every assertion.
    case: &'static str,
    operation: &'static str,
    method: &'static str,
    /// The request URI (already carrying any unresolvable label).
    uri: String,
    body: Option<Value>,
    status: u16,
    code: &'static str,
}

/// RULE 2, through every member a reference arrives in: an ARN or a name that resolves
/// to no scope or monitor is 403 `AccessDeniedException` on the operations that model
/// no `ResourceNotFoundException`, and 404 on the ones that do.
#[tokio::test]
async fn an_unresolvable_scope_or_monitor_reference_is_the_modelled_refusal() {
    let emulator = Emulator::new();

    // A live scope and a live monitor, so every refusal below is about the reference
    // and not about an empty store.
    let scope_arn = emulator.scope_for_monitors().await;
    let live_monitor = "wire-anchor-monitor";
    let created = emulator
        .create_monitor(json!({
            "monitorName": live_monitor,
            "scopeArn": scope_arn,
            "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
        }))
        .await;
    assert_eq!(
        created.status, OK,
        "the anchor monitor must be created, got {} with body {}",
        created.status, created.body
    );
    let monitor_arn = created.text("monitorArn");

    // A scope that existed and is gone: its ARN is still well formed, so the refusal
    // is the store answering rather than the ARN's shape. It claims another Region's
    // scope slot, so the anchor scope above stays live.
    let retired = emulator
        .create_scope_in(
            extra_region(0),
            json!({ "targets": targets(&[("210987654321", "eu-west-1")]) }),
        )
        .await;
    let retired_arn = retired.text("scopeArn");
    assert_eq!(
        emulator.delete_scope(&retired.text("scopeId")).await.status,
        OK,
        "the scope to retire must delete cleanly"
    );

    // `CreateMonitor` models no `ResourceNotFoundException`, so every unresolvable
    // `scopeArn` is the modelled 403 — whatever the ARN's resource segment looks like,
    // because nothing inspects the id INSIDE an ARN.
    let unresolvable_arns: Vec<(&str, String)> = vec![
        (
            "a scope ARN for an id this store never minted",
            format!(
                "arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT_ID}:scope/\
                 00000000-0000-4000-8000-00000000ffff"
            ),
        ),
        (
            "a scope ARN whose id is not even of the generated form",
            format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT_ID}:scope/sc-tor-b66da349"),
        ),
        ("the ARN of a scope deleted earlier", retired_arn.clone()),
        (
            "a live monitor's ARN: this service's, but not a scope",
            monitor_arn.clone(),
        ),
        (
            "a scope ARN in another Region",
            scope_arn.replace(REGION, "eu-central-1"),
        ),
        (
            "a scope ARN in another account",
            scope_arn.replace(ACCOUNT_ID, "123456789012"),
        ),
        (
            "another service's ARN",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT_ID}:vpc/vpc-01234567"),
        ),
    ];

    for (index, (case, arn)) in unresolvable_arns.iter().enumerate() {
        let name = format!("wire-refused-{index}");
        let answer = emulator
            .create_monitor(json!({
                "monitorName": name,
                "scopeArn": arn,
                "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
            }))
            .await;
        assert_refusal(
            &format!("CreateMonitor / {case}"),
            &answer,
            FORBIDDEN,
            ACCESS_DENIED,
        );
        // A refusal stores nothing.
        assert_eq!(
            emulator.get_monitor(&name).await.status,
            NOT_FOUND,
            "case {case:?}: a refused create must store nothing"
        );
    }

    // The reference is what was refused: the live scope's ARN is accepted by the very
    // same request.
    let accepted = emulator
        .create_monitor(json!({
            "monitorName": "wire-accepted",
            "scopeArn": scope_arn,
            "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
        }))
        .await;
    assert_eq!(
        accepted.status, OK,
        "the live scope's ARN must be accepted, got {} with body {}",
        accepted.status, accepted.body
    );

    // The query plane, over both parent types and all four operation families: the
    // parent the URI names is resolved before the query id, so an absent parent is the
    // operation's own resolution error.
    const ABSENT_MONITOR: &str = "wire-absent-monitor";
    const ABSENT_SCOPE: &str = "00000000-0000-4000-8000-00000000fffe";
    let references = vec![
        Reference {
            case: "a monitorName the store does not hold",
            operation: "StartQueryMonitorTopContributors",
            method: "POST",
            uri: format!("/monitors/{ABSENT_MONITOR}/topContributorsQueries"),
            body: Some(monitor_query()),
            status: FORBIDDEN,
            code: ACCESS_DENIED,
        },
        Reference {
            case: "a monitorName the store does not hold",
            operation: "GetQueryStatusMonitorTopContributors",
            method: "GET",
            uri: format!(
                "/monitors/{ABSENT_MONITOR}/topContributorsQueries/{UNMINTED_QUERY_ID}/status"
            ),
            body: None,
            status: FORBIDDEN,
            code: ACCESS_DENIED,
        },
        Reference {
            case: "a monitorName the store does not hold",
            operation: "StopQueryMonitorTopContributors",
            method: "DELETE",
            uri: format!("/monitors/{ABSENT_MONITOR}/topContributorsQueries/{UNMINTED_QUERY_ID}"),
            body: None,
            status: FORBIDDEN,
            code: ACCESS_DENIED,
        },
        Reference {
            // This family DOES model `ResourceNotFoundException`, so the same absence
            // is reported as an absence.
            case: "a monitorName the store does not hold",
            operation: "GetQueryResultsMonitorTopContributors",
            method: "GET",
            uri: format!(
                "/monitors/{ABSENT_MONITOR}/topContributorsQueries/{UNMINTED_QUERY_ID}/results"
            ),
            body: None,
            status: NOT_FOUND,
            code: RESOURCE_NOT_FOUND,
        },
        Reference {
            case: "a scopeId the store does not hold",
            operation: "StartQueryWorkloadInsightsTopContributors",
            method: "POST",
            uri: format!("/workloadInsights/{ABSENT_SCOPE}/topContributorsQueries"),
            body: Some(workload_insights_query()),
            status: FORBIDDEN,
            code: ACCESS_DENIED,
        },
        Reference {
            case: "a scopeId the store does not hold",
            operation: "StopQueryWorkloadInsightsTopContributorsData",
            method: "DELETE",
            uri: format!(
                "/workloadInsights/{ABSENT_SCOPE}/topContributorsDataQueries/{UNMINTED_QUERY_ID}"
            ),
            body: None,
            status: FORBIDDEN,
            code: ACCESS_DENIED,
        },
        Reference {
            case: "a scopeId the store does not hold",
            operation: "GetQueryResultsWorkloadInsightsTopContributors",
            method: "GET",
            uri: format!(
                "/workloadInsights/{ABSENT_SCOPE}/topContributorsQueries/\
                 {UNMINTED_QUERY_ID}/results"
            ),
            body: None,
            status: NOT_FOUND,
            code: RESOURCE_NOT_FOUND,
        },
    ];

    for reference in references {
        let answer = emulator
            .call(reference.method, &reference.uri, reference.body)
            .await;
        assert_refusal(
            &format!("{} / {}", reference.operation, reference.case),
            &answer,
            reference.status,
            reference.code,
        );
    }

    // And with the LIVE monitor the same status read is not a 403: a query id that
    // names nothing under a parent that EXISTS is a different fault.
    let under_live = emulator
        .call(
            "GET",
            &format!(
                "/monitors/{live_monitor}/topContributorsQueries/{UNMINTED_QUERY_ID}/status"
            ),
            None,
        )
        .await;
    assert_ne!(
        under_live.status, FORBIDDEN,
        "a query id under a live monitor must not be refused as unauthorizable, got body {}",
        under_live.body
    );
}

/// One row of the tag-mutation table: what to apply, and the tag set the resource must
/// carry afterwards.
struct TagStep {
    case: &'static str,
    /// `Some(tags)` for a `TagResource`, `None` for an `UntagResource` of `keys`.
    tags: Option<Value>,
    keys: Vec<&'static str>,
    expected: Value,
}

/// RULE 3: every successful tag mutation answers 204 with no content — and still
/// writes, which the reader proves.
#[tokio::test]
async fn a_tag_mutation_answers_204_with_no_content_and_still_writes() {
    for kind in KINDS {
        let emulator = Emulator::new();
        let resource = create(
            &emulator,
            *kind,
            41,
            Some(&json!({ "env": "prod", "owner": "net" })),
        )
        .await;

        // The rows differ along the mutation, the number of keys, the key and value
        // form, and whether the call changes anything at all. The recorded case (one
        // key onto a resource, then that key removed) is the second and the last row.
        let steps = vec![
            TagStep {
                case: "a tag request that changes nothing",
                tags: Some(json!({})),
                keys: Vec::new(),
                expected: json!({ "env": "prod", "owner": "net" }),
            },
            TagStep {
                case: "one new key (the recorded case)",
                tags: Some(json!({ "k": "v" })),
                keys: Vec::new(),
                expected: json!({ "env": "prod", "owner": "net", "k": "v" }),
            },
            TagStep {
                case: "several keys at once, adding and replacing together",
                tags: Some(json!({ "env": "staging", "team": "net-eng", "note": "" })),
                keys: Vec::new(),
                expected: json!({
                    "env": "staging", "owner": "net", "k": "v", "team": "net-eng", "note": ""
                }),
            },
            TagStep {
                case: "the same tag request replayed (@idempotent)",
                tags: Some(json!({ "env": "staging", "team": "net-eng", "note": "" })),
                keys: Vec::new(),
                expected: json!({
                    "env": "staging", "owner": "net", "k": "v", "team": "net-eng", "note": ""
                }),
            },
            TagStep {
                case: "an untag of a key the resource does not carry",
                tags: None,
                keys: vec!["never-set"],
                expected: json!({
                    "env": "staging", "owner": "net", "k": "v", "team": "net-eng", "note": ""
                }),
            },
            TagStep {
                case: "an untag of several keys at once",
                tags: None,
                keys: vec!["team", "note", "owner"],
                expected: json!({ "env": "staging", "k": "v" }),
            },
            TagStep {
                case: "an untag of the recorded key",
                tags: None,
                keys: vec!["k"],
                expected: json!({ "env": "staging" }),
            },
        ];

        for step in steps {
            let case = format!("{kind:?}/{}", step.case);
            let answer = match &step.tags {
                Some(tags) => emulator.tag_resource(resource.arn(), tags).await,
                None => emulator.untag_resource(resource.arn(), &step.keys).await,
            };
            assert_eq!(
                answer.status, NO_CONTENT,
                "case {case:?}: expected HTTP {NO_CONTENT}, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_type, "",
                "case {case:?}: a 204 is a success and carries no error code, got {:?}",
                answer.error_type
            );
            // "No content": nothing for a client to parse — no body, or a body with no
            // members.
            let empty_body =
                answer.body.is_null() || answer.body.as_object().is_some_and(|it| it.is_empty());
            assert!(
                empty_body,
                "case {case:?}: expected no content, got body {}",
                answer.body
            );

            // A 204 is never an excuse for doing nothing: the write landed, and both
            // the tag reader and the record itself report it.
            let listing = emulator.list_tags(resource.arn()).await;
            assert_eq!(
                listing.member("tags"),
                &step.expected,
                "case {case:?}: expected {}, got {}",
                step.expected,
                listing.member("tags")
            );
            let read = resource.read(&emulator).await;
            assert_eq!(
                read.member("tags"),
                &step.expected,
                "case {case:?}: expected the record to carry {}, got {}",
                step.expected,
                read.member("tags")
            );
        }
    }
}

/// RULE 3's boundary: 204 is for a success with NO content. A success that reports
/// content keeps 200, and so does a success whose output structure is memberless but
/// whose status no recorded response states — the deletes and the query stop, which
/// keep the model's defaulted code.
#[tokio::test]
async fn a_success_outside_the_rule_keeps_200() {
    let emulator = Emulator::new();

    let scope = emulator
        .create_scope(json!({ "targets": targets(&[("123456789012", "us-east-1")]) }))
        .await;
    let scope_id = scope.text("scopeId");
    let scope_arn = scope.text("scopeArn");
    let monitor_name = "wire-boundary-monitor";
    let monitor = emulator
        .create_monitor(json!({
            "monitorName": monitor_name,
            "scopeArn": scope_arn,
            "localResources": [{ "type": "AWS::EC2::VPC", "identifier": "vpc-1" }]
        }))
        .await;

    // A started query, so the stop below has one to stop.
    let started = emulator
        .call(
            "POST",
            &format!("/monitors/{monitor_name}/topContributorsQueries"),
            Some(monitor_query()),
        )
        .await;
    let query_id = started.text("queryId");

    for (case, answer) in [
        ("CreateScope (output carries members)", &scope),
        ("CreateMonitor (output carries members)", &monitor),
        ("StartQueryMonitorTopContributors", &started),
    ] {
        assert_eq!(
            answer.status, OK,
            "case {case:?}: expected HTTP {OK}, got {} with body {}",
            answer.status, answer.body
        );
    }

    for (case, answer) in [
        ("GetScope", emulator.get_scope(&scope_id).await),
        ("GetMonitor", emulator.get_monitor(monitor_name).await),
        (
            "ListTagsForResource",
            emulator.list_tags(&scope_arn).await,
        ),
        (
            // A memberless output whose status no recording states: it keeps the
            // model's defaulted 200.
            "StopQueryMonitorTopContributors",
            emulator
                .call(
                    "DELETE",
                    &format!("/monitors/{monitor_name}/topContributorsQueries/{query_id}"),
                    None,
                )
                .await,
        ),
        ("DeleteMonitor", emulator.delete_monitor(monitor_name).await),
        ("DeleteScope", emulator.delete_scope(&scope_id).await),
    ] {
        assert_eq!(
            answer.status, OK,
            "case {case:?}: expected HTTP {OK}, got {} with body {}",
            answer.status, answer.body
        );
    }
}
