// SPDX-License-Identifier: CC0-1.0

//! The routes of this service whose URI template puts LITERAL segments AFTER a
//! greedy label, and the one rule that serves them.
//!
//! A greedy `@httpLabel` (`{Name+}`) matches a sequence of path segments, so a
//! template that places literals after one — `/v20180820/mrap/instances/{Name+}/policy`
//! — is matched by reading those trailing literals FIRST and binding the label
//! to what is left between the leading literals and them. A matcher that lets
//! the greedy label run to the end of the path instead can never match such a
//! template at all: the literal after it has no segment left to match, so the
//! request falls through to the sibling route whose label ends the path
//! (`/v20180820/mrap/instances/{Name+}`), which then binds the label to the
//! sub-resource suffix as part of the resource's own name. Every operation
//! bound to a template of this shape is unreachable, for every value of its
//! label.
//!
//! This module is the rule for this service: [`GREEDY_SUFFIX_ROUTES`] holds
//! every route of this model whose template has that shape, and
//! [`resolve`] answers, for one request, the operation it names together with
//! the path its `@httpLabel` members bind from — the request path with the
//! route's trailing literals removed, which is what makes the greedy label bind
//! the resource and nothing else.
//!
//! The table is derived from the model, not from any one request:
//! `tests/all/multi_region_access_point_sub_resources.rs` re-derives it from
//! `smithy-model/` and fails if the model grows, loses or renames a route of
//! this shape.

/// One route whose URI template puts literal segments after its greedy label.
#[derive(Debug, Clone, Copy)]
pub struct GreedySuffixRoute {
    /// The HTTP method the model binds the operation to.
    pub method: &'static str,
    /// The operation's `@http.uri` template, verbatim from the model.
    pub uri: &'static str,
    /// The operation's shape name, which is the key the generated routing table
    /// wires it under.
    pub operation: &'static str,
}

/// Every route of this service's model whose `@http.uri` puts a literal segment
/// after a greedy label.
///
/// All four are Multi-Region Access Point sub-resources, each a sibling of
/// `GET /v20180820/mrap/instances/{Name+}`; the model's own templates are
/// reproduced verbatim so the segments matched here cannot drift from the ones
/// the framework binds the labels from.
pub const GREEDY_SUFFIX_ROUTES: &[GreedySuffixRoute] = &[
    GreedySuffixRoute {
        method: "GET",
        uri: "/v20180820/mrap/instances/{Name+}/policy",
        operation: "GetMultiRegionAccessPointPolicy",
    },
    GreedySuffixRoute {
        method: "GET",
        uri: "/v20180820/mrap/instances/{Name+}/policystatus",
        operation: "GetMultiRegionAccessPointPolicyStatus",
    },
    GreedySuffixRoute {
        method: "GET",
        uri: "/v20180820/mrap/instances/{Mrap+}/routes",
        operation: "GetMultiRegionAccessPointRoutes",
    },
    GreedySuffixRoute {
        method: "PATCH",
        uri: "/v20180820/mrap/instances/{Mrap+}/routes",
        operation: "SubmitMultiRegionAccessPointRoutes",
    },
];

/// The segments of a path or template, with the empty ones a leading, trailing
/// or doubled separator produces dropped.
fn segments(path: &str) -> Vec<&str> {
    path.split('/').filter(|segment| !segment.is_empty()).collect()
}

/// Whether a template segment is a label (`{Name}` or `{Name+}`) rather than a
/// literal.
fn is_label(segment: &str) -> bool {
    segment.starts_with('{') && segment.ends_with('}')
}

/// Whether a template segment is a GREEDY label (`{Name+}`).
fn is_greedy_label(segment: &str) -> bool {
    is_label(segment) && segment.trim_end_matches('}').ends_with('+')
}

/// The literal segments a template places before its greedy label, and the ones
/// it places after it — `None` for a template with no greedy label, or with
/// nothing after it, which the framework's own matcher already serves.
///
/// A template whose trailing segments are not all literals is also `None`: the
/// rule here is about reading FIXED sub-resource segments off the end of a
/// path, and nothing in this model binds a second label after a greedy one.
fn around_greedy_label(uri: &str) -> Option<(Vec<&str>, Vec<&str>)> {
    let template = segments(uri);
    let greedy_at = template.iter().position(|segment| is_greedy_label(segment))?;
    let (before, from_greedy) = template.split_at(greedy_at);
    let after = from_greedy.get(1..)?;
    if after.is_empty() || after.iter().any(|segment| is_label(segment)) {
        return None;
    }
    Some((before.to_vec(), after.to_vec()))
}

/// The operation `method` and `path` name under this rule, with the path its
/// `@httpLabel` members bind from, or `None` when no route of
/// [`GREEDY_SUFFIX_ROUTES`] matches.
///
/// The bound path is the request path less the route's trailing literals, so
/// the greedy label binds the resource the request addressed and not the
/// sub-resource segment that follows it. A path that leaves NO segment for the
/// label matches nothing: a greedy label matches one segment or more.
pub fn resolve(method: &str, path: &str) -> Option<(&'static str, String)> {
    let requested = segments(path);
    for route in GREEDY_SUFFIX_ROUTES {
        if !route.method.eq_ignore_ascii_case(method) {
            continue;
        }
        let Some((before, after)) = around_greedy_label(route.uri) else {
            continue;
        };
        // One segment for the greedy label at least, beside every literal.
        if requested.len() <= before.len() + after.len() {
            continue;
        }
        let (requested_before, rest) = requested.split_at(before.len());
        if requested_before != before.as_slice() {
            continue;
        }
        let Some(suffix_at) = rest.len().checked_sub(after.len()) else {
            continue;
        };
        let (label, requested_after) = rest.split_at(suffix_at);
        if requested_after != after.as_slice() {
            continue;
        }
        let bound: Vec<&str> = requested_before.iter().chain(label.iter()).copied().collect();
        return Some((route.operation, format!("/{}", bound.join("/"))));
    }
    None
}

#[cfg(test)]
mod tests {
    //! The rule read off templates and paths. The crate's integration suite
    //! drives the same routes end to end over HTTP.

    use super::*;

    #[test]
    fn a_sub_resource_path_names_its_own_operation_and_binds_the_resource() {
        let cases = [
            (
                "GET",
                "/v20180820/mrap/instances/reports-mrap/policy",
                "GetMultiRegionAccessPointPolicy",
            ),
            (
                "GET",
                "/v20180820/mrap/instances/reports-mrap/policystatus",
                "GetMultiRegionAccessPointPolicyStatus",
            ),
            (
                "GET",
                "/v20180820/mrap/instances/abcdef.mrap/routes",
                "GetMultiRegionAccessPointRoutes",
            ),
            (
                "PATCH",
                "/v20180820/mrap/instances/abcdef.mrap/routes",
                "SubmitMultiRegionAccessPointRoutes",
            ),
        ];
        for (method, path, operation) in cases {
            let resolved = resolve(method, path);
            assert_eq!(
                resolved.as_ref().map(|(named, _)| *named),
                Some(operation),
                "case {method} {path:?}: expected the sub-resource operation, got {resolved:?}"
            );
            let expected_label = path.rsplit_once('/').map(|(head, _)| head.to_string());
            assert_eq!(
                resolved.as_ref().map(|(_, bound)| bound.clone()),
                expected_label,
                "case {method} {path:?}: expected the label path without the suffix, \
                 got {resolved:?}"
            );
        }
    }

    #[test]
    fn a_path_this_rule_does_not_own_resolves_to_nothing() {
        let cases = [
            // The sibling route whose greedy label ends the path: the
            // framework's own matcher serves it.
            ("GET", "/v20180820/mrap/instances/reports-mrap"),
            // No segment left for the greedy label.
            ("GET", "/v20180820/mrap/instances/policy"),
            // A method no route of this shape is bound to.
            ("DELETE", "/v20180820/mrap/instances/reports-mrap/policy"),
            // A suffix none of them ends in.
            ("GET", "/v20180820/mrap/instances/reports-mrap/tagging"),
            // Another resource's path space entirely.
            ("GET", "/v20180820/accesspoint/reports-ap/policy"),
        ];
        for (method, path) in cases {
            let resolved = resolve(method, path);
            assert!(
                resolved.is_none(),
                "case {method} {path:?}: expected no sub-resource route, got {resolved:?}"
            );
        }
    }

    #[test]
    fn a_template_with_no_literal_after_its_greedy_label_is_not_this_rule_s() {
        for uri in [
            "/v20180820/mrap/instances/{Name+}",
            "/v20180820/mrap/instances",
            "/v20180820/accesspoint/{Name}/policy",
            "/v20180820/tags/{ResourceArn+}",
        ] {
            assert!(
                around_greedy_label(uri).is_none(),
                "case {uri:?}: expected no greedy-plus-literal shape"
            );
        }
    }
}
