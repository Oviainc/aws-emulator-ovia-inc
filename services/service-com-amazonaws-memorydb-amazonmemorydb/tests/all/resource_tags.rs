// SPDX-License-Identifier: CC0-1.0

//! MemoryDB resource tags, end to end over the wire.
//!
//! Every case drives the SAME path a client does: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AmazonMemoryDB.<Operation>` through the multi-service router, so what
//! is asserted is the emulator's HTTP behaviour and not a handler called directly.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * `ListTags` reports the tags the store holds for the resource its `ResourceArn`
//!   names, in key order — whatever kind of resource that is and however the tags got
//!   there (a create's `Tags` member, or `TagResource`);
//! * `TagResource` MERGES: a key the resource does not carry is added, a key it does
//!   carry takes the new value, and a key the request does not name is left alone;
//! * `UntagResource` removes exactly the keys `TagKeys` names and answers with the tag
//!   set the resource is left with;
//! * both mutations are ATOMIC: a request that cannot be honoured in full — an ARN that
//!   names no resource, a tag with no key, a key the resource does not carry, a merge
//!   past the per-resource tag quota, a key list longer than that quota — changes
//!   NOTHING, and the next `ListTags` proves it;
//! * a tag set belongs to ONE resource: an ARN that resolves to no resource of this
//!   store is that resource kind's modelled 404, an ARN that is not a MemoryDB resource
//!   ARN at all is the modelled 400 `InvalidARNFault`, and an ARN is resolved whole, so
//!   another region's or partition's copy of a stored name resolves to nothing;
//! * tags are a caller's own metadata on a caller's own resource: a tag key under AWS's
//!   reserved `aws:` prefix, and a tag mutation on one of the service's own default
//!   resources, are both the modelled 400;
//! * `ListTags` is a read: two of them agree, and neither changes what a later caller
//!   sees.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The suite-wide harness: ONE SigV4 request builder, ONE router constructor and ONE
// error-code reader for every suite in this crate.
use crate::cluster_lifecycle::{call, call_signed, error_code, ok, router};

/// The region the suite-wide harness signs its requests for
/// (`crate::cluster_lifecycle::REGION`), which every ARN the emulator mints must carry.
const REGION: &str = "us-west-2";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The most tags one resource may carry, mirrored from
/// `crate::state::MAX_TAGS_PER_RESOURCE`: `TagList` is `@length(min: 0, max: 200)`
/// (`smithy_query query="shape" shape="TagList"`) and is the shape a resource's whole
/// tag set is reported in. Exercised AT the bound and one past it.
const MAX_TAGS_PER_RESOURCE: usize = 200;

/// An access string that is valid for every user the suite creates (MemoryDB's
/// "full access" string: enabled, all keys, all channels, all commands).
const FULL_ACCESS: &str = "on ~* &* +@all";

/// A password that breaks none of the documented user password rules (16 characters,
/// none of `,` `"` `/` `@`).
const PASSWORD: &str = "Aa1!Aa1!Aa1!Aa1!";

/// A parameter group family MemoryDB publishes for its Redis OSS engine; the model
/// carries no family catalog, so any family a create sends is recorded.
const FAMILY: &str = "memorydb_redis7";

/// One `arn:aws:memorydb:<region>:<account>:<type>/<name>` in this suite's region.
fn arn(resource_type: &str, name: &str) -> String {
    format!("arn:aws:memorydb:{REGION}:{ACCOUNT}:{resource_type}/{name}")
}

/// One kind of resource a MemoryDB ARN can name: the ARN's resource-type segment and
/// the fault the three tagging operations report for an ARN of that kind that resolves
/// to nothing (`smithy_query query="operation" operation="ListTags"` lists one
/// not-found fault per kind).
struct Kind {
    resource_type: &'static str,
    not_found: &'static str,
    /// Whether this emulator can create a resource of this kind at all. The kinds it
    /// cannot (snapshots, subnet groups, the multi-Region resources — their creates are
    /// still scaffolded) have no resource for an ARN to resolve to, which is exactly
    /// what their not-found faults say.
    creatable: bool,
}

/// Every resource kind a MemoryDB ARN can name, creatable or not.
const KINDS: &[Kind] = &[
    Kind {
        resource_type: "cluster",
        not_found: "ClusterNotFoundFault",
        creatable: true,
    },
    Kind {
        resource_type: "acl",
        not_found: "ACLNotFoundFault",
        creatable: true,
    },
    Kind {
        resource_type: "user",
        not_found: "UserNotFoundFault",
        creatable: true,
    },
    Kind {
        resource_type: "parametergroup",
        not_found: "ParameterGroupNotFoundFault",
        creatable: true,
    },
    Kind {
        resource_type: "snapshot",
        not_found: "SnapshotNotFoundFault",
        creatable: false,
    },
    Kind {
        resource_type: "subnetgroup",
        not_found: "SubnetGroupNotFoundFault",
        creatable: false,
    },
    Kind {
        resource_type: "multiregioncluster",
        not_found: "MultiRegionClusterNotFoundFault",
        creatable: false,
    },
    Kind {
        resource_type: "multiregionparametergroup",
        not_found: "MultiRegionParameterGroupNotFoundFault",
        creatable: false,
    },
];

/// The resource types this emulator can create, in the order [`KINDS`] lists them.
fn creatable_kinds() -> impl Iterator<Item = &'static Kind> {
    KINDS.iter().filter(|kind| kind.creatable)
}

/// The create request for one resource of `resource_type` named `name`, carrying `tags`
/// as its `Tags` member (absent when `None`) — plus whatever that create needs to exist
/// first, which this creates on the way.
async fn create_request(
    router: &MultiServiceRouter,
    resource_type: &str,
    name: &str,
    tags: Option<&Value>,
) -> (&'static str, Value) {
    let (operation, mut body) = match resource_type {
        "cluster" => {
            // A cluster's `ACLName` must name a stored ACL, so the ACL comes first.
            let acl_name = format!("{name}-acl");
            ok(router, "CreateACL", &json!({ "ACLName": acl_name }), name).await;
            (
                "CreateCluster",
                json!({
                    "ClusterName": name,
                    "NodeType": "db.t4g.small",
                    "ACLName": acl_name,
                }),
            )
        }
        "acl" => ("CreateACL", json!({ "ACLName": name })),
        "user" => (
            "CreateUser",
            json!({
                "UserName": name,
                "AccessString": FULL_ACCESS,
                "AuthenticationMode": { "Type": "password", "Passwords": [PASSWORD] },
            }),
        ),
        "parametergroup" => (
            "CreateParameterGroup",
            json!({ "ParameterGroupName": name, "Family": FAMILY }),
        ),
        other => panic!("case {name:?}: this suite cannot create a {other}"),
    };
    if let Some(tags) = tags {
        body["Tags"] = tags.clone();
    }
    (operation, body)
}

/// Create one resource of `resource_type` named `name`, carrying `tags` as its create's
/// `Tags` member (absent when `None`), and answer the ARN the service minted for it.
async fn create(
    router: &MultiServiceRouter,
    resource_type: &str,
    name: &str,
    tags: Option<&Value>,
) -> String {
    let (operation, body) = create_request(router, resource_type, name, tags).await;
    ok(router, operation, &body, name).await;
    arn(resource_type, name)
}

/// The `TagList` of a response, as `(Key, Value)` pairs IN THE ORDER THE WIRE CARRIED
/// THEM, so a case comparing it against a key-ordered expectation also pins the order.
///
/// The value stays an `Option`, so a case can tell a `Value` that is present and empty
/// from one the response omitted: "A tag with a null Value is permitted" (`Tag`) and a
/// stored tag reports that value as the EMPTY one, never as an absent member.
fn tag_list(body: &Value, case: &str) -> Vec<(String, Option<String>)> {
    body.get("TagList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a TagList in {body}"))
        .iter()
        .map(|tag| {
            let key = tag
                .get("Key")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: expected a Key in {tag}"));
            (
                key.to_string(),
                tag.get("Value").and_then(Value::as_str).map(str::to_string),
            )
        })
        .collect()
}

/// `(Key, Value)` pairs written as a case expects them.
fn expected(pairs: &[(&str, Option<&str>)]) -> Vec<(String, Option<String>)> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.map(str::to_string)))
        .collect()
}

/// A `Tags` member built from `(Key, Value)` pairs; a `None` value is omitted, which is
/// the "null value" `Tag$Value` permits.
fn tags_member(pairs: &[(&str, Option<&str>)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| match value {
                Some(value) => json!({ "Key": key, "Value": value }),
                None => json!({ "Key": key }),
            })
            .collect(),
    )
}

/// `ListTags` on `resource_arn`, which must succeed, as key-ordered pairs.
async fn list_tags(
    router: &MultiServiceRouter,
    resource_arn: &str,
    case: &str,
) -> Vec<(String, Option<String>)> {
    let body = ok(
        router,
        "ListTags",
        &json!({ "ResourceArn": resource_arn }),
        case,
    )
    .await;
    tag_list(&body, case)
}

/// The tags a create's `Tags` member persisted are the tags `ListTags` reports, for
/// EVERY kind of resource this emulator creates and for every shape a tag set takes.
#[tokio::test]
async fn listtags_reports_the_tags_a_create_persisted() {
    /// The rows differ along what the rule generalises over: how many tags, whether a
    /// value was given, whether a key repeats, and the key order the request used.
    struct Case {
        label: &'static str,
        given: Option<&'static [(&'static str, Option<&'static str>)]>,
        reported: &'static [(&'static str, Option<&'static str>)],
    }
    const CASES: &[Case] = &[
        // No `Tags` member at all: the resource carries no tag.
        Case {
            label: "absent",
            given: None,
            reported: &[],
        },
        // An empty `Tags` list is within `TagList`'s `@length(min: 0)`.
        Case {
            label: "empty",
            given: Some(&[]),
            reported: &[],
        },
        Case {
            label: "one pair",
            given: Some(&[("team", Some("platform"))]),
            reported: &[("team", Some("platform"))],
        },
        // Reported in KEY order, not request order.
        Case {
            label: "three pairs out of order",
            given: Some(&[
                ("zone", Some("c")),
                ("env", Some("test")),
                ("owner", Some("db")),
            ]),
            reported: &[
                ("env", Some("test")),
                ("owner", Some("db")),
                ("zone", Some("c")),
            ],
        },
        // "A tag with a null Value is permitted" (`Tag`): the key is kept, and the tag
        // reports the empty value from then on.
        Case {
            label: "a key with no value",
            given: Some(&[("archived", None), ("env", Some("test"))]),
            reported: &[("archived", Some("")), ("env", Some("test"))],
        },
        // An empty value is what a null value becomes, so these two rows agree.
        Case {
            label: "an empty value",
            given: Some(&[("env", Some(""))]),
            reported: &[("env", Some(""))],
        },
        // A key identifies a tag, so a repeated key leaves ONE tag: the last value.
        Case {
            label: "a repeated key",
            given: Some(&[("env", Some("first")), ("env", Some("second"))]),
            reported: &[("env", Some("second"))],
        },
        // Keys and values are case-sensitive ("A tag is a key-value pair where the key
        // and value are case-sensitive"), so these are two different tags.
        Case {
            label: "case-sensitive keys",
            given: Some(&[("Env", Some("Prod")), ("env", Some("prod"))]),
            reported: &[("Env", Some("Prod")), ("env", Some("prod"))],
        },
    ];

    let router = router();
    for kind in creatable_kinds() {
        for (index, case) in CASES.iter().enumerate() {
            let case_label = format!("{} / {}", kind.resource_type, case.label);
            let name = format!("{}-tags-{index}", kind.resource_type);
            let given = case.given.map(tags_member);
            let resource_arn = create(&router, kind.resource_type, &name, given.as_ref()).await;
            let reported = list_tags(&router, &resource_arn, &case_label).await;
            assert_eq!(
                reported,
                expected(case.reported),
                "case {case_label:?}: expected ListTags to report the created tags in key \
                 order, got {reported:?}"
            );
            // A read changes nothing a later caller sees.
            let again = list_tags(&router, &resource_arn, &case_label).await;
            assert_eq!(
                again, reported,
                "case {case_label:?}: a second ListTags must agree with the first, got \
                 {again:?} after {reported:?}"
            );
        }
    }
}

/// `TagResource` merges into the tags already on the resource, and `ListTags` answers
/// from the same state — for every kind of resource this emulator creates.
#[tokio::test]
async fn tagresource_merges_into_the_tags_already_on_the_resource() {
    /// One step of a merge sequence: the `Tags` a call sends, and the resource's whole
    /// tag set afterwards. The steps differ along what a merge can do: add, overwrite,
    /// re-add an identical tag, add several at once, and add nothing.
    struct Step {
        label: &'static str,
        add: &'static [(&'static str, Option<&'static str>)],
        after: &'static [(&'static str, Option<&'static str>)],
    }
    const STEPS: &[Step] = &[
        Step {
            label: "add the first tag to a resource created without any",
            add: &[("env", Some("test"))],
            after: &[("env", Some("test"))],
        },
        Step {
            label: "add a second key, keeping the first",
            add: &[("owner", Some("db"))],
            after: &[("env", Some("test")), ("owner", Some("db"))],
        },
        Step {
            label: "overwrite an existing key, keeping the other",
            add: &[("env", Some("prod"))],
            after: &[("env", Some("prod")), ("owner", Some("db"))],
        },
        Step {
            label: "re-add an identical tag: idempotent",
            add: &[("env", Some("prod"))],
            after: &[("env", Some("prod")), ("owner", Some("db"))],
        },
        Step {
            label: "add several at once, one of them an overwrite",
            add: &[
                ("zone", Some("c")),
                ("owner", Some("ops")),
                ("archived", None),
            ],
            after: &[
                ("archived", Some("")),
                ("env", Some("prod")),
                ("owner", Some("ops")),
                ("zone", Some("c")),
            ],
        },
        Step {
            label: "an empty Tags list changes nothing",
            add: &[],
            after: &[
                ("archived", Some("")),
                ("env", Some("prod")),
                ("owner", Some("ops")),
                ("zone", Some("c")),
            ],
        },
    ];

    let router = router();
    for kind in creatable_kinds() {
        let name = format!("{}-merge", kind.resource_type);
        let resource_arn = create(&router, kind.resource_type, &name, None).await;
        // A second resource of the same kind, left alone: a merge touches ONE resource.
        let other_name = format!("{}-untouched", kind.resource_type);
        let other_arn = create(
            &router,
            kind.resource_type,
            &other_name,
            Some(&tags_member(&[("env", Some("other"))])),
        )
        .await;

        for step in STEPS {
            let case = format!("{} / {}", kind.resource_type, step.label);
            let answered = ok(
                &router,
                "TagResource",
                &json!({ "ResourceArn": resource_arn, "Tags": tags_member(step.add) }),
                &case,
            )
            .await;
            let answered = tag_list(&answered, &case);
            assert_eq!(
                answered,
                expected(step.after),
                "case {case:?}: TagResource must answer the resource's whole tag set in \
                 key order, got {answered:?}"
            );
            let reported = list_tags(&router, &resource_arn, &case).await;
            assert_eq!(
                reported, answered,
                "case {case:?}: ListTags must agree with what TagResource answered, got \
                 {reported:?} after {answered:?}"
            );
            let untouched = list_tags(&router, &other_arn, &case).await;
            assert_eq!(
                untouched,
                expected(&[("env", Some("other"))]),
                "case {case:?}: tagging one resource must not touch another, got \
                 {untouched:?}"
            );
        }
    }
}

/// `UntagResource` removes exactly the keys `TagKeys` names, answers with the tag set
/// the resource is LEFT with, and leaves every other tag where it was.
#[tokio::test]
async fn untagresource_removes_exactly_the_keys_it_is_given() {
    /// The rows differ along which and how many keys are removed, and whether a key is
    /// named twice.
    struct Case {
        label: &'static str,
        remove: &'static [&'static str],
        left: &'static [(&'static str, Option<&'static str>)],
    }
    /// The tag set every row starts from, as the create SENDS it: `archived` is given
    /// no value at all.
    const START: &[(&str, Option<&str>)] = &[
        ("archived", None),
        ("env", Some("test")),
        ("owner", Some("db")),
        ("zone", Some("c")),
    ];
    /// The same tag set as the service REPORTS it: a null value is stored as the empty
    /// one (`Tag`: "A tag with a null Value is permitted").
    const START_REPORTED: &[(&str, Option<&str>)] = &[
        ("archived", Some("")),
        ("env", Some("test")),
        ("owner", Some("db")),
        ("zone", Some("c")),
    ];
    const CASES: &[Case] = &[
        Case {
            label: "no keys at all removes nothing",
            remove: &[],
            left: START_REPORTED,
        },
        Case {
            label: "one key",
            remove: &["env"],
            left: &[
                ("archived", Some("")),
                ("owner", Some("db")),
                ("zone", Some("c")),
            ],
        },
        Case {
            label: "a key whose value was given as null",
            remove: &["archived"],
            left: &[
                ("env", Some("test")),
                ("owner", Some("db")),
                ("zone", Some("c")),
            ],
        },
        Case {
            label: "two keys at once",
            remove: &["zone", "env"],
            left: &[("archived", Some("")), ("owner", Some("db"))],
        },
        Case {
            label: "a key named twice is removed once",
            remove: &["owner", "owner"],
            left: &[
                ("archived", Some("")),
                ("env", Some("test")),
                ("zone", Some("c")),
            ],
        },
        Case {
            label: "every key",
            remove: &["archived", "env", "owner", "zone"],
            left: &[],
        },
    ];

    let router = router();
    for (index, case) in CASES.iter().enumerate() {
        // Each row gets its own resource, so a row starts from START whatever the row
        // before it removed. The kind rotates over the creatable ones: the rule is the
        // resource kind's, not one kind's.
        let kinds: Vec<&Kind> = creatable_kinds().collect();
        let kind = kinds[index % kinds.len()];
        let name = format!("{}-untag-{index}", kind.resource_type);
        let resource_arn = create(
            &router,
            kind.resource_type,
            &name,
            Some(&tags_member(START)),
        )
        .await;
        let label = format!("{} / {}", kind.resource_type, case.label);

        let answered = ok(
            &router,
            "UntagResource",
            &json!({ "ResourceArn": resource_arn, "TagKeys": case.remove }),
            &label,
        )
        .await;
        let answered = tag_list(&answered, &label);
        assert_eq!(
            answered,
            expected(case.left),
            "case {label:?}: UntagResource must answer the tag set the resource is left \
             with, in key order, got {answered:?}"
        );
        let left = list_tags(&router, &resource_arn, &label).await;
        assert_eq!(
            left, answered,
            "case {label:?}: ListTags must agree with what UntagResource answered, got \
             {left:?} after {answered:?}"
        );
    }
}

/// A tag mutation this service cannot honour in full changes NOTHING: the next
/// `ListTags` reports the tag set the resource had before the refused call.
#[tokio::test]
async fn a_refused_mutation_leaves_every_tag_where_it_was() {
    /// One refused request: the operation, its body beside `ResourceArn`, the HTTP
    /// status and the wire error code it must answer with.
    struct Case {
        label: &'static str,
        operation: &'static str,
        payload: fn() -> Value,
        status: u16,
        code: &'static str,
    }
    const START: &[(&str, Option<&str>)] = &[("env", Some("test")), ("owner", Some("db"))];
    const CASES: &[Case] = &[
        // `UntagResource` is atomic: one key the resource does not carry refuses the
        // whole request, including the keys it does carry.
        Case {
            label: "untag a key the resource never carried",
            operation: "UntagResource",
            payload: || json!({ "TagKeys": ["absent"] }),
            status: 404,
            code: "TagNotFoundFault",
        },
        Case {
            label: "untag one present and one absent key",
            operation: "UntagResource",
            payload: || json!({ "TagKeys": ["env", "absent"] }),
            status: 404,
            code: "TagNotFoundFault",
        },
        Case {
            label: "untag a key that is only on another resource",
            operation: "UntagResource",
            payload: || json!({ "TagKeys": ["elsewhere"] }),
            status: 404,
            code: "TagNotFoundFault",
        },
        // `Tag$Key` "may not be null", so a tag with no usable key is refused before
        // the tags beside it are written.
        Case {
            label: "tag with a keyless tag",
            operation: "TagResource",
            payload: || json!({ "Tags": [{ "Value": "nameless" }] }),
            status: 400,
            code: "InvalidParameterValueException",
        },
        Case {
            label: "tag with an empty key",
            operation: "TagResource",
            payload: || json!({ "Tags": [{ "Key": "", "Value": "empty" }] }),
            status: 400,
            code: "InvalidParameterValueException",
        },
        Case {
            label: "tag with a good tag beside a keyless one",
            operation: "TagResource",
            payload: || json!({ "Tags": [{ "Key": "zone", "Value": "c" }, { "Value": "x" }] }),
            status: 400,
            code: "InvalidParameterValueException",
        },
        // The `aws:` prefix is reserved for AWS: "You can't edit or delete tag names or
        // values with this prefix", in any letter case.
        Case {
            label: "tag under the reserved prefix",
            operation: "TagResource",
            payload: || json!({ "Tags": [{ "Key": "aws:owner", "Value": "x" }] }),
            status: 400,
            code: "InvalidParameterValueException",
        },
        Case {
            label: "tag under the reserved prefix in another letter case",
            operation: "TagResource",
            payload: || json!({ "Tags": [{ "Key": "AWS:owner", "Value": "x" }] }),
            status: 400,
            code: "InvalidParameterValueException",
        },
        Case {
            label: "untag a key under the reserved prefix",
            operation: "UntagResource",
            payload: || json!({ "TagKeys": ["aws:owner"] }),
            status: 400,
            code: "InvalidParameterValueException",
        },
        Case {
            label: "untag a key that is empty",
            operation: "UntagResource",
            payload: || json!({ "TagKeys": [""] }),
            status: 400,
            code: "InvalidParameterValueException",
        },
    ];

    let router = router();
    let resource_arn = create(
        &router,
        "cluster",
        "refusals",
        Some(&tags_member(START)),
    )
    .await;
    // The resource that carries the key one row names, to show a key is resolved
    // against the resource the ARN names and not against the store as a whole.
    create(
        &router,
        "acl",
        "refusals-elsewhere",
        Some(&tags_member(&[("elsewhere", Some("yes"))])),
    )
    .await;

    for case in CASES {
        let mut body = (case.payload)();
        body["ResourceArn"] = json!(resource_arn);
        let (status, answered) = call(&router, case.operation, &body).await;
        assert_eq!(
            status, case.status,
            "case {:?}: expected {} to answer {}, got {status} with {answered}",
            case.label, case.operation, case.status
        );
        let code = error_code(&answered);
        assert_eq!(
            code, case.code,
            "case {:?}: expected the error code {}, got {code} with {answered}",
            case.label, case.code
        );
        let left = list_tags(&router, &resource_arn, case.label).await;
        assert_eq!(
            left,
            expected(START),
            "case {:?}: a refused mutation must leave every tag where it was, got {left:?}",
            case.label
        );
    }
}

/// A resource carries at most `TagList`'s 200 tags: a merge that would push it past
/// that is the modelled quota fault and stores nothing, while a merge that only
/// overwrites keys the resource already has is accepted AT the bound.
#[tokio::test]
async fn a_merge_past_the_tag_quota_is_refused_and_stores_nothing() {
    let router = router();
    let resource_arn = create(&router, "cluster", "quota", None).await;

    // AT the bound: `TagList` permits 200 tags in one request, and 200 is also the most
    // a resource may hold, so this is the largest merge that can be accepted.
    let full: Vec<(String, Option<String>)> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index:03}"), Some(format!("value-{index}"))))
        .collect();
    let full_member = Value::Array(
        full.iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    );
    let case = "200 tags at once";
    let answered = ok(
        &router,
        "TagResource",
        &json!({ "ResourceArn": resource_arn, "Tags": full_member }),
        case,
    )
    .await;
    let answered = tag_list(&answered, case);
    assert_eq!(
        answered, full,
        "case {case:?}: expected the resource to carry all {MAX_TAGS_PER_RESOURCE} tags, \
         got {} of them",
        answered.len()
    );

    // Still at the bound: overwriting keys it already has adds no tag, so it is not a
    // quota breach.
    let case = "overwrite at the bound";
    let answered = ok(
        &router,
        "TagResource",
        &json!({
            "ResourceArn": resource_arn,
            "Tags": [{ "Key": "key-000", "Value": "rewritten" }],
        }),
        case,
    )
    .await;
    let answered = tag_list(&answered, case);
    assert_eq!(
        answered.len(),
        MAX_TAGS_PER_RESOURCE,
        "case {case:?}: an overwrite must not grow the tag set, got {} tags",
        answered.len()
    );
    assert_eq!(
        answered.first().map(|(key, value)| (key.as_str(), value.as_deref())),
        Some(("key-000", Some("rewritten"))),
        "case {case:?}: the overwritten key must carry the new value, got {answered:?}"
    );

    // ONE PAST the bound: a new key would make 201.
    let case = "one tag past the bound";
    let (status, refused) = call(
        &router,
        "TagResource",
        &json!({
            "ResourceArn": resource_arn,
            "Tags": [{ "Key": "key-200", "Value": "too-many" }],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the quota refusal to be a 400, got {status} with {refused}"
    );
    let code = error_code(&refused);
    assert_eq!(
        code, "TagQuotaPerResourceExceeded",
        "case {case:?}: expected the modelled quota fault, got {code} with {refused}"
    );

    // A request one past `TagList`'s own `@length` bound is refused too, by the
    // boundary validator rather than by the handler — either way with a 4xx and no
    // change to the stored tags.
    let case = "201 tags in one request";
    let oversized = Value::Array(
        (0..=MAX_TAGS_PER_RESOURCE)
            .map(|index| json!({ "Key": format!("over-{index:03}"), "Value": "x" }))
            .collect(),
    );
    let (status, refused) = call(
        &router,
        "TagResource",
        &json!({ "ResourceArn": resource_arn, "Tags": oversized }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a 400 for a Tags list past TagList's @length bound, got \
         {status} with {refused}"
    );

    // Nothing either refusal sent is on the resource, and the accepted set is intact.
    let case = "after the refusals";
    let left = list_tags(&router, &resource_arn, case).await;
    assert_eq!(
        left.len(),
        MAX_TAGS_PER_RESOURCE,
        "case {case:?}: a refused merge must store nothing, got {} tags",
        left.len()
    );
    assert!(
        left.iter().all(|(key, _)| key.starts_with("key-")),
        "case {case:?}: no refused key may be on the resource, got {left:?}"
    );
}

/// An ARN that names no resource of this store is that resource kind's modelled 404,
/// for every one of the three tagging operations — never an empty tag list, and never a
/// tag write.
#[tokio::test]
async fn an_arn_that_names_no_resource_is_that_kinds_not_found_fault() {
    let router = router();
    // One resource of each creatable kind exists under ANOTHER name, so a row's 404 is
    // about the name in its ARN rather than about an empty store.
    for kind in creatable_kinds() {
        create(
            &router,
            kind.resource_type,
            &format!("{}-present", kind.resource_type),
            Some(&tags_member(&[("env", Some("test"))])),
        )
        .await;
    }

    for kind in KINDS {
        let resource_arn = arn(kind.resource_type, "nothing-here");
        for (operation, payload) in [
            ("ListTags", json!({})),
            ("TagResource", json!({ "Tags": [{ "Key": "k", "Value": "v" }] })),
            ("UntagResource", json!({ "TagKeys": ["k"] })),
        ] {
            let case = format!("{} / {operation}", kind.resource_type);
            let mut body = payload;
            body["ResourceArn"] = json!(resource_arn);
            let (status, answered) = call(&router, operation, &body).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected a 404 for an ARN naming no resource, got {status} \
                 with {answered}"
            );
            let code = error_code(&answered);
            assert_eq!(
                code, kind.not_found,
                "case {case:?}: expected {}, got {code} with {answered}",
                kind.not_found
            );
        }
    }
}

/// An ARN is resolved WHOLE: a stored resource's name in another region, account or
/// partition names no resource, so it answers that kind's 404 rather than the stored
/// resource's tags.
#[tokio::test]
async fn an_arn_from_another_scope_does_not_resolve_to_a_stored_resource() {
    const OTHER_REGION: &str = "eu-west-1";
    let router = router();
    let resource_arn = create(
        &router,
        "cluster",
        "scoped",
        Some(&tags_member(&[("env", Some("test"))])),
    )
    .await;

    // The same name under a different region, account or partition.
    let rows = [
        (
            "another region",
            format!("arn:aws:memorydb:{OTHER_REGION}:{ACCOUNT}:cluster/scoped"),
        ),
        (
            "another account",
            format!("arn:aws:memorydb:{REGION}:999988887777:cluster/scoped"),
        ),
        (
            "another partition",
            format!("arn:aws-cn:memorydb:{REGION}:{ACCOUNT}:cluster/scoped"),
        ),
        (
            "another resource type",
            format!("arn:aws:memorydb:{REGION}:{ACCOUNT}:acl/scoped"),
        ),
    ];
    for (label, candidate) in rows {
        let (status, answered) = call(&router, "ListTags", &json!({ "ResourceArn": candidate })).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected a 404 for {candidate}, got {status} with {answered}"
        );
    }

    // And the resource itself is still reachable, from a call signed ELSEWHERE: the ARN
    // decides which resource is read, not the caller's region.
    let case = "the stored ARN read from another region";
    let (status, answered) = call_signed(
        &router,
        OTHER_REGION,
        "ListTags",
        &json!({ "ResourceArn": resource_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: got {status} with {answered}"
    );
    let reported = tag_list(&answered, case);
    assert_eq!(
        reported,
        expected(&[("env", Some("test"))]),
        "case {case:?}: expected the stored tags, got {reported:?}"
    );
}

/// An ARN that is not a MemoryDB resource ARN at all is the modelled 400
/// `InvalidARNFault`, for every one of the three tagging operations.
#[tokio::test]
async fn an_arn_that_is_not_a_memorydb_resource_arn_is_invalid() {
    let router = router();
    create(&router, "cluster", "valid-arn", None).await;

    // Each row breaks ONE part of `arn:<partition>:memorydb:<region>:<account>:<type>/<name>`.
    let candidates: [(&str, String); 11] = [
        ("empty", String::new()),
        ("not an ARN at all", "my-cluster".to_string()),
        (
            "no arn prefix",
            format!("urn:aws:memorydb:{REGION}:{ACCOUNT}:cluster/valid-arn"),
        ),
        (
            "another service",
            format!("arn:aws:elasticache:{REGION}:{ACCOUNT}:cluster/valid-arn"),
        ),
        (
            "no partition",
            format!("arn::memorydb:{REGION}:{ACCOUNT}:cluster/valid-arn"),
        ),
        (
            "no region",
            format!("arn:aws:memorydb::{ACCOUNT}:cluster/valid-arn"),
        ),
        ("no account", format!("arn:aws:memorydb:{REGION}::cluster/valid-arn")),
        (
            "too few fields",
            format!("arn:aws:memorydb:{REGION}:{ACCOUNT}"),
        ),
        (
            "a resource type MemoryDB does not tag",
            format!("arn:aws:memorydb:{REGION}:{ACCOUNT}:reservednode/valid-arn"),
        ),
        (
            "no resource name",
            format!("arn:aws:memorydb:{REGION}:{ACCOUNT}:cluster/"),
        ),
        (
            "no resource type separator",
            format!("arn:aws:memorydb:{REGION}:{ACCOUNT}:cluster"),
        ),
    ];

    for (label, candidate) in candidates {
        for (operation, payload) in [
            ("ListTags", json!({})),
            ("TagResource", json!({ "Tags": [{ "Key": "k", "Value": "v" }] })),
            ("UntagResource", json!({ "TagKeys": ["k"] })),
        ] {
            let case = format!("{label} / {operation}");
            let mut body = payload;
            body["ResourceArn"] = json!(candidate);
            let (status, answered) = call(&router, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected a 400 for {candidate:?}, got {status} with {answered}"
            );
            let code = error_code(&answered);
            assert_eq!(
                code, "InvalidARNFault",
                "case {case:?}: expected InvalidARNFault for {candidate:?}, got {code} with \
                 {answered}"
            );
        }
    }
}

/// The `TagNotFoundFault` of an untag NAMES the keys the resource does not carry: that
/// message is built from the request, so what it must say is a rule rather than one
/// recorded string. It names at most the first ten of them, so one response cannot grow
/// with the whole key list.
#[tokio::test]
async fn untagging_absent_keys_names_every_one_of_them() {
    /// How many missing keys the refusal spells out before it falls back to a count,
    /// mirrored from `UntagResource`'s own `MESSAGE_KEYS`. Exercised AT the bound and
    /// past it.
    const MESSAGE_KEYS: usize = 10;

    let router = router();
    let resource_arn = create(
        &router,
        "user",
        "missing-keys",
        Some(&tags_member(&[("env", Some("test"))])),
    )
    .await;

    // The rows differ along how many keys are missing and whether a present key is
    // named beside them; each expects only the MISSING keys, in request order.
    let rows: [(&str, &[&str], &str); 4] = [
        ("one absent key", &["absent"], "absent"),
        (
            "two absent keys, in request order",
            &["later", "earlier"],
            "later,earlier",
        ),
        (
            "an absent key beside a present one",
            &["env", "absent"],
            "absent",
        ),
        (
            "an absent key named twice",
            &["absent", "absent"],
            "absent",
        ),
    ];
    for (label, keys, named) in rows {
        let (status, answered) = call(
            &router,
            "UntagResource",
            &json!({ "ResourceArn": resource_arn, "TagKeys": keys }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected a 404 for keys {keys:?}, got {status} with {answered}"
        );
        let message = answered
            .get("message")
            .or_else(|| answered.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {label:?}: expected a message in {answered}"));
        assert_eq!(
            message,
            format!("Failing request. These tags are not present : {named}"),
            "case {label:?}: expected the message to name every absent key, got {message:?}"
        );
    }

    // AT the message bound, and past it: a longer list is summarised rather than spelled
    // out, so the response does not grow with the request.
    for extra in [0, 2] {
        let keys: Vec<String> = (0..MESSAGE_KEYS + extra)
            .map(|index| format!("absent-{index:02}"))
            .collect();
        let case = format!("{} absent keys", keys.len());
        let (status, answered) = call(
            &router,
            "UntagResource",
            &json!({ "ResourceArn": resource_arn, "TagKeys": keys }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected a 404, got {status} with {answered}"
        );
        let message = answered
            .get("message")
            .or_else(|| answered.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected a message in {answered}"))
            .to_string();
        let named = keys[..MESSAGE_KEYS].join(",");
        let wanted = if extra == 0 {
            format!("Failing request. These tags are not present : {named}")
        } else {
            format!("Failing request. These tags are not present : {named} and {extra} more")
        };
        assert_eq!(
            message, wanted,
            "case {case:?}: expected the message to name at most {MESSAGE_KEYS} keys, got \
             {message:?}"
        );
    }

    // A key list longer than the most tags a resource can carry can never name only keys
    // the resource has, so it is refused outright rather than scanned.
    let case = "one key past the per-resource tag maximum";
    let keys: Vec<String> = (0..=MAX_TAGS_PER_RESOURCE)
        .map(|index| format!("over-{index:03}"))
        .collect();
    let (status, answered) = call(
        &router,
        "UntagResource",
        &json!({ "ResourceArn": resource_arn, "TagKeys": keys }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a 400 for {} keys, got {status} with {answered}",
        keys.len()
    );
    let code = error_code(&answered);
    assert_eq!(
        code, "InvalidParameterValueException",
        "case {case:?}: expected InvalidParameterValueException, got {code} with {answered}"
    );
    // AT the maximum the list is accepted and reports the missing keys instead.
    let case = "exactly the per-resource tag maximum";
    let keys: Vec<String> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| format!("over-{index:03}"))
        .collect();
    let (status, answered) = call(
        &router,
        "UntagResource",
        &json!({ "ResourceArn": resource_arn, "TagKeys": keys }),
    )
    .await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected a 404 for {} keys, got {status} with {answered}",
        keys.len()
    );
    // And the resource still carries what it started with.
    let left = list_tags(&router, &resource_arn, case).await;
    assert_eq!(
        left,
        expected(&[("env", Some("test"))]),
        "case {case:?}: a refused untag must remove nothing, got {left:?}"
    );
}

/// The `aws:` tag-key prefix is AWS's own, so a CALLER cannot write a tag under it
/// WHEREVER this service accepts tags: not through a create's `Tags`, and not through
/// `TagResource`. The rule is the prefix's, not one key's, and it ignores letter case.
#[tokio::test]
async fn the_reserved_tag_key_prefix_is_refused_wherever_tags_are_accepted() {
    // Keys under the reserved prefix, differing in letter case and in what follows the
    // prefix (including nothing at all).
    const RESERVED: &[&str] = &["aws:owner", "AWS:owner", "Aws:cost-centre", "aws:"];
    // Keys that merely resemble it: the prefix is `aws:`, so these are a caller's own.
    const ALLOWED: &[&str] = &["aws", "awsomething", "my-aws:owner", "a:ws"];

    let router = router();
    for kind in creatable_kinds() {
        for (index, key) in RESERVED.iter().enumerate() {
            let case = format!("{} / create with {key}", kind.resource_type);
            let name = format!("{}-reserved-{index}", kind.resource_type);
            let (operation, body) = create_request(
                &router,
                kind.resource_type,
                &name,
                Some(&tags_member(&[(key, Some("x"))])),
            )
            .await;
            let (status, answered) = call(&router, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected {operation} to refuse a reserved tag key, got \
                 {status} with {answered}"
            );
            let code = error_code(&answered);
            assert_eq!(
                code, "InvalidParameterValueException",
                "case {case:?}: expected InvalidParameterValueException, got {code} with \
                 {answered}"
            );
        }
    }

    // A key that only LOOKS like the reserved one is a caller's to use, through a
    // create and through `TagResource` alike.
    let resource_arn = create(&router, "cluster", "lookalike-keys", None).await;
    for key in ALLOWED {
        let case = format!("TagResource with {key}");
        let answered = ok(
            &router,
            "TagResource",
            &json!({ "ResourceArn": resource_arn, "Tags": tags_member(&[(key, Some("mine"))]) }),
            &case,
        )
        .await;
        let answered = tag_list(&answered, &case);
        assert!(
            answered
                .iter()
                .any(|(stored, value)| stored == key && value.as_deref() == Some("mine")),
            "case {case:?}: expected the tag to be written, got {answered:?}"
        );
    }
}

/// A resource MemoryDB itself provides is not a caller's to tag: a tag mutation aimed
/// at one of the `default.<family>` parameter groups every account starts with is the
/// modelled 400, while a group the CALLER created is tagged normally even if its name
/// mentions the word.
#[tokio::test]
async fn a_default_resource_is_not_a_tag_target() {
    // The default parameter groups MemoryDB publishes are named `default.<family>` for
    // the families of its engines (`default.memorydb-redis7`, `default.memorydb-valkey7`
    // …), so the rule is the `default.` prefix rather than any one of these names.
    const DEFAULTS: &[(&str, &str)] = &[
        ("parametergroup", "default.memorydb-redis7"),
        ("parametergroup", "default.memorydb-redis6"),
        ("parametergroup", "default.memorydb-valkey7"),
        ("multiregionparametergroup", "default.memorydb-redis7"),
    ];

    let router = router();
    for (resource_type, name) in DEFAULTS {
        let resource_arn = arn(resource_type, name);
        for (operation, payload) in [
            (
                "TagResource",
                json!({ "Tags": [{ "Key": "env", "Value": "test" }] }),
            ),
            ("UntagResource", json!({ "TagKeys": ["env"] })),
        ] {
            let case = format!("{name} / {operation}");
            let mut body = payload;
            body["ResourceArn"] = json!(resource_arn);
            let (status, answered) = call(&router, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected a tag mutation on a default resource to be \
                 refused, got {status} with {answered}"
            );
            let code = error_code(&answered);
            assert_eq!(
                code, "InvalidParameterValueException",
                "case {case:?}: expected InvalidParameterValueException, got {code} with \
                 {answered}"
            );
        }
    }

    // A group the caller created is theirs to tag, whatever its name says.
    let case = "a caller's own group named for a default";
    let resource_arn = create(&router, "parametergroup", "my-default-copy", None).await;
    let answered = ok(
        &router,
        "TagResource",
        &json!({
            "ResourceArn": resource_arn,
            "Tags": [{ "Key": "env", "Value": "test" }],
        }),
        case,
    )
    .await;
    let answered = tag_list(&answered, case);
    assert_eq!(
        answered,
        expected(&[("env", Some("test"))]),
        "case {case:?}: expected the caller's own group to be tagged, got {answered:?}"
    );
}

/// A resource the account ALREADY HAD is a tag target like any other: MemoryDB's
/// built-in `default` user carries no tag until one is written, and then carries
/// exactly that one.
#[tokio::test]
async fn the_builtin_user_is_a_tag_target() {
    let router = router();
    let resource_arn = arn("user", "default");

    let case = "the built-in user before any tag";
    let reported = list_tags(&router, &resource_arn, case).await;
    assert_eq!(
        reported,
        expected(&[]),
        "case {case:?}: expected no tags on the built-in user, got {reported:?}"
    );

    let case = "the built-in user after a tag";
    let answered = ok(
        &router,
        "TagResource",
        &json!({
            "ResourceArn": resource_arn,
            "Tags": [{ "Key": "managed-by", "Value": "memorydb" }],
        }),
        case,
    )
    .await;
    let answered = tag_list(&answered, case);
    assert_eq!(
        answered,
        expected(&[("managed-by", Some("memorydb"))]),
        "case {case:?}: expected the written tag, got {answered:?}"
    );
    let reported = list_tags(&router, &resource_arn, case).await;
    assert_eq!(
        reported, answered,
        "case {case:?}: ListTags must agree with TagResource, got {reported:?}"
    );
}
