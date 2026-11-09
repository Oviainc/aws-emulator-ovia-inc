// SPDX-License-Identifier: CC0-1.0

//! How a preferred-resource preference's include and exclude lists RESOLVE into the
//! effective set of rightsizing-recommendation candidates.
//!
//! A `PreferredResource` is written as "a combination of include and exclude lists" whose
//! values are either an exact instance type or a wildcard expression; what a READER of the
//! preference reports is an `EffectivePreferredResource`, whose `effectiveIncludeList` is
//! "the expanded version of your preferred resource's include list". Expanding one needs the
//! catalog of instance types the expression is resolved against, which is what this module
//! holds, together with the one matching rule every value — exact or wildcard — is applied
//! through. Every reader of a preference goes through
//! [`super::effective_preferred_resource`], so the preference reader, the effective-preference
//! reader and the default preference cannot disagree about which candidates a preference
//! leaves.
//!
//! The resolution is bounded by construction, because a caller chooses how many values a
//! stored list holds. The catalog is sorted once ([`sorted_catalog`]), so the types under one
//! wildcard are a CONTIGUOUS range of it that two binary searches find
//! ([`candidates_under`]); the written values are first reduced to the smallest set of
//! prefixes that names the same types ([`minimal_prefixes`]), which makes those ranges
//! disjoint; and whether an exclusion covers a candidate is one more binary search
//! ([`is_named_by`]). So resolving ONE entry costs `O(|values| log |values| + |catalog|)` and
//! reports at most `|catalog|` candidates, in ascending instance-type order, however many
//! values the entry holds — and the entry's lists are themselves capped at the 1000 values
//! the member documents ([`MAX_PREFERRED_RESOURCE_VALUES`]).
//!
//! A whole RECORD is bounded the same way: an entry names a preferred resource list, so one
//! name holds one list and `PutRecommendationPreferences` refuses a request naming the same
//! list twice. A record therefore holds at most as many entries as the model declares
//! `PreferredResourceName` values, and one response resolves at most that many candidate sets
//! per record — a constant, not a number a request chooses.

use std::sync::LazyLock;

use super::*;

/// Amazon EC2, the service that publishes which instance types exist.
use ::service_com_amazonaws_ec2_amazonec2 as ec2;

/// How many values one `PreferredResourceValues` list may hold: "You can specify up to 1000
/// values in this list" (`PreferredResource$includeList`, and `$excludeList` in the same
/// words). The cap a write is admitted against, so the resolution below has a bounded number
/// of values to resolve however many a request sends.
pub const MAX_PREFERRED_RESOURCE_VALUES: usize = 1000;

/// The catalog a preferred-resource value is resolved against, SORTED: every Amazon EC2
/// instance type, from the Amazon EC2 model's own `InstanceType` value set
/// (`ec2::ops::types::INSTANCE_TYPE_VALUES`). Read from that service rather than copied here,
/// so the candidates this service expands to are the instance types Amazon EC2 itself names —
/// "Compute Optimizer only supports Amazon EC2 instance types"
/// (`EffectivePreferredResource`).
///
/// Sorted once, on first use: in sorted order the types a wildcard expression names are a
/// contiguous range, which is what makes a resolution cost a binary search rather than a scan
/// per value.
fn sorted_catalog() -> &'static [&'static str] {
    static SORTED: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
        let mut types: Vec<&'static str> = ec2::ops::types::INSTANCE_TYPE_VALUES.to_vec();
        types.sort_unstable();
        types
    });
    SORTED.as_slice()
}

/// How many instance types the catalog names — the most candidates any resolution can report.
pub fn instance_type_count() -> usize {
    sorted_catalog().len()
}

/// The prefix a preferred-resource value matches instance types by.
///
/// `PreferredResource$includeList`: "You can specify the exact resource type value, such as
/// m5.large, or use wild card expressions, such as m5." So a value names every instance type
/// its text is a prefix of — which makes an exact type name itself, a family name every size
/// in that family, and the catch-all [`INCLUDE_EVERY_RESOURCE_TYPE`] every instance type in
/// the catalog. A trailing `*` is the same expression written explicitly and is dropped here.
fn prefix_of(value: &str) -> &str {
    value.strip_suffix('*').unwrap_or(value)
}

/// Whether the preferred-resource value `value` names the instance type `instance_type`.
pub fn value_names_instance_type(value: &str, instance_type: &str) -> bool {
    instance_type.starts_with(prefix_of(value))
}

/// The smallest set of prefixes naming exactly the instance types `values` names, sorted and
/// prefix-free: duplicates collapse, and a value another value already covers (`m5.large`
/// beside `m5`) is dropped. Sorted and prefix-free is what makes each prefix's range of the
/// catalog disjoint from the others', so a resolution touches each candidate once.
fn minimal_prefixes(values: &[String]) -> Vec<&str> {
    let mut prefixes: Vec<&str> = values.iter().map(|value| prefix_of(value)).collect();
    prefixes.sort_unstable();
    prefixes.dedup();
    let mut minimal: Vec<&str> = Vec::with_capacity(prefixes.len());
    for prefix in prefixes {
        // In sorted order every string a kept prefix covers follows it immediately, so the
        // last prefix kept is the only one that can cover this value.
        if minimal.last().is_some_and(|kept| prefix.starts_with(kept)) {
            continue;
        }
        minimal.push(prefix);
    }
    minimal
}

/// The instance types one prefix names: the contiguous range of the sorted catalog that
/// begins with it, found by two binary searches rather than a scan.
fn candidates_under(prefix: &str) -> &'static [&'static str] {
    let catalog = sorted_catalog();
    let start = catalog.partition_point(|instance_type| *instance_type < prefix);
    let rest = catalog.get(start..).unwrap_or_default();
    let length = rest.partition_point(|instance_type| instance_type.starts_with(prefix));
    rest.get(..length).unwrap_or_default()
}

/// Whether one of the sorted, prefix-free `prefixes` names `instance_type`.
///
/// At most one of them can — a shorter prefix of the same type would cover the longer one —
/// and it is the greatest prefix not above the type, so one binary search decides it.
fn is_named_by(prefixes: &[&str], instance_type: &str) -> bool {
    let above = prefixes.partition_point(|prefix| *prefix <= instance_type);
    above
        .checked_sub(1)
        .and_then(|index| prefixes.get(index))
        .is_some_and(|prefix| instance_type.starts_with(*prefix))
}

/// The effective include list of `resource` — "the expanded version of your preferred
/// resource's include list", i.e. the candidates the preference leaves, in ascending
/// instance-type order (the order the sorted catalog names them in).
///
/// The include list is expanded against the catalog, and an include list the preference does
/// not state stands for every candidate, because `PreferredResource$includeList` documents
/// that "if this isn't specified, all supported resources are included by default". Every
/// instance type the exclude list names is then left out, because those are the values "to
/// exclude from the recommendation candidates": a candidate set that still held them would
/// report an exclusion as having had no effect.
pub fn effective_include_list(resource: &PreferredResource) -> Vec<String> {
    let written = resource.include_list.as_deref().unwrap_or_default();
    let included = if written.is_empty() {
        // An include list the request leaves out, or leaves empty, includes every candidate.
        vec![prefix_of(INCLUDE_EVERY_RESOURCE_TYPE)]
    } else {
        minimal_prefixes(written)
    };
    let excluded = minimal_prefixes(resource.exclude_list.as_deref().unwrap_or_default());
    let mut candidates: Vec<String> = Vec::new();
    for prefix in included {
        for instance_type in candidates_under(prefix) {
            if !is_named_by(&excluded, instance_type) {
                candidates.push((*instance_type).to_string());
            }
        }
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A written list is reduced to the prefixes that name the same types and nothing more:
    /// duplicates collapse, a value a shorter one already covers is dropped, and an explicit
    /// wildcard is the same expression as the family it names.
    #[test]
    fn a_written_list_reduces_to_prefix_free_prefixes() {
        let cases: [(&[&str], &[&str]); 5] = [
            (&["m5.large", "m5.large"], &["m5.large"]),
            (&["m5.large", "m5"], &["m5"]),
            (&["m5*", "m5.large"], &["m5"]),
            (&["t3", "m5.large", "c5"], &["c5", "m5.large", "t3"]),
            (&["*", "m5"], &[""]),
        ];
        for (written, expected) in cases {
            let values: Vec<String> = written.iter().map(|value| (*value).to_string()).collect();
            let reduced = minimal_prefixes(&values);
            assert_eq!(
                reduced, expected,
                "case {written:?}: expected {expected:?}, got {reduced:?}"
            );
        }
    }

    /// The types one prefix names are exactly the catalog entries that begin with it, and the
    /// range found by binary search is the same set a scan of the catalog finds.
    #[test]
    fn a_prefix_names_the_catalog_entries_that_begin_with_it() {
        for prefix in ["", "m6g", "m6g.large", "t2.", "r7g", "not-an-instance-family"] {
            let found: Vec<&str> = candidates_under(prefix).to_vec();
            let scanned: Vec<&str> = sorted_catalog()
                .iter()
                .copied()
                .filter(|instance_type| instance_type.starts_with(prefix))
                .collect();
            assert_eq!(
                found, scanned,
                "case {prefix:?}: expected {} types, got {}",
                scanned.len(),
                found.len()
            );
        }
    }

    /// An exclusion covers a candidate exactly when one of its prefixes names it, whichever
    /// prefix of the set that is.
    #[test]
    fn an_exclusion_covers_the_candidates_its_prefixes_name() {
        let written = [
            "c5".to_string(),
            "m6g.large".to_string(),
            "t2*".to_string(),
        ];
        let excluded = minimal_prefixes(&written);
        let cases: [(&str, bool); 6] = [
            ("c5.large", true),
            ("c5n.large", true),
            ("m6g.large", true),
            ("m6g.xlarge", false),
            ("t2.micro", true),
            ("t3.micro", false),
        ];
        for (instance_type, expected) in cases {
            let covered = is_named_by(&excluded, instance_type);
            assert_eq!(
                covered, expected,
                "case {instance_type:?}: expected covered={expected}, got {covered}"
            );
        }
    }

    /// However many values a list holds, the resolution reports at most the catalog's own
    /// size and reports each candidate once — so no stored preference can amplify a read
    /// beyond the catalog.
    #[test]
    fn a_resolution_reports_at_most_the_catalog_once_over() {
        let every_value_the_cap_allows: Vec<String> = (0..MAX_PREFERRED_RESOURCE_VALUES)
            .map(|_| INCLUDE_EVERY_RESOURCE_TYPE.to_string())
            .collect();
        let resolved = effective_include_list(&PreferredResource {
            name: Some(PREFERRED_RESOURCE_NAME_EC2_INSTANCE_TYPES.to_string()),
            include_list: Some(every_value_the_cap_allows),
            exclude_list: Some(Vec::new()),
        });
        assert_eq!(
            resolved.len(),
            instance_type_count(),
            "expected the catalog once over, got {} of {}",
            resolved.len(),
            instance_type_count()
        );
        let mut unique = resolved.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            resolved.len(),
            "expected each candidate once, got {} of {}",
            unique.len(),
            resolved.len()
        );
    }
}
