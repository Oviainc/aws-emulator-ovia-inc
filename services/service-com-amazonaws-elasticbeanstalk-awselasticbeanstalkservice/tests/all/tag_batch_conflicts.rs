// SPDX-License-Identifier: CC0-1.0

//! One `UpdateTagsForResource` call cannot both set and remove a tag key.
//!
//! RULE: the operation takes two lists — "`TagsToAdd`: A list of tags to add or
//! update" and "`TagsToRemove`: A list of tag keys to remove" — so a key named
//! in BOTH asks the one call to set it and to drop it. That request is
//! contradictory whatever the resource holds, so it is refused and NOTHING is
//! applied; the refusal is on the request, not on the stored tag set, and it is
//! the same for every taggable resource type. Keys are compared exactly, so two
//! keys differing in case are two keys and such a batch is applicable, and two
//! SEPARATE calls may of course set a key and then remove it.
//!
//! The tag round trip, the `ResourceArn` refusals and the per-resource tag quota
//! are covered by `resource_tags.rs` and `environment_operations.rs`; this suite
//! is only the both-lists rule, read back through `ListTagsForResource` so what
//! is asserted is what the store holds rather than what the update answered.
//!
//! The HTTP harness is the one `lifecycle.rs` owns and the other suites in this
//! test target drive the service through, so this suite adds no second copy.

use std::collections::BTreeMap;

use crate::lifecycle::{account, Emulator, SOLUTION_STACK, TEST_REGION};

/// The application every case is created under.
const APPLICATION: &str = "tagged-app";
/// The application version the version cases tag.
const VERSION_LABEL: &str = "v1";
/// The environment the environment cases tag.
const ENVIRONMENT: &str = "tagged-env";

/// Which resource a case tags: the three types this store holds a record — and
/// therefore a tag list — for (`grep -n 'pub tags' src/state.rs`).
#[derive(Debug, Clone, Copy)]
enum ResourceKind {
    Application,
    ApplicationVersion,
    Environment,
}

impl ResourceKind {
    /// The ARN of this case's resource, in the form the service's own
    /// `application_arn` / `application_version_arn` / `environment_arn` build.
    fn arn(self) -> String {
        let resource = match self {
            Self::Application => format!("application/{APPLICATION}"),
            Self::ApplicationVersion => {
                format!("applicationversion/{APPLICATION}/{VERSION_LABEL}")
            }
            Self::Environment => format!("environment/{APPLICATION}/{ENVIRONMENT}"),
        };
        format!(
            "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:{resource}",
            account()
        )
    }
}

/// Create the resource `kind` names, seeded with `initial` tags, and report its
/// ARN. Every resource is created through the service's own create operation, so
/// the tag list an update edits is the one a create seeded. One resource per
/// emulator, so the application a case creates is always new.
async fn seed(emulator: &Emulator, kind: ResourceKind, initial: &[(&str, &str)]) -> String {
    let seeded = match kind {
        ResourceKind::Application => tag_params("Tags", initial),
        _ => Vec::new(),
    };
    let mut params = vec![("ApplicationName".to_string(), APPLICATION.to_string())];
    params.extend(seeded);
    expect_ok(emulator, "CreateApplication", &params, "seed").await;

    match kind {
        ResourceKind::Application => {}
        ResourceKind::ApplicationVersion => {
            let mut params = vec![
                ("ApplicationName".to_string(), APPLICATION.to_string()),
                ("VersionLabel".to_string(), VERSION_LABEL.to_string()),
            ];
            params.extend(tag_params("Tags", initial));
            expect_ok(emulator, "CreateApplicationVersion", &params, "seed").await;
        }
        ResourceKind::Environment => {
            let mut params = vec![
                ("ApplicationName".to_string(), APPLICATION.to_string()),
                ("EnvironmentName".to_string(), ENVIRONMENT.to_string()),
                // `CreateEnvironment` refuses a request that names no configuration
                // template, no solution stack and no platform version, so the fixture
                // names one from the service's own published catalogue.
                ("SolutionStackName".to_string(), SOLUTION_STACK.to_string()),
            ];
            params.extend(tag_params("Tags", initial));
            expect_ok(emulator, "CreateEnvironment", &params, "seed").await;
        }
    }
    kind.arn()
}

/// Call `action` and require a 200.
async fn expect_ok(emulator: &Emulator, action: &str, params: &[(String, String)], case: &str) {
    let answer = emulator.call(action, &borrowed(params)).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from {action}, got {} and body {}",
        answer.status, answer.body
    );
}

/// The awsQuery form parameters for a `TagList` member named `list`:
/// `<list>.member.<n>.Key` / `.Value`, the unflattened list form this service's
/// query protocol reads.
fn tag_params(list: &str, tags: &[(&str, &str)]) -> Vec<(String, String)> {
    tags.iter()
        .enumerate()
        .flat_map(|(index, (key, value))| {
            let position = index + 1;
            [
                (format!("{list}.member.{position}.Key"), (*key).to_string()),
                (
                    format!("{list}.member.{position}.Value"),
                    (*value).to_string(),
                ),
            ]
        })
        .collect()
}

/// The awsQuery form parameters for a `TagKeyList` member named `list`.
fn key_params(list: &str, keys: &[&str]) -> Vec<(String, String)> {
    keys.iter()
        .enumerate()
        .map(|(index, key)| (format!("{list}.member.{}", index + 1), (*key).to_string()))
        .collect()
}

/// The parameters of one `UpdateTagsForResource` call.
fn update_params(arn: &str, add: &[(&str, &str)], remove: &[&str]) -> Vec<(String, String)> {
    let mut params = vec![("ResourceArn".to_string(), arn.to_string())];
    params.extend(tag_params("TagsToAdd", add));
    params.extend(key_params("TagsToRemove", remove));
    params
}

fn borrowed(params: &[(String, String)]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// The tags `ListTagsForResource` reports for `arn`, as a key -> value map so a
/// comparison does not depend on the order the list happens to be stored in.
async fn listed_tags(emulator: &Emulator, arn: &str, case: &str) -> BTreeMap<String, String> {
    let listed = emulator
        .call("ListTagsForResource", &[("ResourceArn", arn)])
        .await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected 200 from ListTagsForResource, got {} and body {}",
        listed.status, listed.body
    );
    let keys = listed.values_at("ResourceTags/member/Key");
    let values = listed.values_at("ResourceTags/member/Value");
    assert_eq!(
        keys.len(),
        values.len(),
        "case {case:?}: every reported tag carries a key and a value, got {}",
        listed.body
    );
    keys.into_iter()
        .map(str::to_string)
        .zip(values.into_iter().map(str::to_string))
        .collect()
}

fn expected_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// One row: a resource seeded with `initial`, one `UpdateTagsForResource` call
/// of `add` / `remove`, the error code it must answer (or `None` when the batch
/// is applicable) and the tags the resource must hold afterwards.
struct Case {
    name: &'static str,
    kind: ResourceKind,
    initial: &'static [(&'static str, &'static str)],
    add: &'static [(&'static str, &'static str)],
    remove: &'static [&'static str],
    refusal: Option<&'static str>,
    expected: &'static [(&'static str, &'static str)],
}

#[tokio::test]
async fn a_tag_key_in_both_lists_is_refused_and_nothing_is_applied() {
    // Rows differ along what the rule generalises over: which resource type is
    // tagged, how many keys each list names, where the shared key sits in each
    // list, whether it is one the resource already carries, and whether the keys
    // match exactly at all.
    let cases = [
        Case {
            name: "the one key is in both lists",
            kind: ResourceKind::Application,
            initial: &[("env", "prod")],
            add: &[("env", "staging")],
            remove: &["env"],
            refusal: Some("InvalidParameterValue"),
            expected: &[("env", "prod")],
        },
        Case {
            name: "the shared key is the last entry of each list, so no change at all is applied",
            kind: ResourceKind::ApplicationVersion,
            initial: &[("env", "prod"), ("team", "platform")],
            add: &[("cost-centre", "42"), ("env", "staging")],
            remove: &["team", "env"],
            refusal: Some("InvalidParameterValue"),
            expected: &[("env", "prod"), ("team", "platform")],
        },
        Case {
            // The contradiction is in the REQUEST, so it is refused even where
            // the removal would have been a documented silent no-op.
            name: "the shared key is one the resource does not carry",
            kind: ResourceKind::Environment,
            initial: &[("env", "prod")],
            add: &[("owner", "platform")],
            remove: &["owner"],
            refusal: Some("InvalidParameterValue"),
            expected: &[("env", "prod")],
        },
        Case {
            // A tag key is case sensitive, so these are two different keys and
            // the batch is applicable: the removal matches nothing.
            name: "the two lists name keys differing only in case",
            kind: ResourceKind::Application,
            initial: &[("Env", "prod")],
            add: &[("Env", "staging")],
            remove: &["env"],
            refusal: None,
            expected: &[("Env", "staging")],
        },
        Case {
            name: "adds and removes that share no key at all",
            kind: ResourceKind::Environment,
            initial: &[("env", "prod"), ("team", "platform")],
            add: &[("owner", "platform"), ("env", "staging")],
            remove: &["team", "absent"],
            refusal: None,
            expected: &[("env", "staging"), ("owner", "platform")],
        },
    ];

    for case in &cases {
        let name = case.name;
        let emulator = Emulator::new();
        let arn = seed(&emulator, case.kind, case.initial).await;
        assert_eq!(
            listed_tags(&emulator, &arn, name).await,
            expected_map(case.initial),
            "case {name:?}: the tags the create seeded must come back from the store"
        );

        let answer = emulator
            .call(
                "UpdateTagsForResource",
                &borrowed(&update_params(&arn, case.add, case.remove)),
            )
            .await;
        match case.refusal {
            Some(code) => {
                assert_eq!(
                    answer.status, 400,
                    "case {name:?}: expected HTTP 400, got {} and body {}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    code,
                    "case {name:?}: expected error code {code}, got body {}",
                    answer.body
                );
            }
            None => assert_eq!(
                answer.status, 200,
                "case {name:?}: expected 200, got {} and body {}",
                answer.status, answer.body
            ),
        }

        let read_back = listed_tags(&emulator, &arn, name).await;
        assert_eq!(
            read_back,
            expected_map(case.expected),
            "case {name:?}: expected {:?} in the store after the call, got {read_back:?}",
            expected_map(case.expected)
        );
    }
}

#[tokio::test]
async fn two_separate_calls_may_set_a_key_and_then_remove_it() {
    // The refusal above is about ONE call naming a key twice over, not about the
    // key: setting it and dropping it in sequence is two well-formed requests.
    let case = "set then remove in two calls";
    let emulator = Emulator::new();
    let arn = seed(&emulator, ResourceKind::Application, &[]).await;

    expect_ok(
        &emulator,
        "UpdateTagsForResource",
        &update_params(&arn, &[("env", "prod")], &[]),
        case,
    )
    .await;
    assert_eq!(
        listed_tags(&emulator, &arn, case).await,
        expected_map(&[("env", "prod")]),
        "case {case:?}: the first call must store the tag"
    );

    expect_ok(
        &emulator,
        "UpdateTagsForResource",
        &update_params(&arn, &[], &["env"]),
        case,
    )
    .await;
    assert!(
        listed_tags(&emulator, &arn, case).await.is_empty(),
        "case {case:?}: the second call must drop it"
    );
}
