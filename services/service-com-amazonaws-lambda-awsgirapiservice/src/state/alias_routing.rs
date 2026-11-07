// SPDX-License-Identifier: CC0-1.0

//! WHICH VERSION one invocation of an alias runs on: the traffic split an alias's routing
//! configuration states, applied per invocation.
//!
//! "You can also map an alias to split invocation requests between two versions. Use the
//! `RoutingConfig` parameter to specify a second version and the percentage of invocation
//! requests that it receives" (`CreateAlias`). So the version an invocation of such an alias
//! runs on is not the alias's `FunctionVersion` alone: it is DRAWN, per invocation, with the
//! share the configuration gives the second version. That is why `ExecutedVersion` varies
//! across invocations of one alias, and it is the whole point of a canary deployment — an
//! alias reporting a split while every invocation reaches one version is exactly the failure
//! a canary exists to catch.
//!
//! [`super::FunctionRecord::resolved_version`] stays the READ: every control-plane answer
//! about an alias reports the version it invokes, which does not move because somebody
//! invoked it, and nothing here writes to the store either — a draw is a choice made for one
//! invocation, not a change a later caller can see. This module is the INVOKE path's
//! resolution ([`super::FunctionRecord::routed_version`]), and it is the only place the draw
//! happens, so no two invoking operations can split one alias's traffic two ways.
//!
//! The draw is the `rand` crate's: one uniform sample of the unit interval per invocation,
//! which is what makes the share each version receives converge on the configured weight
//! over many invocations. [`AliasRecord::version_for_draw`] is the rule the sample is read
//! by, and takes the sample, so the mapping from weights to versions is exercised without
//! one.

use super::{AliasRecord, FunctionRecord};

impl AliasRecord {
    /// The version ONE invocation of this alias runs on.
    ///
    /// An alias that shifts no traffic invokes the version it names, every time. One that
    /// does draws for each invocation, so the share of invocations a version receives is the
    /// share the configuration gives it.
    #[must_use]
    pub fn routed_version(&self) -> String {
        if self.additional_version_weights.is_empty() {
            return self.function_version.clone();
        }
        self.version_for_draw(::rand::random::<f64>())
    }

    /// The version this alias routes an invocation to whose draw on the unit interval is
    /// `drawn`.
    ///
    /// Each additional version holds the leading slice of the interval its weight gives it —
    /// "the percentage of invocation requests that it receives" — and the version the alias
    /// invokes holds whatever is left, so a draw past every additional share runs the
    /// alias's own version. A weight outside the unit interval cannot reach the store
    /// (`CreateAlias`/`UpdateAlias` refuse one), and is clamped here rather than allowed to
    /// make the slices disagree with the interval they are cut from.
    #[must_use]
    fn version_for_draw(&self, drawn: f64) -> String {
        let mut remaining = drawn;
        for (version, weight) in &self.additional_version_weights {
            let share = weight.clamp(0.0, 1.0);
            if remaining < share {
                return version.clone();
            }
            remaining -= share;
        }
        self.function_version.clone()
    }
}

impl FunctionRecord {
    /// The version label `qualifier` names for ONE INVOCATION, or `None` when this function
    /// has no such version or alias.
    ///
    /// The same resolution every read performs ([`FunctionRecord::resolved_version`]) for
    /// every qualifier but one: an alias that splits its invocations between two versions
    /// names the version THIS invocation runs on ([`AliasRecord::routed_version`]). The
    /// invoking operation resolves the qualifier through this once and runs, reports and
    /// reads the package of the version it answers, so the version an invocation reports is
    /// the version whose code it ran.
    #[must_use]
    pub fn routed_version(&self, qualifier: &str) -> Option<String> {
        match self.aliases.get(qualifier) {
            Some(alias) => Some(alias.routed_version()),
            None => self.resolved_version(qualifier),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An alias named `name` on `version`, shifting `weights` of its invocations.
    fn alias(version: &str, weights: &[(&str, f64)]) -> AliasRecord {
        AliasRecord {
            name: "live".to_string(),
            function_version: version.to_string(),
            description: None,
            additional_version_weights: weights
                .iter()
                .map(|(version, weight)| ((*version).to_string(), *weight))
                .collect(),
            revision_id: "11112222-3333-4444-5555-666677778888".to_string(),
        }
    }

    /// One row of the table below: the case, the alias, the invocation's draw, and the
    /// version that invocation runs on.
    type Row = (&'static str, AliasRecord, f64, &'static str);

    /// The RULE over the whole interval: the additional version receives exactly the share
    /// its weight states and the alias's own version receives the rest — including at each
    /// end of the interval and at the boundary between the two slices.
    #[test]
    fn a_draw_lands_on_the_version_whose_share_holds_it() {
        let rows: Vec<Row> = vec![
            ("no split, the lowest draw", alias("1", &[]), 0.0, "1"),
            ("no split, the highest draw", alias("1", &[]), 0.999_999, "1"),
            (
                "a half split, below the boundary",
                alias("1", &[("2", 0.5)]),
                0.499_999,
                "2",
            ),
            (
                "a half split, at the boundary",
                alias("1", &[("2", 0.5)]),
                0.5,
                "1",
            ),
            (
                "a half split, the lowest draw",
                alias("1", &[("2", 0.5)]),
                0.0,
                "2",
            ),
            (
                "a tenth to the second version, inside its share",
                alias("1", &[("2", 0.1)]),
                0.099_999,
                "2",
            ),
            (
                "a tenth to the second version, outside its share",
                alias("1", &[("2", 0.1)]),
                0.100_001,
                "1",
            ),
            (
                "the whole of the traffic shifted",
                alias("1", &[("2", 1.0)]),
                0.999_999,
                "2",
            ),
            (
                "nothing shifted, stated as a zero weight",
                alias("1", &[("2", 0.0)]),
                0.0,
                "1",
            ),
            (
                "the unpublished version as the alias's own",
                alias("$LATEST", &[("3", 0.25)]),
                0.3,
                "$LATEST",
            ),
            (
                "two additional versions, the first share",
                alias("1", &[("2", 0.25), ("3", 0.25)]),
                0.2,
                "2",
            ),
            (
                "two additional versions, the second share",
                alias("1", &[("2", 0.25), ("3", 0.25)]),
                0.4,
                "3",
            ),
            (
                "two additional versions, neither share",
                alias("1", &[("2", 0.25), ("3", 0.25)]),
                0.6,
                "1",
            ),
        ];
        for (case, alias, drawn, expected) in rows {
            let routed = alias.version_for_draw(drawn);
            assert_eq!(
                routed, expected,
                "case {case:?}: a draw of {drawn} must run version {expected}, got {routed:?}"
            );
        }
    }

    /// An alias that shifts no traffic needs no draw at all: the version it names is the
    /// version every invocation of it runs.
    #[test]
    fn an_alias_without_a_routing_configuration_invokes_the_version_it_names() {
        for version in ["$LATEST", "1", "7"] {
            let routed = alias(version, &[]).routed_version();
            assert_eq!(
                routed, version,
                "case {version:?}: an alias with no routing configuration invokes the \
                 version it names, got {routed:?}"
            );
        }
    }

    /// The DRAW is what makes the split observable: over many invocations of a 50/50 alias
    /// both versions are reached, and neither is reached outside the pair.
    #[test]
    fn a_split_alias_reaches_both_versions_over_many_invocations() {
        let alias = alias("1", &[("2", 0.5)]);
        let drawn: Vec<String> = (0..200).map(|_| alias.routed_version()).collect();
        for version in &drawn {
            assert!(
                version == "1" || version == "2",
                "case the drawn set: an invocation may only run one of the two configured \
                 versions, got {version:?}"
            );
        }
        for expected in ["1", "2"] {
            assert!(
                drawn.iter().any(|version| version == expected),
                "case version {expected}: 200 invocations of a 50/50 alias must reach both \
                 versions, got {} of version 1",
                drawn.iter().filter(|version| *version == "1").count()
            );
        }
    }
}
