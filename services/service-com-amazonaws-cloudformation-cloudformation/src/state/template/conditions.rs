// SPDX-License-Identifier: CC0-1.0

//! The branch an `Fn::If` chooses.
//!
//! "Fn::If returns one value if the specified condition evaluates to true and another value
//! if the specified condition evaluates to false", and EITHER value may be a structure — a
//! whole property map, a list of policy statements, a nested list — rather than a scalar. So
//! which branch the condition chooses is decided here, once, and the two readers of a
//! template's values then resolve the branch they were given in the shape it is in: the
//! EXPRESSION reader ([`super::intrinsic`]) evaluates it to the text it substitutes, and the
//! VALUE reader ([`super::resolved_value`]) resolves it as a value, keeping the objects and
//! arrays inside it.
//!
//! The condition itself is read by [`super::condition_holds`], the one reader of the
//! `Conditions` section, so a branch is never chosen by a second reading of a condition.

use super::*;

/// The branch an `Fn::If` CHOOSES: its argument is
/// `[condition_name, value_if_true, value_if_false]`, and what this answers is the value
/// itself, unresolved — the caller resolves it in the shape it is in.
///
/// `None` for an argument that is not that triple, or for a condition this module cannot
/// decide, which is what every other unevaluable expression answers.
pub(super) fn chosen_branch<'a>(
    argument: &'a Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<&'a Value> {
    let branches = argument.as_array()?;
    if branches.len() != 3 {
        return None;
    }
    let holds = condition_holds(branches[0].as_str()?, scope, depth + 1, budget)?;
    Some(if holds { &branches[1] } else { &branches[2] })
}
