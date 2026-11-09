// SPDX-License-Identifier: CC0-1.0

//! What the CAPACITY-PROVIDER surface reports about a provider and about an identifier it
//! cannot resolve, driven end to end over the shared wire harness in [`crate::ecs_wire`].
//!
//! The rules under test:
//!
//! * `CapacityProvider$type` names the class of capacity the provider binds onto, as a
//!   value of `CapacityProviderType` (`smithy_query(query="shape",
//!   shape="com.amazonaws.ecs#CapacityProviderType")` -> `["EC2_AUTOSCALING",
//!   "MANAGED_INSTANCES", "FARGATE", "FARGATE_SPOT"]`): the caller's own Auto Scaling
//!   group, the instances Amazon ECS manages for a cluster, or — for each provider Amazon
//!   ECS holds for every account — the Fargate class that provider is;
//! * `CapacityProvider$tags` is reported whatever the request asked for: the provider's own
//!   tags when `include` names `TAGS`, and the empty list otherwise, so a client reading the
//!   member never gets an absent value;
//! * an identifier a `DescribeCapacityProviders` cannot resolve fails BESIDE the results —
//!   200 with one `failures` entry naming the identifier as the request spelled it and the
//!   one sentence that states no provider answers to it — rather than failing the call;
//! * a page size above the page this read serves is served that page, and only a page size
//!   below one — which asks for no page at all — is refused; and
//! * `autoScalingGroupProvider` names a group of the CALLER's, so a create whose reference
//!   resolves to no Auto Scaling group of the account in the Region the request was signed
//!   for is refused with `ClientException` and stores nothing.
//!
//! Every case goes through the same front door a client uses — including the Auto Scaling
//! groups, which are created through that service's own API — so nothing reaches into a
//! state struct and a response that is not computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::capacity_providers::{group_arn, managed_instances};
use crate::ecs_wire::{
    create_auto_scaling_group, create_auto_scaling_group_in, create_cluster, list, ok, refusal,
    service, string, ACCOUNT, SIGNED_REGION,
};

/// The capacity providers Amazon ECS holds for every account, in every Region
/// (`CapacityProviderStrategyItem`: "the only valid capacity provider values are FARGATE or
/// FARGATE_SPOT"). Nothing creates them and nothing can take their names.
const BUILT_IN_PROVIDERS: [&str; 2] = ["FARGATE", "FARGATE_SPOT"];

/// The whole sentence a `failures` entry carries for a capacity-provider identifier that
/// names no provider of the account.
const DOES_NOT_EXIST: &str = "The specified capacity provider does not exist. Specify a \
                              valid name or ARN and try again.";

/// The whole sentence a create whose `autoScalingGroupProvider` names no Auto Scaling group
/// of the account is refused with.
const NO_SUCH_GROUP: &str = "The specified Auto Scaling group does not exist. Specify a \
                             valid Auto Scaling group and try again.";

/// The ARN of the capacity provider named `name` in the signed Region.
fn provider_arn(name: &str) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:capacity-provider/{name}")
}

/// Describe the providers `references` names, with or without their tags, and return the
/// whole answer.
async fn describe(
    router: &MultiServiceRouter,
    references: &[String],
    include_tags: bool,
    case: &str,
) -> Value {
    let mut request = json!({ "capacityProviders": references });
    if include_tags {
        request["include"] = json!(["TAGS"]);
    }
    ok(router, "DescribeCapacityProviders", request, case).await
}

/// One row of the `type`/`tags` table: a provider of one capacity class, how it gets into
/// the account, and what the surface must then report about it.
struct ClassCase {
    case: &'static str,
    /// The provider's name, which for a built-in one is not created at all.
    provider_name: &'static str,
    /// The Auto Scaling group the provider binds onto, if any.
    group: Option<&'static str>,
    /// The members `CreateCapacityProvider` carries beside `name`, or `None` for a provider
    /// nothing creates.
    request: Option<fn() -> Value>,
    /// The `CapacityProviderType` value the provider must report.
    expect_type: &'static str,
    /// The tags the provider must report when the request asks for them.
    expect_tags: fn() -> Value,
}

#[tokio::test]
async fn every_capacity_provider_reports_the_class_it_binds_onto_and_a_tag_list() {
    let router = service();
    create_cluster(&router, "classes", "setup").await;

    // The rows span every class of capacity a provider binds onto — the two Fargate
    // providers Amazon ECS holds for the account, one over a caller's Auto Scaling group,
    // and one over the instances Amazon ECS manages for a cluster — and, across them, a
    // provider created with tags and one created with none.
    let rows: [ClassCase; 5] = [
        ClassCase {
            case: "the Fargate provider Amazon ECS holds for the account",
            provider_name: "FARGATE",
            group: None,
            request: None,
            expect_type: "FARGATE",
            expect_tags: || json!([]),
        },
        ClassCase {
            case: "the Fargate Spot provider Amazon ECS holds for the account",
            provider_name: "FARGATE_SPOT",
            group: None,
            request: None,
            expect_type: "FARGATE_SPOT",
            expect_tags: || json!([]),
        },
        ClassCase {
            case: "a provider over a caller's Auto Scaling group, with tags",
            provider_name: "overgroup",
            group: Some("asg-overgroup"),
            request: Some(|| {
                json!({
                    "autoScalingGroupProvider": { "autoScalingGroupArn": "asg-overgroup" },
                    "tags": [{ "key": "team", "value": "platform" }],
                })
            }),
            expect_type: "EC2_AUTOSCALING",
            expect_tags: || json!([{ "key": "team", "value": "platform" }]),
        },
        ClassCase {
            case: "a provider over a caller's Auto Scaling group named by its ARN, untagged",
            provider_name: "byarnclass",
            group: Some("asg-byarnclass"),
            request: Some(|| {
                json!({
                    "autoScalingGroupProvider": {
                        "autoScalingGroupArn": group_arn("asg-byarnclass"),
                    },
                })
            }),
            expect_type: "EC2_AUTOSCALING",
            expect_tags: || json!([]),
        },
        ClassCase {
            case: "a provider over the instances Amazon ECS manages for a cluster",
            provider_name: "overmanaged",
            group: None,
            request: Some(|| {
                json!({
                    "cluster": "classes",
                    "managedInstancesProvider": managed_instances(),
                    "tags": [{ "key": "env", "value": "test" }],
                })
            }),
            expect_type: "MANAGED_INSTANCES",
            expect_tags: || json!([{ "key": "env", "value": "test" }]),
        },
    ];

    for row in &rows {
        let case = row.case;
        if let Some(group) = row.group {
            create_auto_scaling_group(&router, group, case).await;
        }
        if let Some(request) = row.request {
            let mut request = request();
            request["name"] = json!(row.provider_name);
            let created = ok(&router, "CreateCapacityProvider", request, case).await;
            let created = &created["capacityProvider"];
            // The create has no `include` member to narrow its answer with, so it reports
            // the class it stored and the tags it was given.
            assert_eq!(
                created["type"].as_str(),
                Some(row.expect_type),
                "case {case:?}: expected the create to report type {:?}, got {created}",
                row.expect_type
            );
            assert_eq!(
                created["tags"],
                (row.expect_tags)(),
                "case {case:?}: unexpected tags from the create, got {created}"
            );
        }

        // The same pair reads back, so the answer is the stored record rather than one the
        // create made up.
        let references = vec![row.provider_name.to_string()];
        for (include_tags, expected_tags) in
            [(true, (row.expect_tags)()), (false, json!([]))]
        {
            let body = describe(&router, &references, include_tags, case).await;
            let described = list(&body, "capacityProviders", case);
            assert_eq!(
                described.len(),
                1,
                "case {case:?}: expected exactly the named provider, got {body}"
            );
            let provider = &described[0];
            assert_eq!(
                provider["type"].as_str(),
                Some(row.expect_type),
                "case {case:?}: expected type {:?} read back, got {provider}",
                row.expect_type
            );
            assert_eq!(
                provider["tags"], expected_tags,
                "case {case:?}: expected tags {expected_tags} with include_tags \
                 {include_tags}, got {provider}"
            );
        }
    }

    // Every class a provider of this account reports is a value of the modelled set, so a
    // client switching on it never sees one outside it.
    let case = "every provider of the account";
    let body = ok(&router, "DescribeCapacityProviders", json!({}), case).await;
    for provider in list(&body, "capacityProviders", case) {
        let reported = string(&provider, "type", case);
        assert!(
            ["EC2_AUTOSCALING", "MANAGED_INSTANCES", "FARGATE", "FARGATE_SPOT"]
                .contains(&reported.as_str()),
            "case {case:?}: {reported:?} is not a CapacityProviderType, got {provider}"
        );
    }
}

#[tokio::test]
async fn an_unfiltered_describe_lists_the_account_s_providers_and_fails_nothing() {
    let router = service();

    // A call that names no provider and no cluster describes every provider the account
    // holds, which in a fresh account is exactly the two Amazon ECS holds for it. The rows
    // differ along the page size the request asks for: none at all, the smallest, the
    // largest page this read serves, and asks ABOVE that page — which are served the page
    // this read serves rather than refused.
    let rows: [(&str, Value); 5] = [
        ("no page size at all", json!({})),
        ("the smallest page", json!({ "maxResults": 1 })),
        ("the largest page this read serves", json!({ "maxResults": 10 })),
        ("one page above it", json!({ "maxResults": 11 })),
        ("far above it", json!({ "maxResults": 1_000 })),
    ];

    for (case, request) in &rows {
        let smallest = request["maxResults"].as_i64() == Some(1);
        let body = ok(&router, "DescribeCapacityProviders", request.clone(), case).await;
        let mut names: Vec<String> = list(&body, "capacityProviders", case)
            .iter()
            .map(|provider| string(provider, "name", case))
            .collect();
        if smallest {
            // One name per page, walked through to the end by the token.
            let mut token = body["nextToken"].as_str().map(str::to_string);
            while let Some(next) = token {
                let body = ok(
                    &router,
                    "DescribeCapacityProviders",
                    json!({ "maxResults": 1, "nextToken": next }),
                    case,
                )
                .await;
                names.extend(
                    list(&body, "capacityProviders", case)
                        .iter()
                        .map(|provider| string(provider, "name", case)),
                );
                token = body["nextToken"].as_str().map(str::to_string);
            }
        }
        names.sort();
        assert_eq!(
            names,
            BUILT_IN_PROVIDERS.map(str::to_string).to_vec(),
            "case {case:?}: expected the providers Amazon ECS holds for the account, got \
             {body}"
        );
        assert_eq!(
            body["failures"],
            json!([]),
            "case {case:?}: a call that names no identifier fails none, got {body}"
        );
        for provider in list(&body, "capacityProviders", case) {
            let name = string(&provider, "name", case);
            assert_eq!(
                (
                    provider["capacityProviderArn"].as_str(),
                    provider["status"].as_str(),
                    provider["type"].as_str(),
                    provider["tags"].clone(),
                ),
                (
                    Some(provider_arn(&name).as_str()),
                    Some("ACTIVE"),
                    Some(name.as_str()),
                    json!([]),
                ),
                "case {case:?}: unexpected projection of {name}, got {provider}"
            );
        }
    }

    // A page size below the smallest page asks for no page at all, which is the one value
    // this member cannot be given.
    let case = "no page at all";
    let (code, message) = refusal(
        &router,
        "DescribeCapacityProviders",
        json!({ "maxResults": 0 }),
        case,
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        ("InvalidParameterException", "Max results can not be lesser than 1"),
        "case {case:?}: expected the smallest page named, got {code:?} with {message:?}"
    );
}

/// One row of the unresolved-identifier table: the identifiers one call names, the
/// providers it must describe, and the identifiers it must report as naming none.
type FailureCase = (&'static str, Vec<String>, Vec<&'static str>, Vec<String>);

#[tokio::test]
async fn an_identifier_that_names_no_provider_is_reported_beside_the_results() {
    let router = service();
    create_auto_scaling_group(&router, "asg-known", "setup").await;
    ok(
        &router,
        "CreateCapacityProvider",
        json!({
            "name": "known",
            "autoScalingGroupProvider": { "autoScalingGroupArn": "asg-known" },
        }),
        "setup",
    )
    .await;

    // The rows differ along how the request spells the identifier that resolves to nothing
    // (a short name, a full ARN, a name that merely resembles a stored one), how many of
    // them one call carries, and whether a resolvable identifier is asked for beside them.
    // Every row's expectation is the SAME rule: one entry per unresolved identifier, naming
    // it as the request spelled it.
    let rows: Vec<FailureCase> = vec![
        (
            "one short name nothing holds",
            vec!["ghost".to_string()],
            vec![],
            vec!["ghost".to_string()],
        ),
        (
            "one full ARN nothing holds",
            vec![provider_arn("ghost")],
            vec![],
            vec![provider_arn("ghost")],
        ),
        (
            "a name that only resembles a stored one",
            vec!["known2".to_string()],
            vec![],
            vec!["known2".to_string()],
        ),
        (
            "a stored provider beside a name nothing holds",
            vec!["known".to_string(), "ghost".to_string()],
            vec!["known"],
            vec!["ghost".to_string()],
        ),
        (
            "a stored provider named by ARN beside two that name nothing",
            vec![
                provider_arn("known"),
                "ghost".to_string(),
                provider_arn("phantom"),
            ],
            vec!["known"],
            vec!["ghost".to_string(), provider_arn("phantom")],
        ),
    ];

    for (case, references, expected_names, expected_failures) in &rows {
        let body = describe(&router, references, true, case).await;
        let names: Vec<String> = list(&body, "capacityProviders", case)
            .iter()
            .map(|provider| string(provider, "name", case))
            .collect();
        assert_eq!(
            names, *expected_names,
            "case {case:?}: unexpected providers described, got {body}"
        );
        let failures: Vec<(String, String)> = list(&body, "failures", case)
            .iter()
            .map(|failure| {
                (
                    string(failure, "arn", case),
                    string(failure, "reason", case),
                )
            })
            .collect();
        let expected: Vec<(String, String)> = expected_failures
            .iter()
            .map(|reference| (reference.clone(), DOES_NOT_EXIST.to_string()))
            .collect();
        assert_eq!(
            failures, expected,
            "case {case:?}: expected one failure per unresolved identifier, got {body}"
        );
    }
}

/// One row of the Auto Scaling group table: how the create names the group, whether the
/// account holds it, and so whether the provider is created at all.
struct GroupCase {
    case: &'static str,
    provider_name: &'static str,
    /// The group to create first, and the Region to create it in.
    existing_group: Option<(&'static str, &'static str)>,
    /// The `autoScalingGroupArn` the create carries.
    reference: fn() -> String,
    accepted: bool,
}

#[tokio::test]
async fn a_provider_over_an_auto_scaling_group_the_account_does_not_hold_is_refused() {
    let router = service();

    // The rows differ along how the create spells the group (a bare name, a full Auto
    // Scaling group ARN), whether the account holds a group of that name, which Region holds
    // it, and whether the spelling of the create matches the spelling of the group's own
    // create — naming one group by ARN and by name names the same group.
    let rows: [GroupCase; 6] = [
        GroupCase {
            case: "a bare name the account holds",
            provider_name: "heldbyname",
            existing_group: Some(("asg-held", SIGNED_REGION)),
            reference: || "asg-held".to_string(),
            accepted: true,
        },
        GroupCase {
            case: "the ARN of a group the account created by name",
            provider_name: "heldbyarn",
            existing_group: Some(("asg-heldarn", SIGNED_REGION)),
            reference: || group_arn("asg-heldarn"),
            accepted: true,
        },
        GroupCase {
            case: "a bare name nothing holds",
            provider_name: "absentbyname",
            existing_group: None,
            reference: || "asg-absent".to_string(),
            accepted: false,
        },
        GroupCase {
            case: "the ARN of a group nothing holds",
            provider_name: "absentbyarn",
            existing_group: None,
            reference: || group_arn("asg-absentarn"),
            accepted: false,
        },
        GroupCase {
            case: "a group of another Region",
            provider_name: "elsewhere",
            existing_group: Some(("asg-elsewhere", "us-east-1")),
            reference: || "asg-elsewhere".to_string(),
            accepted: false,
        },
        GroupCase {
            case: "a name that only resembles a group the account holds",
            provider_name: "nearmiss",
            existing_group: Some(("asg-near", SIGNED_REGION)),
            reference: || "asg-near2".to_string(),
            accepted: false,
        },
    ];

    for row in &rows {
        let case = row.case;
        if let Some((group, region)) = row.existing_group {
            create_auto_scaling_group_in(&router, region, group, case).await;
        }
        let request = json!({
            "name": row.provider_name,
            "autoScalingGroupProvider": { "autoScalingGroupArn": (row.reference)() },
        });
        if row.accepted {
            let created = ok(&router, "CreateCapacityProvider", request, case).await;
            assert_eq!(
                created["capacityProvider"]["capacityProviderArn"].as_str(),
                Some(provider_arn(row.provider_name).as_str()),
                "case {case:?}: expected the provider created, got {created}"
            );
        } else {
            let (code, message) = refusal(&router, "CreateCapacityProvider", request, case).await;
            assert_eq!(
                (code.as_str(), message.as_str()),
                ("ClientException", NO_SUCH_GROUP),
                "case {case:?}: expected the group refusal, got {code:?} with {message:?}"
            );
        }

        // The refused create stored nothing, and the accepted one stored exactly the
        // provider it answered with: the read decides, not the create's answer.
        let body = describe(
            &router,
            &[row.provider_name.to_string()],
            false,
            case,
        )
        .await;
        let described: Vec<String> = list(&body, "capacityProviders", case)
            .iter()
            .map(|provider| string(provider, "name", case))
            .collect();
        let expected: Vec<String> = if row.accepted {
            vec![row.provider_name.to_string()]
        } else {
            vec![]
        };
        assert_eq!(
            described, expected,
            "case {case:?}: a provider exists exactly when its create was accepted, got \
             {body}"
        );
    }
}
