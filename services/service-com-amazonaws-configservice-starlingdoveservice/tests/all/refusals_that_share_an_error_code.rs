// SPDX-License-Identifier: CC0-1.0

//! The SENTENCE that tells apart Config refusals which share one error code, on the
//! wire.
//!
//! Each error code below stands for several refusals, so the message is the only signal
//! a client can branch on. Five production rules are pinned here, each over a table
//! spanning its class rather than one instance:
//!
//! 1. the tag API's `ResourceNotFoundException` names the ARN the request carried and
//!    the account it was looked up in — the account every resource this store holds
//!    belongs to — so a caller learns both what was not found and whose resources were
//!    searched;
//! 2. `NoSuchRetentionConfigurationException` names the retention configuration the
//!    request asked for and nothing else, wherever it is raised;
//! 3. `GetCustomRulePolicy` refuses every name that selects no Custom Policy rule with
//!    the one rule-name sentence Config answers for a rule name it cannot serve,
//!    whatever kind of rule the account happens to hold under that name;
//! 4. an aggregation source that asks for a list of Regions AND for all Regions is
//!    refused with one FIXED sentence about the request, naming neither the member nor
//!    the Regions — and so are the source-selection legs that share its error code,
//!    each with its own sentence;
//! 5. a Custom Policy rule submitted without its details is refused by naming the
//!    member and the owner that requires it.
//!
//! A sixth rule, on the `ValidationException` envelope: a value is stated against every
//! constraint of its member that it broke — for an account id, the twelve-digit length
//! as well as the pattern — so one member can account for several of the validation
//! errors the envelope counts.
//!
//! Every case drives the emulator the way a client does, through the same
//! `MultiServiceRouter` the production router builds, reusing the `Emulator` harness
//! `config_setup_and_rule_loop` provides and the `refusal` helper `config_connectors`
//! already shares.

use serde_json::{json, Value};

use crate::config_connectors::refusal;
use crate::config_setup_and_rule_loop::{
    managed_rule, with_recorder_and_channel, Emulator, ROLE_ARN,
};

/// The account every resource this emulator holds belongs to, which a tag-API not-found
/// refusal reports the lookup as having happened in.
fn account() -> String {
    ::emulator_server::emulator_account_id().to_string()
}

/// The three operations of Config's tag API, each addressing a resource by the ARN
/// `arn`: the message rule is a property of the lookup, so all three state it.
fn tag_api_requests(arn: &str) -> [(&'static str, Value); 3] {
    [
        (
            "TagResource",
            json!({ "ResourceArn": arn, "Tags": [{ "Key": "owner", "Value": "platform" }] }),
        ),
        (
            "UntagResource",
            json!({ "ResourceArn": arn, "TagKeys": ["owner"] }),
        ),
        ("ListTagsForResource", json!({ "ResourceArn": arn })),
    ]
}

/// A tag-API refusal for an ARN naming no resource of this account states the ARN the
/// request carried and the account the lookup happened in.
#[tokio::test]
async fn a_tag_api_not_found_names_the_arn_and_the_account_it_was_looked_up_in() {
    let emulator = Emulator::new();
    let account = account();

    // Rows differ along the KIND of resource the ARN names and the shape of its
    // resource field: a Config rule, a stored query (two path segments), a recorder, a
    // conformance pack, an aggregation authorization, and an ARN of another service
    // with neither Region nor account. None of them names a resource in this store, so
    // each one is the same refusal with a different ARN.
    let arns: [(&str, String); 6] = [
        (
            "a config rule",
            format!("arn:aws:config:us-east-1:{account}:config-rule/config-rule-zzzzzz"),
        ),
        (
            "a stored query",
            format!("arn:aws:config:us-east-1:{account}:stored-query/absent-query/query-{account}"),
        ),
        (
            "a configuration recorder",
            format!("arn:aws:config:us-east-1:{account}:configuration-recorder/default/0000000000000000"),
        ),
        (
            "a conformance pack",
            format!("arn:aws:config:us-east-1:{account}:conformance-pack/absent-pack/cp-abcd1234"),
        ),
        (
            "an aggregation authorization in another Region",
            format!("arn:aws:config:eu-west-1:{account}:aggregation-authorization/{account}/us-east-1"),
        ),
        (
            "another service's ARN, with no Region or account",
            "arn:aws:s3:::config-bucket".to_string(),
        ),
    ];

    for (case, arn) in arns {
        let expected = format!("Resource {arn} does not exist for account {account}");
        for (operation, body) in tag_api_requests(&arn) {
            let (status, code, message) = refusal(&emulator, operation, &body).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ResourceNotFoundException"),
                "case {case:?} on {operation}: expected HTTP 400 \
                 ResourceNotFoundException, got {status} {code} {message:?}"
            );
            assert_eq!(
                message, expected,
                "case {case:?} on {operation}: expected {expected:?}, got {message:?}"
            );
        }
    }
}

/// The same lookup against a resource the store DOES hold is served, so the sentence
/// above reports a state of the store rather than a fixed answer for every ARN.
#[tokio::test]
async fn the_tag_api_serves_an_arn_the_store_holds() {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    let created = emulator
        .ok(
            "PutStoredQuery",
            &json!({ "StoredQuery": {
                "QueryName": "tagged-query",
                "Expression": "SELECT resourceId, resourceType WHERE resourceType = \
                               'AWS::S3::Bucket'",
            }}),
        )
        .await;
    let arn = created["QueryArn"]
        .as_str()
        .unwrap_or_else(|| panic!("PutStoredQuery must report the query ARN, got {created}"))
        .to_string();

    emulator
        .ok(
            "TagResource",
            &json!({ "ResourceArn": arn, "Tags": [{ "Key": "owner", "Value": "platform" }] }),
        )
        .await;
    let listed = emulator
        .ok("ListTagsForResource", &json!({ "ResourceArn": arn }))
        .await;
    assert_eq!(
        listed["Tags"],
        json!([{ "Key": "owner", "Value": "platform" }]),
        "case {:?}: expected the tag the write stored to be read back, got {listed}",
        "a stored query this account holds"
    );
}

/// A request that names a retention configuration the account does not hold is refused
/// with the one sentence naming that name — from either operation that raises the class.
#[tokio::test]
async fn a_missing_retention_configuration_is_reported_by_the_name_the_request_carried() {
    // Rows differ along the name asked for: the only name Config ever assigns the
    // object, a near miss of it, a differently cased one, a one-character name and a
    // long one — a name is matched exactly, so each is absent.
    let names: [&str; 5] = [
        "default",
        "default-wrong",
        "Default",
        "x",
        "retention-configuration-that-was-never-written",
    ];

    for name in names {
        let emulator = Emulator::new();
        let expected = format!("Cannot find retention configuration with specified name '{name}'.");
        let requests: [(&str, Value); 2] = [
            (
                "DeleteRetentionConfiguration",
                json!({ "RetentionConfigurationName": name }),
            ),
            (
                "DescribeRetentionConfigurations",
                json!({ "RetentionConfigurationNames": [name] }),
            ),
        ];
        for (operation, body) in requests {
            let (status, code, message) = refusal(&emulator, operation, &body).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "NoSuchRetentionConfigurationException"),
                "case {name:?} on {operation}: expected HTTP 400 \
                 NoSuchRetentionConfigurationException, got {status} {code} {message:?}"
            );
            assert_eq!(
                message, expected,
                "case {name:?} on {operation}: expected {expected:?}, got {message:?}"
            );
        }
    }

    // The name the account DOES hold after a write is served by both operations, so the
    // refusal above reports the store rather than refusing every name.
    let emulator = Emulator::new();
    emulator
        .ok(
            "PutRetentionConfiguration",
            &json!({ "RetentionPeriodInDays": 30 }),
        )
        .await;
    let described = emulator
        .ok(
            "DescribeRetentionConfigurations",
            &json!({ "RetentionConfigurationNames": ["default"] }),
        )
        .await;
    assert_eq!(
        described["RetentionConfigurations"][0]["Name"],
        json!("default"),
        "case {:?}: expected the written object to be served, got {described}",
        "the name the account holds"
    );
    emulator
        .ok(
            "DeleteRetentionConfiguration",
            &json!({ "RetentionConfigurationName": "default" }),
        )
        .await;
}

/// The `PutConfigRule` body for a rule named `name` whose `Source` carries `owner`,
/// with the member that owner requires — except for `CUSTOM_POLICY`, whose details the
/// cases below deliberately leave out.
fn rule_with_owner(name: &str, owner: &str) -> Value {
    let mut source = json!({ "Owner": owner });
    if owner == "CUSTOM_LAMBDA" {
        source["SourceIdentifier"] =
            json!("arn:aws:lambda:us-east-1:000000000000:function:evaluate");
    }
    json!({ "ConfigRule": { "ConfigRuleName": name, "Source": source } })
}

/// `GetCustomRulePolicy` refuses every name that selects no Custom Policy rule with the
/// one rule-name sentence, whatever the account holds under that name: Config does not
/// distinguish "the wrong kind of rule" from "no such rule" here.
#[tokio::test]
async fn get_custom_rule_policy_states_the_rule_name_sentence_for_every_name_it_cannot_serve() {
    // Rows differ along WHY the name selects no Custom Policy rule: nothing at all
    // under it, a managed (`AWS`) rule, a Custom Lambda rule, and a Custom Policy rule
    // that was deleted.
    enum Holds {
        Nothing,
        Rule(&'static str),
        DeletedPolicyRule,
    }

    for (case, name, holds) in [
        ("no rule under the name", "absent-rule", Holds::Nothing),
        ("a managed rule", "managed-rule", Holds::Rule("AWS")),
        ("a Custom Lambda rule", "lambda-rule", Holds::Rule("CUSTOM_LAMBDA")),
        (
            "a Custom Policy rule that was deleted",
            "deleted-policy-rule",
            Holds::DeletedPolicyRule,
        ),
    ] {
        let emulator = Emulator::new();
        with_recorder_and_channel(&emulator).await;
        match holds {
            Holds::Nothing => {}
            Holds::Rule("AWS") => {
                emulator.ok("PutConfigRule", &managed_rule(name)).await;
            }
            Holds::Rule(owner) => {
                emulator
                    .ok("PutConfigRule", &rule_with_owner(name, owner))
                    .await;
            }
            Holds::DeletedPolicyRule => {
                emulator
                    .ok("PutConfigRule", &policy_rule(name, "rule always { true }"))
                    .await;
                emulator
                    .ok("DeleteConfigRule", &json!({ "ConfigRuleName": name }))
                    .await;
            }
        }

        let expected = format!(
            "The ConfigRule '{name}' provided in the request is invalid. Please check the \
             configRule name."
        );
        let (status, code, message) = refusal(
            &emulator,
            "GetCustomRulePolicy",
            &json!({ "ConfigRuleName": name }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NoSuchConfigRuleException"),
            "case {case:?}: expected HTTP 400 NoSuchConfigRuleException, got {status} \
             {code} {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

/// The `PutConfigRule` body for a Custom Policy rule named `name` carrying `policy`.
fn policy_rule(name: &str, policy: &str) -> Value {
    json!({
        "ConfigRule": {
            "ConfigRuleName": name,
            "Source": {
                "Owner": "CUSTOM_POLICY",
                "CustomPolicyDetails": {
                    "PolicyRuntime": "guard-2.x.x",
                    "PolicyText": policy,
                },
            },
        }
    })
}

/// A Custom Policy rule the account DOES hold is served its policy body, so the
/// refusal above is a property of what the store holds.
#[tokio::test]
async fn get_custom_rule_policy_serves_a_policy_rule_the_store_holds() {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    for (name, policy) in [("alpha", "rule alpha { true }"), ("beta", "")] {
        emulator
            .ok("PutConfigRule", &policy_rule(name, policy))
            .await;
        let got = emulator
            .ok("GetCustomRulePolicy", &json!({ "ConfigRuleName": name }))
            .await;
        assert_eq!(
            got["PolicyText"],
            json!(policy),
            "case {name:?}: expected that rule's own policy body, got {got}"
        );
    }
}

/// An aggregation source asking for both an explicit Region list and all Regions is
/// refused with one fixed sentence about the aggregator, whichever kind of source
/// carried the two forms — and the other two legs of the same error code keep their own
/// sentences, so a client can still tell the three apart.
#[tokio::test]
async fn an_aggregation_source_naming_both_region_forms_is_refused_with_one_sentence() {
    const BOTH_FORMS: &str = "Your configuration aggregator contains a list of regions and \
                              also specifies the use of all regions. You must choose one of \
                              these options.";
    let account_id = account();

    // Rows differ along the KIND of source (account, organization), how many Regions
    // the list carries, and how many accounts the source aggregates.
    let bodies: [(&str, Value); 4] = [
        (
            "an account source with one listed Region",
            json!({
                "ConfigurationAggregatorName": "both-forms",
                "AccountAggregationSources": [{
                    "AccountIds": [account_id],
                    "AllAwsRegions": true,
                    "AwsRegions": ["us-east-1"],
                }],
            }),
        ),
        (
            "an account source with several listed Regions",
            json!({
                "ConfigurationAggregatorName": "both-forms-many",
                "AccountAggregationSources": [{
                    "AccountIds": [account_id],
                    "AllAwsRegions": true,
                    "AwsRegions": ["us-east-1", "eu-west-1", "ap-southeast-2"],
                }],
            }),
        ),
        (
            "an organization source",
            json!({
                "ConfigurationAggregatorName": "org-both-forms",
                "OrganizationAggregationSource": {
                    "RoleArn": ROLE_ARN,
                    "AllAwsRegions": true,
                    "AwsRegions": ["us-east-1"],
                },
            }),
        ),
        (
            // The modelled list holds one source, so the second variation is in the
            // source itself: several accounts beside the two region forms.
            "a source aggregating several accounts",
            json!({
                "ConfigurationAggregatorName": "many-accounts-both-forms",
                "AccountAggregationSources": [{
                    "AccountIds": [account_id, "111111111111", "222222222222"],
                    "AllAwsRegions": true,
                    "AwsRegions": ["eu-west-1"],
                }],
            }),
        ),
    ];

    for (case, body) in bodies {
        let emulator = Emulator::new();
        let (status, code, message) = refusal(&emulator, "PutConfigurationAggregator", &body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: expected HTTP 400 InvalidParameterValueException, got \
             {status} {code} {message:?}"
        );
        assert_eq!(
            message, BOTH_FORMS,
            "case {case:?}: expected {BOTH_FORMS:?}, got {message:?}"
        );
    }

    // The other legs of `InvalidParameterValueException` on this operation each carry
    // their OWN sentence, so a client keyed on the text can still tell the four apart.
    // Each sentence states the request as a whole and names no member of it.
    let other_legs: [(&str, Value, &str); 3] = [
        (
            "a source naming no Region at all",
            json!({
                "ConfigurationAggregatorName": "no-regions",
                "AccountAggregationSources": [{ "AccountIds": [account_id] }],
            }),
            "Your request does not specify any regions. Select AWS Config-supported \
             regions and try again.",
        ),
        (
            "an organization source naming no Region at all",
            json!({
                "ConfigurationAggregatorName": "org-no-regions",
                "OrganizationAggregationSource": { "RoleArn": ROLE_ARN },
            }),
            "Your request does not specify any regions. Select AWS Config-supported \
             regions and try again.",
        ),
        (
            "both kinds of source at once",
            json!({
                "ConfigurationAggregatorName": "two-kinds",
                "AccountAggregationSources": [{
                    "AccountIds": [account_id],
                    "AllAwsRegions": true,
                }],
                "OrganizationAggregationSource": { "RoleArn": ROLE_ARN, "AllAwsRegions": true },
            }),
            "The configuration aggregator cannot be created because your request \
             contains both the AccountAggregationSource and the \
             OrganizationAggregationSource. Include only one aggregation source and try \
             again.",
        ),
    ];
    for (case, body, expected) in other_legs {
        let emulator = Emulator::new();
        let (status, code, message) = refusal(&emulator, "PutConfigurationAggregator", &body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: expected HTTP 400 InvalidParameterValueException, got \
             {status} {code} {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert_ne!(
            message, BOTH_FORMS,
            "case {case:?}: this leg must not read as the both-forms refusal, got \
             {message:?}"
        );
    }

    // A source that names ONE of the two forms is accepted, so the refusal is about the
    // pair rather than about either member.
    for (case, source) in [
        (
            "all Regions",
            json!({ "AccountIds": [account_id], "AllAwsRegions": true }),
        ),
        (
            "a listed Region",
            json!({ "AccountIds": [account_id], "AwsRegions": ["us-east-1"] }),
        ),
    ] {
        let emulator = Emulator::new();
        emulator
            .ok(
                "PutConfigurationAggregator",
                &json!({
                    "ConfigurationAggregatorName": "one-form",
                    "AccountAggregationSources": [source],
                }),
            )
            .await;
        let described = emulator
            .ok(
                "DescribeConfigurationAggregators",
                &json!({ "ConfigurationAggregatorNames": ["one-form"] }),
            )
            .await;
        assert_eq!(
            described["ConfigurationAggregators"][0]["ConfigurationAggregatorName"],
            json!("one-form"),
            "case {case:?}: expected the aggregator to be stored, got {described}"
        );
    }
}

/// A rule whose owner is `CUSTOM_POLICY` and which carries no details is refused by
/// naming the member and the owner that requires it — the sentence that tells this leg
/// apart from the other refusals sharing its code.
#[tokio::test]
async fn a_custom_policy_rule_without_its_details_names_the_member_and_the_owner() {
    const EXPECTED: &str = "CustomPolicyDetails is required when Owner is CUSTOM_POLICY";

    // Rows differ along what else the rule carries: nothing, a description, a scope,
    // and a `SourceIdentifier` (which satisfies every OTHER owner, and so must not
    // satisfy this one).
    let rules: [(&str, Value); 4] = [
        ("the bare source", rule_with_owner("policy-rule", "CUSTOM_POLICY")),
        (
            "a described rule",
            json!({ "ConfigRule": {
                "ConfigRuleName": "described-policy-rule",
                "Description": "a Guard rule with no policy",
                "Source": { "Owner": "CUSTOM_POLICY" },
            }}),
        ),
        (
            "a scoped rule",
            json!({ "ConfigRule": {
                "ConfigRuleName": "scoped-policy-rule",
                "Scope": { "ComplianceResourceTypes": ["AWS::S3::Bucket"] },
                "Source": { "Owner": "CUSTOM_POLICY" },
            }}),
        ),
        (
            "a rule carrying a source identifier instead",
            json!({ "ConfigRule": {
                "ConfigRuleName": "identified-policy-rule",
                "Source": { "Owner": "CUSTOM_POLICY", "SourceIdentifier": "S3_BUCKET_VERSIONING_ENABLED" },
            }}),
        ),
    ];

    for (case, body) in rules {
        let emulator = Emulator::new();
        with_recorder_and_channel(&emulator).await;
        let (status, code, message) = refusal(&emulator, "PutConfigRule", &body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: expected HTTP 400 InvalidParameterValueException, got \
             {status} {code} {message:?}"
        );
        assert_eq!(
            message, EXPECTED,
            "case {case:?}: expected {EXPECTED:?}, got {message:?}"
        );
    }

    // The same rule WITH its details is stored and read back, so the refusal is about
    // the missing member rather than about the owner.
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    emulator
        .ok(
            "PutConfigRule",
            &policy_rule("policy-rule", "rule always { true }"),
        )
        .await;
    let got = emulator
        .ok(
            "GetCustomRulePolicy",
            &json!({ "ConfigRuleName": "policy-rule" }),
        )
        .await;
    assert_eq!(
        got["PolicyText"],
        json!("rule always { true }"),
        "case {:?}: expected the stored policy body, got {got}",
        "a Custom Policy rule with its details"
    );
}

/// An account id of any length but twelve breaks the length its twelve digits fix as
/// well as the pattern, and the envelope counts and states a clause for each.
#[tokio::test]
async fn an_account_id_of_the_wrong_length_is_stated_against_both_its_constraints() {
    // The member path a clause names, per operation that takes an account id, from
    // `smithy_query(query="operation", operation="PutAggregationAuthorization,
    // DeleteAggregationAuthorization,DeletePendingAggregationRequest")`.
    let members: [(&str, &str, &str); 3] = [
        (
            "PutAggregationAuthorization",
            "AuthorizedAccountId",
            "authorizedAccountId",
        ),
        (
            "DeleteAggregationAuthorization",
            "AuthorizedAccountId",
            "authorizedAccountId",
        ),
        (
            "DeletePendingAggregationRequest",
            "RequesterAccountId",
            "requesterAccountId",
        ),
    ];
    // The Region member each of those operations also requires.
    let region_member = |operation: &str| match operation {
        "DeletePendingAggregationRequest" => "RequesterAwsRegion",
        _ => "AuthorizedAwsRegion",
    };

    // Rows differ along the submitted value's LENGTH: past twelve, under twelve, empty,
    // and exactly twelve (the only length the pattern admits, so the pattern is then
    // the only clause).
    let values: [(&str, &str, Option<&str>); 5] = [
        (
            "longer than an account id",
            "not-an-account-id",
            Some("Member must have length less than or equal to 12"),
        ),
        (
            "thirteen digits",
            "0000000000000",
            Some("Member must have length less than or equal to 12"),
        ),
        (
            "shorter than an account id",
            "12345",
            Some("Member must have length greater than or equal to 12"),
        ),
        (
            "the empty value",
            "",
            Some("Member must have length greater than or equal to 12"),
        ),
        ("twelve characters that are not digits", "not-an-acct1", None),
    ];

    for (operation, member, path) in members {
        for (case, value, length_requirement) in values {
            let emulator = Emulator::new();
            let mut body = json!({});
            body[member] = json!(value);
            body[region_member(operation)] = json!("us-east-1");
            let (status, code, message) = refusal(&emulator, operation, &body).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ValidationException"),
                "case {case:?} on {operation}: expected HTTP 400 ValidationException, \
                 got {status} {code} {message:?}"
            );
            let pattern_clause = format!(
                "Value '{value}' at '{path}' failed to satisfy constraint: Member must \
                 satisfy regular expression pattern: \\d{{12}}"
            );
            let expected = match length_requirement {
                Some(requirement) => format!(
                    "2 validation errors detected: Value '{value}' at '{path}' failed to \
                     satisfy constraint: {requirement}; {pattern_clause}"
                ),
                None => format!("1 validation error detected: {pattern_clause}"),
            };
            assert_eq!(
                message, expected,
                "case {case:?} on {operation}: expected {expected:?}, got {message:?}"
            );
        }

        // A twelve-digit account id breaks neither constraint, so the operation is
        // served: the clauses above report the value the caller sent rather than
        // refusing every value.
        let emulator = Emulator::new();
        let mut body = json!({});
        body[member] = json!(account());
        body[region_member(operation)] = json!("us-east-1");
        let (status, parsed) = emulator.call(operation, &body).await;
        assert_eq!(
            status, 200,
            "case {:?} on {operation}: a twelve-digit account id breaks no constraint, \
             got {status} {parsed}",
            "a well-formed account id"
        );
    }

    // The recorder's `ServicePrincipal` keeps its single clause: the implied length
    // belongs to the account-id shape, not to every patterned member.
    let emulator = Emulator::new();
    let (_, _, message) = refusal(
        &emulator,
        "PutServiceLinkedConfigurationRecorder",
        &json!({ "ServicePrincipal": "not a valid principal!!" }),
    )
    .await;
    assert!(
        message.starts_with("1 validation error detected: "),
        "case {:?}: expected one clause, got {message:?}",
        "another shape's pattern"
    );
}
