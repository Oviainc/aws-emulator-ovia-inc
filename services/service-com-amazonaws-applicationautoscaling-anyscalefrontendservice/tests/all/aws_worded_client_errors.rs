// SPDX-License-Identifier: CC0-1.0

//! The WORDING of the client errors this service answers with, over the classes of request
//! that reach each one.
//!
//! One rule, in one sentence: a refused request is answered with the SERVICE's own sentence
//! for the rule it broke — the fixed text as the service words it, with only the parts that
//! are about the request (an identifier, a name, a position, a token, a member path) built
//! from the request and from what the store holds — and never with the documentation prose
//! that explains the member to a reader of the API reference.
//!
//! Each suite below asserts that rule over a TABLE spanning the class it covers:
//!
//! * a scheduled action its target does not hold is reported with the TARGET's own
//!   identifier — the one its `ScalableTargetARN` carries — beside the namespace and the
//!   action's name, while an identity nothing registered a target for is reported as the
//!   missing target instead;
//! * a step-scaling configuration with no step adjustment at all, and one step adjustment
//!   with neither bound, each carry one fixed sentence whatever the dimension, the adjustment
//!   type or the other adjustments;
//! * a target-tracking configuration naming both metric specifications is refused with the
//!   sentence that names the two MODELLED members, so a caller is told which to drop;
//! * a `cron(…)` body the parser refuses states where in that BODY it stopped, counted from
//!   zero, and the token it stopped on as the parser reads the expression;
//! * a value outside a `@pattern` the model declares quotes the expression the service
//!   PUBLISHES — the declared one without the whole-value anchors the model wraps it in —
//!   for every member of this service that declares one.

use serde_json::{json, Value};

use super::emulator::{error_code, error_message, scalable_target_id, Emulator};

// ---------------------------------------------------------------------------
// A scheduled action its scalable target does not hold
// ---------------------------------------------------------------------------

/// The scalable dimensions the deletion table runs over: values of the model's own
/// `ScalableDimension` enum
/// (`smithy_query(query="shape",shape="com.amazonaws.applicationautoscaling#ScalableDimension")`),
/// one per family of resource-id form the harness names a resource for — a path with two
/// segments, a table, a table/index pair, a colon-qualified name — so the rows differ in the
/// namespace and in the shape of the identity the request addresses, neither of which the
/// sentence under test states.
const DELETION_DIMENSIONS: &[(&str, &str)] = &[
    ("an ECS service's desired count", "ecs:service:DesiredCount"),
    (
        "a DynamoDB table's read capacity",
        "dynamodb:table:ReadCapacityUnits",
    ),
    (
        "a DynamoDB index's write capacity",
        "dynamodb:index:WriteCapacityUnits",
    ),
    (
        "a Lambda alias's provisioned concurrency",
        "lambda:function:ProvisionedConcurrency",
    ),
    (
        "a SageMaker variant's instance count",
        "sagemaker:variant:DesiredInstanceCount",
    ),
];

/// How a request can name a scheduled action its target does not hold, beyond never having
/// stored one: each row is the stored name off by one detail of itself.
const MISSING_NAMES: &[(&str, &str)] = &[
    ("a name nothing stored", "weekly"),
    ("the stored name in another letter case", "Nightly"),
    ("the stored name with a trailing space", "nightly "),
    ("the stored name with another separator", "nightly-2"),
];

/// Store a scheduled action called `name` on `target`.
async fn put_action(emulator: &Emulator, target: &Value, name: &str, case: &str) {
    let mut request = target.clone();
    request["ScheduledActionName"] = json!(name);
    request["Schedule"] = json!("rate(5 minutes)");
    let (status, body) = emulator.call("PutScheduledAction", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected PutScheduledAction for {name:?} to answer 200, got {status} \
         {body:?}"
    );
}

#[tokio::test]
async fn a_scheduled_action_its_target_does_not_hold_is_reported_by_the_targets_own_id() {
    let emulator = Emulator::empty();

    for (case, dimension) in DELETION_DIMENSIONS {
        let target = emulator.given_target(dimension, 1, 10).await;
        let arn = emulator.target_arn(&target).await;
        let target_id = scalable_target_id(&arn);
        let namespace = target["ServiceNamespace"].as_str().unwrap_or_default();
        let resource_id = target["ResourceId"].as_str().unwrap_or_default();

        // The identifier is the target's own, not a rendering of the triple the request
        // addresses it with: it is the tail of the ARN the registration handed back, in the
        // grouping every identifier of this service is written in.
        assert!(
            arn.ends_with(&target_id.replace('-', "")),
            "case {case:?}: expected the id {target_id:?} to be the digits {arn:?} ends with"
        );
        let groups: Vec<usize> = target_id.split('-').map(str::len).collect();
        assert_eq!(
            groups,
            vec![8, 4, 4, 4, 12],
            "case {case:?}: expected the id {target_id:?} grouped 8-4-4-4-12"
        );

        // The target holds ONE action, so every refusal below is about the name asked for
        // rather than about a target that holds nothing at all.
        put_action(&emulator, &target, "nightly", case).await;

        for (which, name) in MISSING_NAMES {
            let case = format!("{case}: {which}");
            let mut request = target.clone();
            request["ScheduledActionName"] = json!(name);
            let (status, body) = emulator.call("DeleteScheduledAction", request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ObjectNotFoundException",
                "case {case:?}: expected an ObjectNotFoundException, got {body:?}"
            );
            let message = error_message(&body);
            let expected = format!(
                "No scheduled action found for service namespace: {namespace}, scalable target \
                 ID: {target_id}, scheduled action name: {name}"
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            assert!(
                !message.contains(resource_id) && !message.contains(dimension),
                "case {case:?}: expected the target named by its own id rather than by the \
                 triple the request addressed it with, got {message:?}"
            );
        }

        // And the request that DOES name the stored action succeeds, so the rows above are
        // about the name rather than about a store that refuses every delete.
        let mut request = target.clone();
        request["ScheduledActionName"] = json!("nightly");
        let (status, body) = emulator.call("DeleteScheduledAction", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the stored name to be deleted, got {status} {body:?}"
        );
    }
}

#[tokio::test]
async fn a_delete_addressed_at_an_identity_no_target_is_registered_for_reports_the_target() {
    let emulator = Emulator::empty();
    // One registered target, so the rows below differ from a registered identity in exactly
    // one member each.
    let registered = emulator
        .given_target("dynamodb:table:ReadCapacityUnits", 1, 10)
        .await;
    let resource_id = registered["ResourceId"].as_str().unwrap_or_default();

    let rows: Vec<(&str, Value)> = vec![
        (
            "nothing registered at all",
            json!({"ServiceNamespace": "dynamodb", "ResourceId": "table/never-registered",
                   "ScalableDimension": "dynamodb:table:ReadCapacityUnits"}),
        ),
        (
            "another dimension of a registered resource",
            json!({"ServiceNamespace": "dynamodb", "ResourceId": resource_id,
                   "ScalableDimension": "dynamodb:table:WriteCapacityUnits"}),
        ),
        (
            "another namespace's resource entirely",
            json!({"ServiceNamespace": "ecs", "ResourceId": "service/web/orders",
                   "ScalableDimension": "ecs:service:DesiredCount"}),
        ),
    ];

    for (case, identity) in &rows {
        let mut request = identity.clone();
        request["ScheduledActionName"] = json!("nightly");
        let (status, body) = emulator.call("DeleteScheduledAction", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ObjectNotFoundException",
            "case {case:?}: expected an ObjectNotFoundException, got {body:?}"
        );
        let expected = format!(
            "No scalable target registered for service namespace: {}, resource ID: {}, scalable \
             dimension: {}",
            identity["ServiceNamespace"].as_str().unwrap_or_default(),
            identity["ResourceId"].as_str().unwrap_or_default(),
            identity["ScalableDimension"].as_str().unwrap_or_default(),
        );
        let message = error_message(&body);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A scaling-policy configuration that cannot be evaluated
// ---------------------------------------------------------------------------

/// One scaling-policy configuration that cannot be evaluated, and the sentence it is refused
/// with.
struct PolicyRow {
    case: &'static str,
    /// The dimension the policy is put on, so the rows span namespaces as well as
    /// configurations.
    dimension: &'static str,
    policy_type: &'static str,
    configuration: fn() -> Value,
    expected: &'static str,
}

/// A step-scaling configuration carrying no step adjustment at all: nothing selects an
/// adjustment for a breach.
const AT_LEAST_ONE_STEP_ADJUSTMENT: &str = "There must be at least one step adjustment";

/// One step adjustment with neither bound, whose range is every metric-to-threshold
/// difference there is.
const STEP_ADJUSTMENT_WITHOUT_A_BOUND: &str =
    "Both lower and upper bounds of a step adjustment cannot be left unspecified";

/// A target-tracking configuration naming both metric specifications, which is two metrics to
/// track. The sentence names the two MODELLED members, so a caller is told which to drop.
const ONE_METRIC_SPECIFICATION: &str = "Only one of PredefinedMetricSpecification or \
                                        CustomizedMetricSpecification can be set for target \
                                        tracking scaling.";

/// The catalog. The rows differ along the scalable dimension, the adjustment type, the number
/// of step adjustments, whether the member is absent or empty, and which predefined metric the
/// configuration names — none of which the sentences above state.
const POLICY_ROWS: &[PolicyRow] = &[
    PolicyRow {
        case: "a step adjustment with neither bound",
        dimension: "ecs:service:DesiredCount",
        policy_type: "StepScaling",
        configuration: || {
            json!({"AdjustmentType": "ChangeInCapacity",
                   "StepAdjustments": [{"ScalingAdjustment": 1}]})
        },
        expected: STEP_ADJUSTMENT_WITHOUT_A_BOUND,
    },
    PolicyRow {
        case: "a second step adjustment with neither bound",
        dimension: "dynamodb:table:ReadCapacityUnits",
        policy_type: "StepScaling",
        configuration: || {
            json!({"AdjustmentType": "PercentChangeInCapacity",
                   "StepAdjustments": [
                       {"MetricIntervalLowerBound": 0.0, "MetricIntervalUpperBound": 10.0,
                        "ScalingAdjustment": 1},
                       {"ScalingAdjustment": 2}]})
        },
        expected: STEP_ADJUSTMENT_WITHOUT_A_BOUND,
    },
    PolicyRow {
        case: "a step adjustment with neither bound on an exact capacity",
        dimension: "lambda:function:ProvisionedConcurrency",
        policy_type: "StepScaling",
        configuration: || {
            json!({"AdjustmentType": "ExactCapacity",
                   "StepAdjustments": [{"ScalingAdjustment": 4}]})
        },
        expected: STEP_ADJUSTMENT_WITHOUT_A_BOUND,
    },
    PolicyRow {
        case: "an empty list of step adjustments",
        dimension: "ecs:service:DesiredCount",
        policy_type: "StepScaling",
        configuration: || json!({"AdjustmentType": "ChangeInCapacity", "StepAdjustments": []}),
        expected: AT_LEAST_ONE_STEP_ADJUSTMENT,
    },
    PolicyRow {
        case: "no step adjustments member at all",
        dimension: "dynamodb:index:ReadCapacityUnits",
        policy_type: "StepScaling",
        configuration: || json!({"AdjustmentType": "ChangeInCapacity"}),
        expected: AT_LEAST_ONE_STEP_ADJUSTMENT,
    },
    PolicyRow {
        case: "a configuration that names only how the metric is aggregated",
        dimension: "sagemaker:variant:DesiredInstanceCount",
        policy_type: "StepScaling",
        configuration: || json!({"MetricAggregationType": "Average"}),
        expected: AT_LEAST_ONE_STEP_ADJUSTMENT,
    },
    PolicyRow {
        case: "both metric specifications on an ECS service",
        dimension: "ecs:service:DesiredCount",
        policy_type: "TargetTrackingScaling",
        configuration: || {
            json!({"TargetValue": 40.0,
                   "PredefinedMetricSpecification": {
                       "PredefinedMetricType": "ECSServiceAverageCPUUtilization"},
                   "CustomizedMetricSpecification": {
                       "MetricName": "Backlog", "Namespace": "Mine", "Statistic": "Average"}})
        },
        expected: ONE_METRIC_SPECIFICATION,
    },
    PolicyRow {
        case: "both metric specifications on a DynamoDB table",
        dimension: "dynamodb:table:WriteCapacityUnits",
        policy_type: "TargetTrackingScaling",
        configuration: || {
            json!({"TargetValue": 70.0,
                   "PredefinedMetricSpecification": {
                       "PredefinedMetricType": "DynamoDBWriteCapacityUtilization"},
                   "CustomizedMetricSpecification": {
                       "MetricName": "Queue", "Namespace": "Mine", "Statistic": "Sum"}})
        },
        expected: ONE_METRIC_SPECIFICATION,
    },
    PolicyRow {
        case: "both metric specifications, the predefined one needing a resource label",
        dimension: "ec2:spot-fleet-request:TargetCapacity",
        policy_type: "TargetTrackingScaling",
        configuration: || {
            json!({"TargetValue": 1000.0,
                   "PredefinedMetricSpecification": {
                       "PredefinedMetricType": "ALBRequestCountPerTarget"},
                   "CustomizedMetricSpecification": {
                       "MetricName": "Backlog", "Namespace": "Mine", "Statistic": "Average"}})
        },
        expected: ONE_METRIC_SPECIFICATION,
    },
];

#[tokio::test]
async fn a_policy_configuration_that_cannot_be_evaluated_carries_the_services_sentence() {
    let emulator = Emulator::empty();

    for row in POLICY_ROWS {
        let case = row.case;
        let target = emulator.given_target(row.dimension, 1, 20).await;
        let member = if row.policy_type == "StepScaling" {
            "StepScalingPolicyConfiguration"
        } else {
            "TargetTrackingScalingPolicyConfiguration"
        };
        let mut request = target.clone();
        request["PolicyName"] = json!("tracker");
        request["PolicyType"] = json!(row.policy_type);
        request[member] = (row.configuration)();

        let (status, body) = emulator.call("PutScalingPolicy", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, row.expected,
            "case {case:?}: expected {:?}, got {message:?}",
            row.expected
        );

        // Nothing was stored: the refusal is of the whole put.
        let (status, described) = emulator
            .call("DescribeScalingPolicies", target.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeScalingPolicies to answer 200, got {status} \
             {described:?}"
        );
        let policies = described["ScalingPolicies"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            policies.is_empty(),
            "case {case:?}: expected the refused put to store nothing, got {policies:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A cron body the parser refuses
// ---------------------------------------------------------------------------

/// One `cron(…)` schedule the parser refuses: the offset into the cron BODY it stopped at,
/// counted from zero, and the token it stopped on as the parser reads the expression.
struct CronRow {
    case: &'static str,
    schedule: &'static str,
    position: usize,
    token: &'static str,
}

/// The catalog: a token the parser cannot read in each of the six fields the format declares,
/// written in either letter case, so what is pinned is the position and the token rather than
/// one expression.
const CRON_ROWS: &[CronRow] = &[
    CronRow {
        case: "a minutes field that is not a minute",
        schedule: "cron(not a cron)",
        position: 0,
        token: "NOT",
    },
    CronRow {
        case: "a named minutes field",
        schedule: "cron(sixty 10 * * ? *)",
        position: 0,
        token: "SIXTY",
    },
    CronRow {
        case: "an hours field out of range",
        schedule: "cron(0 99 * * ? *)",
        position: 2,
        token: "99",
    },
    CronRow {
        case: "a day of month that is not a day",
        schedule: "cron(0 10 someday * ? *)",
        position: 5,
        token: "SOMEDAY",
    },
    CronRow {
        case: "a month that is not a month",
        schedule: "cron(0 10 * SMARCH ? *)",
        position: 7,
        token: "SMARCH",
    },
    CronRow {
        case: "a day of week that is not a day, in lower case",
        schedule: "cron(0 10 ? * funday *)",
        position: 9,
        token: "FUNDAY",
    },
    CronRow {
        case: "a year that is not a year",
        schedule: "cron(0 10 * * ? someday)",
        position: 11,
        token: "SOMEDAY",
    },
    CronRow {
        case: "seven fields, so the year field reads a day-of-week token",
        schedule: "cron(0 0 10 * * ? *)",
        position: 11,
        token: "?",
    },
];

#[tokio::test]
async fn a_cron_body_the_parser_refuses_states_where_in_the_body_it_stopped() {
    let emulator = Emulator::empty();
    let target = emulator.given_target("ecs:service:DesiredCount", 1, 5).await;

    for row in CRON_ROWS {
        let case = row.case;
        let mut request = target.clone();
        request["ScheduledActionName"] = json!("nightly");
        request["Schedule"] = json!(row.schedule);
        let (status, body) = emulator.call("PutScheduledAction", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
        let expected = format!(
            "Invalid schedule expression. Details: Invalid CRON expression: error at position \
             {}, details: Illegal characters for this position: '{}'",
            row.position, row.token
        );
        let message = error_message(&body);
        assert_eq!(
            message, expected,
            "case {case:?} ({:?}): expected {expected:?}, got {message:?}",
            row.schedule
        );
    }
}

/// A cron body carrying fewer fields than the format declares: the parse stops at the end of
/// the body, which is where the field it is missing would have been.
const CRON_FIELD_COUNT_ROWS: &[(&str, &str, usize, &str)] = &[
    ("five fields, so no year", "cron(0 10 * * ?)", 10, "Year"),
    ("no fields at all", "cron()", 0, "Minutes"),
    ("only the minutes", "cron(0)", 1, "Hours"),
];

#[tokio::test]
async fn a_cron_body_of_too_few_fields_stops_where_the_missing_field_would_be() {
    let emulator = Emulator::empty();
    let target = emulator.given_target("ecs:service:DesiredCount", 1, 5).await;

    for (case, schedule, position, missing) in CRON_FIELD_COUNT_ROWS {
        let mut request = target.clone();
        request["ScheduledActionName"] = json!("nightly");
        request["Schedule"] = json!(schedule);
        let (status, body) = emulator.call("PutScheduledAction", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        let message = error_message(&body);
        let opening = format!(
            "Invalid schedule expression. Details: Invalid CRON expression: error at position \
             {position}, details: Unexpected end of expression, the {missing} field is missing,"
        );
        assert!(
            message.starts_with(&opening),
            "case {case:?}: expected the message to open with {opening:?}, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A value outside a modelled `@pattern`
// ---------------------------------------------------------------------------

/// The `@pattern` the model declares on `ScheduledActionName`, as the MODEL declares it
/// (`smithy_query(query="shape",shape="com.amazonaws.applicationautoscaling#ScheduledActionName")`).
const SCHEDULED_ACTION_NAME_PATTERN: &str =
    r"^(?!((^[ ]+.*)|(.*([\u0000-\u001f]|[\u007f-\u009f]|[:/|])+.*)|(.*[ ]+$))).+$";

/// The `@pattern` the model declares on `AmazonResourceName`, the shape every tag operation's
/// `ResourceARN` targets, same source
/// (`smithy_query(query="shape",shape="com.amazonaws.applicationautoscaling#AmazonResourceName")`).
const AMAZON_RESOURCE_NAME_PATTERN: &str =
    r"^arn:.+:application-autoscaling:.+:[0-9]+:scalable-target\/[a-zA-Z0-9-]+$";

/// The expression a refusal quotes for a declared `@pattern`: the declared one without the
/// `^` and `$` the model wraps it in to turn Smithy's find into a whole-value match. Computed
/// from the declared expression rather than spelled out, so the row states the RULE.
fn published(declared: &str) -> String {
    assert!(
        declared.starts_with('^') && declared.ends_with('$'),
        "case {declared:?}: expected the model to declare a whole-value expression"
    );
    declared[1..declared.len() - 1].to_string()
}

/// One request carrying a value outside a `@pattern` its member declares: the operation, the
/// request, the member path a clause names it by and the expression it had to satisfy.
struct PatternRow {
    case: &'static str,
    operation: &'static str,
    path: &'static str,
    declared: &'static str,
    value: &'static str,
    request: fn(&str) -> Value,
}

/// The catalog: both of this service's members whose declared `@pattern` a request can break
/// — the scheduled action's name, checked by the handler because its expression is a negative
/// lookahead, and the tag operations' `ResourceARN`, checked at the protocol boundary — over
/// every operation that carries one and over several ways to break each expression.
const PATTERN_ROWS: &[PatternRow] = &[
    PatternRow {
        case: "a scheduled action name with a slash",
        operation: "PutScheduledAction",
        path: "scheduledActionName",
        declared: SCHEDULED_ACTION_NAME_PATTERN,
        value: "scale-up/weekday",
        request: |_| Value::Null,
    },
    PatternRow {
        case: "a scheduled action name with a colon",
        operation: "PutScheduledAction",
        path: "scheduledActionName",
        declared: SCHEDULED_ACTION_NAME_PATTERN,
        value: "scale:up",
        request: |_| Value::Null,
    },
    PatternRow {
        case: "a scheduled action name with a leading space",
        operation: "PutScheduledAction",
        path: "scheduledActionName",
        declared: SCHEDULED_ACTION_NAME_PATTERN,
        value: " scale-up",
        request: |_| Value::Null,
    },
    PatternRow {
        case: "an ARN that is not an ARN at all",
        operation: "ListTagsForResource",
        path: "resourceARN",
        declared: AMAZON_RESOURCE_NAME_PATTERN,
        value: "not-an-arn",
        request: |value| json!({"ResourceARN": value}),
    },
    PatternRow {
        case: "an ARN of another service",
        operation: "TagResource",
        path: "resourceARN",
        declared: AMAZON_RESOURCE_NAME_PATTERN,
        value: "arn:aws:sqs:us-east-1:000000000000:orders",
        request: |value| json!({"ResourceARN": value, "Tags": {"owner": "orders-team"}}),
    },
    PatternRow {
        case: "a scalable-target ARN whose id carries a character the expression forbids",
        operation: "UntagResource",
        path: "resourceARN",
        declared: AMAZON_RESOURCE_NAME_PATTERN,
        value: "arn:aws:application-autoscaling:us-east-1:000000000000:scalable-target/an_id",
        request: |value| json!({"ResourceARN": value, "TagKeys": ["owner"]}),
    },
];

#[tokio::test]
async fn a_value_outside_a_modelled_pattern_quotes_the_expression_the_service_publishes() {
    let emulator = Emulator::empty();
    let target = emulator
        .given_target("dynamodb:table:ReadCapacityUnits", 1, 10)
        .await;

    for row in PATTERN_ROWS {
        let case = row.case;
        let request = match (row.request)(row.value) {
            // A scheduled action's name is only a name OF a target, so that row's request is
            // the target's identity carrying the offending name.
            Value::Null => {
                let mut request = target.clone();
                request["ScheduledActionName"] = json!(row.value);
                request["Schedule"] = json!("rate(5 minutes)");
                request
            }
            built => built,
        };
        let (status, body) = emulator.call(row.operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
        let expected = format!(
            "1 validation error detected: Value '{}' at '{}' failed to satisfy constraint: \
             Member must satisfy regular expression pattern: {}",
            row.value,
            row.path,
            published(row.declared)
        );
        let message = error_message(&body);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert!(
            !message.contains("pattern: ^") && !message.ends_with('$'),
            "case {case:?}: expected the quoted expression to carry neither anchor, got \
             {message:?}"
        );
    }

    // A value every one of those expressions admits is accepted, so the rows above are about
    // the expression rather than about members nothing satisfies.
    let mut request = target.clone();
    request["ScheduledActionName"] = json!("scale up nightly.2_a");
    request["Schedule"] = json!("rate(5 minutes)");
    let (status, body) = emulator.call("PutScheduledAction", request).await;
    assert_eq!(
        status, 200,
        "case \"a name the expression admits\": expected 200, got {status} {body:?}"
    );
    let arn = emulator.target_arn(&target).await;
    let (status, body) = emulator
        .call("ListTagsForResource", json!({"ResourceARN": arn}))
        .await;
    assert_eq!(
        status, 200,
        "case \"an ARN the expression admits\": expected 200, got {status} {body:?}"
    );
}
