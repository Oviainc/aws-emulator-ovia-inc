// SPDX-License-Identifier: CC0-1.0

//! `ListDeploymentConfigs`: every deployment configuration name this account
//! holds — the `CodeDeployDefault.*` ones CodeDeploy predefines and the ones a
//! client created — reported one page at a time.
//!
//! Every case goes through the same front door a client uses — the router, the
//! boundary validator and the awsJson1_1 envelope — by reusing the harness the
//! lifecycle suite owns.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{error_code, Emulator};

/// The `CodeDeployDefault.*` configurations CodeDeploy predefines, which the
/// store seeds before any request is served. Source: the AWS CodeDeploy User
/// Guide's "Deployment configurations on an EC2/on-premises compute platform"
/// and "Deployment configurations on an AWS Lambda or Amazon ECS compute
/// platform" pages, the same list `crate::state::BUILTIN_DEPLOYMENT_CONFIGS`
/// seeds.
const PREDEFINED: &[&str] = &[
    "CodeDeployDefault.OneAtATime",
    "CodeDeployDefault.HalfAtATime",
    "CodeDeployDefault.AllAtOnce",
    "CodeDeployDefault.LambdaAllAtOnce",
    "CodeDeployDefault.ECSAllAtOnce",
];

/// The names in a listing's `deploymentConfigsList` member.
fn config_names(body: &Value) -> Vec<String> {
    body["deploymentConfigsList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|name| name.as_str().map(str::to_string))
        .collect()
}

/// Every page of the listing, concatenated, following `nextToken` to the end.
///
/// Asserts along the way what paging itself owes a caller: no page is empty, and
/// the walk terminates.
async fn list_every_config(emulator: &Emulator, case: &str) -> Vec<String> {
    let mut collected: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut request = json!({});
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let page = emulator.ok("ListDeploymentConfigs", request).await;
        let names = config_names(&page);
        assert!(
            !names.is_empty(),
            "case {case:?}: expected a non-empty page {pages}, got {page:?}"
        );
        collected.extend(names);
        pages += 1;
        assert!(
            pages <= 20,
            "case {case:?}: paging did not terminate after {pages} pages"
        );
        token = page["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return collected;
        }
    }
}

/// Create a `Server` deployment configuration under `name`.
async fn create(emulator: &Emulator, name: &str) {
    emulator
        .ok(
            "CreateDeploymentConfig",
            json!({
                "deploymentConfigName": name,
                "computePlatform": "Server",
                "minimumHealthyHosts": {"type": "HOST_COUNT", "value": 1},
            }),
        )
        .await;
}

/// RULE: the listing answers the name of every deployment configuration the
/// account holds — the predefined ones and each one created, each exactly once —
/// and nothing else.
#[tokio::test]
async fn listing_answers_the_predefined_and_the_created_configurations() {
    let emulator = Emulator::new();

    // A fresh account has created nothing, so the listing is exactly what
    // CodeDeploy predefines — never an empty list.
    let fresh = list_every_config(&emulator, "no created configurations").await;
    let predefined: BTreeSet<String> = PREDEFINED.iter().map(|name| (*name).to_string()).collect();
    assert_eq!(
        fresh.len(),
        PREDEFINED.len(),
        "case no created configurations: expected the {} predefined names with no repeats, got \
         {fresh:?}",
        PREDEFINED.len()
    );
    assert_eq!(
        fresh.iter().cloned().collect::<BTreeSet<String>>(),
        predefined,
        "case no created configurations: expected exactly the predefined names {PREDEFINED:?}, \
         got {fresh:?}"
    );

    // Rows differ along the compute platform the configuration describes (and so
    // along which optional member it carries), the number created, and the FORM
    // of the name: the punctuation row spans every non-alphanumeric character
    // `DeploymentConfigName`'s modelled pattern allows, and the bounds rows sit
    // on each end of its modelled length
    // (`smithy_query(query="shape",shape="DeploymentConfigName")`: length 1..100,
    // pattern ^[A-Za-z0-9+=,.@_-]*$).
    let max_length = "c".repeat(100);
    let cases: &[(&str, Value)] = &[
        (
            "cfg-server",
            json!({"computePlatform": "Server", "minimumHealthyHosts": {"type": "FLEET_PERCENT", "value": 75}}),
        ),
        (
            "cfg-lambda",
            json!({"computePlatform": "Lambda", "trafficRoutingConfig": {"type": "AllAtOnce"}}),
        ),
        (
            "cfg-ecs",
            json!({
                "computePlatform": "ECS",
                "trafficRoutingConfig": {"type": "TimeBasedLinear", "timeBasedLinear": {"linearPercentage": 20, "linearInterval": 1}},
            }),
        ),
        // `computePlatform` omitted: CodeDeploy's default platform.
        ("cfg-defaulted-platform", json!({})),
        ("c+1", json!({})),
        ("c=2", json!({})),
        ("c,3", json!({})),
        ("c.4", json!({})),
        ("c@5", json!({})),
        ("c_6", json!({})),
        ("c-7", json!({})),
        // The modelled length bounds: one character, and one hundred.
        ("c", json!({})),
        (max_length.as_str(), json!({})),
    ];

    let mut created: Vec<String> = Vec::new();
    for (name, extra) in cases {
        let mut request = json!({"deploymentConfigName": name});
        for (member, value) in extra.as_object().expect("the case body is an object") {
            request[member] = value.clone();
        }
        emulator.ok("CreateDeploymentConfig", request).await;
        created.push((*name).to_string());

        // After each create the listing carries the predefined names plus exactly
        // the names created so far — so a name is listed once, and creating one
        // drops none of the others.
        let mut expected: BTreeSet<String> =
            PREDEFINED.iter().map(|name| (*name).to_string()).collect();
        expected.extend(created.iter().cloned());
        let listed = list_every_config(&emulator, name).await;
        assert_eq!(
            listed.len(),
            expected.len(),
            "case {name:?}: expected {} names with no repeats, got {listed:?}",
            expected.len()
        );
        assert_eq!(
            listed.iter().cloned().collect::<BTreeSet<String>>(),
            expected,
            "case {name:?}: expected the predefined names plus {created:?}, got {listed:?}"
        );
    }
}

/// RULE: the listing REPORTS the store — it is answered from the stored records,
/// so two calls in a row answer the same thing, the order they are answered in
/// is stable, and a configuration created afterwards appears in the next call.
#[tokio::test]
async fn listing_is_a_stable_report_of_the_store() {
    let emulator = Emulator::new();
    for name in ["stable-a", "stable-b", "stable-c"] {
        create(&emulator, name).await;
    }

    let first = list_every_config(&emulator, "first read").await;
    let second = list_every_config(&emulator, "second read").await;
    assert_eq!(
        first, second,
        "case repeated listing: a read must not change what the next caller sees, got {first:?} \
         then {second:?}"
    );

    // Created configurations are reported in the order they were created, after
    // the predefined ones the store seeds first.
    let created_positions: Vec<usize> = ["stable-a", "stable-b", "stable-c"]
        .iter()
        .map(|name| {
            second
                .iter()
                .position(|listed| listed == name)
                .unwrap_or_else(|| panic!("case {name:?}: expected it listed, got {second:?}"))
        })
        .collect();
    assert!(
        created_positions.windows(2).all(|pair| pair[0] < pair[1]),
        "case creation order: expected stable-a, stable-b, stable-c in that order, got {second:?}"
    );

    create(&emulator, "stable-late").await;
    let after = list_every_config(&emulator, "created after a listing").await;
    assert_eq!(
        after[..second.len()],
        second[..],
        "case created after a listing: expected the earlier names unmoved, got {after:?}"
    );
    assert_eq!(
        after.last().map(String::as_str),
        Some("stable-late"),
        "case created after a listing: expected it appended, got {after:?}"
    );
}

/// RULE: the listing is `@paginated` on `nextToken` alone, so a token CONTINUES
/// the listing — every stored name reported exactly once across the pages, in
/// one stable order — and a token this service did not issue is refused with
/// `InvalidNextTokenException`.
#[tokio::test]
async fn listing_pages_without_repeating_or_restarting() {
    let emulator = Emulator::new();
    // More than one page's worth, counting the predefined names already stored,
    // so the listing cannot fit in a single page.
    let page_size = service_com_amazonaws_codedeploy_codedeploy_20141006::state::PAGE_SIZE;
    let created: Vec<String> = (0..page_size + 5)
        .map(|index| format!("paged-{index:03}"))
        .collect();
    for name in &created {
        create(&emulator, name).await;
    }

    let mut token: Option<String> = None;
    let mut first_token: Option<String> = None;
    let mut collected: Vec<String> = Vec::new();
    let mut pages = 0;
    loop {
        let mut request = json!({});
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let page = emulator.ok("ListDeploymentConfigs", request).await;
        let names = config_names(&page);
        assert!(
            names.len() <= page_size,
            "case page {pages}: expected at most {page_size} names on a page, got {}",
            names.len()
        );
        collected.extend(names);
        pages += 1;
        token = page["nextToken"].as_str().map(str::to_string);
        if first_token.is_none() {
            first_token = token.clone();
        }
        if token.is_none() {
            break;
        }
        assert!(
            pages <= 10,
            "case {} configurations: paging did not terminate after {pages} pages",
            created.len()
        );
    }
    assert!(
        pages > 1,
        "case {} configurations: expected more than one page, got {pages}",
        created.len()
    );

    let mut expected: BTreeSet<String> = PREDEFINED.iter().map(|name| (*name).to_string()).collect();
    expected.extend(created.iter().cloned());
    assert_eq!(
        collected.len(),
        expected.len(),
        "case paged listing: expected every name exactly once, got {} names",
        collected.len()
    );
    assert_eq!(
        collected.iter().cloned().collect::<BTreeSet<String>>(),
        expected,
        "case paged listing: expected every stored name across the pages, got {} names",
        collected.len()
    );

    // A token the service issued continues the listing; anything else is not a
    // page of it.
    let issued = first_token.expect("the first page of a multi-page listing issues a token");
    let continued = emulator
        .ok("ListDeploymentConfigs", json!({"nextToken": issued}))
        .await;
    assert_eq!(
        config_names(&continued),
        collected[page_size..],
        "case an issued token: expected the listing continued after the first page, got \
         {continued:?}"
    );

    let refusals: &[(&str, Value)] = &[
        ("not a token at all", json!("not-a-token")),
        ("the token prefix alone", json!("p")),
        ("a token with a malformed payload", json!("pzzz-")),
        ("an empty token", json!("")),
        (
            "a well-formed token naming a configuration no listing carries",
            // The encoding the service issues, over a name that was never
            // created: `p` + the name's bytes as hex.
            json!(format!("p{}", hex::encode("never-created"))),
        ),
    ];
    for (case, bogus) in refusals {
        let (status, body) = emulator
            .call("ListDeploymentConfigs", json!({"nextToken": bogus}))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidNextTokenException"),
            "case {case}: expected 400 InvalidNextTokenException, got {status} {body:?}"
        );
    }
}
