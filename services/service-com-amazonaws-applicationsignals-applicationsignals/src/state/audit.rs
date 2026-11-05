// SPDX-License-Identifier: CC0-1.0

//! The SLO auditor's findings: what an audit of an entity assesses and reports.

use super::*;

/// The auditor this emulator holds the input of: the one that assesses the SERVICE LEVEL
/// OBJECTIVES of the audited entity.
///
/// The name is the one the API configures that auditor by, and the only one of the
/// configurable auditors whose input is a resource this service STORES
/// (`smithy_query(query="doc", shape="ListAuditFindingsInput", member="Auditors")`: "`slo` -
/// SloAuditor: Identifies SLO violations and detects breached thresholds during the
/// Assessment phase"). Every other auditor that doc names reads telemetry — the request,
/// error and latency metrics, the traces, the EMF logs and the change events an instrumented
/// application sends Application Signals — which this emulator never receives, so each of
/// them produces no result and an `Auditors` filter naming only those reports nothing.
pub const SLO_AUDITOR: &str = "slo";

/// The `Severity` of an auditor result that reports NO issue
/// (`smithy_query(shape="Severity")` -> `["CRITICAL", "HIGH", "MEDIUM", "LOW", "NONE"]`).
pub(super) const SEVERITY_NONE: &str = "NONE";

/// [`SEVERITY_NONE`], for a result whose assessed objective is in the state its own goal asked
/// to be warned about: a warning is the lesser of the two issues this auditor can report.
pub(super) const SEVERITY_WARNING: &str = "MEDIUM";

/// [`SEVERITY_NONE`], for a result whose assessed objective has exhausted its error budget,
/// which is the most severe thing the SLO auditor can observe — the objective is unmet.
pub(super) const SEVERITY_BREACHED: &str = "CRITICAL";

/// The `DetailLevel` that asks for the fullest per-finding context
/// (`smithy_query(shape="DetailLevel")` -> `["BRIEF", "DETAILED"]`).
pub const DETAIL_LEVEL_DETAILED: &str = "DETAILED";

/// The `AuditorResult.Data` entries the SLO auditor reports: "additional data"
/// (`smithy_query(shape="AuditorResult")`), each read off the assessed record, named after
/// the member of that record it carries.
pub(super) const AUDITOR_DATA_SLO_NAME: &str = "SloName";
/// [`AUDITOR_DATA_SLO_NAME`], for the assessed objective's ARN.
pub(super) const AUDITOR_DATA_SLO_ARN: &str = "SloArn";
/// [`AUDITOR_DATA_SLO_NAME`], for the attainment goal it is assessed against.
pub(super) const AUDITOR_DATA_ATTAINMENT_GOAL: &str = "AttainmentGoal";
/// [`AUDITOR_DATA_SLO_NAME`], for the budget status the assessment reached.
pub(super) const AUDITOR_DATA_BUDGET_STATUS: &str = "BudgetStatus";

/// The kind of audit target a finding was produced for, as `AuditFinding.Type` reports it.
///
/// The tokens are the ones the target's own type member enumerates — "The type of entity
/// being audited, such as `service`, `SLO`, `service_operation`, or `canary`"
/// (`smithy_query(query="doc", shape="AuditTarget", member="Type")`) — each spelled exactly
/// as that doc spells it, so a client comparing against the documented set matches. The kind
/// is read off the modelled `AuditTargetEntity` UNION the request carried, not off the
/// target's free-form `Type` string, so the finding states the kind of entity that was
/// actually resolved.
pub(super) fn audit_target_kind(target: &AuditTargetEntity) -> &'static str {
    match target {
        AuditTargetEntity::Service(_) => "service",
        AuditTargetEntity::Slo(_) => "SLO",
        AuditTargetEntity::ServiceOperation(_) => "service_operation",
        AuditTargetEntity::Canary(_) => "canary",
    }
}

/// The `NextToken` tag `ListAuditFindings` issues its page tokens under: the read's own
/// [`AUDIT_FINDINGS_TOKEN_TAG`]-style tag plus a fingerprint of the SELECTION the page was taken
/// over — the targets the request named, the auditors it filtered by and the period it covered.
///
/// The reported findings are a function of all three
/// ([`ComAmazonawsApplicationsignalsApplicationsignalsState::audited_findings`]), so each is
/// bound into the tag for the reason [`subject_token_tag`] states: the cursor is a bare offset,
/// so a token issued over one selection would silently skip another's first findings if it were
/// honoured. `DetailLevel` is NOT part of the selection — it chooses how much context each
/// reported finding carries, not which findings are reported nor in what order — so a token
/// stays honoured whatever detail level either request asked for.
///
/// An objective target is fingerprinted on the NAME its identity resolves to
/// ([`slo_name_of`]), not on the spelling the request used, so a token handed back for a target
/// naming an objective by name resumes a read naming the same objective by ARN. An identity
/// that resolves to no objective of this account is fingerprinted on the spelling itself,
/// since it selects a different set from the name it spells. The targets are
/// fingerprinted as a SET — each one rendered, then the renderings sorted and deduplicated —
/// and the auditor names likewise, because neither the order the request listed them in nor a
/// target or an auditor it named twice changes which findings are reported or in what order:
/// the findings are keyed by the name of the objective each assessed
/// ([`ComAmazonawsApplicationsignalsApplicationsignalsState::audited_findings`]).
pub fn audit_findings_token_tag(
    tag: &str,
    targets: &[AuditTarget],
    auditors: Option<&[String]>,
    window_start: &DateTime,
    window_end: &DateTime,
) -> String {
    let mut rendered: Vec<Vec<Option<String>>> = Vec::with_capacity(targets.len());
    for target in targets {
        // The kind of entity the target names is the modelled UNION member it carried, which
        // is also what says how the fields after it are read. The target's own free-form
        // `Type` is not bound: the projection is resolved off the union
        // ([`audit_target_kind`]), so that member selects nothing.
        let mut fields: Vec<Option<String>> =
            vec![Some(audit_target_kind(&target.data).to_string())];
        match &target.data {
            AuditTargetEntity::Service(entity) => {
                fields.extend(audited_entity_fields(Some(entity)));
            }
            AuditTargetEntity::Slo(entity) => {
                let mut names: Vec<&str> = [entity.slo_name.as_deref(), entity.slo_arn.as_deref()]
                    .into_iter()
                    .flatten()
                    .map(|id| slo_name_of(id).unwrap_or(id))
                    .collect();
                names.sort_unstable();
                names.dedup();
                fields.push(Some(names.len().to_string()));
                fields.extend(names.into_iter().map(|name| Some(name.to_string())));
            }
            AuditTargetEntity::ServiceOperation(entity) => {
                fields.extend(audited_entity_fields(entity.service.as_ref()));
                fields.push(entity.operation.clone());
                fields.push(entity.metric_type.clone());
            }
            AuditTargetEntity::Canary(entity) => {
                fields.push(Some(entity.canary_name.clone()));
            }
        }
        rendered.push(fields);
    }
    rendered.sort_unstable();
    rendered.dedup();

    // The number of targets precedes them, so the auditor filter and the period that follow are
    // read in their own places however many fields the targets took.
    let mut fields: Vec<Option<String>> = vec![Some(rendered.len().to_string())];
    fields.extend(rendered.into_iter().flatten());
    match auditors {
        // An absent filter is not an empty one: it names no auditor to filter by, while an
        // empty list names none to keep ([`retaining_auditors`]).
        None => fields.push(None),
        Some(named) => {
            let mut names: Vec<&str> = named.iter().map(String::as_str).collect();
            names.sort_unstable();
            names.dedup();
            fields.push(Some(names.len().to_string()));
            fields.extend(names.into_iter().map(|name| Some(name.to_string())));
        }
    }
    fields.extend(
        window_fields(window_start, window_end)
            .into_iter()
            .map(Some),
    );
    subject_token_tag(tag, &borrowed_fields(&fields))
}

/// The fields one audited `ServiceEntity` contributes to a cursor's tag: the attributes it
/// names, each in a place of its own.
///
/// An entity that leaves an attribute unnamed selects a different set from one that names it,
/// since the identity is matched as a SUBSET of a stored one ([`attributes_contain`]) — so an
/// absent attribute is an absent field rather than a missing one. Always four fields, whether
/// or not the target carried an entity at all, so the fields of whatever follows are still
/// read in their own places.
pub(super) fn audited_entity_fields(entity: Option<&ServiceEntity>) -> [Option<String>; 4] {
    [
        entity.and_then(|entity| entity.r#type.clone()),
        entity.and_then(|entity| entity.name.clone()),
        entity.and_then(|entity| entity.environment.clone()),
        entity.and_then(|entity| entity.aws_account_id.clone()),
    ]
}

/// The key attributes a `ServiceEntity` names, or `None` when it names no entity of this
/// store.
///
/// The shape's members ARE `Attributes` entries: `Type` "the type of the service entity",
/// `Name` "the name of the service" and `Environment` the location it is hosted in
/// (`smithy_query(shape="ServiceEntity")`), so a target's service entity becomes the same
/// kind of identity map `GetService` and `ListServiceOperations` take, and an absent member
/// simply names one fewer attribute ([`attributes_contain`] matches on a subset).
///
/// `None` for an entity that names NO attribute at all, since an empty map would match every
/// stored identity rather than one, and `None` for an entity in another account: the member
/// is "the Amazon Web Services account ID where the service is located. Provide this value
/// only for cross-account access", and this store holds one account's records, so another
/// account's entity is one it holds nothing for ([`names_other_account`], the one reader of
/// that rule).
pub(super) fn audited_entity_attributes(entity: &ServiceEntity) -> Option<HashMap<String, String>> {
    if names_other_account(entity.aws_account_id.as_deref()) {
        return None;
    }
    let mut attributes = HashMap::new();
    for (attribute, value) in [
        (TYPE_ATTRIBUTE, entity.r#type.as_deref()),
        (NAME_ATTRIBUTE, entity.name.as_deref()),
        (ENVIRONMENT_ATTRIBUTE, entity.environment.as_deref()),
    ] {
        if let Some(value) = value {
            attributes.insert(attribute.to_string(), value.to_string());
        }
    }
    (!attributes.is_empty()).then_some(attributes)
}

/// The SLO auditor's result for one assessed objective, over the period audited and as of
/// the instant `now` the audit is served at.
///
/// The auditor "identifies SLO violations and detects breached thresholds" (see
/// [`SLO_AUDITOR`]), so what it assesses is the objective's error budget over that period —
/// the one quantity a budget report reports as `BudgetStatus`, derived by the one rule this
/// module owns ([`budget_assessment`]) and reported in the `Data` entry of that name. The
/// result's `Description` and `Severity` state the SAME assessment in words and on the
/// modelled severity scale, so no part of a result contradicts another, and an audit and a
/// budget report of one objective at one instant cannot contradict each other.
///
/// Every other value in `Data` is read off the assessed record, so two objectives' results
/// differ exactly as their stored configuration does. The attainment goal is the one the
/// objective is evaluated against, which is the stored goal with the defaults the member
/// documents filled in ([`goal_with_defaults`]), so the result cannot disagree with what
/// `GetServiceLevelObjective` reports for the same objective.
pub(super) fn slo_auditor_result(
    slo: &ServiceLevelObjective,
    window_start: &DateTime,
    window_end: &DateTime,
    now: &DateTime,
) -> AuditorResult {
    let assessment = budget_assessment(slo, Some(window_start), window_end, now);
    let mut data = HashMap::from([
        (AUDITOR_DATA_SLO_NAME.to_string(), slo.name.clone()),
        (AUDITOR_DATA_SLO_ARN.to_string(), slo.arn.clone()),
        (
            AUDITOR_DATA_BUDGET_STATUS.to_string(),
            assessment.budget_status.to_string(),
        ),
    ]);
    if let Some(attainment_goal) = goal_with_defaults(Some(&slo.goal)).attainment_goal {
        data.insert(
            AUDITOR_DATA_ATTAINMENT_GOAL.to_string(),
            attainment_goal.to_string(),
        );
    }
    AuditorResult {
        auditor: Some(SLO_AUDITOR.to_string()),
        description: Some(assessed_description(&slo.name, assessment.budget_status)),
        data: Some(data),
        severity: Some(assessed_severity(assessment.budget_status).to_string()),
    }
}

/// The `Description` of the SLO auditor's result: "a detailed description of the audit
/// finding, explaining what was observed and potential implications"
/// (`smithy_query(shape="AuditorResult")`) — which is the budget status the assessment
/// reached, stated for the objective it was reached for.
pub(super) fn assessed_description(name: &str, budget_status: &str) -> String {
    match budget_status {
        BUDGET_STATUS_OK => format!(
            "The remaining error budget of service level objective {name} is above the \
             warning threshold its goal states for the requested period, so no breached \
             threshold is reported.",
        ),
        BUDGET_STATUS_WARNING => format!(
            "The remaining error budget of service level objective {name} is below the \
             warning threshold its goal states for the requested period.",
        ),
        BUDGET_STATUS_BREACHED => format!(
            "Service level objective {name} has exhausted its error budget for the requested \
             period, so its attainment goal is breached.",
        ),
        // The remaining modelled value: an objective whose attainment has no evaluated
        // datapoint over the period, which is the assessment nothing was measured for.
        _ => format!(
            "No attainment datapoint is recorded for service level objective {name} in the \
             requested period, so no breached threshold is reported.",
        ),
    }
}

/// The `Severity` of the SLO auditor's result: "the severity level of this audit finding,
/// indicating the importance and potential impact of the issue"
/// (`smithy_query(shape="AuditorResult")`) — so it is the severity of the budget status the
/// assessment reached, and an assessment that found no issue reports the modelled value for
/// none rather than claiming a violation nothing measured.
pub(super) fn assessed_severity(budget_status: &str) -> &'static str {
    match budget_status {
        BUDGET_STATUS_WARNING => SEVERITY_WARNING,
        BUDGET_STATUS_BREACHED => SEVERITY_BREACHED,
        _ => SEVERITY_NONE,
    }
}

/// The `length` bound `MetricDataQuery.Id` carries
/// (`smithy_query(query="shape", shape="MetricDataQuery")`, member `Id`: `length 1..255`),
/// which a reported id has to stay inside however it was derived.
pub(super) const MAX_METRIC_ID_LENGTH: usize = 255;

/// Several independently scoped `MetricDataQueries` arrays, reported as ONE such array.
///
/// `MetricDataQuery.Id` "must be unique within a `MetricDataQueries` array"
/// (`smithy_query(query="shape", shape="MetricDataQuery")`, member `Id`), and each stored
/// array is a `MetricDataQueries` of its own — so its ids are unique only within itself and
/// two of them may use one id while each still satisfies the rule. Reporting them in a
/// single array makes that one array the scope the rule is stated for, so a query whose id
/// an earlier reported query already took is reported under a fresh id derived from its own
/// ([`unique_metric_query_id`]) rather than repeated: the id is "a short name used to tie
/// this object to the results in the response", so a repeated one leaves a client keying the
/// reported queries by it holding one query where the objective stores two.
///
/// An id is also how the other queries of the SAME stored array refer to a query in a math
/// expression ("a math expression can use the `Id` of the other metrics or queries to refer
/// to those metrics"), so a renamed id is rewritten in those references
/// ([`with_renamed_query_references`]): the reported array then describes the metric math
/// its source arrays describe, rather than expressions pointing at another array's query.
///
/// Nothing else about a query changes, and the order is the source arrays' own, each array's
/// queries in the order it holds them.
pub(super) fn merged_metric_data_queries(sources: &[&[MetricDataQuery]]) -> Vec<MetricDataQuery> {
    let mut merged: Vec<MetricDataQuery> = Vec::new();
    let mut taken: BTreeSet<String> = BTreeSet::new();
    for source in sources {
        // The ids of a whole source array are settled BEFORE its expressions are rewritten:
        // an expression may refer to a query that appears after it in its own array, so the
        // renaming has to be known for the array as a whole.
        let mut renamed: BTreeMap<String, String> = BTreeMap::new();
        let mut reported: Vec<String> = Vec::with_capacity(source.len());
        for query in source.iter() {
            let id = unique_metric_query_id(&query.id, &taken);
            if id != query.id {
                renamed.insert(query.id.clone(), id.clone());
            }
            taken.insert(id.clone());
            reported.push(id);
        }
        for (query, id) in source.iter().zip(reported) {
            merged.push(MetricDataQuery {
                id,
                expression: query
                    .expression
                    .as_deref()
                    .map(|expression| with_renamed_query_references(expression, &renamed)),
                ..query.clone()
            });
        }
    }
    merged
}

/// `id`, or the first id derived from it that no query reported in the same array has
/// ([`merged_metric_data_queries`]).
///
/// A derived id keeps the original as its stem and appends an underscore and the number of
/// the occurrence, so an id that met the member's documented character rules ("the valid
/// characters are letters, numbers, and underscore. The first character must be a lowercase
/// letter") still meets them, and it stays inside the member's own length bound
/// ([`MAX_METRIC_ID_LENGTH`]) — the stem gives way to the suffix rather than the reported id
/// outgrowing the bound.
pub(super) fn unique_metric_query_id(id: &str, taken: &BTreeSet<String>) -> String {
    if !taken.contains(id) {
        return id.to_string();
    }
    let mut occurrence = 2usize;
    loop {
        let suffix = format!("_{occurrence}");
        let stem: String = id
            .chars()
            .take(MAX_METRIC_ID_LENGTH.saturating_sub(suffix.chars().count()))
            .collect();
        let candidate = format!("{stem}{suffix}");
        if !taken.contains(&candidate) {
            return candidate;
        }
        occurrence += 1;
    }
}

/// `expression` with every reference to a renamed query id replaced by the id that query is
/// reported under ([`merged_metric_data_queries`]).
///
/// A reference is a whole name in the expression, so a name is substituted only when the
/// renaming names it exactly — `m1` in `SUM(m1)` is the query `m1`, while the `m1` inside
/// `m10` or inside a quoted metric name is not. Only a name beginning with a lowercase
/// letter can be a reference, which is what the id's own doc requires of one ("the first
/// character must be a lowercase letter"), so a number literal and an expression function
/// are left as they are.
///
/// The expression itself is neither interpreted nor validated here: the only things read out
/// of it are where one name begins and ends and whether it stands inside a quoted literal,
/// and every other character is copied through unchanged, so what is reported is the stored
/// expression with its references brought along rather than one this read rebuilt.
pub(super) fn with_renamed_query_references(expression: &str, renamed: &BTreeMap<String, String>) -> String {
    if renamed.is_empty() {
        return expression.to_string();
    }
    let mut rewritten = String::with_capacity(expression.len());
    let mut name = String::new();
    let mut quote: Option<char> = None;
    for character in expression.chars() {
        if let Some(open) = quote {
            rewritten.push(character);
            if character == open {
                quote = None;
            }
            continue;
        }
        if character.is_ascii_alphanumeric() || character == '_' {
            name.push(character);
            continue;
        }
        push_metric_query_name(&mut rewritten, &mut name, renamed);
        if character == '\'' || character == '"' {
            quote = Some(character);
        }
        rewritten.push(character);
    }
    push_metric_query_name(&mut rewritten, &mut name, renamed);
    rewritten
}

/// One complete name read out of an expression, appended to the rewritten expression under
/// the id it is reported as when the renaming names it
/// ([`with_renamed_query_references`]).
pub(super) fn push_metric_query_name(
    rewritten: &mut String,
    name: &mut String,
    renamed: &BTreeMap<String, String>,
) {
    if name.is_empty() {
        return;
    }
    let reference = name
        .starts_with(|character: char| character.is_ascii_lowercase())
        .then(|| renamed.get(name.as_str()))
        .flatten();
    rewritten.push_str(reference.map_or(name.as_str(), String::as_str));
    name.clear();
}

/// The metric graph a DETAILED finding carries for one assessed objective: "metric data
/// queries and time range information that provides context for audit findings through
/// relevant performance metrics" (`smithy_query(shape="MetricGraph")`).
///
/// The queries are the objective's OWN stored ones — the `MetricDataQueries` of a
/// period-based indicator, or the total and monitored request-count queries of a
/// request-based one — so the graph names the metrics the assessment is evaluated from
/// rather than a catalogue this read made up, and the range is the window the request asked
/// to be audited over.
///
/// Those are SEVERAL stored arrays reported as ONE, so they are combined by
/// [`merged_metric_data_queries`], which is what keeps the reported array's ids unique.
pub(super) fn assessed_metric_graph(
    slo: &ServiceLevelObjective,
    window_start: &DateTime,
    window_end: &DateTime,
) -> MetricGraph {
    let mut sources: Vec<&[MetricDataQuery]> = Vec::new();
    if let Some(sli) = slo.sli.as_ref() {
        sources.push(&sli.sli_metric.metric_data_queries);
    }
    if let Some(request_based_sli) = slo.request_based_sli.as_ref() {
        let metric = &request_based_sli.request_based_sli_metric;
        sources.push(&metric.total_request_count_metric);
        sources.push(monitored_request_count_queries(
            &metric.monitored_request_count_metric,
        ));
    }
    let queries = merged_metric_data_queries(&sources);
    MetricGraph {
        // An absent fact is an absent member: an objective whose indicator holds no query
        // contributes no query rather than an empty list.
        metric_data_queries: (!queries.is_empty()).then_some(queries),
        start_time: Some(*window_start),
        end_time: Some(*window_end),
    }
}

/// `finding` with only the results of the named auditors left, or `None` once nothing is
/// left.
///
/// The filter is the one the member documents: "a list of auditor names to filter the
/// findings by. Only findings generated by the specified auditors will be returned" (see
/// [`SLO_AUDITOR`]) — so a finding none of whose results came from a named auditor was not
/// generated by one of them and is not reported. An ABSENT list names no filter and keeps
/// every finding; a list that is present but empty names no auditor, so it keeps none.
pub(super) fn retaining_auditors(
    mut finding: AuditFinding,
    auditors: Option<&[String]>,
) -> Option<AuditFinding> {
    let Some(auditors) = auditors else {
        return Some(finding);
    };
    let results: Vec<AuditorResult> = finding
        .auditor_results
        .take()
        .unwrap_or_default()
        .into_iter()
        .filter(|result| {
            result
                .auditor
                .as_deref()
                .is_some_and(|auditor| auditors.iter().any(|named| named == auditor))
        })
        .collect();
    if results.is_empty() {
        return None;
    }
    finding.auditor_results = Some(results);
    Some(finding)
}

impl ComAmazonawsApplicationsignalsApplicationsignalsState {
    /// The audit findings the request's targets resolve to over the window, filtered by the
    /// auditors it named and carrying the per-finding context its detail level asked for.
    ///
    /// Every finding is an assessment of ONE stored service level objective: the audited
    /// entity is the entity that objective monitors (`AuditFinding.KeyAttributes` is
    /// `@required` with `length 1..4`, so an objective whose indicator identifies no entity
    /// is reported for no target), and the result it carries is the SLO auditor's
    /// ([`slo_auditor_result`]). Which objectives a target resolves to is [`Self::audited`];
    /// a target this store holds nothing for — an objective that was never created, an
    /// entity nothing monitors, a kind of resource this service does not hold — contributes
    /// no finding, exactly as the topology reads report an entity they observed nothing for
    /// as absent.
    ///
    /// An objective named by SEVERAL targets is assessed ONCE: the findings are the
    /// assessments of a set of objectives, not one per target, and the kind reported for a
    /// repeated one is that of the first target in the request that resolved to it. The
    /// order is the objective name's — the order `slos` is keyed in — which is what makes a
    /// page cursor over this projection a stable boundary.
    pub fn audited_findings(
        &self,
        targets: &[AuditTarget],
        auditors: Option<&[String]>,
        detailed: bool,
        window_start: &DateTime,
        window_end: &DateTime,
        now: &DateTime,
    ) -> Vec<AuditFinding> {
        // `now` is the instant the audit is served at, sampled ONCE by the handler, so every
        // assessment in one answer is of the same instant. It bounds every assessed window:
        // nothing in the future has been evaluated, however far ahead the requested period
        // ends — the same rule [`window_has_begun`] applies to the period itself, on that
        // same one sample.
        let mut assessed: BTreeMap<String, &'static str> = BTreeMap::new();
        for target in targets {
            let kind = audit_target_kind(&target.data);
            for name in self.audited(&target.data, window_start, window_end, now) {
                assessed.entry(name).or_insert(kind);
            }
        }
        assessed
            .into_iter()
            .filter_map(|(name, kind)| {
                let slo = self.slos.get(&name)?;
                let entity = entity_of(slo);
                let key_attributes = entity
                    .key_attributes
                    .filter(|attributes| !attributes.is_empty())?;
                Some(AuditFinding {
                    key_attributes,
                    auditor_results: Some(vec![slo_auditor_result(
                        slo,
                        window_start,
                        window_end,
                        now,
                    )]),
                    operation: entity.operation_name,
                    // Only the SLO auditor produces a result here, and the context it
                    // reports is the metric graph of the objective it assessed. A
                    // `DependencyGraph` is the trace and critical-path auditors' artefact —
                    // built from the distributed traces this emulator never receives — so
                    // no finding here carries one.
                    metric_graph: detailed
                        .then(|| assessed_metric_graph(slo, window_start, window_end)),
                    dependency_graph: None,
                    r#type: Some(kind.to_string()),
                })
            })
            .filter_map(|finding| retaining_auditors(finding, auditors))
            .collect()
    }

    /// The names of the stored objectives one audit target resolves to over the window.
    ///
    /// Each kind of target names its entity the way the API identifies that kind:
    ///
    /// * an objective by name or ARN, resolved as every operation taking a
    ///   `ServiceLevelObjectiveId` resolves one ([`Self::resolve`]). A target that states
    ///   both must state them of ONE objective: two identities naming two records name no
    ///   single objective, and an identity that resolves to nothing resolves the whole
    ///   target to nothing.
    /// * a service by its key attributes ([`audited_entity_attributes`]), resolving to the
    ///   objectives that monitor it ([`Self::monitoring`] — the same subset match and the
    ///   same window rule the topology reads apply).
    /// * a service operation by its service AND its operation, since an operation is
    ///   identified by the service it belongs to; its optional metric type narrows further
    ///   to the objectives whose indicator measures that type.
    /// * a canary names a Synthetics resource, which this service does not hold, so it
    ///   resolves to nothing.
    pub(super) fn audited(
        &self,
        target: &AuditTargetEntity,
        window_start: &DateTime,
        window_end: &DateTime,
        now: &DateTime,
    ) -> Vec<String> {
        // The request's own half of the window test, decided ONCE for the whole answer from
        // the instant this read is served at: a period that has not begun then evaluated
        // nothing, so no target resolves to an objective in it ([`window_has_begun`], the
        // same decision [`Self::monitoring`] below applies).
        if !window_has_begun(window_start, now) {
            return Vec::new();
        }
        match target {
            AuditTargetEntity::Slo(entity) => {
                let mut resolved: Option<&ServiceLevelObjective> = None;
                for id in [entity.slo_name.as_deref(), entity.slo_arn.as_deref()]
                    .into_iter()
                    .flatten()
                {
                    let Some(slo) = self.resolve(id) else {
                        return Vec::new();
                    };
                    if resolved.is_some_and(|already| already.name != slo.name) {
                        return Vec::new();
                    }
                    resolved = Some(slo);
                }
                resolved
                    .filter(|slo| monitored_in_window(slo, window_end))
                    .map(|slo| vec![slo.name.clone()])
                    .unwrap_or_default()
            }
            AuditTargetEntity::Service(entity) => {
                let Some(attributes) = audited_entity_attributes(entity) else {
                    return Vec::new();
                };
                self.monitoring(&attributes, window_start, window_end, now)
                    .map(|slo| slo.name.clone())
                    .collect()
            }
            AuditTargetEntity::ServiceOperation(entity) => {
                let (Some(service), Some(operation)) =
                    (entity.service.as_ref(), entity.operation.as_deref())
                else {
                    return Vec::new();
                };
                let Some(attributes) = audited_entity_attributes(service) else {
                    return Vec::new();
                };
                self.monitoring(&attributes, window_start, window_end, now)
                    .filter(|slo| {
                        let assessed = entity_of(slo);
                        assessed.operation_name.as_deref() == Some(operation)
                            && entity.metric_type.as_deref().is_none_or(|metric_type| {
                                assessed.metric_type.as_deref() == Some(metric_type)
                            })
                    })
                    .map(|slo| slo.name.clone())
                    .collect()
            }
            AuditTargetEntity::Canary(_) => Vec::new(),
        }
    }
}
