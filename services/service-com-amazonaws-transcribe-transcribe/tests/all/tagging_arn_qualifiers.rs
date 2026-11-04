// SPDX-License-Identifier: CC0-1.0

//! What an ARN's partition, Region and account segments mean to the three tagging
//! operations, end to end over the wire.
//!
//! The rule these tests pin: an AWS resource ARN is partition-, Region- and
//! account-qualified, so a resource is identified by those three segments TOGETHER with
//! its `<type>/<name>` pair. A tagging call resolves a caller-supplied ARN only when its
//! qualifiers are the ones the request is served in — this emulator's partition and
//! account, and the Region of the caller's SigV4 credential scope (the emulator's
//! default Region for an unsigned request) — and an ARN that changes any of them names a
//! resource this call cannot reach: it is refused, never answered with (or written
//! through to) the same-named local record.
//!
//! The rows differ along what the rule generalises over: WHICH qualifier the ARN changes
//! (partition, Region, account), the value it changes it to (several Regions, several
//! account ids, the other published partitions, and the empty Region segment the
//! `TranscribeArn` pattern admits), the Region the request itself is served in (unsigned
//! and two signed Regions), the resource FAMILY the ARN's `<type>` segment names (every
//! family this service stores records of, reused from `crate::tagging_resource_families`)
//! and which of the three tagging operations is called.
//!
//! Every call goes through the same `MultiServiceRouter` the production router dispatches
//! on, reusing the transcription-job suite's harness (`crate::transcription_jobs`), so
//! these tests exercise the awsJson1_1 envelope and the `TranscribeArn` boundary
//! constraint too.

use serde_json::{json, Value};

use service_com_amazonaws_transcribe_transcribe::service::medical_scribe_availability::ACCOUNT_NOT_AUTHORIZED;
use service_com_amazonaws_transcribe_transcribe::state::DEFAULT_REGION;

use crate::tagging_resource_families::{expected_pairs, families, pairs, tag_list, Family};
use crate::transcription_jobs::{Reply, Transcribe};

/// The modelled client error this service answers for an ARN that names no resource it
/// holds, and the status its awsJson1_1 wire protocol puts on every client error (both
/// pinned by `crate::client_error_wire_status`).
const ABSENT_CODE: &str = "BadRequestException";
/// See [`ABSENT_CODE`].
const CLIENT_ERROR_STATUS: u16 = 400;

/// The tag set every resource in this suite is created with, so a refusal that leaked
/// into the store (a write through a foreign ARN) shows up as a changed set.
const CREATED_TAGS: [(&str, &str); 2] = [("team", "speech"), ("env", "test")];

/// The credential scopes these tests serve requests in: no scope at all (served in
/// [`DEFAULT_REGION`]) and two signed Regions. They only have to be distinct, well-formed
/// Region labels — the rule is about the Region the request is served in, not about any
/// particular one.
const SERVED_IN: [Option<&str>; 3] = [None, Some("eu-west-1"), Some("ap-southeast-2")];

/// Which segment of a six-field ARN a row rewrites. The fields are
/// `arn:<partition>:<vendor>:<region>:<account>:<resource>`, so these are the three
/// qualifiers that stand before the resource.
const PARTITION: usize = 1;
const REGION: usize = 3;
const ACCOUNT: usize = 4;

/// Partition names other than the one this emulator's ARNs carry, each admitted by the
/// `TranscribeArn` pattern's own `aws(-[^:]+)?` partition group
/// (`smithy_query(query="operation", operation="ListTagsForResource")`) — so the boundary
/// validator passes them through to the handler, which is where the rule applies.
const OTHER_PARTITIONS: [&str; 2] = ["aws-cn", "aws-us-gov"];

/// Account ids other than the emulator's one, each twelve digits as the `TranscribeArn`
/// pattern's `[0-9]{12}` account group requires. The last is a one-digit near miss of the
/// emulator account, so a comparison that only looked at the length or the shape of the
/// segment would still fail this row.
const OTHER_ACCOUNTS: [&str; 3] = ["999999999999", "123456789012", "000000000001"];

/// Region labels a row may put in the `<region>` segment. The empty string is one: the
/// pattern's `[a-zA-Z0-9-]*` Region group admits it (that is how AWS writes a
/// partition-global resource), and it is not the Region any request is served in, so it
/// names no resource here either.
const REGION_LABELS: [&str; 5] = ["us-east-1", "us-west-2", "eu-central-1", "cn-north-1", ""];

/// One ARN a row asks about: how it differs from the one the service hands out, and the
/// ARN itself.
struct Foreign {
    case: String,
    arn: String,
}

/// `arn` with its `index`-th colon-separated field replaced by `value`.
///
/// The ARN is taken apart rather than formatted from scratch, so every other field stays
/// exactly what this service hands out and a row varies ONE qualifier.
fn with_field(arn: &str, index: usize, value: &str) -> String {
    let mut fields: Vec<String> = arn.split(':').map(str::to_string).collect();
    fields[index] = value.to_string();
    fields.join(":")
}

/// Every ARN that differs from `local` in exactly one qualifier, for a request served in
/// `served`: another partition, another Region (every label but the served one) or
/// another account.
fn foreign_arns(local: &str, served: &str) -> Vec<Foreign> {
    let mut rows: Vec<Foreign> = Vec::new();
    for partition in OTHER_PARTITIONS {
        rows.push(Foreign {
            case: format!("partition {partition:?}"),
            arn: with_field(local, PARTITION, partition),
        });
    }
    for region in REGION_LABELS.into_iter().filter(|region| *region != served) {
        rows.push(Foreign {
            case: format!("region {region:?}"),
            arn: with_field(local, REGION, region),
        });
    }
    for account in OTHER_ACCOUNTS {
        rows.push(Foreign {
            case: format!("account {account:?}"),
            arn: with_field(local, ACCOUNT, account),
        });
    }
    rows
}

/// The three tagging calls for `arn`: the read and the two writes, so a row asserts the
/// rule on every operation that resolves a `ResourceArn`.
fn tagging_calls(arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("ListTagsForResource", json!({"ResourceArn": arn})),
        (
            "TagResource",
            json!({"ResourceArn": arn, "Tags": tag_list(&[("leaked", "yes")])}),
        ),
        (
            "UntagResource",
            json!({"ResourceArn": arn, "TagKeys": ["team", "env"]}),
        ),
    ]
}

/// Create `name` of `family`, carrying [`CREATED_TAGS`], from a request served in
/// `served_in`.
async fn create(transcribe: &Transcribe, family: &Family, name: &str, served_in: Option<&str>, case: &str) {
    transcribe
        .call_in_region(
            family.create,
            family.create_request(name, &CREATED_TAGS),
            served_in,
        )
        .await
        .ok(case);
}

/// `ListTagsForResource` for `arn` from a request served in `served_in`, asserting it
/// answered and echoed the ARN asked about, and returning the tag set it reported.
async fn listed(
    transcribe: &Transcribe,
    arn: &str,
    served_in: Option<&str>,
    case: &str,
) -> Vec<(String, String)> {
    let reply = transcribe
        .call_in_region("ListTagsForResource", json!({"ResourceArn": arn}), served_in)
        .await;
    let body = reply.ok(case);
    assert_eq!(
        body.get("ResourceArn").and_then(Value::as_str),
        Some(arn),
        "case {case:?}: the answer must name the resource asked about, got {body:?}"
    );
    let tags = body
        .get("Tags")
        .unwrap_or_else(|| panic!("case {case:?}: expected a Tags member, got {body:?}"));
    pairs(tags, case)
}

/// One call served in `served_in`, as a [`Reply`] to assert on.
async fn call(
    transcribe: &Transcribe,
    operation: &str,
    request: Value,
    served_in: Option<&str>,
) -> Reply {
    transcribe.call_in_region(operation, request, served_in).await
}

/// RULE: an ARN differing from the one this service hands out in its partition, its
/// Region or its account segment names no resource a request served here can reach, so
/// all three tagging operations refuse it — and the local record it would otherwise have
/// reached keeps the tag set it was created with.
#[tokio::test]
async fn an_arn_of_another_partition_region_or_account_resolves_nothing() {
    for served_in in SERVED_IN {
        let transcribe = Transcribe::new();
        let served = served_in.unwrap_or(DEFAULT_REGION);

        for family in families() {
            // A family AWS does not offer in this Region answers every call of its own
            // with its authorization refusal, so no resource of it exists here for an
            // ARN to name — the create says so, and the sentence is
            // `crate::medical_scribe_region_availability`'s subject.
            if !family.is_served_in(served) {
                let case = format!("{} is not served in {served}", family.case);
                let reply = transcribe
                    .call_in_region(
                        family.create,
                        family.create_request(&family.resource_name("qualified"), &CREATED_TAGS),
                        served_in,
                    )
                    .await;
                reply.refused(&case, CLIENT_ERROR_STATUS, ABSENT_CODE);
                assert_eq!(
                    reply.error_message(&case),
                    ACCOUNT_NOT_AUTHORIZED,
                    "case {case:?}: expected the refusal of a family this Region does not \
                     serve, got {:?}",
                    reply.body
                );
                continue;
            }
            let name = family.resource_name("qualified");
            let scope = served_in.map_or_else(|| "unsigned".to_string(), str::to_string);
            let case = format!("{} / served in {served} ({scope})", family.case);
            create(&transcribe, &family, &name, served_in, &case).await;

            // The control: the ARN this service hands out for a request served here
            // resolves, so the rows below fail on the qualifier and not on the resource.
            let local = family.arn_in(served, &name);
            assert_eq!(
                listed(&transcribe, &local, served_in, &case).await,
                expected_pairs(&CREATED_TAGS),
                "case {case:?}: the ARN this service hands out must resolve"
            );

            for foreign in foreign_arns(&local, served) {
                for (operation, request) in tagging_calls(&foreign.arn) {
                    let case = format!("{case} / {operation} with {}", foreign.case);
                    call(&transcribe, operation, request, served_in)
                        .await
                        .refused(&case, CLIENT_ERROR_STATUS, ABSENT_CODE);
                }

                // Neither refused write reached the record the local ARN names: the set
                // is still the one the create stored, with no `leaked` key added and no
                // created key removed.
                let case = format!("{case} / after {} was refused", foreign.case);
                assert_eq!(
                    listed(&transcribe, &local, served_in, &case).await,
                    expected_pairs(&CREATED_TAGS),
                    "case {case:?}: a refused tagging call must leave the stored set alone"
                );
            }
        }
    }
}

/// RULE, the other side: the refusal is about the ARN's qualifiers, not about signing —
/// the ARN this service hands out to a request served in a given Region resolves through
/// all three tagging operations from a request served in that same Region, whichever
/// Region (or no credential scope at all) that is.
#[tokio::test]
async fn the_arn_this_service_hands_out_resolves_for_every_served_region() {
    for served_in in SERVED_IN {
        let transcribe = Transcribe::new();
        let served = served_in.unwrap_or(DEFAULT_REGION);

        for family in families() {
            // See the sibling test: a family this Region does not serve has no resource
            // here to hand out an ARN for, and its create says so.
            if !family.is_served_in(served) {
                let case = format!("{} is not served in {served}", family.case);
                let reply = transcribe
                    .call_in_region(
                        family.create,
                        family.create_request(&family.resource_name("served"), &CREATED_TAGS),
                        served_in,
                    )
                    .await;
                reply.refused(&case, CLIENT_ERROR_STATUS, ABSENT_CODE);
                assert_eq!(
                    reply.error_message(&case),
                    ACCOUNT_NOT_AUTHORIZED,
                    "case {case:?}: expected the refusal of a family this Region does not \
                     serve, got {:?}",
                    reply.body
                );
                continue;
            }
            let name = family.resource_name("served");
            let scope = served_in.map_or_else(|| "unsigned".to_string(), str::to_string);
            let case = format!("{} / served in {served} ({scope})", family.case);
            create(&transcribe, &family, &name, served_in, &case).await;
            let local = family.arn_in(served, &name);

            call(
                &transcribe,
                "TagResource",
                json!({"ResourceArn": local, "Tags": tag_list(&[("cost", "7")])}),
                served_in,
            )
            .await
            .ok(&case);
            assert_eq!(
                listed(&transcribe, &local, served_in, &case).await,
                expected_pairs(&[("team", "speech"), ("env", "test"), ("cost", "7")]),
                "case {case:?}: TagResource through the service's own ARN must merge"
            );

            call(
                &transcribe,
                "UntagResource",
                json!({"ResourceArn": local, "TagKeys": ["env", "cost"]}),
                served_in,
            )
            .await
            .ok_status(&case, 200);
            assert_eq!(
                listed(&transcribe, &local, served_in, &case).await,
                expected_pairs(&[("team", "speech")]),
                "case {case:?}: UntagResource through the service's own ARN must remove"
            );
        }
    }
}
