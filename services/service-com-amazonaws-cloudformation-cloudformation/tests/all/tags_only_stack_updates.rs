// SPDX-License-Identifier: CC0-1.0

//! An update that changes only the stack's own CONFIGURATION — its `Tags` — end to end over
//! the awsQuery wire.
//!
//! RULE: a stack's tags are part of the configuration an update writes, so an update that
//! restates the template unchanged and submits different tags IS a change: it is carried
//! out, the stack settles `UPDATE_COMPLETE` and reports the tags it was given, while an
//! update that restates the stack exactly as it already is changes nothing and is refused
//! with "No updates are to be performed." A member the request omits is left as it is ("if
//! you don't specify this parameter, CloudFormation doesn't modify the stack's tags"), so an
//! absent `Tags` is not an empty one.
//!
//! The rows differ along what the rule generalises over: whether the tag set is added to a
//! stack that had none, has one of its VALUES changed, or is extended by a second key;
//! whether the template SPELLS its resource's physical name or leaves CloudFormation to
//! generate one (which is what decides whether the update re-computes the resource's
//! identity); and whether the update restates the template or asks for the one the stack
//! already carries (`UsePreviousTemplate`), which is how a client that is only re-tagging
//! calls it.

use crate::change_set_lifecycle::settle_the_store;
use crate::harness::{call, create_stack, member, router_with_engine};
use crate::stack_lifecycle::{describe_stack, xml_blocks, xml_text, xml_texts};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// One row of the tags table: how the bucket is named, the stack's tags before the update,
/// and the tags the update submits.
struct Tagged {
    case: &'static str,
    stack: &'static str,
    /// Whether the template SPELLS the bucket's name or leaves CloudFormation to generate
    /// one, which is what decides whether the resource's identity is computed again.
    named: bool,
    /// The stack-level tags the create carries.
    created_with: &'static [(&'static str, &'static str)],
    /// The stack-level tags the update submits, which is all it changes.
    updated_with: &'static [(&'static str, &'static str)],
    /// Whether the update restates the template or asks for the one the stack already
    /// carries (`UsePreviousTemplate`).
    reuses_template: bool,
}

fn tagged_rows() -> Vec<Tagged> {
    vec![
        Tagged {
            case: "a tag added to a stack that had none, bucket named by the template",
            stack: "tagged-bucket-added",
            named: true,
            created_with: &[],
            updated_with: &[("owner", "platform")],
            reuses_template: false,
        },
        Tagged {
            case: "a tag added to a stack whose bucket name CloudFormation generated",
            stack: "tagged-bucket-generated",
            named: false,
            created_with: &[],
            updated_with: &[("owner", "platform")],
            reuses_template: false,
        },
        Tagged {
            case: "a tag VALUE changed on a stack that already carried it",
            stack: "tagged-bucket-changed",
            named: true,
            created_with: &[("owner", "platform")],
            updated_with: &[("owner", "data")],
            reuses_template: false,
        },
        Tagged {
            case: "a second tag added beside the one the stack carried",
            stack: "tagged-bucket-extended",
            named: false,
            created_with: &[("owner", "platform")],
            updated_with: &[("owner", "platform"), ("tier", "gold")],
            reuses_template: false,
        },
        Tagged {
            case: "a tag added by an update that reuses the stack's own template",
            stack: "tagged-bucket-previous",
            named: true,
            created_with: &[],
            updated_with: &[("owner", "platform")],
            reuses_template: true,
        },
    ]
}

/// The template one row deploys: a single bucket, named by the template or left to
/// CloudFormation's own naming.
fn bucket_template(stack: &str, named: bool) -> String {
    let properties = if named {
        format!(r#"{{"BucketName":"{stack}-data"}}"#)
    } else {
        "{}".to_string()
    };
    format!(r#"{{"Resources":{{"Data":{{"Type":"AWS::S3::Bucket","Properties":{properties}}}}}}}"#)
}

/// The request members one row's tags travel on.
fn tag_members(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    tags.iter()
        .enumerate()
        .flat_map(|(index, (key, value))| {
            let position = index + 1;
            [
                member(&format!("Tags.member.{position}.Key"), key),
                member(&format!("Tags.member.{position}.Value"), value),
            ]
        })
        .collect()
}

#[tokio::test]
async fn a_tags_only_update_applies_the_tags_and_completes() {
    let (router, engine) = router_with_engine();

    for row in tagged_rows() {
        let case = row.case;
        let template = bucket_template(row.stack, row.named);
        let (code, body) = create_stack(
            &router,
            row.stack,
            &template,
            &tag_members(row.created_with),
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the stack must be created, got {code} {body}"
        );
        settle_the_store(&router, &engine).await;

        let mut updated = vec![member("StackName", row.stack)];
        if row.reuses_template {
            updated.push(member("UsePreviousTemplate", "true"));
        } else {
            updated.push(member("TemplateBody", &template));
        }
        updated.extend(tag_members(row.updated_with));
        let (code, body) = call(&router, "UpdateStack", &updated).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the tagged update must be accepted, got {code} {body}"
        );
        settle_the_store(&router, &engine).await;
        let (code, body) = describe_stack(&router, row.stack).await;
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some(status::UPDATE_COMPLETE),
            "case {case:?}: expected UPDATE_COMPLETE, got {code} {body}"
        );
        let reported: Vec<String> = match xml_blocks(&body, "Tags").first() {
            Some(block) => xml_texts(block, "Key"),
            None => Vec::new(),
        };
        let expected: Vec<String> = row
            .updated_with
            .iter()
            .map(|(key, _)| (*key).to_string())
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: the stack must report the applied tags, got {body}"
        );

        // ...while restating the stack exactly as it now is changes nothing.
        let (code, body) = call(&router, "UpdateStack", &updated).await;
        assert_eq!(
            code, 400,
            "case {case:?}: the no-op update must be refused, got {code} {body}"
        );
        assert_eq!(
            xml_text(&body, "Message").as_deref(),
            Some("No updates are to be performed."),
            "case {case:?}: expected CloudFormation's own refusal, got {body}"
        );
    }
}
