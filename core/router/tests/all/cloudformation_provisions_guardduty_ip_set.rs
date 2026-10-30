// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::GuardDuty::IPSet` really creates a trusted
//! IP list in Amazon GuardDuty, and the id the stack hands back is the one GuardDuty answers
//! to.
//!
//! The rule under test: **a stack's physical resource id is the identifier the service that
//! owns the resource assigned — for a type whose identifier that service MINTS, the one its
//! create answered with — so `Ref` and `Fn::GetAtt` resolve to a value the owning service's
//! own read resolves to the resource the template configured.** Two crates have to agree for
//! that to hold (CloudFormation makes the calls; GuardDuty serves them and mints the ids), so
//! the suite lives here rather than in either crate. It is the sibling of
//! `cloudformation_provisions_guardduty_detector.rs`, which pins the same rule for the
//! detector a list hangs off: here the resource is a CHILD, identified by the pair
//! `(DetectorId, IpSetId)`, whose detector half is itself an identifier GuardDuty minted
//! during the same deployment.
//!
//! The rows differ along what the rule generalises over: the list's format (four of the six
//! the model admits), the S3 URL form its location is written in, whether the template
//! activates the list, whether it NAMES the list at all (CloudFormation generates the name
//! for one that does not), whether it carries `Tags` and an `ExpectedBucketOwner`, whether it
//! is written in JSON or YAML, and which intrinsic the outputs read the resource with (`Ref`,
//! `Fn::GetAtt … Id`, `Fn::Sub`). Each row deploys in its own Region, because GuardDuty allows
//! one detector per account per Region.
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked — except the two ids, which are GuardDuty's to mint: what is
//! checked about the list's id is that ONE value appears everywhere the stack reports the
//! resource's identity, that it has the form GuardDuty mints, that it is not the detector's,
//! and that GuardDuty's own `ListIPSets` and `GetIPSet` resolve it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{elements, form_encode, opt_element};

/// Long enough for the stack's clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) and the
/// list's own activation span (`IP_SET_TRANSITION_MILLIS`, 500 ms) to come due, so the stack
/// reports the template it deployed and GuardDuty reports the list settled.
const PAST_EVERY_SPAN: Duration = Duration::from_millis(1200);

/// The length and alphabet of a GuardDuty identifier: 32 lowercase hexadecimal characters,
/// the form that service's own `mint_id` produces for a detector id and for an `IpSetId`
/// alike.
const GUARDDUTY_ID_LEN: usize = 32;

/// The logical ids both resources of every row are declared under.
const DETECTOR_LOGICAL_ID: &str = "GDDetector";
const IP_SET_LOGICAL_ID: &str = "GDIPSet";

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way (the
/// credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_guardduty_guarddutyapiservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which
/// is what routes the request and what each handler reads its Region from. The signature is
/// never checked: this router runs with request authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
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
        .header("host", format!("cloudformation.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one GuardDuty operation over its restJson1 wire, signed for `region`.
async fn guardduty(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("guardduty.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("guardduty", region))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as. Each
/// `member` of the list carries exactly one `OutputKey` and one `OutputValue`, so the two
/// document-ordered lists pair up.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// A `DescribeStackResources` answer as the map from each resource's LOGICAL id to the
/// PHYSICAL one the stack reports for it. One `member` per resource, each carrying exactly
/// one of each, so the two document-ordered lists pair up.
fn physical_ids_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "LogicalResourceId")
        .into_iter()
        .zip(elements(xml, "PhysicalResourceId"))
        .collect()
}

/// One row of the table: a template, and what the rule says the trusted IP list it declares
/// must then be.
struct Row {
    case: &'static str,
    /// The Region the row deploys in. One detector per account per Region, so each row has its
    /// own — which also exercises that the stack's Region is the one the list is created in.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: fn() -> String,
    /// The `Name` `GetIPSet` must report, or `None` for a row whose declaration names the list
    /// nowhere: CloudFormation generates the name then, from the stack and the logical id.
    expected_name: Option<&'static str>,
    /// The `Format` `GetIPSet` must report, which the template declares.
    expected_format: &'static str,
    /// The `Location` `GetIPSet` must report, which the template declares.
    expected_location: &'static str,
    /// The `Status` `GetIPSet` must report once the activation span has come due, which the
    /// template's `Activate` decides.
    expected_status: &'static str,
    /// The `ExpectedBucketOwner` `GetIPSet` must report, for a row that declares one.
    expected_bucket_owner: Option<&'static str>,
    /// The tags `GetIPSet` must report, which the template's `Tags` decide.
    expected_tags: &'static [(&'static str, &'static str)],
    /// The output keys whose value must be the list's id, each written with a different
    /// intrinsic over the same resource.
    ip_set_id_outputs: &'static [&'static str],
}

const ROWS: &[Row] = &[
    Row {
        case: "an activated TXT list, named, tagged, JSON, Ref and Fn::GetAtt Id",
        region: "us-east-1",
        template: || {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "a GuardDuty trusted IP list",
                "Resources": {
                    DETECTOR_LOGICAL_ID: {
                        "Type": "AWS::GuardDuty::Detector",
                        "Properties": { "Enable": true }
                    },
                    IP_SET_LOGICAL_ID: {
                        "Type": "AWS::GuardDuty::IPSet",
                        "Properties": {
                            "DetectorId": { "Ref": DETECTOR_LOGICAL_ID },
                            "Name": "office-ranges",
                            "Format": "TXT",
                            "Location": "https://s3.us-east-1.amazonaws.com/lists/office.txt",
                            "Activate": true,
                            "Tags": [
                                { "Key": "Owner", "Value": "security" },
                                { "Key": "Tier", "Value": "prod" }
                            ]
                        }
                    }
                },
                "Outputs": {
                    "IpSetId": { "Value": { "Ref": IP_SET_LOGICAL_ID } },
                    "IpSetIdAttribute": {
                        "Value": { "Fn::GetAtt": [IP_SET_LOGICAL_ID, "Id"] }
                    }
                }
            })
            .to_string()
        },
        expected_name: Some("office-ranges"),
        expected_format: "TXT",
        expected_location: "https://s3.us-east-1.amazonaws.com/lists/office.txt",
        expected_status: "ACTIVE",
        expected_bucket_owner: None,
        // `TagKey`'s pattern admits no digits (`^(?!aws:)[a-zA-Z+-=._:/]+$`), so the keys here
        // are letters.
        expected_tags: &[("Owner", "security"), ("Tier", "prod")],
        ip_set_id_outputs: &["IpSetId", "IpSetIdAttribute"],
    },
    Row {
        case: "a list the template does not activate, STIX, an s3:// location, YAML",
        region: "us-west-2",
        template: || {
            [
                "Resources:",
                "  GDDetector:",
                "    Type: AWS::GuardDuty::Detector",
                "    Properties:",
                "      Enable: true",
                "  GDIPSet:",
                "    Type: AWS::GuardDuty::IPSet",
                "    Properties:",
                "      DetectorId: !Ref GDDetector",
                "      Name: dormant list",
                "      Format: STIX",
                "      Location: s3://threat-lists/dormant.stix",
                "      Activate: false",
                "Outputs:",
                "  IpSetId:",
                "    Value: !Ref GDIPSet",
                "",
            ]
            .join("\n")
        },
        // A name with whitespace in it: "allowed characters are alphanumeric, whitespace,
        // dash (-), and underscores (_)" (`CreateIPSetRequest$Name`).
        expected_name: Some("dormant list"),
        expected_format: "STIX",
        expected_location: "s3://threat-lists/dormant.stix",
        expected_status: "INACTIVE",
        expected_bucket_owner: None,
        expected_tags: &[],
        ip_set_id_outputs: &["IpSetId"],
    },
    Row {
        case: "an ALIEN_VAULT list with an ExpectedBucketOwner and an Fn::Sub output",
        region: "eu-west-1",
        template: || {
            json!({
                "Resources": {
                    DETECTOR_LOGICAL_ID: {
                        "Type": "AWS::GuardDuty::Detector",
                        "Properties": { "Enable": true, "FindingPublishingFrequency": "ONE_HOUR" }
                    },
                    IP_SET_LOGICAL_ID: {
                        "Type": "AWS::GuardDuty::IPSet",
                        "Properties": {
                            "DetectorId": { "Ref": DETECTOR_LOGICAL_ID },
                            "Name": "partner_ranges",
                            "Format": "ALIEN_VAULT",
                            "Location": "https://partner-lists.s3.eu-west-1.amazonaws.com/ranges",
                            "Activate": true,
                            "ExpectedBucketOwner": "000000000000"
                        }
                    }
                },
                "Outputs": {
                    "IpSetId": { "Value": { "Fn::GetAtt": [IP_SET_LOGICAL_ID, "Id"] } },
                    "IpSetPath": {
                        "Value": {
                            "Fn::Sub": [
                                "detector/${Detector}/ipset/${IpSet}",
                                {
                                    "Detector": { "Ref": DETECTOR_LOGICAL_ID },
                                    "IpSet": { "Ref": IP_SET_LOGICAL_ID }
                                }
                            ]
                        }
                    }
                }
            })
            .to_string()
        },
        expected_name: Some("partner_ranges"),
        expected_format: "ALIEN_VAULT",
        // The virtual-hosted-style S3 URL form, the third `Location` spelling that service
        // admits beside the path-style and `s3://` ones above.
        expected_location: "https://partner-lists.s3.eu-west-1.amazonaws.com/ranges",
        expected_status: "ACTIVE",
        expected_bucket_owner: Some("000000000000"),
        expected_tags: &[],
        ip_set_id_outputs: &["IpSetId"],
    },
    Row {
        case: "a list the template names nowhere, OTX_CSV, YAML, Fn::GetAtt only",
        region: "ap-northeast-1",
        template: || {
            [
                "Resources:",
                "  GDDetector:",
                "    Type: AWS::GuardDuty::Detector",
                "    Properties:",
                "      Enable: true",
                "  GDIPSet:",
                "    Type: AWS::GuardDuty::IPSet",
                "    Properties:",
                "      DetectorId: !Ref GDDetector",
                "      Format: OTX_CSV",
                "      Location: https://s3-ap-northeast-1.amazonaws.com/lists/unnamed.csv",
                "      Activate: true",
                "Outputs:",
                "  IpSetId:",
                "    Value: !GetAtt GDIPSet.Id",
                "",
            ]
            .join("\n")
        },
        // CloudFormation's own `<stack>-<logical id>-<unique id>`, checked by form below
        // rather than by a value this suite would have to invent.
        expected_name: None,
        expected_format: "OTX_CSV",
        // The dashed regional path-style S3 endpoint spelling.
        expected_location: "https://s3-ap-northeast-1.amazonaws.com/lists/unnamed.csv",
        expected_status: "ACTIVE",
        expected_bucket_owner: None,
        expected_tags: &[],
        ip_set_id_outputs: &["IpSetId"],
    },
];

/// Every row: the id the stack reports for its `AWS::GuardDuty::IPSet` is the id GuardDuty
/// minted, that service's own `ListIPSets` and `GetIPSet` resolve it to the list the template
/// configured, and deleting the stack deletes it there.
#[tokio::test]
async fn a_stack_provisions_the_guardduty_ip_set_its_template_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("guardduty-ipset-stack-{index}");
        let template = (row.template)();

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", &stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        // The stack reports the template it deployed once the create COMPLETES.
        tokio::time::sleep(PAST_EVERY_SPAN).await;
        let (status, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
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

        // The physical resource ids of the two declared resources: what a client reads out of
        // DescribeStackResources, and what every identity output must carry.
        let (status, resources) = cloudformation(
            &router,
            region,
            "DescribeStackResources",
            &[("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
             body {resources}"
        );
        let reported = physical_ids_of(&resources);
        let detector_id = reported
            .get(DETECTOR_LOGICAL_ID)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no {DETECTOR_LOGICAL_ID} in {resources}"));
        let ip_set_id = reported
            .get(IP_SET_LOGICAL_ID)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no {IP_SET_LOGICAL_ID} in {resources}"));
        assert_eq!(
            ip_set_id.len(),
            GUARDDUTY_ID_LEN,
            "case {case:?}: expected a {GUARDDUTY_ID_LEN}-character GuardDuty IpSetId, got \
             {ip_set_id:?}"
        );
        assert!(
            ip_set_id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "case {case:?}: expected a lowercase hexadecimal IpSetId, got {ip_set_id:?}"
        );
        assert_ne!(
            ip_set_id, detector_id,
            "case {case:?}: expected the list's own id, got the detector's ({detector_id:?})"
        );

        let outputs = outputs_of(&described);
        for key in row.ip_set_id_outputs {
            assert_eq!(
                outputs.get(*key),
                Some(&ip_set_id),
                "case {case:?}: expected output {key:?} to be the IpSetId {ip_set_id:?}, got {:?}",
                outputs.get(*key)
            );
        }
        // An output composing both ids is the whole URI the list answers on, so it carries
        // the detector's minted id as well as the list's.
        if let Some(path) = outputs.get("IpSetPath") {
            assert_eq!(
                path,
                &format!("detector/{detector_id}/ipset/{ip_set_id}"),
                "case {case:?}: expected the composed output to carry both minted ids, got \
                 {path:?}"
            );
        }

        // GuardDuty itself lists the id under the stack's detector …
        let (status, listed) =
            guardduty(&router, region, "GET", &format!("/detector/{detector_id}/ipset")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListIPSets on the detector the stack \
             reported, got {status} with body {listed}"
        );
        assert_eq!(
            listed.get("ipSetIds").and_then(Value::as_array).map(|ids| {
                ids.iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect::<Vec<String>>()
            }),
            Some(vec![ip_set_id.clone()]),
            "case {case:?}: expected ListIPSets to report exactly the stack's list, got {listed}"
        );

        // … and resolves it to the list the template configured.
        let (status, ip_set) = guardduty(
            &router,
            region,
            "GET",
            &format!("/detector/{detector_id}/ipset/{ip_set_id}"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetIPSet on the id the stack reported, got \
             {status} with body {ip_set}"
        );
        let reported_name = ip_set
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        match row.expected_name {
            Some(expected) => assert_eq!(
                reported_name, expected,
                "case {case:?}: expected name {expected:?}, got {ip_set}"
            ),
            // The name CloudFormation generated for a declaration that states none:
            // `<stack name>-<logical id>-<unique id>`.
            None => {
                let prefix = format!("{stack_name}-{IP_SET_LOGICAL_ID}-");
                assert!(
                    reported_name.starts_with(&prefix) && reported_name.len() > prefix.len(),
                    "case {case:?}: expected a generated name starting {prefix:?}, got \
                     {reported_name:?}"
                );
            }
        }
        assert_eq!(
            ip_set.get("format").and_then(Value::as_str),
            Some(row.expected_format),
            "case {case:?}: expected format {:?}, got {ip_set}",
            row.expected_format
        );
        assert_eq!(
            ip_set.get("location").and_then(Value::as_str),
            Some(row.expected_location),
            "case {case:?}: expected location {:?}, got {ip_set}",
            row.expected_location
        );
        assert_eq!(
            ip_set.get("status").and_then(Value::as_str),
            Some(row.expected_status),
            "case {case:?}: expected status {:?}, got {ip_set}",
            row.expected_status
        );
        assert_eq!(
            ip_set.get("expectedBucketOwner").and_then(Value::as_str),
            row.expected_bucket_owner,
            "case {case:?}: expected expectedBucketOwner {:?}, got {ip_set}",
            row.expected_bucket_owner
        );
        let reported_tags = ip_set.get("tags").cloned().unwrap_or(Value::Null);
        for (key, value) in row.expected_tags {
            assert_eq!(
                reported_tags.get(*key).and_then(Value::as_str),
                Some(*value),
                "case {case:?}: expected tag {key:?} to be {value:?}, got {reported_tags}"
            );
        }
        // Beside what the template declared, CloudFormation records three tags of its own on
        // every resource it provisions — `aws:cloudformation:stack-name`,
        // `aws:cloudformation:stack-id` and `aws:cloudformation:logical-id` — plus any
        // stack-level `Tags` (these rows supply none).
        let stack_id = opt_element(&described, "StackId")
            .unwrap_or_else(|| panic!("case {case:?}: no <StackId> in {described}"));
        for (key, value) in [
            ("aws:cloudformation:stack-name", stack_name.as_str()),
            ("aws:cloudformation:stack-id", stack_id.as_str()),
            ("aws:cloudformation:logical-id", IP_SET_LOGICAL_ID),
        ] {
            assert_eq!(
                reported_tags.get(key).and_then(Value::as_str),
                Some(value),
                "case {case:?}: expected tag {key:?} to be {value:?}, got {reported_tags}"
            );
        }
        assert_eq!(
            reported_tags.as_object().map(serde_json::Map::len),
            Some(row.expected_tags.len() + 3),
            "case {case:?}: expected exactly the {} tag(s) the template declared beside the \
             three CloudFormation records, got {reported_tags}",
            row.expected_tags.len()
        );

        // Deleting the stack deletes the list GuardDuty holds for it — and the detector it
        // hung off, so the pair resolves to nothing at all.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        let (status, gone) = guardduty(
            &router,
            region,
            "GET",
            &format!("/detector/{detector_id}/ipset/{ip_set_id}"),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from GetIPSet once the stack deleted the list, \
             got {status} with body {gone}"
        );
    }
}

/// A list whose declaration names a detector GuardDuty holds none of is NOT reported as
/// created: that service's refusal is what the stack carries, and nothing is passed off as a
/// resource it created.
///
/// This is the other half of the rule above — a stack reports what the owning service really
/// decided — on the property that makes the other half of this type's identity.
#[tokio::test]
async fn an_ip_set_for_an_unknown_detector_fails_the_stack() {
    let router = build_router();
    let case = "an IPSet declaring a detector id GuardDuty does not hold";
    let region = "us-east-2";
    let stack_name = "guardduty-orphan-ipset";
    // A well-formed identifier of the shape GuardDuty mints, which no detector of this
    // account and Region was created under.
    let unknown_detector = "0".repeat(GUARDDUTY_ID_LEN);
    let template = json!({
        "Resources": {
            IP_SET_LOGICAL_ID: {
                "Type": "AWS::GuardDuty::IPSet",
                "Properties": {
                    "DetectorId": &unknown_detector,
                    "Name": "orphan-ranges",
                    "Format": "TXT",
                    "Location": "https://s3.us-east-2.amazonaws.com/lists/orphan.txt",
                    "Activate": true
                }
            }
        }
    })
    .to_string();

    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", &template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_EVERY_SPAN).await;

    let (_, described) =
        cloudformation(&router, region, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("ROLLBACK_COMPLETE"),
        "case {case:?}: expected the failed create to be rolled back, got {described}"
    );
    let reason = opt_element(&described, "StackStatusReason").unwrap_or_default();
    assert!(
        reason.contains(IP_SET_LOGICAL_ID),
        "case {case:?}: expected the failure to name the resource, got {reason:?}"
    );
    // And GuardDuty holds nothing under that detector id: the refusal was the whole of what
    // happened there.
    let (status, listed) = guardduty(
        &router,
        region,
        "GET",
        &format!("/detector/{unknown_detector}/ipset"),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected GuardDuty to refuse the unknown detector, got {status} with \
         body {listed}"
    );
}
