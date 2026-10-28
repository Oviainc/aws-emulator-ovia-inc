// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the LIFECYCLE a device registration lives through:
//! `CreatePlatformEndpoint` against the endpoint records `DeleteEndpoint` removes and
//! `GetEndpointAttributes` / `ListEndpointsByPlatformApplication` report.
//!
//! The rule under test: the ARN a registration answers is READ from the endpoint record
//! the account holds for that (application, device token) pair, so the idempotency of
//! the call is a property of that live record and not of the device token — while the
//! record is there the same token answers its ARN, and once the record is gone the same
//! token registers afresh under a newly minted ARN that is never one already handed out,
//! carrying the attributes of the NEW registration (so the attribute conflict a live
//! record raises is gone with it), while the deleted ARN resolves nowhere and every
//! other device's record is left exactly as it was.
//!
//! The rows differ along what the rule generalises over: the push platform (including
//! the one whose endpoints carry a channel and a user of their own), the device token's
//! form and length, the attribute set and user data each registration of one token
//! carries (identical to the previous one, or differing in the delivery flag, the user
//! data, the channel attributes, or carrying no map at all), how many
//! register/delete cycles one device token goes through, and the region the request
//! addresses.
//!
//! Every call goes over the wire through the router the emulator serves in production,
//! so nothing but persisted state carries information between calls.

use std::collections::HashMap;

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::{
    attribute_flag, CHANNEL_ENDPOINT_ATTRIBUTE_NAMES, ENDPOINT_CUSTOM_USER_DATA_ATTRIBUTE,
    ENDPOINT_DOES_NOT_EXIST, ENDPOINT_ENABLED_ATTRIBUTE, ENDPOINT_TOKEN_ATTRIBUTE,
    PLATFORM_WITH_CHANNEL_ENDPOINTS,
};

use super::create_platform_application::{attribute_params, registered_application};
use super::topics_subscriptions_publish::{attribute_map, Answer, Sns};

/// The region the harness signs its calls for unless a row names another one.
const REGION: &str = "us-east-1";

/// Owned attribute entries from a row's borrowed pairs.
fn pairs(attributes: &[(&str, &str)]) -> Vec<(String, String)> {
    attributes
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

/// One registration of one device token: the user data and attribute map it carries.
struct Stage {
    case: &'static str,
    custom_user_data: Option<&'static str>,
    attributes: Vec<(String, String)>,
}

/// One device token's whole life: the application it registers against, and the
/// successive registrations it is put through, each one separated from the next by the
/// delete of the record the one before it minted.
struct Row {
    case: &'static str,
    region: &'static str,
    platform: &'static str,
    application: &'static str,
    token: &'static str,
    stages: Vec<Stage>,
}

/// The rows: one device token per push platform, differing in the token's form, the
/// region, and what each successive registration of that token carries.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an Apple device re-registered with other user data",
            region: REGION,
            platform: "APNS",
            application: "lifecycle-apns",
            token: "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90",
            stages: vec![
                Stage {
                    case: "the first registration, with user data",
                    custom_user_data: Some("cart-watcher"),
                    attributes: Vec::new(),
                },
                Stage {
                    case: "re-registered with other user data",
                    custom_user_data: Some("price-watcher"),
                    attributes: Vec::new(),
                },
            ],
        },
        Row {
            case: "a Firebase device re-registered identically, twice over",
            region: REGION,
            platform: "GCM",
            application: "lifecycle-gcm",
            token: "cZ:APA91bF_lifecycle-device-token_0123456789-abcdefghijklmnopqrstuvwxyz",
            stages: vec![
                Stage {
                    case: "the first registration",
                    custom_user_data: None,
                    attributes: pairs(&[(ENDPOINT_ENABLED_ATTRIBUTE, "true")]),
                },
                Stage {
                    case: "the identical registration after the delete",
                    custom_user_data: None,
                    attributes: pairs(&[(ENDPOINT_ENABLED_ATTRIBUTE, "true")]),
                },
                Stage {
                    case: "the identical registration after the second delete",
                    custom_user_data: None,
                    attributes: pairs(&[(ENDPOINT_ENABLED_ATTRIBUTE, "true")]),
                },
            ],
        },
        Row {
            case: "an Amazon device re-registered with the delivery flag turned off",
            region: REGION,
            platform: "ADM",
            application: "lifecycle-adm",
            token: "amzn1.adm-registration.v3.shortish-token",
            stages: vec![
                Stage {
                    case: "the first registration, with no map at all",
                    custom_user_data: None,
                    attributes: Vec::new(),
                },
                Stage {
                    case: "re-registered disabled",
                    custom_user_data: None,
                    attributes: pairs(&[(ENDPOINT_ENABLED_ATTRIBUTE, "false")]),
                },
            ],
        },
        Row {
            case: "a Baidu device re-registered on another channel and user",
            region: REGION,
            platform: PLATFORM_WITH_CHANNEL_ENDPOINTS,
            application: "lifecycle-baidu",
            token: "channel-9876543210",
            stages: vec![
                Stage {
                    case: "the first registration, carrying its channel and user",
                    custom_user_data: None,
                    attributes: pairs(&[
                        (CHANNEL_ENDPOINT_ATTRIBUTE_NAMES[0], "9876543210"),
                        (CHANNEL_ENDPOINT_ATTRIBUTE_NAMES[1], "user-42"),
                    ]),
                },
                Stage {
                    case: "re-registered for another user of that channel",
                    custom_user_data: Some("{\"tier\":\"gold\"}"),
                    attributes: pairs(&[
                        (CHANNEL_ENDPOINT_ATTRIBUTE_NAMES[0], "9876543210"),
                        (CHANNEL_ENDPOINT_ATTRIBUTE_NAMES[1], "user-77"),
                    ]),
                },
            ],
        },
        Row {
            case: "a Windows device of another region, re-registered with user data",
            region: "eu-west-1",
            platform: "WNS",
            application: "lifecycle-wns",
            token: "https://db5.notify.windows.com/?token=AgYAAAD",
            stages: vec![
                Stage {
                    case: "the first registration, with no user data",
                    custom_user_data: None,
                    attributes: Vec::new(),
                },
                Stage {
                    case: "re-registered with empty user data",
                    custom_user_data: Some(""),
                    attributes: Vec::new(),
                },
            ],
        },
    ]
}

/// One `CreatePlatformEndpoint` call signed for `region`.
async fn register(
    sns: &Sns,
    region: &str,
    application_arn: &str,
    token: &str,
    stage: &Stage,
) -> Answer {
    let mut params: Vec<(String, String)> = vec![
        (
            "PlatformApplicationArn".to_string(),
            application_arn.to_string(),
        ),
        ("Token".to_string(), token.to_string()),
    ];
    if let Some(data) = stage.custom_user_data {
        params.push(("CustomUserData".to_string(), data.to_string()));
    }
    params.extend(attribute_params(&stage.attributes));
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    sns.call_in_region(region, "CreatePlatformEndpoint", &borrowed)
        .await
}

/// One registration asserted to have been SERVED, with the endpoint ARN it answered.
async fn registered(
    sns: &Sns,
    case: &str,
    region: &str,
    application_arn: &str,
    token: &str,
    stage: &Stage,
) -> String {
    let answer = register(sns, region, application_arn, token, stage).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer
        .element("EndpointArn")
        .unwrap_or_else(|| panic!("case {case:?}: no EndpointArn in {}", answer.body))
}

/// The attributes `GetEndpointAttributes` reports for `arn`, signed for `region`.
async fn read(sns: &Sns, region: &str, arn: &str) -> Answer {
    sns.call_in_region(region, "GetEndpointAttributes", &[("EndpointArn", arn)])
        .await
}

/// Remove the endpoint `arn` names, asserting the delete was served.
async fn delete(sns: &Sns, case: &str, region: &str, arn: &str) {
    let answer = sns
        .call_in_region(region, "DeleteEndpoint", &[("EndpointArn", arn)])
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected the delete to answer 200, got {} / {}",
        answer.status, answer.body
    );
}

/// Every endpoint ARN `ListEndpointsByPlatformApplication` reports for `application`.
async fn listed(sns: &Sns, case: &str, region: &str, application: &str) -> Vec<String> {
    let answer = sns
        .call_in_region(
            region,
            "ListEndpointsByPlatformApplication",
            &[("PlatformApplicationArn", application)],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected the listing to answer 200, got {} / {}",
        answer.status, answer.body
    );
    answer.elements("EndpointArn")
}

/// How many endpoint records this account currently holds.
fn held(sns: &Sns) -> usize {
    sns.state().lock().platform_endpoints.len()
}

/// The attributes the endpoint a stage registered must report: what the stage carried,
/// the device token off the record, and the delivery flag an endpoint nobody disabled
/// has.
fn expected_attributes(stage: &Stage, token: &str) -> HashMap<String, String> {
    let mut expected: HashMap<String, String> = stage.attributes.iter().cloned().collect();
    if let Some(data) = stage.custom_user_data {
        expected.insert(
            ENDPOINT_CUSTOM_USER_DATA_ATTRIBUTE.to_string(),
            data.to_string(),
        );
    }
    expected.insert(ENDPOINT_TOKEN_ATTRIBUTE.to_string(), token.to_string());
    expected
        .entry(ENDPOINT_ENABLED_ATTRIBUTE.to_string())
        .or_insert_with(|| attribute_flag(true));
    expected
}

/// Assert the endpoint `arn` named resolves nowhere: the read of a record this account
/// no longer holds is the lookup miss, and the application lists it no more.
async fn assert_gone(sns: &Sns, case: &str, region: &str, application: &str, arn: &str) {
    let answer = read(sns, region, arn).await;
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected the deleted endpoint to read as 404, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.code(),
        "NotFound",
        "case {case:?}: expected the NotFound code, got {}",
        answer.body
    );
    assert_eq!(
        answer.element("Message").as_deref(),
        Some(ENDPOINT_DOES_NOT_EXIST),
        "case {case:?}: expected the endpoint miss sentence, got {}",
        answer.body
    );
    let reported = listed(sns, case, region, application).await;
    assert!(
        !reported.contains(&arn.to_string()),
        "case {case:?}: expected the deleted endpoint to be listed no more, got {reported:?}"
    );
}

/// A device token registers afresh once the record it addressed is gone: a NEW ARN,
/// never one already handed out, holding the attributes of the new registration — and
/// while the record IS there, the same registration answers its ARN rather than minting
/// a second endpoint for one device.
#[tokio::test]
async fn a_deleted_device_registers_afresh_under_a_new_endpoint_arn() {
    let sns = Sns::new();
    let mut ever_minted: Vec<String> = Vec::new();
    let table = rows();
    let stages: usize = table.iter().map(|row| row.stages.len()).sum();

    for row in table {
        let application =
            registered_application(&sns, row.region, row.application, row.platform).await;
        let before = held(&sns);

        for stage in &row.stages {
            let case = format!("{} — {}", row.case, stage.case);
            let arn = registered(
                &sns,
                &case,
                row.region,
                &application,
                row.token,
                stage,
            )
            .await;
            assert!(
                !ever_minted.contains(&arn),
                "case {case:?}: expected an endpoint ARN never handed out before, got {arn:?} \
                 among {ever_minted:?}"
            );
            assert_eq!(
                held(&sns),
                before + 1,
                "case {case:?}: expected the registration to persist exactly one endpoint"
            );

            // While that record is there it IS the answer: the same registration
            // addresses it rather than minting a second endpoint for one device.
            let again = registered(
                &sns,
                &format!("{case} — repeated"),
                row.region,
                &application,
                row.token,
                stage,
            )
            .await;
            assert_eq!(
                again, arn,
                "case {case:?}: expected the ARN the device already has, got {again:?}"
            );
            assert_eq!(
                held(&sns),
                before + 1,
                "case {case:?}: a repeated registration must persist nothing new"
            );

            // The record holds what THIS registration carried, not what the registration
            // before the delete did.
            let reported = attribute_map(&read(&sns, row.region, &arn).await);
            let expected = expected_attributes(stage, row.token);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected {expected:?}, got {reported:?}"
            );
            assert_eq!(
                listed(&sns, &case, row.region, &application).await,
                vec![arn.clone()],
                "case {case:?}: expected the application to list its one endpoint"
            );

            delete(&sns, &case, row.region, &arn).await;
            assert_eq!(
                held(&sns),
                before,
                "case {case:?}: expected the delete to remove the record"
            );
            assert_gone(&sns, &case, row.region, &application, &arn).await;
            ever_minted.push(arn);
        }
    }

    assert_eq!(
        held(&sns),
        0,
        "every endpoint this suite registered was deleted again, got {} left",
        held(&sns)
    );
    assert_eq!(
        ever_minted.len(),
        stages,
        "expected one distinct minted ARN per registration of the table, got {ever_minted:?}"
    );
}

/// The idempotency is per RECORD: deleting one device's endpoint re-mints that device's
/// ARN and leaves every other device of the same application answering the ARN it
/// already had.
#[tokio::test]
async fn deleting_one_device_leaves_the_other_devices_untouched() {
    let sns = Sns::new();
    let application = registered_application(&sns, REGION, "lifecycle-neighbours", "GCM").await;
    let stage = Stage {
        case: "a plain registration",
        custom_user_data: Some("shelf-1"),
        attributes: pairs(&[(ENDPOINT_ENABLED_ATTRIBUTE, "true")]),
    };

    let tokens = [
        "cZ:APA91bF_neighbour-token-one",
        "cZ:APA91bF_neighbour-token-two",
        "cZ:APA91bF_neighbour-token-three",
    ];
    let mut arns: Vec<String> = Vec::new();
    for token in tokens {
        arns.push(registered(&sns, token, REGION, &application, token, &stage).await);
    }
    assert_eq!(
        held(&sns),
        tokens.len(),
        "expected one endpoint per device token"
    );

    // The middle device's record goes away; the two beside it do not.
    let dropped = arns[1].clone();
    delete(&sns, "the middle device", REGION, &dropped).await;
    assert_gone(&sns, "the middle device", REGION, &application, &dropped).await;

    for (index, token) in tokens.iter().enumerate() {
        let case = format!("device {index} after the middle one was deleted");
        let answered = registered(&sns, &case, REGION, &application, token, &stage).await;
        if index == 1 {
            assert_ne!(
                answered, dropped,
                "case {case:?}: expected a newly minted ARN, got the deleted {dropped:?}"
            );
            assert!(
                !arns.contains(&answered),
                "case {case:?}: expected an ARN never handed out before, got {answered:?}"
            );
        } else {
            assert_eq!(
                answered, arns[index],
                "case {case:?}: expected the ARN that device already has ({:?}), got {answered:?}",
                arns[index]
            );
        }
        let reported = attribute_map(&read(&sns, REGION, &answered).await);
        let expected = expected_attributes(&stage, token);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    assert_eq!(
        held(&sns),
        tokens.len(),
        "expected one endpoint per device token once the deleted one registered again"
    );
}
