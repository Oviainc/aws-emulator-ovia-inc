// SPDX-License-Identifier: CC0-1.0

//! How Config WORDS four refusals whose error code is shared by many legs, on the wire.
//!
//! Each error code below stands for more than one refusal, so the MESSAGE is the only
//! thing that tells a caller which refusal it got. Four production rules are pinned
//! here, each over a table rather than one instance:
//!
//! 1. `NoSuchConfigurationAggregatorException` carries ONE fixed sentence, whatever
//!    operation raises it and whatever name the request carried, and that sentence
//!    carries no part of the request: the aggregator name is not echoed back;
//! 2. `InvalidNextTokenException` carries ONE fixed sentence, whatever paged operation
//!    raises it and whatever the rejected token looked like, and it does not echo the
//!    token;
//! 3. `InvalidExpressionException` for a selected item that names no field of the
//!    configuration item quotes that item as the query wrote it;
//! 4. `InvalidExpressionException` for the GROUP BY coverage rule of the Config query
//!    language names the RULE and the selected fields of THIS request that broke it,
//!    while a query the engine READ and will not run for any other reason carries the
//!    one fixed sentence for that, and a command that cannot be read as a query at all
//!    keeps its own.
//!
//! Every case drives the emulator the way a client does, through the same
//! `MultiServiceRouter` the production router builds, reusing the `Emulator` harness
//! `config_setup_and_rule_loop` provides and the `refusal` helper `config_connectors`
//! already shares.

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state;

use crate::config_connectors::refusal;
use crate::config_setup_and_rule_loop::Emulator;

/// The one sentence every operation answers for an aggregator this account does not
/// hold, written out here rather than read from the service's own constant so this
/// suite PINS it instead of restating it.
const NO_SUCH_AGGREGATOR_TEXT: &str = "The configuration aggregator does not exist. \
     Check the configuration aggregator name and try again.";

/// The one sentence every paged read answers for a `NextToken` it did not issue. Pinned
/// here for the same reason as [`NO_SUCH_AGGREGATOR_TEXT`].
const INVALID_NEXT_TOKEN_TEXT: &str = "The nextToken provided is invalid";

/// The sentence that opens the refusal of a command this service cannot READ as a
/// Config query — the OTHER leg of `InvalidExpressionException`, which the GROUP BY
/// rule's own sentence must not collapse into.
const UNREADABLE_QUERY_PREFIX: &str = "The expression is not a valid Config query:";

/// A 12-digit account id, the form `AccountId` takes (`^\d{12}$`), for the members that
/// identify an aggregated resource.
const SOURCE_ACCOUNT: &str = "222222222222";

/// A Region a source aggregates from, for the same members.
const SOURCE_REGION: &str = "us-east-1";

/// An entry of `com.amazonaws.configservice#ResourceType`, from
/// `smithy_query(query="shape", shape="ResourceType")`.
const EC2_VOLUME: &str = "AWS::EC2::Volume";

/// The `AggregateResourceIdentifier` the aggregate reads below name, with every member
/// the model requires (`SourceAccountId`, `SourceRegion`, `ResourceId`, `ResourceType`).
fn resource_identifier() -> Value {
    json!({
        "SourceAccountId": SOURCE_ACCOUNT,
        "SourceRegion": SOURCE_REGION,
        "ResourceId": "vol-0abc1234",
        "ResourceType": EC2_VOLUME,
    })
}

/// Every operation that models `NoSuchConfigurationAggregatorException`, with a
/// well-formed request that names `name` as the aggregator to act on — the member it
/// names it with differs per operation (a single name, a list of names, or a name beside
/// the resource or rule the read is about), which is part of what the one sentence has
/// to survive.
fn aggregator_requests(name: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "DescribeConfigurationAggregatorSourcesStatus",
            json!({ "ConfigurationAggregatorName": name }),
        ),
        (
            "DescribeConfigurationAggregators",
            json!({ "ConfigurationAggregatorNames": [name] }),
        ),
        (
            "DeleteConfigurationAggregator",
            json!({ "ConfigurationAggregatorName": name }),
        ),
        (
            "DescribeAggregateComplianceByConfigRules",
            json!({ "ConfigurationAggregatorName": name }),
        ),
        (
            "GetAggregateConfigRuleComplianceSummary",
            json!({ "ConfigurationAggregatorName": name }),
        ),
        (
            "GetAggregateComplianceDetailsByConfigRule",
            json!({
                "ConfigurationAggregatorName": name,
                "ConfigRuleName": "rule-one",
                "AccountId": SOURCE_ACCOUNT,
                "AwsRegion": SOURCE_REGION,
            }),
        ),
        (
            "GetAggregateResourceConfig",
            json!({
                "ConfigurationAggregatorName": name,
                "ResourceIdentifier": resource_identifier(),
            }),
        ),
        (
            "BatchGetAggregateResourceConfig",
            json!({
                "ConfigurationAggregatorName": name,
                "ResourceIdentifiers": [resource_identifier()],
            }),
        ),
    ]
}

#[tokio::test]
async fn an_absent_aggregator_is_refused_with_one_sentence_that_withholds_the_name() {
    let emulator = Emulator::new();

    // Rows differ along the NAME the request carried, over the form
    // `ConfigurationAggregatorName` takes (`^[\w\-]+$`, 1..256): a short name, one with
    // the hyphens and underscores the pattern allows, a single character, and one at the
    // length maximum. None of them exists, and the sentence may not vary with any of
    // them.
    let absent_names: [(&str, String); 4] = [
        ("a plain name", "absent-aggregator".to_string()),
        (
            "hyphens and underscores",
            "absent_aggregator-2-x".to_string(),
        ),
        ("a single character", "7".to_string()),
        ("the length maximum", "n".repeat(256)),
    ];

    for (case, name) in &absent_names {
        for (operation, body) in aggregator_requests(name) {
            let (status, code, message) = refusal(&emulator, operation, &body).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "NoSuchConfigurationAggregatorException"),
                "case {case:?} at {operation}: expected HTTP 400 \
                 NoSuchConfigurationAggregatorException, got {status} {code} with message \
                 {message:?}"
            );
            assert_eq!(
                message, NO_SUCH_AGGREGATOR_TEXT,
                "case {case:?} at {operation}: expected the one fixed sentence, got {message:?}"
            );
            assert!(
                !message.contains(name.as_str()),
                "case {case:?} at {operation}: the sentence must not echo the name the \
                 request carried, got {message:?}"
            );
        }
    }

    // The control: the refusal is about the aggregator being ABSENT, not about every
    // request. An aggregator that exists is served.
    let present = "present-aggregator";
    emulator
        .ok(
            "PutConfigurationAggregator",
            &json!({
                "ConfigurationAggregatorName": present,
                "AccountAggregationSources": [
                    { "AccountIds": [SOURCE_ACCOUNT], "AwsRegions": [SOURCE_REGION] },
                ],
            }),
        )
        .await;
    for operation in [
        "DescribeConfigurationAggregatorSourcesStatus",
        "GetAggregateConfigRuleComplianceSummary",
    ] {
        let (status, _) = emulator
            .call(
                operation,
                &json!({ "ConfigurationAggregatorName": present }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case \"an aggregator that exists\" at {operation}: expected HTTP 200, got {status}"
        );
    }
}

/// Every paged read whose modelled refusal of a token it did not issue is
/// `InvalidNextTokenException`, with a request that carries `token` and asks for nothing
/// else. The member the token arrives under is the one the model declares for that
/// operation — `NextToken` for most of them, `nextToken` for the counts read — which is
/// part of what the one sentence has to survive.
fn paged_requests(token: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "SelectResourceConfig",
            json!({ "Expression": "SELECT resourceId", "NextToken": token }),
        ),
        ("DescribeConfigRules", json!({ "NextToken": token })),
        (
            "DescribeConfigRuleEvaluationStatus",
            json!({ "NextToken": token }),
        ),
        (
            "DescribeComplianceByConfigRule",
            json!({ "NextToken": token }),
        ),
        (
            "DescribeConfigurationAggregators",
            json!({ "NextToken": token }),
        ),
        (
            "DescribeRetentionConfigurations",
            json!({ "NextToken": token }),
        ),
        (
            "DescribeAggregationAuthorizations",
            json!({ "NextToken": token }),
        ),
        ("ListStoredQueries", json!({ "NextToken": token })),
        ("ListResourceEvaluations", json!({ "NextToken": token })),
        (
            "GetDiscoveredResourceCounts",
            json!({ "nextToken": token }),
        ),
    ]
}

#[tokio::test]
async fn a_next_token_the_service_did_not_issue_is_refused_with_one_sentence() {
    let emulator = Emulator::new();

    // Rows differ along the FORM of the value a client sent as a page token: the token
    // is opaque, so what makes each of these no page is only that this service never
    // issued it — a hyphenated word, punctuation, a value carrying whitespace, and a
    // hexadecimal-looking id.
    let forged: [(&str, &str); 4] = [
        ("a hyphenated word", "forged-token-abc"),
        ("punctuation", "%%%%"),
        ("a value carrying whitespace", "page two"),
        ("an id-looking value", "0123456789-abcdef"),
    ];

    for (case, token) in forged {
        for (operation, body) in paged_requests(token) {
            let (status, code, message) = refusal(&emulator, operation, &body).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidNextTokenException"),
                "case {case:?} at {operation}: expected HTTP 400 InvalidNextTokenException, \
                 got {status} {code} with message {message:?}"
            );
            assert_eq!(
                message, INVALID_NEXT_TOKEN_TEXT,
                "case {case:?} at {operation}: expected the one fixed sentence, got {message:?}"
            );
            assert!(
                !message.contains(token),
                "case {case:?} at {operation}: the sentence must not echo the token, got \
                 {message:?}"
            );
        }
    }

    // A value that IS of the opaque form this service issues but names no page of this
    // query is the same refusal: the token is read, not matched.
    for (case, key) in [
        ("a key that is no count of answered results", "not-a-number"),
        ("an empty key", ""),
    ] {
        let token = state::encode_page_token(key);
        let (status, code, message) = refusal(
            &emulator,
            "SelectResourceConfig",
            &json!({ "Expression": "SELECT resourceId", "NextToken": token }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidNextTokenException"),
            "case {case:?}: expected HTTP 400 InvalidNextTokenException, got {status} {code} \
             with message {message:?}"
        );
        assert_eq!(
            message, INVALID_NEXT_TOKEN_TEXT,
            "case {case:?}: expected the one fixed sentence, got {message:?}"
        );
    }

    // The controls: a request that asks for no page at all is served, and the OTHER
    // refusal of the same operation's paging keeps its own class and message — a `Limit`
    // outside the modelled range is a request-validation failure, not a token one.
    let served = emulator
        .ok(
            "SelectResourceConfig",
            &json!({ "Expression": "SELECT resourceId", "Limit": 100 }),
        )
        .await;
    assert!(
        served.get("NextToken").is_none(),
        "case \"a first page over an empty inventory\": nothing to resume from, got {served}"
    );
    let (status, code, message) = refusal(
        &emulator,
        "SelectResourceConfig",
        &json!({ "Expression": "SELECT resourceId", "Limit": 101 }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case \"a Limit past the modelled maximum\": expected HTTP 400 ValidationException, \
         got {status} {code} with message {message:?}"
    );
    assert_ne!(
        message, INVALID_NEXT_TOKEN_TEXT,
        "case \"a Limit past the modelled maximum\": a limit failure is not a token failure, \
         got {message:?}"
    );
}

/// The sentence Config refuses an aggregating query with when the fields it selects are
/// not covered by what it groups by, naming those `fields` as the request wrote them.
fn ungrouped_text(fields: &str) -> String {
    format!(
        "when a GROUP BY clause or Aggregate Function (AVG|COUNT|MAX|MIN|SUM) is present, \
         selected fields ({fields}) must appear in the GROUP BY field list"
    )
}

/// The one sentence a query this engine READ but will not run is refused with — every
/// rule of the query language except the GROUP BY coverage one, whose own sentence names
/// the fields that broke it.
const QUERY_IS_INVALID_TEXT: &str = "Query is invalid.";

#[tokio::test]
async fn a_selected_item_that_names_no_field_is_refused_naming_the_item() {
    let emulator = Emulator::new();

    // The language has five functions (`AVG`, `COUNT`, `MAX`, `MIN`, `SUM`), so anything
    // else written as a call names no function and is read as a FIELD NAME instead —
    // one whose brackets a field name cannot carry. Rows differ along the name written,
    // where it sits in the projection, and whether the query also groups: the sentence
    // quotes the one item that is not a field, as the query wrote it.
    let cases: [(&str, &str, &str); 5] = [
        (
            "a function the language does not have",
            "SELECT FROBNICATE(resourceId)",
            "FROBNICATE(resourceId)",
        ),
        (
            "a SQL function the language does not have",
            "SELECT UPPER(resourceId)",
            "UPPER(resourceId)",
        ),
        (
            "the second of two selected items",
            "SELECT resourceId, LOWER(resourceType)",
            "LOWER(resourceType)",
        ),
        (
            "a call over a nested property",
            "SELECT TOTAL(configuration.size)",
            "TOTAL(configuration.size)",
        ),
        (
            "a call beside a GROUP BY",
            "SELECT resourceType, LENGTH(resourceId) GROUP BY resourceType",
            "LENGTH(resourceId)",
        ),
    ];

    for (case, expression, item) in cases {
        let (status, code, message) = refusal(
            &emulator,
            "SelectResourceConfig",
            &json!({ "Expression": expression }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidExpressionException"),
            "case {case:?}: expected HTTP 400 InvalidExpressionException, got {status} {code} \
             with message {message:?}"
        );
        assert_eq!(
            message,
            format!("field name {item} contains invalid characters"),
            "case {case:?}: expected the sentence naming {item:?}, got {message:?}"
        );
    }

    // The control: each of the five functions the language DOES have is read as that
    // function, not as a field name.
    for (case, expression) in [
        ("COUNT", "SELECT COUNT(*)"),
        ("MIN", "SELECT MIN(resourceId)"),
        ("MAX", "SELECT MAX(resourceId)"),
        ("SUM", "SELECT SUM(configuration.size)"),
        ("AVG", "SELECT AVG(configuration.size)"),
        ("a function named in lower case", "SELECT count(*)"),
    ] {
        let (status, parsed) = emulator
            .call("SelectResourceConfig", &json!({ "Expression": expression }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a function the language has is no field name, got {status} with \
             body {parsed}"
        );
    }
}

#[tokio::test]
async fn an_arithmetic_aggregate_over_a_field_that_carries_no_numbers_is_refused() {
    let emulator = Emulator::new();

    // Rows differ along the function (AVG and SUM both compute over numbers) and along
    // the member it is applied to: members of `ConfigurationItem` the model declares as
    // a string, an enum, a timestamp and a list — none of which is a number
    // (`smithy_query(query="shape", shape="ConfigurationItem")`).
    let cases: [(&str, &str); 6] = [
        ("a resource id", "SELECT SUM(resourceId)"),
        ("an ARN", "SELECT AVG(arn)"),
        ("a Region", "SELECT SUM(awsRegion)"),
        ("an enum member", "SELECT AVG(configurationItemStatus)"),
        ("a timestamp member", "SELECT SUM(resourceCreationTime)"),
        (
            "a list member beside a grouped property",
            "SELECT resourceType, SUM(relatedEvents) GROUP BY resourceType",
        ),
    ];

    for (case, expression) in cases {
        let (status, code, message) = refusal(
            &emulator,
            "SelectResourceConfig",
            &json!({ "Expression": expression }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidExpressionException"),
            "case {case:?}: expected HTTP 400 InvalidExpressionException, got {status} {code} \
             with message {message:?}"
        );
        assert_eq!(
            message, QUERY_IS_INVALID_TEXT,
            "case {case:?}: expected the one sentence a query the engine will not run is \
             refused with, got {message:?}"
        );
    }

    // The controls: what the rule does NOT reach. COUNT counts values of any type and
    // MIN and MAX order them, and a property INSIDE the `configuration` document the
    // resource type defines is one no schema types here — which is where a Config query
    // finds numbers to sum.
    for (case, expression) in [
        ("COUNT over the same member", "SELECT COUNT(resourceId)"),
        ("MIN over the same member", "SELECT MIN(resourceId)"),
        ("MAX over the same member", "SELECT MAX(resourceId)"),
        (
            "SUM over a member of the configuration document",
            "SELECT SUM(configuration.size)",
        ),
        (
            "AVG over a nested member of the configuration document",
            "SELECT AVG(configuration.capacity.units)",
        ),
        ("SUM over a tag", "SELECT SUM(tags.weight)"),
    ] {
        let (status, parsed) = emulator
            .call("SelectResourceConfig", &json!({ "Expression": expression }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the rule reaches only an arithmetic aggregate over a member the \
             item's own schema declares, got {status} with body {parsed}"
        );
    }
}

#[tokio::test]
async fn an_aggregating_query_names_the_selected_fields_it_does_not_group_by() {
    let emulator = Emulator::new();

    // Rows differ along what the rule generalises over: what makes the query an
    // aggregating one (a GROUP BY clause, an aggregate function, or both), how many
    // selected fields escape the GROUP BY list, how many properties are grouped, and
    // whether the escaping property is a nested one. The expected text lists exactly the
    // escaping fields, in the order the query selected them.
    let cases: [(&str, &str, &str); 8] = [
        (
            "a GROUP BY that does not cover the selected field",
            "SELECT resourceId GROUP BY resourceType",
            "resourceId",
        ),
        (
            "two selected fields, neither grouped",
            "SELECT resourceId, resourceName GROUP BY resourceType",
            "resourceId, resourceName",
        ),
        (
            "one of the selected fields grouped",
            "SELECT resourceType, resourceId GROUP BY resourceType",
            "resourceId",
        ),
        (
            "an aggregate function and no GROUP BY at all",
            "SELECT resourceId, COUNT(*)",
            "resourceId",
        ),
        (
            "an aggregate function beside a GROUP BY the projection escapes",
            "SELECT resourceId, COUNT(*) GROUP BY resourceType",
            "resourceId",
        ),
        (
            "a nested property",
            "SELECT configuration.state GROUP BY resourceType",
            "configuration.state",
        ),
        (
            "two grouped properties and one field outside both",
            "SELECT resourceType, awsRegion, resourceId GROUP BY resourceType, awsRegion",
            "resourceId",
        ),
        (
            "an aggregate over a property beside an ungrouped one",
            "SELECT resourceId, AVG(configuration.size)",
            "resourceId",
        ),
    ];

    for (case, expression, fields) in cases {
        let (status, code, message) = refusal(
            &emulator,
            "SelectResourceConfig",
            &json!({ "Expression": expression }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidExpressionException"),
            "case {case:?}: expected HTTP 400 InvalidExpressionException, got {status} {code} \
             with message {message:?}"
        );
        assert_eq!(
            message,
            ungrouped_text(fields),
            "case {case:?}: expected the GROUP BY coverage sentence naming ({fields}), got \
             {message:?}"
        );
    }

    // The controls. A query whose projection IS covered is served and echoes it in
    // order, an unknown property name is still accepted (the inventory is schemaless),
    // and a command that cannot be read as a Config query at all keeps the OTHER
    // sentence of this error code rather than the rule's.
    for (case, expression, select_fields) in [
        (
            "an aggregate beside the property it groups by",
            "SELECT resourceType, COUNT(*) GROUP BY resourceType",
            vec!["resourceType", "COUNT(*)"],
        ),
        (
            "a property the inventory does not carry",
            "SELECT resourceId, thisPropertyDoesNotExist",
            vec!["resourceId", "thisPropertyDoesNotExist"],
        ),
    ] {
        let body = emulator
            .ok("SelectResourceConfig", &json!({ "Expression": expression }))
            .await;
        let answered: Vec<String> = body["QueryInfo"]["SelectFields"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected QueryInfo.SelectFields, got {body}"))
            .iter()
            .map(|field| {
                field["Name"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: a FieldInfo carries a Name, got {body}"))
                    .to_string()
            })
            .collect();
        let expected: Vec<String> = select_fields.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            answered, expected,
            "case {case:?}: expected the projection {select_fields:?} echoed in order, got \
             {answered:?}"
        );
    }

    for (case, expression, expected) in [
        (
            "a command that is not a SELECT at all",
            "SELECT WHERE",
            UNREADABLE_QUERY_PREFIX,
        ),
    ] {
        let (status, code, message) = refusal(
            &emulator,
            "SelectResourceConfig",
            &json!({ "Expression": expression }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidExpressionException"),
            "case {case:?}: expected HTTP 400 InvalidExpressionException, got {status} {code} \
             with message {message:?}"
        );
        assert!(
            message.starts_with(expected),
            "case {case:?}: a command that cannot be read keeps its own sentence, got \
             {message:?}"
        );
    }

    // A query that IS read and breaks any OTHER rule of the language carries the single
    // sentence for that, not the GROUP BY one: `*` names no group, and an ORDER BY of an
    // aggregating query can only order by what such a result carries.
    for (case, expression) in [
        ("every member beside a GROUP BY", "SELECT * GROUP BY resourceType"),
        (
            "an ORDER BY the aggregating result does not carry",
            "SELECT resourceType, COUNT(*) GROUP BY resourceType ORDER BY resourceId",
        ),
    ] {
        let (status, code, message) = refusal(
            &emulator,
            "SelectResourceConfig",
            &json!({ "Expression": expression }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidExpressionException"),
            "case {case:?}: expected HTTP 400 InvalidExpressionException, got {status} {code} \
             with message {message:?}"
        );
        assert_eq!(
            message, QUERY_IS_INVALID_TEXT,
            "case {case:?}: expected the one sentence a query the engine will not run is \
             refused with, got {message:?}"
        );
    }
}
