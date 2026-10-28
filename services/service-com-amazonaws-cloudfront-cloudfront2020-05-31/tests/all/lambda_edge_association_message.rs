// SPDX-License-Identifier: CC0-1.0

//! What the `InvalidLambdaFunctionAssociation` refusal SAYS: the fixed sentence stating the
//! version requirement, then " ARN: " and the `LambdaFunctionARN` of the refused
//! association, echoed exactly as the request wrote it.
//!
//! A `DistributionConfig` may carry Lambda@Edge associations on `DefaultCacheBehavior` and
//! on every entry of `CacheBehaviors`, so the sentence by itself does not say which entry
//! was refused: the ARN at the end of the message is the only thing in the response that
//! identifies it. The value is the request's — its partition, Region, account, function name
//! and qualifier all come from the submitted string — so these rows vary every one of those
//! parts, and also submit values that are no ARN at all, holding the message to the fixed
//! sentence plus the submitted ARN for each. One row submits a value carrying the
//! characters XML escapes, so the echoed value is held to the string the request MEANT and
//! the answer still parses as the document it claims to be.
//!
//! The second test asks WHICH entry a configuration carrying several of them names: the
//! first association that breaks the rule, read in the order the configuration states them —
//! the default cache behaviour, then the ordered ones.
//!
//! Every path that stores a configuration is exercised: `CreateDistribution`,
//! `CreateDistributionWithTags` and `UpdateDistribution`.

use crate::distribution_lifecycle::{
    create_distribution, emulator, get_distribution, update_distribution, Answer,
};
use crate::lambda_edge_association::{
    config_xml, config_xml_per_behavior, create_distribution_with_tags, refusal_message,
    reported_arns, Association, Behavior, REFUSAL_PREFIX,
};

/// A published version of a function, which every valid association in this suite names.
const VERSION_ONE: &str = "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:1";

/// An unqualified ARN whose function name is long, to show the echoed value is the string
/// the request submitted rather than anything this emulator shortens or rewrites.
const LONG_NAME_ARN: &str = concat!(
    "arn:aws:lambda:us-east-1:000000000000:function:",
    "abcdefghijabcdefghijabcdefghijabcdefghijabcdefghijabcdefghijabcd"
);

/// One association the refusal has to name, and the part of the ARN the row varies.
struct Row {
    case: &'static str,
    /// The `LambdaFunctionARN` text as the request body spells it, so a row can submit a
    /// value carrying characters XML has to escape.
    arn: &'static str,
    /// The value that text DECODES to — what the message has to name. The same as `arn`
    /// for every row whose ARN needs no escaping.
    decoded: &'static str,
    event_type: &'static str,
}

/// A row whose ARN travels as written, which is every row but the escaped one.
fn row(case: &'static str, arn: &'static str, event_type: &'static str) -> Row {
    Row {
        case,
        arn,
        decoded: arn,
        event_type,
    }
}

fn rows() -> Vec<Row> {
    vec![
        // The unqualified function ARN: the form the recorded refusal is filed about.
        row(
            "unqualified",
            "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter",
            "viewer-request",
        ),
        row(
            "latest-qualifier",
            "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:$LATEST",
            "origin-request",
        ),
        row(
            "alias-qualifier",
            "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:prod",
            "origin-response",
        ),
        // Another partition: the message carries the partition the request wrote.
        row(
            "other-partition",
            "arn:aws-cn:lambda:cn-north-1:111122223333:function:edge-greeter",
            "viewer-response",
        ),
        // Another Region, another account, another function-name form, and an alias.
        row(
            "other-region-and-account",
            "arn:aws:lambda:eu-west-3:210987654321:function:edge.greeter_2:staging",
            "viewer-request",
        ),
        row("long-function-name", LONG_NAME_ARN, "origin-request"),
        // A trailing colon with nothing after it names no version either, and the message
        // keeps that trailing colon.
        row(
            "empty-qualifier",
            "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:",
            "viewer-request",
        ),
        // One segment too many: still refused, still echoed whole.
        row(
            "extra-segment",
            "arn:aws:lambda:us-east-1:000000000000:function:edge-greeter:1:extra",
            "origin-response",
        ),
        // An ARN of another service.
        row(
            "not-a-lambda-arn",
            "arn:aws:states:us-east-1:000000000000:stateMachine:edge-greeter:1",
            "viewer-request",
        ),
        row("not-an-arn-at-all", "edge-greeter", "viewer-request"),
        // An ARN's prefix and service are lower case, so this names nothing — and the
        // message reports the spelling the request used, letter for letter.
        row(
            "upper-case-prefix",
            "ARN:AWS:LAMBDA:us-east-1:000000000000:function:edge-greeter:1",
            "origin-request",
        ),
        Row {
            // A value carrying the characters XML has to escape: the request escapes them,
            // and the message names the DECODED value — which the answer has to escape
            // again, or its body would not parse as the document it claims to be.
            case: "xml-escaped-characters",
            arn: "arn:aws:lambda:us-east-1:000000000000:function:a&amp;b&lt;c&gt;d",
            decoded: "arn:aws:lambda:us-east-1:000000000000:function:a&b<c>d",
            event_type: "viewer-request",
        },
    ]
}

/// The refusal's `Message`, or the empty string when the payload carries none.
fn refusal_text(answer: &Answer) -> String {
    answer
        .root()
        .find_text("Message")
        .unwrap_or_default()
        .to_string()
}

/// A refused write: HTTP 400 with the code the model declares for this rule.
fn assert_refused(answer: &Answer, case: &str, path: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case} ({path}): expected HTTP 400, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "InvalidLambdaFunctionAssociation",
        "case {case} ({path}): expected InvalidLambdaFunctionAssociation, got {:?} with {}",
        answer.error_code(),
        answer.raw
    );
}

#[tokio::test]
async fn the_refusal_names_the_submitted_lambda_function_arn() {
    let router = emulator();

    // A distribution whose association names a published version, so the UPDATE path has
    // something to refuse against.
    let created = create_distribution(
        &router,
        &config_xml(
            "ref-message-setup",
            Behavior::Default,
            &[Association {
                arn: VERSION_ONE,
                event_type: "viewer-request",
            }],
        ),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case setup: expected HTTP 201, got {} with {}",
        created.status, created.raw
    );
    let id = created.root().text_at("Id").unwrap_or_default().to_string();
    let etag = created.header("etag").to_string();

    for (index, row) in rows().into_iter().enumerate() {
        let expected = refusal_message(row.decoded);
        let submitted = [Association {
            arn: row.arn,
            event_type: row.event_type,
        }];
        for behavior in [Behavior::Default, Behavior::Ordered] {
            for (path, answer) in [
                (
                    "CreateDistribution",
                    create_distribution(
                        &router,
                        &config_xml(
                            &format!("ref-message-{index}-{behavior:?}"),
                            behavior,
                            &submitted,
                        ),
                    )
                    .await,
                ),
                (
                    "CreateDistributionWithTags",
                    create_distribution_with_tags(
                        &router,
                        &config_xml(
                            &format!("ref-message-tagged-{index}-{behavior:?}"),
                            behavior,
                            &submitted,
                        ),
                    )
                    .await,
                ),
                (
                    "UpdateDistribution",
                    update_distribution(
                        &router,
                        &id,
                        &config_xml("ref-message-setup", behavior, &submitted),
                        Some(&etag),
                    )
                    .await,
                ),
            ] {
                assert_refused(&answer, row.case, path);
                let message = refusal_text(&answer);
                assert_eq!(
                    message, expected,
                    "case {} ({path}, {behavior:?}): expected the refusal to state the \
                     version requirement and then the submitted ARN, got {message:?}",
                    row.case
                );
                assert!(
                    message.starts_with(REFUSAL_PREFIX),
                    "case {} ({path}, {behavior:?}): expected the fixed sentence first, \
                     got {message:?}",
                    row.case
                );
            }
        }
    }

    // The refused writes changed nothing: the distribution still names the version it was
    // created on.
    let read = get_distribution(&router, &id).await;
    assert_eq!(
        reported_arns(&read),
        vec![VERSION_ONE.to_string()],
        "case setup-read-back: a refused write must leave the stored association in \
         place, got {:?}",
        reported_arns(&read)
    );
}

/// Which association of a configuration carrying several the message names: the first one
/// that breaks the rule, in the order the configuration states them.
#[tokio::test]
async fn the_refusal_names_the_first_association_that_breaks_the_rule() {
    let router = emulator();

    const GOOD: Association = Association {
        arn: VERSION_ONE,
        event_type: "viewer-request",
    };
    const DEFAULT_BAD: Association = Association {
        arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-default:$LATEST",
        event_type: "origin-request",
    };
    const DEFAULT_BAD_SECOND: Association = Association {
        arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-default-two",
        event_type: "origin-response",
    };
    const ORDERED_BAD: Association = Association {
        arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-ordered:canary",
        event_type: "viewer-response",
    };

    /// One configuration layout: what each cache behaviour carries, the ARN the refusal has
    /// to name and the ARNs it must not name.
    struct Layout {
        case: &'static str,
        default: Option<&'static [Association]>,
        ordered: Option<&'static [Association]>,
        named: &'static str,
        not_named: &'static [&'static str],
    }

    let layouts = [
        Layout {
            case: "default-behaviour-carries-the-bad-entry",
            default: Some(&[GOOD, DEFAULT_BAD]),
            ordered: Some(&[GOOD]),
            named: DEFAULT_BAD.arn,
            not_named: &[GOOD.arn],
        },
        Layout {
            case: "ordered-behaviour-carries-the-bad-entry",
            default: Some(&[GOOD]),
            ordered: Some(&[GOOD, ORDERED_BAD]),
            named: ORDERED_BAD.arn,
            not_named: &[GOOD.arn],
        },
        Layout {
            case: "two-bad-entries-on-the-default-behaviour",
            default: Some(&[GOOD, DEFAULT_BAD, DEFAULT_BAD_SECOND]),
            ordered: None,
            named: DEFAULT_BAD.arn,
            not_named: &[DEFAULT_BAD_SECOND.arn, GOOD.arn],
        },
        Layout {
            case: "both-behaviours-carry-a-bad-entry",
            default: Some(&[DEFAULT_BAD]),
            ordered: Some(&[ORDERED_BAD]),
            named: DEFAULT_BAD.arn,
            not_named: &[ORDERED_BAD.arn],
        },
        Layout {
            case: "only-the-ordered-behaviour-states-the-member",
            default: None,
            ordered: Some(&[ORDERED_BAD]),
            named: ORDERED_BAD.arn,
            not_named: &[DEFAULT_BAD.arn],
        },
    ];

    for (index, layout) in layouts.iter().enumerate() {
        let answer = create_distribution(
            &router,
            &config_xml_per_behavior(
                &format!("ref-which-{index}"),
                layout.default,
                layout.ordered,
            ),
        )
        .await;
        assert_refused(&answer, layout.case, "CreateDistribution");
        let message = refusal_text(&answer);
        let expected = refusal_message(layout.named);
        assert_eq!(
            message, expected,
            "case {}: expected the refusal to name the first offending association, got \
             {message:?}",
            layout.case
        );
        for other in layout.not_named {
            assert!(
                !message.contains(other),
                "case {}: the refusal must not name {other:?}, got {message:?}",
                layout.case
            );
        }
    }
}
