// SPDX-License-Identifier: CC0-1.0

//! What `DeleteTags` removes from a stored resource, and what it refuses — end to end over
//! the wire the service speaks (`ec2Query`, through the shared [`super::ec2_query`]
//! harness).
//!
//! The rules under test are:
//!
//! 1. `DeleteTags` removes from each resource it names exactly the tags the request
//!    SELECTS: a key given without a value whatever value it holds, a key given with a
//!    value only when the stored value is that value (the empty value included), and every
//!    user-defined tag when the request names no tag at all. A tag the resource does not
//!    carry removes nothing, so the call is idempotent;
//! 2. that holds for a resource of ANY kind this service stores tags for — the tag
//!    operations resolve a resource id without being told its kind — and a resource tagged
//!    through `CreateTags` reads back through that kind's own describe;
//! 3. a request naming an id this account holds no resource for, or a tag key the service
//!    will not act on, is refused with EC2's own error code and removes NOTHING, and a
//!    `DryRun` reports the outcome without removing anything either.
//!
//! The rows of each table are distinct inputs along what the rule generalises over: which
//! form the selector takes, how many tags it names, which kind of resource holds them, and
//! which id or key makes the request refusable. No row is the specification.

use std::collections::BTreeMap;

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The image `RunInstances` launches from, in the long id form this service accepts.
const AMI: &str = "ami-0123456789abcdef0";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// The tags the request stores on every resource a row works on, chosen so that each
/// selector form below has both a tag it must remove and tags it must leave alone: a
/// plain value, a second plain value, an EMPTY value, and a key whose case differs.
const STORED_TAGS: [(&str, &str); 4] = [
    ("env", "prod"),
    ("owner", "team-a"),
    ("blank", ""),
    ("Name", "demo"),
];

/// Every resource kind this suite tags. `CreateTags` and `DeleteTags` take a resource id
/// of any kind, and these differ in how the store holds the resource's tags (the instance
/// record keeps a tag MAP, every other kind the modelled tag LIST) and in the id form the
/// request carries.
const KINDS: [&str; 5] = [
    "instance",
    "vpc",
    "elastic-ip",
    "security-group",
    "subnet",
];

/// Create one resource of `kind` and answer its resource id.
async fn create_resource(router: &MultiServiceRouter, case: &str, kind: &str) -> String {
    match kind {
        "instance" => {
            created_id(
                router,
                case,
                "RunInstances",
                &[p("ImageId", AMI), p("MinCount", "1"), p("MaxCount", "1")],
                "instanceId",
            )
            .await
        }
        "vpc" => create_vpc(router, case, "10.0.0.0/16").await,
        "elastic-ip" => {
            created_id(
                router,
                case,
                "AllocateAddress",
                &[p("Domain", "vpc")],
                "allocationId",
            )
            .await
        }
        "security-group" => {
            let vpc_id = create_vpc(router, case, "10.1.0.0/16").await;
            created_id(
                router,
                case,
                "CreateSecurityGroup",
                &[
                    p("GroupName", "tagged-group"),
                    p("GroupDescription", "a group this suite tags"),
                    p("VpcId", &vpc_id),
                ],
                "groupId",
            )
            .await
        }
        "subnet" => {
            let vpc_id = create_vpc(router, case, "10.2.0.0/16").await;
            created_id(
                router,
                case,
                "CreateSubnet",
                &[p("VpcId", &vpc_id), p("CidrBlock", "10.2.1.0/24")],
                "subnetId",
            )
            .await
        }
        other => panic!("case {case:?}: this suite knows no resource kind {other:?}"),
    }
}

/// One VPC, as the owner of the kinds that need one.
async fn create_vpc(router: &MultiServiceRouter, case: &str, cidr: &str) -> String {
    created_id(
        router,
        case,
        "CreateVpc",
        &[p("CidrBlock", cidr)],
        "vpcId",
    )
    .await
}

/// Call `action`, require HTTP 200, and answer the `id_element` of the response.
async fn created_id(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(String, String)],
    id_element: &str,
) -> String {
    let (status, body) = call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {action} must succeed, got {status}: {body}"
    );
    element(&body, id_element)
        .unwrap_or_else(|| panic!("case {case:?}: expected a <{id_element}> in {body}"))
}

/// The describe that reports `kind`'s tags, and the parameter that narrows it to one
/// resource.
fn describe_of(kind: &str) -> (&'static str, &'static str) {
    match kind {
        "instance" => ("DescribeInstances", "InstanceId.1"),
        "vpc" => ("DescribeVpcs", "VpcId.1"),
        "elastic-ip" => ("DescribeAddresses", "AllocationId.1"),
        "security-group" => ("DescribeSecurityGroups", "GroupId.1"),
        "subnet" => ("DescribeSubnets", "SubnetId.1"),
        other => panic!("this suite knows no resource kind {other:?}"),
    }
}

/// Every `tagSet` entry of a response body, as key -> value. The describes are narrowed to
/// one resource, so this is that resource's tags as the service reports them.
fn reported_tags(case: &str, body: &str) -> BTreeMap<String, String> {
    let document = roxmltree::Document::parse(body)
        .unwrap_or_else(|error| panic!("case {case:?}: the body must be XML ({error}): {body}"));
    document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "tagSet")
        .flat_map(|set| {
            set.children()
                .filter(|node| node.is_element() && node.tag_name().name() == "item")
                .collect::<Vec<_>>()
        })
        .filter_map(|item| {
            let text = |name: &str| {
                item.children()
                    .find(|node| node.is_element() && node.tag_name().name() == name)
                    .map(|node| node.text().unwrap_or_default().to_string())
            };
            Some((text("key")?, text("value").unwrap_or_default()))
        })
        .collect()
}

/// The tags `kind`'s own describe reports for `resource_id` — the round trip every
/// assertion about a write reads through.
async fn described_tags(
    router: &MultiServiceRouter,
    case: &str,
    kind: &str,
    resource_id: &str,
) -> BTreeMap<String, String> {
    let (action, narrow) = describe_of(kind);
    let (status, body) = call(router, REGION, action, &[p(narrow, resource_id)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {action} for {resource_id} must succeed, got {status}: {body}"
    );
    reported_tags(case, &body)
}

/// The `Tag.N.*` form fields for a list of selectors: `None` is the bare-key form (no
/// `Tag.N.Value` field at all), `Some(value)` the key-with-value one.
fn tag_fields(selectors: &[(&str, Option<&str>)]) -> Params {
    let mut params = Vec::new();
    for (index, (key, value)) in selectors.iter().enumerate() {
        params.push(p(&format!("Tag.{}.Key", index + 1), *key));
        if let Some(value) = value {
            params.push(p(&format!("Tag.{}.Value", index + 1), *value));
        }
    }
    params
}

/// Write `STORED_TAGS` onto `resource_id` through `CreateTags`, and prove the describe
/// reports them before anything is removed.
async fn tag_resource(router: &MultiServiceRouter, case: &str, kind: &str, resource_id: &str) {
    let mut params = vec![p("ResourceId.1", resource_id)];
    for (index, (key, value)) in STORED_TAGS.iter().enumerate() {
        params.push(p(&format!("Tag.{}.Key", index + 1), *key));
        params.push(p(&format!("Tag.{}.Value", index + 1), *value));
    }
    let (status, body) = call(router, REGION, "CreateTags", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateTags on a {kind} must succeed, got {status}: {body}"
    );
    let expected: BTreeMap<String, String> = STORED_TAGS
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    let reported = described_tags(router, case, kind, resource_id).await;
    assert_eq!(
        reported, expected,
        "case {case:?}: the {kind} must report the tags CreateTags wrote, got {reported:?}"
    );
}

/// One row of the selection table: the case, the tags the request names, and the keys the
/// resource must still carry afterwards.
struct SelectionRow {
    case: &'static str,
    selectors: &'static [(&'static str, Option<&'static str>)],
    remaining: &'static [&'static str],
}

/// Rule 1 and rule 2: the tags a request selects go, the rest stay, and that is true of
/// every resource kind the service stores tags for.
#[tokio::test]
async fn delete_tags_removes_exactly_the_tags_the_request_selects() {
    let rows: [SelectionRow; 9] = [
        SelectionRow {
            case: "a key with no value removes it whatever value it holds",
            selectors: &[("env", None)],
            remaining: &["owner", "blank", "Name"],
        },
        SelectionRow {
            case: "a key with the value it holds removes it",
            selectors: &[("owner", Some("team-a"))],
            remaining: &["env", "blank", "Name"],
        },
        SelectionRow {
            case: "a key with another value removes nothing",
            selectors: &[("owner", Some("team-b"))],
            remaining: &["env", "owner", "blank", "Name"],
        },
        SelectionRow {
            case: "a key with the empty value removes an empty-valued tag",
            selectors: &[("blank", Some(""))],
            remaining: &["env", "owner", "Name"],
        },
        SelectionRow {
            case: "a key with the empty value leaves a tag whose value is not empty",
            selectors: &[("env", Some(""))],
            remaining: &["env", "owner", "blank", "Name"],
        },
        SelectionRow {
            case: "two keys at once remove both",
            selectors: &[("env", None), ("Name", Some("demo"))],
            remaining: &["owner", "blank"],
        },
        SelectionRow {
            case: "a key the resource does not carry removes nothing",
            selectors: &[("absent", None)],
            remaining: &["env", "owner", "blank", "Name"],
        },
        SelectionRow {
            case: "one key selected twice, bare and with a value it does not hold",
            selectors: &[("env", None), ("env", Some("nope"))],
            remaining: &["owner", "blank", "Name"],
        },
        SelectionRow {
            case: "no tag named at all removes every user-defined tag",
            selectors: &[],
            remaining: &[],
        },
    ];

    for kind in KINDS {
        for row in &rows {
            let case = format!("{kind}: {}", row.case);
            let router = emulator();
            let resource_id = create_resource(&router, &case, kind).await;
            tag_resource(&router, &case, kind, &resource_id).await;

            let mut params = vec![p("ResourceId.1", &resource_id)];
            params.extend(tag_fields(row.selectors));
            let (status, body) = call(&router, REGION, "DeleteTags", &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DeleteTags must succeed, got {status}: {body}"
            );

            let expected: BTreeMap<String, String> = STORED_TAGS
                .iter()
                .filter(|(key, _)| row.remaining.contains(key))
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect();
            let reported = described_tags(&router, &case, kind, &resource_id).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the tags {expected:?} to be left, got {reported:?}"
            );

            // Rule 1's idempotence: the same request again is still served, and the
            // resource is left exactly as the first call left it.
            let (again_status, again_body) = call(&router, REGION, "DeleteTags", &params).await;
            assert_eq!(
                again_status, 200,
                "case {case:?}: a repeat of the same DeleteTags must succeed, \
                 got {again_status}: {again_body}"
            );
            let after = described_tags(&router, &case, kind, &resource_id).await;
            assert_eq!(
                after, expected,
                "case {case:?}: a repeated DeleteTags must remove nothing further, got {after:?}"
            );
        }
    }
}

/// Rule 1 over a BATCH: one request untags several resources of different kinds at once,
/// and a resource it does not name keeps its tags.
#[tokio::test]
async fn delete_tags_untags_every_resource_the_request_names() {
    let case = "a batch over three kinds";
    let router = emulator();
    let mut tagged: Vec<(&str, String)> = Vec::new();
    for kind in ["instance", "vpc", "elastic-ip"] {
        let resource_id = create_resource(&router, case, kind).await;
        tag_resource(&router, case, kind, &resource_id).await;
        tagged.push((kind, resource_id));
    }
    // A second address, tagged the same way and NOT named in the request below.
    let untouched = create_resource(&router, case, "elastic-ip").await;
    tag_resource(&router, case, "elastic-ip", &untouched).await;

    let mut params: Params = tagged
        .iter()
        .enumerate()
        .map(|(index, (_, id))| p(&format!("ResourceId.{}", index + 1), id.clone()))
        // The same id named twice is one resource: a repeat must not change the outcome.
        .chain(std::iter::once(p(
            &format!("ResourceId.{}", tagged.len() + 1),
            tagged[0].1.clone(),
        )))
        .collect();
    params.extend(tag_fields(&[("env", None), ("blank", Some(""))]));
    let (status, body) = call(&router, REGION, "DeleteTags", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteTags over a batch must succeed, got {status}: {body}"
    );

    let expected: BTreeMap<String, String> = [
        ("owner".to_string(), "team-a".to_string()),
        ("Name".to_string(), "demo".to_string()),
    ]
    .into_iter()
    .collect();
    for (kind, resource_id) in &tagged {
        let reported = described_tags(&router, case, kind, resource_id).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: the {kind} named in the batch must keep only {expected:?}, \
             got {reported:?}"
        );
    }
    let all: BTreeMap<String, String> = STORED_TAGS
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    let reported = described_tags(&router, case, "elastic-ip", &untouched).await;
    assert_eq!(
        reported, all,
        "case {case:?}: a resource the request did not name must keep every tag, \
         got {reported:?}"
    );
}

/// Rule 3: what `DeleteTags` refuses, with the code EC2 answers — and every refusal leaves
/// the tags of the resource the request named exactly as they were.
#[tokio::test]
async fn delete_tags_refuses_an_unresolvable_id_or_an_unactionable_key() {
    // `(case, the parameters beyond the resource id, the id to name, the status, the code)`
    let rows: [(&str, Params, Option<&str>, u16, &str); 11] = [
        (
            "a well-formed instance id nothing was launched for",
            tag_fields(&[("env", None)]),
            Some("i-00000000000000999"),
            400,
            "InvalidInstanceID.NotFound",
        ),
        (
            // An Elastic IP allocation is a taggable resource whose own not-found refusal
            // this crate does not word for a tagging call, so an `eipalloc-` id nothing holds
            // is still refused as an id the tagging calls cannot resolve.
            "an allocation id this account does not hold",
            tag_fields(&[("env", None)]),
            Some("eipalloc-00000000000000999"),
            400,
            "InvalidID",
        ),
        (
            // A well-formed id of a kind EC2 publishes names a resource that does not exist,
            // which is that kind's own not-found refusal — the same one every other
            // operation of the resource answers for the id.
            "a VPC id this account does not hold",
            tag_fields(&[("env", None)]),
            Some("vpc-00000000000000999"),
            400,
            "InvalidVpcID.NotFound",
        ),
        (
            "a subnet id this account does not hold",
            tag_fields(&[("env", None)]),
            Some("subnet-00000000000000999"),
            400,
            "InvalidSubnetID.NotFound",
        ),
        (
            "a security group id this account does not hold",
            tag_fields(&[("env", None)]),
            Some("sg-00000000000000999"),
            400,
            "InvalidGroup.NotFound",
        ),
        (
            // Not a well-formed id of any kind: `InvalidID` is what the tagging calls answer
            // for a value that is no resource id at all.
            "a subnet id that is not well formed",
            tag_fields(&[("env", None)]),
            Some("subnet-zzz"),
            400,
            "InvalidID",
        ),
        (
            "a value that is no resource id at all",
            tag_fields(&[("env", None)]),
            Some("not-an-id"),
            400,
            "InvalidID",
        ),
        (
            "a reserved tag key, which names a tag the service generates",
            tag_fields(&[("aws:cloudformation:stack-name", None)]),
            None,
            400,
            "InvalidParameterValue",
        ),
        (
            "an empty tag key, which names no tag",
            tag_fields(&[("", None)]),
            None,
            400,
            "InvalidParameterValue",
        ),
        (
            // `Resources` is `@required`, so the boundary validator refuses a request
            // carrying none of it before the handler is reached at all.
            "no resource named at all",
            tag_fields(&[("env", None)]),
            Some(""),
            400,
            "MalformedInput",
        ),
        (
            "a DryRun that would have succeeded",
            {
                let mut params = tag_fields(&[("env", None)]);
                params.push(p("DryRun", "true"));
                params
            },
            None,
            412,
            "DryRunOperation",
        ),
    ];

    for (case, extra, named_id, expected_status, expected_code) in rows {
        let router = emulator();
        let resource_id = create_resource(&router, case, "elastic-ip").await;
        tag_resource(&router, case, "elastic-ip", &resource_id).await;

        let mut params: Params = match named_id {
            // The empty string stands for "no ResourceId field at all".
            Some("") => Vec::new(),
            Some(other) => vec![p("ResourceId.1", other)],
            None => vec![p("ResourceId.1", &resource_id)],
        };
        params.extend(extra);
        let (status, body) = call(&router, REGION, "DeleteTags", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the error code {expected_code}, got {body}"
        );

        let all: BTreeMap<String, String> = STORED_TAGS
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        let reported = described_tags(&router, case, "elastic-ip", &resource_id).await;
        assert_eq!(
            reported, all,
            "case {case:?}: a refused DeleteTags must remove nothing, got {reported:?}"
        );
    }
}

/// Rule 3 over a batch: a request naming one id the account holds no resource for removes
/// nothing at all, not even from the ids beside it that do resolve.
#[tokio::test]
async fn one_unresolvable_id_leaves_every_other_resource_tagged() {
    let case = "a batch with one unknown id";
    let router = emulator();
    let first = create_resource(&router, case, "elastic-ip").await;
    tag_resource(&router, case, "elastic-ip", &first).await;
    let second = create_resource(&router, case, "vpc").await;
    tag_resource(&router, case, "vpc", &second).await;

    let mut params = vec![
        p("ResourceId.1", &first),
        p("ResourceId.2", "eipalloc-00000000000000999"),
        p("ResourceId.3", &second),
    ];
    params.extend(tag_fields(&[("env", None)]));
    let (status, body) = call(&router, REGION, "DeleteTags", &params).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a 400 refusal, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidID",
        "case {case:?}: expected InvalidID, got {body}"
    );

    let all: BTreeMap<String, String> = STORED_TAGS
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    for (kind, resource_id) in [("elastic-ip", &first), ("vpc", &second)] {
        let reported = described_tags(&router, case, kind, resource_id).await;
        assert_eq!(
            reported, all,
            "case {case:?}: the {kind} must keep every tag, got {reported:?}"
        );
    }
}

/// Rule 3 for `CreateTags`, over the resource KINDS a tagging call resolves: a resource this
/// account created and then DELETED is tagged with that kind's own not-found refusal — the
/// same one every other operation of the resource answers for its id — while a value that is
/// no resource id of any kind is refused as the id it is not.
#[tokio::test]
async fn create_tags_refuses_a_deleted_resource_with_the_kinds_own_not_found() {
    // `(case, the kind to create and delete, the delete that takes it away, the member that
    // names it there, the error code the tagging call must answer)`
    let rows: [(&str, &str, &str, &str, &str); 3] = [
        (
            "a subnet the account deleted",
            "subnet",
            "DeleteSubnet",
            "SubnetId",
            "InvalidSubnetID.NotFound",
        ),
        (
            "a security group the account deleted",
            "security-group",
            "DeleteSecurityGroup",
            "GroupId",
            "InvalidGroup.NotFound",
        ),
        (
            "a VPC the account deleted",
            "vpc",
            "DeleteVpc",
            "VpcId",
            "InvalidVpcID.NotFound",
        ),
    ];

    for (case, kind, delete, id_member, expected_code) in rows {
        let router = emulator();
        let resource_id = create_resource(&router, case, kind).await;
        // The resource is tagged while it exists, so the refusal below is about the resource
        // being gone and not about the request.
        tag_resource(&router, case, kind, &resource_id).await;
        let (status, body) = call(
            &router,
            REGION,
            delete,
            &[p(id_member, resource_id.clone())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: {delete} must succeed, got {status}: {body}"
        );

        let mut params = vec![p("ResourceId.1", resource_id.clone())];
        params.extend(tag_fields(&[("env", Some("prod"))]));
        let (status, body) = call(&router, REGION, "CreateTags", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: tagging a resource that is gone must be refused, got {status}: \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the error code {expected_code} for {resource_id}, got \
             {body}"
        );
    }

    // A value that is no resource id of any kind EC2 publishes is refused as such, whatever
    // kind's prefix it starts with. A well-formed id of a kind nothing was ever created for
    // is the other side of the same rule: it names a resource that does not exist.
    for (resource_id, expected_code) in [
        ("not-an-id", "InvalidID"),
        ("subnet-zzzz", "InvalidID"),
        ("sg-", "InvalidID"),
        ("vpc-012345", "InvalidID"),
        ("i-00000000000000999", "InvalidInstanceID.NotFound"),
        ("vol-00000000000000999", "InvalidVolume.NotFound"),
        ("eni-00000000000000999", "InvalidNetworkInterfaceID.NotFound"),
        ("lt-00000000000000999", "InvalidLaunchTemplateId.NotFound"),
    ] {
        let router = emulator();
        let mut params = vec![p("ResourceId.1", resource_id)];
        params.extend(tag_fields(&[("env", Some("prod"))]));
        let (status, body) = call(&router, REGION, "CreateTags", &params).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, expected_code.to_string()),
            "case {resource_id:?}: expected {expected_code}, got {status}: {body}"
        );
    }
}

/// Rule 1's byte accounting: tagging and untagging the same resource over and over gives
/// the store's byte budget back, so the budget cannot drift upwards. Proven through the
/// public API: the number of tags a resource may carry is bounded, so a drifting budget
/// shows up as a refusal of a write the resource has room for.
#[tokio::test]
async fn untagging_gives_the_stored_bytes_back() {
    let case = "a long tag written and removed repeatedly";
    let router = emulator();
    let resource_id = create_resource(&router, case, "elastic-ip").await;
    // A value long enough that a leaked charge would be visible, written and removed
    // enough times to leak far more than the whole budget if it leaked at all.
    let value = "v".repeat(256);
    for round in 0..64 {
        let (status, body) = call(
            &router,
            REGION,
            "CreateTags",
            &[
                p("ResourceId.1", &resource_id),
                p("Tag.1.Key", "big"),
                p("Tag.1.Value", value.clone()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?} round {round}: CreateTags must succeed, got {status}: {body}"
        );
        let (status, body) = call(
            &router,
            REGION,
            "DeleteTags",
            &[p("ResourceId.1", &resource_id), p("Tag.1.Key", "big")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?} round {round}: DeleteTags must succeed, got {status}: {body}"
        );
        let reported = described_tags(&router, case, "elastic-ip", &resource_id).await;
        assert!(
            reported.is_empty(),
            "case {case:?} round {round}: the address must carry no tag, got {reported:?}"
        );
    }
}
