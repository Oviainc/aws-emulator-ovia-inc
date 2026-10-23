// SPDX-License-Identifier: CC0-1.0

//! The registry's replication configuration, end to end over the wire.
//!
//! Every case goes through the router, so it exercises routing, deserialization, the
//! handler, the persisted state and serialization — the same path a client takes.
//!
//! The rule under test is the registry's own: `DescribeRegistry` reports this registry's
//! id together with the replication configuration exactly as the last
//! `PutReplicationConfiguration` persisted it — an empty rule list for a registry nobody
//! has configured — and a put is refused when a destination names the source registry
//! itself, that is this account's registry in the Region the request was made against.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call_in_region, emulator, ok_in_region, REGISTRY_ID};

/// Another account's registry: a well-formed `RegistryId` (pattern `^[0-9]{12}$`) that is
/// not this registry, so a destination naming it is cross-account replication.
const FOREIGN_REGISTRY_ID: &str = "111122223333";

/// The modelled ceiling on `ReplicationConfiguration.rules`:
/// `ReplicationRuleList` is `@length(min: 0, max: 25)`
/// (`smithy_query(query="shape",shape="ReplicationRuleList")`).
const MAX_REPLICATION_RULES: usize = 25;

/// Region codes this suite replicates to, from `emulator_telemetry::policy::KNOWN_REGIONS`
/// ("every AWS region the wire may carry"), the catalog a replication destination's
/// Region must appear in. Enough of them to fill the modelled maximum rule count, one
/// Region per rule.
const DESTINATION_REGIONS: [&str; MAX_REPLICATION_RULES] = [
    "af-south-1",
    "ap-east-1",
    "ap-east-2",
    "ap-northeast-1",
    "ap-northeast-2",
    "ap-northeast-3",
    "ap-south-1",
    "ap-south-2",
    "ap-southeast-1",
    "ap-southeast-2",
    "ap-southeast-3",
    "ap-southeast-4",
    "ap-southeast-5",
    "ap-southeast-6",
    "ap-southeast-7",
    "ca-central-1",
    "ca-west-1",
    "eu-central-1",
    "eu-central-2",
    "eu-north-1",
    "eu-south-1",
    "eu-south-2",
    "eu-west-2",
    "eu-west-3",
    "il-central-1",
];

/// The `replicationConfiguration` `DescribeRegistry` reports, having checked that the
/// registry it reports is this one.
async fn described_configuration(router: &MultiServiceRouter, case: &str, region: &str) -> Value {
    let body = ok_in_region(router, case, region, "DescribeRegistry", &json!({})).await;
    assert_eq!(
        body["registryId"],
        json!(REGISTRY_ID),
        "case {case:?}: DescribeRegistry must report this registry's id, got {body:?}"
    );
    body["replicationConfiguration"].clone()
}

/// One replication rule: the destinations it replicates to, and nothing else.
fn rule(destinations: Value) -> Value {
    json!({"destinations": destinations})
}

/// One destination: a registry id and the Region it is in.
fn destination(registry_id: &str, region: &str) -> Value {
    json!({"region": region, "registryId": registry_id})
}

/// `DescribeRegistry` reports whatever the last put persisted, whatever shape that
/// configuration has: no rules, one rule, several, filters or none, up to the modelled
/// maximum — and the report is stable across reads.
#[tokio::test]
async fn describe_registry_reports_the_configuration_each_put_persisted() {
    let router = emulator();
    let region = "eu-west-1";

    // Before any put: a registry nobody has configured has an EMPTY rule list, not an
    // absent configuration.
    assert_eq!(
        described_configuration(&router, "unconfigured registry", region).await,
        json!({"rules": []}),
        "an unconfigured registry reports an empty rule list"
    );

    // The rows differ along what the rule generalises over: the number of rules, the
    // number of destinations in a rule, whether a rule carries repository filters, and
    // whether a destination is in this account or another.
    let many_rules: Vec<Value> = DESTINATION_REGIONS
        .iter()
        .map(|region| rule(json!([destination(REGISTRY_ID, region)])))
        .collect();

    let cases: Vec<(&str, Value)> = vec![
        (
            "one rule, one destination in this account",
            json!([rule(json!([destination(REGISTRY_ID, "us-east-2")]))]),
        ),
        (
            "one rule, two destinations",
            json!([rule(json!([
                destination(REGISTRY_ID, "us-east-2"),
                destination(FOREIGN_REGISTRY_ID, "ap-south-1"),
            ]))]),
        ),
        (
            "a cross-account destination in the source Region",
            json!([rule(json!([destination(FOREIGN_REGISTRY_ID, "eu-west-1")]))]),
        ),
        (
            "a rule with a prefix filter",
            json!([{
                "destinations": [destination(REGISTRY_ID, "us-west-2")],
                "repositoryFilters": [{"filter": "team-a", "filterType": "PREFIX_MATCH"}],
            }]),
        ),
        (
            "two rules, one filtered and one not",
            json!([
                {
                    "destinations": [destination(REGISTRY_ID, "us-west-2")],
                    "repositoryFilters": [
                        {"filter": "team-a", "filterType": "PREFIX_MATCH"},
                        {"filter": "team-b/service", "filterType": "PREFIX_MATCH"},
                    ],
                },
                rule(json!([destination(FOREIGN_REGISTRY_ID, "sa-east-1")])),
            ]),
        ),
        (
            "the modelled maximum of 25 rules",
            Value::Array(many_rules),
        ),
        // "Creates or updates": a put carrying no rules replaces the configuration, so
        // this is how replication is turned off again.
        ("back to no rules", json!([])),
    ];

    for (case, rules) in cases {
        let expected = json!({"rules": rules});
        let put = ok_in_region(
            &router,
            case,
            region,
            "PutReplicationConfiguration",
            &json!({"replicationConfiguration": expected}),
        )
        .await;
        assert_eq!(
            put["replicationConfiguration"], expected,
            "case {case:?}: the put must answer with the configuration it stored, got {put:?}"
        );

        let described = described_configuration(&router, case, region).await;
        assert_eq!(
            described, expected,
            "case {case:?}: DescribeRegistry must report the stored configuration, got \
             {described:?}"
        );

        // A report is not a mutation: reading twice reports the same thing.
        let again = described_configuration(&router, case, region).await;
        assert_eq!(
            again, described,
            "case {case:?}: a second DescribeRegistry must report the same configuration, \
             got {again:?}"
        );
    }
}

/// A destination is another registry: this account's registry in a different Region
/// (cross-Region), or another account's registry in any Region (cross-account). A
/// destination naming this account's registry in the Region the call was made against
/// names the source itself, and is refused.
#[tokio::test]
async fn a_destination_naming_the_source_registry_is_refused() {
    let router = emulator();

    // (case, the Region the call is made against, the rules, whether it is accepted)
    let cases: Vec<(&str, &str, Value, bool)> = vec![
        (
            "this account's registry in another Region is cross-Region replication",
            "eu-west-1",
            json!([rule(json!([destination(REGISTRY_ID, "us-east-2")]))]),
            true,
        ),
        (
            "another account's registry in the call's Region is cross-account replication",
            "eu-west-1",
            json!([rule(json!([destination(FOREIGN_REGISTRY_ID, "eu-west-1")]))]),
            true,
        ),
        (
            "this account's registry in the call's Region is the source itself",
            "eu-west-1",
            json!([rule(json!([destination(REGISTRY_ID, "eu-west-1")]))]),
            false,
        ),
        (
            "the source appears in the second of two rules",
            "eu-west-1",
            json!([
                rule(json!([destination(REGISTRY_ID, "us-east-2")])),
                rule(json!([destination(REGISTRY_ID, "eu-west-1")])),
            ]),
            false,
        ),
        (
            "the source appears beside another destination in one rule",
            "eu-west-1",
            json!([rule(json!([
                destination(FOREIGN_REGISTRY_ID, "ap-south-1"),
                destination(REGISTRY_ID, "eu-west-1"),
            ]))]),
            false,
        ),
        (
            "the same destination is the source when the call is made against it",
            "us-east-1",
            json!([rule(json!([destination(REGISTRY_ID, "us-east-1")]))]),
            false,
        ),
        (
            "and is a destination when the call is made against another Region",
            "ap-northeast-1",
            json!([rule(json!([destination(REGISTRY_ID, "us-east-1")]))]),
            true,
        ),
    ];

    // What the registry holds right now: only an accepted put changes it.
    let mut stored = json!({"rules": []});

    for (case, region, rules, accepted) in cases {
        let configuration = json!({"rules": rules});
        let answer = call_in_region(
            &router,
            region,
            "PutReplicationConfiguration",
            &json!({"replicationConfiguration": configuration}),
        )
        .await;

        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            stored = configuration;
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "InvalidParameterException",
                "case {case:?}: expected InvalidParameterException, got {:?}",
                answer.body
            );
        }

        // Whatever the verdict, the stored configuration is the last ACCEPTED one: a
        // refused put persists nothing.
        let described = described_configuration(&router, case, region).await;
        assert_eq!(
            described, stored,
            "case {case:?}: expected the registry to still hold {stored:?}, got {described:?}"
        );
    }
}

/// The modelled bound on the rule list is enforced: 25 rules are accepted, 26 are
/// refused at the boundary, and the refusal stores nothing.
#[tokio::test]
async fn a_rule_list_past_the_modelled_maximum_is_refused() {
    let router = emulator();
    let region = "eu-west-1";

    let rules = |count: usize| -> Value {
        Value::Array(
            (0..count)
                .map(|index| {
                    // Cycling the catalog, so a count past the number of Regions listed
                    // still produces that many rules: the bound under test is on the
                    // rule COUNT, and two rules may name one destination.
                    let region = DESTINATION_REGIONS[index % DESTINATION_REGIONS.len()];
                    rule(json!([destination(REGISTRY_ID, region)]))
                })
                .collect(),
        )
    };

    // (case, how many rules, whether the modelled `@length(max: 25)` admits it)
    for (case, count, accepted) in [
        ("one under the maximum", MAX_REPLICATION_RULES - 1, true),
        ("exactly the maximum", MAX_REPLICATION_RULES, true),
        ("one past the maximum", MAX_REPLICATION_RULES + 1, false),
    ] {
        let configuration = json!({"rules": rules(count)});
        let answer = call_in_region(
            &router,
            region,
            "PutReplicationConfiguration",
            &json!({"replicationConfiguration": configuration}),
        )
        .await;

        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200 for {count} rules, got {} with {:?}",
                answer.status, answer.body
            );
            let described = described_configuration(&router, case, region).await;
            assert_eq!(
                described, configuration,
                "case {case:?}: expected the {count} stored rules back, got {described:?}"
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400 for {count} rules, got {} with {:?}",
                answer.status, answer.body
            );
            // A refusal against a constraint the model declares is stated with the
            // client-input error this service models — `InvalidParameterException`,
            // which this operation declares — naming the member and the bound it
            // failed. See `override_refusal` in `src/service.rs`.
            assert_eq!(
                answer.error_code(),
                "InvalidParameterException",
                "case {case:?}: expected InvalidParameterException for {count} rules, got {:?}",
                answer.body
            );
            // The refused put left the last accepted configuration in place.
            let described = described_configuration(&router, case, region).await;
            assert_eq!(
                described["rules"].as_array().map(Vec::len),
                Some(MAX_REPLICATION_RULES),
                "case {case:?}: a refused put must not change the stored rules, got \
                 {described:?}"
            );
        }
    }
}
