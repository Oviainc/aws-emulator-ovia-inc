// SPDX-License-Identifier: CC0-1.0

//! The tagging surface (`TagResource`, `UntagResource`, `ListTagsForResource`), driven
//! end-to-end over this service's awsJson1_0 wire (`POST /`, `X-Amz-Target:
//! AWSBCMPricingCalculator.<Operation>`) through the same `MultiServiceRouter` the
//! production front door uses.
//!
//! Each suite asserts a RULE over a table of inputs, not one recorded request: the rows
//! differ along whatever the rule generalises over (the kind of resource the ARN
//! addresses, the tags it already carries, the keys and values written, the key count,
//! which keys are removed, and the way an ARN can fail to name a live resource).
//!
//! The wire harness is the one `bill_scenario_lifecycle` already defines; a second copy
//! could drift from the front door that one exercises.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_bcmpricingcalculator_awsbcmpricingcalculator::{
    state, ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState as ServiceState,
};

use crate::bill_scenario_lifecycle::{error_code, Emulator};

/// The ARN of a resource of `kind` (an ARN resource-type segment, e.g.
/// `state::WORKLOAD_ESTIMATE_ARN_TYPE`) with this id, built the way a client holding one
/// of this service's ARNs sends it back: through `engine::emulator_arn`, whose account
/// segment is the single account this emulator presents, with the empty region segment a
/// partitional service's ARNs carry (`state::ARN_REGION`).
fn resource_arn(kind: &str, id: &str) -> String {
    resource_arn_in_region(kind, id, state::ARN_REGION)
}

/// The same ARN with `region` written into its region field — a form the `Arn` pattern
/// (`[-a-z0-9]*`) admits and which addresses the same resource, since this service's
/// resources are not region-scoped.
fn resource_arn_in_region(kind: &str, id: &str, region: &str) -> String {
    engine::emulator_arn(state::ARN_NAMESPACE, region, &format!("{kind}/{id}"))
}

/// Create a bill scenario and return its id, failing the test if the call did not
/// succeed.
async fn create_scenario(emulator: &Emulator, name: &str, tags: Option<Value>) -> String {
    let mut request = json!({ "name": name });
    if let Some(tags) = tags {
        request["tags"] = tags;
    }
    let (status, body) = emulator.call("CreateBillScenario", request).await;
    assert_eq!(
        status, 200,
        "CreateBillScenario({name}): expected 200, got {status} with {body}"
    );
    body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateBillScenario({name}): no id in {body}"))
        .to_string()
}

/// Create a workload estimate and return its id, failing the test if the call did not
/// succeed.
async fn create_estimate(emulator: &Emulator, name: &str, tags: Option<Value>) -> String {
    let mut request = json!({ "name": name });
    if let Some(tags) = tags {
        request["tags"] = tags;
    }
    let (status, body) = emulator.call("CreateWorkloadEstimate", request).await;
    assert_eq!(
        status, 200,
        "CreateWorkloadEstimate({name}): expected 200, got {status} with {body}"
    );
    body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateWorkloadEstimate({name}): no id in {body}"))
        .to_string()
}

/// The tags `ListTagsForResource` reports for `arn`, as an ordered map so two readings
/// compare regardless of the order the wire happened to carry them in.
async fn listed_tags(emulator: &Emulator, arn: &str, case: &str) -> BTreeMap<String, String> {
    let (status, body) = emulator
        .call("ListTagsForResource", json!({ "arn": arn }))
        .await;
    assert_eq!(
        status, 200,
        "case {case}: expected ListTagsForResource 200, got {status} with {body}"
    );
    body["tags"]
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: expected a tags map, got {body}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value.as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The tags the store holds for `arn`, read straight off the resource so a test can tell
/// "the reader renders it" from "the writer persisted it".
fn stored_tags(emulator: &Emulator, arn: &str, case: &str) -> BTreeMap<String, String> {
    let store = emulator.engine.state::<ServiceState>();
    let guard = store.lock();
    guard
        .resource_tags(arn)
        .unwrap_or_else(|| panic!("case {case}: the store holds no resource for {arn}"))
        .clone()
}

/// A `(key, value)` table as a JSON `Tags` map.
fn tags_json(pairs: &[(&str, &str)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect(),
    )
}

fn expected_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// One row of the tag-write table.
struct TagCase {
    /// The ARN resource-type segment of the resource being tagged.
    kind: &'static str,
    /// The tags the resource is created with.
    created_with: &'static [(&'static str, &'static str)],
    /// The successive `TagResource` requests applied to it.
    writes: &'static [&'static [(&'static str, &'static str)]],
    /// What the resource is expected to carry afterwards.
    expected: &'static [(&'static str, &'static str)],
}

/// The longest `ResourceTagKey` the model accepts (`len 1..128`, pattern
/// `^[\w\s:+=@/-]+$` — smithy_query: `shape="Tags"`).
const LONGEST_KEY: &str = "k2345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345678";

/// The longest `ResourceTagValue` the model accepts (`len 0..256`, pattern
/// `^[\w\s:+=@/-]*$`).
const LONGEST_VALUE: &str = "v234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456";

/// The model's own bounds for a tag key and value, asserted here so a boundary row that
/// drifted from the bound fails loudly instead of silently testing an interior value.
fn assert_boundary_literals() {
    assert_eq!(
        LONGEST_KEY.len(),
        128,
        "the boundary key row must be exactly `ResourceTagKey`'s maximum length, got {}",
        LONGEST_KEY.len()
    );
    assert_eq!(
        LONGEST_VALUE.len(),
        256,
        "the boundary value row must be exactly `ResourceTagValue`'s maximum length, got {}",
        LONGEST_VALUE.len()
    );
}

/// RULE: `TagResource` MERGES the tags it is given into those the addressed resource
/// already carries — a key the request names takes the request's value, a key it does not
/// name keeps the value it had — and `ListTagsForResource` reports exactly the tags the
/// store then holds for that resource.
///
/// The table spans the kind of resource the ARN addresses (both of this service's root
/// resources), the tags the resource was created with, the number of writes, and the key
/// and value shapes the `Tags` map allows — including an added key, an overwritten key, an
/// empty value, an empty request, and the longest key and value the model accepts.
#[tokio::test]
async fn tagging_merges_into_what_the_resource_already_carries() {
    assert_boundary_literals();

    const CASES: &[TagCase] = &[
        // A resource created with no tags, tagged once.
        TagCase {
            kind: state::WORKLOAD_ESTIMATE_ARN_TYPE,
            created_with: &[],
            writes: &[&[("team", "finops")]],
            expected: &[("team", "finops")],
        },
        // The same, on the other root resource.
        TagCase {
            kind: state::BILL_SCENARIO_ARN_TYPE,
            created_with: &[],
            writes: &[&[("team", "finops")]],
            expected: &[("team", "finops")],
        },
        // A tag the create put there is kept, and a second write adds beside it.
        TagCase {
            kind: state::WORKLOAD_ESTIMATE_ARN_TYPE,
            created_with: &[("env", "test")],
            writes: &[&[("team", "finops")], &[("owner", "a-b_c")]],
            expected: &[("env", "test"), ("owner", "a-b_c"), ("team", "finops")],
        },
        // A repeated key is overwritten, not duplicated or appended to.
        TagCase {
            kind: state::BILL_SCENARIO_ARN_TYPE,
            created_with: &[("env", "test")],
            writes: &[&[("env", "prod")], &[("env", "staging")]],
            expected: &[("env", "staging")],
        },
        // An empty value is a value: `ResourceTagValue` is `len 0..256`.
        TagCase {
            kind: state::WORKLOAD_ESTIMATE_ARN_TYPE,
            created_with: &[("keep", "me")],
            writes: &[&[("blank", "")]],
            expected: &[("blank", ""), ("keep", "me")],
        },
        // An empty request changes nothing: `Tags` is `len 0..200`.
        TagCase {
            kind: state::BILL_SCENARIO_ARN_TYPE,
            created_with: &[("only", "one")],
            writes: &[&[]],
            expected: &[("only", "one")],
        },
        // The boundary key and value shapes, and the punctuation the pattern allows.
        TagCase {
            kind: state::WORKLOAD_ESTIMATE_ARN_TYPE,
            created_with: &[],
            writes: &[
                &[("k", "v")],
                &[(LONGEST_KEY, LONGEST_VALUE)],
                &[("cost:center", "a+b=c@d/e f-g")],
            ],
            expected: &[
                ("cost:center", "a+b=c@d/e f-g"),
                ("k", "v"),
                (LONGEST_KEY, LONGEST_VALUE),
            ],
        },
    ];

    for (index, case) in CASES.iter().enumerate() {
        let label = format!("row={index} kind={}", case.kind);
        let emulator = Emulator::new();
        let created_with = (!case.created_with.is_empty()).then(|| tags_json(case.created_with));
        let id = if case.kind == state::BILL_SCENARIO_ARN_TYPE {
            create_scenario(&emulator, "tagged", created_with).await
        } else {
            create_estimate(&emulator, "tagged", created_with).await
        };
        let arn = resource_arn(case.kind, &id);

        // The ARN a client builds is the ARN the resource was minted with.
        assert!(
            emulator
                .engine
                .state::<ServiceState>()
                .lock()
                .resource_tags(&arn)
                .is_some(),
            "case {label}: expected {arn} to address the created resource"
        );

        // Before any write, the reader reports exactly what the create persisted.
        let want_initial = expected_map(case.created_with);
        let got_initial = listed_tags(&emulator, &arn, &label).await;
        assert_eq!(
            got_initial, want_initial,
            "case {label}: expected the created tags {want_initial:?}, got {got_initial:?}"
        );

        for (write, tags) in case.writes.iter().enumerate() {
            let (status, body) = emulator
                .call(
                    "TagResource",
                    json!({ "arn": arn.clone(), "tags": tags_json(tags) }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {label} write={write}: expected TagResource 200, got {status} with {body}"
            );
        }

        let want = expected_map(case.expected);
        let listed = listed_tags(&emulator, &arn, &label).await;
        assert_eq!(
            listed, want,
            "case {label}: expected ListTagsForResource to report {want:?}, got {listed:?}"
        );
        let stored = stored_tags(&emulator, &arn, &label);
        assert_eq!(
            stored, want,
            "case {label}: expected the store to hold {want:?}, got {stored:?}"
        );

        // Tagging one resource is not tagging its neighbour.
        let neighbour_id = if case.kind == state::BILL_SCENARIO_ARN_TYPE {
            create_scenario(&emulator, "neighbour", None).await
        } else {
            create_estimate(&emulator, "neighbour", None).await
        };
        let neighbour = resource_arn(case.kind, &neighbour_id);
        let neighbour_tags = listed_tags(&emulator, &neighbour, &label).await;
        assert!(
            neighbour_tags.is_empty(),
            "case {label}: expected the untouched neighbour to carry no tags, got {neighbour_tags:?}"
        );
    }
}

/// One row of the untag table.
struct UntagCase {
    kind: &'static str,
    created_with: &'static [(&'static str, &'static str)],
    /// The keys each successive `UntagResource` request names.
    removals: &'static [&'static [&'static str]],
    expected: &'static [(&'static str, &'static str)],
}

/// RULE: `UntagResource` removes exactly the keys it names and leaves every other tag in
/// place; naming a key the resource does not carry removes nothing and still succeeds, so
/// the same request repeated leaves the same tags.
///
/// The table spans the kind of resource, the number of keys removed at once, a key that is
/// not there, a repeated key, an empty key list, and removing every key the resource has.
#[tokio::test]
async fn untagging_removes_only_the_keys_it_names() {
    assert_boundary_literals();

    const CASES: &[UntagCase] = &[
        UntagCase {
            kind: state::WORKLOAD_ESTIMATE_ARN_TYPE,
            created_with: &[("env", "test"), ("team", "finops")],
            removals: &[&["env"]],
            expected: &[("team", "finops")],
        },
        UntagCase {
            kind: state::BILL_SCENARIO_ARN_TYPE,
            created_with: &[("env", "test"), ("team", "finops"), ("owner", "x")],
            removals: &[&["env", "owner"]],
            expected: &[("team", "finops")],
        },
        // A key the resource does not carry is a no-op, as is an empty list
        // (`ResourceTagKeys` is `len 0..200`).
        UntagCase {
            kind: state::WORKLOAD_ESTIMATE_ARN_TYPE,
            created_with: &[("env", "test")],
            removals: &[&["absent"], &[], &["env", "absent"]],
            expected: &[],
        },
        // The same removal twice leaves the same tags: removing is idempotent.
        UntagCase {
            kind: state::BILL_SCENARIO_ARN_TYPE,
            created_with: &[("env", "test"), ("team", "finops")],
            removals: &[&["env"], &["env"], &["env", "env"]],
            expected: &[("team", "finops")],
        },
        // Every key at once: a resource with no tags left is still a resource.
        UntagCase {
            kind: state::WORKLOAD_ESTIMATE_ARN_TYPE,
            created_with: &[("a", "1"), ("b", "2"), ("c", "3")],
            removals: &[&["a", "b", "c"]],
            expected: &[],
        },
        // The longest key the model accepts is removed like any other.
        UntagCase {
            kind: state::BILL_SCENARIO_ARN_TYPE,
            created_with: &[(LONGEST_KEY, "x"), ("keep", "me")],
            removals: &[&[LONGEST_KEY]],
            expected: &[("keep", "me")],
        },
    ];

    for (index, case) in CASES.iter().enumerate() {
        let label = format!("row={index} kind={}", case.kind);
        let emulator = Emulator::new();
        let created_with = (!case.created_with.is_empty()).then(|| tags_json(case.created_with));
        let id = if case.kind == state::BILL_SCENARIO_ARN_TYPE {
            create_scenario(&emulator, "untagged", created_with).await
        } else {
            create_estimate(&emulator, "untagged", created_with).await
        };
        let arn = resource_arn(case.kind, &id);

        for (removal, keys) in case.removals.iter().enumerate() {
            let (status, body) = emulator
                .call(
                    "UntagResource",
                    json!({ "arn": arn.clone(), "tagKeys": keys }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {label} removal={removal}: expected UntagResource 200, got {status} with {body}"
            );
        }

        let want = expected_map(case.expected);
        let listed = listed_tags(&emulator, &arn, &label).await;
        assert_eq!(
            listed, want,
            "case {label}: expected ListTagsForResource to report {want:?}, got {listed:?}"
        );
        let stored = stored_tags(&emulator, &arn, &label);
        assert_eq!(
            stored, want,
            "case {label}: expected the store to hold {want:?}, got {stored:?}"
        );
    }
}

/// RULE: an operation that addresses a resource by ARN resolves it by the identity that
/// ARN names — partition, account, resource type and resource id — so an ARN that
/// addresses no live resource of this account is refused, under HTTP 400 and with the
/// service's fixed unresolvable-ARN message, never with a 200 and never with the 404 that
/// would tell a client to create the resource and retry.
///
/// The table spans the ways an ARN can fail to address a live resource: a well-formed id
/// that was never minted, the id of a resource of the OTHER kind, a resource that has
/// been deleted, a resource kind this service cannot yet create, another account's ARN,
/// another partition's, and — since the region field does not select a resource — an
/// absent id with a region written into it, which is refused the same way.
#[tokio::test]
async fn an_arn_addressing_no_live_resource_is_refused_with_a_400() {
    let emulator = Emulator::new();
    let estimate = create_estimate(&emulator, "live", Some(json!({ "env": "test" }))).await;
    let scenario = create_scenario(&emulator, "livescenario", None).await;
    let deleted = create_estimate(&emulator, "doomed", None).await;
    let (status, body) = emulator
        .call("DeleteWorkloadEstimate", json!({ "identifier": deleted }))
        .await;
    assert_eq!(
        status, 200,
        "deleting the doomed estimate: expected 200, got {status} with {body}"
    );

    let never_minted = "00000000-0000-0000-0000-000000000000";
    let cases: Vec<(&str, String)> = vec![
        (
            "never minted",
            resource_arn(state::WORKLOAD_ESTIMATE_ARN_TYPE, never_minted),
        ),
        (
            "the other kind's id",
            resource_arn(state::WORKLOAD_ESTIMATE_ARN_TYPE, &scenario),
        ),
        (
            "an estimate id read as a scenario",
            resource_arn(state::BILL_SCENARIO_ARN_TYPE, &estimate),
        ),
        (
            "deleted",
            resource_arn(state::WORKLOAD_ESTIMATE_ARN_TYPE, &deleted),
        ),
        // A kind this service models but cannot yet create: `CreateBillEstimate` is still
        // a scaffold stub, so the store holds none.
        ("a kind nothing creates yet", resource_arn("bill-estimate", never_minted)),
        // Another account's ARN: this emulator presents exactly one account.
        (
            "another account",
            format!(
                "arn:aws:{}::111111111111:{}/{estimate}",
                state::ARN_NAMESPACE,
                state::WORKLOAD_ESTIMATE_ARN_TYPE
            ),
        ),
        // Another partition's: `Arn`'s pattern admits `aws-cn`, and a resource of one
        // partition is not a resource of another.
        (
            "another partition",
            format!(
                "arn:aws-cn:{}::{}:{}/{estimate}",
                state::ARN_NAMESPACE,
                engine::emulator_account_id(),
                state::WORKLOAD_ESTIMATE_ARN_TYPE
            ),
        ),
        // A region in the region field changes nothing about an id that names nothing.
        (
            "never minted, with a region",
            resource_arn_in_region(state::WORKLOAD_ESTIMATE_ARN_TYPE, never_minted, "eu-west-1"),
        ),
    ];

    for (case, arn) in cases {
        for (operation, request) in [
            ("ListTagsForResource", json!({ "arn": arn.clone() })),
            (
                "TagResource",
                json!({ "arn": arn.clone(), "tags": { "env": "test" } }),
            ),
            (
                "UntagResource",
                json!({ "arn": arn.clone(), "tagKeys": ["env"] }),
            ),
        ] {
            let (status, body) = emulator.call(operation, request).await;
            assert_eq!(
                status, 400,
                "case {case} op={operation}: expected 400, got {status} with {body}"
            );
            // The status and the error code are independent facts of the wire: the
            // recorded service answers the not-found shape under that 400.
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case} op={operation}: expected ResourceNotFoundException, got {body}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(state::UNRESOLVABLE_ARN_MESSAGE),
                "case {case} op={operation}: expected the fixed unresolvable-ARN message, got {body}"
            );
            assert!(
                !body["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(arn.as_str()),
                "case {case} op={operation}: expected the message not to echo the requested ARN, got {body}"
            );
            // The requested ARN is reported in the member modelled for it.
            assert_eq!(
                body["resourceId"].as_str(),
                Some(arn.as_str()),
                "case {case} op={operation}: expected resourceId to name the ARN looked for, got {body}"
            );
        }
    }

    // The live resource is untouched by every refusal above.
    let live = resource_arn(state::WORKLOAD_ESTIMATE_ARN_TYPE, &estimate);
    let listed = listed_tags(&emulator, &live, "the live estimate").await;
    assert_eq!(
        listed,
        expected_map(&[("env", "test")]),
        "expected the live estimate to keep its tags, got {listed:?}"
    );
}

/// RULE: the region field of one of this service's ARNs does not select a resource — the
/// service is partitional, its own ARNs leave that field empty, and its `Arn` pattern
/// admits any region text there — so every tagging operation addresses the SAME resource
/// whichever region form a client sends.
///
/// The table spans the kind of resource and the region forms a client can send: the empty
/// one the service mints, the region this emulator is called in, and another region
/// entirely.
#[tokio::test]
async fn the_region_field_of_an_arn_does_not_select_a_resource() {
    // The region forms: `ARN_REGION` is what this service's own ARNs carry; the others
    // are region texts the `Arn` pattern's `[-a-z0-9]*` admits.
    let regions = [state::ARN_REGION, "us-east-1", "eu-west-1", "ap-southeast-2"];

    for kind in [
        state::WORKLOAD_ESTIMATE_ARN_TYPE,
        state::BILL_SCENARIO_ARN_TYPE,
    ] {
        let emulator = Emulator::new();
        let id = if kind == state::BILL_SCENARIO_ARN_TYPE {
            create_scenario(&emulator, "regions", None).await
        } else {
            create_estimate(&emulator, "regions", None).await
        };

        // One key per region form, so what each form wrote is visible in the one map.
        let key_for = |index: usize| format!("written-by-form-{index}");

        for (index, region) in regions.iter().enumerate() {
            let case = format!("kind={kind} region={region:?}");
            let arn = resource_arn_in_region(kind, &id, region);
            let (status, body) = emulator
                .call(
                    "TagResource",
                    json!({ "arn": arn.clone(), "tags": { key_for(index): region } }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case}: expected TagResource 200, got {status} with {body}"
            );
        }

        // Every write landed on the ONE resource, so each form reads back all of them.
        let want: BTreeMap<String, String> = regions
            .iter()
            .enumerate()
            .map(|(index, region)| (key_for(index), (*region).to_string()))
            .collect();
        for region in regions {
            let case = format!("kind={kind} region={region:?}");
            let arn = resource_arn_in_region(kind, &id, region);
            let listed = listed_tags(&emulator, &arn, &case).await;
            assert_eq!(
                listed, want,
                "case {case}: expected every region form to report {want:?}, got {listed:?}"
            );
        }

        // And untagging through one form removes from the same map the others read.
        let (status, body) = emulator
            .call(
                "UntagResource",
                json!({
                    "arn": resource_arn_in_region(kind, &id, "eu-west-1"),
                    "tagKeys": [key_for(0), key_for(1)],
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "kind={kind}: expected UntagResource 200, got {status} with {body}"
        );
        let remaining = listed_tags(&emulator, &resource_arn(kind, &id), "after untagging").await;
        let want_remaining: BTreeMap<String, String> = [2usize, 3]
            .iter()
            .map(|index| (key_for(*index), regions[*index].to_string()))
            .collect();
        assert_eq!(
            remaining, want_remaining,
            "kind={kind}: expected {want_remaining:?} to remain, got {remaining:?}"
        );
    }
}

/// RULE: a tag key carrying the prefix AWS reserves for its own tags cannot be set by a
/// caller — the request is refused with HTTP 400 `ValidationException` and stores NOTHING,
/// not even the keys of the same request that are allowed — while a key that merely starts
/// with the same letters is an ordinary key.
///
/// The table spans the casing of the reserved prefix (it is reserved however it is cased),
/// a reserved key mixed in with an allowed one, and the near-misses that are not reserved
/// at all.
#[tokio::test]
async fn a_reserved_tag_key_is_refused_and_stores_nothing() {
    let refused: &[(&str, Value)] = &[
        ("lowercase", json!({ "aws:cloudformation:stack": "s" })),
        ("uppercase", json!({ "AWS:created-by": "x" })),
        ("mixed case", json!({ "Aws:thing": "x" })),
        ("exactly the prefix", json!({ "aws:": "x" })),
        (
            "mixed with an allowed key",
            json!({ "env": "test", "aws:internal": "x" }),
        ),
    ];
    // Not reserved: the prefix is `aws:`, so these are ordinary keys.
    let allowed: &[(&str, &str)] = &[
        ("awsome", "yes"),
        ("aws", "no-colon"),
        ("team:aws:owned", "not-a-prefix"),
    ];

    let emulator = Emulator::new();
    let id = create_estimate(&emulator, "reserved", None).await;
    let arn = resource_arn(state::WORKLOAD_ESTIMATE_ARN_TYPE, &id);

    for (case, tags) in refused {
        let (status, body) = emulator
            .call("TagResource", json!({ "arn": arn.clone(), "tags": tags }))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for a reserved tag key, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected ValidationException, got {body}"
        );
        let stored = stored_tags(&emulator, &arn, case);
        assert!(
            stored.is_empty(),
            "case {case}: expected a refused request to store nothing, got {stored:?}"
        );
    }

    for (key, value) in allowed {
        let (status, body) = emulator
            .call(
                "TagResource",
                json!({ "arn": arn.clone(), "tags": { *key: *value } }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case key={key}: expected an unreserved key to be accepted, got {status} with {body}"
        );
    }
    let listed = listed_tags(&emulator, &arn, "the allowed keys").await;
    assert_eq!(
        listed,
        expected_map(allowed),
        "expected only the unreserved keys stored, got {listed:?}"
    );

    // ORDER OF REFUSALS: the reserved key is refused as such even when the ARN names no
    // live resource, so the caller is told what is wrong with the request it sent.
    let absent = resource_arn(
        state::WORKLOAD_ESTIMATE_ARN_TYPE,
        "00000000-0000-0000-0000-000000000000",
    );
    let (status, body) = emulator
        .call(
            "TagResource",
            json!({ "arn": absent, "tags": { "aws:internal": "x" } }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case a reserved key on an absent resource: expected the 400 to win over the 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case a reserved key on an absent resource: expected ValidationException, got {body}"
    );
}

/// RULE: a tag key is request data of arbitrary UTF-8, so every answer to one is a bounded
/// modelled answer — an accepted key, or a refusal naming what is wrong with it — and never
/// an unwound request; the reserved-prefix test is applied to the key as TEXT, so a key
/// whose characters straddle the prefix's byte length is judged on its characters.
///
/// The rows are multi-byte keys whose byte 4 (the reserved prefix's length) falls INSIDE a
/// character, which is what a byte-range prefix test would panic on. Whether the modelled
/// key pattern `^[\w\s:+=@/-]+$` admits a non-ASCII word character is the boundary
/// validator's business, so an unreserved row accepts either the 200 of an ordinary key or
/// that validator's 400 — what it must never be is a dropped request.
#[tokio::test]
async fn a_multibyte_tag_key_gets_a_bounded_answer() {
    let emulator = Emulator::new();
    let id = create_estimate(&emulator, "multibyte", None).await;
    let arn = resource_arn(state::WORKLOAD_ESTIMATE_ARN_TYPE, &id);

    // Not reserved, and each one has a character boundary that is NOT at byte 4:
    // "aa日" is 5 bytes (boundaries 0,1,2,5), "aaa日" 6 bytes (0,1,2,3,6), "日本語" 9.
    for key in ["aa日", "aaa日", "日本語", "a日b:c"] {
        let (status, body) = emulator
            .call(
                "TagResource",
                json!({ "arn": arn.clone(), "tags": { key: "x" } }),
            )
            .await;
        assert!(
            status == 200 || status == 400,
            "case key={key:?}: expected a bounded 200 or 400, got {status} with {body}"
        );
        if status == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case key={key:?}: expected a modelled refusal, got {body}"
            );
        }
    }

    // Reserved whatever follows the prefix: either this handler's 400 for the reserved
    // key, or the validator's 400 for the character after it — a 400 `ValidationException`
    // either way, and never a stored `aws:` tag.
    for key in ["aws:日", "AWS:日本語"] {
        let (status, body) = emulator
            .call(
                "TagResource",
                json!({ "arn": arn.clone(), "tags": { key: "x" } }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case key={key:?}: expected 400 for a reserved key, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case key={key:?}: expected ValidationException, got {body}"
        );
        let stored = stored_tags(&emulator, &arn, key);
        assert!(
            stored.keys().all(|stored_key| !stored_key
                .to_lowercase()
                .starts_with(state::RESERVED_TAG_KEY_PREFIX)),
            "case key={key:?}: expected no reserved key stored, got {stored:?}"
        );
    }

    // The service is still serving after every one of those: the reader answers.
    let (status, body) = emulator
        .call("ListTagsForResource", json!({ "arn": arn }))
        .await;
    assert_eq!(
        status, 200,
        "expected the resource to still be readable after the multi-byte keys, got {status} with {body}"
    );
}

/// RULE: a resource carries at most as many tags as the `Tags` map that reports them holds
/// (`state::MAX_RESOURCE_TAGS`, inferred from that map's modelled `length 0..200` payload
/// bound, since a resource holding more could not be read back); a `TagResource` that
/// would leave it with more is refused with the modelled `ServiceQuotaExceededException`
/// (HTTP 402) and stores nothing, while a request that only overwrites keys the resource
/// already carries stays inside the cap however often it is repeated.
#[tokio::test]
async fn tagging_past_the_tag_cap_is_refused() {
    let emulator = Emulator::new();
    let id = create_estimate(&emulator, "capped", None).await;
    let arn = resource_arn(state::WORKLOAD_ESTIMATE_ARN_TYPE, &id);

    // Fill the resource to exactly the cap, in two writes so the merge path is what
    // reaches it.
    let cap = state::MAX_RESOURCE_TAGS;
    for (case, range) in [("first half", 0..cap / 2), ("second half", cap / 2..cap)] {
        let tags: Value = Value::Object(
            range
                .map(|index| (format!("key{index}"), json!(format!("value{index}"))))
                .collect(),
        );
        let (status, body) = emulator
            .call("TagResource", json!({ "arn": arn.clone(), "tags": tags }))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 up to the cap, got {status} with {body}"
        );
    }
    let listed = listed_tags(&emulator, &arn, "at the cap").await;
    assert_eq!(
        listed.len(),
        cap,
        "expected the resource to hold exactly the cap, got {}",
        listed.len()
    );

    // Overwriting keys it already carries adds nothing, so it stays allowed.
    for attempt in 0..2 {
        let (status, body) = emulator
            .call(
                "TagResource",
                json!({ "arn": arn.clone(), "tags": { "key0": "rewritten" } }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case attempt={attempt}: expected an overwrite at the cap to be allowed, got {status} with {body}"
        );
    }

    // One more KEY is past it, and is refused the same way however often it is tried.
    for attempt in 0..2 {
        let (status, body) = emulator
            .call(
                "TagResource",
                json!({ "arn": arn.clone(), "tags": { "one-too-many": "x" } }),
            )
            .await;
        assert_eq!(
            status, 402,
            "case attempt={attempt}: expected 402 past the cap, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ServiceQuotaExceededException",
            "case attempt={attempt}: expected ServiceQuotaExceededException, got {body}"
        );
        assert_eq!(
            body["resourceId"].as_str(),
            Some(arn.as_str()),
            "case attempt={attempt}: expected the refusal to name the resource, got {body}"
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some(state::WORKLOAD_ESTIMATE_RESOURCE_TYPE),
            "case attempt={attempt}: expected the refusal to name the resource kind, got {body}"
        );
    }

    let after = listed_tags(&emulator, &arn, "after the refusals").await;
    assert_eq!(
        after.len(),
        cap,
        "expected the refusals to store nothing, got {} tags",
        after.len()
    );
    assert_eq!(
        after.get("key0").map(String::as_str),
        Some("rewritten"),
        "expected the allowed overwrite to have landed, got {:?}",
        after.get("key0")
    );
    assert!(
        !after.contains_key("one-too-many"),
        "expected the refused key to be absent, got {after:?}"
    );
}

/// RULE: a resource past the `expiresAt` it advertises is gone from the tagging surface
/// exactly as it is from every other reader, so its tags cannot be read or written after
/// it should have been dropped.
#[tokio::test]
async fn an_expired_resource_is_gone_from_the_tagging_surface() {
    // On each side of the boundary instant: `expiresAt` in the past, exactly now, and
    // still ahead.
    let cases: [(i64, bool); 3] = [(-60, false), (0, false), (60, true)];

    for (offset, expected_live) in cases {
        let case = format!("expiresAt = now{offset:+}");
        let emulator = Emulator::new();
        let id = create_estimate(&emulator, "aging", Some(json!({ "env": "test" }))).await;
        let arn = resource_arn(state::WORKLOAD_ESTIMATE_ARN_TYPE, &id);
        {
            let store = emulator.engine.state::<ServiceState>();
            let mut guard = store.lock();
            let estimate = guard
                .workload_estimate_mut(&id)
                .unwrap_or_else(|| panic!("case {case}: {id} was not stored"));
            estimate.expires_at = aws_smithy_types::DateTime::from_secs(state::now_secs() + offset);
        }

        // Gone from this surface means the ARN addresses nothing, which is the 400 of
        // `an_arn_addressing_no_live_resource_is_refused_with_a_400`.
        let expected_status = if expected_live { 200 } else { 400 };
        for operation in ["ListTagsForResource", "UntagResource", "TagResource"] {
            let request = match operation {
                "TagResource" => json!({ "arn": arn.clone(), "tags": { "env": "prod" } }),
                "UntagResource" => json!({ "arn": arn.clone(), "tagKeys": ["env"] }),
                _ => json!({ "arn": arn.clone() }),
            };
            let (status, body) = emulator.call(operation, request).await;
            assert_eq!(
                status, expected_status,
                "case {case} op={operation}: expected {expected_status}, got {status} with {body}"
            );
        }
    }
}
