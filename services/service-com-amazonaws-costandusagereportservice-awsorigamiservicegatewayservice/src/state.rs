// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-costandusagereportservice-awsorigamiservicegatewayservice service.
//!
//! The service's single primary resource is the *report definition*: every operation
//! this crate implements creates, reads, updates, tags or deletes one of them, so the
//! whole store is the insertion-ordered list of definitions below plus the tags
//! attached to each. Insertion order is the enumeration order
//! `DescribeReportDefinitions` pages over, which is what makes its pagination
//! deterministic.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use crate::ops::types::{ReportDefinition, ResourceNotFoundException, Tag};

/// How many report definitions one account may hold.
///
/// Grounded in the modelled `ReportLimitReachedException` doc: "This account already
/// has five reports defined. To define a new report, you must delete an existing
/// report." Enforced by `PutReportDefinition`, the only operation that adds one.
pub const MAX_REPORTS: usize = 5;

/// How many tags one report definition may hold.
///
/// Grounded in the modelled `Tag` doc: "Describes a tag. A tag is a key-value pair. You
/// can add up to 50 tags to a report definition."
/// (`smithy_query(shape="Tag")`). The modelled `TagList` length 0..200 bounds what ONE
/// request may carry; this bounds what a report ACCUMULATES across requests, so the
/// stored tag set of a report cannot grow without limit however often a caller tags it.
/// Enforced by every path that attaches tags — `PutReportDefinition` and `TagResource`
/// — through [`StoredReport::merge_tags`].
pub const MAX_TAGS_PER_REPORT: usize = 50;

/// The refusal for a `ReportName` the account does not hold, shared by every operation
/// that looks a report up by name before touching its tags.
///
/// Built with NO `Message`. The modelled exception's only member is an OPTIONAL
/// `Message`, documented as "A message to show the detail of the exception"
/// (`smithy_query(shape="ErrorMessage")`), and the error shape's own documentation
/// describes the error CLASS rather than being text this service puts on the wire — so
/// the refusal carries its error code and HTTP status alone, and nothing from the
/// request (least of all the report name) is echoed back in it. One constructor so the
/// handlers that answer this refusal cannot drift apart.
pub fn report_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException { message: None }
}

/// Why a tag merge was refused: it would leave the report holding more tags than
/// [`MAX_TAGS_PER_REPORT`].
///
/// Carries the resulting count so the refusal can name the number the caller asked for
/// beside the limit it crosses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TagLimitExceeded {
    /// How many distinct tag keys the report would hold if the merge were applied.
    pub requested: usize,
    /// The per-report limit the merge would cross: [`MAX_TAGS_PER_REPORT`].
    pub limit: usize,
}

/// One persisted report definition: the definition exactly as the caller supplied it,
/// its tags, and when it was first created.
///
/// Tags live in a `BTreeMap` because a report definition carries at most one tag per
/// key ("Each report definition can only have up to one tag with the same key", per
/// the modelled `Tag$Key` doc), and a sorted map gives tag reads a stable order.
#[derive(Debug, Clone)]
pub struct StoredReport {
    /// The definition as supplied, member for member.
    pub definition: ReportDefinition,
    /// `Key` -> `Value` for every tag currently attached to this report.
    pub tags: BTreeMap<String, String>,
    /// When `PutReportDefinition` first inserted this report.
    pub created_at: SystemTime,
}

impl StoredReport {
    /// A new stored report from a supplied definition and the tags that came with it.
    ///
    /// Refused with [`TagLimitExceeded`] when those tags alone would already take the
    /// report past [`MAX_TAGS_PER_REPORT`], so a report is never created over the limit.
    pub fn new(definition: ReportDefinition, tags: &[Tag]) -> Result<Self, TagLimitExceeded> {
        let mut stored = Self {
            definition,
            tags: BTreeMap::new(),
            created_at: SystemTime::now(),
        };
        stored.merge_tags(tags)?;
        Ok(stored)
    }

    /// Merge `tags` onto this report: a key already present takes the new value, a key
    /// not present is added. This is what `TagResource` does, and what
    /// `PutReportDefinition` does with the tags supplied beside the definition.
    ///
    /// The resulting key count is computed BEFORE anything is inserted, so a merge that
    /// would cross [`MAX_TAGS_PER_REPORT`] is refused whole: the stored tag set is left
    /// exactly as it was rather than partly grown. Re-tagging keys the report already
    /// holds never crosses the limit, because it adds no key.
    pub fn merge_tags(&mut self, tags: &[Tag]) -> Result<(), TagLimitExceeded> {
        let mut keys: BTreeSet<&str> = self.tags.keys().map(String::as_str).collect();
        keys.extend(tags.iter().map(|tag| tag.key.as_str()));
        let requested = keys.len();
        if requested > MAX_TAGS_PER_REPORT {
            return Err(TagLimitExceeded {
                requested,
                limit: MAX_TAGS_PER_REPORT,
            });
        }

        for tag in tags {
            self.tags.insert(tag.key.clone(), tag.value.clone());
        }
        Ok(())
    }

    /// Remove every tag whose key is in `keys`; a key that is not attached is ignored.
    pub fn remove_tags(&mut self, keys: &[String]) {
        for key in keys {
            self.tags.remove(key);
        }
    }

    /// This report's tags as the modelled `TagList`, in key order.
    pub fn tag_list(&self) -> Vec<Tag> {
        self.tags
            .iter()
            .map(|(key, value)| Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }
}

/// Per-service state for com-amazonaws-costandusagereportservice-awsorigamiservicegatewayservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCostandusagereportserviceAwsorigamiservicegatewayserviceState {
    /// Every report definition this account holds, in the order they were created.
    ///
    /// A `Vec` rather than a map: the account holds at most [`MAX_REPORTS`] of them, so
    /// a name lookup is a short scan, and the vector *is* the insertion order
    /// `DescribeReportDefinitions` pages over.
    pub reports: Vec<StoredReport>,
}

impl ComAmazonawsCostandusagereportserviceAwsorigamiservicegatewayserviceState {
    /// The index of the report called `report_name`, if the account holds it.
    ///
    /// Report names are the resource's identity, and AWS report names are
    /// case-sensitive, so this compares them exactly.
    pub fn position(&self, report_name: &str) -> Option<usize> {
        self.reports
            .iter()
            .position(|report| report.definition.report_name == report_name)
    }

    /// The stored report called `report_name`, if the account holds it.
    pub fn get(&self, report_name: &str) -> Option<&StoredReport> {
        self.position(report_name).map(|at| &self.reports[at])
    }

    /// The stored report called `report_name`, mutably, if the account holds it.
    pub fn get_mut(&mut self, report_name: &str) -> Option<&mut StoredReport> {
        match self.position(report_name) {
            Some(at) => self.reports.get_mut(at),
            None => None,
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCostandusagereportserviceAwsorigamiservicegatewayserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCostandusagereportserviceAwsorigamiservicegatewayserviceState>)
}
