// SPDX-License-Identifier: CC0-1.0

//! What a related-item search reports, and what it narrows on.
//!
//! A stored association ([`RelatedItem`]) carries the content it was written
//! with, in the INPUT union the write accepted; a reader reports it in the
//! `RelatedItemContent` union, whose members carry the derived values as well
//! as the written ones (an SLA's target instant and its status). That
//! projection, and the `RelatedItemTypeFilter` comparisons a search narrows it
//! with, live beside the store rather than in one operation's file: every
//! related-item reader reports the same projection of the same records.

use super::*;

use crate::ops::types::{
    ConnectCaseContent, ContactContent, CustomContent, CustomFieldsFilter, FieldValue,
    RelatedItemContent, RelatedItemTypeFilter, SlaConfiguration, SlaContent, SlaInputConfiguration,
    SlaInputContent,
};

/// Most `filters` clauses one related-item search evaluates: `RelatedItemFilterList`
/// is `@length(0..10)`
/// (`smithy_query(query="operation", operation="SearchAllRelatedItems")`). Re-checked
/// in the handler as well as at the boundary, so the per-request work is bounded by
/// this crate rather than by the boundary validator alone.
pub const MAX_RELATED_ITEM_FILTERS: usize = 10;

/// Most related items one page of a related-item search reports, which is also
/// the default when a request names no `maxResults`: the model's own cap, stated
/// twice — `maxResults` is `@range(1..25)` and the response's `relatedItems` is
/// `@length(max: 25)`
/// (`smithy_query(query="operation", operation="SearchAllRelatedItems,SearchRelatedItems")`).
pub const MAX_RELATED_ITEMS_PER_PAGE: i32 = 25;

/// `SlaStatus` of an SLA whose target instant has not passed and whose target
/// has not been reached. One of `SLA_STATUS_VALUES`
/// (`Active | Met | NotMet | Overdue`).
pub const SLA_STATUS_ACTIVE: &str = "Active";

/// `SlaStatus` of an SLA whose target instant has passed with its target never
/// reached. One of `SLA_STATUS_VALUES`.
pub const SLA_STATUS_OVERDUE: &str = "Overdue";

/// `SlaStatus` of an SLA whose target was reached by its target instant. One of
/// `SLA_STATUS_VALUES`.
pub const SLA_STATUS_MET: &str = "Met";

/// `SlaStatus` of an SLA whose target was reached only after its target
/// instant. One of `SLA_STATUS_VALUES`.
pub const SLA_STATUS_NOT_MET: &str = "NotMet";

/// The `Channel` a `Contact` related item reports.
///
/// `ContactContent` requires the contact's `channel` and the instant it was
/// connected to the system, and both are properties of the Amazon Connect
/// CONTACT the association links to — a resource of another service, which this
/// one neither mints nor holds; a related item "may or may not have an external
/// identifier linking it to an external resource" (`CreateRelatedItem`'s
/// documentation), and a contact ARN is such an identifier. A reader therefore
/// reports what the association itself records — the ARN it was written with and
/// the instant it was made — and this channel, the one every contact
/// association in this store is recorded under. `ContactFilter$channel` is
/// matched against it, so a search narrowed to any other channel reports no
/// contact association. `Channel` is a free-form string in this service's model
/// (`smithy_query(query="shape", shape="Channel")`, `@length(1..100)`).
pub const CONTACT_ASSOCIATION_CHANNEL: &str = "VOICE";

impl Case {
    /// The content of one of this case's related items, as a reader reports it:
    /// the `RelatedItemContent` member matching the item's type, carrying what
    /// the association recorded.
    ///
    /// `now` is the instant the read is answered at, which an SLA's status is
    /// measured against. The projection is pure — nothing here writes, so two
    /// reads of an unchanged store answer the same thing except where the CLOCK
    /// has moved an SLA past its target.
    pub fn related_item_content(&self, item: &RelatedItem, now: SystemTime) -> RelatedItemContent {
        match &item.content {
            // The content of these four is reported exactly as it was written:
            // every member the reader's union declares for them is a member the
            // write carried.
            RelatedItemInputContent::Comment(comment) => {
                RelatedItemContent::Comment(comment.clone())
            }
            RelatedItemInputContent::File(file) => RelatedItemContent::File(file.clone()),
            RelatedItemInputContent::connectCase(link) => {
                RelatedItemContent::connectCase(ConnectCaseContent {
                    case_id: link.case_id.clone(),
                })
            }
            RelatedItemInputContent::Custom(custom) => RelatedItemContent::Custom(CustomContent {
                fields: custom.fields.clone(),
            }),
            // A contact's own properties are not this service's state; see
            // [`CONTACT_ASSOCIATION_CHANNEL`].
            RelatedItemInputContent::Contact(contact) => {
                RelatedItemContent::Contact(ContactContent {
                    contact_arn: contact.contact_arn.clone(),
                    channel: CONTACT_ASSOCIATION_CHANNEL.to_string(),
                    connected_to_system_time: timestamp(item.association_time),
                })
            }
            RelatedItemInputContent::Sla(SlaInputContent::slaInputConfiguration(sla)) => {
                RelatedItemContent::Sla(SlaContent {
                    sla_configuration: self.sla_configuration(item, sla, now),
                })
            }
        }
    }

    /// One SLA association as a reader reports it.
    ///
    /// The write names a target in MINUTES from the association
    /// (`SlaInputConfiguration$targetSlaMinutes`) and the read reports the
    /// target INSTANT (`SlaConfiguration$targetTime`), so the instant is
    /// computed from the stored association time rather than stored a second
    /// time. The status follows from the same two facts the case already holds:
    /// whether the tracked field has reached one of the target values
    /// ([`Self::sla_completion_time`]) and whether that happened before the
    /// target instant — and, where it has not, whether the clock has passed
    /// that instant.
    fn sla_configuration(
        &self,
        item: &RelatedItem,
        sla: &SlaInputConfiguration,
        now: SystemTime,
    ) -> SlaConfiguration {
        let target_time = item
            .association_time
            .checked_add(::std::time::Duration::from_secs(
                (sla.target_sla_minutes.max(0) as u64).saturating_mul(60),
            ))
            .unwrap_or(item.association_time);
        let completion_time = self.sla_completion_time(item, sla);
        let status = match completion_time {
            Some(reached) if reached <= target_time => SLA_STATUS_MET,
            Some(_) => SLA_STATUS_NOT_MET,
            None if now <= target_time => SLA_STATUS_ACTIVE,
            None => SLA_STATUS_OVERDUE,
        };
        SlaConfiguration {
            name: sla.name.clone(),
            r#type: sla.r#type.clone(),
            status: status.to_string(),
            field_id: sla.field_id.clone(),
            target_field_values: sla.target_field_values.clone(),
            target_time: timestamp(target_time),
            completion_time: completion_time.map(timestamp),
        }
    }

    /// When the tracked field of an SLA association first reached one of the
    /// values that SLA targets, or `None` while it has reached none of them.
    ///
    /// The one modelled `SlaType` is `CaseField`
    /// (`smithy_query(query="shape", shape="SlaType")`), so an SLA is reached
    /// when the case's value for the field it names becomes one of its target
    /// values. The case's own audit history is the record of WHEN each of its
    /// field values was written, so the completion instant is the earliest
    /// recorded edit, at or after the association, that wrote a target value;
    /// an SLA whose field already held a target value when it was created was
    /// reached at the association itself. An SLA that names no field, or targets
    /// no value, has no condition to reach.
    fn sla_completion_time(
        &self,
        item: &RelatedItem,
        sla: &SlaInputConfiguration,
    ) -> Option<SystemTime> {
        let field_id = sla.field_id.as_deref()?;
        let targets = sla.target_field_values.as_deref().unwrap_or_default();
        if targets.is_empty() {
            return None;
        }
        let reached = |value: &FieldValueUnion| {
            targets.iter().any(|target| {
                compared_field_values(target, value) == Some(::std::cmp::Ordering::Equal)
            })
        };
        let recorded = self
            .audit_events
            .iter()
            .filter(|event| event.performed_time >= item.association_time)
            .find(|event| {
                event
                    .fields
                    .iter()
                    .any(|field| field.field_id == field_id && reached(&field.new_value))
            })
            .map(|event| event.performed_time);
        if recorded.is_some() {
            return recorded;
        }
        self.field_values
            .get(field_id)
            .filter(|value| reached(value))
            .map(|_| item.association_time)
    }
}

/// Does one related item satisfy one `RelatedItemTypeFilter` clause?
///
/// A clause names the TYPE it is about — the union member it is given as — so a
/// clause of one type never admits an item of another, which is what makes a
/// list of clauses a filter on types. Within its type, every part the clause
/// names has to match, and a part it leaves out constrains nothing, so a clause
/// that names no part at all admits every item of that type.
pub fn related_item_matches(content: &RelatedItemContent, filter: &RelatedItemTypeFilter) -> bool {
    match (filter, content) {
        // `CommentFilter` declares no members at all
        // (`smithy_query(query="shape", shape="CommentFilter")`): it narrows to
        // the comments and no further.
        (RelatedItemTypeFilter::Comment(_), RelatedItemContent::Comment(_)) => true,
        (RelatedItemTypeFilter::Contact(wanted), RelatedItemContent::Contact(contact)) => {
            wanted
                .contact_arn
                .as_deref()
                .is_none_or(|arn| arn == contact.contact_arn)
                && wanted
                    .channel
                    .as_deref()
                    .is_none_or(|channels| channels.contains(&contact.channel))
        }
        (RelatedItemTypeFilter::File(wanted), RelatedItemContent::File(file)) => wanted
            .file_arn
            .as_deref()
            .is_none_or(|arn| arn == file.file_arn),
        (RelatedItemTypeFilter::Sla(wanted), RelatedItemContent::Sla(sla)) => {
            wanted
                .name
                .as_deref()
                .is_none_or(|name| name == sla.sla_configuration.name)
                && wanted
                    .status
                    .as_deref()
                    .is_none_or(|status| status == sla.sla_configuration.status)
        }
        (RelatedItemTypeFilter::connectCase(wanted), RelatedItemContent::connectCase(link)) => {
            wanted
                .case_id
                .as_deref()
                .is_none_or(|case_id| case_id == link.case_id)
        }
        (RelatedItemTypeFilter::Custom(wanted), RelatedItemContent::Custom(custom)) => wanted
            .fields
            .as_ref()
            .is_none_or(|fields| custom_fields_filter_matches(&custom.fields, fields)),
        _ => false,
    }
}

/// Do the field values of a custom related item satisfy `filter`?
///
/// `andAll` over no clauses narrows nothing and `orAll` over no clauses admits
/// nothing, which is what the conjunction and the disjunction of an empty set
/// of conditions are. The recursion is bounded by [`MAX_CASE_FILTER_DEPTH`],
/// which [`custom_fields_filter_too_complex`] enforces before anything is
/// matched.
pub fn custom_fields_filter_matches(fields: &[FieldValue], filter: &CustomFieldsFilter) -> bool {
    match filter {
        CustomFieldsFilter::Field(field) => field_filter_matches(
            held_field_value(fields, &field_filter_operand(field).id),
            field,
        ),
        CustomFieldsFilter::Not(inner) => !custom_fields_filter_matches(fields, inner),
        CustomFieldsFilter::andAll(clauses) => clauses
            .iter()
            .all(|clause| custom_fields_filter_matches(fields, clause)),
        CustomFieldsFilter::orAll(clauses) => clauses
            .iter()
            .any(|clause| custom_fields_filter_matches(fields, clause)),
    }
}

/// The value a custom item carries for `field_id`, or `None` when it carries
/// none. A stored `emptyValue` is "no value": it is the union's own empty
/// member, and exactly what a field that was never written reads back as.
fn held_field_value<'a>(fields: &'a [FieldValue], field_id: &str) -> Option<&'a FieldValueUnion> {
    fields
        .iter()
        .find(|field| field.id == field_id)
        .map(|field| &field.value)
        .filter(|value| !matches!(value, FieldValueUnion::emptyValue(_)))
}

/// `Some(message)` when a custom-item filter asks for more work than one
/// request may, else `None`. `member` is the request member the filter arrived
/// in, so the refusal names what a client has to fix.
///
/// `CustomFieldsFilter` nests through `not`, `andAll` and `orAll` with no
/// modelled bound on its depth, and the search evaluates every clause against
/// every candidate item, so both the depth and the total clause count are held
/// to [`MAX_CASE_FILTER_DEPTH`] and [`MAX_CASE_FILTER_CLAUSES`] — the same
/// ceilings a search of cases holds its own filter tree to. The walk is
/// iterative and stops at the first breach, so checking a filter is itself
/// bounded by those constants.
pub fn custom_fields_filter_too_complex(
    member: &str,
    filter: &CustomFieldsFilter,
) -> Option<String> {
    let mut pending: Vec<(&CustomFieldsFilter, usize)> = vec![(filter, 1)];
    let mut clauses: usize = 0;
    while let Some((clause, depth)) = pending.pop() {
        clauses += 1;
        if depth > MAX_CASE_FILTER_DEPTH {
            return Some(format!(
                "{member} nests deeper than {MAX_CASE_FILTER_DEPTH} levels; at most that many \
                 are evaluated in one request"
            ));
        }
        if clauses > MAX_CASE_FILTER_CLAUSES {
            return Some(format!(
                "{member} carries more than {MAX_CASE_FILTER_CLAUSES} clauses; at most that \
                 many are evaluated in one request"
            ));
        }
        match clause {
            CustomFieldsFilter::Field(_) => {}
            CustomFieldsFilter::Not(inner) => pending.push((inner, depth + 1)),
            CustomFieldsFilter::andAll(list) | CustomFieldsFilter::orAll(list) => {
                if let Some(message) =
                    list_too_long(member, list.len(), MAX_CASE_FILTER_CLAUSES_PER_LIST)
                {
                    return Some(message);
                }
                pending.extend(list.iter().map(|clause| (clause, depth + 1)));
            }
        }
    }
    None
}

/// Every field id a custom-item filter compares on, so they can be resolved
/// against the domain's own schema before anything is matched. Iterative, and
/// run only after [`custom_fields_filter_too_complex`] has bounded the tree.
pub fn custom_fields_filter_field_ids(filter: &CustomFieldsFilter) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut pending: Vec<&CustomFieldsFilter> = vec![filter];
    while let Some(clause) = pending.pop() {
        match clause {
            CustomFieldsFilter::Field(field) => {
                ids.push(field_filter_operand(field).id.clone());
            }
            CustomFieldsFilter::Not(inner) => pending.push(inner),
            CustomFieldsFilter::andAll(list) | CustomFieldsFilter::orAll(list) => {
                pending.extend(list.iter());
            }
        }
    }
    ids
}

/// One `RelatedItemTypeFilter` clause rendered so that two requests carrying
/// the same clause render the same text and two carrying different clauses do
/// not: what a related-item search builds its paging scope out of, so a
/// continuation token belongs to the criteria it was issued for. A token stamps
/// a DIGEST of the whole scope ([`page_scope`]), so nothing a clause carries is
/// readable from it.
pub fn related_item_filter_fingerprint(filter: &RelatedItemTypeFilter) -> String {
    match filter {
        RelatedItemTypeFilter::Comment(_) => "comment()".to_string(),
        RelatedItemTypeFilter::Contact(wanted) => format!(
            "contact({:?} {:?})",
            wanted.contact_arn.as_deref(),
            // The `Option` itself, not the list inside it: a clause naming an
            // EMPTY channel list admits no contact and one naming none admits
            // every contact, so the two are different criteria and must not
            // render alike.
            wanted.channel.as_deref()
        ),
        RelatedItemTypeFilter::File(wanted) => {
            format!("file({:?})", wanted.file_arn.as_deref())
        }
        RelatedItemTypeFilter::Sla(wanted) => format!(
            "sla({:?} {:?})",
            wanted.name.as_deref(),
            wanted.status.as_deref()
        ),
        RelatedItemTypeFilter::connectCase(wanted) => {
            format!("connectCase({:?})", wanted.case_id.as_deref())
        }
        RelatedItemTypeFilter::Custom(wanted) => format!(
            "custom({})",
            wanted
                .fields
                .as_ref()
                .map(|fields| custom_fields_filter_fingerprint(fields, 0))
                .unwrap_or_default()
        ),
    }
}

/// [`related_item_filter_fingerprint`]'s rendering of a custom-item filter
/// tree, with the nesting it has descended so far, so the recursion is bounded
/// by this function rather than only by its caller having run
/// [`custom_fields_filter_too_complex`] first. Reaching the bound is
/// unreachable through a handler, which refuses a deeper filter before anything
/// renders it.
fn custom_fields_filter_fingerprint(filter: &CustomFieldsFilter, depth: usize) -> String {
    if depth >= MAX_CASE_FILTER_DEPTH {
        return "deeper".to_string();
    }
    match filter {
        CustomFieldsFilter::Field(field) => field_filter_fingerprint(field),
        CustomFieldsFilter::Not(inner) => {
            format!("not({})", custom_fields_filter_fingerprint(inner, depth + 1))
        }
        CustomFieldsFilter::andAll(clauses) => {
            format!("and({})", fingerprints(clauses, depth + 1))
        }
        CustomFieldsFilter::orAll(clauses) => format!("or({})", fingerprints(clauses, depth + 1)),
    }
}

/// The clauses of one filter list, rendered in the order they were given: two
/// orders of the same clauses are two filters, because `orAll` and `andAll` are
/// evaluated in order.
fn fingerprints(clauses: &[CustomFieldsFilter], depth: usize) -> String {
    clauses
        .iter()
        .map(|clause| custom_fields_filter_fingerprint(clause, depth))
        .collect::<Vec<String>>()
        .join(",")
}
