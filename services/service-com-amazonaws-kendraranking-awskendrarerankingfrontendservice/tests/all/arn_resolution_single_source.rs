// SPDX-License-Identifier: CC0-1.0

//! THE RULE: a `ResourceARN` is turned into a stored rescore execution plan by ONE
//! mapping, so the half of the tagging surface that WRITES the plan it resolves
//! (`TagResource`, `UntagResource`) and the half that READS it (`ListTagsForResource`)
//! always reach the same plan — the plan whose ARN the caller named, in the region the
//! request was signed for — and agree on which ARNs resolve to no plan at all.
//!
//! The store used to spell that mapping out twice, once per half, so the two halves
//! could resolve an ARN differently. This suite drives one operation of each former
//! half over a TABLE of plans, so a write made through the shared mapping is the write
//! the read through it reports back; the rows differ in the region the plan lives in,
//! how many other plans share that region's bucket, and whether the plan was created
//! carrying tags.

use serde_json::{json, Value};

use crate::harness::{call_in, create_plan_in_with_tags, emulator, error_code, list_tags_in};

/// One row of the table: a plan addressed by its ARN, in `region`, with `neighbours`
/// other plans created in the same region before it (so the ARN resolves to one plan
/// out of several in that region's bucket) and the tags its create carried.
struct Addressed {
    case: &'static str,
    region: &'static str,
    neighbours: usize,
    created_with: Value,
}

/// The plans the suite addresses. The rows differ along what the mapping generalises
/// over — the region the ARN resolves in (the emulator's default region, a non-default
/// one, and a region of another partition), how many plans that region holds beside the
/// addressed one, and how many tags the addressed plan already carries.
fn addressed_plans() -> Vec<Addressed> {
    vec![
        Addressed {
            case: "the only plan of the default region, created without tags",
            region: "us-east-1",
            neighbours: 0,
            created_with: Value::Null,
        },
        Addressed {
            case: "one plan of three in a non-default region, created with a tag",
            region: "eu-west-1",
            neighbours: 2,
            created_with: json!([{ "Key": "team", "Value": "search" }]),
        },
        Addressed {
            case: "one plan of four in the default region, created without tags",
            region: "us-east-1",
            neighbours: 3,
            created_with: Value::Null,
        },
        Addressed {
            case: "a plan of a region in another partition, created with two tags",
            region: "us-gov-west-1",
            neighbours: 1,
            created_with: json!([
                { "Key": "env", "Value": "prod" },
                { "Key": "owner", "Value": "ranking" },
            ]),
        },
    ]
}

/// The `(Key, Value)` pairs of a `Tags` request member, as a listing answers them.
fn tag_pairs(tags: &Value) -> Vec<(String, String)> {
    tags.as_array()
        .map(|list| {
            list.iter()
                .map(|tag| {
                    (
                        tag["Key"].as_str().unwrap_or_default().to_string(),
                        tag["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The three ARN-addressed operations, with a request body naming `arn`: the two of the
/// writing half first, then the one of the reading half.
fn arn_addressed_calls(arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": [{ "Key": "stage", "Value": "beta" }] }),
        ),
        (
            "UntagResource",
            json!({ "ResourceARN": arn, "TagKeys": ["stage"] }),
        ),
        ("ListTagsForResource", json!({ "ResourceARN": arn })),
    ]
}

#[tokio::test]
async fn a_write_and_a_read_addressed_by_the_same_arn_reach_the_same_plan() {
    for row in addressed_plans() {
        let router = emulator();
        let case = row.case;
        let region = row.region;

        // The other plans of this region, created FIRST so the addressed plan is not the
        // only entry of its region's bucket: a lookup that resolved an ARN to the wrong
        // plan would show up as a tag landing on one of these.
        let mut neighbours: Vec<(String, String)> = Vec::new();
        for n in 0..row.neighbours {
            neighbours.push(
                create_plan_in_with_tags(&router, region, &format!("neighbour-{n}"), Value::Null)
                    .await,
            );
        }
        let (_, arn) =
            create_plan_in_with_tags(&router, region, "addressed", row.created_with.clone()).await;

        // What the addressed plan carries before any tagging call: whatever its create
        // was given, read back through the reading half of the surface.
        let initial = tag_pairs(&row.created_with);
        let listed = list_tags_in(&router, region, &arn).await;
        assert_eq!(
            listed, initial,
            "case {case:?}: the read half must report the tags the create gave the plan, \
             expected {initial:?}, got {listed:?}"
        );

        // The WRITING half, addressed by the same ARN: it must resolve to the plan the
        // reading half just answered for.
        let (status, answer) = call_in(
            &router,
            region,
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": [{ "Key": "stage", "Value": "beta" }] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: tagging {arn} in {region}: expected 200, got {status} {answer}"
        );

        let mut expected = initial.clone();
        expected.push(("stage".to_string(), "beta".to_string()));
        expected.sort();
        let mut listed = list_tags_in(&router, region, &arn).await;
        listed.sort();
        assert_eq!(
            listed, expected,
            "case {case:?}: the read half must see the write the same ARN made, \
             expected {expected:?}, got {listed:?}"
        );

        // And the write landed on NO other plan of the region: the ARN resolves to one
        // plan, so every neighbour still carries the empty tag set it was created with.
        for (neighbour_id, neighbour_arn) in &neighbours {
            let listed = list_tags_in(&router, region, neighbour_arn).await;
            assert!(
                listed.is_empty(),
                "case {case:?}: plan {neighbour_id} was not the one addressed, so it must \
                 still carry no tags, got {listed:?}"
            );
        }

        // The other operation of the writing half, through the same mapping: it removes
        // the tag the first write added, and the reading half is back at the plan's
        // original set.
        let (status, answer) = call_in(
            &router,
            region,
            "UntagResource",
            json!({ "ResourceARN": arn, "TagKeys": ["stage"] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: untagging {arn} in {region}: expected 200, got {status} {answer}"
        );
        let listed = list_tags_in(&router, region, &arn).await;
        assert_eq!(
            listed, initial,
            "case {case:?}: the read half must see the removal the same ARN made, \
             expected {initial:?}, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn both_halves_agree_on_the_arns_that_resolve_to_no_plan() {
    for row in addressed_plans() {
        let router = emulator();
        let case = row.case;
        let region = row.region;
        let (id, arn) =
            create_plan_in_with_tags(&router, region, "addressed", row.created_with.clone()).await;

        // ARNs that resolve to nothing in `region`, by FORM: an id this store never
        // minted, the plan's ARN rewritten to another region, another service's ARN, and
        // a string that is not an ARN at all. Both halves answer each of them the same
        // way, because both resolve an ARN through the one mapping.
        let unknown = arn.replace(&id, "33333333-3333-4333-8333-333333333333");
        let elsewhere_region = if region == "ca-central-1" {
            "sa-east-1"
        } else {
            "ca-central-1"
        };
        let elsewhere = arn.replace(region, elsewhere_region);
        let other_service = arn.replace("kendra-ranking", "kendra");
        let forms: Vec<(&str, &str)> = vec![
            ("an id this store never minted", unknown.as_str()),
            ("the plan's ARN under another region", elsewhere.as_str()),
            ("another service's ARN", other_service.as_str()),
            ("a string that is not an ARN", "not-an-arn"),
        ];

        for (form, unresolvable) in forms {
            for (operation, body) in arn_addressed_calls(unresolvable) {
                let (status, answer) = call_in(&router, region, operation, body).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}, {form:?} on {operation}: expected 400, got {status} {answer}"
                );
                let code = error_code(&answer);
                assert_eq!(
                    code, "ResourceUnavailableException",
                    "case {case:?}, {form:?} on {operation}: expected \
                     ResourceUnavailableException, got {code:?} in {answer}"
                );
            }
        }

        // The ARN that DOES resolve is served by both halves, so the agreement above is
        // not both halves refusing everything.
        for (operation, body) in arn_addressed_calls(&arn) {
            let (status, answer) = call_in(&router, region, operation, body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {operation} on the ARN that resolves: expected 200, \
                 got {status} {answer}"
            );
        }

        // The refused calls wrote nothing, and the served pair above added and removed
        // the same key: the plan is back at the tags its create gave it.
        let initial = tag_pairs(&row.created_with);
        let listed = list_tags_in(&router, region, &arn).await;
        assert_eq!(
            listed, initial,
            "case {case:?}: the refused calls must leave the plan's tags as they were, \
             expected {initial:?}, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn the_same_arn_resolves_in_one_region_for_both_halves() {
    // The region's bucket is the first step of the mapping, and each half now takes it
    // through one accessor: a plan's ARN presented to another region resolves to nothing
    // for the writing half exactly as it does for the reading half.
    let router = emulator();
    let home = "eu-central-1";
    let (_, home_arn) = create_plan_in_with_tags(
        &router,
        home,
        "home",
        json!([{ "Key": "team", "Value": "search" }]),
    )
    .await;

    for away in ["us-east-1", "ap-southeast-2", "us-gov-west-1"] {
        for (operation, body) in arn_addressed_calls(&home_arn) {
            let (status, answer) = call_in(&router, away, operation, body).await;
            assert_eq!(
                status, 400,
                "{operation} on a plan of {home}, signed for {away}: expected 400, \
                 got {status} {answer}"
            );
            let code = error_code(&answer);
            assert_eq!(
                code, "ResourceUnavailableException",
                "{operation} signed for {away}: expected ResourceUnavailableException, \
                 got {code:?} in {answer}"
            );
        }
    }

    // None of those calls reached the plan: its home region still reports exactly the
    // tag its create gave it.
    let listed = list_tags_in(&router, home, &home_arn).await;
    let expected = tag_pairs(&json!([{ "Key": "team", "Value": "search" }]));
    assert_eq!(
        listed, expected,
        "a cross-region tagging call must not touch the plan, expected {expected:?}, \
         got {listed:?}"
    );
}
