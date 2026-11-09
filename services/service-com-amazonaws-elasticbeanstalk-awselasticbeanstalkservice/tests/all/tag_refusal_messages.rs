// SPDX-License-Identifier: CC0-1.0

//! How the two tag operations WORD the requests they refuse, over this service's
//! awsQuery surface.
//!
//! RULE 1 (a `ResourceArn` that is not this service's): both tag operations
//! document `ResourceArn` as "the ARN of an Elastic Beanstalk resource", so a
//! value that is any other service's ARN — or no ARN at all — is refused with one
//! request-derived sentence naming the value it was given, the same sentence
//! whichever of the two operations was called.
//!
//! RULE 2 (a tag key in both lists): `UpdateTagsForResource` refuses a key named
//! in both `TagsToAdd` and `TagsToRemove` with WHOLLY FIXED text — the
//! contradiction is a property of the request's shape, so the sentence carries no
//! key, no list and no resource.
//!
//! The rows span what the rules generalise over: for rule 1 the shape of the
//! value (another service's ARN in this account, another service's ARN in another
//! account, another partition, a value that is no ARN at all, an empty resource
//! segment) and which operation reads it; for rule 2 the key that is contested,
//! where it sits in each list, and which resource type is addressed.

use crate::lifecycle::{account, Emulator, TEST_REGION};

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

/// The application the rule 2 cases tag.
const APPLICATION: &str = "tag-message-app";

/// One value under test for rule 1.
struct ArnCase {
    case: &'static str,
    resource_arn: String,
}

fn arn_cases() -> Vec<ArnCase> {
    vec![
        ArnCase {
            case: "another service's ARN in this account",
            resource_arn: format!("arn:aws:sns:{TEST_REGION}:{}:not-eb", account()),
        },
        ArnCase {
            case: "another service's ARN in another account",
            resource_arn: format!("arn:aws:sqs:{TEST_REGION}:123456789012:queue"),
        },
        ArnCase {
            case: "an ARN of a service whose name merely starts the same",
            resource_arn: format!(
                "arn:aws:elasticbeanstalkplus:{TEST_REGION}:{}:application/app",
                account()
            ),
        },
        ArnCase {
            case: "a value that is no ARN at all",
            resource_arn: "not-an-arn".to_string(),
        },
        ArnCase {
            case: "an ARN whose service segment is empty",
            resource_arn: format!("arn:aws::{TEST_REGION}:{}:application/app", account()),
        },
    ]
}

/// RULE 1, over both operations that take a `ResourceArn`.
#[tokio::test]
async fn a_foreign_resource_arn_is_refused_with_one_request_derived_sentence() {
    let emulator = Emulator::new();

    for ArnCase { case, resource_arn } in arn_cases() {
        let expected = format!(
            "The name '{resource_arn}' isn't a valid ARN of an Elastic Beanstalk resource."
        );
        // Stated by the service in exactly one place, so the two operations
        // cannot word it differently.
        assert_eq!(
            eb::state::invalid_resource_arn_message(&resource_arn),
            expected,
            "case {case:?}: the service's own template must be AWS's sentence"
        );

        let read = emulator
            .call(
                "ListTagsForResource",
                &[("ResourceArn", resource_arn.as_str())],
            )
            .await;
        let write = emulator
            .call(
                "UpdateTagsForResource",
                &[
                    ("ResourceArn", resource_arn.as_str()),
                    ("TagsToAdd.member.1.Key", "env"),
                    ("TagsToAdd.member.1.Value", "prod"),
                ],
            )
            .await;
        for (operation, answer) in [("ListTagsForResource", &read), ("UpdateTagsForResource", &write)] {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400 from {operation}, got {} and body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "InvalidParameterValue",
                "case {case:?}: unexpected error code from {operation} in {}",
                answer.body
            );
            assert_eq!(
                answer.error_message(),
                expected,
                "case {case:?}: expected {expected:?} from {operation}, got {:?}",
                answer.error_message()
            );
        }
    }
}

/// One row of rule 2: which key is contested, and the two lists as the request
/// carries them.
struct BothListsCase {
    case: &'static str,
    add: &'static [(&'static str, &'static str)],
    remove: &'static [&'static str],
}

fn both_lists_cases() -> Vec<BothListsCase> {
    vec![
        BothListsCase {
            case: "the only key of each list",
            add: &[("env", "staging")],
            remove: &["env"],
        },
        BothListsCase {
            case: "the contested key is the last entry of each list",
            add: &[("cost-centre", "42"), ("contested", "value")],
            remove: &["team", "contested"],
        },
        BothListsCase {
            case: "a key carrying characters a message would have to escape",
            add: &[("a&b<c>", "value")],
            remove: &["a&b<c>"],
        },
        BothListsCase {
            case: "a key at the modelled key length bound",
            add: &[("kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk", "value")],
            remove: &["kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk"],
        },
    ]
}

/// RULE 2: one fixed sentence, whatever the contested key and whatever is tagged.
///
/// AWS's own sentence, spelled out here rather than taken from the service's own
/// constant: it carries nothing of the request, so the whole of it is fixed text.
const BOTH_LISTS_SENTENCE: &str =
    "A tag with the same key can't be added and removed in a single operation.";

#[tokio::test]
async fn a_tag_key_in_both_lists_is_refused_with_one_fixed_sentence() {
    let emulator = Emulator::new();
    let created = emulator
        .call("CreateApplication", &[("ApplicationName", APPLICATION)])
        .await;
    assert_eq!(
        created.status, 200,
        "case \"setup\": expected 200 from CreateApplication, got {} and body {}",
        created.status, created.body
    );
    let version = emulator
        .call(
            "CreateApplicationVersion",
            &[
                ("ApplicationName", APPLICATION),
                ("VersionLabel", "v1"),
            ],
        )
        .await;
    assert_eq!(
        version.status, 200,
        "case \"setup\": expected 200 from CreateApplicationVersion, got {} and body {}",
        version.status, version.body
    );

    // The two resource types this suite addresses, so the sentence is asserted to
    // be the same for both.
    let resources = [
        (
            "an application",
            format!(
                "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:application/{APPLICATION}",
                account()
            ),
        ),
        (
            "an application version",
            format!(
                "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:applicationversion/{APPLICATION}/v1",
                account()
            ),
        ),
    ];

    for BothListsCase { case, add, remove } in both_lists_cases() {
        for (kind, resource_arn) in &resources {
            let mut params: Vec<(String, String)> =
                vec![("ResourceArn".to_string(), resource_arn.clone())];
            for (index, (key, value)) in add.iter().enumerate() {
                let position = index + 1;
                params.push((
                    format!("TagsToAdd.member.{position}.Key"),
                    (*key).to_string(),
                ));
                params.push((
                    format!("TagsToAdd.member.{position}.Value"),
                    (*value).to_string(),
                ));
            }
            for (index, key) in remove.iter().enumerate() {
                params.push((
                    format!("TagsToRemove.member.{}", index + 1),
                    (*key).to_string(),
                ));
            }
            let borrowed: Vec<(&str, &str)> = params
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect();
            let refused = emulator.call("UpdateTagsForResource", &borrowed).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?} on {kind}: expected HTTP 400, got {} and body {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                "InvalidParameterValue",
                "case {case:?} on {kind}: unexpected error code in {}",
                refused.body
            );
            assert_eq!(
                refused.error_message(),
                BOTH_LISTS_SENTENCE,
                "case {case:?} on {kind}: expected {BOTH_LISTS_SENTENCE:?}, got {:?}",
                refused.error_message()
            );
            // Stated by the service in exactly one place, which is that same
            // sentence.
            assert_eq!(
                eb::state::TAG_KEY_IN_BOTH_LISTS_MESSAGE, BOTH_LISTS_SENTENCE,
                "case {case:?} on {kind}: the service's own constant must be AWS's sentence"
            );
            // The sentence carries nothing of the request: not the contested key,
            // not the resource it was asked about.
            for (key, _) in add {
                assert!(
                    !refused.error_message().contains(key),
                    "case {case:?} on {kind}: the fixed sentence must not quote the key {key:?}, \
                     got {:?}",
                    refused.error_message()
                );
            }
            assert!(
                !refused.error_message().contains(resource_arn.as_str()),
                "case {case:?} on {kind}: the fixed sentence must not quote the resource, got {:?}",
                refused.error_message()
            );
        }
    }
}
