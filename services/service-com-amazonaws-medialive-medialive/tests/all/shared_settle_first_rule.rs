// SPDX-License-Identifier: CC0-1.0

//! The ONE rule every operation of this service applies before it answers:
//! bring the store up to the calling instant, then decide and report from the
//! result.
//!
//! The rule: an operation handed this service's state materialises the
//! transitions the clock had already made due BEFORE it checks a precondition,
//! writes or reports — and only those, so it never changes what a later caller
//! would otherwise have seen.
//!
//! The table spans the OPERATION rather than one resource: each row drives one
//! operation on its own engine, over a store that holds exactly one transition
//! the clock has already made due (one input whose deletion was accepted a
//! transient span ago, so the terminal `DELETED` is owed), and asserts the
//! operation left that transition materialised. The rows differ along what the
//! operation DOES (mint a resource, patch one, report one, report a collection,
//! delete one), along the resource family it is about (the SdiSource family,
//! the two monitor-template hierarchies, the engine-version catalogue) and
//! along whether the clock-owed record is one the operation reads at all — the
//! last row is the recorded `DeleteSdiSource` case, whose documented
//! precondition is judged against the very input store the clock owes the
//! transition to.
//!
//! The clock-owed record is read through the state seam
//! (`service_state_in`), which settles nothing: every READER of this service
//! settles its own read, so the seam is the only way to tell a store an
//! operation brought up to date from one it answered stale.
//!
//! Every call is driven the way a client drives it, through the crate's shared
//! [`crate::wire`] harness, so what is asserted is the emulator's wire
//! behaviour beside the store it leaves behind.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::service_state_in;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::support::resolved_input_body;
use crate::wire::{call, medialive_with_engine, DEFAULT_REGION};

/// The collections the operations under test are bound to
/// (`smithy_query(query="operation", operation="CreateCloudWatchAlarmTemplate")`
/// and the siblings of each hierarchy: POST /prod/cloudwatch-alarm-templates,
/// /prod/cloudwatch-alarm-template-groups, /prod/eventbridge-rule-templates,
/// /prod/eventbridge-rule-template-groups, /prod/sdiSources, GET
/// /prod/versions).
const ALARM_GROUPS: &str = "/prod/cloudwatch-alarm-template-groups";
const ALARM_TEMPLATES: &str = "/prod/cloudwatch-alarm-templates";
const RULE_GROUPS: &str = "/prod/eventbridge-rule-template-groups";
const RULE_TEMPLATES: &str = "/prod/eventbridge-rule-templates";
const SDI_SOURCES: &str = "/prod/sdiSources";
const VERSIONS: &str = "/prod/versions";
const INPUTS: &str = "/prod/inputs";

/// One `EventBridgeRuleTemplateEventType`, which a rule template's create makes
/// required (`smithy_query(query="shape",
/// shape="EventBridgeRuleTemplateEventType")`).
const EVENT_TYPE: &str = "MEDIALIVE_CHANNEL_ALERT";

/// The deletion phase names this suite compares, which are the `InputState`
/// values the two halves of an input's deletion report
/// (`smithy_query(query="shape", shape="InputState")`).
const DELETING: &str = "DELETING";
const DELETED: &str = "DELETED";
/// The phase of an input no delete has been accepted for, which is not a
/// deletion phase at all.
const NOT_DELETING: &str = "<no delete accepted>";

/// One request, as a client sends it.
struct Request {
    method: &'static str,
    uri: String,
    body: Option<Value>,
}

/// One row of the table, prepared: the operation, the engine its store lives
/// in, the request that drives it and the id of the input whose terminal
/// `DELETED` the clock owes that store.
struct Prepared {
    op: Op,
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    request: Request,
    probe: String,
}

/// The operation a row drives.
#[derive(Debug, Clone, Copy)]
enum Op {
    CreateAlarmTemplateGroup,
    UpdateAlarmTemplateGroup,
    GetAlarmTemplateGroup,
    ListAlarmTemplateGroups,
    CreateAlarmTemplate,
    UpdateAlarmTemplate,
    GetAlarmTemplate,
    ListAlarmTemplates,
    DeleteAlarmTemplate,
    CreateRuleTemplateGroup,
    UpdateRuleTemplateGroup,
    GetRuleTemplateGroup,
    ListRuleTemplateGroups,
    CreateRuleTemplate,
    UpdateRuleTemplate,
    GetRuleTemplate,
    ListRuleTemplates,
    DeleteRuleTemplate,
    ListVersions,
    CreateSdiSource,
    UpdateSdiSource,
    DescribeSdiSource,
    ListSdiSources,
    DeleteSdiSource,
    /// The recorded case: the source under test is the one the clock-owed input
    /// ingests through, so this delete's documented precondition ("must not be
    /// attached to any input") is judged against the store the clock owes the
    /// deletion to.
    DeleteSdiSourceTheOwedInputUses,
}

/// Every row of the table, in the order they are prepared.
const ROWS: [Op; 25] = [
    Op::CreateAlarmTemplateGroup,
    Op::UpdateAlarmTemplateGroup,
    Op::GetAlarmTemplateGroup,
    Op::ListAlarmTemplateGroups,
    Op::CreateAlarmTemplate,
    Op::UpdateAlarmTemplate,
    Op::GetAlarmTemplate,
    Op::ListAlarmTemplates,
    Op::DeleteAlarmTemplate,
    Op::CreateRuleTemplateGroup,
    Op::UpdateRuleTemplateGroup,
    Op::GetRuleTemplateGroup,
    Op::ListRuleTemplateGroups,
    Op::CreateRuleTemplate,
    Op::UpdateRuleTemplate,
    Op::GetRuleTemplate,
    Op::ListRuleTemplates,
    Op::DeleteRuleTemplate,
    Op::ListVersions,
    Op::CreateSdiSource,
    Op::UpdateSdiSource,
    Op::DescribeSdiSource,
    Op::ListSdiSources,
    Op::DeleteSdiSource,
    Op::DeleteSdiSourceTheOwedInputUses,
];

impl Op {
    /// The operation's name, as the model spells it — the case label of every
    /// assertion below.
    fn name(self) -> &'static str {
        match self {
            Self::CreateAlarmTemplateGroup => "CreateCloudWatchAlarmTemplateGroup",
            Self::UpdateAlarmTemplateGroup => "UpdateCloudWatchAlarmTemplateGroup",
            Self::GetAlarmTemplateGroup => "GetCloudWatchAlarmTemplateGroup",
            Self::ListAlarmTemplateGroups => "ListCloudWatchAlarmTemplateGroups",
            Self::CreateAlarmTemplate => "CreateCloudWatchAlarmTemplate",
            Self::UpdateAlarmTemplate => "UpdateCloudWatchAlarmTemplate",
            Self::GetAlarmTemplate => "GetCloudWatchAlarmTemplate",
            Self::ListAlarmTemplates => "ListCloudWatchAlarmTemplates",
            Self::DeleteAlarmTemplate => "DeleteCloudWatchAlarmTemplate",
            Self::CreateRuleTemplateGroup => "CreateEventBridgeRuleTemplateGroup",
            Self::UpdateRuleTemplateGroup => "UpdateEventBridgeRuleTemplateGroup",
            Self::GetRuleTemplateGroup => "GetEventBridgeRuleTemplateGroup",
            Self::ListRuleTemplateGroups => "ListEventBridgeRuleTemplateGroups",
            Self::CreateRuleTemplate => "CreateEventBridgeRuleTemplate",
            Self::UpdateRuleTemplate => "UpdateEventBridgeRuleTemplate",
            Self::GetRuleTemplate => "GetEventBridgeRuleTemplate",
            Self::ListRuleTemplates => "ListEventBridgeRuleTemplates",
            Self::DeleteRuleTemplate => "DeleteEventBridgeRuleTemplate",
            Self::ListVersions => "ListVersions",
            Self::CreateSdiSource => "CreateSdiSource",
            Self::UpdateSdiSource => "UpdateSdiSource",
            Self::DescribeSdiSource => "DescribeSdiSource",
            Self::ListSdiSources => "ListSdiSources",
            Self::DeleteSdiSource => "DeleteSdiSource",
            Self::DeleteSdiSourceTheOwedInputUses => {
                "DeleteSdiSource, on the source the owed input uses"
            }
        }
    }

    /// The status a success answers with, which is the operation's own
    /// `SUCCESS_STATUS`: a create answers `201 Created`, a monitor-template
    /// delete `204 No Content`, and every other operation here `200 OK`.
    fn success(self) -> u16 {
        match self {
            Self::CreateAlarmTemplateGroup
            | Self::CreateAlarmTemplate
            | Self::CreateRuleTemplateGroup
            | Self::CreateRuleTemplate
            | Self::CreateSdiSource => 201,
            Self::DeleteAlarmTemplate | Self::DeleteRuleTemplate => 204,
            _ => 200,
        }
    }

    /// Whether this operation only REPORTS what the store holds, so repeating it
    /// must answer exactly the same bytes.
    fn reports(self) -> bool {
        matches!(
            self,
            Self::GetAlarmTemplateGroup
                | Self::ListAlarmTemplateGroups
                | Self::GetAlarmTemplate
                | Self::ListAlarmTemplates
                | Self::GetRuleTemplateGroup
                | Self::ListRuleTemplateGroups
                | Self::GetRuleTemplate
                | Self::ListRuleTemplates
                | Self::ListVersions
                | Self::DescribeSdiSource
                | Self::ListSdiSources
        )
    }

    /// The request that drives this operation, with whatever resource it is
    /// about created first.
    async fn request(self, router: &MultiServiceRouter) -> Request {
        match self {
            Self::CreateAlarmTemplateGroup => Request {
                method: "POST",
                uri: ALARM_GROUPS.to_string(),
                body: Some(json!({ "name": "settle-alarm-group" })),
            },
            Self::UpdateAlarmTemplateGroup => Request {
                method: "PATCH",
                uri: format!("{ALARM_GROUPS}/{}", group(router, ALARM_GROUPS).await),
                body: Some(json!({ "description": "patched by the settle suite" })),
            },
            Self::GetAlarmTemplateGroup => Request {
                method: "GET",
                uri: format!("{ALARM_GROUPS}/{}", group(router, ALARM_GROUPS).await),
                body: None,
            },
            Self::ListAlarmTemplateGroups => {
                group(router, ALARM_GROUPS).await;
                Request {
                    method: "GET",
                    uri: ALARM_GROUPS.to_string(),
                    body: None,
                }
            }
            Self::CreateAlarmTemplate => {
                let group = group(router, ALARM_GROUPS).await;
                Request {
                    method: "POST",
                    uri: ALARM_TEMPLATES.to_string(),
                    body: Some(alarm_template_body(&group, "settle-alarm-template")),
                }
            }
            Self::UpdateAlarmTemplate => Request {
                method: "PATCH",
                uri: format!("{ALARM_TEMPLATES}/{}", alarm_template(router).await),
                body: Some(json!({ "description": "patched by the settle suite" })),
            },
            Self::GetAlarmTemplate => Request {
                method: "GET",
                uri: format!("{ALARM_TEMPLATES}/{}", alarm_template(router).await),
                body: None,
            },
            Self::ListAlarmTemplates => {
                alarm_template(router).await;
                Request {
                    method: "GET",
                    uri: ALARM_TEMPLATES.to_string(),
                    body: None,
                }
            }
            Self::DeleteAlarmTemplate => Request {
                method: "DELETE",
                uri: format!("{ALARM_TEMPLATES}/{}", alarm_template(router).await),
                body: None,
            },
            Self::CreateRuleTemplateGroup => Request {
                method: "POST",
                uri: RULE_GROUPS.to_string(),
                body: Some(json!({ "name": "settle-rule-group" })),
            },
            Self::UpdateRuleTemplateGroup => Request {
                method: "PATCH",
                uri: format!("{RULE_GROUPS}/{}", group(router, RULE_GROUPS).await),
                body: Some(json!({ "description": "patched by the settle suite" })),
            },
            Self::GetRuleTemplateGroup => Request {
                method: "GET",
                uri: format!("{RULE_GROUPS}/{}", group(router, RULE_GROUPS).await),
                body: None,
            },
            Self::ListRuleTemplateGroups => {
                group(router, RULE_GROUPS).await;
                Request {
                    method: "GET",
                    uri: RULE_GROUPS.to_string(),
                    body: None,
                }
            }
            Self::CreateRuleTemplate => {
                let group = group(router, RULE_GROUPS).await;
                Request {
                    method: "POST",
                    uri: RULE_TEMPLATES.to_string(),
                    body: Some(json!({
                        "name": "settle-rule-template",
                        "groupIdentifier": group,
                        "eventType": EVENT_TYPE,
                    })),
                }
            }
            Self::UpdateRuleTemplate => Request {
                method: "PATCH",
                uri: format!("{RULE_TEMPLATES}/{}", rule_template(router).await),
                body: Some(json!({ "description": "patched by the settle suite" })),
            },
            Self::GetRuleTemplate => Request {
                method: "GET",
                uri: format!("{RULE_TEMPLATES}/{}", rule_template(router).await),
                body: None,
            },
            Self::ListRuleTemplates => {
                rule_template(router).await;
                Request {
                    method: "GET",
                    uri: RULE_TEMPLATES.to_string(),
                    body: None,
                }
            }
            Self::DeleteRuleTemplate => Request {
                method: "DELETE",
                uri: format!("{RULE_TEMPLATES}/{}", rule_template(router).await),
                body: None,
            },
            Self::ListVersions => Request {
                method: "GET",
                uri: VERSIONS.to_string(),
                body: None,
            },
            Self::CreateSdiSource => Request {
                method: "POST",
                uri: SDI_SOURCES.to_string(),
                body: Some(json!({ "name": "settle-sdi-source" })),
            },
            Self::UpdateSdiSource => Request {
                method: "PUT",
                uri: format!("{SDI_SOURCES}/{}", sdi_source(router, "settle-sdi").await),
                body: Some(json!({ "name": "settle-sdi-renamed" })),
            },
            Self::DescribeSdiSource => Request {
                method: "GET",
                uri: format!("{SDI_SOURCES}/{}", sdi_source(router, "settle-sdi").await),
                body: None,
            },
            Self::ListSdiSources => {
                sdi_source(router, "settle-sdi").await;
                Request {
                    method: "GET",
                    uri: SDI_SOURCES.to_string(),
                    body: None,
                }
            }
            Self::DeleteSdiSource | Self::DeleteSdiSourceTheOwedInputUses => Request {
                method: "DELETE",
                uri: format!("{SDI_SOURCES}/{}", sdi_source(router, "settle-sdi").await),
                body: None,
            },
        }
    }

    /// The input this row's store owes the terminal `DELETED` to, created and
    /// deleted here so the clock has one transition to make due.
    ///
    /// Every row's probe is an input of its own account: the recorded row's
    /// probe INGESTS through the source its delete is about (so the precondition
    /// that delete is judged by is a derivation over this very record), and
    /// every other row's is an input nothing else names.
    async fn probe(self, router: &MultiServiceRouter, request: &Request) -> String {
        let body = match self {
            Self::DeleteSdiSourceTheOwedInputUses => json!({
                "name": "settle-probe-input",
                "type": "SDI",
                "sdiSources": [last_segment(&request.uri)],
            }),
            _ => json!({
                "name": "settle-probe-input",
                "type": "URL_PULL",
            }),
        };
        let body = resolved_input_body(router, body).await;
        let (status, created) = call(router, "POST", INPUTS, Some(body)).await;
        assert_eq!(
            status, 201,
            "case {:?}: expected the probe input to be created (201), got {status}: {created}",
            self.name()
        );
        let id = created["input"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let (status, deleted) = call(router, "DELETE", &format!("{INPUTS}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the probe input's delete to be accepted (200), got {status}: {deleted}",
            self.name()
        );
        assert_eq!(
            deleted["state"],
            json!(DELETING),
            "case {:?}: expected the accepted delete to report {DELETING}, got {:?}",
            self.name(),
            deleted["state"]
        );
        id
    }
}

/// The last path segment of `uri` — the resource id a row's request addresses.
fn last_segment(uri: &str) -> String {
    uri.rsplit('/').next().unwrap_or_default().to_string()
}

/// One monitor-template group in `collection`, answering its minted id.
async fn group(router: &MultiServiceRouter, collection: &str) -> String {
    let (status, group) = call(
        router,
        "POST",
        collection,
        Some(json!({ "name": format!("settle-group-in-{}", last_segment(collection)) })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected a group in {collection} to be created (201), got {status}: {group}"
    );
    group["id"].as_str().unwrap_or_default().to_string()
}

/// A `CreateCloudWatchAlarmTemplate` body carrying every member the model makes
/// required (`smithy_query(query="operation",
/// operation="CreateCloudWatchAlarmTemplate")`: ComparisonOperator,
/// EvaluationPeriods, GroupIdentifier, MetricName, Name, Period, Statistic,
/// TargetResourceType, Threshold, TreatMissingData).
fn alarm_template_body(group: &str, name: &str) -> Value {
    json!({
        "name": name,
        "groupIdentifier": group,
        "comparisonOperator": "GreaterThanOrEqualToThreshold",
        "evaluationPeriods": 3,
        "metricName": "InputVideoFrameRate",
        "period": 60,
        "statistic": "Average",
        "targetResourceType": "MEDIALIVE_CHANNEL",
        "threshold": 29.97,
        "treatMissingData": "missing",
    })
}

/// One cloudwatch alarm template, in a group of its own, answering its minted
/// id.
async fn alarm_template(router: &MultiServiceRouter) -> String {
    let group = group(router, ALARM_GROUPS).await;
    let (status, template) = call(
        router,
        "POST",
        ALARM_TEMPLATES,
        Some(alarm_template_body(&group, "settle-alarm-template")),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected CreateCloudWatchAlarmTemplate to answer 201, got {status}: {template}"
    );
    template["id"].as_str().unwrap_or_default().to_string()
}

/// One eventbridge rule template, in a group of its own, answering its minted
/// id.
async fn rule_template(router: &MultiServiceRouter) -> String {
    let group = group(router, RULE_GROUPS).await;
    let (status, template) = call(
        router,
        "POST",
        RULE_TEMPLATES,
        Some(json!({
            "name": "settle-rule-template",
            "groupIdentifier": group,
            "eventType": EVENT_TYPE,
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected CreateEventBridgeRuleTemplate to answer 201, got {status}: {template}"
    );
    template["id"].as_str().unwrap_or_default().to_string()
}

/// One SdiSource named `name`, answering its minted id.
async fn sdi_source(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(router, "POST", SDI_SOURCES, Some(json!({ "name": name }))).await;
    assert_eq!(
        status, 201,
        "setup: expected CreateSdiSource to answer 201, got {status}: {created}"
    );
    created["sdiSource"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The deletion phase the STORE holds `input` in, read through the state seam,
/// which settles nothing — so this is the record as the last operation left it,
/// not as a reader would settle it.
fn stored_phase(engine: &engine::InMemoryEngine, input: &str) -> &'static str {
    let region = service_state_in(engine, DEFAULT_REGION).expect("the medialive store");
    let region = region.lock();
    let record = region
        .inputs
        .get(input)
        .unwrap_or_else(|| panic!("the store holds no input {input}"));
    if record.is_deleted() {
        DELETED
    } else if record.is_deleting() {
        DELETING
    } else {
        NOT_DELETING
    }
}

/// Rule: an operation handed this service's state materialises the transitions
/// the clock had already made due before it answers, so the store it leaves
/// behind — and the answer it gives — are the ones a reader at that instant
/// would see.
///
/// Every row prepares its own engine, so the settle one row's operation runs
/// cannot settle another row's probe, and the clock is let run ONCE for the
/// whole table rather than once per row.
#[tokio::test]
async fn every_operation_settles_the_clock_before_it_answers() {
    let mut table: Vec<Prepared> = Vec::with_capacity(ROWS.len());
    for op in ROWS {
        let (router, engine) = medialive_with_engine();
        let request = op.request(&router).await;
        let probe = op.probe(&router, &request).await;
        table.push(Prepared {
            op,
            router,
            engine,
            request,
            probe,
        });
    }

    wait_for_the_clock();

    for row in table {
        let case = row.op.name();

        // Nothing has touched this store since the delete was accepted, so the
        // transition is DUE but not yet materialised: this is the stale store
        // the rule is about.
        let before = stored_phase(&row.engine, &row.probe);
        assert_eq!(
            before, DELETING,
            "case {case:?}: expected the store to still hold the probe input in \
             {DELETING} before the call, got {before:?}"
        );

        let (status, body) = call(
            &row.router,
            row.request.method,
            &row.request.uri,
            row.request.body.clone(),
        )
        .await;
        assert_eq!(
            status,
            row.op.success(),
            "case {case:?}: expected {} from {} {}, got {status}: {body}",
            row.op.success(),
            row.request.method,
            row.request.uri
        );

        let after = stored_phase(&row.engine, &row.probe);
        assert_eq!(
            after, DELETED,
            "case {case:?}: expected the call to have materialised the due \
             transition, leaving the probe input {DELETED}, got {after:?}"
        );
    }
}

/// Rule, second half: the settle an operation runs applies only transitions
/// that were owed at that instant anyway, so it never changes what a later
/// caller would otherwise have seen — a reporting operation asked twice over a
/// store that holds a due transition answers the same bytes both times, the
/// first read being the one that materialises it.
///
/// Prepared exactly as the table above is, so each row's store really does owe
/// the first of its two reads a transition: a read that applied MORE than what
/// the clock owed would answer differently the second time.
#[tokio::test]
async fn a_reporting_operation_reports_the_same_store_twice() {
    let mut table: Vec<Prepared> = Vec::new();
    for op in ROWS.into_iter().filter(|op| op.reports()) {
        let (router, engine) = medialive_with_engine();
        let request = op.request(&router).await;
        let probe = op.probe(&router, &request).await;
        table.push(Prepared {
            op,
            router,
            engine,
            request,
            probe,
        });
    }

    wait_for_the_clock();

    for row in table {
        let Prepared {
            op,
            router,
            engine,
            request,
            probe,
        } = row;
        let case = op.name();
        let before = stored_phase(&engine, &probe);
        assert_eq!(
            before, DELETING,
            "case {case:?}: expected the store to still owe the probe input's \
             deletion before the first read, got {before:?}"
        );

        let (first_status, first) = call(
            &router,
            request.method,
            &request.uri,
            request.body.clone(),
        )
        .await;
        assert_eq!(
            first_status,
            op.success(),
            "case {case:?}: expected {} from the first read, got {first_status}: {first}",
            op.success()
        );

        let (second_status, second) = call(
            &router,
            request.method,
            &request.uri,
            request.body.clone(),
        )
        .await;
        assert_eq!(
            second_status, first_status,
            "case {case:?}: expected the repeated read to answer {first_status}, \
             got {second_status}: {second}"
        );
        assert_eq!(
            second, first,
            "case {case:?}: expected the repeated read to report the same store, \
             got {second} after {first}"
        );
    }
}
