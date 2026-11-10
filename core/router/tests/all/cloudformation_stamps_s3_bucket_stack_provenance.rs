// SPDX-License-Identifier: CC0-1.0

//! The tags an `AWS::S3::Bucket` a CloudFormation stack created carries, read back through
//! Amazon S3's own wire.
//!
//! The rule under test: **a resource CloudFormation provisions carries, in the service that
//! owns it, the stack-level `Tags` the stack operation supplied PLUS the three tags
//! CloudFormation records itself — `aws:cloudformation:stack-name`,
//! `aws:cloudformation:stack-id` and `aws:cloudformation:logical-id` — and a key the
//! resource's own `Tags` property states keeps the value that property gives it, except
//! those three, which are CloudFormation's to set. The reserved `aws:` namespace is closed
//! to a CALLER's request, not to the service setting its own tags, so the owning service's
//! tag read on the handle the stack publishes answers with the whole set** — which is what
//! a bucket-to-stack reverse lookup and tag-based cost attribution key off. Two crates have
//! to agree for that to hold (CloudFormation stamps the tags, Amazon S3 stores and reports
//! them), so the suite lives here rather than in either crate.
//!
//! The rows differ along what the rule generalises over: how many stack-level tags the
//! deployment supplies (none, one, several), how many buckets the stack declares (so the
//! `logical-id` tag differs per bucket while the other two do not), whether a bucket
//! declares `Tags` of its own, whether one of those keys COLLIDES with a stack tag, whether
//! a declaration spells one of the three reserved keys (it must not displace
//! CloudFormation's value), and whether the bucket's name is spelled by the template or left
//! to CloudFormation.
//!
//! Every expected value is computed here from what the row declares plus the stack id
//! `DescribeStacks` reports — never read back out of the tag answer being checked. The
//! bucket each tag read addresses is the physical id the STACK reports for the resource
//! ("Ref returns the bucket name"), so what is read is the handle a client holds.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use super::query_wire::{elements, form_encode, opt_element, physical_ids_of};

/// The Region every request here is signed for, and so the Region the stack's buckets
/// belong to.
const REGION: &str = "us-east-1";

/// Long enough for the stack's clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) to come
/// due, so the stack reports the resources it deployed.
const PAST_STACK_SPAN: Duration = Duration::from_millis(900);

/// The tag keys CloudFormation records on every resource it provisions.
///
/// Source: the `aws:cloudformation:*` keys CloudFormation's own documentation names for a
/// stack's resources, which this emulator's CloudFormation crate carries as
/// `state::provisioning::{STACK_NAME_TAG, STACK_ID_TAG, LOGICAL_ID_TAG}`.
const STACK_NAME_TAG: &str = "aws:cloudformation:stack-name";
const STACK_ID_TAG: &str = "aws:cloudformation:stack-id";
const LOGICAL_ID_TAG: &str = "aws:cloudformation:logical-id";

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way
/// (the credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: this router runs with request
/// authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).into_owned())
}

/// `GET /<bucket>?tagging` — Amazon S3's `GetBucketTagging`, as a CLIENT's request: the read
/// a caller holding only the bucket name makes.
async fn get_bucket_tagging(router: &MultiServiceRouter, bucket: &str) -> (u16, String) {
    let host = format!("s3.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("https://{host}/{bucket}?tagging"))
        .header("host", host)
        .header("authorization", authorization("s3"))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).into_owned())
}

/// The tag set a `GetBucketTagging` body reports, as the key-to-value map a client reads it
/// as. Each `<Tag>` carries exactly one `<Key>` and one `<Value>`, so the two document-ordered
/// lists pair up.
fn tag_set_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "Key")
        .into_iter()
        .zip(elements(xml, "Value"))
        .collect()
}

/// One bucket a row declares: its logical id in the template and the `Tags` that declaration
/// asks for.
struct Declared {
    logical_id: &'static str,
    declared: &'static [(&'static str, &'static str)],
}

/// One row: a template, the stack-level tags the deployment supplies, and the buckets whose
/// tag sets the rule decides.
struct Row {
    case: &'static str,
    /// The `Tags` the `CreateStack` call supplies, propagated to every resource.
    stack_tags: &'static [(&'static str, &'static str)],
    template: fn() -> String,
    resources: &'static [Declared],
}

const ROWS: &[Row] = &[
    Row {
        case: "no stack tags and no declared tags: the three CloudFormation records alone",
        stack_tags: &[],
        template: || {
            json!({
                "Resources": {
                    "Assets": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": { "BucketName": "provenance-plain" }
                    }
                }
            })
            .to_string()
        },
        resources: &[Declared {
            logical_id: "Assets",
            declared: &[],
        }],
    },
    Row {
        case: "one stack tag beside a declaration of the bucket's own",
        stack_tags: &[("team", "platform")],
        template: || {
            json!({
                "Resources": {
                    "Assets": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {
                            "BucketName": "provenance-declared",
                            "Tags": [{ "Key": "Service", "Value": "assets" }]
                        }
                    }
                }
            })
            .to_string()
        },
        resources: &[Declared {
            logical_id: "Assets",
            declared: &[("Service", "assets")],
        }],
    },
    Row {
        case: "several stack tags beside a declaration that collides on one of them",
        stack_tags: &[("Owner", "platform"), ("Tier", "prod")],
        template: || {
            json!({
                "Resources": {
                    "Assets": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {
                            "BucketName": "provenance-collision",
                            "Tags": [
                                { "Key": "Tier", "Value": "dev" },
                                { "Key": "Service", "Value": "assets" }
                            ]
                        }
                    }
                }
            })
            .to_string()
        },
        resources: &[Declared {
            logical_id: "Assets",
            declared: &[("Tier", "dev"), ("Service", "assets")],
        }],
    },
    Row {
        case: "a declaration spelling a reserved key CloudFormation sets itself",
        stack_tags: &[("Owner", "platform")],
        template: || {
            json!({
                "Resources": {
                    "Assets": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {
                            "BucketName": "provenance-reserved",
                            "Tags": [
                                { "Key": "aws:cloudformation:logical-id", "Value": "Forged" },
                                { "Key": "Service", "Value": "assets" }
                            ]
                        }
                    }
                }
            })
            .to_string()
        },
        resources: &[Declared {
            logical_id: "Assets",
            // The reserved key the declaration spells is NOT part of the expected set as
            // the declaration wrote it: it is CloudFormation's to set, and the expectation
            // below gives it CloudFormation's own value.
            declared: &[("Service", "assets")],
        }],
    },
    Row {
        case: "two buckets in one stack, so only the logical-id tag differs",
        stack_tags: &[("Owner", "platform")],
        template: || {
            json!({
                "Resources": {
                    "Assets": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": { "BucketName": "provenance-pair-assets" }
                    },
                    "Reports": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {
                            "BucketName": "provenance-pair-reports",
                            "Tags": [{ "Key": "Service", "Value": "reports" }]
                        }
                    }
                }
            })
            .to_string()
        },
        resources: &[
            Declared {
                logical_id: "Assets",
                declared: &[],
            },
            Declared {
                logical_id: "Reports",
                declared: &[("Service", "reports")],
            },
        ],
    },
    Row {
        case: "a YAML template whose bucket CloudFormation names itself",
        stack_tags: &[("Owner", "platform")],
        template: || {
            [
                "Resources:",
                "  Assets:",
                "    Type: AWS::S3::Bucket",
                "",
            ]
            .join("\n")
        },
        resources: &[Declared {
            logical_id: "Assets",
            declared: &[],
        }],
    },
];

/// The tag set the rule says the bucket `logical_id` of this row ends up carrying, in the
/// stack `stack_name` whose id is `stack_id`.
fn expected_tags(
    row: &Row,
    logical_id: &str,
    stack_name: &str,
    stack_id: &str,
) -> BTreeMap<String, String> {
    let mut expected: BTreeMap<String, String> = BTreeMap::new();
    for resource in row.resources {
        if resource.logical_id == logical_id {
            for (key, value) in resource.declared {
                expected.insert((*key).to_string(), (*value).to_string());
            }
        }
    }
    // A stack tag whose key the declaration already states does not displace it.
    for (key, value) in row.stack_tags {
        expected
            .entry((*key).to_string())
            .or_insert_with(|| (*value).to_string());
    }
    // The three CloudFormation records itself, which nothing displaces.
    expected.insert(STACK_NAME_TAG.to_string(), stack_name.to_string());
    expected.insert(STACK_ID_TAG.to_string(), stack_id.to_string());
    expected.insert(LOGICAL_ID_TAG.to_string(), logical_id.to_string());
    expected
}

/// The `Tags.member.N.Key`/`Value` request members a stack-level tag list is.
fn tag_members(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut members = Vec::new();
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        members.push((format!("Tags.member.{position}.Key"), (*key).to_string()));
        members.push((
            format!("Tags.member.{position}.Value"),
            (*value).to_string(),
        ));
    }
    members
}

/// Deploy one row's stack and report the stack id `DescribeStacks` gives it together with
/// the physical id the stack publishes for each of its resources.
async fn deployed(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    row: &Row,
) -> (String, BTreeMap<String, String>) {
    let template = (row.template)();
    let mut members: Vec<(String, String)> = vec![
        ("StackName".to_string(), stack_name.to_string()),
        ("TemplateBody".to_string(), template),
    ];
    members.extend(tag_members(row.stack_tags));
    let borrowed: Vec<(&str, &str)> = members
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let (status, body) = cloudformation(router, "CreateStack", &borrowed).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );

    tokio::time::sleep(PAST_STACK_SPAN).await;
    let (status, described) =
        cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
         {described}"
    );
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
    );
    let stack_id = opt_element(&described, "StackId").unwrap_or_default();

    let (status, resources) =
        cloudformation(router, "DescribeStackResources", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with body \
         {resources}"
    );
    (stack_id, physical_ids_of(&resources))
}

/// Every row: `GetBucketTagging` on the bucket the stack published answers with the stack's
/// own tags, the declaration's own tags and the three tags CloudFormation records — so the
/// bucket names the stack and its place in that stack's template.
#[tokio::test]
async fn a_stack_stamps_its_provenance_onto_the_buckets_it_creates() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        // One router per row, so a bucket name a row spells cannot collide with another's.
        let router = build_router();
        let stack_name = format!("s3-bucket-provenance-{index}");
        let (stack_id, reported) = deployed(&router, case, &stack_name, row).await;
        assert!(
            stack_id.contains(&stack_name),
            "case {case:?}: expected the reported stack id to name the stack, got {stack_id:?}"
        );

        for resource in row.resources {
            let logical_id = resource.logical_id;
            // The handle the stack publishes for the resource, which for this type IS the
            // bucket name ("Ref returns the bucket name").
            let bucket = reported.get(logical_id).cloned().unwrap_or_else(|| {
                panic!("case {case:?}: no physical id for {logical_id:?} in {reported:?}")
            });

            let (status, body) = get_bucket_tagging(&router, &bucket).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from GetBucketTagging({bucket}), got {status} \
                 with body {body}"
            );
            let held = tag_set_of(&body);
            let expected = expected_tags(row, logical_id, &stack_name, &stack_id);
            assert_eq!(
                held, expected,
                "case {case:?}: expected {logical_id} to carry {expected:?}, got {held:?}"
            );
        }
    }
}

/// The three keys are the bucket-to-stack REVERSE LOOKUP: the stack id they carry is the one
/// `DescribeStacks` reports, so a client holding only a bucket name reaches that stack's own
/// record — and its resources — from the tag read alone.
#[tokio::test]
async fn the_stamped_stack_id_resolves_back_to_the_stack_that_created_the_bucket() {
    let router = build_router();
    let stack_name = "s3-bucket-reverse-lookup";
    let row = &ROWS[1];
    let (_, reported) = deployed(&router, row.case, stack_name, row).await;
    let bucket = reported
        .get("Assets")
        .cloned()
        .unwrap_or_else(|| panic!("no physical id for \"Assets\" in {reported:?}"));

    // Everything the client knows from here on is what the tag read answered.
    let (status, body) = get_bucket_tagging(&router, &bucket).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from GetBucketTagging({bucket}), got {status} with body {body}"
    );
    let held = tag_set_of(&body);
    let stamped_id = held.get(STACK_ID_TAG).cloned().unwrap_or_default();
    let stamped_name = held.get(STACK_NAME_TAG).cloned().unwrap_or_default();
    let stamped_logical_id = held.get(LOGICAL_ID_TAG).cloned().unwrap_or_default();

    // The stack id is a handle `DescribeStacks` resolves, and the stack it resolves to is the
    // one the bucket belongs to: its name is the stamped name, and the resource it reports
    // under the stamped logical id is the bucket the tag read was made on.
    let (status, described) =
        cloudformation(&router, "DescribeStacks", &[("StackName", &stamped_id)]).await;
    assert_eq!(
        status, 200,
        "expected the stamped stack id {stamped_id:?} to resolve, got {status} with body \
         {described}"
    );
    assert_eq!(
        opt_element(&described, "StackName"),
        Some(stamped_name.clone()),
        "expected the stack the stamped id resolves to be {stamped_name:?}, got {described}"
    );
    let (status, resources) = cloudformation(
        &router,
        "DescribeStackResources",
        &[("StackName", &stamped_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the stamped stack id to resolve for its resources, got {status} with body \
         {resources}"
    );
    assert_eq!(
        physical_ids_of(&resources).get(&stamped_logical_id),
        Some(&bucket),
        "expected the resource at the stamped logical id {stamped_logical_id:?} to be the \
         bucket {bucket}, got {resources}"
    );
}
