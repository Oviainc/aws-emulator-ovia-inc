// SPDX-License-Identifier: CC0-1.0

//! The tag plane end to end: `AddTagsToResource` writes, `ListTagsForResource` reports,
//! `RemoveTagsFromResource` erases.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`router`], [`call`], [`call_in_region`]) dispatches
//! `POST /` with `X-Amz-Target: AmazonSSM.<Operation>` through the multi-service router.
//! Nothing reaches into the state struct, so a response that is not computed from
//! persisted state cannot pass: every tag read follows a tag write, and the resources
//! being tagged are created through `PutParameter` and `CreateDocument` first.
//!
//! The suites are tables. Each row is one input along the axis the rule under test
//! generalises over — the resource type addressed, the id form, the tag count, the key
//! already held or not, the value form, which keys a remove names, the Region — rather
//! than the single resource a recorded unit happened to tag.

use serde_json::{json, Value};
use service_com_amazonaws_ssm_amazonssm::ops::types::InstanceProperty;
use service_com_amazonaws_ssm_amazonssm::state::{ComAmazonawsSsmAmazonssmState, ManagedNodeRecord};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{
    call, call_in_region, error_code, router, router_with_engine, ACCOUNT, REGION,
};

/// How many tags one resource may carry
/// (`crate::state::tagging::MAX_TAGS_PER_RESOURCE`, from `AddTagsToResource`'s own doc:
/// "Most resources can have a maximum of 50 tags").
const MAX_TAGS: usize = service_com_amazonaws_ssm_amazonssm::state::tagging::MAX_TAGS_PER_RESOURCE;

/// A second Region, so a tag set is shown to live with the resource of ONE Region.
const OTHER_REGION: &str = "eu-central-1";

/// A minimal `Command` document, so a document exists to tag.
const DOCUMENT_CONTENT: &str = r#"{
  "schemaVersion": "2.2",
  "description": "A document to tag",
  "mainSteps": [
    {
      "action": "aws:runShellScript",
      "name": "runShell",
      "inputs": { "runCommand": ["echo hi"] }
    }
  ]
}"#;

/// The `ResourceTypeForTagging` values the model's enum declares
/// (`smithy_query(query="shape", shape="ResourceTypeForTagging")`), each beside an id of
/// the form `AddTagsToResourceRequest$ResourceId` documents for it: `mw-012345abcde` for
/// a maintenance window, `pb-012345abcde` for a patch baseline,
/// `example-c160-4567-8519-012345abcde` for an automation, the name for a document or a
/// parameter, `mi-1a2b3c4d5e6f` for a managed node, and the ARN tail for an OpsMetadata
/// object. Each id is well formed for its type and names nothing: the `OpsItem` row has
/// the form the modelled `OpsItemId` pattern requires (`^(oi)-[0-9a-f]{12}$`), and the
/// `CloudConnector` row the 36-character id a `CloudConnectorId` is.
const TAGGABLE_TYPES: &[(&str, &str)] = &[
    ("Association", "aa112233-4455-6677-8899-aabbccddeeff"),
    ("Automation", "example-c160-4567-8519-012345abcde"),
    ("CloudConnector", "ffffffff-ffff-ffff-ffff-ffffffffffff"),
    ("Document", "ADocumentNobodyCreated"),
    ("MaintenanceWindow", "mw-012345abcde"),
    ("ManagedInstance", "mi-1a2b3c4d5e6f"),
    ("OpsItem", "oi-0123456789ab"),
    ("OpsMetadata", "aws/ssm/MyGroup/appmanager"),
    ("Parameter", "/Tags/NobodyPutThis"),
    ("PatchBaseline", "pb-012345abcde"),
];

/// `AddTagsToResource`, returning `(status, body)`.
async fn add(
    router: &MultiServiceRouter,
    resource_type: &str,
    resource_id: &str,
    tags: &Value,
) -> (u16, Value) {
    call(
        router,
        "AddTagsToResource",
        &json!({ "ResourceType": resource_type, "ResourceId": resource_id, "Tags": tags }),
    )
    .await
}

/// `AddTagsToResource`, asserting it succeeded.
async fn add_ok(
    router: &MultiServiceRouter,
    case: &str,
    resource_type: &str,
    resource_id: &str,
    tags: &Value,
) {
    let (status, response) = add(router, resource_type, resource_id, tags).await;
    assert_eq!(
        status, 200,
        "case {case}: AddTagsToResource {resource_type} {resource_id} must succeed, \
         got {status} {response}"
    );
}

/// `ListTagsForResource`, returning `(status, body)`.
async fn list(
    router: &MultiServiceRouter,
    resource_type: &str,
    resource_id: &str,
) -> (u16, Value) {
    call(
        router,
        "ListTagsForResource",
        &json!({ "ResourceType": resource_type, "ResourceId": resource_id }),
    )
    .await
}

/// The `TagList` a resource reports, asserting the read succeeded.
async fn listed(
    router: &MultiServiceRouter,
    case: &str,
    resource_type: &str,
    resource_id: &str,
) -> Value {
    let (status, response) = list(router, resource_type, resource_id).await;
    assert_eq!(
        status, 200,
        "case {case}: ListTagsForResource {resource_type} {resource_id} must succeed, \
         got {status} {response}"
    );
    response["TagList"].clone()
}

/// `RemoveTagsFromResource`, returning `(status, body)`.
async fn remove(
    router: &MultiServiceRouter,
    resource_type: &str,
    resource_id: &str,
    keys: &[&str],
) -> (u16, Value) {
    call(
        router,
        "RemoveTagsFromResource",
        &json!({ "ResourceType": resource_type, "ResourceId": resource_id, "TagKeys": keys }),
    )
    .await
}

/// A modelled `TagList` from `(key, value)` pairs.
fn tag_list(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

/// `PutParameter` of an untagged `String` parameter in `region`.
async fn put_parameter(router: &MultiServiceRouter, region: &str, name: &str) {
    let (status, response) = call_in_region(
        router,
        region,
        "PutParameter",
        &json!({ "Name": name, "Type": "String", "Value": "a value" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "PutParameter {name} in {region} must succeed, got {status} {response}"
    );
}

/// `CreateDocument` of a document carrying `tags`.
async fn create_document(router: &MultiServiceRouter, name: &str, tags: &Value) {
    let (status, response) = call(
        router,
        "CreateDocument",
        &json!({ "Name": name, "Content": DOCUMENT_CONTENT, "Tags": tags }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateDocument {name} must succeed, got {status} {response}"
    );
}

/// One round-trip row: a resource to create, the tags to write, and the set the resource
/// must then report.
struct RoundTrip<'a> {
    case: &'a str,
    resource_type: &'a str,
    resource_id: &'a str,
    /// The tags the resource is created with, which is the set a tag write merges into.
    created_with: &'a [(&'a str, &'a str)],
    /// The tags one `AddTagsToResource` call submits.
    added: &'a [(&'a str, &'a str)],
    /// What the resource must then carry, in the key order a report walks.
    expected: &'a [(&'a str, &'a str)],
}

/// What a tag write persisted is what a later tag read reports, for every resource type
/// this service holds records for, and a key the resource already carries takes the
/// submitted value rather than appearing twice.
#[tokio::test]
async fn adding_tags_is_what_a_later_list_reports() {
    let router = router();
    let cases = vec![
        RoundTrip {
            case: "a parameter at a top-level name",
            resource_type: "Parameter",
            resource_id: "TagsTopLevel",
            created_with: &[],
            added: &[("Owner", "DbAdmin"), ("Stack", "Production")],
            expected: &[("Owner", "DbAdmin"), ("Stack", "Production")],
        },
        RoundTrip {
            case: "a parameter in a hierarchy, addressed with its leading separator",
            resource_type: "Parameter",
            resource_id: "/Tags/Dev/DBServer",
            created_with: &[],
            // The value member may be empty (`TagValue` length 0..256) and a key may
            // carry the separators the key pattern allows.
            added: &[("cost:center", ""), ("Owner", "SysAdmin")],
            expected: &[("Owner", "SysAdmin"), ("cost:center", "")],
        },
        RoundTrip {
            case: "a parameter tagged one tag at a time",
            resource_type: "Parameter",
            resource_id: "/Tags/Single",
            created_with: &[],
            added: &[("Stack", "Test")],
            expected: &[("Stack", "Test")],
        },
        RoundTrip {
            case: "a document whose create already carried tags",
            resource_type: "Document",
            resource_id: "TaggedDocument",
            created_with: &[("Environment", "Production"), ("OS", "Windows")],
            // `Environment` is already held, so it takes the submitted value; `Owner` is
            // new, so the set grows by it.
            added: &[("Environment", "Pre-Production"), ("Owner", "Dev")],
            expected: &[
                ("Environment", "Pre-Production"),
                ("OS", "Windows"),
                ("Owner", "Dev"),
            ],
        },
        RoundTrip {
            case: "a document created with no tags at all",
            resource_type: "Document",
            resource_id: "UntaggedDocument",
            created_with: &[],
            added: &[("Stack", "Pre-Production")],
            expected: &[("Stack", "Pre-Production")],
        },
    ];

    for case in cases {
        let RoundTrip {
            case,
            resource_type,
            resource_id,
            created_with,
            added,
            expected,
        } = case;

        match resource_type {
            "Parameter" => put_parameter(&router, REGION, resource_id).await,
            _ => create_document(&router, resource_id, &tag_list(created_with)).await,
        }

        // Before any tag write, the resource carries the tags its create carried —
        // reported as the EMPTY list when that create carried none.
        let mut sorted = created_with.to_vec();
        sorted.sort_by(|left, right| left.0.cmp(right.0));
        let before = listed(&router, case, resource_type, resource_id).await;
        assert_eq!(
            before,
            tag_list(&sorted),
            "case {case}: before any tag write the resource must report what its create \
             carried, got {before}"
        );

        add_ok(
            &router,
            case,
            resource_type,
            resource_id,
            &tag_list(added),
        )
        .await;

        let after = listed(&router, case, resource_type, resource_id).await;
        assert_eq!(
            after,
            tag_list(expected),
            "case {case}: the merged tag set must be what a list reports, got {after}"
        );

        // A read changes nothing a later caller sees.
        let again = listed(&router, case, resource_type, resource_id).await;
        assert_eq!(
            again, after,
            "case {case}: a second list must report the same set, got {again}"
        );

        // A document's tags are part of the document, so the describe that reads the
        // document back reports the very same set.
        if resource_type == "Document" {
            let (status, response) =
                call(&router, "DescribeDocument", &json!({ "Name": resource_id })).await;
            assert_eq!(
                status, 200,
                "case {case}: DescribeDocument {resource_id} must succeed, got {status} {response}"
            );
            assert_eq!(
                response["Document"]["Tags"],
                tag_list(expected),
                "case {case}: the document must report the tags that were written, \
                 got {response}"
            );
        }
    }
}

/// One removal row: the keys a remove names, and what the resource must then carry.
struct Removal<'a> {
    case: &'a str,
    keys: &'a [&'a str],
    expected: &'a [(&'a str, &'a str)],
}

/// A remove drops exactly the keys it names, leaves every other key as it was, and a key
/// the resource does not carry leaves the set alone.
#[tokio::test]
async fn removing_keys_drops_only_the_keys_named() {
    let router = router();
    let held: &[(&str, &str)] = &[
        ("Owner", "DbAdmin"),
        ("Stack", "Production"),
        ("cost:center", "4711"),
        ("team", "payments"),
    ];
    let cases = vec![
        Removal {
            case: "no key at all",
            keys: &[],
            expected: held,
        },
        Removal {
            case: "one held key",
            keys: &["Owner"],
            expected: &[
                ("Stack", "Production"),
                ("cost:center", "4711"),
                ("team", "payments"),
            ],
        },
        Removal {
            case: "two held keys at once",
            keys: &["Owner", "Stack"],
            expected: &[("cost:center", "4711"), ("team", "payments")],
        },
        Removal {
            case: "a key the resource never carried",
            keys: &["NobodyWroteThis"],
            expected: held,
        },
        Removal {
            case: "a held key beside one never carried",
            keys: &["team", "NobodyWroteThis"],
            expected: &[
                ("Owner", "DbAdmin"),
                ("Stack", "Production"),
                ("cost:center", "4711"),
            ],
        },
        Removal {
            case: "every held key",
            keys: &["Owner", "Stack", "cost:center", "team"],
            expected: &[],
        },
    ];

    for (index, Removal {
        case,
        keys,
        expected,
    }) in cases.into_iter().enumerate()
    {
        // Each row starts from the same held set, on a resource of its own.
        let name = format!("/Tags/Removal/{index}");
        put_parameter(&router, REGION, &name).await;
        add_ok(&router, case, "Parameter", &name, &tag_list(held)).await;

        let (status, response) = remove(&router, "Parameter", &name, keys).await;
        assert_eq!(
            status, 200,
            "case {case}: RemoveTagsFromResource must succeed, got {status} {response}"
        );

        let after = listed(&router, case, "Parameter", &name).await;
        assert_eq!(
            after,
            tag_list(expected),
            "case {case}: removing {keys:?} must leave exactly the other keys, got {after}"
        );

        // Removing the same keys again is the same answer: the request asks for those
        // keys to be absent, and they already are.
        let (status, response) = remove(&router, "Parameter", &name, keys).await;
        assert_eq!(
            status, 200,
            "case {case}: a repeated remove must succeed, got {status} {response}"
        );
        let twice = listed(&router, case, "Parameter", &name).await;
        assert_eq!(
            twice, after,
            "case {case}: a repeated remove must leave the set as it was, got {twice}"
        );
    }
}

/// An id naming no resource the Region holds is refused `InvalidResourceId` at HTTP 400,
/// by every operation of the tag plane and for every resource type the model declares.
#[tokio::test]
async fn an_id_naming_no_resource_is_invalid_resource_id() {
    let router = router();
    // One resource of each kind the store does hold EXISTS, so the refusals below are
    // about the id rather than about an empty store.
    put_parameter(&router, REGION, "/Tags/Existing").await;
    create_document(&router, "ExistingDocument", &tag_list(&[])).await;

    for (resource_type, resource_id) in TAGGABLE_TYPES {
        let case = format!("{resource_type} {resource_id}");

        let (status, response) = add(
            &router,
            resource_type,
            resource_id,
            &tag_list(&[("Owner", "DbAdmin")]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: AddTagsToResource must be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidResourceId",
            "case {case}: AddTagsToResource must answer InvalidResourceId, got {response}"
        );

        let (status, response) = list(&router, resource_type, resource_id).await;
        assert_eq!(
            status, 400,
            "case {case}: ListTagsForResource must be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidResourceId",
            "case {case}: ListTagsForResource must answer InvalidResourceId, got {response}"
        );

        let (status, response) = remove(&router, resource_type, resource_id, &["Owner"]).await;
        assert_eq!(
            status, 400,
            "case {case}: RemoveTagsFromResource must be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidResourceId",
            "case {case}: RemoveTagsFromResource must answer InvalidResourceId, got {response}"
        );
    }

    // A refused request writes nothing: the resources that DO exist still carry no tags.
    for (resource_type, resource_id) in [("Parameter", "/Tags/Existing"), ("Document", "ExistingDocument")] {
        let after = listed(&router, "a refusal persists nothing", resource_type, resource_id).await;
        assert_eq!(
            after,
            tag_list(&[]),
            "case {resource_type} {resource_id}: must still carry no tags, got {after}"
        );
    }
}

/// A resource carries at most the documented maximum number of tags: the merge that
/// would take it past that maximum is refused `TooManyTagsError` and writes none of its
/// tags, while a merge that only overwrites keys already held stays within it.
#[tokio::test]
async fn a_resource_carries_at_most_the_documented_maximum() {
    let router = router();

    // The published bound: exactly the maximum, in one request.
    let keys: Vec<String> = (0..MAX_TAGS).map(|index| format!("key{index:03}")).collect();
    let at_bound: Vec<(&str, &str)> = keys.iter().map(|key| (key.as_str(), "held")).collect();
    put_parameter(&router, REGION, "/Tags/AtTheBound").await;
    add_ok(
        &router,
        "exactly the maximum",
        "Parameter",
        "/Tags/AtTheBound",
        &tag_list(&at_bound),
    )
    .await;
    let held = listed(&router, "exactly the maximum", "Parameter", "/Tags/AtTheBound").await;
    assert_eq!(
        held.as_array().map(Vec::len),
        Some(MAX_TAGS),
        "case exactly the maximum: the resource must carry {MAX_TAGS} tags, got {held}"
    );

    // One past it: a new key on a resource already at the maximum.
    let (status, response) = add(
        &router,
        "Parameter",
        "/Tags/AtTheBound",
        &tag_list(&[("oneTooMany", "v")]),
    )
    .await;
    assert_eq!(
        status, 400,
        "case one past the maximum: must be refused, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "TooManyTagsError",
        "case one past the maximum: expected TooManyTagsError, got {response}"
    );
    let after = listed(&router, "one past the maximum", "Parameter", "/Tags/AtTheBound").await;
    assert_eq!(
        after, held,
        "case one past the maximum: a refused merge must write none of its tags, got {after}"
    );

    // A key already held is not a new key, so overwriting it at the maximum is served.
    add_ok(
        &router,
        "overwriting at the maximum",
        "Parameter",
        "/Tags/AtTheBound",
        &tag_list(&[(keys[0].as_str(), "overwritten")]),
    )
    .await;
    let overwritten =
        listed(&router, "overwriting at the maximum", "Parameter", "/Tags/AtTheBound").await;
    assert_eq!(
        overwritten.as_array().map(Vec::len),
        Some(MAX_TAGS),
        "case overwriting at the maximum: the count must not change, got {overwritten}"
    );
    assert_eq!(
        overwritten[0]["Value"],
        json!("overwritten"),
        "case overwriting at the maximum: the held key must take the new value, \
         got {overwritten}"
    );

    // One request carrying more than the maximum is refused on a resource carrying
    // nothing at all, and leaves it carrying nothing.
    let too_many_keys: Vec<String> = (0..=MAX_TAGS)
        .map(|index| format!("bulk{index:03}"))
        .collect();
    let too_many: Vec<(&str, &str)> = too_many_keys
        .iter()
        .map(|key| (key.as_str(), "held"))
        .collect();
    put_parameter(&router, REGION, "/Tags/BulkRefused").await;
    let (status, response) = add(&router, "Parameter", "/Tags/BulkRefused", &tag_list(&too_many)).await;
    assert_eq!(
        status, 400,
        "case one request past the maximum: must be refused, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "TooManyTagsError",
        "case one request past the maximum: expected TooManyTagsError, got {response}"
    );
    let untouched = listed(&router, "one request past the maximum", "Parameter", "/Tags/BulkRefused").await;
    assert_eq!(
        untouched,
        tag_list(&[]),
        "case one request past the maximum: the resource must carry no tags, got {untouched}"
    );
}

/// A tag set lives with the resource of ONE Region: the same name in another Region is
/// another resource with its own tags, and a Region that holds no such resource refuses
/// the id.
#[tokio::test]
async fn tags_belong_to_the_resource_of_one_region() {
    let router = router();
    let name = "/Tags/PerRegion";
    put_parameter(&router, REGION, name).await;
    put_parameter(&router, OTHER_REGION, name).await;

    add_ok(
        &router,
        "the signed Region's resource",
        "Parameter",
        name,
        &tag_list(&[("Owner", "DbAdmin")]),
    )
    .await;

    let signed = listed(&router, "the signed Region's resource", "Parameter", name).await;
    assert_eq!(
        signed,
        tag_list(&[("Owner", "DbAdmin")]),
        "case the signed Region's resource: must report the tag that was written, got {signed}"
    );

    let (status, response) = call_in_region(
        &router,
        OTHER_REGION,
        "ListTagsForResource",
        &json!({ "ResourceType": "Parameter", "ResourceId": name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case another Region's resource: the read must succeed, got {status} {response}"
    );
    assert_eq!(
        response["TagList"],
        tag_list(&[]),
        "case another Region's resource: it must carry none of the other Region's tags, \
         got {response}"
    );

    // A Region holding no resource of that name refuses the id rather than resolving
    // one of the Region that does hold it.
    put_parameter(&router, REGION, "/Tags/OnlyHere").await;
    let (status, response) = call_in_region(
        &router,
        OTHER_REGION,
        "AddTagsToResource",
        &json!({
            "ResourceType": "Parameter",
            "ResourceId": "/Tags/OnlyHere",
            "Tags": tag_list(&[("Owner", "DbAdmin")]),
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a Region holding no such resource: must be refused, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "InvalidResourceId",
        "case a Region holding no such resource: expected InvalidResourceId, got {response}"
    );
    let untouched = listed(
        &router,
        "a Region holding no such resource",
        "Parameter",
        "/Tags/OnlyHere",
    )
    .await;
    assert_eq!(
        untouched,
        tag_list(&[]),
        "case a Region holding no such resource: the resource of the other Region must \
         keep its own empty set, got {untouched}"
    );
}

/// A managed node is addressed by the `mi-` id its registration holds, and the node the
/// inventory holds is the record whose tag set the plane reads and writes.
///
/// Nothing in this service's model registers a node — that is the agent's own call — so
/// the node is seeded into the inventory the way `all/managed_node_filter_validation.rs`
/// seeds one, and every tag call still goes through the router's front door.
#[tokio::test]
async fn a_registered_managed_node_carries_its_own_tag_set() {
    let (router, engine) = router_with_engine();
    // "You must specify the name of the managed node in the following format:
    // mi-ID_number" (`AddTagsToResourceRequest$ResourceId`); the modelled `InstanceId`
    // pattern admits `mi-` followed by 17 word characters.
    let node_id = "mi-0123456789abcdef0";
    let record = ManagedNodeRecord {
        properties: InstanceProperty {
            instance_id: Some(node_id.to_string()),
            resource_type: Some("ManagedInstance".to_string()),
            platform_type: Some("Linux".to_string()),
            ping_status: Some("Online".to_string()),
            ..InstanceProperty::default()
        },
        ..ManagedNodeRecord::default()
    };
    assert!(
        engine
            .state::<ComAmazonawsSsmAmazonssmState>()
            .lock()
            .insert_managed_node(REGION, record),
        "case a registered node: the seeded node must carry the id it is stored under"
    );

    let before = listed(&router, "a registered node", "ManagedInstance", node_id).await;
    assert_eq!(
        before,
        tag_list(&[]),
        "case a registered node: a node nothing has tagged must report no tags, got {before}"
    );

    add_ok(
        &router,
        "a registered node",
        "ManagedInstance",
        node_id,
        &tag_list(&[("Owner", "SysAdmin"), ("Stack", "Production")]),
    )
    .await;
    let after = listed(&router, "a registered node", "ManagedInstance", node_id).await;
    assert_eq!(
        after,
        tag_list(&[("Owner", "SysAdmin"), ("Stack", "Production")]),
        "case a registered node: the written tags must be what a list reports, got {after}"
    );

    let (status, response) = remove(&router, "ManagedInstance", node_id, &["Owner"]).await;
    assert_eq!(
        status, 200,
        "case a registered node: RemoveTagsFromResource must succeed, got {status} {response}"
    );
    let remaining = listed(&router, "a registered node", "ManagedInstance", node_id).await;
    assert_eq!(
        remaining,
        tag_list(&[("Stack", "Production")]),
        "case a registered node: only the named key must be dropped, got {remaining}"
    );
}

/// A `ResourceType` outside the modelled `ResourceTypeForTagging` enum never reaches the
/// resource resolution: the boundary walk refuses it as a malformed request, rather than
/// it being answered as an id that names nothing.
#[tokio::test]
async fn a_resource_type_outside_the_modelled_enum_is_refused() {
    let router = router();
    // A resource of a type the plane DOES hold exists, so the refusals below are about
    // the type rather than about an empty store.
    put_parameter(&router, REGION, "/Tags/EnumGuard").await;

    // Each row is a spelling the enum does not declare: a type of another service, the
    // right value in the wrong case, a plural, and the empty string.
    for resource_type in ["EC2Instance", "parameter", "ManagedInstances", ""] {
        let case = format!("ResourceType {resource_type:?}");

        let (status, response) = add(
            &router,
            resource_type,
            "/Tags/EnumGuard",
            &tag_list(&[("Owner", "DbAdmin")]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: AddTagsToResource must be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case}: AddTagsToResource must refuse the type as a malformed request, \
             got {response}"
        );

        let (status, response) = list(&router, resource_type, "/Tags/EnumGuard").await;
        assert_eq!(
            status, 400,
            "case {case}: ListTagsForResource must be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case}: ListTagsForResource must refuse the type as a malformed request, \
             got {response}"
        );

        let (status, response) = remove(&router, resource_type, "/Tags/EnumGuard", &["Owner"]).await;
        assert_eq!(
            status, 400,
            "case {case}: RemoveTagsFromResource must be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case}: RemoveTagsFromResource must refuse the type as a malformed \
             request, got {response}"
        );
    }

    // None of those refusals touched the resource that does exist.
    let untouched = listed(&router, "an unmodelled type", "Parameter", "/Tags/EnumGuard").await;
    assert_eq!(
        untouched,
        tag_list(&[]),
        "case an unmodelled type: the parameter must still carry no tags, got {untouched}"
    );
}

/// An OpsItem is addressed by the id its create minted, and the OpsItem OpsCenter holds
/// is the record whose tag set the plane reads and writes — including the tags the create
/// itself assigned.
#[tokio::test]
async fn an_ops_item_carries_its_own_tag_set() {
    let router = router();
    let (status, created) = call(
        &router,
        "CreateOpsItem",
        &json!({
            "Title": "A tagged OpsItem",
            "Description": "what the tagged OpsItem is about",
            "Source": "emulator-tests",
            "Tags": tag_list(&[("Department", "Finance")]),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case an OpsItem: CreateOpsItem must succeed, got {status} {created}"
    );
    let ops_item_id = created["OpsItemId"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_default();
    assert!(
        !ops_item_id.is_empty(),
        "case an OpsItem: the create must report an OpsItemId, got {created}"
    );

    // "To add tags to a new OpsItem, you must have IAM permissions ... To add tags to an
    // existing OpsItem, use the AddTagsToResource operation" (`CreateOpsItem`'s `Tags`
    // doc), so the create's tags are the set the add merges into.
    let before = listed(&router, "an OpsItem", "OpsItem", &ops_item_id).await;
    assert_eq!(
        before,
        tag_list(&[("Department", "Finance")]),
        "case an OpsItem: the tags its create assigned must be what a list reports, \
         got {before}"
    );

    add_ok(
        &router,
        "an OpsItem",
        "OpsItem",
        &ops_item_id,
        &tag_list(&[("Department", "Operations"), ("Owner", "Dev")]),
    )
    .await;
    let after = listed(&router, "an OpsItem", "OpsItem", &ops_item_id).await;
    assert_eq!(
        after,
        tag_list(&[("Department", "Operations"), ("Owner", "Dev")]),
        "case an OpsItem: the held key must take the submitted value and the new key must \
         be added, got {after}"
    );

    let (status, response) = remove(&router, "OpsItem", &ops_item_id, &["Department"]).await;
    assert_eq!(
        status, 200,
        "case an OpsItem: RemoveTagsFromResource must succeed, got {status} {response}"
    );
    let remaining = listed(&router, "an OpsItem", "OpsItem", &ops_item_id).await;
    assert_eq!(
        remaining,
        tag_list(&[("Owner", "Dev")]),
        "case an OpsItem: only the named key must be dropped, got {remaining}"
    );
}

/// A cloud connector is addressed by the id its create minted, and the connector the
/// store holds is the record whose tag set the plane reads and writes — including the
/// tags the create itself assigned.
#[tokio::test]
async fn a_cloud_connector_carries_its_own_tag_set() {
    let router = router();
    // The members `CreateCloudConnector` requires, in the forms its model gives them;
    // the configuration is the `AzureConfiguration` member of the modelled union.
    let (status, created) = call(
        &router,
        "CreateCloudConnector",
        &json!({
            "DisplayName": "Azure production tenant",
            "Description": "the connector this case tags",
            "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/SsmCloudConnectorRole"),
            "ConfigConnectorArn": format!("arn:aws:config:{REGION}:{ACCOUNT}:connector/production"),
            "Configuration": {
                "AzureConfiguration": {
                    "TenantId": "55555555-6666-7777-8888-999999999999",
                    "TenantDisplayName": "Contoso",
                    "ApplicationId": "66666666-7777-8888-9999-aaaaaaaaaaaa",
                    "ApplicationDisplayName": "Systems Manager connector",
                }
            },
            "Tags": tag_list(&[("Team", "platform")]),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case a cloud connector: CreateCloudConnector must succeed, got {status} {created}"
    );
    let connector_id = created["CloudConnectorId"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_default();
    assert!(
        !connector_id.is_empty(),
        "case a cloud connector: the create must report a CloudConnectorId, got {created}"
    );

    let before = listed(&router, "a cloud connector", "CloudConnector", &connector_id).await;
    assert_eq!(
        before,
        tag_list(&[("Team", "platform")]),
        "case a cloud connector: the tags its create assigned must be what a list reports, \
         got {before}"
    );

    add_ok(
        &router,
        "a cloud connector",
        "CloudConnector",
        &connector_id,
        &tag_list(&[("Team", "networking"), ("Stack", "Production")]),
    )
    .await;
    let after = listed(&router, "a cloud connector", "CloudConnector", &connector_id).await;
    assert_eq!(
        after,
        tag_list(&[("Stack", "Production"), ("Team", "networking")]),
        "case a cloud connector: the held key must take the submitted value and the new \
         key must be added, got {after}"
    );

    let (status, response) = remove(&router, "CloudConnector", &connector_id, &["Team"]).await;
    assert_eq!(
        status, 200,
        "case a cloud connector: RemoveTagsFromResource must succeed, got {status} {response}"
    );
    let remaining = listed(&router, "a cloud connector", "CloudConnector", &connector_id).await;
    assert_eq!(
        remaining,
        tag_list(&[("Stack", "Production")]),
        "case a cloud connector: only the named key must be dropped, got {remaining}"
    );
}
