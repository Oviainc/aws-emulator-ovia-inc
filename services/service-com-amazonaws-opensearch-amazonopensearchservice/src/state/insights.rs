// SPDX-License-Identifier: CC0-1.0

//! Insights of an account or a domain, and the feedback a caller leaves on one.
//!
//! `ListInsights` reports the insights an entity holds and `DescribeInsightDetails`
//! details one of them, both from the [`InsightRecord`] below; `InsightFeedback` records
//! what a caller thought of an insight in the one [`InsightFeedbackRecord`] a later read
//! of that feedback answers from.

use super::*;

/// `InsightFieldType`'s `text` value — "possible values are `text` and `metric`" —
/// the type of a field that carries a text description rather than metric data. One
/// of [`crate::ops::types::INSIGHT_FIELD_TYPE_VALUES`].
pub const INSIGHT_FIELD_TYPE_TEXT: &str = "text";

/// One field of an insight's DETAILED view, as this emulator persists it.
///
/// A detail read "returns detailed fields associated with the specified insight, such
/// as text descriptions and metric data", each field a name, a type out of
/// `InsightFieldType` (`text` or `metric`) and a value — so this is the ONE copy of a
/// field, held on the insight it details ([`InsightRecord::details`]) in the order a
/// detail read reports it.
///
/// A field's content is stored once per rendering the service raises it in: [`value`]
/// is the plain one and [`html_value`] the SAME content with the markup the service
/// renders it with, for a field that has such a rendering. The request's
/// `ShowHtmlContent` picks between them ([`InsightDetailField::to_insight_field`]), so
/// neither is derived from the other at read time.
///
/// [`value`]: InsightDetailField::value
/// [`html_value`]: InsightDetailField::html_value
#[derive(Debug, Clone)]
pub struct InsightDetailField {
    /// `InsightField$Name`: "the name of the insight field".
    pub name: String,
    /// `InsightField$Type`, one of `InsightFieldType`'s values: whether this field
    /// carries a text description or metric data.
    pub field_type: String,
    /// `InsightField$Value` with no markup in it — what a caller that did not ask for
    /// HTML content is answered with.
    pub value: String,
    /// The same content as the service renders it WITH markup, for a field that has
    /// such a rendering; `None` where it has none and the plain value is the whole of
    /// what there is to report.
    pub html_value: Option<String>,
}

impl InsightDetailField {
    /// This field as `InsightField` — the ONE projection of it, so every operation
    /// that reports a detail field reports the same values.
    ///
    /// `show_html` is the request's `ShowHtmlContent` — "specifies whether to show
    /// response with HTML content in response or not" — so it selects the marked-up
    /// rendering where this field has one, and changes nothing where it has none.
    pub fn to_insight_field(&self, show_html: bool) -> InsightField {
        let value = match (show_html, self.html_value.as_deref()) {
            (true, Some(html)) => html.to_string(),
            _ => self.value.clone(),
        };
        InsightField {
            name: self.name.clone(),
            r#type: self.field_type.clone(),
            value,
        }
    }
}

/// The feedback a caller has submitted on one insight, as this emulator persists it.
///
/// A submission "allows users to provide a thumbs up or thumbs down rating and
/// optional text feedback for a specific insight", so a record is that rating, the
/// text that came with it where the caller wrote any, and the instant it was
/// submitted. It is held on the insight it rates ([`InsightRecord::feedback`]), so it
/// goes when that insight goes.
///
/// One caller holds ONE rating of an insight — "thumbs up or thumbs down" is a single
/// verdict, not a tally — so a later submission about the same insight REPLACES the
/// rating an earlier one left rather than accumulating beside it.
#[derive(Debug, Clone)]
pub struct InsightFeedbackRecord {
    /// `InsightFeedbackRequest$Thumbs`, one of `InsightFeedbackThumbs`' values: the
    /// thumbs up or thumbs down rating the caller gave the insight.
    pub thumbs: String,
    /// `InsightFeedbackRequest$FeedbackText`: "optional text feedback providing
    /// additional details about the insight", `None` where the submission carried
    /// none.
    pub feedback_text: Option<String>,
    /// The instant this rating was submitted — the submission this record is of.
    pub submitted_at: SystemTime,
}

/// One insight, as this emulator persists it.
///
/// An insight is "a notification about a domain event or recommendation that helps
/// you optimize your Amazon OpenSearch Service domain": a record with an identity of
/// its own — a `GUID`, a display name, a type, a priority, a status and the instants
/// it was created and last changed at — so this is the ONE copy `ListInsights`
/// reports and the one `DescribeInsightDetails` and `InsightFeedback` address by id.
/// The service itself raises an insight; no operation of this emulator does yet, so
/// today the only writer is whatever puts a record here directly.
///
/// Its members are kept as the values the model's own enums admit (`InsightType`,
/// `InsightPriorityLevel`, `InsightStatus`) so a listing reports the stored record
/// rather than a second derivation of it.
#[derive(Debug, Clone)]
pub struct InsightRecord {
    /// `Insight$InsightId`: "the unique identifier of the insight", a [`new_guid`].
    /// Unique across the account's insights, so it is the key the set holds the
    /// record under and the place a pagination token names.
    pub insight_id: String,
    /// `Insight$DisplayName`: "the display name of the insight".
    pub display_name: String,
    /// `Insight$Type`, one of `InsightType`'s values: whether the insight reports a
    /// domain event or a recommendation.
    pub insight_type: String,
    /// `Insight$Priority`, one of `InsightPriorityLevel`'s values.
    pub priority: String,
    /// `Insight$Status`, one of `InsightStatus`'s values: where the insight is in its
    /// own lifecycle.
    pub status: String,
    /// The instant the insight was raised, reported as `Insight$CreationTime` — and
    /// the instant a listing's time range and sort order are measured on.
    pub created_at: SystemTime,
    /// The instant the insight last changed, reported as `Insight$UpdateTime`.
    pub updated_at: SystemTime,
    /// `Insight$IsExperimental`: "indicates whether the insight is experimental".
    pub is_experimental: bool,
    /// The fields of this insight's DETAILED view, in the order a detail read reports
    /// them — the text descriptions and metric data a detail read answers with. Held
    /// on the insight itself, because a detail field details THAT insight and goes
    /// when it goes; empty for an insight the service has raised no detail for.
    pub details: Vec<InsightDetailField>,
    /// The feedback submitted on THIS insight, or `None` while no caller has rated it.
    /// Held on the insight itself, because feedback rates THAT insight and goes when
    /// it goes. See [`InsightFeedbackRecord`].
    pub feedback: Option<InsightFeedbackRecord>,
}

impl InsightRecord {
    /// This record as `Insight` — the ONE projection of it, so every operation that
    /// reports an insight reports the same values.
    pub fn to_insight(&self) -> Insight {
        Insight {
            insight_id: Some(self.insight_id.clone()),
            display_name: Some(self.display_name.clone()),
            r#type: Some(self.insight_type.clone()),
            priority: Some(self.priority.clone()),
            status: Some(self.status.clone()),
            creation_time: Some(datetime(self.created_at)),
            update_time: Some(datetime(self.updated_at)),
            is_experimental: Some(self.is_experimental),
        }
    }
}

/// The entity type values `InsightEntityType` admits: an insight is held either for
/// one domain or for the account itself.
pub const INSIGHT_ENTITY_ACCOUNT: &str = "Account";
/// See [`INSIGHT_ENTITY_ACCOUNT`].
pub const INSIGHT_ENTITY_DOMAIN_NAME: &str = "DomainName";

/// The values `InsightFeedbackThumbs` admits: "the thumbs up or thumbs down feedback
/// for an insight. Possible values are `Up` and `Down`". One of
/// [`crate::ops::types::INSIGHT_FEEDBACK_THUMBS_VALUES`].
pub const INSIGHT_FEEDBACK_THUMBS_UP: &str = "Up";
/// See [`INSIGHT_FEEDBACK_THUMBS_UP`].
pub const INSIGHT_FEEDBACK_THUMBS_DOWN: &str = "Down";

/// Why the entity a request names has no insights to report, as the modelled
/// exception payload the refusal is reported through.
///
/// Each operation declares its OWN error enum, so this names the payload and the
/// handler lifts it into its own channel: resolving an `InsightEntity` is ONE
/// decision even though the error channels of the operations that make it are not.
#[derive(Debug)]
pub enum InsightEntityRefusal {
    /// The entity names a resource this account does not hold.
    NotFound(ResourceNotFoundException),
    /// The entity itself is not one this service can resolve.
    Validation(ValidationException),
}
