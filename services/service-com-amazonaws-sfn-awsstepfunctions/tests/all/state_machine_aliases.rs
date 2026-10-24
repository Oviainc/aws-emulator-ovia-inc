// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CreateStateMachineAlias` and `DescribeStateMachineAlias`,
//! driven over this service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`, so what is asserted is what a signed AWS SDK request
//! observes.
//!
//! The rules under test:
//!
//! 1. a create stores ONE alias record under the alias ARN — its state machine's ARN
//!    qualified with the alias name — and a later describe answers every member from
//!    that record: the name, the description, and the routing configuration entry for
//!    entry, in the order it was given;
//! 2. the alias belongs to the state machine its routing configuration's VERSIONS
//!    belong to: the operation takes no `stateMachineArn`, so a configuration whose
//!    entries do not all name versions of one state machine, or whose weights do not
//!    add up to 100, is refused and nothing is stored;
//! 3. "CreateStateMachineAlias is an idempotent API" on the name, the description and
//!    the routing configuration: a repeat of those parameters answers the first
//!    response, and a create naming an alias that exists under other parameters is a
//!    conflict;
//! 4. describing REPORTS: it changes nothing a later caller sees, and an alias does
//!    not outlive the state machine whose versions it points at.
//!
//! The harness (`Emulator`, `string`, `error_code`) is the one
//! `state_machine_lifecycle` builds, and the published versions come from
//! `state_machine_versions`' own `machine_with_versions`; `tests/all.rs` is this
//! binary's crate root, so both are reached through `crate::`.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::{
    MAX_ALIASES_PER_STATE_MACHINE, MAX_ALIAS_NAME_CHARS,
};

use crate::state_machine_lifecycle::{
    error_code, string, Emulator, ACCOUNT, PAST_THE_DELETE_DELAY, REGION,
};
use crate::state_machine_versions::machine_with_versions;

/// One entry of a routing configuration.
fn route(version_arn: &str, weight: i64) -> Value {
    json!({ "stateMachineVersionArn": version_arn, "weight": weight })
}

/// One row of the create table: the case, how many versions its state machine
/// publishes, the `(version number, weight)` entries its routing configuration
/// names, its description and its alias name.
type CreateCase<'a> = (&'a str, u32, Vec<(u32, i64)>, Option<&'a str>, &'a str);

/// One row of the routing-refusal table: the case, the routing entries, and the HTTP
/// status, error code and `reason` the refusal must carry.
type RoutingCase<'a> = (&'a str, Vec<Value>, u16, &'a str, Option<&'a str>);

/// One row of the idempotency table: the case, the alias name, the description and
/// the routing entries — the parameters the idempotency check reads.
type RepeatCase<'a> = (&'a str, &'a str, Option<&'a str>, Vec<Value>);

/// The `CreateStateMachineAlias` request body for `name`, `description` and
/// `routing`.
fn create_request(name: &str, description: Option<&str>, routing: &[Value]) -> Value {
    let mut request = json!({ "name": name, "routingConfiguration": routing });
    if let Some(description) = description {
        request["description"] = json!(description);
    }
    request
}

/// Create an alias that must succeed, and return the response body.
async fn create_alias(
    emulator: &Emulator,
    name: &str,
    description: Option<&str>,
    routing: &[Value],
) -> Value {
    emulator
        .ok(
            "CreateStateMachineAlias",
            &create_request(name, description, routing),
        )
        .await
}

/// `DescribeStateMachineAlias` on `arn`, which must succeed.
async fn describe_alias(emulator: &Emulator, arn: &str) -> Value {
    emulator
        .ok(
            "DescribeStateMachineAlias",
            &json!({ "stateMachineAliasArn": arn }),
        )
        .await
}

/// The `creationDate` of a body, as the wire carries it.
fn date(body: &Value, member: &str) -> f64 {
    body.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("expected a numeric '{member}' in {body:?}"))
}

/// The routing configuration of a body, as `(versionArn, weight)` pairs in the order
/// the response carries them.
fn reported_routing(body: &Value) -> Vec<(String, i64)> {
    body.get("routingConfiguration")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a routingConfiguration list in {body:?}"))
        .iter()
        .map(|item| {
            (
                string(item, "stateMachineVersionArn"),
                item.get("weight")
                    .and_then(Value::as_i64)
                    .unwrap_or_else(|| panic!("expected a weight in {item:?}")),
            )
        })
        .collect()
}

/// The `(versionArn, weight)` pairs a routing configuration request carries.
fn requested_routing(routing: &[Value]) -> Vec<(String, i64)> {
    routing
        .iter()
        .map(|item| {
            (
                string(item, "stateMachineVersionArn"),
                item.get("weight").and_then(Value::as_i64).expect("a weight"),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// What a create stores, and what a describe reports
// ---------------------------------------------------------------------------

/// A create stores the alias under `<stateMachineArn>:<aliasName>` and a describe
/// answers every member from that record — over a table whose rows differ in the
/// NUMBER of versions routed to, the WEIGHTS, the ORDER of the entries, the
/// description and the alias name form, which is what the rule generalises over.
#[tokio::test]
async fn an_alias_reports_the_routing_configuration_it_was_created_with() {
    let emulator = Emulator::new();

    // (case, versions published, (version number, weight) entries, description,
    // alias name). The names span the modelled character rule: letters, a digit
    // inside the name, `_`, `-` and `.`.
    let cases: Vec<CreateCase<'_>> = vec![
        ("one version at the whole weight", 1, vec![(1, 100)], None, "PROD"),
        (
            "two versions split evenly",
            2,
            vec![(1, 50), (2, 50)],
            Some("an even split"),
            "live",
        ),
        (
            "two versions split unevenly",
            3,
            vec![(2, 10), (3, 90)],
            Some("a canary"),
            "canary.v1",
        ),
        (
            "the entries in descending version order",
            2,
            vec![(2, 30), (1, 70)],
            Some("newest first"),
            "weighted-mix",
        ),
        (
            "one version of several, and an empty description",
            4,
            vec![(3, 100)],
            Some(""),
            "A_b-c.D9",
        ),
        (
            "a version that takes none of the traffic",
            2,
            vec![(1, 100), (2, 0)],
            None,
            "staged",
        ),
    ];

    for (case, versions, entries, description, alias_name) in &cases {
        let machine = machine_with_versions(
            &emulator,
            &format!("aliased-{}", alias_name.to_lowercase()),
            *versions,
        )
        .await;
        let routing: Vec<Value> = entries
            .iter()
            .map(|(version, weight)| route(&format!("{machine}:{version}"), *weight))
            .collect();

        let created = create_alias(&emulator, alias_name, *description, &routing).await;
        let alias_arn = string(&created, "stateMachineAliasArn");
        assert_eq!(
            alias_arn,
            format!("{machine}:{alias_name}"),
            "case {case:?}: the alias ARN is its state machine's ARN qualified with the alias \
             name, got {alias_arn:?}"
        );
        let creation_date = date(&created, "creationDate");

        let described = describe_alias(&emulator, &alias_arn).await;
        assert_eq!(
            described.get("stateMachineAliasArn").and_then(Value::as_str),
            Some(alias_arn.as_str()),
            "case {case:?}: the describe must report the alias it was asked about, got \
             {described:?}"
        );
        assert_eq!(
            described.get("name").and_then(Value::as_str),
            Some(*alias_name),
            "case {case:?}: expected the name {alias_name:?}, got {described:?}"
        );
        assert_eq!(
            described.get("description").and_then(Value::as_str),
            *description,
            "case {case:?}: expected the description {description:?}, got {described:?}"
        );
        let reported = reported_routing(&described);
        assert_eq!(
            reported,
            requested_routing(&routing),
            "case {case:?}: expected the routing configuration as it was given, got {reported:?}"
        );
        assert_eq!(
            date(&described, "creationDate"),
            creation_date,
            "case {case:?}: the describe must report the create's own creationDate, got \
             {described:?}"
        );
        assert_eq!(
            date(&described, "updateDate"),
            creation_date,
            "case {case:?}: a newly created alias reports its creation date as its updateDate, \
             got {described:?}"
        );
    }
}

/// Several aliases of one state machine are several records: each describes as the
/// configuration IT was created with, and none of them disturbs another.
#[tokio::test]
async fn aliases_of_one_state_machine_are_separate_records() {
    let emulator = Emulator::new();
    let machine = machine_with_versions(&emulator, "many-aliases", 3).await;

    let plans: Vec<(&str, Vec<(u32, i64)>)> = vec![
        ("PROD", vec![(1, 100)]),
        ("BETA", vec![(2, 100)]),
        ("rollout", vec![(2, 25), (3, 75)]),
    ];
    let mut expected: Vec<(String, Vec<(String, i64)>)> = Vec::new();
    for (name, entries) in &plans {
        let routing: Vec<Value> = entries
            .iter()
            .map(|(version, weight)| route(&format!("{machine}:{version}"), *weight))
            .collect();
        let created = create_alias(&emulator, name, None, &routing).await;
        expected.push((
            string(&created, "stateMachineAliasArn"),
            requested_routing(&routing),
        ));
    }

    for (alias_arn, routing) in &expected {
        let described = describe_alias(&emulator, alias_arn).await;
        let reported = reported_routing(&described);
        assert_eq!(
            &reported, routing,
            "case {alias_arn:?}: each alias must report its own routing configuration, got \
             {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The routing configuration decides what may be created
// ---------------------------------------------------------------------------

/// A routing configuration that cannot be pointed at is refused, and nothing is
/// stored: the ARN of an entry is judged first (`InvalidArn`), then what it names and
/// what the weights add up to (`ValidationException` carrying the modelled
/// `INVALID_ROUTING_CONFIGURATION`), then whether the version exists at all
/// (`ResourceNotFound`).
#[tokio::test]
async fn a_routing_configuration_that_cannot_be_pointed_at_is_refused() {
    let emulator = Emulator::new();
    let machine = machine_with_versions(&emulator, "routing-rule", 2).await;
    let other = machine_with_versions(&emulator, "routing-other", 1).await;
    let absent = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:never-created");

    // (case, routing entries, expected status, expected error code, expected
    // `reason`). The rows vary along WHICH part of the configuration is wrong and
    // along the form of the ARN, not only along its value.
    let cases: Vec<RoutingCase<'_>> = vec![
        (
            "weights that add up to nothing",
            vec![route(&format!("{machine}:1"), 0)],
            400,
            "ValidationException",
            Some("INVALID_ROUTING_CONFIGURATION"),
        ),
        (
            "one version at half the traffic",
            vec![route(&format!("{machine}:1"), 50)],
            400,
            "ValidationException",
            Some("INVALID_ROUTING_CONFIGURATION"),
        ),
        (
            "two versions adding up past 100",
            vec![
                route(&format!("{machine}:1"), 60),
                route(&format!("{machine}:2"), 60),
            ],
            400,
            "ValidationException",
            Some("INVALID_ROUTING_CONFIGURATION"),
        ),
        (
            "two versions adding up short of 100",
            vec![
                route(&format!("{machine}:1"), 50),
                route(&format!("{machine}:2"), 49),
            ],
            400,
            "ValidationException",
            Some("INVALID_ROUTING_CONFIGURATION"),
        ),
        (
            "an unqualified state machine ARN",
            vec![route(&machine, 100)],
            400,
            "ValidationException",
            Some("INVALID_ROUTING_CONFIGURATION"),
        ),
        (
            "an alias ARN rather than a version ARN",
            vec![route(&format!("{machine}:PROD"), 100)],
            400,
            "ValidationException",
            Some("INVALID_ROUTING_CONFIGURATION"),
        ),
        (
            "a map-run labeled ARN",
            vec![route(&format!("{machine}/mapStateLabel:1"), 100)],
            400,
            "ValidationException",
            Some("INVALID_ROUTING_CONFIGURATION"),
        ),
        (
            "versions of two different state machines",
            vec![
                route(&format!("{machine}:1"), 50),
                route(&format!("{other}:1"), 50),
            ],
            400,
            "ValidationException",
            Some("INVALID_ROUTING_CONFIGURATION"),
        ),
        (
            "a version number that was never published",
            vec![route(&format!("{machine}:9"), 100)],
            404,
            "ResourceNotFound",
            None,
        ),
        (
            "a version of a state machine that does not exist",
            vec![route(&format!("{absent}:1"), 100)],
            404,
            "ResourceNotFound",
            None,
        ),
        (
            "text that is not an ARN",
            vec![route("not-an-arn", 100)],
            400,
            "InvalidArn",
            None,
        ),
        (
            "another service's ARN",
            vec![route(
                &format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:routing-rule:1"),
                100,
            )],
            400,
            "InvalidArn",
            None,
        ),
        (
            "an ARN of another region",
            vec![route(
                &format!("arn:aws:states:eu-west-1:{ACCOUNT}:stateMachine:routing-rule:1"),
                100,
            )],
            400,
            "InvalidArn",
            None,
        ),
        (
            "an execution ARN",
            vec![route(
                &format!("arn:aws:states:{REGION}:{ACCOUNT}:execution:routing-rule:run-1"),
                100,
            )],
            400,
            "InvalidArn",
            None,
        ),
    ];

    for (case, routing, status, code, reason) in &cases {
        let (answered, body) = emulator
            .call(
                "CreateStateMachineAlias",
                &create_request("REFUSED", None, routing),
            )
            .await;
        let answered_code = error_code(&body);
        assert_eq!(
            (answered, answered_code.as_str()),
            (*status, *code),
            "case {case:?}: expected {status} {code}, got {answered} {answered_code} with body \
             {body:?}"
        );
        assert_eq!(
            body.get("reason").and_then(Value::as_str),
            *reason,
            "case {case:?}: expected reason {reason:?}, got {body:?}"
        );
    }

    // None of those refusals stored an alias: the name every one of them asked for
    // still resolves to nothing.
    let (status, body) = emulator
        .call(
            "DescribeStateMachineAlias",
            &json!({ "stateMachineAliasArn": format!("{machine}:REFUSED") }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "ResourceNotFound"),
        "a refused create must store nothing, got {status} with body {body:?}"
    );
}

/// An alias name outside the modelled character rule is the modelled `InvalidName`,
/// and the refusal is taken before anything is looked up or stored.
#[tokio::test]
async fn an_alias_name_outside_the_character_rule_is_invalid_name() {
    let emulator = Emulator::new();
    let machine = machine_with_versions(&emulator, "named-aliases", 1).await;
    let routing = vec![route(&format!("{machine}:1"), 100)];

    // Rows vary along WHY the name is outside the modelled
    // `^(?=.*[a-zA-Z_\-\.])[a-zA-Z0-9_\-\.]+$`: nothing but digits (which would read
    // as a version ARN), and characters the set does not admit — whitespace,
    // punctuation, a separator and a non-ASCII letter.
    for name in [
        "1",
        "42",
        "007",
        "has space",
        "PROD!",
        "pro/d",
        "alias:1",
        "caf\u{e9}",
        "+",
    ] {
        let (status, body) = emulator
            .call(
                "CreateStateMachineAlias",
                &create_request(name, None, &routing),
            )
            .await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidName"),
            "case {name:?}: expected 400 InvalidName, got {status} {code} with body {body:?}"
        );
        assert_eq!(
            body.get("message").and_then(Value::as_str),
            Some(format!("Invalid Name: '{name}'").as_str()),
            "case {name:?}: unexpected message in {body:?}"
        );
    }
}

/// An alias cannot be created on a state machine whose deletion has begun: the
/// modelled `StateMachineDeleting` while it is deleting, and — once the clock has
/// removed it — the versions it held are gone, so a create is a resource that cannot
/// be found.
#[tokio::test]
async fn an_alias_of_a_deleting_state_machine_is_refused() {
    let emulator = Emulator::new();
    let machine = machine_with_versions(&emulator, "deleting-aliases", 1).await;
    let routing = vec![route(&format!("{machine}:1"), 100)];

    emulator
        .ok("DeleteStateMachine", &json!({ "stateMachineArn": machine }))
        .await;
    let (status, body) = emulator
        .call(
            "CreateStateMachineAlias",
            &create_request("PROD", None, &routing),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "StateMachineDeleting"),
        "a create on a deleting state machine must be refused, got {status} with body {body:?}"
    );

    std::thread::sleep(PAST_THE_DELETE_DELAY);
    let (status, body) = emulator
        .call(
            "CreateStateMachineAlias",
            &create_request("PROD", None, &routing),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "ResourceNotFound"),
        "once the deletion is due the versions are gone, got {status} with body {body:?}"
    );
}

/// "You can create up to 100 aliases for each state machine": the quota is per state
/// machine, so one at its limit refuses the next with the modelled
/// `ServiceQuotaExceededException` while another still takes one.
#[tokio::test]
async fn the_alias_quota_is_per_state_machine() {
    let emulator = Emulator::new();
    let machine = machine_with_versions(&emulator, "quota-aliases", 1).await;
    let other = machine_with_versions(&emulator, "quota-other", 1).await;
    let routing = vec![route(&format!("{machine}:1"), 100)];

    for number in 0..MAX_ALIASES_PER_STATE_MACHINE {
        create_alias(&emulator, &format!("alias-{number}"), None, &routing).await;
    }
    let (status, body) = emulator
        .call(
            "CreateStateMachineAlias",
            &create_request("one-too-many", None, &routing),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (402, "ServiceQuotaExceededException"),
        "the {MAX_ALIASES_PER_STATE_MACHINE}th alias must be the last one, got {status} with body \
         {body:?}"
    );

    // The quota counts one state machine's aliases, so another's is untouched, and a
    // repeat of an alias that already exists is still the idempotent response rather
    // than a quota refusal.
    let elsewhere = create_alias(
        &emulator,
        "alias-0",
        None,
        &[route(&format!("{other}:1"), 100)],
    )
    .await;
    assert_eq!(
        string(&elsewhere, "stateMachineAliasArn"),
        format!("{other}:alias-0"),
        "another state machine's quota is its own, got {elsewhere:?}"
    );
    let repeat = create_alias(&emulator, "alias-0", None, &routing).await;
    assert_eq!(
        string(&repeat, "stateMachineAliasArn"),
        format!("{machine}:alias-0"),
        "a repeat at the quota is the idempotent response, got {repeat:?}"
    );
}

// ---------------------------------------------------------------------------
// Idempotency
// ---------------------------------------------------------------------------

/// The create is idempotent on the name, the description and the routing
/// configuration: a repeat of those parameters answers the FIRST response and stores
/// no second alias, while a create naming an alias that exists under other parameters
/// is the modelled `ConflictException` and changes nothing.
#[tokio::test]
async fn a_repeat_of_the_same_parameters_is_the_first_response() {
    let emulator = Emulator::new();
    let machine = machine_with_versions(&emulator, "idempotent-aliases", 2).await;
    let first = format!("{machine}:1");
    let second = format!("{machine}:2");

    // Rows vary along the parameters the check reads: one entry or two, a
    // description or none, and an empty description — which is a value, not an
    // absence.
    let cases: Vec<RepeatCase<'_>> = vec![
        ("one entry, no description", "PROD", None, vec![route(&first, 100)]),
        (
            "two entries and a description",
            "split",
            Some("half and half"),
            vec![route(&first, 50), route(&second, 50)],
        ),
        (
            "an empty description",
            "quiet",
            Some(""),
            vec![route(&second, 100)],
        ),
    ];

    for (case, name, description, routing) in &cases {
        let created = create_alias(&emulator, name, *description, routing).await;
        let alias_arn = string(&created, "stateMachineAliasArn");
        let creation_date = date(&created, "creationDate");

        for round in 0..3 {
            let repeat = create_alias(&emulator, name, *description, routing).await;
            assert_eq!(
                string(&repeat, "stateMachineAliasArn"),
                alias_arn,
                "case {case:?} round {round}: a repeat must answer the same ARN, got {repeat:?}"
            );
            assert_eq!(
                date(&repeat, "creationDate"),
                creation_date,
                "case {case:?} round {round}: a repeat must answer the first creationDate, got \
                 {repeat:?}"
            );
        }

        // And the repeats left exactly one record, still carrying what the first
        // create gave it.
        let described = describe_alias(&emulator, &alias_arn).await;
        assert_eq!(
            reported_routing(&described),
            requested_routing(routing),
            "case {case:?}: the repeats must not have changed the record, got {described:?}"
        );
        assert_eq!(
            date(&described, "updateDate"),
            creation_date,
            "case {case:?}: an idempotent repeat is not an update, got {described:?}"
        );
    }

    // A create of the SAME name with other parameters is a conflict, whichever
    // parameter differs.
    let kept = describe_alias(&emulator, &format!("{machine}:PROD")).await;
    let conflicts: Vec<(&str, Option<&str>, Vec<Value>)> = vec![
        ("a description where there was none", Some("new"), vec![route(&first, 100)]),
        ("another version", None, vec![route(&second, 100)]),
        (
            "another weighting",
            None,
            vec![route(&first, 90), route(&second, 10)],
        ),
        (
            "the same versions in the other order",
            None,
            vec![route(&second, 50), route(&first, 50)],
        ),
    ];
    for (case, description, routing) in &conflicts {
        let (status, body) = emulator
            .call(
                "CreateStateMachineAlias",
                &create_request("PROD", *description, routing),
            )
            .await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: expected 409 ConflictException, got {status} {code} with body {body:?}"
        );
    }
    let after = describe_alias(&emulator, &format!("{machine}:PROD")).await;
    assert_eq!(
        after, kept,
        "a refused create must leave the alias exactly as it was, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// The describe reports; it does not mutate
// ---------------------------------------------------------------------------

/// Text that is not an alias ARN of this service in the endpoint's region is the
/// modelled `InvalidArn`, judged BEFORE the lookup; a well-formed alias ARN no record
/// answers for is the modelled `ResourceNotFound`.
#[tokio::test]
async fn an_arn_that_names_no_alias_is_refused() {
    let emulator = Emulator::new();
    let machine = machine_with_versions(&emulator, "arn-aliases", 1).await;
    create_alias(
        &emulator,
        "PROD",
        None,
        &[route(&format!("{machine}:1"), 100)],
    )
    .await;
    let absent = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:never-created");

    // Rows vary along which part of the ARN is not an alias ARN: the text, the
    // service, the region, the resource type, the missing qualifier, and a qualifier
    // that names something other than an alias.
    let invalid: Vec<(&str, String)> = vec![
        ("text that is not an ARN", "not-an-arn".to_string()),
        (
            "another service's ARN",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:arn-aliases:PROD"),
        ),
        (
            "an ARN of another region",
            format!("arn:aws:states:eu-west-1:{ACCOUNT}:stateMachine:arn-aliases:PROD"),
        ),
        (
            "an activity ARN",
            format!("arn:aws:states:{REGION}:{ACCOUNT}:activity:arn-aliases:PROD"),
        ),
        ("an unqualified state machine ARN", machine.clone()),
        ("a version ARN", format!("{machine}:1")),
        ("a version ARN of a version never published", format!("{machine}:12")),
        ("a map-run labeled ARN", format!("{machine}/label:PROD")),
        ("a qualifier outside the alias name rule", format!("{machine}:PR OD")),
        (
            // The modelled `CharacterRestrictedName` length is 1..80, and a
            // qualifier reaches no modelled constraint, so the rule is applied to it
            // here: one character past the bound is not an alias name.
            "a qualifier past the modelled name length",
            format!("{machine}:{}", "a".repeat(MAX_ALIAS_NAME_CHARS + 1)),
        ),
        (
            "a qualifier at the modelled name length, for a machine with no such alias",
            format!("{machine}:{}", "a".repeat(MAX_ALIAS_NAME_CHARS)),
        ),
    ];
    for (case, arn) in &invalid {
        let (status, body) = emulator
            .call(
                "DescribeStateMachineAlias",
                &json!({ "stateMachineAliasArn": arn }),
            )
            .await;
        let code = error_code(&body);
        // The last row is a WELL-FORMED alias ARN, so it is the one case here that
        // reaches the lookup and answers `ResourceNotFound` instead.
        let expected = if case.starts_with("a qualifier at the modelled name length") {
            (404, "ResourceNotFound")
        } else {
            (400, "InvalidArn")
        };
        assert_eq!(
            (status, code.as_str()),
            expected,
            "case {case:?}: expected {expected:?}, got {status} {code} with body {body:?}"
        );
    }

    // A well-formed alias ARN nothing answers for is a resource that is absent.
    for (case, arn) in [
        ("an alias that was never created", format!("{machine}:BETA")),
        ("an alias of a state machine that does not exist", format!("{absent}:PROD")),
    ] {
        let (status, body) = emulator
            .call(
                "DescribeStateMachineAlias",
                &json!({ "stateMachineAliasArn": arn }),
            )
            .await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (404, "ResourceNotFound"),
            "case {case:?}: expected 404 ResourceNotFound, got {status} {code} with body {body:?}"
        );
    }
}

/// Describing changes nothing a later caller sees, and an alias does not outlive the
/// state machine whose versions it points at.
#[tokio::test]
async fn describing_an_alias_changes_nothing_and_it_dies_with_its_state_machine() {
    let emulator = Emulator::new();
    let machine = machine_with_versions(&emulator, "reporting-aliases", 2).await;
    let routing = vec![
        route(&format!("{machine}:1"), 20),
        route(&format!("{machine}:2"), 80),
    ];
    let created = create_alias(&emulator, "PROD", Some("reported"), &routing).await;
    let alias_arn = string(&created, "stateMachineAliasArn");

    let first = describe_alias(&emulator, &alias_arn).await;
    for round in 0..3 {
        let again = describe_alias(&emulator, &alias_arn).await;
        assert_eq!(
            again, first,
            "round {round}: a describe must answer the same thing, got {again:?}"
        );
    }
    // And the create is still the idempotent response after those reads, so no read
    // consumed the record.
    let repeat = create_alias(&emulator, "PROD", Some("reported"), &routing).await;
    assert_eq!(
        string(&repeat, "stateMachineAliasArn"),
        alias_arn,
        "the reads must not have changed what a later create sees, got {repeat:?}"
    );

    emulator
        .ok("DeleteStateMachine", &json!({ "stateMachineArn": machine }))
        .await;
    std::thread::sleep(PAST_THE_DELETE_DELAY);
    let (status, body) = emulator
        .call(
            "DescribeStateMachineAlias",
            &json!({ "stateMachineAliasArn": &alias_arn }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "ResourceNotFound"),
        "an alias does not outlive its state machine, got {status} with body {body:?}"
    );
}
