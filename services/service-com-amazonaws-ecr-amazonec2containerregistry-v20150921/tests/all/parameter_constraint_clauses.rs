// SPDX-License-Identifier: CC0-1.0

//! How this registry states a refused PARAMETER, end to end over the wire.
//!
//! Every refusal below already carries the right HTTP status (400) and error code
//! (`InvalidParameterException`); what these suites pin is the clause, which ECR builds
//! from the request and the model to one template — `Invalid parameter at '<parameter>'
//! failed to satisfy constraint: '<constraint>'`. Two rules, one suite each:
//!
//! 1. a value this registry does not recognise, where the modelled shape declares NO
//!    enum, fails a stable constraint phrase about the parameter — never a value set the
//!    model does not declare, and never this emulator's own catalogue of what it serves;
//! 2. which validator caught the value decides how both halves are spelled: a parameter
//!    ECR validates itself (one the request carries, and an ARN it must RESOLVE wherever
//!    the request carries one) is named by its API label with the constraint in ECR's own
//!    words, while a value the model-driven walk caught — a member of a NESTED structure,
//!    or a parameter whose value the registry only RECORDS — keeps that walk's path and
//!    the `@pattern` as the model declares it.
//!
//! Each suite is a TABLE whose rows differ along what its rule generalises over — the
//! shape of the unrecognised value and the operation asked, which constraint failed and
//! where in the request the member sits — and the recorded instance each cause was filed
//! against is ONE row, marked as such. Every case goes through the router, so each
//! exercises routing, the boundary validator, the handler, the persisted state and
//! serialization.

use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state::ACCOUNT_SETTINGS;

use crate::wire::{call, emulator, REGISTRY_ID};

/// The error code ECR states a refused parameter with: the client-input error 57 of this
/// model's 58 operations declare.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The whole clause, written out literally — the point of these suites is that the
/// service builds exactly this from the request, so computing it from the implementation
/// would assert nothing.
fn clause(parameter: &str, constraint: &str) -> String {
    format!("Invalid parameter at '{parameter}' failed to satisfy constraint: '{constraint}'")
}

/// The constraint an account setting name this registry does not serve fails.
///
/// `GetAccountSetting$name` and `PutAccountSetting$name` target `AccountSettingName`,
/// which `smithy_query(query="shape", shape="AccountSettingName")` gives as a string of
/// length 1..64 with NO enum trait — so the constraint is this stable phrase, which names
/// no value set.
const UNKNOWN_SETTING_CONSTRAINT: &str = "Invalid account setting name provided";

/// A name this registry does not serve is refused by both account-setting operations with
/// the stable constraint phrase, and the refusal publishes neither a value set nor the
/// registry's own catalogue of settings.
///
/// The rows differ along the shape of the unrecognised name — a name of no setting, a
/// real one differently cased, a prefix and a suffixed form of a real one, and the
/// shortest and longest the model's `@length(1..64)` admits — crossed with both
/// operations that take the parameter. The recorded instance is the first row.
#[tokio::test]
async fn an_unrecognised_account_setting_name_states_a_constraint_the_model_declares() {
    let longest = "A".repeat(64);
    let names: Vec<(&str, String)> = vec![
        // The recorded instance: a name of no setting at all.
        ("a name of no setting", "NOT_A_SETTING".to_string()),
        (
            "a real name in lower case",
            "basic_scan_type_version".to_string(),
        ),
        ("a prefix of a real name", "BASIC_SCAN_TYPE".to_string()),
        ("a real name with a suffix", "BLOB_MOUNTING_V2".to_string()),
        ("the shortest name the model admits", "A".to_string()),
        ("the longest name the model admits", longest),
    ];

    let router = emulator();
    let expected = clause("name", UNKNOWN_SETTING_CONSTRAINT);
    for (case, name) in names {
        for operation in ["GetAccountSetting", "PutAccountSetting"] {
            let case = format!("{case} on {operation}");
            let answer = call(
                &router,
                operation,
                &json!({"name": name, "value": "ENABLED"}),
            )
            .await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                INVALID_PARAMETER,
                "case {case:?}: expected {INVALID_PARAMETER}, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.message(),
                expected,
                "case {case:?}: expected the clause {expected:?}, got {:?}",
                answer.message()
            );
            // Two things the model does not declare, so the refusal must state neither:
            // an enum-membership failure, and the set of settings this registry serves.
            assert!(
                !answer.message().contains("enum value set"),
                "case {case:?}: the clause must assert no enum constraint, got {:?}",
                answer.message()
            );
            for spec in ACCOUNT_SETTINGS {
                assert!(
                    !answer.message().contains(spec.name),
                    "case {case:?}: the clause must not publish the setting {:?}, got {:?}",
                    spec.name,
                    answer.message()
                );
            }
        }
    }
}

/// `RepositoryName`'s `@pattern` as ECR'S OWN validation publishes it: without the `^` /
/// `$` the model anchors it with, and with the namespace separator bare, because `/` is
/// no regular-expression metacharacter and `\/` matches exactly what `/` does.
///
/// `smithy_query(query="shape", shape="RepositoryName")` gives
/// `^[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(\/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*$`.
const REPOSITORY_NAME_EXPRESSION: &str =
    r"[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*";

/// `PullThroughCacheRuleRepositoryPrefix`'s `@pattern` as ECR'S OWN validation publishes
/// it, by the same two rules as [`REPOSITORY_NAME_EXPRESSION`].
///
/// `smithy_query(query="shape", shape="PullThroughCacheRuleRepositoryPrefix")` gives
/// `^([a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(\/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*\/?|ROOT)$`.
const PREFIX_EXPRESSION: &str =
    r"([a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*/?|ROOT)";

/// `SigningRepositoryFilterValue`'s `@pattern` as THE MODEL declares it — anchors
/// included, because the model-driven walk states the expression it was given.
///
/// `smithy_query(query="shape", shape="SigningRepositoryFilter")`, member `filter`.
const SIGNING_FILTER_PATTERN: &str =
    "^(?:[a-z0-9*]+(?:[._-][a-z0-9*]+)*/)*[a-z0-9*]+(?:[._-][a-z0-9*]+)*$";

/// `PrincipalArn`'s `@pattern` as THE MODEL declares it, anchors included
/// (`smithy_query(query="shape", shape="PrincipalArn")`): the registry RECORDS a principal
/// ARN as the key of an exclusion rather than resolving it to a resource, so nothing of
/// ECR's own checks its form and the model's constraint is what refuses it.
const PRINCIPAL_ARN_PATTERN: &str =
    r"^arn:aws(-[a-z]+)*:iam::[0-9]{12}:(role|user)/[\w+=,.@-]+(/[\w+=,.@-]+)*$";

/// `ImageTagMutabilityExclusionFilterValue`'s `@pattern`, as the model declares it
/// (`smithy_query(query="shape", shape="ImageTagMutabilityExclusionFilter")`, member
/// `filter`) — the same nested-member rule reached through another operation.
const EXCLUSION_FILTER_PATTERN: &str = "^[0-9a-zA-Z._*-]{1,128}$";

/// The path the validating walk descends to the `filter` of the `rule`-th signing rule's
/// `filter`-th repository filter, with the 1-based element steps it spells
/// (`vendor/smithy-schema-server/src/constraints.rs`, "Path fidelity").
fn signing_filter_path(rule: usize, filter: usize, member: &str) -> String {
    format!("signingConfiguration.rules.{rule}.member.repositoryFilters.{filter}.member.{member}")
}

/// One signing rule: `arn` as its signing profile, plus `filters` when it carries any.
fn signing_rule(arn: &str, filters: Option<Vec<Value>>) -> Value {
    match filters {
        Some(filters) => json!({"signingProfileArn": arn, "repositoryFilters": filters}),
        None => json!({"signingProfileArn": arn}),
    }
}

/// One well-formed repository filter.
fn wildcard_filter(filter: &str) -> Value {
    json!({"filter": filter, "filterType": "WILDCARD_MATCH"})
}

/// A refused parameter is stated by whichever validator caught it: ECR's own validation
/// names the API parameter and states the constraint in ECR's words, and the model-driven
/// walk names the path it descended and states the model's own `@pattern`.
///
/// The rows differ along what the rule generalises over: where the member sits (a
/// top-level member, an element of a top-level list, a member of a structure nested in a
/// list, and the SECOND such element so a position cannot be mistaken for a name), which
/// constraint failed (`@pattern` on a plain string, on an ARN ECR resolves and on one it
/// only records, `@enum`, `@length`, `@required`), and which operation was called. The
/// three recorded instances are marked.
#[tokio::test]
async fn a_refused_parameter_is_stated_by_the_validator_that_caught_it() {
    let well_formed_profile =
        format!("arn:aws:signer:eu-west-1:{REGISTRY_ID}:/signing-profiles/WellFormed");
    let long_filter = "a".repeat(257);
    // (case, operation, request, the parameter the clause names, the constraint it states)
    let rows: Vec<(&str, &str, Value, String, String)> = vec![
        (
            "a top-level member outside its pattern",
            "CreateRepository",
            json!({"repositoryName": "Invalid_Name"}),
            "repositoryName".to_string(),
            format!("must satisfy regular expression '{REPOSITORY_NAME_EXPRESSION}'"),
        ),
        (
            // The recorded instance: a top-level ARN whose value the registry only
            // records, so the model's own constraint is what states it.
            "a top-level ARN the registry records rather than resolves",
            "RegisterPullTimeUpdateExclusion",
            json!({"principalArn": "not-an-arn"}),
            "principalArn".to_string(),
            format!("Member must satisfy regular expression pattern: {PRINCIPAL_ARN_PATTERN}"),
        ),
        (
            "the same member on the operation that removes an exclusion",
            "DeregisterPullTimeUpdateExclusion",
            json!({"principalArn": "arn:aws:iam::000000000000:group/admins"}),
            "principalArn".to_string(),
            format!("Member must satisfy regular expression pattern: {PRINCIPAL_ARN_PATTERN}"),
        ),
        (
            "an element of a top-level list, which is one value of that parameter",
            "DescribeRepositories",
            json!({"repositoryNames": ["well-formed", "ALSO-Upper"]}),
            "repositoryName".to_string(),
            format!("must satisfy regular expression '{REPOSITORY_NAME_EXPRESSION}'"),
        ),
        (
            // The recorded instance: ECR checks the form of an ARN it must RESOLVE in its
            // own code, wherever the request carries it, so the clause names the API
            // parameter and not the walk's path to it.
            "an ARN ECR resolves, inside a list of structures",
            "PutSigningConfiguration",
            json!({"signingConfiguration": {
                "rules": [signing_rule("not-a-valid-signer-arn", None)],
            }}),
            "signingProfileArn".to_string(),
            "The signing profile ARN 'not-a-valid-signer-arn' must be well formed".to_string(),
        ),
        (
            "an ARN inside the SECOND element of that list",
            "PutSigningConfiguration",
            json!({"signingConfiguration": {
                "rules": [
                    signing_rule(&well_formed_profile, None),
                    signing_rule("arn:aws:iam::000000000000:role/NotASigningProfile", None),
                ],
            }}),
            "signingProfileArn".to_string(),
            "The signing profile ARN 'arn:aws:iam::000000000000:role/NotASigningProfile' \
             must be well formed"
                .to_string(),
        ),
        (
            // The recorded instance: a member of a structure NESTED in the request, which
            // nothing of ECR's own names, so the walk's path and wording stand.
            "a nested structure's member outside its pattern",
            "PutSigningConfiguration",
            json!({"signingConfiguration": {
                "rules": [signing_rule(
                    &well_formed_profile,
                    Some(vec![wildcard_filter("Team/App")]),
                )],
            }}),
            signing_filter_path(1, 1, "filter"),
            format!("Member must satisfy regular expression pattern: {SIGNING_FILTER_PATTERN}"),
        ),
        (
            "the same member in the second rule's second filter",
            "PutSigningConfiguration",
            json!({"signingConfiguration": {
                "rules": [
                    signing_rule(&well_formed_profile, Some(vec![wildcard_filter("team/app")])),
                    signing_rule(
                        &well_formed_profile,
                        Some(vec![wildcard_filter("team/app"), wildcard_filter("Team/App")]),
                    ),
                ],
            }}),
            signing_filter_path(2, 2, "filter"),
            format!("Member must satisfy regular expression pattern: {SIGNING_FILTER_PATTERN}"),
        ),
        (
            "a nested structure's member outside its enum",
            "PutSigningConfiguration",
            json!({"signingConfiguration": {
                "rules": [signing_rule(
                    &well_formed_profile,
                    Some(vec![json!({"filter": "team/app", "filterType": "EXACT_MATCH"})]),
                )],
            }}),
            signing_filter_path(1, 1, "filterType"),
            "Member must satisfy enum value set: [WILDCARD_MATCH]".to_string(),
        ),
        (
            "a nested structure's member past its maximum length",
            "PutSigningConfiguration",
            json!({"signingConfiguration": {
                "rules": [signing_rule(
                    &well_formed_profile,
                    Some(vec![wildcard_filter(&long_filter)]),
                )],
            }}),
            signing_filter_path(1, 1, "filter"),
            "Member must have length less than or equal to 256".to_string(),
        ),
        (
            "a nested structure's required member left out",
            "PutSigningConfiguration",
            json!({"signingConfiguration": {
                "rules": [signing_rule(
                    &well_formed_profile,
                    Some(vec![json!({"filterType": "WILDCARD_MATCH"})]),
                )],
            }}),
            signing_filter_path(1, 1, "filter"),
            "Member must not be null".to_string(),
        ),
        (
            "a nested structure's member in another operation",
            "CreateRepository",
            json!({
                "repositoryName": "well-formed",
                "imageTagMutability": "MUTABLE_WITH_EXCLUSION",
                "imageTagMutabilityExclusionFilters": [
                    {"filterType": "WILDCARD", "filter": "team/app"},
                ],
            }),
            "imageTagMutabilityExclusionFilters.1.member.filter".to_string(),
            format!("Member must satisfy regular expression pattern: {EXCLUSION_FILTER_PATTERN}"),
        ),
        (
            // The same element rule reached through another operation and another list of
            // names, so the singular is the rule rather than one operation's wording.
            "an element of another operation's name list",
            "BatchGetRepositoryScanningConfiguration",
            json!({"repositoryNames": ["well-formed", "Also-Upper"]}),
            "repositoryName".to_string(),
            format!("must satisfy regular expression '{REPOSITORY_NAME_EXPRESSION}'"),
        ),
        (
            // A collection whose name is the `-es` plural of its parameter, so undoing the
            // plural is more than dropping an `s`.
            "an element of a list whose name takes the -es plural",
            "DescribePullThroughCacheRules",
            json!({"ecrRepositoryPrefixes": ["Not A Prefix"]}),
            "ecrRepositoryPrefix".to_string(),
            format!("must satisfy regular expression '{PREFIX_EXPRESSION}'"),
        ),
        (
            "an element of the shortest such list name",
            "DescribeRepositoryCreationTemplates",
            json!({"prefixes": ["Not A Prefix"]}),
            "prefix".to_string(),
            format!("must satisfy regular expression '{PREFIX_EXPRESSION}'"),
        ),
    ];

    let router = emulator();
    for (case, operation, request, parameter, constraint) in rows {
        let answer = call(&router, operation, &request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 from {operation}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER} from {operation}, got {:?}",
            answer.body
        );
        let expected = clause(&parameter, &constraint);
        assert_eq!(
            answer.message(),
            expected,
            "case {case:?}: expected the clause {expected:?}, got {:?}",
            answer.message()
        );
    }
}
