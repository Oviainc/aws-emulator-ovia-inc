// SPDX-License-Identifier: CC0-1.0

//! What `UpdateStackSet` PUTS IN FORCE, end to end over the awsQuery wire.
//!
//! The rule every case here holds the service to: **`UpdateStackSet` records the
//! configuration the request states on the StackSet itself and deploys that revision to
//! exactly the stack instances the request selects — each selected instance's stack is
//! updated from the revision and the resources it declares are brought into step in the
//! service that owns them — while an instance the request does NOT select keeps the
//! deployment it already had and is reported `OUTDATED` once the revision changed; and the
//! call answers ONE `UPDATE` operation id, which is the caller's when it named one, so a
//! request whose selection, template sources or operation id cannot be served is refused and
//! writes nothing at all.**
//!
//! The tables span what the rule generalises over rather than one recorded request: the
//! selection is taken over its breadth (one account in one Region, one account across two
//! Regions, two accounts in one Region, and every instance by selecting none) and over the
//! kind of change it rolls out (a parameter value alone, and a revised template body); the
//! template sources are taken over every combination the three members admit, none and all
//! three included; the operation ids are taken over the forms and lengths
//! `ClientRequestToken` admits (`smithy_query(query="shape", shape="ClientRequestToken")`:
//! `@length(1..128)`, `@pattern("^[a-zA-Z0-9][-a-zA-Z0-9]*$")`); and the unheld selections
//! are taken over an unheld Region, an unheld account, both at once, and an unheld pair
//! beside a held one.
//!
//! Every call goes through the real request path (form-encoded body in, XML envelope out),
//! so what is asserted is what a client sees, and each instance's own resource is read back
//! through Amazon SNS's own wire in the instance's target Region.
//!
//! What this suite OWNS, beside `stack_set_mutations.rs`: that suite's fourth rule is that an
//! update records the revision and rolls it out, read over ONE selection in one Region. The
//! rule here is which instances a selection reaches and which it leaves behind — the
//! selection taken over its breadth and over two Regions, each instance's deployment read in
//! the service that OWNS its resource, and the three ways a request for a revision cannot be
//! served (its template sources, its operation id, a pair the StackSet does not hold) shown
//! to leave the StackSet exactly as it was. The small wire helpers below are this suite's
//! own, as every StackSet suite of this target carries its own (`delete_stack_set.rs`,
//! `stack_set_refusal_wording.rs`, `stack_set_lifecycle.rs`, `stop_stack_set_operation.rs`),
//! over the one harness in `harness.rs` that all of them share.

use service_com_amazonaws_cloudformation_cloudformation::state::stack_sets::{
    detailed_status, instance_status, operation_action,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, call_in, member, router, sns_in, wait_for_the_transition, xml_blocks, xml_text,
    xml_texts, ACCOUNT_ID, REGION,
};

/// A second target account beside the emulator's own (`Account` is `@pattern("^[0-9]{12}$")`).
const ACCOUNT_A: &str = "111111111111";

/// A second target Region beside the one the StackSet's own calls are signed for.
///
/// SOURCE: an entry of `emulator_telemetry::policy::KNOWN_REGIONS`, the one catalog of
/// Region codes this emulator holds, which every `RegionList` entry is checked against —
/// asserted against that catalog by the suite rather than taken on trust.
const REGION_B: &str = "us-west-2";

/// The template a StackSet here is created from: one topic whose `DisplayName` is the
/// parameter in force on the instance that deploys it, so a read of the topic in Amazon SNS
/// says which revision that instance is really running.
const TEMPLATE: &str = r#"{"Parameters":{"Shown":{"Type":"String","Default":"created-display"}},
     "Resources":{"Topic":{"Type":"AWS::SNS::Topic",
     "Properties":{"DisplayName":{"Ref":"Shown"}}}}}"#;

/// A revised template body: the same resource with a second parameter declared, so a row
/// that submits it changes the StackSet's template and not only a parameter value.
const REVISED_TEMPLATE: &str = r#"{"Parameters":{"Shown":{"Type":"String","Default":"created-display"},
     "Note":{"Type":"String","Default":"revised"}},
     "Resources":{"Topic":{"Type":"AWS::SNS::Topic",
     "Properties":{"DisplayName":{"Ref":"Shown"}}}}}"#;

/// The refusal for a request that does not name exactly one source for the template it
/// submits, spelled out here rather than read from the crate: the sentence IS the
/// requirement, so a reworded one must fail this suite.
const ONE_TEMPLATE_SOURCE: &str =
    "You must specify exactly one of TemplateBody, TemplateURL or UsePreviousTemplate";

/// A `TemplateURL` for the rows that name a second template source: the refusal for naming
/// more than one is answered before anything is fetched, so what the URL addresses never
/// matters to those rows.
const TEMPLATE_URL: &str = "https://s3.us-east-1.amazonaws.com/templates/revision.json";

/// `CreateStackSet` with a template and whatever else a case states.
async fn create_stack_set(
    router: &MultiServiceRouter,
    stack_set_name: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut members = vec![
        member("StackSetName", stack_set_name),
        member("TemplateBody", TEMPLATE),
    ];
    members.extend_from_slice(extra);
    call(router, "CreateStackSet", &members).await
}

/// The `Accounts.member.N` / `Regions.member.N` members one selection states.
fn selection(accounts: &[&str], regions: &[&str]) -> Vec<(String, String)> {
    let mut members = Vec::new();
    for (index, account) in accounts.iter().enumerate() {
        members.push(member(&format!("Accounts.member.{}", index + 1), account));
    }
    for (index, region) in regions.iter().enumerate() {
        members.push(member(&format!("Regions.member.{}", index + 1), region));
    }
    members
}

/// `CreateStackInstances` for one product of accounts and Regions.
async fn deploy(
    router: &MultiServiceRouter,
    stack_set_name: &str,
    accounts: &[&str],
    regions: &[&str],
) -> (u16, String) {
    let mut members = vec![member("StackSetName", stack_set_name)];
    members.extend(selection(accounts, regions));
    call(router, "CreateStackInstances", &members).await
}

/// The `OperationId` a call that starts a StackSet operation answered with.
fn operation_id_of(case: &str, http_status: u16, body: &str) -> String {
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected the call to succeed, got {http_status} {body}"
    );
    xml_text(body, "OperationId")
        .unwrap_or_else(|| panic!("case {case:?}: no OperationId in {body}"))
}

/// The `<Code>` and `<Message>` of a refusal envelope.
fn refusal(body: &str) -> (Option<String>, Option<String>) {
    (xml_text(body, "Code"), xml_text(body, "Message"))
}

/// `DescribeStackInstance` for one account and Region pair, read through the StackSet's own
/// Region.
async fn describe_stack_instance(
    router: &MultiServiceRouter,
    stack_set_name: &str,
    account: &str,
    region: &str,
) -> (u16, String) {
    call(
        router,
        "DescribeStackInstance",
        &[
            member("StackSetName", stack_set_name),
            member("StackInstanceAccount", account),
            member("StackInstanceRegion", region),
        ],
    )
    .await
}

/// The `ParameterValue` a read reports for `key`, out of the first `block` list the response
/// carries (`Parameters` on a StackSet, `ParameterOverrides` on an instance).
fn parameter_value(body: &str, block: &str, key: &str) -> Option<String> {
    let list = xml_blocks(body, block).into_iter().next()?;
    let keys = xml_texts(&list, "ParameterKey");
    let values = xml_texts(&list, "ParameterValue");
    keys.iter()
        .position(|held| held == key)
        .and_then(|index| values.get(index).cloned())
}

/// The `DisplayName` Amazon SNS holds for the topic the stack `stack_id` deployed in
/// `region`: the instance's own resource, read through the owning service rather than
/// through CloudFormation, so a rollout that never reached it cannot pass.
async fn deployed_display_name(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    stack_id: &str,
) -> String {
    let (http_status, body) = call_in(
        router,
        region,
        "DescribeStackResources",
        &[
            member("StackName", stack_id),
            member("LogicalResourceId", "Topic"),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected DescribeStackResources to succeed, got {http_status} {body}"
    );
    let topic_arn = xml_text(&body, "PhysicalResourceId")
        .unwrap_or_else(|| panic!("case {case:?}: no PhysicalResourceId in {body}"));
    let (http_status, body) = sns_in(
        router,
        region,
        "GetTopicAttributes",
        &[("TopicArn", topic_arn.as_str())],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected GetTopicAttributes({topic_arn}) to succeed, got {http_status} \
         {body}"
    );
    xml_blocks(&body, "entry")
        .iter()
        .find(|entry| xml_text(entry, "key").as_deref() == Some("DisplayName"))
        .and_then(|entry| xml_text(entry, "value"))
        .unwrap_or_default()
}

/// What one instance of the StackSet under test is deployed from right now: the operation
/// that last touched it, the template its stack carries and the `DisplayName` its topic
/// really holds in Amazon SNS.
#[derive(Debug, Clone)]
struct Deployed {
    account: &'static str,
    region: &'static str,
    operation_id: String,
    template: &'static str,
    display_name: String,
}

/// One row of the rollout table: the selection an update states, and the revision it puts in
/// force.
struct Rollout {
    case: &'static str,
    accounts: &'static [&'static str],
    regions: &'static [&'static str],
    /// The template body the update submits, or `None` for a row that asks for the one the
    /// StackSet already holds (`UsePreviousTemplate`).
    template_body: Option<&'static str>,
    /// The `Shown` parameter value the revision carries, which is the `DisplayName` each
    /// instance it reaches must end up with.
    display_name: &'static str,
}

/// The selection breadths and change kinds the rule covers, applied in order to ONE StackSet
/// holding four instances, so each row is read against the deployment the row before it left.
const ROLLOUTS: &[Rollout] = &[
    Rollout {
        case: "one account in one Region",
        accounts: &[ACCOUNT_ID],
        regions: &[REGION],
        template_body: None,
        display_name: "first-rollout",
    },
    Rollout {
        case: "one account across both Regions, with a revised template",
        accounts: &[ACCOUNT_ID],
        regions: &[REGION, REGION_B],
        template_body: Some(REVISED_TEMPLATE),
        display_name: "second-rollout",
    },
    Rollout {
        case: "both accounts in one Region",
        accounts: &[ACCOUNT_ID, ACCOUNT_A],
        regions: &[REGION_B],
        template_body: None,
        display_name: "third-rollout",
    },
    Rollout {
        case: "every instance, by selecting none",
        accounts: &[],
        regions: &[],
        template_body: None,
        display_name: "fourth-rollout",
    },
];

#[tokio::test]
async fn an_update_deploys_its_revision_to_exactly_the_instances_it_selects() {
    // The second Region is one the emulator's own catalog holds, so a row targeting it is
    // judged on the selection and not refused for naming a Region AWS does not have.
    assert!(
        ::emulator_telemetry::policy::KNOWN_REGIONS.contains(&REGION_B),
        "case {:?}: expected the Region catalog to hold {REGION_B:?}",
        "the second target Region"
    );

    let router = router();
    let stack_set_name = "revision-rollout";
    let (http_status, body) = create_stack_set(&router, stack_set_name, &[]).await;
    assert_eq!(
        http_status, 200,
        "expected CreateStackSet to succeed, got {http_status} {body}"
    );

    // Four instances: the product of two accounts and two Regions, so a row can select by
    // account, by Region, by both or by neither.
    let (http_status, body) = deploy(
        &router,
        stack_set_name,
        &[ACCOUNT_ID, ACCOUNT_A],
        &[REGION, REGION_B],
    )
    .await;
    let deployment = operation_id_of("the deployment under the created revision", http_status, &body);
    wait_for_the_transition().await;

    let mut deployed: Vec<Deployed> = Vec::new();
    for account in [ACCOUNT_ID, ACCOUNT_A] {
        for region in [REGION, REGION_B] {
            deployed.push(Deployed {
                account,
                region,
                operation_id: deployment.clone(),
                template: TEMPLATE,
                // The template's own `Default` for `Shown`, since the create supplied none.
                display_name: "created-display".to_string(),
            });
        }
    }

    // The StackSet's template as the rows have left it: a row that submits a body changes
    // it, a row that asks for the previous one does not.
    let mut stack_set_template = TEMPLATE;
    for rollout in ROLLOUTS {
        let case = rollout.case;
        let mut members = vec![
            member("StackSetName", stack_set_name),
            member("Parameters.member.1.ParameterKey", "Shown"),
            member("Parameters.member.1.ParameterValue", rollout.display_name),
        ];
        match rollout.template_body {
            Some(body) => members.push(member("TemplateBody", body)),
            None => members.push(member("UsePreviousTemplate", "true")),
        }
        members.extend(selection(rollout.accounts, rollout.regions));
        let (http_status, body) = call(&router, "UpdateStackSet", &members).await;
        let update = operation_id_of(case, http_status, &body);
        if let Some(submitted) = rollout.template_body {
            stack_set_template = submitted;
        }

        // The call answers ONE operation, and it is an `UPDATE` of this StackSet.
        let (http_status, body) = call(
            &router,
            "DescribeStackSetOperation",
            &[
                member("StackSetName", stack_set_name),
                member("OperationId", &update),
            ],
        )
        .await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected DescribeStackSetOperation to resolve {update}, got \
             {http_status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Action").as_deref(),
            Some(operation_action::UPDATE),
            "case {case:?}: expected an UPDATE operation, got {body}"
        );
        wait_for_the_transition().await;

        // What the row selected, as the pairs it names — a row naming neither member selects
        // every instance the StackSet holds.
        let selects = |instance: &Deployed| -> bool {
            if rollout.accounts.is_empty() && rollout.regions.is_empty() {
                return true;
            }
            rollout.accounts.contains(&instance.account)
                && rollout.regions.contains(&instance.region)
        };
        for instance in deployed.iter_mut() {
            if selects(instance) {
                instance.operation_id = update.clone();
                instance.template = stack_set_template;
                instance.display_name = rollout.display_name.to_string();
            }
        }

        // The StackSet itself carries the revision, whichever instances the row selected:
        // "Even if the StackSet operation created by updating the StackSet fails [...] the
        // StackSet is updated with your changes."
        let (http_status, body) = call(
            &router,
            "DescribeStackSet",
            &[member("StackSetName", stack_set_name)],
        )
        .await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected DescribeStackSet to succeed, got {http_status} {body}"
        );
        assert_eq!(
            xml_text(&body, "TemplateBody").as_deref(),
            Some(stack_set_template),
            "case {case:?}: expected the StackSet to carry the revision's template, got {body}"
        );
        assert_eq!(
            parameter_value(&body, "Parameters", "Shown").as_deref(),
            Some(rollout.display_name),
            "case {case:?}: expected the StackSet to carry the revision's parameter, got {body}"
        );

        // Each instance: the ones the row selected are up to date with the StackSet and
        // really running the revision, the ones it did not are OUTDATED and still running
        // what they ran before.
        for instance in &deployed {
            let selected = selects(instance);
            let (account, region) = (instance.account, instance.region);
            let (http_status, body) =
                describe_stack_instance(&router, stack_set_name, account, region).await;
            assert_eq!(
                http_status, 200,
                "case {case:?} on {account}/{region}: expected the instance to be described, \
                 got {http_status} {body}"
            );
            let expected_status = if selected {
                instance_status::CURRENT
            } else {
                instance_status::OUTDATED
            };
            assert_eq!(
                xml_text(&body, "Status").as_deref(),
                Some(expected_status),
                "case {case:?} on {account}/{region}: expected {expected_status}, got {body}"
            );
            assert_eq!(
                xml_text(&body, "LastOperationId").as_deref(),
                Some(instance.operation_id.as_str()),
                "case {case:?} on {account}/{region}: expected the operation that last \
                 deployed it ({}), got {body}",
                instance.operation_id
            );
            if selected {
                assert_eq!(
                    xml_text(&body, "DetailedStatus").as_deref(),
                    Some(detailed_status::SUCCEEDED),
                    "case {case:?} on {account}/{region}: expected the rollout to have \
                     succeeded, got {body}"
                );
            }
            let stack_id = xml_text(&body, "StackId").unwrap_or_else(|| {
                panic!("case {case:?} on {account}/{region}: no StackId in {body}")
            });

            // The instance's own stack, in the instance's own Region: the template it is
            // deployed from.
            let (http_status, body) = call_in(
                &router,
                region,
                "GetTemplate",
                &[member("StackName", &stack_id)],
            )
            .await;
            assert_eq!(
                http_status, 200,
                "case {case:?} on {account}/{region}: expected GetTemplate to succeed, got \
                 {http_status} {body}"
            );
            assert_eq!(
                xml_text(&body, "TemplateBody").as_deref(),
                Some(instance.template),
                "case {case:?} on {account}/{region}: expected the template it is deployed \
                 from, got {body}"
            );

            // And the resource that template declares, in the service that owns it.
            let display_name =
                deployed_display_name(case, &router, region, &stack_id).await;
            assert_eq!(
                display_name, instance.display_name,
                "case {case:?} on {account}/{region}: expected the topic to hold {:?}, got \
                 {display_name:?}",
                instance.display_name
            );
        }
    }
}

/// One row of the template-source table: the members a request names beside the StackSet,
/// and whether naming them is a request the service can serve. A served row is read back
/// through `DescribeStackSet`, so a row answered with an operation id over a discarded write
/// fails.
struct TemplateSource {
    case: &'static str,
    members: &'static [(&'static str, &'static str)],
    served: bool,
}

/// Every combination of the three members that say WHERE the revision's template comes from,
/// which the model documents as "you must specify only one of the following parameters:
/// TemplateBody or TemplateURL—or set UsePreviousTemplate to true".
const TEMPLATE_SOURCES: &[TemplateSource] = &[
    TemplateSource {
        case: "no template source at all",
        members: &[],
        served: false,
    },
    TemplateSource {
        case: "UsePreviousTemplate false, which names no source",
        members: &[("UsePreviousTemplate", "false")],
        served: false,
    },
    TemplateSource {
        case: "a body and the previous template",
        members: &[
            ("TemplateBody", REVISED_TEMPLATE),
            ("UsePreviousTemplate", "true"),
        ],
        served: false,
    },
    TemplateSource {
        case: "a body and a URL",
        members: &[
            ("TemplateBody", REVISED_TEMPLATE),
            ("TemplateURL", TEMPLATE_URL),
        ],
        served: false,
    },
    TemplateSource {
        case: "a URL and the previous template",
        members: &[
            ("TemplateURL", TEMPLATE_URL),
            ("UsePreviousTemplate", "true"),
        ],
        served: false,
    },
    TemplateSource {
        case: "all three at once",
        members: &[
            ("TemplateBody", REVISED_TEMPLATE),
            ("TemplateURL", TEMPLATE_URL),
            ("UsePreviousTemplate", "true"),
        ],
        served: false,
    },
    TemplateSource {
        case: "a body alone",
        members: &[("TemplateBody", REVISED_TEMPLATE)],
        served: true,
    },
    TemplateSource {
        case: "the previous template alone",
        members: &[("UsePreviousTemplate", "true")],
        served: true,
    },
];

#[tokio::test]
async fn an_update_must_name_exactly_one_source_for_its_template() {
    let router = router();
    let stack_set_name = "one-template-source";
    let (http_status, body) = create_stack_set(&router, stack_set_name, &[]).await;
    assert_eq!(
        http_status, 200,
        "expected CreateStackSet to succeed, got {http_status} {body}"
    );

    // The template the StackSet carries as the rows have left it, which a served row is read
    // back against.
    let mut in_force = TEMPLATE;
    for source in TEMPLATE_SOURCES {
        let case = source.case;
        let mut members = vec![member("StackSetName", stack_set_name)];
        for (key, value) in source.members {
            members.push(member(key, value));
        }
        // A served row starts an operation of its own, and a StackSet serves one at a time.
        if source.served {
            wait_for_the_transition().await;
        }
        let (http_status, body) = call(&router, "UpdateStackSet", &members).await;
        if source.served {
            operation_id_of(case, http_status, &body);
            if let Some((_, submitted)) =
                source.members.iter().find(|(key, _)| *key == "TemplateBody")
            {
                in_force = submitted;
            }
            // A served row WROTE: the StackSet carries the template that row put in force —
            // the body it submitted, or the one it asked to keep.
            let (http_status, body) = call(
                &router,
                "DescribeStackSet",
                &[member("StackSetName", stack_set_name)],
            )
            .await;
            assert_eq!(
                http_status, 200,
                "case {case:?}: expected DescribeStackSet to succeed, got {http_status} {body}"
            );
            assert_eq!(
                xml_text(&body, "TemplateBody").as_deref(),
                Some(in_force),
                "case {case:?}: expected the StackSet to carry the template now in force, got \
                 {body}"
            );
            continue;
        }
        assert_eq!(
            http_status, 400,
            "case {case:?}: expected the request to be refused with 400, got {http_status} \
             {body}"
        );
        let (code, message) = refusal(&body);
        assert_eq!(
            code.as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected <Code>ValidationError</Code>, got {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(ONE_TEMPLATE_SOURCE),
            "case {case:?}: expected {ONE_TEMPLATE_SOURCE:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn the_operation_id_a_request_names_is_the_operations_own_and_cannot_be_reused() {
    let router = router();
    let stack_set_name = "named-operations";
    let (http_status, body) = create_stack_set(&router, stack_set_name, &[]).await;
    assert_eq!(
        http_status, 200,
        "expected CreateStackSet to succeed, got {http_status} {body}"
    );

    // The forms and lengths `ClientRequestToken` admits: the uuid shape a client generates,
    // the shortest id the member takes, and one at its longest.
    let longest = format!("o{}", "p".repeat(127));
    let ids: [(&str, &str); 3] = [
        ("a uuid-shaped id a client generated", "7d1f0a2b-4c5d-4e6f-8a90-b1c2d3e4f506"),
        ("the shortest id the member admits", "x"),
        ("an id at the longest the member admits", longest.as_str()),
    ];

    for (case, operation_id) in ids {
        let members = vec![
            member("StackSetName", stack_set_name),
            member("UsePreviousTemplate", "true"),
            member("OperationId", operation_id),
        ];
        let (http_status, body) = call(&router, "UpdateStackSet", &members).await;
        let answered = operation_id_of(case, http_status, &body);
        assert_eq!(
            answered, operation_id,
            "case {case:?}: expected the caller's own operation id back, got {answered:?}"
        );
        // It is THAT operation the StackSet holds: the id addresses it.
        let (http_status, body) = call(
            &router,
            "DescribeStackSetOperation",
            &[
                member("StackSetName", stack_set_name),
                member("OperationId", operation_id),
            ],
        )
        .await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected the named operation to be described, got {http_status} \
             {body}"
        );
        assert_eq!(
            xml_text(&body, "Action").as_deref(),
            Some(operation_action::UPDATE),
            "case {case:?}: expected an UPDATE operation, got {body}"
        );
        wait_for_the_transition().await;

        // "The specified operation ID already exists": once the StackSet holds an operation
        // under that id, the same request is not performed a second time.
        let (http_status, body) = call(&router, "UpdateStackSet", &members).await;
        assert_eq!(
            http_status, 409,
            "case {case:?}: expected the reused id to be refused with 409, got {http_status} \
             {body}"
        );
        let (code, message) = refusal(&body);
        assert_eq!(
            code.as_deref(),
            Some("OperationIdAlreadyExistsException"),
            "case {case:?}: expected the modelled refusal, got {body}"
        );
        let expected = format!("Operation with Id {operation_id} already exists");
        assert_eq!(
            message.as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

/// One row of the unheld-selection table: the pairs an update selects, and the Region its
/// refusal names — the first selected Region the StackSet holds no instance in.
struct Unheld {
    case: &'static str,
    accounts: &'static [&'static str],
    regions: &'static [&'static str],
    named: &'static str,
}

/// The ways a selection can address an instance the StackSet does not hold: an unheld
/// Region, an unheld account, both at once, and an unheld pair beside a held one.
const UNHELD: &[Unheld] = &[
    Unheld {
        case: "a Region the StackSet holds no instance in",
        accounts: &[ACCOUNT_ID],
        regions: &[REGION_B],
        named: REGION_B,
    },
    Unheld {
        case: "an account the StackSet holds no instance for",
        accounts: &[ACCOUNT_A],
        regions: &[REGION],
        named: REGION,
    },
    Unheld {
        case: "an account and a Region the StackSet holds neither of",
        accounts: &[ACCOUNT_A],
        regions: &[REGION_B],
        named: REGION_B,
    },
    Unheld {
        case: "an unheld pair beside a held one",
        accounts: &[ACCOUNT_ID],
        regions: &[REGION, REGION_B],
        named: REGION_B,
    },
];

#[tokio::test]
async fn an_update_selecting_an_instance_the_stack_set_does_not_hold_is_refused_and_writes_nothing()
{
    let router = router();
    let stack_set_name = "unheld-selection";
    let (http_status, body) = create_stack_set(&router, stack_set_name, &[]).await;
    assert_eq!(
        http_status, 200,
        "expected CreateStackSet to succeed, got {http_status} {body}"
    );
    // ONE instance, so every other pair of the two accounts and two Regions is a pair the
    // StackSet holds nothing for.
    let (http_status, body) = deploy(&router, stack_set_name, &[ACCOUNT_ID], &[REGION]).await;
    let deployment = operation_id_of("the one deployment", http_status, &body);
    wait_for_the_transition().await;

    for unheld in UNHELD {
        let case = unheld.case;
        let mut members = vec![
            member("StackSetName", stack_set_name),
            member("UsePreviousTemplate", "true"),
            member("Parameters.member.1.ParameterKey", "Shown"),
            member("Parameters.member.1.ParameterValue", "never-applied"),
        ];
        members.extend(selection(unheld.accounts, unheld.regions));
        let (http_status, body) = call(&router, "UpdateStackSet", &members).await;
        assert_eq!(
            http_status, 404,
            "case {case:?}: expected the request to be refused with 404, got {http_status} \
             {body}"
        );
        let (code, message) = refusal(&body);
        assert_eq!(
            code.as_deref(),
            Some("StackInstanceNotFoundException"),
            "case {case:?}: expected the modelled refusal, got {body}"
        );
        let expected = format!("No Stack Instance found in region {}", unheld.named);
        assert_eq!(
            message.as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {message:?}"
        );

        // Refused WHOLESALE: the StackSet still carries what it carried before, so the
        // parameter the refused request stated is in force nowhere.
        let (http_status, body) = call(
            &router,
            "DescribeStackSet",
            &[member("StackSetName", stack_set_name)],
        )
        .await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected DescribeStackSet to succeed, got {http_status} {body}"
        );
        assert_eq!(
            xml_text(&body, "TemplateBody").as_deref(),
            Some(TEMPLATE),
            "case {case:?}: expected the StackSet's template to be untouched, got {body}"
        );
        assert_eq!(
            parameter_value(&body, "Parameters", "Shown").as_deref(),
            Some("created-display"),
            "case {case:?}: expected the StackSet's parameter to be untouched, got {body}"
        );

        // The instance it does hold is untouched too, and no operation was recorded for the
        // refused request.
        let (http_status, body) =
            describe_stack_instance(&router, stack_set_name, ACCOUNT_ID, REGION).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected the held instance to be described, got {http_status} \
             {body}"
        );
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some(instance_status::CURRENT),
            "case {case:?}: expected the held instance to stay up to date, got {body}"
        );
        assert_eq!(
            xml_text(&body, "LastOperationId").as_deref(),
            Some(deployment.as_str()),
            "case {case:?}: expected the deployment to still be its last operation, got {body}"
        );
        let (http_status, body) = call(
            &router,
            "ListStackSetOperations",
            &[member("StackSetName", stack_set_name)],
        )
        .await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected ListStackSetOperations to succeed, got {http_status} \
             {body}"
        );
        assert_eq!(
            xml_texts(&body, "OperationId"),
            vec![deployment.clone()],
            "case {case:?}: expected only the deployment to be recorded, got {body}"
        );
    }
}
