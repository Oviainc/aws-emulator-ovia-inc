// SPDX-License-Identifier: CC0-1.0

//! The dashboard-body schema every operation that takes a dashboard definition
//! validates `body` against, and the refusal text a violated schema renders.
//!
//! `body`'s model doc is "The dashboard definition, as a JSON document", and the model
//! can express only its length — the document's own schema is not modelled, so it is
//! stated here: the definition is a JSON OBJECT, CLOSED at the top level, whose single
//! member `panels` is required and is a list of panel objects.
//!
//! A body is validated against that schema as a whole rather than up to its first
//! problem: EVERY constraint it violates is collected, and the refusal names each one as
//! its own clause, so one round trip tells a caller everything it has to fix. A clause
//! is `<location>: does not satisfy the '<constraint>' constraint of the dashboard body
//! schema`, and the clauses are joined with `; `. A location is the document root
//! ([`DOCUMENT_ROOT`]) for a constraint the whole document violates, and the member's
//! name for one a named member violates. A constraint is named as the JSON-schema
//! keyword it is: `type`, `required`, `items`, `additionalProperties`.

/// The member of a dashboard body that carries the dashboard's panels.
pub const DASHBOARD_BODY_PANELS: &str = "panels";

/// The location a constraint on the whole document is reported at.
pub const DOCUMENT_ROOT: &str = "$";

/// The keyword for "this value is of the wrong JSON type" — the document root when it is
/// not an object (text that is not JSON at all does not parse into one either), or a
/// member whose value is not of the type the schema gives it.
const CONSTRAINT_TYPE: &str = "type";

/// The keyword for "a member the schema requires is absent".
const CONSTRAINT_REQUIRED: &str = "required";

/// The keyword for "a list member holds an entry the schema does not admit".
const CONSTRAINT_ITEMS: &str = "items";

/// The keyword for "this object carries a member the schema does not declare" — the
/// schema is closed, so any top-level member beyond `panels` violates it.
const CONSTRAINT_ADDITIONAL_PROPERTIES: &str = "additionalProperties";

/// What joins one violation's clause to the next in the refusal text.
const CLAUSE_SEPARATOR: &str = "; ";

/// One violated constraint of the dashboard-body schema: WHERE in the document it was
/// violated, and WHICH constraint was violated there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashboardBodyViolation {
    /// The document root, or the name of the member that violated the constraint.
    pub location: String,
    /// The JSON-schema keyword the document violates at [`Self::location`].
    pub constraint: &'static str,
}

impl DashboardBodyViolation {
    /// This violation as the one clause it contributes to the refusal text.
    pub fn clause(&self) -> String {
        format!(
            "{}: does not satisfy the '{}' constraint of the dashboard body schema",
            self.location, self.constraint
        )
    }

    /// A constraint the whole document violates.
    fn at_root(constraint: &'static str) -> Self {
        Self {
            location: DOCUMENT_ROOT.to_string(),
            constraint,
        }
    }

    /// A constraint the named member violates.
    fn at_member(member: &str, constraint: &'static str) -> Self {
        Self {
            location: member.to_string(),
            constraint,
        }
    }
}

/// Every constraint of the dashboard-body schema `body` violates, in document order:
/// the root's own type first, then the `panels` member, then the closedness of the
/// document. An empty result means `body` IS a dashboard definition.
///
/// Collecting rather than returning at the first problem is what lets a caller fix a
/// body in one round trip: a document that both omits `panels` and carries a member the
/// schema does not declare violates two constraints, and both are reported.
pub fn dashboard_body_violations(body: &str) -> Vec<DashboardBodyViolation> {
    let document: ::serde_json::Value = match ::serde_json::from_str(body) {
        Ok(document) => document,
        // Text that is not JSON at all is no document of the schema's type either, so
        // it violates the root's `type` constraint and nothing below the root is
        // reachable to be checked.
        Err(_) => return vec![DashboardBodyViolation::at_root(CONSTRAINT_TYPE)],
    };
    let Some(members) = document.as_object() else {
        return vec![DashboardBodyViolation::at_root(CONSTRAINT_TYPE)];
    };

    let mut violations = Vec::new();
    match members.get(DASHBOARD_BODY_PANELS) {
        None => violations.push(DashboardBodyViolation::at_member(
            DASHBOARD_BODY_PANELS,
            CONSTRAINT_REQUIRED,
        )),
        Some(panels) => match panels.as_array() {
            None => violations.push(DashboardBodyViolation::at_member(
                DASHBOARD_BODY_PANELS,
                CONSTRAINT_TYPE,
            )),
            Some(panels) if panels.iter().any(|panel| !panel.is_object()) => violations.push(
                DashboardBodyViolation::at_member(DASHBOARD_BODY_PANELS, CONSTRAINT_ITEMS),
            ),
            Some(_) => {}
        },
    }
    if members.keys().any(|name| name != DASHBOARD_BODY_PANELS) {
        violations.push(DashboardBodyViolation::at_root(
            CONSTRAINT_ADDITIONAL_PROPERTIES,
        ));
    }
    violations
}

/// The refusal text for a `body` that is not a dashboard definition — one clause per
/// violated constraint, joined with `; ` — or `None` when it is one.
///
/// This is the message an operation carries into its modelled `ValidationException`.
pub fn dashboard_body_rejection(body: &str) -> Option<String> {
    let violations = dashboard_body_violations(body);
    (!violations.is_empty()).then(|| {
        violations
            .iter()
            .map(DashboardBodyViolation::clause)
            .collect::<Vec<String>>()
            .join(CLAUSE_SEPARATOR)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_document_violating_two_constraints_reports_both_in_document_order() {
        // Rows: the body, and the (location, constraint) pairs the schema says it
        // violates — in the order the clauses are rendered.
        for (body, expected) in [
            (r#"{"panels":[]}"#, vec![]),
            (r#"{"panels":[{"type":"logs"}]}"#, vec![]),
            (
                r#"{"widgets":[]}"#,
                vec![("panels", "required"), ("$", "additionalProperties")],
            ),
            (r#"{}"#, vec![("panels", "required")]),
            (
                r#"{"panels":"none","widgets":[]}"#,
                vec![("panels", "type"), ("$", "additionalProperties")],
            ),
            (r#"{"panels":[1]}"#, vec![("panels", "items")]),
            (r#"{"panels":[],"widgets":[]}"#, vec![("$", "additionalProperties")]),
            (r#"[{"type":"logs"}]"#, vec![("$", "type")]),
            ("panels", vec![("$", "type")]),
        ] {
            let actual: Vec<(String, &str)> = dashboard_body_violations(body)
                .into_iter()
                .map(|violation| (violation.location, violation.constraint))
                .collect();
            let actual: Vec<(&str, &str)> = actual
                .iter()
                .map(|(location, constraint)| (location.as_str(), *constraint))
                .collect();
            assert_eq!(
                actual, expected,
                "case {body:?}: expected violations {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn the_refusal_joins_one_clause_per_violation() {
        assert_eq!(
            dashboard_body_rejection(r#"{"widgets":[]}"#).as_deref(),
            Some(
                "panels: does not satisfy the 'required' constraint of the dashboard body \
                 schema; $: does not satisfy the 'additionalProperties' constraint of the \
                 dashboard body schema"
            ),
            "a document that omits panels and carries another member reports both clauses"
        );
        assert_eq!(
            dashboard_body_rejection(r#"{"panels":[]}"#),
            None,
            "a conforming document is not refused"
        );
    }
}
