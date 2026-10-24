// SPDX-License-Identifier: CC0-1.0

//! Cluster security groups, end to end over the awsQuery wire.
//!
//! A cluster security group is EC2-Classic's access-control mechanism: it "control[s]
//! access to non-VPC clusters" (`smithy_query(query="doc",
//! shape="CreateClusterSecurityGroup")`), while a cluster inside a VPC is governed by
//! VPC security groups instead. EC2-Classic is retired, so every Availability Zone this
//! service can place a cluster in publishes the VPC platform alone — and the rule under
//! test follows from that one published fact:
//!
//! * a call that would mint a cluster security group is refused for EVERY request, with
//!   HTTP 400 and the `InvalidParameterValue` code Amazon Redshift answers a parameter
//!   it cannot honour with, whatever name, description or tags the request carries;
//! * the refusal stores nothing, so a repeated identical call is refused the SAME way
//!   and never with `ClusterSecurityGroupAlreadyExists`, which a persisted group would
//!   have to produce;
//! * the rule's input is observable: no zone this service publishes offers the
//!   EC2-Classic platform, which is what makes the refusal total rather than a choice
//!   this handler makes.
//!
//! The table below therefore spans the request values the rule generalises over — name
//! length (one character and the documented 255-character maximum), the characters a
//! name may carry, the `Default` name the documentation forbids, an empty and a long
//! description, and zero, one and several tags — rather than restating the one recorded
//! request.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Emulator};
use crate::static_catalog::listed_zones;

/// One create request the rule must refuse.
struct CreateCase {
    case: &'static str,
    name: String,
    description: String,
    tags: Vec<(String, String)>,
}

/// The documented maximum length of a cluster security group name.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterSecurityGroupMessage",
/// member="ClusterSecurityGroupName")` — "Must contain no more than 255 alphanumeric
/// characters or hyphens."
const NAME_MAX_LEN: usize = 255;

/// A name of exactly `len` characters built from the documented alphabet
/// (alphanumerics and hyphens), so a row at the published bound cannot fail the
/// boundary validator for a different reason.
fn name_of_length(len: usize) -> String {
    let alphabet = ['s', 'g', '-', '7'];
    (0..len).map(|index| alphabet[index % alphabet.len()]).collect()
}

/// `n` distinct tag pairs, so the tag-count rows differ in the one dimension they are
/// there to vary.
fn tags(n: usize) -> Vec<(String, String)> {
    (0..n)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect()
}

/// Every row of the table: requests that differ in name, description and tags, and must
/// all be refused the same way.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            case: "the shortest name, no tags",
            name: "s".to_string(),
            description: "a security group".to_string(),
            tags: Vec::new(),
        },
        CreateCase {
            case: "a name at the documented 255-character maximum",
            name: name_of_length(NAME_MAX_LEN),
            description: "a security group named at the bound".to_string(),
            tags: Vec::new(),
        },
        CreateCase {
            case: "a mixed-case name, which the service would fold to lower case",
            name: "ExampleSecurityGroup".to_string(),
            description: "a security group named in mixed case".to_string(),
            tags: Vec::new(),
        },
        CreateCase {
            case: "hyphens and digits, the rest of the documented alphabet",
            name: "sg-00-alpha-99".to_string(),
            description: "a security group named with hyphens".to_string(),
            tags: Vec::new(),
        },
        CreateCase {
            case: "the reserved name the documentation forbids",
            name: "Default".to_string(),
            description: "the reserved name".to_string(),
            tags: Vec::new(),
        },
        CreateCase {
            case: "an empty description",
            name: "empty-description".to_string(),
            description: String::new(),
            tags: Vec::new(),
        },
        CreateCase {
            case: "a long description",
            name: "long-description".to_string(),
            description: "d".repeat(512),
            tags: Vec::new(),
        },
        CreateCase {
            case: "one tag",
            name: "one-tag".to_string(),
            description: "tagged once".to_string(),
            tags: tags(1),
        },
        CreateCase {
            case: "several tags",
            name: "several-tags".to_string(),
            description: "tagged several times".to_string(),
            tags: tags(5),
        },
    ]
}

/// One row as awsQuery form parameters: the two required members, plus the flattened
/// `Tags.Tag.<n>.{Key,Value}` list.
fn request_params(case: &CreateCase) -> Vec<(String, String)> {
    let mut params = vec![
        (
            "ClusterSecurityGroupName".to_string(),
            case.name.clone(),
        ),
        ("Description".to_string(), case.description.clone()),
    ];
    for (index, (key, value)) in case.tags.iter().enumerate() {
        let position = index + 1;
        params.push((format!("Tags.Tag.{position}.Key"), key.clone()));
        params.push((format!("Tags.Tag.{position}.Value"), value.clone()));
    }
    params
}

/// `Emulator::call` takes borrowed pairs; the rows build owned parameters.
fn borrowed(params: &[(String, String)]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// Assert one answer is the refusal the rule requires, and nothing else.
fn assert_refused(case: &str, answer: &crate::cluster_lifecycle::Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code().as_deref(),
        Some("InvalidParameterValue"),
        "case {case:?}: expected <Code>InvalidParameterValue</Code>, got {:?}",
        answer.body
    );
    let messages = all_text(&answer.body, "Message");
    assert_eq!(
        messages.first().map(String::as_str),
        Some(state::CLUSTER_SECURITY_GROUPS_UNAVAILABLE),
        "case {case:?}: expected the service's one account of the rule in <Message>, got {:?}",
        answer.body
    );
    // A refusal reports no group: an answer carrying one would mean the call had
    // minted something the service has no cluster to attach it to.
    assert!(
        all_text(&answer.body, "ClusterSecurityGroupName").is_empty(),
        "case {case:?}: a refusal must report no cluster security group, got {:?}",
        answer.body
    );
}

/// Every create request in the table is refused as an invalid parameter value.
#[tokio::test]
async fn every_create_request_is_refused_as_an_invalid_parameter_value() {
    let emulator = Emulator::new();

    for case in create_cases() {
        let params = request_params(&case);
        let answer = emulator
            .call("CreateClusterSecurityGroup", &borrowed(&params))
            .await;
        assert_refused(case.case, &answer);
    }
}

/// The refusal persists nothing: the same request, repeated, is refused identically.
///
/// Had the first call stored a group, the model leaves the service only one answer for
/// the second — `ClusterSecurityGroupAlreadyExists` (HTTP 400) — so an unchanged
/// `InvalidParameterValue` is what shows the store was never written.
#[tokio::test]
async fn a_refused_create_stores_nothing() {
    let emulator = Emulator::new();

    for case in create_cases() {
        let params = request_params(&case);
        for attempt in 1..=3 {
            let answer = emulator
                .call("CreateClusterSecurityGroup", &borrowed(&params))
                .await;
            assert_refused(&format!("{} (attempt {attempt})", case.case), &answer);
            assert_ne!(
                answer.error_code().as_deref(),
                Some("ClusterSecurityGroupAlreadyExists"),
                "case {:?} (attempt {attempt}): nothing may have been stored by an earlier \
                 refusal, got {:?}",
                case.case,
                answer.body
            );
        }
    }
}

/// The rule's input, asserted where a client reads it: no zone offers the platform a
/// cluster security group governs, which is why no request can create one.
#[tokio::test]
async fn no_published_zone_offers_the_platform_such_a_group_governs() {
    let emulator = Emulator::new();

    assert!(
        !state::cluster_security_groups_available(),
        "the published platform list must offer no EC2-Classic platform, got {:?}",
        state::zone_supported_platforms()
    );

    // Two Regions, one publishing the smallest zone count and one the largest, so the
    // claim is about the platform list rather than about one Region's zones.
    for region in ["eu-central-1", "us-east-1"] {
        let answer = emulator
            .call_in_region(
                region,
                "DescribeOrderableClusterOptions",
                &[("NodeType", "rg.4xlarge")],
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case platforms in {region}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let zones = listed_zones(&answer.body);
        assert!(
            !zones.is_empty(),
            "case platforms in {region}: expected the Region's zones, got {:?}",
            answer.body
        );
        for (zone, platforms) in &zones {
            assert_eq!(
                platforms,
                &vec![state::PLATFORM_VPC.to_string()],
                "case platforms in {region}: zone {zone:?} must offer the VPC platform alone, \
                 got {platforms:?}"
            );
            assert!(
                !platforms.contains(&state::PLATFORM_EC2_CLASSIC.to_string()),
                "case platforms in {region}: zone {zone:?} must offer no EC2-Classic platform, \
                 got {platforms:?}"
            );
        }
    }
}
