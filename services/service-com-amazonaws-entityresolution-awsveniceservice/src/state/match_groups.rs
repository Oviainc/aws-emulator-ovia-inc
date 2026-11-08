// SPDX-License-Identifier: CC0-1.0

//! How the records of ONE run are resolved into match groups.
//!
//! "Two records are considered to match according to this rule if all of the
//! `MatchingKeys` match" (`smithy_query(query="shape",shape="Rule")`), and a
//! `matchingKey` compares every schema field that carries it
//! (`SchemaInputAttribute$matchKey`). So a group is a set of records connected by those
//! comparisons: two records that agree on one rule's keys share a `matchId`, and a
//! record no rule joins to another opens a group of its own.
//!
//! This module is the COMPARISON and nothing else: it reads no store, calls no service
//! and writes nothing, so the same records and rules always resolve the same way. The
//! comparison itself is the one this service already performs for its real-time pair
//! ([`ComAmazonawsEntityresolutionAwsveniceserviceState::rule_values`] over the fields
//! [`ComAmazonawsEntityresolutionAwsveniceserviceState::rule_fields`] resolved), so a
//! batch run and `GenerateMatchId` cannot disagree about what matches.
//!
//! Connectedness is transitive, because a `matchId` names a GROUP rather than a pair:
//! if A matches B on one rule and B matches C on another, all three are one resolved
//! entity. The records are therefore unioned (the textbook disjoint-set union with
//! path halving) instead of being compared pair by pair, which also keeps the cost
//! linear in the records per rule rather than quadratic.

use std::collections::hash_map::Entry;
use std::collections::HashMap;

use super::ComAmazonawsEntityresolutionAwsveniceserviceState as State;

/// One record of a run, as the comparison reads it.
#[derive(Debug, Clone)]
pub struct RunRecord {
    /// The `inputSourceARN` of the input source this record was read from, which is
    /// the `InputSourceARN` column of the output table.
    pub input_source_arn: String,
    /// The record's identity inside the workflow: the value of the schema mapping's
    /// `UNIQUE_ID` attribute, and `None` when the schema names no such attribute or the
    /// record carries no value for it.
    ///
    /// Carried for the match index a finished run records its records in, never
    /// compared: a `uniqueId` identifies a record, it is not something records match
    /// on.
    pub unique_id: Option<String>,
    /// Whether this record's own input source asks for normalization
    /// (`InputSource$applyNormalization`): a property of the SOURCE, so two sources of
    /// one workflow can be read differently.
    pub normalize: bool,
    /// The record's attributes, keyed by the column name the input table carries them
    /// under — which is the `fieldName` a schema mapping maps.
    pub attributes: HashMap<String, String>,
}

/// One rule of a workflow with each of its `matchingKeys` already resolved to the
/// schema fields that key compares.
///
/// Resolved ONCE per run rather than per record, which is what keeps the comparison
/// linear in the number of records.
#[derive(Debug, Clone)]
pub struct ResolvedRule {
    /// `Rule$ruleName`, reported as the `MatchRule` of every record the rule joined.
    pub rule_name: String,
    /// The fields each `matchingKey` compares, in the rule's own key order.
    pub fields: Vec<Vec<String>>,
}

/// What one record was resolved to: the group it belongs to and the rule that joined
/// it there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupAssignment {
    /// The `matchId` of the group — the same value for every record of it.
    pub match_id: String,
    /// The `ruleName` of the rule that joined this record to its group, and the EMPTY
    /// string for a record no rule joined to another: such a record stands alone, so
    /// there is no rule to name. This is the convention the real-time index already
    /// uses ([`super::IndexedRecord::match_rule`]).
    pub match_rule: String,
}

/// Resolve `records` into match groups under `rules`, answering one assignment per
/// record IN RECORD ORDER.
///
/// `mint_match_id` is the generator of a group's `matchId`, taken as an argument so the
/// grouping can be asserted on its own: production passes [`super::new_match_id`].
/// One id is minted per GROUP, never per record.
pub fn resolve(
    records: &[RunRecord],
    rules: &[ResolvedRule],
    mut mint_match_id: impl FnMut() -> String,
) -> Vec<GroupAssignment> {
    let mut sets = DisjointSets::new(records.len());
    // The rule that joined each record, as the INDEX of the first rule that did: a
    // record is reported under the rule that first put it in company, and the rules are
    // walked in the order the workflow declares them.
    let mut joined_by: Vec<Option<usize>> = vec![None; records.len()];

    for (rule_index, rule) in rules.iter().enumerate() {
        // The first record seen for each distinct comparison value under this rule.
        // Every later record with the same value joins that record's group, so one pass
        // per rule connects all of them.
        let mut first_seen: HashMap<Vec<String>, usize> = HashMap::new();
        for (index, record) in records.iter().enumerate() {
            // A rule none of whose matching keys the record carries is simply not
            // applicable to it; the NEXT rule still can be.
            let Some(values) = State::rule_values(&rule.fields, &record.attributes, record.normalize)
            else {
                continue;
            };
            match first_seen.entry(values) {
                Entry::Vacant(slot) => {
                    slot.insert(index);
                }
                Entry::Occupied(slot) => {
                    let first = *slot.get();
                    sets.union(index, first);
                    for joined in [index, first] {
                        if let Some(rule_of) = joined_by.get_mut(joined) {
                            rule_of.get_or_insert(rule_index);
                        }
                    }
                }
            }
        }
    }

    // One `matchId` per group, minted when the group's first record is reached, so the
    // ids are handed out in record order and two groups never share one.
    let mut id_of_group: HashMap<usize, String> = HashMap::new();
    (0..records.len())
        .map(|index| {
            let root = sets.root(index);
            let match_id = id_of_group
                .entry(root)
                .or_insert_with(&mut mint_match_id)
                .clone();
            let match_rule = joined_by
                .get(index)
                .copied()
                .flatten()
                .and_then(|rule_index| rules.get(rule_index))
                .map(|rule| rule.rule_name.clone())
                .unwrap_or_default();
            GroupAssignment {
                match_id,
                match_rule,
            }
        })
        .collect()
}

/// The disjoint-set forest the records are unioned in: each record starts in its own
/// set, and a matching pair merges the two sets it belongs to.
///
/// Total by construction: an index outside the forest answers itself, so nothing here
/// indexes out of bounds whatever it is handed.
#[derive(Debug)]
struct DisjointSets {
    parent: Vec<usize>,
}

impl DisjointSets {
    fn new(members: usize) -> Self {
        Self {
            parent: (0..members).collect(),
        }
    }

    /// The representative of the set `member` belongs to, flattening the path to it as
    /// it walks (path halving), so repeated lookups stay near-constant.
    fn root(&mut self, member: usize) -> usize {
        let mut current = member;
        while let Some(parent) = self.parent.get(current).copied() {
            if parent == current {
                return current;
            }
            // Point this member at its grandparent before stepping on, which is what
            // keeps the tree shallow without a second pass.
            let grandparent = self.parent.get(parent).copied().unwrap_or(parent);
            if let Some(slot) = self.parent.get_mut(current) {
                *slot = grandparent;
            }
            current = grandparent;
        }
        current
    }

    /// Merge the sets of `left` and `right`. The LOWER root wins, so a group's
    /// representative is its earliest record and the grouping does not depend on the
    /// order the unions arrived in.
    fn union(&mut self, left: usize, right: usize) {
        let left = self.root(left);
        let right = self.root(right);
        if left == right {
            return;
        }
        let (keep, merged) = if left < right {
            (left, right)
        } else {
            (right, left)
        };
        if let Some(slot) = self.parent.get_mut(merged) {
            *slot = keep;
        }
    }
}
