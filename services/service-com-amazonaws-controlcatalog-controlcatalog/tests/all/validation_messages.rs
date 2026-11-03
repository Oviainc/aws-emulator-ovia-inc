// SPDX-License-Identifier: CC0-1.0

//! How Control Catalog words a refused input.
//!
//! The rule under test is service-wide rather than per-operation: a request that
//! violates a constraint the model declares is answered 400 `ValidationException`
//! whose message is the AWS envelope (`N validation error(s) detected: `) over one
//! clause per violation, and each clause names the MEMBER PATH and the VIOLATED
//! CONSTRAINT only — it carries no rendering of the value that was rejected, and a
//! `@pattern` requirement is stated without the `^` / `$` anchors the model spells
//! it with.
//!
//! So the tests are TABLES spanning the class: every constraint kind the service
//! declares (`@required`, `@length` on a string, `@length` on a list, `@range`,
//! `@pattern`, enum value set), at the top level and nested inside a filter, on all
//! six operations, and over rejected values of several shapes (short, long past any
//! echo cap, multibyte, quote- and backslash-bearing). Each row's expected message is
//! written out in full, and every row is additionally checked against the invariants
//! of the rule, so a row whose text drifted back to echoing the value fails twice.
//!
//! One clause is not stated at the member the walk reported: a constraint broken by a
//! COLLECTION's ELEMENT is stated once at the collection member, over the element
//! shape's WHOLE declared constraint set nested inside
//! `Member must satisfy constraint: [ … ]` — never at a synthetic `<n>.member` path and
//! never as the one constraint that happened to fail.
//! [`an_element_constraint_is_stated_at_the_collection_over_the_whole_set`] spans that half
//! of the class on its own.
//!
//! Every constraint quoted here comes from the model, via
//! `smithy_query(query="closure", operation="GetControl,ListControls,ListControlMappings,ListDomains,ListObjectives,ListCommonControls",
//! model_path="services/service-com-amazonaws-controlcatalog-controlcatalog/smithy-model/com-amazonaws-controlcatalog-controlcatalog.json")`.

use crate::control_catalog::{error_code, Emulator};
use service_com_amazonaws_controlcatalog_controlcatalog::state::{page_refusal, PageError};

/// The requirement a clause states at a COLLECTION member whose ELEMENTS broke a
/// constraint: the element shape's whole declared set, nested in AWS's wrapper.
fn element_set(requirements: &[&str]) -> String {
    format!(
        "Member must satisfy constraint: [{}]",
        requirements.join(", ")
    )
}

/// `ControlArn`'s `@pattern`, with the anchors the model spells it with stripped —
/// which is exactly what a clause reporting it must state.
const CONTROL_ARN_PATTERN: &str =
    r"arn:(aws(?:[-a-z]*)?):(controlcatalog|controltower):[a-zA-Z0-9-]*::control/[0-9a-zA-Z_\-]+";

/// `DomainArn`'s `@pattern`, unanchored (shape `DomainArn`).
const DOMAIN_ARN_PATTERN: &str = r"arn:(aws(?:[-a-z]*)?):controlcatalog:::domain/[0-9a-z]+";

/// `ObjectiveArn`'s `@pattern`, unanchored (shape `ObjectiveArn`).
const OBJECTIVE_ARN_PATTERN: &str = r"arn:(aws(?:[-a-z]*)?):controlcatalog:::objective/[0-9a-z]+";

/// `GovernedProvider`'s `@pattern`, unanchored (shape `GovernedProvider`).
const GOVERNED_PROVIDER_PATTERN: &str = r"[A-Z]{2,64}";

/// A control ARN the modeled shape accepts: 34 characters, `ControlArn`'s `@length`
/// minimum, so it exercises the bound itself rather than a value past it.
const SHORTEST_VALID_CONTROL_ARN: &str = "arn:aws:controlcatalog:::control/x";

/// The envelope a single violation renders into.
fn one(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// One clause of the envelope: member path, then the constraint it failed.
fn clause(path: &str, requirement: &str) -> String {
    format!("Value at '{path}' failed to satisfy constraint: {requirement}")
}

/// The clause a `@pattern` violation of `ControlArn` states.
fn control_arn_pattern_clause(path: &str) -> String {
    clause(
        path,
        &format!("Member must satisfy regular expression pattern: {CONTROL_ARN_PATTERN}"),
    )
}

/// The invariants of the rule, checked on every refusal in every table: the AWS
/// envelope, no rendering of the rejected value, and no pattern anchors.
fn assert_rule_invariants(case: &str, message: &str) {
    assert!(
        message.starts_with("1 validation error detected: ")
            || message
                .split_once(" validation errors detected: ")
                .is_some_and(|(count, _)| count.parse::<u32>().is_ok_and(|n| n > 1)),
        "case {case:?}: expected the AWS validation envelope, got {message:?}"
    );
    for clause in message
        .split_once(": ")
        .map(|(_, clauses)| clauses)
        .unwrap_or_default()
        .split("; ")
    {
        assert!(
            clause.starts_with("Value at '") || clause.starts_with("Value null at '"),
            "case {case:?}: a clause must name the member and nothing of the value, \
             got {clause:?}"
        );
    }
    assert!(
        !message.contains("Value '"),
        "case {case:?}: the message renders a rejected value, got {message:?}"
    );
    assert!(
        !message.contains('^') && !message.contains('$'),
        "case {case:?}: a reported pattern must carry no anchors, got {message:?}"
    );
}

/// Post `body` to `path_and_query`, require a 400 `ValidationException`, and return
/// the message it carried.
async fn refusal_message(
    emulator: &Emulator,
    case: &str,
    path_and_query: &str,
    body: &str,
) -> String {
    let (status, response) = emulator.post(path_and_query, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} with {response:?}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "case {case:?}: expected a ValidationException, got {response:?}"
    );
    message_of(&response)
        .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no message: {response:?}"))
}

/// The message an error body carries. The framework's own refusal envelope spells the
/// member `message`; the service's modeled `ValidationException`, which a handler
/// returns, spells it `Message` — both are read, so a refusal cannot hide from these
/// tests by travelling through the other channel.
///
/// `pub(crate)` so the sibling suite covering the refusals a HANDLER words
/// (`refusal_wording`) reads a message the same way rather than rolling its own.
pub(crate) fn message_of(response: &serde_json::Value) -> Option<String> {
    ["message", "Message"]
        .into_iter()
        .find_map(|member| response.get(member).and_then(serde_json::Value::as_str))
        .map(str::to_string)
}

/// Check a whole table of (case, route, body, expected message) rows.
async fn assert_table(rows: Vec<(String, String, String, String)>) {
    let emulator = Emulator::new();
    for (case, path_and_query, body, expected) in rows {
        let message = refusal_message(&emulator, &case, &path_and_query, &body).await;
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert_rule_invariants(&case, &message);
    }
}

/// A scalar member's own constraints — `@required`, `@length`, `@pattern` — on
/// GetControl's `ControlArn`, the one required scalar input in the service.
#[tokio::test]
async fn scalar_member_constraints_name_the_member_and_the_constraint_only() {
    let get_control = |control_arn: &str| format!(r#"{{"ControlArn":"{control_arn}"}}"#);
    let too_short_and_unmatched = format!(
        "2 validation errors detected: {}; {}",
        clause(
            "controlArn",
            "Member must have length greater than or equal to 34"
        ),
        control_arn_pattern_clause("controlArn"),
    );
    let rows = vec![
        (
            "ControlArn absent".to_string(),
            "/get-control".to_string(),
            "{}".to_string(),
            one(
                "Value null at 'controlArn' failed to satisfy constraint: \
                 Member must not be null",
            ),
        ),
        (
            "a well-formed ARN of another resource type".to_string(),
            "/get-control".to_string(),
            get_control("arn:aws:controlcatalog:::domain/abcdefghijklmnopqrstuvwxy"),
            one(&control_arn_pattern_clause("controlArn")),
        ),
        (
            "a well-formed ARN of another service".to_string(),
            "/get-control".to_string(),
            get_control("arn:aws:s3:::control/notthisservice123456"),
            one(&control_arn_pattern_clause("controlArn")),
        ),
        (
            "a partition the pattern does not admit".to_string(),
            "/get-control".to_string(),
            get_control("arn:AWS:controlcatalog:::control/4x11tie7h25sws4g7ilakhcb7"),
            one(&control_arn_pattern_clause("controlArn")),
        ),
        (
            // Longer than any echo cap a value rendering could have used, so even a
            // truncated echo would show up.
            "an identifier 400 characters long and outside the character class".to_string(),
            "/get-control".to_string(),
            get_control(&format!(
                "arn:aws:controlcatalog:::control/{}",
                "+".repeat(400)
            )),
            one(&control_arn_pattern_clause("controlArn")),
        ),
        (
            "a multibyte identifier".to_string(),
            "/get-control".to_string(),
            get_control(&format!(
                "arn:aws:controlcatalog:::control/{}",
                "é".repeat(30)
            )),
            one(&control_arn_pattern_clause("controlArn")),
        ),
        (
            // 33 characters: one past `ControlArn`'s `@length` minimum of 34, and
            // it also has no control identifier, so both constraints are reported.
            "one character under the 34-character minimum".to_string(),
            "/get-control".to_string(),
            get_control("arn:aws:controlcatalog:::control/"),
            too_short_and_unmatched.clone(),
        ),
        (
            "not an ARN at all".to_string(),
            "/get-control".to_string(),
            get_control("nope"),
            too_short_and_unmatched,
        ),
    ];
    assert_table(rows).await;
}

/// The `@length` bounds on every filter list the service declares (all of them are
/// `min 1, max 1`), and the element constraints inside those lists: a nested path is
/// reported the same value-free way a top-level member is.
#[tokio::test]
async fn filter_constraints_name_the_nested_member_and_the_constraint_only() {
    let two_arns = format!(
        r#"{{"Filter":{{"ControlArns":["{SHORTEST_VALID_CONTROL_ARN}","{SHORTEST_VALID_CONTROL_ARN}"]}}}}"#
    );
    let rows = vec![
        (
            // The recorded instance of cause 2: two ARNs where the model allows one.
            "two ControlArns in a ControlMappingFilter".to_string(),
            "/list-control-mappings".to_string(),
            two_arns,
            one(&clause(
                "filter.controlArns",
                "Member must have length less than or equal to 1",
            )),
        ),
        (
            "an empty ControlArns list".to_string(),
            "/list-control-mappings".to_string(),
            r#"{"Filter":{"ControlArns":[]}}"#.to_string(),
            one(&clause(
                "filter.controlArns",
                "Member must have length greater than or equal to 1",
            )),
        ),
        (
            "an empty CommonControlArns list".to_string(),
            "/list-control-mappings".to_string(),
            r#"{"Filter":{"CommonControlArns":[]}}"#.to_string(),
            one(&clause(
                "filter.commonControlArns",
                "Member must have length greater than or equal to 1",
            )),
        ),
        (
            "two MappingTypes where one is allowed".to_string(),
            "/list-control-mappings".to_string(),
            r#"{"Filter":{"MappingTypes":["FRAMEWORK","COMMON_CONTROL"]}}"#.to_string(),
            one(&clause(
                "filter.mappingTypes",
                "Member must have length less than or equal to 1",
            )),
        ),
        (
            // A constraint an ELEMENT broke is stated at the collection member, over
            // `ControlArn`'s whole declared set (shape `ControlArn`).
            "a malformed ARN inside ControlArns".to_string(),
            "/list-control-mappings".to_string(),
            r#"{"Filter":{"ControlArns":["arn:aws:controlcatalog:::domain/abcdefghijklmnopqrstuvwxy"]}}"#
                .to_string(),
            one(&clause(
                "filter.controlArns",
                &element_set(&[
                    "Member must have length less than or equal to 2048",
                    "Member must have length greater than or equal to 34",
                    &format!(
                        "Member must satisfy regular expression pattern: {CONTROL_ARN_PATTERN}"
                    ),
                ]),
            )),
        ),
        (
            "two GovernedProviders in a ControlFilter".to_string(),
            "/list-controls".to_string(),
            r#"{"Filter":{"GovernedProviders":["AWS","AZURE"]}}"#.to_string(),
            one(&clause(
                "filter.governedProviders",
                "Member must have length less than or equal to 1",
            )),
        ),
        (
            // The recorded instance: the element set is `GovernedProvider`'s whole
            // declared set (shape `GovernedProvider`), not the one constraint that
            // failed.
            "a lowercase GovernedProvider".to_string(),
            "/list-controls".to_string(),
            r#"{"Filter":{"GovernedProviders":["aws"]}}"#.to_string(),
            one(&clause(
                "filter.governedProviders",
                &element_set(&[
                    "Member must have length less than or equal to 64",
                    "Member must have length greater than or equal to 2",
                    &format!(
                        "Member must satisfy regular expression pattern: \
                         {GOVERNED_PROVIDER_PATTERN}"
                    ),
                ]),
            )),
        ),
        (
            "two implementation types in a ControlFilter".to_string(),
            "/list-controls".to_string(),
            r#"{"Filter":{"Implementations":{"Types":["AWS::Config::ConfigRule","AWS::SecurityHub::SecurityControl"]}}}"#
                .to_string(),
            one(&clause(
                "filter.implementations.types",
                "Member must have length less than or equal to 1",
            )),
        ),
        (
            "a domain ARN with uppercase in an ObjectiveFilter".to_string(),
            "/objectives".to_string(),
            r#"{"ObjectiveFilter":{"Domains":[{"Arn":"arn:aws:controlcatalog:::domain/ABCDEFG"}]}}"#
                .to_string(),
            one(&clause(
                "objectiveFilter.domains.1.member.arn",
                &format!("Member must satisfy regular expression pattern: {DOMAIN_ARN_PATTERN}"),
            )),
        ),
        (
            "an objective ARN with uppercase in a CommonControlFilter".to_string(),
            "/common-controls".to_string(),
            r#"{"CommonControlFilter":{"Objectives":[{"Arn":"arn:aws:controlcatalog:::objective/ABCDEFG"}]}}"#
                .to_string(),
            one(&clause(
                "commonControlFilter.objectives.1.member.arn",
                &format!("Member must satisfy regular expression pattern: {OBJECTIVE_ARN_PATTERN}"),
            )),
        ),
    ];
    assert_table(rows).await;
}

/// A constraint one of a COLLECTION's ELEMENTS broke is stated once at the collection
/// member, over the element shape's WHOLE declared constraint set.
///
/// The table spans every list of constrained scalars the service's input surface declares
/// — the three members of `ControlMappingFilter`, `ControlFilter.GovernedProviders` and
/// both members of `ImplementationFilter` — and what the rule generalises over: the
/// constraint the element broke (`@pattern`, `@length`, an enum value set), how many of the
/// element's constraints it broke (one, and two at once), and how many elements broke one
/// (one, and two). The clause is the same in every case, because the set is the model's and
/// not the request's. A violation inside a STRUCTURE element keeps its own indexed path and
/// its own single requirement — the rows for `ObjectiveFilter.Domains` and
/// `CommonControlFilter.Objectives` in
/// [`filter_constraints_name_the_nested_member_and_the_constraint_only`] are the other side
/// of this rule.
#[tokio::test]
async fn an_element_constraint_is_stated_at_the_collection_over_the_whole_set() {
    // Each element shape's whole declared set, in the order a clause states it
    // (`smithy_query(query="shape",shape="ControlArn,CommonControlArn,MappingType,\
    // GovernedProvider,ImplementationType,ImplementationIdentifier")`).
    let control_arn_set = element_set(&[
        "Member must have length less than or equal to 2048",
        "Member must have length greater than or equal to 34",
        &format!("Member must satisfy regular expression pattern: {CONTROL_ARN_PATTERN}"),
    ]);
    let common_control_arn_set = element_set(&[
        "Member must have length less than or equal to 2048",
        "Member must have length greater than or equal to 41",
        "Member must satisfy regular expression pattern: \
         arn:(aws(?:[-a-z]*)?):controlcatalog:::common-control/[0-9a-z]+",
    ]);
    // `MappingType`'s values as this service lists them: the reverse of the model's
    // declaration order.
    let mapping_type_set = element_set(&[
        "Member must satisfy enum value set: [RELATED_CONTROL, COMMON_CONTROL, FRAMEWORK]",
    ]);
    let governed_provider_set = element_set(&[
        "Member must have length less than or equal to 64",
        "Member must have length greater than or equal to 2",
        &format!("Member must satisfy regular expression pattern: {GOVERNED_PROVIDER_PATTERN}"),
    ]);
    let implementation_type_set = element_set(&[
        "Member must have length less than or equal to 2048",
        "Member must have length greater than or equal to 7",
        r"Member must satisfy regular expression pattern: [A-Za-z0-9]+(::[A-Za-z0-9_]+){2,3}",
    ]);
    let implementation_identifier_set = element_set(&[
        "Member must have length less than or equal to 256",
        "Member must have length greater than or equal to 1",
        r"Member must satisfy regular expression pattern: [a-zA-Z0-9_\.-]+",
    ]);
    // The clause a list whose `@length` maximum of 1 was broken states, which a row
    // sending two elements meets BESIDE the element clause.
    let two_elements = |collection: &str| {
        clause(
            collection,
            "Member must have length less than or equal to 1",
        )
    };

    let rows = vec![
        (
            "a control ARN of the wrong resource type in ControlArns".to_string(),
            "/list-control-mappings".to_string(),
            r#"{"Filter":{"ControlArns":["arn:aws:controlcatalog:::domain/abcdefghijklmnopqrstuvwxy"]}}"#
                .to_string(),
            one(&clause("filter.controlArns", &control_arn_set)),
        ),
        (
            // Shorter than the minimum AND outside the pattern: two constraints of one
            // element, stated as ONE validation error about one member.
            "a control ARN both too short and unmatched in ControlArns".to_string(),
            "/list-control-mappings".to_string(),
            r#"{"Filter":{"ControlArns":["nope"]}}"#.to_string(),
            one(&clause("filter.controlArns", &control_arn_set)),
        ),
        (
            "a catalog ARN of the wrong resource type in CommonControlArns".to_string(),
            "/list-control-mappings".to_string(),
            r#"{"Filter":{"CommonControlArns":["arn:aws:controlcatalog:::domain/abcdefghijklmnopqrstuvwxy"]}}"#
                .to_string(),
            one(&clause(
                "filter.commonControlArns",
                &common_control_arn_set,
            )),
        ),
        (
            // The recorded instance of the enum case.
            "a mapping type outside the value set".to_string(),
            "/list-control-mappings".to_string(),
            r#"{"Filter":{"MappingTypes":["NOT_A_REAL_MAPPING_TYPE"]}}"#.to_string(),
            one(&clause("filter.mappingTypes", &mapping_type_set)),
        ),
        (
            "a mapping type in the wrong case".to_string(),
            "/list-control-mappings".to_string(),
            r#"{"Filter":{"MappingTypes":["framework"]}}"#.to_string(),
            one(&clause("filter.mappingTypes", &mapping_type_set)),
        ),
        (
            // The recorded instance of the length-and-pattern case.
            "a lowercase governed provider".to_string(),
            "/list-controls".to_string(),
            r#"{"Filter":{"GovernedProviders":["aws"]}}"#.to_string(),
            one(&clause("filter.governedProviders", &governed_provider_set)),
        ),
        (
            "a governed provider both too short and unmatched".to_string(),
            "/list-controls".to_string(),
            r#"{"Filter":{"GovernedProviders":["a"]}}"#.to_string(),
            one(&clause("filter.governedProviders", &governed_provider_set)),
        ),
        (
            // TWO offending elements of one list: the elements are the SAME one clause
            // however many broke it, and the list's own `@length` is a second clause
            // after it (the walk reports a collection's own bound after its elements).
            "two unmatched governed providers".to_string(),
            "/list-controls".to_string(),
            r#"{"Filter":{"GovernedProviders":["aws","azure"]}}"#.to_string(),
            format!(
                "2 validation errors detected: {}; {}",
                clause("filter.governedProviders", &governed_provider_set),
                two_elements("filter.governedProviders"),
            ),
        ),
        (
            "an implementation type with too few segments".to_string(),
            "/list-controls".to_string(),
            r#"{"Filter":{"Implementations":{"Types":["AWS::Config"]}}}"#.to_string(),
            one(&clause(
                "filter.implementations.types",
                &implementation_type_set,
            )),
        ),
        (
            "an implementation identifier outside the character class".to_string(),
            "/list-controls".to_string(),
            r#"{"Filter":{"Implementations":{"Identifiers":["not a valid identifier"]}}}"#
                .to_string(),
            one(&clause(
                "filter.implementations.identifiers",
                &implementation_identifier_set,
            )),
        ),
    ];
    assert_table(rows).await;
}

/// The pagination inputs every list operation shares: `MaxResults`' `@range` (min 1,
/// max 100 — 1000 for ListControlMappings) and `NextToken`'s `@length` (max 1024),
/// each tested one past the bound. The bounds themselves are exercised in
/// [`inputs_at_a_published_bound_are_not_refused`].
#[tokio::test]
async fn pagination_constraints_name_the_member_and_the_constraint_only() {
    // The `@range` maximum each operation's own `MaxList*Results` shape declares.
    let per_operation_max = [
        ("/domains", 100),
        ("/objectives", 100),
        ("/common-controls", 100),
        ("/list-controls", 100),
        ("/list-control-mappings", 1000),
    ];
    let mut rows: Vec<(String, String, String, String)> = Vec::new();
    for (path, modeled_max) in per_operation_max {
        rows.push((
            format!("maxResults=0 on {path}"),
            format!("{path}?maxResults=0"),
            "{}".to_string(),
            one(&clause(
                "maxResults",
                "Member must have value greater than or equal to 1",
            )),
        ));
        rows.push((
            format!("maxResults one past the maximum on {path}"),
            format!("{path}?maxResults={}", modeled_max + 1),
            "{}".to_string(),
            one(&clause(
                "maxResults",
                &format!("Member must have value less than or equal to {modeled_max}"),
            )),
        ));
        rows.push((
            format!("a nextToken one character over the 1024 maximum on {path}"),
            format!("{path}?nextToken={}", "a".repeat(1025)),
            "{}".to_string(),
            one(&clause(
                "nextToken",
                "Member must have length less than or equal to 1024",
            )),
        ));
    }
    assert_table(rows).await;
}

/// The other side of every bound above: a value AT a published bound is not refused,
/// so the rule is about how a refusal is worded and not about refusing more.
#[tokio::test]
async fn inputs_at_a_published_bound_are_not_refused() {
    let emulator = Emulator::new();
    // (case, route and query, body) — each must come back with something other than a
    // constraint refusal.
    let rows: Vec<(String, String, String)> = vec![
        (
            "maxResults at the minimum".to_string(),
            "/domains?maxResults=1".to_string(),
            "{}".to_string(),
        ),
        (
            "maxResults at ListDomains' maximum".to_string(),
            "/domains?maxResults=100".to_string(),
            "{}".to_string(),
        ),
        (
            "maxResults at ListControlMappings' maximum".to_string(),
            "/list-control-mappings?maxResults=1000".to_string(),
            "{}".to_string(),
        ),
        (
            "a nextToken at the 1024-character maximum".to_string(),
            format!("/domains?nextToken={}", "a".repeat(1024)),
            "{}".to_string(),
        ),
        (
            "one ControlArn, the length the filter allows".to_string(),
            "/list-control-mappings".to_string(),
            format!(r#"{{"Filter":{{"ControlArns":["{SHORTEST_VALID_CONTROL_ARN}"]}}}}"#),
        ),
        (
            "a control ARN at ControlArn's 34-character minimum".to_string(),
            "/get-control".to_string(),
            format!(r#"{{"ControlArn":"{SHORTEST_VALID_CONTROL_ARN}"}}"#),
        ),
    ];

    for (case, path_and_query, body) in rows {
        let (status, response) = emulator.post(&path_and_query, &body).await;
        let message = message_of(&response).unwrap_or_default();
        assert!(
            !message.contains("validation error"),
            "case {case:?}: a value at a published bound must not be refused as a \
             constraint violation, got {status} with {response:?}"
        );
        // A value at a bound passes the constraint walk, so the answer comes from the
        // handler: the catalog itself (200), an ARN it does not hold (404), or a
        // pagination token it never issued — never a constraint refusal.
        assert!(
            status == 200 || status == 404 || !message.is_empty(),
            "case {case:?}: expected an answer, got {status} with {response:?}"
        );
    }
}

/// The point of the rule: whatever a caller sends, the refusal never hands it back.
/// Asserted over values that would be unmistakable if they were echoed.
#[tokio::test]
async fn a_refusal_never_echoes_the_rejected_value() {
    let emulator = Emulator::new();
    // (route, body, the distinctive value that body carries)
    let rows: Vec<(&str, String, String)> = vec![
        (
            "/get-control",
            r#"{"ControlArn":"arn:aws:controlcatalog:::domain/sensitivevalue0123456789"}"#
                .to_string(),
            "sensitivevalue0123456789".to_string(),
        ),
        (
            "/get-control",
            // A quote and a backslash: an echoed value would also have to be escaped
            // into the JSON envelope, which is a second way for it to show up.
            r#"{"ControlArn":"arn:aws:controlcatalog:::control/not\"a\\control\""}"#.to_string(),
            // The value the escapes above encode, as a decoded message would carry it.
            r#"not"a\control""#.to_string(),
        ),
        (
            "/get-control",
            format!(
                r#"{{"ControlArn":"arn:aws:controlcatalog:::control/{}"}}"#,
                "+".repeat(400)
            ),
            "+".repeat(64),
        ),
        (
            "/list-controls",
            r#"{"Filter":{"GovernedProviders":["secretprovider"]}}"#.to_string(),
            "secretprovider".to_string(),
        ),
        (
            "/objectives",
            r#"{"ObjectiveFilter":{"Domains":[{"Arn":"arn:aws:controlcatalog:::domain/SECRETDOMAIN"}]}}"#
                .to_string(),
            "SECRETDOMAIN".to_string(),
        ),
        (
            "/domains?maxResults=424242",
            "{}".to_string(),
            "424242".to_string(),
        ),
        (
            "/list-control-mappings",
            // A collection whose size the model refuses: the framework's own clause
            // renders that size as "'2 items'", which is a value AWS never produces.
            r#"{"Filter":{"ControlArns":[]}}"#.to_string(),
            "items".to_string(),
        ),
    ];

    for (path_and_query, body, secret) in rows {
        let case = format!("{path_and_query} {body}");
        let message = refusal_message(&emulator, &case, path_and_query, &body).await;
        assert!(
            !message.contains(&secret),
            "case {case:?}: the refusal echoes {secret:?}: {message:?}"
        );
        assert_rule_invariants(&case, &message);
    }
}

/// The handler side of the same rule: `page` refuses a page size outside the
/// operation's modeled `@range` for an in-process caller (unreachable from the wire,
/// because the modeled range is checked before a handler runs), and that refusal is
/// worded by the one renderer, with the requested size absent from the text.
#[test]
fn an_out_of_range_page_size_is_worded_by_the_same_renderer() {
    // (requested page size, the operation's modeled maximum, the constraint reported)
    let rows: [(i32, i32, &str); 5] = [
        (0, 100, "Member must have value greater than or equal to 1"),
        (-7, 100, "Member must have value greater than or equal to 1"),
        (101, 100, "Member must have value less than or equal to 100"),
        (
            424_242,
            100,
            "Member must have value less than or equal to 100",
        ),
        (
            1001,
            1000,
            "Member must have value less than or equal to 1000",
        ),
    ];

    for (requested, modeled_max, requirement) in rows {
        let case = format!("maxResults={requested} against a maximum of {modeled_max}");
        let message = page_refusal(PageError::MaxResultsOutOfRange {
            requested,
            modeled_max,
        });
        assert_eq!(
            message,
            one(&clause("maxResults", requirement)),
            "case {case:?}: got {message:?}"
        );
        assert!(
            !message.contains(&requested.to_string()),
            "case {case:?}: the refusal echoes the requested size: {message:?}"
        );
        assert_rule_invariants(&case, &message);
    }
}
