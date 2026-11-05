// SPDX-License-Identifier: CC0-1.0

//! The `ProjectionExpression` member: the document paths a read asks for, and what
//! those paths report of one item.
//!
//! "A string that identifies one or more attributes to retrieve from the table. These
//! attributes can include scalars, sets, or elements of a JSON document. The attributes
//! in the expression must be separated by commas" (`ProjectionExpression`) — so the
//! member is a comma-separated list of DOCUMENT PATHS, read through the same lexer,
//! path grammar and `ExpressionAttributeNames` substitution as every other expression
//! member of this service ([`crate::state::ExprParser`]), and a path that reaches inside
//! an attribute is as ordinary here as `a` is.
//!
//! What a path REPORTS is the value at that path wrapped in the containers the path
//! steps through — a map entry per attribute step and a one-element list per index step
//! — which is also how an `UPDATED_*` projection reports a path an update changed.
//!
//! A file of its own beside the item store rather than inside it: this is about the TEXT
//! one member carries and the item it narrows, not about what the store holds. It is
//! re-exported up to `crate::state`, so every caller names it `crate::state::…`.

use crate::state::{
    check_paths_disjoint, merge_reported, path_skeleton, tokenize_expression_spanned, DocumentPath,
    ExprParser, ExprToken, Item, PathStep, RequestError, Substitutions, MAX_EXPRESSION_TOKENS,
};

/// The most document paths one projection may name, whichever member states it.
///
/// A `ProjectionExpression` is already bounded by the token ceiling every expression of
/// this service is held to ([`MAX_EXPRESSION_TOKENS`]) — the shortest path a list can
/// spell takes a token, and every path after the first takes the comma before it as well
/// — and this is that same bound, named, so the legacy `AttributesToGet` is held to it
/// too: that member carries no modelled length, so without it the number of locations one
/// request can ask the service to read and report FOR EVERY ITEM IT ANSWERS would be the
/// number of entries a request body can carry. Reaching it is the ordinary
/// `ValidationException`.
pub const MAX_PROJECTED_PATHS: usize = MAX_EXPRESSION_TOKENS / 2;

/// The paths one projection reports by: held to [`MAX_PROJECTED_PATHS`], refused when
/// two of them address the same location or one inside the other, and put in the order a
/// report applies them.
///
/// Settled ONCE, before anything is read, so what [`project_paths`] does per reported item
/// is a function of this service's constants rather than of the request body.
pub fn projected_paths(
    member: &'static str,
    mut paths: Vec<DocumentPath>,
) -> Result<Vec<DocumentPath>, RequestError> {
    if paths.len() > MAX_PROJECTED_PATHS {
        return Err(RequestError::Validation(format!(
            "One or more parameter values were invalid: a projection may name at most \
             {MAX_PROJECTED_PATHS} attributes, got {}",
            paths.len()
        )));
    }
    // Two paths that address one location, or one that addresses a location inside
    // another, cannot both be reported: the request is wrong rather than one of them
    // winning silently, which is the rule an UPDATE's actions are held to as well
    // ([`crate::state::check_paths_disjoint`]).
    check_paths_disjoint(member, &paths.iter().collect::<Vec<&DocumentPath>>())?;
    // DOCUMENT-PATH order, so what a report carries does not depend on the order the
    // request happened to list the paths in: two indexes of one list are reported in index
    // order.
    paths.sort_by(|one, two| one.0.cmp(&two.0));
    Ok(paths)
}

/// The document paths one `ProjectionExpression` names, in the order it named them.
///
/// Every name is resolved before anything is read: a `#alias` the request never defined,
/// a bare reserved word, a path deeper than a stored value may nest and a list that does
/// not read as `path (, path)*` are all refusals of the REQUEST, whichever operation
/// carried it.
pub fn projection_expression_paths(
    member: &'static str,
    expression: &str,
    substitutions: Substitutions<'_>,
) -> Result<Vec<DocumentPath>, RequestError> {
    let read = tokenize_expression_spanned(member, expression)?;
    let mut parser = ExprParser::over(member, &read, substitutions);
    let mut paths = vec![parser.path()?];
    while parser.eat(&ExprToken::Comma) {
        paths.push(parser.path()?);
    }
    if !parser.at_end() {
        return Err(parser.syntax());
    }
    parser.check_no_reserved_name()?;
    Ok(paths)
}

/// What a projection of `paths` reports of `item`.
///
/// Each path contributes the value AT that path, wrapped in the containers the path
/// steps through, and a path that addresses nothing in the item contributes nothing:
/// "If any of the requested attributes are not found, they will not appear in the
/// result" (`ProjectionExpression`). Two paths that step through the same container are
/// reported under one entry for it, so projecting `a.b` and `a.c` reports one `a`.
///
/// `paths` is the list [`projected_paths`] settled: bounded, de-duplicated and in
/// document-path order, which is the order the containers of one attribute are folded
/// together in. Nothing here is a function of the request beyond that list, so one read
/// costs the same per item it reports whichever member stated the projection.
pub fn project_paths(item: &Item, paths: &[DocumentPath]) -> Item {
    let mut reported = Item::new();
    for path in paths {
        let Some(value) = path.get(item) else { continue };
        let Some((PathStep::Attribute(name), rest)) = path.0.split_first() else {
            continue;
        };
        let built = path_skeleton(rest, value.clone());
        match reported.get_mut(name) {
            Some(slot) => merge_reported(slot, built),
            None => {
                reported.insert(name.clone(), built);
            }
        }
    }
    reported
}
