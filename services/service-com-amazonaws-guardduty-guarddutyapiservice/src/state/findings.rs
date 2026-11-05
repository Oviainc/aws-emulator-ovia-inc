// SPDX-License-Identifier: CC0-1.0

//! Findings and the criteria that select them: the stored finding, the attribute
//! lookup every filter and sort resolves through, and the saved finding filter.
//!
//! One file for the whole finding-criteria surface, so the operations that list,
//! filter, sort and save criteria cannot disagree about which field a criterion
//! names or what a condition means.

use super::*;

/// One stored finding. Timestamps are kept as epoch milliseconds — the single
/// source for both the ISO-8601 strings the wire carries and the numeric
/// comparisons `FindingCriteria` makes.
#[derive(Debug, Clone)]
pub struct FindingRecord {
    pub id: String,
    pub arn: String,
    pub account_id: String,
    pub region: String,
    pub partition: String,
    pub schema_version: String,
    pub r#type: String,
    pub severity: f64,
    pub confidence: Option<f64>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub resource_type: String,
    pub detector_id: String,
    pub resource_role: String,
    /// `Service.Count`: "The total count of the occurrences of this finding type"
    /// (`smithy_query(shape="Service")`).
    pub count: i32,
    pub created_at_millis: i64,
    pub updated_at_millis: i64,
    /// `Service.EventLastSeen`: when the ACTIVITY this finding reports was last
    /// observed, which is a different clock from [`Self::updated_at_millis`] — the
    /// instant the finding RECORD last changed. A recurrence of the activity moves
    /// both; a change to the record's own metadata (its archive state, its
    /// feedback) moves at most the record's clock, because nothing was observed.
    pub event_last_seen_millis: i64,
    pub archived: bool,
    /// True for a finding `CreateSampleFindings` generated.
    pub sample: bool,
    /// `Service.UserFeedback` — "Feedback that was submitted about the finding"
    /// (`smithy_query(shape="Service")`): the `Feedback` value of the latest
    /// `UpdateFindingsFeedback` call that named this finding, and absent until one
    /// does.
    pub user_feedback: Option<String>,
    /// The `Comments` submitted beside [`Self::user_feedback`] — "additional feedback
    /// about the GuardDuty findings"
    /// (`smithy_query(operation="UpdateFindingsFeedback")`). The member is
    /// `@sensitive` and no modelled read reports it (`Service` has no comments
    /// member), so it is kept with the feedback it arrived with rather than published.
    pub feedback_comments: Option<String>,
}

/// One finding attribute, resolved for comparison or sorting.
#[derive(Debug, Clone, PartialEq)]
pub enum Attribute {
    Text(String),
    Number(f64),
}

impl Attribute {
    /// The textual form `Equals` / `NotEquals` compare against: GuardDuty's
    /// `findingCriteria` carries every value as a string, including numeric ones.
    pub fn as_text(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Number(n) => format_number(*n),
        }
    }

    /// The numeric form the ordering conditions compare against, when the
    /// attribute has one.
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Text(_) => None,
            Self::Number(n) => Some(*n),
        }
    }
}

/// Render a finding's numeric attribute without a trailing `.0`, so `severity`
/// equality against the string `"8"` behaves the way it does on the real service.
pub(super) fn format_number(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// The second segment of a finding type — the AFFECTED RESOURCE TYPE, in GuardDuty's
/// own abbreviations: what sits between the `:` and the `/` of
/// `ThreatPurpose:ResourceTypeAffected/ThreatFamilyName`.
///
/// Shared because more than one operation reads that convention: the sample-finding
/// generator derives the `Resource.ResourceType` it stores from this segment, and the
/// usage report derives the feature whose data source generated the finding from it.
/// One reader, so the two can never disagree about what a finding type says.
pub fn finding_type_affected_resource(finding_type: &str) -> &str {
    finding_type
        .split_once(':')
        .map_or("", |(_, rest)| rest.split('/').next().unwrap_or(""))
}

/// The last segment of a finding type: the threat family name with its suffixes — the
/// part that says what the finding was generated FROM (`!DNS` for DNS query data, a
/// `…MaliciousFile` / `…SuspiciousFile` verdict for a malware scan).
pub fn finding_type_threat_family(finding_type: &str) -> &str {
    finding_type.rsplit('/').next().unwrap_or("")
}

/// The finding fields a STORED record populates, so the ones a criterion or a sort
/// actually discriminates on, as the `Finding`, `Resource` and `Service` shapes'
/// `jsonName`s name them (`smithy_query(shape="Finding")`,
/// `smithy_query(shape="Resource")`, `smithy_query(shape="Service")`).
///
/// This is NOT the set a criterion key may name — that is every field of a finding
/// ([`is_finding_field`]). A field a finding may carry but this emulator's records do
/// not populate selects no finding rather than refusing the request, exactly as a
/// finding without a value for it behaves on the real service.
pub const FINDING_ATTRIBUTES: [&str; 23] = [
    "accountId",
    "arn",
    "confidence",
    "createdAt",
    "description",
    "id",
    "partition",
    "region",
    "resource.resourceType",
    "schemaVersion",
    "service.additionalInfo.sample",
    "service.archived",
    "service.count",
    "service.detectorId",
    "service.eventFirstSeen",
    "service.eventLastSeen",
    "service.resourceRole",
    "service.serviceName",
    "service.userFeedback",
    "severity",
    "title",
    "type",
    "updatedAt",
];

/// The subset of [`FINDING_ATTRIBUTES`] that resolves to a NUMBER, so the subset an
/// ordering condition can compare. The timestamps are compared as epoch milliseconds
/// and reported as ISO-8601 text, which is how GuardDuty's own filters treat them
/// ("Type: Timestamp in Unix Epoch millisecond format",
/// `smithy_query(query="doc",shape="CreateFilterRequest",member="FindingCriteria")`).
pub(super) const NUMERIC_FINDING_ATTRIBUTES: [&str; 7] = [
    "confidence",
    "createdAt",
    "service.count",
    "service.eventFirstSeen",
    "service.eventLastSeen",
    "severity",
    "updatedAt",
];

/// The `jsonName` of the `Finding` member carrying the finding's severity
/// (`smithy_query(shape="Finding")`), which is the one numeric finding field whose
/// value domain is a published SCALE rather than an open-ended number.
pub(super) const SEVERITY_ATTRIBUTE: &str = "severity";

/// The lowest value on GuardDuty's finding severity scale.
///
/// The scale runs from 1 to 10 and is divided into the bands this service already
/// assigns a generated sample finding by its finding type — Low 1.0–3.9, Medium
/// 4.0–6.9, High 7.0–8.9, Critical 9.0–10.0. The model types the member as a plain
/// double and states no range at all
/// (`smithy_query(query="doc",shape="Finding",member="Severity")`), so the scale is
/// the service's own and is enforced here.
pub const FINDING_SEVERITY_MIN: f64 = 1.0;

/// The highest value on GuardDuty's finding severity scale. See
/// [`FINDING_SEVERITY_MIN`].
pub const FINDING_SEVERITY_MAX: f64 = 10.0;

/// The [`NUMERIC_FINDING_ATTRIBUTES`] that carry an INSTANT, and so the ones whose
/// values have two admissible forms: the epoch milliseconds a criterion's ordering
/// compares ("Type: Timestamp in Unix Epoch millisecond format",
/// `smithy_query(query="doc",shape="CreateFilterRequest",member="FindingCriteria")`)
/// and the ISO-8601 text a finding reports. Either is a value the field can hold,
/// so an equality value on one of them is not held to the numeric form.
pub(super) const TIMESTAMP_FINDING_ATTRIBUTES: [&str; 4] = [
    "createdAt",
    "service.eventFirstSeen",
    "service.eventLastSeen",
    "updatedAt",
];

/// Can the finding field at `path` HOLD `value` — is it a value of that field?
///
/// A criterion carries every value as text, so the field's own form is what decides:
/// a field this service resolves as a number cannot hold a value that is not one,
/// and the one field whose numbers run over a published scale cannot hold a number
/// off that scale ([`FINDING_SEVERITY_MIN`]). A field resolved as text, an instant
/// (which has two admissible forms, see [`TIMESTAMP_FINDING_ATTRIBUTES`]) and a
/// field a stored record does not populate have no form here to contradict, so any
/// value is one they could hold — the same reasoning
/// [`attribute_is_orderable`] applies to an ordering condition.
pub(super) fn is_finding_field_value(path: &str, value: &str) -> bool {
    if !NUMERIC_FINDING_ATTRIBUTES.contains(&path) || TIMESTAMP_FINDING_ATTRIBUTES.contains(&path) {
        return true;
    }
    let Ok(number) = value.parse::<f64>() else {
        return false;
    };
    if !number.is_finite() {
        return false;
    }
    if path != SEVERITY_ATTRIBUTE {
        return true;
    }
    (FINDING_SEVERITY_MIN..=FINDING_SEVERITY_MAX).contains(&number)
}

/// The `Finding` members that carry a SCALAR, by `jsonName`
/// (`smithy_query(shape="Finding")`): a criterion key or a sort attribute names one
/// of these on its own, with nothing beneath it.
pub(super) const FINDING_SCALAR_FIELDS: [&str; 15] = [
    "accountId",
    "arn",
    "associatedAttackSequenceArn",
    "confidence",
    "createdAt",
    "customDetectionRuleName",
    "description",
    "id",
    "partition",
    "region",
    "schemaVersion",
    "severity",
    "title",
    "type",
    "updatedAt",
];

/// The `jsonName` of the `Finding` member carrying the affected resource, and of the
/// one carrying what the service observed (`smithy_query(shape="Finding")`). A
/// criterion key reaches BELOW the top level only through these two.
pub(super) const RESOURCE_FIELD: &str = "resource";
/// See [`RESOURCE_FIELD`].
pub(super) const SERVICE_FIELD: &str = "service";

/// The `Resource` members that carry a scalar, by `jsonName`
/// (`smithy_query(shape="Resource")`).
pub(super) const RESOURCE_SCALAR_FIELDS: [&str; 1] = ["resourceType"];

/// The `Resource` members that carry a nested detail document, by `jsonName`
/// (`smithy_query(shape="Resource")`): the fields of that document are what the
/// remaining segments of a criterion key address.
pub(super) const RESOURCE_DOCUMENT_FIELDS: [&str; 17] = [
    "accessKeyDetails",
    "bedrockGuardrailDetails",
    "containerDetails",
    "ebsSnapshotDetails",
    "ebsVolumeDetails",
    "ec2ImageDetails",
    "ecsClusterDetails",
    "eksClusterDetails",
    "instanceDetails",
    "kubernetesDetails",
    "lambdaDetails",
    "modelDetails",
    "rdsDbInstanceDetails",
    "rdsDbUserDetails",
    "rdsLimitlessDbDetails",
    "recoveryPointDetails",
    "s3BucketDetails",
];

/// The `Service` members that carry a scalar, by `jsonName`
/// (`smithy_query(shape="Service")`).
pub(super) const SERVICE_SCALAR_FIELDS: [&str; 9] = [
    "archived",
    "count",
    "detectorId",
    "eventFirstSeen",
    "eventLastSeen",
    "featureName",
    "resourceRole",
    "serviceName",
    "userFeedback",
];

/// The `Service` members that carry a nested document, by `jsonName`
/// (`smithy_query(shape="Service")`). See [`RESOURCE_DOCUMENT_FIELDS`].
pub(super) const SERVICE_DOCUMENT_FIELDS: [&str; 7] = [
    "action",
    "additionalInfo",
    "detection",
    "ebsVolumeScanDetails",
    "evidence",
    "malwareScanDetails",
    "runtimeDetails",
];

/// Is `path` a FIELD OF A FINDING — a name a `FindingCriteria.Criterion` key or a
/// `SortCriteria.AttributeName` may carry?
///
/// Decided from the finding's own shape rather than from what this emulator happens to
/// store, and before any finding is looked at, so the answer does not depend on how
/// many findings exist: the first segment must be a `Finding` member's `jsonName`, and
/// a path that continues must continue through the `resource` or `service`
/// sub-document, whose next segment must be a member of that shape. A member carrying
/// a scalar ENDS the path — there is nothing beneath it to name — while a member
/// carrying a nested detail document takes at least one further segment, naming a field
/// of that document the way the model's list of fields available for filtering does
/// (`smithy_query(query="doc",shape="CreateFilterRequest",member="FindingCriteria")`).
pub fn is_finding_field(path: &str) -> bool {
    let mut segments = path.split('.');
    let Some(first) = segments.next() else {
        return false;
    };
    let rest: Vec<&str> = segments.collect();
    match first {
        RESOURCE_FIELD => {
            sub_document_field(&rest, &RESOURCE_SCALAR_FIELDS, &RESOURCE_DOCUMENT_FIELDS)
        }
        SERVICE_FIELD => sub_document_field(&rest, &SERVICE_SCALAR_FIELDS, &SERVICE_DOCUMENT_FIELDS),
        _ => rest.is_empty() && FINDING_SCALAR_FIELDS.contains(&first),
    }
}

/// Do the segments BELOW `resource` or `service` name a field of that sub-document?
///
/// `scalars` are its members that end a path and `documents` those that take one, so
/// the two shapes are checked by the same rule and cannot drift apart.
pub(super) fn sub_document_field(rest: &[&str], scalars: &[&str], documents: &[&str]) -> bool {
    let Some((member, deeper)) = rest.split_first() else {
        // The sub-document itself is not a field: there is no value to compare.
        return false;
    };
    if scalars.contains(member) {
        return deeper.is_empty();
    }
    documents.contains(member) && !deeper.is_empty() && deeper.iter().all(|s| is_field_name(s))
}

/// Is `segment` a well-formed field name — the lower camel-case run of ASCII letters
/// and digits every field name in the model's filterable-field list is?
pub(super) fn is_field_name(segment: &str) -> bool {
    segment.starts_with(|c: char| c.is_ascii_lowercase())
        && segment.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Why a criterion key or a sort attribute was refused, with the message that says so.
///
/// One type for every operation that accepts a `FindingCriteria` or a `SortCriteria`,
/// so all of them answer an unacceptable name with the same words under their own
/// member name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CriterionRejection {
    /// The name is not a field of a finding at all, so nothing could ever be selected
    /// or ordered by it.
    UnknownField(String),
    /// It names a field, but not one an ordering condition can compare.
    NotOrderable(String),
    /// The criterion applies a `Condition` member the model restricts to the filter
    /// APIs — `matches` or `notMatches` — on an operation that is not one of them
    /// (see [`filter_only_condition`]). Carries that member's `jsonName`, which is a
    /// name from the MODEL and not a value from the request.
    ConditionNotAvailable(&'static str),
    /// A condition on the field asserts it equals a value that field cannot hold
    /// ([`is_finding_field_value`]). Carries the field path, which is a name from
    /// the request, so nothing of it reaches the message.
    ValueNotInDomain(String),
}

impl CriterionRejection {
    /// The refusal message, naming the request `member` that carried the name.
    ///
    /// GuardDuty answers a criteria structure it will not accept with its fixed
    /// invalid-value sentence for that member ([`invalid_parameter_value_message`]):
    /// which key inside the structure was at fault, and why, is not echoed — the
    /// offending name came from the request, and a refusal does not quote request
    /// input back. The variants are the same refusal to a caller and differ only
    /// in why this service would not take the criterion; they are kept apart because
    /// the readers that produce them answer different questions.
    pub fn message(&self, member: &str) -> String {
        match self {
            Self::UnknownField(_) | Self::NotOrderable(_) | Self::ConditionNotAvailable(_) => {
                invalid_parameter_value_message(member)
            }
            // A criteria structure whose SHAPE the service takes but whose
            // condition values it does not gets the sentence that says so: the
            // fault is located in a condition rather than in the member as a
            // whole, and the value is still not echoed.
            Self::ValueNotInDomain(_) => invalid_condition_values_message(member),
        }
    }
}

/// Resolve one finding attribute by its wire path, or `None` when the finding carries
/// NO VALUE for that field — either because the field is one this emulator's records do
/// not populate, or because this record has not got it (no feedback submitted, not a
/// generated sample).
///
/// `updatedAt` is accepted as well as `createdAt`; both are compared as epoch
/// milliseconds by the ordering conditions and as ISO-8601 text by the equality
/// ones, which is how GuardDuty's own filters treat them, and the service document's
/// two activity timestamps follow the same convention.
pub fn finding_attribute(finding: &FindingRecord, path: &str) -> Option<Attribute> {
    let attr = match path {
        "accountId" => Attribute::Text(finding.account_id.clone()),
        "arn" => Attribute::Text(finding.arn.clone()),
        "confidence" => Attribute::Number(finding.confidence.unwrap_or(0.0)),
        "createdAt" => Attribute::Number(finding.created_at_millis as f64),
        "updatedAt" => Attribute::Number(finding.updated_at_millis as f64),
        "description" => Attribute::Text(finding.description.clone().unwrap_or_default()),
        "id" => Attribute::Text(finding.id.clone()),
        "partition" => Attribute::Text(finding.partition.clone()),
        "region" => Attribute::Text(finding.region.clone()),
        "resource.resourceType" => Attribute::Text(finding.resource_type.clone()),
        "schemaVersion" => Attribute::Text(finding.schema_version.clone()),
        // The additional-information document carries the `sample` marker on a
        // generated sample finding and nothing at all on anything else, so the field
        // is absent rather than false on a finding that is not a sample.
        "service.additionalInfo.sample" if finding.sample => Attribute::Text(true.to_string()),
        "service.archived" => Attribute::Text(finding.archived.to_string()),
        "service.count" => Attribute::Number(f64::from(finding.count)),
        "service.detectorId" => Attribute::Text(finding.detector_id.clone()),
        "service.eventFirstSeen" => Attribute::Number(finding.created_at_millis as f64),
        "service.eventLastSeen" => Attribute::Number(finding.event_last_seen_millis as f64),
        "service.resourceRole" => Attribute::Text(finding.resource_role.clone()),
        "service.serviceName" => Attribute::Text(GUARDDUTY_SERVICE_NAME.to_string()),
        // Absent until an `UpdateFindingsFeedback` call records one.
        "service.userFeedback" => Attribute::Text(finding.user_feedback.clone()?),
        "severity" => Attribute::Number(finding.severity),
        "title" => Attribute::Text(finding.title.clone().unwrap_or_default()),
        "type" => Attribute::Text(finding.r#type.clone()),
        _ => return None,
    };
    Some(attr)
}

/// A finding attribute rendered for EQUALITY comparison. A timestamp compares as its
/// ISO-8601 text — the form the finding reports it in — and every other attribute as
/// its own textual form.
pub(super) fn equality_text(finding: &FindingRecord, path: &str) -> Option<String> {
    match path {
        "createdAt" | "service.eventFirstSeen" => Some(iso8601(finding.created_at_millis)),
        "updatedAt" => Some(iso8601(finding.updated_at_millis)),
        "service.eventLastSeen" => Some(iso8601(finding.event_last_seen_millis)),
        _ => finding_attribute(finding, path).map(|a| a.as_text()),
    }
}

/// Does `finding` satisfy one `Condition` on `path`?
///
/// `Err` means `path` names no field of a finding, which the caller turns into a
/// refusal rather than silently returning every finding. A finding that carries NO
/// VALUE for a field it could have satisfies only the negative conditions on it: an
/// absent value equals nothing and matches no pattern, so it cannot be selected by an
/// equality, a pattern or an ordering, and cannot be excluded by a negation.
/// Within one condition every populated operator must hold, which is how
/// GuardDuty composes a criterion.
pub fn condition_matches(
    finding: &FindingRecord,
    path: &str,
    condition: &crate::ops::types::Condition,
) -> Result<bool, CriterionRejection> {
    if !is_finding_field(path) {
        return Err(CriterionRejection::UnknownField(path.to_string()));
    }
    let resolved = finding_attribute(finding, path);
    let text = equality_text(finding, path);

    // `Equals`/`NotEquals` and the deprecated `Eq`/`Neq` are the same comparison;
    // a list holds alternatives, so `Equals` passes when ANY entry matches and
    // `NotEquals` passes only when NONE does.
    for list in [condition.equals.as_ref(), condition.eq.as_ref()].into_iter().flatten() {
        if list.is_empty() {
            continue;
        }
        if !text.as_ref().is_some_and(|value| list.contains(value)) {
            return Ok(false);
        }
    }
    for list in [condition.not_equals.as_ref(), condition.neq.as_ref()]
        .into_iter()
        .flatten()
    {
        if text.as_ref().is_some_and(|value| list.contains(value)) {
            return Ok(false);
        }
    }

    // The ordering conditions only apply to an attribute with a numeric form.
    let bounds: [(Option<f64>, Ordering); 8] = [
        (condition.greater_than.map(|v| v as f64), Ordering::Gt),
        (condition.gt.map(f64::from), Ordering::Gt),
        (condition.greater_than_or_equal.map(|v| v as f64), Ordering::Gte),
        (condition.gte.map(f64::from), Ordering::Gte),
        (condition.less_than.map(|v| v as f64), Ordering::Lt),
        (condition.lt.map(f64::from), Ordering::Lt),
        (condition.less_than_or_equal.map(|v| v as f64), Ordering::Lte),
        (condition.lte.map(f64::from), Ordering::Lte),
    ];
    for (bound, ordering) in bounds {
        let Some(bound) = bound else { continue };
        let Some(value) = resolved.as_ref().and_then(Attribute::as_number) else {
            if !attribute_is_orderable(path) {
                return Err(CriterionRejection::NotOrderable(path.to_string()));
            }
            // A field with no value to order selects nothing.
            return Ok(false);
        };
        let holds = match ordering {
            Ordering::Gt => value > bound,
            Ordering::Gte => value >= bound,
            Ordering::Lt => value < bound,
            Ordering::Lte => value <= bound,
        };
        if !holds {
            return Ok(false);
        }
    }

    // `Matches` / `NotMatches` compare the attribute against a pattern in which
    // `*` stands for any run of characters; like the equality lists, a list holds
    // alternatives. The model offers the two only to the filter APIs, so the criteria
    // that reach here carrying them are a SAVED filter's — a findings read is refused
    // them up front ([`validate_finding_criteria`]) instead of evaluating them.
    if let Some(patterns) = condition.matches.as_ref() {
        let selected = text
            .as_ref()
            .is_some_and(|value| patterns.iter().any(|p| wildcard_matches(p, value)));
        if !patterns.is_empty() && !selected {
            return Ok(false);
        }
    }
    if let Some(patterns) = condition.not_matches.as_ref() {
        let excluded = text
            .as_ref()
            .is_some_and(|value| patterns.iter().any(|p| wildcard_matches(p, value)));
        if excluded {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Does `value` match `pattern`, where `*` in the pattern stands for any run of
/// characters (including none)?
pub(super) fn wildcard_matches(pattern: &str, value: &str) -> bool {
    let mut segments = pattern.split('*');
    let Some(first) = segments.next() else {
        return pattern == value;
    };
    if !value.starts_with(first) {
        return false;
    }
    let mut rest = &value[first.len()..];
    let segments: Vec<&str> = segments.collect();
    let Some((last, middle)) = segments.split_last() else {
        // No `*` at all: the pattern is a literal.
        return rest.is_empty();
    };
    for segment in middle {
        match rest.find(segment) {
            Some(at) => rest = &rest[at + segment.len()..],
            None => return false,
        }
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

/// Which way an ordering condition compares.
pub(super) enum Ordering {
    Gt,
    Gte,
    Lt,
    Lte,
}

/// Does `finding` satisfy every criterion in `criteria`? An absent or empty
/// criteria map matches every finding.
pub fn finding_matches(
    finding: &FindingRecord,
    criteria: Option<&crate::ops::types::FindingCriteria>,
) -> Result<bool, CriterionRejection> {
    let Some(criterion) = criteria.and_then(|c| c.criterion.as_ref()) else {
        return Ok(true);
    };
    for (path, condition) in criterion {
        if !condition_matches(finding, path, condition)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Which OPERATION CLASS a `FindingCriteria` arrived at, and so which `Condition`
/// members are available on it.
///
/// `Condition` is the one condition shape a `FindingCriteria.Criterion` targets, so the
/// filter APIs and the findings reads are handed the same structure and the shape alone
/// cannot say which members each of them takes; the model says it per member
/// ([`filter_only_condition`]). Every caller of [`validate_finding_criteria`] therefore
/// states which class it is, and a new operation taking a `FindingCriteria` cannot be
/// written without answering the question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CriteriaSurface {
    /// The filter APIs — the create and update that SAVE a `FindingCriteria` — on
    /// which every `Condition` member is available.
    Filter,
    /// A findings read that QUERIES with a `FindingCriteria`, on which the members the
    /// model reserves for the filter APIs are not available.
    FindingsQuery,
}

/// The `Condition` member the model documents as available only to the filter APIs that
/// `condition` applies, by `jsonName`, if it applies one.
///
/// The restriction is the model's, stated per member rather than by the shape: `Matches`
/// is "available only for create-filter and update-filter APIs"
/// (`smithy_query(query="doc",shape="Condition",member="Matches")`) and `NotMatches`
/// says the same of itself
/// (`smithy_query(query="doc",shape="Condition",member="NotMatches")`), while no other
/// member of the shape is narrowed that way. The pair is read here, in one place, so the
/// operations handed a `FindingCriteria` cannot disagree about which members a caller
/// may apply.
///
/// PRESENCE is what counts, not whether the list holds a pattern: a member the caller
/// sent is a condition the caller applied, and a member an operation does not offer is
/// not offered emptily either.
pub(super) fn filter_only_condition(condition: &crate::ops::types::Condition) -> Option<&'static str> {
    [
        ("matches", condition.matches.is_some()),
        ("notMatches", condition.not_matches.is_some()),
    ]
    .into_iter()
    .find_map(|(member, present)| present.then_some(member))
}

/// Does a `Condition` on the finding field at `path` assert that field equals a
/// value it cannot hold?
///
/// `Equals` and the deprecated `Eq` are the members that CLAIM the field takes one
/// of the values they carry, so they are the ones held to the field's domain
/// ([`is_finding_field_value`]): a claim no value of the field can satisfy selects
/// nothing, whatever findings exist. The other members make no such claim and are
/// therefore not checked against it — `NotEquals`/`Neq` name values to EXCLUDE,
/// which is well defined for a value the field never takes (it excludes nothing),
/// and an ordering member carries a THRESHOLD rather than a value, so a bound past
/// the field's domain selects everything or nothing instead of being unsatisfiable.
pub(super) fn condition_value_not_in_domain(path: &str, condition: &crate::ops::types::Condition) -> bool {
    [condition.equals.as_ref(), condition.eq.as_ref()]
        .into_iter()
        .flatten()
        .flatten()
        .any(|value| !is_finding_field_value(path, value))
}

/// Refuse a `FindingCriteria` this service cannot answer on `surface`, naming the field
/// path or the unavailable condition.
///
/// Checked BEFORE any finding is looked at, so whether a criterion is refused does
/// not depend on how many findings happen to be stored: a detector with none must
/// answer an unevaluable criterion exactly as a populated one does.
pub fn validate_finding_criteria(
    criteria: Option<&crate::ops::types::FindingCriteria>,
    surface: CriteriaSurface,
) -> Result<(), CriterionRejection> {
    let Some(criterion) = criteria.and_then(|c| c.criterion.as_ref()) else {
        return Ok(());
    };
    for (path, condition) in criterion {
        if !is_finding_field(path) {
            return Err(CriterionRejection::UnknownField(path.clone()));
        }
        // A condition the model offers only to the filter APIs is refused on a
        // findings read rather than evaluated there: the operation does not offer it,
        // so a caller who sends it is told so instead of being answered 200 with a
        // page selected by a condition this operation does not have.
        if surface == CriteriaSurface::FindingsQuery {
            if let Some(member) = filter_only_condition(condition) {
                return Err(CriterionRejection::ConditionNotAvailable(member));
            }
        }
        let orders_numerically = condition.greater_than.is_some()
            || condition.greater_than_or_equal.is_some()
            || condition.less_than.is_some()
            || condition.less_than_or_equal.is_some()
            || condition.gt.is_some()
            || condition.gte.is_some()
            || condition.lt.is_some()
            || condition.lte.is_some();
        if orders_numerically && !attribute_is_orderable(path) {
            return Err(CriterionRejection::NotOrderable(path.clone()));
        }
        // A criterion is a claim about the VALUE of a finding field, so a value the
        // field cannot hold is refused here rather than saved in a filter that can
        // never select anything or evaluated against findings it cannot describe.
        // Checked on every surface, for the same reason the field check is: the two
        // answers must not depend on which operation was handed the criteria.
        if condition_value_not_in_domain(path, condition) {
            return Err(CriterionRejection::ValueNotInDomain(path.clone()));
        }
    }
    Ok(())
}

/// Can an ordering condition compare the field at `path`?
///
/// A field this service resolves from a stored record has a KNOWN form, so an ordering
/// condition on one it resolves as text is refused rather than quietly selecting
/// nothing. A field a finding may carry but a stored record does not populate has no
/// form here to contradict, so an ordering condition on it is accepted and simply
/// selects no finding.
pub(super) fn attribute_is_orderable(path: &str) -> bool {
    NUMERIC_FINDING_ATTRIBUTES.contains(&path) || !FINDING_ATTRIBUTES.contains(&path)
}

/// Order `findings` for `ListFindings` / `GetFindings`.
///
/// With no `SortCriteria` the newest finding comes first, with the finding id
/// breaking ties so the order is total and a page token stays meaningful. A sort
/// attribute that names no field of a finding is refused; one that names a field no
/// stored record populates leaves every record equal, so the tiebreaker alone orders
/// them.
pub fn sort_findings(
    findings: &mut [&FindingRecord],
    sort: Option<&crate::ops::types::SortCriteria>,
) -> Result<(), CriterionRejection> {
    let attribute = sort
        .and_then(|s| s.attribute_name.as_deref())
        .filter(|a| !a.is_empty());
    let descending = match sort.and_then(|s| s.order_by.as_deref()) {
        Some(ORDER_BY_ASCENDING) => false,
        Some(ORDER_BY_DESCENDING) => true,
        // No explicit order: newest-first for the default attribute, ascending for
        // an attribute the caller named.
        _ => attribute.is_none(),
    };
    let attribute = attribute.unwrap_or("createdAt");
    if !is_finding_field(attribute) {
        return Err(CriterionRejection::UnknownField(attribute.to_string()));
    }

    findings.sort_by(|a, b| {
        compare_by_attribute(
            *a,
            *b,
            descending,
            |finding| finding_attribute(finding, attribute),
            |finding| finding.id.clone(),
        )
    });
    Ok(())
}

/// Compare two records by one resolved attribute, in `descending` order or not,
/// with `tiebreak` making the order total.
///
/// Both [`sort_findings`] and [`sort_malware_scans`] order a borrowed record set
/// by an attribute that is either a number or a text, so the comparison lives here
/// once: the two differ only in the attribute resolver and the tiebreaker, and
/// cannot drift apart. An attribute the resolver cannot produce sorts as the
/// neutral key rather than dropping the record.
pub(super) fn compare_by_attribute<T, K: Ord>(
    left: &T,
    right: &T,
    descending: bool,
    attribute: impl Fn(&T) -> Option<Attribute>,
    tiebreak: impl Fn(&T) -> K,
) -> std::cmp::Ordering {
    let key = |record: &T| match attribute(record) {
        Some(Attribute::Number(n)) => (n, String::new()),
        Some(Attribute::Text(t)) => (0.0, t),
        None => (0.0, String::new()),
    };
    let (left_number, left_text) = key(left);
    let (right_number, right_text) = key(right);
    let primary = left_number
        .partial_cmp(&right_number)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| left_text.cmp(&right_text));
    let primary = if descending { primary.reverse() } else { primary };
    primary.then_with(|| tiebreak(left).cmp(&tiebreak(right)))
}

/// A saved finding filter. `CreateFilter` writes it, `UpdateFilter` changes it,
/// `GetFilter` reports it, `ListFilters` names it, `DeleteFilter` drops it and
/// `DeleteDetector`'s cascade drops it with its detector.
#[derive(Debug, Clone, Default)]
pub struct FilterRecord {
    pub name: String,
    pub description: Option<String>,
    /// `FilterAction`: `ARCHIVE` or [`FILTER_ACTION_NOOP`].
    pub action: Option<String>,
    pub rank: Option<i32>,
    /// The criteria the filter selects findings by, kept as the modelled shape so
    /// a read reports exactly what the create stored.
    pub finding_criteria: crate::ops::types::FindingCriteria,
    pub tags: BTreeMap<String, String>,
    /// The `@idempotencyToken` the create carried, so a replayed `CreateFilter`
    /// returns the filter the first call saved instead of refusing the name.
    pub client_token: Option<String>,
    pub created_at_millis: i64,
    pub updated_at_millis: i64,
    /// `FilterVersion`: 1 for a filter nothing has updated.
    pub version: i64,
}

impl FilterRecord {
    /// Does this filter ARCHIVE the findings its criteria select?
    ///
    /// The stored action is one of the two `FilterAction` values
    /// (`smithy_query(shape="FilterAction")`) and only
    /// [`FILTER_ACTION_ARCHIVE`] is applied to a finding; a filter carrying the
    /// other value, or carrying none at all (whose default is
    /// [`FILTER_ACTION_NOOP`]), leaves every finding exactly as it is.
    pub fn archives_matching_findings(&self) -> bool {
        self.action.as_deref() == Some(FILTER_ACTION_ARCHIVE)
    }
}
