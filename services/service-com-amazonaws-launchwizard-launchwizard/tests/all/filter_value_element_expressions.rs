// SPDX-License-Identifier: CC0-1.0

//! That a refused list ELEMENT is worded by the constraints of the element shape THE
//! LIST THE REQUEST CARRIED declares, driven over HTTP through the same router the
//! production emulator serves with.
//!
//! RULE: a scalar element of an input list that failed the element shape's constraints
//! is refused at its collection member with that shape's own composed constraint set —
//! the length bounds and the expression the model declares for the elements of THAT
//! list. Two operations may bind one member path to lists whose elements are different
//! shapes, so the path does not choose the wording: `ListDeployments$filters[].values`
//! holds `DeploymentFilterValue`s and
//! `ListDeploymentPatternVersions$filters[].values` holds
//! `DeploymentPatternVersionFilterValue`s. The expression the validating walk judged the
//! element by NAMES the shape, because the walk read it off the list the request
//! carried; where a value breaks no expression and so names neither list, the refusal
//! states what those shapes declare ALIKE, which is what is still true of the request
//! whichever of them judged it.
//!
//! The rows differ along what the rule generalises over: which operation (and so which
//! element shape) was asked, which of the shape's constraints the value broke (the
//! expression, the length floor, the length ceiling), whether the report names the
//! shape, the position of the offending element inside its list and of its filter
//! inside the filter list, and how many elements failed. One row per shape sends a value
//! the OTHER shape admits, so a row that quoted the wrong list's expression could not
//! pass by accident.

use serde_json::{json, Value};

use crate::deployment_update::one_pattern_per_workload;
use crate::harness::Harness;
use crate::validation_messages::{
    composed_clause, refusal_message, FILTER_VALUE_CONSTRAINTS, SHARED_FILTER_VALUE_CONSTRAINTS,
};

/// The dotted path of the collection most rows are reported at: the filter list's first
/// filter, and the `values` list inside it.
const FIRST_FILTER_VALUES: &str = "filters.1.member.values";

/// `DeploymentFilterValue`'s declared `@pattern` — the element shape of
/// `ListDeployments$filters[].values`
/// (`smithy_query(query="shape",shape="DeploymentFilterValues,DeploymentFilterValue")`:
/// @length 1..100, @pattern `(^(([A-Za-z])+([0-9A-Za-z\-\_])+)$)`).
const DEPLOYMENT_FILTER_VALUE_PATTERN: &str = r"(^(([A-Za-z])+([0-9A-Za-z\-\_])+)$)";

/// `DeploymentPatternVersionFilterValue`'s declared `@pattern` — the element shape of
/// `ListDeploymentPatternVersions$filters[].values` (`smithy_query(query="shape",
/// shape="DeploymentPatternVersionFilterValues,DeploymentPatternVersionFilterValue")`:
/// @length 1..100, @pattern `^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$`).
const VERSION_FILTER_VALUE_PATTERN: &str = r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$";

/// The `@length` bounds both element shapes declare (1..100, read off the same two
/// shape queries as the expressions above).
const VALUE_MIN_LEN: i64 = 1;
/// See [`VALUE_MIN_LEN`].
const VALUE_MAX_LEN: i64 = 100;

/// The composed requirement AWS states for an element shape whose declared constraints
/// are `@length(min, max)` and `@pattern`: the ceiling, then the floor, then the
/// expression, each as the framework words that constraint and the expression embedded
/// in the collection's own rendering.
fn composed(max: i64, min: i64, pattern: &str) -> String {
    format!(
        "[Member must have length less than or equal to {max}, Member must have length \
         greater than or equal to {min}, Member must satisfy regular expression pattern: \
         .*{pattern}.*]"
    )
}

#[test]
fn the_composed_wording_this_suite_builds_is_the_wording_the_message_suite_pins() {
    // One statement of the composed wording: the deployment-filter element shape runs
    // through the helper above and must come out as the message suite's own constant, so
    // the two suites cannot drift into different renderings of one constraint set.
    let built = composed(VALUE_MAX_LEN, VALUE_MIN_LEN, DEPLOYMENT_FILTER_VALUE_PATTERN);
    assert_eq!(
        built, FILTER_VALUE_CONSTRAINTS,
        "the composed wording must be the one tests/all/validation_messages.rs pins, got \
         {built:?}"
    );
}

/// One list member this service's inputs bind a constrained element shape to, and the
/// operation that carries it.
struct ListUnderTest {
    /// The request target the refusal answers.
    path: &'static str,
    /// A value of the operation's own `@enum` filter-key shape, so a row is refused for
    /// its VALUES rather than for the name that carries them
    /// (`DeploymentFilterKey` / `DeploymentPatternVersionFilterKey`).
    key: &'static str,
    /// The element shape's declared `@pattern`.
    pattern: &'static str,
    /// Values this shape's EXPRESSION refuses, each with what it breaks named. The walk
    /// reports that expression, which names the shape, so the whole set is stated.
    breaking_the_expression: Vec<(&'static str, String)>,
    /// Values this shape's expression ADMITS that are outside its length bounds, each
    /// with what it breaks named. The walk reports no expression, so neither list is
    /// named and only what both declare alike is stated.
    breaking_the_length: Vec<(&'static str, String)>,
    /// A value INSIDE the element shape, which must not be refused at the boundary.
    inside: &'static str,
}

/// The two lists, with a `filters` request body builder for each.
async fn lists(harness: &Harness) -> Vec<(ListUnderTest, Box<dyn Fn(Value) -> Value>)> {
    // A real workload and pattern, so a `ListDeploymentPatternVersions` row that is
    // NOT refused at the boundary names something the catalogue holds.
    let (workload, pattern) = one_pattern_per_workload(harness)
        .await
        .into_iter()
        .next()
        .expect("the catalogue holds a pattern");

    let ceiling = VALUE_MAX_LEN as usize;
    let deployments = ListUnderTest {
        path: "/listDeployments",
        key: "WORKLOAD_NAME",
        pattern: DEPLOYMENT_FILTER_VALUE_PATTERN,
        breaking_the_expression: vec![
            ("the expression", "not a valid name!!".to_string()),
            // A dotted version: the value the SIBLING list admits, and the one this
            // shape's expression can never match.
            (
                "the expression, with a value the sibling list admits",
                "1.0.0".to_string(),
            ),
            ("the expression, one character long", "A".to_string()),
            // An empty element breaks the length floor AND the expression, so the
            // report still names this shape.
            ("the length floor and the expression", String::new()),
        ],
        breaking_the_length: vec![
            ("the length ceiling by one", "A".repeat(ceiling + 1)),
            // Past the length the report echoes a value up to, so nothing but the
            // broken bound is on the report at all.
            ("the length ceiling by far", "A".repeat(4 * ceiling)),
        ],
        inside: "SAP",
    };
    let versions = ListUnderTest {
        path: "/listDeploymentPatternVersions",
        key: "updateFromVersion",
        pattern: VERSION_FILTER_VALUE_PATTERN,
        breaking_the_expression: vec![
            ("the expression", "1.0".to_string()),
            // A workload name: the value the SIBLING list admits.
            (
                "the expression, with a value the sibling list admits",
                "SAP".to_string(),
            ),
            ("the expression, with a leading zero", "01.0.0".to_string()),
            ("the expression, prefixed", "v1.0.0".to_string()),
            ("the length floor and the expression", String::new()),
        ],
        breaking_the_length: vec![
            (
                "the length ceiling by one",
                format!("1.0.{}", "1".repeat(ceiling - 3)),
            ),
            (
                "the length ceiling by far",
                format!("1.0.{}", "1".repeat(4 * ceiling)),
            ),
        ],
        inside: "9.9.9",
    };

    let deployment_filters: Box<dyn Fn(Value) -> Value> =
        Box::new(|filters| json!({"filters": filters}));
    let version_filters: Box<dyn Fn(Value) -> Value> = Box::new(move |filters| {
        json!({
            "workloadName": workload,
            "deploymentPatternName": pattern,
            "filters": filters,
        })
    });

    vec![
        (deployments, deployment_filters),
        (versions, version_filters),
    ]
}

/// One filter naming `values`, under the filter key `list` is asked with.
fn filter(list: &ListUnderTest, values: &[&str]) -> Value {
    json!([{"name": list.key, "values": values}])
}

#[tokio::test]
async fn a_refused_filter_value_quotes_the_expression_its_own_list_declares() {
    let harness = Harness::new();
    let lists = lists(&harness).await;
    let declared_patterns: Vec<&str> = lists.iter().map(|(list, _)| list.pattern).collect();

    for (list, body_of) in &lists {
        let expected = composed_clause(
            FIRST_FILTER_VALUES,
            &composed(VALUE_MAX_LEN, VALUE_MIN_LEN, list.pattern),
        );
        for (broken, value) in &list.breaking_the_expression {
            let case = format!("{}: a value breaking {broken}", list.path);
            let message = refusal_message(
                &harness,
                &case,
                list.path,
                body_of(filter(list, &[value.as_str()])),
            )
            .await;
            assert_eq!(
                message, expected,
                "case {case:?}: expected message {expected:?}, got {message:?}"
            );
            // The wording states the rule this caller's value was judged by, and no
            // other list's rule: an expression no legal value of this member can match
            // leaves the caller unable to fix the request from the response.
            for other in &declared_patterns {
                assert!(
                    *other == list.pattern || !message.contains(*other),
                    "case {case:?}: the refusal quotes another list's expression \
                     {other:?}: {message:?}"
                );
            }
            // The rejected value is not replayed into the error channel. A value of one
            // or two characters is a substring of the expression the clause quotes
            // whatever the renderer does, so it carries no evidence either way and is
            // skipped — as the message suite skips an empty one.
            assert!(
                value.chars().count() < 3 || !message.contains(value.as_str()),
                "case {case:?}: the message echoes the rejected element {value:?}: {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_value_the_report_places_in_neither_list_is_worded_by_what_both_declare() {
    let harness = Harness::new();
    let lists = lists(&harness).await;
    let declared_patterns: Vec<&str> = lists.iter().map(|(list, _)| list.pattern).collect();
    // Both element shapes declare @length 1..100 and different expressions, so a value
    // inside its own expression and outside the bounds is reported with the bound alone:
    // the refusal then states both bounds, which hold whichever list it came from, and
    // no expression, because quoting either would state a rule the request may not have
    // been judged by.
    let expected = composed_clause(FIRST_FILTER_VALUES, SHARED_FILTER_VALUE_CONSTRAINTS);

    for (list, body_of) in &lists {
        for (broken, value) in &list.breaking_the_length {
            let case = format!("{}: a value breaking {broken}", list.path);
            let message = refusal_message(
                &harness,
                &case,
                list.path,
                body_of(filter(list, &[value.as_str()])),
            )
            .await;
            assert_eq!(
                message, expected,
                "case {case:?}: expected message {expected:?}, got {message:?}"
            );
            for pattern in &declared_patterns {
                assert!(
                    !message.contains(*pattern),
                    "case {case:?}: the refusal quotes an expression the report places in \
                     neither list {pattern:?}: {message:?}"
                );
            }
            assert!(
                !message.contains(value.as_str()),
                "case {case:?}: the message echoes the rejected element: {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_wording_is_the_shapes_whichever_element_failed_and_however_many() {
    let harness = Harness::new();
    for (list, body_of) in &lists(&harness).await {
        // Rows differ in the POSITION of the offending element, the position of its
        // filter, and HOW MANY elements failed — the composed requirement is a property
        // of the element shape, so every one of them states the same set, at the path of
        // the filter that carried it.
        let broken = list
            .breaking_the_expression
            .first()
            .map(|(_, value)| value.clone())
            .expect("each list under test names a value its expression refuses");
        let rows: Vec<(String, Value, &str)> = vec![
            (
                "the offending element is the second of two".to_string(),
                filter(list, &[list.inside, broken.as_str()]),
                FIRST_FILTER_VALUES,
            ),
            (
                // Two elements failing two DIFFERENT rules of one shape: the element
                // shape's whole set is one error, stated once.
                "two offending elements, one breaking the expression and one the floor"
                    .to_string(),
                filter(list, &[broken.as_str(), ""]),
                FIRST_FILTER_VALUES,
            ),
            (
                "the offending filter is the second of two".to_string(),
                json!([
                    filter(list, &[list.inside])[0].clone(),
                    filter(list, &[broken.as_str()])[0].clone(),
                ]),
                "filters.2.member.values",
            ),
        ];

        for (label, filters, collection) in rows {
            let case = format!("{}: {label}", list.path);
            let expected = composed_clause(
                collection,
                &composed(VALUE_MAX_LEN, VALUE_MIN_LEN, list.pattern),
            );
            let message = refusal_message(&harness, &case, list.path, body_of(filters)).await;
            assert_eq!(
                message, expected,
                "case {case:?}: expected message {expected:?}, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_value_inside_a_lists_own_element_shape_is_not_refused_there() {
    let harness = Harness::new();
    let lists = lists(&harness).await;
    // Each list admits a value the other refuses, which is what makes the two element
    // shapes different shapes rather than one rule stated twice.
    for (list, body_of) in &lists {
        let case = format!("{}: a value inside its element shape", list.path);
        let reply = harness
            .send(
                "POST",
                list.path,
                Some(&body_of(filter(list, &[list.inside]))),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a value inside the element shape must not be refused, got {} with \
             body {}",
            reply.status, reply.body
        );

        for (other, other_body_of) in &lists {
            if other.path == list.path {
                continue;
            }
            let case = format!("{}: the value {} admits", other.path, list.path);
            let message = refusal_message(
                &harness,
                &case,
                other.path,
                other_body_of(filter(other, &[list.inside])),
            )
            .await;
            let expected = composed_clause(
                FIRST_FILTER_VALUES,
                &composed(VALUE_MAX_LEN, VALUE_MIN_LEN, other.pattern),
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected message {expected:?}, got {message:?}"
            );
        }
    }
}
