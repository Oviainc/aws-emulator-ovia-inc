// SPDX-License-Identifier: CC0-1.0

//! How MemoryDB refuses a call that addresses a resource it cannot resolve, end to end
//! over the wire.
//!
//! Every case drives the SAME path a client does: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AmazonMemoryDB.<Operation>` through the multi-service router (the
//! shared harness in `cluster_lifecycle`), so what is asserted is the emulator's HTTP
//! behaviour and not a handler called directly.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * an ARN-ADDRESSED call (`ListTags`, `TagResource`, `UntagResource`) reads its
//!   `ResourceArn` in STAGES, and the stage that refuses it decides what the caller is
//!   told: a value that does not carry an ARN's seven components is refused for its
//!   component count; one that does but names a resource type this service does not have
//!   is refused for the resource type; one whose identity is not this service's is
//!   refused as not one of its ARNs — all three the modelled 400 `InvalidARNFault`;
//! * an ARN whose Region or account is not the one the endpoint serves is the modelled
//!   400 `InvalidParameterValueException`, worded for whichever of the two it is: the
//!   REQUEST disagrees with its own ARN, so this is not a not-found fault;
//! * the Region field is part of a REGIONAL resource's identity and absent from a GLOBAL
//!   one's, so an empty Region field resolves for the multi-Region kinds and is an
//!   invalid ARN for every other kind;
//! * an in-scope ARN naming no resource in state is that kind's own modelled not-found
//!   fault, carrying that kind's own sentence built from the name the ARN carried;
//! * a NAME-addressed call that names an ACL this service does not hold states it in the
//!   one sentence this service words that with, at every operation that resolves an ACL
//!   by name;
//! * and none of these refusals changes what a later caller sees.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{call, call_signed, error_code, ok, router};

/// The region the suite-wide harness signs its requests for
/// (`crate::cluster_lifecycle::REGION`), which every ARN the emulator mints carries.
const REGION: &str = "us-west-2";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// A node type MemoryDB publishes, for the cluster this suite seeds.
const NODE_TYPE: &str = "db.t4g.small";

/// A parameter group family MemoryDB publishes for its Redis OSS engine.
const FAMILY: &str = "memorydb_redis7";

/// Names NO seeded resource carries, so every row below is about the name the request
/// carried rather than about an empty store. They differ along what the sentence under
/// test generalises over: length, letter case, and the characters a name may hold. Each
/// is short enough for the service's own name rule (under 40 characters), so a row that
/// sends one as a NAME reaches the resolution under test rather than that rule.
const ABSENT_NAMES: &[&str] = &[
    "a",
    "absent-resource",
    "Absent-RESOURCE",
    "absent-resource-with-a-longer-name-012",
];

/// The names this suite SEEDS, one resource of each kind this emulator can create.
const PRESENT_CLUSTER: &str = "present-cluster";
const PRESENT_ACL: &str = "present-acl";
const PRESENT_PARAMETER_GROUP: &str = "present-pg";

/// One `arn:aws:memorydb:<region>:<account>:<type>/<name>`.
fn arn_in(region: &str, account: &str, resource_type: &str, name: &str) -> String {
    format!("arn:aws:memorydb:{region}:{account}:{resource_type}/{name}")
}

/// One ARN in this suite's Region and account.
fn arn(resource_type: &str, name: &str) -> String {
    arn_in(REGION, ACCOUNT, resource_type, name)
}

/// One operation that addresses a resource BY ARN, with a payload that breaks no other
/// rule of that operation, so the only thing a row varies is the `ResourceArn`.
struct ArnAddressed {
    operation: &'static str,
    payload: fn() -> Value,
}

/// The three operations that address a resource BY ARN.
const ARN_ADDRESSED: &[ArnAddressed] = &[
    ArnAddressed {
        operation: "ListTags",
        payload: || json!({}),
    },
    ArnAddressed {
        operation: "TagResource",
        payload: || json!({ "Tags": [{ "Key": "env", "Value": "test" }] }),
    },
    ArnAddressed {
        operation: "UntagResource",
        payload: || json!({ "TagKeys": ["env"] }),
    },
];

/// The `message` of a refusal body.
fn message(body: &Value, case: &str) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a message in {body}"))
        .to_string()
}

/// A router with one resource of each creatable kind in it.
async fn seeded() -> MultiServiceRouter {
    let router = router();
    ok(
        &router,
        "CreateACL",
        &json!({ "ACLName": PRESENT_ACL }),
        "seed acl",
    )
    .await;
    ok(
        &router,
        "CreateParameterGroup",
        &json!({ "ParameterGroupName": PRESENT_PARAMETER_GROUP, "Family": FAMILY }),
        "seed parameter group",
    )
    .await;
    ok(
        &router,
        "CreateCluster",
        &json!({
            "ClusterName": PRESENT_CLUSTER,
            "NodeType": NODE_TYPE,
            "ACLName": PRESENT_ACL,
            "Tags": [{ "Key": "env", "Value": "test" }],
        }),
        "seed cluster",
    )
    .await;
    router
}

/// Call one ARN-addressed row against every operation that takes a `ResourceArn`, and
/// assert each answers `(400, code, expected_message)`.
async fn assert_arn_refusal(
    router: &MultiServiceRouter,
    label: &str,
    resource_arn: &str,
    code: &str,
    expected_message: &str,
) {
    for row in ARN_ADDRESSED {
        let operation = row.operation;
        let case = format!("{label} / {operation}");
        let mut body = (row.payload)();
        body["ResourceArn"] = json!(resource_arn);
        let (status, answered) = call(router, operation, &body).await;
        let answered_code = error_code(&answered);
        assert_eq!(
            (status, answered_code.as_str()),
            (400, code),
            "case {case:?}: expected 400 {code} for {resource_arn:?}, got {status} with \
             {answered}"
        );
        assert_eq!(
            message(&answered, &case),
            expected_message,
            "case {case:?}: expected the stage's own sentence for {resource_arn:?}, got \
             {answered}"
        );
    }
}

/// The sentence the component-count stage answers with. One ARN names seven components:
/// `arn`, the partition, the service, the Region, the account, the resource type and the
/// resource name.
const COMPONENT_COUNT_MESSAGE: &str = "Input ARN string does not have 7 components.";

/// The sentence the resource-type stage answers with.
const RESOURCE_TYPE_MESSAGE: &str = "Resource Type is either not supported or invalid.";

/// A value too short to carry an ARN's seven components is refused for its component
/// count — whatever else is also wrong with it.
#[tokio::test]
async fn a_value_with_too_few_components_is_refused_for_its_component_count() {
    let router = seeded().await;
    // Each row carries FEWER than seven components, in a different way.
    let rows: [(&str, String); 6] = [
        ("empty", String::new()),
        ("a bare name", "my-cluster".to_string()),
        ("the prefix alone", "arn".to_string()),
        (
            "no resource field",
            format!("arn:aws:memorydb:{REGION}:{ACCOUNT}"),
        ),
        (
            "a resource field that names no type and name",
            format!("arn:aws:memorydb:{REGION}:{ACCOUNT}:cluster"),
        ),
        (
            "another service's ARN, one component short",
            format!("arn:aws:s3:::{PRESENT_CLUSTER}"),
        ),
    ];
    for (label, candidate) in rows {
        assert_arn_refusal(
            &router,
            label,
            &candidate,
            "InvalidARNFault",
            COMPONENT_COUNT_MESSAGE,
        )
        .await;
    }
}

/// A complete ARN whose resource-type component names no resource type this service has
/// is refused FOR THAT TYPE, not for the value as a whole.
#[tokio::test]
async fn a_resource_type_this_service_does_not_have_is_refused_for_the_type() {
    let router = seeded().await;
    // Resource-type components this service has no resource for: an invented one, two
    // MemoryDB resources that are not taggable by ARN (`reservednode`, `shard`), and
    // another service's own type.
    let rows: [(&str, String); 5] = [
        ("an invented type", arn("widget", "anything")),
        ("a reserved node", arn("reservednode", "anything")),
        ("a shard", arn("shard", "anything")),
        (
            "the type spelled in another case",
            arn("Cluster", PRESENT_CLUSTER),
        ),
        (
            "another service's type",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:bucket/anything"),
        ),
    ];
    for (label, candidate) in rows {
        assert_arn_refusal(
            &router,
            label,
            &candidate,
            "InvalidARNFault",
            RESOURCE_TYPE_MESSAGE,
        )
        .await;
    }
}

/// A complete ARN naming a type this service HAS, whose identity is still not this
/// service's, is refused as a value that is not one of this service's ARNs — and the
/// sentence quotes the value, because that is the only thing left to show the caller.
#[tokio::test]
async fn an_identity_that_is_not_this_services_is_refused_as_a_foreign_arn() {
    let router = seeded().await;
    let rows: [(&str, String); 5] = [
        (
            "not the arn prefix",
            format!("urn:aws:memorydb:{REGION}:{ACCOUNT}:cluster/{PRESENT_CLUSTER}"),
        ),
        (
            "no partition",
            format!("arn::memorydb:{REGION}:{ACCOUNT}:cluster/{PRESENT_CLUSTER}"),
        ),
        (
            "another service, the same type",
            format!("arn:aws:elasticache:{REGION}:{ACCOUNT}:cluster/{PRESENT_CLUSTER}"),
        ),
        (
            "no Region on a regional type",
            arn_in("", ACCOUNT, "cluster", PRESENT_CLUSTER),
        ),
        ("no resource name", arn("cluster", "")),
    ];
    for (label, candidate) in rows {
        let expected = format!("{candidate} is not a valid MemoryDB resource ARN.");
        assert_arn_refusal(&router, label, &candidate, "InvalidARNFault", &expected).await;
    }
}

/// An endpoint serves ONE Region of ONE account, so an ARN naming another is refused as
/// a request at odds with its own ARN — a different fault, and a different sentence,
/// from an ARN this service cannot read at all.
#[tokio::test]
async fn an_arn_outside_the_endpoints_scope_is_a_request_fault() {
    let router = seeded().await;
    // The Region rows span the kinds and the names, so the refusal is about the scope
    // rather than about one resource.
    for name in [PRESENT_CLUSTER, ABSENT_NAMES[0]] {
        for region in ["eu-west-1", "us-east-1", "ap-southeast-2"] {
            let candidate = arn_in(region, ACCOUNT, "cluster", name);
            assert_arn_refusal(
                &router,
                &format!("{region} / {name}"),
                &candidate,
                "InvalidParameterValueException",
                "Please check the region or customer ID specified in the request.",
            )
            .await;
        }
        for account in ["999988887777", "000000000001"] {
            let candidate = arn_in(REGION, account, "cluster", name);
            assert_arn_refusal(
                &router,
                &format!("{account} / {name}"),
                &candidate,
                "InvalidParameterValueException",
                "The resource ARN does not belong to the caller's account.",
            )
            .await;
        }
    }

    // The SAME ARN the emulator minted, sent to another Region's endpoint: the ARN is
    // this service's and the resource exists, and it is still the two disagreeing.
    let stored = arn("cluster", PRESENT_CLUSTER);
    let case = "a stored ARN read from another Region's endpoint";
    let (status, answered) = call_signed(
        &router,
        "eu-west-1",
        "ListTags",
        &json!({ "ResourceArn": stored }),
    )
    .await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "InvalidParameterValueException"),
        "case {case:?}: got {status} with {answered}"
    );
}

/// One resource kind an ARN can name: its resource-type component, the fault an ARN of
/// that kind that resolves to nothing answers with, whether the kind is GLOBAL (and so
/// carries no Region in its ARN), and the sentence this service states a missing one of
/// that kind with.
///
/// The faults are the per-kind not-found errors the tagging operations model
/// (`smithy_query query="operation" operation="ListTags,TagResource,UntagResource"`);
/// the sentences are the wording this service's wire answers each kind with.
struct Kind {
    resource_type: &'static str,
    not_found: &'static str,
    global: bool,
    /// The sentence, with `{name}` standing for the name the ARN carried.
    sentence: &'static str,
}

/// Every resource kind a MemoryDB ARN can name.
const KINDS: &[Kind] = &[
    Kind {
        resource_type: "cluster",
        not_found: "ClusterNotFoundFault",
        global: false,
        sentence: "{name} is not present",
    },
    Kind {
        resource_type: "acl",
        not_found: "ACLNotFoundFault",
        global: false,
        sentence: "{name} is either not present or not available.",
    },
    Kind {
        resource_type: "user",
        not_found: "UserNotFoundFault",
        global: false,
        sentence: "{name} is either not present or not available",
    },
    Kind {
        resource_type: "parametergroup",
        not_found: "ParameterGroupNotFoundFault",
        global: false,
        sentence: "{name} is not present",
    },
    Kind {
        resource_type: "snapshot",
        not_found: "SnapshotNotFoundFault",
        global: false,
        sentence: "{name} is either not present or not available",
    },
    Kind {
        resource_type: "subnetgroup",
        not_found: "SubnetGroupNotFoundFault",
        global: false,
        sentence: "{name} is not present",
    },
    Kind {
        resource_type: "multiregioncluster",
        not_found: "MultiRegionClusterNotFoundFault",
        global: true,
        sentence: "{name} is not present",
    },
    Kind {
        resource_type: "multiregionparametergroup",
        not_found: "MultiRegionParameterGroupNotFoundFault",
        global: true,
        sentence: "{name} is not present",
    },
];

/// An in-scope ARN naming no resource in state answers THAT KIND's not-found fault,
/// with that kind's sentence built from the name the ARN carried.
#[tokio::test]
async fn an_absent_resource_answers_its_own_kinds_sentence() {
    let router = seeded().await;
    for kind in KINDS {
        for name in ABSENT_NAMES {
            let candidate = arn(kind.resource_type, name);
            let expected = kind.sentence.replace("{name}", name);
            assert_arn_refusal(
                &router,
                &format!("{} / {name}", kind.resource_type),
                &candidate,
                kind.not_found,
                &expected,
            )
            .await;
        }
    }
}

/// The Region field belongs to a REGIONAL resource's identity and not to a global one's:
/// an ARN with an EMPTY Region field resolves for the multi-Region kinds (and so answers
/// their not-found fault) and is an invalid ARN for every other kind.
#[tokio::test]
async fn an_empty_region_field_resolves_only_for_a_global_kind() {
    let router = seeded().await;
    for kind in KINDS {
        let name = ABSENT_NAMES[1];
        let candidate = arn_in("", ACCOUNT, kind.resource_type, name);
        let (code, expected) = if kind.global {
            (kind.not_found, kind.sentence.replace("{name}", name))
        } else {
            (
                "InvalidARNFault",
                format!("{candidate} is not a valid MemoryDB resource ARN."),
            )
        };
        assert_arn_refusal(
            &router,
            &format!("no Region / {}", kind.resource_type),
            &candidate,
            code,
            &expected,
        )
        .await;
    }
}

/// A NAME-addressed call that names an ACL this service does not hold states it in ONE
/// sentence, at every operation that resolves an ACL by name — the by-ARN calls have
/// their own sentence, which is why this one is pinned beside them.
#[tokio::test]
async fn a_missing_acl_named_directly_has_one_sentence_everywhere() {
    let router = seeded().await;
    for name in ABSENT_NAMES {
        // The name is a case-insensitive identifier, so the sentence names the FOLDED
        // name whatever case the request spelled it in.
        let folded = name.to_ascii_lowercase();
        let rows: [(&str, Value); 5] = [
            ("DescribeACLs", json!({ "ACLName": name })),
            ("DeleteACL", json!({ "ACLName": name })),
            (
                "UpdateACL",
                json!({ "ACLName": name, "UserNamesToAdd": ["someone"] }),
            ),
            (
                "CreateCluster",
                json!({
                    "ClusterName": "cluster-for-a-missing-acl",
                    "NodeType": NODE_TYPE,
                    "ACLName": name,
                }),
            ),
            (
                "UpdateCluster",
                json!({ "ClusterName": PRESENT_CLUSTER, "ACLName": name }),
            ),
        ];
        for (operation, body) in rows {
            let case = format!("{operation} / {name}");
            let (status, answered) = call(&router, operation, &body).await;
            assert_eq!(
                (status, error_code(&answered).as_str()),
                (400, "ACLNotFoundFault"),
                "case {case:?}: expected 400 ACLNotFoundFault, got {status} with {answered}"
            );
            assert_eq!(
                message(&answered, &case),
                format!("ACL {folded} not found"),
                "case {case:?}: expected the one sentence a missing ACL is stated with, \
                 got {answered}"
            );
        }
    }
}

/// None of these refusals changes what a later caller sees: the seeded resources, and
/// the cluster's tag set, are exactly where they were after the whole table has run.
#[tokio::test]
async fn the_refusals_leave_every_seeded_resource_in_place() {
    let router = seeded().await;
    for kind in KINDS {
        for name in ABSENT_NAMES {
            for row in ARN_ADDRESSED {
                let mut body = (row.payload)();
                body["ResourceArn"] = json!(arn(kind.resource_type, name));
                let (_, _) = call(&router, row.operation, &body).await;
                body["ResourceArn"] = json!(arn_in("eu-west-1", ACCOUNT, kind.resource_type, name));
                let (_, _) = call(&router, row.operation, &body).await;
            }
        }
    }

    let case = "after the whole table";
    let tags = ok(
        &router,
        "ListTags",
        &json!({ "ResourceArn": arn("cluster", PRESENT_CLUSTER) }),
        case,
    )
    .await;
    assert_eq!(
        tags["TagList"],
        json!([{ "Key": "env", "Value": "test" }]),
        "case {case:?}: a refused tag mutation must leave the tag set alone, got {tags}"
    );
    for (operation, body) in [
        ("DescribeClusters", json!({ "ClusterName": PRESENT_CLUSTER })),
        ("DescribeACLs", json!({ "ACLName": PRESENT_ACL })),
        (
            "DescribeParameterGroups",
            json!({ "ParameterGroupName": PRESENT_PARAMETER_GROUP }),
        ),
    ] {
        ok(&router, operation, &body, case).await;
    }
}
