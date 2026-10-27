// SPDX-License-Identifier: CC0-1.0

//! `UpdateWebApp`: what an update assigns to a stored web app, and what a second,
//! concurrent update gets.
//!
//! Two rules:
//!
//! 1. An update "assigns new properties to a web app" — the members it carries, and only
//!    those: every member it names is what `DescribeWebApp` then reports, every member it
//!    omits is left exactly as it stood, and a web app the store does not hold is refused
//!    with `ResourceNotFoundException`.
//! 2. A web app applies ONE update at a time, so of two writers that overlap exactly one
//!    wins: the loser is refused with the modelled `ConflictException` (409) rather than
//!    silently overwriting the winner, and the winner's values are what the web app then
//!    reports.
//!
//! The rows of the first rule differ along which members the update carries, how many of
//! them it carries at once, and the size of each (the access endpoint's URL, the unit
//! count, the role, the subnet list), over web apps that differ in whether their endpoint
//! is VPC-hosted.

use serde_json::{json, Value};

use crate::harness::{member_of, Emulator, PAST_WEB_APP_UPDATE_SPAN, ROLE};

/// An IAM Identity Center instance ARN satisfying `IdentityCenterInstanceArn`'s modelled
/// pattern `^arn:[\w-]+:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$`.
const INSTANCE_ARN: &str = "arn:aws:sso:::instance/ssoins-1234567890abcdef";

/// A `WebAppId` satisfying the modelled `^webapp-[0-9a-f]{17}$` that no create minted.
const NEVER_MINTED_WEB_APP: &str = "webapp-0000000000000dead";

/// Create one web app and hand back its id.
async fn create_web_app(emulator: &Emulator, case: &str, request: Value) -> String {
    let answer = emulator.call("CreateWebApp", request).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateWebApp 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .get("WebAppId")
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a WebAppId, got {}", answer.body))
        .to_string()
}

/// The `DescribedWebApp` of one stored web app.
async fn described(emulator: &Emulator, case: &str, web_app_id: &str) -> Value {
    let answer = emulator
        .call("DescribeWebApp", json!({"WebAppId": web_app_id}))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected DescribeWebApp 200, got {} with body {}",
        answer.status, answer.body
    );
    answer.get("WebApp").clone()
}

/// RULE 1 (see the module docs): an update assigns the members it carries and leaves the
/// rest, and `DescribeWebApp` reports the result.
#[tokio::test]
async fn an_update_assigns_the_members_it_carries() {
    let emulator = Emulator::new();

    // Two web apps, so the rows cover both endpoint kinds: a VPC-hosted one has subnets
    // an update can re-point, a public one does not.
    let public_app = create_web_app(
        &emulator,
        "a public web app",
        json!({
            "IdentityProviderDetails": {
                "IdentityCenterConfig": {"InstanceArn": INSTANCE_ARN, "Role": ROLE}
            },
            "WebAppUnits": {"Provisioned": 1},
        }),
    )
    .await;
    let vpc_app = create_web_app(
        &emulator,
        "a VPC-hosted web app",
        json!({
            "IdentityProviderDetails": {
                "IdentityCenterConfig": {"InstanceArn": INSTANCE_ARN, "Role": ROLE}
            },
            "EndpointDetails": {"Vpc": {
                "VpcId": "vpc-0123456789abcdef0",
                "SubnetIds": ["subnet-0123456789abcdef0"],
            }},
        }),
    )
    .await;

    // (case, which web app, the update's members beyond WebAppId, what the describe must
    // then report)
    let rows: Vec<(&str, &String, Value, Value)> = vec![
        (
            "a custom access endpoint alone",
            &public_app,
            json!({"AccessEndpoint": "https://files.example.com"}),
            json!({"AccessEndpoint": "https://files.example.com"}),
        ),
        (
            "the unit count alone",
            &public_app,
            json!({"WebAppUnits": {"Provisioned": 4}}),
            json!({"WebAppUnits": {"Provisioned": 4}, "AccessEndpoint": "https://files.example.com"}),
        ),
        (
            "the access endpoint and the unit count at once",
            &public_app,
            json!({"AccessEndpoint": "https://portal.example.com/sftp",
                   "WebAppUnits": {"Provisioned": 50}}),
            json!({"AccessEndpoint": "https://portal.example.com/sftp",
                   "WebAppUnits": {"Provisioned": 50}}),
        ),
        (
            "an update carrying nothing but the id",
            &public_app,
            json!({}),
            json!({"AccessEndpoint": "https://portal.example.com/sftp",
                   "WebAppUnits": {"Provisioned": 50}}),
        ),
        (
            "the identity provider's role",
            &vpc_app,
            json!({"IdentityProviderDetails": {"IdentityCenterConfig": {
                "Role": "arn:aws:iam::000000000000:role/transfer-webapp"
            }}}),
            json!({}),
        ),
        (
            "a longer subnet list on the VPC endpoint",
            &vpc_app,
            json!({"EndpointDetails": {"Vpc": {
                "SubnetIds": ["subnet-0aaaaaaaaaaaaaaaa", "subnet-0bbbbbbbbbbbbbbbb"]
            }}}),
            json!({}),
        ),
    ];

    for (case, web_app_id, members, expected) in rows {
        let mut request = json!({"WebAppId": web_app_id});
        for (member, value) in members.as_object().expect("the row's members are an object") {
            request[member] = value.clone();
        }
        let answer = emulator.call("UpdateWebApp", request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected UpdateWebApp 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.get("WebAppId").as_str(),
            Some(web_app_id.as_str()),
            "case {case:?}: expected the update to answer the web app it changed, got {}",
            answer.body
        );

        let record = described(&emulator, case, web_app_id).await;
        for (member, value) in expected.as_object().expect("the row's expectation is an object") {
            assert_eq!(
                member_of(&record, member),
                value,
                "case {case:?}: expected {member} to be {value}, got {record}"
            );
        }
        // A member the update named and the describe projects differently is checked
        // against the stored record directly.
        if let Some(role) = members
            .get("IdentityProviderDetails")
            .and_then(|details| details.get("IdentityCenterConfig"))
            .and_then(|config| config.get("Role"))
        {
            let reported = member_of(
                member_of(&record, "DescribedIdentityProviderDetails"),
                "IdentityCenterConfig",
            )
            .clone();
            assert_eq!(
                member_of(&reported, "Role"),
                role,
                "case {case:?}: expected the assigned role reported back, got {reported}"
            );
            // The instance and the application registered in it are the web app's own,
            // from its create: an update of the role leaves them.
            assert_eq!(
                member_of(&reported, "InstanceArn").as_str(),
                Some(INSTANCE_ARN),
                "case {case:?}: expected the create's own InstanceArn to survive the update, \
                 got {reported}"
            );
            assert!(
                member_of(&reported, "ApplicationArn").is_string(),
                "case {case:?}: expected the registered application to survive the update, \
                 got {reported}"
            );
        }
        if let Some(subnets) = members
            .get("EndpointDetails")
            .and_then(|details| details.get("Vpc"))
            .and_then(|vpc| vpc.get("SubnetIds"))
        {
            let reported =
                member_of(member_of(&record, "DescribedEndpointDetails"), "Vpc").clone();
            assert_eq!(
                member_of(&reported, "SubnetIds"),
                subnets,
                "case {case:?}: expected the assigned subnets reported back, got {reported}"
            );
            // The endpoint is the SAME endpoint: it keeps the id the create minted and
            // the VPC it was placed in.
            assert!(
                member_of(&reported, "VpcEndpointId").is_string(),
                "case {case:?}: expected the endpoint to keep its own id, got {reported}"
            );
            assert_eq!(
                member_of(&reported, "VpcId").as_str(),
                Some("vpc-0123456789abcdef0"),
                "case {case:?}: expected the endpoint to keep its VPC, got {reported}"
            );
        }

        // Past the roll-out window, so the next row is not read as racing this one.
        tokio::time::sleep(PAST_WEB_APP_UPDATE_SPAN).await;
    }
}

/// RULE: a web app the store does not hold is refused, and so is a VPC-endpoint update of
/// a web app that has no VPC endpoint — in both cases without changing anything.
#[tokio::test]
async fn an_update_that_addresses_nothing_is_refused() {
    let emulator = Emulator::new();
    let public_app = create_web_app(
        &emulator,
        "a public web app",
        json!({
            "IdentityProviderDetails": {
                "IdentityCenterConfig": {"InstanceArn": INSTANCE_ARN, "Role": ROLE}
            },
        }),
    )
    .await;
    let before = described(&emulator, "before any refusal", &public_app).await;

    // (case, request, expected status, expected error code)
    let rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a web app no create minted",
            json!({"WebAppId": NEVER_MINTED_WEB_APP, "AccessEndpoint": "https://a.example.com"}),
            400,
            "ResourceNotFoundException",
        ),
        (
            "another web app no create minted",
            json!({"WebAppId": "webapp-fffffffffffffffff"}),
            400,
            "ResourceNotFoundException",
        ),
        (
            "a VPC endpoint update of a public web app",
            json!({"WebAppId": &public_app,
                   "EndpointDetails": {"Vpc": {"SubnetIds": ["subnet-0123456789abcdef0"]}}}),
            400,
            "InvalidRequestException",
        ),
    ];

    for (case, request, status, code) in rows {
        let answer = emulator.call("UpdateWebApp", request).await;
        assert_eq!(
            (answer.status, answer.error_code().as_str()),
            (status, code),
            "case {case:?}: expected {status} {code}, got {} with body {}",
            answer.status,
            answer.body
        );
    }

    let after = described(&emulator, "after the refusals", &public_app).await;
    assert_eq!(
        before, after,
        "expected a refused update to change nothing, got {after} where it was {before}"
    );
}

/// RULE 2: of two updates that overlap, exactly one wins; the loser is refused with
/// `ConflictException` (409) and the winner's value is what the web app reports.
///
/// The two calls are polled CONCURRENTLY (`tokio::join!`), which is what two SDK clients
/// writing at once do; the rows then differ along how the second writer arrives — in
/// flight at the same moment, and sent while the first roll-out is still in progress —
/// because the rule is about the window, not about the thread.
#[tokio::test]
async fn concurrent_updates_have_exactly_one_winner() {
    let emulator = Emulator::new();
    let web_app_id = create_web_app(
        &emulator,
        "a web app two writers race on",
        json!({
            "IdentityProviderDetails": {
                "IdentityCenterConfig": {"InstanceArn": INSTANCE_ARN, "Role": ROLE}
            },
            "WebAppUnits": {"Provisioned": 1},
        }),
    )
    .await;

    let endpoints = [
        "https://racer-0.example.com",
        "https://racer-1.example.com",
    ];
    let (first, second) = tokio::join!(
        emulator.call(
            "UpdateWebApp",
            json!({"WebAppId": &web_app_id, "AccessEndpoint": endpoints[0]}),
        ),
        emulator.call(
            "UpdateWebApp",
            json!({"WebAppId": &web_app_id, "AccessEndpoint": endpoints[1]}),
        ),
    );
    let answers = [
        (first.status, first.error_code()),
        (second.status, second.error_code()),
    ];
    let winners: Vec<usize> = answers
        .iter()
        .enumerate()
        .filter(|(_, (status, _))| *status == 200)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        winners.len(),
        1,
        "case \"two writers at once\": expected exactly one winning UpdateWebApp call, \
         got {answers:?}"
    );
    for (index, (status, code)) in answers.iter().enumerate() {
        if winners.contains(&index) {
            continue;
        }
        assert_eq!(
            (*status, code.as_str()),
            (409, "ConflictException"),
            "case \"two writers at once\": expected the loser refused 409 ConflictException, \
             got {answers:?}"
        );
    }

    // The web app carries the winner's value, not the loser's.
    let record = described(&emulator, "two writers at once", &web_app_id).await;
    let expected = endpoints[winners[0]];
    assert_eq!(
        member_of(&record, "AccessEndpoint").as_str(),
        Some(expected),
        "case \"two writers at once\": expected the winner's access endpoint {expected:?}, \
         got {record}"
    );

    // A further writer that arrives while that roll-out is still in progress is refused
    // on the same rule, and changes nothing.
    let late = emulator
        .call(
            "UpdateWebApp",
            json!({"WebAppId": &web_app_id, "AccessEndpoint": "https://late.example.com"}),
        )
        .await;
    assert_eq!(
        (late.status, late.error_code().as_str()),
        (409, "ConflictException"),
        "case \"a writer inside the window\": expected 409 ConflictException, got {} with \
         body {}",
        late.status,
        late.body
    );
    let unchanged = described(&emulator, "a writer inside the window", &web_app_id).await;
    assert_eq!(
        member_of(&unchanged, "AccessEndpoint").as_str(),
        Some(expected),
        "case \"a writer inside the window\": expected the refused update to change nothing, \
         got {unchanged}"
    );

    // Once the roll-out completes, the next update is accepted again.
    tokio::time::sleep(PAST_WEB_APP_UPDATE_SPAN).await;
    let after = emulator
        .call(
            "UpdateWebApp",
            json!({"WebAppId": &web_app_id, "AccessEndpoint": "https://after.example.com"}),
        )
        .await;
    assert_eq!(
        after.status, 200,
        "case \"after the window\": expected the next update accepted, got {} with body {}",
        after.status, after.body
    );
    let settled = described(&emulator, "after the window", &web_app_id).await;
    assert_eq!(
        member_of(&settled, "AccessEndpoint").as_str(),
        Some("https://after.example.com"),
        "case \"after the window\": expected the accepted update applied, got {settled}"
    );
}
