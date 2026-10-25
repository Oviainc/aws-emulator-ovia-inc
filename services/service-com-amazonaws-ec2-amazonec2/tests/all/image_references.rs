// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeImageReferences`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DescribeImageReferences`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that the operation REPORTS THE STORE: it answers one reference
//! per (named image id, referencing resource) pair computed from persisted state — an
//! instance whose `ImageId` is one of the named images, narrowed by the `state-name`
//! option, and a launch template one of whose `version-depth` most recent versions names
//! one of them — each carrying that resource's type and its ARN in the Region the request
//! was signed for, cut into `MaxResults`-sized pages.
//!
//! Every table row is a distinct input along what that generalises over: how many image
//! ids the request names and in which id form, which resource types it checks and with
//! which options, which instance states and which version depth those options select, and
//! which page size it asks for. The resources are created through the API that mints them
//! (`RunInstances`, `CreateLaunchTemplate`, `StopInstances`), so a reported reference is a
//! resource read back out of the store rather than an echo of the request.
//!
//! Launch template versions beyond the first are seeded into the store directly:
//! `CreateLaunchTemplateVersion` is still a scaffold (its own stub issue), and the
//! `version-depth` option is only observable on a template that has several versions.

use std::time::{Duration, SystemTime};

use ec2::ops::types::RequestLaunchTemplateData;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, LaunchTemplateVersionRecord, MAX_PAGE_SIZE,
    TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, indexed};

/// The Region every call here is signed for — not the service's default, so an ARN that
/// named a hardcoded Region instead of the signed one would fail every assertion below.
const REGION: &str = "us-west-2";

/// The image three running instances, one stopped instance and one single-version launch
/// template reference.
const AMI_A: &str = "ami-0a1b2c3d4e5f60001";

/// The image one running instance references, and the NEWEST version of the multi-version
/// launch template.
const AMI_B: &str = "ami-0a1b2c3d4e5f60002";

/// The image only version 2 of the multi-version template names — outside the documented
/// default `version-depth` of 10, inside a depth of 11.
const AMI_C: &str = "ami-0a1b2c3d4e5f60003";

/// The image only version 1 (the oldest) of the multi-version template names.
const AMI_D: &str = "ami-0a1b2c3d4e5f60004";

/// The image the filler versions 3..=11 of the multi-version template name.
const AMI_FILLER: &str = "ami-0a1b2c3d4e5f60005";

/// A well-formed image id nothing in the store references.
const AMI_UNREFERENCED: &str = "ami-0a1b2c3d4e5f60009";

/// A well-formed image id in the SHORT (8 hex digit) form, which nothing references.
const AMI_SHORT_FORM: &str = "ami-0abc1234";

/// The newest version number of the multi-version launch template.
const NEWEST_VERSION: i64 = 12;

/// One `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One ec2Query call through the crate's shared harness.
async fn api(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One `DescribeImageReferences` call.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    api(router, "DescribeImageReferences", params).await
}

/// The form fields for one `ResourceTypes` entry at `index` (counting from 1): the type,
/// and each of its `ResourceTypeOptions` as a name with its values.
fn resource_type_fields(
    index: usize,
    resource_type: &str,
    options: &[(&str, &[&str])],
) -> Vec<(String, String)> {
    let mut fields = vec![p(
        &format!("ResourceType.{index}.ResourceType"),
        resource_type,
    )];
    for (option_index, (name, values)) in options.iter().enumerate() {
        let prefix = format!(
            "ResourceType.{index}.ResourceTypeOption.{}",
            option_index + 1
        );
        fields.push(p(&format!("{prefix}.OptionName"), *name));
        for (value_index, value) in values.iter().enumerate() {
            fields.push(p(
                &format!("{prefix}.OptionValue.{}", value_index + 1),
                *value,
            ));
        }
    }
    fields
}

/// `ResourceTypes` with a single entry.
fn one_resource_type(resource_type: &str, options: &[(&str, &[&str])]) -> Vec<(String, String)> {
    resource_type_fields(1, resource_type, options)
}

/// One reported reference: every member `ImageReference` carries.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Reference {
    image_id: String,
    resource_type: String,
    arn: String,
}

/// The account every ARN this emulator mints carries.
fn account() -> &'static str {
    engine::emulator_account_id()
}

/// The reference an instance launched from `image_id` must be reported as.
fn instance_reference(image_id: &str, instance_id: &str) -> Reference {
    Reference {
        image_id: image_id.to_string(),
        resource_type: "ec2:Instance".to_string(),
        arn: format!(
            "arn:aws:ec2:{REGION}:{}:instance/{instance_id}",
            account()
        ),
    }
}

/// The reference a launch template naming `image_id` in a checked version must be reported
/// as.
fn template_reference(image_id: &str, template_id: &str) -> Reference {
    Reference {
        image_id: image_id.to_string(),
        resource_type: "ec2:LaunchTemplate".to_string(),
        arn: format!(
            "arn:aws:ec2:{REGION}:{}:launch-template/{template_id}",
            account()
        ),
    }
}

/// Every reference in a response body, in the order the response reported them.
fn references(body: &str) -> Vec<Reference> {
    let document = roxmltree::Document::parse(body)
        .unwrap_or_else(|error| panic!("the response body must be XML ({error}): {body}"));
    document
        .descendants()
        .filter(|node| {
            node.is_element()
                && node
                    .children()
                    .any(|child| child.is_element() && child.has_tag_name("arn"))
        })
        .map(|node| Reference {
            image_id: child_text(node, "imageId"),
            resource_type: child_text(node, "resourceType"),
            arn: child_text(node, "arn"),
        })
        .collect()
}

/// The text of `node`'s `name` child, or the empty string when it has none.
fn child_text(node: roxmltree::Node<'_, '_>, name: &str) -> String {
    node.children()
        .find(|child| child.is_element() && child.has_tag_name(name))
        .and_then(|child| child.text())
        .unwrap_or_default()
        .to_string()
}

/// The references of a successful response, sorted so a set can be compared.
async fn described(router: &MultiServiceRouter, case: &str, params: &[(String, String)]) -> Vec<Reference> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let mut found = references(&body);
    found.sort();
    found
}

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// The store this suite reports on, as the API that mints each resource built it.
struct Seeded {
    router: MultiServiceRouter,
    engine: std::sync::Arc<engine::InMemoryEngine>,
    /// The three instances launched from [`AMI_A`] and left running.
    running_a: Vec<String>,
    /// The instance launched from [`AMI_A`] and then stopped.
    stopped_a: String,
    /// The instance launched from [`AMI_B`] and left running.
    running_b: String,
    /// A launch template whose only version names [`AMI_A`].
    template_a: String,
    /// A launch template with [`NEWEST_VERSION`] versions: version 1 names [`AMI_D`],
    /// version 2 [`AMI_C`], versions 3..=11 [`AMI_FILLER`] and the newest [`AMI_B`].
    template_versions: String,
}

/// Launch `count` instances from `image_id` and answer their ids.
async fn run_instances(router: &MultiServiceRouter, image_id: &str, count: usize) -> Vec<String> {
    let (status, body) = api(
        router,
        "RunInstances",
        &[
            p("ImageId", image_id),
            p("MinCount", count.to_string()),
            p("MaxCount", count.to_string()),
        ],
    )
    .await;
    assert_eq!(status, 200, "RunInstances must succeed, got {status}: {body}");
    let ids = elements(&body, "instanceId");
    assert_eq!(
        ids.len(),
        count,
        "RunInstances must report {count} instances, got {ids:?}"
    );
    ids
}

/// Create a launch template whose first version names `image_id`, and answer its id.
async fn create_launch_template(
    router: &MultiServiceRouter,
    name: &str,
    image_id: &str,
) -> String {
    let (status, body) = api(
        router,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", name),
            p("LaunchTemplateData.ImageId", image_id),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateLaunchTemplate must succeed, got {status}: {body}"
    );
    element(&body, "launchTemplateId")
        .unwrap_or_else(|| panic!("the create must report a launch template id: {body}"))
}

/// Give `template_id` one more version, naming `image_id`.
///
/// `CreateLaunchTemplateVersion` is still a scaffold, so the version is written into the
/// store the way the create path writes one: the record the describe path reads, with the
/// template's latest version number moved on to it.
fn seed_version(
    engine: &engine::InMemoryEngine,
    template_id: &str,
    version_number: i64,
    image_id: &str,
) {
    let state = engine.state::<Ec2State>();
    let mut store = state.lock();
    let record = store
        .launch_templates
        .get_mut(template_id)
        .unwrap_or_else(|| panic!("{template_id} must be stored"));
    record.versions.insert(
        version_number,
        LaunchTemplateVersionRecord {
            version_number,
            version_description: None,
            create_time: SystemTime::now(),
            created_by: account().to_string(),
            default_version: false,
            data: RequestLaunchTemplateData {
                image_id: Some(image_id.to_string()),
                ..RequestLaunchTemplateData::default()
            },
        },
    );
    record.template.latest_version_number = Some(version_number);
}

impl Seeded {
    /// Build the store every table below reports on.
    async fn new() -> Self {
        let (router, engine) = super::ec2_query::emulator();

        let running_a = run_instances(&router, AMI_A, 3).await;
        let stopped_a = run_instances(&router, AMI_A, 1).await.remove(0);
        let running_b = run_instances(&router, AMI_B, 1).await.remove(0);

        // The launch of `stopped_a` is `pending`, and a stop is only accepted from
        // `pending` or `running`; the wait before and after makes the stop's own
        // clock-owed move to `stopped` the state every row below selects on.
        past_the_transition().await;
        let (status, body) = api(
            &router,
            "StopInstances",
            &[p("InstanceId.1", stopped_a.clone())],
        )
        .await;
        assert_eq!(status, 200, "StopInstances must succeed, got {status}: {body}");
        past_the_transition().await;

        let template_a = create_launch_template(&router, "references-single", AMI_A).await;
        let template_versions = create_launch_template(&router, "references-many", AMI_D).await;
        seed_version(&engine, &template_versions, 2, AMI_C);
        for version in 3..NEWEST_VERSION {
            seed_version(&engine, &template_versions, version, AMI_FILLER);
        }
        seed_version(&engine, &template_versions, NEWEST_VERSION, AMI_B);

        Self {
            router,
            engine,
            running_a,
            stopped_a,
            running_b,
            template_a,
            template_versions,
        }
    }

    /// Every reference the store holds for [`AMI_A`] and [`AMI_B`] under the documented
    /// defaults, sorted.
    fn all_references(&self) -> Vec<Reference> {
        let mut expected: Vec<Reference> = self
            .running_a
            .iter()
            .map(|id| instance_reference(AMI_A, id))
            .collect();
        expected.push(instance_reference(AMI_A, &self.stopped_a));
        expected.push(instance_reference(AMI_B, &self.running_b));
        expected.push(template_reference(AMI_A, &self.template_a));
        expected.push(template_reference(AMI_B, &self.template_versions));
        expected.sort();
        expected
    }
}

/// One row of the refusal table: what the case is, the parameters the call carries, and
/// the error code it must be refused with.
type RefusalCase = (&'static str, Vec<(String, String)>, &'static str);

/// One row of the reporting table: the case, the request parameters, and the references the
/// store must answer with.
struct Row {
    case: &'static str,
    params: Vec<(String, String)>,
    expected: Vec<Reference>,
}

/// `ImageId.N` fields for `ids`.
fn image_ids(ids: &[&str]) -> Vec<(String, String)> {
    indexed("ImageId", ids)
}

/// The rows the reporting table is written against.
fn rows(seeded: &Seeded) -> Vec<Row> {
    let instances_a: Vec<Reference> = seeded
        .running_a
        .iter()
        .map(|id| instance_reference(AMI_A, id))
        .collect();
    let stopped_a = instance_reference(AMI_A, &seeded.stopped_a);
    let running_b = instance_reference(AMI_B, &seeded.running_b);
    let template_a = template_reference(AMI_A, &seeded.template_a);
    let template_b = template_reference(AMI_B, &seeded.template_versions);
    let template_c = template_reference(AMI_C, &seeded.template_versions);
    let template_d = template_reference(AMI_D, &seeded.template_versions);

    let mut every_a = instances_a.clone();
    every_a.push(stopped_a.clone());
    every_a.push(template_a.clone());

    let ten_ids = [
        AMI_A,
        AMI_B,
        AMI_C,
        AMI_D,
        AMI_FILLER,
        AMI_UNREFERENCED,
        AMI_SHORT_FORM,
        "ami-0a1b2c3d4e5f6000a",
        "ami-0a1b2c3d4e5f6000b",
        "ami-0a1b2c3d4e5f6000c",
    ];

    let mut rows = vec![
        Row {
            case: "one image id, every resource type",
            params: [image_ids(&[AMI_A]), vec![p("IncludeAllResourceTypes", "true")]].concat(),
            expected: every_a.clone(),
        },
        Row {
            case: "one image id, only ec2:Instance",
            params: [image_ids(&[AMI_A]), one_resource_type("ec2:Instance", &[])].concat(),
            expected: [instances_a.clone(), vec![stopped_a.clone()]].concat(),
        },
        Row {
            case: "one image id, only ec2:LaunchTemplate",
            params: [
                image_ids(&[AMI_A]),
                one_resource_type("ec2:LaunchTemplate", &[]),
            ]
            .concat(),
            expected: vec![template_a.clone()],
        },
        Row {
            case: "two image ids, every resource type",
            params: [
                image_ids(&[AMI_A, AMI_B]),
                vec![p("IncludeAllResourceTypes", "true")],
            ]
            .concat(),
            expected: [
                every_a.clone(),
                vec![running_b.clone(), template_b.clone()],
            ]
            .concat(),
        },
        Row {
            case: "state-name selects the stopped instance only",
            params: [
                image_ids(&[AMI_A]),
                one_resource_type("ec2:Instance", &[("state-name", &["stopped"])]),
            ]
            .concat(),
            expected: vec![stopped_a.clone()],
        },
        Row {
            case: "state-name selects two states",
            params: [
                image_ids(&[AMI_A]),
                one_resource_type("ec2:Instance", &[("state-name", &["running", "stopped"])]),
            ]
            .concat(),
            expected: [instances_a.clone(), vec![stopped_a.clone()]].concat(),
        },
        Row {
            case: "state-name selects a state nothing is in",
            params: [
                image_ids(&[AMI_A]),
                one_resource_type("ec2:Instance", &[("state-name", &["terminated"])]),
            ]
            .concat(),
            expected: Vec::new(),
        },
        Row {
            case: "the default version-depth reaches the newest version",
            params: [
                image_ids(&[AMI_B]),
                one_resource_type("ec2:LaunchTemplate", &[]),
            ]
            .concat(),
            expected: vec![template_b.clone()],
        },
        Row {
            case: "the default version-depth of 10 stops before version 2",
            params: [
                image_ids(&[AMI_C]),
                one_resource_type("ec2:LaunchTemplate", &[]),
            ]
            .concat(),
            expected: Vec::new(),
        },
        Row {
            case: "version-depth 11 reaches version 2",
            params: [
                image_ids(&[AMI_C]),
                one_resource_type("ec2:LaunchTemplate", &[("version-depth", &["11"])]),
            ]
            .concat(),
            expected: vec![template_c.clone()],
        },
        Row {
            case: "version-depth at the published maximum reaches the oldest version",
            params: [
                image_ids(&[AMI_D]),
                one_resource_type("ec2:LaunchTemplate", &[("version-depth", &["10000"])]),
            ]
            .concat(),
            expected: vec![template_d.clone()],
        },
        Row {
            case: "version-depth at the published minimum sees the newest version only",
            params: [
                image_ids(&[AMI_B, AMI_D]),
                one_resource_type("ec2:LaunchTemplate", &[("version-depth", &["1"])]),
            ]
            .concat(),
            expected: vec![template_b.clone()],
        },
        Row {
            case: "a well-formed image id nothing references",
            params: [
                image_ids(&[AMI_UNREFERENCED]),
                vec![p("IncludeAllResourceTypes", "true")],
            ]
            .concat(),
            expected: Vec::new(),
        },
        Row {
            case: "a short-form image id nothing references",
            params: [
                image_ids(&[AMI_SHORT_FORM]),
                vec![p("IncludeAllResourceTypes", "true")],
            ]
            .concat(),
            expected: Vec::new(),
        },
        Row {
            case: "ten image ids, the modelled maximum",
            params: [
                image_ids(&ten_ids),
                vec![p("IncludeAllResourceTypes", "true")],
                // The default depth would hide version 2's image, so the depth that
                // reaches every version is asked for alongside the other types.
                resource_type_fields(1, "ec2:LaunchTemplate", &[("version-depth", &["10000"])]),
            ]
            .concat(),
            expected: [
                every_a.clone(),
                vec![
                    running_b.clone(),
                    template_b.clone(),
                    template_c.clone(),
                    template_d.clone(),
                    template_reference(AMI_FILLER, &seeded.template_versions),
                ],
            ]
            .concat(),
        },
        Row {
            case: "both EC2 types named with their own options",
            params: [
                image_ids(&[AMI_A, AMI_C]),
                resource_type_fields(1, "ec2:Instance", &[("state-name", &["running"])]),
                resource_type_fields(2, "ec2:LaunchTemplate", &[("version-depth", &["11"])]),
            ]
            .concat(),
            expected: [
                instances_a.clone(),
                vec![template_a.clone(), template_c.clone()],
            ]
            .concat(),
        },
    ];

    // Each resource type of another service is checked and contributes nothing: this
    // service's store holds no parameter and no recipe to reference an image.
    for cross_service in ["ssm:Parameter", "imagebuilder:ImageRecipe", "imagebuilder:ContainerRecipe"] {
        rows.push(Row {
            case: "a resource type of another service",
            params: [image_ids(&[AMI_A]), one_resource_type(cross_service, &[])].concat(),
            expected: Vec::new(),
        });
    }

    for row in &mut rows {
        row.expected.sort();
    }
    rows
}

/// Every value the model carries for `InstanceStateName` is a `state-name` the option
/// accepts, and a word outside that list is refused — so the states this operation
/// selects over are exactly the modelled ones, checked against the list the crate
/// generates from the enum rather than a copy of it.
#[tokio::test]
async fn the_selectable_states_are_the_modelled_ones() {
    let seeded = Seeded::new().await;
    for value in ec2::ops::types::INSTANCE_STATE_NAME_VALUES {
        let params = [
            image_ids(&[AMI_A]),
            one_resource_type("ec2:Instance", &[("state-name", &[value])]),
        ]
        .concat();
        let (status, body) = describe(&seeded.router, &params).await;
        assert_eq!(
            status, 200,
            "case {value:?}: a modelled state must be accepted, got {status} with body {body}"
        );
    }
    for value in ["", "available", "Running", "shutting_down"] {
        let params = [
            image_ids(&[AMI_A]),
            one_resource_type("ec2:Instance", &[("state-name", &[value])]),
        ]
        .concat();
        let (status, body) = describe(&seeded.router, &params).await;
        assert_eq!(
            status, 400,
            "case {value:?}: a state outside the model must be refused, got {status} {body}"
        );
    }
}

/// The reported references are the store's, narrowed by the image ids, the resource types
/// and the options the request names.
#[tokio::test]
async fn image_references_are_computed_from_the_store() {
    let seeded = Seeded::new().await;
    for row in rows(&seeded) {
        let found = described(&seeded.router, row.case, &row.params).await;
        assert_eq!(
            found, row.expected,
            "case {:?}: expected {:?}, got {:?}",
            row.case, row.expected, found
        );
    }
}

/// Every reference is reported exactly once across the pages a `MaxResults` cut produces,
/// and the last page carries no token.
///
/// The rows differ in the page size: the modelled minimum, a size that leaves a remainder,
/// one that takes the whole set, and the service's ceiling.
#[tokio::test]
async fn pages_partition_the_reference_set() {
    let seeded = Seeded::new().await;
    let expected = seeded.all_references();
    let named = [
        image_ids(&[AMI_A, AMI_B]),
        vec![p("IncludeAllResourceTypes", "true")],
    ]
    .concat();

    for page_size in [5_usize, 6, expected.len(), MAX_PAGE_SIZE] {
        let mut collected: Vec<Reference> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = named.clone();
            params.push(p("MaxResults", page_size.to_string()));
            if let Some(cursor) = &token {
                params.push(p("NextToken", cursor.clone()));
            }
            let (status, body) = describe(&seeded.router, &params).await;
            assert_eq!(
                status, 200,
                "case MaxResults={page_size}: expected HTTP 200, got {status} with body {body}"
            );
            let page = references(&body);
            assert!(
                page.len() <= page_size,
                "case MaxResults={page_size}: a page carries {} references, above the page size",
                page.len()
            );
            collected.extend(page);
            token = element(&body, "nextToken");
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case MaxResults={page_size}: pagination must terminate, got {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        let mut sorted = collected.clone();
        sorted.sort();
        let mut deduplicated = sorted.clone();
        deduplicated.dedup();
        assert_eq!(
            sorted, deduplicated,
            "case MaxResults={page_size}: every reference must be reported once, got {sorted:?}"
        );
        assert_eq!(
            sorted, expected,
            "case MaxResults={page_size}: expected {expected:?}, got {sorted:?}"
        );
        let expected_pages = expected.len().div_ceil(page_size).max(1);
        assert_eq!(
            pages, expected_pages,
            "case MaxResults={page_size}: expected {expected_pages} pages, got {pages}"
        );
    }
}

/// A request this operation cannot answer is refused with HTTP 400 naming what is at
/// fault, never with an empty reference set — which would read as "nothing references
/// these images".
///
/// The rows differ in what is wrong: no resource type selected at all, an id that is not
/// an image id, a resource type outside the modelled enum, a type named twice, an option
/// the named type does not document, an option value outside its documented set or range,
/// a page size above the service's ceiling, and a token this service did not issue.
#[tokio::test]
async fn a_request_that_cannot_be_answered_is_refused() {
    let seeded = Seeded::new().await;
    let cases: Vec<RefusalCase> = vec![
        (
            "neither IncludeAllResourceTypes nor ResourceTypes",
            image_ids(&[AMI_A]),
            "MissingParameter",
        ),
        (
            "IncludeAllResourceTypes false and no ResourceTypes",
            [image_ids(&[AMI_A]), vec![p("IncludeAllResourceTypes", "false")]].concat(),
            "MissingParameter",
        ),
        (
            "an instance id where an image id belongs",
            [
                image_ids(&["i-0a1b2c3d4e5f60001"]),
                vec![p("IncludeAllResourceTypes", "true")],
            ]
            .concat(),
            "InvalidAMIID.Malformed",
        ),
        (
            "an image id with a non-hex suffix",
            [
                image_ids(&["ami-notahexidatall"]),
                vec![p("IncludeAllResourceTypes", "true")],
            ]
            .concat(),
            "InvalidAMIID.Malformed",
        ),
        (
            "an image id of the wrong length",
            [
                image_ids(&["ami-0a1b2c3"]),
                vec![p("IncludeAllResourceTypes", "true")],
            ]
            .concat(),
            "InvalidAMIID.Malformed",
        ),
        (
            "one malformed id beside a well-formed one",
            [
                image_ids(&[AMI_A, "ami-"]),
                vec![p("IncludeAllResourceTypes", "true")],
            ]
            .concat(),
            "InvalidAMIID.Malformed",
        ),
        (
            "a resource type named twice",
            [
                image_ids(&[AMI_A]),
                resource_type_fields(1, "ec2:Instance", &[]),
                resource_type_fields(2, "ec2:Instance", &[("state-name", &["running"])]),
            ]
            .concat(),
            "InvalidParameterValue",
        ),
        (
            "version-depth on ec2:Instance",
            [
                image_ids(&[AMI_A]),
                one_resource_type("ec2:Instance", &[("version-depth", &["5"])]),
            ]
            .concat(),
            "InvalidParameterValue",
        ),
        (
            "state-name on ec2:LaunchTemplate",
            [
                image_ids(&[AMI_A]),
                one_resource_type("ec2:LaunchTemplate", &[("state-name", &["running"])]),
            ]
            .concat(),
            "InvalidParameterValue",
        ),
        (
            "an option on a resource type that documents none",
            [
                image_ids(&[AMI_A]),
                one_resource_type("ssm:Parameter", &[("state-name", &["running"])]),
            ]
            .concat(),
            "InvalidParameterValue",
        ),
        (
            "a state-name value the model does not carry",
            [
                image_ids(&[AMI_A]),
                one_resource_type("ec2:Instance", &[("state-name", &["available"])]),
            ]
            .concat(),
            "InvalidParameterValue",
        ),
        (
            "a member-cased state-name value",
            [
                image_ids(&[AMI_A]),
                one_resource_type("ec2:Instance", &[("state-name", &["Running"])]),
            ]
            .concat(),
            "InvalidParameterValue",
        ),
        (
            "version-depth below the published minimum",
            [
                image_ids(&[AMI_A]),
                one_resource_type("ec2:LaunchTemplate", &[("version-depth", &["0"])]),
            ]
            .concat(),
            "InvalidParameterValue",
        ),
        (
            "version-depth one past the published maximum",
            [
                image_ids(&[AMI_A]),
                one_resource_type("ec2:LaunchTemplate", &[("version-depth", &["10001"])]),
            ]
            .concat(),
            "InvalidParameterValue",
        ),
        (
            "a negative version-depth",
            [
                image_ids(&[AMI_A]),
                one_resource_type("ec2:LaunchTemplate", &[("version-depth", &["-1"])]),
            ]
            .concat(),
            "InvalidParameterValue",
        ),
        (
            "a version-depth that is not a number",
            [
                image_ids(&[AMI_A]),
                one_resource_type("ec2:LaunchTemplate", &[("version-depth", &["ten"])]),
            ]
            .concat(),
            "InvalidParameterValue",
        ),
        (
            "two values for a one-value option",
            [
                image_ids(&[AMI_A]),
                one_resource_type("ec2:LaunchTemplate", &[("version-depth", &["1", "2"])]),
            ]
            .concat(),
            "InvalidParameterValue",
        ),
        (
            "version-depth named twice",
            [
                image_ids(&[AMI_A]),
                one_resource_type(
                    "ec2:LaunchTemplate",
                    &[("version-depth", &["1"]), ("version-depth", &["2"])],
                ),
            ]
            .concat(),
            "InvalidParameterValue",
        ),
        (
            "state-name named twice",
            [
                image_ids(&[AMI_A]),
                one_resource_type(
                    "ec2:Instance",
                    &[("state-name", &["running"]), ("state-name", &["stopped"])],
                ),
            ]
            .concat(),
            "InvalidParameterValue",
        ),
        (
            "a page size above the service's ceiling",
            [
                image_ids(&[AMI_A]),
                vec![
                    p("IncludeAllResourceTypes", "true"),
                    p("MaxResults", (MAX_PAGE_SIZE + 1).to_string()),
                ],
            ]
            .concat(),
            "InvalidParameterValue",
        ),
        (
            "a token this service did not issue",
            [
                image_ids(&[AMI_A]),
                vec![
                    p("IncludeAllResourceTypes", "true"),
                    p("NextToken", "not-a-token-this-service-minted"),
                ],
            ]
            .concat(),
            "InvalidPaginationToken",
        ),
    ];

    for (case, params, code) in cases {
        let (status, body) = describe(&seeded.router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got {body}"
        );
        assert!(
            references(&body).is_empty(),
            "case {case:?}: a refusal must report no reference, got {body}"
        );
    }
}

/// A resource type outside the modelled `ImageReferenceResourceType` enum is refused with
/// HTTP 400, never read as one of the supported types.
///
/// The modelled enum is a constraint the request walk enforces before the handler runs, so
/// the refusal may come from either boundary; the status is what both agree on. The rows
/// differ in how the value is wrong: another EC2 resource, a type with no namespace, a
/// member-cased value, and the empty string.
#[tokio::test]
async fn an_unsupported_resource_type_is_refused() {
    let seeded = Seeded::new().await;
    for (case, value) in [
        ("another EC2 resource", "ec2:Volume"),
        ("no namespace", "Instance"),
        ("member-cased", "EC2:Instance"),
        ("empty", ""),
    ] {
        let params = [image_ids(&[AMI_A]), one_resource_type(value, &[])].concat();
        let (status, body) = describe(&seeded.router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for ResourceType {value:?}, got {status} {body}"
        );
        assert!(
            references(&body).is_empty(),
            "case {case:?}: a refusal must report no reference, got {body}"
        );
    }
}

/// A `DryRun` request is answered with HTTP 412 `DryRunOperation` and reports nothing, and
/// reading the references twice answers the same set and leaves the store as it was: this
/// operation reports, it does not mutate.
#[tokio::test]
async fn a_read_changes_nothing_a_later_caller_sees() {
    let seeded = Seeded::new().await;
    let named = [
        image_ids(&[AMI_A, AMI_B]),
        vec![p("IncludeAllResourceTypes", "true")],
    ]
    .concat();

    let first = described(&seeded.router, "first read", &named).await;
    assert_eq!(
        first,
        seeded.all_references(),
        "the first read must report the store, got {first:?}"
    );

    let dry_run = [named.clone(), vec![p("DryRun", "true")]].concat();
    let (status, body) = describe(&seeded.router, &dry_run).await;
    assert_eq!(
        status, 412,
        "case \"dry run\": expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case \"dry run\": expected DryRunOperation, got {body}"
    );

    let (instances, templates, states) = {
        let state = seeded.engine.state::<Ec2State>();
        let store = state.lock();
        let mut states: Vec<(String, &'static str)> = store
            .instances
            .values()
            .map(|instance| (instance.instance_id.clone(), instance.state.as_str()))
            .collect();
        states.sort();
        (store.instances.len(), store.launch_templates.len(), states)
    };

    let second = described(&seeded.router, "second read", &named).await;
    assert_eq!(
        second, first,
        "the second read must report the same set, got {second:?}"
    );

    let state = seeded.engine.state::<Ec2State>();
    let store = state.lock();
    let mut after: Vec<(String, &'static str)> = store
        .instances
        .values()
        .map(|instance| (instance.instance_id.clone(), instance.state.as_str()))
        .collect();
    after.sort();
    assert_eq!(
        (store.instances.len(), store.launch_templates.len(), after),
        (instances, templates, states),
        "reading the references must leave the store as it was"
    );
}
